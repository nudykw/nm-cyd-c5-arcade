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
    pub fn write_game_bin(
        &mut self,
        file_name: &str,
        total_len: u32,
        buf: &mut [u8],
        mut fill_chunk: impl FnMut(u32, &mut [u8]) -> Result<(), SdErr>,
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

    /// Обчислює повний CRC32 файла `/GAMES/<file_name>` на SD-картці для перевірки після запису.
    pub fn verify_game_crc(&mut self, file_name: &str, buf: &mut [u8]) -> Result<(u32, u32), SdErr> {
        let mut crc = 0u32;
        let len = self.stream_game_bin(file_name, buf, |_off, chunk| {
            crc = crc32_update(crc, chunk);
            Ok(())
        })?;
        Ok((len, crc))
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
}
