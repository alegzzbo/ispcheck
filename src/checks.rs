use crate::config::*;
use crate::dns;
use crate::state::*;
use crate::sys;
use reqwest::blocking::{Body, Client, ClientBuilder};
use std::io::{self, Read};
use std::net::{IpAddr, Ipv4Addr, SocketAddr, TcpStream, ToSocketAddrs};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering::Relaxed};
use std::sync::Arc;
use std::thread::sleep;
use std::time::{Duration, Instant};

const MS: Duration = Duration::from_millis(1);

fn http(secs: u64) -> ClientBuilder {
    // no_proxy: прямое соединение, системный прокси проверяется отдельно
    crate::net::tls(Client::builder()).no_proxy().user_agent("Mozilla/5.0").timeout(Duration::from_secs(secs))
}

fn ms(d: Duration) -> f64 {
    d.as_secs_f64() * 1000.0
}

fn avg_jitter(ts: &[f64]) -> (f64, f64) {
    let avg = ts.iter().sum::<f64>() / ts.len() as f64;
    let mut jit = 0.0;
    for w in ts.windows(2) {
        jit += (w[1] - w[0]).abs();
    }
    if ts.len() > 1 {
        jit /= (ts.len() - 1) as f64;
    }
    (avg, jit)
}

fn rnd() -> u64 {
    static C: AtomicU64 = AtomicU64::new(0x9E37_79B9_7F4A_7C15);
    let t = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(1);
    let mut x = t ^ C.fetch_add(0x9E37_79B9_7F4A_7C15, Relaxed);
    x ^= x << 13;
    x ^= x >> 7;
    x ^= x << 17;
    x
}

pub fn check_connection_info() {
    log(HDR, "Тип подключения");
    let c = sys::connection_type();
    st().summ.conn = c.clone();
    log(OK, format!("Соединение: {c}"));
    check_link();
}

/// Скорость линка Ethernet/Wi-Fi (согласованная, а не измеренная).
fn check_link() {
    let Some(l) = sys::link_info() else { return };
    st().link = Some(l.clone());
    if l.rx_mbps <= 0.0 && l.tx_mbps <= 0.0 {
        log(WARN, format!("Скорость линка {} ({}): не определена", l.kind_ru(), l.name));
        if l.kind == "wifi" && cfg!(target_os = "linux") {
            log("·", "Для определения скорости Wi-Fi установите пакет iw: sudo apt install iw (либо wireless-tools / NetworkManager)");
        }
        return;
    }
    let mut s = format!("{} {:.0}/{:.0} Мбит/с (приём/передача)", l.kind_ru(), l.rx_mbps, l.tx_mbps);
    if let Some(p) = l.signal_pct {
        s += &format!(", сигнал {p}%");
    }
    if let Some(d) = l.signal_dbm {
        s += &format!(", сигнал {d} дБм");
    }
    if !l.band.is_empty() {
        s += &format!(", {}", l.band_text());
    }
    st().summ.link = s.clone();
    let weak = l.signal_pct.map_or(false, |p| p < 40) || l.signal_dbm.map_or(false, |d| d < -75);
    let slow_eth = l.kind == "ethernet" && l.rx_mbps > 0.0 && l.rx_mbps <= 100.0;
    log(if weak || slow_eth { WARN } else { OK }, format!("Линк: {s}"));
    if l.kind == "wifi" && l.ssid.is_empty() && cfg!(target_os = "android") {
        log("·", "SSID не показан: разрешите приложению доступ к геолокации и включите геолокацию в настройках телефона");
    }
    if l.kind == "wifi" && l.ssid.is_empty() && cfg!(target_os = "macos") {
        log("·", "SSID не показан: macOS скрывает его от приложений без доступа к геолокации");
    }
    if !l.ssid.is_empty() {
        let phy = if l.phy.is_empty() { String::new() } else { format!(", {}", l.phy) };
        log("·", format!("Wi-Fi сеть: {}{phy}", l.ssid));
        if l.band == "2.4" {
            log("·", "Диапазон 2,4 ГГц: больше помех и ниже максимальная скорость, чем на 5 ГГц");
        }
    }
    if !l.duplex.is_empty() && l.duplex != "full" {
        log(WARN, format!("Дуплекс: {} (ожидается full)", l.duplex));
    }
    if slow_eth {
        log(WARN, "Порт работает на 100 Мбит/с или ниже: проверьте кабель, разъёмы и порт роутера");
    }
    if weak {
        log(WARN, "Слабый сигнал Wi-Fi: скорость и стабильность будут ниже");
    }
}

/// Средняя скорость интерфейса за secs секунд: (приём, передача) в Мбит/с.
fn sample_rates(secs: f64) -> Option<(f64, f64)> {
    let a = sys::if_counters()?;
    let t = Instant::now();
    sleep(Duration::from_secs_f64(secs));
    let b = sys::if_counters()?;
    let dt = t.elapsed().as_secs_f64();
    Some((
        b.0.saturating_sub(a.0) as f64 * 8.0 / dt / 1e6,
        b.1.saturating_sub(a.1) as f64 * 8.0 / dt / 1e6,
    ))
}

fn bg_tip() -> &'static str {
    if cfg!(windows) {
        "Кто грузит сеть: Диспетчер задач -> вкладка «Процессы», столбец «Сеть», либо Монитор ресурсов (resmon) -> Сеть"
    } else if cfg!(target_os = "android") {
        "Кто грузит сеть: Настройки -> Сеть и интернет -> Передача данных (или Wi-Fi) -> трафик по приложениям"
    } else if cfg!(target_os = "macos") {
        "Кто грузит сеть: Мониторинг системы -> вкладка «Сеть»"
    } else {
        "Кто грузит сеть: sudo nethogs (или iftop)"
    }
}

