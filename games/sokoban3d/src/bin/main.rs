#![no_std]
#![no_main]

use core::{fmt::Write, mem::MaybeUninit};
use cyd_c5_common::{
    Board, dump_frame_hex, fb_mut,
    ui::{
        ACCENT_CYAN, ACCENT_GOLD, ACCENT_GREEN, BG_DEEP, BTN_STD_BG, BTN_STD_BORDER,
        BtnRect, CARD_BG, FontKind, LineBuf, PANEL_BG, TEXT_MUTED, TEXT_WHITE, blend_fill, draw_btn,
        draw_demo_btn, draw_exit_btn, elapsed_ms, fill, rgb, stroke_rect, text,
        text_centered, u32_str,
    },
};
use cyd_game_sokoban3d::game::{
    Dir, LEVELS, MAX_CELLS, MAX_W, MoveResult, Sokoban, cos_q15, selftest, sin_q15,
};

const LEVEL_COUNT: usize = LEVELS.len();
use embedded_graphics::{
    pixelcolor::Rgb565,
    prelude::Point,
};
use esp_hal::time::Instant;
use esp_println::println;

#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    println!("SOKOBAN3D PANIC: {:?}", info);
    loop {}
}

esp_bootloader_esp_idf::esp_app_desc!(
    "0.1.0",
    "cyd-game-sokoban3d",
    "00:00:00",
    "2026-11-12",
    "5.5.6",
    64 * 1024,
    0,
    u16::MAX,
    0
);

const SAVE_FILE: &str = "SOKOBAN.SAV";

static mut GAME_CELL: MaybeUninit<Sokoban> = MaybeUninit::uninit();

const FP: i64 = 1 << 16;

// Геометрія екрана
const BOARD_TOP: i32 = 22;
const BOARD_BOTTOM: i32 = 210;
const CX: i32 = 160;
const CY: i32 = 118;

// Компактний D-pad ліворуч над кнопками (дублює свайпи)
const DP_UP: BtnRect = BtnRect::new(20, 136, 26, 22);
const DP_LEFT: BtnRect = BtnRect::new(2, 160, 26, 22);
const DP_RIGHT: BtnRect = BtnRect::new(38, 160, 26, 22);
const DP_DOWN: BtnRect = BtnRect::new(20, 184, 26, 22);
const DP_BG: Rgb565 = rgb(34, 44, 62);

const BTN_ROT: BtnRect = BtnRect::new(4, 214, 58, 22);
const BTN_UNDO: BtnRect = BtnRect::new(66, 214, 58, 22);
const BTN_RESET: BtnRect = BtnRect::new(128, 214, 58, 22);
const BTN_DEMO: BtnRect = BtnRect::new(190, 214, 58, 22);
const BTN_EXIT: BtnRect = BtnRect::new(252, 214, 64, 22);

// Палітра 3D
const WALL_TOP: Rgb565 = rgb(104, 128, 160);
const WALL_A: Rgb565 = rgb(62, 82, 108);
const WALL_B: Rgb565 = rgb(42, 56, 78);
const FLOOR_A: Rgb565 = rgb(50, 58, 76);
const FLOOR_B: Rgb565 = rgb(42, 50, 66);
const GOAL_COL: Rgb565 = rgb(236, 194, 74);
const BOX_TOP: Rgb565 = rgb(232, 152, 62);
const BOX_A: Rgb565 = rgb(198, 112, 42);
const BOX_B: Rgb565 = rgb(150, 78, 28);
const BOX_OK_TOP: Rgb565 = rgb(112, 226, 132);
const BOX_OK_A: Rgb565 = rgb(66, 168, 96);
const BOX_OK_B: Rgb565 = rgb(44, 122, 72);
const PLR_TOP: Rgb565 = rgb(132, 234, 252);
const PLR_A: Rgb565 = rgb(64, 176, 206);
const PLR_B: Rgb565 = rgb(38, 118, 148);

struct Anim {
    frames: u16,
    total: u16,
    kind: u8, // 0 = обертання після перемоги, 1 = ручний поворот
}

struct Ui {
    yaw: u16,
    demo: bool,
    demo_ticks: u32,
    anim: Option<Anim>,
    clean_frames: u32,
    best: u32,
}

