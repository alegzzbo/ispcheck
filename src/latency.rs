//! TCP handshake latency: the same destination and method at idle and under load.
//! This is not ICMP RTT; TCP filtering and server limits can affect the result.
use std::net::{SocketAddr, TcpStream};
use std::time::{Duration, Instant};

#[derive(Default, Debug)]
pub struct Samples {
    pub sent: usize,
    pub rtts: Vec<f64>,
}

impl Samples {
    fn probe(&mut self, target: SocketAddr, timeout: Duration) {
        self.sent += 1;
        let start = Instant::now();
        if TcpStream::connect_timeout(&target, timeout).is_ok() {
            self.rtts.push(start.elapsed().as_secs_f64() * 1000.0);
        }
    }

    pub fn mean(&self) -> Option<f64> {
        if self.rtts.is_empty() { None } else {
            Some(self.rtts.iter().sum::<f64>() / self.rtts.len() as f64)
        }
    }

    pub fn loss(&self) -> f64 {
        if self.sent == 0 { 0.0 } else {
            100.0 * self.sent.saturating_sub(self.rtts.len()) as f64 / self.sent as f64
        }
    }
}

pub fn baseline(target: SocketAddr) -> Samples {
    let mut samples = Samples::default();
    for _ in 0..6 {
        samples.probe(target, Duration::from_millis(750));
        std::thread::sleep(Duration::from_millis(150));
    }
    samples
}

/// Fixed cadence, no overlapping probes; all I/O is bounded by the phase deadline.
pub fn under_load(target: SocketAddr, end: Instant) -> Samples {
    let mut samples = Samples::default();
    while let Some(left) = end.checked_duration_since(Instant::now()) {
        if left < Duration::from_millis(10) { break; }
        let tick = Instant::now();
        samples.probe(target, left.min(Duration::from_millis(750)));
        let pause = Duration::from_millis(300).saturating_sub(tick.elapsed());
        std::thread::sleep(pause.min(end.saturating_duration_since(Instant::now())));
    }
    samples
}

/// Heuristic, not a diagnosis of where the queue is located.
pub fn describe(idle: &Samples, loaded: &Samples) -> (bool, String) {
    let counts = format!("ответов {}/{}, без ответа {:.0}%", loaded.rtts.len(), loaded.sent, loaded.loss());
    match (idle.mean(), loaded.mean()) {
        (Some(base), Some(now)) if idle.rtts.len() >= 3 && loaded.rtts.len() >= 3 => {
            let delta = now - base;
            let warn = delta >= 30.0 || idle.loss() > 0.0 || loaded.loss() > 0.0;
            let verdict = if delta >= 100.0 {
                "сильный рост задержки (признак bufferbloat)"
            } else if delta >= 30.0 {
                "заметный рост задержки (возможен bufferbloat)"
            } else {
                "заметного роста задержки не обнаружено"
            };
            (warn, format!("{base:.1} → {now:.1} мс (Δ {delta:+.1} мс), {counts}; {verdict}"))
        }
        _ => (true, format!("недостаточно ответов для оценки bufferbloat; {counts}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn samples(values: &[f64], sent: usize) -> Samples {
        Samples { sent, rtts: values.to_vec() }
    }

    #[test]
    fn reports_latency_growth() {
        let idle = samples(&[10.0; 6], 6);
        let (warn, text) = describe(&idle, &samples(&[120.0; 10], 10));
        assert!(warn && text.contains("сильный рост") && text.contains("+110.0"));
        assert!(!describe(&idle, &samples(&[15.0; 10], 10)).0);
    }

    #[test]
    fn missing_replies_are_not_zero_latency() {
        let idle = samples(&[10.0; 6], 6);
        let (warn, text) = describe(&idle, &Samples::default());
        assert!(warn && text.contains("недостаточно"));
        assert!(describe(&idle, &samples(&[10.0; 3], 6)).0);
        assert!(describe(&samples(&[10.0; 2], 6), &samples(&[120.0; 5], 5)).1.contains("недостаточно"));
    }

    #[test]
    fn tcp_probe_uses_local_listener() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let mut s = Samples::default();
        s.probe(listener.local_addr().unwrap(), Duration::from_millis(500));
        assert_eq!(s.sent, 1);
        assert_eq!(s.rtts.len(), 1);
        assert_eq!(s.loss(), 0.0);
    }
}
