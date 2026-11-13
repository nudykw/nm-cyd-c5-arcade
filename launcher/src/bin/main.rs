#![no_std]
#![no_main]

use core::fmt::Write;
use cyd_c5_common::{
    Board, Display, SCREEN_H, SCREEN_W,
    board::present_rect_on,
    boot::{
        has_sdpack_auto_flag, has_sdpack_in_flash, install_sdpack_to_sd, last_launched_name,
        launch_game_from_sd,
    },
    dump_frame_hex, put_px,
    sd::{LayoutKind, SdErr, SdGameFile, SdInfo},
    ui::{
        ACCENT_CYAN, ACCENT_GOLD, ACCENT_GREEN, ACCENT_RED, BG_DEEP, BTN_DEMO_OFF_BG,
        BTN_DEMO_ON_BG, BTN_DEMO_ON_BORDER, BTN_STD_BG, BTN_STD_BORDER, BtnRect, CARD_BG,
        CARD_SEL_BG, FontKind, LineBuf, PANEL_BG, TEXT_MUTED, TEXT_WHITE, draw_btn, fill,
        fill_rounded, in_box, rgb, stroke_rect, text, text_centered, u32_str,
    },
};
use embedded_graphics::pixelcolor::Rgb565;
use esp_println::println;

#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    println!("LAUNCHER PANIC: {:?}", info);
    loop {}
}

esp_bootloader_esp_idf::esp_app_desc!(
    "0.1.0",
    "cyd-launcher",
    "00:00:00",
    "2026-11-12",
    "5.5.6",
    64 * 1024,
    0,
    u16::MAX,
    0
);

const MAX_GAMES: usize = 24;
const VISIBLE_ROWS: usize = 5;

#[derive(Clone, Copy)]
struct KnownGameMeta {
    bin_name: &'static str,
    save_name: &'static str,
    title: &'static str,
    genre: &'static str,
    supports_demo: bool,
    order: u8,
    icon_id: u8,
}

const KNOWN_GAMES: &[KnownGameMeta] = &[
    KnownGameMeta {
        bin_name: "TTT.BIN",
        save_name: "TTT.SAV",
        title: "TIC-TAC-TOE",
        genre: "3x3 MINIMAX",
        supports_demo: false,
        order: 0,
        icon_id: 1,
    },
    KnownGameMeta {
        bin_name: "G2048.BIN",
        save_name: "G2048.SAV",
        title: "2048",
        genre: "TILE PUZZLE",
        supports_demo: true,
        order: 1,
        icon_id: 2,
    },
    KnownGameMeta {
        bin_name: "JEWELS.BIN",
        save_name: "JEWELS.SAV",
        title: "JEWELS",
        genre: "MATCH-3 GEMS",
        supports_demo: true,
        order: 2,
        icon_id: 3,
    },
    KnownGameMeta {
        bin_name: "ZUMA.BIN",
        save_name: "ZUMA.SAV",
        title: "ZUMA",
        genre: "TEMPLE MARBLE",
        supports_demo: true,
        order: 3,
        icon_id: 4,
    },
    KnownGameMeta {
        bin_name: "BIRDS.BIN",
        save_name: "BIRDS.SAV",
        title: "ANGRY BIRDS",
        genre: "SLING PHYSICS",
        supports_demo: true,
        order: 4,
        icon_id: 5,
    },
    KnownGameMeta {
        bin_name: "CHESS.BIN",
        save_name: "CHESS.SAV",
        title: "CHESS",
        genre: "CLASSIC 8x8",
        supports_demo: true,
        order: 5,
        icon_id: 6,
    },
    KnownGameMeta {
        bin_name: "CHECKERS.BIN",
        save_name: "CHECKERS.SAV",
        title: "CHECKERS",
        genre: "DRAUGHTS 64",
        supports_demo: true,
        order: 6,
        icon_id: 7,
    },
    KnownGameMeta {
        bin_name: "SUDOKU.BIN",
        save_name: "SUDOKU.SAV",
        title: "SUDOKU",
        genre: "9x9 LOGIC",
        supports_demo: true,
        order: 7,
        icon_id: 8,
    },
    KnownGameMeta {
        bin_name: "GOMOKU.BIN",
        save_name: "GOMOKU.SAV",
        title: "GOMOKU",
        genre: "5 IN A ROW 10x10",
        supports_demo: true,
        order: 8,
        icon_id: 9,
    },
    KnownGameMeta {
        bin_name: "CYBERMZ.BIN",
        save_name: "CYBERMZ.SAV",
        title: "CYBER MAZE",
        genre: "3D RAYCAST FPS",
        supports_demo: true,
        order: 9,
        icon_id: 10,
    },
    KnownGameMeta {
        bin_name: "SOKOBAN.BIN",
        save_name: "SOKOBAN.SAV",
        title: "3D SOKOBAN",
        genre: "CUBE PUZZLE 3D",
        supports_demo: true,
        order: 10,
        icon_id: 11,
    },
];

fn lookup_meta(file_name: &str) -> Option<&'static KnownGameMeta> {
    KNOWN_GAMES.iter().find(|m| m.bin_name == file_name)
}

#[derive(Clone, Copy)]
struct MenuEntry {
    sd_file: SdGameFile,
    best: u32,
    supports_demo: bool,
    order: u8,
    icon_id: u8,
}

impl MenuEntry {
    const fn empty() -> Self {
        Self {
            sd_file: SdGameFile::empty(),
            best: 0,
            supports_demo: false,
            order: 99,
            icon_id: 0,
        }
    }

    fn title(&self) -> &str {
        if let Some(m) = lookup_meta(self.sd_file.file_name_str()) {
            m.title
        } else {
            let p = self.sd_file.project_name_str();
            if !p.is_empty() {
                p
            } else {
                self.sd_file.file_name_str()
            }
        }
    }

    fn genre(&self) -> &str {
        if let Some(m) = lookup_meta(self.sd_file.file_name_str()) {
            m.genre
        } else {
            "SD CARD APP"
        }
    }
}

// Геометрія екрана GameLauncher (320×240)
const LIST_X: i32 = 6;
const LIST_Y: i32 = 31;
const LIST_W_FULL: u32 = 176;
const LIST_W_SCROLL: u32 = 162;
const ROW_H: i32 = 39;
const ROW_GAP: i32 = 2;
const LIST_H: i32 = (VISIBLE_ROWS as i32) * ROW_H + ((VISIBLE_ROWS as i32) - 1) * ROW_GAP; // 203

// Смуга прокрутки праворуч від списку ігор (видима, коли ігор > VISIBLE_ROWS)
const SB_X: i32 = 170;
const SB_Y: i32 = LIST_Y;
const SB_W: u32 = 14;
const SB_H: i32 = LIST_H;
const SB_CAP_H: i32 = 18;

const RIGHT_X: i32 = 188;
const RIGHT_W: u32 = 126;
const INFO_Y: i32 = 31;
const INFO_H: u32 = 102;

