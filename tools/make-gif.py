#!/usr/bin/env python3
"""Складає анімований GIF із PNG-кадрів, знятих `tools/screenshot.py` з плати.

Навіщо свій енкодер: у репозиторії немає жодних залежностей, а ffmpeg/ImageMagick на
ARM-платі тягнуть пів системи. Тут — тільки стандартна бібліотека Python: декодування PNG
(фільтр 0, RGB8 — саме те, що пише `screenshot.py`), медіан-розсічення палітри до 256 кольорів
і LZW-компресія GIF89a.

Використання:
    python3 tools/make-gif.py out.gif frame1.png frame2.png ... [--delay 15] [--width 320]
"""

import struct
import sys
import zlib

# ---------------------------------------------------------------- PNG


def read_png(path):
    """Повертає (width, height, [рядок байтів RGB]) для PNG, який пише screenshot.py."""
    data = open(path, "rb").read()
    if data[:8] != b"\x89PNG\r\n\x1a\n":
        raise ValueError(f"{path}: не PNG")
    pos = 8
    width = height = None
    idat = b""
    while pos < len(data):
        (length,) = struct.unpack(">I", data[pos : pos + 4])
        tag = data[pos + 4 : pos + 8]
        chunk = data[pos + 8 : pos + 8 + length]
        if tag == b"IHDR":
            width, height, depth, ctype = struct.unpack(">IIBB", chunk[:10])
            if depth != 8 or ctype != 2:
                raise ValueError(f"{path}: очікується 8-бітний RGB")
        elif tag == b"IDAT":
            idat += chunk
        elif tag == b"IEND":
            break
        pos += 12 + length

    raw = zlib.decompress(idat)
    stride = width * 3
    rows = []
    for y in range(height):
        start = y * (stride + 1)
        if raw[start] != 0:
            raise ValueError(f"{path}: підтримується лише фільтр 0")
        rows.append(raw[start + 1 : start + 1 + stride])
    return width, height, rows


def scale_rows(rows, src_w, src_h, dst_w, dst_h):
    """Найближчий сусід (без згладжування — так піксель-арт лишається чітким)."""
    if src_w == dst_w and src_h == dst_h:
        return rows
    out = []
    for y in range(dst_h):
        sy = y * src_h // dst_h
        src = rows[sy]
        line = bytearray(dst_w * 3)
        for x in range(dst_w):
            sx = x * src_w // dst_w
            line[x * 3 : x * 3 + 3] = src[sx * 3 : sx * 3 + 3]
        out.append(bytes(line))
    return out


# ---------------------------------------------------------------- палітра (медіан-розсічення)


def collect_colors(frames):
    """Гістограма кольорів по всіх кадрах: { (r,g,b): кількість }."""
    hist = {}
    for rows in frames:
        for line in rows:
            for i in range(0, len(line), 3):
                c = line[i : i + 3]
                hist[c] = hist.get(c, 0) + 1
    return hist


def median_cut(hist, max_colors):
    """Медіан-розсічення гістограми до <= max_colors кольорів."""
    if len(hist) <= max_colors:
        palette = sorted(hist.keys())
        while len(palette) < 256:            # GIF вимагає таблицю розміру степеня двійки
            palette.append((0, 0, 0))
        return palette

    boxes = [[c for c in hist.keys()]]
    while len(boxes) < max_colors:
        # розщеплюємо найбільшу за кількістю пікселів коробку
        boxes.sort(key=lambda b: -sum(hist[c] for c in b))
        box = boxes.pop(0)
        if len(box) < 2:
            boxes.append(box)
            break
        # найдовший канал
        ranges = []
        for ch in range(3):
            vals = [c[ch] for c in box]
            ranges.append(max(vals) - min(vals))
        ch = ranges.index(max(ranges))
        box.sort(key=lambda c: c[ch])
        # зсуваємо межу, щоб не розрізати однакові значення каналу
        mid = len(box) // 2
        while 0 < mid < len(box) and box[mid][ch] == box[mid - 1][ch]:
            mid += 1
        if mid <= 0 or mid >= len(box):
            mid = len(box) // 2
        boxes.append(box[:mid])
        boxes.append(box[mid:])

    palette = []
    for box in boxes:
        total = sum(hist[c] for c in box) or 1
        r = sum(c[0] * hist[c] for c in box) // total
        g = sum(c[1] * hist[c] for c in box) // total
        b = sum(c[2] * hist[c] for c in box) // total
        palette.append((r, g, b))
    while len(palette) < 256:                # добиваємо до 256
        palette.append((0, 0, 0))
    return palette


