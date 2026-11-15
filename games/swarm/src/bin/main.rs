//! `SWARM` — RTS-lite для NM-CYD-C5 (ESP32-C5): гравець (сині) проти AI (червоні).
//!
//! Карта 20×13 тайлів по 16 px (320×208) + HUD 32 px. Симуляція 20 Гц, рендер — dirty-rect:
//! кожен кадр перемальовуються лише прямокутники, які змінилися (рух юнітів, будівництво,
//! ефекти, HUD), а не весь кадр (повний кадр коштує ~45 ms, dirty — 5–12 ms).
//!
//! Керування (один палець):
//! - тап по своєму юніту — вибрати, тап по землі — наказ руху / атаки;
//! - потягнути від землі — рамка виділення; подвійний тап по юніту — всі юніти того ж типу;
//! - тап по своїй будівлі — вибрати (в HUD з'являться кнопки виробництва);
//! - HUD: `BLD` (поставити барак), `STOP`, `WRK`/`GRT`/`HVY` (найм), `DEMO`, `EXIT`.
//!
//! UART 115200 (для перевірки без пальця): `X`/`Q` — вихід, `D` — демо, `S` — статус,
//! `T` — найняти робітника, `B` — поставити барак, `Y` — найняти ґранта, `A` — атакувати ворожий HQ,
//! `G` — усі робітники на родовище, `1`/`2`/`3` — вибрати тип юнітів, `?` — знімок екрана.

#![no_std]
#![no_main]

use core::fmt::Write;
use cyd_c5_common::{
    Board, SCREEN_W, put_px,
    ui::{
        ACCENT_CYAN, ACCENT_GOLD, ACCENT_GREEN, ACCENT_RED, BG_DEEP, BTN_STD_BG, BTN_STD_BORDER,
        BtnRect, FontKind, LineBuf, TEXT_MUTED, TEXT_WHITE, blend_fill, draw_btn, draw_demo_btn,
        draw_exit_btn, elapsed_ms, fill, rgb, stroke_rect, text, text_centered, u32_str,
    },
};
use cyd_game_swarm::game::{
    B_BARRACKS, B_HQ, Building, Game, HUD_H, MAP, MAP_PX_H, MAP_PX_W, MAX_BUILDINGS,
    MAX_EFFECTS, MAX_UNITS, O_ATTACK_MOVE, SIDE_BLUE, SIDE_RED, SUPPLY_MAX, TILE, T_MINERAL, T_ROCK,
    T_SAND, TICK_MS, U_GRUNT, U_HEAVY, U_WORKER, UNIT_SIZE, Unit, FX_HIT, bld_stats, selftest,
    unit_stats,
};
use embedded_graphics::{pixelcolor::Rgb565, prelude::Point};
use esp_hal::time::Instant;
use esp_println::println;

#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    println!("SWARM PANIC: {:?}", info);
    loop {}
}

esp_bootloader_esp_idf::esp_app_desc!(
    "0.1.0",
    "cyd-game-swarm",
    "00:00:00",
    "2026-11-13",
    "5.5.6",
    64 * 1024,
    0,
    u16::MAX,
    0
);

const SAVE_FILE: &str = "SWARM.SAV";

// ── Палітра ──────────────────────────────────────────────────────────────────

const C_GRASS: Rgb565 = rgb(34, 74, 44);
const C_GRASS_D: Rgb565 = rgb(26, 60, 36);
const C_SAND: Rgb565 = rgb(104, 90, 56);
const C_SAND_D: Rgb565 = rgb(84, 72, 44);
const C_ROCK: Rgb565 = rgb(78, 86, 104);
const C_ROCK_HI: Rgb565 = rgb(104, 114, 134);
const C_ROCK_LO: Rgb565 = rgb(48, 54, 68);
const C_MINE: Rgb565 = rgb(28, 58, 92);
const C_MINE_HI: Rgb565 = rgb(96, 180, 240);
const C_MINE_MID: Rgb565 = rgb(56, 128, 190);
const C_BLUE: Rgb565 = rgb(72, 132, 232);
const C_BLUE_D: Rgb565 = rgb(26, 52, 116);
const C_RED: Rgb565 = rgb(224, 88, 72);
const C_RED_D: Rgb565 = rgb(108, 32, 28);
const C_CORE: Rgb565 = rgb(250, 250, 255);
const C_HP_BG: Rgb565 = rgb(20, 16, 24);

// ── HUD ──────────────────────────────────────────────────────────────────────

const HUD_Y: i32 = MAP_PX_H;
const BTN_ACT1: BtnRect = BtnRect::new(108, HUD_Y + 3, 46, 26);
const BTN_ACT2: BtnRect = BtnRect::new(158, HUD_Y + 3, 46, 26);
const BTN_DEMO: BtnRect = BtnRect::new(208, HUD_Y + 3, 52, 26);
const BTN_EXIT: BtnRect = BtnRect::new(264, HUD_Y + 3, 52, 26);

type HudState = (u32, u16, u32, u32, u8, bool, bool, [u8; 2]);
const HUD_STATE_INIT: HudState = (u32::MAX, 0, 0, 0, 0, false, false, [0, 0]);