const BTN_PLAY: BtnRect = BtnRect::new(RIGHT_X, 138, RIGHT_W, 32);
const BTN_DEMO: BtnRect = BtnRect::new(RIGHT_X, 174, RIGHT_W, 28);
const BTN_SD_ACT: BtnRect = BtnRect::new(RIGHT_X, 206, RIGHT_W, 26);
const BTN_SCR_UP: BtnRect = BtnRect::new(RIGHT_X, 206, 38, 26);
const BTN_SCR_MID: BtnRect = BtnRect::new(RIGHT_X + 41, 206, 44, 26);
const BTN_SCR_DN: BtnRect = BtnRect::new(RIGHT_X + 88, 206, 38, 26);
const HDR_SD_BOX: BtnRect = BtnRect::new(190, 3, 124, 20);

fn draw_arrow_v(cx: i32, cy: i32, up: bool, col: Rgb565) {
    for row in 0..5i32 {
        let half = if up { row } else { 4 - row };
        fill(cx - half, cy - 2 + row, (half * 2 + 1) as u32, 1, col);
    }
}

fn draw_disc(cx: i32, cy: i32, r: i32, col: Rgb565) {
    let r2 = r * r;
    for dy in -r..=r {
        for dx in -r..=r {
            if dx * dx + dy * dy <= r2 {
                put_px(cx + dx, cy + dy, col);
            }
        }
    }
}

/// Піксель-арт міні-іконка 28×28 для кожного тайтла.
fn draw_game_icon(x: i32, y: i32, icon_id: u8) {
    match icon_id {
        1 => {
            // Tic-Tac-Toe: сітка 3×3 + хрестик і нулик
            fill_rounded(x, y, 28, 28, 4, rgb(22, 32, 52));
            stroke_rect(x, y, 28, 28, rgb(70, 96, 140));
            fill(x + 10, y + 3, 1, 22, rgb(84, 110, 156));
            fill(x + 18, y + 3, 1, 22, rgb(84, 110, 156));
            fill(x + 3, y + 10, 22, 1, rgb(84, 110, 156));
            fill(x + 3, y + 18, 22, 1, rgb(84, 110, 156));
            // X у лівому верхньому куті
            for i in 0..5 {
                put_px(x + 4 + i, y + 4 + i, ACCENT_CYAN);
                put_px(x + 8 - i, y + 4 + i, ACCENT_CYAN);
            }
            // O у центрі
            draw_disc(x + 14, y + 14, 3, ACCENT_GOLD);
            draw_disc(x + 14, y + 14, 1, rgb(22, 32, 52));
            // X у правому нижньому
            for i in 0..5 {
                put_px(x + 20 + i, y + 20 + i, ACCENT_CYAN);
                put_px(x + 24 - i, y + 20 + i, ACCENT_CYAN);
            }
        }
        2 => {
            // 2048: золота плитка
            fill_rounded(x, y, 28, 28, 5, rgb(236, 194, 46));
            stroke_rect(x, y, 28, 28, rgb(252, 228, 120));
            text_centered(x + 14, y + 4, "20", FontKind::Small, rgb(44, 34, 18));
            text_centered(x + 14, y + 14, "48", FontKind::Small, rgb(44, 34, 18));
        }
        3 => {
            // Jewels: гранований ромб
            fill_rounded(x, y, 28, 28, 4, rgb(28, 22, 52));
            stroke_rect(x, y, 28, 28, rgb(110, 82, 182));
            for dy in -9i32..=9 {
                let hw = 9 - dy.abs();
                let col = if dy < 0 {
                    rgb(96, 228, 252)
                } else {
                    rgb(38, 156, 224)
                };
                for dx in -hw..=hw {
                    put_px(x + 14 + dx, y + 14 + dy, col);
                }
            }
            put_px(x + 11, y + 10, TEXT_WHITE);
            put_px(x + 12, y + 10, TEXT_WHITE);
        }
        4 => {
            // Zuma: храмове коло + кольорові кулі на орбіті
            fill_rounded(x, y, 28, 28, 4, rgb(42, 34, 22));
            stroke_rect(x, y, 28, 28, rgb(146, 118, 62));
            draw_disc(x + 14, y + 14, 6, rgb(112, 128, 110));
            draw_disc(x + 12, y + 12, 1, ACCENT_GOLD);
            draw_disc(x + 16, y + 12, 1, ACCENT_GOLD);
            draw_disc(x + 6, y + 7, 3, ACCENT_RED);
            draw_disc(x + 21, y + 8, 3, ACCENT_GREEN);
            draw_disc(x + 21, y + 20, 3, ACCENT_CYAN);
            draw_disc(x + 7, y + 20, 3, ACCENT_GOLD);
        }
        5 => {
            // Angry Birds: червоний птах + зелена свинка
            fill_rounded(x, y, 28, 28, 4, rgb(44, 96, 156));
            fill(x + 1, y + 22, 26, 5, rgb(64, 146, 58));
            draw_disc(x + 9, y + 15, 6, rgb(224, 52, 52));
            fill(x + 13, y + 14, 4, 3, ACCENT_GOLD); // дзьоб
            put_px(x + 10, y + 13, TEXT_WHITE);
            draw_disc(x + 21, y + 17, 5, rgb(86, 198, 72));
            put_px(x + 20, y + 16, rgb(24, 48, 24));
            put_px(x + 23, y + 16, rgb(24, 48, 24));
        }
        6 => {
            // Chess: шахова дошка 4×4 + золота королівська тура/корона
            fill(x, y, 28, 28, rgb(138, 94, 58));
            for r in 0..4i32 {
                for c in 0..4i32 {
                    if (r + c) & 1 == 0 {
                        fill(x + 2 + c * 6, y + 2 + r * 6, 6, 6, rgb(224, 202, 166));
                    }
                }
            }
            stroke_rect(x, y, 28, 28, ACCENT_GOLD);
            // Фігура в центрі з темним контуром
            fill(x + 9, y + 20, 10, 4, rgb(20, 24, 34));
            fill(x + 10, y + 21, 8, 2, ACCENT_GOLD);
            fill(x + 11, y + 11, 6, 9, rgb(20, 24, 34));
            fill(x + 12, y + 12, 4, 8, TEXT_WHITE);
            draw_disc(x + 14, y + 9, 4, rgb(20, 24, 34));
            draw_disc(x + 14, y + 9, 3, TEXT_WHITE);
            fill(x + 13, y + 4, 2, 4, ACCENT_GOLD);
            fill(x + 12, y + 5, 4, 2, ACCENT_GOLD);
        }
        7 => {
            // Checkers: червонодеревна дошка + біла дамка з короною й темна шашка
            fill(x, y, 28, 28, rgb(104, 54, 42));
            for r in 0..4i32 {
                for c in 0..4i32 {
                    if (r + c) & 1 == 0 {
                        fill(x + 2 + c * 6, y + 2 + r * 6, 6, 6, rgb(236, 220, 190));
                    }
                }
            }
            stroke_rect(x, y, 28, 28, ACCENT_GOLD);
            // Чорна шашка зверху праворуч
            draw_disc(x + 19, y + 9, 5, TEXT_WHITE);
            draw_disc(x + 19, y + 9, 4, rgb(28, 34, 48));
            // Біла дамка знизу ліворуч із золотою короною
            draw_disc(x + 9, y + 19, 6, rgb(20, 22, 30));
            draw_disc(x + 9, y + 19, 5, rgb(248, 242, 226));
            fill(x + 7, y + 18, 5, 3, ACCENT_GOLD);
            put_px(x + 7, y + 17, ACCENT_GOLD);
            put_px(x + 9, y + 17, ACCENT_GOLD);
            put_px(x + 11, y + 17, ACCENT_GOLD);
        }
        8 => {
            // Sudoku: сітка 3×3 з великою золотою «9» і підсвіченою клітинкою
            fill_rounded(x, y, 28, 28, 4, rgb(24, 30, 46));
            stroke_rect(x, y, 28, 28, rgb(78, 96, 132));
            for i in 1..3i32 {
                fill(x + 3 + i * 7, y + 4, 1, 20, rgb(72, 92, 128));
                fill(x + 4, y + 3 + i * 7, 20, 1, rgb(72, 92, 128));
            }
            // Підсвічена клітинка
            fill(x + 11, y + 11, 6, 6, rgb(44, 96, 138));
            // Велика «9» у центрі
            fill(x + 12, y + 9, 4, 1, ACCENT_GOLD);
            fill(x + 16, y + 9, 1, 6, ACCENT_GOLD);
            fill(x + 12, y + 14, 4, 1, ACCENT_GOLD);
            fill(x + 12, y + 15, 1, 4, ACCENT_GOLD);
            fill(x + 12, y + 18, 4, 1, ACCENT_GOLD);
            // Цифри-підказки на периферії
            put_px(x + 7, y + 8, ACCENT_CYAN);
            put_px(x + 20, y + 8, ACCENT_CYAN);
            put_px(x + 7, y + 20, ACCENT_CYAN);
            put_px(x + 20, y + 20, ACCENT_CYAN);
            put_px(x + 8, y + 8, ACCENT_CYAN);
        }
        9 => {
            // Gomoku: дерев'яна дошка з чорним і білим каменями
            fill_rounded(x, y, 28, 28, 4, rgb(206, 166, 114));
            stroke_rect(x, y, 28, 28, rgb(120, 82, 46));
            for i in 1..3i32 {
                fill(x + 3 + i * 7, y + 3, 1, 22, rgb(96, 66, 40));
                fill(x + 3, y + 3 + i * 7, 22, 1, rgb(96, 66, 40));
            }
            draw_disc(x + 10, y + 11, 5, rgb(6, 6, 8));
            draw_disc(x + 10, y + 11, 3, rgb(36, 38, 46));
            draw_disc(x + 19, y + 20, 5, rgb(150, 144, 134));
            draw_disc(x + 19, y + 20, 4, rgb(240, 238, 230));
            put_px(x + 18, y + 19, TEXT_WHITE);
            put_px(x + 10, y + 10, rgb(96, 102, 118));
        }
        10 => {
            // Cyber Maze: перспективний коридор + дрон у прицілі
            fill_rounded(x, y, 28, 28, 4, rgb(10, 16, 26));
            stroke_rect(x, y, 28, 28, ACCENT_CYAN);
            // стеля і підлога
            fill(x + 2, y + 2, 24, 9, rgb(18, 30, 46));
            fill(x + 2, y + 20, 24, 6, rgb(26, 26, 34));
            // стіни-тунель
            for i in 0..5i32 {
                let inset = i * 3;
                let col = if i % 2 == 0 {
                    rgb(48, 150, 172)
                } else {
                    rgb(28, 92, 116)
                };
                fill(x + 2 + inset, y + 11, 2, 9, col);
                fill(x + 26 - inset, y + 11, 2, 9, col);
            }
            // дрон у центрі
            fill(x + 12, y + 12, 5, 6, ACCENT_RED);
            fill(x + 13, y + 13, 3, 2, TEXT_WHITE);
            // приціл
            fill(x + 14, y + 8, 1, 2, ACCENT_CYAN);
            fill(x + 14, y + 20, 1, 2, ACCENT_CYAN);
            fill(x + 9, y + 14, 2, 1, ACCENT_CYAN);
            fill(x + 18, y + 14, 2, 1, ACCENT_CYAN);
        }
        11 => {
            // 3D Sokoban: ізометричний ящик на цілі + гравець
            fill_rounded(x, y, 28, 28, 4, rgb(12, 16, 26));
            stroke_rect(x, y, 28, 28, ACCENT_GOLD);
            // Підлога-ромб
            for dy in -6i32..=6 {
                let hw = 12 - dy.abs() * 2;
                fill(x + 14 - hw, y + 18 + dy, (hw * 2) as u32, 1, rgb(38, 46, 62));
            }
            // Ящик на цілі (ізометрія)
            fill(x + 10, y + 10, 9, 6, rgb(112, 226, 132));
            fill(x + 8, y + 13, 9, 5, rgb(66, 168, 96));
            fill(x + 15, y + 13, 9, 5, rgb(44, 122, 72));
            fill(x + 13, y + 15, 4, 4, rgb(236, 194, 74));
            // Гравець-кубик
            fill(x + 4, y + 6, 4, 3, rgb(132, 234, 252));
            fill(x + 3, y + 8, 4, 3, rgb(64, 176, 206));
        }
        _ => {
            // Картридж для будь-якого стороннього .BIN
            fill_rounded(x, y, 28, 28, 4, rgb(52, 62, 82));
            stroke_rect(x, y, 28, 28, ACCENT_CYAN);
            fill(x + 6, y + 5, 16, 10, rgb(28, 36, 52));
            text_centered(x + 14, y + 17, "BIN", FontKind::Small, ACCENT_GOLD);
        }
    }
}

