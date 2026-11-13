#![no_std]
#![no_main]

use core::{fmt::Write, mem::MaybeUninit};
use cyd_c5_common::{
    Board, SCREEN_H, dump_frame_hex, put_px,
    ui::{
        ACCENT_CYAN, ACCENT_GOLD, ACCENT_GREEN, ACCENT_RED, BG_DEEP, BTN_STD_BG, BTN_STD_BORDER,
        BtnRect, FontKind, LineBuf, PANEL_BG, TEXT_MUTED, TEXT_WHITE, blend_fill, draw_btn,
        draw_demo_btn, draw_exit_btn, elapsed_ms, fill, in_box, rgb, stroke_rect, text,
        text_centered, u32_str,
    },
};
use cyd_game_gomoku::game::{
    AiLevel, BLACK, CELLS, EMPTY, GameStatus, GomokuGame, N, PlayMode, WHITE, col_of, row_of,
    selftest, sq_of,
};
use embedded_graphics::{pixelcolor::Rgb565, prelude::Point};
use esp_hal::time::Instant;
use esp_println::println;

#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    println!("GOMOKU PANIC: {:?}", info);
    loop {}
}

esp_bootloader_esp_idf::esp_app_desc!(
    "0.1.0",
    "cyd-game-gomoku",
    "00:00:00",
    "2026-11-12",
    "5.5.6",
    64 * 1024,
    0,
    u16::MAX,
    0
);

const SAVE_FILE: &str = "GOMOKU.SAV";

static mut GAME_CELL: MaybeUninit<GomokuGame> = MaybeUninit::uninit();

// Геометрія: дошка 10×10 по 21 px (210×210) + права панель
const CELL: i32 = 21;
const BOARD_X: i32 = 6;
const BOARD_Y: i32 = 24;
const BOARD_PX: i32 = CELL * (N as i32); // 210

const PANEL_X: i32 = 220;
const PANEL_W: u32 = 100;

const BTN_NEW: BtnRect = BtnRect::new(222, 28, 46, 22);
const BTN_UNDO: BtnRect = BtnRect::new(272, 28, 46, 22);
const BTN_MODE: BtnRect = BtnRect::new(222, 54, 96, 24);
const BTN_DEMO: BtnRect = BtnRect::new(222, 82, 96, 24);
const BTN_EXIT: BtnRect = BtnRect::new(222, 110, 96, 26);

// Палітра дерев'яної дошки ґомоку
const C_WOOD: Rgb565 = rgb(206, 166, 114);
const C_WOOD_DARK: Rgb565 = rgb(168, 128, 82);
const C_GRID: Rgb565 = rgb(96, 66, 40);
const C_SEL: Rgb565 = rgb(120, 186, 224);

fn stone_colors(color: u8) -> (Rgb565, Rgb565, Rgb565) {
    if color == BLACK {
        (rgb(26, 28, 34), rgb(6, 6, 8), rgb(92, 98, 112))
    } else {
        (rgb(238, 236, 228), rgb(148, 144, 136), rgb(255, 255, 255))
    }
}

fn draw_stone(cx: i32, cy: i32, color: u8, highlight: bool) {
    // Тінь
    for dy in -9i32..=9 {
        for dx in -9i32..=9 {
            if dx * dx + dy * dy <= 81 {
                put_px(cx + dx + 1, cy + dy + 2, C_WOOD_DARK);
            }
        }
    }

    let (body, rim, shine) = stone_colors(color);
    for dy in -9i32..=9 {
        for dx in -9i32..=9 {
            let d2 = dx * dx + dy * dy;
            if d2 > 81 {
                continue;
            }
            let col = if highlight && d2 >= 60 {
                ACCENT_GOLD
            } else if d2 >= 66 {
                rim
            } else if (dx + 3) * (dx + 3) + (dy + 3) * (dy + 3) <= 9 {
                shine
            } else {
                body
            };
            put_px(cx + dx, cy + dy, col);
        }
    }
}

#[inline]
fn cell_center(sq: usize) -> (i32, i32) {
    (
        BOARD_X + (col_of(sq) as i32) * CELL + CELL / 2,
        BOARD_Y + (row_of(sq) as i32) * CELL + CELL / 2,
    )
}