const ACT_NONE: u8 = 0;
const ACT_BUILD: u8 = 1;
const ACT_STOP: u8 = 2;
const ACT_WORKER: u8 = 3;
const ACT_GRUNT: u8 = 4;
const ACT_HEAVY: u8 = 5;
const ACT_CANCEL: u8 = 6;

/// Скільки кадрів у вікні статистики (лог у UART).
const STATS_FRAMES: u32 = 60;
/// Як часто опитувати тач (мс) — XPT2046 сидить на тій самій шині, що й дисплей.
const TOUCH_MS: u32 = 16;
/// Як часто перемальовувати HUD (мс).
const HUD_MS: u32 = 200;

fn act_label(a: u8) -> &'static str {
    match a {
        ACT_BUILD => "BLD",
        ACT_STOP => "STOP",
        ACT_WORKER => "WRK",
        ACT_GRUNT => "GRT",
        ACT_HEAVY => "HVY",
        ACT_CANCEL => "CANCEL",
        _ => "",
    }
}

// ── Прямокутники (dirty rect) ────────────────────────────────────────────────

#[derive(Clone, Copy, PartialEq, Eq)]
struct Rect {
    x: i32,
    y: i32,
    w: i32,
    h: i32,
}

impl Rect {
    const EMPTY: Self = Self { x: 0, y: 0, w: 0, h: 0 };

    #[inline]
    fn area(&self) -> i32 {
        self.w * self.h
    }

    #[inline]
    fn is_empty(&self) -> bool {
        self.w <= 0 || self.h <= 0
    }

    #[inline]
    fn intersects(&self, o: Rect) -> bool {
        !self.is_empty()
            && !o.is_empty()
            && self.x < o.x + o.w
            && o.x < self.x + self.w
            && self.y < o.y + o.h
            && o.y < self.y + self.h
    }

    fn union(&self, o: Rect) -> Rect {
        if self.is_empty() {
            return o;
        }
        if o.is_empty() {
            return *self;
        }
        let x0 = self.x.min(o.x);
        let y0 = self.y.min(o.y);
        let x1 = (self.x + self.w).max(o.x + o.w);
        let y1 = (self.y + self.h).max(o.y + o.h);
        Rect { x: x0, y: y0, w: x1 - x0, h: y1 - y0 }
    }

    fn clip_map(&self) -> Rect {
        let x0 = self.x.max(0);
        let y0 = self.y.max(0);
        let x1 = (self.x + self.w).min(MAP_PX_W);
        let y1 = (self.y + self.h).min(MAP_PX_H);
        if x1 <= x0 || y1 <= y0 {
            return Rect::EMPTY;
        }
        Rect { x: x0, y: y0, w: x1 - x0, h: y1 - y0 }
    }

    fn to_tiles(&self) -> Rect {
        let x0 = (self.x / TILE) * TILE;
        let y0 = (self.y / TILE) * TILE;
        let x1 = (((self.x + self.w + TILE - 1) / TILE) * TILE).min(MAP_PX_W);
        let y1 = (((self.y + self.h + TILE - 1) / TILE) * TILE).min(MAP_PX_H);
        Rect { x: x0.max(0), y: y0.max(0), w: x1 - x0.max(0), h: y1 - y0.max(0) }
    }
}

/// Список брудних прямокутників кадру.
struct Dirty {
    r: [Rect; 48],
    n: usize,
}

impl Dirty {
    fn new() -> Self {
        Self { r: [Rect::EMPTY; 48], n: 0 }
    }

    fn add(&mut self, r: Rect) {
        let r = r.to_tiles();
        if r.is_empty() {
            return;
        }
        if self.n < self.r.len() {
            self.r[self.n] = r;
            self.n += 1;
        }
    }

    /// Злити перекривні прямокутники (менше вікон SPI — менше оверхеду).
    fn merge(&mut self) -> usize {
        let mut count = self.n;
        let mut i = 0;
        while i < count {
            let mut j = i + 1;
            while j < count {
                if self.r[i].intersects(self.r[j]) {
                    let u = self.r[i].union(self.r[j]);
                    if u.area() * 10 <= (self.r[i].area() + self.r[j].area()) * 13 {
                        self.r[i] = u;
                        self.r[j] = self.r[count - 1];
                        count -= 1;
                        continue;
                    }
                }
                j += 1;
            }
            i += 1;
        }
        count
    }
}

#[inline]
fn unit_rect(u: &Unit) -> Rect {
    // спрайт + 1 px рамки виділення + 3 px смужка HP зверху
    Rect { x: u.x - 1, y: u.y - 4, w: UNIT_SIZE + 2, h: UNIT_SIZE + 5 }
}

fn bld_rect(b: &Building) -> Rect {
    let (px, py) = b.px();
    Rect { x: px - 1, y: py - 4, w: b.w() * TILE + 2, h: b.h() * TILE + 5 }
}

// ── Малювання: теракта ───────────────────────────────────────────────────────

