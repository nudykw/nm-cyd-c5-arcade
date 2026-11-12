//! **Angry Birds** для плати **NM-CYD-C5** у складі **CYD-C5 Arcade** (використовує `cyd-c5-common`).

#![no_std]
#![no_main]

use cyd_c5_common::{
    Board, Fb, SCREEN_H, SCREEN_W, fb_mut,
    ui::{
        BTN_STD_BORDER, BtnRect, FontKind as UiFont, draw_btn as ui_draw_btn, draw_demo_btn,
        draw_exit_btn, elapsed_ms,
    },
};
use cyd_game_birds::game::{
    Block, FIELD_H, FIELD_W, FP, Game, GROUND_Y, KIND_ICE, KIND_STONE, Pig, PreviewPt, SLING_X,
    SLING_Y, State, selftest_checks,
};
use embedded_graphics::{
    mono_font::{
        MonoFont, MonoTextStyle,
        ascii::{FONT_6X10, FONT_9X18_BOLD, FONT_10X20},
    },
    pixelcolor::Rgb565,
    prelude::*,
    primitives::{PrimitiveStyle, Rectangle},
    text::Text,
};
use esp_hal::time::Instant;
use esp_println::println;

esp_bootloader_esp_idf::esp_app_desc!(
    "0.1.0",
    "cyd-game-birds",
    "00:00:00",
    "2026-11-12",
    "5.5.6",
    64 * 1024,
    0,
    u16::MAX,
    0
);

#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    println!("BIRDS PANIC: {:?}", info);
    loop {}
}

const SAVE_FILE: &str = "BIRDS.SAV";

const PANEL_X: i32 = FIELD_W; // 240
const PANEL_W: i32 = SCREEN_W - PANEL_X; // 80
const FB_W: usize = SCREEN_W as usize;
const TARGET_FRAME_MS: u32 = 30;

const BTN_RETRY: BtnRect = BtnRect::new(PANEL_X + 4, 134, 35, 22);
const BTN_NEW: BtnRect = BtnRect::new(PANEL_X + 41, 134, 35, 22);
const BTN_PAUSE: BtnRect = BtnRect::new(PANEL_X + 4, 159, 72, 22);
const BTN_DEMO: BtnRect = BtnRect::new(PANEL_X + 4, 184, 72, 22);
const BTN_EXIT: BtnRect = BtnRect::new(PANEL_X + 4, 209, 72, 25);

const BAND_L: (i32, i32) = (SLING_X - 10, SLING_Y - 6);
const BAND_R: (i32, i32) = (SLING_X + 10, SLING_Y - 6);
const DRAG_R2: i32 = 34 * 34;

const C_TRANSPARENT: Rgb565 = Rgb565::new(0, 1, 0);

const C_PANEL_BG: Rgb565 = Rgb565::new(2, 6, 5);
const C_CARD_BG: Rgb565 = Rgb565::new(3, 10, 8);
const C_GOLD: Rgb565 = Rgb565::new(31, 50, 6);
const C_GREEN_OK: Rgb565 = Rgb565::new(6, 58, 12);
const C_TEXT: Rgb565 = Rgb565::new(29, 59, 30);
const C_DIM: Rgb565 = Rgb565::new(15, 36, 20);
const C_WHITE: Rgb565 = Rgb565::new(31, 63, 31);
const C_BLACK: Rgb565 = Rgb565::new(1, 2, 1);
const C_RED: Rgb565 = Rgb565::new(31, 8, 6);
const C_RED_DARK: Rgb565 = Rgb565::new(13, 3, 2);
const C_BTN_RETRY: Rgb565 = Rgb565::new(9, 20, 25);
const C_BTN_NEW: Rgb565 = Rgb565::new(5, 24, 24);
const C_BTN_PAUSE: Rgb565 = Rgb565::new(9, 22, 16);
const C_BTN_PAUSE_ON: Rgb565 = Rgb565::new(28, 42, 4);

const BG_PAL: [Rgb565; 8] = [
    Rgb565::new(4, 13, 26),
    Rgb565::new(9, 25, 31),
    Rgb565::new(16, 44, 31),
    Rgb565::new(29, 62, 31),
    Rgb565::new(6, 22, 17),
    Rgb565::new(9, 44, 12),
    Rgb565::new(18, 32, 9),
    Rgb565::new(10, 18, 5),
];

const FIELD_PACKED_LEN: usize = (FIELD_W as usize * FIELD_H as usize) / 2;
const SPR: usize = 21;
const SPR_HALF: i32 = 10;

static mut FIELD_BG: [u8; FIELD_PACKED_LEN] = [0u8; FIELD_PACKED_LEN];
static mut BIRD_SPR: [Rgb565; SPR * SPR] = [C_TRANSPARENT; SPR * SPR];
static mut PIG_SPR: [Rgb565; SPR * SPR] = [C_TRANSPARENT; SPR * SPR];
static mut GAME: Game = Game::empty();

#[inline]
fn field_bg_mut() -> &'static mut [u8; FIELD_PACKED_LEN] {
    unsafe { &mut *core::ptr::addr_of_mut!(FIELD_BG) }
}

#[inline]
fn field_bg_ref() -> &'static [u8; FIELD_PACKED_LEN] {
    unsafe { &*core::ptr::addr_of!(FIELD_BG) }
}

