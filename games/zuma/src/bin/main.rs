//! **Zuma** для плати **NM-CYD-C5** у складі **CYD-C5 Arcade** (використовує `cyd-c5-common`).

#![no_std]
#![no_main]

use cyd_c5_common::{
    Board, Fb, SCREEN_H, SCREEN_W, fb_mut,
    ui::{
        BTN_STD_BORDER, BtnRect, FontKind as UiFont, draw_btn as ui_draw_btn, draw_demo_btn,
        draw_exit_btn, elapsed_ms,
    },
};
use cyd_game_zuma::game::{
    BALL_R, Bonus, FIELD_H, FIELD_W, FP, Game, MAX_COLORS, State, Track, isqrt, selftest_checks,
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
    "cyd-game-zuma",
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
    println!("ZUMA PANIC: {:?}", info);
    loop {}
}

const SAVE_FILE: &str = "ZUMA.SAV";

const PANEL_X: i32 = FIELD_W; // 240
const PANEL_W: i32 = SCREEN_W - PANEL_X; // 80
const FB_W: usize = SCREEN_W as usize;
const TARGET_FRAME_MS: u32 = 30;

const BTN_SWAP: BtnRect = BtnRect::new(PANEL_X + 4, 136, 35, 22);
const BTN_PAUSE: BtnRect = BtnRect::new(PANEL_X + 41, 136, 35, 22);
const BTN_NEW: BtnRect = BtnRect::new(PANEL_X + 4, 161, 72, 22);
const BTN_DEMO: BtnRect = BtnRect::new(PANEL_X + 4, 186, 72, 22);
const BTN_EXIT: BtnRect = BtnRect::new(PANEL_X + 4, 211, 72, 24);

const FROG_SWAP_R2: i32 = 22 * 22;

const C_TRANSPARENT: Rgb565 = Rgb565::new(0, 1, 0);

const C_PANEL_BG: Rgb565 = Rgb565::new(2, 6, 5);
const C_CARD_BG: Rgb565 = Rgb565::new(3, 10, 8);
const C_GOLD: Rgb565 = Rgb565::new(31, 50, 6);
const C_GREEN_OK: Rgb565 = Rgb565::new(6, 58, 12);
const C_TEXT: Rgb565 = Rgb565::new(29, 59, 30);
const C_DIM: Rgb565 = Rgb565::new(15, 36, 20);
const C_BTN_SWAP: Rgb565 = Rgb565::new(6, 28, 18);
const C_BTN_PAUSE: Rgb565 = Rgb565::new(9, 22, 16);
const C_BTN_PAUSE_ON: Rgb565 = Rgb565::new(28, 42, 4);
const C_BTN_NEW: Rgb565 = Rgb565::new(5, 24, 24);
const C_WHITE: Rgb565 = Rgb565::new(31, 63, 31);
const C_BLACK: Rgb565 = Rgb565::new(1, 2, 1);
const C_RED_WARN: Rgb565 = Rgb565::new(31, 6, 4);

const BG_PAL: [Rgb565; 8] = [
    Rgb565::new(3, 10, 7),
    Rgb565::new(2, 6, 4),
    Rgb565::new(5, 14, 9),
    Rgb565::new(8, 21, 13),
    Rgb565::new(1, 4, 3),
    Rgb565::new(2, 7, 5),
    Rgb565::new(5, 15, 10),
    Rgb565::new(6, 17, 11),
];

const FIELD_PACKED_LEN: usize = (FIELD_W as usize * FIELD_H as usize) / 2;
const SPR_SZ: usize = 15;

static mut FIELD_BG: [u8; FIELD_PACKED_LEN] = [0u8; FIELD_PACKED_LEN];
static mut BALL_SPRITES: [[Rgb565; SPR_SZ * SPR_SZ]; MAX_COLORS as usize] =
    [[C_TRANSPARENT; SPR_SZ * SPR_SZ]; MAX_COLORS as usize];
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
fn sprites_mut() -> &'static mut [[Rgb565; SPR_SZ * SPR_SZ]; MAX_COLORS as usize] {
    unsafe { &mut *core::ptr::addr_of_mut!(BALL_SPRITES) }
}

#[inline]
fn sprites_ref() -> &'static [[Rgb565; SPR_SZ * SPR_SZ]; MAX_COLORS as usize] {
    unsafe { &*core::ptr::addr_of!(BALL_SPRITES) }
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

/* ---------- запікання спрайтів куль і фону траси ---------- */

fn base_ball_rgb(color: u8) -> (u8, u8, u8) {
    // 5-6-5 компоненти (R: 0..31, G: 0..63, B: 0..31)
    match color {
        1 => (30, 6, 6),   // рубіново-червоний
        2 => (4, 52, 10),  // смарагдово-зелений
        3 => (5, 18, 31),  // сапфірово-синій
        4 => (31, 52, 3),  // сонячно-золотий
        5 => (26, 6, 28),  // аметистово-пурпуровий
        _ => (3, 54, 28),  // бірюзовий
    }
}

pub fn ball_color_565(color: u8) -> Rgb565 {
    let (r, g, b) = base_ball_rgb(color);
    Rgb565::new(r, g, b)
}

fn scale_rgb(r: u8, g: u8, b: u8, num: u32, den: u32) -> Rgb565 {
    let nr = ((r as u32 * num) / den).min(31) as u8;
    let ng = ((g as u32 * num) / den).min(63) as u8;
    let nb = ((b as u32 * num) / den).min(31) as u8;
    Rgb565::new(nr, ng, nb)
}

