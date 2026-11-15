//! Робота з microSD на спільній шині SPI2 (CS=IO10):
//! - автоадаптація MBR / GPT (macOS) / Superfloppy (FAT16/FAT32 без MBR);
//! - каталоги `/GAMES` (образи `.BIN` ігор) та `/SAVES` (рекорди `.SAV`);
//! - потокове читання й запис `.BIN`-файлів без динамічних алокацій.

use core::{cell::Cell, ops::ControlFlow};
use embedded_sdmmc::{
    Block, BlockCount, BlockDevice, BlockIdx, Mode, SdCard, SdCardError, TimeSource, Timestamp,
    VolumeIdx, VolumeManager, sdcard::spi::AcquireOpts,
};
use esp_hal::{delay::Delay, gpio::Output};

use crate::{
    crc::{crc32, crc32_update},
    spi::{SharedSpiBus, SharedSpiDevice},
};

pub const SD_INIT_SPI_HZ: u32 = 400_000;
pub const SD_FAST_SPI_HZ: u32 = 16_000_000;

/// Кількість секторів, які форматтер пише за один виклик (8 × 512 B = 4 KB у BSS).
const FMT_CHUNK: usize = 8;
static mut FMT_BYTES: [[u8; 512]; FMT_CHUNK] = [[0u8; 512]; FMT_CHUNK];

