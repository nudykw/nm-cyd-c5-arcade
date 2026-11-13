#![no_std]
#![no_main]

use core::{fmt::Write, mem::MaybeUninit};
use cyd_c5_common::{
    Board, SCREEN_H, dump_frame_hex, put_px,
    ui::{
        ACCENT_CYAN, ACCENT_GOLD, ACCENT_GREEN, ACCENT_RED, BG_DEEP, BTN_STD_BG, BTN_STD_BORDER,
        BtnRect, CARD_BG, FontKind, LineBuf, PANEL_BG, TEXT_MUTED, TEXT_WHITE, blend_fill,
        draw_btn, draw_demo_btn, draw_exit_btn, elapsed_ms, fill, in_box, rgb, stroke_rect, text,
        text_centered, u32_str,
    },
};
use cyd_game_checkers::game::{
    AiLevel, CheckersGame, Color, GameStatus, MAX_STEPS, MoveList, Piece, PieceKind, PlayMode,
    TurnMove, file_of, is_dark_sq, rank_of, selftest, sq_of,
};
use embedded_graphics::{pixelcolor::Rgb565, prelude::Point};
use esp_hal::time::Instant;
use esp_println::println;

#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    println!("CHECKERS PANIC: {:?}", info);
    loop {}
}

esp_bootloader_esp_idf::esp_app_desc!(
    "0.1.0",
    "cyd-game-checkers",
    "00:00:00",
    "2026-11-12",
    "5.5.6",
    64 * 1024,
    0,
    u16::MAX,
    0
);

const SAVE_FILE: &str = "CHECKERS.SAV";

static mut GAME_CELL: MaybeUninit<CheckersGame> = MaybeUninit::uninit();

// Геометрія шашкової дошки 8×8 (28×28 px = 224×224 px) та правої панелі на екрані 320×240
const SQ: i32 = 28;
const BOARD_X: i32 = 12;
const BOARD_Y: i32 = 3;
const BOARD_PX: i32 = SQ * 8; // 224

const PANEL_X: i32 = 240;
const PANEL_W: u32 = 80;

const BTN_NEW: BtnRect = BtnRect::new(242, 122, 36, 25);
const BTN_UNDO: BtnRect = BtnRect::new(282, 122, 36, 25);
const BTN_MODE: BtnRect = BtnRect::new(242, 150, 76, 26);
const BTN_DEMO: BtnRect = BtnRect::new(242, 179, 76, 26);
const BTN_EXIT: BtnRect = BtnRect::new(242, 208, 76, 28);

// Палітра класичної червонодеревної шашкової дошки
const SQ_LIGHT: Rgb565 = rgb(236, 220, 190);
const SQ_DARK: Rgb565 = rgb(104, 54, 42);
const SQ_LAST_DARK: Rgb565 = rgb(156, 124, 44);
const SQ_SEL_BG: Rgb565 = rgb(52, 134, 174);

const W_RIM: Rgb565 = rgb(20, 22, 30);
const W_BODY: Rgb565 = rgb(248, 242, 226);
const W_RING: Rgb565 = rgb(190, 174, 146);

const B_RIM: Rgb565 = rgb(218, 228, 246);
const B_BODY: Rgb565 = rgb(26, 32, 46);
const B_RING: Rgb565 = rgb(88, 112, 152);

// Корона для дамки (11×7 пікселів)
const CROWN_BITMAP: [&[u8; 11]; 7] = [
    b".#...#...#.",
    b"#G#.#G#.#G#",
    b"#GG#GGG#GG#",
    b".#GGGGGGG#.",
    b".#G#GGG#G#.",
    b".#########.",
    b"..#######..",
];

