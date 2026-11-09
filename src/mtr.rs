use crate::config::*;
use crate::dns;
use crate::state::*;
use crate::sys;
use std::collections::HashMap;
use std::fmt::Write;
use std::net::Ipv4Addr;
use std::sync::atomic::{AtomicBool, Ordering::Relaxed};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

struct RouteChange {
    ttl: usize,
    from: String,
    to: String,
}

/// Сквозной путь: один цикл = один путь (последовательность ответивших узлов).
struct PathStat {
    sig: String,
    hop: HashMap<usize, String>,
    cycles: usize,
    okc: usize,
    sum: f64,
}

#[derive(Default, Clone)]
struct HopStat {
    addr: String,
    addrs: HashMap<String, usize>,
    sent: usize,
    recv: usize,
    last: f64,
    best: f64,
    worst: f64,
    sum: f64,
    sq: f64,
}

#[derive(Default)]
struct Data {
    hops: Vec<HopStat>,
    reached: usize,
    changes: Vec<RouteChange>,
    paths: Vec<PathStat>,
    idx: HashMap<String, usize>,
    ierr: Option<String>,
}

type RdnsMap = HashMap<String, (String, Instant)>;

fn rdns() -> &'static Mutex<RdnsMap> {
    static R: OnceLock<Mutex<RdnsMap>> = OnceLock::new();
    R.get_or_init(Default::default)
}

/// Кэш имён сбрасывается при каждом запуске MTR: после смены сети имена могут быть другими.
fn clear_rdns() {
    rdns().lock().unwrap_or_else(|e| e.into_inner()).clear();
}

/// Обратный DNS в фоне: сначала пусто, имя появится в следующих обновлениях таблицы.
/// Неудачный поиск повторяется не чаще раза в 30 с.
fn rdns_name(ip: &str) -> String {
    let mut m = rdns().lock().unwrap_or_else(|e| e.into_inner());
    if let Some((n, t)) = m.get(ip) {
        if !n.is_empty() || t.elapsed() < Duration::from_secs(30) {
            return n.clone();
        }
    }
    m.insert(ip.to_string(), (String::new(), Instant::now()));
    let ip = ip.to_string();
    std::thread::spawn(move || {
        if let Ok(a) = ip.parse::<std::net::IpAddr>() {
            if let Ok(n) = dns_lookup::lookup_addr(&a) {
                if n != ip {
                    rdns().lock().unwrap_or_else(|e| e.into_inner()).insert(
                        ip,
                        (n.trim_end_matches('.').to_string(), Instant::now()),
                    );
                }
            }
        }
    });
    String::new()
}

fn primary(h: &HopStat) -> (String, usize) {
    let (mut best, mut n) = (h.addr.clone(), 0usize);
    for (a, c) in &h.addrs {
        if *c > n || (*c == n && *a < best) {
            best = a.clone();
            n = *c;
        }
    }
    (best, h.addrs.len().saturating_sub(1))
}

fn last_hop(d: &Data) -> usize {
    if d.reached > 0 {
        return d.reached;
    }
    (1..d.hops.len()).rev().find(|&t| d.hops[t].recv > 0).unwrap_or(0)
}

