//! `GARDEN DEF` — tower defense «грядка проти зомбі» для NM-CYD-C5 (ESP32-C5).
//!
//! Стиль — як у Plants vs Zombies: 5 рядів, сонце як ресурс, пакети насіння,
//! зомбі йдуть з правого краю, косарка в кожному ряду — останній шанс.
//!
//! Рендер — dirty-rect: перемальовуються лише прямокутники, які змінилися
//! (зомбі, горошини, сонце, ефекти), HUD — окремо і не частіше 10 Гц.
//!
//! Керування (один палець):
//! - тап по пакету насіння (верхній ряд) — вибрати рослину;
//! - тап по клітинці грядки — посадити; тап по кульці сонця — зібрати;
//! - `DEMO` / `EXIT` — автопілот і вихід у GameLauncher.
//!
//! UART 115200 (перевірка без пальця): `X`/`Q` — вихід, `D` — демо, `S` — статус,
//! `G` — зібрати все сонце, `Z` — додати зомбі, `W` — наступна хвиля негайно,
//! `1`..`5` — вибрати пакет, `P<col><row>` — посадити (напр. `P20`), `?` — знімок екрана.

#![no_std]
#![no_main]

use core::fmt::Write;
use cyd_c5_common::{
    Board, SCREEN_H, SCREEN_W, put_px,
    ui::{
        ACCENT_CYAN, ACCENT_GOLD, ACCENT_GREEN, ACCENT_RED, BG_DEEP, BTN_STD_BORDER,
        BtnRect, FontKind, LineBuf, TEXT_MUTED, TEXT_WHITE, blend_fill, draw_demo_btn,
        draw_exit_btn, elapsed_ms, fill, rgb, stroke_rect, text, text_centered, u32_str,
    },
};
use cyd_game_td::game::*;
use embedded_graphics::{pixelcolor::Rgb565, prelude::Point};
use esp_hal::time::Instant;
use esp_println::println;

#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    println!("GARDEN PANIC: {:?}", info);
    loop {}
}

esp_bootloader_esp_idf::esp_app_desc!(
    "0.1.0",
    "cyd-game-td",
    "00:00:00",
    "2026-11-14",
    "5.5.6",
    64 * 1024,
    0,
    u16::MAX,
    0
);

const SAVE_FILE: &str = "GARDEN.SAV";

// ── Розкладка екрана ─────────────────────────────────────────────────────────

const PLAY_X: i32 = 28;
const PLAY_Y: i32 = 56;
const HUD_H: i32 = 22;
const SEED_Y: i32 = 22;
const SEED_W: u32 = 60;
const SEED_H: u32 = 32;
const SEED_X0: i32 = 4;
const SEED_DX: i32 = 63;

const BTN_DEMO: BtnRect = BtnRect::new(216, 2, 44, 18);
const BTN_EXIT: BtnRect = BtnRect::new(264, 2, 48, 18);

// ── Палітра ──────────────────────────────────────────────────────────────────

const C_BAR: Rgb565 = rgb(24, 30, 44);
const C_BAR_LINE: Rgb565 = rgb(46, 58, 82);
const C_LAWN_A: Rgb565 = rgb(78, 146, 64);
const C_LAWN_B: Rgb565 = rgb(66, 128, 56);
const C_TUFT: Rgb565 = rgb(52, 108, 44);
const C_SOIL: Rgb565 = rgb(104, 78, 52);
const C_SOIL_D: Rgb565 = rgb(78, 56, 38);
const C_HEDGE: Rgb565 = rgb(40, 82, 40);
const C_HEDGE_D: Rgb565 = rgb(28, 60, 30);
const C_SHADOW: Rgb565 = rgb(44, 92, 38);

const C_STEM: Rgb565 = rgb(64, 168, 68);
const C_STEM_D: Rgb565 = rgb(36, 108, 44);
const C_LEAF: Rgb565 = rgb(96, 200, 84);
const C_GOLD: Rgb565 = rgb(252, 208, 64);
const C_GOLD_D: Rgb565 = rgb(196, 148, 32);
const C_BROWN: Rgb565 = rgb(120, 78, 36);
const C_NUT: Rgb565 = rgb(182, 126, 64);
const C_NUT_D: Rgb565 = rgb(132, 84, 40);
const C_ICE: Rgb565 = rgb(126, 220, 250);
const C_ICE_D: Rgb565 = rgb(64, 156, 200);
const C_CHERRY: Rgb565 = rgb(226, 56, 62);
const C_CHERRY_D: Rgb565 = rgb(158, 26, 40);

const C_SKIN: Rgb565 = rgb(152, 176, 142);
const C_SKIN_D: Rgb565 = rgb(104, 128, 100);
const C_SHIRT: Rgb565 = rgb(92, 104, 132);
const C_SHIRT_D: Rgb565 = rgb(58, 66, 90);
const C_TROUSER: Rgb565 = rgb(58, 52, 60);
const C_CONE: Rgb565 = rgb(232, 148, 48);
const C_CONE_D: Rgb565 = rgb(168, 96, 28);
const C_METAL: Rgb565 = rgb(196, 202, 214);
const C_METAL_D: Rgb565 = rgb(128, 136, 150);
const C_DARK: Rgb565 = rgb(20, 18, 24);
const C_MOWER: Rgb565 = rgb(212, 68, 60);
const C_MOWER_D: Rgb565 = rgb(140, 36, 34);

const C_HP_BG: Rgb565 = rgb(30, 50, 30);

// ── Дрібні примітиви ─────────────────────────────────────────────────────────

fn disc(cx: i32, cy: i32, r: i32, c: Rgb565) {
    disc_clip(cx, cy, r, c, 0);
}