fn draw_checker_piece(sx: i32, sy: i32, piece: Piece) {
    let cx = sx + SQ / 2;
    let cy = sy + SQ / 2;

    // Легка об'ємна тінь знизу шашки
    for dy in -10i32..=10 {
        for dx in -10i32..=10 {
            if dx * dx + dy * dy <= 104 {
                put_px(cx + dx, cy + dy + 2, rgb(34, 18, 14));
            }
        }
    }

    let (rim, body, ring) = match piece.color {
        Color::White => (W_RIM, W_BODY, W_RING),
        Color::Black => (B_RIM, B_BODY, B_RING),
    };

    for dy in -11i32..=11 {
        for dx in -11i32..=11 {
            let d2 = dx * dx + dy * dy;
            if d2 > 112 {
                continue;
            }
            let col = if d2 >= 86 {
                rim
            } else if piece.kind == PieceKind::Man && (30..=44).contains(&d2) {
                ring
            } else if piece.kind == PieceKind::King && (64..=78).contains(&d2) {
                ACCENT_GOLD
            } else {
                body
            };
            put_px(cx + dx, cy + dy, col);
        }
    }

    if piece.kind == PieceKind::King {
        let ox = cx - 5;
        let oy = cy - 3;
        for (ry, row) in CROWN_BITMAP.iter().enumerate() {
            for (rx, &ch) in row.iter().enumerate() {
                let c = match ch {
                    b'#' => rgb(20, 18, 14),
                    b'G' => ACCENT_GOLD,
                    _ => continue,
                };
                put_px(ox + rx as i32, oy + ry as i32, c);
            }
        }
    }
}

fn draw_dot(cx: i32, cy: i32, r: i32, col: Rgb565) {
    let r2 = r * r;
    for dy in -r..=r {
        for dx in -r..=r {
            if dx * dx + dy * dy <= r2 {
                put_px(cx + dx, cy + dy, col);
            }
        }
    }
}

fn draw_corner_brackets(tx: i32, ty: i32, c: Rgb565) {
    fill(tx + 1, ty + 1, 6, 2, c);
    fill(tx + 1, ty + 1, 2, 6, c);
    fill(tx + SQ - 7, ty + 1, 6, 2, c);
    fill(tx + SQ - 3, ty + 1, 2, 6, c);
    fill(tx + 1, ty + SQ - 3, 6, 2, c);
    fill(tx + 1, ty + SQ - 7, 2, 6, c);
    fill(tx + SQ - 7, ty + SQ - 3, 6, 2, c);
    fill(tx + SQ - 3, ty + SQ - 7, 2, 6, c);
}

#[inline]
fn sq_screen_xy(sq: u8) -> (i32, i32) {
    let col = file_of(sq) as i32;
    let row = 7 - (rank_of(sq) as i32);
    (BOARD_X + col * SQ, BOARD_Y + row * SQ)
}

#[inline]
fn hit_board_sq(p: Point) -> Option<u8> {
    if in_box(p, BOARD_X, BOARD_Y, BOARD_PX, BOARD_PX) {
        let col = ((p.x - BOARD_X) / SQ) as usize;
        let row = ((p.y - BOARD_Y) / SQ) as usize;
        let rank = 7usize.saturating_sub(row);
        Some(sq_of(col.min(7), rank.min(7)))
    } else {
        None
    }
}

struct UiState {
    mode: PlayMode,
    demo_mode: bool,
    legal: MoveList,
    prefix: [u8; MAX_STEPS],
    prefix_len: usize,
    status: GameStatus,
    banner_open: bool,
    thinking: bool,
    best: u32,
}

impl UiState {
    fn sync_from_game(&mut self, game: &CheckersGame) {
        self.legal = game.legal_moves();
        self.status = game.status();
        self.prefix_len = 0;
        if self.status.is_game_over() {
            self.banner_open = true;
        }
    }
}

fn draw_board_frame() {
    fill(0, 0, PANEL_X as u32, SCREEN_H as u32, BG_DEEP);
    stroke_rect(
        BOARD_X - 2,
        BOARD_Y - 2,
        (BOARD_PX + 4) as u32,
        (BOARD_PX + 4) as u32,
        ACCENT_GOLD,
    );
    stroke_rect(
        BOARD_X - 1,
        BOARD_Y - 1,
        (BOARD_PX + 2) as u32,
        (BOARD_PX + 2) as u32,
        rgb(76, 38, 28),
    );

    for row in 0..8i32 {
        let rank_ch = [b'8' - (row as u8)];
        let s = unsafe { core::str::from_utf8_unchecked(&rank_ch) };
        text(2, BOARD_Y + row * SQ + 9, s, FontKind::Small, TEXT_MUTED);
    }
    for col in 0..8i32 {
        let file_ch = [b'a' + (col as u8)];
        let s = unsafe { core::str::from_utf8_unchecked(&file_ch) };
        text(
            BOARD_X + col * SQ + 11,
            BOARD_Y + BOARD_PX + 3,
            s,
            FontKind::Small,
            TEXT_MUTED,
        );
    }
}

