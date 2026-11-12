#![no_std]
#![no_main]

use core::fmt::Write;
use cyd_c5_common::{
    Board as HwBoard, Fb, SCREEN_H, SCREEN_W, dump_frame_hex,
    ui::{
        BTN_STD_BG, BTN_STD_BORDER, BtnRect, FontKind, LineBuf, blend_fill, draw_btn,
        draw_demo_btn, draw_exit_btn, elapsed_ms, fill, in_box, stroke_rect, text, text_centered,
        u32_str,
    },
};
use cyd_game_jewels::game::{Board as GemBoard, CELLS as GAME_CELLS, Fall, N as GAME_N, selftest_checks};
use embedded_graphics::{
    pixelcolor::Rgb565,
    prelude::*,
    primitives::{Circle, PrimitiveStyle, Triangle},
};
use esp_hal::time::Instant;
use esp_println::println;

#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    println!("JEWELS PANIC: {:?}", info);
    loop {}
}

esp_bootloader_esp_idf::esp_app_desc!(
    "0.1.0",
    "cyd-game-jewels",
    "00:00:00",
    "2026-11-12",
    "5.5.6",
    64 * 1024,
    0,
    u16::MAX,
    0
);

const SAVE_FILE: &str = "JEWELS.SAV";

const CELL: i32 = 26;
const GAP: i32 = 2;
const BOARD_X: i32 = 2;
const BOARD_Y: i32 = 7;
const BOARD_SZ: i32 = (GAME_N as i32) * CELL + ((GAME_N as i32) + 1) * GAP; // 226
const PANEL_X: i32 = BOARD_X + BOARD_SZ + 4; // 232
const PANEL_W: i32 = SCREEN_W - PANEL_X; // 88

const BTN_NEW: BtnRect = BtnRect::new(236, 142, 78, 26);
const BTN_DEMO: BtnRect = BtnRect::new(236, 174, 78, 26);
const BTN_EXIT: BtnRect = BtnRect::new(236, 206, 78, 26);

const SWAP_FRAMES: i32 = 3;
const FALL_FRAMES: i32 = 4;
const FLASH_MS: u32 = 70;

const C_BG: Rgb565 = Rgb565::new(2, 6, 4);
const C_BOARD: Rgb565 = Rgb565::new(2, 8, 5);
const C_CELL: Rgb565 = Rgb565::new(4, 12, 8);
const C_TITLE: Rgb565 = Rgb565::new(30, 48, 9);
const C_TEXT: Rgb565 = Rgb565::new(29, 59, 30);
const C_DIM: Rgb565 = Rgb565::new(17, 40, 22);
const C_WHITE: Rgb565 = Rgb565::new(31, 63, 31);

const G1: Rgb565 = Rgb565::new(31, 4, 4);
const G2: Rgb565 = Rgb565::new(4, 52, 10);
const G3: Rgb565 = Rgb565::new(6, 14, 31);
const G4: Rgb565 = Rgb565::new(31, 52, 3);
const G5: Rgb565 = Rgb565::new(26, 4, 26);
const G6: Rgb565 = Rgb565::new(3, 56, 26);

#[derive(Clone, Copy, PartialEq, Eq)]
enum Overlay {
    None,
    NoMoves,
}

fn gem_color(kind: u8) -> Rgb565 {
    match kind {
        1 => G1,
        2 => G2,
        3 => G3,
        4 => G4,
        5 => G5,
        _ => G6,
    }
}

fn circle(cx: i32, cy: i32, r: i32, c: Rgb565) {
    let d = (r * 2) as u32;
    let _ = Circle::new(Point::new(cx - r, cy - r), d)
        .into_styled(PrimitiveStyle::with_fill(c))
        .draw(&mut Fb);
}

fn tri(p1: Point, p2: Point, p3: Point, c: Rgb565) {
    let _ = Triangle::new(p1, p2, p3)
        .into_styled(PrimitiveStyle::with_fill(c))
        .draw(&mut Fb);
}

fn draw_gem(cx: i32, cy: i32, kind: u8, cell_bg: Rgb565) {
    let c = gem_color(kind);
    let r = 10;
    match kind {
        1 => circle(cx, cy, r, c),
        2 => fill(cx - 9, cy - 9, 18, 18, c),
        3 => {
            tri(
                Point::new(cx, cy - r),
                Point::new(cx - r, cy),
                Point::new(cx + r, cy),
                c,
            );
            tri(
                Point::new(cx, cy + r),
                Point::new(cx - r, cy),
                Point::new(cx + r, cy),
                c,
            );
        }
        4 => tri(
            Point::new(cx, cy - r),
            Point::new(cx - r, cy + r),
            Point::new(cx + r, cy + r),
            c,
        ),
        5 => {
            circle(cx, cy, r, c);
            circle(cx, cy, 4, cell_bg);
        }
        _ => {
            fill(cx - 9, cy - 4, 18, 8, c);
            fill(cx - 4, cy - 9, 8, 18, c);
        }
    }
}

