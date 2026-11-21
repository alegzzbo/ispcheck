//! Android: сетевые данные берутся из Kotlin (MainActivity.ispSnapshot / ispCounters) через JNI,
//! ICMP идёт через непривилегированный DGRAM-сокет; промежуточные хопы (TTL) читаются из очереди ошибок.
use super::{Echo, Link};
use jni::objects::{JObject, JString, JValue};
use jni::JavaVM;
use std::net::Ipv4Addr;
use std::sync::Mutex;
use std::time::{Duration, Instant};

// ── JNI-мост ─────────────────────────────────────────────────

static ACTIVITY: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// Глобальная ссылка на MainActivity (ndk_context хранит Application, а не Activity).
pub fn set_activity(p: *mut std::ffi::c_void) {
    ACTIVITY.store(p as usize, std::sync::atomic::Ordering::Relaxed);
}

/// Вызов `String method([String])` у MainActivity.
fn call_string(method: &str, arg: Option<&str>) -> Option<String> {
    let cx = ndk_context::android_context();
    let vm = unsafe { JavaVM::from_raw(cx.vm().cast()) }.ok()?;
    let mut env = vm.attach_current_thread().ok()?;
    let raw = ACTIVITY.load(std::sync::atomic::Ordering::Relaxed);
    if raw == 0 {
        return None;
    }
    let act = unsafe { JObject::from_raw(raw as jni::sys::jobject) };
    let res = match arg {
        None => env.call_method(&act, method, "()Ljava/lang/String;", &[]),
        Some(a) => {
            let j = env.new_string(a).ok()?;
            env.call_method(&act, method, "(Ljava/lang/String;)Ljava/lang/String;", &[JValue::Object(&j)])
        }
    };
    let obj = match res {
        Ok(v) => v.l().ok()?,
        Err(_) => {
            let _ = env.exception_clear();
            return None;
        }
    };
    if obj.is_null() {
        return None;
    }
    let s: String = env.get_string(&JString::from(obj)).ok()?.into();
    Some(s)
}

#[derive(Default, Clone, serde::Deserialize)]
#[serde(default)]
struct Snap {
    iface: String,
    transport: String, // wifi | ethernet | cellular | vpn | other | none
    gateway: String,
    dns: Vec<String>,
    proxy: String,
    pac: String,
    ssid: String,
    rssi: Option<i32>,
    freq: u32,
    rx: f64,
    tx: f64,
    phy: String,
    down_kbps: f64,
    up_kbps: f64,
}

static CACHE: Mutex<Option<(Instant, Snap)>> = Mutex::new(None);

fn snap() -> Snap {
    let mut g = CACHE.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((t, s)) = g.as_ref() {
        if t.elapsed() < Duration::from_secs(1) {
            return s.clone();
        }
    }
    let s = call_string("ispSnapshot", None)
        .and_then(|j| serde_json::from_str::<Snap>(&j).ok())
        .unwrap_or_default();
    *g = Some((Instant::now(), s.clone()));
    s
}

static INSETS: Mutex<Option<(Instant, [f32; 4])>> = Mutex::new(None);

/// Отступы окна в пикселях: (left, top, right, bottom).
pub fn insets() -> [f32; 4] {
    let mut g = INSETS.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((t, v)) = g.as_ref() {
        if t.elapsed() < Duration::from_millis(250) {
            return *v;
        }
    }
    let mut v = [0f32; 4];
    if let Some(s) = call_string("ispInsets", None) {
        for (i, x) in s.split(',').take(4).enumerate() {
            v[i] = x.trim().parse().unwrap_or(0.0);
        }
    }
    *g = Some((Instant::now(), v));
    v
}

/// Каталог данных приложения (для login.txt).
pub fn files_dir() -> String {
    call_string("ispFilesDir", None).unwrap_or_default()
}

/// Копирование текста в буфер обмена Android.
pub fn copy_text(t: &str) {
    let _ = call_string("ispCopy", Some(t));
}

// ── данные о сети ────────────────────────────────────────────