struct LauncherState {
    entries: [MenuEntry; MAX_GAMES],
    count: usize,
    selected: usize,
    scroll: usize,
    first_boot_sync: bool,
    sd_status: Result<SdInfo, SdErr>,
    has_flash_pack: bool,
    status_msg: LineBuf<32>,
    /// Скільки тіків лишилось на підтвердження форматування картки (0 = не активоване).
    format_armed: u16,
}

impl LauncherState {
    fn new() -> Self {
        Self {
            entries: [MenuEntry::empty(); MAX_GAMES],
            count: 0,
            selected: 0,
            scroll: 0,
            first_boot_sync: true,
            sd_status: Err(SdErr::NoCard),
            has_flash_pack: false,
            status_msg: LineBuf::new(),
            format_armed: 0,
        }
    }

    #[inline]
    fn max_scroll(&self) -> usize {
        self.count.saturating_sub(VISIBLE_ROWS)
    }

    fn clamp_scroll(&mut self) {
        let ms = self.max_scroll();
        if self.scroll > ms {
            self.scroll = ms;
        }
    }

    fn ensure_selected_visible(&mut self) {
        if self.count == 0 {
            self.scroll = 0;
            return;
        }
        if self.selected < self.scroll {
            self.scroll = self.selected;
        } else if self.selected >= self.scroll + VISIBLE_ROWS {
            self.scroll = self.selected + 1 - VISIBLE_ROWS;
        }
        self.clamp_scroll();
    }

