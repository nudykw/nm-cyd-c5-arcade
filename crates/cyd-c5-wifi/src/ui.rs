//! Екран Wi-Fi для GameLauncher: статус, список мереж (2.4 + 5 ГГц), екранна
//! клавіатура для пароля, кнопки SCAN / CONNECT / DISC / FORGET.

use core::fmt::Write;

use cyd_c5_common::{
    Display, sd::WifiCfg,
    ui::{
        ACCENT_CYAN, ACCENT_GOLD, ACCENT_GREEN, ACCENT_RED, BG_DEEP, BTN_EXIT_BG, BTN_EXIT_BORDER,
        BTN_STD_BG, BTN_STD_BORDER, BtnRect, CARD_BG, CARD_SEL_BG, FontKind, LineBuf, PANEL_BG,
        TEXT_MUTED, TEXT_WHITE, draw_btn, fill, rgb, stroke_rect, text,
    },
};
use embedded_graphics::prelude::Point;

use crate::net::{self, Ap, Band, MAX_APS, Phase, Status};
use crate::update::{self, UpdateState};

const CLR_OK: embedded_graphics::pixelcolor::Rgb565 = ACCENT_GREEN;

// ── Геометрія екрана 320×240 ────────────────────────────────────────────────────────────
const HDR_H: i32 = 26;
const BTN_CLOSE: BtnRect = BtnRect::new(252, 3, 62, 20);
const BTN_UPDATE: BtnRect = BtnRect::new(176, 3, 70, 20);

// екран оновлень
const UPD_STATUS_Y: i32 = 28;
const UPD_STATUS_H: u32 = 32;
const UPD_LIST_Y: i32 = 62;
const UPD_ROW_H: i32 = 26;
const UPD_ROWS: usize = 5;
const BTN_CHECK: BtnRect = BtnRect::new(4, 202, 90, 34);
const BTN_GAMES: BtnRect = BtnRect::new(98, 202, 96, 34);
const BTN_LAUNCHER: BtnRect = BtnRect::new(198, 202, 116, 34);

const STATUS_Y: i32 = 28;
const STATUS_H: u32 = 32;

const LIST_X: i32 = 4;
const LIST_Y: i32 = 62;
const LIST_W: u32 = 292;
const ROW_H: i32 = 26;
const ROW_GAP: i32 = 2;
const ROWS: usize = 5;
const LIST_H: i32 = (ROWS as i32) * ROW_H + ((ROWS as i32) - 1) * ROW_GAP; // 138

const SB_X: i32 = 298;
const SB_W: u32 = 18;
const SB_CAP_H: i32 = 20;

const BTN_SCAN: BtnRect = BtnRect::new(4, 202, 90, 34);
const BTN_JOIN: BtnRect = BtnRect::new(98, 202, 78, 34);
const BTN_DISC: BtnRect = BtnRect::new(180, 202, 64, 34);
const BTN_FORGET: BtnRect = BtnRect::new(248, 202, 68, 34);

const FIELD_Y: i32 = 40;
const FIELD_X: i32 = 6;
const FIELD_W: u32 = 308;
const FIELD_H: u32 = 26;
const BTN_CLR: BtnRect = BtnRect::new(232, 42, 40, 22);

/// Скільки символів пароля видно у полі (решта — «прокручена» вліво).
const FIELD_VISIBLE: usize = 36;

const KB_Y0: i32 = 70;
const KEY_W: i32 = 30;
const KEY_H: i32 = 30;
const KEY_GAP: i32 = 2;

pub const PASS_MAX: usize = 63;

// ── Розкладки клавіатури (10 клавіш у рядку × 4 рядки) ──────────────────────────────────
const KB_LOWER: [[u8; 10]; 4] = [
    *b"1234567890",
    *b"qwertyuiop",
    *b"asdfghjkl-",
    *b"zxcvbnm._,",
];
const KB_UPPER: [[u8; 10]; 4] = [
    *b"1234567890",
    *b"QWERTYUIOP",
    *b"ASDFGHJKL-",
    *b"ZXCVBNM._,",
];
const KB_SYMS: [[u8; 10]; 4] = [
    *b"1234567890",
    *b"!@#$%^&*()",
    *b"+={}[];:'_",
    *b"\"<>,.?/\\|~",
];

#[derive(Clone, Copy, PartialEq, Eq)]
enum Layer {
    Lower,
    Upper,
    Syms,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum View {
    List,
    Keyboard,
    Update,
}

/// Що зробити з мережею (виконує лаунчер, бо це async + SD/флеш).
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum UpdateAction {
    Check,
    InstallGames,
    InstallLauncher,
    /// Діагностика: `UURL <owner/repo> <asset>` — просто скачати у `dl` і показати розмір/CRC.
    Probe,
}

/// Результат дії користувача: що перемалювати і чи треба забути мережу на SD.
#[derive(Clone, Copy, Default)]
pub struct Outcome {
    pub redraw: bool,
    pub forget: bool,
    pub update: Option<UpdateAction>,
}

impl Outcome {
    fn redraw() -> Self {
        Outcome {
            redraw: true,
            ..Default::default()
        }
    }
    fn none() -> Self {
        Outcome::default()
    }
}

pub struct WifiScreen {
    pub open: bool,
    pub update: UpdateState,
    view: View,
    sel: usize,
    scroll: usize,
    layer: Layer,
    input: [u8; PASS_MAX + 1],
    input_len: usize,
    target: Option<Ap>,
    saved: Option<WifiCfg>,
    /// Мережа, до якої підключаємось: пароль запишеться на SD лише після успіху.
    pending: Option<WifiCfg>,
    /// Готовий до запису на SD конфіг (забирає лаунчер).
    saved_update: Option<WifiCfg>,
    forget_armed: bool,
    status: Status,
    aps: [Ap; MAX_APS],
    ap_count: usize,
    seen_status: u32,
    seen_scan: u32,
    chip_key: u8,
}

impl Default for WifiScreen {
    fn default() -> Self {
        Self::new()
    }
}

impl WifiScreen {
    pub fn new() -> Self {
        WifiScreen {
            open: false,
            update: UpdateState::new(),
            view: View::List,
            sel: 0,
            scroll: 0,
            layer: Layer::Lower,
            input: [0; PASS_MAX + 1],
            input_len: 0,
            target: None,
            saved: None,
            pending: None,
            saved_update: None,
            forget_armed: false,
            status: net::status(),
            aps: [Ap::EMPTY; MAX_APS],
            ap_count: 0,
            seen_status: 0,
            seen_scan: 0,
            chip_key: 0xFF,
        }
    }

