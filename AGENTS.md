# AGENTS.md — `nm-cyd-c5-arcade` (Rust, ESP32-C5)

Інструкція для агентів (і людей), які змінюють цей репозиторій. Опис проєкту для користувача —
[`README.md`](README.md); залізо й розпіновка — [`docs/HARDWARE.md`](docs/HARDWARE.md).

| | |
|---|---|
| що це | Cargo-workspace: **GameLauncher** у `factory` + **11 ігор**, що вантажаться з `SD:/GAMES/*.BIN` в `ota_0` |
| плата | **NM-CYD-C5** — ESP32-C5 (RV32IMAC, 240 MHz, **без FPU**), 16 MB флеш, 8 MB PSRAM, ST7789 320×240, XPT2046, microSD |
| порт | `/dev/ttyUSB0` (CH340). `ttyACM0` (native USB) на цій платі **не працює** |
| мова | документація й коментарі — **українська**; ідентифікатори коду — англійська. Інші мови не додавати |

---

## 1. Встановлення інструментів (з нуля)

Потрібні **лише** Rust-тулчейн і `espflash`. ESP-IDF, PlatformIO, Python-пакети — **не потрібні**
(`python3` використовується тільки для stdlib-скриптів у `tools/`).

```bash
# Arch Linux ARM64
sudo pacman -S --needed base-devel git python3 espflash

# Rust (якщо ще немає rustup)
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y
rustup target add riscv32imac-unknown-none-elf   # ціль ESP32-C5
rustup component add rust-src                    # потрібен для build-std (core)

# доступ до послідовного порту
sudo usermod -aG uucp "$USER"                    # після цього — релогін
```

Варіанти `espflash`: `pacman -S espflash` (швидко) або `cargo install espflash --locked` (свіжіше,
~5–10 хв збірки). Перевірка: `espflash --version` (у проєкті перевірено на 4.6–4.7).

Перевірка готовності середовища:

```bash
cargo --version                                    # 1.9x
rustc --version
rustup target list --installed | grep riscv32imac  # має бути riscv32imac-unknown-none-elf
espflash --version
python3 --version
```

### Якщо інструментів немає зовсім

- **мережа/дзеркала**: `sudo pacman -Sy` перед встановленням;
- **rustup не ставиться**: пакет `rust` + `rust-src` з Arch теж працює, але тоді target треба
  доставляти `rustup`-ом — тому rustup краще;
- **`espflash` падає на `libudev`**: `sudo pacman -S systemd-libs` (udev потрібен для пошуку портів),
  або завжди передавайте `--port /dev/ttyUSB0` явно.

---

## 2. Збірка, тести, прошивка

Хост-тести рушіїв окремо (так само робить CI): `bash tools/test-host.sh`.

```bash
cd nm-cyd-c5-arcade

bash tools/build-all.sh                # 1) хост-тести рушіїв → 2) cargo build --release --workspace
                                       # 3) dist/sd/GAMES/*.BIN → 4) dist/sdseed.bin   (~1–3 хв)
bash tools/flash-all.sh /dev/ttyUSB0            # build-all + лаунчер у factory + пакет ігор у sdseed
bash tools/flash-all.sh /dev/ttyUSB0 --no-seed  # тільки лаунчер, /GAMES на картці не оновлюється
cargo build --release -p cyd-game-chess    # один крейт
```

Що робить `flash-all.sh` (те саме руками, якщо скрипт недоступний):

```bash
espflash flash --non-interactive --chip esp32c5 --port /dev/ttyUSB0 --flash-size 16mb \
  --partition-table partitions.csv --target-app-partition factory \
  --erase-parts otadata target/riscv32imac-unknown-none-elf/release/cyd-launcher
espflash write-bin --chip esp32c5 --port /dev/ttyUSB0 0x310000 dist/sdseed.bin
```

- **Перед прошивкою** перевірити, що порт вільний: `fuser -v /dev/ttyUSB0` (інакше
  `Unable to acquire exclusive lock`).