    fn scroll_by(&mut self, delta: i32) -> bool {
        let old = self.scroll;
        let ms = self.max_scroll() as i32;
        let next = ((self.scroll as i32) + delta).clamp(0, ms) as usize;
        self.scroll = next;
        self.scroll != old
    }

    fn refresh(&mut self, board: &mut Board) {
        self.has_flash_pack = has_sdpack_in_flash(&mut board.flash);
        self.count = 0;
        self.status_msg.clear();

        let mut cached_buf = [0u8; 16];
        let cached_len = if self.first_boot_sync {
            last_launched_name(&mut board.flash, &mut cached_buf).map_or(0, |s| s.len())
        } else {
            0
        };
        self.first_boot_sync = false;

        match board.sd.mount() {
            Ok(info) => {
                self.sd_status = Ok(info);
                let _ = board.sd.ensure_dirs();
                let mut raw = [SdGameFile::empty(); MAX_GAMES];
                match board.sd.scan_games(&mut raw) {
                    Ok(n) => {
                        for item in raw.iter().take(n) {
                            let fname = item.file_name_str();
                            let (supports_demo, order, icon_id, best) =
                                if let Some(m) = lookup_meta(fname) {
                                    let b = board.sd.load_best(m.save_name).unwrap_or(0);
                                    (m.supports_demo, m.order, m.icon_id, b)
                                } else {
                                    (false, 50, 0, 0)
                                };
                            self.entries[self.count] = MenuEntry {
                                sd_file: *item,
                                best,
                                supports_demo,
                                order,
                                icon_id,
                            };
                            self.count += 1;
                        }
                        // Сортуємо канонічні ігри у зручному порядку, а сторонні — за іменем.
                        for i in 0..self.count {
                            for j in i + 1..self.count {
                                let swap = if self.entries[j].order != self.entries[i].order {
                                    self.entries[j].order < self.entries[i].order
                                } else {
                                    self.entries[j].sd_file.file_name
                                        < self.entries[i].sd_file.file_name
                                };
                                if swap {
                                    self.entries.swap(i, j);
                                }
                            }
                        }
                        if cached_len > 0
                            && let Ok(cname) = core::str::from_utf8(&cached_buf[..cached_len])
                            && let Some(idx) = (0..self.count)
                                .find(|&i| self.entries[i].sd_file.file_name_str() == cname)
                        {
                            self.selected = idx;
                        }
                        if self.selected >= self.count {
                            self.selected = 0;
                        }
                        self.ensure_selected_visible();
                        println!(
                            "LAUNCHER: SD змонтовано ({} MB, {:?}), знайдено ігор: {}",
                            info.size_mb, info.layout, self.count
                        );
                        for idx in 0..self.count {
                            let e = &self.entries[idx];
                            println!(
                                "  [{}] {} ({} B, app='{}', best={})",
                                idx,
                                e.sd_file.file_name_str(),
                                e.sd_file.size,
                                e.sd_file.project_name_str(),
                                e.best
                            );
                        }
                    }
                    Err(e) => {
                        let _ = write!(self.status_msg, "SCAN ERR: {:?}", e);
                        println!("LAUNCHER: помилка сканування /GAMES: {:?}", e);
                    }
                }
            }
            Err(e) => {
                self.sd_status = Err(e);
                println!("LAUNCHER: SD недоступна: {:?}", e);
            }
        }
    }
}

fn draw_modal_progress(display: &mut Display, title_s: &str, sub_s: &str, done: u32, total: u32) {
    let mx = 36;
    let my = 72;
    let mw = 248u32;
    let mh = 96u32;
    fill(mx, my, mw, mh, rgb(20, 28, 46));
    stroke_rect(mx, my, mw, mh, ACCENT_CYAN);
    stroke_rect(mx + 2, my + 2, mw - 4, mh - 4, rgb(48, 68, 104));
    text_centered(160, my + 12, title_s, FontKind::Bold, ACCENT_GOLD);
    text_centered(160, my + 36, sub_s, FontKind::Small, TEXT_WHITE);

    let bx = mx + 18;
    let by = my + 58;
    let bw = mw - 36;
    let bh = 16u32;
    fill(bx, by, bw, bh, rgb(12, 16, 26));
    stroke_rect(bx, by, bw, bh, BTN_STD_BORDER);
    let fill_w = if total > 0 {
        (((done as u64) * ((bw - 4) as u64)) / (total as u64)).min((bw - 4) as u64) as u32
    } else {
        0
    };
    if fill_w > 0 {
        fill(bx + 2, by + 2, fill_w, bh - 4, ACCENT_GREEN);
    }
    present_rect_on(display, mx as u16, my as u16, mw as u16, mh as u16);
}