// ---------- Растеризація ----------

fn clip_y(y: i32) -> i32 {
    y.clamp(BOARD_TOP, BOARD_BOTTOM)
}

fn fill_span(y: i32, x0: i32, x1: i32, col: Rgb565) {
    if y < BOARD_TOP || y > BOARD_BOTTOM {
        return;
    }
    let a = x0.clamp(0, 319);
    let b = x1.clamp(0, 319);
    if b < a {
        return;
    }
    let fb = fb_mut();
    let row = (y as usize) * 320;
    for x in a..=b {
        fb[row + x as usize] = col;
    }
}

/// Заливка трикутника методом сканлайнів (цілочисельна інтерполяція).
fn fill_tri(p0: (i32, i32), p1: (i32, i32), p2: (i32, i32), col: Rgb565) {
    let mut pts = [p0, p1, p2];
    // сортування за y (3 елементи — вставками)
    if pts[0].1 > pts[1].1 {
        pts.swap(0, 1);
    }
    if pts[1].1 > pts[2].1 {
        pts.swap(1, 2);
    }
    if pts[0].1 > pts[1].1 {
        pts.swap(0, 1);
    }
    let (a, b, c) = (pts[0], pts[1], pts[2]);
    let y0 = clip_y(a.1);
    let y2 = clip_y(c.1);
    if y2 <= y0 {
        return;
    }
    let total_h = (c.1 - a.1).max(1) as i64;

    for y in y0..=y2 {
        let yl = y as i64;
        // x на ребрі a->c
        let x_ac = a.0 as i64 + (c.0 - a.0) as i64 * (yl - a.1 as i64) / total_h;
        // x на ребрі a->b (верх) або b->c (низ)
        let x_ab = if y <= b.1 {
            let h = (b.1 - a.1).max(1) as i64;
            a.0 as i64 + (b.0 - a.0) as i64 * (yl - a.1 as i64) / h
        } else {
            let h = (c.1 - b.1).max(1) as i64;
            b.0 as i64 + (c.0 - b.0) as i64 * (yl - b.1 as i64) / h
        };
        let (lo, hi) = if x_ac < x_ab {
            (x_ac, x_ab)
        } else {
            (x_ab, x_ac)
        };
        fill_span(y, lo as i32, hi as i32, col);
    }
}

fn fill_quad(q: [(i32, i32); 4], col: Rgb565) {
    fill_tri(q[0], q[1], q[2], col);
    fill_tri(q[0], q[2], q[3], col);
}

// ---------- Камера ----------

struct Cam {
    yaw: u16,
    scale: i64,
    tilt: i64,
    zh: i64,
    ox: i64,
    oy: i64,
}

impl Cam {
    fn new(yaw: u16, w: usize, h: usize) -> Self {
        let cells = (w + h).max(8) as i64;
        // Масштаб (px на клітинку): діагональ діорами 0.707·(w+h)·s має влізти у 296 px
        // по горизонталі (по вертикалі запас більший, бо нахил камери стискає її).
        let scale = (419 / cells).clamp(10, 32);
        Self {
            yaw,
            scale,
            tilt: (scale * 55 / 100).max(6),
            zh: (scale * 52 / 100).max(6),
            ox: (w as i64) * FP / 2,
            oy: (h as i64) * FP / 2,
        }
    }

    #[inline]
    fn project(&self, x: i64, y: i64, z: i64) -> (i32, i32, i64) {
        let c = cos_q15(self.yaw) as i64;
        let s = sin_q15(self.yaw) as i64;
        let x = x - self.ox;
        let y = y - self.oy;
        let xr = (x * c + y * s) >> 15;
        let yr = (-x * s + y * c) >> 15;
        let sx = CX + ((xr * self.scale) >> 16) as i32;
        let sy = CY + ((((yr * self.tilt) >> 16) - ((z * self.zh) >> 16)) as i32);
        (sx, sy, yr)
    }
}

