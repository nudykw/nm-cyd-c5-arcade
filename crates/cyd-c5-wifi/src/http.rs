//! Мінімальний HTTPS-клієнт для оновлень по Wi-Fi:
//! DNS → TCP → **TLS 1.3** (`embedded-tls` + `rustpki`, повна перевірка ланцюжка) → HTTP/1.1 `GET`.
//!
//! Свідомі обмеження (свідомо, щоб не тягнути у прошивку зайвий мегабайт):
//! - лише `https://`, лише `GET`, без gzip (тому й без `Accept-Encoding`);
//! - тіло читається за `Content-Length` (GitHub для API і для ассетів його надсилає);
//! - редіректи (302 на CDN релізів) проходяться вручну, до 4 стрибків, кожен — нове TLS-з'єднання.

use alloc::{string::String, vec, vec::Vec};
use core::cell::Cell;
use critical_section::Mutex;

use embassy_net::{
    IpAddress, IpEndpoint, Stack,
    dns::DnsQueryType,
    tcp::TcpSocket,
};
use embassy_time::{Duration, with_timeout};
use embedded_io_async::Write as _;
use embedded_tls::{
    Aes128GcmSha256, Certificate, CryptoProvider, CryptoRngCore, TlsClock, TlsConfig, TlsConnection,
    TlsContext, TlsError, TlsVerifier,
};

use crate::ca;

/// Скільки секунд чекати на всю операцію (DNS + усі TLS-рукостискання + тіло).
pub const TIMEOUT_API: u64 = 40;
pub const TIMEOUT_DOWNLOAD: u64 = 180;

// Буфери. TLS-запис може сягати 16 КБ, тому read-буфер мусить його вмістити;
// решта — мінімум, щоб не з'їсти всю купу (див. docs/WIFI.md про RAM).
const TCP_RX: usize = 4096;
const TCP_TX: usize = 2048;
const TLS_RECORD_RX: usize = 16 * 1024;
const TLS_RECORD_TX: usize = 2048;
const HEADER_MAX: usize = 4096;

/// Час Unix (сек) для перевірки строку дії сертифікатів. 0 — ще не синхронізовано.
/// (На riscv32 немає `AtomicU64`, тому звичайна критична секція.)
static UNIX_TIME: Mutex<Cell<u64>> = Mutex::new(Cell::new(0));

pub fn set_unix_time(t: u64) {
    critical_section::with(|cs| UNIX_TIME.borrow(cs).set(t));
}

pub fn unix_time() -> u64 {
    critical_section::with(|cs| UNIX_TIME.borrow(cs).get())
}

/// Годинник для `embedded-tls`: без синхронізації (`None`) перевірка дат у сертифікатах
/// неможлива, тому `update` спершу робить SNTP і відмовляється працювати без часу.
pub struct SysClock;

impl TlsClock for SysClock {
    fn now() -> Option<u64> {
        match unix_time() {
            0 => None,
            t => Some(t),
        }
    }
}

/// Апаратний RNG у термінах `rand_core 0.6` (так його хоче `embedded-tls`).
struct EspRng(esp_hal::rng::Rng);

impl rand_core::RngCore for EspRng {
    fn next_u32(&mut self) -> u32 {
        self.0.random()
    }
    fn next_u64(&mut self) -> u64 {
        let mut b = [0u8; 8];
        self.0.read(&mut b);
        u64::from_le_bytes(b)
    }
    fn fill_bytes(&mut self, dst: &mut [u8]) {
        self.0.read(dst);
    }
    fn try_fill_bytes(&mut self, dst: &mut [u8]) -> Result<(), rand_core::Error> {
        self.0.read(dst);
        Ok(())
    }
}

impl rand_core::CryptoRng for EspRng {}

struct TlsProvider<'a> {
    rng: EspRng,
    verifier: embedded_tls::pki::CertVerifier<'a, Aes128GcmSha256, SysClock, 8192>,
}

