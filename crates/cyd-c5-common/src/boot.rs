//! Керування розділами флеша: запуск ігор із SD у `ota_0`, повернення в лаунчер (`EXIT`),
//! A/B-слоти лаунчера (`factory` / `ota_1` / `ota_2`) для оновлення по Wi-Fi,
//! staging-розділ `dl` для завантажень і розпаковка `sdpack` на SD-картку.

use esp_bootloader_esp_idf::{
    ota::{Ota, OtaImageState},
    partitions::{
        AppPartitionSubType, DataPartitionSubType, PARTITION_TABLE_MAX_LEN, PartitionType,
        read_partition_table,
    },
};
use esp_storage::FlashStorage;

use crate::{
    crc::{crc32, crc32_update},
    sd::{SdErr, SdGameFile, SdSlot},
};

pub const APP_SLOT_SIZE: u32 = 0x0020_0000; // 2 MB на кожен app-слот
pub const FACTORY_OFFSET: u32 = 0x0001_0000;

/// Єдиний слот для ігор (лаунчер пише сюди `.BIN` при запуску).
pub const OTA0_OFFSET: u32 = 0x0021_0000;
pub const OTA0_SIZE: u32 = APP_SLOT_SIZE;
/// A/B-слоти самого лаунчера (оновлення по Wi-Fi).
pub const OTA1_OFFSET: u32 = 0x0041_0000;
pub const OTA2_OFFSET: u32 = 0x0061_0000;
/// Штамп останнього запуску — в кінці game-слота.
pub const STAMP_SECTOR_OFFSET: u32 = OTA0_OFFSET + OTA0_SIZE - 0x1000;
pub const SDSEED_OFFSET: u32 = 0x0081_0000;
pub const SDSEED_PACK_OFFSET: u32 = SDSEED_OFFSET + 0x1000;
/// Staging-розділ для завантажень по Wi-Fi (образ перевіряється тут, перш ніж потрапити в слот/SD).
pub const DL_OFFSET: u32 = 0x00A1_0000;
pub const DL_SIZE: u32 = APP_SLOT_SIZE;

const STAMP_MAGIC: u32 = 0x4359_4448; // "CYDG2" (формат із слотом лаунчера)
const SDPACK_AUTO_MAGIC: u32 = 0x4F54_5541; // "AUTO"
const SDPACK_MAGIC: u32 = 0x4B50_4453; // "SDPK"

pub const LAUNCH_FLAG_DEMO: u32 = 1 << 0;

#[repr(C, align(4))]
struct AlignedBuf([u32; 1024]); // 4096 байтів, вирівняні на 4 байти

/// Скільки зараз OTA-app-розділів у таблиці (`ota_0` гра + `ota_1`/`ota_2` лаунчер).
const OTA_SLOTS: usize = 3;

static mut PT_BUF: [u8; PARTITION_TABLE_MAX_LEN] = [0u8; PARTITION_TABLE_MAX_LEN];
static mut IO_WORDS: AlignedBuf = AlignedBuf([0u32; 1024]);

/// Спільний 4-кілобайтний вирівняний буфер (флеш/CRC). Не тримати зайнятим довго.
#[inline]
pub fn scratch_buf() -> &'static mut [u8; 4096] {
    io_bytes_mut()
}

#[inline]
fn io_bytes_mut() -> &'static mut [u8; 4096] {
    unsafe { &mut *(core::ptr::addr_of_mut!(IO_WORDS) as *mut [u8; 4096]) }
}

#[repr(C, align(4))]
#[derive(Clone, Copy)]
pub struct LaunchStamp {
    pub magic: u32,
    pub file_size: u32,
    pub head_crc: u32,
    pub flags: u32,
    /// З якого слоту лаунчера запущено гру (`EXIT` повертає саме туди).
    pub launcher: u32,
    pub name: [u8; 16],
}

/// Де зараз живе лаунчер: «золотий» `factory` (ніколи не перезаписується по Wi-Fi)
/// або один із двох OTA-слотів A/B.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LauncherSlot {
    Factory,
    A,
    B,
}

impl LauncherSlot {
    pub fn offset(self) -> u32 {
        match self {
            LauncherSlot::Factory => FACTORY_OFFSET,
            LauncherSlot::A => OTA1_OFFSET,
            LauncherSlot::B => OTA2_OFFSET,
        }
    }

