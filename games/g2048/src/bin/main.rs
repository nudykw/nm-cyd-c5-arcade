#![no_std]
#![no_main]

use core::fmt::Write;
use cyd_c5_common::{
    Board, Fb, SCREEN_H, SCREEN_W, dump_frame_hex,
    ui::{
        BTN_STD_BG, BTN_STD_BORDER, BtnRect, FontKind, LineBuf, draw_btn, draw_demo_btn,
        draw_exit_btn, elapsed_ms, fill, fill_rounded, in_box, stroke_rect, text, text_centered,
        u32_str,
    },
};
use cyd_game_g2048::game::{CELLS as GAME_CELLS, Dir, Game, N as GAME_N, Steps, selftest_checks};
use embedded_graphics::{
    pixelcolor::Rgb565,
    prelude::*,
    primitives::{PrimitiveStyle, Triangle},
};
use esp_hal::time::Instant;
use esp_println::println;

#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    println!("G2048 PANIC: {:?}", info);
    loop {}
}

esp_bootloader_esp_idf::esp_app_desc!(
    "0.1.0",
    "cyd-game-g2048",
    "00:00:00",
    "2026-11-12",
    "5.5.6",
    64 * 1024,
    0,
    u16::MAX,
    0
);

const SAVE_FILE: &str = "G2048.SAV";

const CELL: i32 = 50;
const GAP: i32 = 6;
const BOARD_X: i32 = 4;
const BOARD_Y: i32 = 4;
const BOARD_SZ: i32 = (GAME_N as i32) * CELL + ((GAME_N as i32) + 1) * GAP; // 230
const PANEL_X: i32 = BOARD_X + BOARD_SZ + 4; // 238
const PANEL_W: i32 = SCREEN_W - PANEL_X; // 82

const ARROW_W: i32 = 24;
const ARROW_H: i32 = 21;
const TILE_RADIUS: u32 = 8;

const BTN_UP: (i32, i32) = (266, 86);
const BTN_LEFT: (i32, i32) = (240, 109);
const BTN_DOWN: (i32, i32) = (266, 109);
const BTN_RIGHT: (i32, i32) = (292, 109);

const BTN_NEW: BtnRect = BtnRect::new(240, 136, 76, 22);
const BTN_DEMO: BtnRect = BtnRect::new(240, 162, 76, 22);
const BTN_EXIT: BtnRect = BtnRect::new(240, 188, 76, 24);

const ANIM_FRAMES: i32 = 4;
const SWIPE_MIN: i32 = 18;

const C_BG: Rgb565 = Rgb565::new(2, 6, 4);
const C_BOARD: Rgb565 = Rgb565::new(2, 8, 5);
const C_HOLE: Rgb565 = Rgb565::new(4, 12, 8);
const C_TITLE: Rgb565 = Rgb565::new(30, 48, 9);
const C_TEXT: Rgb565 = Rgb565::new(29, 59, 30);
const C_DIM: Rgb565 = Rgb565::new(17, 40, 22);
const C_BTN_ALT: Rgb565 = Rgb565::new(7, 19, 12);
const C_DIMBG: Rgb565 = Rgb565::new(1, 4, 3);
const C_DARKTEXT: Rgb565 = Rgb565::new(7, 12, 4);

const T2: Rgb565 = Rgb565::new(29, 57, 27);
const T4: Rgb565 = Rgb565::new(29, 56, 25);
const T8: Rgb565 = Rgb565::new(30, 44, 15);
const T16: Rgb565 = Rgb565::new(30, 37, 12);
const T32: Rgb565 = Rgb565::new(30, 31, 11);
const T64: Rgb565 = Rgb565::new(30, 23, 7);
const T128: Rgb565 = Rgb565::new(29, 51, 14);
const T256: Rgb565 = Rgb565::new(29, 51, 12);
const T512: Rgb565 = Rgb565::new(29, 50, 10);
const T1024: Rgb565 = Rgb565::new(29, 49, 7);
const T2048: Rgb565 = Rgb565::new(29, 48, 5);
const TBIG: Rgb565 = Rgb565::new(7, 14, 6);

#[derive(Clone, Copy, PartialEq, Eq)]
enum Overlay {
    None,
    Win,
    Over,
}

#[derive(Clone, Copy)]
enum Action {
    None,
    Move(Dir),
    NewGame,
    ToggleDemo,
    CloseOverlay,
    Exit,
}

