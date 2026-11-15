#!/usr/bin/env python3
"""Генерує MANIFEST.TXT для GitHub Release (його читає лаунчер при оновленні по Wi-Fi).

Формат (текст, по рядку на файл):
    version=<версія>
    launcher <ім'я> <розмір> <crc32-hex>
    game     <ІМ'Я.BIN> <розмір> <crc32-hex>

Використання:
    tools/make-manifest.py [--version X.Y.Z] [--out MANIFEST.TXT] [dist-dir]

Типово dist-dir=dist (тобто dist/cyd-launcher.bin + dist/sd/GAMES/*.BIN), а версія береться
з кореневого Cargo.toml ([workspace.package] version).
"""
import argparse
import pathlib
import re
import sys
import zlib

ROOT = pathlib.Path(__file__).resolve().parent.parent
LAUNCHER = "cyd-launcher.bin"


def workspace_version() -> str:
    text = (ROOT / "Cargo.toml").read_text(encoding="utf-8")
    m = re.search(r'^\[workspace\.package\][^\[]*?^version\s*=\s*"([^"]+)"', text, re.M | re.S)
    return m.group(1) if m else "0.0.0"


def crc32(path: pathlib.Path) -> int:
    crc = 0
    with path.open("rb") as f:
        while chunk := f.read(65536):
            crc = zlib.crc32(chunk, crc)
    return crc & 0xFFFFFFFF


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("dist", nargs="?", default=str(ROOT / "dist"))
    ap.add_argument("--version", default=workspace_version())
    ap.add_argument("--out", default=None)
    args = ap.parse_args()

    dist = pathlib.Path(args.dist)
    out = pathlib.Path(args.out) if args.out else dist / "MANIFEST.TXT"
    games_dir = dist / "sd" / "GAMES"

    lines = [f"version={args.version}"]
    launcher = dist / LAUNCHER
    if launcher.is_file():
        lines.append(f"launcher {LAUNCHER} {launcher.stat().st_size} {crc32(launcher):08x}")
    else:
        print(f"warning: {launcher} не знайдено", file=sys.stderr)

    games = sorted(games_dir.glob("*.BIN")) if games_dir.is_dir() else []
    if not games:
        print(f"warning: {games_dir} порожній", file=sys.stderr)
    for g in games:
        lines.append(f"game {g.name} {g.stat().st_size} {crc32(g):08x}")

    out.write_text("\n".join(lines) + "\n", encoding="utf-8")
    print(f"{out}: {len(lines) - 1} файлів, version={args.version}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
