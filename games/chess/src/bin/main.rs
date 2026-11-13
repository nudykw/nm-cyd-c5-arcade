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
use cyd_game_chess::game::{
    AiLevel, ChessGame, Color, GameStatus, Move, Piece, PieceKind, PlayMode, file_of, rank_of,
    selftest, sq_of,
};
use embedded_graphics::{pixelcolor::Rgb565, prelude::Point};
use esp_hal::time::Instant;
use esp_println::println;

#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    println!("CHESS PANIC: {:?}", info);
    loop {}
}

esp_bootloader_esp_idf::esp_app_desc!(
    "0.1.0",
    "cyd-game-chess",
    "00:00:00",
    "2026-11-12",
    "5.5.6",
    64 * 1024,
    0,
    u16::MAX,
    0
);

const SAVE_FILE: &str = "CHESS.SAV";

// Розміщення стану гри в BSS, щоб стек лишався абсолютно вільним для глибокого пошуку ШІ.
static mut GAME_CELL: MaybeUninit<ChessGame> = MaybeUninit::uninit();

// Геометрія шахівниці (8×8 по 28×28 px = 224×224 px) та правої панелі на екрані 320×240
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

// Модальне вікно перетворення пішака (Q / R / B / N)
const PROMO_X: i32 = 24;
const PROMO_Y: i32 = 78;
const PROMO_W: u32 = 200;
const PROMO_H: u32 = 74;

// Кольори класичної шахівниці та фігур (1 bpp / чіткий піксель-арт без AA-пилу)
const SQ_LIGHT: Rgb565 = rgb(226, 204, 168);
const SQ_DARK: Rgb565 = rgb(138, 94, 58);
const SQ_LAST_LIGHT: Rgb565 = rgb(242, 224, 108);
const SQ_LAST_DARK: Rgb565 = rgb(186, 152, 54);
const SQ_SEL_BG: Rgb565 = rgb(92, 176, 212);
const SQ_CHECK_BG: Rgb565 = rgb(206, 56, 56);

const W_OUTLINE: Rgb565 = rgb(18, 22, 30);
const W_BODY: Rgb565 = rgb(248, 244, 232);
const W_DETAIL: Rgb565 = rgb(192, 180, 154);

const B_OUTLINE: Rgb565 = rgb(216, 228, 246);
const B_BODY: Rgb565 = rgb(26, 32, 46);
const B_DETAIL: Rgb565 = rgb(92, 116, 156);

// Піксель-арт фігури Staunton 24×24 ('.' = прозорий, '#' = контур, 'O' = тіло, '+' = відблиск/деталь)
const SPRITE_PAWN: [&[u8; 24]; 24] = [
    b"........................",
    b"........................",
    b".........######.........",
    b"........##+OOO##........",
    b".......##+OOOOO##.......",
    b".......##+OOOOO##.......",
    b".......##OOOOOO##.......",
    b"........##OOOO##........",
    b".........######.........",
    b".......##########.......",
    b".......##++++++##.......",
    b"........########........",
    b"........##+OOO##........",
    b"........##+OOO##........",
    b".......##+OOOOO##.......",
    b"......##+OOOOOOO##......",
    b".....##OOOOOOOOOO##.....",
    b"....################....",
    b"...##++++++++++++++##...",
    b"...##OOOOOOOOOOOOOO##...",
    b"...##OOOOOOOOOOOOOO##...",
    b"...##################...",
    b"........................",
    b"........................",
];

const SPRITE_ROOK: [&[u8; 24]; 24] = [
    b"........................",
    b"........................",
    b"....####..####..####....",
    b"....#+O#..#+O#..#+O#....",
    b"....#OO####OO####OO#....",
    b"....#OOOOOOOOOOOOOO#....",
    b"....################....",
    b".....##++++++++++##.....",
    b"......############......",
    b"......##+OOOOOOO##......",
    b"......##+OOOOOOO##......",
    b"......##+OOOOOOO##......",
    b"......##+OOOOOOO##......",
    b"......##+OOOOOOO##......",
    b".....##+OOOOOOOOO##.....",
    b"....##OOOOOOOOOOOO##....",
    b"...##################...",
    b"..##++++++++++++++++##..",
    b"..##OOOOOOOOOOOOOOOO##..",
    b"..##OOOOOOOOOOOOOOOO##..",
    b"..####################..",
    b"........................",
    b"........................",
    b"........................",
];