fn draw_cube(
    cam: &Cam,
    x0: i64,
    y0: i64,
    x1: i64,
    y1: i64,
    z1: i64,
    top: Rgb565,
    sa: Rgb565,
    sb: Rgb565,
    inset: Option<Rgb565>,
) {
    let bottom = [
        cam.project(x0, y0, 0),
        cam.project(x1, y0, 0),
        cam.project(x1, y1, 0),
        cam.project(x0, y1, 0),
    ];
    let tops = [
        cam.project(x0, y0, z1),
        cam.project(x1, y0, z1),
        cam.project(x1, y1, z1),
        cam.project(x0, y1, z1),
    ];

    // Найближчий до глядача кут основи — той, що найнижче на екрані
    let mut front = 0usize;
    for i in 1..4 {
        if bottom[i].1 > bottom[front].1 {
            front = i;
        }
    }
    let next = (front + 1) % 4;
    let prev = (front + 3) % 4;

    // Дві видимі бічні грані
    fill_quad(
        [
            (bottom[front].0, bottom[front].1),
            (bottom[next].0, bottom[next].1),
            (tops[next].0, tops[next].1),
            (tops[front].0, tops[front].1),
        ],
        sa,
    );
    fill_quad(
        [
            (bottom[front].0, bottom[front].1),
            (bottom[prev].0, bottom[prev].1),
            (tops[prev].0, tops[prev].1),
            (tops[front].0, tops[front].1),
        ],
        sb,
    );

    // Верхня грань
    fill_quad(
        [
            (tops[0].0, tops[0].1),
            (tops[1].0, tops[1].1),
            (tops[2].0, tops[2].1),
            (tops[3].0, tops[3].1),
        ],
        top,
    );

    // Внутрішня рамка на кришці: відрізняє сусідні куби один від одного
    if let Some(col) = inset {
        let ix0 = x0 + (x1 - x0) * 18 / 100;
        let iy0 = y0 + (y1 - y0) * 18 / 100;
        let ix1 = x1 - (x1 - x0) * 18 / 100;
        let iy1 = y1 - (y1 - y0) * 18 / 100;
        let p = [
            cam.project(ix0, iy0, z1),
            cam.project(ix1, iy0, z1),
            cam.project(ix1, iy1, z1),
            cam.project(ix0, iy1, z1),
        ];
        fill_quad(
            [(p[0].0, p[0].1), (p[1].0, p[1].1), (p[2].0, p[2].1), (p[3].0, p[3].1)],
            col,
        );
    }
}

fn draw_floor(cam: &Cam, cx: usize, cy: usize, col: Rgb565) {
    let x0 = cx as i64 * FP;
    let y0 = cy as i64 * FP;
    let x1 = x0 + FP;
    let y1 = y0 + FP;
    let q = [
        cam.project(x0, y0, 0),
        cam.project(x1, y0, 0),
        cam.project(x1, y1, 0),
        cam.project(x0, y1, 0),
    ];
    fill_quad(
        [(q[0].0, q[0].1), (q[1].0, q[1].1), (q[2].0, q[2].1), (q[3].0, q[3].1)],
        col,
    );
}

fn draw_goal_mark(cam: &Cam, cx: usize, cy: usize) {
    // Ромб у центрі клітинки
    let x = cx as i64 * FP + FP / 2;
    let y = cy as i64 * FP + FP / 2;
    let r = FP * 28 / 100;
    let p = [
        cam.project(x, y - r, 0),
        cam.project(x + r, y, 0),
        cam.project(x, y + r, 0),
        cam.project(x - r, y, 0),
    ];
    fill_quad([(p[0].0, p[0].1), (p[1].0, p[1].1), (p[2].0, p[2].1), (p[3].0, p[3].1)], GOAL_COL);
}

fn background_fill() {
    let fb = fb_mut();
    for y in BOARD_TOP..=BOARD_BOTTOM {
        let t = (y - BOARD_TOP) as u32;
        let col = rgb(
            (10 + t / 12) as u8,
            (14 + t / 14) as u8,
            (24 + t / 10) as u8,
        );
        let row = (y as usize) * 320;
        for x in 0..320 {
            fb[row + x] = col;
        }
    }
}

