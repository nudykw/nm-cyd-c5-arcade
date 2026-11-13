#![no_std]
#![no_main]

use core::{fmt::Write, mem::MaybeUninit};
use cyd_c5_common::{
    Board, SCREEN_H, SCREEN_W, dump_frame_hex, fb_mut, put_px,
    ui::{
        ACCENT_CYAN, ACCENT_GOLD, ACCENT_GREEN, ACCENT_RED, BG_DEEP, BtnRect, FontKind, LineBuf,
        TEXT_MUTED, TEXT_WHITE, blend_fill, draw_btn, draw_demo_btn, draw_exit_btn, elapsed_ms,
        fill, rgb, stroke_rect, text, text_centered, u32_str,
    },
};
use cyd_game_cybermaze::game::{
    Game, GameState, HP_MAX, Input, MAX_ENEMIES, MAX_PICKUPS, MAP_H, MAP_W, cos_q15, selftest,
    sin_q15,
};
use embedded_graphics::{
    pixelcolor::{Rgb565, RgbColor},
    prelude::Point,
};
use esp_hal::time::Instant;
use esp_println::println;

#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    println!("CYBERMAZE PANIC: {:?}", info);
    loop {}
}

esp_bootloader_esp_idf::esp_app_desc!(
    "0.1.0",
    "cyd-game-cybermaze",
    "00:00:00",
    "2026-11-12",
    "5.5.6",
    64 * 1024,
    0,
    u16::MAX,
    0
);

const SAVE_FILE: &str = "CYBERMZ.SAV";

static mut GAME_CELL: MaybeUninit<Game> = MaybeUninit::uninit();

// Внутрішній 3D-буфер: 160×120 (38 КБ), далі розтягуємо ×2 у повний кадр 320×240
const VW: usize = 160;
const VH: usize = 120;
static mut VIEW: [Rgb565; VW * VH] = [Rgb565::BLACK; VW * VH];
static mut COL_OFF: [i32; VW] = [0; VW];
static mut COL_COS: [i32; VW] = [0; VW];
static mut WALL_LIGHT: [Rgb565; 16] = [Rgb565::BLACK; 16];
static mut WALL_DARK: [Rgb565; 16] = [Rgb565::BLACK; 16];
static mut ROW_COL: [Rgb565; VH] = [Rgb565::BLACK; VH];
static mut DEPTH: [i32; VW] = [0; VW];

// HUD
const BTN_FIRE: BtnRect = BtnRect::new(166, 210, 46, 26);
const BTN_DEMO: BtnRect = BtnRect::new(216, 210, 46, 26);
const BTN_EXIT: BtnRect = BtnRect::new(266, 210, 48, 26);
const STICK_SPLIT: i32 = 160;

fn init_palette() {
    unsafe {
        let off = core::ptr::addr_of_mut!(COL_OFF);
        let cos = core::ptr::addr_of_mut!(COL_COS);
        let span = (2 * 94) as i32; // FOV ≈ 66°
        for col in 0..VW {
            let o = ((col as i32) * span) / (VW as i32 - 1) - 94;
            (*off)[col] = o;
            (*cos)[col] = cos_q15(o as u16);
        }

        let light = core::ptr::addr_of_mut!(WALL_LIGHT);
        let dark = core::ptr::addr_of_mut!(WALL_DARK);
        for i in 0..16 {
            let l = i as i32; // 0..15, 15 = найближче
            let f = (40 + l * 14) as u16; // 40..250
            (*light)[i] = rgb((f / 6) as u8, (f * 3 / 4) as u8, (f * 9 / 10) as u8);
            (*dark)[i] = rgb((f / 9) as u8, (f / 2) as u8, (f * 3 / 4) as u8);
        }

        let row = core::ptr::addr_of_mut!(ROW_COL);
        for y in 0..VH {
            if y < VH / 2 {
                // стеля: від темного горизонту до синього верху
                let t = (VH / 2 - y) as u16;
                (*row)[y] = rgb((8 + t / 5) as u8, (12 + t / 4) as u8, (26 + t / 2) as u8);
            } else {
                // підлога: від темного горизонту до сірого низу
                let t = (y - VH / 2) as u16;
                (*row)[y] = rgb((18 + t / 2) as u8, (20 + t / 2) as u8, (26 + t / 2) as u8);
            }
        }
    }
}

