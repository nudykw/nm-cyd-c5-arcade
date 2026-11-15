//! Менеджер Wi-Fi для ESP32-C5: один фоновий таск `esp-rtos`, який виконує
//! сканування (2.4 + 5 ГГц), підключення, DHCP і публікує стан для UI лаунчера.
//!
//! Архітектура:
//! - `esp_rtos::start()` (викликає лаунчер) дає планувальник, потрібний радіо-блобам;
//! - `start()` спавнить [`worker`] — єдиний власник `WifiController` і DHCP-стеку;
//! - UI спілкується з ним через [`request_scan`] / [`request_connect`] / [`request_disconnect`]
//!   (канал команд) і читає знімок через [`status`] / [`aps`] (критична секція, без блокувань).

use core::cell::RefCell;

use cyd_c5_common::sd::WifiCfg;
use critical_section::Mutex;
use embassy_executor::Spawner;
use embassy_net::{Runner, Stack, StackResources};
use embassy_sync::{blocking_mutex::raw::CriticalSectionRawMutex, channel::Channel};
use embassy_time::{Duration, Timer, with_timeout};
use esp_hal::peripherals::WIFI;
use esp_radio::wifi::{
    AuthenticationMethod, AuthenticationMethodConfig, Config, ControllerConfig, DisconnectReason,
    Interface, Password, Ssid, WifiController,
    scan::ScanConfig,
    sta::StationConfig,
};
use static_cell::StaticCell;

/// Скільки мереж тримаємо в списку сканування.
pub const MAX_APS: usize = 24;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Band {
    /// 2.4 ГГц (канали 1..14).
    G24,
    /// 5 ГГц (канали 32..177).
    G5,
}

impl Band {
    pub fn label(self) -> &'static str {
        match self {
            Band::G24 => "2G",
            Band::G5 => "5G",
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Auth {
    Open,
    Wep,
    Wpa,
    Wpa2,
    Wpa3,
    Enterprise,
    Other,
}

impl Auth {
    pub fn label(self) -> &'static str {
        match self {
            Auth::Open => "OPEN",
            Auth::Wep => "WEP",
            Auth::Wpa => "WPA",
            Auth::Wpa2 => "WPA2",
            Auth::Wpa3 => "WPA3",
            Auth::Enterprise => "ENT",
            Auth::Other => "?",
        }
    }

    pub fn is_open(self) -> bool {
        matches!(self, Auth::Open)
    }
}

fn auth_of(m: Option<AuthenticationMethod>) -> Auth {
    match m {
        None | Some(AuthenticationMethod::None) => Auth::Open,
        Some(AuthenticationMethod::Wep) => Auth::Wep,
        Some(AuthenticationMethod::Wpa) => Auth::Wpa,
        Some(AuthenticationMethod::Wpa2Personal) => Auth::Wpa2,
        Some(AuthenticationMethod::WpaWpa2Personal) => Auth::Wpa2,
        Some(AuthenticationMethod::Wpa3Personal)
        | Some(AuthenticationMethod::Wpa2Wpa3Personal)
        | Some(AuthenticationMethod::Wpa3ExtPsk)
        | Some(AuthenticationMethod::Wpa3ExtPskMixed) => Auth::Wpa3,
        Some(AuthenticationMethod::Wpa2Enterprise)
        | Some(AuthenticationMethod::Wpa3Enterprise)
        | Some(AuthenticationMethod::Wpa2Wpa3Enterprise)
        | Some(AuthenticationMethod::WpaEnterprise)
        | Some(AuthenticationMethod::Wpa3EntSuiteB192Bit) => Auth::Enterprise,
        _ => Auth::Other,
    }
}

/// Одна точка доступу з результату сканування.
#[derive(Clone, Copy)]
pub struct Ap {
    ssid: [u8; 33],
    ssid_len: u8,
    pub bssid: [u8; 6],
    pub channel: u8,
    pub rssi: i8,
    pub auth: Auth,
}

impl Ap {
    pub const EMPTY: Ap = Ap {
        ssid: [0u8; 33],
        ssid_len: 0,
        bssid: [0u8; 6],
        channel: 0,
        rssi: 0,
        auth: Auth::Open,
    };

    pub fn ssid(&self) -> &str {
        core::str::from_utf8(&self.ssid[..self.ssid_len as usize]).unwrap_or("?")
    }