/// Пише `count` нульових секторів, починаючи з `lba` (дані беруться зі статичного буфера).
fn write_zero_sectors(
    mgr: &VolMgr,
    lba: u32,
    count: usize,
) -> Result<(), SdErr> {
    debug_assert!(count <= FMT_CHUNK);
    let mut blocks = [Block {
        contents: [0u8; 512],
    }; 1];
    let raw = unsafe { &*core::ptr::addr_of!(FMT_BYTES) };
    for i in 0..count {
        blocks[0].contents.copy_from_slice(&raw[i]);
        mgr.device(|d| d.write(&blocks, BlockIdx(lba + i as u32)))
            .map_err(|_| SdErr::IoError)?;
    }
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SdErr {
    NoCard,
    ExFat,
    UnsupportedLayout,
    FsError,
    NotFound,
    InvalidAppImage,
    IoError,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LayoutKind {
    Mbr,
    Gpt,
    Superfloppy,
}

#[derive(Debug, Clone, Copy)]
pub struct SdInfo {
    pub size_mb: u32,
    pub layout: LayoutKind,
}

#[derive(Clone, Copy)]
pub struct SdGameFile {
    pub file_name: [u8; 13], // Напр. "ZUMA.BIN\0..."
    pub file_name_len: u8,
    pub project_name: [u8; 32],
    pub project_name_len: u8,
    pub version: [u8; 16],
    pub version_len: u8,
    pub size: u32,
    pub head_crc: u32,
}

impl SdGameFile {
    pub const fn empty() -> Self {
        Self {
            file_name: [0; 13],
            file_name_len: 0,
            project_name: [0; 32],
            project_name_len: 0,
            version: [0; 16],
            version_len: 0,
            size: 0,
            head_crc: 0,
        }
    }

    pub fn file_name_str(&self) -> &str {
        unsafe { core::str::from_utf8_unchecked(&self.file_name[..self.file_name_len as usize]) }
    }

    pub fn project_name_str(&self) -> &str {
        unsafe {
            core::str::from_utf8_unchecked(&self.project_name[..self.project_name_len as usize])
        }
    }

    pub fn version_str(&self) -> &str {
        unsafe { core::str::from_utf8_unchecked(&self.version[..self.version_len as usize]) }
    }
}

pub struct FixedTimeSource;

impl TimeSource for FixedTimeSource {
    fn get_timestamp(&self) -> Timestamp {
        Timestamp {
            year_since_1970: 56, // 2026
            zero_indexed_month: 10,
            zero_indexed_day: 11,
            hours: 12,
            minutes: 0,
            seconds: 0,
        }
    }
}

#[derive(Clone, Copy)]
enum MapMode {
    Direct,
    VirtualMbr {
        part_type: u8,
        lba_start: u32,
        num_blocks: u32,
    },
    Superfloppy {
        part_type: u8,
        real_blocks: u32,
    },
}

/// Адаптер блочного пристрою, який прозоро підтримує MBR (будь-який із 4 слотів),
/// GPT (macOS Disk Utility: EFI + FAT32) та Superfloppy (BPB у секторі 0).
pub struct SmartSdBlock {
    card: SdCard<SharedSpiDevice<'static>, Delay>,
    mode: Cell<MapMode>,
}

impl SmartSdBlock {
    fn new(dev: SharedSpiDevice<'static>) -> Self {
        let mut opts = AcquireOpts::default();
        opts.use_crc = false;
        opts.acquire_retries = 30;
        Self {
            card: SdCard::new_with_options(dev, Delay::new(), opts),
            mode: Cell::new(MapMode::Direct),
        }
    }

    fn fill_virtual_mbr(out: &mut [u8; 512], part_type: u8, lba_start: u32, num_blocks: u32) {
        out.fill(0);
        let p = &mut out[446..462];
        p[0] = 0x00;
        p[4] = part_type;
        p[8..12].copy_from_slice(&lba_start.to_le_bytes());
        p[12..16].copy_from_slice(&num_blocks.to_le_bytes());
        out[510] = 0x55;
        out[511] = 0xAA;
    }

    fn is_fat_type(t: u8) -> bool {
        matches!(t, 0x04 | 0x06 | 0x0B | 0x0C | 0x0E)
    }

    fn detect_bpb_fat(sec: &[u8; 512]) -> Result<Option<u8>, SdErr> {
        if &sec[3..11] == b"EXFAT   " {
            return Err(SdErr::ExFat);
        }
        if sec[510] != 0x55 || sec[511] != 0xAA {
            return Ok(None);
        }
        let bps = u16::from_le_bytes([sec[11], sec[12]]);
        let spc = sec[13];
        let fats = sec[16];
        if bps != 512 || spc == 0 || !spc.is_power_of_two() || !(1..=2).contains(&fats) {
            return Ok(None);
        }
        if &sec[82..87] == b"FAT32" {
            return Ok(Some(0x0C));
        }
        if &sec[54..58] == b"FAT1" {
            return Ok(Some(0x0E));
        }
        Ok(None)
    }

    fn probe_layout(&self, total_blocks: u32) -> Result<LayoutKind, SdErr> {
        let mut b0 = [Block::new()];
        self.card.read(&mut b0, BlockIdx(0)).map_err(|_| SdErr::IoError)?;
        let s0: &[u8; 512] = (&b0[0].contents[..]).try_into().unwrap();

        // 1. Спочатку перевіримо Superfloppy (BPB прямо у секторі 0).
        if let Some(pt) = Self::detect_bpb_fat(s0)? {
            self.mode.set(MapMode::Superfloppy {
                part_type: pt,
                real_blocks: total_blocks,
            });
            return Ok(LayoutKind::Superfloppy);
        }

        if s0[510] != 0x55 || s0[511] != 0xAA {
            return Err(SdErr::UnsupportedLayout);
        }

        // 2. Перевіримо MBR (усі 4 слоти) та GPT (0xEE).
        let mut saw_gpt = false;
        let mut saw_exfat = false;
        for slot in 0..4usize {
            let off = 446 + slot * 16;
            let ptype = s0[off + 4];
            let lba = u32::from_le_bytes(s0[off + 8..off + 12].try_into().unwrap());
            let cnt = u32::from_le_bytes(s0[off + 12..off + 16].try_into().unwrap());
            if ptype == 0 || cnt == 0 {
                continue;
            }
            if ptype == 0xEE {
                saw_gpt = true;
                continue;
            }
            if ptype == 0x07 {
                // 0x07 може бути exFAT або NTFS — глянемо перший сектор розділу.
                let mut bp = [Block::new()];
                if self.card.read(&mut bp, BlockIdx(lba)).is_ok() {
                    let sp: &[u8; 512] = (&bp[0].contents[..]).try_into().unwrap();
                    if &sp[3..11] == b"EXFAT   " {
                        saw_exfat = true;
                    }
                }
            }
            if Self::is_fat_type(ptype) {
                if slot == 0 {
                    self.mode.set(MapMode::Direct);
                } else {
                    self.mode.set(MapMode::VirtualMbr {
                        part_type: ptype,
                        lba_start: lba,
                        num_blocks: cnt,
                    });
                }
                return Ok(LayoutKind::Mbr);
            }
        }

        // 3. Якщо це GPT (типово з macOS Disk Utility: slot 0 = EFI, slot 1 = Basic Data FAT32).
        if saw_gpt {
            let mut hdr_blk = [Block::new()];
            self.card
                .read(&mut hdr_blk, BlockIdx(1))
                .map_err(|_| SdErr::IoError)?;
            let h = &hdr_blk[0].contents;
            if &h[0..8] == b"EFI PART" {
                let entries_lba = u64::from_le_bytes(h[72..80].try_into().unwrap()) as u32;
                let num_entries =
                    u32::from_le_bytes(h[80..84].try_into().unwrap()).min(16);
                let entry_sz = u32::from_le_bytes(h[84..88].try_into().unwrap());
                if entry_sz == 128 {
                    const EFI_SYSTEM_GUID: [u8; 16] = [
                        0x28, 0x73, 0x2A, 0xC1, 0x1F, 0xF8, 0xD2, 0x11, 0xBA, 0x4B, 0x00, 0xA0,
                        0xC9, 0x3E, 0xC9, 0x3B,
                    ];
                    let mut chosen: Option<(u8, u32, u32)> = None;
                    let sectors = (num_entries * 128).div_ceil(512);
                    for s in 0..sectors {
                        let mut eb = [Block::new()];
                        self.card
                            .read(&mut eb, BlockIdx(entries_lba + s))
                            .map_err(|_| SdErr::IoError)?;
                        for idx in 0..4usize {
                            let e = &eb[0].contents[idx * 128..(idx + 1) * 128];
                            if e[0..16].iter().all(|&b| b == 0) {
                                continue;
                            }
                            let is_efi = e[0..16] == EFI_SYSTEM_GUID;
                            let first = u64::from_le_bytes(e[32..40].try_into().unwrap());
                            let last = u64::from_le_bytes(e[40..48].try_into().unwrap());
                            if first == 0 || last < first || last >= u32::MAX as u64 {
                                continue;
                            }
                            let first32 = first as u32;
                            let cnt32 = (last - first + 1) as u32;
                            let mut pb = [Block::new()];
                            if self.card.read(&mut pb, BlockIdx(first32)).is_ok() {
                                let sp: &[u8; 512] = (&pb[0].contents[..]).try_into().unwrap();
                                match Self::detect_bpb_fat(sp) {
                                    Ok(Some(pt)) => {
                                        if !is_efi {
                                            self.mode.set(MapMode::VirtualMbr {
                                                part_type: pt,
                                                lba_start: first32,
                                                num_blocks: cnt32,
                                            });
                                            return Ok(LayoutKind::Gpt);
                                        } else if chosen.is_none() {
                                            chosen = Some((pt, first32, cnt32));
                                        }
                                    }
                                    Err(SdErr::ExFat) => saw_exfat = true,
                                    _ => {}
                                }
                            }
                        }
                    }
                    if let Some((pt, first32, cnt32)) = chosen {
                        self.mode.set(MapMode::VirtualMbr {
                            part_type: pt,
                            lba_start: first32,
                            num_blocks: cnt32,
                        });
                        return Ok(LayoutKind::Gpt);
                    }
                }
            }
        }

        if saw_exfat {
            Err(SdErr::ExFat)
        } else {
            Err(SdErr::UnsupportedLayout)
        }
    }
}

impl BlockDevice for SmartSdBlock {
    type Error = SdCardError;

    fn read(&self, blocks: &mut [Block], start_block_idx: BlockIdx) -> Result<(), Self::Error> {
        if blocks.is_empty() {
            return Ok(());
        }
        match self.mode.get() {
            MapMode::Direct => self.card.read(blocks, start_block_idx),
            MapMode::VirtualMbr {
                part_type,
                lba_start,
                num_blocks,
            } => {
                if start_block_idx.0 == 0 {
                    let s0: &mut [u8; 512] = (&mut blocks[0].contents[..]).try_into().unwrap();
                    Self::fill_virtual_mbr(s0, part_type, lba_start, num_blocks);
                    if blocks.len() > 1 {
                        self.card.read(&mut blocks[1..], BlockIdx(1))?;
                    }
                    Ok(())
                } else {
                    self.card.read(blocks, start_block_idx)
                }
            }
            MapMode::Superfloppy {
                part_type,
                real_blocks,
            } => {
                if start_block_idx.0 == 0 {
                    let s0: &mut [u8; 512] = (&mut blocks[0].contents[..]).try_into().unwrap();
                    Self::fill_virtual_mbr(s0, part_type, 1, real_blocks);
                    if blocks.len() > 1 {
                        self.card.read(&mut blocks[1..], BlockIdx(0))?;
                    }
                    Ok(())
                } else {
                    self.card.read(blocks, BlockIdx(start_block_idx.0 - 1))
                }
            }
        }
    }

    fn write(&self, blocks: &[Block], start_block_idx: BlockIdx) -> Result<(), Self::Error> {
        if blocks.is_empty() {
            return Ok(());
        }
        match self.mode.get() {
            MapMode::Direct => self.card.write(blocks, start_block_idx),
            MapMode::VirtualMbr { .. } => {
                if start_block_idx.0 == 0 {
                    return Err(SdCardError::WriteError);
                }
                self.card.write(blocks, start_block_idx)
            }
            MapMode::Superfloppy { .. } => {
                if start_block_idx.0 == 0 {
                    return Err(SdCardError::WriteError);
                }
                self.card.write(blocks, BlockIdx(start_block_idx.0 - 1))
            }
        }
    }

    fn num_blocks(&self) -> Result<BlockCount, Self::Error> {
        let n = self.card.num_blocks()?;
        match self.mode.get() {
            MapMode::Superfloppy { .. } => Ok(BlockCount(n.0.saturating_add(1))),
            _ => Ok(n),
        }
    }
}

type VolMgr = VolumeManager<SmartSdBlock, FixedTimeSource, 4, 4, 1>;

/// `"CHESS.BIN"` → `"CHESS.TMP"` (8.3): тимчасове ім'я для безпечної заміни файлу.
fn tmp_name(file_name: &str) -> Result<LineBufName, SdErr> {
    let base = file_name.split('.').next().unwrap_or(file_name);
    if base.is_empty() || base.len() > 8 {
        return Err(SdErr::NotFound);
    }
    let mut out = LineBufName::new();
    out.push_str(base);
    out.push_str(".TMP");
    Ok(out)
}

/// Маленький `no_std`-рядок під ім'я 8.3 (без алокацій).
struct LineBufName {
    buf: [u8; 13],
    len: usize,
}

impl LineBufName {
    const fn new() -> Self {
        Self {
            buf: [0; 13],
            len: 0,
        }
    }

    fn push_str(&mut self, s: &str) {
        for b in s.bytes() {
            if self.len < self.buf.len() {
                self.buf[self.len] = b;
                self.len += 1;
            }
        }
    }
}

/// Збережені облікові дані Wi-Fi з `SD:/WIFI.CFG`.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct WifiCfg {
    ssid: [u8; 32],
    ssid_len: u8,
    pass: [u8; 64],
    pass_len: u8,
}

impl WifiCfg {
    /// Створює конфіг; `None`, якщо SSID порожній/довший за 32 байти або пароль довший за 64.
    pub fn new(ssid: &[u8], pass: &[u8]) -> Option<Self> {
        if ssid.is_empty() || ssid.len() > 32 || pass.len() > 64 {
            return None;
        }
        let mut cfg = WifiCfg {
            ssid: [0u8; 32],
            ssid_len: ssid.len() as u8,
            pass: [0u8; 64],
            pass_len: pass.len() as u8,
        };
        cfg.ssid[..ssid.len()].copy_from_slice(ssid);
        cfg.pass[..pass.len()].copy_from_slice(pass);
        Some(cfg)
    }

    pub fn ssid(&self) -> &str {
        core::str::from_utf8(&self.ssid[..self.ssid_len as usize]).unwrap_or("")
    }

    pub fn pass(&self) -> &str {
        core::str::from_utf8(&self.pass[..self.pass_len as usize]).unwrap_or("")
    }

    pub fn ssid_bytes(&self) -> &[u8] {
        &self.ssid[..self.ssid_len as usize]
    }

    pub fn pass_bytes(&self) -> &[u8] {
        &self.pass[..self.pass_len as usize]
    }
}

impl core::fmt::Debug for WifiCfg {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "WifiCfg {{ ssid: {:?}, pass: *** }}", self.ssid())
    }
}

