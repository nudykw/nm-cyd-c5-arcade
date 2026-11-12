# NM-CYD-C5 Arcade — GameLauncher + 5 ігор на Rust + microSD + спільна бібліотека

Єдиний Cargo-workspace для плати **NM-CYD-C5** (`ESP32-C5HR8`, 16 MB Flash, 8 MB PSRAM, 2.8" TFT
320×240 **ST7789**, резистивний тач **XPT2046**, слот **microSD** на спільній шині SPI2).

Усі 5 раніше створених для цієї плати ігор перенесені сюди, переведені на спільну бібліотеку
`cyd-c5-common`, забезпечені кнопкою **`EXIT`** (повернення в лаунчер), кнопкою **`DEMO`**
(для всіх ігор, що підтримують демо-режим) і збереженням рекордів на microSD.

---

## Структура проєкту

```text
nm-cyd-c5-arcade/
├── Cargo.toml                 # Cargo workspace (спільний target/ і залежності)
├── partitions.csv             # Розмітка 16 MB флеша: factory (launcher), ota_0, ota_1, otadata, sdseed
├── crates/
│   └── cyd-c5-common/         # Спільна бібліотека плати (SPI2+DMA, ST7789, XPT2046, SD, FB, UI, OTA)
├── launcher/                  # GameLauncher (прошивається у розділ `factory` за 0x10000)
├── games/
│   ├── ttt/                   # Tic-Tac-Toe (3×3 + мінімакс-ШІ)           → SD:/GAMES/TTT.BIN
│   ├── g2048/                 # 2048 (свайпи, стрілки, анімація, бот)    → SD:/GAMES/G2048.BIN
│   ├── jewels/                # Jewels (match-3 8×8, каскади, бот)       → SD:/GAMES/JEWELS.BIN
│   ├── zuma/                  # Zuma (3 храмові траси, комбо, бонуси)    → SD:/GAMES/ZUMA.BIN
│   └── birds/                 # Angry Birds (рогатка, фізика блоків, бот)→ SD:/GAMES/BIRDS.BIN
├── dist/
│   ├── cyd-launcher.bin       # Образ лаунчера
│   ├── sdseed.bin             # Пакет SDPK для автоматичної розпаковки ігор на microSD по USB
│   └── sd/GAMES/*.BIN         # Готове дерево файлів для прямого копіювання на microSD з ПК
└── tools/
    ├── build-all.sh           # Прогін хост-тестів + збірка всіх крейтів + експорт .BIN + sdseed.bin
    ├── pack-sd.py             # Пакувальник dist/sd/GAMES/*.BIN у dist/sdseed.bin з CRC32
    └── flash-all.sh           # Повна прошивка лаунчера й інсталяція всіх ігор на microSD плати
```

---

## Як це працює (архітектура SD → OTA → Exit)

1. **Таблиця розділів (`partitions.csv`, 16 MB Flash)**:
   - `0x009000` (`20 KB`) — `nvs`
   - `0x00E000` (`8 KB`)  — `otadata` (вибір активного app-розділу для ESP-IDF 2nd-stage bootloader)
   - `0x010000` (`1 MB`)  — `factory`: **`cyd-launcher`** (GameLauncher)
   - `0x110000` (`1 MB`)  — `ota_0`: слот виконання вибраної гри
   - `0x210000` (`1 MB`)  — `ota_1`: резервний другий OTA-слот
   - `0x310000` (`2 MB`)  — `sdseed`: транспортний/резервний пакет `SDPK` для запису `.BIN` на microSD

2. **Зберігання ігор і рекордів на microSD**:
   - Образи всіх ігор лежать на картці пам'яті у каталозі **`/GAMES/*.BIN`** (`TTT.BIN`, `G2048.BIN`,
     `JEWELS.BIN`, `ZUMA.BIN`, `BIRDS.BIN`, формат 8.3 uppercase).
   - Рекорди зберігаються у **`/SAVES/*.SAV`** у текстовому вигляді й автоматично підтягуються як у
     самій грі, так і в картці гри всередині **GameLauncher**.
   - Якщо скопіювати у `/GAMES/` будь-який новий ESP32-C5 образ `*.BIN` (зібраний `espflash save-image`),
     лаунчер автоматично виявить його, витягне назву з `EspAppDesc` і додасть до меню.
   - Драйвер `cyd_c5_common::sd` прозоро підтримує картки з **MBR**, **GPT** (macOS Disk Utility:
     EFI + FAT32) та **Superfloppy** (FAT16/FAT32 без MBR).

