#!/usr/bin/env python3
"""Пакує dist/sd/GAMES/*.BIN у образ розділу sdseed (dist/sdseed.bin):
- сектор 0 (0x0000..0x1000): прапорець одноразового автоінсталу 'AUTO' (0x4F545541);
- сектор 1+ (0x1000..): пакет 'SDPK' (0x4B504453) із таблицею файлів та їхніми образами + CRC32.
"""

import os
import struct
import sys
import zlib


def main() -> None:
    if len(sys.argv) < 3:
        print("Usage: pack-sd.py <games_dir> <out_sdseed.bin>", file=sys.stderr)
        sys.exit(1)

    games_dir = sys.argv[1]
    out_path = sys.argv[2]

    order = [
        "TTT.BIN",
        "G2048.BIN",
        "JEWELS.BIN",
        "ZUMA.BIN",
        "BIRDS.BIN",
        "CHESS.BIN",
        "CHECKERS.BIN",
        "SUDOKU.BIN",
        "GOMOKU.BIN",
        "CYBERMZ.BIN",
        "SOKOBAN.BIN",
        "SWARM.BIN",
        "GARDEN.BIN",
    ]
    found = sorted(
        f for f in os.listdir(games_dir) if f.upper().endswith(".BIN") and os.path.isfile(os.path.join(games_dir, f))
    )
    files = [f for f in order if f in found] + [f for f in found if f not in order]
    if not files:
        print("No .BIN files found in", games_dir, file=sys.stderr)
        sys.exit(1)

    # Сектор 0 (4096 B): прапорець AUTO, який лаунчер зітре після першого копіювання на SD.
    sec0 = bytearray(b"\xff" * 4096)
    struct.pack_into("<I", sec0, 0, 0x4F545541)  # "AUTO"

    # Зміщення 0x1000 у розділі sdseed: заголовок SDPK (32 B) + N*32 B записів + вирівняні тіла файлів.
    count = len(files)
    table_end = 32 + count * 32
    cur_rel = (table_end + 63) & ~63  # вирівняємо початок даних на 64 B

    entries = []
    payloads = bytearray()
    for fname in files:
        up = fname.upper()
        if len(up.encode("ascii")) > 15:
            raise ValueError(f"Filename too long for 8.3/16B slot: {up}")
        raw = open(os.path.join(games_dir, fname), "rb").read()
        crc = zlib.crc32(raw) & 0xFFFFFFFF
        rel_off = cur_rel + len(payloads)
        entries.append((up.encode("ascii"), rel_off, len(raw), crc))
        payloads.extend(raw)
        pad = (-len(raw)) & 3
        if pad:
            payloads.extend(b"\xff" * pad)

    pack_hdr = bytearray(b"\x00" * cur_rel)
    # magic="SDPK" (0x4B504453), version=1 (u16), count (u16)
    struct.pack_into("<IHH", pack_hdr, 0, 0x4B504453, 1, count)
    for idx, (name_b, rel_off, fsize, crc) in enumerate(entries):
        base = 32 + idx * 32
        pack_hdr[base : base + len(name_b)] = name_b
        struct.pack_into("<IIII", pack_hdr, base + 16, rel_off, fsize, crc, 0)

    out = sec0 + pack_hdr + payloads
    # Доповнимо до межі 4 КБ
    rem = (-len(out)) & 4095
    if rem:
        out.extend(b"\xff" * rem)

    os.makedirs(os.path.dirname(os.path.abspath(out_path)), exist_ok=True)
    with open(out_path, "wb") as fp:
        fp.write(out)

    print(f"Packed {count} games into {out_path} ({len(out)} bytes):")
    for name_b, rel_off, fsize, crc in entries:
        print(f"  {name_b.decode():12s}  size={fsize:7d} B  rel=0x{rel_off:06x}  crc32=0x{crc:08x}")


if __name__ == "__main__":
    main()