    pub fn set_saved(&mut self, cfg: Option<WifiCfg>) {
        self.saved = cfg;
        self.chip_key = 0xFF;
    }

    /// Відкрити екран і одразу запустити сканування.
    pub fn open_screen(&mut self) {
        self.open = true;
        self.view = View::List;
        self.sel = 0;
        self.scroll = 0;
        self.forget_armed = false;
        net::request_scan();
    }

    pub fn close(&mut self) {
        self.open = false;
        self.view = View::List;
    }

    /// Підтягнути стан із менеджера; `true` — треба перемалювати.
    pub fn tick(&mut self) -> bool {
        let st = net::status();
        let (count, seq) = net::aps(&mut self.aps);
        let changed = st.seq != self.seen_status || seq != self.seen_scan;
        if changed {
            self.seen_status = st.seq;
            self.seen_scan = seq;
            self.status = st;
            self.ap_count = count;
            self.clamp();
        }

        // Пароль остаточно зберігаємо лише після успішного підключення й отримання IP.
        if st.phase == Phase::Connected
            && st.has_ip
            && let Some(cfg) = self.pending.take()
        {
            self.saved = Some(cfg);
            self.saved_update = Some(cfg);
            self.chip_key = 0xFF;
        }

        if !changed {
            return false;
        }
        if self.open {
            true
        } else {
            let key = chip_key(&st);
            let dirty = key != self.chip_key;
            self.chip_key = key;
            dirty
        }
    }

    /// Конфіг, який треба записати в `SD:/WIFI.CFG` (після успішного підключення).
    pub fn take_saved_update(&mut self) -> Option<WifiCfg> {
        self.saved_update.take()
    }

    pub fn status(&self) -> Status {
        self.status
    }

    pub fn chip_label(&self) -> (&'static str, Phase) {
        net::chip()
    }

    fn clamp(&mut self) {
        if self.ap_count == 0 {
            self.sel = 0;
            self.scroll = 0;
            return;
        }
        if self.sel >= self.ap_count {
            self.sel = self.ap_count - 1;
        }
        if self.sel < self.scroll {
            self.scroll = self.sel;
        } else if self.sel >= self.scroll + ROWS {
            self.scroll = self.sel + 1 - ROWS;
        }
        let max_scroll = self.ap_count.saturating_sub(ROWS);
        if self.scroll > max_scroll {
            self.scroll = max_scroll;
        }
    }

    fn selected(&self) -> Option<Ap> {
        if self.ap_count > 0 && self.sel < self.ap_count {
            Some(self.aps[self.sel])
        } else {
            None
        }
    }

    // ── Тапи ────────────────────────────────────────────────────────────────────────────
    pub fn on_tap(&mut self, p: Point) -> Outcome {
        match self.view {
            View::List => self.tap_list(p),
            View::Keyboard => self.tap_keyboard(p),
            View::Update => self.tap_update(p),
        }
    }

    fn tap_update(&mut self, p: Point) -> Outcome {
        if BTN_CLOSE.contains(p) {
            self.view = View::List;
            return Outcome::redraw();
        }
        if BTN_CHECK.contains(p) {
            return Outcome {
                redraw: true,
                forget: false,
                update: Some(UpdateAction::Check),
            };
        }
        if BTN_GAMES.contains(p) {
            return Outcome {
                redraw: true,
                forget: false,
                update: Some(UpdateAction::InstallGames),
            };
        }
        if BTN_LAUNCHER.contains(p) {
            return Outcome {
                redraw: true,
                forget: false,
                update: Some(UpdateAction::InstallLauncher),
            };
        }
        Outcome::none()
    }

    fn tap_list(&mut self, p: Point) -> Outcome {
        if BTN_CLOSE.contains(p) {
            self.close();
            return Outcome::redraw();
        }
        if BTN_SCAN.contains(p) {
            net::request_scan();
            return Outcome::redraw();
        }
        if BTN_DISC.contains(p) {
            net::request_disconnect();
            return Outcome::redraw();
        }
        if BTN_FORGET.contains(p) {
            if self.forget_armed {
                self.forget_armed = false;
                self.saved = None;
                self.chip_key = 0xFF;
                net::request_disconnect();
                return Outcome {
                    redraw: true,
                    forget: true,
                    update: None,
                };
            }
            self.forget_armed = true;
            return Outcome::redraw();
        }
        if BTN_UPDATE.contains(p) {
            self.view = View::Update;
            return Outcome::redraw();
        }
        if BTN_JOIN.contains(p) {
            return self.join_selected();
        }

        // стрілки прокрутки
        if self.ap_count > ROWS && in_rect(p, SB_X, LIST_Y, SB_W, SB_CAP_H as u32) {
            if self.scroll > 0 {
                self.scroll -= 1;
                if self.sel >= self.scroll + ROWS {
                    self.sel = self.scroll + ROWS - 1;
                }
            }
            return Outcome::redraw();
        }
        if self.ap_count > ROWS && in_rect(p, SB_X, LIST_Y + LIST_H - SB_CAP_H, SB_W, SB_CAP_H as u32) {
            if self.scroll + ROWS < self.ap_count {
                self.scroll += 1;
                if self.sel < self.scroll {
                    self.sel = self.scroll;
                }
            } else if self.sel + 1 < self.ap_count {
                self.sel += 1;
            }
            return Outcome::redraw();
        }

        // рядок списку
        if self.ap_count > 0 && in_rect(p, LIST_X, LIST_Y, LIST_W, LIST_H as u32) {
            let slot = ((p.y - LIST_Y) / (ROW_H + ROW_GAP)) as usize;
            let idx = self.scroll + slot;
            if slot < ROWS && idx < self.ap_count {
                self.sel = idx;
                // тап по мережі одразу відкриває клавіатуру (закрита) або підключає (відкрита)
                return self.join_selected();
            }
        }
        Outcome::none()
    }