impl CryptoProvider for TlsProvider<'_> {
    type CipherSuite = Aes128GcmSha256;
    type Signature = p256::ecdsa::DerSignature;

    fn rng(&mut self) -> impl CryptoRngCore {
        &mut self.rng
    }

    fn verifier(&mut self) -> Result<&mut impl TlsVerifier<Self::CipherSuite>, TlsError> {
        Ok(&mut self.verifier)
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum HttpErr {
    BadUrl,
    Dns,
    Tcp,
    Timeout,
    TlsHandshake,
    TlsIo,
    NoTime,
    BadResponse,
    TooManyRedirects,
    Status(u16),
}

impl HttpErr {
    /// Короткий текст для екрана/логу.
    pub fn label(self) -> &'static str {
        match self {
            HttpErr::BadUrl => "BAD URL",
            HttpErr::Dns => "DNS FAILED",
            HttpErr::Tcp => "TCP FAILED",
            HttpErr::Timeout => "NETWORK TIMEOUT",
            HttpErr::TlsHandshake => "TLS FAILED (CERT?)",
            HttpErr::TlsIo => "TLS I/O ERROR",
            HttpErr::NoTime => "NO TIME (SNTP)",
            HttpErr::BadResponse => "HTTP PARSE ERROR",
            HttpErr::TooManyRedirects => "TOO MANY REDIRECTS",
            HttpErr::Status(_) => "HTTP STATUS",
        }
    }
}

pub struct Response {
    pub status: u16,
    pub content_length: u32,
    pub location: Option<String>,
}

/// Розбирає `https://host[:port]/path…` → (host, port, path).
fn split_url(url: &str) -> Result<(u16, &str, String), HttpErr> {
    let rest = url.strip_prefix("https://").ok_or(HttpErr::BadUrl)?;
    let (authority, path) = match rest.find('/') {
        Some(i) => (&rest[..i], &rest[i..]),
        None => (rest, "/"),
    };
    let (host, port) = match authority.rsplit_once(':') {
        Some((h, p)) if p.bytes().all(|b| b.is_ascii_digit()) => {
            (h, p.parse::<u16>().map_err(|_| HttpErr::BadUrl)?)
        }
        _ => (authority, 443),
    };
    // host потрібен із тим самим життям, що й `url`, тому повертаємо позиції через String
    Ok((port, host, String::from(path)))
}

/// Один `GET` із проходженням редіректів. `body` викликається шматками з тіла **кінцевої** відповіді.
pub async fn get<F>(
    stack: Stack<'static>,
    url: &str,
    accept: &str,
    timeout_s: u64,
    body: &mut F,
) -> Result<Response, HttpErr>
where
    F: FnMut(&[u8]) -> Result<(), HttpErr>,
{
    let mut current = String::from(url);
    for hop in 0..4 {
        let (port, host, path) = split_url(&current)?;
        let host = String::from(host);

        let mut resp = match with_timeout(
            Duration::from_secs(timeout_s),
            once(stack, &host, port, &path, accept, body),
        )
        .await
        {
            Ok(r) => r?,
            Err(_) => return Err(HttpErr::Timeout),
        };

        if (300..400).contains(&resp.status) && hop < 3 {
            let loc = resp.location.take().ok_or(HttpErr::BadResponse)?;
            current = loc;
            continue;
        }
        if resp.status != 200 {
            return Err(HttpErr::Status(resp.status));
        }
        return Ok(resp);
    }
    Err(HttpErr::TooManyRedirects)
}

async fn once<F>(
    stack: Stack<'static>,
    host: &str,
    port: u16,
    path: &str,
    accept: &str,
    body: &mut F,
) -> Result<Response, HttpErr>
where
    F: FnMut(&[u8]) -> Result<(), HttpErr>,
{
    esp_println::println!("HTTP: resolve {host}...");
    let ip = match resolve(stack, host).await {
        Ok(ip) => {
            esp_println::println!("HTTP: {host} -> {ip:?}");
            ip
        }
        Err(e) => {
            esp_println::println!("HTTP: DNS failed for {host}: {:?}", e);
            return Err(e);
        }
    };

    // Пробуємо корені по черзі: api.github.com (Sectigo) і CDN (Let's Encrypt).
    let mut last = HttpErr::TlsHandshake;
    for (i, root) in ca::ROOTS.iter().enumerate() {
        match with_root(stack, ip, host, port, path, accept, root, body).await {
            Err(HttpErr::TlsHandshake) if i + 1 < ca::ROOTS.len() => last = HttpErr::TlsHandshake,
            other => return other,
        }
    }
    Err(last)
}

async fn resolve(stack: Stack<'static>, host: &str) -> Result<IpAddress, HttpErr> {
    let addrs = stack
        .dns_query(host, DnsQueryType::A)
        .await
        .map_err(|_| HttpErr::Dns)?;
    addrs.first().copied().ok_or(HttpErr::Dns)
}

