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

/// Кнопка отправки показывается, только если задан REPORT_URL.
const SEND_ENABLED: bool = !REPORT_URL.is_empty();

static PENDING: Mutex<Option<(update::UpdInfo, update::UpdFile)>> = Mutex::new(None);

fn login_path() -> std::path::PathBuf {
    let base = if cfg!(windows) {
        std::env::var_os("APPDATA").map(std::path::PathBuf::from)
    } else {
        std::env::var_os("HOME").map(|h| std::path::PathBuf::from(h).join(".config"))
    };
    base.unwrap_or_default().join("ispcheck").join("login.txt")
}

// ─── Цветовая палитра ───────────────────────────────────────────────
mod palette {
    use eframe::egui::Color32;
    // Фон — три уровня глубины
    pub const BG_BASE: Color32 = Color32::from_rgb(17, 21, 28);
    pub const BG_PANEL: Color32 = Color32::from_rgb(25, 31, 40);
    pub const BG_CARD: Color32 = Color32::from_rgb(32, 39, 50);
    pub const BG_INPUT: Color32 = Color32::from_rgb(22, 27, 35);
    pub const BG_HOVER: Color32 = Color32::from_rgb(42, 52, 65);
    // Рамки
    pub const BORDER: Color32 = Color32::from_rgb(52, 63, 78);
    pub const BORDER_FOCUS: Color32 = Color32::from_rgb(88, 166, 255);
    // Текст
    pub const TEXT: Color32 = Color32::from_rgb(230, 237, 243);
    pub const TEXT_SEC: Color32 = Color32::from_rgb(155, 165, 176);
    pub const TEXT_HINT: Color32 = Color32::from_rgb(90, 100, 112);
    // Акценты
    pub const ACCENT: Color32 = Color32::from_rgb(88, 166, 255);
    pub const GREEN: Color32 = Color32::from_rgb(63, 185, 80);
    pub const GREEN_DIM: Color32 = Color32::from_rgb(35, 90, 45);
    pub const RED: Color32 = Color32::from_rgb(248, 81, 73);
    pub const RED_DIM: Color32 = Color32::from_rgb(100, 35, 32);
    pub const YELLOW: Color32 = Color32::from_rgb(210, 167, 52);
    pub const YELLOW_DIM: Color32 = Color32::from_rgb(90, 72, 22);
    pub const ACCENT_DIM: Color32 = Color32::from_rgb(30, 56, 90);
    // Кнопки
    pub const BTN_PRIMARY: Color32 = Color32::from_rgb(35, 134, 54);
    pub const BTN_PRIMARY_HOVER: Color32 = Color32::from_rgb(46, 160, 67);
    pub const BTN_SECONDARY: Color32 = Color32::from_rgb(44, 50, 58);
    pub const BTN_SECONDARY_HOVER: Color32 = Color32::from_rgb(55, 62, 71);
    pub const BTN_DANGER: Color32 = Color32::from_rgb(160, 40, 38);
    pub const BTN_DANGER_HOVER: Color32 = Color32::from_rgb(200, 55, 50);
}

use egui::Color32;