    fn join_selected(&mut self) -> Outcome {
        let Some(ap) = self.selected() else {
            return Outcome::none();
        };
        self.target = Some(ap);
        if ap.auth.is_open() {
            if let Some(cfg) = WifiCfg::new(ap.ssid_bytes(), b"") {
                net::request_connect(cfg, Some(ap.channel));
                self.pending = Some(cfg);
                return Outcome::redraw();
            }
            return Outcome::none();
        }

        // Закрита мережа: пароль вводить користувач на екранній клавіатурі.
        // Збережений у /WIFI.CFG пароль НЕ підставляється — підключення лише після OK.
        self.view = View::Keyboard;
        self.layer = Layer::Lower;
        self.input_len = 0;
        self.input = [0; PASS_MAX + 1];
        Outcome::redraw()
    }

    fn tap_keyboard(&mut self, p: Point) -> Outcome {
        if BTN_CLOSE.contains(p) {
            self.view = View::List;
            return Outcome::redraw();
        }
        if BTN_CLR.contains(p) {
            self.input = [0; PASS_MAX + 1];
            self.input_len = 0;
            return Outcome::redraw();
        }
        if in_rect(p, FIELD_X, FIELD_Y, FIELD_W, FIELD_H) {
            return Outcome::none();
        }

        // 4 ряди по 10 клавіш
        if p.y >= KB_Y0 && p.y < KB_Y0 + 4 * (KEY_H + KEY_GAP) {
            let row = ((p.y - KB_Y0) / (KEY_H + KEY_GAP)) as usize;
            let rel = p.x - 1;
            if rel >= 0 {
                let col = (rel / (KEY_W + KEY_GAP)) as usize;
                if col < 10 && (rel % (KEY_W + KEY_GAP)) < KEY_W && row < 4 {
                    let table = match self.layer {
                        Layer::Lower => &KB_LOWER,
                        Layer::Upper => &KB_UPPER,
                        Layer::Syms => &KB_SYMS,
                    };
                    let ch = table[row][col];
                    if self.input_len < PASS_MAX {
                        self.input[self.input_len] = ch;
                        self.input_len += 1;
                    }
                    return Outcome::redraw();
                }
            }
            return Outcome::none();
        }

        // нижній ряд керування
        const KB_Y: i32 = 200;
        const KB_H: i32 = 36;
        if p.y >= KB_Y && p.y < KB_Y + KB_H {
            if in_rect(p, 2, KB_Y, 52, KB_H as u32) {
                self.layer = match self.layer {
                    Layer::Upper => Layer::Lower,
                    Layer::Lower | Layer::Syms => Layer::Upper,
                };
                return Outcome::redraw();
            }
            if in_rect(p, 56, KB_Y, 52, KB_H as u32) {
                self.layer = match self.layer {
                    Layer::Syms => Layer::Lower,
                    _ => Layer::Syms,
                };
                return Outcome::redraw();
            }
            if in_rect(p, 110, KB_Y, 96, KB_H as u32) {
                if self.input_len < PASS_MAX {
                    self.input[self.input_len] = b' ';
                    self.input_len += 1;
                }
                return Outcome::redraw();
            }
            if in_rect(p, 208, KB_Y, 50, KB_H as u32) {
                if self.input_len > 0 {
                    self.input_len -= 1;
                    self.input[self.input_len] = 0;
                }
                return Outcome::redraw();
            }
            if in_rect(p, 260, KB_Y, 58, KB_H as u32) {
                return self.submit_password();
            }
        }
        Outcome::none()
    }

    fn submit_password(&mut self) -> Outcome {
        let Some(ap) = self.target else {
            self.view = View::List;
            return Outcome::redraw();
        };
        let Some(cfg) = WifiCfg::new(ap.ssid_bytes(), &self.input[..self.input_len]) else {
            return Outcome::none();
        };
        net::request_connect(cfg, Some(ap.channel));
        self.pending = Some(cfg);
        self.view = View::List;
        self.input_len = 0;
        Outcome::redraw()
    }