#[inline]
fn game_mut() -> &'static mut Game {
    unsafe { &mut *core::ptr::addr_of_mut!(GAME) }
}

#[inline]
fn paint_field(board: &mut Board) {
    board.present_rect(0, 0, FIELD_W as u16, FIELD_H as u16);
}

#[inline]
fn paint_panel(board: &mut Board) {
    board.present_rect(PANEL_X as u16, 0, PANEL_W as u16, SCREEN_H as u16);
}

#[inline]
fn paint_all(board: &mut Board) {
    board.present_all();
}

/* ---------- запечене тло: небо, пагорби, трава, рогатка ---------- */

#[inline]
fn bg_set_px(bg: &mut [u8; FIELD_PACKED_LEN], x: i32, y: i32, pal_idx: u8) {
    if !(0..FIELD_W).contains(&x) || !(0..FIELD_H).contains(&y) {
        return;
    }
    let idx = (y as usize) * (FIELD_W as usize) + (x as usize);
    let byte_i = idx >> 1;
    if (idx & 1) == 0 {
        bg[byte_i] = (bg[byte_i] & 0x0F) | ((pal_idx & 0x0F) << 4);
    } else {
        bg[byte_i] = (bg[byte_i] & 0xF0) | (pal_idx & 0x0F);
    }
}

fn bg_disc(bg: &mut [u8; FIELD_PACKED_LEN], cx: i32, cy: i32, r: i32, pal_idx: u8) {
    let r2 = r * r;
    for dy in -r..=r {
        for dx in -r..=r {
            if dx * dx + dy * dy <= r2 {
                bg_set_px(bg, cx + dx, cy + dy, pal_idx);
            }
        }
    }
}

fn bg_ellipse(bg: &mut [u8; FIELD_PACKED_LEN], cx: i32, cy: i32, rx: i32, ry: i32, pal_idx: u8) {
    for dy in -ry..=ry {
        for dx in -rx..=rx {
            if dx * dx * ry * ry + dy * dy * rx * rx <= rx * rx * ry * ry {
                bg_set_px(bg, cx + dx, cy + dy, pal_idx);
            }
        }
    }
}

fn bg_line(bg: &mut [u8; FIELD_PACKED_LEN], x0: i32, y0: i32, x1: i32, y1: i32, r: i32, c: u8) {
    let dx = (x1 - x0).abs();
    let sx = if x0 < x1 { 1 } else { -1 };
    let dy = -(y1 - y0).abs();
    let sy = if y0 < y1 { 1 } else { -1 };
    let mut err = dx + dy;
    let (mut x, mut y) = (x0, y0);
    loop {
        bg_disc(bg, x, y, r, c);
        if x == x1 && y == y1 {
            break;
        }
        let e2 = 2 * err;
        if e2 >= dy {
            err += dy;
            x += sx;
        }
        if e2 <= dx {
            err += dx;
            y += sy;
        }
    }
}

fn bake_field_bg() {
    let bg = field_bg_mut();

    // 1. Небо трьома смугами з дизерингом на межах.
    for y in 0..FIELD_H {
        let band: u8 = if y < 70 {
            0
        } else if y < 155 {
            1
        } else {
            2
        };
        for x in 0..FIELD_W {
            let mut c: u8 = band;
            // м'який перехід: 6 рядків шахівниці між смугами
            if (66..72).contains(&y) || (151..157).contains(&y) {
                let edge = if y < 100 { 66 } else { 151 };
                let t = (y - edge).clamp(0, 5);
                if ((x + y) & 1) == 0 && t < 3 {
                    c = band.saturating_sub(1);
                }
            }
            bg_set_px(bg, x, y, c);
        }
    }

    // 2. Сонце (біле ядро + світлий ореол) і кілька хмар.
    bg_disc(bg, 198, 46, 13, 3);
    for (dx, dy, r) in [(0, 0, 16), (0, 0, 19)] {
        bg_disc(bg, 198 + dx, 46 + dy, r, 2);
    }
    bg_disc(bg, 198, 46, 12, 3);

    let clouds = [
        (44, 58, 12, 5),
        (58, 54, 9, 4),
        (30, 56, 8, 4),
        (118, 34, 11, 4),
        (132, 38, 8, 3),
        (108, 38, 7, 3),
        (196, 96, 14, 5),
        (212, 92, 9, 4),
        (182, 94, 8, 4),
    ];
    for (cx, cy, rx, ry) in clouds {
        bg_ellipse(bg, cx, cy, rx, ry, 3);
    }

    // 3. Далекі та близькі пагорби.
    bg_ellipse(bg, 30, GROUND_Y + 40, 95, 70, 4);
    bg_ellipse(bg, 210, GROUND_Y + 46, 105, 78, 4);
    bg_ellipse(bg, 122, GROUND_Y + 52, 88, 52, 5);
    bg_ellipse(bg, 8, GROUND_Y + 44, 60, 44, 5);

    // 4. Земля: смуга трави, ґрунт із вкрапленнями.
    for y in GROUND_Y..FIELD_H {
        for x in 0..FIELD_W {
            let c = if y < GROUND_Y + 3 {
                5
            } else if y < GROUND_Y + 4 {
                7
            } else if ((x * 7 + y * 13) % 29) == 0 {
                7
            } else {
                6
            };
            bg_set_px(bg, x, y, c);
        }
    }
    // травинки-зубці на кромці
    for x in (0..FIELD_W).step_by(5) {
        let h = 2 + ((x * 3) % 3);
        for k in 0..h {
            bg_set_px(bg, x, GROUND_Y - k, 5);
        }
    }

    // 5. Рогатка: стовп і дві розвилки (стовп «вкопаний» у траву).
    bg_line(bg, SLING_X, GROUND_Y, SLING_X, SLING_Y + 12, 4, 7);
    bg_line(bg, SLING_X, GROUND_Y, SLING_X, SLING_Y + 12, 3, 6);
    bg_line(bg, SLING_X, SLING_Y + 14, BAND_L.0, BAND_L.1, 3, 7);
    bg_line(bg, SLING_X, SLING_Y + 14, BAND_L.0, BAND_L.1, 2, 6);
    bg_line(bg, SLING_X, SLING_Y + 14, BAND_R.0, BAND_R.1, 3, 7);
    bg_line(bg, SLING_X, SLING_Y + 14, BAND_R.0, BAND_R.1, 2, 6);
    // темна тінь під рогаткою
    bg_ellipse(bg, SLING_X, GROUND_Y + 2, 12, 3, 7);
}

