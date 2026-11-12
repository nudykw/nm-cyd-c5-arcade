//! Спільні UI-примітиви, шрифти, кнопки (включно з `DEMO` і `EXIT`) та форматування без алокацій.

use core::fmt::Write;
use embedded_graphics::{
    mono_font::{
        MonoFont, MonoTextStyle,
        ascii::{FONT_6X10, FONT_9X18_BOLD, FONT_10X20},
    },
    pixelcolor::Rgb565,
    prelude::*,
    primitives::{CornerRadii, PrimitiveStyle, PrimitiveStyleBuilder, Rectangle, RoundedRectangle},
    text::{Baseline, Text},
};
use esp_hal::time::Instant;

use crate::fb::{Fb, SCREEN_H, SCREEN_W, fb_mut};

#[inline]
pub const fn rgb(r: u8, g: u8, b: u8) -> Rgb565 {
    Rgb565::new(r >> 3, g >> 2, b >> 3)
}

// Спільна палітра стилю NM-CYD-C5 Arcade
pub const BG_DEEP: Rgb565 = rgb(14, 18, 28);
pub const PANEL_BG: Rgb565 = rgb(24, 30, 44);
pub const CARD_BG: Rgb565 = rgb(32, 40, 58);
pub const CARD_SEL_BG: Rgb565 = rgb(44, 58, 86);
pub const TEXT_WHITE: Rgb565 = rgb(244, 247, 252);
pub const TEXT_MUTED: Rgb565 = rgb(150, 164, 186);
pub const ACCENT_GOLD: Rgb565 = rgb(250, 200, 64);
pub const ACCENT_CYAN: Rgb565 = rgb(72, 212, 242);
pub const ACCENT_GREEN: Rgb565 = rgb(74, 214, 120);
pub const ACCENT_RED: Rgb565 = rgb(232, 82, 82);

pub const BTN_STD_BG: Rgb565 = rgb(46, 58, 82);
pub const BTN_STD_BORDER: Rgb565 = rgb(88, 108, 146);
pub const BTN_DEMO_OFF_BG: Rgb565 = rgb(44, 62, 76);
pub const BTN_DEMO_ON_BG: Rgb565 = rgb(36, 134, 86);
pub const BTN_DEMO_ON_BORDER: Rgb565 = rgb(118, 244, 166);
pub const BTN_EXIT_BG: Rgb565 = rgb(116, 40, 48);
pub const BTN_EXIT_BORDER: Rgb565 = rgb(224, 96, 106);

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum FontKind {
    Small, // 6x10
    Bold,  // 9x18 bold
    Large, // 10x20
}

impl FontKind {
    #[inline]
    pub fn font(self) -> &'static MonoFont<'static> {
        match self {
            Self::Small => &FONT_6X10,
            Self::Bold => &FONT_9X18_BOLD,
            Self::Large => &FONT_10X20,
        }
    }

    #[inline]
    pub fn char_w(self) -> i32 {
        match self {
            Self::Small => 6,
            Self::Bold => 9,
            Self::Large => 10,
        }
    }

    #[inline]
    pub fn char_h(self) -> i32 {
        match self {
            Self::Small => 10,
            Self::Bold => 18,
            Self::Large => 20,
        }
    }
}

#[inline]
pub fn fill(x: i32, y: i32, w: u32, h: u32, color: Rgb565) {
    let _ = Rectangle::new(Point::new(x, y), Size::new(w, h))
        .into_styled(PrimitiveStyle::with_fill(color))
        .draw(&mut Fb);
}

#[inline]
pub fn fill_rounded(x: i32, y: i32, w: u32, h: u32, r: u32, color: Rgb565) {
    let _ = RoundedRectangle::new(
        Rectangle::new(Point::new(x, y), Size::new(w, h)),
        CornerRadii::new(Size::new(r, r)),
    )
    .into_styled(PrimitiveStyle::with_fill(color))
    .draw(&mut Fb);
}

#[inline]
pub fn stroke_rect(x: i32, y: i32, w: u32, h: u32, color: Rgb565) {
    let style = PrimitiveStyleBuilder::new()
        .stroke_color(color)
        .stroke_width(1)
        .build();
    let _ = Rectangle::new(Point::new(x, y), Size::new(w, h))
        .into_styled(style)
        .draw(&mut Fb);
}

#[inline]
pub fn blend_rgb565(dst: Rgb565, src: Rgb565, alpha: u8) -> Rgb565 {
    let a = alpha as u16;
    let inv = 255 - a;
    let r = ((dst.r() as u16) * inv + (src.r() as u16) * a) / 255;
    let g = ((dst.g() as u16) * inv + (src.g() as u16) * a) / 255;
    let b = ((dst.b() as u16) * inv + (src.b() as u16) * a) / 255;
    Rgb565::new(r as u8, g as u8, b as u8)
}