/// Порог: заметный фоновый трафик >= 2 Мбит/с, сильный >= 10 Мбит/с или 10% скорости линка.
fn bg_level(rx: f64, tx: f64, link_mbps: f64) -> u8 {
    let m = rx.max(tx);
    if m >= 10.0 || (link_mbps > 0.0 && m >= 0.1 * link_mbps) {
        2
    } else if m >= 2.0 {
        1
    } else {
        0
    }
}

/// Перед проверками: не забивают ли канал другие программы на этом компьютере.
/// Видны только данные этого компьютера (счётчики его сетевого интерфейса), а не других устройств в сети.
pub fn check_background() {
    log(HDR, "Фоновая нагрузка сети");
    // порог от скорости линка: берём меньшее из положительных значений приёма и передачи
    let link = st().link.as_ref().map_or(0.0, |l| match (l.rx_mbps > 0.0, l.tx_mbps > 0.0) {
        (true, true) => l.rx_mbps.min(l.tx_mbps),
        (true, false) => l.rx_mbps,
        (false, true) => l.tx_mbps,
        _ => 0.0,
    });
    let Some(mut r) = sample_rates(3.0) else {
        log("·", "Счётчики сетевого интерфейса недоступны, фоновая нагрузка не измерена");
        return;
    };
    let mut lvl = bg_level(r.0, r.1, link);
    if lvl > 0 {
        // короткий всплеск не считаем: перемеряем ещё раз через 5 с
        sleep(Duration::from_secs(5));
        if let Some(r2) = sample_rates(3.0) {
            let l2 = bg_level(r2.0, r2.1, link);
            if l2 <= lvl {
                r = r2;
                lvl = l2;
            }
        }
    }
    let txt = format!("приём {:.1}, передача {:.1} Мбит/с", r.0, r.1);
    st().summ.bg.push(format!("перед проверкой: {txt}"));
    match lvl {
        0 => log(OK, format!("Канал свободен: {txt}")),
        1 => {
            log(WARN, format!("Есть фоновый трафик: {txt}. Он может исказить результаты"));
            log("·", bg_tip());
        }
        _ => {
            log(WARN, format!("Другие программы заметно загружают канал: {txt}. Скорость и задержка будут хуже реальных"));
            log("·", "Закройте загрузки, торренты, облачную синхронизацию, обновления и видео, затем повторите проверку");
            log("·", bg_tip());
        }
    }
}

pub fn check_gateway() -> bool {
    log(HDR, "Шлюз по умолчанию");
    let gw = match sys::default_gateway() {
        Ok(g) => g,
        Err(e) => {
            st().summ.gw = "не определён".into();
            log(BAD, format!("не определён: {e}"));
            return false;
        }
    };
    const N: usize = 8;
    let mut ts = vec![];
    for _ in 0..N {
        match sys::icmp_echo(gw, 64, Duration::from_secs(1)) {
            Err(e) => {
                log(WARN, format!("{gw}: ICMP недоступен: {e}"));
                return true;
            }
            Ok(r) if r.reached => ts.push(ms(r.rtt)),
            _ => {}
        }
        sleep(150 * MS);
    }
    if ts.is_empty() {
        st().summ.gw = format!("{gw}: не отвечает на ICMP");
        log(WARN, format!("{gw}: не отвечает на ICMP (возможно, фильтруется роутером)"));
        return true;
    }
    let (avg, jit) = avg_jitter(&ts);
    let loss = 100.0 * (N - ts.len()) as f64 / N as f64;
    let s = format!("{gw}: {avg:.1} мс, джиттер {jit:.1} мс, потери {loss:.0}%");
    st().summ.gw = s.clone();
    log(if loss > 0.0 || avg > 20.0 { WARN } else { OK }, s);
    true
}

/// Есть ли выход в интернет: TCP/443 до публичных адресов (параллельно), либо через системный прокси.
pub fn internet_reachable() -> bool {
    let hs: Vec<_> = ["1.1.1.1:443", "8.8.8.8:443", "77.88.8.8:443"]
        .into_iter()
        .map(|a| {
            std::thread::spawn(move || {
                a.parse::<SocketAddr>()
                    .ok()
                    .map_or(false, |a| TcpStream::connect_timeout(&a, Duration::from_secs(2)).is_ok())
            })
        })
        .collect();
    let direct = hs.into_iter().fold(false, |acc, h| h.join().unwrap_or(false) || acc);
    if direct {
        return true;
    }
    let proxy = sys::system_proxy();
    !proxy.is_empty()
        && reqwest::Proxy::all(&proxy)
            .map(|p| !get_ip("https://api.ipify.org", crate::net::tls(Client::builder()).proxy(p).timeout(Duration::from_secs(5))).is_empty())
            .unwrap_or(false)
}

/// Отвечает ли сервер OpenSpeedTest (любой HTTP-ответ).
fn ost_reachable() -> Result<(), String> {
    http(4).build().map_err(|e| e.to_string())?.get(OST_URL).send().map(|_| ()).map_err(|e| e.to_string())
}

fn get_ip(url: &str, b: ClientBuilder) -> String {
    let Ok(c) = b.build() else { return String::new() };
    let Ok(r) = c.get(url).send() else { return String::new() };
    let mut s = String::new();
    let _ = r.take(64).read_to_string(&mut s);
    let s = s.trim().to_string();
    if s.parse::<IpAddr>().is_ok() { s } else { String::new() }
}

/// Адреса одного семейства (v4/v6) и разные.
fn ip_differs(a: &str, b: &str) -> bool {
    match (a.parse::<IpAddr>(), b.parse::<IpAddr>()) {
        (Ok(x), Ok(y)) => x.is_ipv4() == y.is_ipv4() && x != y,
        _ => false,
    }
}

