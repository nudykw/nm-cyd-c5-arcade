#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
PORT="${1:-/dev/ttyUSB0}"

cd "$ROOT"
bash "$ROOT/tools/build-all.sh"

echo "=== Flashing Bootloader + Partition Table + GameLauncher (factory) ==="
espflash flash --non-interactive --chip esp32c5 --port "$PORT" --flash-size 16mb \
  --partition-table "$ROOT/partitions.csv" \
  --target-app-partition factory \
  --erase-parts otadata \
  "$ROOT/target/riscv32imac-unknown-none-elf/release/cyd-launcher"

echo "=== Flashing SD Seed Bundle to 0x310000 (auto-installs /GAMES/*.BIN onto microSD on boot) ==="
# espflash write-bin replaces esptool.py here, so no ESP-IDF checkout is required
espflash write-bin --chip esp32c5 --port "$PORT" 0x310000 "$ROOT/dist/sdseed.bin"

echo "=== Done! Board is booting GameLauncher and installing games onto SD:/GAMES/ ==="
