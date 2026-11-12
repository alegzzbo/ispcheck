//! Консольная версия (cargo build --release --no-default-features): без окна и OpenGL,
//! работает по SSH и на серверах без графики.

use crate::config::*;
use crate::state::*;
use crate::{checks, mtr};
use std::io::IsTerminal;
use std::sync::atomic::{AtomicBool, Ordering::Relaxed};
use std::sync::Arc;

const VERSION: &str = env!("CARGO_PKG_VERSION");

const HELP: &str = "\
ispcheck — диагностика интернет-соединения (консольная версия)

Использование: ispcheck [параметры]

  -s, --site URL     дополнительный сайт для проверки (можно несколько раз)
  -m, --mtr ХОСТ     после проверок запустить MTR до хоста
  -c, --cycles N     число циклов MTR (по умолчанию 20, 0 = до Ctrl+C)
      --no-check     пропустить основные проверки (только MTR)
      --send         отправить отчёт в поддержку
      --login ЛОГИН  логин для отправки (по умолчанию сохранённый)
      --note ТЕКСТ   комментарий к отчёту
      --json         итог в JSON на stdout (журнал уходит в stderr)
      --no-color     без цветов
  -h, --help         эта справка
  -V, --version      версия

Код возврата: 0 — ошибок нет, 1 — были ошибки (✖), 2 — неверные параметры.
Для MTR и трассировки на Linux/macOS нужен raw ICMP: root или
sudo setcap cap_net_raw+ep ./ispcheck
";

struct Opts {
    sites: Vec<String>,
    mtr: Option<String>,
    cycles: u32,
    no_check: bool,
    json: bool,
    send: bool,
    login: String,
    note: String,
    color: bool,
}

fn die(msg: &str) -> ! {
    eprintln!("ispcheck: {msg}\nСправка: ispcheck --help");
    std::process::exit(2)
}

fn need(it: &mut impl Iterator<Item = String>, name: &str) -> String {
    it.next().unwrap_or_else(|| die(&format!("{name}: нужно значение")))
}

fn parse() -> Opts {
    let mut o = Opts {
        sites: vec![],
        mtr: None,
        cycles: MTR_CYCLES,
        no_check: false,
        json: false,
        send: false,
        login: std::fs::read_to_string(login_path()).unwrap_or_default().trim().to_string(),
        note: String::new(),
        color: true,
    };
    let mut it = std::env::args().skip(1);
    while let Some(a) = it.next() {
        match a.as_str() {
            "-h" | "--help" => {
                print!("{HELP}");
                std::process::exit(0)
            }
            "-V" | "--version" => {
                println!("ispcheck {VERSION}");
                std::process::exit(0)
            }
            "-s" | "--site" => o.sites.push(need(&mut it, &a)),
            "-m" | "--mtr" => o.mtr = Some(need(&mut it, &a)),
            "-c" | "--cycles" => {
                o.cycles = need(&mut it, &a).parse().unwrap_or_else(|_| die("--cycles: целое число >= 0"))
            }
            "--no-check" => o.no_check = true,
            "--json" => o.json = true,
            "--send" => o.send = true,
            "--login" => o.login = need(&mut it, &a),
            "--note" => o.note = need(&mut it, &a),
            "--no-color" => o.color = false,
            _ => die(&format!("неизвестный параметр: {a}")),
        }
    }
    if o.no_check && o.mtr.is_none() {
        die("--no-check имеет смысл только вместе с --mtr");
    }
    if o.send && REPORT_URL.is_empty() {
        die("отправка отключена в этой сборке (REPORT_URL пуст)");
    }
    if o.send && o.login.trim().is_empty() {
        die("--send требует --login (сохранённого логина нет)");
    }
    o
}

/// Windows: UTF-8 в консоли и ANSI-цвета. Возвращает, можно ли использовать цвета.
#[cfg(windows)]
fn init_console(stderr: bool) -> bool {
    use windows_sys::Win32::System::Console::*;
    unsafe {
        SetConsoleOutputCP(65001);
        let h = GetStdHandle(if stderr { STD_ERROR_HANDLE } else { STD_OUTPUT_HANDLE });
        let mut m = 0;
        GetConsoleMode(h, &mut m) != 0 && SetConsoleMode(h, m | ENABLE_VIRTUAL_TERMINAL_PROCESSING) != 0
    }
}

#[cfg(not(windows))]
fn init_console(_stderr: bool) -> bool {
    true
}

pub fn run() -> ! {
    let o = parse();
    let to_err = o.json; // с --json журнал идёт в stderr, чтобы stdout остался чистым JSON
    let tty = if to_err { std::io::stderr().is_terminal() } else { std::io::stdout().is_terminal() };
    let vt = init_console(to_err);
    set_echo(to_err, o.color && tty && vt && std::env::var_os("NO_COLOR").is_none());

    // Ctrl+C: во время MTR аккуратно останавливает его и печатает итог, в остальное время выходит
    let (stop, mtr_on) = (Arc::new(AtomicBool::new(false)), Arc::new(AtomicBool::new(false)));
    {
        let (s, m) = (stop.clone(), mtr_on.clone());
        let _ = ctrlc::set_handler(move || {
            if m.load(Relaxed) {
                s.store(true, Relaxed);
            } else {
                std::process::exit(130);
            }
        });
    }

    if !o.no_check {
        checks::run_checks(o.sites.clone());
    }
    if let Some(h) = &o.mtr {
        mtr_on.store(true, Relaxed);
        mtr::run(h, o.cycles, stop.clone());
        mtr_on.store(false, Relaxed);
    }
    checks::log_summary();
    log(HDR, "Готово.");

    if o.send {
        match checks::send_report(o.login.trim(), &o.note) {
            Ok(()) => log(OK, "Отчёт отправлен"),
            Err(e) => log(BAD, format!("Отправка не удалась: {e}")),
        }
    }

    if o.json {
        let g = st();
        let j = serde_json::json!({
            "isp": ISP_NAME, "version": VERSION,
            "time": chrono::Local::now().to_rfc3339(),
            "ok": bad_count() == 0,
            "summary": g.summ, "link": g.link,
            "ip4": g.ip4, "ip6": g.ip6, "ip_proxy": g.ip_proxy,
            "report": g.report,
        });
        println!("{}", serde_json::to_string_pretty(&j).unwrap_or_default());
    }
    std::process::exit(if bad_count() == 0 { 0 } else { 1 })
}