/// Швидко розпакувати запечене тло у ліву частину `FB` (~0.4 мс).
fn restore_field_bg() {
    let bg = field_bg_ref();
    let fb = fb_mut();
    let fw = FIELD_W as usize;
    for y in 0..(FIELD_H as usize) {
        let bg_row = y * (fw / 2);
        let fb_row = y * FB_W;
        for bx in 0..(fw / 2) {
            let packed = bg[bg_row + bx];
            fb[fb_row + bx * 2] = BG_PAL[(packed >> 4) as usize];
            fb[fb_row + bx * 2 + 1] = BG_PAL[(packed & 0x07) as usize];
        }
    }
}

/* ---------- спрайти птаха й свині ---------- */

fn bake_bird_sprite() {
    let s = unsafe { &mut *core::ptr::addr_of_mut!(BIRD_SPR) };
    let body = Rgb565::new(30, 10, 7);
    let light = Rgb565::new(31, 26, 16);
    let dark = Rgb565::new(15, 4, 3);
    let belly = Rgb565::new(31, 47, 27);
    let beak = Rgb565::new(31, 38, 3);
    let beak_d = Rgb565::new(21, 20, 2);
    for py in 0..SPR {
        for px in 0..SPR {
            let dx = px as i32 - SPR_HALF;
            let dy = py as i32 - SPR_HALF;
            let d2 = dx * dx + dy * dy;
            let mut c = C_TRANSPARENT;
            if d2 <= 82 {
                c = body;
                if d2 >= 64 {
                    c = dark;
                }
                let bx = dx + 1;
                let by = dy - 3;
                if bx * bx + by * by <= 17 {
                    c = belly;
                }
                let hx = dx + 3;
                let hy = dy + 4;
                if hx * hx + hy * hy <= 6 {
                    c = light;
                }
                // хвіст-клинець зліва
                if dx <= -8 && dy.abs() <= 2 {
                    c = dark;
                }
            }
            // дзьоб: трикутник уперед
            if dx >= 5 && dy.abs() <= (10 - dx).max(0) {
                c = beak;
                if dy.abs() == (10 - dx).max(0) {
                    c = beak_d;
                }
            }
            // очі
            let e1 = (dx + 3) * (dx + 3) + (dy + 3) * (dy + 3);
            let e2 = (dx - 3) * (dx - 3) + (dy + 3) * (dy + 3);
            if e1 <= 6 || e2 <= 6 {
                c = C_WHITE;
            }
            if e1 <= 1 || e2 <= 1 {
                c = C_BLACK;
            }
            // брови
            if (-7..=-5).contains(&dy) && (2..=7).contains(&dx.abs()) {
                c = dark;
            }
            s[py * SPR + px] = c;
        }
    }
}

fn bake_pig_sprite() {
    let s = unsafe { &mut *core::ptr::addr_of_mut!(PIG_SPR) };
    let body = Rgb565::new(11, 50, 12);
    let light = Rgb565::new(18, 61, 18);
    let dark = Rgb565::new(4, 23, 5);
    let snout = Rgb565::new(21, 58, 16);
    for py in 0..SPR {
        for px in 0..SPR {
            let dx = px as i32 - SPR_HALF;
            let dy = py as i32 - SPR_HALF;
            let d2 = dx * dx + dy * dy;
            // вушка
            let ear_l = (dx + 6) * (dx + 6) + (dy + 7) * (dy + 7) <= 6;
            let ear_r = (dx - 6) * (dx - 6) + (dy + 7) * (dy + 7) <= 6;
            let mut c = C_TRANSPARENT;
            if d2 <= 82 || ear_l || ear_r {
                c = body;
                if (d2 >= 64 && !ear_l && !ear_r) || (d2 >= 90 && (ear_l || ear_r)) {
                    c = dark;
                }
                let hx = dx + 3;
                let hy = dy + 4;
                if hx * hx + hy * hy <= 8 {
                    c = light;
                }
                // писок
                let sx = dx;
                let sy = (dy - 3) * 2;
                if sx * sx + sy * sy <= 18 {
                    c = snout;
                }
            }
            // ніздрі
            if (dx == -2 || dx == 2) && dy == 3 {
                c = dark;
            }
            // очі
            let e1 = (dx + 3) * (dx + 3) + (dy + 2) * (dy + 2);
            let e2 = (dx - 3) * (dx - 3) + (dy + 2) * (dy + 2);
            if e1 <= 4 || e2 <= 4 {
                c = C_WHITE;
            }
            if e1 == 0 || e2 == 0 {
                c = C_BLACK;
            }
            s[py * SPR + px] = c;
        }
    }
}

