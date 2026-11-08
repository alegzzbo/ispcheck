#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

macro_rules! logf {
    ($m:expr, $($a:tt)*) => { $crate::state::log($m, format!($($a)*)) };
}

mod checks;
mod config;
mod dns;
mod mtr;
mod state;
mod sys;
mod update;

use config::*;
use eframe::egui;
use state::*;
use std::sync::atomic::{AtomicBool, Ordering::Relaxed};
use std::sync::{Arc, Mutex};

static PENDING: Mutex<Option<(update::UpdInfo, update::UpdFile)>> = Mutex::new(None);

fn login_path() -> std::path::PathBuf {
    let base = if cfg!(windows) {
        std::env::var_os("APPDATA").map(std::path::PathBuf::from)
    } else {
        std::env::var_os("HOME").map(|h| std::path::PathBuf::from(h).join(".config"))
    };
    base.unwrap_or_default().join("ispcheck").join("login.txt")
}

struct App {
    login: String,
    extra: String,
    note: String,
    mtr_host: String,
    mtr_cycles: String,
    busy: Arc<AtomicBool>,
    mtr_run: Arc<AtomicBool>,
    mtr_stop: Arc<AtomicBool>,
    sending: Arc<AtomicBool>,
    can_send: Arc<AtomicBool>,
    updating: Arc<AtomicBool>,
}

impl App {
    fn new(cc: &eframe::CreationContext<'_>) -> Self {
        set_ctx(cc.egui_ctx.clone());
        let ctx = cc.egui_ctx.clone();
        std::thread::spawn(move || {
            update::cleanup();
            std::thread::sleep(std::time::Duration::from_secs(2));
            if let Ok((info, f)) = update::fetch() {
                if update::newer(&info.version, update::VERSION) {
                    *PENDING.lock().unwrap() = Some((info, f));
                    ctx.request_repaint();
                }
            }
        });
        App {
            login: std::fs::read_to_string(login_path()).unwrap_or_default().trim().to_string(),
            extra: String::new(),
            note: String::new(),
            mtr_host: MTR_DEFAULT_HOST.into(),
            mtr_cycles: MTR_CYCLES.to_string(),
            busy: Default::default(),
            mtr_run: Default::default(),
            mtr_stop: Default::default(),
            sending: Default::default(),
            can_send: Default::default(),
            updating: Default::default(),
        }
    }

    fn start_check(&self) {
        let list: Vec<String> =
            self.extra.lines().map(str::trim).filter(|l| !l.is_empty()).map(String::from).collect();
        reset();
        self.busy.store(true, Relaxed);
        let (busy, can) = (self.busy.clone(), self.can_send.clone());
        std::thread::spawn(move || {
            checks::run_all(list);
            can.store(true, Relaxed);
            busy.store(false, Relaxed);
            set_live(String::new());
        });
    }

    fn send(&self) {
        let login = self.login.trim().to_string();
        if login.is_empty() {
            logf!(WARN, "Укажите логин перед отправкой отчёта");
            return;
        }
        let (note, sending) = (self.note.clone(), self.sending.clone());
        sending.store(true, Relaxed);
        std::thread::spawn(move || {
            match checks::send_report(&login, &note) {
                Ok(()) => logf!(OK, "Отчёт отправлен"),
                Err(e) => logf!(BAD, "Отправка не удалась: {e}"),
            }
            sending.store(false, Relaxed);
        });
    }

