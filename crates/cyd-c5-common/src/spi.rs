//! Спільна шина SPI2+DMA з автоматичним перемиканням частоти між ST7789 (40 MHz),
//! XPT2046 (2.5 MHz) та SD-карткою (400 kHz → 16 MHz).

use core::cell::{Cell, RefCell};
use embedded_hal::{
    delay::DelayNs,
    spi::{ErrorKind, ErrorType, Operation, SpiBus, SpiDevice},
};
use esp_hal::{
    Blocking,
    delay::Delay,
    gpio::Output,
    spi::master::{Config as SpiConfig, SpiDma},
    time::Rate,
};

pub struct SharedSpiBus {
    pub spi: RefCell<SpiDma<'static, Blocking>>,
    pub last_hz: Cell<u32>,
}

impl SharedSpiBus {
    pub fn new(spi: SpiDma<'static, Blocking>, initial_hz: u32) -> Self {
        Self {
            spi: RefCell::new(spi),
            last_hz: Cell::new(initial_hz),
        }
    }

    pub fn with_hz<R>(&self, hz: u32, f: impl FnOnce(&mut SpiDma<'static, Blocking>) -> R) -> R {
        let mut spi = self.spi.borrow_mut();
        if self.last_hz.get() != hz {
            let cfg = SpiConfig::default().with_frequency(Rate::from_hz(hz));
            let _ = spi.apply_config(&cfg);
            self.last_hz.set(hz);
        }
        f(&mut spi)
    }

    /// Подає холості такти SCK із MOSI=0xFF, коли всі CS підняті в HIGH (потрібно перед ініціалізацією SD).
    pub fn send_dummy_clocks(&self, hz: u32, byte_count: usize) {
        let ff = [0xFFu8; 16];
        self.with_hz(hz, |spi| {
            let mut rem = byte_count;
            while rem > 0 {
                let n = rem.min(ff.len());
                let _ = SpiBus::write(spi, &ff[..n]);
                rem -= n;
            }
            let _ = SpiBus::flush(spi);
        });
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SharedSpiError;

impl embedded_hal::spi::Error for SharedSpiError {
    fn kind(&self) -> ErrorKind {
        ErrorKind::Other
    }
}

pub struct SharedSpiDevice<'a> {
    bus: &'a SharedSpiBus,
    cs: Output<'static>,
    hz: u32,
    delay: Delay,
}

impl<'a> SharedSpiDevice<'a> {
    pub fn new(bus: &'a SharedSpiBus, mut cs: Output<'static>, hz: u32) -> Self {
        cs.set_high();
        Self {
            bus,
            cs,
            hz,
            delay: Delay::new(),
        }
    }

    #[inline]
    pub fn set_frequency(&mut self, hz: u32) {
        self.hz = hz;
    }

    #[inline]
    pub fn frequency(&self) -> u32 {
        self.hz
    }

    #[inline]
    pub fn bus(&self) -> &'a SharedSpiBus {
        self.bus
    }
}

impl ErrorType for SharedSpiDevice<'_> {
    type Error = SharedSpiError;
}

impl SpiDevice<u8> for SharedSpiDevice<'_> {
    fn transaction(&mut self, operations: &mut [Operation<'_, u8>]) -> Result<(), Self::Error> {
        let hz = self.hz;
        self.bus.with_hz(hz, |spi| {
            self.cs.set_low();
            let mut res = Ok(());
            for op in operations {
                let r = match op {
                    Operation::Read(buf) => {
                        // Тримаємо MOSI=0xFF під час читання (критично для SD у режимі SPI).
                        buf.fill(0xFF);
                        SpiBus::transfer_in_place(spi, buf)
                    }
                    Operation::Write(buf) => SpiBus::write(spi, buf),
                    Operation::Transfer(rd, wr) => SpiBus::transfer(spi, rd, wr),
                    Operation::TransferInPlace(buf) => SpiBus::transfer_in_place(spi, buf),
                    Operation::DelayNs(ns) => {
                        self.delay.delay_ns(*ns);
                        Ok(())
                    }
                };
                if r.is_err() {
                    res = Err(SharedSpiError);
                    break;
                }
            }
            let _ = SpiBus::flush(spi);
            self.cs.set_high();
            res
        })
    }
}