fn view() -> &'static mut [Rgb565; VW * VH] {
    unsafe { &mut *core::ptr::addr_of_mut!(VIEW) }
}


fn paint_background() {
    let row = unsafe { &*core::ptr::addr_of!(ROW_COL) };
    let v = view();
    for y in 0..VH {
        let c = row[y];
        let base = y * VW;
        for x in 0..VW {
            v[base + x] = c;
        }
    }
}

fn render_walls(game: &Game) {
    let light = unsafe { &*core::ptr::addr_of!(WALL_LIGHT) };
    let dark = unsafe { &*core::ptr::addr_of!(WALL_DARK) };
    let offs = unsafe { &*core::ptr::addr_of!(COL_OFF) };
    let cosc = unsafe { &*core::ptr::addr_of!(COL_COS) };
    let depth = unsafe { &mut *core::ptr::addr_of_mut!(DEPTH) };
    let v = view();

    for col in 0..VW {
        let ang = game.angle.wrapping_add(offs[col] as u16);
        let hit = game.cast_ray(ang);
        // Прибираємо «риб'яче око»: множимо на cos відхилення променя від центру
        let perp = (((hit.dist as i64) * cosc[col] as i64) >> 15) as i32;
        let perp = perp.max(FP_ONE / 8);
        depth[col] = perp;

        let h = ((FP_ONE as i64 * 92) / perp as i64) as i32;
        let y0 = VH as i32 / 2 - h / 2;
        let y1 = y0 + h;

        let mut shade = 15 - ((perp >> 16) * 2);
        // «Шаховий» малюнок по клітинках стіни — дешевий орієнтир глибини
        let parity = ((hit.cell as i32 % MAP_W as i32) + (hit.cell as i32 / MAP_W as i32)) & 1;
        if parity == 1 {
            shade -= 2;
        }
        let mut shade = shade.clamp(0, 15) as i32;
        if hit.side == 1 && shade > 0 {
            shade -= 1;
        }
        let c = if hit.side == 0 {
            light[shade as usize]
        } else {
            dark[shade as usize]
        };

        let ys = y0.max(0);
        let ye = y1.min(VH as i32);
        let mut y = ys;
        while y < ye {
            v[(y as usize) * VW + col] = c;
            y += 1;
        }
    }
}

const FP_ONE: i32 = 1 << 16;

fn render_sprite(game: &Game, cx: i32, cy: i32, _depth: i32, height: i32, kind: u8, flash: bool) {
    // Позиція в системі камери
    let rx = cx - game.px;
    let ry = cy - game.py;
    let ca = cos_q15(game.angle) as i64;
    let sa = sin_q15(game.angle) as i64;
    let along = ((rx as i64 * ca + ry as i64 * sa) >> 15) as i32;
    let lateral = ((-rx as i64 * sa + ry as i64 * ca) >> 15) as i32;
    if along <= FP_ONE / 8 {
        return;
    }
    let sx = VW as i32 / 2 + ((lateral as i64 * 123) / along as i64) as i32;

    let h = ((FP_ONE as i64 * height as i64) / along as i64) as i32;
    let w = (h / 2).clamp(1, VW as i32);
    let y0 = VH as i32 / 2 - h / 2;
    let y1 = y0 + h;
    let x0 = sx - w / 2;
    let x1 = x0 + w;

    let base = match kind {
        0 => {
            // дрон: корпус + «визор»
            rgb(196, 92, 32)
        }
        1 => {
            // аптечка
            rgb(64, 200, 96)
        }
        _ => {
            // вихід
            rgb(72, 232, 232)
        }
    };
    let head = match kind {
        0 => rgb(236, 168, 64),
        1 => rgb(180, 246, 200),
        _ => rgb(180, 255, 255),
    };
    let visor = if flash {
        rgb(255, 255, 255)
    } else {
        rgb(240, 64, 64)
    };

    let wall_depth = unsafe { &*core::ptr::addr_of!(DEPTH) };
    for x in x0.max(0)..x1.min(VW as i32) {
        if wall_depth[x as usize] < along {
            continue; // стіна ближче — справу приховано
        }
        for y in y0.max(0)..y1.min(VH as i32) {
            let rel = (y - y0) as i32;
            let c = if kind == 0 {
                if rel < h / 4 {
                    head
                } else if rel < h / 3 {
                    visor
                } else {
                    base
                }
            } else {
                if rel < h / 5 { head } else { base }
            };
            view()[(y as usize) * VW + x as usize] = c;
        }
    }
}