    pub fn subtype(self) -> AppPartitionSubType {
        match self {
            LauncherSlot::Factory => AppPartitionSubType::Factory,
            LauncherSlot::A => AppPartitionSubType::Ota1,
            LauncherSlot::B => AppPartitionSubType::Ota2,
        }
    }

    pub fn code(self) -> u32 {
        match self {
            LauncherSlot::Factory => 0,
            LauncherSlot::A => 1,
            LauncherSlot::B => 2,
        }
    }

    pub fn from_code(c: u32) -> Self {
        match c {
            1 => LauncherSlot::A,
            2 => LauncherSlot::B,
            _ => LauncherSlot::Factory,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            LauncherSlot::Factory => "factory",
            LauncherSlot::A => "ota_1",
            LauncherSlot::B => "ota_2",
        }
    }

    /// Куди писати новий образ лаунчера: завжди в інший слот (running-слот не чіпаємо).
    pub fn other(self) -> Self {
        match self {
            LauncherSlot::Factory => LauncherSlot::A,
            LauncherSlot::A => LauncherSlot::B,
            LauncherSlot::B => LauncherSlot::A,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LaunchErr {
    Sd(SdErr),
    Flash,
    Partition,
    BadImage,
}

impl From<SdErr> for LaunchErr {
    fn from(e: SdErr) -> Self {
        Self::Sd(e)
    }
}

/// Читає прапорці запуску поточної гри зі штампа у кінці `ota_0` (зокрема `LAUNCH_FLAG_DEMO`).
pub fn read_launch_flags(flash: &mut FlashStorage<'_>) -> u32 {
    let mut raw = [0u32; 8];
    let bytes = unsafe { core::slice::from_raw_parts_mut(raw.as_mut_ptr() as *mut u8, 32) };
    if flash.read_nor(STAMP_SECTOR_OFFSET, bytes).is_err() {
        return 0;
    }
    if raw[0] == STAMP_MAGIC { raw[3] } else { 0 }
}

/// Чи потрібно стартувати гру одразу в режимі `DEMO` (з прапорця лаунчера або фічі `demo`).
pub fn should_start_demo(flash: &mut FlashStorage<'_>, feature_demo: bool) -> bool {
    feature_demo || (read_launch_flags(flash) & LAUNCH_FLAG_DEMO) != 0
}

/// Повертає ім'я останнього завантаженого у `ota_0` образу (наприклад, `"CHESS.BIN"`), якщо штамп валідний.
pub fn last_launched_name<'a>(flash: &mut FlashStorage<'_>, out: &'a mut [u8; 16]) -> Option<&'a str> {
    let stamp = read_stamp(flash)?;
    *out = stamp.name;
    let len = out.iter().position(|&b| b == 0).unwrap_or(out.len());
    if len == 0 {
        return None;
    }
    core::str::from_utf8(&out[..len]).ok()
}

fn read_stamp(flash: &mut FlashStorage<'_>) -> Option<LaunchStamp> {
    let mut stamp = LaunchStamp {
        magic: 0,
        file_size: 0,
        head_crc: 0,
        flags: 0,
        launcher: 0,
        name: [0; 16],
    };
    let bytes = unsafe {
        core::slice::from_raw_parts_mut(
            &mut stamp as *mut LaunchStamp as *mut u8,
            core::mem::size_of::<LaunchStamp>(),
        )
    };
    flash.read_nor(STAMP_SECTOR_OFFSET, bytes).ok()?;
    if stamp.magic == STAMP_MAGIC {
        Some(stamp)
    } else {
        None
    }
}

fn write_stamp(flash: &mut FlashStorage<'_>, stamp: &LaunchStamp) -> Result<(), LaunchErr> {
    flash
        .erase(STAMP_SECTOR_OFFSET, STAMP_SECTOR_OFFSET + 0x1000)
        .map_err(|_| LaunchErr::Flash)?;
    let bytes = unsafe {
        core::slice::from_raw_parts(
            stamp as *const LaunchStamp as *const u8,
            core::mem::size_of::<LaunchStamp>(),
        )
    };
    flash
        .write_nor(STAMP_SECTOR_OFFSET, bytes)
        .map_err(|_| LaunchErr::Flash)
}

/// Який app-розділ зараз виконується (0 = factory, 1 = ota_1/A, 2 = ota_2/B).
fn current_app_number(flash: &mut FlashStorage<'_>) -> u32 {
    let pt_buf = unsafe { &mut *core::ptr::addr_of_mut!(PT_BUF) };
    let Ok(pt) = read_partition_table(flash, pt_buf) else {
        return 0;
    };
    let Ok(Some(ota_part)) = pt.find_partition(PartitionType::Data(DataPartitionSubType::Ota))
    else {
        return 0;
    };
    let region = ota_part.as_flash_region(flash);
    let Ok(mut ota) = Ota::new(region, OTA_SLOTS) else {
        return 0;
    };
    match ota.current_app_partition() {
        Ok(AppPartitionSubType::Ota1) => 1,
        Ok(AppPartitionSubType::Ota2) => 2,
        _ => 0,
    }
}

/// Поточний слот лаунчера (викликати з лаунчера; гра завжди живе в `ota_0`).
pub fn launcher_slot(flash: &mut FlashStorage<'_>) -> LauncherSlot {
    LauncherSlot::from_code(current_app_number(flash))
}

/// Перемикає `otadata` на вказаний слот і перезавантажує чип.
pub fn activate_slot(flash: &mut FlashStorage<'_>, slot: LauncherSlot) -> ! {
    let pt_buf = unsafe { &mut *core::ptr::addr_of_mut!(PT_BUF) };
    if let Ok(pt) = read_partition_table(flash, pt_buf)
        && let Ok(Some(ota_part)) = pt.find_partition(PartitionType::Data(DataPartitionSubType::Ota))
    {
        let region = ota_part.as_flash_region(flash);
        if let Ok(mut ota) = Ota::new(region, OTA_SLOTS) {
            let _ = ota.set_current_app_partition(slot.subtype());
            let _ = ota.set_current_ota_state(OtaImageState::Valid);
        }
    }
    esp_hal::system::software_reset()
}

/// Повернення з гри в лаунчер: `EXIT` веде в той слот, з якого гру запускали
/// (записано у штампі), а не завжди в `factory` — інакше оновлений лаунчер «зникав» би.
pub fn exit_to_launcher(flash: &mut FlashStorage<'_>) -> ! {
    let slot = read_stamp(flash)
        .map(|s| LauncherSlot::from_code(s.launcher))
        .unwrap_or(LauncherSlot::Factory);
    activate_slot(flash, slot)
}

fn activate_ota0_and_reboot(
    flash: &mut FlashStorage<'_>,
) -> Result<core::convert::Infallible, LaunchErr> {
    let pt_buf = unsafe { &mut *core::ptr::addr_of_mut!(PT_BUF) };
    let pt = read_partition_table(flash, pt_buf).map_err(|_| LaunchErr::Partition)?;
    let ota_part = pt
        .find_partition(PartitionType::Data(DataPartitionSubType::Ota))
        .map_err(|_| LaunchErr::Partition)?
        .ok_or(LaunchErr::Partition)?;
    let region = ota_part.as_flash_region(flash);
    let mut ota = Ota::new(region, OTA_SLOTS).map_err(|_| LaunchErr::Partition)?;
    ota.set_current_app_partition(AppPartitionSubType::Ota0)
        .map_err(|_| LaunchErr::Partition)?;
    ota.set_current_ota_state(OtaImageState::Valid)
        .map_err(|_| LaunchErr::Partition)?;
    esp_hal::system::software_reset()
}

/// Завантажує `/GAMES/<game.file_name>` із SD-картки у розділ `ota_0` (якщо він ще не закешований),
/// виставляє прапорці (`DEMO`) і перезавантажується у гру.
pub fn launch_game_from_sd(
    flash: &mut FlashStorage<'_>,
    sd: &mut SdSlot,
    game: &SdGameFile,
    start_demo: bool,
    mut on_progress: impl FnMut(u32, u32),
) -> Result<core::convert::Infallible, LaunchErr> {
    let mut target_name = [0u8; 16];
    let flen = (game.file_name_len as usize).min(16);
    target_name[..flen].copy_from_slice(&game.file_name[..flen]);
    let flags = if start_demo { LAUNCH_FLAG_DEMO } else { 0 };
    // Куди повертатись по EXIT: поточний слот лаунчера (factory/A/B).
    let launcher = launcher_slot(flash).code();

    // Перевіримо кеш у `ota_0`: якщо образ уже залитий і збігається за ім'ям/розміром/CRC заголовка,
    // не зношуємо флеш повторним записом!
    let cached = if let Some(st) = read_stamp(flash) {
        let mut hdr = [0u32; 16];
        let hdr_b = unsafe { core::slice::from_raw_parts_mut(hdr.as_mut_ptr() as *mut u8, 64) };
        let ok_hdr = flash.read_nor(OTA0_OFFSET, hdr_b).is_ok()
            && hdr_b[0] == 0xE9
            && u32::from_le_bytes(hdr_b[0x20..0x24].try_into().unwrap()) == 0xABCD_5432;
        ok_hdr
            && st.name == target_name
            && st.file_size == game.size
            && st.head_crc == game.head_crc
    } else {
        false
    };

    if cached {
        on_progress(game.size, game.size);
        let new_stamp = LaunchStamp {
            magic: STAMP_MAGIC,
            file_size: game.size,
            head_crc: game.head_crc,
            flags,
            launcher,
            name: target_name,
        };
        if read_stamp(flash).map(|s| s.flags) != Some(flags) {
            write_stamp(flash, &new_stamp)?;
        }
        return activate_ota0_and_reboot(flash);
    }

    // Кеш не збігся — стираємо потрібну частину `ota_0` і стримимо `.BIN` із SD-картки по 4 КБ.
    if game.size > OTA0_SIZE - 0x1000 {
        return Err(LaunchErr::BadImage);
    }
    on_progress(0, game.size);
    let erase_len = (game.size + 4095) & !4095;
    flash
        .erase(OTA0_OFFSET, OTA0_OFFSET + erase_len)
        .map_err(|_| LaunchErr::Flash)?;

    let buf = io_bytes_mut();
    let mut checked_header = false;
    sd.stream_game_bin(game.file_name_str(), buf, |off, chunk| {
        if !checked_header {
            if chunk.len() < 64 || chunk[0] != 0xE9 {
                return Err(SdErr::InvalidAppImage);
            }
            checked_header = true;
        }
        let aligned_len = (chunk.len() + 3) & !3;
        // Якщо останній блок не кратний 4 байтам — доповнимо 0xFF у вирівняному буфері `IO_WORDS`.
        let write_slice: &[u8] = if aligned_len == chunk.len() && (chunk.as_ptr() as usize) % 4 == 0
        {
            chunk
        } else {
            let mut tmp = [0xFFu8; 4096];
            tmp[..chunk.len()].copy_from_slice(chunk);
            if flash
                .write_nor(OTA0_OFFSET + off, &tmp[..aligned_len])
                .is_err()
            {
                return Err(SdErr::IoError);
            }
            on_progress(off + chunk.len() as u32, game.size);
            return Ok(());
        };
        if flash.write_nor(OTA0_OFFSET + off, write_slice).is_err() {
            return Err(SdErr::IoError);
        }
        on_progress(off + chunk.len() as u32, game.size);
        Ok(())
    })?;

    let stamp = LaunchStamp {
        magic: STAMP_MAGIC,
        file_size: game.size,
        head_crc: game.head_crc,
        flags,
        launcher,
        name: target_name,
    };
    write_stamp(flash, &stamp)?;
    activate_ota0_and_reboot(flash)
}

/// Скільки байтів образу треба прочитати, щоб дістати `EspAppDesc`
/// (24 B image header + 8 B segment header + 0x20 дескриптор + поля; 0x50 + 32 = 0x70).
pub const DESC_PROBE: usize = 0x80;
/// `project_name` у дескрипторі: 0x20 (дескриптор) + 48 = 0x50.
const DESC_NAME_OFF: usize = 0x50;
const APP_MAGIC: u32 = 0xABCD_5432;

/// Читає `project_name` з образу в staging-розділі (`dl`).
pub fn stage_project_name(flash: &mut FlashStorage<'_>, out: &mut [u8; 32]) -> Option<()> {
    *out = [0u8; 32];
    let mut hdr = [0u8; DESC_PROBE];
    flash.read_nor(DL_OFFSET, &mut hdr).ok()?;
    if hdr[0] != 0xE9 || u32::from_le_bytes(hdr[0x20..0x24].try_into().ok()?) != APP_MAGIC {
        return None;
    }
    out.copy_from_slice(&hdr[DESC_NAME_OFF..DESC_NAME_OFF + 32]);
    Some(())
}

/// Пише потік байтів у staging-розділ `dl`, перевіряючи заголовок ESP-образу та рахуючи CRC32.
pub struct Stage {
    /// Скільки вже записано у флеш (кратно 4 КБ).
    size: u32,
    /// Скільки накопичено в scratch-буфері (0..4096).
    fill: usize,
    crc: u32,
    max_size: u32,
}

impl Stage {
    /// Стирає `dl` під образ не більший за `max_size`.
    pub fn begin(flash: &mut FlashStorage<'_>, max_size: u32) -> Result<Self, LaunchErr> {
        if max_size == 0 || max_size > DL_SIZE - 0x1000 {
            return Err(LaunchErr::BadImage);
        }
        let erase = (max_size + 4095) & !4095;
        flash
            .erase(DL_OFFSET, DL_OFFSET + erase)
            .map_err(|_| LaunchErr::Flash)?;
        Ok(Self {
            size: 0,
            fill: 0,
            crc: 0,
            max_size,
        })
    }

    /// Додає шматок (≤ 4 КБ); у флеш пише лише повні 4-КБ блоки (вирівнювання `write_nor`).
    /// `check_image` — перевіряти, що це ESP-образ (для ігор/лаунчера).
    fn feed_inner(
        &mut self,
        flash: &mut FlashStorage<'_>,
        chunk: &[u8],
        check_image: bool,
    ) -> Result<(), LaunchErr> {
        if chunk.is_empty() || chunk.len() > 4096 {
            return Err(LaunchErr::BadImage);
        }
        if self.size + self.fill as u32 + chunk.len() as u32 > self.max_size {
            return Err(LaunchErr::BadImage);
        }
        let first_bytes = self.size == 0 && self.fill == 0;
        let mut src = chunk;
        while !src.is_empty() {
            let buf = io_bytes_mut();
            let room = 4096 - self.fill;
            let take = room.min(src.len());
            if first_bytes && check_image {
                // заголовок перевіряємо на першому шматку
                if chunk[0] != 0xE9 {
                    return Err(LaunchErr::BadImage);
                }
            }
            buf[self.fill..self.fill + take].copy_from_slice(&src[..take]);
            self.crc = crc32_update(self.crc, &src[..take]);
            self.fill += take;
            src = &src[take..];
            if self.fill == 4096 {
                flash
                    .write_nor(DL_OFFSET + self.size, &buf[..4096])
                    .map_err(|_| LaunchErr::Flash)?;
                self.size += 4096;
                self.fill = 0;
            }
        }
        Ok(())
    }

    /// Додає шматок без перевірки ESP-заголовка (діагностика `UURL`).
    pub fn feed_raw(
        &mut self,
        flash: &mut FlashStorage<'_>,
        chunk: &[u8],
    ) -> Result<(), LaunchErr> {
        self.feed_inner(flash, chunk, false)
    }

    /// Додає шматок образу (перший шматок мусить починатися з 0xE9).
    pub fn feed(&mut self, flash: &mut FlashStorage<'_>, chunk: &[u8]) -> Result<(), LaunchErr> {
        self.feed_inner(flash, chunk, true)
    }

    /// Дописує «хвіст» (< 4 КБ) із вирівнюванням до 4 байтів.
    pub fn flush(&mut self, flash: &mut FlashStorage<'_>) -> Result<(), LaunchErr> {
        if self.fill == 0 {
            return Ok(());
        }
        let buf = io_bytes_mut();
        let aligned = (self.fill + 3) & !3;
        for b in &mut buf[self.fill..aligned] {
            *b = 0xFF;
        }
        flash
            .write_nor(DL_OFFSET + self.size, &buf[..aligned])
            .map_err(|_| LaunchErr::Flash)?;
        self.size += self.fill as u32;
        self.fill = 0;
        Ok(())
    }

    /// Скільки байтів образу вже прийнято (записано + у буфері).
    pub fn size(&self) -> u32 {
        self.size + self.fill as u32
    }

    /// CRC32 усього прийнятого образу.
    pub fn crc(&self) -> u32 {
        self.crc
    }

    /// Звіряє кількість байтів, CRC32 (із маніфесту) і `project_name` образу.
    pub fn verify(
        &self,
        flash: &mut FlashStorage<'_>,
        expect_size: u32,
        expect_crc: u32,
        expect_project: &str,
    ) -> Result<(), LaunchErr> {
        if self.size != expect_size || self.crc != expect_crc {
            return Err(LaunchErr::BadImage);
        }
        let mut name = [0u8; 32];
        stage_project_name(flash, &mut name).ok_or(LaunchErr::BadImage)?;
        let len = name.iter().position(|&b| b == 0).unwrap_or(32);
        if &name[..len] != expect_project.as_bytes() {
            return Err(LaunchErr::BadImage);
        }
        Ok(())
    }
}

/// Ставить завантажений лаунчер (`dl`) у **інший** OTA-слот, активує його й перезавантажує чип.
pub fn install_launcher_from_stage(
    flash: &mut FlashStorage<'_>,
    size: u32,
    expect_crc: u32,
) -> Result<core::convert::Infallible, LaunchErr> {
    if size == 0 || size > APP_SLOT_SIZE - 0x1000 {
        return Err(LaunchErr::BadImage);
    }
    let slot = launcher_slot(flash).other();
    let base = slot.offset();
    let buf = io_bytes_mut();
    flash
        .erase(base, base + ((size + 4095) & !4095))
        .map_err(|_| LaunchErr::Flash)?;

    let mut off = 0u32;
    let mut crc = 0u32;
    while off < size {
        let n = ((size - off) as usize).min(4096);
        flash
            .read_nor(DL_OFFSET + off, &mut buf[..n])
            .map_err(|_| LaunchErr::Flash)?;
        crc = crc32_update(crc, &buf[..n]);
        let aligned = (n + 3) & !3;
        for b in &mut buf[n..aligned] {
            *b = 0xFF;
        }
        flash
            .write_nor(base + off, &buf[..aligned])
            .map_err(|_| LaunchErr::Flash)?;
        off += n as u32;
    }
    if crc != expect_crc {
        return Err(LaunchErr::BadImage);
    }
    activate_slot(flash, slot)
}

/// CRC32 перших `size` байтів app-слота (щоб порівняти «я вже цю версію?» з маніфестом).
pub fn slot_crc(flash: &mut FlashStorage<'_>, slot: LauncherSlot, size: u32) -> Option<u32> {
    if size == 0 || size > APP_SLOT_SIZE {
        return None;
    }
    let base = slot.offset();
    let buf = io_bytes_mut();
    let mut off = 0u32;
    let mut crc = 0u32;
    while off < size {
        let n = ((size - off) as usize).min(buf.len());
        flash.read_nor(base + off, &mut buf[..n]).ok()?;
        crc = crc32_update(crc, &buf[..n]);
        off += n as u32;
    }
    Some(crc)
}

/// Перевіряє, чи є у розділі `sdseed` (`0x811000`) резервний пакет ігор `SDPK`.
pub fn has_sdpack_in_flash(flash: &mut FlashStorage<'_>) -> bool {
    let mut hdr = [0u32; 2];
    let b = unsafe { core::slice::from_raw_parts_mut(hdr.as_mut_ptr() as *mut u8, 8) };
    flash.read_nor(SDSEED_PACK_OFFSET, b).is_ok() && hdr[0] == SDPACK_MAGIC
}

/// Чи стоїть прапорець одноразового автоінсталу `AUTO` у `0x310000` (після `flash-all.sh`).
pub fn has_sdpack_auto_flag(flash: &mut FlashStorage<'_>) -> bool {
    let mut w = [0u32; 1];
    let b = unsafe { core::slice::from_raw_parts_mut(w.as_mut_ptr() as *mut u8, 4) };
    flash.read_nor(SDSEED_OFFSET, b).is_ok() && w[0] == SDPACK_AUTO_MAGIC && has_sdpack_in_flash(flash)
}

/// Розпаковує всі ігри з розділу `sdseed` (`0x311000`) на SD-картку в `/GAMES/*.BIN`,
/// перевіряє CRC32 кожного записаного файла і очищає прапорець `AUTO` у `0x310000`.
pub fn install_sdpack_to_sd(
    flash: &mut FlashStorage<'_>,
    sd: &mut SdSlot,
    mut on_file: impl FnMut(usize, usize, &str, bool),
) -> Result<(usize, usize), LaunchErr> {
    let mut hdr_words = [0u32; 8]; // 32 байти заголовка SDPK
    let hdr_b = unsafe { core::slice::from_raw_parts_mut(hdr_words.as_mut_ptr() as *mut u8, 32) };
    flash
        .read_nor(SDSEED_PACK_OFFSET, hdr_b)
        .map_err(|_| LaunchErr::Flash)?;
    if hdr_words[0] != SDPACK_MAGIC {
        return Err(LaunchErr::BadImage);
    }
    let count = ((hdr_words[1] >> 16) & 0xFFFF) as usize;
    if count == 0 || count > 16 {
        return Err(LaunchErr::BadImage);
    }

    let mut installed = 0usize;
    let mut skipped = 0usize;
    let buf = io_bytes_mut();
    for idx in 0..count {
        let mut ent_words = [0u32; 8]; // 32 байти на запис
        let ent_b =
            unsafe { core::slice::from_raw_parts_mut(ent_words.as_mut_ptr() as *mut u8, 32) };
        let ent_addr = SDSEED_PACK_OFFSET + 32 + (idx as u32) * 32;
        flash
            .read_nor(ent_addr, ent_b)
            .map_err(|_| LaunchErr::Flash)?;

        let name_raw = &ent_b[0..16];
        let nlen = name_raw.iter().position(|&b| b == 0).unwrap_or(16);
        let fname = core::str::from_utf8(&name_raw[..nlen]).map_err(|_| LaunchErr::BadImage)?;
        let rel_off = u32::from_le_bytes(ent_b[16..20].try_into().unwrap());
        let fsize = u32::from_le_bytes(ent_b[20..24].try_into().unwrap());
        let expected_crc = u32::from_le_bytes(ent_b[24..28].try_into().unwrap());

        // Інкрементальність: якщо на картці вже лежить файл із тим самим розміром і CRC —
        // не перезаписуємо (менше записів = менший ризик пошкодити FAT при втраті живлення).
        let matches = sd.game_matches(fname, fsize, expected_crc, buf);
        on_file(idx + 1, count, fname, !matches);
        if matches {
            installed += 1;
            skipped += 1;
            continue;
        }

        let src_base = SDSEED_PACK_OFFSET + rel_off;
        let written_crc = sd.write_game_bin(fname, fsize, buf, |file_off, dst| {
            let aligned = (dst.len() + 3) & !3;
            if aligned == dst.len() && (dst.as_ptr() as usize) % 4 == 0 {
                flash
                    .read_nor(src_base + file_off, dst)
                    .map_err(|_| SdErr::IoError)?;
            } else {
                let mut tmp = [0u32; 1024];
                let tb =
                    unsafe { core::slice::from_raw_parts_mut(tmp.as_mut_ptr() as *mut u8, 4096) };
                flash
                    .read_nor(src_base + file_off, &mut tb[..aligned])
                    .map_err(|_| SdErr::IoError)?;
                dst.copy_from_slice(&tb[..dst.len()]);
            }
            Ok(())
        })?;

        if written_crc != expected_crc {
            return Err(LaunchErr::BadImage);
        }
        // Додаткова перевірка читанням із самої SD-картки!
        let (vlen, vcrc) = sd.verify_game_crc(fname, buf)?;
        if vlen != fsize || vcrc != expected_crc {
            return Err(LaunchErr::BadImage);
        }
        let _ = crc32(&[]); // використовуємо імпорт
        installed += 1;
    }

    // Знімаємо одноразовий прапорець AUTO, залишаючи сам пакет SDPK у 0x311000 як резерв.
    let _ = flash.erase(SDSEED_OFFSET, SDSEED_OFFSET + 0x1000);
    Ok((installed, skipped))
}