fn cell_pos(i: usize) -> (i32, i32) {
    let c = (i % GAME_N) as i32;
    let r = (i / GAME_N) as i32;
    (
        BOARD_X + GAP + c * (CELL + GAP),
        BOARD_Y + GAP + r * (CELL + GAP),
    )
}

fn cell_center_rc(r: i32, c: i32) -> (i32, i32) {
    (
        BOARD_X + GAP + c * (CELL + GAP) + CELL / 2,
        BOARD_Y + GAP + r * (CELL + GAP) + CELL / 2,
    )
}

fn cell_center(i: usize) -> (i32, i32) {
    cell_center_rc((i / GAME_N) as i32, (i % GAME_N) as i32)
}

fn draw_board_bg() {
    fill(BOARD_X, BOARD_Y, BOARD_SZ as u32, BOARD_SZ as u32, C_BOARD);
    for i in 0..GAME_CELLS {
        let (x, y) = cell_pos(i);
        fill(x, y, CELL as u32, CELL as u32, C_CELL);
    }
}

fn draw_board(b: &GemBoard) {
    draw_board_bg();
    for i in 0..GAME_CELLS {
        if b.cell[i] != 0 {
            let (cx, cy) = cell_center(i);
            draw_gem(cx, cy, b.cell[i], C_CELL);
        }
    }
}

fn draw_swap_frame(b: &GemBoard, a: usize, bb: usize, f: i32) {
    draw_board_bg();
    for i in 0..GAME_CELLS {
        if i == a || i == bb || b.cell[i] == 0 {
            continue;
        }
        let (cx, cy) = cell_center(i);
        draw_gem(cx, cy, b.cell[i], C_CELL);
    }
    let (ax, ay) = cell_center(a);
    let (bx, by) = cell_center(bb);
    draw_gem(
        ax + (bx - ax) * f / SWAP_FRAMES,
        ay + (by - ay) * f / SWAP_FRAMES,
        b.cell[a],
        C_CELL,
    );
    draw_gem(
        bx + (ax - bx) * f / SWAP_FRAMES,
        by + (ay - by) * f / SWAP_FRAMES,
        b.cell[bb],
        C_CELL,
    );
}

fn draw_fall_frame(b: &GemBoard, fall: &Fall, f: i32) {
    draw_board_bg();
    let mut busy = [false; GAME_CELLS];
    for m in fall.iter() {
        busy[(m.to_r as usize) * GAME_N + (m.to_c as usize)] = true;
    }
    for (i, &is_busy) in busy.iter().enumerate() {
        if is_busy || b.cell[i] == 0 {
            continue;
        }
        let (cx, cy) = cell_center(i);
        draw_gem(cx, cy, b.cell[i], C_CELL);
    }
    for m in fall.iter() {
        let (fx, fy) = cell_center_rc(m.from_r as i32, m.from_c as i32);
        let (tx, ty) = cell_center_rc(m.to_r as i32, m.to_c as i32);
        draw_gem(
            fx + (tx - fx) * f / FALL_FRAMES,
            fy + (ty - fy) * f / FALL_FRAMES,
            m.value,
            C_CELL,
        );
    }
}

fn draw_flash_frame(b: &GemBoard, mask: &[bool; GAME_CELLS]) {
    draw_board(b);
    for (i, &marked) in mask.iter().enumerate() {
        if marked {
            let (x, y) = cell_pos(i);
            blend_fill(x, y, CELL as u32, CELL as u32, C_WHITE, 150);
        }
    }
}

fn draw_selection(sel: Option<u8>) {
    if let Some(i) = sel {
        let (x, y) = cell_pos(i as usize);
        stroke_rect(x - 1, y - 1, (CELL + 2) as u32, (CELL + 2) as u32, C_WHITE);
    }
}