fn render_scene(game: &Game) {
    paint_background();
    render_walls(game);

    // Спрайти: вихід, аптечки, дрони — від далеких до близьких
    let exit_cx = (((game.level.exit % MAP_W) as i32) << 16) + FP_ONE / 2;
    let exit_cy = (((game.level.exit / MAP_W) as i32) << 16) + FP_ONE / 2;
    let dx = exit_cx - game.px;
    let dy = exit_cy - game.py;
    let exit_d = (dx * dx + dy * dy).max(1);
    render_sprite(game, exit_cx, exit_cy, exit_d, 70, 2, false);

    for i in 0..MAX_PICKUPS {
        let p = game.pickups[i];
        if p.taken {
            continue;
        }
        let dx = p.x - game.px;
        let dy = p.y - game.py;
        render_sprite(game, p.x, p.y, (dx * dx + dy * dy).max(1), 26, 1, false);
    }

    for i in 0..MAX_ENEMIES {
        let e = game.enemies[i];
        if !e.alive {
            continue;
        }
        let dx = e.x - game.px;
        let dy = e.y - game.py;
        render_sprite(game, e.x, e.y, (dx * dx + dy * dy).max(1), 54, 0, e.hit_flash > 0);
    }
}

/// Розтягування 160×120 → 320×240 прямо у кадровий буфер.
fn blit_scaled() {
    let v = view();
    let fb = fb_mut();
    for y in 0..VH {
        let src = y * VW;
        let d0 = (y * 2) * SCREEN_W as usize;
        let d1 = d0 + SCREEN_W as usize;
        for x in 0..VW {
            let c = v[src + x];
            let dx = x * 2;
            fb[d0 + dx] = c;
            fb[d0 + dx + 1] = c;
            fb[d1 + dx] = c;
            fb[d1 + dx + 1] = c;
        }
    }
}