#[inline]
fn hit_cell(p: Point) -> Option<usize> {
    if in_box(p, BOARD_X, BOARD_Y, BOARD_PX, BOARD_PX) {
        let c = ((p.x - BOARD_X) / CELL) as usize;
        let r = ((p.y - BOARD_Y) / CELL) as usize;
        if r < N && c < N {
            return Some(sq_of(r, c));
        }
    }
    None
}

struct UiState {
    mode: PlayMode,
    demo_mode: bool,
    thinking: bool,
    banner_open: bool,
    best: u32,
    hover: Option<usize>,
}

fn draw_header() {
    fill(0, 0, PANEL_X as u32, BOARD_Y as u32, PANEL_BG);
    fill(0, BOARD_Y - 1, PANEL_X as u32, 1, rgb(64, 88, 128));
    text(6, 3, "GOMOKU", FontKind::Bold, ACCENT_GOLD);
    text(78, 5, "10x10 / 5 IN A ROW", FontKind::Small, TEXT_MUTED);
}

fn draw_board_frame() {
    fill(
        0,
        BOARD_Y,
        PANEL_X as u32,
        (SCREEN_H - BOARD_Y) as u32,
        BG_DEEP,
    );
    stroke_rect(
        BOARD_X - 3,
        BOARD_Y - 3,
        (BOARD_PX + 6) as u32,
        (BOARD_PX + 6) as u32,
        rgb(120, 82, 46),
    );
}

fn draw_grid() {
    fill(
        BOARD_X,
        BOARD_Y,
        BOARD_PX as u32,
        BOARD_PX as u32,
        C_WOOD,
    );
    for i in 0..=N as i32 {
        let gx = BOARD_X + i * CELL;
        let gy = BOARD_Y + i * CELL;
        let w = if i == 0 || i == N as i32 { 2 } else { 1 };
        fill(gx, BOARD_Y, w as u32, BOARD_PX as u32, C_GRID);
        fill(BOARD_X, gy, BOARD_PX as u32, w as u32, C_GRID);
    }
}

fn draw_stones(game: &GomokuGame, ui: &UiState) {
    for sq in 0..CELLS {
        let v = game.cell(sq);
        if v == EMPTY {
            continue;
        }
        let (cx, cy) = cell_center(sq);
        let wins = (game.win_mask >> sq) & 1 == 1;
        let is_last = game.last_move == Some(sq as u8);
        if is_last && !wins {
            // Позначка останнього ходу — маленьке кільце під каменем
            for dy in -9i32..=9 {
                for dx in -9i32..=9 {
                    let d2 = dx * dx + dy * dy;
                    if (92..=104).contains(&d2) {
                        put_px(cx + dx, cy + dy, ACCENT_CYAN);
                    }
                }
            }
        }
        draw_stone(cx, cy, v, wins);
    }

    if let Some(h) = ui.hover
        && game.is_empty(h)
    {
        let (cx, cy) = cell_center(h);
        for dy in -9i32..=9 {
            for dx in -9i32..=9 {
                let d2 = dx * dx + dy * dy;
                if (66..=76).contains(&d2) {
                    put_px(cx + dx, cy + dy, C_SEL);
                }
            }
        }
    }
}

fn draw_banner(game: &GomokuGame, ui: &UiState) {
    let st = game.status();
    if !ui.banner_open || !st.is_game_over() {
        return;
    }
    let bx = 18;
    let by = 78;
    let bw = 190u32;
    let bh = 62u32;
    blend_fill(bx, by, bw, bh, BG_DEEP, 210);
    stroke_rect(bx, by, bw, bh, ACCENT_GOLD);

    let (head, sub, col) = match st {
        GameStatus::Win { winner } if winner == BLACK => ("BLACK WINS!", "5 IN A ROW", ACCENT_GOLD),
        GameStatus::Win { winner } if winner == WHITE => ("WHITE WINS!", "5 IN A ROW", ACCENT_CYAN),
        GameStatus::Win { .. } => ("GAME OVER", "5 IN A ROW", ACCENT_GOLD),
        GameStatus::Draw => ("DRAW", "BOARD IS FULL", TEXT_MUTED),
        GameStatus::Ongoing => ("", "", TEXT_WHITE),
    };
    text_centered(bx + 94, by + 10, head, FontKind::Bold, col);
    text_centered(bx + 94, by + 32, sub, FontKind::Small, TEXT_WHITE);
    text_centered(
        bx + 94,
        by + 46,
        "TAP FOR BOARD / NEW",
        FontKind::Small,
        TEXT_MUTED,
    );
}