/// Второй источник внешнего IP. При split tunneling (часть трафика мимо VPN/прокси) разные сервисы
/// видят разные адреса: российский nekoip.ru и «глобальный» ipify могут ответить по-разному.
fn check_ip_alt(ip4: &str) {
    let alt = get_ip(IP_ALT_URL, http(5));
    if alt.is_empty() {
        log(WARN, format!("Второй источник IP ({IP_ALT_NAME}) не ответил"));
        return;
    }
    st().ip_alt = alt.clone();
    if ip4.is_empty() {
        // основной источник недоступен: используем второй
        if alt.parse::<std::net::Ipv4Addr>().is_ok() {
            let mut g = st();
            g.ip4 = alt.clone();
            g.summ.ip4 = alt.clone();
        }
        log(WARN, format!("IP по {IP_ALT_NAME}: {alt} (основной источник не ответил)"));
        return;
    }
    if ip_differs(ip4, &alt) {
        st().summ.ip_alt = format!("{alt} (ОТЛИЧАЕТСЯ от {ip4})");
        log(WARN, format!("IP по {IP_ALT_NAME}: {alt} — ОТЛИЧАЕТСЯ от {ip4}"));
        log(
            WARN,
            "Сервисы видят разные адреса: вероятно, включён VPN или прокси со split tunneling (часть трафика идёт мимо туннеля)",
        );
    } else {
        st().summ.ip_alt = format!("{alt} (совпадает)");
        log(OK, format!("IP по {IP_ALT_NAME}: {alt} (совпадает)"));
    }
}

pub fn detect_ip() {
    log(HDR, "Внешний IP");
    let ip4 = get_ip("https://api.ipify.org", http(5));
    let ip6 = if ENABLE_IPV6 { get_ip("https://api6.ipify.org", http(5)) } else { String::new() };
    {
        let mut g = st();
        g.ip4 = ip4.clone();
        g.ip6 = ip6.clone();
    }
    if ip4.is_empty() {
        log(BAD, "IPv4: не определён");
    } else {
        st().summ.ip4 = ip4.clone();
        log(OK, format!("IPv4: {ip4}"));
    }
    if ENABLE_IPV6 {
        if ip6.is_empty() {
            log(WARN, "IPv6: нет");
        } else {
            log(OK, format!("IPv6: {ip6}"));
        }
    }
    check_ip_alt(&ip4);
    // IP через системный прокси
    let proxy = sys::system_proxy();
    if proxy.is_empty() {
        let pac = sys::pac_url();
        if pac.is_empty() {
            st().summ.ip_proxy = "прокси не задан".into();
            log(OK, "Системный прокси: не задан");
        } else {
            st().summ.ip_proxy = format!("PAC {pac} (не проверяется)");
            log(WARN, format!("Автонастройка прокси (PAC): {pac} — не проверяется"));
        }
        return;
    }
    log(HDR, format!("Системный прокси: {proxy}"));
    let via = match reqwest::Proxy::all(&proxy) {
        Ok(p) => get_ip("https://api.ipify.org", crate::net::tls(Client::builder()).proxy(p).timeout(Duration::from_secs(8))),
        Err(_) => String::new(),
    };
    st().ip_proxy = via.clone();
    if via.is_empty() {
        st().summ.ip_proxy = format!("{proxy} — IP не определён");
        log(WARN, "IP через прокси: не определён");
    } else if via == ip4 {
        st().summ.ip_proxy = format!("{via} (как прямой), прокси {proxy}");
        log(OK, format!("IP через прокси: {via} (совпадает с прямым)"));
    } else {
        st().summ.ip_proxy = format!("{via} (ОТЛИЧАЕТСЯ от прямого), прокси {proxy}");
        log(WARN, format!("IP через прокси: {via} (ОТЛИЧАЕТСЯ от прямого {ip4})"));
    }
}

pub fn check_system_dns(online: bool) {
    log(HDR, "DNS-серверы системы");
    let s = sys::system_dns();
    if s.is_empty() {
        log(WARN, "Не удалось определить системные DNS");
    } else {
        let j = s.join(", ");
        st().summ.dns = j.clone();
        log(OK, format!("DNS адаптера: {j}"));
    }
    if !online {
        return;
    }
    // whoami.akamai.net возвращает IP резолвера, дошедшего до интернета
    match dns::lookup_host("whoami.akamai.net", Duration::from_secs(3)) {
        Ok(v) => {
            let ip = v[0].to_string();
            st().summ.dns_up = ip.clone();
            log(OK, format!("Upstream-DNS (виден из интернета): {ip}"));
        }
        Err(_) => log(WARN, "Upstream-DNS не определён"),
    }
}

fn measure(h: &str) -> (Vec<f64>, usize, &'static str) {
    const N: usize = 6;
    let mut ts = vec![];
    if let Some(ip) = dns::resolve4(h) {
        for _ in 0..N {
            match sys::icmp_echo(ip, 64, Duration::from_secs(2)) {
                Err(_) => break,
                Ok(r) if r.reached => ts.push(ms(r.rtt)),
                _ => {}
            }
            sleep(150 * MS);
        }
    }
    if !ts.is_empty() {
        return (ts, N, "ICMP");
    }
    let addr = (h, 443).to_socket_addrs().ok().and_then(|mut a| a.next());
    for _ in 0..N {
        if let Some(a) = addr {
            let t = Instant::now();
            if TcpStream::connect_timeout(&a, Duration::from_secs(2)).is_ok() {
                ts.push(ms(t.elapsed()));
            }
        }
        sleep(150 * MS);
    }
    (ts, N, "TCP:443, ICMP недоступен")
}