    fn toggle_mtr(&mut self) {
        if self.mtr_run.load(Relaxed) {
            self.mtr_stop.store(true, Relaxed);
            return;
        }
        let host = self.mtr_host.trim().to_string();
        if host.is_empty() {
            return;
        }
        let Ok(n) = self.mtr_cycles.trim().parse::<u32>() else {
            logf!(WARN, "Циклов: целое число >= 0 (0 = бесконечно)");
            return;
        };
        self.mtr_stop.store(false, Relaxed);
        self.mtr_run.store(true, Relaxed);
        let (run, stop, can) = (self.mtr_run.clone(), self.mtr_stop.clone(), self.can_send.clone());
        std::thread::spawn(move || {
            let need_ip = {
                let g = st();
                g.ip4.is_empty() && g.ip6.is_empty()
            };
            if need_ip {
                checks::detect_ip();
            }
            mtr::run(&host, n, stop);
            can.store(true, Relaxed);
            run.store(false, Relaxed);
        });
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _: &mut eframe::Frame) {
        let ctx = &ui.ctx().clone();
        let busy = self.busy.load(Relaxed);
        let mtr_on = self.mtr_run.load(Relaxed);
        let working = busy || mtr_on || self.sending.load(Relaxed);

        // диалог обновления
        let pending = PENDING.lock().unwrap().clone();
        if let Some((info, f)) = pending {
            egui::Window::new("Обновление")
                .collapsible(false)
                .resizable(false)
                .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
                .show(ctx, |ui| {
                    ui.label(format!(
                        "Доступна версия {} (у вас {}).\n{}\nОбновить сейчас?",
                        info.version,
                        update::VERSION,
                        info.notes
                    ));
                    if self.updating.load(Relaxed) {
                        ui.horizontal(|ui| {
                            ui.spinner();
                            ui.label("Загрузка и проверка…");
                        });
                    } else {
                        ui.horizontal(|ui| {
                            if ui.button("Обновить").clicked() {
                                self.updating.store(true, Relaxed);
                                let up = self.updating.clone();
                                std::thread::spawn(move || match update::install_and_restart(&f) {
                                    Ok(()) => std::process::exit(0),
                                    Err(e) => {
                                        *PENDING.lock().unwrap() = None;
                                        up.store(false, Relaxed);
                                        logf!(BAD, "Обновление не удалось: {e}");
                                    }
                                });
                            }
                            if ui.button("Позже").clicked() {
                                *PENDING.lock().unwrap() = None;
                            }
                        });
                    }
                });
        }

        egui::Panel::top("top").show(ui, |ui| {
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                ui.label("Логин:");
                let r = ui.add(
                    egui::TextEdit::singleline(&mut self.login)
                        .hint_text("Ваш логин в системе провайдера")
                        .desired_width(f32::INFINITY),
                );
                if r.changed() {
                    let p = login_path();
                    if let Some(d) = p.parent() {
                        let _ = std::fs::create_dir_all(d);
                    }
                    let _ = std::fs::write(p, self.login.trim());
                }
            });
            ui.label("Сайты, которые не открываются (по одному в строке):");
            ui.add(
                egui::TextEdit::multiline(&mut self.extra)
                    .hint_text("example.com\nother-site.org")
                    .desired_rows(3)
                    .desired_width(f32::INFINITY),
            );
            ui.add(egui::TextEdit::singleline(&mut self.note).hint_text("Комментарий").desired_width(f32::INFINITY));
            ui.horizontal(|ui| {
                if ui.add_enabled(!busy && !mtr_on, egui::Button::new("Начать проверку")).clicked() {
                    self.start_check();
                }
                let can = self.can_send.load(Relaxed) && !working;
                if ui.add_enabled(can, egui::Button::new("Отправить отчёт в поддержку")).clicked() {
                    self.send();
                }
                if ui.button("Копировать отчёт").clicked() {
                    ctx.copy_text(report());
                }
            });
            ui.horizontal(|ui| {
                ui.label("MTR до:");
                ui.add(egui::TextEdit::singleline(&mut self.mtr_host).desired_width(160.0));
                ui.label("циклов (0 = ∞):");
                ui.add(egui::TextEdit::singleline(&mut self.mtr_cycles).desired_width(50.0));
                let stopping = mtr_on && self.mtr_stop.load(Relaxed);
                let label = if mtr_on { "Остановить MTR" } else { "Запустить MTR" };
                if ui.add_enabled(!busy && !stopping, egui::Button::new(label)).clicked() {
                    self.toggle_mtr();
                }
                if working {
                    ui.spinner();
                }
            });
            ui.add_space(4.0);
        });

        egui::CentralPanel::default().show(ui, |ui| {
            egui::ScrollArea::both().auto_shrink([false; 2]).stick_to_bottom(true).show(ui, |ui| {
                ui.add(egui::Label::new(egui::RichText::new(text()).monospace()).selectable(true));
            });
        });
    }
}

fn main() -> eframe::Result<()> {
    // TLS-провайдер ring (без C++/cmake/NASM)
    let _ = rustls::crypto::ring::default_provider().install_default();
    let opts = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([720.0, 640.0])
            .with_title(format!("{ISP_NAME} — проверка соединения v{}", update::VERSION)),
        ..Default::default()
    };
    eframe::run_native("ispcheck", opts, Box::new(|cc| Ok(Box::new(App::new(cc)))))
}
