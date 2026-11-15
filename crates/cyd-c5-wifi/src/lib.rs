//! `cyd-c5-wifi` — Wi-Fi для GameLauncher на NM-CYD-C5 (ESP32-C5).
//!
//! - [`net`] — фоновий таск: сканування (2.4 + 5 ГГц), підключення, DHCP, знімок стану;
//! - [`ui`] — екран `WI-FI` з екранною клавіатурою, кнопками SCAN/CONNECT/DISC/FORGET.
//!
//! Лаунчер викликає [`net::init_heap`] першою в `main`, потім `esp_rtos::start(...)`
//! і [`net::start`], а в циклі — [`ui::WifiScreen::tick`] / [`ui::WifiScreen::on_tap`].

#![no_std]

extern crate alloc;

pub mod ca;
pub mod http;
pub mod net;
pub mod update;
pub mod ui;

pub use net::{Ap, Band, Phase, Status, init_heap, start, MAX_APS};
pub use ui::{Outcome, UpdateAction, WifiScreen};