fn apply_theme(ctx: &egui::Context) {
    use palette::*;

    ctx.style_mut_of(egui::Theme::Dark, |style| {
        let v = &mut style.visuals;
        v.dark_mode = true;
        v.override_text_color = Some(TEXT);

        // Окно
        v.window_fill = BG_CARD;
        v.window_stroke = egui::Stroke::new(1.0, BORDER);
        v.window_shadow = egui::epaint::Shadow {
            offset: [0, 6],
            blur: 16,
            spread: 2,
            color: Color32::from_black_alpha(100),
        };
        v.window_corner_radius = egui::CornerRadius::same(12);

        // Панели
        v.panel_fill = BG_BASE;

        // Виджеты
        let w = &mut v.widgets;
        w.noninteractive.bg_fill = BG_INPUT;
        w.noninteractive.bg_stroke = egui::Stroke::new(1.0, BORDER);
        w.noninteractive.fg_stroke = egui::Stroke::new(1.0, TEXT_SEC);
        w.noninteractive.corner_radius = egui::CornerRadius::same(8);

        w.inactive.bg_fill = BTN_SECONDARY;
        w.inactive.bg_stroke = egui::Stroke::new(1.0, BORDER);
        w.inactive.fg_stroke = egui::Stroke::new(1.0, TEXT);
        w.inactive.corner_radius = egui::CornerRadius::same(8);

        w.hovered.bg_fill = BTN_SECONDARY_HOVER;
        w.hovered.bg_stroke = egui::Stroke::new(1.0, BORDER_FOCUS);
        w.hovered.fg_stroke = egui::Stroke::new(1.0, TEXT);
        w.hovered.corner_radius = egui::CornerRadius::same(8);

        w.active.bg_fill = BG_HOVER;
        w.active.bg_stroke = egui::Stroke::new(2.0, ACCENT);
        w.active.fg_stroke = egui::Stroke::new(1.0, TEXT);
        w.active.corner_radius = egui::CornerRadius::same(8);

        w.open.bg_fill = BG_HOVER;
        w.open.bg_stroke = egui::Stroke::new(1.0, ACCENT);
        w.open.fg_stroke = egui::Stroke::new(1.0, TEXT);
        w.open.corner_radius = egui::CornerRadius::same(8);

        // Курсор и выделение
        v.text_cursor.stroke = egui::Stroke::new(2.0, ACCENT);
        v.selection.bg_fill = Color32::from_rgba_premultiplied(60, 110, 180, 140);
        v.selection.stroke = egui::Stroke::new(0.0, Color32::TRANSPARENT);

        v.extreme_bg_color = BG_INPUT;

        // Пространство
        style.spacing.item_spacing = egui::vec2(8.0, 5.0);
        style.spacing.button_padding = egui::vec2(16.0, 6.0);
        style.spacing.window_margin = egui::Margin::same(16);
    });
    ctx.set_theme(egui::Theme::Dark);
}

// ─── Вспомогательные функции UI ─────────────────────────────────────

/// Фрейм-карточка с фоном, скруглением и тенью.
fn card_frame() -> egui::Frame {
    egui::Frame::new()
        .fill(palette::BG_CARD)
        .corner_radius(10.0)
        .inner_margin(egui::Margin::same(12))
        .stroke(egui::Stroke::new(1.0, palette::BORDER))
        .shadow(egui::epaint::Shadow {
            offset: [0, 2],
            blur: 8,
            spread: 0,
            color: Color32::from_black_alpha(50),
        })
}

/// Маленький заголовок секции.
fn section_title(ui: &mut egui::Ui, title: &str) {
    ui.label(
        egui::RichText::new(title)
            .size(12.5)
            .color(palette::TEXT_SEC),
    );
    ui.add_space(2.0);
}

/// Стилизованная кнопка (цвета по состоянию задаются стилем, без перерисовки поверх).
fn btn(ui: &mut egui::Ui, label: &str, kind: u8, enabled: bool) -> bool {
    let (fill, hover_fill) = match kind {
        1 => (palette::BTN_PRIMARY, palette::BTN_PRIMARY_HOVER),
        2 => (palette::BTN_DANGER, palette::BTN_DANGER_HOVER),
        _ => (palette::BTN_SECONDARY, palette::BTN_SECONDARY_HOVER),
    };
    let text_col = if enabled { palette::TEXT } else { palette::TEXT_HINT };
    let r = ui
        .scope(|ui| {
            let (f, h) = if enabled { (fill, hover_fill) } else { (palette::BG_INPUT, palette::BG_INPUT) };
            let w = &mut ui.style_mut().visuals.widgets;
            for (st, c) in [(&mut w.inactive, f), (&mut w.hovered, h), (&mut w.active, h)] {
                st.weak_bg_fill = c;
                st.bg_fill = c;
            }
            ui.add_enabled(
                enabled,
                egui::Button::new(egui::RichText::new(label).size(13.0).color(text_col)).min_size(egui::vec2(0.0, 30.0)),
            )
        })
        .inner;
    if enabled {
        r.on_hover_cursor(egui::CursorIcon::PointingHand).clicked()
    } else {
        false
    }
}

/// «Чип» со статусом.
fn chip(ui: &mut egui::Ui, text: &str, color: Color32) {
    egui::Frame::new()
        .fill(palette::BG_INPUT)
        .stroke(egui::Stroke::new(1.0, palette::BORDER))
        .corner_radius(10.0)
        .inner_margin(egui::Margin::symmetric(9, 3))
        .show(ui, |ui| {
            ui.label(egui::RichText::new(text).size(12.0).color(color));
        });
}

