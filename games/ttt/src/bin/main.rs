#![no_std]
#![no_main]

use core::fmt::Write;
use cyd_c5_common::{
    Board, Fb, SCREEN_H, SCREEN_W, dump_frame_hex,
    ui::{
        ACCENT_GOLD, BTN_STD_BG, BTN_STD_BORDER, BtnRect, FontKind, LineBuf, TEXT_MUTED,
        TEXT_WHITE, draw_btn, draw_exit_btn, elapsed_ms, fill, rgb, text,
    },
};
use cyd_game_ttt::game::{AiMode, Cell, Game, selftest};
use embedded_graphics::{
    pixelcolor::Rgb565,
    prelude::*,
    primitives::{Circle, Line, PrimitiveStyle, Rectangle},
};
use esp_println::println;

#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    println!("TTT PANIC: {:?}", info);
    loop {}
}

esp_bootloader_esp_idf::esp_app_desc!(
    "0.1.0",
    "cyd-game-ttt",
    "00:00:00",
    "2026-11-12",
    "5.5.6",
    64 * 1024,
    0,
    u16::MAX,
    0
);

const SAVE_FILE: &str = "TTT.SAV";

const C_BG: Rgb565 = Rgb565::new(2, 4, 5);
const C_BAR: Rgb565 = Rgb565::new(3, 6, 9);
const C_CELL: Rgb565 = Rgb565::new(4, 9, 12);
const C_BORDER: Rgb565 = Rgb565::new(8, 18, 24);
const C_X: Rgb565 = Rgb565::new(31, 12, 12);
const C_O: Rgb565 = Rgb565::new(10, 48, 31);
const C_WIN_X: Rgb565 = rgb(92, 34, 34);
const C_WIN_O: Rgb565 = rgb(24, 92, 64);

const CELL_W: i32 = 96;
const CELL_H: i32 = 54;
const CELL_GAP: i32 = 4;
const BOARD_X: i32 = 12;
const BOARD_Y: i32 = 34;

const BTN_NEW: BtnRect = BtnRect::new(12, 208, 90, 28);
const BTN_AI: BtnRect = BtnRect::new(110, 208, 102, 28);
const BTN_EXIT: BtnRect = BtnRect::new(220, 208, 88, 28);

fn cell_rect(i: usize) -> Rectangle {
    let col = (i % 3) as i32;
    let row = (i / 3) as i32;
    Rectangle::new(
        Point::new(
            BOARD_X + col * (CELL_W + CELL_GAP),
            BOARD_Y + row * (CELL_H + CELL_GAP),
        ),
        Size::new(CELL_W as u32, CELL_H as u32),
    )
}

fn present_cell(board: &mut Board, i: usize) {
    let r = cell_rect(i);
    board.present_rect(
        r.top_left.x as u16,
        r.top_left.y as u16,
        CELL_W as u16,
        CELL_H as u16,
    );
}

fn hit_cell(p: Point) -> Option<usize> {
    for i in 0..9 {
        let r = cell_rect(i);
        if p.x >= r.top_left.x
            && p.x < r.top_left.x + CELL_W
            && p.y >= r.top_left.y
            && p.y < r.top_left.y + CELL_H
        {
            return Some(i);
        }
    }
    None
}

fn hit_board_area(p: Point) -> bool {
    let w = 3 * CELL_W + 2 * CELL_GAP;
    let h = 3 * CELL_H + 2 * CELL_GAP;
    p.x >= BOARD_X && p.x < BOARD_X + w && p.y >= BOARD_Y && p.y < BOARD_Y + h
}

fn draw_status(g: &Game) {
    fill(0, 0, SCREEN_W as u32, 30, C_BAR);
    let mut line = LineBuf::<24>::new();
    let _ = write!(line, "X:{} O:{} D:{}", g.wins_x, g.wins_o, g.draws);
    text(8, 6, line.as_str(), FontKind::Bold, TEXT_WHITE);

    let (msg, color) = if g.over {
        match g.winner() {
            1 => ("X WINS! TAP", ACCENT_GOLD),
            2 => ("O WINS! TAP", C_O),
            _ => ("DRAW! TAP", TEXT_MUTED),
        }
    } else if g.turn_x {
        ("TURN: X", C_X)
    } else {
        ("TURN: O", C_O)
    };
    let mx = SCREEN_W - (msg.len() as i32) * 9 - 8;
    text(mx, 6, msg, FontKind::Bold, color);
}

