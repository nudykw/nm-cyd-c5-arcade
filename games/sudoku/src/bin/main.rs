#![no_std]
#![no_main]

use core::{fmt::Write, mem::MaybeUninit};
use cyd_c5_common::{
    Board, SCREEN_H, dump_frame_hex,
    ui::{
        ACCENT_CYAN, ACCENT_GOLD, ACCENT_GREEN, BG_DEEP, BTN_STD_BG, BTN_STD_BORDER, BtnRect,
        CARD_SEL_BG, FontKind, LineBuf, PANEL_BG, TEXT_MUTED,
        TEXT_WHITE, draw_btn, draw_demo_btn, draw_exit_btn, elapsed_ms, fill, in_box, rgb,
        stroke_rect, text, text_centered, u32_str,
    },
};
use cyd_game_sudoku::game::{
    CELLS, Difficulty, PlaceResult, SudokuGame, col_of, row_of, selftest,
};
use embedded_graphics::{pixelcolor::Rgb565, prelude::Point};
use esp_hal::time::Instant;
use esp_println::println;

#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    println!("SUDOKU PANIC: {:?}", info);
    loop {}
}

esp_bootloader_esp_idf::esp_app_desc!(
    "0.1.0",
    "cyd-game-sudoku",
    "00:00:00",
    "2026-11-12",
    "5.5.6",
    64 * 1024,
    0,
    u16::MAX,
    0
);

const SAVE_FILE: &str = "SUDOKU.SAV";

// Стан гри тримаємо в BSS, щоб стек лишався вільним для солвера
static mut GAME_CELL: MaybeUninit<SudokuGame> = MaybeUninit::uninit();

// Геометрія: дошка 9×9 по 22 px, праворуч панель із цифровою клавіатурою
const CELL: i32 = 22;
const BOARD_X: i32 = 6;
const BOARD_Y: i32 = 24;
const BOARD_PX: i32 = CELL * 9; // 198

const PANEL_X: i32 = 208;
const PANEL_W: u32 = 112;

const BTN_W: u32 = 34;
const BTN_H: u32 = 24;
const PAD_X: i32 = 210;
const PAD_Y: i32 = 26;
const PAD_GAP: i32 = 2;

const BTN_ERASE: BtnRect = BtnRect::new(210, 106, 52, 24);
const BTN_HINT: BtnRect = BtnRect::new(264, 106, 52, 24);
const BTN_NEW: BtnRect = BtnRect::new(210, 132, 52, 24);
const BTN_DIFF: BtnRect = BtnRect::new(264, 132, 52, 24);
const BTN_DEMO: BtnRect = BtnRect::new(210, 158, 106, 24);
const BTN_EXIT: BtnRect = BtnRect::new(210, 186, 106, 28);

// Палітра
const C_CELL: Rgb565 = rgb(26, 32, 46);
const C_CELL_PEER: Rgb565 = rgb(38, 48, 68);
const C_CELL_CONFLICT: Rgb565 = rgb(74, 30, 36);
const C_GIVEN: Rgb565 = rgb(246, 248, 252);
const C_USER: Rgb565 = rgb(96, 216, 244);
const C_HINT: Rgb565 = rgb(250, 200, 64);
const C_WRONG: Rgb565 = rgb(240, 88, 88);
const C_LINE: Rgb565 = rgb(62, 78, 108);
const C_LINE_BOLD: Rgb565 = rgb(140, 168, 212);

fn pad_rect(digit: u8) -> BtnRect {
    let idx = (digit - 1) as i32;
    let col = idx % 3;
    let row = idx / 3;
    BtnRect::new(
        PAD_X + col * (BTN_W as i32 + PAD_GAP),
        PAD_Y + row * (BTN_H as i32 + PAD_GAP),
        BTN_W,
        BTN_H,
    )
}

#[inline]
fn cell_xy(sq: usize) -> (i32, i32) {
    (
        BOARD_X + (col_of(sq) as i32) * CELL,
        BOARD_Y + (row_of(sq) as i32) * CELL,
    )
}