pub fn default_gateway() -> Result<Ipv4Addr, String> {
    snap().gateway.parse().map_err(|_| "маршрут по умолчанию не найден".into())
}

pub fn connection_type() -> String {
    let s = snap();
    let i = &s.iface;
    match s.transport.as_str() {
        "wifi" => format!("Wi-Fi ({i})"),
        "ethernet" => format!("Ethernet, кабель ({i})"),
        "cellular" => format!("Мобильная сеть ({i})"),
        "vpn" => format!("VPN ({i})"),
        "none" | "" => "нет подключения".into(),
        _ => format!("неизвестно ({i})"),
    }
}

pub fn system_dns() -> Vec<String> {
    snap().dns
}

pub fn system_proxy() -> String {
    snap().proxy
}

pub fn pac_url() -> String {
    snap().pac
}

pub fn link_info() -> Option<Link> {
    let s = snap();
    let kind = match s.transport.as_str() {
        "none" | "" => return None,
        k @ ("wifi" | "ethernet" | "vpn" | "cellular") => k,
        _ => "other",
    };
    let mut l = Link { kind: kind.into(), name: s.iface.clone(), ..Default::default() };
    match kind {
        "wifi" => {
            l.ssid = s.ssid;
            l.signal_dbm = s.rssi;
            l.rx_mbps = s.rx;
            l.tx_mbps = s.tx;
            l.phy = s.phy;
            l.set_freq(s.freq);
        }
        _ => {
            // оценка системы (NetworkCapabilities), а не измерение
            l.rx_mbps = s.down_kbps / 1000.0;
            l.tx_mbps = s.up_kbps / 1000.0;
        }
    }
    Some(l)
}

/// Байты всего устройства (TrafficStats): с Android 10 счётчики отдельного интерфейса недоступны.
pub fn if_counters() -> Option<(u64, u64)> {
    let s = call_string("ispCounters", None)?;
    let (a, b) = s.split_once(',')?;
    let (a, b): (i64, i64) = (a.trim().parse().ok()?, b.trim().parse().ok()?);
    (a >= 0 && b >= 0).then(|| (a as u64, b as u64))
}

// ── ICMP ─────────────────────────────────────────────────────

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

struct Fd(libc::c_int);
impl Drop for Fd {
    fn drop(&mut self) {
        unsafe { libc::close(self.0) };
    }
}