fn draw_panel(score: u32, best: u32, combo: u32, demo_mode: bool) {
    fill(PANEL_X, 0, PANEL_W as u32, SCREEN_H as u32, C_BG);

    text(PANEL_X + 4, 4, "JEWELS", FontKind::Bold, C_TITLE);

    text(PANEL_X + 4, 28, "SCORE", FontKind::Small, C_DIM);
    let mut b1 = [0u8; 10];
    text(
        PANEL_X + 4,
        40,
        u32_str(score, &mut b1),
        FontKind::Bold,
        C_TEXT,
    );

    text(PANEL_X + 4, 64, "BEST", FontKind::Small, C_DIM);
    let mut b2 = [0u8; 10];
    text(
        PANEL_X + 4,
        76,
        u32_str(best, &mut b2),
        FontKind::Bold,
        C_TITLE,
    );

    if combo > 1 {
        let mut lb = LineBuf::<12>::new();
        let _ = write!(lb, "x{}", combo);
        text(PANEL_X + 4, 102, "COMBO", FontKind::Small, C_DIM);
        text(PANEL_X + 4, 114, lb.as_str(), FontKind::Large, C_TITLE);
    } else if demo_mode {
        text(PANEL_X + 4, 110, "AUTO BOT", FontKind::Small, G2);
    }

    draw_btn(
        BTN_NEW,
        "NEW",
        FontKind::Bold,
        BTN_STD_BG,
        C_TEXT,
        BTN_STD_BORDER,
    );
    draw_demo_btn(BTN_DEMO, demo_mode, FontKind::Small);
    draw_exit_btn(BTN_EXIT, FontKind::Bold);
}

fn draw_overlay(score: u32, best: u32) {
    let w = 212u32;
    let h = 112u32;
    let x = BOARD_X + (BOARD_SZ - w as i32) / 2;
    let y = (SCREEN_H - h as i32) / 2;
    fill(x, y, w, h, C_BOARD);
    stroke_rect(x, y, w, h, C_TITLE);
    text_centered(
        x + (w as i32) / 2,
        y + 16,
        "NO MOVES",
        FontKind::Large,
        C_TITLE,
    );
    let mut lb = LineBuf::<32>::new();
    let _ = write!(lb, "SCORE {}  BEST {}", score, best);
    text_centered(
        x + (w as i32) / 2,
        y + 50,
        lb.as_str(),
        FontKind::Small,
        C_TEXT,
    );
    text_centered(
        x + (w as i32) / 2,
        y + 84,
        "TAP BOARD TO CONTINUE",
        FontKind::Small,
        C_DIM,
    );
}

fn draw_all(b: &GemBoard, sel: Option<u8>, score: u32, best: u32, demo_mode: bool, ov: Overlay) {
    fill(0, 0, SCREEN_W as u32, SCREEN_H as u32, C_BG);
    draw_board(b);
    draw_selection(sel);
    draw_panel(score, best, 1, demo_mode);
    if matches!(ov, Overlay::NoMoves) {
        draw_overlay(score, best);
    }
}

fn paint_board(hw: &mut HwBoard) {
    hw.present_rect(
        BOARD_X as u16,
        BOARD_Y as u16,
        BOARD_SZ as u16,
        BOARD_SZ as u16,
    );
}

fn paint_panel(hw: &mut HwBoard) {
    hw.present_rect(PANEL_X as u16, 0, PANEL_W as u16, SCREEN_H as u16);
}

fn hit_cell(p: Point) -> Option<usize> {
    if !in_box(p, BOARD_X, BOARD_Y, BOARD_SZ, BOARD_SZ) {
        return None;
    }
    for i in 0..GAME_CELLS {
        let (x, y) = cell_pos(i);
        if in_box(p, x, y, CELL, CELL) {
            return Some(i);
        }
    }
    None
}

fn do_swap(
    hw: &mut HwBoard,
    board: &mut GemBoard,
    a: usize,
    b: usize,
    score: &mut u32,
    best: &mut u32,
    demo_mode: bool,
) {
    board.swap_cells(a, b);
    for f in 1..=SWAP_FRAMES {
        draw_swap_frame(board, a, b, f);
        paint_board(hw);
    }
    if !board.has_match() {
        board.swap_cells(a, b);
        for f in 1..=SWAP_FRAMES {
            draw_swap_frame(board, a, b, f);
            paint_board(hw);
        }
        draw_board(board);
        paint_board(hw);
        return;
    }

    let mut combo = 1u32;
    let mut best_raised = false;
    loop {
        let mask = board.find_matches();
        if !mask.iter().any(|&x| x) {
            break;
        }
        let cleared = board.clear_matches(&mask);
        *score += GemBoard::points(cleared, combo);
        if *score > *best {
            *best = *score;
            best_raised = true;
        }

        draw_flash_frame(board, &mask);
        paint_board(hw);
        hw.delay.delay_millis(FLASH_MS);
        draw_board(board);
        paint_board(hw);

        let fall = board.collapse();
        for f in 1..FALL_FRAMES {
            draw_fall_frame(board, &fall, f);
            paint_board(hw);
        }
        draw_board(board);
        paint_board(hw);

        draw_panel(*score, *best, combo + 1, demo_mode);
        paint_panel(hw);
        combo += 1;
    }

    if best_raised {
        hw.save_best(SAVE_FILE, *best);
    }

    draw_board(board);
    paint_board(hw);
    draw_panel(*score, *best, 1, demo_mode);
    paint_panel(hw);
}