def nearest_index_map(hist, palette):
    """Для кожного унікального кольору — індекс найближчого в палітрі."""
    mapping = {}
    for c in hist.keys():
        best = 0
        best_d = 1 << 30
        for i, p in enumerate(palette):
            dr = c[0] - p[0]
            dg = c[1] - p[1]
            db = c[2] - p[2]
            d = dr * dr + dg * dg + db * db
            if d < best_d:
                best_d = d
                best = i
        mapping[c] = best
    return mapping


def index_frame(rows, mapping):
    out = []
    for line in rows:
        idx = bytearray(len(line) // 3)
        for x in range(len(idx)):
            idx[x] = mapping[line[x * 3 : x * 3 + 3]]
        out.append(bytes(idx))
    return out


# ---------------------------------------------------------------- LZW


def lzw_encode(indices, min_code_size):
    """GIF-LZW. indices — плаский список індексів палітри."""
    clear = 1 << min_code_size
    end = clear + 1
    code_size = min_code_size + 1
    table = {(i,): i for i in range(clear)}
    next_code = end + 1

    out = bytearray()
    bitbuf = 0
    bitcount = 0

    def emit(code):
        nonlocal bitbuf, bitcount
        bitbuf |= code << bitcount
        bitcount += code_size
        while bitcount >= 8:
            out.append(bitbuf & 0xFF)
            bitbuf >>= 8
            bitcount -= 8

    emit(clear)
    if not indices:
        emit(end)
    else:
        prefix = (indices[0],)
        for value in indices[1:]:
            candidate = prefix + (value,)
            if candidate in table:
                prefix = candidate
                continue
            emit(table[prefix])
            if next_code < 4096:
                table[candidate] = next_code
                next_code += 1
                if next_code > (1 << code_size) and code_size < 12:
                    code_size += 1
            else:
                emit(clear)
                table = {(i,): i for i in range(clear)}
                next_code = end + 1
                code_size = min_code_size + 1
            prefix = (value,)
        emit(table[prefix])
        emit(end)
    if bitcount:
        out.append(bitbuf & 0xFF)
    return bytes(out)


def sub_blocks(data):
    """GIF-дані розбиваються на блоки <=255 байтів, кожен з байтом довжини."""
    out = bytearray()
    for i in range(0, len(data), 255):
        chunk = data[i : i + 255]
        out.append(len(chunk))
        out.extend(chunk)
    out.append(0)
    return bytes(out)


# ---------------------------------------------------------------- GIF


def write_gif(path, frames, width, height, palette, delay_cs, loop=True):
    out = bytearray()
    out += b"GIF89a"
    out += struct.pack("<HH", width, height)
    out += bytes([0xF7, 0, 0])  # GCT 256 кольорів, 8 біт на канал
    for i in range(256):
        r, g, b = palette[i]
        out += bytes([r, g, b])

    if loop:
        out += b"\x21\xFF\x0BNETSCAPE2.0\x03\x01"
        out += struct.pack("<H", 0)
        out += b"\x00"

    for idx_rows in frames:
        # графічний контроль: затримка кадру
        out += b"\x21\xF9\x04\x00"
        out += struct.pack("<H", delay_cs)
        out += b"\x00\x00"
        # дескриптор зображення
        out += b"\x2C"
        out += struct.pack("<HHHH", 0, 0, width, height)
        out += b"\x00"
        out += bytes([8])  # 8 біт на індекс
        flat = b"".join(idx_rows)
        out += sub_blocks(lzw_encode(flat, 8))

    out += b"\x3B"
    open(path, "wb").write(out)
    return len(out)


def main():
    args = [a for a in sys.argv[1:]]
    delay = 15
    width = None
    if "--delay" in args:
        i = args.index("--delay")
        delay = int(args[i + 1])
        del args[i : i + 2]
    if "--width" in args:
        i = args.index("--width")
        width = int(args[i + 1])
        del args[i : i + 2]
    if len(args) < 3:
        print(__doc__)
        sys.exit(1)

    out_path, sources = args[0], args[1:]
    frames = []
    w = h = None
    for src in sources:
        fw, fh, rows = read_png(src)
        if w is None:
            w, h = fw, fh
        if (fw, fh) != (w, h):
            raise ValueError("усі кадри мають бути одного розміру")
        frames.append(rows)

    if width and width != w:
        dst_h = h * width // w
        frames = [scale_rows(r, w, h, width, dst_h) for r in frames]
        w, h = width, dst_h

    hist = collect_colors(frames)
    palette = median_cut(hist, 256)
    mapping = nearest_index_map(hist, palette)
    indexed = [index_frame(rows, mapping) for rows in frames]

    size = write_gif(out_path, indexed, w, h, palette, delay)
    print(
        f"{out_path}: {len(frames)} кадрів, {w}×{h}, {len(hist)} унікальних кольорів → "
        f"палітра 256, {size / 1024:.0f} KB"
    )


if __name__ == "__main__":
    main()