impl core::ops::Deref for LineBufName {
    type Target = str;
    fn deref(&self) -> &str {
        unsafe { core::str::from_utf8_unchecked(&self.buf[..self.len]) }
    }
}

pub struct SdSlot {
    bus: &'static SharedSpiBus,
    mgr: VolMgr,
    mounted: Option<SdInfo>,
}

impl SdSlot {
    pub fn new(bus: &'static SharedSpiBus, cs_sd: Output<'static>) -> Self {
        let dev = SharedSpiDevice::new(bus, cs_sd, SD_INIT_SPI_HZ);
        let block = SmartSdBlock::new(dev);
        Self {
            bus,
            mgr: VolumeManager::new(block, FixedTimeSource),
            mounted: None,
        }
    }

    #[inline]
    pub fn info(&self) -> Option<SdInfo> {
        self.mounted
    }

    /// Ініціалізує картку (80 тактів на 400 kHz з CS=HIGH → CMD0/CMD8/ACMD41 → 16 MHz) і перевіряє том FAT.
    pub fn mount(&mut self) -> Result<SdInfo, SdErr> {
        self.mounted = None;
        self.mgr.device(|d| {
            d.card.mark_card_uninit();
            d.card.spi(|dev| dev.set_frequency(SD_INIT_SPI_HZ));
        });
        self.bus.send_dummy_clocks(SD_INIT_SPI_HZ, 10);

        let bytes = self
            .mgr
            .device(|d| d.card.num_bytes())
            .map_err(|_| SdErr::NoCard)?;
        let total_blocks = (bytes / 512).min(u32::MAX as u64) as u32;
        let size_mb = (bytes / (1024 * 1024)) as u32;

        // Після ініціалізації переходимо на робочу швидкість 16 MHz.
        let layout = self.mgr.device(|d| {
            d.card.spi(|dev| dev.set_frequency(SD_FAST_SPI_HZ));
            d.probe_layout(total_blocks)
        })?;

        // Перевіримо, що FAT-том реально відкривається.
        let vol = self
            .mgr
            .open_volume(VolumeIdx(0))
            .map_err(|_| SdErr::FsError)?;
        let _root = vol.open_root_dir().map_err(|_| SdErr::FsError)?;
        drop(_root);
        drop(vol);

        let info = SdInfo { size_mb, layout };
        self.mounted = Some(info);
        Ok(info)
    }