    pub fn ssid_bytes(&self) -> &[u8] {
        &self.ssid[..self.ssid_len as usize]
    }

    pub fn band(&self) -> Band {
        if self.channel > 14 { Band::G5 } else { Band::G24 }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Phase {
    /// Радіо ще не піднято.
    Off,
    /// Готово, не підключено.
    Idle,
    Scanning,
    Connecting,
    /// Канал Wi-Fi є; IP може ще не бути (`Status::has_ip`).
    Connected,
    Failed,
}

impl Phase {
    pub fn label(self) -> &'static str {
        match self {
            Phase::Off => "WI-FI OFF",
            Phase::Idle => "NOT CONNECTED",
            Phase::Scanning => "SCANNING...",
            Phase::Connecting => "CONNECTING...",
            Phase::Connected => "CONNECTED",
            Phase::Failed => "FAILED",
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Err {
    None,
    Init,
    Ssid,
    Password,
    NotFound,
    Auth,
    Timeout,
    Dhcp,
    Radio,
}

impl Err {
    pub fn label(self) -> &'static str {
        match self {
            Err::None => "",
            Err::Init => "RADIO INIT FAILED",
            Err::Ssid => "BAD SSID",
            Err::Password => "BAD PASSWORD LENGTH",
            Err::NotFound => "NETWORK NOT FOUND",
            Err::Auth => "AUTH/WRONG PASSWORD",
            Err::Timeout => "CONNECT TIMEOUT",
            Err::Dhcp => "CONNECTED, NO IP (DHCP)",
            Err::Radio => "RADIO ERROR",
        }
    }
}

/// Знімок стану Wi-Fi для UI (копія під критичною секцією).
#[derive(Clone, Copy)]
pub struct Status {
    pub phase: Phase,
    /// Лічильник змін: UI перемальовується, коли він змінився.
    pub seq: u32,
    pub ssid: [u8; 33],
    pub ssid_len: u8,
    /// Канал підключення (0 — невідомо).
    pub channel: u8,
    pub ip: [u8; 4],
    pub has_ip: bool,
    pub rssi: i8,
    pub err: Err,
}

impl Status {
    pub const EMPTY: Status = Status {
        phase: Phase::Off,
        seq: 0,
        ssid: [0u8; 33],
        ssid_len: 0,
        channel: 0,
        ip: [0u8; 4],
        has_ip: false,
        rssi: 0,
        err: Err::None,
    };

    pub fn ssid(&self) -> &str {
        core::str::from_utf8(&self.ssid[..self.ssid_len as usize]).unwrap_or("")
    }

    pub fn ssid_bytes(&self) -> &[u8] {
        &self.ssid[..self.ssid_len as usize]
    }

    pub fn band(&self) -> Band {
        if self.channel > 14 { Band::G5 } else { Band::G24 }
    }

    pub fn bump(&mut self) {
        self.seq = self.seq.wrapping_add(1);
    }
}

#[derive(Clone, Copy)]
enum Cmd {
    Scan,
    Connect { cfg: WifiCfg, channel: Option<u8> },
    Disconnect,
}

struct ApList {
    items: [Ap; MAX_APS],
    count: usize,
    seq: u32,
}

static STATUS: Mutex<RefCell<Status>> = Mutex::new(RefCell::new(Status::EMPTY));
static APS: Mutex<RefCell<ApList>> = Mutex::new(RefCell::new(ApList {
    items: [Ap::EMPTY; MAX_APS],
    count: 0,
    seq: 0,
}));
static CMD: Channel<CriticalSectionRawMutex, Cmd, 4> = Channel::new();
static NET_RES: StaticCell<StackResources<3>> = StaticCell::new();
const SEED: u64 = 0x4359_4435_5749_4649; // "CYD5WIFI"

fn with_status<R>(f: impl FnOnce(&mut Status) -> R) -> R {
    critical_section::with(|cs| f(&mut STATUS.borrow_ref_mut(cs)))
}

/// Поточний стан (дешева копія).
pub fn status() -> Status {
    critical_section::with(|cs| *STATUS.borrow_ref(cs))
}

/// Копіює список мереж; повертає `(кількість, лічильник змін)`.
pub fn aps(out: &mut [Ap; MAX_APS]) -> (usize, u32) {
    critical_section::with(|cs| {
        let list = APS.borrow_ref(cs);
        out.copy_from_slice(&list.items);
        (list.count, list.seq)
    })
}

pub fn request_scan() {
    let _ = CMD.try_send(Cmd::Scan);
}

/// Підключитися до мережі. `channel` фіксує діапазон (обраний у списку 5G-канал),
/// `None` — драйвер сам вибере точку доступу за SSID.
pub fn request_connect(cfg: WifiCfg, channel: Option<u8>) {
    let _ = CMD.try_send(Cmd::Connect { cfg, channel });
}

pub fn request_disconnect() {
    let _ = CMD.try_send(Cmd::Disconnect);
}

/// Компактний текст для індикатора в шапці лаунчера + поточна фаза (для кольору).
pub fn chip() -> (&'static str, Phase) {
    let st = status();
    let text = match st.phase {
        Phase::Connected if st.has_ip => match st.band() {
            Band::G5 => "WIFI 5G",
            Band::G24 => "WIFI 2G",
        },
        Phase::Connected => "WIFI DHCP",
        Phase::Connecting => "WIFI ...",
        Phase::Scanning => "WIFI SCAN",
        Phase::Failed => "WIFI ERR",
        Phase::Off => "WIFI OFF",
        Phase::Idle => "WIFI ON",
    };
    (text, st.phase)
}

/// Одноразова ініціалізація купи для радіо-блоба (64 KB reclaimed + 36 KB DRAM).
/// Викликати першою в `main`, до `Board::init()`.
pub fn init_heap() {
    esp_alloc::heap_allocator!(#[esp_hal::ram(reclaimed)] size: 64 * 1024);
    esp_alloc::heap_allocator!(size: 36 * 1024);
}

/// Стартує фоновий таск Wi-Fi. Викликати після `esp_rtos::start(...)`.
///
/// Автопідключення немає: до закритої мережі лаунчер підключається лише за командою
/// з екрана (пароль вводить користувач на клавіатурі).
///
/// Повертає `Stack` DHCP-стеку — його ж використовує екран оновлень (HTTP/TLS у головному таску).
pub fn start(spawner: Spawner, wifi: WIFI<'static>) -> Option<Stack<'static>> {
    // Мережевий стек створюємо тут (у головному таску), а хендл `Stack` — Copy:
    // один екземпляр лишається UI, другий іде у фоновий таск разом із runner-ом.
    let (stack, runner) = embassy_net::new(
        Interface::station(),
        embassy_net::Config::dhcpv4(Default::default()),
        NET_RES.init(StackResources::new()),
        SEED,
    );
    match worker(wifi, spawner, stack, runner) {
        Ok(token) => spawner.spawn(token),
        Err(_) => with_status(|s| {
            s.phase = Phase::Failed;
            s.err = Err::Init;
            s.bump();
        }),
    }
    Some(stack)
}

#[embassy_executor::task]
async fn net_task(mut runner: Runner<'static, Interface>) {
    runner.run().await
}

#[embassy_executor::task]
async fn worker(
    wifi: WIFI<'static>,
    spawner: Spawner,
    stack: Stack<'static>,
    runner: Runner<'static, Interface>,
) {
    esp_println::println!("WIFI: init esp-radio (band mode auto = 2.4 + 5 GHz, country UA)");
    let config = ControllerConfig::default().with_country_info(*b"UA");
    let mut ctl = match WifiController::new(wifi, config) {
        Ok(c) => c,
        Err(e) => {
            esp_println::println!("WIFI: init failed: {e:?}");
            with_status(|s| {
                s.phase = Phase::Failed;
                s.err = Err::Init;
                s.bump();
            });
            return;
        }
    };

    // DHCP-стек живе весь час: після кожного link-up він сам отримує адресу.
    match net_task(runner) {
        Ok(token) => spawner.spawn(token),
        Err(_) => {
            with_status(|s| {
                s.phase = Phase::Failed;
                s.err = Err::Radio;
                s.bump();
            });
            return;
        }
    }

    with_status(|s| {
        s.phase = Phase::Idle;
        s.bump();
    });
    esp_println::println!("WIFI: ready (2.4 + 5 GHz, чекає команду)");

    let mut creds: Option<WifiCfg> = None;

    loop {
        if let Ok(cmd) = CMD.try_receive() {
            match cmd {
                Cmd::Scan => scan(&mut ctl).await,
                Cmd::Connect { cfg, channel } => {
                    creds = Some(cfg);
                    connect(&mut ctl, &stack, cfg, channel).await;
                }
                Cmd::Disconnect => {
                    creds = None;
                    if ctl.is_connected() {
                        let _ = ctl.disconnect_async().await;
                    }
                    set_idle();
                }
            }
            continue;
        }

        // Домашні справи: оновити IP/RSSI, помітити втрату зв'язку й спробувати відновити.
        if status().phase == Phase::Connected {
            if ctl.is_connected() {
                publish_link(&ctl, stack.config_v4());
            } else {
                esp_println::println!("WIFI: link lost, reconnecting");
                with_status(|s| {
                    s.phase = Phase::Connecting;
                    s.has_ip = false;
                    s.err = Err::None;
                    s.bump();
                });
                match creds {
                    Some(c) => connect(&mut ctl, &stack, c, None).await,
                    None => set_idle(),
                }
            }
        }

        Timer::after(Duration::from_millis(300)).await;
    }
}

/// Перетворює причину розриву/невдалого підключення у короткий код для UI.
fn reason_err(reason: DisconnectReason) -> Err {
    match reason {
        DisconnectReason::NoAccessPointFound
        | DisconnectReason::NoAccessPointFoundWithCompatibleSecurity
        | DisconnectReason::NoAccessPointFoundInAuthmodeThreshold
        | DisconnectReason::NoAccessPointFoundInRssiThreshold
        | DisconnectReason::BeaconTimeout
        | DisconnectReason::Timeout => Err::NotFound,
        DisconnectReason::AuthenticationFailed
        | DisconnectReason::FourWayHandshakeTimeout
        | DisconnectReason::HandshakeTimeout
        | DisconnectReason::AssociationNotAuthenticated
        | DisconnectReason::InvalidPmkid
        | DisconnectReason::MicFailure => Err::Auth,
        _ => Err::Auth,
    }
}

fn set_idle() {
    with_status(|s| {
        s.phase = Phase::Idle;
        s.has_ip = false;
        s.ssid_len = 0;
        s.channel = 0;
        s.rssi = 0;
        s.err = Err::None;
        s.ip = [0; 4];
        s.bump();
    });
}

fn fail(e: Err) {
    esp_println::println!("WIFI: {}", e.label());
    with_status(|s| {
        s.phase = Phase::Failed;
        s.has_ip = false;
        s.err = e;
        s.bump();
    });
}

fn publish_link(ctl: &WifiController<'static>, cfg: Option<embassy_net::StaticConfigV4>) {
    let rssi = ctl.rssi().unwrap_or(0);
    let ip = cfg.map(|c| c.address.address().octets());
    with_status(|s| {
        let mut changed = false;
        if let Some(ip) = ip
            && (s.ip != ip || !s.has_ip)
        {
            s.ip = ip;
            s.has_ip = true;
            changed = true;
        }
        let rssi = rssi.clamp(-128, 127) as i8;
        if (s.rssi as i16 - rssi as i16).unsigned_abs() >= 3 {
            s.rssi = rssi;
            changed = true;
        }
        if changed {
            s.bump();
        }
    });
}

async fn scan(ctl: &mut WifiController<'static>) {
    esp_println::println!("WIFI: scan...");
    with_status(|s| {
        s.phase = Phase::Scanning;
        s.bump();
    });

    let result = ctl.scan_async(&ScanConfig::default().with_max(32)).await;
    let mut buf = [Ap::EMPTY; MAX_APS];
    let mut n = 0usize;
    match result {
        Ok(list) => {
            for ap in list.iter() {
                if n >= MAX_APS {
                    break;
                }
                // одна точка доступу = один BSSID
                if buf[..n].iter().any(|e| e.bssid == ap.bssid) {
                    continue;
                }
                let ssid = ap.ssid.as_str();
                let len = ssid.len().min(32);
                let mut e = Ap {
                    ssid: [0u8; 33],
                    ssid_len: len as u8,
                    bssid: ap.bssid,
                    channel: ap.channel,
                    rssi: ap.signal_strength,
                    auth: auth_of(ap.auth_method),
                };
                e.ssid[..len].copy_from_slice(&ssid.as_bytes()[..len]);
                buf[n] = e;
                n += 1;
            }
            // сортування за RSSI (спадіння), n ≤ 24 — вставками
            for i in 1..n {
                let mut j = i;
                while j > 0 && buf[j - 1].rssi < buf[j].rssi {
                    buf.swap(j - 1, j);
                    j -= 1;
                }
            }
            esp_println::println!("WIFI: scan done, {} AP", n);
            critical_section::with(|cs| {
                let mut list = APS.borrow_ref_mut(cs);
                list.items = buf;
                list.count = n;
                list.seq = list.seq.wrapping_add(1);
            });
        }
        Err(e) => {
            esp_println::println!("WIFI: scan failed: {e:?}");
            fail(Err::Radio);
            return;
        }
    }

    let connected = ctl.is_connected();
    with_status(|s| {
        s.phase = if connected { Phase::Connected } else { Phase::Idle };
        s.bump();
    });
}

async fn connect(
    ctl: &mut WifiController<'static>,
    stack: &Stack<'static>,
    cfg: WifiCfg,
    channel: Option<u8>,
) {
    esp_println::println!(
        "WIFI: connect to \"{}\" (pass {} chars, channel {:?})",
        cfg.ssid(),
        cfg.pass_bytes().len(),
        channel
    );
    with_status(|s| {
        s.phase = Phase::Connecting;
        s.err = Err::None;
        s.has_ip = false;
        s.ip = [0; 4];
        s.channel = 0;
        s.rssi = 0;
        let len = cfg.ssid_bytes().len().min(32);
        s.ssid = [0; 33];
        s.ssid[..len].copy_from_slice(&cfg.ssid_bytes()[..len]);
        s.ssid_len = len as u8;
        s.bump();
    });

    // Завжди гасимо попередній стан (Connected *або* Connecting): інакше `esp_wifi_connect`
    // поверне помилку, якщо попередня спроба ще не завершилась. Для Idle це просто NotConnected.
    let _ = with_timeout(Duration::from_secs(2), ctl.disconnect_async()).await;

    let ssid = match Ssid::try_from(cfg.ssid()) {
        Ok(s) => s,
        Err(_) => return fail(Err::Ssid),
    };
    let auth = if cfg.pass_bytes().is_empty() {
        AuthenticationMethodConfig::Open
    } else {
        match Password::try_from(cfg.pass()) {
            Ok(p) => AuthenticationMethodConfig::Wpa2Personal(p),
            Err(_) => return fail(Err::Password),
        }
    };
    let mut station = StationConfig::default()
        .with_ssid(ssid)
        .with_authentication(auth);
    if let Some(ch) = channel {
        station = station.with_channel(ch);
    }
    if let Err(e) = ctl.set_config(&Config::Station(station)) {
        esp_println::println!("WIFI: set_config failed: {e:?}");
        return fail(Err::Radio);
    }

    match with_timeout(Duration::from_secs(15), ctl.connect_async()).await {
        Ok(Ok(info)) => {
            esp_println::println!(
                "WIFI: associated \"{}\" ch={} {:?}",
                info.ssid.as_str(),
                info.channel,
                info.authmode
            );
            with_status(|s| {
                s.phase = Phase::Connected;
                s.channel = info.channel;
                s.err = Err::None;
                s.bump();
            });
        }
        Ok(Err(e)) => {
            esp_println::println!("WIFI: connect error: {e:?}");
            let code = match e {
                esp_radio::wifi::ConnectionError::Failed(info) => reason_err(info.reason),
                esp_radio::wifi::ConnectionError::WifiError(_) => Err::Radio,
                _ => Err::Radio,
            };
            return fail(code);
        }
        Err(_) => {
            esp_println::println!("WIFI: connect timeout");
            if ctl.is_connected() {
                let _ = ctl.disconnect_async().await;
            }
            return fail(Err::Timeout);
        }
    }

    // DHCP: чекаємо адресу до 20 с (звичайно 1–3 с).
    for _ in 0..100 {
        if let Some(c) = stack.config_v4() {
            let oct = c.address.address().octets();
            esp_println::println!("WIFI: IP {}.{}.{}.{}", oct[0], oct[1], oct[2], oct[3]);
            with_status(|s| {
                s.ip = oct;
                s.has_ip = true;
                s.bump();
            });
            publish_link(ctl, Some(c));
            return;
        }
        Timer::after(Duration::from_millis(200)).await;
    }
    fail(Err::Dhcp);
}