fn render_scene(game: &Sokoban, ui: &Ui) {
    background_fill();
    let cam = Cam::new(ui.yaw, game.w, game.h);

    // Сортуємо клітинки за глибиною (дальші — першими)
    let mut order = [0u16; MAX_CELLS];
    let mut depth = [0i64; MAX_CELLS];
    let mut n = 0usize;
    for cy in 0..game.h {
        for cx in 0..game.w {
            let i = cy * MAX_W + cx;
            if !game.inside[i] && !game.wall[i] {
                continue;
            }
            let px = cx as i64 * FP + FP / 2;
            let py = cy as i64 * FP + FP / 2;
            let (_, _, yr) = cam.project(px, py, 0);
            order[n] = i as u16;
            depth[n] = yr;
            n += 1;
        }
    }
    for i in 1..n {
        let mut j = i;
        while j > 0 && depth[j - 1] > depth[j] {
            depth.swap(j - 1, j);
            order.swap(j - 1, j);
            j -= 1;
        }
    }

    for k in 0..n {
        let idx = order[k] as usize;
        let cx = idx % MAX_W;
        let cy = idx / MAX_W;
        let x0 = cx as i64 * FP;
        let y0 = cy as i64 * FP;
        let x1 = x0 + FP;
        let y1 = y0 + FP;

        if game.inside[idx] {
            let col = if (cx + cy) & 1 == 0 { FLOOR_A } else { FLOOR_B };
            draw_floor(&cam, cx, cy, col);
            if game.goal[idx] && !game.box_at[idx] {
                draw_goal_mark(&cam, cx, cy);
            }
        }

        if game.wall[idx] {
            // Стіну, оточену стінами з усіх боків, малювати не треба: її повністю
            // закривають сусідні куби тієї ж висоти. На великих оригінальних рівнях
            // (19×16) це прибирає більшість кубів.
            let mut buried = true;
            for (dx, dy) in [(-1i32, 0i32), (1, 0), (0, -1), (0, 1)] {
                let walled = match game.idx(cx as i32 + dx, cy as i32 + dy) {
                    Some(n) => game.wall[n],
                    None => true,
                };
                if !walled {
                    buried = false;
                    break;
                }
            }
            if !buried {
                draw_cube(&cam, x0, y0, x1, y1, FP * 3 / 2, WALL_TOP, WALL_A, WALL_B, None);
            }
            continue;
        }

        if game.box_at[idx] {
            if game.goal[idx] {
                draw_cube(
                    &cam, x0, y0, x1, y1, FP * 3 / 4, BOX_OK_TOP, BOX_OK_A, BOX_OK_B,
                    Some(rgb(190, 250, 200)),
                );
            } else {
                draw_cube(
                    &cam, x0, y0, x1, y1, FP * 3 / 4, BOX_TOP, BOX_A, BOX_B,
                    Some(rgb(250, 190, 96)),
                );
            }
        }

        if game.player == idx {
            draw_cube(
                &cam, x0, y0, x1, y1, FP * 6 / 10, PLR_TOP, PLR_A, PLR_B,
                Some(rgb(210, 250, 255)),
            );
        }
    }
}

// ---------- HUD ----------

fn tap_dir_button(p: Point) -> Option<Dir> {
    if DP_UP.contains(p) {
        Some(Dir::Up)
    } else if DP_DOWN.contains(p) {
        Some(Dir::Down)
    } else if DP_LEFT.contains(p) {
        Some(Dir::Left)
    } else if DP_RIGHT.contains(p) {
        Some(Dir::Right)
    } else {
        None
    }
}

fn draw_arrow(r: BtnRect, dir: Dir, col: Rgb565) {
    let cx = r.x + r.w as i32 / 2;
    let cy = r.y + r.h as i32 / 2;
    let n = 9i32;
    for i in 0..n {
        let w = (i * 8 / n).max(1);
        match dir {
            Dir::Up => fill(cx - w / 2, cy + 4 - i, w as u32, 1, col),
            Dir::Down => fill(cx - w / 2, cy - 4 + i, w as u32, 1, col),
            Dir::Left => fill(cx + 4 - i, cy - w / 2, 1, w as u32, col),
            Dir::Right => fill(cx - 4 + i, cy - w / 2, 1, w as u32, col),
        }
    }
}