    pub fn ensure_mounted(&mut self) -> Result<SdInfo, SdErr> {
        if let Some(info) = self.mounted {
            Ok(info)
        } else {
            self.mount()
        }
    }

    /// Гарантує наявність каталогів `/GAMES` та `/SAVES`.
    pub fn ensure_dirs(&mut self) -> Result<(), SdErr> {
        self.ensure_mounted()?;
        let vol = self
            .mgr
            .open_volume(VolumeIdx(0))
            .map_err(|_| SdErr::FsError)?;
        let root = vol.open_root_dir().map_err(|_| SdErr::FsError)?;
        if root.open_dir("GAMES").is_err() {
            let _ = root.make_dir_in_dir("GAMES");
        }
        if root.open_dir("SAVES").is_err() {
            let _ = root.make_dir_in_dir("SAVES");
        }
        Ok(())
    }

    /// Сканує `/GAMES/*.BIN`, витягуючи `EspAppDesc` (`project_name`, `version`) і CRC заголовка.
    pub fn scan_games(&mut self, out: &mut [SdGameFile]) -> Result<usize, SdErr> {
        self.ensure_mounted()?;
        let vol = self
            .mgr
            .open_volume(VolumeIdx(0))
            .map_err(|_| SdErr::FsError)?;
        let root = vol.open_root_dir().map_err(|_| SdErr::FsError)?;
        let Ok(games_dir) = root.open_dir("GAMES") else {
            return Ok(0);
        };

        let mut count = 0usize;
        games_dir
            .iterate_dir(|de| {
                if count >= out.len() {
                    return ControlFlow::Break(());
                }
                if de.attributes.is_directory() || de.attributes.is_hidden() {
                    return ControlFlow::Continue(());
                }
                if de.name.extension() != b"BIN" {
                    return ControlFlow::Continue(());
                }
                if de.size < 288 || de.size > 0xFF_000 {
                    return ControlFlow::Continue(());
                }
                let mut item = SdGameFile::empty();
                let base = de.name.base_name();
                let ext = de.name.extension();
                let mut p = 0usize;
                item.file_name[p..p + base.len()].copy_from_slice(base);
                p += base.len();
                item.file_name[p] = b'.';
                p += 1;
                item.file_name[p..p + ext.len()].copy_from_slice(ext);
                p += ext.len();
                item.file_name_len = p as u8;
                item.size = de.size;
                out[count] = item;
                count += 1;
                ControlFlow::Continue(())
            })
            .map_err(|_| SdErr::FsError)?;

        // Тепер для кожного знайденого `.BIN` прочитаємо перші 512 байтів (заголовок образу + EspAppDesc).
        let mut head = [0u8; 512];
        let mut valid = 0usize;
        for i in 0..count {
            let fname_copy = out[i].file_name;
            let fname_len = out[i].file_name_len as usize;
            let fname = unsafe { core::str::from_utf8_unchecked(&fname_copy[..fname_len]) };
            let Ok(file) = games_dir.open_file_in_dir(fname, Mode::ReadOnly) else {
                continue;
            };
            let Ok(n) = file.read(&mut head) else {
                continue;
            };
            drop(file);
            if n < 288 || head[0] != 0xE9 {
                continue;
            }
            let mut entry = out[i];
            entry.head_crc = crc32(&head[..n]);
            // У стандартному образі ESP-IDF: 24B image header + 8B segment header = 0x20 -> EspAppDesc.
            let desc_magic = u32::from_le_bytes(head[0x20..0x24].try_into().unwrap());
            if desc_magic == 0xABCD_5432 {
                // version @ 0x20 + 16 = 0x30 (32B), project_name @ 0x20 + 48 = 0x50 (32B)
                let ver_raw = &head[0x30..0x40];
                let vlen = ver_raw.iter().position(|&b| b == 0).unwrap_or(ver_raw.len());
                entry.version[..vlen].copy_from_slice(&ver_raw[..vlen]);
                entry.version_len = vlen as u8;

                let name_raw = &head[0x50..0x70];
                let nlen = name_raw
                    .iter()
                    .position(|&b| b == 0)
                    .unwrap_or(name_raw.len());
                entry.project_name[..nlen].copy_from_slice(&name_raw[..nlen]);
                entry.project_name_len = nlen as u8;
            }
            out[valid] = entry;
            valid += 1;
        }
        Ok(valid)
    }