/// Чи належить піксель `(dx, dy)` відмітному символу в центрі кулі кольору `color`.
fn glyph_pixel(color: u8, dx: i32, dy: i32) -> Option<Rgb565> {
    let light = C_WHITE;
    let dark = Rgb565::new(2, 4, 2);
    match color {
        1 => {
            // Червона: білий ромб у центрі
            if dx.abs() + dy.abs() <= 2 {
                Some(light)
            } else {
                None
            }
        }
        2 => {
            // Зелена: контрастна горизонтальна смужка
            if dy.abs() <= 1 && dx.abs() <= 3 {
                Some(dark)
            } else {
                None
            }
        }
        3 => {
            // Синя: білий плюсик '+'
            if (dx == 0 && dy.abs() <= 3) || (dy == 0 && dx.abs() <= 3) {
                Some(light)
            } else {
                None
            }
        }
        4 => {
            // Жовта: темне кільце
            let d2 = dx * dx + dy * dy;
            if (4..=9).contains(&d2) {
                Some(dark)
            } else {
                None
            }
        }
        5 => {
            // Пурпурова: білий діагональний хрестик 'X'
            if dx.abs() <= 2 && dy.abs() <= 2 && dx.abs() == dy.abs() {
                Some(light)
            } else {
                None
            }
        }
        _ => {
            // Бірюзова: темний трикутник вгору '^'
            if (-2..=2).contains(&dy) && dx.abs() <= (dy + 2) / 2 + 1 && (dy == 2 || dx.abs() == (dy + 2) / 2) {
                Some(dark)
            } else {
                None
            }
        }
    }
}

/// Один раз на старті запекти 6 об'ємних сферичних спрайтів `15 × 15` (без розмиття зовнішнього краю!).
fn bake_ball_sprites() {
    let sprs = sprites_mut();
    let c_half = (SPR_SZ as i32) / 2; // 7
    for (idx, spr) in sprs.iter_mut().enumerate().take(MAX_COLORS as usize) {
        let col_id = (idx + 1) as u8;
        let (br, bg, bb) = base_ball_rgb(col_id);
        for py in 0..SPR_SZ {
            for px in 0..SPR_SZ {
                let dx = px as i32 - c_half;
                let dy = py as i32 - c_half;
                let d2 = dx * dx + dy * dy;
                let out = if d2 > 51 {
                    C_TRANSPARENT
                } else if let Some(gc) = glyph_pixel(col_id, dx, dy) {
                    gc
                } else if d2 >= 40 {
                    // Чіткий темний обідок сфери (без напівпрозорого AA об фон!)
                    scale_rgb(br, bg, bb, 52, 100)
                } else {
                    let ldx = dx + 2;
                    let ldy = dy + 3;
                    let ld2 = ldx * ldx + ldy * ldy;
                    if ld2 <= 3 {
                        Rgb565::new((br + 16).min(31), (bg + 32).min(63), (bb + 16).min(31))
                    } else if ld2 <= 16 {
                        scale_rgb(br, bg, bb, 128, 100)
                    } else if ld2 >= 44 {
                        scale_rgb(br, bg, bb, 72, 100)
                    } else {
                        Rgb565::new(br, bg, bb)
                    }
                };
                spr[py * SPR_SZ + px] = out;
            }
        }
    }
}

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

/// Запекти кам'яну підлогу храму + вівтар жабки + тришаровий рельєфний жолоб траси у `FIELD_BG`.
fn bake_field_bg(track: &Track) {
    let bg = field_bg_mut();

    // 1. Кам'яна кладка храму (плити 24×16 зі зміщенням рядів)
    for y in 0..FIELD_H {
        let row_id = y / 16;
        let x_off = (row_id & 1) * 12;
        let ry = y % 16;
        for x in 0..FIELD_W {
            let rx = (x + x_off) % 24;
            let c = if ry == 0 || rx == 0 {
                1 // темний шов
            } else if ry == 1 || rx == 1 {
                2 // світла фаска
            } else {
                0
            };
            bg_set_px(bg, x, y, c);
        }
    }

    // 2. Різьблений сонячний вівтар під жабкою (з 8 кам'яними зубцями по колу)
    bg_disc(bg, track.frog_x, track.frog_y, 25, 1);
    bg_disc(bg, track.frog_x, track.frog_y, 24, 3);
    for (ox, oy) in [
        (24, 0),
        (-24, 0),
        (0, 24),
        (0, -24),
        (17, 17),
        (-17, 17),
        (17, -17),
        (-17, -17),
    ] {
        bg_disc(bg, track.frog_x + ox, track.frog_y + oy, 2, 6);
    }
    bg_disc(bg, track.frog_x, track.frog_y, 22, 7);
    bg_disc(bg, track.frog_x, track.frog_y, 19, 4);

    // 3. Тришаровий кам'яний жолоб уздовж усієї дуги `0..=total_s`
    // Спочатку зовнішній бортик (r=9), потім тінь (r=8), потім дно (r=6).
    let step = 2 * FP;
    let mut s = 0;
    while s <= track.total_s {
        let (x, y) = track.pos_px(s);
        bg_disc(bg, x, y, 9, 3);
        s += step;
    }
    s = 0;
    while s <= track.total_s {
        let (x, y) = track.pos_px(s);
        bg_disc(bg, x, y, 8, 4);
        s += step;
    }
    s = 0;
    while s <= track.total_s {
        let (x, y) = track.pos_px(s);
        bg_disc(bg, x, y, 6, 5);
        s += step;
    }
    // Кам'яні насічки по центру жолоба кожні 18 px
    s = 9 * FP;
    while s < track.total_s - 8 * FP {
        let (x, y) = track.pos_px(s);
        bg_disc(bg, x, y, 1, 6);
        s += 18 * FP;
    }
}

/// Швидко розпакувати запечене тло `FIELD_BG` (240×240) у ліву частину `FB` (~0.4 мс).
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