// ─── App ─────────────────────────────────────────────────────────────

fn setup_fonts(ctx: &egui::Context) {
    // Маленький шрифт символов (подмножество DejaVu Sans Mono, лицензия в assets/LICENSE-DejaVu.txt):
    // ✔ ✖ ⚠ ▶ ─ • ↑ ↓ ■ рисуются одинаково на всех системах.
    let mut fonts = egui::FontDefinitions::default();
    fonts.font_data.insert(
        "symbols".into(),
        std::sync::Arc::new(egui::FontData::from_static(include_bytes!("../assets/symbols.ttf"))),
    );
    for fam in [egui::FontFamily::Proportional, egui::FontFamily::Monospace] {
        if let Some(v) = fonts.families.get_mut(&fam) {
            v.insert(1.min(v.len()), "symbols".into()); // раньше встроенных emoji-шрифтов
        }
    }
    ctx.set_fonts(fonts);
}

#[cfg(has_icon)]
fn window_icon() -> Option<egui::IconData> {
    let img = image::load_from_memory(include_bytes!("../favicon.ico")).ok()?.into_rgba8();
    let (width, height) = img.dimensions();
    Some(egui::IconData { rgba: img.into_raw(), width, height })
}
#[cfg(not(has_icon))]
fn window_icon() -> Option<egui::IconData> {
    None
}

/// Сбрасывает флаги занятости при любом завершении потока, в том числе при панике:
/// иначе после сбоя интерфейс навсегда остался бы в состоянии «идёт проверка».
struct Done {
    flag: Arc<AtomicBool>,
    can_send: Arc<AtomicBool>,
}

impl Drop for Done {
    fn drop(&mut self) {
        self.can_send.store(true, Relaxed);
        self.flag.store(false, Relaxed);
        set_live(String::new());
    }
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
    themed: bool,
    copied_at: Option<std::time::Instant>,
}

impl App {
    fn new(cc: &eframe::CreationContext<'_>) -> Self {
        set_ctx(cc.egui_ctx.clone());
        setup_fonts(&cc.egui_ctx);
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
            copied_at: None,
            themed: false,
        }
    }

    fn start_check(&self) {
        let list: Vec<String> =
            self.extra.lines().map(str::trim).filter(|l| !l.is_empty()).map(String::from).collect();
        reset();
        self.busy.store(true, Relaxed);
        let (busy, can) = (self.busy.clone(), self.can_send.clone());
        std::thread::spawn(move || {
            let _done = Done { flag: busy, can_send: can };
            checks::run_all(list);
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
            struct Off(Arc<AtomicBool>);
            impl Drop for Off {
                fn drop(&mut self) {
                    self.0.store(false, Relaxed);
                }
            }
            let _off = Off(sending.clone());
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
            let _done = Done { flag: run, can_send: can };
            let need_ip = {
                let g = st();
                g.ip4.is_empty() && g.ip6.is_empty()
            };
            if need_ip {
                checks::detect_ip();
            }
            mtr::run(&host, n, stop);
        });
    }
}

// ─── Рендеринг лога с цветными бейджами ─────────────────────────────

fn render_log(ui: &mut egui::Ui) {
    let txt = text();
    if txt.is_empty() {
        ui.add_space(40.0);
        ui.vertical_centered(|ui| {
            ui.label(
                egui::RichText::new("Нажмите  Начать проверку  для запуска диагностики")
                    .size(14.0)
                    .color(palette::TEXT_HINT),
            );
        });
        return;
    }
    let job = build_log_layout(&txt);
    ui.add(egui::Label::new(job).selectable(true));
}