fn redraw(g: &Game) {
    fill(0, 0, SCREEN_W as u32, SCREEN_H as u32, C_BG);
    draw_status(g);
    let win = g.winning_line();
    let w_who = g.winner();
    let win_bg = if w_who == 1 { C_WIN_X } else { C_WIN_O };

    for i in 0..9 {
        let r = cell_rect(i);
        let hi = win.map(|l| l.contains(&i)).unwrap_or(false);
        let _ = r
            .into_styled(PrimitiveStyle::with_fill(if hi { win_bg } else { C_CELL }))
            .draw(&mut Fb);
        let border_col = if hi { ACCENT_GOLD } else { C_BORDER };
        let _ = r
            .into_styled(PrimitiveStyle::with_stroke(border_col, 2))
            .draw(&mut Fb);
        let c = r.center();
        match g.b[i] {
            Cell::X => {
                let dx = 18;
                let dy = 12;
                let _ = Line::new(Point::new(c.x - dx, c.y - dy), Point::new(c.x + dx, c.y + dy))
                    .into_styled(PrimitiveStyle::with_stroke(C_X, 6))
                    .draw(&mut Fb);
                let _ = Line::new(Point::new(c.x - dx, c.y + dy), Point::new(c.x + dx, c.y - dy))
                    .into_styled(PrimitiveStyle::with_stroke(C_X, 6))
                    .draw(&mut Fb);
            }
            Cell::O => {
                let _ = Circle::new(Point::new(c.x - 15, c.y - 15), 30)
                    .into_styled(PrimitiveStyle::with_stroke(C_O, 5))
                    .draw(&mut Fb);
            }
            Cell::Empty => {}
        }
    }

    // Яскрава золота лінія перекреслення трьох виграшних клітинок!
    if let Some(l) = win {
        let p0 = cell_rect(l[0]).center();
        let p2 = cell_rect(l[2]).center();
        let _ = Line::new(p0, p2)
            .into_styled(PrimitiveStyle::with_stroke(ACCENT_GOLD, 5))
            .draw(&mut Fb);
    }

    draw_btn(
        BTN_NEW,
        "NEW",
        FontKind::Bold,
        BTN_STD_BG,
        TEXT_WHITE,
        BTN_STD_BORDER,
    );
    let (ai_lbl, ai_bg) = match g.ai_mode {
        AiMode::Hard => ("AI:HARD", rgb(24, 78, 56)),
        AiMode::Easy => ("AI:EASY", rgb(34, 88, 112)),
        AiMode::Off => ("2 PLAYR", BTN_STD_BG),
    };
    draw_btn(
        BTN_AI,
        ai_lbl,
        FontKind::Bold,
        ai_bg,
        TEXT_WHITE,
        BTN_STD_BORDER,
    );
    draw_exit_btn(BTN_EXIT, FontKind::Bold);
}

