use std::net::{IpAddr, Ipv4Addr, ToSocketAddrs, UdpSocket};
use std::sync::mpsc;
use std::time::{Duration, Instant};

/// Wire-запрос типа A (RFC 1035); ID=0 для DoH (RFC 8484).
pub fn query_a(name: &str, id: u16) -> Vec<u8> {
    let mut b = vec![(id >> 8) as u8, id as u8, 1, 0, 0, 1, 0, 0, 0, 0, 0, 0];
    for l in name.trim_end_matches('.').split('.') {
        b.push(l.len() as u8);
        b.extend_from_slice(l.as_bytes());
    }
    b.extend_from_slice(&[0, 0, 1, 0, 1]);
    b
}

pub struct Reply {
    pub id: u16,
    pub rcode: u8,
    pub ancount: u16,
    pub a: Vec<Ipv4Addr>,
}

fn skip_name(b: &[u8], mut p: usize) -> Option<usize> {
    loop {
        let l = *b.get(p)? as usize;
        if l == 0 {
            return Some(p + 1);
        }
        if l & 0xC0 == 0xC0 {
            return Some(p + 2);
        }
        p += 1 + l;
    }
}

pub fn parse(b: &[u8]) -> Option<Reply> {
    if b.len() < 12 || b[2] & 0x80 == 0 {
        return None; // не ответ
    }
    let qd = u16::from_be_bytes([b[4], b[5]]);
    let an = u16::from_be_bytes([b[6], b[7]]);
    let mut p = 12;
    for _ in 0..qd {
        p = skip_name(b, p)? + 4;
    }
    let mut a = vec![];
    for _ in 0..an {
        p = skip_name(b, p)?;
        let t = u16::from_be_bytes([*b.get(p)?, *b.get(p + 1)?]);
        let rl = u16::from_be_bytes([*b.get(p + 8)?, *b.get(p + 9)?]) as usize;
        p += 10;
        if t == 1 && rl == 4 {
            let d = b.get(p..p + 4)?;
            a.push(Ipv4Addr::new(d[0], d[1], d[2], d[3]));
        }
        p += rl;
    }
    Some(Reply { id: u16::from_be_bytes([b[0], b[1]]), rcode: b[3] & 0x0F, ancount: an, a })
}

fn rnd16() -> u16 {
    let n = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(1);
    (n ^ (n >> 16)) as u16 | 1
}

/// Запрос A к конкретному серверу по UDP/53.
pub fn udp_query(server: &str, domain: &str, timeout: Duration) -> Option<Reply> {
    let s = UdpSocket::bind("0.0.0.0:0").ok()?;
    s.connect((server, 53)).ok()?;
    let id = rnd16();
    s.send(&query_a(domain, id)).ok()?;
    let end = Instant::now() + timeout;
    let mut buf = [0u8; 1500];
    loop {
        let left = end.checked_duration_since(Instant::now())?;
        s.set_read_timeout(Some(left.max(Duration::from_millis(1)))).ok()?;
        let n = s.recv(&mut buf).ok()?;
        if let Some(r) = parse(&buf[..n]) {
            if r.id == id {
                return Some(r);
            }
        }
    }
}

/// Системный резолвер (getaddrinfo) с таймаутом.
pub fn lookup_host(host: &str, timeout: Duration) -> Result<Vec<IpAddr>, String> {
    let (tx, rx) = mpsc::channel();
    let h = host.to_string();
    std::thread::spawn(move || {
        let r = (h.as_str(), 0u16)
            .to_socket_addrs()
            .map(|it| it.map(|a| a.ip()).collect::<Vec<_>>())
            .map_err(|e| e.to_string());
        let _ = tx.send(r);
    });
    match rx.recv_timeout(timeout) {
        Ok(Ok(v)) if !v.is_empty() => Ok(v),
        Ok(Ok(_)) => Err("пустой ответ".into()),
        Ok(Err(e)) => Err(e),
        Err(_) => Err("таймаут".into()),
    }
}

pub fn resolve4(h: &str) -> Option<Ipv4Addr> {
    if let Ok(ip) = h.parse::<Ipv4Addr>() {
        return Some(ip);
    }
    lookup_host(h, Duration::from_secs(3)).ok()?.into_iter().find_map(|i| match i {
        IpAddr::V4(v) => Some(v),
        _ => None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn query_wire() {
        let q = query_a("yandex.ru", 0);
        assert_eq!(hex::encode(q), "0000010000010000000000000679616e6465780272750000010001");
    }

    #[test]
    fn parse_reply() {
        // ответ: вопрос yandex.ru A, один A=1.2.3.4 с указателем на имя
        let mut r = query_a("yandex.ru", 7);
        r[2] = 0x81;
        r[3] = 0x80;
        r[7] = 1;
        r.extend_from_slice(&[0xC0, 12, 0, 1, 0, 1, 0, 0, 0, 60, 0, 4, 1, 2, 3, 4]);
        let p = parse(&r).unwrap();
        assert_eq!((p.id, p.rcode, p.ancount), (7, 0, 1));
        assert_eq!(p.a, vec![Ipv4Addr::new(1, 2, 3, 4)]);
        assert!(parse(&query_a("x.ru", 1)).is_none()); // это запрос, не ответ
    }
}