/// Коло з відсіканням зверху (`min_y`) — щоб сонце, яке ще падає, не лізло в HUD.
fn disc_clip(cx: i32, cy: i32, r: i32, c: Rgb565, min_y: i32) {
    let mut dy = -r;
    while dy <= r {
        let yy = cy + dy;
        if yy >= min_y {
            let mut hw = 0i32;
            while (hw + 1) * (hw + 1) + dy * dy <= r * r {
                hw += 1;
            }
            fill(cx - hw, yy, (hw * 2 + 1) as u32, 1, c);
        }
        dy += 1;
    }
}

fn hp_bar(x: i32, y: i32, w: i32, hp: u16, max: u16, color: Rgb565) {
    if w <= 0 || max == 0 {
        return;
    }
    fill(x, y, w as u32, 2, C_HP_BG);
    let full = (w as u32 * hp.min(max) as u32) / max as u32;
    fill(x, y, full, 2, color);
}

#[inline]
fn plant_px(col: i32, row: i32) -> (i32, i32) {
    (PLAY_X + col * CELL_W, PLAY_Y + row * CELL_H)
}

// ── Прямокутники (dirty-rect) ────────────────────────────────────────────────

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

    /// Кліп до смуги грядки (без HUD).
    fn clip_field(&self) -> Rect {
        let x0 = self.x.max(0);
        let y0 = self.y.max(PLAY_Y);
        let x1 = (self.x + self.w).min(SCREEN_W);
        let y1 = (self.y + self.h).min(PLAY_Y + PLAY_H);
        if x1 <= x0 || y1 <= y0 {
            return Rect::EMPTY;
        }
        Rect { x: x0, y: y0, w: x1 - x0, h: y1 - y0 }
    }
}

struct Dirty {
    r: [Rect; 96],
    n: usize,
}

impl Dirty {
    fn new() -> Self {
        Self { r: [Rect::EMPTY; 96], n: 0 }
    }

    fn add(&mut self, r: Rect) {
        let r = r.clip_field();
        if r.is_empty() {
            return;
        }
        if self.n < self.r.len() {
            self.r[self.n] = r;
            self.n += 1;
        } else {
            // переповнення: зливаємо з останнім, щоб не лишити немитих пікселів
            let last = self.n - 1;
            self.r[last] = self.r[last].union(r);
        }
    }