#[esp_hal::main]
fn main() -> ! {
    let mut board = Board::init();
    let checks = selftest();
    println!("TTT: selftest {} OK", checks);

    let mut game = Game::new(board.rng.random());
    let mut best_wins = board.load_best(SAVE_FILE);
    game.wins_x = best_wins;

    redraw(&game);
    let t0 = esp_hal::time::Instant::now();
    board.present_all();
    println!("TTT: старт, кадр = {} ms", elapsed_ms(t0));

    if cfg!(feature = "framedump") {
        dump_frame_hex();
    }

    let mut was_pressed = false;
    let mut press_pt = Point::zero();

    loop {
        let mut sim_tap: Option<Point> = None;
        if let Some(ch) = board.poll_uart_byte() {
            match ch {
                b'X' | b'x' | b'Q' | b'q' => {
                    if game.wins_x > best_wins {
                        board.save_best(SAVE_FILE, game.wins_x);
                    }
                    println!("TTT: вихід у GameLauncher");
                    board.exit_to_launcher();
                }
                b'N' | b'n' => sim_tap = Some(Point::new(BTN_NEW.x + 10, BTN_NEW.y + 10)),
                b'A' | b'a' => sim_tap = Some(Point::new(BTN_AI.x + 10, BTN_AI.y + 10)),
                b'1'..=b'9' => {
                    let idx = (ch - b'1') as usize;
                    sim_tap = Some(cell_rect(idx).center());
                }
                _ => {}
            }
        }

        let pressed = board.touch.pressed();
        let triggered_pt = if let Some(pt) = sim_tap {
            Some(pt)
        } else if pressed && !was_pressed {
            board.delay.delay_millis(6);
            if board.touch.pressed() {
                press_pt = board.touch.point();
                was_pressed = true;
            }
            None
        } else if pressed && was_pressed {
            press_pt = board.touch.point();
            None
        } else if !pressed && was_pressed {
            was_pressed = false;
            Some(press_pt)
        } else {
            None
        };

        if let Some(p) = triggered_pt {

            if BTN_EXIT.contains(p) {
                if game.wins_x > best_wins {
                    board.save_best(SAVE_FILE, game.wins_x);
                }
                board.exit_to_launcher();
            } else if BTN_NEW.contains(p) {
                game.reset();
                redraw(&game);
                board.present_all();
            } else if BTN_AI.contains(p) {
                game.ai_mode = game.ai_mode.next();
                game.reset();
                redraw(&game);
                board.present_all();
            } else if game.over && hit_board_area(p) {
                // Після перемоги чи нічиєї тап по полю одразу починає нову партію!
                game.reset();
                redraw(&game);
                board.present_all();
            } else if let Some(i) = hit_cell(p)
                && !game.over
                && game.b[i] == Cell::Empty
                && !(game.ai_mode.is_ai() && !game.turn_x)
            {
                game.b[i] = if game.turn_x { Cell::X } else { Cell::O };
                let mut raised_best = false;
                match game.winner() {
                    3 => {
                        game.over = true;
                        game.draws += 1;
                    }
                    1 => {
                        game.over = true;
                        game.wins_x += 1;
                        if game.wins_x > best_wins {
                            best_wins = game.wins_x;
                            raised_best = true;
                        }
                    }
                    2 => {
                        game.over = true;
                        game.wins_o += 1;
                    }
                    _ => {
                        game.turn_x = !game.turn_x;
                    }
                }

                // Спершу оновлюємо екран (повністю при завершенні гри, щоб підсвітити всі 3 клітинки і лінію!),
                // і лише після малювання зберігаємо рекорд на SD!
                redraw(&game);
                if game.over {
                    board.present_all();
                } else {
                    present_cell(&mut board, i);
                    board.present_rect(0, 0, SCREEN_W as u16, 30);
                }
                if raised_best {
                    board.save_best(SAVE_FILE, best_wins);
                }

                if !game.over && game.ai_mode.is_ai() && !game.turn_x {
                    board.delay.delay_millis(280);
                    if let Some(m) = game.ai_choose_move() {
                        game.b[m] = Cell::O;
                        match game.winner() {
                            3 => {
                                game.over = true;
                                game.draws += 1;
                            }
                            2 => {
                                game.over = true;
                                game.wins_o += 1;
                            }
                            _ => {
                                game.turn_x = true;
                            }
                        }
                        redraw(&game);
                        if game.over {
                            // ВАЖЛИВО: при перемозі O перемальовуємо УСЕ поле (усі 3 виграшні клітинки + золоту лінію)!
                            board.present_all();
                        } else {
                            present_cell(&mut board, m);
                            board.present_rect(0, 0, SCREEN_W as u16, 30);
                        }
                    }
                    was_pressed = board.touch.pressed();
                }
            }
        }
        board.delay.delay_millis(15);
    }
}
