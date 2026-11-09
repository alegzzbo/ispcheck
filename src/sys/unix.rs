use super::Echo;
use socket2::{Domain, Protocol, SockAddr, Socket, Type};
use std::mem::MaybeUninit;
use std::net::{Ipv4Addr, SocketAddrV4};
use std::process::Command;
use std::time::{Duration, Instant};

fn checksum(b: &[u8]) -> u16 {
    let mut s = 0u32;
    for c in b.chunks(2) {
        s += u16::from_be_bytes([c[0], *c.get(1).unwrap_or(&0)]) as u32;
    }
    while s >> 16 != 0 {
        s = (s & 0xffff) + (s >> 16);
    }
    !(s as u16)
}

/// Raw ICMP (нужен root/CAP_NET_RAW); для обычного пинга fallback на DGRAM.
pub fn icmp_echo(ip: Ipv4Addr, ttl: u8, timeout: Duration) -> Result<Echo, String> {
    let mut raw = true;
    let sock = match Socket::new(Domain::IPV4, Type::RAW, Some(Protocol::ICMPV4)) {
        Ok(s) => s,
        Err(_) => {
            raw = false;
            Socket::new(Domain::IPV4, Type::DGRAM, Some(Protocol::ICMPV4)).map_err(|e| e.to_string())?
        }
    };
    sock.set_ttl_v4(ttl as u32).map_err(|e| e.to_string())?;
    let id = (std::process::id() & 0xffff) as u16;
    let seq = ttl as u16;
    let mut pkt = vec![8u8, 0, 0, 0, (id >> 8) as u8, id as u8, (seq >> 8) as u8, seq as u8];
    pkt.extend_from_slice(b"ispcheck");
    let c = checksum(&pkt);
    pkt[2] = (c >> 8) as u8;
    pkt[3] = c as u8;
    let start = Instant::now();
    sock.send_to(&pkt, &SockAddr::from(SocketAddrV4::new(ip, 0))).map_err(|e| e.to_string())?;
    let mut buf = [MaybeUninit::<u8>::uninit(); 1500];
    loop {
        let Some(left) = timeout.checked_sub(start.elapsed()) else {
            return Ok(Echo { hop: None, rtt: start.elapsed(), reached: false });
        };
        sock.set_read_timeout(Some(left.max(Duration::from_millis(1)))).ok();
        let Ok((n, peer)) = sock.recv_from(&mut buf) else {
            return Ok(Echo { hop: None, rtt: start.elapsed(), reached: false });
        };
        let b: Vec<u8> = buf[..n].iter().map(|x| unsafe { x.assume_init() }).collect();
        let b = if b.len() >= 20 && b[0] >> 4 == 4 { b[(b[0] & 0x0f) as usize * 4..].to_vec() } else { b };
        if b.len() < 8 {
            continue;
        }
        let hop = peer.as_socket_ipv4().map(|a| *a.ip());
        match b[0] {
            0 => {
                let (rid, rseq) = (u16::from_be_bytes([b[4], b[5]]), u16::from_be_bytes([b[6], b[7]]));
                if !raw || (rid == id && rseq == seq) {
                    return Ok(Echo { hop, rtt: start.elapsed(), reached: true });
                }
            }
            3 | 11 => {
                // внутри — IP-заголовок исходной пробы + 8 байт ICMP: сверяем id/seq
                let d = &b[8..];
                if d.len() >= 20 {
                    let ihl = (d[0] & 0x0f) as usize * 4;
                    if d.len() >= ihl + 8 {
                        let oid = u16::from_be_bytes([d[ihl + 4], d[ihl + 5]]);
                        let oseq = u16::from_be_bytes([d[ihl + 6], d[ihl + 7]]);
                        if oid == id && oseq == seq {
                            return Ok(Echo { hop, rtt: start.elapsed(), reached: false });
                        }
                    }
                }
            }
            _ => {}
        }
    }
}

/// Путь к утилите: в Debian/MX Linux `iw` лежит в /usr/sbin, которого нет в PATH обычного пользователя.
fn tool(name: &str) -> String {
    for d in ["/usr/sbin", "/sbin", "/usr/bin", "/bin", "/usr/local/bin"] {
        let p = format!("{d}/{name}");
        if std::path::Path::new(&p).exists() {
            return p;
        }
    }
    name.to_string()
}