pub fn check_ping() {
    log(HDR, "Задержка / потери");
    let mut targets = vec!["1.1.1.1", "8.8.8.8"];
    targets.extend_from_slice(ISP_HOSTS);
    for h in targets {
        let (ts, n, method) = measure(h);
        if ts.is_empty() {
            log(BAD, format!("{h}: НЕДОСТУПЕН"));
            st().summ.pings.push(format!("{h}: НЕДОСТУПЕН"));
            continue;
        }
        let (avg, jit) = avg_jitter(&ts);
        let loss = 100.0 * (n - ts.len()) as f64 / n as f64;
        log(
            if loss > 0.0 || avg > 100.0 { WARN } else { OK },
            format!("{h} [{method}]: {avg:.0} мс, джиттер {jit:.0} мс, потери {loss:.0}%"),
        );
        st().summ.pings.push(format!("{h}: {avg:.0} мс, джиттер {jit:.0} мс, потери {loss:.0}%"));
    }
}

fn report_dns(name: &str, good: usize, total: f64) -> usize {
    let n = DNS_DOMAINS.len();
    if good == 0 {
        log(BAD, format!("{name}: нет ответа"));
    } else {
        let m = if good < n { WARN } else { OK };
        log(m, format!("{name}: {good}/{n}, ср. {:.0} мс", total / good as f64));
    }
    good
}

pub fn check_dns() {
    log(HDR, "DNS");
    let (mut good, mut total) = (0, 0.0);
    for d in DNS_DOMAINS {
        let t = Instant::now();
        if dns::lookup_host(d, Duration::from_secs(3)).is_ok() {
            good += 1;
            total += ms(t.elapsed());
        }
    }
    let sys_good = report_dns("Системный DNS", good, total);
    let mut pub_ok = false;
    for (name, ip) in PUBLIC_DNS {
        let (mut good, mut total) = (0, 0.0);
        for d in DNS_DOMAINS {
            let t = Instant::now();
            if dns::udp_query(ip, d, Duration::from_secs(3)).map_or(false, |r| r.rcode == 0 && r.ancount > 0) {
                good += 1;
                total += ms(t.elapsed());
            }
        }
        if report_dns(name, good, total) > 0 {
            pub_ok = true;
        }
    }
    let doh_ok = check_doh();
    if sys_good < DNS_DOMAINS.len() && (pub_ok || doh_ok) {
        st().summ.dns_problem = "публичные DNS работают, системный — нет".into();
        log(BAD, "Проблема DNS провайдера: публичные DNS работают, системный — нет");
    }
    if !pub_ok && doh_ok {
        st().summ.dns_problem.push_str("; UDP/53 до публичных DNS блокируется, DoH работает");
        log(BAD, "UDP/53 до публичных DNS блокируется или подменяется, DoH работает");
    }
}

/// DoH по RFC 8484: GET ?dns=base64url, application/dns-message.
fn check_doh() -> bool {
    use base64::Engine;
    let mut any = false;
    for (name, url, ip) in DOH_SERVERS {
        let host = reqwest::Url::parse(url).ok().and_then(|u| u.host_str().map(String::from)).unwrap_or_default();
        let Ok(addr) = ip.parse::<IpAddr>() else { continue };
        let cl = match http(5).resolve(&host, SocketAddr::new(addr, 443)).build() {
            Ok(c) => c,
            Err(e) => {
                log(BAD, format!("{name}: {e}"));
                continue;
            }
        };
        let (mut good, mut total) = (0usize, 0.0);
        let mut last_err = String::new();
        for d in DNS_DOMAINS {
            let q = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(dns::query_a(d, 0));
            let t = Instant::now();
            let r = cl.get(format!("{url}?dns={q}")).header("Accept", "application/dns-message").send();
            let r = match r {
                Ok(r) => r,
                Err(e) => {
                    last_err = e.to_string();
                    continue;
                }
            };
            let status = r.status();
            let ct = r.headers().get("content-type").and_then(|v| v.to_str().ok()).unwrap_or("").to_string();
            let body = r.bytes().unwrap_or_default();
            if !status.is_success() {
                last_err = format!("HTTP {}", status.as_u16());
                continue;
            }
            match dns::parse(&body) {
                None => last_err = format!("не DNS-ответ (Content-Type {ct:?})"),
                Some(p) if p.rcode != 0 => last_err = format!("RCODE {}", p.rcode),
                Some(p) if p.ancount == 0 => last_err = "пустой ответ".into(),
                Some(_) => {
                    good += 1;
                    total += ms(t.elapsed());
                }
            }
        }
        let n = DNS_DOMAINS.len();
        if good == 0 {
            log(BAD, format!("{name}: нет ответа ({last_err})"));
        } else {
            log(if good < n { WARN } else { OK }, format!("{name}: {good}/{n}, ср. {:.0} мс", total / good as f64));
            any = true;
        }
    }
    any
}

pub fn check_sites() {
    log(HDR, "Доступность сайтов");
    let Ok(cl) = http(6).build() else {
        log(BAD, "Не удалось создать HTTP-клиент");
        return;
    };
    for u in SITES {
        let t = Instant::now();
        match cl.get(*u).send() {
            Err(_) => {
                log(BAD, format!("{u}: ОШИБКА"));
                st().summ.sites.push(format!("{u}: НЕ ОТКРЫВАЕТСЯ"));
            }
            Ok(r) => {
                let code = r.status().as_u16();
                let _ = r.take(1024).read_to_end(&mut vec![]);
                let m = t.elapsed().as_millis();
                log(OK, format!("{u}: {code}, {m} мс"));
                st().summ.sites.push(format!("{u}: {code}, {m} мс"));
            }
        }
    }
    if ENABLE_IPV6 {
        let a: SocketAddr = "[2606:4700:4700::1111]:443".parse().unwrap();
        if TcpStream::connect_timeout(&a, Duration::from_secs(3)).is_ok() {
            st().summ.ipv6 = "работает".into();
            log(OK, "IPv6: работает");
        } else {
            st().summ.ipv6 = "недоступен".into();
            log(WARN, "IPv6: недоступен");
        }
    }
}