    /// Потоково читає `/GAMES/<file_name>` блоками у `buf` і передає їх у `on_chunk(offset, slice)`.
    pub fn stream_game_bin(
        &mut self,
        file_name: &str,
        buf: &mut [u8],
        mut on_chunk: impl FnMut(u32, &[u8]) -> Result<(), SdErr>,
    ) -> Result<u32, SdErr> {
        self.ensure_mounted()?;
        let vol = self
            .mgr
            .open_volume(VolumeIdx(0))
            .map_err(|_| SdErr::FsError)?;
        let root = vol.open_root_dir().map_err(|_| SdErr::FsError)?;
        let games_dir = root.open_dir("GAMES").map_err(|_| SdErr::NotFound)?;
        let file = games_dir
            .open_file_in_dir(file_name, Mode::ReadOnly)
            .map_err(|_| SdErr::NotFound)?;
        let total = file.length();
        let mut offset = 0u32;
        while !file.is_eof() {
            let n = file.read(buf).map_err(|_| SdErr::IoError)?;
            if n == 0 {
                break;
            }
            on_chunk(offset, &buf[..n])?;
            offset += n as u32;
        }
        if offset != total {
            return Err(SdErr::IoError);
        }
        Ok(total)
    }

    /// Записує `/GAMES/<file_name>` із потокового джерела, повертаючи CRC32 записаного файла.
    /// Записує `/GAMES/<file_name>` з колбеку-джерела.
    ///
    /// Схема «безпечного коміту»:
    /// 1. якщо файлу ще немає — пишемо одразу на місце (один прохід);
    /// 2. якщо файл уже є — пишемо в `<name>.TMP`, перечитуємо й звіряємо CRC, і лише потім
    ///    видаляємо старий `<name>.BIN` і копіюємо TMP на його місце.
    ///
    /// Так обірване живлення лишає або цілий старий файл, або `<name>.TMP` (який лаунчер
    /// прибирає на наступному старті), але не напів-записаний `.BIN`.
    pub fn write_game_bin(
        &mut self,
        file_name: &str,
        total_len: u32,
        buf: &mut [u8],
        mut fill_chunk: impl FnMut(u32, &mut [u8]) -> Result<(), SdErr>,
    ) -> Result<u32, SdErr> {
        self.ensure_dirs()?;
        let existed = self.game_matches(
            file_name,
            total_len,
            u32::MAX, // лише «чи файл існує і має потрібний розмір»
            buf,
        );
        if existed {
            // Замінюємо наявний файл через тимчасовий
            let tmp = tmp_name(file_name)?;
            let crc = self.write_new(&tmp, total_len, buf, &mut fill_chunk)?;
            let (vlen, vcrc) = self.verify_game_crc(&tmp, buf)?;
            if vlen != total_len || vcrc != crc {
                let _ = self.delete_file(&tmp);
                return Err(SdErr::IoError);
            }
            let _ = self.delete_file(file_name);
            self.copy_file(&tmp, file_name, buf)?;
            let _ = self.delete_file(&tmp);
            Ok(crc)
        } else {
            self.write_new(file_name, total_len, buf, &mut fill_chunk)
        }
    }