fn draw_minimap(game: &Game) {
    const MM_CELL: i32 = 3;
    let x0 = 320 - 4 - (MAP_W as i32) * MM_CELL;
    let y0 = 22;
    blend_fill(
        x0 - 2,
        y0 - 2,
        ((MAP_W as i32) * MM_CELL + 4) as u32,
        ((MAP_H as i32) * MM_CELL + 4) as u32,
        BG_DEEP,
        190,
    );
    for cy in 0..MAP_H as i32 {
        for cx in 0..MAP_W as i32 {
            let wall = game.level.is_wall_cell(cx, cy);
            let c = if wall {
                rgb(40, 92, 120)
            } else {
                rgb(12, 22, 34)
            };
            fill(
                x0 + cx * MM_CELL,
                y0 + cy * MM_CELL,
                MM_CELL as u32,
                MM_CELL as u32,
                c,
            );
        }
    }
    // вихід
    let ex = game.level.exit % MAP_W;
    let ey = game.level.exit / MAP_W;
    fill(
        x0 + ex as i32 * MM_CELL,
        y0 + ey as i32 * MM_CELL,
        MM_CELL as u32,
        MM_CELL as u32,
        ACCENT_GREEN,
    );
    // аптечки
    for p in game.pickups.iter() {
        if p.taken {
            continue;
        }
        let px = (p.x >> 16).clamp(0, MAP_W as i32 - 1);
        let py = (p.y >> 16).clamp(0, MAP_H as i32 - 1);
        put_px(x0 + px * MM_CELL + 1, y0 + py * MM_CELL + 1, ACCENT_CYAN);
    }
    // дрони
    for e in game.enemies.iter() {
        if !e.alive {
            continue;
        }
        let px = (e.x >> 16).clamp(0, MAP_W as i32 - 1);
        let py = (e.y >> 16).clamp(0, MAP_H as i32 - 1);
        fill(
            x0 + px * MM_CELL,
            y0 + py * MM_CELL,
            MM_CELL as u32,
            MM_CELL as u32,
            ACCENT_RED,
        );
    }
    // гравець
    let px = (game.px >> 16).clamp(0, MAP_W as i32 - 1);
    let py = (game.py >> 16).clamp(0, MAP_H as i32 - 1);
    fill(
        x0 + px * MM_CELL,
        y0 + py * MM_CELL,
        MM_CELL as u32,
        MM_CELL as u32,
        ACCENT_GOLD,
    );
}

fn draw_hud(game: &Game, demo: bool, best: u32) {
    // Напівпрозора підкладка, щоб текст читався на тлі яскравих стін
    blend_fill(2, 2, 196, 42, BG_DEEP, 165);

    // Здоров'я
    fill(6, 6, 96, 12, rgb(28, 16, 20));
    let hp_w = (92 * game.hp.max(0) as u32) / HP_MAX as u32;
    let hp_col = if game.hp > 60 {
        ACCENT_GREEN
    } else if game.hp > 30 {
        ACCENT_GOLD
    } else {
        ACCENT_RED
    };
    fill(8, 8, hp_w, 8, hp_col);
    stroke_rect(6, 6, 96, 12, rgb(120, 132, 160));

    let mut l1 = LineBuf::<24>::new();
    let mut nb = [0u8; 10];
    let _ = write!(l1, "HP {}", u32_str(game.hp.max(0) as u32, &mut nb));
    text(106, 5, l1.as_str(), FontKind::Small, TEXT_WHITE);

    let mut l2 = LineBuf::<24>::new();
    let mut nb1 = [0u8; 10];
    let mut nb2 = [0u8; 10];
    let _ = write!(
        l2,
        "AMMO {}  SCR {}",
        u32_str(game.ammo as u32, &mut nb1),
        u32_str(game.score, &mut nb2)
    );
    text(6, 20, l2.as_str(), FontKind::Small, ACCENT_CYAN);

    let mut l3 = LineBuf::<24>::new();
    let mut nb1 = [0u8; 10];
    let mut nb2 = [0u8; 10];
    let mut nb3 = [0u8; 10];
    let _ = write!(
        l3,
        "LVL {}  KILLS {}  BEST {}",
        u32_str(game.level_no as u32, &mut nb1),
        u32_str(game.kills as u32, &mut nb2),
        u32_str(best, &mut nb3)
    );
    text(6, 32, l3.as_str(), FontKind::Small, ACCENT_GOLD);

    // Приціл
    let cx = SCREEN_W / 2;
    let cy = SCREEN_H / 2;
    fill(cx - 5, cy, 4, 1, TEXT_WHITE);
    fill(cx + 2, cy, 4, 1, TEXT_WHITE);
    fill(cx, cy - 5, 1, 4, TEXT_WHITE);
    fill(cx, cy + 2, 1, 4, TEXT_WHITE);

    // Кнопки
    draw_btn(
        BTN_FIRE,
        "FIRE",
        FontKind::Small,
        rgb(140, 40, 40),
        TEXT_WHITE,
        ACCENT_RED,
    );
    draw_demo_btn(BTN_DEMO, demo, FontKind::Small);
    draw_exit_btn(BTN_EXIT, FontKind::Small);

    // Червоне попередження про поранення
    if game.flash > 0 {
        for i in 0..4i32 {
            stroke_rect(
                i,
                i,
                (SCREEN_W - 2 * i) as u32,
                (SCREEN_H - 2 * i) as u32,
                ACCENT_RED,
            );
        }
    }

    // Смерть / новий рівень
    if game.state == GameState::Dead {
        blend_fill(40, 90, 240, 60, BG_DEEP, 220);
        stroke_rect(40, 90, 240, 60, ACCENT_RED);
        text_centered(160, 98, "SYSTEM FAILURE", FontKind::Bold, ACCENT_RED);
        text_centered(160, 120, "RESPAWNING...", FontKind::Small, TEXT_MUTED);
    } else if game.level_cleared > 0 {
        blend_fill(50, 92, 220, 52, BG_DEEP, 200);
        stroke_rect(50, 92, 220, 52, ACCENT_GREEN);
        text_centered(160, 100, "LEVEL CLEARED", FontKind::Bold, ACCENT_GREEN);
        let mut s = LineBuf::<18>::new();
        let mut nb = [0u8; 10];
        let _ = write!(s, "NEXT: LVL {}", u32_str(game.level_no as u32, &mut nb));
        text_centered(160, 122, s.as_str(), FontKind::Small, TEXT_WHITE);
    }

    let _ = demo;
}