fn build_log_layout(text: &str) -> egui::text::LayoutJob {
    let mut job = egui::text::LayoutJob::default();
    job.wrap = egui::text::TextWrapping {
        max_rows: usize::MAX,
        break_anywhere: false,
        overflow_character: None,
        max_width: f32::INFINITY,
    };

    let mono = egui::FontId::monospace(12.5);

    let fmt = |color: Color32| egui::text::TextFormat {
        font_id: mono.clone(),
        color,
        ..Default::default()
    };

    let badge = |bg: Color32, fg: Color32| egui::text::TextFormat {
        font_id: egui::FontId::monospace(11.0),
        color: fg,
        background: bg,
        ..Default::default()
    };

    let header_fmt = egui::text::TextFormat {
        font_id: egui::FontId::monospace(12.5),
        color: palette::ACCENT,
        background: palette::ACCENT_DIM,
        ..Default::default()
    };

    for line in text.split('\n') {
        if line.is_empty() {
            job.append("\n", 0.0, fmt(palette::TEXT));
            continue;
        }

        // Определяем тип строки по маркеру
        if let Some(rest) = strip_marker(line, OK) {
            // Зелёный бейдж OK
            job.append(" ✔ ", 0.0, badge(palette::GREEN_DIM, palette::GREEN));
            job.append(rest, 4.0, fmt(palette::TEXT));
        } else if let Some(rest) = strip_marker(line, BAD) {
            // Красный бейдж ERR
            job.append(" ✖ ", 0.0, badge(palette::RED_DIM, palette::RED));
            job.append(rest, 4.0, fmt(palette::RED));
        } else if let Some(rest) = strip_marker(line, WARN) {
            // Жёлтый бейдж
            job.append(" ⚠ ", 0.0, badge(palette::YELLOW_DIM, palette::YELLOW));
            job.append(rest, 4.0, fmt(palette::YELLOW));
        } else if let Some(rest) = strip_marker(line, HDR) {
            // Заголовок секции — полоса с акцентным фоном
            job.append(&format!("  ▶  {}  ", rest.trim()), 0.0, header_fmt.clone());
        } else if line.starts_with(SEP) {
            // Разделитель — тонкая линия
            job.append(&"─".repeat(56), 0.0, fmt(palette::BORDER));
        } else if let Some(rest) = strip_marker(line, "\u{00B7}") {
            // Маркер-точка ·
            job.append("  •  ", 0.0, fmt(palette::TEXT_HINT));
            job.append(rest, 0.0, fmt(palette::TEXT_SEC));
        } else {
            // Обычный текст (MTR-таблицы и пр.)
            job.append(line, 0.0, fmt(palette::TEXT_SEC));
        }
        job.append("\n", 0.0, fmt(palette::TEXT));
    }
    job
}

/// Отделяет маркер от текста строки, возвращая остаток.
fn strip_marker<'a>(line: &'a str, marker: &str) -> Option<&'a str> {
    if line.starts_with(marker) {
        Some(&line[marker.len()..])
    } else {
        None
    }
}