const SPRITE_KNIGHT: [&[u8; 24]; 24] = [
    b"........................",
    b".........####...........",
    b"........##+O###.........",
    b".......##+OOOO###.......",
    b".....####O#OOOO+###.....",
    b"....##+OOOOOOOOOO+##....",
    b"...##+OOOOOOOOOOOO+##...",
    b"..##+OOOOO####OOOOO+##..",
    b"..##OOOOO##..#OOOOOO##..",
    b"...#####.....#OOOOOO##..",
    b"............##OOOOOO##..",
    b"...........##+OOOOOO##..",
    b"..........##+OOOOOOO##..",
    b".........##+OOOOOOOO##..",
    b"........##+OOOOOOOOO##..",
    b".......##+OOOOOOOOOO##..",
    b"......##OOOOOOOOOOOO##..",
    b"....##################..",
    b"...##++++++++++++++++##.",
    b"...##OOOOOOOOOOOOOOOO##.",
    b"...####################.",
    b"........................",
    b"........................",
    b"........................",
];

const SPRITE_BISHOP: [&[u8; 24]; 24] = [
    b"........................",
    b"..........####..........",
    b".........##+O##.........",
    b"..........####..........",
    b".........######.........",
    b"........##+OOO##........",
    b".......##+OOO#O##.......",
    b"......##+OOO#OOO##......",
    b"......##+OO#OOOO##......",
    b"......##OOOOOOOO##......",
    b".......##OOOOOO##.......",
    b"........########........",
    b".......##++++++##.......",
    b"........########........",
    b".......##+OOOOO##.......",
    b"......##+OOOOOOO##......",
    b".....##OOOOOOOOOO##.....",
    b"....################....",
    b"...##++++++++++++++##...",
    b"...##OOOOOOOOOOOOOO##...",
    b"...##################...",
    b"........................",
    b"........................",
    b"........................",
];

const SPRITE_QUEEN: [&[u8; 24]; 24] = [
    b"........................",
    b"...##..##..##..##..##...",
    b"..#O#.#O#.#+O#.#O#.#O#..",
    b"...##..##..##..##..##...",
    b"...##..##..##..##..##...",
    b"...###.##..##..##.###...",
    b"....#O###.#OO#.###O#....",
    b"....#OOO###OO###OOO#....",
    b".....#OOOOOOOOOOOO#.....",
    b".....##OOOOOOOOOO##.....",
    b"......############......",
    b"......##++++++++##......",
    b".......##########.......",
    b".......##+OOOOO##.......",
    b"......##+OOOOOOO##......",
    b".....##+OOOOOOOOO##.....",
    b"....##OOOOOOOOOOOO##....",
    b"...##################...",
    b"..##++++++++++++++++##..",
    b"..##OOOOOOOOOOOOOOOO##..",
    b"..####################..",
    b"........................",
    b"........................",
    b"........................",
];

const SPRITE_KING: [&[u8; 24]; 24] = [
    b"..........####..........",
    b"..........#++#..........",
    b"........########........",
    b"........#++++++#........",
    b"........########........",
    b"..........#OO#..........",
    b".....#####OOOO#####.....",
    b"....##+OO##OO##OO+##....",
    b"...##+OOOO#++#OOOO+##...",
    b"...##OOOOOO++OOOOOO##...",
    b"....##OOOOOOOOOOOO##....",
    b".....##############.....",
    b".....##++++++++++##.....",
    b"......############......",
    b"......##+OOOOOOO##......",
    b".....##+OOOOOOOOO##.....",
    b"....##OOOOOOOOOOOO##....",
    b"...##################...",
    b"..##++++++++++++++++##..",
    b"..##OOOOOOOOOOOOOOOO##..",
    b"..####################..",
    b"........................",
    b"........................",
    b"........................",
];