struct CountRead {
    data: Arc<Vec<u8>>,
    pos: usize,
    total: Arc<AtomicU64>,
    end: Instant,
}

impl Read for CountRead {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if Instant::now() >= self.end {
            return Err(io::Error::new(io::ErrorKind::Other, "stop"));
        }
        let n = buf.len().min(self.data.len() - self.pos);
        buf[..n].copy_from_slice(&self.data[self.pos..self.pos + n]);
        self.pos += n;
        self.total.fetch_add(n as u64, Relaxed);
        Ok(n)
    }
}

/// HTTP/1.1, по одному TCP на поток: HTTP/2 мультиплексировал бы все потоки в одно соединение.
fn speed(down: bool) -> (f64, u64) {
    let client = crate::net::tls(Client::builder())
        .no_proxy()
        .http1_only()
        .pool_max_idle_per_host(THREADS * 2)
        .connect_timeout(Duration::from_secs(5))
        .user_agent("Mozilla/5.0")
        .build();
    let Ok(client) = client else {
        log(BAD, "Не удалось создать HTTP-клиент для замера скорости");
        return (0.0, 0);
    };
    let end = Instant::now() + Duration::from_secs(SPEED_SECONDS);
    let total = Arc::new(AtomicU64::new(0));
    let payload = Arc::new(if down {
        vec![]
    } else {
        (0..8 << 20).map(|_| rnd() as u8).collect::<Vec<u8>>()
    });
    for _ in 0..THREADS {
        let (c, total, payload) = (client.clone(), total.clone(), payload.clone());
        std::thread::spawn(move || {
            let mut buf = vec![0u8; 256 << 10];
            while Instant::now() < end {
                if down {
                    match c.get(format!("{OST_URL}/downloading?n={}", rnd())).send() {
                        Ok(mut r) => {
                            while Instant::now() < end {
                                match r.read(&mut buf) {
                                    Ok(0) | Err(_) => break,
                                    Ok(n) => {
                                        total.fetch_add(n as u64, Relaxed);
                                    }
                                }
                            }
                        }
                        Err(_) => sleep(300 * MS),
                    }
                } else {
                    let len = payload.len() as u64;
                    let body = Body::sized(CountRead { data: payload.clone(), pos: 0, total: total.clone(), end }, len);
                    match c.post(format!("{OST_URL}/upload")).body(body).send() {
                        Ok(r) => {
                            let _ = r.bytes();
                        }
                        Err(_) => {
                            if Instant::now() < end {
                                sleep(300 * MS)
                            }
                        }
                    }
                }
            }
        });
    }
    // первые 2 с (TCP slow start) не учитываем, как и браузерный тест
    sleep(Duration::from_secs(2));
    let (base, t0) = (total.load(Relaxed), Instant::now());
    sleep(end.saturating_duration_since(Instant::now()));
    let all = total.load(Relaxed);
    let bytes = all.saturating_sub(base);
    (bytes as f64 * 8.0 / t0.elapsed().as_secs_f64() / 1e6, all)
}

/// Фоновые пробы задержки, пока идёт замер скорости. Возвращает (RTT удачных проб, всего проб).
/// icmp: пробовать ICMP; tcp_port: запасной вариант — время TCP-подключения (если ICMP недоступен).
fn load_probe(
    ip: Ipv4Addr,
    icmp: bool,
    tcp_port: Option<u16>,
    stop: Arc<AtomicBool>,
) -> std::thread::JoinHandle<(Vec<f64>, usize)> {
    std::thread::spawn(move || {
        let (mut ts, mut sent) = (vec![], 0usize);
        let mut use_tcp = !icmp;
        while !stop.load(Relaxed) {
            let t0 = Instant::now();
            if !use_tcp {
                match sys::icmp_echo(ip, 64, Duration::from_secs(2)) {
                    Ok(r) => {
                        sent += 1;
                        if r.reached {
                            ts.push(ms(r.rtt));
                        }
                    }
                    Err(_) if tcp_port.is_some() => {
                        use_tcp = true;
                        continue;
                    }
                    Err(_) => return (ts, 0),
                }
            } else if let Some(p) = tcp_port {
                sent += 1;
                if TcpStream::connect_timeout(&SocketAddr::new(IpAddr::V4(ip), p), Duration::from_secs(2)).is_ok() {
                    ts.push(ms(t0.elapsed()));
                }
            } else {
                return (ts, 0);
            }
            if let Some(rest) = Duration::from_millis(200).checked_sub(t0.elapsed()) {
                sleep(rest); // ровный темп, около 5 проб в секунду
            }
        }
        (ts, sent)
    })
}

struct LoadStat {
    avg: f64,
    max: f64,
    loss: f64,
}

fn load_stat(ts: &[f64], sent: usize) -> Option<LoadStat> {
    if sent == 0 {
        return None;
    }
    let avg = if ts.is_empty() { 0.0 } else { ts.iter().sum::<f64>() / ts.len() as f64 };
    Some(LoadStat {
        avg,
        max: ts.iter().cloned().fold(0.0, f64::max),
        loss: 100.0 * (sent - ts.len()) as f64 / sent as f64,
    })
}