fn tile_colors(v: u32) -> (Rgb565, Rgb565) {
    let bg = match v {
        2 => T2,
        4 => T4,
        8 => T8,
        16 => T16,
        32 => T32,
        64 => T64,
        128 => T128,
        256 => T256,
        512 => T512,
        1024 => T1024,
        2048 => T2048,
        _ => TBIG,
    };
    let fg = if v <= 4 { C_DARKTEXT } else { C_TEXT };
    (bg, fg)
}

fn cell_pos(i: usize) -> (i32, i32) {
    let c = (i % GAME_N) as i32;
    let r = (i / GAME_N) as i32;
    (
        BOARD_X + GAP + c * (CELL + GAP),
        BOARD_Y + GAP + r * (CELL + GAP),
    )
}

fn draw_tile(x: i32, y: i32, v: u32) {
    let (bg, fg) = tile_colors(v);
    fill_rounded(x, y, CELL as u32, CELL as u32, TILE_RADIUS, bg);
    let mut buf = [0u8; 10];
    let s = u32_str(v, &mut buf);
    let kind = if v < 100 {
        FontKind::Large
    } else if v < 10000 {
        FontKind::Bold
    } else {
        FontKind::Small
    };
    let ty = y + (CELL - kind.char_h()) / 2;
    text_centered(x + CELL / 2, ty, s, kind, fg);
}

fn draw_board_bg() {
    fill(BOARD_X, BOARD_Y, BOARD_SZ as u32, BOARD_SZ as u32, C_BOARD);
    for i in 0..GAME_CELLS {
        let (x, y) = cell_pos(i);
        fill_rounded(x, y, CELL as u32, CELL as u32, TILE_RADIUS, C_HOLE);
    }
}

fn draw_board(g: &Game) {
    draw_board_bg();
    for i in 0..GAME_CELLS {
        if g.cell[i] != 0 {
            let (x, y) = cell_pos(i);
            draw_tile(x, y, g.cell[i]);
        }
    }
}

fn draw_board_anim(st: &Steps, f: i32) {
    draw_board_bg();
    for s in st.iter() {
        let (fx, fy) = cell_pos(s.from as usize);
        let (tx, ty) = cell_pos(s.to as usize);
        let x = fx + (tx - fx) * f / ANIM_FRAMES;
        let y = fy + (ty - fy) * f / ANIM_FRAMES;
        draw_tile(x, y, s.value);
    }
}

fn arrow_button(at: (i32, i32), dir: Dir) {
    let (x, y) = at;
    fill(x, y, ARROW_W as u32, ARROW_H as u32, C_BTN_ALT);
    stroke_rect(x, y, ARROW_W as u32, ARROW_H as u32, BTN_STD_BORDER);
    let cx = x + ARROW_W / 2;
    let cy = y + ARROW_H / 2;
    let s = 5;
    let tri = match dir {
        Dir::Up => Triangle::new(
            Point::new(cx, cy - s),
            Point::new(cx - s, cy + s),
            Point::new(cx + s, cy + s),
        ),
        Dir::Down => Triangle::new(
            Point::new(cx, cy + s),
            Point::new(cx - s, cy - s),
            Point::new(cx + s, cy - s),
        ),
        Dir::Left => Triangle::new(
            Point::new(cx - s, cy),
            Point::new(cx + s, cy - s),
            Point::new(cx + s, cy + s),
        ),
        Dir::Right => Triangle::new(
            Point::new(cx + s, cy),
            Point::new(cx - s, cy - s),
            Point::new(cx - s, cy + s),
        ),
    };
    let _ = tri
        .into_styled(PrimitiveStyle::with_fill(C_TEXT))
        .draw(&mut Fb);
}

fn draw_panel(g: &Game, best: u32, demo_mode: bool) {
    fill(PANEL_X, 0, PANEL_W as u32, SCREEN_H as u32, C_BG);

    text(PANEL_X + 2, 2, "2048", FontKind::Large, C_TITLE);

    text(PANEL_X + 2, 24, "SCORE", FontKind::Small, C_DIM);
    let mut b1 = [0u8; 10];
    text(
        PANEL_X + 2,
        35,
        u32_str(g.score, &mut b1),
        FontKind::Bold,
        C_TEXT,
    );

    text(PANEL_X + 2, 55, "BEST", FontKind::Small, C_DIM);
    let mut b2 = [0u8; 10];
    text(
        PANEL_X + 2,
        66,
        u32_str(best, &mut b2),
        FontKind::Bold,
        C_TITLE,
    );

    arrow_button(BTN_UP, Dir::Up);
    arrow_button(BTN_LEFT, Dir::Left);
    arrow_button(BTN_DOWN, Dir::Down);
    arrow_button(BTN_RIGHT, Dir::Right);

    draw_btn(
        BTN_NEW,
        "NEW",
        FontKind::Small,
        BTN_STD_BG,
        C_TEXT,
        BTN_STD_BORDER,
    );
    draw_demo_btn(BTN_DEMO, demo_mode, FontKind::Small);
    draw_exit_btn(BTN_EXIT, FontKind::Small);
}