fn render(target: &str, ip: Ipv4Addr, d: &Data, cycle: usize, total: u32, state: &str, full: bool) -> String {
    let mut b = String::new();
    let cyc = if total == 0 { format!("{cycle}/∞") } else { format!("{cycle}/{total}") };
    let _ = writeln!(b, "MTR {target} [{ip}] — цикл {cyc} {state}");
    let _ = writeln!(
        b,
        "{:<4} {:<16} {:>6} {:>4} {:>7} {:>7} {:>7} {:>7} {:>7}  Имя",
        "", "Узел", "Loss%", "Snt", "Last", "Avg", "Best", "Wrst", "StDev"
    );
    let last = last_hop(d);
    if last == 0 {
        b.push_str("Ожидание ответов…\n");
        return b;
    }
    for t in 1..=last {
        let h = &d.hops[t];
        let loss = 100.0 * (h.sent - h.recv) as f64 / (h.sent.max(1)) as f64;
        if h.recv == 0 {
            let _ = writeln!(b, "{t:>2}. {:<16} {loss:>5.1}% {:>4} {:>7} {:>7} {:>7} {:>7} {:>7}", "???", h.sent, "-", "-", "-", "-", "-");
            continue;
        }
        let avg = h.sum / h.recv as f64;
        let sd = (h.sq / h.recv as f64 - avg * avg).max(0.0).sqrt();
        let (addr, extra) = primary(h);
        let mut name: String = rdns_name(&addr).chars().take(28).collect();
        if extra > 0 {
            name += &format!(" (+{extra} пути)");
        }
        let _ = writeln!(
            b,
            "{t:>2}. {addr:<16} {loss:>5.1}% {:>4} {:>7.1} {avg:>7.1} {:>7.1} {:>7.1} {sd:>7.1}  {name}",
            h.sent, h.last, h.best, h.worst
        );
    }
    if d.paths.len() > 1 {
        let mut idx: Vec<usize> = (0..d.paths.len()).collect();
        idx.sort_by(|&a, &c| d.paths[c].cycles.cmp(&d.paths[a].cycles)); // стабильная
        let max_show = if full { 10 } else { 4 };
        let _ = writeln!(b, "Пути: {} (P1 — основной, у остальных только отличия)", idx.len());
        let base = &d.paths[idx[0]];
        for (i, &pi) in idx.iter().enumerate() {
            if i >= max_show {
                let _ = writeln!(b, "  … ещё {}", idx.len() - i);
                break;
            }
            let p = &d.paths[pi];
            let ms = if p.okc > 0 { format!("{:.0} мс", p.sum / p.okc as f64) } else { "-".into() };
            let diff = if i > 0 {
                let mut df = vec![];
                for t in 1..=MTR_MAX_HOPS {
                    if df.len() >= 3 {
                        break;
                    }
                    let (a, c) = (base.hop.get(&t), p.hop.get(&t));
                    if a != c {
                        df.push(format!("{t}:{}", c.map_or("—", |s| s.as_str())));
                    }
                }
                let mut s = format!(" хоп {}", df.join(", "));
                if p.okc == 0 {
                    s += " ЦЕЛЬ НЕ ОТВЕЧАЕТ";
                }
                s
            } else {
                " основной".into()
            };
            let _ = writeln!(b, "  P{} {}ц цель {}/{} {ms}{diff}", i + 1, p.cycles, p.okc, p.cycles);
        }
    }
    if !d.changes.is_empty() {
        let mut order: Vec<(usize, String, String)> = vec![];
        let mut cnt: HashMap<(usize, String, String), usize> = HashMap::new();
        for c in &d.changes {
            let k = (c.ttl, c.from.clone(), c.to.clone());
            let e = cnt.entry(k.clone()).or_insert(0);
            if *e == 0 {
                order.push(k);
            }
            *e += 1;
        }
        let max_show = if full { 10 } else { 3 };
        let _ = writeln!(b, "Смен маршрута: {} ({} видов)", d.changes.len(), order.len());
        for (i, k) in order.iter().enumerate() {
            if i >= max_show {
                let _ = writeln!(b, "  … ещё {}", order.len() - i);
                break;
            }
            let _ = writeln!(b, "  хоп {}: {} → {} ×{}", k.0, k.1, k.2, cnt[k]);
        }
    }
    b
}