#[esp_hal::main]
fn main() -> ! {
    let mut hw = Board::init();
    let (run, failed) = selftest();
    println!("CYBERMAZE: selftest {}/{} failed={}", run - failed, run, failed);
    init_palette();

    let game: &'static mut Game = unsafe {
        let ptr = core::ptr::addr_of_mut!(GAME_CELL);
        (*ptr).write(Game::new(hw.rng.random()));
        (*ptr).assume_init_mut()
    };

    let demo_start = hw.start_in_demo(cfg!(feature = "demo"));
    let mut demo_mode = demo_start;
    let mut best = hw.load_best(SAVE_FILE);

    let t0 = Instant::now();
    render_scene(game);
    blit_scaled();
    draw_minimap(game);
    draw_hud(game, demo_mode, best);
    hw.present_all();
    println!(
        "CYBERMAZE: старт, кадр = {} ms, demo={}, best={}",
        elapsed_ms(t0),
        demo_mode,
        best
    );

    if cfg!(feature = "framedump") {
        dump_frame_hex();
    }

    let mut was_pressed = false;
    let mut press_pt = Point::zero();
    let mut last_pt = Point::zero();
    let mut stick = (0i32, 0i32);
    let mut look_prev_x = 0i32;
    let mut look_moved = 0i32;
    let mut firing = false;
    let mut turn_accum = 0i32;

    let mut frame: u32 = 0;
    let mut t_fps = Instant::now();
    let mut sim_turn = 0i32;
    let mut sim_forward = 0i32;
    let mut sim_fire = false;

    loop {
        // UART: X/Q — вихід, D — демо, N — новий лабіринт, F — постріл, WASD-рух/поворот
        while let Some(ch) = hw.poll_uart_byte() {
            match ch {
                b'X' | b'x' | b'Q' | b'q' => {
                    if game.score > best {
                        best = game.score;
                    }
                    hw.save_best(SAVE_FILE, best);
                    println!("CYBERMAZE: вихід у GameLauncher (best={})", best);
                    hw.exit_to_launcher();
                }
                b'D' | b'd' => {
                    demo_mode = !demo_mode;
                    stick = (0, 0);
                    firing = false;
                }
                b'N' | b'n' => {
                    game.next_level();
                }
                b'F' | b'f' => sim_fire = true,
                b'W' | b'w' => sim_forward = 256,
                b'S' | b's' => sim_forward = -256,
                b'A' | b'a' => sim_turn = -26,
                b'L' | b'l' => sim_turn = 26,
                _ => {}
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
                stick = (0, 0);
                look_prev_x = pt.x;
                look_moved = 0;
                firing = BTN_FIRE.contains(pt);
            } else if !firing {
                if press_pt.x < STICK_SPLIT && press_pt.y < 200 {
                    let dx = (pt.x - press_pt.x).clamp(-60, 60);
                    let dy = (pt.y - press_pt.y).clamp(-60, 60);
                    stick = (dx, dy);
                } else if press_pt.x >= STICK_SPLIT && press_pt.y < 200 {
                    let dx = pt.x - look_prev_x;
                    look_prev_x = pt.x;
                    look_moved += dx.abs();
                    turn_accum = (turn_accum + dx * 3).clamp(-60, 60);
                }
            }
        } else if was_pressed {
            let p = press_pt;
            let is_tap = (last_pt.x - press_pt.x).abs() < 16 && (last_pt.y - press_pt.y).abs() < 16;

            if is_tap && BTN_EXIT.contains(p) {
                if game.score > best {
                    best = game.score;
                }
                hw.save_best(SAVE_FILE, best);
                println!("CYBERMAZE: вихід у GameLauncher (best={})", best);
                hw.exit_to_launcher();
            } else if is_tap && BTN_DEMO.contains(p) {
                demo_mode = !demo_mode;
            } else if is_tap
                && press_pt.x >= STICK_SPLIT
                && press_pt.y < 200
                && look_moved < 12
                && !demo_mode
            {
                // короткий тап по правій половині — постріл
                game.shoot();
            }
            stick = (0, 0);
            firing = false;
            turn_accum = 0;
        }
        was_pressed = pressed;

        // Крок симуляції
        let input = if demo_mode {
            game.demo_input()
        } else {
            let mut fwd = 0i32;
            let mut strafe = 0i32;
            if stick.1 < -10 {
                fwd = (-stick.1 * 4).min(256);
            } else if stick.1 > 10 {
                fwd = ((-stick.1) * 4).max(-256);
            }
            if stick.0.abs() > 10 {
                strafe = (stick.0 * 3).clamp(-256, 256);
            }
            if sim_forward != 0 {
                fwd = sim_forward;
                sim_forward = 0;
            }
            let turn = if turn_accum != 0 {
                let t = turn_accum;
                turn_accum = 0;
                t
            } else {
                let t = sim_turn;
                sim_turn = 0;
                t
            };
            Input {
                forward: fwd,
                strafe,
                turn,
                fire: firing || sim_fire,
                run: false,
            }
        };
        let was_firing = input.fire;
        sim_fire = false;

        game.apply(input);
        if was_firing && game.ammo == 0 {
            // порожній магазин — просто нічого не стається
        }
        game.tick();

        // Рендер кадру
        let t_frame = Instant::now();
        render_scene(game);
        blit_scaled();
        draw_minimap(game);
        draw_hud(game, demo_mode, best);
        hw.present_all();
        let frame_ms = elapsed_ms(t_frame);

        frame += 1;
        if frame % 30 == 0 {
            let total = elapsed_ms(t_fps);
            let fps = if total > 0 { 30_000 / total } else { 0 };
            println!(
                "CYBERMAZE: fps={} frame={} ms hp={} ammo={} lvl={} score={}",
                fps, frame_ms, game.hp, game.ammo, game.level_no, game.score
            );
            t_fps = Instant::now();
            if game.score > best && !demo_mode {
                best = game.score;
                hw.save_best(SAVE_FILE, best);
            }
        }

        hw.delay.delay_millis(2);
    }
}