#[inline]
fn hit_cell(p: Point) -> Option<usize> {
    if in_box(p, BOARD_X, BOARD_Y, BOARD_PX, BOARD_PX) {
        let c = ((p.x - BOARD_X) / CELL) as usize;
        let r = ((p.y - BOARD_Y) / CELL) as usize;
        if r < 9 && c < 9 {
            return Some(r * 9 + c);
        }
    }
    None
}

struct UiState {
    selected: Option<usize>,
    demo_mode: bool,
    banner_open: bool,
    best: u32,
}

fn draw_board(game: &SudokuGame, ui: &UiState) {
    fill(
        0,
        BOARD_Y,
        PANEL_X as u32,
        (SCREEN_H - BOARD_Y) as u32,
        BG_DEEP,
    );
    stroke_rect(
        BOARD_X - 2,
        BOARD_Y - 2,
        (BOARD_PX + 4) as u32,
        (BOARD_PX + 4) as u32,
        rgb(78, 96, 132),
    );

    let conflicts = game.conflict_mask();
    let sel = ui.selected;

    for sq in 0..CELLS {
        let (x, y) = cell_xy(sq);
        let is_conflict = (conflicts >> sq) & 1 == 1;
        let is_sel = sel == Some(sq);
        let is_peer = sel.is_some_and(|s| {
            s != sq
                && (row_of(s) == row_of(sq)
                    || col_of(s) == col_of(sq)
                    || ((s / 27) == (sq / 27) && ((s % 9) / 3) == ((sq % 9) / 3)))
        });

        let bg = if is_sel {
            CARD_SEL_BG
        } else if is_conflict && !game.is_given(sq) {
            C_CELL_CONFLICT
        } else if is_peer {
            C_CELL_PEER
        } else {
            C_CELL
        };
        fill(x, y, CELL as u32, CELL as u32, bg);

        let v = game.value(sq);
        if v != 0 {
            let col = if is_conflict {
                C_WRONG
            } else if game.is_given(sq) {
                C_GIVEN
            } else if game.hinted[sq] {
                C_HINT
            } else if v == game.solution[sq] {
                C_USER
            } else {
                C_WRONG
            };
            text(x + 6, y + 2, digit_str(v), FontKind::Bold, col);
        }

        if is_sel {
            stroke_rect(x, y, CELL as u32, CELL as u32, ACCENT_CYAN);
        }
    }

    // Тонкі лінії сітки та товсті межі квадратів 3×3
    for i in 0..=9 {
        let bold = i % 3 == 0;
        let col = if bold { C_LINE_BOLD } else { C_LINE };
        let w = if bold { 2 } else { 1 };
        let gx = BOARD_X + i * CELL - if bold { 1 } else { 0 };
        let gy = BOARD_Y + i * CELL - if bold { 1 } else { 0 };
        fill(gx, BOARD_Y, w as u32, BOARD_PX as u32, col);
        fill(BOARD_X, gy, BOARD_PX as u32, w as u32, col);
    }

    if ui.banner_open && game.is_solved() {
        let bx = 24;
        let by = 84;
        let bw = 162u32;
        let bh = 62u32;
        fill(bx, by, bw, bh, rgb(16, 40, 30));
        stroke_rect(bx, by, bw, bh, ACCENT_GREEN);
        stroke_rect(bx + 1, by + 1, bw - 2, bh - 2, ACCENT_GOLD);
        text_centered(bx + 80, by + 10, "SOLVED!", FontKind::Bold, ACCENT_GOLD);
        let mut sub = LineBuf::<20>::new();
        let _ = write!(sub, "{} SCORE", game.score);
        text_centered(bx + 80, by + 32, sub.as_str(), FontKind::Small, TEXT_WHITE);
        text_centered(
            bx + 80,
            by + 46,
            "TAP NEW FOR NEXT",
            FontKind::Small,
            ACCENT_GREEN,
        );
    }
}

