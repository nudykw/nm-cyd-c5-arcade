//! Керування розділами флеша, завантаженням ігор із SD у `ota_0`,
//! поверненням у лаунчер (`EXIT` → `factory`) та розпаковкою `sdpack` на SD-картку.

use esp_bootloader_esp_idf::{
    ota::{Ota, OtaImageState},
    partitions::{
        AppPartitionSubType, DataPartitionSubType, PARTITION_TABLE_MAX_LEN, PartitionType,
        read_partition_table,
    },
};
use esp_storage::FlashStorage;

use crate::{
    crc::crc32,
    sd::{SdErr, SdGameFile, SdSlot},
};

pub const OTA0_OFFSET: u32 = 0x0011_0000;
pub const OTA0_SIZE: u32 = 0x0010_0000;
pub const STAMP_SECTOR_OFFSET: u32 = OTA0_OFFSET + OTA0_SIZE - 0x1000; // 0x0020_F000
pub const SDSEED_OFFSET: u32 = 0x0031_0000;
pub const SDSEED_PACK_OFFSET: u32 = SDSEED_OFFSET + 0x1000; // 0x0031_1000

const STAMP_MAGIC: u32 = 0x4359_4447; // "CYDG"
const SDPACK_AUTO_MAGIC: u32 = 0x4F54_5541; // "AUTO"
const SDPACK_MAGIC: u32 = 0x4B50_4453; // "SDPK"

pub const LAUNCH_FLAG_DEMO: u32 = 1 << 0;

#[repr(C, align(4))]
struct AlignedBuf([u32; 1024]); // 4096 байтів, вирівняні на 4 байти

static mut PT_BUF: [u8; PARTITION_TABLE_MAX_LEN] = [0u8; PARTITION_TABLE_MAX_LEN];
static mut IO_WORDS: AlignedBuf = AlignedBuf([0u32; 1024]);

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
    pub name: [u8; 16],
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

/// Перемикає `otadata` назад на `factory` (GameLauncher) і перезавантажує чип.
pub fn exit_to_launcher(flash: &mut FlashStorage<'_>) -> ! {
    let pt_buf = unsafe { &mut *core::ptr::addr_of_mut!(PT_BUF) };
    if let Ok(pt) = read_partition_table(flash, pt_buf)
        && let Ok(Some(ota_part)) = pt.find_partition(PartitionType::Data(DataPartitionSubType::Ota))
    {
        let mut region = ota_part.as_flash_region(flash);
        let _ = region.erase(0, 0x2000);
    }
    esp_hal::system::software_reset()
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
    let mut ota = Ota::new(region, 2).map_err(|_| LaunchErr::Partition)?;
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
        name: target_name,
    };
    write_stamp(flash, &stamp)?;
    activate_ota0_and_reboot(flash)
}

/// Перевіряє, чи є у розділі `sdseed` (`0x311000`) резервний пакет ігор `SDPK`.
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