fn draw_squares(game: &CheckersGame, ui: &UiState) {
    let last = game.last_move();
    let active_sq = if ui.prefix_len > 0 {
        Some(ui.prefix[ui.prefix_len - 1])
    } else {
        None
    };

    for sq in 0..64u8 {
        let (sx, sy) = sq_screen_xy(sq);
        let dark = is_dark_sq(sq);
        let in_last = last.is_some_and(|m| {
            m.steps[..m.steps_len as usize].contains(&sq)
        });
        let in_prefix = ui.prefix[..ui.prefix_len].contains(&sq);

        let bg = if active_sq == Some(sq) || in_prefix {
            SQ_SEL_BG
        } else if dark && in_last {
            SQ_LAST_DARK
        } else if dark {
            SQ_DARK
        } else {
            SQ_LIGHT
        };

        fill(sx, sy, SQ as u32, SQ as u32, bg);

        if active_sq == Some(sq) {
            stroke_rect(sx, sy, SQ as u32, SQ as u32, ACCENT_CYAN);
            stroke_rect(sx + 1, sy + 1, (SQ - 2) as u32, (SQ - 2) as u32, TEXT_WHITE);
        }

        if let Some(piece) = game.piece_at(sq) {
            draw_checker_piece(sx, sy, piece);
        }
    }

    // Підсвітка шашок, які зобов'язані бити (коли ще не вибрана шашка)
    let human_turn = !ui.demo_mode && (!ui.mode.is_vs_ai() || game.turn == Color::White);
    if human_turn
        && ui.prefix_len == 0
        && matches!(ui.status, GameStatus::Ongoing { must_capture: true })
    {
        let mut marked = 0u32;
        for mv in ui.legal.as_slice() {
            let fsq = mv.from();
            let bit = 1u32 << (fsq >> 1);
            if (marked & bit) == 0 {
                marked |= bit;
                let (tx, ty) = sq_screen_xy(fsq);
                draw_corner_brackets(tx, ty, ACCENT_GOLD);
            }
        }
    }

    // Підсвітка доступних наступних кроків із вибраної шашки / проміжного поля серії
    if ui.prefix_len > 0 {
        let pref = &ui.prefix[..ui.prefix_len];
        let mut drawn = 0u32;
        for mv in ui.legal.as_slice() {
            if !mv.matches_prefix(pref) || (mv.steps_len as usize) <= ui.prefix_len {
                continue;
            }
            let next_sq = mv.steps[ui.prefix_len];
            let bit = 1u32 << (next_sq >> 1);
            if (drawn & bit) != 0 {
                continue;
            }
            drawn |= bit;
            let (tx, ty) = sq_screen_xy(next_sq);
            if mv.is_capture() {
                draw_corner_brackets(tx, ty, ACCENT_RED);
                draw_dot(tx + SQ / 2, ty + SQ / 2, 4, ACCENT_GOLD);
            } else {
                draw_dot(tx + SQ / 2, ty + SQ / 2, 4, rgb(28, 128, 86));
                draw_dot(tx + SQ / 2, ty + SQ / 2, 2, ACCENT_GREEN);
            }
        }
    }

    if ui.banner_open && ui.status.is_game_over() {
        let bx = 22;
        let by = 82;
        let bw = 204u32;
        let bh = 66u32;
        blend_fill(bx, by, bw, bh, BG_DEEP, 215);
        stroke_rect(bx, by, bw, bh, ACCENT_GOLD);
        let (headline, sub, col) = match ui.status {
            GameStatus::Win {
                winner: Color::White,
            } => ("VICTORY!", "WHITE WINS", ACCENT_GOLD),
            GameStatus::Win {
                winner: Color::Black,
            } => ("GAME OVER", "BLACK WINS", ACCENT_RED),
            GameStatus::DrawRepetition => ("DRAW GAME", "3x REPETITION", ACCENT_CYAN),
            GameStatus::DrawThirtyMove => ("DRAW GAME", "30 KING MOVES", ACCENT_CYAN),
            GameStatus::Ongoing { .. } => ("", "", TEXT_WHITE),
        };
        text_centered(bx + 102, by + 10, headline, FontKind::Bold, col);
        text_centered(bx + 102, by + 32, sub, FontKind::Small, TEXT_WHITE);
        text_centered(
            bx + 102,
            by + 48,
            "TAP BOARD TO INSPECT",
            FontKind::Small,
            TEXT_MUTED,
        );
    }
}