    /// Записує файл «з нуля» і повертає CRC32 записаних даних.
    fn write_new(
        &mut self,
        file_name: &str,
        total_len: u32,
        buf: &mut [u8],
        fill_chunk: &mut impl FnMut(u32, &mut [u8]) -> Result<(), SdErr>,
    ) -> Result<u32, SdErr> {
        self.ensure_dirs()?;
        let vol = self
            .mgr
            .open_volume(VolumeIdx(0))
            .map_err(|_| SdErr::FsError)?;
        let root = vol.open_root_dir().map_err(|_| SdErr::FsError)?;
        let games_dir = root.open_dir("GAMES").map_err(|_| SdErr::FsError)?;
        let file = games_dir
            .open_file_in_dir(file_name, Mode::ReadWriteCreateOrTruncate)
            .map_err(|_| SdErr::FsError)?;

        let mut offset = 0u32;
        let mut running_crc = 0u32;
        while offset < total_len {
            let take = ((total_len - offset) as usize).min(buf.len());
            fill_chunk(offset, &mut buf[..take])?;
            file.write(&buf[..take]).map_err(|_| SdErr::IoError)?;
            running_crc = crc32_update(running_crc, &buf[..take]);
            offset += take as u32;
        }
        file.flush().map_err(|_| SdErr::IoError)?;
        Ok(running_crc)
    }

    /// Копіює `/GAMES/<from>` у `/GAMES/<to>` (посторінково через `buf`).
    fn copy_file(&mut self, from: &str, to: &str, buf: &mut [u8]) -> Result<u32, SdErr> {
        self.ensure_mounted()?;
        let vol = self
            .mgr
            .open_volume(VolumeIdx(0))
            .map_err(|_| SdErr::FsError)?;
        let root = vol.open_root_dir().map_err(|_| SdErr::FsError)?;
        let games_dir = root.open_dir("GAMES").map_err(|_| SdErr::FsError)?;
        let src = games_dir
            .open_file_in_dir(from, Mode::ReadOnly)
            .map_err(|_| SdErr::NotFound)?;
        let dst = games_dir
            .open_file_in_dir(to, Mode::ReadWriteCreateOrTruncate)
            .map_err(|_| SdErr::FsError)?;

        let mut total = 0u32;
        loop {
            let n = src.read(buf).map_err(|_| SdErr::IoError)?;
            if n == 0 {
                break;
            }
            dst.write(&buf[..n]).map_err(|_| SdErr::IoError)?;
            total += n as u32;
        }
        dst.flush().map_err(|_| SdErr::IoError)?;
        Ok(total)
    }

    /// Читає `/GAMES/<file_name>` із картки й повертає `(довжина, CRC32)`.
    pub fn verify_game_crc(&mut self, file_name: &str, buf: &mut [u8]) -> Result<(u32, u32), SdErr> {
        let mut crc = 0u32;
        let len = self.stream_game_bin(file_name, buf, |_off, chunk| {
            crc = crc32_update(crc, chunk);
            Ok(())
        })?;
        Ok((len, crc))
    }

    /// Видаляє `/GAMES/<name>` (відсутність файлу — не помилка).
    pub fn delete_file(&mut self, name: &str) -> Result<(), SdErr> {
        self.ensure_mounted()?;
        let vol = self
            .mgr
            .open_volume(VolumeIdx(0))
            .map_err(|_| SdErr::FsError)?;
        let root = vol.open_root_dir().map_err(|_| SdErr::FsError)?;
        let games_dir = root.open_dir("GAMES").map_err(|_| SdErr::NotFound)?;
        let _ = games_dir.delete_entry_in_dir(name);
        Ok(())
    }

    /// Чи лежить на картці `<name>` із потрібними розміром і CRC.
    /// `expected_crc == u32::MAX` — перевіряти лише розмір.
    pub fn game_matches(
        &mut self,
        name: &str,
        expected_len: u32,
        expected_crc: u32,
        buf: &mut [u8],
    ) -> bool {
        let mut len = 0u32;
        let mut crc = 0u32;
        if self
            .stream_game_bin(name, buf, |_off, chunk| {
                len += chunk.len() as u32;
                crc = crc32_update(crc, chunk);
                Ok(())
            })
            .is_err()
        {
            return false;
        }
        len == expected_len && (expected_crc == u32::MAX || crc == expected_crc)
    }

    /// Прибирає залишки невдалих записів (`*.TMP`) у `/GAMES`.
    pub fn cleanup_temp_files(&mut self) {
        let mut temps = [[0u8; 13]; 16];
        let mut n = 0usize;
        if self.ensure_mounted().is_err() {
            return;
        }
        {
            let Ok(vol) = self.mgr.open_volume(VolumeIdx(0)) else {
                return;
            };
            let Ok(root) = vol.open_root_dir() else {
                return;
            };
            let Ok(dir) = root.open_dir("GAMES") else {
                return;
            };
            let _ = dir.iterate_dir(|de| {
                if n < temps.len() && !de.attributes.is_directory() && de.name.extension() == b"TMP" {
                    let mut e = [0u8; 13];
                    let base = de.name.base_name();
                    let ext = de.name.extension();
                    if base.len() + 1 + ext.len() <= 13 {
                        e[..base.len()].copy_from_slice(base);
                        e[base.len()] = b'.';
                        e[base.len() + 1..base.len() + 1 + ext.len()].copy_from_slice(ext);
                        temps[n] = e;
                        n += 1;
                    }
                }
                core::ops::ControlFlow::Continue(())
            });
        }
        for name in temps.iter().take(n) {
            let len = name.iter().position(|&b| b == 0).unwrap_or(13);
            if let Ok(s) = core::str::from_utf8(&name[..len]) {
                let _ = self.delete_file(s);
            }
        }
    }

