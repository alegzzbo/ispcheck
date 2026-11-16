use super::Echo;
use std::ffi::c_void;
use std::net::{IpAddr, Ipv4Addr, UdpSocket};
use std::time::{Duration, Instant};
use windows_sys::Win32::Foundation::{ERROR_BUFFER_OVERFLOW, INVALID_HANDLE_VALUE};
use windows_sys::Win32::NetworkManagement::IpHelper::*;
use winreg::enums::HKEY_CURRENT_USER;
use winreg::RegKey;

/// ICMP через iphlpapi.dll: без прав администратора и внешних процессов.
pub fn icmp_echo(ip: Ipv4Addr, ttl: u8, timeout: Duration) -> Result<Echo, String> {
    unsafe {
        let h = IcmpCreateFile();
        if h == INVALID_HANDLE_VALUE || h.is_null() {
            return Err("IcmpCreateFile".into());
        }
        let opt = IP_OPTION_INFORMATION {
            Ttl: ttl,
            Tos: 0,
            Flags: 0,
            OptionsSize: 0,
            OptionsData: std::ptr::null_mut(),
        };
        let data = b"ispcheck-echo-payload-32bytes!!!";
        let mut buf = vec![0u64; 128];
        let start = Instant::now();
        let r = IcmpSendEcho(
            h,
            u32::from_ne_bytes(ip.octets()),
            data.as_ptr() as *const c_void,
            data.len() as u16,
            &opt,
            buf.as_mut_ptr() as *mut c_void,
            (buf.len() * 8) as u32,
            timeout.as_millis() as u32,
        );
        let rtt = start.elapsed();
        IcmpCloseHandle(h);
        if r == 0 {
            return Ok(Echo { hop: None, rtt, reached: false });
        }
        let rep = &*(buf.as_ptr() as *const ICMP_ECHO_REPLY);
        Ok(Echo {
            hop: Some(Ipv4Addr::from(rep.Address.to_ne_bytes())),
            rtt,
            reached: rep.Status == 0,
        })
    }
}

/// Шлюз лучшего маршрута до 8.8.8.8.
pub fn default_gateway() -> Result<Ipv4Addr, String> {
    unsafe {
        let mut row: MIB_IPFORWARDROW = std::mem::zeroed();
        let r = GetBestRoute(u32::from_ne_bytes([8, 8, 8, 8]), 0, &mut row);
        if r != 0 {
            return Err(format!("GetBestRoute: код {r} (нет маршрута по умолчанию)"));
        }
        if row.dwForwardNextHop == 0 {
            return Err("маршрут без шлюза".into());
        }
        Ok(Ipv4Addr::from(row.dwForwardNextHop.to_ne_bytes()))
    }
}

unsafe fn sockaddr_ip(sa: *const u8) -> Option<IpAddr> {
    if sa.is_null() {
        return None;
    }
    match *(sa as *const u16) {
        2 => {
            let b = std::slice::from_raw_parts(sa.add(4), 4);
            Some(IpAddr::from([b[0], b[1], b[2], b[3]]))
        }
        23 => {
            let mut a = [0u8; 16];
            a.copy_from_slice(std::slice::from_raw_parts(sa.add(8), 16));
            Some(IpAddr::from(a))
        }
        _ => None,
    }
}

unsafe fn wide(p: *const u16) -> String {
    if p.is_null() {
        return String::new();
    }
    let mut n = 0;
    while *p.add(n) != 0 {
        n += 1;
    }
    String::from_utf16_lossy(std::slice::from_raw_parts(p, n))
}

/// Активный адаптер — тот, чей адрес выбирается для маршрута до 8.8.8.8:
/// (IfType, имя, DNS-серверы адаптера).
struct Adapter {
    if_type: u32,
    name: String,
    dns: Vec<String>,
    rx_bps: u64,
    tx_bps: u64,
    up: bool, // OperStatus == Up (кабель/сеть подключены)
}

/// Скорость линка в Мбит/с; Windows отдаёт u64::MAX (-1), если скорость неизвестна.
fn mbps(bps: u64) -> f64 {
    if bps == 0 || bps >= 1 << 50 {
        0.0
    } else {
        bps as f64 / 1e6
    }
}

