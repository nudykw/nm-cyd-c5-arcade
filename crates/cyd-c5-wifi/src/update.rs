//! Оновлення лаунчера та ігор із **GitHub Releases** по Wi-Fi.
//!
//! Схема (без API-токенів і без JSON):
//! 1. `SNTP` — без часу перевірка сертифікатів неможлива (немає RTC-батарейки).
//! 2. `GET https://github.com/<repo>/releases/latest/download/MANIFEST.TXT` — маленький текст
//!    із версією та списком файлів (ім'я, розмір, CRC32).
//! 3. Порівнюємо з тим, що вже є (ігри — на microSD, лаунчер — за версією з app descriptor).
//! 4. Кожен потрібний файл качаємо в staging-розділ `dl`, перевіряємо CRC32 + заголовок
//!    ESP-образу + `project_name`, і лише потім:
//!    - гра → `SD:/GAMES/<NAME>.BIN` (штатний шлях `SdSlot::write_game_bin`);
//!    - лаунчер → інший OTA-слот + `otadata` + reset (`boot::install_launcher_from_stage`).
//!
//! Маніфест генерує `tools/make-manifest.py` (див. README §11) і публікується як ассет релізу
//! разом із `cyd-launcher.bin` та `dist/sd/GAMES/*.BIN`.

use alloc::string::String;
use cyd_c5_common::{
    Board,
    boot::{self, Stage, DL_OFFSET},
    sd::SdErr,
};

use crate::http;

/// Репозиторій за замовчуванням (можна перевизначити `SD:/UPDATE.CFG` → `repo=owner/name`).
pub const DEFAULT_REPO: &str = "nudykw/nm-cyd-c5-arcade";
pub const MANIFEST_NAME: &str = "MANIFEST.TXT";
const NTP_SERVER: &str = "pool.ntp.org";
pub const MAX_ENTRIES: usize = 16;
pub const MAX_MANIFEST: usize = 2048;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    Launcher,
    Game,
}

#[derive(Clone, Copy)]
pub struct Entry {
    pub kind: Kind,
    pub name: [u8; 16],
    pub name_len: u8,
    pub size: u32,
    pub crc: u32,
}

impl Entry {
    pub const EMPTY: Entry = Entry {
        kind: Kind::Game,
        name: [0; 16],
        name_len: 0,
        size: 0,
        crc: 0,
    };

    pub fn name(&self) -> &str {
        core::str::from_utf8(&self.name[..self.name_len as usize]).unwrap_or("?")
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Phase {
    Idle,
    TimeSync,
    Checking,
    /// Маніфест отримано, є що оновлювати.
    Ready,
    Downloading,
    Done,
    Failed,
}

impl Phase {
    pub fn label(self) -> &'static str {
        match self {
            Phase::Idle => "NOT CHECKED",
            Phase::TimeSync => "SYNCING TIME...",
            Phase::Checking => "CHECKING GITHUB...",
            Phase::Ready => "UPDATE AVAILABLE",
            Phase::Downloading => "DOWNLOADING...",
            Phase::Done => "UP TO DATE",
            Phase::Failed => "FAILED",
        }
    }
}

pub struct UpdateState {
    pub phase: Phase,
    pub seq: u32,
    pub version: [u8; 16],
    pub version_len: u8,
    pub entries: [Entry; MAX_ENTRIES],
    pub count: usize,
    /// `true` — цей файл уже актуальний (не качаємо).
    pub have: [bool; MAX_ENTRIES],
    pub progress: u32,
    pub total: u32,
    pub err: &'static str,
    pub slot: &'static str,
    pub repo: [u8; 40],
    pub repo_len: u8,
    /// Діагностика `UURL`: ім'я ассета, який просто качаємо в `dl`.
    probe: [u8; 32],
    probe_len: u8,
}

impl Default for UpdateState {
    fn default() -> Self {
        Self::new()
    }
}

impl UpdateState {
    pub fn new() -> Self {
        let mut st = UpdateState {
            phase: Phase::Idle,
            seq: 0,
            version: [0; 16],
            version_len: 0,
            entries: [Entry::EMPTY; MAX_ENTRIES],
            count: 0,
            have: [false; MAX_ENTRIES],
            progress: 0,
            total: 0,
            err: "",
            slot: "factory",
            repo: [0; 40],
            repo_len: 0,
            probe: [0; 32],
            probe_len: 0,
        };
        st.set_repo(DEFAULT_REPO);
        st
    }

    pub fn set_repo(&mut self, repo: &str) {
        let n = repo.len().min(self.repo.len());
        self.repo[..n].copy_from_slice(&repo.as_bytes()[..n]);
        self.repo_len = n as u8;
        self.bump();
    }