/* ---------- примітиви поля ---------- */

#[inline]
fn put_field_px(x: i32, y: i32, c: Rgb565) {
    if (0..FIELD_W).contains(&x) && (0..FIELD_H).contains(&y) {
        fb_mut()[y as usize * FB_W + x as usize] = c;
    }
}

fn field_disc(cx: i32, cy: i32, r: i32, c: Rgb565) {
    let r2 = r * r;
    let y0 = (cy - r).max(0);
    let y1 = (cy + r).min(FIELD_H - 1);
    let x0 = (cx - r).max(0);
    let x1 = (cx + r).min(FIELD_W - 1);
    let fb = fb_mut();
    for y in y0..=y1 {
        let dy = y - cy;
        let dy2 = dy * dy;
        let row = y as usize * FB_W;
        for x in x0..=x1 {
            let dx = x - cx;
            if dx * dx + dy2 <= r2 {
                fb[row + x as usize] = c;
            }
        }
    }
}

fn field_ring(cx: i32, cy: i32, r_in: i32, r_out: i32, c: Rgb565) {
    let ri2 = r_in * r_in;
    let ro2 = r_out * r_out;
    for dy in -r_out..=r_out {
        for dx in -r_out..=r_out {
            let d2 = dx * dx + dy * dy;
            if d2 >= ri2 && d2 <= ro2 {
                put_field_px(cx + dx, cy + dy, c);
            }
        }
    }
}

fn field_line(x0: i32, y0: i32, x1: i32, y1: i32, r: i32, c: Rgb565) {
    let dx = (x1 - x0).abs();
    let sx = if x0 < x1 { 1 } else { -1 };
    let dy = -(y1 - y0).abs();
    let sy = if y0 < y1 { 1 } else { -1 };
    let mut err = dx + dy;
    let (mut x, mut y) = (x0, y0);
    loop {
        field_disc(x, y, r, c);
        if x == x1 && y == y1 {
            break;
        }
        let e2 = 2 * err;
        if e2 >= dy {
            err += dy;
            x += sx;
        }
        if e2 <= dx {
            err += dx;
            y += sy;
        }
    }
}

fn blit(spr: &[Rgb565; SPR * SPR], cx: i32, cy: i32) {
    let x0 = cx - SPR_HALF;
    let y0 = cy - SPR_HALF;
    let fb = fb_mut();
    for sy in 0..SPR {
        let py = y0 + sy as i32;
        if !(0..FIELD_H).contains(&py) {
            continue;
        }
        let row = py as usize * FB_W;
        let srow = sy * SPR;
        for sx in 0..SPR {
            let px = x0 + sx as i32;
            if !(0..FIELD_W).contains(&px) {
                continue;
            }
            let c = spr[srow + sx];
            if c != C_TRANSPARENT {
                fb[row + px as usize] = c;
            }
        }
    }
}

fn draw_block(b: &Block) {
    let x = b.x;
    let y = b.y_fp / FP;
    let w = b.w;
    let h = b.h;
    let (base, light, dark) = match b.kind {
        KIND_ICE => (
            Rgb565::new(13, 42, 42),
            Rgb565::new(24, 61, 60),
            Rgb565::new(6, 22, 27),
        ),
        KIND_STONE => (
            Rgb565::new(17, 34, 19),
            Rgb565::new(27, 49, 28),
            Rgb565::new(7, 15, 9),
        ),
        _ => (
            Rgb565::new(24, 41, 10),
            Rgb565::new(31, 54, 15),
            Rgb565::new(12, 21, 5),
        ),
    };
    // основа + рамка
    for yy in y..(y + h) {
        for xx in x..(x + w) {
            put_field_px(xx, yy, base);
        }
    }
    for xx in x..(x + w) {
        put_field_px(xx, y, dark);
        put_field_px(xx, y + h - 1, dark);
    }
    for yy in y..(y + h) {
        put_field_px(x, yy, dark);
        put_field_px(x + w - 1, yy, dark);
    }
    // фаска
    for xx in (x + 1)..(x + w - 1) {
        put_field_px(xx, y + 1, light);
    }
    for yy in (y + 1)..(y + h - 1) {
        put_field_px(x + 1, yy, light);
    }
    // текстура
    match b.kind {
        KIND_ICE => {
            for k in 0..4 {
                let xx = x + 3 + (k * (w - 5)) / 4;
                let yy = y + 2 + (k * (h - 5)) / 4;
                put_field_px(xx, yy, C_WHITE);
                put_field_px(xx + 1, yy, light);
            }
        }
        KIND_STONE => {
            let mut seed = (x * 31 + y * 17) as u32;
            for _ in 0..((w * h) / 40).max(3) {
                seed = seed.wrapping_mul(1103515245).wrapping_add(12345);
                let xx = x + 2 + (seed >> 8) as i32 % (w - 3).max(1);
                let yy = y + 2 + (seed >> 16) as i32 % (h - 3).max(1);
                put_field_px(xx, yy, dark);
            }
        }
        _ => {
            if w >= h {
                let yy = y + h / 2;
                for xx in (x + 2)..(x + w - 2) {
                    put_field_px(xx, yy, dark);
                }
            } else {
                let xx = x + w / 2;
                for yy in (y + 2)..(y + h - 2) {
                    put_field_px(xx, yy, dark);
                }
            }
        }
    }
    // спалах від удару — біла рамка
    if b.flash > 0 {
        for xx in x..(x + w) {
            put_field_px(xx, y, C_WHITE);
            put_field_px(xx, y + h - 1, C_WHITE);
        }
        for yy in y..(y + h) {
            put_field_px(x, yy, C_WHITE);
            put_field_px(x + w - 1, yy, C_WHITE);
        }
    }
}