fn draw_dpad() {
    for (r, d) in [
        (DP_UP, Dir::Up),
        (DP_DOWN, Dir::Down),
        (DP_LEFT, Dir::Left),
        (DP_RIGHT, Dir::Right),
    ] {
        draw_btn(r, "", FontKind::Small, DP_BG, TEXT_WHITE, rgb(84, 104, 138));
        draw_arrow(r, d, ACCENT_CYAN);
    }
}

fn draw_hud(game: &Sokoban, ui: &Ui) {
    fill(0, 0, 320, BOARD_TOP as u32, PANEL_BG);
    fill(0, BOARD_TOP - 1, 320, 1, rgb(64, 88, 128));
    text(6, 3, "3D SOKOBAN", FontKind::Bold, ACCENT_GOLD);

    let mut l = LineBuf::<28>::new();
    let mut n1 = [0u8; 10];
    let mut n2 = [0u8; 10];
    let mut n3 = [0u8; 10];
    let mut n4 = [0u8; 10];
    let _ = write!(
        l,
        "LVL {}/{} M:{} P:{}",
        u32_str(game.level as u32 + 1, &mut n1),
        u32_str(LEVEL_COUNT as u32, &mut n2),
        u32_str(game.moves, &mut n3),
        u32_str(game.pushes, &mut n4)
    );
    text(110, 3, l.as_str(), FontKind::Small, TEXT_MUTED);

    let mut b = LineBuf::<20>::new();
    let mut m1 = [0u8; 10];
    let mut m2 = [0u8; 10];
    let _ = write!(
        b,
        "BOX {}/{}",
        u32_str(game.boxes_on_goals() as u32, &mut m1),
        u32_str(game.box_count() as u32, &mut m2)
    );
    let bx = 320 - 6 - (b.as_str().len() as i32) * 6;
    text(bx, 3, b.as_str(), FontKind::Small, ACCENT_CYAN);

    // D-pad і кнопки
    draw_dpad();
    draw_btn(BTN_ROT, "ROTATE", FontKind::Small, BTN_STD_BG, TEXT_WHITE, BTN_STD_BORDER);
    let undo_fg = if game.can_undo() && !ui.demo {
        TEXT_WHITE
    } else {
        TEXT_MUTED
    };
    draw_btn(BTN_UNDO, "UNDO", FontKind::Small, BTN_STD_BG, undo_fg, BTN_STD_BORDER);
    draw_btn(BTN_RESET, "RESET", FontKind::Small, CARD_BG, ACCENT_GOLD, BTN_STD_BORDER);
    draw_demo_btn(BTN_DEMO, ui.demo, FontKind::Small);
    draw_exit_btn(BTN_EXIT, FontKind::Small);

    // Банер перемоги під час анімації обертання
    if matches!(ui.anim.as_ref().map(|a| a.kind), Some(0)) {
        blend_fill(46, 96, 228, 56, BG_DEEP, 210);
        stroke_rect(46, 96, 228, 56, ACCENT_GREEN);
        text_centered(160, 104, "LEVEL CLEARED", FontKind::Bold, ACCENT_GREEN);
        text_centered(160, 128, "3D SPIN...", FontKind::Small, TEXT_MUTED);
    }
}