    pub fn set_probe(&mut self, asset: &str) {
        let n = asset.len().min(self.probe.len());
        self.probe[..n].copy_from_slice(&asset.as_bytes()[..n]);
        self.probe_len = n as u8;
        self.bump();
    }

    pub fn probe_asset(&self) -> &str {
        core::str::from_utf8(&self.probe[..self.probe_len as usize]).unwrap_or("")
    }

    pub fn repo(&self) -> &str {
        core::str::from_utf8(&self.repo[..self.repo_len as usize]).unwrap_or(DEFAULT_REPO)
    }

    pub fn version(&self) -> &str {
        core::str::from_utf8(&self.version[..self.version_len as usize]).unwrap_or("?")
    }

    pub fn bump(&mut self) {
        self.seq = self.seq.wrapping_add(1);
    }

    /// Скільки файлів ще треба оновити.
    pub fn pending(&self) -> usize {
        (0..self.count).filter(|&i| !self.have[i]).count()
    }

    pub fn pending_size(&self) -> u32 {
        (0..self.count)
            .filter(|&i| !self.have[i])
            .map(|i| self.entries[i].size)
            .sum()
    }

    fn fail(&mut self, msg: &'static str) {
        self.phase = Phase::Failed;
        self.err = msg;
        self.bump();
    }
}

// ── маніфест ─────────────────────────────────────────────────────────────────────────────
/// Розбір `MANIFEST.TXT`:
/// ```text
/// version=0.5.0
/// launcher cyd-launcher.bin 1180000 1a2b3c4d
/// game TTT.BIN 110480 deadbeef
/// ```
pub fn parse_manifest(st: &mut UpdateState, text: &[u8]) {
    st.count = 0;
    st.version_len = 0;
    st.entries = [Entry::EMPTY; MAX_ENTRIES];

    for line in text.split(|&b| b == b'\n') {
        let line = trim(line);
        if line.is_empty() || line[0] == b'#' {
            continue;
        }
        if let Some(v) = line.strip_prefix(b"version=") {
            let n = v.len().min(16);
            st.version[..n].copy_from_slice(&v[..n]);
            st.version_len = n as u8;
            continue;
        }
        if st.count >= MAX_ENTRIES {
            break;
        }
        let kind = if line.starts_with(b"launcher ") {
            Kind::Launcher
        } else if line.starts_with(b"game ") {
            Kind::Game
        } else {
            continue;
        };
        let rest = &line[7..];
        let mut it = rest.split(|&b| b == b' ');
        let (Some(name), Some(size), Some(crc)) = (it.next(), it.next(), it.next()) else {
            continue;
        };
        let (Ok(size), Ok(crc)) = (parse_u32(size), parse_hex(crc)) else {
            continue;
        };
        if name.is_empty() || name.len() > 16 {
            continue;
        }
        let mut e = Entry {
            kind,
            name: [0; 16],
            name_len: name.len() as u8,
            size,
            crc,
        };
        e.name[..name.len()].copy_from_slice(name);
        st.entries[st.count] = e;
        st.have[st.count] = false;
        st.count += 1;
    }
}

fn trim(mut s: &[u8]) -> &[u8] {
    while let Some((first, rest)) = s.split_first() {
        if first.is_ascii_whitespace() {
            s = rest;
        } else {
            break;
        }
    }
    while let Some((last, rest)) = s.split_last() {
        if last.is_ascii_whitespace() {
            s = rest;
        } else {
            break;
        }
    }
    s
}

fn parse_u32(b: &[u8]) -> Result<u32, ()> {
    let mut v: u32 = 0;
    if b.is_empty() {
        return Err(());
    }
    for &c in b {
        if !c.is_ascii_digit() {
            return Err(());
        }
        v = v.checked_mul(10).ok_or(())?.checked_add((c - b'0') as u32).ok_or(())?;
    }
    Ok(v)
}

fn parse_hex(b: &[u8]) -> Result<u32, ()> {
    let b = b.strip_prefix(b"0x").unwrap_or(b);
    let mut v: u32 = 0;
    if b.is_empty() {
        return Err(());
    }
    for &c in b {
        let d = match c {
            b'0'..=b'9' => c - b'0',
            b'a'..=b'f' => c - b'a' + 10,
            b'A'..=b'F' => c - b'A' + 10,
            _ => return Err(()),
        };
        v = v.checked_mul(16).ok_or(())?.checked_add(d as u32).ok_or(())?;
    }
    Ok(v)
}

// ── перевірка / встановлення ────────────────────────────────────────────────────────────
fn url_for(st: &UpdateState, file: &str) -> String {
    let mut s = String::with_capacity(96);
    s.push_str("https://github.com/");
    s.push_str(st.repo());
    s.push_str("/releases/latest/download/");
    s.push_str(file);
    s
}

fn net_stack(
    st: &mut UpdateState,
    stack: embassy_net::Stack<'static>,
) -> Result<embassy_net::Stack<'static>, ()> {
    if crate::net::status().has_ip {
        Ok(stack)
    } else {
        st.fail("WI-FI NOT CONNECTED");
        Err(())
    }
}

