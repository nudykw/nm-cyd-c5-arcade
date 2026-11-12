//! `cyd-c5-common` — спільна бібліотека апаратури, графіки, SD-картки та OTA-перемикання
//! для плати **NM-CYD-C5** (`ESP32-C5`, ST7789 320×240, XPT2046, microSD).

#![no_std]

pub mod board;
pub mod boot;
pub mod crc;
pub mod fb;
pub mod sd;
pub mod spi;
pub mod touch;
pub mod ui;

pub use board::{Board, Display, LCD_SPI_HZ};
pub use fb::{FB_PIXELS, Fb, SCREEN_H, SCREEN_W, dump_frame_hex, fb_mut, fb_ref, get_px, put_px};
