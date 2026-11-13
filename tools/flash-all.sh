#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
PORT="/dev/ttyUSB0"
NO_SEED=0

for arg in "$@"; do
  case "$arg" in
    --no-seed) NO_SEED=1 ;;
    -h|--help)
      cat <<'USAGE'
Використання: tools/flash-all.sh [порт] [--no-seed]

  порт        послідовний порт CH340 (типово /dev/ttyUSB0)
  --no-seed   не заливати пакет ігор у розділ sdseed (не тригерить переустановку /GAMES на картці).
              Корисно, коли ігри на microSD вже є і треба оновити лише лаунчер.
USAGE
      exit 0
      ;;
    -*)
      echo "Невідомий параметр: $arg (див. --help)" >&2
      exit 2
      ;;
    *) PORT="$arg" ;;
  esac
done

cd "$ROOT"
bash "$ROOT/tools/build-all.sh"

echo "=== Flashing Bootloader + Partition Table + GameLauncher (factory) ==="
espflash flash --non-interactive --chip esp32c5 --port "$PORT" --flash-size 16mb \
  --partition-table "$ROOT/partitions.csv" \
  --target-app-partition factory \
  --erase-parts otadata \
  "$ROOT/target/riscv32imac-unknown-none-elf/release/cyd-launcher"

if [ "$NO_SEED" -eq 1 ]; then
  echo "=== Skipping SD seed bundle (--no-seed): /GAMES on the card is left untouched ==="
else
  echo "=== Flashing SD Seed Bundle to 0x310000 (installs/updates /GAMES/*.BIN on boot) ==="
  # espflash write-bin замінює esptool.py — ESP-IDF не потрібен
  espflash write-bin --chip esp32c5 --port "$PORT" 0x310000 "$ROOT/dist/sdseed.bin"
fi

echo "=== Done! GameLauncher is flashed. Games are installed/updated from sdseed on boot. ==="
