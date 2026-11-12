#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

echo "=== [1/4] Host unit tests (game engines + CRC32) ==="
TMP_TEST_DIR="$(mktemp -d)"
trap 'rm -rf "$TMP_TEST_DIR"' EXIT

for pair in \
  "ttt:games/ttt/src/game.rs" \
  "g2048:games/g2048/src/game.rs" \
  "jewels:games/jewels/src/game.rs" \
  "zuma:games/zuma/src/game.rs" \
  "birds:games/birds/src/game.rs"
do
  name="${pair%%:*}"
  src="${pair#*:}"
  echo "  -> testing $name ($src)"
  rustc --edition 2024 --test "$src" -O -o "$TMP_TEST_DIR/test_$name"
  "$TMP_TEST_DIR/test_$name" --quiet
done

echo "=== [2/4] Building workspace (--release, target riscv32imac-unknown-none-elf) ==="
cargo build --release --workspace

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
  "cyd-game-birds:BIRDS.BIN"
do
  bin="${map%%:*}"
  out="${map#*:}"
  espflash save-image --chip esp32c5 --flash-size 16mb "$REL_DIR/$bin" "$SD_GAMES/$out"
done

espflash save-image --chip esp32c5 --flash-size 16mb "$REL_DIR/cyd-launcher" "$ROOT/dist/cyd-launcher.bin"

echo "=== [4/4] Packing SD seed bundle (dist/sdseed.bin) ==="
python3 "$ROOT/tools/pack-sd.py" "$SD_GAMES" "$ROOT/dist/sdseed.bin"

ls -lh "$ROOT/dist/cyd-launcher.bin" "$ROOT/dist/sdseed.bin" "$SD_GAMES"/*.BIN