    /// Форматує картку у **FAT32** (superfloppy: BPB у LBA 0, 32 KB кластери, мітка `GAMES`).
    ///
    /// Потрібно для відновлення картки, у якої пошкодився FAT (наприклад, якщо живлення зникло
    /// посеред запису ігор). Після форматування картка одразу монтується; вміст, звісно, втрачається.
    pub fn format_fat32(
        &mut self,
        mut on_progress: impl FnMut(u32, u32),
    ) -> Result<SdInfo, SdErr> {
        // 1. Переініціалізація картки на повільній частоті
        self.mounted = None;
        self.mgr.device(|d| {
            d.card.mark_card_uninit();
            d.card.spi(|dev| dev.set_frequency(SD_INIT_SPI_HZ));
        });
        self.bus.send_dummy_clocks(SD_INIT_SPI_HZ, 10);

        let bytes = self
            .mgr
            .device(|d| d.card.num_bytes())
            .map_err(|_| SdErr::NoCard)?;
        let total = (bytes / 512).min(u32::MAX as u64) as u32;
        if total < 16 * 1024 {
            return Err(SdErr::UnsupportedLayout);
        }

        // Далі пишемо «як є» — без віртуального MBR
        self.mgr.device(|d| {
            d.card.spi(|dev| dev.set_frequency(SD_FAST_SPI_HZ));
            d.mode.set(MapMode::Direct);
        });

        // 2. Геометрія FAT32: reserved + 2 FAT + data, кластер 64 сектори (32 KB)
        const RESERVED: u32 = 32;
        const NFATS: u32 = 2;
        let spc: u32 = 64;
        let mut fat_sz = ((total / spc) * 4).div_ceil(512);
        for _ in 0..4 {
            let data = total.saturating_sub(RESERVED + NFATS * fat_sz);
            let clusters = data / spc;
            let need = ((clusters + 2) * 4).div_ceil(512);
            if need == fat_sz {
                break;
            }
            fat_sz = need;
        }
        let data_start = RESERVED + NFATS * fat_sz;
        let clusters = total.saturating_sub(data_start) / spc;
        if clusters == 0 {
            return Err(SdErr::UnsupportedLayout);
        }

        // 3. Boot sector (BPB) + резервна копія + FSInfo
        let mut bpb = [0u8; 512];
        bpb[0] = 0xEB;
        bpb[1] = 0x58;
        bpb[2] = 0x90;
        bpb[3..11].copy_from_slice(b"MSWIN4.1");
        bpb[11..13].copy_from_slice(&512u16.to_le_bytes());
        bpb[13] = spc as u8;
        bpb[14..16].copy_from_slice(&(RESERVED as u16).to_le_bytes());
        bpb[16] = NFATS as u8;
        bpb[21] = 0xF8; // media: fixed disk
        bpb[24..26].copy_from_slice(&32u16.to_le_bytes());
        bpb[26..28].copy_from_slice(&64u16.to_le_bytes());
        bpb[32..36].copy_from_slice(&total.to_le_bytes());
        bpb[36..40].copy_from_slice(&fat_sz.to_le_bytes());
        bpb[44..48].copy_from_slice(&2u32.to_le_bytes()); // кореневий каталог — кластер 2
        bpb[48..50].copy_from_slice(&1u16.to_le_bytes()); // FSInfo
        bpb[50..52].copy_from_slice(&6u16.to_le_bytes()); // backup boot sector
        bpb[64] = 0x80;
        bpb[66] = 0x29;
        bpb[67..71].copy_from_slice(&0x2026_1113u32.to_le_bytes());
        bpb[71..82].copy_from_slice(b"GAMES      ");
        bpb[82..90].copy_from_slice(b"FAT32   ");
        bpb[510] = 0x55;
        bpb[511] = 0xAA;

        let mut fsinfo = [0u8; 512];
        fsinfo[0..4].copy_from_slice(&0x4161_5252u32.to_le_bytes());
        fsinfo[484..488].copy_from_slice(&0x6141_7272u32.to_le_bytes());
        fsinfo[488..492].copy_from_slice(&(clusters.saturating_sub(1)).to_le_bytes());
        fsinfo[492..496].copy_from_slice(&3u32.to_le_bytes());
        fsinfo[510] = 0x55;
        fsinfo[511] = 0xAA;

        let total_steps = 3 + 2 * fat_sz + spc;
        let mut done = 0u32;

        // один блок за раз — для BPB/FSInfo
        let write_one = |lba: u32, data: &[u8; 512]| -> Result<(), SdErr> {
            let mut blocks = [Block {
                contents: [0u8; 512],
            }];
            blocks[0].contents.copy_from_slice(data);
            self.mgr
                .device(|d| d.write(&blocks, BlockIdx(lba)))
                .map_err(|_| SdErr::IoError)
        };

        write_one(0, &bpb)?;
        write_one(6, &bpb)?;
        write_one(1, &fsinfo)?;
        done += 3;
        on_progress(done, total_steps);

        // 4. Обидві FAT: перший сектор зі службовими записами, решта — нулі
        let mut fat_head = [0u8; 512];
        fat_head[0..4].copy_from_slice(&0x0FFF_FFF8u32.to_le_bytes());
        fat_head[4..8].copy_from_slice(&0x0FFF_FFFFu32.to_le_bytes());
        fat_head[8..12].copy_from_slice(&0x0FFF_FFFFu32.to_le_bytes());

        for fat in 0..NFATS {
            let base = RESERVED + fat * fat_sz;
            write_one(base, &fat_head)?;
            done += 1;
            for chunk in 0..(fat_sz - 1).div_ceil(FMT_CHUNK as u32) {
                let first = base + 1 + chunk * FMT_CHUNK as u32;
                let count = ((fat_sz - 1) - chunk * FMT_CHUNK as u32).min(FMT_CHUNK as u32) as usize;
                write_zero_sectors(&self.mgr, first, count)?;
                done += count as u32;
                on_progress(done, total_steps);
            }
        }

        // 5. Кореневий каталог (кластер 2) — нулі
        for chunk in 0..spc.div_ceil(FMT_CHUNK as u32) {
            let first = data_start + chunk * FMT_CHUNK as u32;
            let count = (spc - chunk * FMT_CHUNK as u32).min(FMT_CHUNK as u32) as usize;
            write_zero_sectors(&self.mgr, first, count)?;
            done += count as u32;
            on_progress(done, total_steps);
        }

        let _ = clusters;

        // 6. Перечитуємо картку «з нуля»: mount() заново визначає розкладку
        let info = self.mount()?;
        on_progress(total_steps, total_steps);
        Ok(info)
    }

