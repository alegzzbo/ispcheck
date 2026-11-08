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
fn active_adapter() -> Option<(u32, String, Vec<String>)> {
    let s = UdpSocket::bind("0.0.0.0:0").ok()?;
    s.connect("8.8.8.8:53").ok()?; // пакеты не отправляются
    let local = match s.local_addr().ok()?.ip() {
        IpAddr::V4(v) => v,
        _ => return None,
    };
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
                let mut u = ad.FirstUnicastAddress;
                while !u.is_null() {
                    if sockaddr_ip((*u).Address.lpSockaddr as *const u8) == Some(IpAddr::V4(local)) {
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
                        return Some((ad.IfType, wide(ad.FriendlyName), dns));
                    }
                    u = (*u).Next;
                }
                a = ad.Next;
            }
        }
        return None;
    }
    None
}

pub fn connection_type() -> String {
    match active_adapter() {
        None => "неизвестно".into(),
        Some((t, name, _)) => match t {
            71 => format!("Wi-Fi ({name})"),
            6 => format!("Ethernet, кабель ({name})"),
            131 | 53 | 23 => format!("Туннель/VPN ({name})"),
            _ => format!("другой ({name})"),
        },
    }
}

/// DNS активного адаптера (статика или DHCP).
pub fn system_dns() -> Vec<String> {
    active_adapter().map(|a| a.2).unwrap_or_default()
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