fn cmd(c: &str, args: &[&str]) -> String {
    Command::new(c)
        .args(args)
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
        .unwrap_or_default()
}

pub fn default_gateway() -> Result<Ipv4Addr, String> {
    if let Ok(t) = std::fs::read_to_string("/proc/net/route") {
        // маршрут 0.0.0.0 с флагом RTF_GATEWAY и минимальной метрикой
        let mut best: Option<(u64, Ipv4Addr)> = None;
        for l in t.lines().skip(1) {
            let f: Vec<&str> = l.split_whitespace().collect();
            if f.len() < 8 || f[1] != "00000000" {
                continue;
            }
            let flags = u32::from_str_radix(f[3], 16).unwrap_or(0);
            let gw = u32::from_str_radix(f[2], 16).unwrap_or(0);
            let metric: u64 = f[6].parse().unwrap_or(u64::MAX);
            if flags & 2 == 0 || gw == 0 || best.map_or(false, |b| metric >= b.0) {
                continue;
            }
            best = Some((metric, Ipv4Addr::from(gw.to_ne_bytes())));
        }
        return best.map(|b| b.1).ok_or_else(|| "маршрут по умолчанию не найден".into());
    }
    // macOS/BSD
    for l in cmd("route", &["-n", "get", "default"]).lines() {
        if let Some(v) = l.trim().strip_prefix("gateway:") {
            if let Ok(ip) = v.trim().parse() {
                return Ok(ip);
            }
        }
    }
    Err("маршрут по умолчанию не найден".into())
}

fn default_iface() -> String {
    if let Ok(t) = std::fs::read_to_string("/proc/net/route") {
        for l in t.lines().skip(1) {
            let f: Vec<&str> = l.split_whitespace().collect();
            if f.len() > 1 && f[1] == "00000000" {
                return f[0].to_string();
            }
        }
        return String::new();
    }
    for l in cmd("route", &["-n", "get", "default"]).lines() {
        if let Some(v) = l.trim().strip_prefix("interface:") {
            return v.trim().to_string();
        }
    }
    String::new()
}

pub fn connection_type() -> String {
    let i = default_iface();
    if i.is_empty() {
        return "неизвестно".into();
    }
    if cfg!(target_os = "linux") {
        if is_wireless(&i) {
            return format!("Wi-Fi ({i})");
        }
        if std::fs::read_to_string(format!("/sys/class/net/{i}/type")).map_or(false, |t| t.trim() == "1") {
            return format!("Ethernet, кабель ({i})");
        }
    } else if i.starts_with("en") {
        #[cfg(target_os = "macos")]
        if let Some(w) = super::macos::wifi() {
            if w.iface == i {
                return format!("Wi-Fi ({i})");
            }
        }
        let o = cmd("/usr/sbin/networksetup", &["-getairportnetwork", &i]);
        if i == "en0" && !o.is_empty() && !o.contains("not associated") && !o.contains("not a Wi-Fi") {
            return format!("Wi-Fi ({i})");
        }
        return format!("Ethernet, кабель ({i})");
    }
    format!("неизвестно ({i})")
}

pub fn system_dns() -> Vec<String> {
    let mut v: Vec<String> = vec![];
    if cfg!(target_os = "macos") {
        for l in cmd("scutil", &["--dns"]).lines() {
            if let Some(r) = l.trim().strip_prefix("nameserver") {
                if let Some(ip) = r.split_whitespace().last() {
                    if ip.parse::<std::net::IpAddr>().is_ok() && !v.iter().any(|x| x == ip) {
                        v.push(ip.to_string());
                    }
                }
            }
        }
    }
    if v.is_empty() {
        for l in std::fs::read_to_string("/etc/resolv.conf").unwrap_or_default().lines() {
            let f: Vec<&str> = l.split_whitespace().collect();
            if f.len() >= 2 && f[0] == "nameserver" && f[1].parse::<std::net::IpAddr>().is_ok() {
                v.push(f[1].to_string());
            }
        }
    }
    v
}