fn draw_pig(p: &Pig, tick: u32) {
    let spr = unsafe { &*core::ptr::addr_of!(PIG_SPR) };
    let cy = p.y_fp / FP;
    blit(spr, p.x, cy);
    if p.flash > 0 {
        field_ring(p.x, cy, p.r + 1, p.r + 2, C_RED);
    } else if (p.hp <= 1 || p.falling) && (tick & 8) == 0 {
        field_ring(p.x, cy, p.r + 1, p.r + 2, C_GOLD);
    }
}

fn draw_bird(cx: i32, cy: i32) {
    let spr = unsafe { &*core::ptr::addr_of!(BIRD_SPR) };
    blit(spr, cx, cy);
}

fn draw_bands(bx: i32, by: i32) {
    let c = Rgb565::new(12, 14, 4);
    field_line(BAND_L.0, BAND_L.1, bx, by, 1, c);
    field_line(BAND_R.0, BAND_R.1, bx, by, 1, c);
}

fn draw_preview(points: &[PreviewPt]) {
    let col = Rgb565::new(31, 62, 20);
    for (i, p) in points.iter().enumerate() {
        let c = if i % 3 == 0 { C_WHITE } else { col };
        field_disc(p.x as i32, p.y as i32, 1, c);
    }
    if let Some(p) = points.last() {
        field_ring(p.x as i32, p.y as i32, 3, 4, C_WHITE);
        field_ring(p.x as i32, p.y as i32, 1, 2, C_GOLD);
    }
}

fn draw_particles(g: &Game) {
    for p in g.particles.iter() {
        if !p.alive {
            continue;
        }
        let c = match p.color {
            0 => Rgb565::new(26, 46, 26), // пил
            1 => Rgb565::new(26, 60, 60), // лід
            2 => Rgb565::new(24, 44, 26), // камінь
            3 => Rgb565::new(28, 44, 12), // дерево
            _ => Rgb565::new(12, 54, 14), // свиня
        };
        let sz = p.size.max(1) as i32;
        let x = p.x_fp / FP;
        let y = p.y_fp / FP;
        for dy in 0..sz {
            for dx in 0..sz {
                put_field_px(x + dx, y + dy, c);
            }
        }
    }
}

fn draw_popups(d: &mut Fb, g: &Game) {
    for p in g.popups.iter() {
        if p.ttl == 0 {
            continue;
        }
        let mut buf = [b'+', b'0', b'0', b'0', b'0'];
        let mut v = p.value.max(0) as u32;
        let mut i = buf.len();
        if v == 0 {
            i -= 1;
            buf[i] = b'0';
        }
        while v > 0 && i > 1 {
            i -= 1;
            buf[i] = b'0' + (v % 10) as u8;
            v /= 10;
        }
        let s = core::str::from_utf8(&buf[i..]).unwrap_or("+");
        let col = if p.kind == 1 {
            C_GREEN_OK
        } else if p.kind == 2 {
            C_GOLD
        } else {
            C_WHITE
        };
        text(d, s, p.x as i32, p.y as i32, &FONT_6X10, col);
    }
}

fn draw_birds_badge(d: &mut Fb, g: &Game) {
    if g.state == State::GameOver {
        return;
    }
    // міні-птах у куточку + кількість птахів, що лишилися
    field_disc(16, 17, 6, C_RED_DARK);
    field_disc(16, 17, 5, C_RED);
    field_disc(18, 16, 2, C_WHITE);
    field_disc(18, 16, 1, C_BLACK);
    let mut buf = [0u8; 8];
    let mut i = buf.len();
    let mut v = g.birds_left as u32;
    loop {
        i -= 1;
        buf[i] = b'0' + (v % 10) as u8;
        v /= 10;
        if v == 0 || i == 0 {
            break;
        }
    }
    let s = core::str::from_utf8(&buf[i..]).unwrap_or("0");
    text(d, "x", 24, 9, &FONT_9X18_BOLD, C_WHITE);
    text(d, s, 34, 9, &FONT_9X18_BOLD, C_GOLD);
}