/// Задержка в покое: короткая серия проб перед нагрузкой.
fn idle_ms(ip: Ipv4Addr, tcp_port: Option<u16>) -> Option<f64> {
    let stop = Arc::new(AtomicBool::new(false));
    let h = load_probe(ip, true, tcp_port, stop.clone());
    sleep(Duration::from_millis(1300));
    stop.store(true, Relaxed);
    let (ts, sent) = h.join().ok()?;
    load_stat(&ts, sent).filter(|s| s.loss < 100.0).map(|s| s.avg)
}

/// Плохо, если под нагрузкой задержка выросла более чем на 100 мс, максимум выше 800 мс или потери выше 2%.
fn load_bad(s: &LoadStat, idle: Option<f64>) -> bool {
    s.loss > 2.0 || s.max > 800.0 || idle.map_or(false, |i| s.avg - i > 100.0)
}

fn report_load(phase: &str, label: &str, s: &LoadStat, idle: Option<f64>) -> bool {
    let bad = load_bad(s, idle);
    let idle_txt = idle.map_or(String::new(), |i| format!(" (в покое {i:.0} мс)"));
    log(
        if bad { WARN } else { OK },
        format!("  Под нагрузкой, {phase}: {label}: ср. {:.0} мс, макс {:.0} мс, потери {:.0}%{idle_txt}", s.avg, s.max, s.loss),
    );
    bad
}

pub fn check_speed() {
    log(HDR, "Скорость (OpenSpeedTest)");
    if let Err(e) = ost_reachable() {
        log(BAD, format!("{OST_URL} недоступен ({e}): замер скорости пропущен"));
        return;
    }
    let link = st().link.clone();
    let gw = sys::default_gateway().ok();
    let target = Ipv4Addr::new(1, 1, 1, 1);
    for (n, (name, down)) in [("Загрузка", true), ("Отдача", false)].into_iter().enumerate() {
        if n > 0 {
            sleep(Duration::from_secs(3)); // очереди после первого замера должны освободиться
        }
        let idle_t = idle_ms(target, Some(443));
        let idle_g = gw.and_then(|g| idle_ms(g, None));
        // пробы задержки до замера и во время него: до внешнего адреса и до шлюза
        let stop = Arc::new(AtomicBool::new(false));
        let ht = load_probe(target, true, Some(443), stop.clone());
        let hg = gw.map(|g| load_probe(g, true, None, stop.clone()));
        let c0 = sys::if_counters();
        let t_speed = Instant::now();
        let (v, ours_bytes) = speed(down);
        let c1 = sys::if_counters();
        let dt_speed = t_speed.elapsed().as_secs_f64();
        stop.store(true, Relaxed);
        let (tt, ts) = ht.join().unwrap_or_default();
        let lg = hg.and_then(|h| h.join().ok()).and_then(|(t, s)| load_stat(&t, s));
        let lt = load_stat(&tt, ts);

        if down {
            st().summ.down = v;
        } else {
            st().summ.up = v;
        }
        log(if v < 1.0 { BAD } else { OK }, format!("{name}: {v:.1} Мбит/с"));
        if let Some(l) = &link {
            let lim = if down { l.rx_mbps } else { l.tx_mbps };
            if lim > 0.0 && v > lim * 0.8 {
                log(WARN, format!("Результат близок к скорости линка ({lim:.0} Мбит/с): замер упирается в линк, а не в провайдера"));
            }
        }
        // Весь трафик интерфейса за время замера минус наш собственный (с запасом на заголовки):
        // остаток принадлежит другим программам этого компьютера.
        if let (Some(a), Some(b)) = (c0, c1) {
            let iface = if down { b.0.saturating_sub(a.0) } else { b.1.saturating_sub(a.1) } as f64 * 8.0 / dt_speed / 1e6;
            let ours = ours_bytes as f64 * 8.0 / dt_speed / 1e6;
            let other = iface - ours * 1.08;
            if other > 2.0f64.max(0.1 * ours) {
                log(
                    WARN,
                    format!("Во время замера канал использовали и другие программы: около {other:.1} Мбит/с. Результат ({name}) занижен"),
                );
                log("·", bg_tip());
                st().summ.bg.push(format!("{name}: другие программы около {other:.1} Мбит/с"));
            }
        }
        if v < 1.0 {
            continue; // нагрузки не было, задержку оценивать не по чему
        }
        let (mut bad_t, mut bad_g) = (false, false);
        match &lt {
            Some(s) => bad_t = report_load(name, "1.1.1.1", s, idle_t),
            None => log("·", "  Задержка под нагрузкой не измерена (нет ICMP и TCP-проб)"),
        }
        if let (Some(s), Some(g)) = (&lg, gw) {
            bad_g = report_load(name, &format!("шлюз {g}"), s, idle_g);
        }
        if let Some(s) = &lt {
            let d = idle_t.map_or(String::new(), |i| format!("{:+.0} мс", s.avg - i));
            st().summ.load.push(format!("{name}: до 1.1.1.1 {d}, макс {:.0} мс, потери {:.0}%", s.max, s.loss));
        }
        if bad_t && bad_g {
            log(WARN, "Задержка растёт и до шлюза: вероятно, перегружены Wi-Fi или роутер (слабый CPU, очередь на LAN-стороне)");
        } else if bad_t {
            log(WARN, "Задержка растёт только за шлюзом: вероятно, раздувание очередей (bufferbloat) на роутере (WAN-сторона) или у провайдера");
        } else if bad_g {
            log(WARN, "Шлюз деградирует под нагрузкой, внешний адрес нет: проверьте Wi-Fi и нагрузку на роутер");
        }
    }
    log("·", "Пауза 4 с: даём очередям освободиться перед следующими проверками");
    sleep(Duration::from_secs(4));
}

fn norm_url(s: &str) -> String {
    let s = s.trim();
    if !s.is_empty() && !s.contains("://") { format!("https://{s}") } else { s.to_string() }
}