    // ── UART-команди ───────────────────────────────────────────────────────────────────
    /// Обробляє `WIFI` / `WSCAN` / `WJOIN ssid|pass` / `WDISC` / `WFORGET` / `WSTATUS`.
    /// `None` — рядок не наш (лаунчер обробить сам). Регістр SSID/пароля зберігається.
    pub fn on_uart(&mut self, raw: &str) -> Option<Outcome> {
        let raw = raw.trim();
        let (cmd, rest) = match raw.split_once(' ') {
            Some((c, r)) => (c, r.trim()),
            None => (raw, ""),
        };

        if cmd.eq_ignore_ascii_case("WIFI") {
            if self.open {
                self.close();
            } else {
                self.open_screen();
            }
            return Some(Outcome::redraw());
        }
        if cmd.eq_ignore_ascii_case("WSCAN") {
            self.open = true;
            self.view = View::List;
            net::request_scan();
            return Some(Outcome::redraw());
        }
        if cmd.eq_ignore_ascii_case("WJOIN") {
            let (ssid, pass) = match rest.split_once('|') {
                Some((s, p)) => (s, p),
                None => (rest, ""),
            };
            let Some(cfg) = WifiCfg::new(ssid.as_bytes(), pass.as_bytes()) else {
                esp_println::println!("WIFI: usage WJOIN <ssid>|<password>");
                return Some(Outcome::none());
            };
            net::request_connect(cfg, None);
            self.pending = Some(cfg);
            self.open = true;
            self.view = View::List;
            return Some(Outcome::redraw());
        }
        if cmd.eq_ignore_ascii_case("WDISC") {
            net::request_disconnect();
            return Some(Outcome::redraw());
        }
        if cmd.eq_ignore_ascii_case("WFORGET") {
            self.saved = None;
            self.pending = None;
            self.chip_key = 0xFF;
            net::request_disconnect();
            return Some(Outcome {
                redraw: true,
                forget: true,
                update: None,
            });
        }
        if cmd.eq_ignore_ascii_case("WTAP") {
            // діагностика без тачу: WTAP <x> <y> — прогнати той самий обробник, що й палець
            let mut it = rest.split_whitespace();
            let x = it.next().and_then(|v| v.parse::<i32>().ok());
            let y = it.next().and_then(|v| v.parse::<i32>().ok());
            if let (Some(x), Some(y)) = (x, y) {
                self.open = true;
                return Some(self.on_tap(Point::new(x, y)));
            }
            esp_println::println!("WIFI: usage WTAP <x> <y>");
            return Some(Outcome::none());
        }
        if cmd.eq_ignore_ascii_case("WKB") {
            // діагностика без тачу: відкрити клавіатуру для мережі N (типово — вибраної)
            let (count, _) = net::aps(&mut self.aps);
            self.ap_count = count;
            let idx = rest.parse::<usize>().unwrap_or(self.sel);
            if idx < self.ap_count {
                self.sel = idx;
                self.target = Some(self.aps[idx]);
                self.view = View::Keyboard;
                self.layer = Layer::Lower;
                self.input_len = 0;
                self.open = true;
                return Some(Outcome::redraw());
            }
            esp_println::println!("WIFI: WKB: немає мережі {idx}; спершу WSCAN");
            return Some(Outcome::none());
        }
        if cmd.eq_ignore_ascii_case("UPD") || cmd.eq_ignore_ascii_case("UPDATE") {
            self.open = true;
            self.view = View::Update;
            return Some(Outcome::redraw());
        }
        if cmd.eq_ignore_ascii_case("UCHECK") {
            self.open = true;
            self.view = View::Update;
            return Some(Outcome {
                redraw: true,
                forget: false,
                update: Some(UpdateAction::Check),
            });
        }
        if cmd.eq_ignore_ascii_case("UGAMES") {
            self.open = true;
            self.view = View::Update;
            return Some(Outcome {
                redraw: true,
                forget: false,
                update: Some(UpdateAction::InstallGames),
            });
        }
        if cmd.eq_ignore_ascii_case("ULAUNCHER") {
            self.open = true;
            self.view = View::Update;
            return Some(Outcome {
                redraw: true,
                forget: false,
                update: Some(UpdateAction::InstallLauncher),
            });
        }
        if cmd.eq_ignore_ascii_case("UURL") {
            let mut it = rest.split_whitespace();
            let (Some(repo), Some(asset)) = (it.next(), it.next()) else {
                esp_println::println!("UPDATE: usage UURL <owner/repo> <asset>");
                return Some(Outcome::none());
            };
            self.update.set_repo(repo);
            self.update.set_probe(asset);
            self.open = true;
            self.view = View::Update;
            return Some(Outcome {
                redraw: true,
                forget: false,
                update: Some(UpdateAction::Probe),
            });
        }
        if cmd.eq_ignore_ascii_case("USTATUS") {
            self.print_update_status();
            return Some(Outcome::none());
        }
        if cmd.eq_ignore_ascii_case("WSTATUS") {
            self.print_status();
            return Some(Outcome::none());
        }
        None
    }

    fn print_update_status(&mut self) {
        let u = &self.update;
        esp_println::println!(
            "UPDATE: phase={:?} err=\"{}\" version={} slot={} repo={} pending={} ({} B) heap_free={}",
            u.phase,
            u.err,
            u.version(),
            u.slot,
            u.repo(),
            u.pending(),
            u.pending_size(),
            esp_alloc::HEAP.free()
        );
        for i in 0..u.count {
            let e = &u.entries[i];
            esp_println::println!(
                "  {} {} size={} crc={:08x} {}",
                match e.kind {
                    update::Kind::Launcher => "LAUNCHER",
                    update::Kind::Game => "GAME    ",
                },
                e.name(),
                e.size,
                e.crc,
                if u.have[i] { "OK" } else { "NEED" }
            );
        }
    }

    fn print_status(&mut self) {
        let (count, _) = net::aps(&mut self.aps);
        self.ap_count = count;
        let st = net::status();
        self.status = st;
        esp_println::println!(
            "WIFI: phase={:?} ssid=\"{}\" ch={} ip={}.{}.{}.{} rssi={} err={}",
            st.phase,
            st.ssid(),
            st.channel,
            st.ip[0],
            st.ip[1],
            st.ip[2],
            st.ip[3],
            st.rssi,
            st.err.label()
        );
        esp_println::println!(
            "WIFI: heap used {} B, free {} B",
            esp_alloc::HEAP.used(),
            esp_alloc::HEAP.free()
        );
        for i in 0..count {
            let ap = &self.aps[i];
            esp_println::println!(
                "  [{}] \"{}\" {} ch={} rssi={} {}",
                i,
                ap.ssid(),
                ap.band().label(),
                ap.channel,
                ap.rssi,
                ap.auth.label()
            );
        }
    }