#[allow(clippy::too_many_arguments)]
async fn with_root<F>(
    stack: Stack<'static>,
    ip: IpAddress,
    host: &str,
    port: u16,
    path: &str,
    accept: &str,
    root: &'static [u8],
    body: &mut F,
) -> Result<Response, HttpErr>
where
    F: FnMut(&[u8]) -> Result<(), HttpErr>,
{
    let mut tcp_rx = vec![0u8; TCP_RX];
    let mut tcp_tx = vec![0u8; TCP_TX];
    let mut sock = TcpSocket::new(stack, &mut tcp_rx, &mut tcp_tx);
    sock.set_timeout(Some(Duration::from_secs(15)));
    esp_println::println!("HTTP: tcp connect {ip:?}:{port}");
    sock.connect(IpEndpoint::new(ip, port))
        .await
        .map_err(|e| {
            esp_println::println!("HTTP: tcp failed: {e:?}");
            HttpErr::Tcp
        })?;

    let mut rec_rx = vec![0u8; TLS_RECORD_RX];
    let mut rec_tx = vec![0u8; TLS_RECORD_TX];
    let mut tls = TlsConnection::new(sock, &mut rec_rx, &mut rec_tx);

    let cfg = TlsConfig::new().with_server_name(host);
    let provider = TlsProvider {
        rng: EspRng(esp_hal::rng::Rng::new()),
        verifier: embedded_tls::pki::CertVerifier::new(Certificate::X509(root)),
    };
    esp_println::println!("HTTP: tls handshake {host}");
    tls.open(TlsContext::new(&cfg, provider)).await.map_err(|e| {
        esp_println::println!("HTTP: tls failed: {e:?}");
        HttpErr::TlsHandshake
    })?;
    esp_println::println!("HTTP: tls ok");

    let mut req = String::with_capacity(256 + path.len());
    req.push_str("GET ");
    req.push_str(path);
    req.push_str(" HTTP/1.1\r\nHost: ");
    req.push_str(host);
    req.push_str("\r\nUser-Agent: cyd-c5-arcade\r\nAccept: ");
    req.push_str(accept);
    req.push_str("\r\nConnection: close\r\n\r\n");

    tls.write_all(req.as_bytes())
        .await
        .map_err(|e| {
            esp_println::println!("HTTP: write failed: {e:?}");
            HttpErr::TlsIo
        })?;
    tls.flush().await.map_err(|e| {
        esp_println::println!("HTTP: flush failed: {e:?}");
        HttpErr::TlsIo
    })?;


    // ── заголовки (до `\r\n\r\n`) ────────────────────────────────────────────────────────

    // Заголовки читаємо ПО РЯДКАХ: GitHub віддає до 6 КБ заголовків (величезний
    // Content-Security-Policy), тому тримаємо лише поточний рядок і три потрібні поля.
    let mut line: Vec<u8> = Vec::with_capacity(HEADER_MAX);
    let mut too_long = false;
    let mut first = true;
    let mut status = 0u16;
    let mut content_length = 0u32;
    let mut location: Option<String> = None;
    // Читаємо шматками (по 256 Б), бо GitHub віддає ~5.5 КБ заголовків:
    // побайтове читання розтягувало б це на десятки секунд (кожен read — await).
    let mut buf = [0u8; 256];
    let mut pos = 0usize;
    let mut len = 0usize;
    loop {
        if pos >= len {
            let n = match with_timeout(Duration::from_secs(15), tls.read(&mut buf)).await {
                Ok(Ok(n)) => n,
                Ok(Err(e)) => {
                    esp_println::println!("HTTP: read failed in header: {e:?}");
                    return Err(HttpErr::TlsIo);
                }
                Err(_) => {
                    esp_println::println!("HTTP: header read TIMEOUT");
                    return Err(HttpErr::Timeout);
                }
            };
            if n == 0 {
                esp_println::println!("HTTP: eof in header");
                return Err(HttpErr::BadResponse);
            }
            len = n;
            pos = 0;
        }
        let b = buf[pos];
        pos += 1;
        if b == b'\r' {
            continue;
        }
        if b != b'\n' {
            if line.len() < HEADER_MAX {
                line.push(b);
            } else {
                too_long = true;
            }
            continue;
        }

        // кінець рядка
        if !too_long {
            if first {
                status = parse_status(&line)?;
                first = false;
            } else if line.is_empty() {
                break; // порожній рядок = кінець заголовків
            } else if let Some(v) = strip_prefix_ci(&line, b"content-length:") {
                content_length = core::str::from_utf8(v)
                    .ok()
                    .map(|s| s.trim())
                    .and_then(|s| s.parse().ok())
                    .unwrap_or(0);
            } else if let Some(v) = strip_prefix_ci(&line, b"location:") {
                if let Ok(s) = core::str::from_utf8(v) {
                    location = Some(String::from(s.trim()));
                }
            }
        }
        line.clear();
        too_long = false;
    }
    esp_println::println!(
        "HTTP: {} content-length={} location={}",
        status,
        content_length,
        location.as_deref().unwrap_or("-")
    );

    // ── тіло ────────────────────────────────────────────────────────────────────────────
    // GitHub завжди надсилає `Content-Length`, тож читаємо рівно стільки байтів.
    // Нульова довжина (302/204) — тіла немає, читати не можна (з'єднання вже «закрите»).
    if content_length > 0 {
        let mut buf = vec![0u8; 4096];
        let mut left = content_length as usize;
        while left > 0 {
            let want = left.min(buf.len());
            let n = match with_timeout(Duration::from_secs(30), tls.read(&mut buf[..want])).await {
                Ok(Ok(n)) => n,
                Ok(Err(e)) => {
                    esp_println::println!("HTTP: body read failed: {e:?}");
                    return Err(HttpErr::TlsIo);
                }
                Err(_) => {
                    esp_println::println!("HTTP: body read TIMEOUT ({left} B left)");
                    return Err(HttpErr::Timeout);
                }
            };
            if n == 0 {
                esp_println::println!("HTTP: body eof, {left} B left");
                return Err(HttpErr::BadResponse);
            }
            body(&buf[..n])?;
            left -= n;
        }
    }
    // `close()` чекає на close_notify від сервера (GitHub його не надсилає) — просто кидаємо з'єднання.
    drop(tls);

    Ok(Response {
        status,
        content_length,
        location,
    })
}