/// Разбор вывода `scutil --proxy` (macOS): (прокси-URL, адрес PAC).
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn parse_scutil_proxy(out: &str) -> (String, String) {
    let mut m = std::collections::HashMap::new();
    for l in out.lines() {
        if let Some((k, v)) = l.split_once(" : ") {
            m.insert(k.trim().to_string(), v.trim().to_string());
        }
    }
    let on = |k: &str| m.get(k).map_or(false, |v| v == "1");
    let hp = |pre: &str| -> Option<String> {
        if !on(&format!("{pre}Enable")) {
            return None;
        }
        let h = m.get(&format!("{pre}Proxy"))?;
        Some(match m.get(&format!("{pre}Port")) {
            Some(p) => format!("{h}:{p}"),
            None => h.clone(),
        })
    };
    let proxy = hp("HTTPS")
        .map(|v| format!("http://{v}"))
        .or_else(|| hp("HTTP").map(|v| format!("http://{v}")))
        .or_else(|| hp("SOCKS").map(|v| format!("socks5://{v}")))
        .unwrap_or_default();
    let pac = if on("ProxyAutoConfigEnable") { m.get("ProxyAutoConfigURLString").cloned().unwrap_or_default() } else { String::new() };
    (proxy, pac)
}

pub fn system_proxy() -> String {
    if cfg!(target_os = "macos") {
        let (p, _) = parse_scutil_proxy(&cmd("scutil", &["--proxy"]));
        if !p.is_empty() {
            return p;
        }
    }
    for e in ["HTTPS_PROXY", "https_proxy", "HTTP_PROXY", "http_proxy"] {
        if let Ok(v) = std::env::var(e) {
            if !v.is_empty() {
                return if v.contains("://") { v } else { format!("http://{v}") };
            }
        }
    }
    String::new()
}

pub fn pac_url() -> String {
    if cfg!(target_os = "macos") {
        return parse_scutil_proxy(&cmd("scutil", &["--proxy"])).1;
    }
    String::new()
}

fn num_before(s: &str, key: &str) -> Option<f64> {
    s.lines().find_map(|l| {
        let l = l.trim();
        let r = l.strip_prefix(key)?;
        r.split_whitespace().next()?.parse().ok()
    })
}

/// Данные Wi-Fi из разных источников (iw, iwconfig, nmcli).
#[derive(Default, Debug, PartialEq)]
struct Wifi {
    ssid: String,
    dbm: Option<i32>,
    pct: Option<u32>,
    freq: u32,
    rx: f64,
    tx: f64,
}

impl Wifi {
    /// Дополняет пустые поля данными из другого источника.
    fn fill(&mut self, o: Wifi) {
        if self.ssid.is_empty() {
            self.ssid = o.ssid;
        }
        self.dbm = self.dbm.or(o.dbm);
        self.pct = self.pct.or(o.pct);
        if self.freq == 0 {
            self.freq = o.freq;
        }
        if self.rx <= 0.0 {
            self.rx = o.rx;
        }
        if self.tx <= 0.0 {
            self.tx = o.tx;
        }
    }
}

/// `iw dev <if> link`
fn parse_iw_link(o: &str) -> Wifi {
    let mut w = Wifi::default();
    for ln in o.lines() {
        if let Some(v) = ln.trim().strip_prefix("SSID:") {
            w.ssid = v.trim().to_string();
        }
    }
    w.dbm = num_before(o, "signal:").map(|v| v as i32);
    w.freq = num_before(o, "freq:").unwrap_or(0.0) as u32;
    w.rx = num_before(o, "rx bitrate:").unwrap_or(0.0);
    w.tx = num_before(o, "tx bitrate:").unwrap_or(0.0);
    w
}

/// `iwconfig <if>`: «Bit Rate=866.7 Mb/s», «Frequency:5.18 GHz», «Signal level=-50 dBm», ESSID:"имя"
fn parse_iwconfig(o: &str) -> Wifi {
    let after = |key: &str| -> Option<String> {
        let p = o.find(key)? + key.len();
        Some(o[p..].trim_start().split(|c: char| c.is_whitespace()).next()?.to_string())
    };
    let mut w = Wifi::default();
    if let Some(p) = o.find("ESSID:\"") {
        let r = &o[p + 7..];
        w.ssid = r.split('"').next().unwrap_or("").to_string();
    }
    if let Some(v) = after("Bit Rate=").or_else(|| after("Bit Rate:")) {
        w.rx = v.parse().unwrap_or(0.0);
        // единицы: Mb/s, Gb/s, kb/s
        let tail = o[o.find("Bit Rate").unwrap_or(0)..].lines().next().unwrap_or("");
        if tail.contains("Gb/s") {
            w.rx *= 1000.0;
        } else if tail.contains("kb/s") {
            w.rx /= 1000.0;
        }
        w.tx = w.rx;
    }
    if let Some(v) = after("Frequency:").or_else(|| after("Frequency=")) {
        if let Ok(g) = v.parse::<f64>() {
            w.freq = (g * 1000.0).round() as u32; // ГГц -> МГц
        }
    }
    if let Some(v) = after("Signal level=").or_else(|| after("Signal level:")) {
        w.dbm = v.parse().ok();
    }
    w
}