    // ── Малювання ──────────────────────────────────────────────────────────────────────
    pub fn draw(&mut self) {
        // підтягуємо свіжий стан, щоб екран ніколи не показував старе
        self.status = net::status();
        let (count, seq) = net::aps(&mut self.aps);
        self.ap_count = count;
        self.seen_scan = seq;
        self.seen_status = self.status.seq;
        self.clamp();

        fill(0, 0, 320, 240, BG_DEEP);
        fill(0, 0, 320, HDR_H as u32, PANEL_BG);
        fill(0, HDR_H, 320, 1, rgb(64, 88, 128));

        match self.view {
            View::List => {
                text(8, 4, "WI-FI 2.4 + 5 GHz", FontKind::Bold, ACCENT_GOLD);
                draw_btn(
                    BTN_UPDATE,
                    "UPDATE",
                    FontKind::Small,
                    BTN_STD_BG,
                    ACCENT_GOLD,
                    ACCENT_GOLD,
                );
                draw_btn(
                    BTN_CLOSE,
                    "CLOSE",
                    FontKind::Small,
                    BTN_EXIT_BG,
                    TEXT_WHITE,
                    BTN_EXIT_BORDER,
                );
                self.draw_status_panel();
                self.draw_list();
                self.draw_buttons();
            }
            View::Update => {
                text(8, 4, "UPDATES", FontKind::Bold, ACCENT_GOLD);
                text(96, 4, "github releases", FontKind::Small, TEXT_MUTED);
                draw_btn(
                    BTN_CLOSE,
                    "BACK",
                    FontKind::Small,
                    BTN_STD_BG,
                    TEXT_WHITE,
                    BTN_STD_BORDER,
                );
                draw_update_screen(&self.update);
            }
            View::Keyboard => {
                text(8, 4, "WI-FI PASSWORD", FontKind::Bold, ACCENT_GOLD);
                draw_btn(
                    BTN_CLOSE,
                    "BACK",
                    FontKind::Small,
                    BTN_STD_BG,
                    TEXT_WHITE,
                    BTN_STD_BORDER,
                );
                self.draw_keyboard();
            }
        }
    }

    fn draw_status_panel(&self) {
        fill(0, STATUS_Y, 320, STATUS_H, PANEL_BG);
        stroke_rect(0, STATUS_Y, 320, STATUS_H, rgb(52, 66, 92));

        let st = self.status;
        let (col, line1) = match st.phase {
            Phase::Connected if st.has_ip => (ACCENT_GREEN, "CONNECTED"),
            Phase::Connected => (ACCENT_GOLD, "CONNECTED (NO IP)"),
            Phase::Connecting => (ACCENT_GOLD, "CONNECTING..."),
            Phase::Scanning => (ACCENT_CYAN, "SCANNING..."),
            Phase::Failed => (ACCENT_RED, "FAILED"),
            Phase::Off => (TEXT_MUTED, "WI-FI OFF"),
            Phase::Idle => (TEXT_MUTED, "NOT CONNECTED"),
        };
        text(8, STATUS_Y + 4, line1, FontKind::Small, col);

        // IP — праворуч у першому рядку (не накладається на довгі SSID)
        if st.phase == Phase::Connected && st.has_ip {
            let mut ip = LineBuf::<20>::new();
            let _ = write!(
                ip,
                "{}.{}.{}.{}",
                st.ip[0], st.ip[1], st.ip[2], st.ip[3]
            );
            text(230, STATUS_Y + 4, ip.as_str(), FontKind::Small, ACCENT_CYAN);
        }

        let mut l2 = LineBuf::<56>::new();
        match st.phase {
            Phase::Connected => {
                let name = st.ssid();
                let name = name.get(..30).unwrap_or(name);
                let _ = write!(
                    l2,
                    "{} {} ch{} {}dBm",
                    name,
                    st.band().label(),
                    st.channel,
                    st.rssi
                );
                text(8, STATUS_Y + 17, l2.as_str(), FontKind::Small, TEXT_WHITE);
            }
            Phase::Connecting => {
                let _ = write!(l2, "{}", st.ssid());
                if st.channel != 0 {
                    let _ = write!(l2, " {}", st.band().label());
                }
                if self.saved.is_some() {
                    let _ = write!(l2, " (saved)");
                }
                text(8, STATUS_Y + 17, l2.as_str(), FontKind::Small, TEXT_MUTED);
            }
            Phase::Failed => {
                let _ = write!(l2, "{}", st.err.label());
                text(8, STATUS_Y + 17, l2.as_str(), FontKind::Small, ACCENT_RED);
            }
            Phase::Scanning => {
                text(
                    8,
                    STATUS_Y + 17,
                    "2.4 GHz + 5 GHz channels",
                    FontKind::Small,
                    TEXT_MUTED,
                );
            }
            _ => {
                if let Some(saved) = self.saved {
                    let mut l = LineBuf::<48>::new();
                    let name = saved.ssid();
                    let _ = write!(l, "SAVED: {} - TAP AND TYPE PASS", name.get(..20).unwrap_or(name));
                    text(8, STATUS_Y + 17, l.as_str(), FontKind::Small, TEXT_MUTED);
                } else {
                    text(
                        8,
                        STATUS_Y + 17,
                        "TAP SCAN TO FIND NETWORKS",
                        FontKind::Small,
                        TEXT_MUTED,
                    );
                }
            }
        }
    }