    fn merge(&mut self) -> usize {
        let mut count = self.n;
        let mut i = 0;
        while i < count {
            let mut j = i + 1;
            while j < count {
                if self.r[i].intersects(self.r[j]) {
                    let u = self.r[i].union(self.r[j]);
                    if u.area() * 10 <= (self.r[i].area() + self.r[j].area()) * 14 {
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
fn zombie_rect(z: &Zombie) -> Rect {
    let px = PLAY_X + z.xm * CELL_W / XM;
    let py = PLAY_Y + z.row as i32 * CELL_H;
    Rect { x: px - 13, y: py + 1, w: 28, h: CELL_H }
}

#[inline]
fn pea_rect(p: &Pea) -> Rect {
    let x = PLAY_X + p.xm * CELL_W / XM;
    Rect { x: x - 4, y: PLAY_Y + p.row as i32 * CELL_H + CELL_H / 2 - 4, w: 9, h: 9 }
}

#[inline]
fn sun_rect(s: &Sun) -> Rect {
    Rect { x: PLAY_X + s.x - 12, y: PLAY_Y + s.y - 12, w: 25, h: 25 }
}

#[inline]
fn fx_rect(f: &Fx) -> Rect {
    Rect { x: PLAY_X + f.x - 7, y: PLAY_Y + f.y - 7, w: 15, h: 15 }
}

#[inline]
fn mower_rect(lane: usize, xm: i32) -> Rect {
    let x = PLAY_X + xm * CELL_W / XM;
    Rect { x: x - 13, y: PLAY_Y + lane as i32 * CELL_H + 9, w: 27, h: 22 }
}

// ── Фон грядки ───────────────────────────────────────────────────────────────

#[inline]
fn cell_hash(col: i32, row: i32) -> u32 {
    let mut h = (col as u32).wrapping_mul(0x9E37_79B9) ^ (row as u32).wrapping_mul(0x85EB_CA6B);
    h ^= h >> 13;
    h = h.wrapping_mul(0xC2B2_AE3D);
    h ^ (h >> 16)
}

fn paint_lawn_rect(game: &Game, r: Rect) {
    let r = r.clip_field();
    if r.is_empty() {
        return;
    }
    // ряди грядки
    for lane in 0..LANES {
        let y0 = PLAY_Y + lane as i32 * CELL_H;
        if y0 + CELL_H <= r.y || y0 >= r.y + r.h {
            continue;
        }
        let c = if lane % 2 == 0 { C_LAWN_A } else { C_LAWN_B };
        let ty0 = y0.max(r.y);
        let ty1 = (y0 + CELL_H).min(r.y + r.h);
        let tx0 = r.x.max(PLAY_X);
        let tx1 = (r.x + r.w).min(PLAY_X + PLAY_W);
        if tx1 <= tx0 {
            continue;
        }
        fill(tx0, ty0, (tx1 - tx0) as u32, (ty1 - ty0) as u32, c);
        // травинки
        let col0 = (tx0 - PLAY_X) / CELL_W;
        let col1 = (tx1 - PLAY_X) / CELL_W;
        for col in col0..=col1 {
            let h = cell_hash(col, lane as i32);
            let bx = PLAY_X + col * CELL_W;
            for k in 0..3u32 {
                let dx = ((h >> (k * 7)) & 0x1F) as i32;
                let dy = ((h >> (k * 7 + 5)) & 0x1F) as i32;
                put_px(bx + dx, y0 + dy, C_TUFT);
            }
        }
    }

    // ліва смуга: ґанок/земля з косарками
    let sx0 = r.x.max(0);
    let sx1 = (r.x + r.w).min(PLAY_X);
    if sx1 > sx0 {
        fill(sx0, r.y, (sx1 - sx0) as u32, r.h as u32, C_SOIL);
        let mut y = PLAY_Y;
        while y < PLAY_Y + PLAY_H {
            if y >= r.y && y < r.y + r.h {
                fill(sx0, y, (sx1 - sx0) as u32, 1, C_SOIL_D);
            }
            y += 24;
        }
    }

    // права смуга: живоплот
    let hx0 = r.x.max(PLAY_X + PLAY_W);
    let hx1 = (r.x + r.w).min(SCREEN_W);
    if hx1 > hx0 {
        fill(hx0, r.y, (hx1 - hx0) as u32, r.h as u32, C_HEDGE);
        let mut y = PLAY_Y;
        while y < PLAY_Y + PLAY_H {
            if y >= r.y && y < r.y + r.h {
                fill(hx0, y, (hx1 - hx0) as u32, 1, C_HEDGE_D);
            }
            y += 8;
        }
    }

    // косарки
    for lane in 0..LANES {
        let Some(xm) = mower_draw_xm(game, lane) else {
            continue;
        };
        if mower_rect(lane, xm).intersects(r) {
            draw_mower(lane, xm);
        }
    }
}

/// Де малювати косарку: `None` — вона вже поїхала, `Some(-60)` — стоїть на парковці.
fn mower_draw_xm(game: &Game, lane: usize) -> Option<i32> {
    match game.mower[lane] {
        MOWER_READY => Some(-350),
        MOWER_RUN => Some(game.mower_xm[lane]),
        _ => None,
    }
}

fn draw_mower(lane: usize, xm: i32) {
    let x = PLAY_X + xm * CELL_W / XM - 11;
    let y = PLAY_Y + lane as i32 * CELL_H + 12;
    fill(x + 1, y + 8, 20, 6, C_MOWER);
    fill(x + 3, y + 4, 14, 5, C_MOWER_D);
    fill(x + 5, y + 2, 5, 3, rgb(160, 168, 180));
    fill(x + 2, y + 13, 6, 4, C_DARK);
    fill(x + 14, y + 13, 6, 4, C_DARK);
    fill(x + 19, y + 2, 2, 8, rgb(150, 156, 168));
}

// ── Рослини ──────────────────────────────────────────────────────────────────

fn draw_plant(p: &Plant) {
    let (x0, y0) = plant_px(p.col as i32, p.row as i32);
    let cx = x0 + CELL_W / 2;
    let shadow = if p.flash > 0 { rgb(180, 60, 60) } else { C_SHADOW };
    fill(cx - 9, y0 + CELL_H - 5, 18, 2, shadow);

    match p.kind {
        P_SUNFLOWER => {
            // стебло + два листки
            fill(cx - 1, y0 + 18, 3, 12, C_STEM_D);
            fill(cx - 8, y0 + 22, 6, 3, C_LEAF);
            fill(cx + 3, y0 + 20, 7, 3, C_LEAF);
            fill(cx - 8, y0 + 24, 6, 2, C_STEM_D);
            fill(cx + 3, y0 + 22, 7, 2, C_STEM_D);
            // пелюстки
            disc(cx, y0 + 13, 9, C_GOLD_D);
            disc(cx, y0 + 13, 8, C_GOLD);
            // насіння
            disc(cx, y0 + 13, 4, C_BROWN);
            put_px(cx - 2, y0 + 12, C_DARK);
            put_px(cx + 2, y0 + 12, C_DARK);
            put_px(cx, y0 + 15, C_DARK);
            if p.t > 380 {
                // «наливається» перед видачею сонця
                put_px(cx - 6, y0 + 8, TEXT_WHITE);
            }
        }
        P_PEASHOOTER | P_ICEPEA => {
            let head = if p.kind == P_ICEPEA { C_ICE } else { C_STEM };
            let head_d = if p.kind == P_ICEPEA { C_ICE_D } else { C_STEM_D };
            fill(cx - 1, y0 + 16, 3, 14, C_STEM_D);
            fill(cx - 9, y0 + 23, 7, 3, C_LEAF);
            fill(cx + 3, y0 + 25, 6, 3, C_LEAF);
            // голова
            disc(cx, y0 + 13, 8, head_d);
            disc(cx, y0 + 12, 7, head);
            // дуло праворуч
            fill(cx + 7, y0 + 9, 9, 7, head_d);
            fill(cx + 8, y0 + 10, 8, 5, head);
            fill(cx + 15, y0 + 11, 2, 3, C_DARK);
            // око
            fill(cx + 1, y0 + 9, 2, 3, C_DARK);
            if p.kind == P_ICEPEA {
                put_px(cx - 3, y0 + 9, TEXT_WHITE);
            }
        }
        P_WALLNUT => {
            disc(cx, y0 + 19, 12, C_NUT_D);
            disc(cx, y0 + 18, 11, C_NUT);
            fill(cx - 7, y0 + 16, 3, 4, C_DARK);
            fill(cx + 4, y0 + 16, 3, 4, C_DARK);
            fill(cx - 4, y0 + 25, 8, 2, C_NUT_D);
            // тріщини за станом HP
            let st = plant_stats(p.kind);
            if p.hp * 2 < st.hp {
                fill(cx + 2, y0 + 10, 2, 8, C_NUT_D);
            }
            if p.hp * 4 < st.hp {
                fill(cx - 6, y0 + 22, 6, 2, C_NUT_D);
            }
        }
        _ => {
            // вишнева бомба
            let pulse = if p.t > CHERRY_FUSE - 3 { C_GOLD } else { C_CHERRY };
            disc(cx - 5, y0 + 22, 7, C_CHERRY_D);
            disc(cx + 5, y0 + 19, 7, C_CHERRY_D);
            disc(cx - 5, y0 + 21, 6, pulse);
            disc(cx + 5, y0 + 18, 6, pulse);
            fill(cx - 1, y0 + 8, 2, 8, C_STEM_D);
            fill(cx + 1, y0 + 6, 5, 2, C_LEAF);
            put_px(cx - 6, y0 + 19, TEXT_WHITE);
            put_px(cx + 4, y0 + 16, TEXT_WHITE);
        }
    }

    let st = plant_stats(p.kind);
    if p.hp < st.hp && p.kind != P_CHERRY {
        hp_bar(x0 + 4, y0 + 1, CELL_W - 8, p.hp, st.hp, ACCENT_GREEN);
    }
}

/// Іконка рослини 20×20 для пакета насіння (лівий верхній кут `(x, y)`).
fn draw_plant_icon(kind: u8, x: i32, y: i32) {
    let cx = x + 10;
    match kind {
        P_SUNFLOWER => {
            fill(cx - 1, y + 12, 2, 8, C_STEM_D);
            disc(cx, y + 8, 6, C_GOLD_D);
            disc(cx, y + 8, 5, C_GOLD);
            disc(cx, y + 8, 2, C_BROWN);
        }
        P_PEASHOOTER => {
            fill(cx - 1, y + 12, 2, 8, C_STEM_D);
            disc(cx - 1, y + 8, 5, C_STEM_D);
            disc(cx - 1, y + 7, 4, C_STEM);
            fill(cx + 3, y + 6, 6, 4, C_STEM_D);
            put_px(cx + 8, y + 7, C_DARK);
        }
        P_WALLNUT => {
            disc(cx, y + 10, 8, C_NUT_D);
            disc(cx, y + 10, 7, C_NUT);
            fill(cx - 5, y + 8, 2, 2, C_DARK);
            fill(cx + 3, y + 8, 2, 2, C_DARK);
        }
        P_ICEPEA => {
            fill(cx - 1, y + 12, 2, 8, C_STEM_D);
            disc(cx - 1, y + 8, 5, C_ICE_D);
            disc(cx - 1, y + 7, 4, C_ICE);
            fill(cx + 3, y + 6, 6, 4, C_ICE_D);
            put_px(cx + 8, y + 7, TEXT_WHITE);
        }
        _ => {
            fill(cx - 1, y + 5, 2, 6, C_STEM_D);
            disc(cx - 4, y + 13, 5, C_CHERRY_D);
            disc(cx + 3, y + 11, 5, C_CHERRY_D);
            disc(cx - 4, y + 12, 4, C_CHERRY);
            disc(cx + 3, y + 10, 4, C_CHERRY);
        }
    }
}

// ── Зомбі ────────────────────────────────────────────────────────────────────

fn draw_zombie(z: &Zombie) {
    let px = PLAY_X + z.xm * CELL_W / XM;
    let x = px - 11;
    let y = PLAY_Y + z.row as i32 * CELL_H + 2;
    let step = if (z.anim / 4) % 2 == 0 { 0 } else { 2 };
    let skin = if z.flash > 0 { rgb(236, 236, 236) } else { C_SKIN };
    let skin_d = if z.flash > 0 { rgb(200, 200, 200) } else { C_SKIN_D };
    let lean = if z.bite_t > 0 { 1 } else { 0 };

    // тінь
    fill(x + 2, y + 30, 18, 2, C_SHADOW);
    // ноги
    fill(x + 5, y + 24 - step, 4, (7 + step) as u32, C_TROUSER);
    fill(x + 12, y + 24, 4, (7 - step) as u32, C_TROUSER);
    fill(x + 4, y + 30, 6, 2, C_DARK);
    fill(x + 11, y + 30, 6, 2, C_DARK);
    // тулуб
    fill(x + 4, y + 13, 14, 12, C_SHIRT_D);
    fill(x + 5, y + 14, 12, 10, C_SHIRT);
    // руки, витягнуті вліво
    fill(x - 2 - lean, y + 15, 8, 4, skin_d);
    fill(x - 4 - lean, y + 16, 4, 3, skin);
    fill(x + 9, y + 15, 4, 4, skin_d);
    // голова
    fill(x + 5, y + 3, 12, 11, skin_d);
    fill(x + 6, y + 4, 10, 9, skin);
    fill(x + 7, y + 7, 2, 2, C_DARK);
    fill(x + 11, y + 7, 2, 2, C_DARK);
    fill(x + 8, y + 11, 5, 2, C_DARK);
    // зачіска-жмут
    put_px(x + 8, y + 2, C_SKIN_D);
    put_px(x + 10, y + 1, C_SKIN_D);

    match z.kind {
        Z_CONE => {
            // конус на голові
            for i in 0..7i32 {
                let w = 3 + i;
                fill(x + 11 - w / 2, y + 2 - i / 2, w as u32, 1, if i % 2 == 0 { C_CONE } else { C_CONE_D });
            }
            fill(x + 5, y + 2, 12, 2, C_CONE_D);
        }
        Z_BUCKET => {
            fill(x + 4, y + 0, 14, 4, C_METAL_D);
            fill(x + 4, y + 1, 14, 3, C_METAL);
            fill(x + 5, y + 4, 12, 1, C_METAL_D);
            put_px(x + 8, y + 2, TEXT_WHITE);
        }
        Z_RUNNER => {
            // бандана й нахил уперед
            fill(x + 5, y + 3, 12, 2, ACCENT_RED);
            fill(x + 3, y + 3, 3, 4, ACCENT_RED);
            fill(x + 8, y + 17, 5, 3, TEXT_MUTED);
        }
        _ => {}
    }

    if z.hp < z.max_hp {
        let color = if z.slow_t > 0 { ACCENT_CYAN } else { ACCENT_RED };
        hp_bar(x + 2, y - 3, 20, z.hp, z.max_hp, color);
    }
}

fn draw_pea(p: &Pea) {
    let x = PLAY_X + p.xm * CELL_W / XM;
    let y = PLAY_Y + p.row as i32 * CELL_H + CELL_H / 2;
    if p.ice {
        disc(x, y, 4, C_ICE_D);
        disc(x, y, 3, C_ICE);
        put_px(x + 1, y - 1, TEXT_WHITE);
    } else {
        disc(x, y, 4, C_STEM_D);
        disc(x, y, 3, C_LEAF);
    }
}

fn draw_sun(s: &Sun) {
    let x = PLAY_X + s.x;
    let y = PLAY_Y + s.y;
    if s.ttl < 60 && (s.ttl / 4) % 2 == 0 {
        return; // блимає перед зникненням
    }
    disc_clip(x, y, 11, C_GOLD_D, PLAY_Y);
    disc_clip(x, y, 10, C_GOLD, PLAY_Y);
    disc_clip(x, y, 6, rgb(255, 240, 140), PLAY_Y);
    // промені (тільки в межах грядки)
    for (dx, dy) in [(-14i32, 0i32), (14, 0), (0, -14), (0, 14), (-10, -10), (10, -10), (-10, 10), (10, 10)] {
        if y + dy >= PLAY_Y {
            put_px(x + dx, y + dy, C_GOLD);
        }
    }
    if y - 4 >= PLAY_Y {
        put_px(x - 3, y - 4, TEXT_WHITE);
    }
}

fn draw_fx(f: &Fx) {
    let x = PLAY_X + f.x;
    let y = PLAY_Y + f.y;
    match f.kind {
        FX_BOOM => {
            let r = (12 - f.ttl as i32).max(2);
            disc(x, y, r, C_GOLD);
            disc(x, y, (r - 3).max(1), C_CHERRY);
        }
        FX_SUN => {
            put_px(x, y - 5, C_GOLD);
            put_px(x, y + 5, C_GOLD);
            put_px(x - 5, y, C_GOLD);
            put_px(x + 5, y, C_GOLD);
        }
        FX_MOW => {
            fill(x - 2, y, 5, 2, TEXT_MUTED);
        }
        _ => {
            put_px(x, y - 2, TEXT_WHITE);
            put_px(x + 2, y, TEXT_WHITE);
            put_px(x, y + 2, TEXT_WHITE);
            put_px(x - 2, y, TEXT_WHITE);
        }
    }
}

// ── Сцена ────────────────────────────────────────────────────────────────────

fn paint_scene_rect(game: &Game, r: Rect) {
    paint_lawn_rect(game, r);
    for p in game.plants.iter() {
        if !p.alive {
            continue;
        }
        let (x0, y0) = plant_px(p.col as i32, p.row as i32);
        if (Rect { x: x0, y: y0, w: CELL_W, h: CELL_H }).intersects(r) {
            draw_plant(p);
        }
    }
    for p in game.peas.iter() {
        if p.alive && pea_rect(p).intersects(r) {
            draw_pea(p);
        }
    }
    for z in game.zombies.iter() {
        if z.alive && zombie_rect(z).intersects(r) {
            draw_zombie(z);
        }
    }
    for s in game.suns.iter() {
        if s.alive && sun_rect(s).intersects(r) {
            draw_sun(s);
        }
    }
    for f in game.fx.iter() {
        if f.alive && fx_rect(f).intersects(r) {
            draw_fx(f);
        }
    }
}

fn paint_scene(game: &Game) {
    fill(0, PLAY_Y, SCREEN_W as u32, PLAY_H as u32, C_LAWN_A);
    fill(0, PLAY_Y + PLAY_H, SCREEN_W as u32, (SCREEN_H - PLAY_Y - PLAY_H) as u32, C_SOIL_D);
    paint_scene_rect(game, Rect { x: 0, y: PLAY_Y, w: SCREEN_W, h: PLAY_H });
}

// ── HUD ──────────────────────────────────────────────────────────────────────

#[derive(Clone, Copy, PartialEq, Eq)]
struct Ui {
    sel: u8,
    demo: bool,
}

#[inline]
fn packet_rect(i: usize) -> BtnRect {
    BtnRect::new(SEED_X0 + i as i32 * SEED_DX, SEED_Y, SEED_W, SEED_H)
}

fn draw_sun_counter(game: &Game) {
    disc(11, 11, 6, C_GOLD_D);
    disc(11, 11, 5, C_GOLD);
    disc(11, 11, 2, rgb(255, 240, 140));
    let mut buf = [0u8; 10];
    text(20, 6, u32_str(game.sun as u32, &mut buf), FontKind::Small, ACCENT_GOLD);
}

fn draw_top_bar(game: &Game, ui: &Ui) {
    fill(0, 0, SCREEN_W as u32, HUD_H as u32, C_BAR);
    fill(0, HUD_H - 1, SCREEN_W as u32, 1, C_BAR_LINE);
    draw_sun_counter(game);
    let mut buf = [0u8; 10];
    let mut l = LineBuf::<12>::new();
    let _ = write!(l, "W{}/{}", game.wave, WAVES);
    text(52, 6, l.as_str(), FontKind::Small, TEXT_WHITE);
    let mut l2 = LineBuf::<12>::new();
    let _ = write!(l2, "K{}", u32_str(game.kills, &mut buf));
    text(100, 6, l2.as_str(), FontKind::Small, TEXT_MUTED);
    // смужка прогресу хвилі
    let bx = 140i32;
    let bw = 68u32;
    let pct = game.wave_progress_pct() as u32;
    fill(bx, 8, bw, 6, rgb(12, 16, 24));
    fill(bx, 8, bw * pct / 100, 6, ACCENT_GREEN);
    stroke_rect(bx, 8, bw, 6, C_BAR_LINE);
    draw_demo_btn(BTN_DEMO, ui.demo, FontKind::Small);
    draw_exit_btn(BTN_EXIT, FontKind::Small);
}

fn draw_packets(game: &Game, ui: &Ui) {
    for i in 0..PLANT_KINDS {
        let kind = i as u8;
        let r = packet_rect(i);
        let st = plant_stats(kind);
        let ready = game.packet_ready(kind);
        let can = game.sun >= st.cost;
        let selected = ui.sel == kind;

        let bg = if selected {
            rgb(58, 74, 108)
        } else if ready && can {
            rgb(34, 42, 60)
        } else {
            rgb(24, 28, 38)
        };
        fill(r.x, r.y, r.w, r.h, bg);
        draw_plant_icon(kind, r.x + 4, r.y + 6);

        let mut buf = [0u8; 10];
        let price = u32_str(st.cost as u32, &mut buf);
        let pc = if can { ACCENT_GOLD } else { rgb(150, 80, 80) };
        text(r.x + 26, r.y + 13, price, FontKind::Small, pc);

        // перезарядка: темна смуга знизу вгору
        if !ready && st.recharge > 0 {
            let h = (r.h * game.cd[i] as u32) / st.recharge as u32;
            if h > 0 {
                blend_fill(r.x + 1, r.y + r.h as i32 - h as i32, r.w - 2, h, rgb(4, 6, 10), 170);
            }
        }
        stroke_rect(
            r.x,
            r.y,
            r.w,
            r.h,
            if selected { ACCENT_GOLD } else { BTN_STD_BORDER },
        );
        if selected {
            fill(r.x + r.w as i32 / 2 - 3, r.y + r.h as i32 - 4, 6, 2, ACCENT_GOLD);
        }
    }
}

fn draw_hud(game: &Game, ui: &Ui) {
    draw_top_bar(game, ui);
    draw_packets(game, ui);
}

fn draw_overlay(game: &Game, best: u32) {
    let win = game.over == OVER_WIN;
    let (c, title) = if win {
        (ACCENT_GREEN, "GARDEN SAVED")
    } else {
        (ACCENT_RED, "GAME OVER")
    };
    blend_fill(36, 76, 248, 92, BG_DEEP, 220);
    stroke_rect(36, 76, 248, 92, c);
    text_centered(160, 84, title, FontKind::Bold, c);

    let mut l = LineBuf::<24>::new();
    let _ = write!(l, "SCORE {}", game.score);
    text_centered(160, 108, l.as_str(), FontKind::Small, TEXT_WHITE);

    let mut l2 = LineBuf::<24>::new();
    let _ = write!(l2, "WAVE {}  KILLS {}", game.wave, game.kills);
    text_centered(160, 122, l2.as_str(), FontKind::Small, TEXT_MUTED);

    if best > 0 {
        let mut l3 = LineBuf::<24>::new();
        let _ = write!(l3, "BEST {}", best);
        text_centered(160, 136, l3.as_str(), FontKind::Small, ACCENT_GOLD);
    }
    text_centered(160, 152, "TAP TO PLAY AGAIN", FontKind::Small, TEXT_WHITE);
}

// ── Ввід ─────────────────────────────────────────────────────────────────────

fn select_packet(ui: &mut Ui, kind: u8) {
    ui.sel = if ui.sel == kind { 255 } else { kind };
}

fn on_tap_field(game: &mut Game, ui: &mut Ui, p: Point) {
    if p.x < PLAY_X || p.x >= PLAY_X + PLAY_W || p.y < PLAY_Y || p.y >= PLAY_Y + PLAY_H {
        return;
    }
    let lx = p.x - PLAY_X;
    let ly = p.y - PLAY_Y;
    // сонце збираємо завжди, навіть у демо
    if game.collect_sun(lx, ly) {
        return;
    }
    if ui.demo || ui.sel == 255 {
        return;
    }
    let col = lx / CELL_W;
    let row = ly / CELL_H;
    if game.place(col, row, ui.sel) {
        println!("GARDEN: посаджено {} у ({},{})", ui.sel, col, row);
        ui.sel = 255;
    } else if game.plant_at(col, row).is_some() {
        println!("GARDEN: клітинка ({},{}) зайнята", col, row);
    } else {
        println!("GARDEN: не вистачає сонця/перезарядка ({},{})", col, row);
    }
}

fn handle_uart(game: &mut Game, ui: &mut Ui, ch: u8, uart_state: &mut u8, uart_col: &mut u8) {
    // стан машини для команди `P<col><row>`
    if *uart_state == 1 {
        if ch.is_ascii_digit() {
            *uart_col = ch - b'0';
            *uart_state = 2;
            return;
        }
        *uart_state = 0;
    } else if *uart_state == 2 {
        *uart_state = 0;
        if ch.is_ascii_digit() && ui.sel != 255 {
            let row = (ch - b'0') as i32;
            let col = *uart_col as i32;
            let ok = game.place(col, row, ui.sel);
            println!("GARDEN: посадка {},{}/{} -> {}", col, row, ui.sel, ok);
        }
        return;
    }

    match ch {
        b'P' | b'p' => *uart_state = 1,
        b'1'..=b'5' => select_packet(ui, ch - b'1'),
        b'G' | b'g' => {
            let n = game.collect_all_suns();
            println!("GARDEN: зібрано сонця {}", n);
        }
        b'Z' | b'z' => {
            let lane = (game.rng() % LANES as u32) as usize;
            let ok = game.spawn_zombie(Z_BASIC, lane);
            println!("GARDEN: зомбі в ряд {} -> {}", lane, ok);
        }
        b'W' | b'w' => {
            game.spawn_left = 0;
            game.wave_t = 0;
            println!("GARDEN: хвилю прискорено");
        }
        b'S' | b's' => {
            println!(
                "GARDEN: t={}s sun={} wave={}/{} z={} plants={} kills={} score={} over={}",
                game.tick_n / TICKS_PER_SEC,
                game.sun,
                game.wave,
                WAVES,
                game.alive_zombies(),
                game.plants_count(),
                game.kills,
                game.score,
                game.over
            );
        }
        _ => {}
    }
}

// ── Точка входу ──────────────────────────────────────────────────────────────

#[esp_hal::main]
fn main() -> ! {
    let mut hw = Board::init();
    let (run, failed) = Game::selftest();
    println!("GARDEN: selftest {}/{} failed={}", run - failed, run, failed);

    let mut ui = Ui { sel: 255, demo: hw.start_in_demo(cfg!(feature = "demo")) };
    let mut game = Game::new(hw.rng.random());
    let mut best = hw.load_best(SAVE_FILE);
    let mut best_saved = false;

    let t0 = Instant::now();
    paint_scene(&game);
    draw_hud(&game, &ui);
    hw.present_all();
    println!(
        "GARDEN: старт, повний кадр = {} ms, demo={}, best={}",
        elapsed_ms(t0),
        ui.demo,
        best
    );

    let mut hud_state: (u16, u16, u32, u8, bool, u8, [u16; PLANT_KINDS]) =
        (u16::MAX, 0, 0, 255, false, 0, [0; PLANT_KINDS]);
    let mut over_drawn = false;
    let mut over_ms: u32 = 0;

    let t_base = Instant::now();
    let mut sim_at: u32 = 0;
    let mut last_touch: u32 = 0;
    let mut last_hud: u32 = 0;
    let mut rendered_tick: u32 = u32::MAX;
    let mut ui_dirty = false;

    let mut was_pressed = false;
    let mut press_pt = Point::zero();

    let mut prev_z = [Rect::EMPTY; MAX_ZOMBIES];
    let mut prev_p = [Rect::EMPTY; MAX_PEAS];
    let mut prev_s = [Rect::EMPTY; MAX_SUNS];
    let mut prev_f = [Rect::EMPTY; MAX_FX];
    let mut prev_m = [Rect::EMPTY; LANES];
    let mut plant_state = [(false, 0u8, 0u16, 0u8); CELLS];

    let mut uart_state: u8 = 0;
    let mut uart_col: u8 = 0;

    let mut frame: u32 = 0;
    let mut win_frames: u32 = 0;
    let mut win_ms: u32 = 0;
    let mut win_rects: u32 = 0;
    let mut t_win = Instant::now();

    loop {
        let now = elapsed_ms(t_base);

        // ── UART
        while let Some(ch) = hw.poll_uart_byte() {
            match ch {
                b'X' | b'x' | b'Q' | b'q' => {
                    if game.score > best {
                        best = game.score;
                        hw.save_best(SAVE_FILE, best);
                    }
                    println!("GARDEN: вихід у GameLauncher (score={})", game.score);
                    hw.exit_to_launcher();
                }
                b'D' | b'd' => {
                    ui.demo = !ui.demo;
                    println!("GARDEN: demo={}", ui.demo);
                    ui_dirty = true;
                }
                other => handle_uart(&mut game, &mut ui, other, &mut uart_state, &mut uart_col),
            }
        }

        // ── Симуляція 20 Гц
        while now >= sim_at + TICK_MS {
            sim_at += TICK_MS;
            game.tick();
            if ui.demo && game.tick_n % 4 == 0 {
                game.ai_step();
            }
        }

        // ── Тач
        if now.wrapping_sub(last_touch) >= 16 {
            last_touch = now;
            let mut pressed = false;
            let mut pt = Point::zero();
            if hw.touch.pressed() {
                pt = hw.touch.point();
                pressed = hw.touch.pressed();
            }
            if pressed && !was_pressed {
                press_pt = pt;
            } else if !pressed && was_pressed {
                let p = press_pt;
                if BTN_EXIT.contains(p) {
                    if game.score > best {
                        best = game.score;
                        hw.save_best(SAVE_FILE, best);
                    }
                    println!("GARDEN: вихід у GameLauncher (тач)");
                    hw.exit_to_launcher();
                } else if BTN_DEMO.contains(p) {
                    ui.demo = !ui.demo;
                    ui_dirty = true;
                    println!("GARDEN: demo={}", ui.demo);
                } else if game.over != OVER_NONE {
                    // нова партія
                    game = Game::new(hw.rng.random());
                    over_drawn = false;
                    best_saved = false;
                    ui.sel = 255;
                    ui_dirty = true;
                    sim_at = now;
                    plant_state = [(false, 0u8, 0u16, 0u8); CELLS];
                    prev_z = [Rect::EMPTY; MAX_ZOMBIES];
                    prev_p = [Rect::EMPTY; MAX_PEAS];
                    prev_s = [Rect::EMPTY; MAX_SUNS];
                    prev_f = [Rect::EMPTY; MAX_FX];
                    prev_m = [Rect::EMPTY; LANES];
                    paint_scene(&game);
                    draw_hud(&game, &ui);
                    hw.present_all();
                    println!("GARDEN: нова партія");
                } else if p.y >= SEED_Y && p.y < SEED_Y + SEED_H as i32 {
                    for i in 0..PLANT_KINDS {
                        if packet_rect(i).contains(p) {
                            select_packet(&mut ui, i as u8);
                            ui_dirty = true;
                            break;
                        }
                    }
                } else if p.y >= PLAY_Y {
                    on_tap_field(&mut game, &mut ui, p);
                    ui_dirty = true;
                }
            }
            was_pressed = pressed;
        }

        // ── Кінець партії
        if game.over != OVER_NONE {
            if !over_drawn {
                over_drawn = true;
                over_ms = now;
                paint_scene(&game);
                draw_hud(&game, &ui);
                draw_overlay(&game, best);
                hw.present_all();
                if game.score > best {
                    best = game.score;
                    hw.save_best(SAVE_FILE, best);
                    best_saved = true;
                }
                println!(
                    "GARDEN: партія завершена over={} wave={} kills={} score={} best={} saved={}",
                    game.over, game.wave, game.kills, game.score, best, best_saved
                );
            } else if ui.demo && now.wrapping_sub(over_ms) > 3000 {
                game = Game::new(hw.rng.random());
                over_drawn = false;
                ui.sel = 255;
                ui_dirty = true;
                sim_at = now;
                plant_state = [(false, 0u8, 0u16, 0u8); CELLS];
                prev_z = [Rect::EMPTY; MAX_ZOMBIES];
                prev_p = [Rect::EMPTY; MAX_PEAS];
                prev_s = [Rect::EMPTY; MAX_SUNS];
                prev_f = [Rect::EMPTY; MAX_FX];
                prev_m = [Rect::EMPTY; LANES];
                paint_scene(&game);
                draw_hud(&game, &ui);
                hw.present_all();
                println!("GARDEN: демо — нова партія");
            }
            hw.delay.delay_millis(20);
            continue;
        }

        // ── HUD (не частіше 10 Гц)
        let key = (
            game.sun,
            game.wave,
            game.kills,
            ui.sel,
            ui.demo,
            game.over,
            game.cd,
        );
        if key != hud_state {
            hud_state = key;
            if ui_dirty || now.wrapping_sub(last_hud) >= 100 {
                last_hud = now;
                ui_dirty = false;
                draw_hud(&game, &ui);
                hw.present_rect(0, 0, SCREEN_W as u16, (SEED_Y as u16) + SEED_H as u16);
            }
        }

        // ── Кадр грядки
        if rendered_tick != game.tick_n {
            rendered_tick = game.tick_n;
            let t_frame = Instant::now();
            let mut dirty = Dirty::new();

            for i in 0..CELLS {
                let p = game.plants[i];
                let st = (p.alive, p.kind, p.hp, p.flash);
                if st != plant_state[i] {
                    plant_state[i] = st;
                    let (x0, y0) = plant_px(p.col as i32, p.row as i32);
                    dirty.add(Rect { x: x0, y: y0, w: CELL_W, h: CELL_H });
                }
            }
            for i in 0..MAX_ZOMBIES {
                let z = &game.zombies[i];
                let r = if z.alive { zombie_rect(z) } else { Rect::EMPTY };
                dirty.add(r.union(prev_z[i]));
                prev_z[i] = r;
            }
            for i in 0..MAX_PEAS {
                let p = &game.peas[i];
                let r = if p.alive { pea_rect(p) } else { Rect::EMPTY };
                dirty.add(r.union(prev_p[i]));
                prev_p[i] = r;
            }
            for i in 0..MAX_SUNS {
                let s = &game.suns[i];
                let r = if s.alive { sun_rect(s) } else { Rect::EMPTY };
                dirty.add(r.union(prev_s[i]));
                prev_s[i] = r;
            }
            for i in 0..MAX_FX {
                let f = &game.fx[i];
                let r = if f.alive { fx_rect(f) } else { Rect::EMPTY };
                dirty.add(r.union(prev_f[i]));
                prev_f[i] = r;
            }
            for lane in 0..LANES {
                let r = match mower_draw_xm(&game, lane) {
                    Some(xm) => mower_rect(lane, xm),
                    None => Rect::EMPTY,
                };
                if r != prev_m[lane] {
                    dirty.add(r.union(prev_m[lane]));
                    prev_m[lane] = r;
                }
            }

            let n = dirty.merge();
            for k in 0..n {
                paint_scene_rect(&game, dirty.r[k]);
            }
            for k in 0..n {
                let r = dirty.r[k];
                hw.present_rect(r.x as u16, r.y as u16, r.w as u16, r.h as u16);
            }

            let frame_ms = elapsed_ms(t_frame);
            frame += 1;
            win_frames += 1;
            win_ms += frame_ms;
            win_rects += n as u32;
            if frame % 120 == 0 {
                let total = elapsed_ms(t_win);
                let fps = if total > 0 { (win_frames * 1000) / total } else { 0 };
                println!(
                    "GARDEN: fps={} frame={}ms avg={}ms rects={} z={} sun={} wave={}",
                    fps,
                    frame_ms,
                    win_ms / win_frames.max(1),
                    win_rects / win_frames.max(1),
                    game.alive_zombies(),
                    game.sun,
                    game.wave
                );
                win_frames = 0;
                win_ms = 0;
                win_rects = 0;
                t_win = Instant::now();
            }
        }

        hw.delay.delay_millis(2);
    }
}
