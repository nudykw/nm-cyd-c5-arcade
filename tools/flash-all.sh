#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
PORT="/dev/ttyUSB0"
NO_SEED=0
ASSUME_YES=0

for arg in "$@"; do
  case "$arg" in
    --no-seed) NO_SEED=1 ;;
    -y|--yes) ASSUME_YES=1 ;;
    -h|--help)
      cat <<'USAGE'
Використання: tools/flash-all.sh [порт] [--no-seed] [--yes]

  порт        послідовний порт CH340 (типово /dev/ttyUSB0)
  --no-seed   не заливати пакет ігор у розділ sdseed (не тригерить переустановку /GAMES на картці).
              Корисно, коли ігри на microSD вже є і треба оновити лише лаунчер.
  -y, --yes   не питати підтвердження перед заливкою sdseed (картка під час запису не захищена —
              див. README §10 «Як не вбити microSD-картку»).
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
  # У режимі ROM-bootloader (`espflash`) ніхто не тримає CS картки (IO10): 1.7 MB трафіку йде
  # по спільній шині SPI2 і картка може затерти собі FAT32 (перевірено 2026-11-13).
  if [ "$ASSUME_YES" -eq 0 ]; then
    echo
    echo "!!! УВАГА: під час запису sdseed microSD-картка не захищена !!!"
    echo "    CS картки (IO10) у ROM-bootloader не керується — трафік заливки йде по тій самій шині."
    echo "    Найбезпечніше: ВИЙНЯТИ microSD на ~2 хвилини, залити sdseed, вставити назад."
    echo "    Якщо картка залишиться в слототі — є ризик втратити FAT32 (лікується 'FORMAT SD')."
    echo "    Записи /SAVES при цьому не потрібні: ігри оновляться при наступному старті."
    echo
    if [ ! -t 0 ]; then
      echo "Немає TTY для підтвердження. Повторіть із --yes, якщо згодні на ризик," >&2
      echo "або з --no-seed, якщо треба оновити лише лаунчер." >&2
      exit 3
    fi
    read -r -p "Продовжити заливку sdseed? [y/N] " answer
    case "$answer" in
      y|Y|yes|YES) ;;
      *) echo "Скасовано: лаунчер залито, sdseed не чіпали."; exit 0 ;;
    esac
  fi
  echo "=== Flashing SD Seed Bundle to 0x810000 (installs/updates /GAMES/*.BIN on boot) ==="
  # espflash write-bin замінює esptool.py — ESP-IDF не потрібен
  espflash write-bin --chip esp32c5 --port "$PORT" 0x810000 "$ROOT/dist/sdseed.bin"
fi

echo "=== Done! GameLauncher is flashed. Games are installed/updated from sdseed on boot. ==="