// ─── Главный UI ──────────────────────────────────────────────────────

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _: &mut eframe::Frame) {
        let ctx = &ui.ctx().clone();

        if !self.themed {
            apply_theme(ctx);
            self.themed = true;
        }

        let busy = self.busy.load(Relaxed);
        let mtr_on = self.mtr_run.load(Relaxed);
        let working = busy || mtr_on || self.sending.load(Relaxed);

        // ── Диалог обновления ────────────────────────────────────
        let pending = PENDING.lock().unwrap().clone();
        if let Some((info, f)) = pending {
            egui::Window::new("Обновление")
                .collapsible(false)
                .resizable(false)
                .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
                .show(ctx, |ui| {
                    ui.add_space(4.0);
                    ui.label(
                        egui::RichText::new(format!(
                            "Доступна версия {} (у вас {}).",
                            info.version,
                            update::VERSION
                        ))
                        .size(14.0),
                    );
                    if !info.notes.is_empty() {
                        ui.add_space(4.0);
                        ui.label(egui::RichText::new(&info.notes).size(13.0).color(palette::TEXT_SEC));
                    }
                    ui.add_space(8.0);
                    if self.updating.load(Relaxed) {
                        ui.horizontal(|ui| {
                            ui.spinner();
                            ui.label(egui::RichText::new("Загрузка и проверка...").color(palette::TEXT_SEC));
                        });
                    } else {
                        ui.horizontal(|ui| {
                            if btn(ui, "Обновить", 1, true) {
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
                            if btn(ui, "Позже", 0, true) {
                                *PENDING.lock().unwrap() = None;
                            }
                        });
                    }
                });
        }

        // ── Верхняя панель ───────────────────────────────────────
        egui::Panel::top("top")
            .frame(
                egui::Frame::new()
                    .fill(palette::BG_PANEL)
                    .inner_margin(egui::Margin::symmetric(16, 8))
                    .shadow(egui::epaint::Shadow {
                        offset: [0, 3],
                        blur: 10,
                        spread: 0,
                        color: Color32::from_black_alpha(60),
                    }),
            )
            .show(ui, |ui| {
                // Заголовок
                ui.horizontal(|ui| {
                    ui.label(
                        egui::RichText::new(format!("{} — диагностика", ISP_NAME))
                            .size(17.0)
                            .color(palette::TEXT)
                            .strong(),
                    );
                    ui.label(egui::RichText::new(format!("v{}", update::VERSION)).size(11.5).color(palette::TEXT_HINT));
                });

                let (link, ip4, down, up, dns_bad) = {
                    let g = st();
                    (g.summ.link.clone(), g.summ.ip4.clone(), g.summ.down, g.summ.up, !g.summ.dns_problem.is_empty())
                };
                if !link.is_empty() || !ip4.is_empty() || down > 0.0 {
                    ui.add_space(4.0);
                    ui.horizontal_wrapped(|ui| {
                        if !link.is_empty() {
                            chip(ui, &link, palette::ACCENT);
                        }
                        if !ip4.is_empty() {
                            chip(ui, &format!("IP {ip4}"), palette::TEXT_SEC);
                        }
                        if down > 0.0 || up > 0.0 {
                            chip(ui, &format!("↓ {down:.0}   ↑ {up:.0} Мбит/с"), palette::GREEN);
                        }
                        if dns_bad {
                            chip(ui, "проблема DNS", palette::YELLOW);
                        }
                    });
                }

                ui.add_space(8.0);

                // ── Карточка: Логин + Сайты + Комментарий ────────
                card_frame().show(ui, |ui| {
                    // Логин
                    section_title(ui, "Логин");
                    let r = ui.add(
                        egui::TextEdit::singleline(&mut self.login)
                            .hint_text(egui::RichText::new("Ваш логин в системе провайдера").color(palette::TEXT_HINT))
                            .text_color(palette::TEXT)
                            .desired_width(f32::INFINITY),
                    );
                    if r.changed() {
                        let p = login_path();
                        if let Some(d) = p.parent() {
                            let _ = std::fs::create_dir_all(d);
                        }
                        let _ = std::fs::write(p, self.login.trim());
                    }

                    ui.add_space(8.0);

                    // Сайты
                    section_title(ui, "Сайты, которые не открываются (по одному в строке)");
                    ui.add(
                        egui::TextEdit::multiline(&mut self.extra)
                            .hint_text(egui::RichText::new("example.com\nother-site.org").color(palette::TEXT_HINT))
                            .text_color(palette::TEXT)
                            .desired_rows(3)
                            .desired_width(f32::INFINITY),
                    );

                    ui.add_space(8.0);

                    // Комментарий
                    section_title(ui, "Комментарий");
                    ui.add(
                        egui::TextEdit::singleline(&mut self.note)
                            .hint_text(egui::RichText::new("Опишите проблему (необязательно)").color(palette::TEXT_HINT))
                            .text_color(palette::TEXT)
                            .desired_width(f32::INFINITY),
                    );
                });

                ui.add_space(8.0);

                // ── Кнопки действий ──────────────────────────────
                ui.horizontal(|ui| {
                    if btn(ui, "▶  Начать проверку", 1, !busy && !mtr_on) {
                        self.start_check();
                    }
                    let can = self.can_send.load(Relaxed) && !working;
                    if SEND_ENABLED && btn(ui, "↑  Отправить отчёт", 0, can) {
                        self.send();
                    }
                    let just_copied = self.copied_at.map_or(false, |t| t.elapsed().as_millis() < 1500);
                    if btn(ui, if just_copied { "✓  Скопировано" } else { "Копировать отчёт" }, 0, true) {
                        ctx.copy_text(text());
                        self.copied_at = Some(std::time::Instant::now());
                        ctx.request_repaint_after(std::time::Duration::from_millis(1600));
                    }
                    if working {
                        ui.add_space(6.0);
                        ui.spinner();
                        ui.label(
                            egui::RichText::new(if busy {
                                "Проверка..."
                            } else if mtr_on {
                                "MTR..."
                            } else {
                                "Отправка..."
                            })
                            .size(12.5)
                            .color(palette::TEXT_SEC),
                        );
                    }
                });

                ui.add_space(6.0);

                // ── Карточка: MTR ────────────────────────────────
                card_frame().show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.label(egui::RichText::new("MTR до:").size(12.5).color(palette::TEXT_SEC));
                        ui.add(
                            egui::TextEdit::singleline(&mut self.mtr_host)
                                .text_color(palette::TEXT)
                                .desired_width(170.0),
                        );
                        ui.label(egui::RichText::new("циклов (0 = беск.):").size(12.5).color(palette::TEXT_SEC));
                        ui.add(
                            egui::TextEdit::singleline(&mut self.mtr_cycles)
                                .text_color(palette::TEXT)
                                .desired_width(50.0),
                        );
                        let stopping = mtr_on && self.mtr_stop.load(Relaxed);
                        if mtr_on {
                            if btn(ui, "■  Остановить", 2, !stopping) {
                                self.toggle_mtr();
                            }
                        } else if btn(ui, "▶  Запустить MTR", 0, !busy) {
                            self.toggle_mtr();
                        }
                    });
                });

                ui.add_space(4.0);
            });

        // ── Строка состояния ─────────────────────────────────────
        egui::Panel::bottom("status")
            .frame(egui::Frame::new().fill(palette::BG_PANEL).inner_margin(egui::Margin::symmetric(14, 5)))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    let (dot, txt) = if busy {
                        (palette::YELLOW, "Идёт проверка…")
                    } else if mtr_on {
                        (palette::YELLOW, "Идёт MTR…")
                    } else if self.sending.load(Relaxed) {
                        (palette::ACCENT, "Отправка отчёта…")
                    } else if self.can_send.load(Relaxed) {
                        (palette::GREEN, if SEND_ENABLED { "Готово. Отчёт можно отправить" } else { "Готово" })
                    } else {
                        (palette::TEXT_HINT, "Ожидание запуска")
                    };
                    ui.label(egui::RichText::new("●").size(11.0).color(dot));
                    ui.label(egui::RichText::new(txt).size(12.0).color(palette::TEXT_SEC));
                });
            });

        // ── Центральная панель: лог ──────────────────────────────
        egui::CentralPanel::default()
            .frame(
                egui::Frame::new()
                    .fill(palette::BG_BASE)
                    .inner_margin(egui::Margin::symmetric(12, 4)),
            )
            .show(ui, |ui| {
                egui::ScrollArea::both()
                    .auto_shrink([false; 2])
                    .stick_to_bottom(true)
                    .show(ui, |ui| {
                        ui.add_space(4.0);
                        render_log(ui);
                        ui.add_space(4.0);
                    });
            });
    }
}

