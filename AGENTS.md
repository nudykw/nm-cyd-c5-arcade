# AGENTS.md — `nm-cyd-c5-arcade` (Rust, ESP32-C5)

Для агента. **Що це за проєкт, архітектура SD → OTA → Exit, розмітка флеша, кнопки й UART-команди —
у [`README.md`](README.md) (142 рядки).** Тут — те, що потрібно, щоб *правильно змінювати код*, і де
шукати решту, щоб не вигадувати.

| | |
|---|---|
| плата | **NM-CYD-C5** — `ESP32-C5HR8`, 16 MB flash, 8 MB PSRAM, 2.8" ST7789 320×240, XPT2046, microSD на спільній SPI2 |
| що це | Cargo-workspace: `cyd-launcher` у `factory` + 5 ігор, що вантажаться з `SD:/GAMES/*.BIN` в `ota_0` |
| код | ~10 700 рядків Rust (з них `game.rs` — 3 700 чистої логіки, `bin/main.rs` — малювання) |
| **git** | репозиторій з 2026-11-12, гілка `main`; `target/`, `dist/` — у `.gitignore`; ідентичність — `nudyk <nudykw@gmail.com>` (у `~/.gitconfig` і локально) |
| збірка | `bash tools/build-all.sh`; прошивка — `bash tools/flash-all.sh /dev/ttyUSB0` |
| тулчейн | `rust-toolchain.toml` → `stable` + target `riscv32imac-unknown-none-elf`; Xtensa-обгортки (`xtensa-env`) **не потрібні** |

## Де що лежить (джерела істини)

| питання | файл |
|---|---|
| як працює лаунчер, розділи флеша, UART-команди, кнопки | `README.md` |
| піни, SPI, вендорські `User_Setup`, схема плати | `../docs/nm-cyd-c5/PINOUT.md` |
| **яка плата зараз на порту** (MAC, флеш, що залито) | `../CURRENT-BOARD.md` |
| історія всіх плат | `../BOARDS.md` |
| порт/права/`esptool`/`espflash`, ловушки ESP32-тулчейнів | `../AGENTS.md` §4–§5 |
| сама машина (Pi 5, Arch, I2C, ядро) | `~/AGENTS.md` |
| спільний код плати | `crates/cyd-c5-common/src/{board,spi,fb,touch,ui,sd,boot,crc}.rs` |

## Розкладка

```text
crates/cyd-c5-common/src/   board (Board::init), spi (SharedSpiBus/Device), fb (320×240 Rgb565),
                            touch (XPT2046+калібрування), ui (палітра, 1bpp шрифти, кнопки),
                            sd (MBR/GPT/Superfloppy, /GAMES, /SAVES), boot (OTA, sdseed, exit_to_launcher)
launcher/src/bin/main.rs    GameLauncher (878 рядків): меню, рекорди, PLAY/DEMO, кеш ota_0
games/<name>/src/game.rs    ПРАВИЛА гри — `no_std`, без залежностей, без алокацій (+ `#[test]` усередині)
games/<name>/src/bin/main.rs малювання/ввід на esp-hal, кнопки EXIT/DEMO, UART
games/<name>/build.rs       лише `cargo:rustc-link-arg=-Tlinkall.x`
partitions.csv              16 MB: nvs 0x9000 · otadata 0xE000 · factory 0x10000 (1 MB) ·
                            ota_0 0x110000 · ota_1 0x210000 (по 1 MB) · sdseed 0x310000 (2 MB)
dist/                       cyd-launcher.bin, sdseed.bin, sd/GAMES/*.BIN (генерується, не руками)
tools/                      build-all.sh, flash-all.sh, pack-sd.py, screenshot.py
```

## Інваріанти (ламати — тільки свідомо)

1. **`games/*/src/game.rs` лишається чистою логікою**: без `std`, без `esp-hal`, без `alloc`. Це не стиль —
   саме тому `tools/build-all.sh` компілює кожен `game.rs` на хості `rustc --edition 2024 --test` і ганяє
   `#[test]`. Додав `esp_hal::` у `game.rs` — зламав усі 5 хост-тестів.
2. **Залізо й малювання — тільки в `src/bin/main.rs`.** Спільні примітиви — у `cyd-c5-common::ui`.
3. **Зміна API `cyd-c5-common` перезбирає лаунчер і всі 5 ігор** (workspace, спільний `target/`). Перевіряти
   `cargo build --release --workspace`, а не один крейт.
4. **`esp-println` — тільки `uart`**: `default-features = false, features = ["esp32c5","uart","critical-section"]`.
   З `auto` прошивка «зависає» на першому ж `println!`, бо на платі два USB-порти (ttyUSB0 + ttyACM0).
5. **Шрифти 1 bpp, `radius 0`.** 4 bpp + заокруглення дають ~120 кольорів AA-«пилу» на панелі.
6. **1 MB на app-слот** (`factory`, `ota_0`, `ota_1`). Стежити за розміром `*.BIN`; `espflash save-image`
   друкує його, а `data_len` сторожать `sd.rs`/`boot.rs`.