#[esp_hal::main]
fn main() -> ! {
    let mut hw = Board::init();
    let (run, failed) = selftest();
    println!("SOKOBAN3D: selftest {}/{} failed={}", run - failed, run, failed);

    let game: &'static mut Sokoban = unsafe {
        let ptr = core::ptr::addr_of_mut!(GAME_CELL);
        (*ptr).write(Sokoban::load(0));
        (*ptr).assume_init_mut()
    };

    let mut ui = Ui {
        yaw: 96, // 33.75°: свайпи по осях екрана мапляться на світові осі однозначно
        demo: hw.start_in_demo(cfg!(feature = "demo")),
        demo_ticks: 0,
        anim: None,
        clean_frames: 0,
        best: hw.load_best(SAVE_FILE),
    };

    let t0 = Instant::now();
    render_scene(game, &ui);
    draw_hud(game, &ui);
    hw.present_all();
    println!(
        "SOKOBAN3D: старт, кадр = {} ms, demo={}, best={}",
        elapsed_ms(t0),
        ui.demo,
        ui.best
    );

    if cfg!(feature = "framedump") {
        dump_frame_hex();
    }

    let mut was_pressed = false;
    let mut press_pt = Point::zero();
    let mut last_pt = Point::zero();

    loop {
        // Будь-яка зміна стану (хід, undo, reset, кнопка) має перемалювати 3D-сцену
        let mut changed = false;

        // ---------- UART ----------
        while let Some(ch) = hw.poll_uart_byte() {
            match ch {
                b'X' | b'x' | b'Q' | b'q' => {
                    let score = game.level as u32 * 100 + game.moves.max(1);
                    if score > ui.best {
                        ui.best = score;
                        hw.save_best(SAVE_FILE, ui.best);
                    }
                    println!("SOKOBAN3D: вихід у GameLauncher (best={})", ui.best);
                    hw.exit_to_launcher();
                }
                b'D' | b'd' => {
                    ui.demo = !ui.demo;
                    ui.demo_ticks = 0;
                }
                b'N' | b'n' => {
                    game.next_level();
                    changed = true;
                }
                b'P' | b'p' => {
                    game.prev_level();
                    changed = true;
                }
                b'R' | b'r' => {
                    game.reset();
                    changed = true;
                }
                b'U' | b'u' => {
                    if game.undo() {
                        changed = true;
                    }
                }
                b'O' | b'o' => {
                    ui.anim = Some(Anim {
                        frames: 0,
                        total: 14,
                        kind: 1,
                    });
                }
                b'8' => {
                    if game.try_move(Dir::Up) != MoveResult::Blocked {
                        log_move(game, Dir::Up);
                        on_move(game, &mut ui, &mut hw);
                        changed = true;
                    }
                }
                b'2' => {
                    if game.try_move(Dir::Down) != MoveResult::Blocked {
                        log_move(game, Dir::Down);
                        on_move(game, &mut ui, &mut hw);
                        changed = true;
                    }
                }
                b'4' => {
                    if game.try_move(Dir::Left) != MoveResult::Blocked {
                        log_move(game, Dir::Left);
                        on_move(game, &mut ui, &mut hw);
                        changed = true;
                    }
                }
                b'6' => {
                    if game.try_move(Dir::Right) != MoveResult::Blocked {
                        log_move(game, Dir::Right);
                        on_move(game, &mut ui, &mut hw);
                        changed = true;
                    }
                }
                _ => {}
            }
        }

        // ---------- Тачскрін ----------
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
            let dx = last_pt.x - press_pt.x;
            let dy = last_pt.y - press_pt.y;
            let adx = dx.abs();
            let ady = dy.abs();

            if BTN_EXIT.contains(p) && adx < 18 && ady < 18 {
                let score = game.level as u32 * 100 + game.moves.max(1);
                if score > ui.best {
                    ui.best = score;
                    hw.save_best(SAVE_FILE, ui.best);
                }
                println!("SOKOBAN3D: вихід у GameLauncher (best={})", ui.best);
                hw.exit_to_launcher();
            } else if BTN_DEMO.contains(p) && adx < 18 && ady < 18 {
                ui.demo = !ui.demo;
                ui.demo_ticks = 0;
                changed = true;
            } else if BTN_ROT.contains(p) && adx < 18 && ady < 18 {
                ui.anim = Some(Anim {
                    frames: 0,
                    total: 14,
                    kind: 1,
                });
            } else if BTN_UNDO.contains(p) && adx < 18 && ady < 18 {
                if game.undo() {
                    changed = true;
                }
            } else if BTN_RESET.contains(p) && adx < 18 && ady < 18 {
                game.reset();
                changed = true;
            } else if adx < 18 && ady < 18 && tap_dir_button(p).is_some() {
                let dir = tap_dir_button(p).unwrap();
                if game.try_move(dir) != MoveResult::Blocked {
                    log_move(game, dir);
                    on_move(game, &mut ui, &mut hw);
                }
                changed = true;
            } else if press_pt.y < BOARD_BOTTOM
                && press_pt.y > BOARD_TOP
                && (adx > 22 || ady > 22)
                && ui.anim.is_none()
                && !game.cleared
            {
                // Свайп по дошці → хід у напрямку, узгодженому з поточним поворотом камери
                let dir = swipe_dir(ui.yaw, dx, dy);
                if game.try_move(dir) != MoveResult::Blocked {
                    log_move(game, dir);
                    on_move(game, &mut ui, &mut hw);
                }
                changed = true;
            }
        }
        was_pressed = pressed;

        // ---------- Анімація обертання ----------
        let mut dirty = changed;
        if let Some(anim) = ui.anim.as_mut() {
            anim.frames += 1;
            if anim.kind == 0 {
                ui.yaw = ui.yaw.wrapping_add(19); // ~1 оберт за 56 кадрів
            } else {
                ui.yaw = ui.yaw.wrapping_add(20); // крок 90° (256/14 ≈ 18..20)
            }
            dirty = true;
            let done = anim.frames >= anim.total;
            if done {
                let kind = anim.kind;
                ui.anim = None;
                if kind == 0 {
                    game.next_level();
                    ui.yaw = 96;
                } else {
                    ui.yaw &= 0x3FF;
                }
                dirty = true;
            }
        }

        // ---------- DEMO: огляд 3D + автоперехід ----------
        if ui.demo {
            ui.demo_ticks += 1;
            if ui.anim.is_none() {
                ui.yaw = ui.yaw.wrapping_add(3);
                if ui.demo_ticks >= 260 {
                    ui.demo_ticks = 0;
                    // показовий «прохід»: обертання + перехід на наступний рівень
                    ui.anim = Some(Anim {
                        frames: 0,
                        total: 14,
                        kind: 1,
                    });
                }
            }
            dirty = true;
        }

        if dirty {
            render_scene(game, &ui);
            draw_hud(game, &ui);
            hw.present_all();
            ui.clean_frames = 0;
        } else {
            ui.clean_frames = ui.clean_frames.wrapping_add(1);
        }

        hw.delay.delay_millis(14);
    }
}