fn draw_overlay(ov: Overlay, g: &Game, best: u32) {
    let win = matches!(ov, Overlay::Win);
    let w = 216u32;
    let h = 124u32;
    let x = BOARD_X + (BOARD_SZ - w as i32) / 2;
    let y = (SCREEN_H - h as i32) / 2;
    fill(x, y, w, h, C_DIMBG);
    stroke_rect(x, y, w, h, if win { C_TITLE } else { C_DIM });

    text_centered(
        x + (w as i32) / 2,
        y + 14,
        if win { "2048! WIN" } else { "GAME OVER" },
        FontKind::Large,
        if win { C_TITLE } else { C_TEXT },
    );

    let mut line = LineBuf::<32>::new();
    let _ = write!(line, "SCORE {}  BEST {}", g.score, best);
    text_centered(
        x + (w as i32) / 2,
        y + 48,
        line.as_str(),
        FontKind::Small,
        C_TEXT,
    );

    let mut line2 = LineBuf::<20>::new();
    let _ = write!(line2, "MAX TILE: {}", g.max_tile());
    text_centered(
        x + (w as i32) / 2,
        y + 68,
        line2.as_str(),
        FontKind::Small,
        C_DIM,
    );

    text_centered(
        x + (w as i32) / 2,
        y + 96,
        "TAP BOARD TO PLAY",
        FontKind::Small,
        C_TITLE,
    );
}

fn draw_all(g: &Game, best: u32, demo_mode: bool, ov: Overlay) {
    fill(0, 0, SCREEN_W as u32, SCREEN_H as u32, C_BG);
    draw_board(g);
    draw_panel(g, best, demo_mode);
    if !matches!(ov, Overlay::None) {
        draw_overlay(ov, g, best);
    }
}

fn present_board(board: &mut Board) {
    board.present_rect(
        BOARD_X as u16,
        BOARD_Y as u16,
        BOARD_SZ as u16,
        BOARD_SZ as u16,
    );
}

fn present_panel(board: &mut Board) {
    board.present_rect(PANEL_X as u16, 0, PANEL_W as u16, SCREEN_H as u16);
}

fn hit_arrow(p: Point) -> Option<Dir> {
    for (at, dir) in [
        (BTN_UP, Dir::Up),
        (BTN_LEFT, Dir::Left),
        (BTN_DOWN, Dir::Down),
        (BTN_RIGHT, Dir::Right),
    ] {
        if in_box(p, at.0, at.1, ARROW_W, ARROW_H) {
            return Some(dir);
        }
    }
    None
}

/// Евристичний бот для режиму DEMO: тримає найбільшу плитку в лівому нижньому куті (змійка).
fn choose_demo_dir(g: &Game) -> Option<Dir> {
    const WEIGHTS: [u32; 16] = [
        4, 3, 2, 1, //
        5, 6, 7, 8, //
        16, 14, 12, 10, //
        32, 64, 128, 256,
    ];
    let mut best_dir = None;
    let mut best_eval = 0u64;
    for dir in [Dir::Down, Dir::Left, Dir::Right, Dir::Up] {
        let mut sim = Game::new(1);
        sim.cell = g.cell;
        sim.score = g.score;
        if sim.try_move(dir).is_some() {
            let empties = sim.cell.iter().filter(|&&v| v == 0).count() as u64;
            let mut corner_sum = 0u64;
            for i in 0..16 {
                corner_sum += (sim.cell[i] as u64) * (WEIGHTS[i] as u64);
            }
            let eval = corner_sum + empties * 512 + (sim.score as u64) * 4 + 1;
            if eval > best_eval {
                best_eval = eval;
                best_dir = Some(dir);
            }
        }
    }
    best_dir
}