    fn draw_list(&self) {
        if self.ap_count == 0 {
            fill(LIST_X, LIST_Y, LIST_W, LIST_H as u32, CARD_BG);
            stroke_rect(LIST_X, LIST_Y, LIST_W, LIST_H as u32, BTN_STD_BORDER);
            text_centered(
                LIST_X + (LIST_W as i32) / 2,
                LIST_Y + 52,
                "NO NETWORKS FOUND",
                FontKind::Bold,
                ACCENT_GOLD,
            );
            text_centered(
                LIST_X + (LIST_W as i32) / 2,
                LIST_Y + 74,
                "TAP SCAN (2.4 + 5 GHz)",
                FontKind::Small,
                TEXT_MUTED,
            );
            return;
        }

        let visible = ROWS.min(self.ap_count - self.scroll.min(self.ap_count));
        for slot in 0..visible {
            let idx = self.scroll + slot;
            let ap = &self.aps[idx];
            let ry = LIST_Y + (slot as i32) * (ROW_H + ROW_GAP);
            let is_sel = idx == self.sel;
            let bg = if is_sel { CARD_SEL_BG } else { CARD_BG };
            let brd = if is_sel { ACCENT_CYAN } else { rgb(52, 66, 92) };
            fill(LIST_X, ry, LIST_W, ROW_H as u32, bg);
            stroke_rect(LIST_X, ry, LIST_W, ROW_H as u32, brd);
            if is_sel {
                fill(LIST_X + 1, ry + 1, 3, (ROW_H - 2) as u32, ACCENT_GOLD);
            }

            text(
                LIST_X + 10,
                ry + 3,
                ap.ssid(),
                FontKind::Small,
                if is_sel { TEXT_WHITE } else { rgb(220, 228, 242) },
            );

            let mut sub = LineBuf::<34>::new();
            let _ = write!(
                sub,
                "{} ch{} {} dBm",
                ap.auth.label(),
                ap.channel,
                ap.rssi
            );
            text(LIST_X + 10, ry + 15, sub.as_str(), FontKind::Small, TEXT_MUTED);

            // бейдж діапазону: 2G зелений, 5G блакитний
            let bx = LIST_X + LIST_W as i32 - 44;
            let (bbg, bfg, bbrd) = match ap.band() {
                Band::G5 => (rgb(24, 64, 92), ACCENT_CYAN, ACCENT_CYAN),
                Band::G24 => (rgb(26, 74, 50), ACCENT_GREEN, rgb(58, 160, 96)),
            };
            fill(bx, ry + 6, 38, 15, bbg);
            stroke_rect(bx, ry + 6, 38, 15, bbrd);
            text(bx + 8, ry + 8, ap.band().label(), FontKind::Small, bfg);

            // збережена мережа — зірочка
            if let Some(saved) = self.saved
                && saved.ssid_bytes() == ap.ssid_bytes()
            {
                text(bx + 22, ry + 8, "*", FontKind::Small, ACCENT_GOLD);
            }
        }

        // смуга прокрутки зі стрілками
        if self.ap_count > ROWS {
            fill(SB_X, LIST_Y, SB_W, LIST_H as u32, PANEL_BG);
            stroke_rect(SB_X, LIST_Y, SB_W, LIST_H as u32, BTN_STD_BORDER);
            draw_arrow(SB_X + (SB_W as i32) / 2, LIST_Y + SB_CAP_H / 2, true,
                if self.scroll > 0 { ACCENT_CYAN } else { TEXT_MUTED });
            draw_arrow(
                SB_X + (SB_W as i32) / 2,
                LIST_Y + LIST_H - SB_CAP_H / 2,
                false,
                if self.scroll + ROWS < self.ap_count { ACCENT_CYAN } else { TEXT_MUTED },
            );
        }
    }

    fn draw_buttons(&self) {
        draw_btn(
            BTN_SCAN,
            "SCAN",
            FontKind::Small,
            BTN_STD_BG,
            TEXT_WHITE,
            ACCENT_CYAN,
        );
        let can_join = self.ap_count > 0;
        draw_btn(
            BTN_JOIN,
            "CONNECT",
            FontKind::Small,
            if can_join { rgb(34, 102, 62) } else { rgb(36, 44, 58) },
            if can_join { TEXT_WHITE } else { TEXT_MUTED },
            if can_join { CLR_OK } else { rgb(62, 74, 94) },
        );
        draw_btn(
            BTN_DISC,
            "DISC",
            FontKind::Small,
            BTN_STD_BG,
            TEXT_WHITE,
            BTN_STD_BORDER,
        );
        let (label, bg, brd) = if self.forget_armed {
            ("SURE?", rgb(150, 40, 40), ACCENT_RED)
        } else {
            ("FORGET", BTN_EXIT_BG, BTN_EXIT_BORDER)
        };
        draw_btn(BTN_FORGET, label, FontKind::Small, bg, TEXT_WHITE, brd);
    }

