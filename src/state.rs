use std::fmt::Display;
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicUsize, Ordering::Relaxed};
use std::sync::{Mutex, MutexGuard, OnceLock};

pub const OK: &str = "✔";
pub const BAD: &str = "✖";
pub const WARN: &str = "⚠";
pub const HDR: &str = "▶";
pub const SEP: &str = "─";

/// Итоги проверок для резюме (текст отчёта не парсится).
#[derive(Default, Clone, serde::Serialize)]
pub struct Summ {
    pub conn: String,
    pub dns: String,
    pub dns_up: String,
    pub gw: String,
    pub dns_problem: String,
    pub ip4: String,
    pub ip_proxy: String,
    pub ipv6: String,
    pub link: String,
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
    pub link: Option<crate::sys::Link>,
}

static STATE: OnceLock<Mutex<Inner>> = OnceLock::new();
#[cfg(feature = "gui")]
static CTX: OnceLock<eframe::egui::Context> = OnceLock::new();

pub fn st() -> MutexGuard<'static, Inner> {
    STATE
        .get_or_init(|| Mutex::new(Inner::default()))
        .lock()
        .unwrap_or_else(|e| e.into_inner())
}

#[cfg(feature = "gui")]
pub fn set_ctx(c: eframe::egui::Context) {
    let _ = CTX.set(c);
}

fn repaint() {
    #[cfg(feature = "gui")]
    if let Some(c) = CTX.get() {
        c.request_repaint();
    }
}

// ── вывод в консоль (CLI) ────────────────────────────────────
static ECHO: AtomicU8 = AtomicU8::new(0); // 0 выкл, 1 stdout, 2 stderr
static COLOR: AtomicBool = AtomicBool::new(false);
static LIVE_LINES: AtomicUsize = AtomicUsize::new(0);
static BAD_COUNT: AtomicUsize = AtomicUsize::new(0);

#[cfg_attr(feature = "gui", allow(dead_code))]
pub fn set_echo(to_stderr: bool, color: bool) {
    ECHO.store(if to_stderr { 2 } else { 1 }, Relaxed);
    COLOR.store(color, Relaxed);
}

/// Сколько ошибок (✖) записано: код возврата консольной версии.
#[cfg_attr(feature = "gui", allow(dead_code))]
pub fn bad_count() -> usize {
    BAD_COUNT.load(Relaxed)
}

fn out(s: &str) {
    use std::io::Write;
    match ECHO.load(Relaxed) {
        1 => {
            let mut o = std::io::stdout().lock();
            let _ = o.write_all(s.as_bytes());
            let _ = o.flush();
        }
        2 => {
            let _ = std::io::stderr().write_all(s.as_bytes());
        }
        _ => {}
    }
}

fn erase_live() {
    let n = LIVE_LINES.swap(0, Relaxed);
    if n > 0 {
        out(&format!("\x1b[{n}A\x1b[J"));
    }
}

fn echo_line(mark: &str, msg: &str) {
    if ECHO.load(Relaxed) == 0 {
        return;
    }
    erase_live();
    let color = match mark {
        OK => "32",
        BAD => "1;31",
        WARN => "33",
        HDR => "1;36",
        SEP => "2",
        _ => "",
    };
    if COLOR.load(Relaxed) && !color.is_empty() {
        out(&format!("\x1b[{color}m{mark} {msg}\x1b[0m\n"));
    } else {
        out(&format!("{mark} {msg}\n"));
    }
}

/// Путь к файлу с сохранённым логином.
pub fn login_path() -> std::path::PathBuf {
    let base = if cfg!(windows) {
        std::env::var_os("APPDATA").map(std::path::PathBuf::from)
    } else {
        std::env::var_os("HOME").map(|h| std::path::PathBuf::from(h).join(".config"))
    };
    base.unwrap_or_default().join("ispcheck").join("login.txt")
}

pub fn log(mark: &str, msg: impl Display) {
    let msg = msg.to_string();
    st().report.push_str(&format!("{mark} {msg}\n"));
    if mark == BAD {
        BAD_COUNT.fetch_add(1, Relaxed);
    }
    echo_line(mark, &msg);
    repaint();
}

/// «Живой» блок (таблица MTR). В консоли при выводе в терминал перерисовывается на месте.
pub fn set_live(s: String) {
    if ECHO.load(Relaxed) == 1 && COLOR.load(Relaxed) {
        erase_live();
        out(&s);
        LIVE_LINES.store(s.lines().count(), Relaxed);
    }
    st().live = s;
    repaint();
}

pub fn commit(s: &str) {
    if ECHO.load(Relaxed) != 0 {
        erase_live();
        out(s);
    }
    {
        let mut g = st();
        g.report.push_str(s);
        g.live.clear();
    }
    repaint();
}

#[cfg_attr(not(feature = "gui"), allow(dead_code))]
pub fn text() -> String {
    let g = st();
    format!("{}{}", g.report, g.live)
}

#[cfg_attr(not(feature = "gui"), allow(dead_code))]
pub fn reset() {
    let mut g = st();
    g.report.clear();
    g.live.clear();
    g.summ = Summ::default();
    g.ip4.clear();
    g.ip6.clear();
    g.ip_proxy.clear();
    g.link = None;
    BAD_COUNT.store(0, Relaxed);
}