pub fn check_user_sites(list: &[String]) -> Vec<String> {
    let mut failed = vec![];
    if list.is_empty() {
        return failed;
    }
    log(HDR, "Сайты, указанные пользователем");
    let Ok(cl) = http(8).build() else {
        log(BAD, "Не удалось создать HTTP-клиент");
        return failed;
    };
    for raw in list {
        let u = norm_url(raw);
        let h = match reqwest::Url::parse(&u).ok().and_then(|p| p.host_str().map(String::from)) {
            Some(h) => h,
            None => {
                log(WARN, format!("{raw}: неверный адрес"));
                continue;
            }
        };
        match dns::lookup_host(&h, Duration::from_secs(5)) {
            Err(e) => log(BAD, format!("{h}: системный DNS не резолвит ({e})")),
            Ok(v) => log(
                OK,
                format!("{h}: DNS -> {}", v.iter().map(|i| i.to_string()).collect::<Vec<_>>().join(", ")),
            ),
        }
        match cl.get(&u).send() {
            Err(e) => {
                log(BAD, format!("{u}: ОШИБКА: {e}"));
                match dns::udp_query("8.8.8.8", &h, Duration::from_secs(3)) {
                    Some(r) if !r.a.is_empty() => log(
                        WARN,
                        format!("{h} через 8.8.8.8: {}", r.a.iter().map(|i| i.to_string()).collect::<Vec<_>>().join(", ")),
                    ),
                    _ => log(WARN, format!("{h} через 8.8.8.8: тоже не резолвится")),
                }
                failed.push(h.clone());
                st().summ.sites.push(format!("{h}: НЕ ОТКРЫВАЕТСЯ"));
            }
            Ok(r) => {
                let code = r.status().as_u16();
                let _ = r.take(1024).read_to_end(&mut vec![]);
                log(OK, format!("{u}: {code}"));
                st().summ.sites.push(format!("{u}: {code}"));
            }
        }
    }
    failed
}

pub fn trace(host: &str) {
    log(HDR, format!("Трассировка до {host}"));
    let Some(ip) = dns::resolve4(host) else {
        log(BAD, format!("{host}: не резолвится"));
        return;
    };
    for ttl in 1..=20u8 {
        let (mut rtts, mut hop, mut done) = (vec![], None, false);
        for _ in 0..3 {
            match sys::icmp_echo(ip, ttl, Duration::from_secs(1)) {
                Err(e) => {
                    log(WARN, format!("ICMP недоступен: {e} (на Linux/macOS нужен root)"));
                    return;
                }
                Ok(r) => match r.hop {
                    None => rtts.push("*".to_string()),
                    Some(h) => {
                        hop = Some(h);
                        rtts.push(format!("{}мс", r.rtt.as_millis()));
                        done |= r.reached;
                    }
                },
            }
        }
        let hs = hop.map_or("*".to_string(), |h| h.to_string());
        log("·", format!("{ttl:>2}  {:<18} {hs}", rtts.join(" ")));
        if done {
            return;
        }
    }
}

pub fn log_summary() {
    let line = SEP.repeat(50);
    log(SEP, &line);
    log(HDR, "КРАТКОЕ РЕЗЮМЕ (для поддержки по телефону)");
    log(SEP, &line);
    let s = st().summ.clone();
    let p = |label: &str, v: &str| {
        if !v.is_empty() {
            log("·", format!("{label}: {v}"));
        }
    };
    p("Соединение", &s.conn);
    p("Линк", &s.link);
    p("IP", &s.ip4);
    p(&format!("IP ({IP_ALT_NAME})"), &s.ip_alt);
    p("IP через прокси", &s.ip_proxy);
    p("DNS", &s.dns);
    p("Upstream-DNS", &s.dns_up);
    p("Шлюз", &s.gw);
    for v in &s.pings {
        log("·", format!("Пинг {v}"));
    }
    p("Проблема DNS", &s.dns_problem);
    if s.down > 0.0 || s.up > 0.0 {
        log("·", format!("Скорость: загрузка {:.1}, отдача {:.1} Мбит/с", s.down, s.up));
    }
    for v in &s.bg {
        log("·", format!("Фоновая нагрузка {v}"));
    }
    for v in &s.load {
        log("·", format!("Под нагрузкой {v}"));
    }
    for v in &s.route {
        log("·", format!("Маршрут {v}"));
    }
    for v in &s.sites {
        log("·", format!("Сайт {v}"));
    }
    if ENABLE_IPV6 {
        p("IPv6", &s.ipv6);
    }
    log(SEP, &line);
}

/// Полный тест (в отдельном потоке).
/// Все проверки без итогового резюме (резюме печатается после, чтобы в него успел попасть MTR).
pub fn run_checks(list: Vec<String>) {
    log(HDR, format!("Тест {ISP_NAME} — {}", chrono::Local::now().format("%Y-%m-%d %H:%M:%S")));
    check_connection_info();
    check_background();
    let gw_ok = check_gateway();
    let online = internet_reachable();
    if !online {
        if !gw_ok {
            log(BAD, "Шлюз по умолчанию не задан и интернет недоступен: проверьте IP, маску, шлюз и кабель/Wi-Fi");
        } else {
            log(BAD, "Интернет недоступен (нет соединения с 1.1.1.1, 8.8.8.8, 77.88.8.8 по TCP/443)");
        }
        log(WARN, "Проверки, требующие интернет, пропущены");
        check_system_dns(false);
        return;
    }
    detect_ip();
    check_system_dns(true);
    check_ping();
    check_dns();
    check_sites();
    let failed = check_user_sites(&list);
    check_speed();
    let mut targets = vec!["8.8.8.8".to_string()];
    if let Some(h) = ISP_HOSTS.first() {
        targets.push(h.to_string());
    }
    targets.extend(failed);
    for t in targets {
        trace(&t);
    }
}