    fn draw_keyboard(&self) {
        let Some(ap) = self.target else {
            return;
        };

        // заголовок мережі
        let mut net_line = LineBuf::<44>::new();
        let _ = write!(net_line, "NET: {} {} ch{}", ap.ssid(), ap.band().label(), ap.channel);
        text(8, STATUS_Y + 2, net_line.as_str(), FontKind::Small, TEXT_MUTED);

        // поле пароля
        fill(FIELD_X, FIELD_Y, FIELD_W, FIELD_H, CARD_BG);
        stroke_rect(FIELD_X, FIELD_Y, FIELD_W, FIELD_H, ACCENT_CYAN);
        // показуємо хвіст пароля (усе поле — 36 символів), курсор — у кінці
        let all = core::str::from_utf8(&self.input[..self.input_len]).unwrap_or("");
        let start = all.len().saturating_sub(FIELD_VISIBLE);
        let shown = &all[start..];
        text(FIELD_X + 6, FIELD_Y + 7, shown, FontKind::Small, TEXT_WHITE);
        let cx = FIELD_X + 6 + (shown.len() as i32) * 6;
        fill(cx, FIELD_Y + 6, 1, 12, ACCENT_CYAN);

        // CLR — стерти все поле одним тапом
        draw_btn(
            BTN_CLR,
            "CLR",
            FontKind::Small,
            BTN_STD_BG,
            TEXT_WHITE,
            BTN_STD_BORDER,
        );

        // клавіші
        let table = match self.layer {
            Layer::Lower => &KB_LOWER,
            Layer::Upper => &KB_UPPER,
            Layer::Syms => &KB_SYMS,
        };
        for (row, keys) in table.iter().enumerate() {
            for (col, &ch) in keys.iter().enumerate() {
                let x = 1 + (col as i32) * (KEY_W + KEY_GAP);
                let y = KB_Y0 + (row as i32) * (KEY_H + KEY_GAP);
                let mut buf = [0u8; 1];
                buf[0] = ch;
                let label = core::str::from_utf8(&buf).unwrap_or("?");
                let r = BtnRect::new(x, y, KEY_W as u32, KEY_H as u32);
                draw_btn(r, label, FontKind::Small, CARD_BG, TEXT_WHITE, rgb(52, 66, 92));
            }
        }

        // керування
        const KB_Y: i32 = 200;
        const KB_H: u32 = 36;
        let shift_on = self.layer == Layer::Upper;
        draw_btn(
            BtnRect::new(2, KB_Y, 52, KB_H),
            "SHIFT",
            FontKind::Small,
            if shift_on { rgb(36, 134, 86) } else { BTN_STD_BG },
            TEXT_WHITE,
            if shift_on { CLR_OK } else { BTN_STD_BORDER },
        );
        let (lbl, on) = if self.layer == Layer::Syms {
            ("abc", true)
        } else {
            ("123#", false)
        };
        draw_btn(
            BtnRect::new(56, KB_Y, 52, KB_H),
            lbl,
            FontKind::Small,
            if on { rgb(36, 90, 134) } else { BTN_STD_BG },
            TEXT_WHITE,
            if on { ACCENT_CYAN } else { BTN_STD_BORDER },
        );
        draw_btn(
            BtnRect::new(110, KB_Y, 96, KB_H),
            "SPACE",
            FontKind::Small,
            BTN_STD_BG,
            TEXT_WHITE,
            BTN_STD_BORDER,
        );
        draw_btn(
            BtnRect::new(208, KB_Y, 50, KB_H),
            "DEL",
            FontKind::Small,
            BTN_STD_BG,
            TEXT_WHITE,
            BTN_STD_BORDER,
        );
        draw_btn(
            BtnRect::new(260, KB_Y, 58, KB_H),
            "OK",
            FontKind::Bold,
            rgb(34, 146, 78),
            TEXT_WHITE,
            rgb(106, 240, 152),
        );

        // лічильник довжини — праворуч у полі пароля (WPA: 8..63)
        let mut hint = LineBuf::<12>::new();
        let _ = write!(hint, "{}/{}", self.input_len, PASS_MAX);
        text(
            FIELD_X + FIELD_W as i32 - 6 - 5 * 6,
            FIELD_Y + 7,
            hint.as_str(),
            FontKind::Small,
            if self.input_len >= 8 { ACCENT_GREEN } else { TEXT_MUTED },
        );
    }

}

fn chip_key(st: &Status) -> u8 {
    let phase = match st.phase {
        Phase::Off => 0,
        Phase::Idle => 1,
        Phase::Scanning => 2,
        Phase::Connecting => 3,
        Phase::Connected => 4,
        Phase::Failed => 5,
    };
    let band = if st.has_ip {
        match st.band() {
            Band::G5 => 1,
            Band::G24 => 2,
        }
    } else {
        0
    };
    phase * 4 + band
}

#[inline]
fn in_rect(p: Point, x: i32, y: i32, w: u32, h: u32) -> bool {
    p.x >= x && p.x < x + w as i32 && p.y >= y && p.y < y + h as i32
}

fn text_centered(cx: i32, y: i32, s: &str, kind: FontKind, color: embedded_graphics::pixelcolor::Rgb565) {
    let w = s.len() as i32 * kind.char_w();
    text(cx - w / 2, y, s, kind, color);
}

fn draw_arrow(cx: i32, cy: i32, up: bool, col: embedded_graphics::pixelcolor::Rgb565) {
    for row in 0..5i32 {
        let half = if up { row } else { 4 - row };
        fill(cx - half, cy - 2 + row, (half * 2 + 1) as u32, 1, col);
    }
}

// ── Екран оновлень ──────────────────────────────────────────────────────────────────────
/// Малює екран `UPDATES` у фреймбуфер (викликається з `WifiScreen::draw` і з прогресу завантаження).
pub fn draw_update_screen(st: &UpdateState) {
    // статус
    fill(0, UPD_STATUS_Y, 320, UPD_STATUS_H, PANEL_BG);
    stroke_rect(0, UPD_STATUS_Y, 320, UPD_STATUS_H, rgb(52, 66, 92));

    let (col, _) = match st.phase {
        update::Phase::Failed => (ACCENT_RED, ()),
        update::Phase::Ready | update::Phase::Downloading => (ACCENT_GOLD, ()),
        update::Phase::Done => (ACCENT_GREEN, ()),
        _ => (TEXT_MUTED, ()),
    };
    text(8, UPD_STATUS_Y + 4, st.phase.label(), FontKind::Small, col);

    let mut l2 = LineBuf::<56>::new();
    if !st.err.is_empty() {
        let _ = write!(l2, "{}", st.err);
        text(8, UPD_STATUS_Y + 17, l2.as_str(), FontKind::Small, ACCENT_RED);
    } else {
        if st.version_len == 0 {
            let _ = write!(l2, "not checked  slot {}", st.slot);
        } else {
            let _ = write!(
                l2,
                "v{}  slot {}  pending {} ({} KB)",
                st.version(),
                st.slot,
                st.pending(),
                st.pending_size().div_ceil(1024)
            );
        }
        text(8, UPD_STATUS_Y + 17, l2.as_str(), FontKind::Small, TEXT_MUTED);
    }

    // список файлів / прогрес
    if st.phase == update::Phase::Downloading {
        fill(4, UPD_LIST_Y, 312, 138, CARD_BG);
        stroke_rect(4, UPD_LIST_Y, 312, 138, BTN_STD_BORDER);
        let mut t = LineBuf::<48>::new();
        let _ = write!(
            t,
            "{} KB / {} KB",
            st.progress.div_ceil(1024),
            st.total.div_ceil(1024)
        );
        text(16, UPD_LIST_Y + 46, t.as_str(), FontKind::Bold, TEXT_WHITE);
        // смуга
        let bx = 16;
        let by = UPD_LIST_Y + 72;
        let bw = 288u32;
        fill(bx, by, bw, 18, rgb(20, 26, 38));
        stroke_rect(bx, by, bw, 18, BTN_STD_BORDER);
        let frac = if st.total > 0 {
            (st.progress.min(st.total) as u64 * (bw as u64 - 4) / st.total as u64) as u32
        } else {
            0
        };
        fill(bx + 2, by + 2, frac, 14, ACCENT_GREEN);
        return;
    }

    if st.count == 0 {
        fill(4, UPD_LIST_Y, 312, 138, CARD_BG);
        stroke_rect(4, UPD_LIST_Y, 312, 138, BTN_STD_BORDER);
        text_centered(
            160,
            UPD_LIST_Y + 52,
            "NO MANIFEST YET",
            FontKind::Bold,
            ACCENT_GOLD,
        );
        text_centered(
            160,
            UPD_LIST_Y + 74,
            "TAP CHECK (WI-FI MUST BE ON)",
            FontKind::Small,
            TEXT_MUTED,
        );
    }

    let rows = UPD_ROWS.min(st.count);
    for i in 0..rows {
        let e = &st.entries[i];
        let ry = UPD_LIST_Y + (i as i32) * (UPD_ROW_H + 2);
        fill(4, ry, 312, UPD_ROW_H as u32, CARD_BG);
        stroke_rect(4, ry, 312, UPD_ROW_H as u32, rgb(52, 66, 92));

        let is_launcher = e.kind == update::Kind::Launcher;
        text(
            12,
            ry + 3,
            e.name(),
            FontKind::Small,
            if is_launcher { ACCENT_CYAN } else { TEXT_WHITE },
        );
        let mut sub = LineBuf::<40>::new();
        let _ = write!(sub, "{} KB  crc {:08x}", e.size.div_ceil(1024), e.crc);
        text(12, ry + 15, sub.as_str(), FontKind::Small, TEXT_MUTED);

        let (badge, bg, fg) = if st.have[i] {
            ("OK", rgb(26, 74, 50), ACCENT_GREEN)
        } else {
            ("NEW", rgb(64, 52, 18), ACCENT_GOLD)
        };
        fill(276, ry + 6, 34, 15, bg);
        stroke_rect(276, ry + 6, 34, 15, fg);
        text(282, ry + 8, badge, FontKind::Small, fg);
    }

    // кнопки
    draw_btn(
        BTN_CHECK,
        "CHECK",
        FontKind::Small,
        BTN_STD_BG,
        TEXT_WHITE,
        ACCENT_CYAN,
    );
    let games_ready = st.count > 0 && (0..st.count).any(|i| !st.have[i] && st.entries[i].kind == update::Kind::Game);
    draw_btn(
        BTN_GAMES,
        "GAMES",
        FontKind::Small,
        if games_ready { rgb(34, 102, 62) } else { rgb(36, 44, 58) },
        if games_ready { TEXT_WHITE } else { TEXT_MUTED },
        if games_ready { CLR_OK } else { rgb(62, 74, 94) },
    );
    let launcher_ready = st.count > 0 && (0..st.count).any(|i| {
        !st.have[i] && st.entries[i].kind == update::Kind::Launcher
    });
    draw_btn(
        BTN_LAUNCHER,
        "LAUNCHER",
        FontKind::Small,
        if launcher_ready { rgb(120, 60, 20) } else { rgb(36, 44, 58) },
        if launcher_ready { TEXT_WHITE } else { TEXT_MUTED },
        if launcher_ready { ACCENT_GOLD } else { rgb(62, 74, 94) },
    );
}

/// Викликається з фонового завантаження: малює екран і одразу віддає його на дисплей.
pub fn update_draw_progress(display: &mut Display, st: &UpdateState) {
    fill(0, 0, 320, 240, BG_DEEP);
    fill(0, 0, 320, 26, PANEL_BG);
    fill(0, 26, 320, 1, rgb(64, 88, 128));
    text(8, 4, "UPDATES", FontKind::Bold, ACCENT_GOLD);
    text(96, 4, "downloading", FontKind::Small, TEXT_MUTED);
    draw_update_screen(st);
    cyd_c5_common::board::present_rect_on(display, 0, 0, 320, 240);
}