/// ICMP echo через DGRAM-сокет (root не нужен). Ответ адресата приходит данными,
/// «Time Exceeded» / «Unreachable» от промежуточных узлов — через MSG_ERRQUEUE (IP_RECVERR).
pub fn icmp_echo(ip: Ipv4Addr, ttl: u8, timeout: Duration) -> Result<Echo, String> {
    unsafe {
        let fd = libc::socket(libc::AF_INET, libc::SOCK_DGRAM | libc::SOCK_CLOEXEC, libc::IPPROTO_ICMP);
        if fd < 0 {
            return Err(std::io::Error::last_os_error().to_string());
        }
        let fd = Fd(fd);
        let one: libc::c_int = 1;
        let t: libc::c_int = ttl as libc::c_int;
        let sz = std::mem::size_of::<libc::c_int>() as libc::socklen_t;
        libc::setsockopt(fd.0, libc::IPPROTO_IP, libc::IP_RECVERR, &one as *const _ as *const _, sz);
        if libc::setsockopt(fd.0, libc::IPPROTO_IP, libc::IP_TTL, &t as *const _ as *const _, sz) != 0 {
            return Err(std::io::Error::last_os_error().to_string());
        }
        // id ядро подставляет само; seq = ttl
        let seq = ttl as u16;
        let mut pkt = vec![8u8, 0, 0, 0, 0, 0, (seq >> 8) as u8, seq as u8];
        pkt.extend_from_slice(b"ispcheck");
        let c = checksum(&pkt);
        pkt[2] = (c >> 8) as u8;
        pkt[3] = c as u8;
        let mut dst: libc::sockaddr_in = std::mem::zeroed();
        dst.sin_family = libc::AF_INET as _;
        dst.sin_addr.s_addr = u32::from_ne_bytes(ip.octets());
        let start = Instant::now();
        let n = libc::sendto(
            fd.0,
            pkt.as_ptr() as *const _,
            pkt.len(),
            0,
            &dst as *const _ as *const libc::sockaddr,
            std::mem::size_of::<libc::sockaddr_in>() as libc::socklen_t,
        );
        if n < 0 {
            return Err(std::io::Error::last_os_error().to_string());
        }
        loop {
            let Some(left) = timeout.checked_sub(start.elapsed()) else {
                return Ok(Echo { hop: None, rtt: start.elapsed(), reached: false });
            };
            let mut p = libc::pollfd { fd: fd.0, events: libc::POLLIN, revents: 0 };
            let r = libc::poll(&mut p, 1, left.as_millis().max(1) as libc::c_int);
            if r <= 0 {
                return Ok(Echo { hop: None, rtt: start.elapsed(), reached: false });
            }
            if p.revents & libc::POLLERR != 0 {
                if let Some(hop) = read_errqueue(fd.0) {
                    return Ok(Echo { hop, rtt: start.elapsed(), reached: false });
                }
                continue;
            }
            if p.revents & libc::POLLIN != 0 {
                let mut buf = [0u8; 1500];
                let mut peer: libc::sockaddr_in = std::mem::zeroed();
                let mut pl = std::mem::size_of::<libc::sockaddr_in>() as libc::socklen_t;
                let n = libc::recvfrom(
                    fd.0,
                    buf.as_mut_ptr() as *mut _,
                    buf.len(),
                    0,
                    &mut peer as *mut _ as *mut libc::sockaddr,
                    &mut pl,
                );
                if n < 8 {
                    continue;
                }
                if buf[0] == 0 {
                    let hop = Some(Ipv4Addr::from(peer.sin_addr.s_addr.to_ne_bytes()));
                    return Ok(Echo { hop, rtt: start.elapsed(), reached: true });
                }
            }
        }
    }
}

/// Читает одну запись очереди ошибок: Some(Some(ip)) — ответил узел, Some(None) — ошибка без адреса,
/// None — очередь пуста (повторить ожидание).
unsafe fn read_errqueue(fd: libc::c_int) -> Option<Option<Ipv4Addr>> {
    #[repr(C)]
    struct ExtErr {
        errno: u32,
        origin: u8,
        etype: u8,
        code: u8,
        pad: u8,
        info: u32,
        data: u32,
    }
    let mut data = [0u8; 256];
    let mut ctrl = [0u8; 512];
    let mut iov = libc::iovec { iov_base: data.as_mut_ptr() as *mut _, iov_len: data.len() };
    let mut msg: libc::msghdr = std::mem::zeroed();
    msg.msg_iov = &mut iov;
    msg.msg_iovlen = 1;
    msg.msg_control = ctrl.as_mut_ptr() as *mut _;
    msg.msg_controllen = ctrl.len() as _;
    if libc::recvmsg(fd, &mut msg, libc::MSG_ERRQUEUE | libc::MSG_DONTWAIT) < 0 {
        return None;
    }
    let mut c = libc::CMSG_FIRSTHDR(&msg);
    while !c.is_null() {
        if (*c).cmsg_level == libc::IPPROTO_IP && (*c).cmsg_type == libc::IP_RECVERR {
            let e = libc::CMSG_DATA(c) as *const ExtErr;
            let off = (e as *const u8).add(std::mem::size_of::<ExtErr>()) as *const libc::sockaddr_in;
            let sa = std::ptr::read_unaligned(off);
            let ip = (sa.sin_family == libc::AF_INET as libc::sa_family_t).then(|| Ipv4Addr::from(sa.sin_addr.s_addr.to_ne_bytes()));
            return Some(ip);
        }
        c = libc::CMSG_NXTHDR(&msg, c);
    }
    Some(None)
}