    /// Читає збережене значення рекорду з `/SAVES/<save_name>` (наприклад, `"ZUMA.SAV"`).
    pub fn load_best(&mut self, save_name: &str) -> Option<u32> {
        self.ensure_mounted().ok()?;
        let vol = self.mgr.open_volume(VolumeIdx(0)).ok()?;
        let root = vol.open_root_dir().ok()?;
        let saves = root.open_dir("SAVES").ok()?;
        let file = saves.open_file_in_dir(save_name, Mode::ReadOnly).ok()?;
        let mut buf = [0u8; 32];
        let n = file.read(&mut buf).ok()?;
        let mut val: u32 = 0;
        let mut seen = false;
        for &b in &buf[..n] {
            if b.is_ascii_digit() {
                val = val.saturating_mul(10).saturating_add((b - b'0') as u32);
                seen = true;
            } else if seen {
                break;
            }
        }
        seen.then_some(val)
    }

    /// Зберігає рекорд у `/SAVES/<save_name>`.
    pub fn save_best(&mut self, save_name: &str, best: u32) -> bool {
        if self.ensure_dirs().is_err() {
            return false;
        }
        let Ok(vol) = self.mgr.open_volume(VolumeIdx(0)) else {
            return false;
        };
        let Ok(root) = vol.open_root_dir() else {
            return false;
        };
        let Ok(saves) = root.open_dir("SAVES") else {
            return false;
        };
        let Ok(file) = saves.open_file_in_dir(save_name, Mode::ReadWriteCreateOrTruncate) else {
            return false;
        };
        let mut tmp = [0u8; 10];
        let s = crate::ui::u32_str(best, &mut tmp);
        if file.write(s.as_bytes()).is_err() {
            return false;
        }
        if file.write(b"\n").is_err() {
            return false;
        }
        file.flush().is_ok()
    }

    /// Читає `SD:/WIFI.CFG` (рядки `SSID=…` та `PASS=…`). `None`, якщо файлу немає або він некоректний.
    pub fn load_wifi_cfg(&mut self) -> Option<WifiCfg> {
        self.ensure_mounted().ok()?;
        let vol = self.mgr.open_volume(VolumeIdx(0)).ok()?;
        let root = vol.open_root_dir().ok()?;
        let file = root.open_file_in_dir("WIFI.CFG", Mode::ReadOnly).ok()?;
        let mut buf = [0u8; 160];
        let n = file.read(&mut buf).ok()?;
        let text = &buf[..n];

        let mut ssid: &[u8] = &[];
        let mut pass: &[u8] = &[];
        for line in text.split(|&b| b == b'\n' || b == b'\r') {
            if let Some(v) = line.strip_prefix(b"SSID=".as_slice()) {
                ssid = v;
            } else if let Some(v) = line.strip_prefix(b"PASS=".as_slice()) {
                pass = v;
            }
        }
        if ssid.is_empty() {
            return None;
        }
        WifiCfg::new(ssid, pass)
    }

    /// Записує `SD:/WIFI.CFG` (перезаписує наявний). Пароль зберігається відкритим текстом.
    pub fn save_wifi_cfg(&mut self, cfg: &WifiCfg) -> bool {
        if self.ensure_mounted().is_err() {
            return false;
        }
        let Ok(vol) = self.mgr.open_volume(VolumeIdx(0)) else {
            return false;
        };
        let Ok(root) = vol.open_root_dir() else {
            return false;
        };
        let Ok(file) = root.open_file_in_dir("WIFI.CFG", Mode::ReadWriteCreateOrTruncate) else {
            return false;
        };
        if file.write(b"SSID=").is_err() || file.write(cfg.ssid_bytes()).is_err() {
            return false;
        }
        if file.write(b"\nPASS=").is_err() || file.write(cfg.pass_bytes()).is_err() {
            return false;
        }
        if file.write(b"\n").is_err() {
            return false;
        }
        file.flush().is_ok()
    }

    /// Видаляє `SD:/WIFI.CFG` (забути мережу). `true`, якщо файлу більше немає.
    pub fn delete_wifi_cfg(&mut self) -> bool {
        let Ok(_) = self.ensure_mounted() else {
            return false;
        };
        let Ok(vol) = self.mgr.open_volume(VolumeIdx(0)) else {
            return false;
        };
        let Ok(root) = vol.open_root_dir() else {
            return false;
        };
        match root.open_file_in_dir("WIFI.CFG", Mode::ReadOnly) {
            Ok(_) => {}
            Err(_) => return true,
        }
        root.delete_entry_in_dir("WIFI.CFG").is_ok()
    }
}