fn draw_hint(d: &mut Fb, g: &Game) {
    if g.stage > 1 || g.tick_no > 260 || g.state != State::Aim {
        return;
    }
    let x = 56;
    let y = 6;
    let w = 148;
    fill(d, x, y, w, 32, C_CARD_BG);
    fill(d, x - 1, y - 1, w + 2, 1, C_GOLD);
    fill(d, x - 1, y + 32, w + 2, 1, C_GOLD);
    text_centered(d, "PULL THE BIRD", x + w / 2, 12, &FONT_6X10, C_GOLD);
    text_centered(d, "OR TAP A TARGET", x + w / 2, 25, &FONT_6X10, C_TEXT);
}

fn draw_aim_gauge(d: &mut Fb, g: &Game) {
    if !g.aiming || g.state != State::Aim {
        return;
    }
    let pct = g.power_pct() as i32;
    fill(d, 6, 228, 62, 7, C_BLACK);
    fill(d, 7, 229, 60, 5, C_DIM);
    if pct > 0 {
        let c = if pct > 80 {
            C_RED
        } else if pct > 45 {
            C_GOLD
        } else {
            C_GREEN_OK
        };
        fill(d, 7, 229, (60 * pct) / 100, 5, c);
    }
    let mut b = [0u8; 8];
    text(d, u32_str(g.power_pct() as u32, &mut b), 74, 228, &FONT_6X10, C_TEXT);
}

/* ---------- HUD-панель та картки ---------- */

fn fill(d: &mut Fb, x: i32, y: i32, w: i32, h: i32, c: Rgb565) {
    Rectangle::new(Point::new(x, y), Size::new(w as u32, h as u32))
        .into_styled(PrimitiveStyle::with_fill(c))
        .draw(d)
        .ok();
}

fn text(d: &mut Fb, s: &str, x: i32, y: i32, font: &MonoFont, c: Rgb565) {
    Text::new(
        s,
        Point::new(x, y + font.baseline as i32),
        MonoTextStyle::new(font, c),
    )
    .draw(d)
    .ok();
}

fn text_centered(d: &mut Fb, s: &str, cx: i32, cy: i32, font: &MonoFont, c: Rgb565) {
    let w = s.len() as i32 * font.character_size.width as i32;
    let h = font.character_size.height as i32;
    text(d, s, cx - w / 2, cy - h / 2, font, c);
}

fn u32_str(mut v: u32, buf: &mut [u8; 8]) -> &str {
    let mut i = buf.len();
    loop {
        i -= 1;
        buf[i] = b'0' + (v % 10) as u8;
        v /= 10;
        if v == 0 || i == 0 {
            break;
        }
    }
    core::str::from_utf8(&buf[i..]).unwrap_or("0")
}

fn render_panel(d: &mut Fb, g: &Game, paused: bool, demo_mode: bool) {
    fill(d, PANEL_X, 0, PANEL_W, SCREEN_H, C_PANEL_BG);
    fill(d, PANEL_X, 0, 2, SCREEN_H, BG_PAL[4]);

    text_centered(d, "ANGRY", PANEL_X + PANEL_W / 2 + 1, 10, &FONT_9X18_BOLD, C_RED);
    text_centered(d, "BIRDS", PANEL_X + PANEL_W / 2 + 1, 29, &FONT_9X18_BOLD, C_GOLD);

    let mut b1 = [0u8; 8];
    text(d, "LVL", PANEL_X + 6, 44, &FONT_6X10, C_DIM);
    let lv = u32_str(g.stage as u32, &mut b1);
    text(d, lv, PANEL_X + 34, 44, &FONT_6X10, C_TEXT);
    text(d, "PIGS", PANEL_X + 6, 56, &FONT_6X10, C_DIM);
    let mut b2 = [0u8; 8];
    let pg = u32_str(g.pigs_left() as u32, &mut b2);
    text(d, pg, PANEL_X + 34, 56, &FONT_6X10, C_TEXT);

    text(d, "SCORE", PANEL_X + 6, 68, &FONT_6X10, C_DIM);
    let mut b3 = [0u8; 8];
    text(d, u32_str(g.score, &mut b3), PANEL_X + 6, 78, &FONT_9X18_BOLD, C_TEXT);
    text(d, "BEST", PANEL_X + 6, 100, &FONT_6X10, C_DIM);
    let mut b4 = [0u8; 8];
    text(d, u32_str(g.best, &mut b4), PANEL_X + 6, 110, &FONT_9X18_BOLD, C_GOLD);

    ui_draw_btn(BTN_RETRY, "RTRY", UiFont::Small, C_BTN_RETRY, C_TEXT, BTN_STD_BORDER);
    ui_draw_btn(BTN_NEW, "NEW", UiFont::Small, C_BTN_NEW, C_TEXT, BTN_STD_BORDER);
    if paused {
        ui_draw_btn(BTN_PAUSE, "PLAY", UiFont::Small, C_BTN_PAUSE_ON, C_BLACK, C_GOLD);
    } else {
        ui_draw_btn(BTN_PAUSE, "PAUSE", UiFont::Small, C_BTN_PAUSE, C_TEXT, BTN_STD_BORDER);
    }
    draw_demo_btn(BTN_DEMO, demo_mode, UiFont::Small);
    draw_exit_btn(BTN_EXIT, UiFont::Small);
}