/// Хід із повідомленням про перемогу та запуском 3D-обертання.
fn on_move(game: &mut Sokoban, ui: &mut Ui, hw: &mut Board) {
    if game.cleared {
        ui.anim = Some(Anim {
            frames: 0,
            total: 56,
            kind: 0,
        });
        let score = game.level as u32 * 100 + 200u32.saturating_sub(game.moves.min(200));
        if score > ui.best {
            ui.best = score;
            hw.save_best(SAVE_FILE, ui.best);
        }
        println!(
            "SOKOBAN3D: рівень {} пройдено за {} кроків ({} штовхань), score={}",
            game.level + 1,
            game.moves,
            game.pushes,
            score
        );
    }
}

/// Перетворює свайп по екрану в напрямок світу: обирає ту з чотирьох світових
/// осей, чий **екранний** вектор найкраще збігається зі свайпом.
/// (Порівнювати світові осі напряму не можна: при повороті камери 0.707·(x+y)
/// вони лягають на діагоналі екрана, і «домінантна вісь» завжди давала Down/Up.)
fn swipe_dir(yaw: u16, dx: i32, dy: i32) -> Dir {
    let c = cos_q15(yaw) as i64; // Q15
    let s = sin_q15(yaw) as i64;
    const CAND: [(Dir, i64, i64); 4] = [
        (Dir::Right, 1, 0),
        (Dir::Down, 0, 1),
        (Dir::Left, -1, 0),
        (Dir::Up, 0, -1),
    ];
    let mut best = Dir::Down;
    let mut best_dot = i64::MIN;
    for (d, wx, wy) in CAND {
        // Екранний вектор світової осі (той самий, що в Cam::project)
        let sx = wx * c + wy * s;
        let sy = (-wx * s + wy * c) * 55 / 100; // вертикаль стиснута нахилом камери
        let dot = sx * dx as i64 + sy * dy as i64;
        if dot > best_dot {
            best_dot = dot;
            best = d;
        }
    }
    best
}

fn log_move(game: &Sokoban, d: Dir) {
    let name = match d {
        Dir::Up => "UP",
        Dir::Down => "DOWN",
        Dir::Left => "LEFT",
        Dir::Right => "RIGHT",
    };
    println!(
        "SOKOBAN3D: крок {} -> moves={} pushes={}",
        name, game.moves, game.pushes
    );
}
