#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

echo "=== [1/4] Host unit tests (game engines) ==="
bash "$ROOT/tools/test-host.sh"

echo "=== [2/4] Building (--release, target riscv32imac-unknown-none-elf) ==="
# Лаунчер окремо: він єдиний вмикає фічу `psram-fb` у cyd-c5-common (фреймбуфер у PSRAM,
# внутрішня SRAM — під купу/стек для TLS при оновленнях по Wi-Fi). Ігри збираються без неї,
# інакше уніфікація фіч у `--workspace` потягла б PSRAM і в них.
cargo build --release -p cyd-launcher
# `cyd-c5-wifi` (esp-alloc з `global-allocator`) не можна тягнути в одну інвокацію з іграми:
# уніфікація фіч додає їм вимогу глобального алокатора, якого в іграх немає.
cargo build --release --workspace --exclude cyd-launcher --exclude cyd-c5-wifi

echo "=== [3/4] Exporting ESP32-C5 app images to dist/sd/GAMES/*.BIN ==="
REL_DIR="$ROOT/target/riscv32imac-unknown-none-elf/release"
SD_GAMES="$ROOT/dist/sd/GAMES"
mkdir -p "$SD_GAMES"
rm -f "$SD_GAMES"/*.BIN

for map in \
  "cyd-game-ttt:TTT.BIN" \
  "cyd-game-g2048:G2048.BIN" \
  "cyd-game-jewels:JEWELS.BIN" \
  "cyd-game-zuma:ZUMA.BIN" \
  "cyd-game-birds:BIRDS.BIN" \
  "cyd-game-chess:CHESS.BIN" \
  "cyd-game-checkers:CHECKERS.BIN" \
  "cyd-game-sudoku:SUDOKU.BIN" \
  "cyd-game-gomoku:GOMOKU.BIN" \
  "cyd-game-cybermaze:CYBERMZ.BIN" \
  "cyd-game-sokoban3d:SOKOBAN.BIN" \
  "cyd-game-swarm:SWARM.BIN" \
  "cyd-game-td:GARDEN.BIN"
do
  bin="${map%%:*}"
  out="${map#*:}"
  espflash save-image --chip esp32c5 --flash-size 16mb "$REL_DIR/$bin" "$SD_GAMES/$out"
done

espflash save-image --chip esp32c5 --flash-size 16mb "$REL_DIR/cyd-launcher" "$ROOT/dist/cyd-launcher.bin"

echo "=== MANIFEST.TXT для оновлень по Wi-Fi ==="
python3 "$ROOT/tools/make-manifest.py"

echo "=== [4/4] Packing SD seed bundle (dist/sdseed.bin) ==="
python3 "$ROOT/tools/pack-sd.py" "$SD_GAMES" "$ROOT/dist/sdseed.bin"

ls -lh "$ROOT/dist/cyd-launcher.bin" "$ROOT/dist/sdseed.bin" "$SD_GAMES"/*.BIN