/// Активный адаптер: тот, через который идёт маршрут до 8.8.8.8. Адаптеры с отключённым кабелем
/// (OperStatus != Up) пропускаются: берётся рабочий адаптер со шлюзом и наименьшей метрикой.
fn active_adapter() -> Option<Adapter> {
    let local = UdpSocket::bind("0.0.0.0:0").ok().and_then(|s| {
        s.connect("8.8.8.8:53").ok()?; // пакеты не отправляются
        match s.local_addr().ok()?.ip() {
            IpAddr::V4(v) => Some(v),
            _ => None,
        }
    });
    // (адаптер, адрес маршрута принадлежит ему, есть шлюз и IPv4, метрика)
    let mut all: Vec<(Adapter, bool, bool, u32)> = vec![];
    let mut size = 15000u32;
    for _ in 0..3 {
        let mut buf = vec![0u64; (size as usize + 7) / 8];
        let head = buf.as_mut_ptr() as *mut IP_ADAPTER_ADDRESSES_LH;
        let r = unsafe {
            GetAdaptersAddresses(0, GAA_FLAG_SKIP_ANYCAST | GAA_FLAG_SKIP_MULTICAST, std::ptr::null(), head, &mut size)
        };
        if r == ERROR_BUFFER_OVERFLOW {
            continue;
        }
        if r != 0 {
            return None;
        }
        unsafe {
            let mut a = head as *const IP_ADAPTER_ADDRESSES_LH;
            while !a.is_null() {
                let ad = &*a;
                a = ad.Next;
                let (mut has_v4, mut is_local) = (false, false);
                let mut u = ad.FirstUnicastAddress;
                while !u.is_null() {
                    if let Some(IpAddr::V4(v)) = sockaddr_ip((*u).Address.lpSockaddr as *const u8) {
                        has_v4 = true;
                        is_local |= Some(v) == local;
                    }
                    u = (*u).Next;
                }
                let mut dns: Vec<String> = vec![];
                let mut d = ad.FirstDnsServerAddress;
                while !d.is_null() {
                    if let Some(ip) = sockaddr_ip((*d).Address.lpSockaddr as *const u8) {
                        let s = ip.to_string();
                        // заглушки Windows fec0:0:0:ffff::1-3
                        if !s.starts_with("fec0:0:0:ffff:") && !dns.contains(&s) {
                            dns.push(s);
                        }
                    }
                    d = (*d).Next;
                }
                let has_gw = has_v4 && !ad.FirstGatewayAddress.is_null();
                all.push((
                    Adapter {
                        if_type: ad.IfType,
                        name: wide(ad.FriendlyName),
                        dns,
                        rx_bps: ad.ReceiveLinkSpeed,
                        tx_bps: ad.TransmitLinkSpeed,
                        up: ad.OperStatus == 1,
                    },
                    is_local,
                    has_gw,
                    ad.Ipv4Metric,
                ));
            }
        }
        break;
    }
    // 1) адаптер маршрута, если он подключён; 2) лучший подключённый со шлюзом; 3) адаптер маршрута как есть
    if let Some(i) = all.iter().position(|a| a.1 && a.0.up) {
        return Some(all.swap_remove(i).0);
    }
    let best = all
        .iter()
        .enumerate()
        .filter(|(_, a)| a.0.up && a.2 && a.0.if_type != 24 && a.0.if_type != 131)
        .min_by_key(|(_, a)| a.3)
        .map(|(i, _)| i);
    if let Some(i) = best {
        return Some(all.swap_remove(i).0);
    }
    let i = all.iter().position(|a| a.1)?;
    Some(all.swap_remove(i).0)
}

pub fn connection_type() -> String {
    match active_adapter() {
        None => "неизвестно".into(),
        Some(a) if !a.up => format!("{} — нет подключения", a.name),
        Some(a) => match a.if_type {
            71 => format!("Wi-Fi ({})", a.name),
            6 => format!("Ethernet, кабель ({})", a.name),
            131 | 53 | 23 => format!("Туннель/VPN ({})", a.name),
            _ => format!("другой ({})", a.name),
        },
    }
}

/// DNS активного адаптера (статика или DHCP).
pub fn system_dns() -> Vec<String> {
    active_adapter().map(|a| a.dns).unwrap_or_default()
}

fn inet_key() -> Option<RegKey> {
    RegKey::predef(HKEY_CURRENT_USER)
        .open_subkey(r"Software\Microsoft\Windows\CurrentVersion\Internet Settings")
        .ok()
}

/// Ручной прокси из реестра: "host:port" или "http=h:p;https=h:p;socks=h:p".
pub fn system_proxy() -> String {
    let Some(k) = inet_key() else { return String::new() };
    if k.get_value::<u32, _>("ProxyEnable").unwrap_or(0) == 0 {
        return String::new();
    }
    let p: String = k.get_value("ProxyServer").unwrap_or_default();
    let p = p.trim();
    if p.is_empty() {
        return String::new();
    }
    if !p.contains('=') {
        return if p.contains("://") { p.to_string() } else { format!("http://{p}") };
    }
    let find = |key: &str| {
        p.split(';').find_map(|part| {
            let (k, v) = part.split_once('=')?;
            (k.trim().eq_ignore_ascii_case(key) && !v.trim().is_empty()).then(|| v.trim().to_string())
        })
    };
    for k in ["https", "http"] {
        if let Some(v) = find(k) {
            return format!("http://{v}");
        }
    }
    find("socks").map(|v| format!("socks5://{v}")).unwrap_or_default()
}

/// Адрес PAC-скрипта (автонастройка прокси), если задан.
pub fn pac_url() -> String {
    inet_key().and_then(|k| k.get_value::<String, _>("AutoConfigURL").ok()).unwrap_or_default()
}