/// Пробує SNTP (UDP/123). Якщо мережа його блокує (типово для готелів/Wi-Fi з фільтрами) —
/// не помилка: ланцюжок сертифікатів усе одно перевіряється до пінованого кореня,
/// просто без перевірки строків дії (див. `embedded_tls::pki::CertVerifier`).
async fn sync_clock(stack: embassy_net::Stack<'static>, st: &mut UpdateState) {
    if http::unix_time() != 0 {
        return;
    }
    st.phase = Phase::TimeSync;
    st.bump();
    esp_println::println!("UPDATE: sntp {NTP_SERVER}...");
    match http::sntp_sync(stack, NTP_SERVER).await {
        Ok(t) => esp_println::println!("UPDATE: sntp ok, unix={t}"),
        Err(e) => esp_println::println!(
            "UPDATE: sntp failed ({e:?}) — продовжуємо без перевірки дат сертифікатів"
        ),
    }
}

/// SNTP + маніфест + порівняння з тим, що вже встановлено.
pub async fn check(
    board: &mut Board,
    st: &mut UpdateState,
    stack: embassy_net::Stack<'static>,
) {
    let Ok(stack) = net_stack(st, stack) else { return };

    sync_clock(stack, st).await;

    st.phase = Phase::Checking;
    st.err = "";
    st.bump();

    let url = url_for(st, MANIFEST_NAME);
    let mut body = String::with_capacity(MAX_MANIFEST);
    let mut sink = |chunk: &[u8]| -> Result<(), http::HttpErr> {
        if body.len() + chunk.len() > MAX_MANIFEST {
            return Err(http::HttpErr::BadResponse);
        }
        let s = core::str::from_utf8(chunk).map_err(|_| http::HttpErr::BadResponse)?;
        body.push_str(s);
        Ok(())
    };
    match http::get(stack, &url, "text/plain", http::TIMEOUT_API, &mut sink).await {
        Ok(_) => {}
        Err(e) => {
            st.fail(e.label());
            return;
        }
    }
    parse_manifest(st, body.as_bytes());
    if st.count == 0 || st.version_len == 0 {
        st.fail("MANIFEST INVALID");
        return;
    }

    // Що вже актуальне: ігри — за CRC на microSD, лаунчер — за версією.
    let launcher_version = current_version();
    for i in 0..st.count {
        let e = st.entries[i];
        st.have[i] = match e.kind {
            Kind::Game => board
                .sd
                .game_matches(e.name(), e.size, e.crc, boot::scratch_buf()),
            Kind::Launcher => {
                st.version() == launcher_version && e.name() == "cyd-launcher.bin"
            }
        };
    }
    st.phase = if st.pending() == 0 {
        Phase::Done
    } else {
        Phase::Ready
    };
    st.slot = boot::launcher_slot(&mut board.flash).label();
    st.bump();
}

/// Версія цієї збірки (з `Cargo.toml` workspace → app descriptor). Джерело правди для
/// «є новіший реліз лаунчера».
pub fn current_version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

/// Качає один файл у staging і перевіряє його (CRC32 + ESP-заголовок + `project_name`).
async fn fetch_to_stage(
    board: &mut Board,
    st: &mut UpdateState,
    stack: embassy_net::Stack<'static>,
    e: Entry,
    project: &str,
    idx: usize,
) -> Result<(), &'static str> {
    let Ok(stack) = net_stack(st, stack) else {
        return Err("WI-FI NOT CONNECTED");
    };
    let url = url_for(st, e.name());

    let mut stage = Stage::begin(&mut board.flash, e.size).map_err(|_| "FLASH ERASE FAILED")?;
    st.phase = Phase::Downloading;
    st.progress = 0;
    st.total = e.size;
    st.bump();
    crate::ui::update_draw_progress(&mut board.display, st);

    let mut res = Ok(());
    {
        let mut sink = |chunk: &[u8]| -> Result<(), http::HttpErr> {
            stage
                .feed_raw(&mut board.flash, chunk)
                .map_err(|_| http::HttpErr::BadResponse)?;
            st.progress = stage.size();
            // перемальовуємо смугу приблизно кожні 16 КБ
            if st.progress % (16 * 1024) < chunk.len() as u32 {
                crate::ui::update_draw_progress(&mut board.display, st);
            }
            Ok(())
        };
        match http::get(
            stack,
            &url,
            "application/octet-stream",
            http::TIMEOUT_DOWNLOAD,
            &mut sink,
        )
        .await
        {
            Ok(_) => {}
            Err(err) => res = Err(err.label()),
        }
    }
    if let Err(msg) = res {
        return Err(msg);
    }

    if stage
        .verify(&mut board.flash, e.size, e.crc, project)
        .is_err()
    {
        return Err("IMAGE CHECK FAILED");
    }

    // гра: staging → SD (штатний шлях із CRC-звіркою)
    if e.kind == Kind::Game {
        let buf = boot::scratch_buf();
        let crc = board
            .sd
            .write_game_bin(e.name(), e.size, buf, |off, dst| {
                board
                    .flash
                    .read_nor(DL_OFFSET + off, dst)
                    .map_err(|_| SdErr::IoError)
            })
            .map_err(|_| "SD WRITE FAILED")?;
        if crc != e.crc {
            return Err("SD VERIFY FAILED");
        }
    }

    st.have[idx] = true;
    st.bump();
    Ok(())
}