#[inline]
fn tile_hash(tx: i32, ty: i32) -> u32 {
    let mut h = (tx as u32).wrapping_mul(0x9E37_79B9) ^ (ty as u32).wrapping_mul(0x85EB_CA6B);
    h ^= h >> 13;
    h = h.wrapping_mul(0xC2B2_AE3D);
    h ^ (h >> 16)
}

fn draw_tile(tx: i32, ty: i32) {
    let t = MAP[ty as usize][tx as usize];
    let x = tx * TILE;
    let y = ty * TILE;
    let h = tile_hash(tx, ty);
    let s = TILE as u32;
    match t {
        T_ROCK => {
            fill(x, y, s, s, C_ROCK_LO);
            fill(x + 1, y + 1, s - 2, s - 2, C_ROCK);
            fill(x + 4, y + 4, 8, 8, C_ROCK_HI);
            fill(x + 6, y + 6, 4, 4, C_ROCK);
        }
        T_MINERAL => {
            fill(x, y, s, s, C_MINE);
            fill(x + 3, y + 2, 4, 5, C_MINE_HI);
            fill(x + 9, y + 5, 4, 5, C_MINE_MID);
            fill(x + 5, y + 10, 5, 4, C_MINE_HI);
        }
        T_SAND => {
            fill(x, y, s, s, C_SAND);
            for i in 0..4u32 {
                let px = ((h >> (i * 6)) & 0xF) as i32;
                let py = ((h >> (i * 6 + 3)) & 0xF) as i32;
                put_px(x + px, y + py, C_SAND_D);
            }
        }
        _ => {
            fill(x, y, s, s, C_GRASS);
            for i in 0..5u32 {
                let px = ((h >> (i * 6)) & 0xF) as i32;
                let py = ((h >> (i * 6 + 3)) & 0xF) as i32;
                put_px(x + px, y + py, C_GRASS_D);
            }
        }
    }
}

fn paint_terrain_rect(r: Rect) {
    let r = r.clip_map();
    if r.is_empty() {
        return;
    }
    let tx0 = r.x / TILE;
    let ty0 = r.y / TILE;
    let tx1 = (r.x + r.w - 1) / TILE;
    let ty1 = (r.y + r.h - 1) / TILE;
    for ty in ty0..=ty1 {
        for tx in tx0..=tx1 {
            draw_tile(tx, ty);
        }
    }
}

// ── Малювання: юніти, будівлі, ефекти ────────────────────────────────────────

#[inline]
fn side_colors(side: u8) -> (Rgb565, Rgb565) {
    if side == SIDE_BLUE { (C_BLUE, C_BLUE_D) } else { (C_RED, C_RED_D) }
}

fn hp_bar(x: i32, y: i32, w: i32, hp: u16, max: u16, color: Rgb565) {
    if w <= 0 || max == 0 {
        return;
    }
    fill(x, y, w as u32, 2, C_HP_BG);
    let full = (w as u32 * hp.min(max) as u32) / max as u32;
    fill(x, y, full, 2, color);
}

fn draw_unit(u: &Unit, selected: bool) {
    let (body, dark) = side_colors(u.side);
    let s = UNIT_SIZE;
    let su = s as u32;
    let x = u.x;
    let y = u.y;
    // корпус: рамка + заливка
    fill(x, y, su, su, dark);
    fill(x + 1, y + 1, su - 2, su - 2, body);
    match u.kind {
        U_WORKER => {
            // «кайло» — жовта смужка праворуч
            fill(x + 6, y + 3, 3, 2, ACCENT_GOLD);
            fill(x + 7, y + 5, 1, 3, dark);
        }
        U_GRUNT => {
            // «дуло» праворуч і ядро
            fill(x + 7, y + 4, 3, 2, dark);
            fill(x + 3, y + 3, 4, 4, C_CORE);
            fill(x + 4, y + 4, 2, 2, body);
        }
        _ => {
            // важкий: «траки» зверху/знизу + велике ядро
            fill(x, y + 1, su, 2, dark);
            fill(x, y + s - 3, su, 2, dark);
            fill(x + 3, y + 3, 4, 4, ACCENT_GOLD);
            fill(x + 4, y + 4, 2, 2, C_CORE);
        }
    }
    if u.flash > 0 {
        put_px(x + 9, y + 2, rgb(255, 255, 200));
    }
    let max = unit_stats(u.kind).hp;
    if u.hp < max {
        let color = if u.side == SIDE_BLUE { ACCENT_GREEN } else { ACCENT_RED };
        hp_bar(x, y - 3, UNIT_SIZE, u.hp, max, color);
    }
    if selected {
        stroke_rect(x - 1, y - 1, (s + 2) as u32, (s + 2) as u32, ACCENT_GOLD);
    }
}

