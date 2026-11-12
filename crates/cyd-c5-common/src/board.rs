//! Ініціалізація всієї периферії плати NM-CYD-C5 в один виклик `Board::init()`.

use core::{
    mem::MaybeUninit,
    sync::atomic::{AtomicBool, Ordering},
};
use esp_hal::{
    Blocking,
    clock::CpuClock,
    delay::Delay,
    gpio::{Level, Output, OutputConfig},
    rng::Rng,
    spi::master::{Config as SpiConfig, Spi},
    time::Rate,
    uart::{Config as UartConfig, Uart},
};
use esp_storage::FlashStorage;
use mipidsi::{
    Builder, Display as MipiDisplay, NoResetPin,
    interface::SpiInterface,
    models::ST7789,
    options::{ColorOrder, Orientation, Rotation},
};

use crate::{
    boot,
    fb::{SCREEN_H, SCREEN_W, fb_ref},
    sd::SdSlot,
    spi::{SharedSpiBus, SharedSpiDevice},
    touch::Touch,
};

pub const LCD_SPI_HZ: u32 = 40_000_000;

static BUS_TAKEN: AtomicBool = AtomicBool::new(false);
static mut BUS_CELL: MaybeUninit<SharedSpiBus> = MaybeUninit::uninit();
static mut LCD_BUF: [u8; 16 * 1024] = [0u8; 16 * 1024];

pub type Display =
    MipiDisplay<SpiInterface<'static, SharedSpiDevice<'static>, Output<'static>>, ST7789, NoResetPin>;

pub fn present_rect_on(display: &mut Display, x: u16, y: u16, w: u16, h: u16) {
    if w == 0 || h == 0 {
        return;
    }
    let x1 = (x + w - 1).min((SCREEN_W - 1) as u16);
    let y1 = (y + h - 1).min((SCREEN_H - 1) as u16);
    let fb = fb_ref();
    let sw = SCREEN_W as usize;
    let it = (y as usize..=y1 as usize)
        .flat_map(|row| fb[row * sw + (x as usize)..=row * sw + (x1 as usize)].iter().copied());
    let _ = display.set_pixels(x, y, x1, y1, it);
}

pub struct Board {
    pub bus: &'static SharedSpiBus,
    pub display: Display,
    pub touch: Touch,
    pub sd: SdSlot,
    pub flash: FlashStorage<'static>,
    pub rng: Rng,
    pub delay: Delay,
    uart: Uart<'static, Blocking>,
    _backlight: Output<'static>,
}

impl Board {
    /// Одноразова ініціалізація CPU (240 MHz), спільної шини SPI2+DMA, ST7789, XPT2046, SD-слота та Flash.
    pub fn init() -> Self {
        assert!(
            !BUS_TAKEN.swap(true, Ordering::SeqCst),
            "Board::init() called twice"
        );

        let p = esp_hal::init(esp_hal::Config::default().with_cpu_clock(CpuClock::max()));
        let mut delay = Delay::new();

        // ВАЖЛИВО: піднімаємо ВСІ ТРИ CS (дисплей IO23, тач IO1, SD IO10) у HIGH ДО запуску SPI2,
        // щоб вставлена SD-картка не створювала конфлікту на лінії MISO!
        let cs_lcd = Output::new(p.GPIO23, Level::High, OutputConfig::default());
        let cs_touch = Output::new(p.GPIO1, Level::High, OutputConfig::default());
        let cs_sd = Output::new(p.GPIO10, Level::High, OutputConfig::default());
        let dc_lcd = Output::new(p.GPIO24, Level::Low, OutputConfig::default());
        let backlight = Output::new(p.GPIO25, Level::High, OutputConfig::default());

        let spi = Spi::new(
            p.SPI2,
            SpiConfig::default().with_frequency(Rate::from_hz(LCD_SPI_HZ)),
        )
        .unwrap()
        .with_sck(p.GPIO6)
        .with_mosi(p.GPIO7)
        .with_miso(p.GPIO2)
        .with_dma(p.DMA_CH0);

        let bus: &'static SharedSpiBus = unsafe {
            let ptr = core::ptr::addr_of_mut!(BUS_CELL);
            (*ptr).write(SharedSpiBus::new(spi, LCD_SPI_HZ));
            (*ptr).assume_init_ref()
        };

        let lcd_dev = SharedSpiDevice::new(bus, cs_lcd, LCD_SPI_HZ);
        let lcd_buf = unsafe { &mut *core::ptr::addr_of_mut!(LCD_BUF) };
        let di = SpiInterface::new(lcd_dev, dc_lcd, lcd_buf);
        let display = Builder::new(ST7789, di)
            .display_size(240, 320)
            .orientation(Orientation::new().rotate(Rotation::Deg270))
            .color_order(ColorOrder::Rgb)
            .init(&mut delay)
            .unwrap();

        let touch = Touch::new(bus, cs_touch);
        let sd = SdSlot::new(bus, cs_sd);
        let flash = FlashStorage::new(p.FLASH);
        let rng = Rng::new();
        let uart = Uart::new(p.UART0, UartConfig::default().with_baudrate(115_200))
            .unwrap()
            .with_tx(p.GPIO11)
            .with_rx(p.GPIO12);

        Self {
            bus,
            display,
            touch,
            sd,
            flash,
            rng,
            delay,
            uart,
            _backlight: backlight,
        }
    }

    /// Неблокуюче читання одного байта з консолі UART0 (`GPIO12`), якщо він надійшов.
    /// Спеціальний символ `'?'` у будь-якій грі чи лаунчері миттєво віддає знімок екрана (`SNAPBEGIN`..`SNAPEND`).
    pub fn poll_uart_byte(&mut self) -> Option<u8> {
        if self.uart.read_ready() {
            let mut b = [0u8; 1];
            if let Ok(1) = self.uart.read_buffered(&mut b) {
                if b[0] == b'?' {
                    crate::fb::dump_frame_hex();
                    return None;
                }
                return Some(b[0]);
            }
        }
        None
    }

    /// Відправляє прямокутник `(x, y, w, h)` із фреймбуфера `FB` на дисплей ST7789 по DMA.
    pub fn present_rect(&mut self, x: u16, y: u16, w: u16, h: u16) {
        present_rect_on(&mut self.display, x, y, w, h);
    }

    #[inline]
    pub fn present_all(&mut self) {
        self.present_rect(0, 0, SCREEN_W as u16, SCREEN_H as u16);
    }

    #[inline]
    pub fn start_in_demo(&mut self, feature_demo: bool) -> bool {
        boot::should_start_demo(&mut self.flash, feature_demo)
    }

    #[inline]
    pub fn load_best(&mut self, save_name: &str) -> u32 {
        self.sd.load_best(save_name).unwrap_or(0)
    }

    #[inline]
    pub fn save_best(&mut self, save_name: &str, best: u32) {
        let _ = self.sd.save_best(save_name, best);
    }

    /// Повернення з будь-якої гри назад у GameLauncher (`factory`).
    pub fn exit_to_launcher(&mut self) -> ! {
        boot::exit_to_launcher(&mut self.flash)
    }
}