fn main() -> eframe::Result<()> {
    // TLS-провайдер ring (без C++/cmake/NASM)
    let _ = rustls::crypto::ring::default_provider().install_default();
    let mut vp = egui::ViewportBuilder::default()
            .with_inner_size([780.0, 700.0])
        .with_min_inner_size([620.0, 480.0])
        .with_title(format!("{ISP_NAME} — проверка соединения v{}", update::VERSION));
    if let Some(ic) = window_icon() {
        vp = vp.with_icon(ic);
    }
    let opts = eframe::NativeOptions { viewport: vp, ..Default::default() };
    eframe::run_native("ispcheck", opts, Box::new(|cc| Ok(Box::new(App::new(cc)))))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Паника в потоке проверки не должна оставлять интерфейс в состоянии «идёт проверка».
    #[test]
    fn flags_reset_after_panic() {
        let (busy, can) = (Arc::new(AtomicBool::new(true)), Arc::new(AtomicBool::new(false)));
        let (b, c) = (busy.clone(), can.clone());
        let r = std::thread::spawn(move || {
            let _done = Done { flag: b, can_send: c };
            panic!("сбой проверки");
        })
        .join();
        assert!(r.is_err());
        assert!(!busy.load(Relaxed) && can.load(Relaxed));
    }
}