fn draw_building(b: &Building, selected: bool) {
    let (px, py) = b.px();
    let w = b.w() * TILE;
    let h = b.h() * TILE;
    let (body, dark) = side_colors(b.side);
    let st = bld_stats(b.kind);
    match b.kind {
        B_HQ => {
            fill(px, py, w as u32, h as u32, dark);
            fill(px + 2, py + 2, (w - 4) as u32, (h - 4) as u32, body);
            fill(px + 8, py + 8, (w - 16) as u32, (h - 16) as u32, dark);
            // «вежа» й антена
            fill(px + w / 2 - 6, py + 4, 12, 12, body);
            fill(px + w / 2 - 2, py + 6, 4, 8, C_CORE);
            fill(px + 6, py + h - 12, 6, 6, ACCENT_CYAN);
            fill(px + w - 12, py + h - 12, 6, 6, ACCENT_CYAN);
        }
        _ => {
            fill(px, py, w as u32, h as u32, dark);
            fill(px + 2, py + 2, (w - 4) as u32, (h - 4) as u32, body);
            // дах смугами
            for k in 0..3 {
                fill(px + 2, py + 4 + k * 8, (w - 4) as u32, 2, dark);
            }
            fill(px + w / 2 - 3, py + h / 2 - 3, 6, 6, ACCENT_GOLD);
        }
    }
    if !b.done {
        // майданчик: пунктирна рамка + смужка прогресу
        stroke_rect(px, py, w as u32, h as u32, ACCENT_GOLD);
        let total = st.build_ticks.max(1) as u32;
        let full = (w as u32 * b.progress as u32) / total;
        fill(px, py + h - 3, w as u32, 3, C_HP_BG);
        fill(px, py + h - 3, full, 3, ACCENT_GOLD);
    } else if b.hp < st.hp {
        hp_bar(px, py - 3, w, b.hp, st.hp, if b.side == SIDE_BLUE { ACCENT_GREEN } else { ACCENT_RED });
    }
    if b.queue_kind != 255 {
        let total = unit_stats(b.queue_kind).train_ticks.max(1) as u32;
        let done = total.saturating_sub(b.queue_t as u32);
        let full = (w as u32 * done) / total;
        fill(px, py + h - 6, w as u32, 3, C_HP_BG);
        fill(px, py + h - 6, full, 3, ACCENT_CYAN);
    }
    if selected {
        stroke_rect(px - 1, py - 1, (w + 2) as u32, (h + 2) as u32, ACCENT_GOLD);
    }
}

fn draw_effect(x: i32, y: i32, kind: u8) {
    let c = if kind == FX_HIT { ACCENT_GOLD } else { rgb(255, 170, 80) };
    put_px(x, y - 2, c);
    put_px(x, y + 2, c);
    put_px(x - 2, y, c);
    put_px(x + 2, y, c);
}

// ── HUD ──────────────────────────────────────────────────────────────────────

struct Ui {
    sel: u32,
    sel_bld: u8,
    build_mode: bool,
    demo: bool,
    tap_ms: u32,
    tap_unit: u8,
}

impl Ui {
    fn new() -> Self {
        Self { sel: 0, sel_bld: 255, build_mode: false, demo: false, tap_ms: 0, tap_unit: 255 }
    }

    fn clear_sel(&mut self) {
        self.sel = 0;
        self.sel_bld = 255;
        self.build_mode = false;
    }
}

/// Доступні дії для поточної маски виділення.
fn actions(game: &Game, ui: &Ui) -> [u8; 2] {
    if ui.build_mode {
        return [ACT_CANCEL, ACT_NONE];
    }
    if ui.sel_bld != 255 {
        let b = game.buildings[ui.sel_bld as usize];
        if !b.alive || !b.done {
            return [ACT_NONE, ACT_NONE];
        }
        return match b.kind {
            B_HQ => [ACT_WORKER, ACT_NONE],
            _ => [ACT_GRUNT, ACT_HEAVY],
        };
    }
    if ui.sel == 0 {
        return [ACT_NONE, ACT_NONE];
    }
    let mut worker = false;
    let mut army = false;
    for i in 0..MAX_UNITS {
        if ui.sel & (1 << i) == 0 || !game.units[i].alive {
            continue;
        }
        if game.units[i].kind == U_WORKER {
            worker = true;
        } else {
            army = true;
        }
    }
    if worker {
        [ACT_BUILD, ACT_STOP]
    } else if army {
        [ACT_STOP, ACT_NONE]
    } else {
        [ACT_NONE, ACT_NONE]
    }
}