fn render_launcher(st: &LauncherState) {
    fill(0, 0, SCREEN_W as u32, SCREEN_H as u32, BG_DEEP);

    // Верхня панель
    fill(0, 0, SCREEN_W as u32, 26, PANEL_BG);
    fill(0, 26, SCREEN_W as u32, 1, rgb(64, 88, 128));
    text(8, 4, "CYD-C5 ARCADE", FontKind::Bold, ACCENT_GOLD);

    // Індикатор SD-картки у шапці
    let mut sd_lbl = LineBuf::<22>::new();
    let (sd_bg, sd_fg, sd_brd) = match st.sd_status {
        Ok(info) => {
            let lay = match info.layout {
                LayoutKind::Mbr => "MBR",
                LayoutKind::Gpt => "GPT",
                LayoutKind::Superfloppy => "FAT",
            };
            if info.size_mb >= 1024 {
                let _ = write!(sd_lbl, "SD {}G {}", info.size_mb / 1024, lay);
            } else {
                let _ = write!(sd_lbl, "SD {}M {}", info.size_mb, lay);
            }
            (rgb(22, 68, 46), ACCENT_GREEN, rgb(58, 160, 96))
        }
        Err(SdErr::NoCard) => {
            let _ = write!(sd_lbl, "NO SD [TAP]");
            (rgb(78, 30, 36), ACCENT_RED, rgb(176, 68, 76))
        }
        Err(SdErr::ExFat) => {
            let _ = write!(sd_lbl, "SD EXFAT! NEED FAT32");
            (rgb(88, 48, 18), ACCENT_GOLD, ACCENT_GOLD)
        }
        Err(_) => {
            let _ = write!(sd_lbl, "SD FS ERR [TAP]");
            (rgb(78, 36, 26), ACCENT_GOLD, ACCENT_RED)
        }
    };
    draw_btn(
        HDR_SD_BOX,
        sd_lbl.as_str(),
        FontKind::Small,
        sd_bg,
        sd_fg,
        sd_brd,
    );

    // Ліва колонка: список ігор із `/GAMES/*.BIN`
    if st.count == 0 {
        fill(LIST_X, LIST_Y, LIST_W_FULL, 201, CARD_BG);
        stroke_rect(LIST_X, LIST_Y, LIST_W_FULL, 201, BTN_STD_BORDER);
        match st.sd_status {
            Ok(_) => {
                text_centered(LIST_X + 88, 74, "NO GAMES IN", FontKind::Bold, ACCENT_GOLD);
                text_centered(LIST_X + 88, 96, "/GAMES/*.BIN", FontKind::Bold, ACCENT_CYAN);
                if st.has_flash_pack {
                    text_centered(
                        LIST_X + 88,
                        130,
                        "TAP 'INSTALL SD'",
                        FontKind::Small,
                        ACCENT_GREEN,
                    );
                    text_centered(
                        LIST_X + 88,
                        144,
                        "TO COPY ALL 11 GAMES",
                        FontKind::Small,
                        TEXT_MUTED,
                    );
                } else {
                    text_centered(
                        LIST_X + 88,
                        134,
                        "COPY .BIN FILES TO",
                        FontKind::Small,
                        TEXT_MUTED,
                    );
                    text_centered(
                        LIST_X + 88,
                        148,
                        "SD:/GAMES/",
                        FontKind::Small,
                        TEXT_WHITE,
                    );
                }
            }
            Err(SdErr::ExFat) => {
                text_centered(LIST_X + 88, 76, "EXFAT CARD", FontKind::Bold, ACCENT_GOLD);
                text_centered(
                    LIST_X + 88,
                    104,
                    "FORMAT SD AS FAT32",
                    FontKind::Small,
                    TEXT_WHITE,
                );
                text_centered(
                    LIST_X + 88,
                    120,
                    "THEN TAP RESCAN",
                    FontKind::Small,
                    TEXT_MUTED,
                );
            }
            Err(_) => {
                text_centered(LIST_X + 88, 82, "INSERT MICROSD", FontKind::Bold, ACCENT_RED);
                text_centered(
                    LIST_X + 88,
                    110,
                    "INTO SLOT (IO10)",
                    FontKind::Small,
                    TEXT_MUTED,
                );
                text_centered(
                    LIST_X + 88,
                    126,
                    "AND TAP RESCAN",
                    FontKind::Small,
                    TEXT_WHITE,
                );
            }
        }
    } else {
        let has_scroll = st.count > VISIBLE_ROWS;
        let row_w = if has_scroll {
            LIST_W_SCROLL
        } else {
            LIST_W_FULL
        };
        let start = st.scroll.min(st.max_scroll());
        let end = (start + VISIBLE_ROWS).min(st.count);

        for slot in 0..(end - start) {
            let idx = start + slot;
            let e = &st.entries[idx];
            let ry = LIST_Y + (slot as i32) * (ROW_H + ROW_GAP);
            let is_sel = idx == st.selected;
            let bg = if is_sel { CARD_SEL_BG } else { CARD_BG };
            let brd = if is_sel { ACCENT_CYAN } else { rgb(52, 66, 92) };
            fill(LIST_X, ry, row_w, ROW_H as u32, bg);
            stroke_rect(LIST_X, ry, row_w, ROW_H as u32, brd);
            if is_sel {
                fill(LIST_X + 1, ry + 1, 3, (ROW_H - 2) as u32, ACCENT_GOLD);
            }

            draw_game_icon(LIST_X + 6, ry + 5, e.icon_id);

            let tx = LIST_X + 40;
            text(
                tx,
                ry + 6,
                e.title(),
                FontKind::Small,
                if is_sel { TEXT_WHITE } else { rgb(220, 228, 242) },
            );

            let mut sub = LineBuf::<24>::new();
            if e.best > 0 {
                let _ = write!(sub, "BEST: {}", e.best);
                text(tx, ry + 21, sub.as_str(), FontKind::Small, ACCENT_GOLD);
            } else {
                let _ = write!(sub, "{} KB", e.sd_file.size.div_ceil(1024));
                text(tx, ry + 21, sub.as_str(), FontKind::Small, TEXT_MUTED);
            }

            if e.supports_demo {
                let badge_x = LIST_X + (row_w as i32) - 34;
                fill(badge_x, ry + 20, 30, 12, rgb(26, 78, 58));
                text(badge_x + 3, ry + 21, "DEMO", FontKind::Small, ACCENT_GREEN);
            }
        }

        // Інтерактивна смуга прокрутки поруч зі списком
        if has_scroll {
            fill(SB_X, SB_Y, SB_W, SB_H as u32, PANEL_BG);
            stroke_rect(SB_X, SB_Y, SB_W, SB_H as u32, BTN_STD_BORDER);

            // Верхня та нижня стрілки на смузі прокрутки
            fill(SB_X + 1, SB_Y + 1, SB_W - 2, (SB_CAP_H - 1) as u32, BTN_STD_BG);
            fill(SB_X + 1, SB_Y + SB_CAP_H, SB_W - 2, 1, BTN_STD_BORDER);
            draw_arrow_v(
                SB_X + (SB_W as i32) / 2,
                SB_Y + SB_CAP_H / 2,
                true,
                if start > 0 { ACCENT_CYAN } else { TEXT_MUTED },
            );

            let bot_y = SB_Y + SB_H - SB_CAP_H;
            fill(SB_X + 1, bot_y, SB_W - 2, (SB_CAP_H - 1) as u32, BTN_STD_BG);
            fill(SB_X + 1, bot_y - 1, SB_W - 2, 1, BTN_STD_BORDER);
            draw_arrow_v(
                SB_X + (SB_W as i32) / 2,
                bot_y + SB_CAP_H / 2,
                false,
                if start < st.max_scroll() {
                    ACCENT_CYAN
                } else {
                    TEXT_MUTED
                },
            );

            // Повзунок (thumb) пропорційно кількості ігор і поточному зсуву
            let track_y = SB_Y + SB_CAP_H + 2;
            let track_h = SB_H - 2 * SB_CAP_H - 4;
            let thumb_h = (((track_h as usize) * VISIBLE_ROWS) / st.count).max(22) as i32;
            let max_s = st.max_scroll().max(1);
            let thumb_y =
                track_y + (((track_h - thumb_h) as usize * start) / max_s) as i32;

            fill(SB_X + 2, thumb_y, SB_W - 4, thumb_h as u32, ACCENT_CYAN);
            stroke_rect(SB_X + 2, thumb_y, SB_W - 4, thumb_h as u32, TEXT_WHITE);
            let mid_ty = thumb_y + thumb_h / 2;
            for dy in [-3i32, 0, 3] {
                fill(SB_X + 4, mid_ty + dy, SB_W - 8, 1, BG_DEEP);
            }
        }
    }

    // Права колонка: картка вибраної гри + кнопки PLAY / DEMO / SD
    fill(RIGHT_X, INFO_Y, RIGHT_W, INFO_H, PANEL_BG);
    stroke_rect(RIGHT_X, INFO_Y, RIGHT_W, INFO_H, BTN_STD_BORDER);

    if st.count > 0 {
        let sel = &st.entries[st.selected];
        text_centered(
            RIGHT_X + (RIGHT_W as i32) / 2,
            INFO_Y + 7,
            sel.title(),
            FontKind::Small,
            ACCENT_GOLD,
        );
        text_centered(
            RIGHT_X + (RIGHT_W as i32) / 2,
            INFO_Y + 20,
            sel.genre(),
            FontKind::Small,
            ACCENT_CYAN,
        );
        fill(RIGHT_X + 8, INFO_Y + 34, RIGHT_W - 16, 1, rgb(52, 66, 92));

        let mut ln1 = LineBuf::<22>::new();
        let _ = write!(ln1, "FILE: {}", sel.sd_file.file_name_str());
        text(RIGHT_X + 6, INFO_Y + 40, ln1.as_str(), FontKind::Small, TEXT_MUTED);

        let mut ln2 = LineBuf::<22>::new();
        let _ = write!(ln2, "SIZE: {} KB", sel.sd_file.size.div_ceil(1024));
        text(RIGHT_X + 6, INFO_Y + 54, ln2.as_str(), FontKind::Small, TEXT_MUTED);

        let mut b_buf = [0u8; 10];
        text(RIGHT_X + 6, INFO_Y + 68, "BEST:", FontKind::Small, TEXT_MUTED);
        text(
            RIGHT_X + 42,
            INFO_Y + 68,
            u32_str(sel.best, &mut b_buf),
            FontKind::Small,
            ACCENT_GOLD,
        );

        let mode_s = if sel.supports_demo {
            "MODE: PLAY + DEMO"
        } else {
            "MODE: 1P AI / 2P"
        };
        text(RIGHT_X + 6, INFO_Y + 83, mode_s, FontKind::Small, ACCENT_GREEN);

        // Кнопка PLAY
        draw_btn(
            BTN_PLAY,
            "PLAY",
            FontKind::Bold,
            rgb(34, 146, 78),
            TEXT_WHITE,
            rgb(106, 240, 152),
        );

        // Кнопка DEMO (активна лише якщо гра підтримує демо-режим)
        if sel.supports_demo {
            draw_btn(
                BTN_DEMO,
                "DEMO",
                FontKind::Bold,
                BTN_DEMO_ON_BG,
                TEXT_WHITE,
                BTN_DEMO_ON_BORDER,
            );
        } else {
            draw_btn(
                BTN_DEMO,
                "NO DEMO",
                FontKind::Small,
                BTN_DEMO_OFF_BG,
                TEXT_MUTED,
                rgb(62, 74, 94),
            );
        }
    } else {
        text_centered(
            RIGHT_X + (RIGHT_W as i32) / 2,
            INFO_Y + 36,
            "SELECT A GAME",
            FontKind::Small,
            TEXT_MUTED,
        );
        draw_btn(
            BTN_PLAY,
            "PLAY",
            FontKind::Bold,
            rgb(36, 44, 58),
            TEXT_MUTED,
            rgb(62, 74, 94),
        );
        draw_btn(
            BTN_DEMO,
            "DEMO",
            FontKind::Bold,
            rgb(36, 44, 58),
            TEXT_MUTED,
            rgb(62, 74, 94),
        );
    }

    // Нижня сервісна кнопка або кнопки прокрутки списку (`▲` / `1-5/6` / `▼`)
    if st.sd_status.is_ok() && st.count == 0 && st.has_flash_pack {
        draw_btn(
            BTN_SD_ACT,
            "INSTALL SD",
            FontKind::Small,
            rgb(138, 96, 20),
            TEXT_WHITE,
            ACCENT_GOLD,
        );
    } else if st.count > VISIBLE_ROWS {
        let can_up = st.scroll > 0 || st.selected > 0;
        let can_dn = st.scroll < st.max_scroll() || st.selected + 1 < st.count;

        draw_btn(
            BTN_SCR_UP,
            "",
            FontKind::Small,
            BTN_STD_BG,
            TEXT_WHITE,
            if can_up { ACCENT_CYAN } else { BTN_STD_BORDER },
        );
        draw_arrow_v(
            BTN_SCR_UP.x + (BTN_SCR_UP.w as i32) / 2,
            BTN_SCR_UP.y + (BTN_SCR_UP.h as i32) / 2,
            true,
            if can_up { TEXT_WHITE } else { TEXT_MUTED },
        );

        let first = st.scroll + 1;
        let last = (st.scroll + VISIBLE_ROWS).min(st.count);
        let mut pos_lbl = LineBuf::<12>::new();
        let _ = write!(pos_lbl, "{}-{}/{}", first, last, st.count);
        draw_btn(
            BTN_SCR_MID,
            pos_lbl.as_str(),
            FontKind::Small,
            CARD_BG,
            ACCENT_GOLD,
            BTN_STD_BORDER,
        );

        draw_btn(
            BTN_SCR_DN,
            "",
            FontKind::Small,
            BTN_STD_BG,
            TEXT_WHITE,
            if can_dn { ACCENT_CYAN } else { BTN_STD_BORDER },
        );
        draw_arrow_v(
            BTN_SCR_DN.x + (BTN_SCR_DN.w as i32) / 2,
            BTN_SCR_DN.y + (BTN_SCR_DN.h as i32) / 2,
            false,
            if can_dn { TEXT_WHITE } else { TEXT_MUTED },
        );
    } else if st.sd_status.is_err() {
        // Картка є, але FAT не читається — пропонуємо форматування (з підтвердженням)
        let (label, bg, brd) = if st.format_armed > 0 {
            ("SURE? TAP", rgb(150, 40, 40), ACCENT_RED)
        } else {
            ("FORMAT SD", rgb(120, 60, 20), ACCENT_GOLD)
        };
        draw_btn(BTN_SD_ACT, label, FontKind::Small, bg, TEXT_WHITE, brd);
    } else {
        draw_btn(
            BTN_SD_ACT,
            "RESCAN SD",
            FontKind::Small,
            BTN_STD_BG,
            TEXT_WHITE,
            BTN_STD_BORDER,
        );
    }
}

