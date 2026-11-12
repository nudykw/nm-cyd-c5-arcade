#!/usr/bin/env python3
"""Знімає живий скріншот 320×240 RGB565 із плати NM-CYD-C5 без її перезавантаження (команда '?' у UART0)
і зберігає у PNG (чиста стандартна бібліотека Python, без Pillow).
"""

import os
import select
import struct
import sys
import termios
import time
import zlib


def write_png(path: str, w: int, h: int, rgb_bytes: bytes) -> None:
    def chunk(tag: bytes, data: bytes) -> bytes:
        return (
            struct.pack(">I", len(data))
            + tag
            + data
            + struct.pack(">I", zlib.crc32(tag + data) & 0xFFFFFFFF)
        )

    raw_rows = bytearray()
    stride = w * 3
    for y in range(h):
        raw_rows.append(0)  # filter = None
        raw_rows.extend(rgb_bytes[y * stride : (y + 1) * stride])

    ihdr = struct.pack(">IIBBBBB", w, h, 8, 2, 0, 0, 0)
    png = (
        b"\x89PNG\r\n\x1a\n"
        + chunk(b"IHDR", ihdr)
        + chunk(b"IDAT", zlib.compress(bytes(raw_rows), 6))
        + chunk(b"IEND", b"")
    )
    with open(path, "wb") as fp:
        fp.write(png)


def capture(port: str, out_png: str) -> None:
    fd = os.open(port, os.O_RDWR | os.O_NOCTTY | os.O_NONBLOCK)
    try:
        attrs = termios.tcgetattr(fd)
        attrs[2] &= ~termios.HUPCL
        attrs[2] |= termios.CLOCAL | termios.CREAD
        attrs[4] = attrs[5] = termios.B115200
        attrs[0] = attrs[1] = attrs[3] = 0
        termios.tcsetattr(fd, termios.TCSANOW, attrs)
        termios.tcflush(fd, termios.TCIFLUSH)

        os.write(fd, b"?")

        buf = bytearray()
        t0 = time.time()
        while time.time() - t0 < 40.0:
            r, _, _ = select.select([fd], [], [], 0.2)
            if r:
                chunk = os.read(fd, 16384)
                if chunk:
                    buf.extend(chunk)
                    if b"SNAPEND" in buf:
                        break
    finally:
        os.close(fd)

    text = buf.decode("ascii", "replace")
    lines = [ln.strip() for ln in text.splitlines() if ln.strip()]
    start = next((i for i, ln in enumerate(lines) if ln.startswith("SNAPBEGIN")), None)
    end = next((i for i, ln in enumerate(lines) if ln.startswith("SNAPEND")), None)
    if start is None or end is None or end <= start:
        raise RuntimeError(f"Did not receive complete frame (got {len(buf)} bytes)")

    parts = lines[start].split()
    w, h = int(parts[1]), int(parts[2])
    rgb = bytearray(w * h * 3)
    p = 0
    for row_hex in lines[start + 1 : end]:
        if len(row_hex) < w * 4:
            continue
        for x in range(w):
            v = int(row_hex[x * 4 : x * 4 + 4], 16)
            r5 = (v >> 11) & 0x1F
            g6 = (v >> 5) & 0x3F
            b5 = v & 0x1F
            rgb[p] = (r5 * 255 + 15) // 31
            rgb[p + 1] = (g6 * 255 + 31) // 63
            rgb[p + 2] = (b5 * 255 + 15) // 31
            p += 3

    write_png(out_png, w, h, bytes(rgb))
    print(f"Saved {w}x{h} screenshot to {out_png}")


if __name__ == "__main__":
    out = sys.argv[1] if len(sys.argv) > 1 else "/tmp/cyd-screen.png"
    port = sys.argv[2] if len(sys.argv) > 2 else "/dev/ttyUSB0"
    capture(port, out)