/* ---------- графічні примітиви на полі `240×240` ---------- */

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

fn draw_ball_sprite(cx: i32, cy: i32, color: u8, bonus: Bonus, tick_no: u32) {
    if !(1..=MAX_COLORS).contains(&color) {
        return;
    }
    let spr = &sprites_ref()[(color - 1) as usize];
    let half = (SPR_SZ as i32) / 2;
    let x0 = cx - half;
    let y0 = cy - half;
    let fb = fb_mut();
    for sy in 0..SPR_SZ {
        let py = y0 + sy as i32;
        if !(0..FIELD_H).contains(&py) {
            continue;
        }
        let row = py as usize * FB_W;
        let srow = sy * SPR_SZ;
        for sx in 0..SPR_SZ {
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

    // Яскрава індикація бонусу поверх кулі
    if bonus != Bonus::None {
        let blink = (tick_no & 4) == 0;
        let ring_c = match bonus {
            Bonus::Slow => {
                if blink {
                    C_WHITE
                } else {
                    Rgb565::new(8, 56, 31)
                }
            }
            Bonus::Reverse => {
                if blink {
                    C_GOLD
                } else {
                    C_WHITE
                }
            }
            Bonus::Bomb | Bonus::None => {
                if blink {
                    C_RED_WARN
                } else {
                    C_GOLD
                }
            }
        };
        field_ring(cx, cy, BALL_R - 1, BALL_R, ring_c);
        match bonus {
            Bonus::Slow => {
                // Пісочний годинник / 'S'
                for dx in -2..=2 {
                    put_field_px(cx + dx, cy - 2, C_WHITE);
                    put_field_px(cx + dx, cy, C_WHITE);
                    put_field_px(cx + dx, cy + 2, C_WHITE);
                }
            }
            Bonus::Reverse => {
                // Стрілка назад '<<'
                for k in 0..=2 {
                    put_field_px(cx - 2 + k, cy - k, C_WHITE);
                    put_field_px(cx - 2 + k, cy + k, C_WHITE);
                    put_field_px(cx + 1 + k, cy - k, C_WHITE);
                    put_field_px(cx + 1 + k, cy + k, C_WHITE);
                }
            }
            Bonus::Bomb => {
                // Зірка детонатора '*'
                for d in -3..=3 {
                    put_field_px(cx + d, cy, C_WHITE);
                    put_field_px(cx, cy + d, C_WHITE);
                }
                for d in -2..=2 {
                    put_field_px(cx + d, cy + d, C_GOLD);
                    put_field_px(cx + d, cy - d, C_GOLD);
                }
            }
            Bonus::None => {}
        }
    }
}

/// Куля довільного положення на весь екран (зокрема прев'ю `NEXT` на правій панелі).
fn draw_ball_anywhere(cx: i32, cy: i32, color: u8) {
    if !(1..=MAX_COLORS).contains(&color) {
        return;
    }
    let spr = &sprites_ref()[(color - 1) as usize];
    let half = (SPR_SZ as i32) / 2;
    let fb = fb_mut();
    for sy in 0..SPR_SZ {
        let py = cy - half + sy as i32;
        if !(0..SCREEN_H).contains(&py) {
            continue;
        }
        for sx in 0..SPR_SZ {
            let px = cx - half + sx as i32;
            if !(0..SCREEN_W).contains(&px) {
                continue;
            }
            let c = spr[sy * SPR_SZ + sx];
            if c != C_TRANSPARENT {
                fb[py as usize * FB_W + px as usize] = c;
            }
        }
    }
}

/// Кам'яна арка тунелю, звідки викочується поїзд.
fn draw_tunnel(track: &Track) {
    let (sx, sy) = track.start_px();
    field_disc(sx, sy, 10, Rgb565::new(6, 15, 10));
    field_ring(sx, sy, 8, 10, Rgb565::new(11, 27, 17));
    field_disc(sx, sy, 7, C_BLACK);
}

/// Череп-лунка наприкінці траси: роззявляє щелепу й світить очима при наближенні голови поїзда.
fn draw_skull(track: &Track, danger_pct: u8, tick_no: u32) {
    let (sx, sy) = track.skull_px();
    let jaw_drop = (danger_pct as i32 * 5) / 100; // 0..5 px

    // Чорна прірва під черепом
    field_disc(sx, sy + 2, 11, C_BLACK);
    field_ring(sx, sy + 2, 10, 12, Rgb565::new(12, 26, 15));

    // Нижня щелепа з зубами
    let jy = sy + 5 + jaw_drop;
    for dy in 0..=3 {
        for dx in -6..=6 {
            put_field_px(sx + dx, jy + dy, Rgb565::new(22, 46, 22));
        }
    }
    for tx in [-4, -1, 2, 5] {
        put_field_px(sx + tx, jy - 1, C_WHITE);
        put_field_px(sx + tx - 1, jy - 1, C_WHITE);
    }

    // Купол черепа (золотисто-кістяний)
    field_disc(sx, sy - 2, 9, Rgb565::new(26, 53, 24));
    field_ring(sx, sy - 2, 8, 9, Rgb565::new(16, 34, 14));

    // Очниці (спалахують червоним при небезпеці!)
    let eye_c = if danger_pct > 0 && ((tick_no & 4) == 0 || danger_pct > 65) {
        C_RED_WARN
    } else {
        C_BLACK
    };
    field_disc(sx - 4, sy - 3, 2, eye_c);
    field_disc(sx + 4, sy - 3, 2, eye_c);
    // Ніс
    put_field_px(sx, sy, C_BLACK);
    put_field_px(sx - 1, sy + 1, C_BLACK);
    put_field_px(sx + 1, sy + 1, C_BLACK);
}

/// Різьблена ацтекська жабка-ідол у локальних координатах `(f, s)` з кроком `1/4` пікселя:
/// мускулисті задні й передні лапки, рельєфний нефритовий тулуб із золотим орнаментом,
/// віддача при пострілі, куля затиснута просто в пащі під верхньою губою, самоцвіт у золотій
/// оправі на спині та виразні опуклі очі, що кліпають.
fn draw_frog(g: &Game) {
    let (ux, uy) = g.frog_dir;
    let ulen = isqrt((ux * ux + uy * uy) as u64).max(1) as i32;
    let fx_u = (ux * 256) / ulen;
    let fy_u = (uy * 256) / ulen;

    // Пружна віддача на 1..2 px назад у момент пострілу
    let recoil = if g.fire_cd >= 5 {
        2
    } else if g.fire_cd >= 3 {
        1
    } else {
        0
    };
    let cx = g.track.frog_x - (fx_u * recoil) / 256;
    let cy = g.track.frog_y - (fy_u * recoil) / 256;

    let c_outline = Rgb565::new(1, 5, 3);
    let c_jade_dark = Rgb565::new(4, 18, 9);
    let c_jade_mid = Rgb565::new(7, 30, 14);
    let c_jade_lit = Rgb565::new(11, 41, 19);
    let c_jade_hi = Rgb565::new(16, 50, 24);
    let c_gold_dark = Rgb565::new(19, 32, 4);
    let c_gold_hi = Rgb565::new(30, 50, 8);
    let c_throat = Rgb565::new(1, 3, 2);

    // Прохід 1: лапки, тулуб, морда, золотий візерунок на спині та темна глибина пащі
    for dy in -23..=23 {
        for dx in -23..=23 {
            // (f, s) у четвертинах пікселя (1 px = 4 одиниці)
            let f = (dx * fx_u + dy * fy_u) >> 6;
            let s = (-dx * fy_u + dy * fx_u) >> 6;
            let a = s.abs();
            let lit = -(dx + dy); // освітлення зверху-зліва

            // Тулуб (грушоподібний овал, ширший у плечах) + витягнута вперед морда
            let bw = (51 + f / 7).clamp(36, 60);
            let torso_d = ((f + 6) * (f + 6) * 100) / (58 * 58) + (a * a * 100) / (bw * bw);
            let snout_d = ((f - 30) * (f - 30) * 100) / (36 * 36) + (a * a * 100) / (38 * 38);
            let body_d = torso_d.min(snout_d);

            // Складені задні стегна + перетинчасті лапки
            let uh = ((f + 22) * 7 + (a - 53) * 4) / 8;
            let vh = (-(f + 22) * 4 + (a - 53) * 7) / 8;
            let hind_d = (uh * uh * 100) / (25 * 25) + (vh * vh * 100) / (14 * 14);
            let foot_d =
                ((f + 4) * (f + 4) * 100) / (15 * 15) + ((a - 64) * (a - 64) * 100) / (11 * 11);
            let back_leg_d = hind_d.min(foot_d);

            // Передні лапки, що охоплюють край вівтаря біля пащі
            let ua = ((f - 24) * 7 - (a - 49) * 4) / 8;
            let va = ((f - 24) * 4 + (a - 49) * 7) / 8;
            let arm_d = (ua * ua * 100) / (21 * 21) + (va * va * 100) / (12 * 12);
            let limb_d = back_leg_d.min(arm_d);

            if body_d <= 100 {
                // Відкрита паща спереду
                let mouth_d =
                    ((f - 50) * (f - 50) * 100) / (25 * 25) + (a * a * 100) / (30 * 30);
                let col = if mouth_d <= 78 {
                    c_throat
                } else if mouth_d <= 102 && f >= 34 {
                    c_gold_dark
                } else if body_d >= 80 {
                    c_outline
                } else {
                    // Суцільний золотий ацтекський комір позаду очей та хребетний гребінь
                    let collar_f = -8 + (a * a) / 95;
                    let on_collar = a <= 42 && (f - collar_f).abs() <= 5;
                    let on_brow_gem = (f - 16) * (f - 16) + a * a <= 20;
                    // Темні різьблені цятки на боках спини
                    let spot1 = (f + 20) * (f + 20) + (a - 32) * (a - 32) <= 36;
                    let spot2 = (f + 34) * (f + 34) + (a - 22) * (a - 22) <= 25;

                    if on_collar || on_brow_gem {
                        if lit > 2 {
                            c_gold_hi
                        } else {
                            c_gold_dark
                        }
                    } else if spot1 || spot2 || body_d >= 64 || lit < -12 {
                        c_jade_dark
                    } else if lit > 12 {
                        c_jade_hi
                    } else if lit > 0 {
                        c_jade_lit
                    } else {
                        c_jade_mid
                    }
                };
                put_field_px(cx + dx, cy + dy, col);
            } else if limb_d <= 100 {
                let col = if limb_d >= 74 || body_d <= 116 {
                    c_outline
                } else if lit > 6 && limb_d < 45 {
                    c_jade_lit
                } else if lit < -6 {
                    c_jade_dark
                } else {
                    c_jade_mid
                };
                put_field_px(cx + dx, cy + dy, col);
            }
        }
    }

    // Прохід 2: поточна куля, вкладена у відкриту пащу (+12 px уперед)
    if matches!(g.state, State::Playing) {
        let mx = cx + (fx_u * 12) / 256;
        let my = cy + (fy_u * 12) / 256;
        draw_ball_sprite(mx, my, g.cur_color, Bonus::None, g.tick_no);
    }

    // Прохід 3: верхня губа з ніздрями (нависає над краєм кулі!), самоцвіт у золотій оправі на спині
    // та опуклі очі з зіницями, бліками й періодичним кліпанням
    let blinking = (g.tick_no % 140) == 90 || (g.tick_no % 140) == 91;
    let (nr, ng, nb) = base_ball_rgb(g.next_color);
    let gem_cx_q = (-32 * fx_u) >> 8;
    let gem_cy_q = (-32 * fy_u) >> 8;

    for dy in -21..=21 {
        for dx in -21..=21 {
            let f = (dx * fx_u + dy * fy_u) >> 6;
            let s = (-dx * fy_u + dy * fx_u) >> 6;
            let a = s.abs();
            let lit = -(dx + dy);

            // 3а. Верхня губа морди (f = 14..28) з золотою облямівкою та двома ніздрями
            if (14..=29).contains(&f) && a <= 26 {
                let arch = ((f - 14) * (f - 14)) / 8 + (a * a) / 25;
                if arch <= 27 {
                    let nostril = (f - 22) * (f - 22) + (a - 9) * (a - 9) <= 5;
                    let col = if nostril {
                        c_outline
                    } else if arch >= 21 && f >= 23 {
                        c_gold_hi
                    } else if lit > 4 {
                        c_jade_lit
                    } else {
                        c_jade_mid
                    };
                    put_field_px(cx + dx, cy + dy, col);
                }
            }

            // 3б. Самоцвіт наступної кулі на спині (f = -32, s = 0) у золотій оправі
            let bf = f + 32;
            let bd2 = bf * bf + s * s;
            if bd2 <= 23 * 23 {
                let col = if bd2 >= 20 * 20 {
                    c_outline
                } else if bd2 >= 15 * 15 {
                    if lit > 0 {
                        c_gold_hi
                    } else {
                        c_gold_dark
                    }
                } else if bd2 >= 12 * 12 {
                    scale_rgb(nr, ng, nb, 55, 100)
                } else {
                    // Блік завжди зверху-зліва у екранних координатах
                    let glx = dx * 4 - gem_cx_q + 5;
                    let gly = dy * 4 - gem_cy_q + 5;
                    if glx * glx + gly * gly <= 26 {
                        C_WHITE
                    } else if lit > 2 {
                        scale_rgb(nr, ng, nb, 125, 100)
                    } else {
                        Rgb565::new(nr, ng, nb)
                    }
                };
                put_field_px(cx + dx, cy + dy, col);
            }

            // 3в. Опуклі очі жабки з кам'яними повіками (f = +4, s = ±35)
            let ef = f - 4;
            let ea = a - 35;
            let ed2 = ef * ef + ea * ea;
            if ed2 <= 19 * 19 {
                let col = if ed2 >= 15 * 15 {
                    c_outline
                } else if a >= 43 {
                    // Зовнішній нефритовий надбрівний валик
                    if lit > 0 {
                        c_jade_lit
                    } else {
                        c_jade_dark
                    }
                } else if blinking {
                    if ef.abs() <= 2 {
                        c_outline
                    } else {
                        c_jade_mid
                    }
                } else {
                    // Зіниця зміщена вперед у бік прицілу (f = +9, a = 34)
                    let pf = f - 9;
                    let pa = a - 34;
                    let pd2 = pf * pf + pa * pa;
                    // Блік на оці зверху-зліва відносно ока
                    let glint = (pf + 4) * (pf + 4) + (pa + 4) * (pa + 4) <= 10;
                    if glint {
                        C_WHITE
                    } else if pd2 <= 8 * 8 {
                        c_outline
                    } else if ed2 >= 11 * 11 {
                        Rgb565::new(22, 45, 7)
                    } else {
                        Rgb565::new(30, 60, 14)
                    }
                };
                put_field_px(cx + dx, cy + dy, col);
            }
        }
    }
}

/// Пунктирна лінія прицілу під час утримання пальця/стилуса на полі.
fn draw_aim_sight(g: &Game) {
    let (hx, hy) = g.raycast_aim();
    let (ux, uy) = g.frog_dir;
    let sx = g.track.frog_x + (ux * 21) / FP;
    let sy = g.track.frog_y + (uy * 21) / FP;
    let dx = hx - sx;
    let dy = hy - sy;
    let dist = isqrt((dx as i64 * dx as i64 + dy as i64 * dy as i64) as u64) as i32;
    if dist < 6 {
        return;
    }
    let col = ball_color_565(g.cur_color);
    let mut d = 0;
    while d < dist {
        let x = sx + dx * d / dist;
        let y = sy + dy * d / dist;
        field_disc(x, y, 1, C_WHITE);
        put_field_px(x, y, col);
        d += 8;
    }
    field_ring(hx, hy, 4, 5, C_GOLD);
}

/// Спалахи вибухів та спливаючі комбо `x2!`, `x3!`.
fn draw_fx(d: &mut Fb, g: &Game) {
    for f in &g.fx {
        if !f.active {
            continue;
        }
        let age = (f.max_ttl - f.ttl) as i32;
        let r = 4 + age * 2;
        let col = if (f.ttl & 1) == 0 {
            C_WHITE
        } else {
            ball_color_565(f.color)
        };
        field_ring(f.x as i32, f.y as i32, (r - 2).max(1), r, col);
        if f.combo > 1 {
            let mut buf = [b'x', b'0' + f.combo.min(9), b'!'];
            if f.combo >= 10 {
                buf = *b"x9+";
            }
            let s = core::str::from_utf8(&buf).unwrap_or("x2!");
            let tx = (f.x as i32 - 12).clamp(2, FIELD_W - 30);
            let ty = (f.y as i32 - 8 - age * 2).clamp(2, FIELD_H - 18);
            text(d, s, tx, ty, &FONT_9X18_BOLD, C_GOLD);
        }
    }
}

/// Намалювати все ігрове поле `240 × 240` у лівий бік `FB`.
fn render_field(d: &mut Fb, g: &Game, show_aim: bool) {
    restore_field_bg();
    draw_tunnel(&g.track);
    draw_skull(&g.track, g.danger_pct(), g.tick_no);

    if show_aim && matches!(g.state, State::Playing) {
        draw_aim_sight(g);
    }

    // Кулі ланцюга (від хвоста до голови)
    for i in 0..g.ball_count {
        let b = g.balls[i];
        if b.s < -4 * FP || b.s > g.track.total_s {
            continue;
        }
        let (px, py) = g.track.pos_px(b.s.max(0));
        draw_ball_sprite(px, py, b.color, b.bonus, g.tick_no);
    }

    // Кулі-снаряди в польоті
    for b in &g.bullets {
        if b.active {
            let px = (b.x_fp + FP / 2) / FP;
            let py = (b.y_fp + FP / 2) / FP;
            draw_ball_sprite(px, py, b.color, Bonus::None, g.tick_no);
        }
    }

    draw_frog(g);
    draw_fx(d, g);
}

/* ---------- права HUD-панель `80 × 240` та оверлеї ---------- */

fn fill(d: &mut Fb, x: i32, y: i32, w: i32, h: i32, c: Rgb565) {
    Rectangle::new(Point::new(x, y), Size::new(w as u32, h as u32))
        .into_styled(PrimitiveStyle::with_fill(c))
        .draw(d)
        .ok();
}

fn text(d: &mut Fb, s: &str, x: i32, y: i32, font: &MonoFont, c: Rgb565) {
    Text::new(s, Point::new(x, y + font.baseline as i32), MonoTextStyle::new(font, c))
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
    fill(d, PANEL_X, 0, 2, SCREEN_H, BG_PAL[3]);

    text_centered(d, "ZUMA", PANEL_X + PANEL_W / 2 + 1, 9, &FONT_9X18_BOLD, C_GOLD);

    let mut sb = [0u8; 8];
    let mut stg_line = *b"STAGE 00";
    let ss = u32_str(g.stage as u32, &mut sb).as_bytes();
    let mut len = 6;
    for &ch in ss {
        if len < stg_line.len() {
            stg_line[len] = ch;
            len += 1;
        }
    }
    let stg_s = core::str::from_utf8(&stg_line[..len]).unwrap_or("STAGE");
    text(d, stg_s, PANEL_X + 6, 20, &FONT_6X10, C_TEXT);

    let bar_x = PANEL_X + 6;
    let bar_y = 32;
    let bar_w = PANEL_W - 10;
    let bar_h = 10;
    fill(d, bar_x, bar_y, bar_w, bar_h, C_DIM);
    fill(d, bar_x + 1, bar_y + 1, bar_w - 2, bar_h - 2, C_CARD_BG);
    let fill_w =
        ((bar_w - 2) * g.zuma_meter.min(g.zuma_target) as i32) / g.zuma_target.max(1) as i32;
    let bar_col = if g.zuma_achieved { C_GREEN_OK } else { C_GOLD };
    if fill_w > 0 {
        fill(d, bar_x + 1, bar_y + 1, fill_w, bar_h - 2, bar_col);
    }
    if g.zuma_achieved {
        text_centered(d, "ZUMA!", bar_x + bar_w / 2, bar_y + bar_h / 2, &FONT_6X10, C_BLACK);
    }

    text(d, "SCORE", PANEL_X + 6, 45, &FONT_6X10, C_DIM);
    let mut b1 = [0u8; 8];
    text(d, u32_str(g.score, &mut b1), PANEL_X + 6, 55, &FONT_9X18_BOLD, C_TEXT);

    text(d, "BEST", PANEL_X + 6, 74, &FONT_6X10, C_DIM);
    let mut b2 = [0u8; 8];
    text(d, u32_str(g.best, &mut b2), PANEL_X + 6, 84, &FONT_9X18_BOLD, C_GOLD);

    text(d, "NEXT", PANEL_X + 6, 109, &FONT_6X10, C_DIM);
    fill(d, PANEL_X + 44, 103, 24, 20, C_CARD_BG);
    draw_ball_anywhere(PANEL_X + 56, 113, g.next_color);

    if paused {
        text(d, "PAUSED", PANEL_X + 6, 124, &FONT_6X10, C_GOLD);
    } else if g.combo_ttl > 0 && g.last_combo > 1 {
        let mut cb = [b'C', b'O', b'M', b'B', b'O', b' ', b'x', b'0' + g.last_combo.min(9)];
        if g.last_combo >= 10 {
            cb[7] = b'9';
        }
        let cs = core::str::from_utf8(&cb).unwrap_or("COMBO");
        text(d, cs, PANEL_X + 6, 124, &FONT_6X10, C_GOLD);
    } else if g.slow_ticks > 0 {
        text(d, "SLOW!", PANEL_X + 6, 124, &FONT_6X10, Rgb565::new(10, 56, 31));
    } else if g.recoil_ticks > 0 {
        text(d, "REVERSE!", PANEL_X + 6, 124, &FONT_6X10, C_GREEN_OK);
    } else if demo_mode {
        text(d, "AUTO ON", PANEL_X + 6, 124, &FONT_6X10, C_GREEN_OK);
    }

    ui_draw_btn(BTN_SWAP, "SWAP", UiFont::Small, C_BTN_SWAP, C_TEXT, BTN_STD_BORDER);
    if paused {
        ui_draw_btn(BTN_PAUSE, "PLAY", UiFont::Small, C_BTN_PAUSE_ON, C_BLACK, C_GOLD);
    } else {
        ui_draw_btn(BTN_PAUSE, "PAUS", UiFont::Small, C_BTN_PAUSE, C_TEXT, BTN_STD_BORDER);
    }
    ui_draw_btn(BTN_NEW, "NEW", UiFont::Small, C_BTN_NEW, C_TEXT, BTN_STD_BORDER);
    draw_demo_btn(BTN_DEMO, demo_mode, UiFont::Small);
    draw_exit_btn(BTN_EXIT, UiFont::Small);
}

fn render_pause_banner(d: &mut Fb) {
    let w = 168;
    let h = 68;
    let x = (FIELD_W - w) / 2;
    let y = (FIELD_H - h) / 2;
    fill(d, x - 2, y - 2, w + 4, h + 4, C_GOLD);
    fill(d, x, y, w, h, C_PANEL_BG);
    text_centered(d, "PAUSED", x + w / 2, y + 22, &FONT_10X20, C_GOLD);
    text_centered(d, "TAP PLAY OR FIELD", x + w / 2, y + 48, &FONT_6X10, C_TEXT);
}

fn render_overlay(d: &mut Fb, g: &Game) {
    let w = 196;
    let h = 112;
    let x = (FIELD_W - w) / 2;
    let y = (FIELD_H - h) / 2;
    fill(d, x - 2, y - 2, w + 4, h + 4, C_GOLD);
    fill(d, x, y, w, h, C_PANEL_BG);

    let (title, hint, col) = match g.state {
        State::StageClear => ("STAGE CLEAR!", "TAP FOR NEXT STAGE", C_GREEN_OK),
        State::GameOver => ("GAME OVER", "TAP TO PLAY AGAIN", C_RED_WARN),
        _ => ("", "", C_GOLD),
    };
    text_centered(d, title, x + w / 2, y + 24, &FONT_10X20, col);

    let mut b1 = [0u8; 8];
    let mut line = [0u8; 24];
    let prefix = b"SCORE ";
    line[..prefix.len()].copy_from_slice(prefix);
    let ss = u32_str(g.score, &mut b1).as_bytes();
    line[prefix.len()..prefix.len() + ss.len()].copy_from_slice(ss);
    let score_s = core::str::from_utf8(&line[..prefix.len() + ss.len()]).unwrap_or("SCORE");
    text_centered(d, score_s, x + w / 2, y + 58, &FONT_9X18_BOLD, C_TEXT);

    text_centered(d, hint, x + w / 2, y + h - 20, &FONT_6X10, C_GOLD);
}

fn render_all(d: &mut Fb, g: &Game, show_aim: bool, paused: bool, demo_mode: bool) {
    render_field(d, g, show_aim);
    render_panel(d, g, paused, demo_mode);
    if matches!(g.state, State::StageClear | State::GameOver) {
        render_overlay(d, g);
    } else if paused {
        render_pause_banner(d);
    }
}

#[inline]
fn near_frog(p: Point, g: &Game) -> bool {
    let dx = p.x - g.track.frog_x;
    let dy = p.y - g.track.frog_y;
    dx * dx + dy * dy <= FROG_SWAP_R2
}

#[esp_hal::main]
fn main() -> ! {
    let mut board = Board::init();

    let g = game_mut();
    let (run, failed) = selftest_checks(g);
    println!("ZUMA: selftest {}/{} failed={}", run - failed, run, failed);

    bake_ball_sprites();
    let mut saved_best = board.load_best(SAVE_FILE);
    g.best = saved_best;
    g.start_new_game(1, board.rng.random());
    bake_field_bg(&g.track);

    let mut paused = false;
    let mut demo_mode = board.start_in_demo(cfg!(feature = "demo"));

    render_all(&mut Fb, g, false, paused, demo_mode);
    let t0 = Instant::now();
    paint_all(&mut board);
    let full_ms = elapsed_ms(t0);
    println!("ZUMA: старт, кадр = {} ms, demo={}", full_ms, demo_mode);

    #[cfg(feature = "framedump")]
    {
        for _ in 0..42 {
            let _ = g.tick();
        }
        if let Some((_, ax, ay)) = g.find_demo_shot() {
            g.aim_at(ax, ay);
        }
        render_all(&mut Fb, g, true, paused, demo_mode);
        paint_all(&mut board);
        cyd_c5_common::dump_frame_hex();
    }

    let mut was_pressed = false;
    let mut press_pt = Point::zero();
    let mut last_pt = Point::zero();
    let mut aiming_field = false;
    let mut demo_overlay_wait = 0u8;
    let mut demo_aim_ttl = 0u8;

    loop {
        if let Some(ch) = board.poll_uart_byte() {
            match ch {
                b'X' | b'x' | b'Q' | b'q' => {
                    if g.best > saved_best {
                        board.save_best(SAVE_FILE, g.best);
                    }
                    println!("ZUMA: вихід у GameLauncher (best={})", g.best);
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

        let pressed = board.touch.pressed();
        if pressed {
            last_pt = board.touch.point();
            if !was_pressed {
                press_pt = last_pt;
                aiming_field = !paused
                    && matches!(g.state, State::Playing)
                    && press_pt.x < FIELD_W
                    && !near_frog(press_pt, g);
            }
            if aiming_field && last_pt.x < FIELD_W && !near_frog(last_pt, g) {
                g.aim_at(last_pt.x, last_pt.y);
            }
        } else if was_pressed {
            if BTN_EXIT.contains(press_pt) {
                if g.best > saved_best {
                    board.save_best(SAVE_FILE, g.best);
                }
                board.exit_to_launcher();
            } else if BTN_NEW.contains(press_pt) {
                paused = false;
                demo_overlay_wait = 0;
                demo_aim_ttl = 0;
                if g.best > saved_best {
                    saved_best = g.best;
                    board.save_best(SAVE_FILE, saved_best);
                }
                g.start_new_game(1, board.rng.random());
                bake_field_bg(&g.track);
                render_all(&mut Fb, g, false, paused, demo_mode);
                paint_all(&mut board);
                was_pressed = false;
                aiming_field = false;
                continue;
            } else if BTN_DEMO.contains(press_pt) {
                demo_mode = !demo_mode;
                demo_overlay_wait = 0;
                demo_aim_ttl = 0;
                if demo_mode && paused {
                    paused = false;
                }
                render_all(&mut Fb, g, false, paused, demo_mode);
                paint_all(&mut board);
                was_pressed = false;
                aiming_field = false;
                continue;
            } else if BTN_PAUSE.contains(press_pt) {
                if matches!(g.state, State::Playing) {
                    paused = !paused;
                    render_all(&mut Fb, g, false, paused, demo_mode);
                    paint_all(&mut board);
                }
                was_pressed = false;
                aiming_field = false;
                continue;
            }

            match g.state {
                State::StageClear => {
                    paused = false;
                    demo_overlay_wait = 0;
                    let next_stg = g.stage.saturating_add(1);
                    g.init_stage(next_stg);
                    bake_field_bg(&g.track);
                    render_all(&mut Fb, g, false, paused, demo_mode);
                    paint_all(&mut board);
                    was_pressed = false;
                    aiming_field = false;
                    continue;
                }
                State::GameOver => {
                    paused = false;
                    demo_overlay_wait = 0;
                    g.start_new_game(1, board.rng.random());
                    bake_field_bg(&g.track);
                    render_all(&mut Fb, g, false, paused, demo_mode);
                    paint_all(&mut board);
                    was_pressed = false;
                    aiming_field = false;
                    continue;
                }
                State::Draining => {}
                State::Playing => {
                    if paused {
                        if BTN_SWAP.contains(press_pt) {
                            if g.swap_balls() {
                                render_all(&mut Fb, g, false, paused, demo_mode);
                                paint_all(&mut board);
                            }
                        } else if press_pt.x < FIELD_W {
                            paused = false;
                            render_all(&mut Fb, g, false, paused, demo_mode);
                            paint_all(&mut board);
                        }
                        was_pressed = false;
                        aiming_field = false;
                        continue;
                    } else if BTN_SWAP.contains(press_pt) || near_frog(last_pt, g) {
                        if g.swap_balls() {
                            hud_dirty = true;
                        }
                    } else if aiming_field && last_pt.x < FIELD_W {
                        g.aim_at(last_pt.x, last_pt.y);
                        if g.shoot() {
                            hud_dirty = true;
                        }
                    }
                }
            }
            aiming_field = false;
        }
        was_pressed = pressed;

        if paused {
            board.delay.delay_millis(20);
            continue;
        }

        if demo_mode && !pressed {
            match g.state {
                State::Playing => {
                    let phase = g.tick_no % 9;
                    if phase == 5 {
                        if let Some((need_swap, ax, ay)) = g.find_demo_shot() {
                            if need_swap && g.swap_balls() {
                                hud_dirty = true;
                            }
                            g.aim_at(ax, ay);
                            demo_aim_ttl = 4;
                        }
                    } else if phase == 0 {
                        if let Some((need_swap, ax, ay)) = g.find_demo_shot() {
                            if need_swap && g.swap_balls() {
                                hud_dirty = true;
                            }
                            g.aim_at(ax, ay);
                        }
                        if g.shoot() {
                            hud_dirty = true;
                        }
                        demo_aim_ttl = 0;
                    }
                }
                State::StageClear => {
                    demo_overlay_wait += 1;
                    if demo_overlay_wait >= 45 {
                        demo_overlay_wait = 0;
                        let next_stg = g.stage.saturating_add(1);
                        g.init_stage(next_stg);
                        bake_field_bg(&g.track);
                        render_all(&mut Fb, g, false, paused, demo_mode);
                        paint_all(&mut board);
                        continue;
                    }
                }
                State::GameOver => {
                    demo_overlay_wait += 1;
                    if demo_overlay_wait >= 45 {
                        demo_overlay_wait = 0;
                        g.start_new_game(1, board.rng.random());
                        bake_field_bg(&g.track);
                        render_all(&mut Fb, g, false, paused, demo_mode);
                        paint_all(&mut board);
                        continue;
                    }
                }
                State::Draining => {}
            }
        }
        demo_aim_ttl = demo_aim_ttl.saturating_sub(1);

        let ev = g.tick();
        if ev.hud_dirty {
            hud_dirty = true;
        }
        if g.state != prev_state && matches!(g.state, State::StageClear | State::GameOver) && g.best > saved_best {
            saved_best = g.best;
            board.save_best(SAVE_FILE, saved_best);
        }
        let _ = Bonus::Slow; // keep import clean

        let show_aim = (aiming_field && pressed && last_pt.x < FIELD_W && !near_frog(last_pt, g))
            || (demo_mode && demo_aim_ttl > 0 && matches!(g.state, State::Playing));
        render_field(&mut Fb, g, show_aim);
        if matches!(g.state, State::StageClear | State::GameOver) {
            render_overlay(&mut Fb, g);
        }
        paint_field(&mut board);

        if hud_dirty {
            render_panel(&mut Fb, g, paused, demo_mode);
            paint_panel(&mut board);
        }

        let spent = elapsed_ms(frame_t0);
        if spent < TARGET_FRAME_MS {
            board.delay.delay_millis(TARGET_FRAME_MS - spent);
        }
    }
}
