#![no_std]
#![no_main]

use core::fmt::Write;
use cyd_c5_common::{
    Board, Display, SCREEN_H, SCREEN_W,
    board::present_rect_on,
    boot::{
        has_sdpack_auto_flag, has_sdpack_in_flash, install_sdpack_to_sd, launch_game_from_sd,
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

const MAX_GAMES: usize = 12;
const PAGE_SIZE: usize = 5;

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
const LIST_W: u32 = 176;
const ROW_H: i32 = 39;
const ROW_GAP: i32 = 2;

const RIGHT_X: i32 = 188;
const RIGHT_W: u32 = 126;
const INFO_Y: i32 = 31;
const INFO_H: u32 = 102;

const BTN_PLAY: BtnRect = BtnRect::new(RIGHT_X, 138, RIGHT_W, 32);
const BTN_DEMO: BtnRect = BtnRect::new(RIGHT_X, 174, RIGHT_W, 28);
const BTN_SD_ACT: BtnRect = BtnRect::new(RIGHT_X, 206, RIGHT_W, 26);
const HDR_SD_BOX: BtnRect = BtnRect::new(190, 3, 124, 20);

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
    page: usize,
    sd_status: Result<SdInfo, SdErr>,
    has_flash_pack: bool,
    status_msg: LineBuf<32>,
}

impl LauncherState {
    fn new() -> Self {
        Self {
            entries: [MenuEntry::empty(); MAX_GAMES],
            count: 0,
            selected: 0,
            page: 0,
            sd_status: Err(SdErr::NoCard),
            has_flash_pack: false,
            status_msg: LineBuf::new(),
        }
    }

    fn refresh(&mut self, board: &mut Board) {
        self.has_flash_pack = has_sdpack_in_flash(&mut board.flash);
        self.count = 0;
        self.status_msg.clear();

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
                        if self.selected >= self.count {
                            self.selected = 0;
                        }
                        self.page = self.selected / PAGE_SIZE;
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
        fill(LIST_X, LIST_Y, LIST_W, 201, CARD_BG);
        stroke_rect(LIST_X, LIST_Y, LIST_W, 201, BTN_STD_BORDER);
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
                        "TO COPY ALL 5 GAMES",
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
        let start = st.page * PAGE_SIZE;
        let end = (start + PAGE_SIZE).min(st.count);
        for slot in 0..(end - start) {
            let idx = start + slot;
            let e = &st.entries[idx];
            let ry = LIST_Y + (slot as i32) * (ROW_H + ROW_GAP);
            let is_sel = idx == st.selected;
            let bg = if is_sel { CARD_SEL_BG } else { CARD_BG };
            let brd = if is_sel { ACCENT_CYAN } else { rgb(52, 66, 92) };
            fill(LIST_X, ry, LIST_W, ROW_H as u32, bg);
            stroke_rect(LIST_X, ry, LIST_W, ROW_H as u32, brd);
            if is_sel {
                fill(LIST_X + 1, ry + 1, 3, (ROW_H - 2) as u32, ACCENT_GOLD);
            }

            draw_game_icon(LIST_X + 7, ry + 5, e.icon_id);

            let tx = LIST_X + 41;
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
                fill(LIST_X + 140, ry + 20, 30, 12, rgb(26, 78, 58));
                text(LIST_X + 143, ry + 21, "DEMO", FontKind::Small, ACCENT_GREEN);
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

    // Нижня сервісна кнопка: якщо SD порожня й у флеші є резервний пакет — показуємо INSTALL SD
    if st.sd_status.is_ok() && st.count == 0 && st.has_flash_pack {
        draw_btn(
            BTN_SD_ACT,
            "INSTALL SD",
            FontKind::Small,
            rgb(138, 96, 20),
            TEXT_WHITE,
            ACCENT_GOLD,
        );
    } else if st.count > PAGE_SIZE {
        let mut pg = LineBuf::<20>::new();
        let pages = st.count.div_ceil(PAGE_SIZE);
        let _ = write!(pg, "PAGE {}/{} >>", st.page + 1, pages);
        draw_btn(
            BTN_SD_ACT,
            pg.as_str(),
            FontKind::Small,
            BTN_STD_BG,
            TEXT_WHITE,
            BTN_STD_BORDER,
        );
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
    let mut cmd_buf = [0u8; 32];
    let mut cmd_len = 0usize;

    loop {
        while let Some(ch) = board.poll_uart_byte() {
            if ch == b'\n' || ch == b'\r' {
                if cmd_len > 0 {
                    if let Ok(line) = core::str::from_utf8(&cmd_buf[..cmd_len]) {
                        let line = line.trim();
                        if line == "RESCAN" {
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
                        } else if let Some(arg) = line.strip_prefix("PLAY ") {
                            let target = arg.trim();
                            if let Some(idx) = (0..st.count)
                                .find(|&i| st.entries[i].sd_file.file_name_str() == target)
                            {
                                st.selected = idx;
                                let chosen = st.entries[idx];
                                do_launch(&mut board, &chosen, false);
                            }
                        } else if let Some(arg) = line.strip_prefix("DEMO ") {
                            let target = arg.trim();
                            if let Some(idx) = (0..st.count)
                                .find(|&i| st.entries[i].sd_file.file_name_str() == target)
                            {
                                st.selected = idx;
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

        let now_pressed = board.touch.pressed();
        if now_pressed && !was_pressed {
            press_pt = board.touch.point();
        } else if !now_pressed && was_pressed {
            let p = press_pt;
            let mut dirty = false;

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
            } else if BTN_SD_ACT.contains(p) {
                if st.sd_status.is_ok() && st.count == 0 && st.has_flash_pack {
                    try_run_sdpack_install(&mut board, &mut st);
                } else if st.count > PAGE_SIZE {
                    let pages = st.count.div_ceil(PAGE_SIZE);
                    st.page = (st.page + 1) % pages;
                    st.selected = st.page * PAGE_SIZE;
                } else {
                    st.refresh(&mut board);
                }
                dirty = true;
            } else if st.count > 0
                && in_box(p, LIST_X, LIST_Y, LIST_W as i32, 5 * (ROW_H + ROW_GAP))
            {
                let rel_y = p.y - LIST_Y;
                let slot = (rel_y / (ROW_H + ROW_GAP)) as usize;
                let idx = st.page * PAGE_SIZE + slot;
                if slot < PAGE_SIZE && idx < st.count {
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

            if dirty {
                render_launcher(&st);
                board.present_all();
            }
        }
        was_pressed = now_pressed;
        board.delay.delay_millis(16);
    }
}