fn draw_piece_sprite(x: i32, y: i32, piece: Piece) {
    let sprite = match piece.kind {
        PieceKind::Pawn => &SPRITE_PAWN,
        PieceKind::Rook => &SPRITE_ROOK,
        PieceKind::Knight => &SPRITE_KNIGHT,
        PieceKind::Bishop => &SPRITE_BISHOP,
        PieceKind::Queen => &SPRITE_QUEEN,
        PieceKind::King => &SPRITE_KING,
    };
    let (outline, body, detail) = match piece.color {
        Color::White => (W_OUTLINE, W_BODY, W_DETAIL),
        Color::Black => (B_OUTLINE, B_BODY, B_DETAIL),
    };

    for (ry, row) in sprite.iter().enumerate() {
        let py = y + ry as i32;
        for (rx, &ch) in row.iter().enumerate() {
            let col = match ch {
                b'#' => outline,
                b'O' => body,
                b'+' => detail,
                _ => continue,
            };
            put_px(x + rx as i32, py, col);
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

const PROMO_KINDS: [PieceKind; 4] = [
    PieceKind::Queen,
    PieceKind::Rook,
    PieceKind::Bishop,
    PieceKind::Knight,
];

fn promo_slot_rect(idx: usize) -> BtnRect {
    let x = PROMO_X + 8 + (idx as i32) * 47;
    BtnRect::new(x, PROMO_Y + 22, 43, 44)
}

struct UiState {
    mode: PlayMode,
    demo_mode: bool,
    selected: Option<u8>,
    sel_moves: [Move; 32],
    sel_moves_len: usize,
    promo_pending: Option<(u8, u8)>,
    status: GameStatus,
    banner_open: bool,
    thinking: bool,
    best: u32,
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
        rgb(72, 50, 30),
    );

    // Координати 8..1 зліва та a..h знизу
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

fn draw_squares(game: &ChessGame, ui: &UiState) {
    let last = game.last_move();
    let checked_king = match ui.status {
        GameStatus::Ongoing { in_check: true } | GameStatus::Checkmate { .. } => {
            if game.in_check(game.turn) {
                Some(game.king_square(game.turn))
            } else {
                None
            }
        }
        _ => None,
    };

    for sq in 0..64u8 {
        let (sx, sy) = sq_screen_xy(sq);
        let is_light = ((file_of(sq) + rank_of(sq)) & 1) != 0;
        let is_last = last.is_some_and(|m| m.from == sq || m.to == sq);

        let bg = if checked_king == Some(sq) {
            SQ_CHECK_BG
        } else if ui.selected == Some(sq) {
            SQ_SEL_BG
        } else if is_last {
            if is_light {
                SQ_LAST_LIGHT
            } else {
                SQ_LAST_DARK
            }
        } else if is_light {
            SQ_LIGHT
        } else {
            SQ_DARK
        };

        fill(sx, sy, SQ as u32, SQ as u32, bg);

        if ui.selected == Some(sq) {
            stroke_rect(sx, sy, SQ as u32, SQ as u32, ACCENT_CYAN);
            stroke_rect(sx + 1, sy + 1, (SQ - 2) as u32, (SQ - 2) as u32, TEXT_WHITE);
        } else if checked_king == Some(sq) {
            stroke_rect(sx, sy, SQ as u32, SQ as u32, ACCENT_GOLD);
        }

        if let Some(piece) = game.piece_at(sq) {
            draw_piece_sprite(sx + 2, sy + 2, piece);
        }
    }

    // Маркери легальних ходів із вибраної клітинки
    if ui.selected.is_some() {
        for &mv in &ui.sel_moves[..ui.sel_moves_len] {
            // Для перетворення пішака у списку 4 варіанти на одне й те саме поле — малюємо один раз
            if let Some(pk) = mv.promo()
                && pk != PieceKind::Queen
            {
                continue;
            }
            let (tx, ty) = sq_screen_xy(mv.to);
            if mv.is_capture() {
                // Кутові дужки взяття (включно з en passant!)
                let c = ACCENT_RED;
                fill(tx + 1, ty + 1, 6, 2, c);
                fill(tx + 1, ty + 1, 2, 6, c);
                fill(tx + SQ - 7, ty + 1, 6, 2, c);
                fill(tx + SQ - 3, ty + 1, 2, 6, c);
                fill(tx + 1, ty + SQ - 3, 6, 2, c);
                fill(tx + 1, ty + SQ - 7, 2, 6, c);
                fill(tx + SQ - 7, ty + SQ - 3, 6, 2, c);
                fill(tx + SQ - 3, ty + SQ - 7, 2, 6, c);
            } else {
                draw_dot(tx + SQ / 2, ty + SQ / 2, 4, rgb(28, 128, 86));
                draw_dot(tx + SQ / 2, ty + SQ / 2, 2, ACCENT_GREEN);
            }
        }
    }

    // Модальний вибір фігури при перетворенні пішака
    if ui.promo_pending.is_some() {
        blend_fill(BOARD_X, BOARD_Y, BOARD_PX as u32, BOARD_PX as u32, BG_DEEP, 140);
        fill(PROMO_X, PROMO_Y, PROMO_W, PROMO_H, PANEL_BG);
        stroke_rect(PROMO_X, PROMO_Y, PROMO_W, PROMO_H, ACCENT_GOLD);
        stroke_rect(PROMO_X + 1, PROMO_Y + 1, PROMO_W - 2, PROMO_H - 2, ACCENT_CYAN);
        text_centered(
            PROMO_X + (PROMO_W as i32) / 2,
            PROMO_Y + 6,
            "PROMOTE PAWN TO:",
            FontKind::Small,
            ACCENT_GOLD,
        );

        let labels = ["Q", "R", "B", "N"];
        for (i, &pk) in PROMO_KINDS.iter().enumerate() {
            let r = promo_slot_rect(i);
            fill(r.x, r.y, r.w, r.h, CARD_BG);
            stroke_rect(r.x, r.y, r.w, r.h, BTN_STD_BORDER);
            draw_piece_sprite(r.x + 9, r.y + 3, Piece::new(game.turn, pk));
            text_centered(
                r.x + (r.w as i32) / 2,
                r.y + 31,
                labels[i],
                FontKind::Small,
                ACCENT_CYAN,
            );
        }
    } else if ui.banner_open && ui.status.is_game_over() {
        let bx = 22;
        let by = 82;
        let bw = 204u32;
        let bh = 66u32;
        blend_fill(bx, by, bw, bh, BG_DEEP, 215);
        stroke_rect(bx, by, bw, bh, ACCENT_GOLD);
        let (headline, sub, col) = match ui.status {
            GameStatus::Checkmate {
                winner: Color::White,
            } => ("CHECKMATE!", "WHITE WINS", ACCENT_GOLD),
            GameStatus::Checkmate {
                winner: Color::Black,
            } => ("CHECKMATE!", "BLACK WINS", ACCENT_RED),
            GameStatus::Stalemate => ("STALEMATE", "DRAW (NO MOVES)", ACCENT_CYAN),
            GameStatus::DrawFiftyMove => ("DRAW GAME", "50-MOVE RULE", ACCENT_CYAN),
            GameStatus::DrawRepetition => ("DRAW GAME", "3x REPETITION", ACCENT_CYAN),
            GameStatus::DrawInsufficientMaterial => ("DRAW GAME", "LOW MATERIAL", ACCENT_CYAN),
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

fn draw_panel(game: &ChessGame, ui: &UiState) {
    fill(PANEL_X, 0, PANEL_W, SCREEN_H as u32, PANEL_BG);
    fill(PANEL_X, 0, 1, SCREEN_H as u32, rgb(56, 74, 106));

    text_centered(PANEL_X + 40, 3, "CHESS", FontKind::Bold, ACCENT_GOLD);

    // Картка стану ходу
    let cx = PANEL_X + 3;
    let cw = PANEL_W - 6;
    fill(cx, 23, cw, 50, CARD_BG);
    stroke_rect(cx, 23, cw, 50, BTN_STD_BORDER);

    let (sw_bg, sw_brd, turn_lbl) = match game.turn {
        Color::White => (W_BODY, W_OUTLINE, "WHITE"),
        Color::Black => (B_BODY, B_OUTLINE, "BLACK"),
    };
    fill(cx + 5, 28, 9, 9, sw_bg);
    stroke_rect(cx + 5, 28, 9, 9, sw_brd);
    text(cx + 18, 28, turn_lbl, FontKind::Small, TEXT_WHITE);

    let (st_lbl, st_col) = if ui.thinking {
        ("THINKING..", ACCENT_GOLD)
    } else {
        match ui.status {
            GameStatus::Ongoing { in_check: true } => ("CHECK!", ACCENT_RED),
            GameStatus::Ongoing { in_check: false } => {
                if ui.demo_mode {
                    ("DEMO AI", ACCENT_GREEN)
                } else if ui.mode.is_vs_ai() && game.turn == Color::White {
                    ("YOUR MOVE", ACCENT_GREEN)
                } else {
                    ("TO MOVE", TEXT_MUTED)
                }
            }
            GameStatus::Checkmate {
                winner: Color::White,
            } => ("WHITE WON", ACCENT_GOLD),
            GameStatus::Checkmate {
                winner: Color::Black,
            } => ("BLACK WON", ACCENT_RED),
            GameStatus::Stalemate => ("STALEMATE", ACCENT_CYAN),
            GameStatus::DrawFiftyMove => ("DRAW 50M", ACCENT_CYAN),
            GameStatus::DrawRepetition => ("DRAW 3x", ACCENT_CYAN),
            GameStatus::DrawInsufficientMaterial => ("DRAW MAT", ACCENT_CYAN),
        }
    };
    text(cx + 5, 42, st_lbl, FontKind::Small, st_col);

    let mut mv_line = LineBuf::<16>::new();
    if let Some(lm) = game.last_move() {
        let mut mb = [0u8; 6];
        let _ = write!(mv_line, "MV:{}", lm.format_short(&mut mb));
    } else {
        let _ = write!(mv_line, "MOVE #{}", game.fullmove_number);
    }
    text(cx + 5, 56, mv_line.as_str(), FontKind::Small, ACCENT_CYAN);

    // Картка матеріалу та рахунку
    fill(cx, 76, cw, 42, CARD_BG);
    stroke_rect(cx, 76, cw, 42, rgb(52, 66, 92));

    let mat = game.material_balance_pawns();
    let mut mat_ln = LineBuf::<16>::new();
    if mat > 0 {
        let _ = write!(mat_ln, "MAT: +{}", mat);
    } else if mat < 0 {
        let _ = write!(mat_ln, "MAT: {}", mat);
    } else {
        let _ = write!(mat_ln, "MAT: EVEN");
    }
    let mat_col = if mat > 0 {
        ACCENT_GREEN
    } else if mat < 0 {
        ACCENT_RED
    } else {
        TEXT_MUTED
    };
    text(cx + 5, 80, mat_ln.as_str(), FontKind::Small, mat_col);

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

fn draw_all(game: &ChessGame, ui: &UiState) {
    draw_board_frame();
    draw_squares(game, ui);
    draw_panel(game, ui);
}

fn present_board_and_panel(hw: &mut Board, game: &ChessGame, ui: &UiState) {
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

fn clear_selection(ui: &mut UiState) {
    ui.selected = None;
    ui.sel_moves_len = 0;
    ui.promo_pending = None;
}

fn apply_move_and_sync(hw: &mut Board, game: &mut ChessGame, ui: &mut UiState, mv: Move) {
    let award = if !ui.demo_mode && ui.mode.is_vs_ai() {
        Some(Color::White)
    } else {
        None
    };
    ui.status = game.play_move(mv, award, ui.mode);
    clear_selection(ui);
    if ui.status.is_game_over() {
        ui.banner_open = true;
    }
    if !ui.demo_mode && game.score > ui.best {
        ui.best = game.score;
        hw.save_best(SAVE_FILE, ui.best);
    }
}

#[esp_hal::main]
fn main() -> ! {
    let mut hw = Board::init();
    let (run, failed) = selftest();
    println!("CHESS: selftest {}/{} failed={}", run - failed, run, failed);

    let game: &'static mut ChessGame = unsafe {
        let ptr = core::ptr::addr_of_mut!(GAME_CELL);
        (*ptr).write(ChessGame::new(hw.rng.random()));
        (*ptr).assume_init_mut()
    };

    let demo_start = hw.start_in_demo(cfg!(feature = "demo"));
    let best = hw.load_best(SAVE_FILE);

    let mut ui = UiState {
        mode: PlayMode::AiHard,
        demo_mode: demo_start,
        selected: None,
        sel_moves: [Move::default(); 32],
        sel_moves_len: 0,
        promo_pending: None,
        status: game.status(),
        banner_open: false,
        thinking: false,
        best,
    };

    draw_all(game, &ui);
    let t0 = Instant::now();
    hw.present_all();
    println!(
        "CHESS: старт, кадр = {} ms, demo={}, best={}",
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

    let mut uci_mode = false;
    let mut uci_buf = [0u8; 8];
    let mut uci_len = 0usize;

    loop {
        // 1. Обробка команд UART (`X`/`Q` — вихід, `D` — демо, `N` — нова, `U` — undo, `A` — режим, `>e2e4\n` — хід)
        if let Some(ch) = hw.poll_uart_byte() {
            if uci_mode {
                if ch == b'\n' || ch == b'\r' {
                    if let Ok(s) = core::str::from_utf8(&uci_buf[..uci_len])
                        && let Some(mv) = game.parse_uci_move(s)
                    {
                        apply_move_and_sync(&mut hw, game, &mut ui, mv);
                        present_board_and_panel(&mut hw, game, &ui);
                    }
                    uci_mode = false;
                    uci_len = 0;
                } else if uci_len < uci_buf.len() {
                    uci_buf[uci_len] = ch;
                    uci_len += 1;
                }
            } else {
                match ch {
                    b'>' => {
                        uci_mode = true;
                        uci_len = 0;
                    }
                    b'X' | b'x' | b'Q' | b'q' => {
                        if game.score > ui.best && !ui.demo_mode {
                            ui.best = game.score;
                        }
                        hw.save_best(SAVE_FILE, ui.best);
                        println!("CHESS: вихід у GameLauncher (best={})", ui.best);
                        hw.exit_to_launcher();
                    }
                    b'D' | b'd' => {
                        ui.demo_mode = !ui.demo_mode;
                        clear_selection(&mut ui);
                        demo_ticks = 0;
                        gameover_ticks = 0;
                        present_board_and_panel(&mut hw, game, &ui);
                    }
                    b'N' | b'n' => {
                        *game = ChessGame::new(hw.rng.random());
                        clear_selection(&mut ui);
                        ui.status = game.status();
                        ui.banner_open = false;
                        present_board_and_panel(&mut hw, game, &ui);
                    }
                    b'U' | b'u' if !ui.demo_mode => {
                        if game.undo(ui.mode) {
                            clear_selection(&mut ui);
                            ui.status = game.status();
                            ui.banner_open = false;
                            present_board_and_panel(&mut hw, game, &ui);
                        }
                    }
                    b'A' | b'a' => {
                        ui.mode = ui.mode.next();
                        clear_selection(&mut ui);
                        draw_panel(game, &ui);
                        hw.present_rect(PANEL_X as u16, 0, PANEL_W as u16, SCREEN_H as u16);
                    }
                    _ => {}
                }
            }
        }

        // 2. Обробка тачскріна (і тап-тап, і перетягування фігури пальцем!)
        // Сендвіч `pressed -> point -> pressed` відсікає 1-семплові завади та відрив стилуса.
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
                println!("CHESS: вихід у GameLauncher (best={})", ui.best);
                hw.exit_to_launcher();
            } else if is_tap && BTN_DEMO.contains(p0) {
                ui.demo_mode = !ui.demo_mode;
                clear_selection(&mut ui);
                demo_ticks = 0;
                gameover_ticks = 0;
                present_board_and_panel(&mut hw, game, &ui);
            } else if is_tap && BTN_NEW.contains(p0) {
                *game = ChessGame::new(hw.rng.random());
                clear_selection(&mut ui);
                ui.status = game.status();
                ui.banner_open = false;
                gameover_ticks = 0;
                present_board_and_panel(&mut hw, game, &ui);
            } else if is_tap && BTN_UNDO.contains(p0) && !ui.demo_mode {
                if game.undo(ui.mode) {
                    clear_selection(&mut ui);
                    ui.status = game.status();
                    ui.banner_open = false;
                    present_board_and_panel(&mut hw, game, &ui);
                }
            } else if is_tap && BTN_MODE.contains(p0) {
                ui.mode = ui.mode.next();
                ui.demo_mode = false;
                clear_selection(&mut ui);
                present_board_and_panel(&mut hw, game, &ui);
            } else if let Some((pf, pt)) = ui.promo_pending {
                // Вибір фігури у модальному вікні перетворення пішака
                let mut picked = None;
                for (i, &pk) in PROMO_KINDS.iter().enumerate() {
                    if promo_slot_rect(i).contains(p0) {
                        picked = Some(pk);
                        break;
                    }
                }
                if let Some(pk) = picked {
                    if let Some(mv) = game.find_legal_move(pf, pt, Some(pk)) {
                        apply_move_and_sync(&mut hw, game, &mut ui, mv);
                    } else {
                        clear_selection(&mut ui);
                    }
                } else {
                    clear_selection(&mut ui);
                }
                present_board_and_panel(&mut hw, game, &ui);
            } else if ui.banner_open && ui.status.is_game_over() && hit_board_sq(p0).is_some() {
                // Тап по дошці ховає банер фіналу, щоб роздивитися позицію мату/пату
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
                        // Перетягування фігури з `from_drag` на `to_drag`
                        if game.is_promotion_move(from_drag, to_drag) {
                            ui.selected = Some(from_drag);
                            ui.promo_pending = Some((from_drag, to_drag));
                            present_board_and_panel(&mut hw, game, &ui);
                        } else if let Some(mv) = game.find_legal_move(from_drag, to_drag, None) {
                            apply_move_and_sync(&mut hw, game, &mut ui, mv);
                            present_board_and_panel(&mut hw, game, &ui);
                        }
                    } else if let Some(tapped) = sq0 {
                        let mut moved_or_modal = false;
                        if let Some(sel_sq) = ui.selected {
                            if sel_sq == tapped {
                                clear_selection(&mut ui);
                                moved_or_modal = true;
                            } else if game.is_promotion_move(sel_sq, tapped) {
                                ui.promo_pending = Some((sel_sq, tapped));
                                moved_or_modal = true;
                            } else if let Some(mv) = game.find_legal_move(sel_sq, tapped, None) {
                                apply_move_and_sync(&mut hw, game, &mut ui, mv);
                                moved_or_modal = true;
                            }
                        }

                        if !moved_or_modal {
                            if let Some(pc) = game.piece_at(tapped)
                                && pc.color == game.turn
                            {
                                ui.selected = Some(tapped);
                                ui.sel_moves_len = game.legal_moves_from(tapped, &mut ui.sel_moves);
                            } else {
                                clear_selection(&mut ui);
                            }
                        }
                        present_board_and_panel(&mut hw, game, &ui);
                    }
                }
            }
        }
        was_pressed = pressed;

        // 3. Хід ШІ (у режимі 1P проти ШІ або в режимі DEMO "ШІ проти ШІ")
        if ui.demo_mode {
            if ui.status.is_game_over() {
                gameover_ticks = gameover_ticks.wrapping_add(1);
                if gameover_ticks >= 140 {
                    gameover_ticks = 0;
                    *game = ChessGame::new(hw.rng.random());
                    clear_selection(&mut ui);
                    ui.status = game.status();
                    ui.banner_open = false;
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
                let mut mb = [0u8; 6];
                println!(
                    "CHESS AI ({:?}): {} за {} ms",
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