/// Форматує картку у FAT32 і, якщо у флеші є пакет `sdseed`, одразу встановлює всі ігри.
fn do_format_sd(board: &mut Board, st: &mut LauncherState) {
    println!("LAUNCHER: форматування SD-картки у FAT32...");
    draw_modal_progress(&mut board.display, "FORMAT SD", "ERASING...", 0, 1);

    let display = &mut board.display;
    let sd = &mut board.sd;
    let res = sd.format_fat32(|done, total| {
        draw_modal_progress(display, "FORMAT SD", "WRITING FAT32...", done, total);
    });
    match res {
        Ok(info) => println!(
            "LAUNCHER: картку відформатовано ({} MB, {:?})",
            info.size_mb, info.layout
        ),
        Err(e) => {
            println!("LAUNCHER: форматування не вдалося: {:?}", e);
            draw_modal_progress(display, "FORMAT FAILED", "CHECK CARD", 0, 1);
            board.delay.delay_millis(1200);
            st.refresh(board);
            return;
        }
    }

    st.format_armed = 0;
    if has_sdpack_in_flash(&mut board.flash) {
        try_run_sdpack_install(board, st);
    } else {
        st.refresh(board);
    }
}

fn try_run_sdpack_install(board: &mut Board, st: &mut LauncherState) {
    println!("LAUNCHER: розпаковка пакета ігор із sdseed на SD-картку...");
    draw_modal_progress(&mut board.display, "INSTALLING TO SD", "MOUNTING...", 0, 1);
    if let Err(e) = board.sd.mount() {
        println!("LAUNCHER: не вдалося змонтувати SD для інсталяції: {:?}", e);
        return;
    }
    let display = &mut board.display;
    let flash = &mut board.flash;
    let sd = &mut board.sd;
    let res = install_sdpack_to_sd(flash, sd, |idx, total, fname| {
        println!("  -> запис /GAMES/{} ({}/{})", fname, idx, total);
        draw_modal_progress(display, "INSTALLING TO SD", fname, idx as u32, total as u32);
    });
    match res {
        Ok(n) => println!("LAUNCHER: успішно встановлено {} ігор у SD:/GAMES/", n),
        Err(e) => println!("LAUNCHER: помилка інсталяції на SD: {:?}", e),
    }
    st.refresh(board);
}