fn draw_panel(game: &GomokuGame, ui: &UiState) {
    fill(PANEL_X, 0, PANEL_W, SCREEN_H as u32, PANEL_BG);
    fill(PANEL_X, 0, 1, SCREEN_H as u32, rgb(56, 74, 106));
    text_centered(PANEL_X + 50, 4, "GOMOKU", FontKind::Bold, ACCENT_GOLD);

    // Картка стану
    fill(222, 20, 96, 2, TEXT_MUTED);

    let st = game.status();
    let (turn_lbl, turn_col) = if ui.thinking {
        ("AI THINKING", ACCENT_GOLD)
    } else if ui.demo_mode {
        ("DEMO AI", ACCENT_GREEN)
    } else if st.is_game_over() {
        ("GAME OVER", ACCENT_RED)
    } else {
        match game.turn {
            BLACK => ("BLACK TO MOVE", TEXT_WHITE),
            _ => ("WHITE TO MOVE", ACCENT_CYAN),
        }
    };

    let mut line = LineBuf::<24>::new();
    let mut nb = [0u8; 10];
    let _ = write!(line, "{}", turn_lbl);
    text_centered(PANEL_X + 50, 152, line.as_str(), FontKind::Small, turn_col);

    let mut m2 = LineBuf::<24>::new();
    let _ = write!(
        m2,
        "B{} W{}",
        game.stones(BLACK),
        game.stones(WHITE)
    );
    text_centered(PANEL_X + 50, 166, m2.as_str(), FontKind::Small, TEXT_MUTED);

    let mut sc = LineBuf::<24>::new();
    let _ = write!(sc, "SCR {}", u32_str(game.score, &mut nb));
    text_centered(PANEL_X + 50, 180, sc.as_str(), FontKind::Small, ACCENT_GREEN);

    let mut bst = LineBuf::<24>::new();
    let _ = write!(bst, "BEST {}", u32_str(ui.best, &mut nb));
    text_centered(PANEL_X + 50, 194, bst.as_str(), FontKind::Small, ACCENT_GOLD);

    let mut mv = LineBuf::<24>::new();
    if let Some(lm) = game.last_move {
        let _ = write!(
            mv,
            "LAST {}{}",
            (b'a' + col_of(lm as usize) as u8) as char,
            row_of(lm as usize) + 1
        );
    } else {
        let _ = write!(mv, "LAST -");
    }
    text_centered(PANEL_X + 50, 208, mv.as_str(), FontKind::Small, TEXT_MUTED);

    // Кнопки
    draw_btn(
        BTN_NEW,
        "NEW",
        FontKind::Small,
        BTN_STD_BG,
        TEXT_WHITE,
        BTN_STD_BORDER,
    );
    let undo_fg = if game.can_undo() && !ui.demo_mode {
        TEXT_WHITE
    } else {
        TEXT_MUTED
    };
    draw_btn(
        BTN_UNDO,
        "UNDO",
        FontKind::Small,
        BTN_STD_BG,
        undo_fg,
        BTN_STD_BORDER,
    );

    let mode_bg = match ui.mode {
        PlayMode::AiHard => rgb(64, 48, 92),
        PlayMode::AiEasy => rgb(36, 76, 86),
        PlayMode::TwoPlayers => BTN_STD_BG,
    };
    draw_btn(
        BTN_MODE,
        ui.mode.label(),
        FontKind::Small,
        mode_bg,
        TEXT_WHITE,
        BTN_STD_BORDER,
    );
    draw_demo_btn(BTN_DEMO, ui.demo_mode, FontKind::Small);
    draw_exit_btn(BTN_EXIT, FontKind::Bold);
}

fn draw_all(game: &GomokuGame, ui: &UiState) {
    draw_header();
    draw_board_frame();
    draw_grid();
    draw_stones(game, ui);
    draw_banner(game, ui);
    draw_panel(game, ui);
}

fn redraw(hw: &mut Board, game: &GomokuGame, ui: &UiState) {
    draw_all(game, ui);
    hw.present_all();
}