fn draw_hud(game: &Game, ui: &Ui) {
    fill(0, HUD_Y, SCREEN_W as u32, HUD_H as u32, rgb(24, 30, 44));
    fill(0, HUD_Y, SCREEN_W as u32, 1, ACCENT_GREEN);

    let mut a = [0u8; 10];
    let mut b = [0u8; 10];
    let mut c = [0u8; 10];
    let mins = game.minerals[SIDE_BLUE as usize];
    let supply = game.count_units(SIDE_BLUE);
    let secs = game.tick_n / 20;
    let mut l1 = LineBuf::<24>::new();
    let _ = write!(
        l1,
        "M{} {}/{}",
        u32_str(mins, &mut a),
        u32_str(supply as u32, &mut b),
        u32_str(SUPPLY_MAX as u32, &mut c)
    );
    text(3, HUD_Y + 2, l1.as_str(), FontKind::Small, ACCENT_GOLD);

    let mut t = LineBuf::<16>::new();
    let _ = write!(t, "{}:{:02}", secs / 60, secs % 60);
    text(78, HUD_Y + 2, t.as_str(), FontKind::Small, TEXT_MUTED);

    // другий рядок: що вибрано
    let mut l2 = LineBuf::<20>::new();
    if ui.build_mode {
        let _ = write!(l2, "PLACE BARRACKS");
    } else if ui.sel_bld != 255 {
        let bd = game.buildings[ui.sel_bld as usize];
        if !bd.alive {
            let _ = write!(l2, "-");
        } else if bd.kind == B_HQ {
            let _ = write!(l2, "HQ {}", bd.hp);
        } else if !bd.done {
            let _ = write!(l2, "BUILDING");
        } else {
            let _ = write!(l2, "BARRACKS {}", bd.hp);
        }
    } else if ui.sel != 0 {
        let mut n = 0u32;
        let mut kind = 255u8;
        let mut hp = 0u16;
        for i in 0..MAX_UNITS {
            if ui.sel & (1 << i) == 0 || !game.units[i].alive {
                continue;
            }
            if n == 0 {
                kind = game.units[i].kind;
                hp = game.units[i].hp;
            }
            n += 1;
        }
        let name = match kind {
            U_WORKER => "WRK",
            U_GRUNT => "GRT",
            _ => "HVY",
        };
        let mut nb = [0u8; 10];
        let _ = write!(l2, "{} x{} {}", name, u32_str(n, &mut nb), hp);
    } else {
        let _ = write!(l2, "TAP A UNIT");
    }
    text(3, HUD_Y + 17, l2.as_str(), FontKind::Small, TEXT_WHITE);

    let act = actions(game, ui);
    for (k, a) in act.iter().enumerate() {
        let r = if k == 0 { BTN_ACT1 } else { BTN_ACT2 };
        if *a == ACT_NONE {
            fill(r.x, r.y, r.w, r.h, rgb(24, 30, 44));
        } else {
            draw_btn(r, act_label(*a), FontKind::Small, BTN_STD_BG, TEXT_WHITE, BTN_STD_BORDER);
        }
    }
    draw_demo_btn(BTN_DEMO, ui.demo, FontKind::Small);
    draw_exit_btn(BTN_EXIT, FontKind::Small);
}

fn draw_overlay(game: &Game, best: u32) {
    let win = game.over == 1;
    let (c, title) = if win { (ACCENT_GREEN, "VICTORY") } else { (ACCENT_RED, "DEFEAT") };
    blend_fill(40, 64, 240, 96, BG_DEEP, 215);
    stroke_rect(40, 64, 240, 96, c);
    text_centered(160, 76, title, FontKind::Bold, c);
    let secs = game.over_tick / 20;
    let mut l = LineBuf::<20>::new();
    let _ = write!(l, "TIME {}:{:02}", secs / 60, secs % 60);
    text_centered(160, 100, l.as_str(), FontKind::Small, TEXT_WHITE);
    if best > 0 {
        let bs = best / 20;
        let mut l2 = LineBuf::<20>::new();
        let _ = write!(l2, "BEST {}:{:02}", bs / 60, bs % 60);
        text_centered(160, 116, l2.as_str(), FontKind::Small, ACCENT_GOLD);
    }
    text_centered(160, 136, "EXIT - MENU", FontKind::Small, TEXT_MUTED);
}

// ── Повний кадр ──────────────────────────────────────────────────────────────

fn paint_scene(game: &Game, ui: &Ui) {
    paint_terrain_rect(Rect { x: 0, y: 0, w: MAP_PX_W, h: MAP_PX_H });
    for (bi, b) in game.buildings.iter().enumerate() {
        if b.alive {
            draw_building(b, ui.sel_bld as usize == bi);
        }
    }
    for (i, u) in game.units.iter().enumerate() {
        if u.alive {
            draw_unit(u, ui.sel & (1 << i) != 0);
        }
    }
    for e in game.effects.iter() {
        if e.alive {
            draw_effect(e.x, e.y, e.kind);
        }
    }
    draw_hud(game, ui);
}

fn present_all(hw: &mut Board) {
    hw.present_all();
}

// ── Дії гравця ───────────────────────────────────────────────────────────────

fn first_selected_worker(game: &Game, ui: &Ui) -> usize {
    for i in 0..MAX_UNITS {
        if ui.sel & (1 << i) != 0 && game.units[i].alive && game.units[i].kind == U_WORKER {
            return i;
        }
    }
    MAX_UNITS
}

fn do_action(game: &mut Game, ui: &mut Ui, a: u8) {
    match a {
        ACT_BUILD => {
            if first_selected_worker(game, ui) < MAX_UNITS {
                ui.build_mode = true;
            }
        }
        ACT_CANCEL => ui.build_mode = false,
        ACT_STOP => {
            game.stop(ui.sel);
            ui.build_mode = false;
        }
        ACT_WORKER | ACT_GRUNT | ACT_HEAVY => {
            let kind = match a {
                ACT_WORKER => U_WORKER,
                ACT_GRUNT => U_GRUNT,
                _ => U_HEAVY,
            };
            if ui.sel_bld != 255 && game.train(ui.sel_bld as usize, kind) {
                println!("SWARM: найм {}", kind);
            }
        }
        _ => {}
    }
}

