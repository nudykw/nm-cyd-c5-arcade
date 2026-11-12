//! Стандартний CRC32 (IEEE 802.3, поліном 0xEDB88320) — сумісний із `zlib.crc32`.

pub fn crc32_update(mut crc: u32, data: &[u8]) -> u32 {
    crc = !crc;
    for &b in data {
        crc ^= b as u32;
        for _ in 0..8 {
            let mask = (crc & 1).wrapping_neg();
            crc = (crc >> 1) ^ (0xEDB8_8320 & mask);
        }
    }
    !crc
}

#[inline]
pub fn crc32(data: &[u8]) -> u32 {
    crc32_update(0, data)
}
