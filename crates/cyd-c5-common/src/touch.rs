//! Драйвер резистивного тачскріна XPT2046 (CS=IO1, 2.5 MHz, медіана з 3 вимірів, афінне калібрування).

use embedded_graphics::prelude::Point;
use embedded_hal::spi::SpiBus;
use esp_hal::gpio::Output;

use crate::{
    fb::{SCREEN_H, SCREEN_W},
    spi::SharedSpiBus,
};

pub const TOUCH_SPI_HZ: u32 = 2_500_000;
const TOUCH_Z_THRESHOLD: u16 = 100;

// Калібрувальні коефіцієнти для плати NM-CYD-C5 при орієнтації ST7789 Deg270 (MADCTL 0xA0).
const K_X: [f32; 3] = [-0.000410, -0.088183, 334.715];
const K_Y: [f32; 3] = [-0.065849, -0.000119, 260.255];

pub struct Touch {
    bus: &'static SharedSpiBus,
    cs: Output<'static>,
}

impl Touch {
    pub fn new(bus: &'static SharedSpiBus, mut cs: Output<'static>) -> Self {
        cs.set_high();
        Self { bus, cs }
    }

    fn read_raw(&mut self, cmd: u8) -> u16 {
        let mut buf = [0u8; 2];
        self.bus.with_hz(TOUCH_SPI_HZ, |spi| {
            self.cs.set_low();
            let _ = SpiBus::write(spi, &[cmd]);
            let _ = SpiBus::transfer_in_place(spi, &mut buf);
            let _ = SpiBus::flush(spi);
            self.cs.set_high();
        });
        (((buf[0] as u16) << 8) | (buf[1] as u16)) >> 3
    }

    fn read_median3(&mut self, cmd: u8) -> u16 {
        let mut v = [self.read_raw(cmd), self.read_raw(cmd), self.read_raw(cmd)];
        v.sort_unstable();
        v[1]
    }

    #[inline]
    pub fn pressed(&mut self) -> bool {
        self.read_raw(0xB0) > TOUCH_Z_THRESHOLD
    }

    pub fn point(&mut self) -> Point {
        let rx = self.read_median3(0xD0) as f32;
        let ry = self.read_median3(0x90) as f32;
        let sx = (K_X[0] * rx + K_X[1] * ry + K_X[2] + 0.5) as i32;
        let sy = (K_Y[0] * rx + K_Y[1] * ry + K_Y[2] + 0.5) as i32;
        Point::new(sx.clamp(0, SCREEN_W - 1), sy.clamp(0, SCREEN_H - 1))
    }

    #[inline]
    pub fn sample(&mut self) -> Option<Point> {
        if self.pressed() {
            Some(self.point())
        } else {
            None
        }
    }
}