fn phy_name(t: i32) -> &'static str {
    match t {
        5 => "802.11a",
        6 => "802.11b",
        7 => "802.11g",
        8 => "802.11n (Wi-Fi 4)",
        9 => "802.11ac (Wi-Fi 5)",
        10 => "802.11ad",
        11 => "802.11ax (Wi-Fi 6)",
        12 => "802.11be (Wi-Fi 7)",
        _ => "",
    }
}

struct Wlan {
    ssid: String,
    signal: u32,
    rx_kbps: u32,
    tx_kbps: u32,
    phy: i32,
    freq_khz: u32,
}

/// Текущее Wi-Fi-подключение через WLAN API: SSID, сигнал, скорости, стандарт и частота точки доступа.
fn wlan_current() -> Option<Wlan> {
    use windows_sys::Win32::NetworkManagement::WiFi::*;
    unsafe {
        let mut ver = 0u32;
        let mut h = std::ptr::null_mut();
        if WlanOpenHandle(2, std::ptr::null(), &mut ver, &mut h) != 0 {
            return None;
        }
        let mut res = None;
        let mut list: *mut WLAN_INTERFACE_INFO_LIST = std::ptr::null_mut();
        if WlanEnumInterfaces(h, std::ptr::null(), &mut list) == 0 && !list.is_null() {
            let n = (*list).dwNumberOfItems as usize;
            let items = std::slice::from_raw_parts((*list).InterfaceInfo.as_ptr(), n);
            for it in items {
                let mut size = 0u32;
                let mut data: *mut c_void = std::ptr::null_mut();
                let mut vt = 0;
                let r = WlanQueryInterface(
                    h,
                    &it.InterfaceGuid,
                    wlan_intf_opcode_current_connection,
                    std::ptr::null(),
                    &mut size,
                    &mut data,
                    &mut vt,
                );
                if r == 0 && !data.is_null() {
                    let c = &*(data as *const WLAN_CONNECTION_ATTRIBUTES);
                    let a = &c.wlanAssociationAttributes;
                    let len = (a.dot11Ssid.uSSIDLength as usize).min(32);
                    let ssid = String::from_utf8_lossy(&a.dot11Ssid.ucSSID[..len]).into_owned();
                    // частота: запись BSS-списка с тем же BSSID, что и у текущей точки доступа
                    let mut freq_khz = 0u32;
                    for secured in [1, 0] {
                        let mut bl: *mut WLAN_BSS_LIST = std::ptr::null_mut();
                        let rr = WlanGetNetworkBssList(
                            h,
                            &it.InterfaceGuid,
                            &a.dot11Ssid,
                            dot11_BSS_type_any,
                            secured,
                            std::ptr::null(),
                            &mut bl,
                        );
                        if rr == 0 && !bl.is_null() {
                            let cnt = (*bl).dwNumberOfItems as usize;
                            let es = std::slice::from_raw_parts((*bl).wlanBssEntries.as_ptr(), cnt);
                            if let Some(e) = es.iter().find(|e| e.dot11Bssid == a.dot11Bssid) {
                                freq_khz = e.ulChCenterFrequency;
                            }
                            WlanFreeMemory(bl as *mut c_void);
                        }
                        if freq_khz != 0 {
                            break;
                        }
                    }
                    res = Some(Wlan {
                        ssid,
                        signal: a.wlanSignalQuality,
                        rx_kbps: a.ulRxRate,
                        tx_kbps: a.ulTxRate,
                        phy: a.dot11PhyType as i32,
                        freq_khz,
                    });
                    WlanFreeMemory(data);
                    break;
                }
                if !data.is_null() {
                    WlanFreeMemory(data);
                }
            }
            WlanFreeMemory(list as *mut c_void);
        }
        WlanCloseHandle(h, std::ptr::null());
        res
    }
}

pub fn link_info() -> Option<super::Link> {
    let a = active_adapter()?;
    if !a.up {
        return None;
    }
    let mut l = super::Link {
        kind: match a.if_type {
            71 => "wifi",
            6 => "ethernet",
            131 | 53 | 23 => "vpn",
            _ => "other",
        }
        .into(),
        name: a.name.clone(),
        rx_mbps: mbps(a.rx_bps),
        tx_mbps: mbps(a.tx_bps),
        ..Default::default()
    };
    if a.if_type == 71 {
        if let Some(w) = wlan_current() {
            l.ssid = w.ssid;
            l.signal_pct = Some(w.signal);
            if w.rx_kbps > 0 {
                l.rx_mbps = w.rx_kbps as f64 / 1000.0;
            }
            if w.tx_kbps > 0 {
                l.tx_mbps = w.tx_kbps as f64 / 1000.0;
            }
            l.phy = phy_name(w.phy).into();
            l.set_freq((w.freq_khz / 1000) as u32);
        }
    }
    Some(l)
}