/// Тап по карті: вибір, наказ або розміщення будівлі.
fn on_tap_map(game: &mut Game, ui: &mut Ui, x: i32, y: i32, now: u32) {
    if ui.build_mode {
        let worker = first_selected_worker(game, ui);
        if worker >= MAX_UNITS {
            ui.build_mode = false;
            return;
        }
        let tx = x / TILE - 1;
        let ty = y / TILE - 1;
        if game.place(SIDE_BLUE, B_BARRACKS, tx, ty, worker) {
            println!("SWARM: барак замовлено ({},{})", tx, ty);
            ui.build_mode = false;
            ui.clear_sel();
        } else {
            println!("SWARM: тут не можна будувати ({},{})", tx, ty);
        }
        return;
    }

    // своя істота → вибір (подвійний тап — усі того ж типу)
    if let Some(i) = game.unit_at(x, y) {
        if game.units[i].side == SIDE_BLUE {
            let kind = game.units[i].kind;
            if ui.tap_unit == i as u8 && now.wrapping_sub(ui.tap_ms) < 400 {
                ui.sel = game.mask_of_kind(SIDE_BLUE, kind);
                ui.sel_bld = 255;
                println!("SWARM: вибрано всіх типу {}", kind);
            } else {
                ui.sel = 1 << i;
                ui.sel_bld = 255;
            }
            ui.tap_unit = i as u8;
            ui.tap_ms = now;
            return;
        }
    }
    // своя будівля → вибір
    if let Some(bi) = game.building_at(x, y) {
        if game.buildings[bi].side == SIDE_BLUE {
            ui.sel_bld = bi as u8;
            ui.sel = 0;
            return;
        }
    }
    // наказ
    if ui.sel != 0 {
        let attacked = game.order_at(ui.sel, x, y, SIDE_BLUE);
        println!("SWARM: наказ {} ({},{})", if attacked { "атака" } else { "рух" }, x / TILE, y / TILE);
    } else {
        ui.clear_sel();
    }
}

// ── Точка входу ──────────────────────────────────────────────────────────────