#[esp_hal::main]
fn main() -> ! {
    let mut board = Board::init();
    let (run, failed) = selftest_checks();
    println!("G2048: selftest {}/{} failed={}", run - failed, run, failed);

    let mut demo_mode = board.start_in_demo(cfg!(feature = "demo"));
    let mut best = board.load_best(SAVE_FILE);
    let mut game = Game::new(board.rng.random());
    let mut overlay = Overlay::None;
    let mut win_shown = false;

    draw_all(&game, best, demo_mode, overlay);
    let t0 = Instant::now();
    board.present_all();
    println!(
        "G2048: старт, кадр = {} ms, demo={}",
        elapsed_ms(t0),
        demo_mode
    );

    if cfg!(feature = "framedump") {
        dump_frame_hex();
    }

    let mut demo_ticks = 0u32;
    let mut was_pressed = false;
    let mut press = Point::zero();
    let mut last = Point::zero();

    loop {
        let mut uart_act = Action::None;
        if let Some(ch) = board.poll_uart_byte() {
            match ch {
                b'X' | b'x' | b'Q' | b'q' => uart_act = Action::Exit,
                b'D' | b'd' => uart_act = Action::ToggleDemo,
                _ => {}
            }
        }
        let pressed = board.touch.pressed();
        if pressed {
            last = board.touch.point();
        }
        if pressed && !was_pressed {
            press = last;
        }

        let mut action = uart_act;
        if !pressed && was_pressed {
            let dx = last.x - press.x;
            let dy = last.y - press.y;
            let adx = dx.abs();
            let ady = dy.abs();

            if adx < SWIPE_MIN && ady < SWIPE_MIN && BTN_EXIT.contains(press) {
                action = Action::Exit;
            } else if adx < SWIPE_MIN && ady < SWIPE_MIN && BTN_DEMO.contains(press) {
                action = Action::ToggleDemo;
            } else if adx < SWIPE_MIN && ady < SWIPE_MIN && BTN_NEW.contains(press) {
                action = Action::NewGame;
            } else {
                action = match overlay {
                    Overlay::Win => Action::CloseOverlay,
                    Overlay::Over => Action::NewGame,
                    Overlay::None => {
                        if adx >= SWIPE_MIN || ady >= SWIPE_MIN {
                            Action::Move(if adx >= ady {
                                if dx > 0 { Dir::Right } else { Dir::Left }
                            } else if dy > 0 {
                                Dir::Down
                            } else {
                                Dir::Up
                            })
                        } else if let Some(dir) = hit_arrow(press) {
                            Action::Move(dir)
                        } else {
                            Action::None
                        }
                    }
                };
            }
        }

        if demo_mode && matches!(action, Action::None) {
            demo_ticks = demo_ticks.wrapping_add(1);
            if demo_ticks.is_multiple_of(16) {
                action = match overlay {
                    Overlay::Over => Action::NewGame,
                    Overlay::Win => Action::CloseOverlay,
                    Overlay::None => choose_demo_dir(&game)
                        .map(Action::Move)
                        .unwrap_or(Action::NewGame),
                };
            }
        }

        match action {
            Action::None => {}
            Action::Exit => {
                board.save_best(SAVE_FILE, best);
                println!("G2048: вихід у GameLauncher (best={})", best);
                board.exit_to_launcher();
            }
            Action::ToggleDemo => {
                demo_mode = !demo_mode;
                demo_ticks = 0;
                draw_panel(&game, best, demo_mode);
                present_panel(&mut board);
            }
            Action::NewGame => {
                game = Game::new(board.rng.random());
                win_shown = false;
                overlay = Overlay::None;
                draw_all(&game, best, demo_mode, overlay);
                board.present_all();
            }
            Action::CloseOverlay => {
                overlay = Overlay::None;
                draw_all(&game, best, demo_mode, overlay);
                board.present_all();
            }
            Action::Move(dir) => {
                if let Some(st) = game.try_move(dir) {
                    for f in 1..ANIM_FRAMES {
                        draw_board_anim(&st, f);
                        present_board(&mut board);
                    }
                    let _ = game.spawn();
                    if game.score > best {
                        best = game.score;
                        board.save_best(SAVE_FILE, best);
                    }
                    draw_board(&game);
                    present_board(&mut board);
                    draw_panel(&game, best, demo_mode);
                    present_panel(&mut board);

                    if game.won() && !win_shown {
                        win_shown = true;
                        overlay = Overlay::Win;
                    } else if !game.can_move() {
                        overlay = Overlay::Over;
                    }
                    if !matches!(overlay, Overlay::None) {
                        draw_all(&game, best, demo_mode, overlay);
                        board.present_all();
                    }
                }
            }
        }

        was_pressed = pressed;
        board.delay.delay_millis(10);
    }
}