- **Одна гра без лаунчера** (швидка ітерація): прошити крейт гри в `factory` — плата стартує одразу з
  неї; повернути лаунчер = `bash tools/flash-all.sh`.
- інсталяція ігор **інкрементальна**: `boot::install_sdpack_to_sd()` спершу звіряє розмір+CRC32
  файлу на картці й пише лише те, що відрізняється (`sd.game_matches`), а заміна наявного файлу йде
  через `<NAME>.TMP` + перевірку CRC перед підміною;
- `otadata` вміє скидати лише `--erase-parts otadata`; `espflash` не вміє активувати OTA-слот — це
  робить лаунчер (`crates/cyd-c5-common/src/boot.rs`).

---

## 3. Перевірка на залізі (без підключення дисплея до очей)

UART 115200 на `/dev/ttyUSB0`. Знімок екрана без перезавантаження:

```bash
python3 tools/screenshot.py /tmp/s.png /dev/ttyUSB0     # ~20–30 с: кадр друкується hex-дампом
```

Команди (детальніше — README §6):

```bash
# лаунчер
printf 'PLAY CHESS.BIN\n' > /dev/ttyUSB0
printf 'DOWN\n' > /dev/ttyUSB0
printf 'FORMAT\n' > /dev/ttyUSB0     # переформатувати microSD у FAT32 (+ автоінсталяція ігор)

# усередині гри
printf 'X' > /dev/ttyUSB0        # EXIT → лаунчер
printf 'D' > /dev/ttyUSB0        # DEMO on/off
printf '?' > /dev/ttyUSB0        # знімок екрана
```

Ознаки здорової прошивки: у UART іде лог `... selftest N/N failed=0` і `старт, кадр = XX ms`.
Тиша + `alive N` на `ttyACM0` = `esp-println` зібраний з `auto` (має бути `uart`).

---

## 4. Інваріанти (ламати — лише свідомо)

1. **`games/*/src/game.rs` — чиста логіка**: без `std`, `alloc`, `esp-hal`. Саме тому
   `tools/build-all.sh` компілює кожен рушій на хості (`rustc --test`) і ганяє юніт-тести. Додав
   `esp_hal::` у `game.rs` — зламав усі хост-тести.
2. **Залізо й малювання — тільки в `src/bin/main.rs`.** Спільні примітиви — `cyd-c5-common::ui`.
3. **Зміна API `cyd-c5-common` перезбирає лаунчер і всі 11 ігор** (спільний `target/`) — перевіряти
   `cargo build --release --workspace`, а не один крейт.
4. **`esp-println` — тільки `uart`** (`default-features = false, features = ["esp32c5","uart","critical-section"]`).
   З `auto` прошивка вішається на першому `println!` (на платі два USB-порти).
5. **Шрифти 1 bpp, `radius 0`** — 4 bpp + заокруглення дають AA-«пил» на панелі.
6. **1 MB на app-слот** (`factory`, `ota_0`, `ota_1`): стежити за розміром `*.BIN` (`espflash save-image`
   друкує його).
7. **`partitions.csv`: кастомний data-розділ — лише іменований subtype** (`spiffs`), не `0x99`:
   `espflash` панікує на числовому hex.
8. **Калібрування тача прив'язане до `MADCTL 0xA0`**: змінив орієнтацію дисплея — калібруй тач заново.
9. **Перед стартом SPI2 усі три CS (`IO23`, `IO1`, `IO10`) — у `HIGH`**, при читанні з SD
   тримається `MOSI=0xFF`.
10. **SPI дисплея — 40 MHz.** Вище — поодинокі чужі пікселі на однотонних ділянках.
11. **3D-ігри — без `f32` у гарячих циклах**: у RV32IMAC немає FPU, тому тільки Q16.16 і таблиці
    синусів (`cybermaze`, `sokoban3d`).

---

## 5. Розкладка