#[esp_hal::main]
fn main() -> ! {
    let mut hw = Board::init();
    let (run, failed) = selftest();
    println!("SWARM: selftest {}/{} failed={}", run - failed, run, failed);

    let mut game = Game::new();
    let mut ui = Ui::new();
    ui.demo = hw.start_in_demo(cfg!(feature = "demo"));
    let mut best = hw.load_best(SAVE_FILE);
    let mut best_saved = false;

    let t0 = Instant::now();
    paint_scene(&game, &ui);
    present_all(&mut hw);
    println!(
        "SWARM: старт, повний кадр = {} ms, demo={}, best={}",
        elapsed_ms(t0),
        ui.demo,
        best
    );

    let t_base = Instant::now();
    let mut sim_at: u32 = 0;
    let mut last_touch: u32 = 0;
    let mut last_hud: u32 = 0;
    let mut hud_state: HudState = HUD_STATE_INIT;
    let mut next_log: u32 = 200;

    let mut was_pressed = false;
    let mut press_pt = Point::zero();
    let mut last_pt = Point::zero();
    let mut drag = false;

    let mut prev_u = [Rect::EMPTY; MAX_UNITS];
    let mut prev_b = [Rect::EMPTY; MAX_BUILDINGS];
    let mut prev_fx = [Rect::EMPTY; MAX_EFFECTS];
    let mut bld_state = [(0u16, 0u16, 0u8, false); MAX_BUILDINGS];

    let mut frame: u32 = 0;
    let mut win_frames: u32 = 0;
    let mut win_ms: u32 = 0;
    let mut win_rects: u32 = 0;
    let mut t_win = Instant::now();
    let mut over_drawn = false;

    loop {
        let now = elapsed_ms(t_base);

        // ── UART
        while let Some(ch) = hw.poll_uart_byte() {
            match ch {
                b'X' | b'x' | b'Q' | b'q' => {
                    if game.over == 1 && !best_saved && (best == 0 || game.over_tick < best) {
                        best = game.over_tick;
                        hw.save_best(SAVE_FILE, best);
                    }
                    println!("SWARM: вихід у GameLauncher");
                    hw.exit_to_launcher();
                }
                b'D' | b'd' => {
                    ui.demo = !ui.demo;
                    println!("SWARM: demo={}", ui.demo);
                }
                b'S' | b's' => {
                    println!(
                        "SWARM: t={}s M={} units={} army={} blds={} over={}",
                        game.tick_n / 20,
                        game.minerals[SIDE_BLUE as usize],
                        game.count_units(SIDE_BLUE),
                        game.army_count(SIDE_BLUE),
                        game.buildings.iter().filter(|b| b.alive).count(),
                        game.over
                    );
                }
                b'T' | b't' => {
                    if let Some(hq) = game.hq(SIDE_BLUE) {
                        let ok = game.train(hq, U_WORKER);
                        println!("SWARM: найм робітника -> {}", ok);
                    }
                }
                b'B' | b'b' => {
                    let w = (0..MAX_UNITS)
                        .find(|&i| game.units[i].alive && game.units[i].side == SIDE_BLUE && game.units[i].kind == U_WORKER)
                        .unwrap_or(MAX_UNITS);
                    let mut placed = false;
                    for (tx, ty) in [(5i32, 7i32), (7, 7), (7, 10), (5, 10)] {
                        if w < MAX_UNITS && game.place(SIDE_BLUE, B_BARRACKS, tx, ty, w) {
                            println!("SWARM: барак на ({},{})", tx, ty);
                            placed = true;
                            break;
                        }
                    }
                    if !placed {
                        println!("SWARM: не вдалося поставити барак");
                    }
                }
                b'A' | b'a' => {
                    if let Some(hq) = game.hq(SIDE_RED) {
                        for i in 0..MAX_UNITS {
                            if game.units[i].alive && game.units[i].side == SIDE_BLUE && game.units[i].kind != U_WORKER {
                                game.units[i].order = O_ATTACK_MOVE;
                                game.set_path_to_bld(i, hq);
                            }
                        }
                        println!("SWARM: усі війська — атака");
                    }
                }
                b'Y' | b'y' => {
                    // найм ґранта в першому власному бараці (для перевірки без тачу)
                    let br = (0..MAX_BUILDINGS).find(|&i| {
                        let b = game.buildings[i];
                        b.alive && b.done && b.side == SIDE_BLUE && b.kind == B_BARRACKS
                    });
                    match br {
                        Some(bi) => println!("SWARM: найм ґранта -> {}", game.train(bi, U_GRUNT)),
                        None => println!("SWARM: немає барака"),
                    }
                }
                b'G' | b'g' => {
                    for i in 0..MAX_UNITS {
                        if game.units[i].alive && game.units[i].side == SIDE_BLUE && game.units[i].kind == U_WORKER {
                            game.order_gather(i);
                        }
                    }
                    println!("SWARM: робітники на родовище");
                }
                b'1' => {
                    ui.sel = game.mask_of_kind(SIDE_BLUE, U_WORKER);
                    ui.sel_bld = 255;
                }
                b'2' => {
                    ui.sel = game.mask_of_kind(SIDE_BLUE, U_GRUNT);
                    ui.sel_bld = 255;
                }
                b'3' => {
                    ui.sel = game.mask_of_kind(SIDE_BLUE, U_HEAVY);
                    ui.sel_bld = 255;
                }
                _ => {}
            }
        }

        // ── Симуляція 20 Гц
        while now >= sim_at + TICK_MS && game.over == 0 {
            sim_at += TICK_MS;
            game.tick();
            if game.tick_n % 4 == 0 {
                if ui.demo {
                    game.ai_step(SIDE_BLUE);
                }
                game.ai_step(SIDE_RED);
            }
            if game.tick_n >= next_log {
                next_log += 200;
                println!(
                    "SWARM: t={}s M={} units B={} R={} army B={} R={} over={}",
                    game.tick_n / 20,
                    game.minerals[SIDE_BLUE as usize],
                    game.count_units(SIDE_BLUE),
                    game.count_units(SIDE_RED),
                    game.army_count(SIDE_BLUE),
                    game.army_count(SIDE_RED),
                    game.over
                );
            }
        }

        // ── Тач (не кожен кадр: XPT2046 на тій самій шині, що й дисплей)
        if now.wrapping_sub(last_touch) >= TOUCH_MS {
            last_touch = now;
            let mut pressed = false;
            let mut pt = Point::zero();
            if hw.touch.pressed() {
                let p = hw.touch.point();
                if hw.touch.pressed() {
                    pressed = true;
                    pt = p;
                }
            }
            if pressed {
                if !was_pressed {
                    press_pt = pt;
                    last_pt = pt;
                    drag = false;
                } else {
                    last_pt = pt;
                    if (pt.x - press_pt.x).abs() > 8 || (pt.y - press_pt.y).abs() > 8 {
                        drag = true;
                    }
                }
            } else if was_pressed {
                let is_tap = !drag
                    && (last_pt.x - press_pt.x).abs() < 12
                    && (last_pt.y - press_pt.y).abs() < 12;
                if is_tap {
                    if BTN_EXIT.contains(press_pt) {
                        if game.over == 1 && !best_saved && (best == 0 || game.over_tick < best) {
                            best = game.over_tick;
                            hw.save_best(SAVE_FILE, best);
                        }
                        println!("SWARM: вихід у GameLauncher (тач)");
                        hw.exit_to_launcher();
                    } else if BTN_DEMO.contains(press_pt) {
                        ui.demo = !ui.demo;
                    } else if game.over == 0 {
                        if BTN_ACT1.contains(press_pt) {
                            let a = actions(&game, &ui)[0];
                            do_action(&mut game, &mut ui, a);
                        } else if BTN_ACT2.contains(press_pt) {
                            let a = actions(&game, &ui)[1];
                            do_action(&mut game, &mut ui, a);
                        } else if press_pt.y < HUD_Y && !ui.demo {
                            on_tap_map(&mut game, &mut ui, press_pt.x, press_pt.y, now);
                        }
                    }
                } else if game.over == 0 && !ui.demo && press_pt.y < HUD_Y && !ui.build_mode {
                    ui.sel = game.units_in_rect(SIDE_BLUE, press_pt.x, press_pt.y, last_pt.x, last_pt.y);
                    ui.sel_bld = 255;
                    println!("SWARM: рамка — вибрано {}", Game::mask_count(ui.sel));
                }
                // після відпускання — повний перемальовка (рамки, HUD)
                hud_state = HUD_STATE_INIT;
            }
            was_pressed = pressed;
        }

        // ── Кінець партії: один повний кадр з оверлеєм
        if game.over != 0 {
            if !over_drawn {
                over_drawn = true;
                paint_scene(&game, &ui);
                draw_overlay(&game, best);
                present_all(&mut hw);
                if game.over == 1 && !best_saved && (best == 0 || game.over_tick < best) {
                    best = game.over_tick;
                    hw.save_best(SAVE_FILE, best);
                    best_saved = true;
                }
                println!("SWARM: партія завершена over={} t={}s best={}", game.over, game.over_tick / 20, best);
            }
            hw.delay.delay_millis(20);
            continue;
        }

        // ── Кадр: dirty-прямокутники
        let t_frame = Instant::now();
        ui.sel &= alive_mask(&game);

        let mut dirty = Dirty::new();
        for i in 0..MAX_UNITS {
            let u = &game.units[i];
            let r = if u.alive { unit_rect(u) } else { Rect::EMPTY };
            dirty.add(r.union(prev_u[i]));
            prev_u[i] = r;
        }
        for bi in 0..MAX_BUILDINGS {
            let b = game.buildings[bi];
            let r = if b.alive { bld_rect(&b) } else { Rect::EMPTY };
            let st = (b.hp, b.progress, b.queue_kind, b.done);
            if st != bld_state[bi] || r != prev_b[bi] {
                dirty.add(r.union(prev_b[bi]));
                bld_state[bi] = st;
            }
            prev_b[bi] = r;
        }
        for i in 0..MAX_EFFECTS {
            let e = game.effects[i];
            let r = if e.alive { Rect { x: e.x - 2, y: e.y - 2, w: 5, h: 5 } } else { Rect::EMPTY };
            dirty.add(r.union(prev_fx[i]));
            prev_fx[i] = r;
        }

        let n = dirty.merge();
        for k in 0..n {
            let r = dirty.r[k];
            paint_terrain_rect(r);
            for b in game.buildings.iter() {
                if b.alive && bld_rect(b).intersects(r) {
                    draw_building(b, false);
                }
            }
            if ui.sel_bld != 255 {
                let b = game.buildings[ui.sel_bld as usize];
                if b.alive && bld_rect(&b).intersects(r) {
                    draw_building(&b, true);
                }
            }
            for (i, u) in game.units.iter().enumerate() {
                if u.alive && unit_rect(u).intersects(r) {
                    draw_unit(u, ui.sel & (1 << i) != 0);
                }
            }
            for e in game.effects.iter() {
                if e.alive && (Rect { x: e.x - 2, y: e.y - 2, w: 5, h: 5 }).intersects(r) {
                    draw_effect(e.x, e.y, e.kind);
                }
            }
        }
        for k in 0..n {
            let r = dirty.r[k];
            hw.present_rect(r.x as u16, r.y as u16, r.w as u16, r.h as u16);
        }

        // ── HUD (тільки коли змінився вміст)
        let key = (
            game.minerals[SIDE_BLUE as usize],
            game.count_units(SIDE_BLUE),
            game.tick_n / 20,
            ui.sel,
            ui.sel_bld,
            ui.build_mode,
            ui.demo,
            actions(&game, &ui),
        );
        if key != hud_state && now.wrapping_sub(last_hud) >= HUD_MS {
            hud_state = key;
            last_hud = now;
            draw_hud(&game, &ui);
            hw.present_rect(0, HUD_Y as u16, SCREEN_W as u16, HUD_H as u16);
        }

        let frame_ms = elapsed_ms(t_frame);
        frame += 1;
        win_frames += 1;
        win_ms += frame_ms;
        win_rects += n as u32;
        if frame % STATS_FRAMES == 0 {
            let total = elapsed_ms(t_win);
            let fps = if total > 0 { (win_frames * 1000) / total } else { 0 };
            println!(
                "SWARM: fps={} frame={}ms avg={}ms rects={} units={}",
                fps,
                frame_ms,
                win_ms / win_frames.max(1),
                win_rects / win_frames.max(1),
                game.count_units(SIDE_BLUE) + game.count_units(SIDE_RED)
            );
            win_frames = 0;
            win_ms = 0;
            win_rects = 0;
            t_win = Instant::now();
        }

        hw.delay.delay_millis(2);
    }
}

fn alive_mask(game: &Game) -> u32 {
    let mut m = 0u32;
    for i in 0..MAX_UNITS {
        if game.units[i].alive {
            m |= 1 << i;
        }
    }
    m
}
