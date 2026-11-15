# Wi-Fi на NM-CYD-C5 (ESP32-C5): 2.4 + 5 ГГц у GameLauncher

Стек живе **тільки в лаунчері** (`crates/cyd-c5-wifi`, підключений лише з `launcher/`). Ігри
залишаються маленькими: `esp-radio` в них не компонується.

| | |
|---|---|
| драйвер | `esp-radio` **1.0.0-beta.1** (features `esp32c5`, `wifi`, `unstable`) |
| планувальник | `esp-rtos` **0.4.0** (features `esp32c5`, `esp-radio`, `esp-alloc`, `embassy`) |
| TCP/IP | `embassy-net` **0.9** (`dhcpv4`, `udp`, `tcp`, `dns`, `medium-ethernet`) |
| купа | `esp-alloc` 0.11: 64 KB reclaimed + 36 KB DRAM (у `net::init_heap`) |
| екран | `crates/cyd-c5-wifi/src/ui.rs` (спільні примітиви — `cyd-c5-common::ui`) |

---

## 1. Навіщо окремий крейт

`crates/cyd-c5-common` — спільна бібліотека, яку тягнуть **усі 13 ігор**. Якби `esp-radio` лежав
там, кожна гра виросла б на ~500 KB і отримала б 100 KB купи в RAM. Тому:

- `cyd-c5-wifi` залежить від `cyd-c5-common`, але не навпаки;
- `Board::init()` лише **віддає** периферію радіо через `Board::take_radio()` (поля `wifi`,
  `timg0`, `intr0`) — ігри її просто ігнорують.

## 2. Потік керування

```text
main (async, #[esp_rtos::main])
 ├─ cyd_c5_wifi::init_heap()                  купа до першої алокації Wi-Fi
 ├─ Board::init()                             дисплей/тач/SD/UART (як і в іграх)
 ├─ esp_rtos::start(timg0.timer0, INTR0)      планувальник для радіо-блобів
 ├─ cyd_c5_wifi::start(spawner, WIFI, saved) ─┐ спавнить worker
 └─ loop { UI; Touch; UART; Timer(14ms).await }
                                             │
worker (окремий таск того ж executor-а) ◄────┘
 ├─ WifiController::new(WIFI, country=UA)     BandMode::Auto → 2.4 + 5 ГГц
 ├─ Interface::station() + embassy_net::new(dhcpv4) + net_task
 └─ loop { CMD.try_receive() → scan / connect / disconnect;  кожні 300 мс — link/IP/RSSI }
```

Спільний стан — **лише** через статики під `critical_section` (`STATUS`, `APS`) і канал команд
(`CMD`, `embassy_sync::channel`). `WifiController` — єдиний власник, тому жодних блокувань між
UI та драйвером немає; UI ніколи не чекає на радіо.

**Важливо:** головний цикл лаунчера мусить віддавати квант (`Timer::after(...).await`). Якщо
замінити це на `busy-wait`, worker не виконається і скан/підключення «зависнуть».

## 3. Діапазони та країна

- `ControllerConfig::default()` на C5 має `BandMode::Auto`, тобто скан іде і по 2.4, і по 5 ГГц;
  `Protocols::default()` — `B/G/N` для 2.4 і `A/AC/AX` для 5.
- Країна — `UA`: `wifi_country_t { schan: 1, nchan: 13, wifi_5g_channel_mask: 0 }`. Маска 0 означає
  «усі 5-ГГц канали за місцевими правилами» (перевірено: видно AP на ch 52 і ch 157).
- Вибір у списку передає **канал** у `StationConfig::with_channel(ch)`, тому підключення не
  «переповзає» на інший діапазон тієї ж SSID. Саме тому одна мережа може бути у списку двічі.
- SSID з не-ASCII (напр. емодзі) зберігаються байтами як є (у `WSTATUS` видно справжній рядок),
  але на екрані шрифт 1 bpp ASCII показує `???`.

## 4. Пароль і збереження

`SD:/WIFI.CFG`, відкритий текст:

```text
SSID=MyNet
PASS=secret123
```

- **Пароль завжди вводить користувач на екранній клавіатурі.** Тап по закритій мережі відкриває
  клавіатуру з порожнім полем; підключення починається лише після `OK`. Ні автопідключення на
  старті, ні підстановки збереженого пароля немає (`WifiScreen::join_selected`).
- Запис — **лише після** `StationConnected` + DHCP (`WifiScreen::tick` → `take_saved_update`);
  невдала спроба не залишає «сміття» на картці.
- Читання — на старті (`board.sd.load_wifi_cfg()`): мережа позначається `*` у списку та в рядку
  статусу, `FORGET` / `WFORGET` видаляє файл. Тобто файл — це «пам'ять про мережу», а не автологін.
- Формат навмисно простий: файл можна створити на ПК і покласти на картку.

## 5. Час і помилки

| подія | поведінка |
|---|---|
| `connect_async` | таймаут **15 с** → `FAILED` + причина (укр. текст у лог, англ. на екрані) |
| DHCP | до **20 с** (200 мс × 100) → `CONNECTED, NO IP (DHCP)` |
| втрата зв'язку | фаза `CONNECTING`, автоперепідключення збереженими даними |
| причини | `NoAccessPointFound` → `NETWORK NOT FOUND`; handshake/PMKID/MIC → `AUTH/WRONG PASSWORD` |
| скан | `ScanConfig::max(32)`, у список потрапляє до 24 BSSID, сортування за RSSI |

`WSTATUS` друкує фазу, SSID, канал, IP, RSSI, помилку, **використання купи** (`esp_alloc::HEAP`)
і список мереж — цього достатньо для діагностики без екрана.

## 6. Пам'ять і розмір

| | значення |
|---|---|
| образ лаунчера | **~765 KB** із 1 048 576 B (`factory`) — 73 % |
| купа | 64 KB `#[ram(reclaimed)]` + 36 KB `.bss` |
| `.stack` (вільна DRAM після статики) | ~21 KB |
| `.bss` лаунчера | ~246 KB (фреймбуфер 150 KB + LCD-буфер 16 KB + статики Wi-Fi/стека) |

Наслідки:

- нова залежність у лаунчері → перевірити `espflash save-image --chip esp32c5 --flash-size 16mb
  --partition-table partitions.csv --target-app-partition factory …` (мусить бути < 1 MB);
- якщо RAM стане тісно — зменшувати другий `heap_allocator!` (36 KB), а не перший
  (`reclaimed` живе поза `.stack`);
- PSRAM (8 MB) не використовується: блобам Wi-Fi потрібна внутрішня DRAM.

## 7. Діагностика

```bash
printf 'WSTATUS\n' > /dev/ttyUSB0        # стан + мережі + heap
printf 'WKB 0\n'    > /dev/ttyUSB0        # екранна клавіатура для мережі №0 (діагностика без тачу)
printf 'WTAP 100 70\n' > /dev/ttyUSB0      # прогнати тап по координатах (діагностика без пальця)
printf 'WSCAN\n'   > /dev/ttyUSB0        # пересканувати
printf 'WJOIN Net|pass\n' > /dev/ttyUSB0 # підключитись
python3 tools/screenshot.py /tmp/w.png /dev/ttyUSB0   # побачити екран
```

Лог у UART:

```text
WIFI: init esp-radio (band mode auto = 2.4 + 5 GHz, country UA)
WIFI: ready (2.4 + 5 GHz, чекає команду)
WIFI: scan done, 21 AP
WIFI: connect to "MyNet" (pass 9 chars, channel Some(52))
WIFI: associated "MyNet" ch=52 Wpa2Personal
WIFI: IP 192.168.1.57
```

Типові проблеми:

| симптом | причина / що робити |
|---|---|
| `WIFI ERR` у шапці, у лог `init failed` | радіо не піднялося (мало RAM / не викликано `esp_rtos::start` **до** `WifiController::new`) |
| `AUTH/WRONG PASSWORD` для правильної мережі | WPA3-only AP: блоб підтримує WPA2/WPA3-mixed; для чистого WPA3 потрібен інший `AuthenticationMethodConfig` (див. `net::connect`) |
| `CONNECTED, NO IP` | DHCP не відповів: перевірити, що AP справді роздає адреси; статична адреса не реалізована |
| 5-ГГц мережі не видно | AP на DFS-каналі, який заборонений для обраної країни; або відстань (5 ГГц «коротший») |
| лаунчер «завмер» після підключення | хтось замінив `Timer::after(...).await` на busy-wait у головному циклі |

## 8. Оновлення з GitHub Releases

```text
CHECK:  GET https://github.com/<repo>/releases/latest/download/MANIFEST.TXT
        (TLS 1.3 + перевірка ланцюжка; SNTP — якщо мережа не блокує UDP/123)
        → version=…, launcher/game <ім'я> <розмір> <crc32>
        → порівняння: ігри — CRC файла на SD, лаунчер — версія з Cargo.toml
GAMES:  для кожного застарілого: GET …/download/<NAME> → staging `dl` (2 MB)
        → перевірка: розмір, CRC32, ESP-заголовок (0xE9), project_name
        → SD:/GAMES/<NAME>.BIN (write_game_bin + контрольний CRC)
LAUNCHER: GET …/download/cyd-launcher.bin → `dl` → перевірка → інший OTA-слот → otadata → reset
```

- Маніфест генерує `tools/make-manifest.py` (у `tools/build-all.sh` це крок після експорту
  `.BIN`); публікується workflow `.github/workflows/release.yml` на тег `v*`.
- `dl` — не «ще один слот», а staging: невдале завантаження ніколи не псує ні SD, ні слоти.
- Лаунчер: `factory` (USB) + `ota_1`/`ota_2` (A/B). Пишемо **завжди в інший** слот, тому
  поточна система залишається робочою навіть якщо новий образ не запуститься (можна повернутися,
  перезаливши `factory` по USB). `EXIT` з гри веде в слот, записаний у штампі `ota_0`.
- TLS: корені піновані (`src/ca.rs`). GitHub зараз віддає Sectigo (github.com/api) і
  Let's Encrypt (CDN). Якщо CA зміниться — `TLS FAILED (CERT?)`, треба оновити `ca.rs`.
- `UURL <owner/repo> <asset>` качає довільний ассет у `dl` і друкує розмір/CRC32 — швидка
  перевірка TLS/редіректів без релізу проєкту.

Типові проблеми:

| симптом | причина |
|---|---|
| `HTTP STATUS` у `USTATUS`, у лозі `404` | у репозиторії немає релізу з `MANIFEST.TXT` |
| `MANIFEST INVALID` | файл не за форматом (рядки `version=`, `launcher …`, `game …`) |
| `IMAGE CHECK FAILED` | розмір/CRC/заголовок не збіглися (обірване або чуже завантаження) |
| `SD WRITE FAILED` | microSD відвалилася під час запису (див. README §10) |
| `LAUNCHER INSTALL FAILED` | не вдалося активувати слот (перевірити `partitions.csv`/otadata) |
| `NETWORK TIMEOUT` | слабкий сигнал або мережа ріже 443 |

## 9. Чого не робити

- не викликати `esp_rtos::start()` вдруге (другий виклик — паніка/невизначена поведінка);
- не переносити `esp-radio` у `cyd-c5-common` (ігри виростуть і отримають зайву RAM);
- не звертатися до `WifiController` з UI: тільки `net::request_*`;
- не тримати `WifiController` у `static` без синхронізації — його тримає лише `worker`;
- не зменшувати `heap_allocator!(#[ram(reclaimed)] …)` нижче 64 KB: блобам бракує буферів
  (`static_rx_buf_num = 10`, `dynamic_rx_buf_num = 32` з `ControllerConfig::default()`);
- не збирати лаунчер без фічі `psram-fb`: TLS-стеку не вистачить стека (перевірено — overflow);
- не перезаписувати `factory` по Wi-Fi: тільки `ota_1`/`ota_2`.