fn do_launch(board: &mut Board, entry: &MenuEntry, start_demo: bool) {
    let fname = entry.sd_file.file_name_str();
    println!(
        "LAUNCHER: запуск /GAMES/{} (size={} B, demo={})",
        fname, entry.sd_file.size, start_demo
    );
    let title = if start_demo {
        "STARTING DEMO"
    } else {
        "LOADING GAME"
    };
    let display = &mut board.display;
    let flash = &mut board.flash;
    let sd = &mut board.sd;
    draw_modal_progress(display, title, fname, 0, entry.sd_file.size);
    let mut last_pct = 0u32;
    let Err(e) = launch_game_from_sd(
        flash,
        sd,
        &entry.sd_file,
        start_demo,
        |done, total| {
            let pct = if total > 0 { (done * 100) / total } else { 100 };
            if done == total || pct >= last_pct + 15 {
                last_pct = pct;
                draw_modal_progress(display, title, fname, done, total);
            }
        },
    );
    println!("LAUNCHER: помилка запуску {:?}: {:?}", fname, e);
    draw_modal_progress(display, "LAUNCH ERROR", fname, 0, 1);
    board.delay.delay_millis(1200);
}

#[esp_hal::main]
fn main() -> ! {
    let mut board = Board::init();
    println!("=== NM-CYD-C5 GAME LAUNCHER v0.1.0 ===");

    let mut st = LauncherState::new();
    if has_sdpack_auto_flag(&mut board.flash) {
        try_run_sdpack_install(&mut board, &mut st);
    } else {
        st.refresh(&mut board);
    }

    render_launcher(&st);
    board.present_all();

    if cfg!(feature = "framedump") {
        dump_frame_hex();
    }

    let mut was_pressed = false;
    let mut press_pt = embedded_graphics::prelude::Point::zero();
    let mut last_pt = embedded_graphics::prelude::Point::zero();
    let mut drag_anchor_y = 0i32;
    let mut list_dragged = false;
    let mut sb_dragging = false;
    let mut cmd_buf = [0u8; 32];
    let mut cmd_len = 0usize;

    loop {
        // підтвердження форматування «живе» ~5 секунд
        if st.format_armed > 0 {
            st.format_armed -= 1;
            if st.format_armed == 0 {
                render_launcher(&st);
                board.present_all();
            }
        }

        while let Some(ch) = board.poll_uart_byte() {
            if ch == b'\n' || ch == b'\r' {
                if cmd_len > 0 {
                    if let Ok(line) = core::str::from_utf8(&cmd_buf[..cmd_len]) {
                        let line = line.trim();
                        if line == "FORMAT" {
                            do_format_sd(&mut board, &mut st);
                            render_launcher(&st);
                            board.present_all();
                        } else if line == "RESCAN" {
                            st.refresh(&mut board);
                            render_launcher(&st);
                            board.present_all();
                        } else if line == "CLEARSAVES" {
                            for m in KNOWN_GAMES {
                                let _ = board.sd.save_best(m.save_name, 0);
                            }
                            st.refresh(&mut board);
                            render_launcher(&st);
                            board.present_all();
                        } else if line == "DOWN" || line == "PAGE" {
                            if st.scroll < st.max_scroll() {
                                st.scroll += 1;
                                st.selected = st.scroll + VISIBLE_ROWS - 1;
                            } else if st.selected + 1 < st.count {
                                st.selected += 1;
                            } else {
                                st.scroll = 0;
                                st.selected = 0;
                            }
                            render_launcher(&st);
                            board.present_all();
                        } else if line == "UP" {
                            if st.scroll > 0 {
                                st.scroll -= 1;
                                if st.selected >= st.scroll + VISIBLE_ROWS {
                                    st.selected = st.scroll + VISIBLE_ROWS - 1;
                                }
                            } else if st.selected > 0 {
                                st.selected -= 1;
                            }
                            render_launcher(&st);
                            board.present_all();
                        } else if let Some(arg) = line.strip_prefix("PLAY ") {
                            let target = arg.trim();
                            if let Some(idx) = (0..st.count)
                                .find(|&i| st.entries[i].sd_file.file_name_str() == target)
                            {
                                st.selected = idx;
                                st.ensure_selected_visible();
                                let chosen = st.entries[idx];
                                do_launch(&mut board, &chosen, false);
                            }
                        } else if let Some(arg) = line.strip_prefix("DEMO ") {
                            let target = arg.trim();
                            if let Some(idx) = (0..st.count)
                                .find(|&i| st.entries[i].sd_file.file_name_str() == target)
                            {
                                st.selected = idx;
                                st.ensure_selected_visible();
                                let chosen = st.entries[idx];
                                if chosen.supports_demo {
                                    do_launch(&mut board, &chosen, true);
                                } else {
                                    println!("LAUNCHER: {} не підтримує DEMO", target);
                                }
                            }
                        }
                    }
                    cmd_len = 0;
                }
            } else if cmd_len < cmd_buf.len() {
                cmd_buf[cmd_len] = ch.to_ascii_uppercase();
                cmd_len += 1;
            }
        }

        // Надійне зчитування тачу (`pressed -> point -> pressed`) без хибних координат на відриві пальця
        let mut valid_pt = None;
        if board.touch.pressed() {
            let pt = board.touch.point();
            if board.touch.pressed() {
                valid_pt = Some(pt);
            }
        }
        let now_pressed = valid_pt.is_some();

        if let Some(pt) = valid_pt {
            last_pt = pt;
            if !was_pressed {
                press_pt = pt;
                drag_anchor_y = pt.y;
                list_dragged = false;
                sb_dragging = st.count > VISIBLE_ROWS
                    && in_box(pt, SB_X - 3, SB_Y, (SB_W as i32) + 8, SB_H);

                if sb_dragging {
                    let top_cap = SB_Y + SB_CAP_H + 2;
                    let bot_cap = SB_Y + SB_H - SB_CAP_H - 2;
                    if pt.y > top_cap && pt.y < bot_cap {
                        let span = (bot_cap - top_cap).max(1);
                        let rel = (pt.y - top_cap).clamp(0, span);
                        let target = ((rel as usize) * st.max_scroll() + (span as usize) / 2)
                            / (span as usize);
                        if target != st.scroll {
                            st.scroll = target;
                            if st.selected < st.scroll {
                                st.selected = st.scroll;
                            } else if st.selected >= st.scroll + VISIBLE_ROWS {
                                st.selected = st.scroll + VISIBLE_ROWS - 1;
                            }
                            render_launcher(&st);
                            board.present_all();
                        }
                    }
                }
            } else if sb_dragging {
                // Живе перетягування повзунка смуги прокрутки
                let top_cap = SB_Y + SB_CAP_H + 2;
                let bot_cap = SB_Y + SB_H - SB_CAP_H - 2;
                if pt.y >= top_cap - 6 && pt.y <= bot_cap + 6 {
                    let span = (bot_cap - top_cap).max(1);
                    let rel = (pt.y - top_cap).clamp(0, span);
                    let target =
                        ((rel as usize) * st.max_scroll() + (span as usize) / 2) / (span as usize);
                    if target != st.scroll {
                        st.scroll = target;
                        if st.selected < st.scroll {
                            st.selected = st.scroll;
                        } else if st.selected >= st.scroll + VISIBLE_ROWS {
                            st.selected = st.scroll + VISIBLE_ROWS - 1;
                        }
                        render_launcher(&st);
                        board.present_all();
                    }
                }
            } else if st.count > VISIBLE_ROWS
                && in_box(press_pt, LIST_X, LIST_Y, LIST_W_SCROLL as i32, LIST_H)
            {
                // Жива прокрутка списку свайпом/перетягуванням пальця вгору або вниз
                let dy = pt.y - drag_anchor_y;
                if dy <= -26 {
                    drag_anchor_y = pt.y;
                    list_dragged = true;
                    if st.scroll_by(1) {
                        if st.selected < st.scroll {
                            st.selected = st.scroll;
                        }
                        render_launcher(&st);
                        board.present_all();
                    }
                } else if dy >= 26 {
                    drag_anchor_y = pt.y;
                    list_dragged = true;
                    if st.scroll_by(-1) {
                        if st.selected >= st.scroll + VISIBLE_ROWS {
                            st.selected = st.scroll + VISIBLE_ROWS - 1;
                        }
                        render_launcher(&st);
                        board.present_all();
                    }
                }
            }
        } else if was_pressed {
            let p = press_pt;
            let total_dy = last_pt.y - press_pt.y;
            let is_tap = !list_dragged
                && (last_pt.x - press_pt.x).abs() < 18
                && total_dy.abs() < 16;
            let mut dirty = false;

            if sb_dragging {
                // Тап по верхній або нижній стрілці смуги прокрутки
                if is_tap {
                    if p.y <= SB_Y + SB_CAP_H + 2 {
                        if st.scroll > 0 {
                            st.scroll -= 1;
                            if st.selected >= st.scroll + VISIBLE_ROWS {
                                st.selected = st.scroll + VISIBLE_ROWS - 1;
                            }
                            dirty = true;
                        }
                    } else if p.y >= SB_Y + SB_H - SB_CAP_H - 2 && st.scroll < st.max_scroll() {
                        st.scroll += 1;
                        st.selected = st.scroll + VISIBLE_ROWS - 1;
                        dirty = true;
                    }
                }
            } else if st.count > VISIBLE_ROWS
                && !list_dragged
                && in_box(p, LIST_X, LIST_Y, LIST_W_SCROLL as i32, LIST_H)
                && total_dy.abs() >= 16
            {
                // Короткий швидкий свайп (flick) по списку ігор
                if total_dy < 0 && st.scroll_by(1) {
                    if st.selected < st.scroll {
                        st.selected = st.scroll;
                    }
                    dirty = true;
                } else if total_dy > 0 && st.scroll_by(-1) {
                    if st.selected >= st.scroll + VISIBLE_ROWS {
                        st.selected = st.scroll + VISIBLE_ROWS - 1;
                    }
                    dirty = true;
                }
            } else if is_tap {
                let row_w = if st.count > VISIBLE_ROWS {
                    LIST_W_SCROLL as i32
                } else {
                    LIST_W_FULL as i32
                };

                if HDR_SD_BOX.contains(p) {
                    st.refresh(&mut board);
                    dirty = true;
                } else if BTN_PLAY.contains(p) && st.count > 0 {
                    let chosen = st.entries[st.selected];
                    do_launch(&mut board, &chosen, false);
                    st.refresh(&mut board);
                    dirty = true;
                } else if BTN_DEMO.contains(p) && st.count > 0 {
                    let chosen = st.entries[st.selected];
                    if chosen.supports_demo {
                        do_launch(&mut board, &chosen, true);
                        st.refresh(&mut board);
                        dirty = true;
                    }
                } else if st.sd_status.is_ok()
                    && st.count == 0
                    && st.has_flash_pack
                    && BTN_SD_ACT.contains(p)
                {
                    try_run_sdpack_install(&mut board, &mut st);
                    dirty = true;
                } else if st.count > VISIBLE_ROWS && BTN_SCR_UP.contains(p) {
                    if st.scroll > 0 {
                        st.scroll -= 1;
                        if st.selected >= st.scroll + VISIBLE_ROWS {
                            st.selected = st.scroll + VISIBLE_ROWS - 1;
                        }
                        dirty = true;
                    } else if st.selected > 0 {
                        st.selected -= 1;
                        dirty = true;
                    }
                } else if st.count > VISIBLE_ROWS && BTN_SCR_DN.contains(p) {
                    if st.scroll < st.max_scroll() {
                        st.scroll += 1;
                        st.selected = st.scroll + VISIBLE_ROWS - 1;
                        dirty = true;
                    } else if st.selected + 1 < st.count {
                        st.selected += 1;
                        dirty = true;
                    }
                } else if st.count > VISIBLE_ROWS && BTN_SCR_MID.contains(p) {
                    st.refresh(&mut board);
                    dirty = true;
                } else if st.sd_status.is_err() && BTN_SD_ACT.contains(p) {
                    // перший тап — «озброїти», другий (протягом ~5 с) — форматувати
                    if st.format_armed > 0 {
                        do_format_sd(&mut board, &mut st);
                    } else {
                        st.format_armed = 320;
                    }
                    dirty = true;
                } else if st.count <= VISIBLE_ROWS && BTN_SD_ACT.contains(p) {
                    st.refresh(&mut board);
                    dirty = true;
                } else if st.count > 0 && in_box(p, LIST_X, LIST_Y, row_w, LIST_H) {
                    let rel_y = p.y - LIST_Y;
                    let slot = (rel_y / (ROW_H + ROW_GAP)) as usize;
                    let idx = st.scroll + slot;
                    if slot < VISIBLE_ROWS && idx < st.count {
                        if st.selected == idx {
                            // Повторний тап по вже вибраній грі одразу запускає її!
                            let chosen = st.entries[idx];
                            do_launch(&mut board, &chosen, false);
                            st.refresh(&mut board);
                        } else {
                            st.selected = idx;
                        }
                        dirty = true;
                    }
                }
            }

            if dirty {
                render_launcher(&st);
                board.present_all();
            }
            list_dragged = false;
            sb_dragging = false;
        }
        was_pressed = now_pressed;
        board.delay.delay_millis(14);
    }
}