#[cfg_attr(not(feature = "gui"), allow(dead_code))]
pub fn run_all(list: Vec<String>) {
    run_checks(list);
    log_summary();
    log(HDR, "Готово.");
}

pub fn send_report(login: &str, note: &str) -> Result<(), String> {
    let (body, ip4, ip6, ipp, ipa, link) = {
        let g = st();
        (g.report.clone(), g.ip4.clone(), g.ip6.clone(), g.ip_proxy.clone(), g.ip_alt.clone(), g.link.clone())
    };
    let payload = serde_json::json!({
        "isp": ISP_NAME, "login": login, "version": env!("CARGO_PKG_VERSION"),
        "time": chrono::Local::now().to_rfc3339(), "ip4": ip4, "ip6": ip6, "ip_proxy": ipp, "ip_alt": ipa,
        "host": gethostname::gethostname().to_string_lossy(),
        "os": format!("{}/{}", std::env::consts::OS, std::env::consts::ARCH),
        "link": link, "note": note, "report": body,
    });
    let r = http(20)
        .build()
        .map_err(|e| e.to_string())?
        .post(REPORT_URL)
        .header("Content-Type", "application/json; charset=utf-8")
        .header("X-Report-Token", REPORT_TOKEN)
        .body(payload.to_string())
        .send()
        .map_err(|e| e.to_string())?;
    if r.status().is_success() { Ok(()) } else { Err(format!("сервер ответил {}", r.status())) }
}

#[cfg(test)]
mod repeat {
    use crate::state::*;

    /// Два теста подряд: второй не должен нести остатки первого. Долгий (нужна сеть), запуск: cargo test -- --ignored
    #[test]
    #[ignore]
    fn second_run_is_clean() {
        let _ = rustls::crypto::ring::default_provider().install_default();
        let snap = || {
            let g = st();
            (g.report.clone(), g.summ.pings.len(), g.summ.sites.len(), g.summ.route.len())
        };
        reset();
        super::run_all(vec![]);
        let first = snap();
        reset();
        assert!(st().report.is_empty() && st().summ.pings.is_empty() && st().link.is_none());
        super::run_all(vec![]);
        let second = snap();
        for (r, name) in [(&first.0, "1"), (&second.0, "2")] {
            assert_eq!(r.matches("КРАТКОЕ РЕЗЮМЕ").count(), 1, "запуск {name}");
            assert_eq!(r.matches("Готово.").count(), 1, "запуск {name}");
            assert_eq!(r.matches("Тест ").count(), 1, "запуск {name}");
        }
        assert_eq!((first.1, first.2, first.3), (second.1, second.2, second.3), "списки не должны копиться");
    }
}

#[cfg(test)]
mod load_tests {
    use super::*;

    #[test]
    fn stats_and_verdict() {
        let s = load_stat(&[10.0, 30.0, 20.0], 4).unwrap();
        assert_eq!((s.avg, s.max, s.loss), (20.0, 30.0, 25.0));
        assert!(load_stat(&[], 0).is_none());
        // нормально: рост < 100 мс, потерь нет
        assert!(!load_bad(&load_stat(&[20.0, 40.0], 2).unwrap(), Some(15.0)));
        // потери > 2%
        assert!(load_bad(&load_stat(&[20.0; 9], 10).unwrap(), Some(15.0)));
        // рост > 100 мс
        assert!(load_bad(&load_stat(&[150.0, 160.0], 2).unwrap(), Some(10.0)));
        // всплеск > 800 мс
        assert!(load_bad(&load_stat(&[20.0, 900.0], 2).unwrap(), None));
    }

    #[test]
    fn tcp_probe_measures_local_listener() {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        let stop = Arc::new(AtomicBool::new(false));
        let h = load_probe(Ipv4Addr::LOCALHOST, false, Some(port), stop.clone());
        sleep(Duration::from_millis(700));
        stop.store(true, Relaxed);
        let (ts, sent) = h.join().unwrap();
        assert!(sent >= 2 && ts.len() == sent, "sent={sent} ok={}", ts.len());
        // без ICMP и без TCP-порта проб нет
        let stop = Arc::new(AtomicBool::new(false));
        let (_, sent) = load_probe(Ipv4Addr::LOCALHOST, false, None, stop).join().unwrap();
        assert_eq!(sent, 0);
    }
}

#[cfg(test)]
mod ip_tests {
    use super::ip_differs;

    #[test]
    fn differs_only_within_same_family() {
        assert!(ip_differs("1.2.3.4", "5.6.7.8"));
        assert!(!ip_differs("1.2.3.4", "1.2.3.4"));
        assert!(!ip_differs("1.2.3.4", "2001:db8::1")); // разные семейства не сравниваем
        assert!(ip_differs("2001:db8::1", "2001:db8::2"));
        assert!(!ip_differs("", "1.2.3.4"));
    }
}

#[cfg(test)]
mod bg_tests {
    use super::bg_level;

    #[test]
    fn levels() {
        assert_eq!(bg_level(0.3, 0.1, 100.0), 0);
        assert_eq!(bg_level(2.5, 0.1, 1000.0), 1);
        assert_eq!(bg_level(12.0, 0.0, 1000.0), 2);
        assert_eq!(bg_level(0.5, 0.0, 0.0), 0);
        assert_eq!(bg_level(6.0, 0.0, 50.0), 2); // 6 Мбит/с = 12% линка 50 Мбит/с -> сильный
        assert_eq!(bg_level(1.9, 1.9, 1000.0), 0);
    }
}