pub fn blend_fill(x: i32, y: i32, w: u32, h: u32, src: Rgb565, alpha: u8) {
    let x0 = x.max(0);
    let y0 = y.max(0);
    let x1 = (x + w as i32).min(SCREEN_W);
    let y1 = (y + h as i32).min(SCREEN_H);
    if x1 <= x0 || y1 <= y0 {
        return;
    }
    let fb = fb_mut();
    for py in y0..y1 {
        let row = (py as usize) * (SCREEN_W as usize);
        for px in x0..x1 {
            let idx = row + (px as usize);
            fb[idx] = blend_rgb565(fb[idx], src, alpha);
        }
    }
}

#[inline]
pub fn text(x: i32, y: i32, s: &str, kind: FontKind, color: Rgb565) {
    let style = MonoTextStyle::new(kind.font(), color);
    let _ = Text::with_baseline(s, Point::new(x, y), style, Baseline::Top).draw(&mut Fb);
}

#[inline]
pub fn text_centered(cx: i32, y: i32, s: &str, kind: FontKind, color: Rgb565) {
    let w = (s.len() as i32) * kind.char_w();
    text(cx - w / 2, y, s, kind, color);
}

#[derive(Clone, Copy)]
pub struct BtnRect {
    pub x: i32,
    pub y: i32,
    pub w: u32,
    pub h: u32,
}

impl BtnRect {
    pub const fn new(x: i32, y: i32, w: u32, h: u32) -> Self {
        Self { x, y, w, h }
    }

    #[inline]
    pub fn contains(&self, p: Point) -> bool {
        in_box(p, self.x, self.y, self.w as i32, self.h as i32)
    }
}

pub fn draw_btn(r: BtnRect, label: &str, kind: FontKind, bg: Rgb565, fg: Rgb565, border: Rgb565) {
    fill(r.x, r.y, r.w, r.h, bg);
    stroke_rect(r.x, r.y, r.w, r.h, border);
    let cx = r.x + (r.w as i32) / 2;
    let ty = r.y + ((r.h as i32) - kind.char_h()) / 2;
    text_centered(cx, ty, label, kind, fg);
}

pub fn draw_demo_btn(r: BtnRect, active: bool, kind: FontKind) {
    let (bg, border, label) = if active {
        (BTN_DEMO_ON_BG, BTN_DEMO_ON_BORDER, "DEMO ON")
    } else {
        (BTN_DEMO_OFF_BG, BTN_STD_BORDER, "DEMO")
    };
    let lbl = if active && (r.w as i32) < 7 * kind.char_w() + 4 {
        "DEMO*"
    } else {
        label
    };
    draw_btn(r, lbl, kind, bg, TEXT_WHITE, border);
}

pub fn draw_exit_btn(r: BtnRect, kind: FontKind) {
    draw_btn(r, "EXIT", kind, BTN_EXIT_BG, TEXT_WHITE, BTN_EXIT_BORDER);
}

#[inline]
pub fn in_box(p: Point, x: i32, y: i32, w: i32, h: i32) -> bool {
    p.x >= x && p.x < x + w && p.y >= y && p.y < y + h
}

pub fn u32_str(mut n: u32, buf: &mut [u8; 10]) -> &str {
    if n == 0 {
        buf[9] = b'0';
        return unsafe { core::str::from_utf8_unchecked(&buf[9..10]) };
    }
    let mut i = 10usize;
    while n > 0 && i > 0 {
        i -= 1;
        buf[i] = b'0' + (n % 10) as u8;
        n /= 10;
    }
    unsafe { core::str::from_utf8_unchecked(&buf[i..10]) }
}

pub struct LineBuf<const N: usize> {
    buf: [u8; N],
    len: usize,
}

impl<const N: usize> LineBuf<N> {
    pub const fn new() -> Self {
        Self {
            buf: [0u8; N],
            len: 0,
        }
    }

    pub fn clear(&mut self) {
        self.len = 0;
    }

    pub fn as_str(&self) -> &str {
        unsafe { core::str::from_utf8_unchecked(&self.buf[..self.len]) }
    }
}

impl<const N: usize> Default for LineBuf<N> {
    fn default() -> Self {
        Self::new()
    }
}

impl<const N: usize> Write for LineBuf<N> {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        let bytes = s.as_bytes();
        let avail = N.saturating_sub(self.len);
        let take = bytes.len().min(avail);
        self.buf[self.len..self.len + take].copy_from_slice(&bytes[..take]);
        self.len += take;
        Ok(())
    }
}

#[inline]
pub fn elapsed_ms(t0: Instant) -> u32 {
    t0.elapsed().as_millis() as u32
}
