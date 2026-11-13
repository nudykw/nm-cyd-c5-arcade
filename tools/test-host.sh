#!/usr/bin/env bash
# Хост-тести ігрових рушіїв: кожен games/*/src/game.rs компілюється звичайним rustc (без target-тулчейну
# і без плати) і проганяється як звичайний тест-бінарник. Так перевіряється чиста логіка ігор.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

TMP_TEST_DIR="$(mktemp -d)"
trap 'rm -rf "$TMP_TEST_DIR"' EXIT

for pair in \
  "ttt:games/ttt/src/game.rs" \
  "g2048:games/g2048/src/game.rs" \
  "jewels:games/jewels/src/game.rs" \
  "zuma:games/zuma/src/game.rs" \
  "birds:games/birds/src/game.rs" \
  "chess:games/chess/src/game.rs" \
  "checkers:games/checkers/src/game.rs" \
  "sudoku:games/sudoku/src/game.rs" \
  "gomoku:games/gomoku/src/game.rs" \
  "cybermaze:games/cybermaze/src/game.rs" \
  "sokoban3d:games/sokoban3d/src/game.rs"
do
  name="${pair%%:*}"
  src="${pair#*:}"
  echo "  -> testing $name ($src)"
  rustc --edition 2024 --test "$src" -O -o "$TMP_TEST_DIR/test_$name"
  "$TMP_TEST_DIR/test_$name" --quiet
done

echo "  -> все гаразд: усі рушії пройшли хост-тести"