#[esp_hal::main]
fn main() -> ! {
    let mut hw = HwBoard::init();
    let (run, failed) = selftest_checks();
    println!("JEWELS: selftest {}/{} failed={}", run - failed, run, failed);

    let mut demo_mode = hw.start_in_demo(cfg!(feature = "demo"));
    let mut best = hw.load_best(SAVE_FILE);
    let mut board = GemBoard::new(hw.rng.random());
    let mut score = 0u32;
    let mut sel: Option<u8> = None;
    let mut overlay = Overlay::None;

    draw_all(&board, sel, score, best, demo_mode, overlay);
    let t0 = Instant::now();
    hw.present_all();
    println!(
        "JEWELS: старт, кадр = {} ms, demo={}",
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

    #[derive(Clone, Copy)]
    enum Action {
        None,
        Tap(Point),
        Swap(usize, usize),
        NewGame,
        ToggleDemo,
        CloseNoMoves,
        Exit,
    }

    loop {
        let mut uart_act = Action::None;
        if let Some(ch) = hw.poll_uart_byte() {
            match ch {
                b'X' | b'x' | b'Q' | b'q' => uart_act = Action::Exit,
                b'D' | b'd' => uart_act = Action::ToggleDemo,
                _ => {}
            }
        }
        let pressed = hw.touch.pressed();
        if pressed {
            last = hw.touch.point();
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

            if adx < 16 && ady < 16 && BTN_EXIT.contains(press) {
                action = Action::Exit;
            } else if adx < 16 && ady < 16 && BTN_DEMO.contains(press) {
                action = Action::ToggleDemo;
            } else if adx < 16 && ady < 16 && BTN_NEW.contains(press) {
                action = Action::NewGame;
            } else if matches!(overlay, Overlay::NoMoves) {
                action = Action::CloseNoMoves;
            } else if let Some(cell) = hit_cell(press) {
                if adx >= 16 || ady >= 16 {
                    // Свайп по каменю одразу міняє його з сусідом у напрямку руху!
                    let r = (cell / GAME_N) as i32;
                    let c = (cell % GAME_N) as i32;
                    let (nr, nc) = if adx >= ady {
                        (r, c + if dx > 0 { 1 } else { -1 })
                    } else {
                        (r + if dy > 0 { 1 } else { -1 }, c)
                    };
                    if (0..GAME_N as i32).contains(&nr) && (0..GAME_N as i32).contains(&nc) {
                        action = Action::Swap(cell, (nr as usize) * GAME_N + (nc as usize));
                    }
                } else {
                    action = match sel {
                        Some(s) if GemBoard::adjacent(s as usize, cell) => {
                            Action::Swap(s as usize, cell)
                        }
                        _ => Action::Tap(press),
                    };
                }
            }
        }

        if demo_mode && matches!(action, Action::None) {
            demo_ticks = demo_ticks.wrapping_add(1);
            if demo_ticks.is_multiple_of(38) {
                action = match overlay {
                    Overlay::NoMoves => Action::CloseNoMoves,
                    Overlay::None => match board.find_valid_move() {
                        Some((a, b)) => Action::Swap(a, b),
                        None => Action::NewGame,
                    },
                };
            }
        }

        match action {
            Action::None => {}
            Action::Exit => {
                hw.save_best(SAVE_FILE, best);
                println!("JEWELS: вихід у GameLauncher (best={})", best);
                hw.exit_to_launcher();
            }
            Action::ToggleDemo => {
                demo_mode = !demo_mode;
                demo_ticks = 0;
                draw_panel(score, best, 1, demo_mode);
                paint_panel(&mut hw);
            }
            Action::CloseNoMoves => {
                overlay = Overlay::None;
                draw_all(&board, sel, score, best, demo_mode, overlay);
                hw.present_all();
            }
            Action::Tap(pt) => {
                if let Some(cell) = hit_cell(pt) {
                    sel = if sel == Some(cell as u8) {
                        None
                    } else {
                        Some(cell as u8)
                    };
                    draw_board(&board);
                    draw_selection(sel);
                    paint_board(&mut hw);
                }
            }
            Action::NewGame => {
                board = GemBoard::new(hw.rng.random());
                score = 0;
                sel = None;
                overlay = Overlay::None;
                draw_all(&board, sel, score, best, demo_mode, overlay);
                hw.present_all();
            }
            Action::Swap(a, b) => {
                sel = None;
                do_swap(&mut hw, &mut board, a, b, &mut score, &mut best, demo_mode);
                if !board.has_move() {
                    board.shuffle();
                    overlay = Overlay::NoMoves;
                    draw_all(&board, sel, score, best, demo_mode, overlay);
                    hw.present_all();
                }
            }
        }

        was_pressed = pressed;
        hw.delay.delay_millis(10);
    }
}