pub fn run(target: &str, cycles: u32, stop: Arc<AtomicBool>) {
    let mut target = target.to_string();
    if target.contains("://") {
        if let Some(h) = reqwest::Url::parse(&target).ok().and_then(|u| u.host_str().map(String::from)) {
            target = h;
        }
    }
    let Some(ip) = dns::resolve4(&target) else {
        log(BAD, format!("{target}: не резолвится"));
        return;
    };
    clear_rdns();
    log(HDR, format!("MTR до {target} [{ip}] — {}", chrono::Local::now().format("%Y-%m-%d %H:%M:%S")));
    let data = Mutex::new(Data { hops: vec![HopStat::default(); MTR_MAX_HOPS + 1], ..Default::default() });
    let mut cycle = 0usize;
    while (cycles == 0 || (cycle as u32) < cycles) && !stop.load(Relaxed) {
        let start = Instant::now();
                let limit = {
            let d = data.lock().unwrap();
            if d.reached > 0 { d.reached } else { MTR_MAX_HOPS }
        };
        let cyc = Mutex::new(vec![(String::new(), 0.0f64, false); MTR_MAX_HOPS + 1]);
        std::thread::scope(|s| {
            for ttl in 1..=limit {
                let (data, cyc) = (&data, &cyc);
                s.spawn(move || {
                    let r = sys::icmp_echo(ip, ttl as u8, Duration::from_secs(1));
                    let mut d = data.lock().unwrap();
                    d.hops[ttl].sent += 1;
                    let r = match r {
                        Err(e) => {
                            d.ierr = Some(e);
                            return;
                        }
                        Ok(r) => r,
                    };
                    let Some(h) = r.hop else { return };
                    let ms = r.rtt.as_secs_f64() * 1000.0;
                    let a = h.to_string();
                    let prev = d.hops[ttl].addr.clone();
                    if !prev.is_empty() && prev != a && d.changes.len() < 100_000 {
                        d.changes.push(RouteChange { ttl, from: prev, to: a.clone() });
                    }
                    cyc.lock().unwrap()[ttl] = (a.clone(), ms, r.reached);
                    let hs = &mut d.hops[ttl];
                    hs.addr = a.clone();
                    *hs.addrs.entry(a).or_insert(0) += 1;
                    hs.recv += 1;
                    hs.last = ms;
                    if hs.recv == 1 || ms < hs.best {
                        hs.best = ms;
                    }
                    if ms > hs.worst {
                        hs.worst = ms;
                    }
                    hs.sum += ms;
                    hs.sq += ms * ms;
                    if r.reached && (d.reached == 0 || ttl < d.reached) {
                        d.reached = ttl;
                    }
                });
            }
        });
        let mut d = data.lock().unwrap();
        if d.ierr.is_some() {
            break;
        }
        // путь этого цикла: ответившие узлы до первого ответа цели
        let cy = cyc.lock().unwrap();
        let (mut parts, mut cur_hop, mut ok_now, mut rtt_now) = (vec![], HashMap::new(), false, 0.0);
        for t in 1..=limit {
            if cy[t].0.is_empty() {
                continue;
            }
            parts.push(cy[t].0.clone());
            cur_hop.insert(t, cy[t].0.clone());
            if cy[t].2 {
                ok_now = true;
                rtt_now = cy[t].1;
                break;
            }
        }
        cycle += 1;
        if !parts.is_empty() {
            let sig = parts.join(" > ");
            let i = match d.idx.get(&sig) {
                Some(&i) => i,
                None => {
                    d.paths.push(PathStat { sig: sig.clone(), hop: cur_hop, cycles: 0, okc: 0, sum: 0.0 });
                    let n = d.paths.len() - 1;
                    d.idx.insert(sig, n);
                    n
                }
            };
            let p = &mut d.paths[i];
            p.cycles += 1;
            if ok_now {
                p.okc += 1;
                p.sum += rtt_now;
            }
        }
        set_live(render(&target, ip, &d, cycle, cycles, "(идёт…)", false));
        drop(d);
        // пауза до конца секунды, прерывается остановкой
        while start.elapsed() < Duration::from_secs(1) && !stop.load(Relaxed) {
            std::thread::sleep(Duration::from_millis(50));
        }
    }
    let d = data.into_inner().unwrap();
    if let Some(e) = &d.ierr {
        set_live(String::new());
        log(WARN, format!("ICMP недоступен: {e} (на Linux/macOS нужен root)"));
        return;
    }
    let state = if cycles == 0 || (cycle as u32) < cycles { "(остановлено)" } else { "(завершено)" };
    commit(&render(&target, ip, &d, cycle, cycles, state, true));
    let last = last_hop(&d);
    if last == 0 {
        log(BAD, "Ответов нет ни от одного узла");
        return;
    }
    if !d.changes.is_empty() {
        log(WARN, format!("Маршрут менялся {} раз(а) за {cycle} циклов", d.changes.len()));
        st().summ.route.push(format!("{target}: менялся {} раз(а)", d.changes.len()));
    } else {
        st().summ.route.push(format!("{target}: стабилен"));
    }
    if d.paths.len() > 1 {
        let ratio = |p: &PathStat| p.okc as f64 / p.cycles as f64;
        let mut worst = &d.paths[0];
        for p in &d.paths {
            if ratio(p) < ratio(worst) {
                worst = p;
            }
        }
        if worst.okc < worst.cycles {
            let lastn = worst.sig.rsplit(" > ").next().unwrap_or("");
            log(BAD, format!("Путь с потерями цели: {}/{} циклов без ответа, последний узел {lastn}", worst.cycles - worst.okc, worst.cycles));
        }
        st().summ.route.push(format!("{target}: путей {}, худший доставил {} из {} циклов", d.paths.len(), worst.okc, worst.cycles));
    }
    let fin = &d.hops[last];
    if fin.recv == 0 {
        log(WARN, "Итог: конечный узел не отвечает на ICMP");
    } else if fin.sent > fin.recv {
        log(BAD, format!("Итог: потери на конечном узле {:.1}%", 100.0 * (fin.sent - fin.recv) as f64 / fin.sent as f64));
    } else {
        log(OK, format!("Итог: потерь до конечного узла нет, ср. {:.1} мс", fin.sum / fin.recv as f64));
    }
    log("·", "Потери на промежуточных узлах значимы, только если они сохраняются до конца пути.");
}