3. **Запуск гри та кешування (`ota_0`)**:
   - Коли користувач натискає **`PLAY`** або **`DEMO`** у лаунчері (чи двічі тапає по рядку гри),
     лаунчер перевіряє штамп `LaunchStamp` в останньому секторі `ota_0` (`0x20F000`).
   - Якщо ця ж гра вже завантажена в `ota_0` (кеш-хіт за ім'ям, розміром і CRC заголовка), повторне
     стирання/запис флеша **не виконується** — перехід у гру займає ~0.6 с і не зношує флеш.
   - Якщо обрано іншу гру, лаунчер стримить `/GAMES/<NAME>.BIN` із microSD по 4 КБ у `ota_0` із
     прогрес-баром на екрані, активує `Ota0` в `otadata` і виконує `software_reset()`.
   - Кожна гра отримує у своє розпорядження **100 % внутрішньої SRAM** чипа без жодних компромісів.

4. **Повернення в лаунчер (`EXIT`)**:
   - У кожній із 5 ігор у правому нижньому куті є червона кнопка **`EXIT`**.
   - Натискання **`EXIT`** зберігає поточний рекорд у `/SAVES/<GAME>.SAV`, очищає `otadata` на
     `factory` (`cyd_c5_common::boot::exit_to_launcher`) і перезавантажує чип прямо в **GameLauncher**
     (~130 мс).

---

## Кнопки в іграх

| Гра | Файл на SD | Демо-режим | Кнопки на екрані | Рекорд на SD |
|---|---|---|---|---|
| **Tic-Tac-Toe** | `TTT.BIN` | ні (`AI:HARD` / `AI:EASY` / `2 PLAYR`) | `NEW`, `AI:HARD/EASY/2P`, **`EXIT`** | `/SAVES/TTT.SAV` |
| **2048** | `G2048.BIN` | так (евристичний бот «змійка») | стрілки `▲▼◀▶`, `NEW`, **`DEMO`**, **`EXIT`** | `/SAVES/G2048.SAV` |
| **Jewels** | `JEWELS.BIN` | так (`find_valid_move`) | `NEW`, **`DEMO`**, **`EXIT`** | `/SAVES/JEWELS.SAV` |
| **Zuma** | `ZUMA.BIN` | так (`find_demo_shot` з променем) | `SWAP`, `PAUS/PLAY`, `NEW`, **`DEMO`**, **`EXIT`** | `/SAVES/ZUMA.SAV` |
| **Angry Birds** | `BIRDS.BIN` | так (симуляція траєкторій `find_demo_shot`) | `RTRY`, `NEW`, `PAUSE/PLAY`, **`DEMO`**, **`EXIT`** | `/SAVES/BIRDS.SAV` |

Усі 4 ігри з підтримкою демо можна запустити **одразу в режимі DEMO** кнопкою `DEMO` у самому
GameLauncher або увімкнути/вимкнути на льоту кнопкою `DEMO` під час гри.

---

## Спільна бібліотека `crates/cyd-c5-common`

Винесений спільний код плати NM-CYD-C5, який перевикористовують лаунчер і всі 5 ігор:

- **`board`** — одновикликова ініціалізація `Board::init()`: 240 MHz CPU, безпечне підняття всіх
  трьох CS (`LCD=IO23`, `TOUCH=IO1`, `SD=IO10`) у `HIGH` до старту SPI2, `ST7789` (`Deg270`),
  підсвітка `IO25`, `Rng`, `FlashStorage`, `UART0`.
- **`spi`** — `SharedSpiBus` + `SharedSpiDevice`: спільна шина `SPI2 + DMA_CH0` із автоматичним
  перемиканням такту між дисплеєм (**40 MHz**), тачем (**2.5 MHz**) і microSD (**400 kHz → 16 MHz**)
  та утриманням `MOSI=0xFF` під час читання з SD.
- **`fb`** — статичний фреймбуфер `320×240 Rgb565` у внутрішній SRAM + реалізація `DrawTarget` і
  діагностичний `dump_frame_hex()`.
- **`touch`** — драйвер `XPT2046` з медіанним фільтром по 3 вимірах та афінним калібруванням.
- **`ui`** — спільна палітра, шрифти, примітиви (`fill`, `fill_rounded`, `stroke_rect`, `blend_fill`),
  стандартні кнопки `draw_btn`, `draw_demo_btn`, `draw_exit_btn`, форматування чисел без алокацій.
- **`sd`** — монтування microSD (MBR / GPT / Superfloppy), робота з `/GAMES/*.BIN` і `/SAVES/*.SAV`.
- **`boot`** — читання/запис штампа `ota_0`, потокове завантаження `.BIN` із SD у флеш, розпаковка
  `sdseed` на SD із перевіркою CRC32 та `exit_to_launcher()`.

---

## Збірка, тести й прошивка

Повна перевірка (хост-тести всіх 5 рушіїв + збірка `--release` + генерація `dist/sd/GAMES/*.BIN`):

```bash
cd ~/Projects/Esp32/nm-cyd-c5-arcade
bash tools/build-all.sh
```

Прошивка плати **разом з автоматичним записом усіх ігор на вставлену в плату microSD**:

```bash
cd ~/Projects/Esp32/nm-cyd-c5-arcade
bash tools/flash-all.sh /dev/ttyUSB0
```

Якщо ж потрібно оновити ігри напряму через кардрідер на комп'ютері — достатньо скопіювати вміст
`dist/sd/GAMES/*.BIN` у теки `/GAMES/` на FAT16/FAT32 microSD. Також якщо вставити у плату чисту
відформатовану microSD, у лаунчері з'явиться кнопка **`INSTALL SD`**, яка відновить усі 5 ігор із
резервного розділу `sdseed` без комп'ютера.

### Керування та живий скріншот через UART-консоль (`115200`, `/dev/ttyUSB0`)

- **Живий знімок екрана без перезавантаження** у будь-який момент із лаунчера або будь-якої гри:
  ```bash
  python3 tools/screenshot.py /tmp/screen.png /dev/ttyUSB0
  ```
  (надсилає символ `?` у UART0, приймає `SNAPBEGIN 320 240`…`SNAPEND` і зберігає PNG).
- У **GameLauncher**: `PLAY <NAME.BIN>`, `DEMO <NAME.BIN>`, `RESCAN`, `CLEARSAVES`.
- У **будь-якій грі**: клавіша `X` або `Q` — вихід у GameLauncher (еквівалент кнопки `EXIT`),
  `D` — перемикання `DEMO` (у `TTT`: `1..9` — хід у клітинку, `A` — зміна режиму ШІ, `N` — нова гра).