/// `nmcli -t -f ACTIVE,SSID,FREQ,RATE,SIGNAL dev wifi list`: строка «yes:Имя:5180 MHz:540 Mbit/s:80»
fn parse_nmcli(o: &str) -> Wifi {
    for l in o.lines() {
        // SSID может содержать экранированные двоеточия (\:), поэтому поля берём с конца
        let parts: Vec<&str> = l.rsplitn(4, ':').collect(); // SIGNAL, RATE, FREQ, "yes:SSID"
        if parts.len() != 4 {
            continue;
        }
        let Some((active, ssid)) = parts[3].split_once(':') else { continue };
        if active != "yes" {
            continue;
        }
        let num = |s: &str| s.split_whitespace().next().and_then(|x| x.parse::<f64>().ok()).unwrap_or(0.0);
        let rate = num(parts[1]);
        return Wifi {
            ssid: ssid.replace("\\:", ":"),
            dbm: None,
            pct: parts[0].trim().parse().ok(),
            freq: num(parts[2]) as u32,
            rx: rate,
            tx: rate,
        };
    }
    Wifi::default()
}

fn is_wireless(i: &str) -> bool {
    ["wireless", "phy80211"].iter().any(|d| std::path::Path::new(&format!("/sys/class/net/{i}/{d}")).exists())
}

fn linux_wifi(i: &str) -> Wifi {
    let mut w = parse_iw_link(&cmd(&tool("iw"), &["dev", i, "link"]));
    if w.rx <= 0.0 || w.tx <= 0.0 || w.ssid.is_empty() || w.freq == 0 {
        w.fill(parse_iwconfig(&cmd(&tool("iwconfig"), &[i])));
    }
    if w.rx <= 0.0 && w.tx <= 0.0 {
        w.fill(parse_nmcli(&cmd("nmcli", &["-t", "-f", "ACTIVE,SSID,FREQ,RATE,SIGNAL", "dev", "wifi", "list", "ifname", i])));
    }
    // уровень сигнала из ядра: /proc/net/wireless (колонка level, дБм)
    if w.dbm.is_none() {
        for l in std::fs::read_to_string("/proc/net/wireless").unwrap_or_default().lines() {
            if let Some(r) = l.trim().strip_prefix(&format!("{i}:")) {
                let f: Vec<&str> = r.split_whitespace().collect();
                if let Some(v) = f.get(2).and_then(|x| x.trim_end_matches('.').parse::<f64>().ok()) {
                    if v < 0.0 {
                        w.dbm = Some(v as i32);
                    }
                }
            }
        }
    }
    // многие драйверы отдают только одно из направлений
    if w.rx <= 0.0 {
        w.rx = w.tx;
    }
    if w.tx <= 0.0 {
        w.tx = w.rx;
    }
    w
}

