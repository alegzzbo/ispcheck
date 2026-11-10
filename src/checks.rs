use crate::config::*;
use crate::dns;
use crate::state::*;
use crate::sys;
use reqwest::blocking::{Body, Client, ClientBuilder};
use std::io::{self, Read};
use std::net::{IpAddr, SocketAddr, TcpStream, ToSocketAddrs};
use std::sync::atomic::{AtomicU64, Ordering::Relaxed};
use std::sync::Arc;
use std::thread::sleep;
use std::time::{Duration, Instant};

const MS: Duration = Duration::from_millis(1);

fn http(secs: u64) -> ClientBuilder {
    // no_proxy: прямое соединение, системный прокси проверяется отдельно
    Client::builder().no_proxy().user_agent("Mozilla/5.0").timeout(Duration::from_secs(secs))
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
            .map(|p| !get_ip("https://api.ipify.org", Client::builder().proxy(p).timeout(Duration::from_secs(5))).is_empty())
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
        Ok(p) => get_ip("https://api.ipify.org", Client::builder().proxy(p).timeout(Duration::from_secs(8))),
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
fn speed(down: bool, latency_target: Option<SocketAddr>) -> (f64, crate::latency::Samples) {
    speed_measure(down, latency_target, OST_URL, Duration::from_secs(SPEED_SECONDS), Duration::from_secs(2), THREADS)
}

fn pick_upload_path(client: &Client, url: &str) -> String {
    let mut seen = vec![];
    for path in ["/upload", "/", "/upload.php"] {
        match client.post(format!("{url}{path}")).timeout(Duration::from_secs(4)).body(vec![0u8; 1024]).send() {
            Ok(r) if r.status().is_success() => return path.to_string(),
            Ok(r) => seen.push(format!("{path}: HTTP {}", r.status().as_u16())),
            Err(e) => seen.push(format!("{path}: {e}")),
        }
    }
    log(WARN, format!("Ни один путь не принимает POST ({}): отдача может не измеряться", seen.join("; ")));
    "/upload".to_string()
}

fn speed_measure(
    down: bool,
    latency_target: Option<SocketAddr>,
    url: &str,
    duration: Duration,
    warmup: Duration,
    threads: usize,
) -> (f64, crate::latency::Samples) {
    let client = Client::builder()
        .no_proxy()
        .http1_only()
        .pool_max_idle_per_host(threads * 2)
        .connect_timeout(Duration::from_secs(5))
        .timeout(duration.max(MS))
        .user_agent("Mozilla/5.0")
        .build();
    let Ok(client) = client else {
        log(BAD, "Не удалось создать HTTP-клиент для замера скорости");
        return (0.0, Default::default());
    };
    let total = Arc::new(AtomicU64::new(0));
    let successful = Arc::new(AtomicU64::new(0));
    let rejected = Arc::new(AtomicU64::new(0));
    let first_err = Arc::new(std::sync::Mutex::new(None::<String>));
    let payload = Arc::new(if down {
        vec![]
    } else {
        (0..1 << 20).map(|_| rnd() as u8).collect::<Vec<u8>>()
    });
    // Сервер отвечает 405 на POST /upload: подбираем путь, который принимает POST.
    let upload_path = if down { String::new() } else { pick_upload_path(&client, url) };
    let end = Instant::now() + duration;
    let mut workers = Vec::new();
    for _ in 0..threads {
        let (c, total, payload, successful, url) = (client.clone(), total.clone(), payload.clone(), successful.clone(), url.to_string());
        let upload_path = upload_path.clone();
        let (rejected, first_err) = (rejected.clone(), first_err.clone());
        workers.push(std::thread::spawn(move || {
            let mut buf = vec![0u8; 256 << 10];
            while let Some(left) = end.checked_duration_since(Instant::now()) {
                if left < MS { break; }
                if down {
                    match c.get(format!("{url}/downloading?n={}", rnd())).timeout(left).send() {
                        Ok(mut r) if r.status().is_success() => {
                            successful.fetch_add(1, Relaxed);
                            while Instant::now() < end {
                                match r.read(&mut buf) {
                                    Ok(0) | Err(_) => break,
                                    Ok(n) => { total.fetch_add(n as u64, Relaxed); }
                                }
                            }
                        }
                        _ => sleep((300 * MS).min(end.saturating_duration_since(Instant::now()))),
                    }
                } else {
                    let len = payload.len() as u64;
                    let body = Body::sized(CountRead { data: payload.clone(), pos: 0, total: total.clone(), end }, len);
                    match c.post(format!("{url}{upload_path}")).timeout(left).body(body).send() {
                        Ok(r) if r.status().is_success() => {
                            successful.fetch_add(1, Relaxed);
                            let _ = r.bytes();
                        }
                        Ok(r) => {
                            rejected.fetch_add(1, Relaxed);
                            first_err.lock().unwrap_or_else(|e| e.into_inner()).get_or_insert(format!("HTTP {}", r.status()));
                            sleep((300 * MS).min(end.saturating_duration_since(Instant::now())));
                        }
                        Err(e) => {
                            if end.saturating_duration_since(Instant::now()) > 500 * MS {
                                first_err.lock().unwrap_or_else(|e| e.into_inner()).get_or_insert(format!("{e:?}"));
                            }
                            sleep((300 * MS).min(end.saturating_duration_since(Instant::now())));
                        }
                    }
                }
            }
        }));
    }
    // Исключаем TCP slow start и измеряем задержку только в оставшемся окне нагрузки.
    sleep(warmup.min(end.saturating_duration_since(Instant::now())));
    let (base, t0) = (total.load(Relaxed), Instant::now());
    let sampler = latency_target.map(|target| std::thread::spawn(move || crate::latency::under_load(target, end)));
    sleep(end.saturating_duration_since(Instant::now()));
    let bytes = total.load(Relaxed).saturating_sub(base);
    let elapsed = t0.elapsed().as_secs_f64();
    // Не оставляем трафик предыдущего направления в следующем замере.
    for worker in workers {
        if worker.join().is_err() {
            log(WARN, "Один из потоков замера скорости завершился с ошибкой");
        }
    }
    let samples = sampler.and_then(|h| h.join().ok()).unwrap_or_default();
    // Отдача: на медленном канале запрос может не завершиться до конца окна; это не ошибка,
    // если сервер ничего не отклонял и байты реально ушли.
    let confirmed = successful.load(Relaxed) > 0 || (!down && rejected.load(Relaxed) == 0 && bytes > 0);
    if let Some(e) = first_err.lock().unwrap_or_else(|e| e.into_inner()).as_ref() {
        log(WARN, format!("Отдача: ошибка запроса к серверу: {e}"));
    }
    let value = if confirmed && elapsed > 0.0 {
        bytes as f64 * 8.0 / elapsed / 1e6
    } else {
        log(BAD, "Сервер не подтвердил успешные передачи: замер скорости недействителен");
        0.0
    };
    (value, samples)
}

pub fn check_speed() {
    log(HDR, "Скорость (OpenSpeedTest)");
    if let Err(e) = ost_reachable() {
        log(BAD, format!("{OST_URL} недоступен ({e}): замер скорости пропущен"));
        return;
    }
    log(HDR, "Задержка под нагрузкой (bufferbloat)");
    log("·", "Сравнивается время TCP-подключения к одному адресу в покое и во время замера скорости; это не ICMP-пинг.");
    let mut latency = None;
    for address in LATENCY_TARGETS {
        let Ok(target) = address.parse::<SocketAddr>() else { continue };
        let idle = crate::latency::baseline(target);
        if idle.rtts.len() >= 3 {
            log(if idle.loss() > 0.0 { WARN } else { OK }, format!(
                "В покое [{target}, TCP]: {:.1} мс, ответов {}/{}, без ответа {:.0}%",
                idle.mean().unwrap_or_default(), idle.rtts.len(), idle.sent, idle.loss()
            ));
            latency = Some((target, idle));
            break;
        }
    }
    if latency.is_none() {
        let reason = "не удалось получить достаточно ответов в покое; оценка bufferbloat пропущена";
        log(WARN, reason);
        st().summ.loaded_latency.push(reason.into());
    }
    let link = st().link.clone();
    for (name, down) in [("Загрузка", true), ("Отдача", false)] {
        let (v, loaded) = speed(down, latency.as_ref().map(|(target, _)| *target));
        if down {
            st().summ.down = v;
        } else {
            st().summ.up = v;
        }
        log(if v < 1.0 { BAD } else { OK }, format!("{name}: {v:.1} Мбит/с"));
        if let Some((target, idle)) = &latency {
            let (warn, detail) = if v > 0.0 {
                crate::latency::describe(idle, &loaded)
            } else {
                (true, "замер скорости не создал подтверждённую нагрузку; оценка bufferbloat недоступна".into())
            };
            let summary = format!("{name} [{target}, TCP]: {detail}");
            log(if warn { WARN } else { OK }, &summary);
            st().summ.loaded_latency.push(summary);
        }
        if let Some(l) = &link {
            let lim = if down { l.rx_mbps } else { l.tx_mbps };
            if lim > 0.0 && v > lim * 0.8 {
                log(WARN, format!("Результат близок к скорости линка ({lim:.0} Мбит/с): замер упирается в линк, а не в провайдера"));
            }
        }
    }
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
    p("IP через прокси", &s.ip_proxy);
    p("DNS", &s.dns);
    p("Upstream-DNS", &s.dns_up);
    p("Шлюз", &s.gw);
    p("Captive portal", &s.portal);
    for result in &s.loaded_latency {
        p("Задержка под нагрузкой", result);
    }
    for v in &s.pings {
        log("·", format!("Пинг {v}"));
    }
    p("Проблема DNS", &s.dns_problem);
    if s.down > 0.0 || s.up > 0.0 {
        log("·", format!("Скорость: загрузка {:.1}, отдача {:.1} Мбит/с", s.down, s.up));
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
pub fn run_all(list: Vec<String>) {
    log(HDR, format!("Тест {ISP_NAME} — {}", chrono::Local::now().format("%Y-%m-%d %H:%M:%S")));
    check_connection_info();
    let gw_ok = check_gateway();
    // HTTP-перехват проверяем и при недоступном TCP/443: портал может блокировать HTTPS.
    let portal = crate::portal::check();
    st().summ.portal = portal;
    let online = internet_reachable();
    if !online {
        if !gw_ok {
            log(BAD, "Шлюз по умолчанию не задан и интернет недоступен: проверьте IP, маску, шлюз и кабель/Wi-Fi");
        } else {
            log(BAD, "Интернет недоступен (нет соединения с 1.1.1.1, 8.8.8.8, 77.88.8.8 по TCP/443)");
        }
        log(WARN, "Проверки, требующие интернет, пропущены");
        check_system_dns(false);
        log_summary();
        log(HDR, "Готово.");
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
    log_summary();
    log(HDR, "Готово.");
}

pub fn send_report(login: &str, note: &str) -> Result<(), String> {
    let (body, ip4, ip6, ipp, link) = {
        let g = st();
        (g.report.clone(), g.ip4.clone(), g.ip6.clone(), g.ip_proxy.clone(), g.link.clone())
    };
    let payload = serde_json::json!({
        "isp": ISP_NAME, "login": login, "version": env!("CARGO_PKG_VERSION"),
        "time": chrono::Local::now().to_rfc3339(), "ip4": ip4, "ip6": ip6, "ip_proxy": ipp,
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