fn sync_after_move(game: &mut GomokuGame, ui: &mut UiState, hw: &mut Board, record: bool) {
    ui.hover = None;
    if game.status().is_game_over() {
        ui.banner_open = true;
        if record && game.score > ui.best {
            ui.best = game.score;
            hw.save_best(SAVE_FILE, ui.best);
        }
        println!(
            "GOMOKU: партія завершена {:?} (b={} w={}, score={})",
            game.status(),
            game.stones(BLACK),
            game.stones(WHITE),
            game.score
        );
    }
}

#[esp_hal::main]
fn main() -> ! {
    let mut hw = Board::init();
    let (run, failed) = selftest();
    println!("GOMOKU: selftest {}/{} failed={}", run - failed, run, failed);

    let game: &'static mut GomokuGame = unsafe {
        let ptr = core::ptr::addr_of_mut!(GAME_CELL);
        (*ptr).write(GomokuGame::new(hw.rng.random()));
        (*ptr).assume_init_mut()
    };

    let demo_start = hw.start_in_demo(cfg!(feature = "demo"));
    let best = hw.load_best(SAVE_FILE);

    let mut ui = UiState {
        mode: PlayMode::AiHard,
        demo_mode: demo_start,
        thinking: false,
        banner_open: false,
        best,
        hover: None,
    };

    draw_all(game, &ui);
    let t0 = Instant::now();
    hw.present_all();
    println!(
        "GOMOKU: старт, кадр = {} ms, demo={}, best={}",
        elapsed_ms(t0),
        ui.demo_mode,
        ui.best
    );

    if cfg!(feature = "framedump") {
        dump_frame_hex();
    }

    let mut was_pressed = false;
    let mut press_pt = Point::zero();
    let mut last_pt = Point::zero();
    let mut demo_ticks = 0u32;

    // Введення координат із UART: '>' + "e5" + Enter (стовпець a..j, рядок 1..10)
    let mut coord_mode = false;
    let mut coord_buf = [0u8; 8];
    let mut coord_len = 0usize;

    loop {
        if let Some(ch) = hw.poll_uart_byte() {
            if coord_mode {
                if ch == b'\n' || ch == b'\r' {
                    if let Ok(txt) = core::str::from_utf8(&coord_buf[..coord_len]) {
                        let bytes = txt.trim().as_bytes();
                        if bytes.len() >= 2 {
                            let file = bytes[0].to_ascii_lowercase();
                            let rank = bytes[1];
                            if (b'a'..=b'j').contains(&file) && (b'1'..=b'9').contains(&rank) {
                                let c = (file - b'a') as usize;
                                let mut r = (rank - b'1') as usize;
                                // Двозначні рядки 10
                                if bytes.len() >= 3 && bytes[1] == b'1' && bytes[2] == b'0' {
                                    r = 9;
                                }
                                if r < N && c < N && game.is_empty(sq_of(r, c)) && !game.status().is_game_over() {
                                    let record = !ui.demo_mode;
                                    game.place(sq_of(r, c), true);
                                    sync_after_move(game, &mut ui, &mut hw, record);
                                    redraw(&mut hw, game, &ui);
                                }
                            }
                        }
                    }
                    coord_mode = false;
                    coord_len = 0;
                } else if coord_len < coord_buf.len() {
                    coord_buf[coord_len] = ch;
                    coord_len += 1;
                }
            } else {
                match ch {
                    b'>' => {
                        coord_mode = true;
                        coord_len = 0;
                    }
                    b'X' | b'x' | b'Q' | b'q' => {
                        if game.score > ui.best {
                            ui.best = game.score;
                        }
                        hw.save_best(SAVE_FILE, ui.best);
                        println!("GOMOKU: вихід у GameLauncher (best={})", ui.best);
                        hw.exit_to_launcher();
                    }
                    b'D' | b'd' => {
                        ui.demo_mode = !ui.demo_mode;
                        demo_ticks = 0;
                        redraw(&mut hw, game, &ui);
                    }
                    b'N' | b'n' => {
                        game.reset();
                        ui.banner_open = false;
                        ui.hover = None;
                        redraw(&mut hw, game, &ui);
                    }
                    b'U' | b'u' if !ui.demo_mode => {
                        if game.undo(ui.mode) {
                            ui.banner_open = false;
                            redraw(&mut hw, game, &ui);
                        }
                    }
                    b'A' | b'a' => {
                        ui.mode = ui.mode.next();
                        ui.demo_mode = false;
                        redraw(&mut hw, game, &ui);
                    }
                    _ => {}
                }
            }
        }

        // Тачскрін
        let mut valid_pt = None;
        if hw.touch.pressed() {
            let pt = hw.touch.point();
            if hw.touch.pressed() {
                valid_pt = Some(pt);
            }
        }
        let pressed = valid_pt.is_some();
        if let Some(pt) = valid_pt {
            last_pt = pt;
            if !was_pressed {
                press_pt = pt;
            }
        }

        if !pressed && was_pressed {
            let p = press_pt;
            let is_tap = (last_pt.x - press_pt.x).abs() < 18 && (last_pt.y - press_pt.y).abs() < 18;

            if is_tap && BTN_EXIT.contains(p) {
                if game.score > ui.best {
                    ui.best = game.score;
                }
                hw.save_best(SAVE_FILE, ui.best);
                println!("GOMOKU: вихід у GameLauncher (best={})", ui.best);
                hw.exit_to_launcher();
            } else if is_tap && BTN_DEMO.contains(p) {
                ui.demo_mode = !ui.demo_mode;
                demo_ticks = 0;
                redraw(&mut hw, game, &ui);
            } else if is_tap && BTN_MODE.contains(p) {
                ui.mode = ui.mode.next();
                ui.demo_mode = false;
                redraw(&mut hw, game, &ui);
            } else if is_tap && BTN_UNDO.contains(p) && !ui.demo_mode {
                if game.undo(ui.mode) {
                    ui.banner_open = false;
                    redraw(&mut hw, game, &ui);
                }
            } else if is_tap && BTN_NEW.contains(p) {
                game.reset();
                ui.banner_open = false;
                ui.hover = None;
                redraw(&mut hw, game, &ui);
            } else if is_tap && ui.banner_open && game.status().is_game_over() {
                // Тап по банеру/дошці просто ховає банер, щоб роздивитися позицію
                ui.banner_open = false;
                redraw(&mut hw, game, &ui);
            } else if let Some(sq) = hit_cell(p)
                && !ui.demo_mode
                && !game.status().is_game_over()
                && (!ui.mode.is_vs_ai() || game.turn == BLACK)
            {
                if game.is_empty(sq) {
                    game.place(sq, true);
                    sync_after_move(game, &mut ui, &mut hw, true);
                    redraw(&mut hw, game, &ui);
                }
            }
        }
        was_pressed = pressed;

        // Хід ШІ
        if ui.demo_mode {
            if game.status().is_game_over() {
                demo_ticks = demo_ticks.wrapping_add(1);
                if demo_ticks >= 260 {
                    demo_ticks = 0;
                    game.reset();
                    ui.banner_open = false;
                    redraw(&mut hw, game, &ui);
                }
            } else {
                demo_ticks = demo_ticks.wrapping_add(1);
                if demo_ticks >= 18 {
                    demo_ticks = 0;
                    if let Some(mv) = game.choose_ai_move(AiLevel::Hard) {
                        game.place(mv, false);
                        sync_after_move(game, &mut ui, &mut hw, false);
                        redraw(&mut hw, game, &ui);
                    }
                }
            }
        } else if !game.status().is_game_over()
            && ui.mode.is_vs_ai()
            && game.turn == WHITE
        {
            ui.thinking = true;
            draw_panel(game, &ui);
            hw.present_rect(PANEL_X as u16, 118, PANEL_W as u16, 100);

            let level = match ui.mode {
                PlayMode::AiHard => AiLevel::Hard,
                PlayMode::AiEasy => AiLevel::Easy,
                PlayMode::TwoPlayers => AiLevel::Easy,
            };
            let t_ai = Instant::now();
            let mv = game.choose_ai_move(level);
            let dt = elapsed_ms(t_ai);
            ui.thinking = false;

            if let Some(sq) = mv {
                println!(
                    "GOMOKU AI ({:?}): {}{} за {} ms",
                    level,
                    (b'a' + col_of(sq) as u8) as char,
                    row_of(sq) + 1,
                    dt
                );
                game.place(sq, false);
                sync_after_move(game, &mut ui, &mut hw, false);
            }
            redraw(&mut hw, game, &ui);
        }

        hw.delay.delay_millis(12);
    }
}