fn draw_panel(game: &CheckersGame, ui: &UiState) {
    fill(PANEL_X, 0, PANEL_W, SCREEN_H as u32, PANEL_BG);
    fill(PANEL_X, 0, 1, SCREEN_H as u32, rgb(56, 74, 106));

    text_centered(PANEL_X + 40, 3, "CHECKERS", FontKind::Bold, ACCENT_GOLD);

    let cx = PANEL_X + 3;
    let cw = PANEL_W - 6;
    fill(cx, 23, cw, 50, CARD_BG);
    stroke_rect(cx, 23, cw, 50, BTN_STD_BORDER);

    let (sw_bg, sw_brd, turn_lbl) = match game.turn {
        Color::White => (W_BODY, W_RIM, "WHITE"),
        Color::Black => (B_BODY, B_RIM, "BLACK"),
    };
    fill(cx + 5, 28, 9, 9, sw_bg);
    stroke_rect(cx + 5, 28, 9, 9, sw_brd);
    text(cx + 18, 28, turn_lbl, FontKind::Small, TEXT_WHITE);

    let (st_lbl, st_col) = if ui.thinking {
        ("THINKING..", ACCENT_GOLD)
    } else if ui.prefix_len >= 2 {
        ("NEXT JUMP!", ACCENT_GOLD)
    } else {
        match ui.status {
            GameStatus::Ongoing { must_capture: true } => ("MUST JUMP!", ACCENT_GOLD),
            GameStatus::Ongoing {
                must_capture: false,
            } => {
                if ui.demo_mode {
                    ("DEMO AI", ACCENT_GREEN)
                } else if ui.mode.is_vs_ai() && game.turn == Color::White {
                    ("YOUR MOVE", ACCENT_GREEN)
                } else {
                    ("TO MOVE", TEXT_MUTED)
                }
            }
            GameStatus::Win {
                winner: Color::White,
            } => ("WHITE WON", ACCENT_GOLD),
            GameStatus::Win {
                winner: Color::Black,
            } => ("BLACK WON", ACCENT_RED),
            GameStatus::DrawRepetition => ("DRAW 3x", ACCENT_CYAN),
            GameStatus::DrawThirtyMove => ("DRAW 30M", ACCENT_CYAN),
        }
    };
    text(cx + 5, 42, st_lbl, FontKind::Small, st_col);

    let mut mv_line = LineBuf::<16>::new();
    if let Some(lm) = game.last_move() {
        let mut mb = [0u8; 8];
        let _ = write!(mv_line, "{}", lm.format_short(&mut mb));
    } else {
        let _ = write!(mv_line, "MOVE #{}", game.move_number);
    }
    text(cx + 5, 56, mv_line.as_str(), FontKind::Small, ACCENT_CYAN);

    // Картка кількості фігур та рахунку
    fill(cx, 76, cw, 42, CARD_BG);
    stroke_rect(cx, 76, cw, 42, rgb(52, 66, 92));

    let (wc, bc, _, _) = game.piece_counts();
    let mut cnt_ln = LineBuf::<16>::new();
    let _ = write!(cnt_ln, "W:{} B:{}", wc, bc);
    let cnt_col = if wc > bc {
        ACCENT_GREEN
    } else if wc < bc {
        ACCENT_RED
    } else {
        TEXT_MUTED
    };
    text(cx + 5, 80, cnt_ln.as_str(), FontKind::Small, cnt_col);

    let mut num_buf = [0u8; 10];
    let mut scr_ln = LineBuf::<16>::new();
    let _ = write!(scr_ln, "SCR:{}", u32_str(game.score, &mut num_buf));
    text(cx + 5, 92, scr_ln.as_str(), FontKind::Small, TEXT_WHITE);

    let mut bst_ln = LineBuf::<16>::new();
    let _ = write!(bst_ln, "BST:{}", u32_str(ui.best, &mut num_buf));
    text(cx + 5, 104, bst_ln.as_str(), FontKind::Small, ACCENT_GOLD);

    // Кнопки керування
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

fn draw_all(game: &CheckersGame, ui: &UiState) {
    draw_board_frame();
    draw_squares(game, ui);
    draw_panel(game, ui);
}

fn present_board_and_panel(hw: &mut Board, game: &CheckersGame, ui: &UiState) {
    draw_squares(game, ui);
    draw_panel(game, ui);
    hw.present_rect(
        BOARD_X as u16,
        BOARD_Y as u16,
        BOARD_PX as u16,
        BOARD_PX as u16,
    );
    hw.present_rect(PANEL_X as u16, 0, PANEL_W as u16, SCREEN_H as u16);
}

fn apply_move_and_sync(hw: &mut Board, game: &mut CheckersGame, ui: &mut UiState, mv: TurnMove) {
    let award = if !ui.demo_mode && ui.mode.is_vs_ai() {
        Some(Color::White)
    } else {
        None
    };
    let _ = game.play_move(mv, award, ui.mode);
    ui.sync_from_game(game);
    if !ui.demo_mode && game.score > ui.best {
        ui.best = game.score;
        hw.save_best(SAVE_FILE, ui.best);
    }
}

/// Обробляє вибір клітинки людиною (підтримує і однокрокові ходи, і покроковий вибір у розгалужених серіях бою).
fn handle_human_square(hw: &mut Board, game: &mut CheckersGame, ui: &mut UiState, sq: u8) {
    if !is_dark_sq(sq) {
        if ui.prefix_len == 1 {
            ui.prefix_len = 0;
        }
        return;
    }

    if ui.prefix_len > 0 {
        // Тап по початковій шашці скасовує незавершений вибір
        if sq == ui.prefix[0] {
            ui.prefix_len = 0;
            return;
        }

        // 1) Перевіряємо, чи є `sq` наступним кроком для поточного префікса `ui.prefix[..prefix_len]`
        if ui.prefix_len < MAX_STEPS {
            ui.prefix[ui.prefix_len] = sq;
            let cand_pref = &ui.prefix[..ui.prefix_len + 1];
            let mut match_count = 0usize;
            let mut single_mv = TurnMove::EMPTY;
            for &mv in ui.legal.as_slice() {
                if mv.matches_prefix(cand_pref) {
                    match_count += 1;
                    single_mv = mv;
                }
            }
            if match_count == 1 {
                apply_move_and_sync(hw, game, ui, single_mv);
                return;
            } else if match_count > 1 {
                ui.prefix_len += 1;
                return;
            }
        }

        // 2) Якщо вибрана лише стартова шашка (`prefix_len == 1`), дозволяємо тап одразу у фінальне поле серії,
        // якщо до нього веде рівно один легальний маршрут!
        if ui.prefix_len == 1 {
            let from = ui.prefix[0];
            let mut match_count = 0usize;
            let mut single_mv = TurnMove::EMPTY;
            for &mv in ui.legal.as_slice() {
                if mv.from() == from && mv.to() == sq {
                    match_count += 1;
                    single_mv = mv;
                }
            }
            if match_count == 1 {
                apply_move_and_sync(hw, game, ui, single_mv);
                return;
            }
        }
    }

    // 3) Інакше (якщо ми не посередині багатоходового розгалуження) — обираємо власну шашку, яка має легальний хід
    if ui.prefix_len <= 1 {
        let has_move = ui.legal.as_slice().iter().any(|m| m.from() == sq);
        if has_move {
            ui.prefix[0] = sq;
            ui.prefix_len = 1;
        } else {
            ui.prefix_len = 0;
        }
    }
}

#[esp_hal::main]
fn main() -> ! {
    let mut hw = Board::init();
    let (run, failed) = selftest();
    println!("CHECKERS: selftest {}/{} failed={}", run - failed, run, failed);

    let game: &'static mut CheckersGame = unsafe {
        let ptr = core::ptr::addr_of_mut!(GAME_CELL);
        (*ptr).write(CheckersGame::new(hw.rng.random()));
        (*ptr).assume_init_mut()
    };

    let demo_start = hw.start_in_demo(cfg!(feature = "demo"));
    let best = hw.load_best(SAVE_FILE);

    let mut ui = UiState {
        mode: PlayMode::AiHard,
        demo_mode: demo_start,
        legal: game.legal_moves(),
        prefix: [0; MAX_STEPS],
        prefix_len: 0,
        status: game.status(),
        banner_open: false,
        thinking: false,
        best,
    };

    draw_all(game, &ui);
    let t0 = Instant::now();
    hw.present_all();
    println!(
        "CHECKERS: старт, кадр = {} ms, demo={}, best={}",
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
    let mut gameover_ticks = 0u32;

    let mut cmd_mode = false;
    let mut cmd_buf = [0u8; 24];
    let mut cmd_len = 0usize;

    loop {
        // 1. Обробка команд UART (`X`/`Q` — вихід, `D` — демо, `N` — нова, `U` — undo, `A` — режим, `>c3-d4\n`)
        if let Some(ch) = hw.poll_uart_byte() {
            if cmd_mode {
                if ch == b'\n' || ch == b'\r' {
                    if let Ok(s) = core::str::from_utf8(&cmd_buf[..cmd_len])
                        && let Some(mv) = game.parse_move_str(s)
                    {
                        apply_move_and_sync(&mut hw, game, &mut ui, mv);
                        present_board_and_panel(&mut hw, game, &ui);
                    }
                    cmd_mode = false;
                    cmd_len = 0;
                } else if cmd_len < cmd_buf.len() {
                    cmd_buf[cmd_len] = ch;
                    cmd_len += 1;
                }
            } else {
                match ch {
                    b'>' => {
                        cmd_mode = true;
                        cmd_len = 0;
                    }
                    b'X' | b'x' | b'Q' | b'q' => {
                        if game.score > ui.best && !ui.demo_mode {
                            ui.best = game.score;
                        }
                        hw.save_best(SAVE_FILE, ui.best);
                        println!("CHECKERS: вихід у GameLauncher (best={})", ui.best);
                        hw.exit_to_launcher();
                    }
                    b'D' | b'd' => {
                        ui.demo_mode = !ui.demo_mode;
                        ui.prefix_len = 0;
                        demo_ticks = 0;
                        gameover_ticks = 0;
                        present_board_and_panel(&mut hw, game, &ui);
                    }
                    b'N' | b'n' => {
                        *game = CheckersGame::new(hw.rng.random());
                        ui.banner_open = false;
                        ui.sync_from_game(game);
                        present_board_and_panel(&mut hw, game, &ui);
                    }
                    b'U' | b'u' if !ui.demo_mode => {
                        if game.undo(ui.mode) {
                            ui.banner_open = false;
                            ui.sync_from_game(game);
                            present_board_and_panel(&mut hw, game, &ui);
                        }
                    }
                    b'A' | b'a' => {
                        ui.mode = ui.mode.next();
                        ui.prefix_len = 0;
                        draw_panel(game, &ui);
                        hw.present_rect(PANEL_X as u16, 0, PANEL_W as u16, SCREEN_H as u16);
                    }
                    _ => {}
                }
            }
        }

        // 2. Тачскрін (`pressed -> point -> pressed` для фільтрації відриву стилуса)
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
            let p0 = press_pt;
            let p1 = last_pt;
            let is_tap = (p1.x - p0.x).abs() < 16 && (p1.y - p0.y).abs() < 16;

            if is_tap && BTN_EXIT.contains(p0) {
                if game.score > ui.best && !ui.demo_mode {
                    ui.best = game.score;
                }
                hw.save_best(SAVE_FILE, ui.best);
                println!("CHECKERS: вихід у GameLauncher (best={})", ui.best);
                hw.exit_to_launcher();
            } else if is_tap && BTN_DEMO.contains(p0) {
                ui.demo_mode = !ui.demo_mode;
                ui.prefix_len = 0;
                demo_ticks = 0;
                gameover_ticks = 0;
                present_board_and_panel(&mut hw, game, &ui);
            } else if is_tap && BTN_NEW.contains(p0) {
                *game = CheckersGame::new(hw.rng.random());
                ui.banner_open = false;
                gameover_ticks = 0;
                ui.sync_from_game(game);
                present_board_and_panel(&mut hw, game, &ui);
            } else if is_tap && BTN_UNDO.contains(p0) && !ui.demo_mode {
                if ui.prefix_len >= 2 {
                    ui.prefix_len = 0;
                    present_board_and_panel(&mut hw, game, &ui);
                } else if game.undo(ui.mode) {
                    ui.banner_open = false;
                    ui.sync_from_game(game);
                    present_board_and_panel(&mut hw, game, &ui);
                }
            } else if is_tap && BTN_MODE.contains(p0) {
                ui.mode = ui.mode.next();
                ui.demo_mode = false;
                ui.prefix_len = 0;
                present_board_and_panel(&mut hw, game, &ui);
            } else if ui.banner_open && ui.status.is_game_over() && hit_board_sq(p0).is_some() {
                ui.banner_open = false;
                present_board_and_panel(&mut hw, game, &ui);
            } else if !ui.demo_mode && !ui.status.is_game_over() {
                let human_turn = !ui.mode.is_vs_ai() || game.turn == Color::White;
                if human_turn {
                    let sq0 = hit_board_sq(p0);
                    let sq1 = hit_board_sq(p1);
                    if let (Some(from_drag), Some(to_drag)) = (sq0, sq1)
                        && from_drag != to_drag
                    {
                        if ui.prefix_len == 0 && ui.legal.as_slice().iter().any(|m| m.from() == from_drag) {
                            ui.prefix[0] = from_drag;
                            ui.prefix_len = 1;
                        }
                        handle_human_square(&mut hw, game, &mut ui, to_drag);
                        present_board_and_panel(&mut hw, game, &ui);
                    } else if let Some(tapped) = sq0 {
                        handle_human_square(&mut hw, game, &mut ui, tapped);
                        present_board_and_panel(&mut hw, game, &ui);
                    }
                }
            }
        }
        was_pressed = pressed;

        // 3. Хід ШІ (1P проти ШІ або DEMO ШІ проти ШІ)
        if ui.demo_mode {
            if ui.status.is_game_over() {
                gameover_ticks = gameover_ticks.wrapping_add(1);
                if gameover_ticks >= 140 {
                    gameover_ticks = 0;
                    *game = CheckersGame::new(hw.rng.random());
                    ui.banner_open = false;
                    ui.sync_from_game(game);
                    present_board_and_panel(&mut hw, game, &ui);
                }
            } else {
                demo_ticks = demo_ticks.wrapping_add(1);
                if demo_ticks >= 22 {
                    demo_ticks = 0;
                    if let Some(mv) = game.choose_ai_move(AiLevel::Demo) {
                        apply_move_and_sync(&mut hw, game, &mut ui, mv);
                        present_board_and_panel(&mut hw, game, &ui);
                    }
                }
            }
        } else if !ui.status.is_game_over() && ui.mode.is_vs_ai() && game.turn == Color::Black {
            ui.thinking = true;
            draw_panel(game, &ui);
            hw.present_rect(PANEL_X as u16, 20, PANEL_W as u16, 56);

            let lvl = match ui.mode {
                PlayMode::AiHard => AiLevel::Hard,
                PlayMode::AiEasy => AiLevel::Easy,
                PlayMode::TwoPlayers => AiLevel::Easy,
            };
            let t_ai = Instant::now();
            let chosen = game.choose_ai_move(lvl);
            let dt = elapsed_ms(t_ai);
            ui.thinking = false;

            if let Some(mv) = chosen {
                let mut mb = [0u8; 8];
                println!(
                    "CHECKERS AI ({:?}): {} за {} ms",
                    lvl,
                    mv.format_short(&mut mb),
                    dt
                );
                apply_move_and_sync(&mut hw, game, &mut ui, mv);
            }
            present_board_and_panel(&mut hw, game, &ui);
        }

        hw.delay.delay_millis(12);
    }
}