7. **`partitions.csv`: кастомний data-розділ — тільки іменований subtype** (`spiffs`), не `0x99`:
   `espflash`/`esp-idf-part` панікує на числовому hex у CSV.
8. **Калібрування тача прив'язане до розкладки дисплея** (`CALIB_DISPLAY_CFG`): змінив MADCTL/поворот —
   старе калібрування в NVS відкидається, треба калібрувати заново.
9. **Перед стартом SPI2 усі три CS (`LCD IO23`, `TOUCH IO1`, `SD IO10`) — у `HIGH`**, а при читанні з SD
   тримається `MOSI=0xFF` (інакше картка глушить MISO).
10. **SPI дисплея — 40 MHz.** Вище — поодинокі чужі пікселі на однотонних ділянках.

## Збірка, тести, прошивка

```bash
cd ~/Projects/Esp32/nm-cyd-c5-arcade
bash tools/build-all.sh              # хост-тести 5 рушіїв → cargo build --release --workspace
                                     # → dist/sd/GAMES/*.BIN → dist/sdseed.bin   (~2–4 хв з нуля)
bash tools/flash-all.sh /dev/ttyUSB0 # той самий build-all + прошивка factory + sdseed @0x310000
cargo build --release -p cyd-game-g2048   # один крейт
cargo run --release -p cyd-launcher       # runner з .cargo/config.toml сам шиє (espflash + partitions.csv)
```

Збірка не бруднить репозиторій: `dist/sd/GAMES/*.BIN` і `dist/*.bin` генеруються, але `dist/` і
`target/` у `.gitignore`, тож після `build-all.sh` `git status --short` лишається порожнім.

Перед прошивкою звірити `../CURRENT-BOARD.md` (що саме на `/dev/ttyUSB0`) і зайнятість порту
(`fuser -v /dev/ttyUSB0`). `ttyACM0` (native USB-JTAG) на цій платі **не працює** — тільки CH340.

## Додати нову гру (чек-ліст)

1. `games/<name>/` за зразком сусідньої: `Cargo.toml` (`name`/`[[bin]] name = "cyd-game-<name>"`),
   `build.rs` (`-Tlinkall.x`), `src/lib.rs` (`#![no_std]`, `pub mod game;`), `src/game.rs` (логіка + тести),
   `src/bin/main.rs` (залізо, `EXIT`, `DEMO`, UART `X`/`Q`/`D`).
2. Додати крейт у `[workspace] members` у кореневому `Cargo.toml`.
3. Додати пару `"<name>:games/<name>/src/game.rs"` (тести) і `"cyd-game-<name>:<NAME>.BIN"` (експорт)
   у `tools/build-all.sh`. Рекорди — `board.save_best("<NAME>", …)` / `load_best` → `SD:/SAVES/<NAME>.SAV`.
4. Назва в меню береться з `EspAppDesc.project_name` **всередині .BIN** — виставити її в крейті.
5. `bash tools/build-all.sh` (мають пройти хост-тести) → `bash tools/flash-all.sh /dev/ttyUSB0` →
   візуальна перевірка `python3 tools/screenshot.py /tmp/s.png /dev/ttyUSB0`.

## Перевірка на залізі

- UART 115200: лаунчер — `PLAY <NAME.BIN>`, `DEMO <NAME.BIN>`, `RESCAN`, `CLEARSAVES`; у грі — `X`/`Q` вихід,
  `D` демо, `?` живий знімок екрана.
- Екран без перезавантаження: `python3 tools/screenshot.py /tmp/screen.png /dev/ttyUSB0`.
- Прошивка не «зависла», якщо після ресету в UART іде лог лаунчера; тиша + `alive N` на `ttyACM0` — це
  випадок з `esp-println` (див. інваріант 4).

## Чого не робити

- історія починається **2026-11-12** (перший коміт — увесь код), давніших змін у ній немає; `git log`/`git diff` працюють нормально.
- не комітити `target/` і `dist/` — вони в `.gitignore`; перевірка після збірки: `git status --short` має лишитися порожнім.
- не комітити/видаляти `target/`, `dist/*.bin` (генеровані) — але `dist/sd/GAMES/*.BIN` перезаписуються
  `build-all.sh`, руками їх не правити.
- не міняти `partitions.csv` без правки адрес: `0x310000` у `tools/flash-all.sh` і константи в `boot.rs`.
- не додавати `std`/`alloc`/`Vec` у код, що йде у 1 MB слот.
- не прошивати навмання: плата одна, на ній тримається робочий стенд.

## Токени

10 700 рядків не читати цілком. Порядок: `lsp_symbols {file}` → `lsp_definition`/`lsp_references` →
`read` з `offset/limit` навколо потрібного рядка. `code-index find NAME` — пошук символу по всьому
`~/Projects/Esp32`; `ast-grep run -p 'PATTERN' -l rust games/` — структурний пошук. Скіл `code-navigation`.