fn render_card(d: &mut Fb, g: &Game) {
    let w = 196;
    let h = 104;
    let x = (FIELD_W - w) / 2;
    let y = (FIELD_H - h) / 2;
    fill(d, x - 2, y - 2, w + 4, h + 4, C_GOLD);
    fill(d, x, y, w, h, C_PANEL_BG);

    let (title, hint, col) = match g.state {
        State::LevelClear => ("LEVEL CLEAR!", "TAP FOR NEXT LEVEL", C_GREEN_OK),
        State::GameOver => ("GAME OVER", "TAP TO PLAY AGAIN", C_RED),
        _ => ("", "", C_GOLD),
    };
    text_centered(d, title, x + w / 2, y + 18, &FONT_10X20, col);

    let mut b1 = [0u8; 8];
    text_centered(
        d,
        u32_str(g.score, &mut b1),
        x + w / 2,
        y + 44,
        &FONT_9X18_BOLD,
        C_TEXT,
    );
    if g.state == State::LevelClear {
        let mut b2 = [0u8; 8];
        text_centered(d, "BONUS", x + w / 2 - 26, y + 66, &FONT_6X10, C_DIM);
        text_centered(
            d,
            u32_str(g.clear_bonus(), &mut b2),
            x + w / 2 + 24,
            y + 66,
            &FONT_6X10,
            C_GOLD,
        );
    }
    text_centered(d, hint, x + w / 2, y + h - 16, &FONT_6X10, C_GOLD);
}

fn render_pause_banner(d: &mut Fb) {
    let w = 150;
    let h = 60;
    let x = (FIELD_W - w) / 2;
    let y = (FIELD_H - h) / 2;
    fill(d, x - 2, y - 2, w + 4, h + 4, C_GOLD);
    fill(d, x, y, w, h, C_PANEL_BG);
    text_centered(d, "PAUSED", x + w / 2, y + 20, &FONT_10X20, C_GOLD);
    text_centered(d, "TAP PLAY OR FIELD", x + w / 2, y + 44, &FONT_6X10, C_TEXT);
}

fn render_field(d: &mut Fb, g: &Game, preview: &[PreviewPt], paused: bool) {
    restore_field_bg();

    for i in 0..g.block_count as usize {
        if g.blocks[i].alive {
            draw_block(&g.blocks[i]);
        }
    }
    for i in 0..g.pig_count as usize {
        if g.pigs[i].alive {
            draw_pig(&g.pigs[i], g.tick_no);
        }
    }

    if g.state == State::Aim && g.birds_left > 0 && !paused {
        let bx = SLING_X + g.pull_x;
        let by = SLING_Y + g.pull_y;
        if g.aiming {
            draw_preview(preview);
        }
        draw_bands(bx, by);
        draw_bird(bx, by);
    } else if g.bird.active {
        draw_bird(g.bird.x_fp / FP, g.bird.y_fp / FP);
    }

    draw_particles(g);
    draw_popups(d, g);
    draw_birds_badge(d, g);
    draw_aim_gauge(d, g);
    draw_hint(d, g);

    if matches!(g.state, State::LevelClear | State::GameOver) {
        render_card(d, g);
    } else if paused {
        render_pause_banner(d);
    }

    fill(d, FIELD_W - 2, 0, 2, SCREEN_H, BG_PAL[4]);
}