/// Linux: /sys/class/net, iw / iwconfig / nmcli; macOS: CoreWLAN (Wi-Fi) и ifconfig (Ethernet).
pub fn link_info() -> Option<super::Link> {
    let i = default_iface();
    if i.is_empty() {
        return None;
    }
    let mut l = super::Link { name: i.clone(), kind: "other".into(), ..Default::default() };
    if cfg!(target_os = "linux") {
        if is_wireless(&i) {
            l.kind = "wifi".into();
            let w = linux_wifi(&i);
            l.ssid = w.ssid;
            l.signal_dbm = w.dbm;
            l.signal_pct = w.pct;
            l.rx_mbps = w.rx;
            l.tx_mbps = w.tx;
            l.set_freq(w.freq);
        } else {
            l.kind = "ethernet".into();
            let sp: f64 = std::fs::read_to_string(format!("/sys/class/net/{i}/speed"))
                .ok()
                .and_then(|t| t.trim().parse().ok())
                .unwrap_or(0.0);
            if sp > 0.0 {
                l.rx_mbps = sp;
                l.tx_mbps = sp;
            }
            l.duplex = std::fs::read_to_string(format!("/sys/class/net/{i}/duplex")).unwrap_or_default().trim().to_string();
        }
    } else {
        #[cfg(target_os = "macos")]
        if let Some(w) = super::macos::wifi() {
            if w.iface == i {
                l.kind = "wifi".into();
                l.ssid = w.ssid;
                l.rx_mbps = w.rate;
                l.tx_mbps = w.rate;
                l.signal_dbm = Some(w.rssi);
                l.channel = w.channel;
                l.band = w.band.into();
                l.phy = w.phy.into();
                return Some(l);
            }
        }
        l.kind = "ethernet".into();
        let o = cmd("ifconfig", &[&i]);
        if let Some(m) = o.lines().find(|x| x.trim_start().starts_with("media:")) {
            if let Some(p) = m.find("base") {
                let digits: String = m[..p].chars().rev().take_while(|c| c.is_ascii_digit()).collect::<Vec<_>>().into_iter().rev().collect();
                if let Ok(v) = digits.parse::<f64>() {
                    l.rx_mbps = v;
                    l.tx_mbps = v;
                }
            }
            if m.contains("full-duplex") {
                l.duplex = "full".into();
            }
        }
    }
    Some(l)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wifi_parsers() {
        let iw = "Connected to 4c:ed:fb:00:00:00 (on wlp2s0)\n\tSSID: My Net\n\tfreq: 5180\n\tsignal: -52 dBm\n\trx bitrate: 866.7 MBit/s VHT-MCS 9 80MHz short GI VHT-NSS 2\n\ttx bitrate: 780.0 MBit/s VHT-MCS 8\n";
        let w = parse_iw_link(iw);
        assert_eq!((w.ssid.as_str(), w.dbm, w.freq, w.rx, w.tx), ("My Net", Some(-52), 5180, 866.7, 780.0));
        assert_eq!(parse_iw_link("Not connected.\n"), Wifi::default());

        let wc = "wlan0     IEEE 802.11  ESSID:\"HomeNet\"  \n          Mode:Managed  Frequency:2.437 GHz  Access Point: AA:BB:CC:DD:EE:FF   \n          Bit Rate=72.2 Mb/s   Tx-Power=20 dBm   \n          Link Quality=60/70  Signal level=-50 dBm\n";
        let w = parse_iwconfig(wc);
        assert_eq!((w.ssid.as_str(), w.dbm, w.freq, w.rx), ("HomeNet", Some(-50), 2437, 72.2));

        let nm = "no:Other:2412 MHz:54 Mbit/s:40\nyes:Cafe\\:Guest:5180 MHz:540 Mbit/s:80\n";
        let w = parse_nmcli(nm);
        assert_eq!((w.ssid.as_str(), w.pct, w.freq, w.rx), ("Cafe:Guest", Some(80), 5180, 540.0));

        // недостающее добирается из другого источника
        let mut a = parse_iw_link("\tSSID: X\n\tsignal: -60 dBm\n");
        a.fill(parse_iwconfig("Bit Rate=300 Mb/s  Frequency:5.18 GHz"));
        assert_eq!((a.rx, a.freq, a.ssid.as_str()), (300.0, 5180, "X"));
    }

    use super::parse_scutil_proxy;

    #[test]
    fn scutil_proxy() {
        let out = "<dictionary> {\n  ExceptionsList : <array> {\n    0 : *.local\n  }\n  HTTPEnable : 1\n  HTTPPort : 8080\n  HTTPProxy : proxy.corp.lan\n  HTTPSEnable : 0\n  ProxyAutoConfigEnable : 1\n  ProxyAutoConfigURLString : http://wpad.corp.lan/proxy.pac\n}\n";
        let (p, pac) = parse_scutil_proxy(out);
        assert_eq!(p, "http://proxy.corp.lan:8080");
        assert_eq!(pac, "http://wpad.corp.lan/proxy.pac");
        let (p, pac) = parse_scutil_proxy("<dictionary> {\n  SOCKSEnable : 1\n  SOCKSPort : 1080\n  SOCKSProxy : 10.0.0.1\n}\n");
        assert_eq!((p.as_str(), pac.as_str()), ("socks5://10.0.0.1:1080", ""));
        assert_eq!(parse_scutil_proxy("<dictionary> {\n  HTTPEnable : 0\n}\n").0, "");
    }
}