fn digit_str(d: u8) -> &'static str {
    match d {
        1 => "1",
        2 => "2",
        3 => "3",
        4 => "4",
        5 => "5",
        6 => "6",
        7 => "7",
        8 => "8",
        _ => "9",
    }
}

fn draw_top_bar(game: &SudokuGame, _ui: &UiState) {
    fill(0, 0, 320, BOARD_Y as u32, PANEL_BG);
    fill(0, BOARD_Y - 1, 320, 1, rgb(64, 88, 128));
    text(6, 3, "SUDOKU", FontKind::Bold, ACCENT_GOLD);

    let mut info = LineBuf::<28>::new();
    let _ = write!(
        info,
        "{} M{} H{}",
        game.difficulty.label(),
        game.mistakes,
        game.hints
    );
    let w = info.as_str().len() as i32 * 6;
    text(320 - 6 - w, 5, info.as_str(), FontKind::Small, ACCENT_CYAN);

    let mut sc = LineBuf::<16>::new();
    let mut nb = [0u8; 10];
    let _ = write!(sc, "SCR {}", u32_str(game.score, &mut nb));
    text(96, 5, sc.as_str(), FontKind::Small, TEXT_MUTED);
}

fn draw_panel(game: &SudokuGame, ui: &UiState) {
    fill(PANEL_X, 0, PANEL_W, SCREEN_H as u32, PANEL_BG);
    fill(PANEL_X, 0, 1, SCREEN_H as u32, rgb(56, 74, 106));

    // Цифрова клавіатура 3×3
    for d in 1..=9u8 {
        let r = pad_rect(d);
        let left = game.remaining_for(d);
        let (bg, fg, brd) = if left == 0 {
            (rgb(30, 52, 40), ACCENT_GREEN, rgb(58, 120, 82))
        } else {
            (BTN_STD_BG, TEXT_WHITE, BTN_STD_BORDER)
        };
        fill(r.x, r.y, r.w, r.h, bg);
        stroke_rect(r.x, r.y, r.w, r.h, brd);
        text_centered(r.x + 11, r.y + 3, digit_str(d), FontKind::Bold, fg);
        let mut cnt = LineBuf::<4>::new();
        let _ = write!(cnt, "{}", left);
        text(r.x + 24, r.y + 13, cnt.as_str(), FontKind::Small, TEXT_MUTED);
    }

    let has_sel = ui.selected.is_some();
    draw_btn(
        BTN_ERASE,
        "ERASE",
        FontKind::Small,
        BTN_STD_BG,
        if has_sel { TEXT_WHITE } else { TEXT_MUTED },
        BTN_STD_BORDER,
    );
    draw_btn(
        BTN_HINT,
        "HINT",
        FontKind::Small,
        BTN_STD_BG,
        ACCENT_GOLD,
        BTN_STD_BORDER,
    );
    draw_btn(
        BTN_NEW,
        "NEW",
        FontKind::Small,
        BTN_STD_BG,
        TEXT_WHITE,
        BTN_STD_BORDER,
    );
    draw_btn(
        BTN_DIFF,
        game.difficulty.label(),
        FontKind::Small,
        rgb(64, 48, 92),
        TEXT_WHITE,
        BTN_STD_BORDER,
    );
    draw_demo_btn(BTN_DEMO, ui.demo_mode, FontKind::Small);
    draw_exit_btn(BTN_EXIT, FontKind::Bold);
}

fn draw_all(game: &SudokuGame, ui: &UiState) {
    // Панель малюємо ПЕРШОЮ: вона займає правий край на всю висоту, включно зі смугою заголовка,
    // тому верхня панель із назвою та статистикою має йти після неї.
    draw_panel(game, ui);
    draw_top_bar(game, ui);
    draw_board(game, ui);
}

fn redraw(hw: &mut Board, game: &SudokuGame, ui: &UiState) {
    draw_all(game, ui);
    hw.present_all();
}