/// Оновлює всі ігри з маніфесту (лаунчер не чіпає).
pub async fn install_games(
    board: &mut Board,
    st: &mut UpdateState,
    stack: embassy_net::Stack<'static>,
) {
    if st.count == 0 {
        st.fail("RUN CHECK FIRST");
        return;
    }
    for i in 0..st.count {
        let e = st.entries[i];
        if e.kind != Kind::Game || st.have[i] {
            continue;
        }
        if let Err(msg) = fetch_to_stage(board, st, stack, e, e.name(), i).await {
            st.fail(msg);
            return;
        }
    }
    st.phase = Phase::Done;
    st.progress = 0;
    st.bump();
}

/// Оновлює лаунчер: качає в `dl`, ставить у інший OTA-слот і перезавантажує чип.
pub async fn install_launcher(
    board: &mut Board,
    st: &mut UpdateState,
    stack: embassy_net::Stack<'static>,
) {
    let Some(idx) = (0..st.count)
        .find(|&i| st.entries[i].kind == Kind::Launcher && !st.have[i])
    else {
        st.fail("NO LAUNCHER UPDATE");
        return;
    };
    let e = st.entries[idx];
    let project = "cyd-launcher";
    if let Err(msg) = fetch_to_stage(board, st, stack, e, project, idx).await {
        st.fail(msg);
        return;
    }
    st.phase = Phase::Done;
    st.bump();
    esp_println::println!(
        "UPDATE: launcher {} → слот {}",
        st.version(),
        boot::launcher_slot(&mut board.flash).other().label()
    );
    let _ = boot::install_launcher_from_stage(&mut board.flash, e.size, e.crc);
    // якщо не спрацювало (не має бути) — лишаємось на старому слоті
    st.fail("LAUNCHER INSTALL FAILED");
}

/// Діагностика: качає довільний ассет релізу в `dl` і друкує розмір/CRC32.
/// Нічого не встановлює — лише перевіряє TLS, редірект на CDN і швидкість.
pub async fn probe(
    board: &mut Board,
    st: &mut UpdateState,
    stack: embassy_net::Stack<'static>,
) {
    let Ok(stack) = net_stack(st, stack) else { return };
    sync_clock(stack, st).await;
    let name = alloc::string::String::from(st.probe_asset());
    let url = url_for(st, &name);
    esp_println::println!("UPDATE: probe {url}");

    let mut stage = match Stage::begin(&mut board.flash, boot::DL_SIZE - 0x1000) {
        Ok(s) => s,
        Err(_) => {
            st.fail("FLASH ERASE FAILED");
            return;
        }
    };
    st.phase = Phase::Downloading;
    st.progress = 0;
    st.total = 0;
    st.err = "";
    st.bump();
    crate::ui::update_draw_progress(&mut board.display, st);

    let res: Result<(), http::HttpErr>;
    {
        let mut sink = |chunk: &[u8]| -> Result<(), http::HttpErr> {
            stage
                .feed_raw(&mut board.flash, chunk)
                .map_err(|_| http::HttpErr::BadResponse)?;
            st.progress = stage.size();
            if st.progress % (32 * 1024) < chunk.len() as u32 {
                crate::ui::update_draw_progress(&mut board.display, st);
            }
            Ok(())
        };
        res = http::get(
            stack,
            &url,
            "application/octet-stream",
            http::TIMEOUT_DOWNLOAD,
            &mut sink,
        )
        .await
        .map(|_| ());
    }
    match res {
        Ok(()) => {
            esp_println::println!(
                "UPDATE: probe OK: {} B, crc32={:08x}, heap_free={}",
                stage.size(),
                stage.crc(),
                esp_alloc::HEAP.free()
            );
            st.phase = Phase::Done;
            st.bump();
        }
        Err(e) => {
            esp_println::println!("UPDATE: probe failed: {}", e.label());
            st.fail(e.label());
        }
    }
}