```text
crates/cyd-c5-common/src/   board (Board::init), spi (SharedSpiBus/Device), fb (320×240 Rgb565),
                            touch (XPT2046+калібрування), ui (палітра, 1bpp шрифти, кнопки),
                            sd (MBR/GPT/Superfloppy, /GAMES, /SAVES, format_fat32 для відновлення картки), boot (OTA, sdseed, exit)
launcher/src/bin/main.rs    GameLauncher: меню, рекорди, PLAY/DEMO, прокрутка списку, INSTALL SD
games/<name>/src/game.rs    правила гри — no_std, без залежностей (+ #[test] усередині)
games/<name>/src/bin/main.rs малювання/ввід на esp-hal, кнопки EXIT/DEMO, UART
games/sokoban3d/src/game.rs 50 рівнів XSokoban у константі LEVELS (формат XSB)
partitions.csv              16 MB: nvs 0x9000 · otadata 0xE000 · factory 0x10000 (1 MB) ·
                            ota_0 0x110000 · ota_1 0x210000 (по 1 MB) · sdseed 0x310000 (2 MB)
dist/                       генерується build-all.sh (у .gitignore)
tools/                      build-all.sh, flash-all.sh, test-host.sh, pack-sd.py,
                            screenshot.py (знімок екрана), make-gif.py (GIF зі stdlib)
```

---

## 6. Додати нову гру (чек-ліст)

1. `games/<name>/` за зразком сусідньої: `Cargo.toml` (`name`/`[[bin]]`), `build.rs` (`-Tlinkall.x`),
   `src/lib.rs` (`#![no_std] pub mod game;`), `src/game.rs` (логіка + тести), `src/bin/main.rs`
   (залізо, `EXIT`, `DEMO`, UART `X`/`Q`/`D`).
2. Додати крейт у `[workspace] members` (кореневий `Cargo.toml`).
3. Додати пару `"<name>:games/<name>/src/game.rs"` (тести) і `"cyd-game-<name>:<NAME>.BIN"` (експорт)
   у `tools/build-all.sh`; ім'я файлу — у `tools/pack-sd.py` (порядок у `order`).
4. Запис у `KNOWN_GAMES` лаунчера: `bin_name`, `save_name`, `title`, `genre`, `supports_demo`, `order`,
   `icon_id` + піксель-арт 28×28 у `draw_game_icon`.
5. Рекорди: `board.save_best("<NAME>", …)` / `board.load_best("<NAME>")` → `SD:/SAVES/<NAME>.SAV`.
6. `bash tools/build-all.sh` (мають пройти хост-тести) → `bash tools/flash-all.sh /dev/ttyUSB0` →
   `python3 tools/screenshot.py /tmp/s.png /dev/ttyUSB0`.

---

## 7. Чого не робити

- не комітити `target/` і `dist/` (вони в `.gitignore`; після `build-all.sh` `git status --short`
  має лишатися порожнім);
- не правити `dist/*.bin` руками — вони генеруються;
- не міняти `partitions.csv` без синхронної правки `0x310000` у `tools/flash-all.sh` і констант у
  `boot.rs`;
- не додавати `std`/`alloc`/`Vec` у код, що йде у 1 MB слот;
- не прошивати навмання: плата одна, на ній тримається робочий стенд (перед `flash-all.sh` —
  `fuser -v /dev/ttyUSB0`);
- не змішувати мови в документації: тільки українська та англійська.

---

## 8. Навігація по коду (економія токенів)

~15 тисяч рядків Rust не читати цілком. Порядок:

1. `lsp_symbols <file>` — структура файлу;
2. `lsp_definition` / `lsp_references` — де символ і хто його викликає;
3. `read` з `offset`/`limit` навколо потрібного рядка;
4. `code-index find NAME` — пошук символу по всьому дереву;
5. `ast-grep run -p 'PATTERN' -l rust games/` — структурний пошук.

`rust-analyzer` для цього воркспейсу стартує з кореня (`Cargo.toml`), `clangd` тут не потрібен.