fn new_puzzle(hw: &mut Board, game: &mut SudokuGame, ui: &mut UiState, difficulty: Difficulty) {
    let t0 = Instant::now();
    if difficulty == game.difficulty {
        game.regenerate();
    } else {
        game.new_with_difficulty(difficulty);
    }
    let dt = elapsed_ms(t0);
    println!(
        "SUDOKU: нова головоломка {} ({} підказок) за {} ms",
        game.difficulty.label(),
        game.givens(),
        dt
    );
    ui.selected = None;
    ui.banner_open = false;
    redraw(hw, game, ui);
}

fn after_change(hw: &mut Board, game: &SudokuGame, ui: &mut UiState) {
    if game.score > ui.best {
        ui.best = game.score;
        hw.save_best(SAVE_FILE, ui.best);
    }
    if game.is_solved() {
        ui.banner_open = true;
        println!(
            "SUDOKU: розв'язано {} (score={}, mistakes={}, hints={})",
            game.difficulty.label(),
            game.score,
            game.mistakes,
            game.hints
        );
        if !ui.demo_mode {
            hw.save_best(SAVE_FILE, ui.best);
        }
    }
    redraw(hw, game, ui);
}

#[esp_hal::main]
fn main() -> ! {
    let mut hw = Board::init();
    let (run, failed) = selftest();
    println!("SUDOKU: selftest {}/{} failed={}", run - failed, run, failed);

    let demo_start = hw.start_in_demo(cfg!(feature = "demo"));
    let best = hw.load_best(SAVE_FILE);

    let t_gen = Instant::now();
    let game: &'static mut SudokuGame = unsafe {
        let ptr = core::ptr::addr_of_mut!(GAME_CELL);
        (*ptr).write(SudokuGame::new(hw.rng.random(), Difficulty::Easy));
        (*ptr).assume_init_mut()
    };
    println!(
        "SUDOKU: перша головоломка за {} ms ({} підказок)",
        elapsed_ms(t_gen),
        game.givens()
    );

    let mut ui = UiState {
        selected: None,
        demo_mode: demo_start,
        banner_open: false,
        best,
    };

    draw_all(game, &ui);
    let t0 = Instant::now();
    hw.present_all();
    println!(
        "SUDOKU: старт, кадр = {} ms, demo={}, best={}",
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

    loop {
        // 1. UART-команди: X/Q вихід, D демо, N нова, A складність, H підказка, E стерти,
        //    C прибрати помилки, 1..9 — поставити цифру у вибрану (або першу порожню) клітинку
        if let Some(ch) = hw.poll_uart_byte() {
            match ch {
                b'X' | b'x' | b'Q' | b'q' => {
                    if game.score > ui.best {
                        ui.best = game.score;
                    }
                    hw.save_best(SAVE_FILE, ui.best);
                    println!("SUDOKU: вихід у GameLauncher (best={})", ui.best);
                    hw.exit_to_launcher();
                }
                b'D' | b'd' => {
                    ui.demo_mode = !ui.demo_mode;
                    demo_ticks = 0;
                    redraw(&mut hw, game, &ui);
                }
                b'N' | b'n' => new_puzzle(&mut hw, game, &mut ui, game.difficulty),
                b'A' | b'a' => {
                    let next = game.difficulty.next();
                    new_puzzle(&mut hw, game, &mut ui, next);
                }
                b'H' | b'h' => {
                    if game.apply_hint(ui.selected).is_some() {
                        after_change(&mut hw, game, &mut ui);
                    }
                }
                b'E' | b'e' => {
                    if let Some(sq) = ui.selected
                        && game.clear(sq)
                    {
                        redraw(&mut hw, game, &ui);
                    }
                }
                b'C' | b'c' => {
                    if game.clear_wrong_entries() > 0 {
                        redraw(&mut hw, game, &ui);
                    }
                }
                b'1'..=b'9' => {
                    let digit = ch - b'0';
                    let target = ui
                        .selected
                        .filter(|&sq| game.is_empty(sq))
                        .or_else(|| (0..CELLS).find(|&sq| game.is_empty(sq)));
                    if let Some(sq) = target {
                        ui.selected = Some(sq);
                        match game.place(sq, digit) {
                            PlaceResult::Correct | PlaceResult::Wrong => {
                                after_change(&mut hw, game, &mut ui);
                            }
                            _ => {}
                        }
                    }
                }
                _ => {}
            }
        }

        // 2. Тачскрін (захист від відриву стилуса: pressed -> point -> pressed)
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
            let is_tap = (last_pt.x - press_pt.x).abs() < 16 && (last_pt.y - press_pt.y).abs() < 16;

            if is_tap && BTN_EXIT.contains(p) {
                if game.score > ui.best {
                    ui.best = game.score;
                }
                hw.save_best(SAVE_FILE, ui.best);
                println!("SUDOKU: вихід у GameLauncher (best={})", ui.best);
                hw.exit_to_launcher();
            } else if is_tap && BTN_DEMO.contains(p) {
                ui.demo_mode = !ui.demo_mode;
                demo_ticks = 0;
                redraw(&mut hw, game, &ui);
            } else if is_tap && BTN_NEW.contains(p) {
                new_puzzle(&mut hw, game, &mut ui, game.difficulty);
            } else if is_tap && BTN_DIFF.contains(p) {
                let next = game.difficulty.next();
                new_puzzle(&mut hw, game, &mut ui, next);
            } else if is_tap && BTN_HINT.contains(p) {
                if game.apply_hint(ui.selected).is_some() {
                    after_change(&mut hw, game, &mut ui);
                }
            } else if is_tap && BTN_ERASE.contains(p) {
                if let Some(sq) = ui.selected
                    && game.clear(sq)
                {
                    redraw(&mut hw, game, &ui);
                }
            } else if is_tap && ui.banner_open && game.is_solved() && hit_cell(p).is_some() {
                ui.banner_open = false;
                redraw(&mut hw, game, &ui);
            } else if is_tap
                && (1..=9u8).any(|d| pad_rect(d).contains(p))
                && !ui.demo_mode
            {
                let digit = (1..=9u8).find(|&d| pad_rect(d).contains(p)).unwrap();
                let target = ui
                    .selected
                    .or_else(|| (0..CELLS).find(|&sq| game.is_empty(sq)));
                if let Some(sq) = target {
                    ui.selected = Some(sq);
                    match game.place(sq, digit) {
                        PlaceResult::Correct | PlaceResult::Wrong => {
                            after_change(&mut hw, game, &mut ui);
                        }
                        _ => {
                            redraw(&mut hw, game, &ui);
                        }
                    }
                }
            } else if let Some(sq) = hit_cell(p) {
                ui.selected = if ui.selected == Some(sq) { None } else { Some(sq) };
                if ui.banner_open && !game.is_solved() {
                    ui.banner_open = false;
                }
                redraw(&mut hw, game, &ui);
            } else {
                redraw(&mut hw, game, &ui);
            }
        }
        was_pressed = pressed;

        // 3. Режим DEMO: ШІ сам дозаповнює дошку по одній клітинці
        if ui.demo_mode {
            demo_ticks = demo_ticks.wrapping_add(1);
            if game.is_solved() {
                if demo_ticks >= 300 {
                    demo_ticks = 0;
                    new_puzzle(&mut hw, game, &mut ui, game.difficulty.next());
                }
            } else if demo_ticks >= 7 {
                demo_ticks = 0;
                if game.demo_step().is_some() {
                    if game.is_solved() {
                        ui.banner_open = true;
                        println!(
                            "SUDOKU DEMO: розв'язано {} (score={})",
                            game.difficulty.label(),
                            game.score
                        );
                    }
                    redraw(&mut hw, game, &ui);
                }
            }
        }

        hw.delay.delay_millis(12);
    }
}