#[esp_hal::main]
fn main() -> ! {
    let mut board = Board::init();

    let g = game_mut();
    let (run, failed) = selftest_checks(g);
    println!("BIRDS: selftest {}/{} failed={}", run - failed, run, failed);

    bake_bird_sprite();
    bake_pig_sprite();
    bake_field_bg();
    let mut saved_best = board.load_best(SAVE_FILE);
    g.best = saved_best;
    g.start_new_game(1, board.rng.random());

    let mut paused = false;
    let mut demo_mode = board.start_in_demo(cfg!(feature = "demo"));

    render_field(&mut Fb, g, &[], paused);
    render_panel(&mut Fb, g, paused, demo_mode);
    let t0 = Instant::now();
    paint_all(&mut board);
    println!("BIRDS: старт, кадр = {} ms, demo={}", elapsed_ms(t0), demo_mode);

    #[cfg(feature = "framedump")]
    {
        cyd_c5_common::dump_frame_hex();
    }

    let mut was_pressed = false;
    let mut press_pt = Point::zero();
    #[allow(unused_assignments)]
    let mut last_pt = Point::zero();
    let mut dragging = false;
    let mut drag_mode = false;
    let mut demo_wait = 0u8;
    let mut demo_overlay_wait = 0u8;
    let mut preview = [PreviewPt::default(); 72];

    loop {
        if let Some(ch) = board.poll_uart_byte() {
            match ch {
                b'X' | b'x' | b'Q' | b'q' => {
                    if g.best > saved_best {
                        board.save_best(SAVE_FILE, g.best);
                    }
                    println!("BIRDS: вихід у GameLauncher (best={})", g.best);
                    board.exit_to_launcher();
                }
                b'D' | b'd' => {
                    demo_mode = !demo_mode;
                    render_panel(&mut Fb, g, paused, demo_mode);
                    paint_panel(&mut board);
                }
                _ => {}
            }
        }
        let frame_t0 = Instant::now();
        let mut hud_dirty = false;
        let prev_state = g.state;
        let was_paused = paused;

        let pressed = board.touch.pressed();
        if pressed {
            last_pt = board.touch.point();
            if !was_pressed {
                press_pt = last_pt;
                dragging = false;
                if press_pt.x < FIELD_W && matches!(g.state, State::Aim) && g.birds_left > 0 && !paused {
                    dragging = true;
                    let dx = press_pt.x - SLING_X;
                    let dy = press_pt.y - SLING_Y;
                    drag_mode = dx * dx + dy * dy <= DRAG_R2;
                    if drag_mode {
                        g.aim_drag(press_pt.x, press_pt.y);
                    } else {
                        g.aim_point(press_pt.x, press_pt.y);
                    }
                }
            } else if dragging {
                if drag_mode {
                    g.aim_drag(last_pt.x, last_pt.y);
                } else {
                    g.aim_point(last_pt.x, last_pt.y);
                }
            }
        } else if was_pressed {
            if press_pt.x >= FIELD_W {
                if BTN_EXIT.contains(press_pt) {
                    if g.best > saved_best {
                        board.save_best(SAVE_FILE, g.best);
                    }
                    board.exit_to_launcher();
                } else if BTN_RETRY.contains(press_pt) {
                    paused = false;
                    demo_wait = 0;
                    demo_overlay_wait = 0;
                    let base = g.stage_base_score;
                    g.score = base;
                    let stage = g.stage;
                    g.init_stage(stage);
                    hud_dirty = true;
                } else if BTN_NEW.contains(press_pt) {
                    paused = false;
                    demo_wait = 0;
                    demo_overlay_wait = 0;
                    if g.best > saved_best {
                        saved_best = g.best;
                        board.save_best(SAVE_FILE, saved_best);
                    }
                    g.start_new_game(1, board.rng.random());
                    hud_dirty = true;
                } else if BTN_PAUSE.contains(press_pt) {
                    if matches!(g.state, State::Aim | State::Flying | State::Settle) {
                        paused = !paused;
                        hud_dirty = true;
                    }
                } else if BTN_DEMO.contains(press_pt) {
                    demo_mode = !demo_mode;
                    demo_wait = 0;
                    demo_overlay_wait = 0;
                    if demo_mode && paused {
                        paused = false;
                    }
                    hud_dirty = true;
                }
                dragging = false;
            } else if dragging {
                if matches!(g.state, State::Aim) && g.power_pct() >= 12 {
                    if g.launch() {
                        hud_dirty = true;
                    }
                } else {
                    g.cancel_aim();
                }
                dragging = false;
            } else if paused {
                paused = false;
            } else {
                match g.state {
                    State::LevelClear => {
                        demo_overlay_wait = 0;
                        let next = g.stage.saturating_add(1);
                        g.init_stage(next);
                        hud_dirty = true;
                    }
                    State::GameOver => {
                        demo_overlay_wait = 0;
                        g.start_new_game(1, board.rng.random());
                        hud_dirty = true;
                    }
                    _ => {}
                }
            }
        }
        was_pressed = pressed;

        if was_paused != paused {
            render_field(&mut Fb, g, &[], paused);
            render_panel(&mut Fb, g, paused, demo_mode);
            paint_all(&mut board);
        }

        if paused {
            board.delay.delay_millis(20);
            continue;
        }

        if demo_mode && !pressed {
            match g.state {
                State::Aim => {
                    if demo_wait == 0 {
                        if g.tick_no.is_multiple_of(30)
                            && let Some((px, py)) = g.find_demo_shot()
                            && g.set_pull(px, py)
                        {
                            demo_wait = 14;
                        }
                    } else {
                        demo_wait -= 1;
                        if demo_wait == 0 && g.launch() {
                            hud_dirty = true;
                        }
                    }
                }
                State::LevelClear => {
                    demo_wait = 0;
                    demo_overlay_wait += 1;
                    if demo_overlay_wait >= 60 {
                        demo_overlay_wait = 0;
                        let next = g.stage.saturating_add(1);
                        g.init_stage(next);
                        hud_dirty = true;
                    }
                }
                State::GameOver => {
                    demo_wait = 0;
                    demo_overlay_wait += 1;
                    if demo_overlay_wait >= 60 {
                        demo_overlay_wait = 0;
                        g.start_new_game(1, board.rng.random());
                        hud_dirty = true;
                    }
                }
                _ => {}
            }
        }

        let ev = g.tick();
        if ev.hud_dirty {
            hud_dirty = true;
        }
        if (ev.stage_clear || ev.game_over) && g.best > saved_best {
            saved_best = g.best;
            board.save_best(SAVE_FILE, saved_best);
        }

        let show_preview = dragging || (demo_mode && demo_wait > 0);
        let n = if show_preview {
            g.preview(&mut preview)
        } else {
            0
        };

        render_field(&mut Fb, g, &preview[..n], paused);
        paint_field(&mut board);

        if hud_dirty || g.state != prev_state {
            render_panel(&mut Fb, g, paused, demo_mode);
            paint_panel(&mut board);
        }

        let spent = elapsed_ms(frame_t0);
        if spent < TARGET_FRAME_MS {
            board.delay.delay_millis(TARGET_FRAME_MS - spent);
        }
    }
}
