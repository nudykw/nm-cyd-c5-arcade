//! Повноекранний RGB565-фреймбуфер 320×240 + `DrawTarget`.
//!
//! Типово жив у внутрішній SRAM (150 KB). Лаунчер може перевести його в PSRAM
//! (`Board::init_psram_fb()`), щоб звільнити внутрішню RAM під купу/TLS-стек:
//! тоді `fb_ref/fb_mut` працюють через вказівник, а ігри без PSRAM нічого не втрачають.

use core::convert::Infallible;
use embedded_graphics::{
    Pixel,
    pixelcolor::{Rgb565, raw::ToBytes},
    prelude::*,
    primitives::Rectangle,
};

pub const SCREEN_W: i32 = 320;
pub const SCREEN_H: i32 = 240;
pub const FB_PIXELS: usize = (SCREEN_W as usize) * (SCREEN_H as usize);

/// Фреймбуфер у внутрішній SRAM (типово для ігор: швидко, 150 KB DRAM).
#[cfg(not(feature = "psram-fb"))]
static mut FB: [Rgb565; FB_PIXELS] = [Rgb565::BLACK; FB_PIXELS];

/// З фічею `psram-fb` (лаунчер) фреймбуфер живе в PSRAM: внутрішня SRAM звільняється
/// під купу і стек (TLS під час оновлення по Wi-Fi).
#[cfg(feature = "psram-fb")]
static mut FB_PTR: *mut Rgb565 = core::ptr::null_mut();

/// Перемикає фреймбуфер на зовнішню пам'ять (викликає `Board::init_psram_fb`).
///
/// # Safety
/// `ptr` мусить указувати на `FB_PIXELS` пікселів, вирівняних на 2 байти, і жити вічно.
#[cfg(feature = "psram-fb")]
pub unsafe fn set_external_fb(ptr: *mut Rgb565) {
    unsafe { FB_PTR = ptr };
}

#[inline]
pub fn fb_mut() -> &'static mut [Rgb565; FB_PIXELS] {
    #[cfg(feature = "psram-fb")]
    unsafe {
        let p = FB_PTR;
        assert!(!p.is_null(), "PSRAM-фреймбуфер не ініціалізовано (Board::init)");
        &mut *(p as *mut [Rgb565; FB_PIXELS])
    }
    #[cfg(not(feature = "psram-fb"))]
    unsafe {
        &mut *core::ptr::addr_of_mut!(FB)
    }
}

#[inline]
pub fn fb_ref() -> &'static [Rgb565; FB_PIXELS] {
    #[cfg(feature = "psram-fb")]
    unsafe {
        let p = FB_PTR;
        assert!(!p.is_null(), "PSRAM-фреймбуфер не ініціалізовано (Board::init)");
        &*(p as *const [Rgb565; FB_PIXELS])
    }
    #[cfg(not(feature = "psram-fb"))]
    unsafe {
        &*core::ptr::addr_of!(FB)
    }
}

#[inline]
pub fn put_px(x: i32, y: i32, c: Rgb565) {
    if (0..SCREEN_W).contains(&x) && (0..SCREEN_H).contains(&y) {
        fb_mut()[(y as usize) * (SCREEN_W as usize) + (x as usize)] = c;
    }
}

#[inline]
pub fn get_px(x: i32, y: i32) -> Rgb565 {
    if (0..SCREEN_W).contains(&x) && (0..SCREEN_H).contains(&y) {
        fb_ref()[(y as usize) * (SCREEN_W as usize) + (x as usize)]
    } else {
        Rgb565::BLACK
    }
}

/// Обгортка `DrawTarget` навколо статичного фреймбуфера `FB`.
pub struct Fb;

impl OriginDimensions for Fb {
    fn size(&self) -> Size {
        Size::new(SCREEN_W as u32, SCREEN_H as u32)
    }
}

impl DrawTarget for Fb {
    type Color = Rgb565;
    type Error = Infallible;

    fn draw_iter<I>(&mut self, pixels: I) -> Result<(), Self::Error>
    where
        I: IntoIterator<Item = Pixel<Self::Color>>,
    {
        let fb = fb_mut();
        for Pixel(p, c) in pixels {
            if p.x >= 0 && p.x < SCREEN_W && p.y >= 0 && p.y < SCREEN_H {
                fb[(p.y as usize) * (SCREEN_W as usize) + (p.x as usize)] = c;
            }
        }
        Ok(())
    }

    fn fill_solid(&mut self, area: &Rectangle, color: Self::Color) -> Result<(), Self::Error> {
        let Some(clipped) = area.intersection(&self.bounding_box()).bottom_right() else {
            return Ok(());
        };
        let tl = area.intersection(&self.bounding_box()).top_left;
        let fb = fb_mut();
        for y in tl.y..=clipped.y {
            let row = (y as usize) * (SCREEN_W as usize);
            fb[row + (tl.x as usize)..=row + (clipped.x as usize)].fill(color);
        }
        Ok(())
    }

    fn fill_contiguous<I>(&mut self, area: &Rectangle, colors: I) -> Result<(), Self::Error>
    where
        I: IntoIterator<Item = Self::Color>,
    {
        let bb = self.bounding_box();
        if area.intersection(&bb) == *area {
            let Some(br) = area.bottom_right() else {
                return Ok(());
            };
            let fb = fb_mut();
            let mut it = colors.into_iter();
            for y in area.top_left.y..=br.y {
                let row = (y as usize) * (SCREEN_W as usize);
                for x in area.top_left.x..=br.x {
                    let Some(c) = it.next() else { return Ok(()) };
                    fb[row + (x as usize)] = c;
                }
            }
            Ok(())
        } else {
            self.draw_iter(
                area.points()
                    .zip(colors)
                    .filter(|(p, _)| bb.contains(*p))
                    .map(|(p, c)| Pixel(p, c)),
            )
        }
    }
}

/// Hex-дамп повного кадру в UART (`SNAPBEGIN 320 240` ... `SNAPEND`) для діагностики.
pub fn dump_frame_hex() {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    esp_println::println!("SNAPBEGIN {} {}", SCREEN_W, SCREEN_H);
    let fb = fb_ref();
    let mut line = [0u8; (SCREEN_W as usize) * 4];
    for y in 0..(SCREEN_H as usize) {
        let row = y * (SCREEN_W as usize);
        for x in 0..(SCREEN_W as usize) {
            let [hi, lo] = fb[row + x].to_be_bytes();
            let o = x * 4;
            line[o] = HEX[(hi >> 4) as usize];
            line[o + 1] = HEX[(hi & 0x0f) as usize];
            line[o + 2] = HEX[(lo >> 4) as usize];
            line[o + 3] = HEX[(lo & 0x0f) as usize];
        }
        let s = unsafe { core::str::from_utf8_unchecked(&line) };
        esp_println::println!("{}", s);
    }
    esp_println::println!("SNAPEND");
}
