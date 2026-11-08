use eframe::egui;
use std::fmt::Display;
use std::sync::{Mutex, MutexGuard, OnceLock};

pub const OK: &str = "✔";
pub const BAD: &str = "✖";
pub const WARN: &str = "⚠";
pub const HDR: &str = "▶";
pub const SEP: &str = "─";

/// Итоги проверок для резюме (текст отчёта не парсится).
#[derive(Default, Clone)]
pub struct Summ {
    pub conn: String,
    pub dns: String,
    pub dns_up: String,
    pub gw: String,
    pub dns_problem: String,
    pub ip4: String,
    pub ip_proxy: String,
    pub ipv6: String,
    pub down: f64,
    pub up: f64,
    pub pings: Vec<String>,
    pub sites: Vec<String>,
    pub route: Vec<String>,
}

#[derive(Default)]
pub struct Inner {
    pub report: String,
    pub live: String, // «живой» блок (MTR), в отчёт попадает после commit
    pub summ: Summ,
    pub ip4: String,
    pub ip6: String,
    pub ip_proxy: String,
}

static STATE: OnceLock<Mutex<Inner>> = OnceLock::new();
static CTX: OnceLock<egui::Context> = OnceLock::new();

pub fn st() -> MutexGuard<'static, Inner> {
    STATE
        .get_or_init(|| Mutex::new(Inner::default()))
        .lock()
        .unwrap_or_else(|e| e.into_inner())
}

pub fn set_ctx(c: egui::Context) {
    let _ = CTX.set(c);
}

fn repaint() {
    if let Some(c) = CTX.get() {
        c.request_repaint();
    }
}

pub fn log(mark: &str, msg: impl Display) {
    st().report.push_str(&format!("{mark} {msg}\n"));
    repaint();
}

pub fn set_live(s: String) {
    st().live = s;
    repaint();
}

pub fn commit(s: &str) {
    {
        let mut g = st();
        g.report.push_str(s);
        g.live.clear();
    }
    repaint();
}

pub fn text() -> String {
    let g = st();
    format!("{}{}", g.report, g.live)
}

pub fn report() -> String {
    st().report.clone()
}

pub fn reset() {
    let mut g = st();
    g.report.clear();
    g.live.clear();
    g.summ = Summ::default();
    g.ip4.clear();
    g.ip6.clear();
    g.ip_proxy.clear();
}