fn parse_status(line: &[u8]) -> Result<u16, HttpErr> {
    // "HTTP/1.1 200 OK"
    let code = line.get(9..12).ok_or(HttpErr::BadResponse)?;
    core::str::from_utf8(code)
        .ok()
        .and_then(|s| s.parse().ok())
        .ok_or(HttpErr::BadResponse)
}

/// `name` — у нижньому регістрі; регістр рядка не важливий.
fn strip_prefix_ci<'a>(line: &'a [u8], name: &[u8]) -> Option<&'a [u8]> {
    if line.len() >= name.len()
        && line[..name.len()]
            .iter()
            .zip(name)
            .all(|(a, b)| a.eq_ignore_ascii_case(b))
    {
        Some(&line[name.len()..])
    } else {
        None
    }
}

// ── SNTP ────────────────────────────────────────────────────────────────────────────────
/// Синхронізує час по SNTP (UDP/123). Без часу перевірка сертифікатів неможлива.
pub async fn sntp_sync(stack: Stack<'static>, server: &str) -> Result<u64, HttpErr> {
    let ip = resolve(stack, server).await?;

    let mut rx_meta = [embassy_net::udp::PacketMetadata::EMPTY; 2];
    let mut rx_buf = [0u8; 128];
    let mut tx_meta = [embassy_net::udp::PacketMetadata::EMPTY; 2];
    let mut tx_buf = [0u8; 64];
    let mut sock = embassy_net::udp::UdpSocket::new(
        stack,
        &mut rx_meta,
        &mut rx_buf,
        &mut tx_meta,
        &mut tx_buf,
    );
    sock.bind(0).map_err(|_| HttpErr::Tcp)?;

    let mut req = [0u8; 48];
    req[0] = 0x23; // LI=0, VN=4, Mode=3 (client)
    sock.send_to(&req, IpEndpoint::new(ip, 123))
        .await
        .map_err(|_| HttpErr::Tcp)?;

    let mut resp = [0u8; 48];
    let (n, _) = with_timeout(Duration::from_secs(5), sock.recv_from(&mut resp))
        .await
        .map_err(|_| HttpErr::Timeout)?
        .map_err(|_| HttpErr::Tcp)?;
    sock.close();
    if n < 48 {
        return Err(HttpErr::BadResponse);
    }
    // transmit timestamp, секунди з 1900-01-01
    let secs = u32::from_be_bytes(resp[40..44].try_into().unwrap()) as u64;
    if secs < 2_208_988_800 {
        return Err(HttpErr::BadResponse);
    }
    let unix = secs - 2_208_988_800;
    set_unix_time(unix);
    Ok(unix)
}
