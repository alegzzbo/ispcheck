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

// ─── Цветовая палитра ───────────────────────────────────────────────
mod palette {
    use eframe::egui::Color32;
    // Фон
    pub const BG_DARK: Color32 = Color32::from_rgb(22, 27, 34);
    pub const BG_PANEL: Color32 = Color32::from_rgb(30, 37, 46);
    pub const BG_INPUT: Color32 = Color32::from_rgb(38, 47, 58);
    pub const BG_HOVER: Color32 = Color32::from_rgb(48, 58, 72);
    // Рамки
    pub const BORDER: Color32 = Color32::from_rgb(56, 68, 82);
    pub const BORDER_FOCUS: Color32 = Color32::from_rgb(88, 166, 255);
    // Текст
    pub const TEXT: Color32 = Color32::from_rgb(230, 237, 243);
    pub const TEXT_DIM: Color32 = Color32::from_rgb(139, 148, 158);
    pub const TEXT_HINT: Color32 = Color32::from_rgb(100, 110, 120);
    // Акценты
    pub const ACCENT: Color32 = Color32::from_rgb(88, 166, 255);
    pub const GREEN: Color32 = Color32::from_rgb(63, 185, 80);
    pub const RED: Color32 = Color32::from_rgb(248, 81, 73);
    pub const YELLOW: Color32 = Color32::from_rgb(210, 167, 52);
    #[allow(dead_code)]
    pub const CYAN: Color32 = Color32::from_rgb(57, 211, 211);
    // Кнопки
    pub const BTN_PRIMARY: Color32 = Color32::from_rgb(35, 134, 54);
    pub const BTN_PRIMARY_HOVER: Color32 = Color32::from_rgb(46, 160, 67);
    pub const BTN_SECONDARY: Color32 = Color32::from_rgb(48, 54, 61);
    pub const BTN_SECONDARY_HOVER: Color32 = Color32::from_rgb(55, 62, 71);
    pub const BTN_DANGER: Color32 = Color32::from_rgb(218, 54, 51);
    pub const BTN_DANGER_HOVER: Color32 = Color32::from_rgb(248, 81, 73);
}

fn apply_theme(ctx: &egui::Context) {
    use palette::*;

    ctx.style_mut_of(egui::Theme::Dark, |style| {
        let v = &mut style.visuals;

        v.dark_mode = true;
        v.override_text_color = Some(TEXT);

        // Окно
        v.window_fill = BG_PANEL;
        v.window_stroke = egui::Stroke::new(1.0, BORDER);
        v.window_shadow = egui::epaint::Shadow {
            offset: [0, 4],
            blur: 12,
            spread: 0,
            color: Color32::from_black_alpha(80),
        };
        v.window_corner_radius = egui::CornerRadius::same(10);

        // Панели
        v.panel_fill = BG_DARK;

        // Виджеты
        let w = &mut v.widgets;
        // Неактивные
        w.noninteractive.bg_fill = BG_INPUT;
        w.noninteractive.bg_stroke = egui::Stroke::new(1.0, BORDER);
        w.noninteractive.fg_stroke = egui::Stroke::new(1.0, TEXT_DIM);
        w.noninteractive.corner_radius = egui::CornerRadius::same(6);

        // Неактивные кнопки/элементы
        w.inactive.bg_fill = BTN_SECONDARY;
        w.inactive.bg_stroke = egui::Stroke::new(1.0, BORDER);
        w.inactive.fg_stroke = egui::Stroke::new(1.0, TEXT);
        w.inactive.corner_radius = egui::CornerRadius::same(6);

        // При наведении
        w.hovered.bg_fill = BTN_SECONDARY_HOVER;
        w.hovered.bg_stroke = egui::Stroke::new(1.0, BORDER_FOCUS);
        w.hovered.fg_stroke = egui::Stroke::new(1.0, TEXT);
        w.hovered.corner_radius = egui::CornerRadius::same(6);

        // При нажатии
        w.active.bg_fill = BG_HOVER;
        w.active.bg_stroke = egui::Stroke::new(1.5, ACCENT);
        w.active.fg_stroke = egui::Stroke::new(1.0, TEXT);
        w.active.corner_radius = egui::CornerRadius::same(6);

        // Открытое (выделенное)
        w.open.bg_fill = BG_HOVER;
        w.open.bg_stroke = egui::Stroke::new(1.0, ACCENT);
        w.open.fg_stroke = egui::Stroke::new(1.0, TEXT);
        w.open.corner_radius = egui::CornerRadius::same(6);

        // Текстовые поля: курсор, выделение
        v.text_cursor.stroke = egui::Stroke::new(2.0, ACCENT);
        v.selection.bg_fill = Color32::from_rgba_premultiplied(88, 166, 255, 60);
        v.selection.stroke = egui::Stroke::new(0.0, Color32::TRANSPARENT);

        // Разделители
        v.widgets.noninteractive.bg_stroke = egui::Stroke::new(1.0, BORDER);

        // Скроллбар
        v.extreme_bg_color = BG_INPUT;

        // Пространство между элементами
        style.spacing.item_spacing = egui::vec2(8.0, 6.0);
        style.spacing.button_padding = egui::vec2(14.0, 5.0);
        style.spacing.window_margin = egui::Margin::same(16);
    });
    ctx.set_theme(egui::Theme::Dark);
}

use egui::Color32;

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
    theme_applied: bool,
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
            theme_applied: false,
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

/// Стилизованная кнопка: primary (зелёная), secondary (серая), danger (красная).
fn styled_button(ui: &mut egui::Ui, label: &str, kind: &str, enabled: bool) -> bool {
    use palette::*;
    let (fill, fill_hover) = match kind {
        "primary" => (BTN_PRIMARY, BTN_PRIMARY_HOVER),
        "danger" => (BTN_DANGER, BTN_DANGER_HOVER),
        _ => (BTN_SECONDARY, BTN_SECONDARY_HOVER),
    };
    let text_color = if enabled { TEXT } else { TEXT_DIM };
    let btn = egui::Button::new(
        egui::RichText::new(label).color(text_color),
    )
    .fill(if enabled { fill } else { BG_INPUT })
    .corner_radius(6.0)
    .stroke(egui::Stroke::new(1.0, if enabled { BORDER } else { Color32::from_rgb(45, 55, 65) }));
    let resp = ui.add_enabled(enabled, btn);
    // Подкрашиваем при наведении вручную через перерисовку фона
    if resp.hovered() && enabled {
        ui.painter().rect_filled(resp.rect, 6.0, fill_hover);
        // перерисовываем текст поверх
        ui.painter().text(
            resp.rect.center(),
            egui::Align2::CENTER_CENTER,
            label,
            egui::FontId::proportional(14.0),
            TEXT,
        );
    }
    resp.clicked()
}

/// Заголовок секции с иконкой.
fn section_label(ui: &mut egui::Ui, icon: &str, text: &str) {
    ui.horizontal(|ui| {
        ui.add_space(2.0);
        ui.label(egui::RichText::new(icon).size(14.0).color(palette::ACCENT));
        ui.label(
            egui::RichText::new(text)
                .size(13.0)
                .color(palette::TEXT_DIM),
        );
    });
}

/// Разделитель-линия.
fn separator_line(ui: &mut egui::Ui) {
    let rect = ui.available_rect_before_wrap();
    let y = rect.top() + 1.0;
    ui.painter().line_segment(
        [egui::pos2(rect.left(), y), egui::pos2(rect.right(), y)],
        egui::Stroke::new(1.0, palette::BORDER),
    );
    ui.add_space(4.0);
}

/// Рисует раскрашенный лог в ScrollArea.
fn render_log(ui: &mut egui::Ui) {
    let txt = text();
    if txt.is_empty() {
        ui.centered_and_justified(|ui| {
            ui.label(
                egui::RichText::new("Нажмите «Начать проверку», чтобы запустить диагностику")
                    .size(14.0)
                    .color(palette::TEXT_HINT),
            );
        });
        return;
    }
    let job = colorize_log(&txt, ui.style());
    ui.add(egui::Label::new(job).selectable(true));
}

/// Создаёт LayoutJob с подсветкой маркеров.
fn colorize_log(text: &str, _style: &egui::Style) -> egui::text::LayoutJob {
    let mut job = egui::text::LayoutJob::default();
    job.wrap = egui::text::TextWrapping {
        max_rows: usize::MAX,
        break_anywhere: false,
        overflow_character: None,
        max_width: f32::INFINITY,
    };
    let mono = egui::FontId::monospace(13.0);
    let mono_bold = egui::FontId::monospace(13.0);

    for line in text.split('\n') {
        if line.is_empty() {
            job.append("\n", 0.0, egui::text::TextFormat {
                font_id: mono.clone(),
                color: palette::TEXT,
                ..Default::default()
            });
            continue;
        }
        // Определяем маркер (первый символ/слово)
        let (marker, rest) = if line.starts_with('✔') {
            ("✔", &line[OK.len()..])
        } else if line.starts_with('✖') {
            ("✖", &line[BAD.len()..])
        } else if line.starts_with('⚠') {
            ("⚠", &line[WARN.len()..])
        } else if line.starts_with('▶') {
            ("▶", &line[HDR.len()..])
        } else if line.starts_with('─') {
            // Линия-разделитель → тонкая линия в цвет границы
            job.append(line, 0.0, egui::text::TextFormat {
                font_id: mono.clone(),
                color: palette::BORDER,
                ..Default::default()
            });
            job.append("\n", 0.0, egui::text::TextFormat {
                font_id: mono.clone(),
                color: palette::TEXT,
                ..Default::default()
            });
            continue;
        } else if line.starts_with('·') {
            ("·", &line['·'.len_utf8()..])
        } else {
            ("", line)
        };

        let (marker_color, text_color, bold) = match marker {
            "✔" => (palette::GREEN, palette::TEXT, false),
            "✖" => (palette::RED, palette::RED, false),
            "⚠" => (palette::YELLOW, palette::YELLOW, false),
            "▶" => (palette::ACCENT, palette::ACCENT, true),
            "·" => (palette::TEXT_DIM, palette::TEXT_DIM, false),
            _ => (palette::TEXT, palette::TEXT, false),
        };

        if !marker.is_empty() {
            job.append(marker, 0.0, egui::text::TextFormat {
                font_id: if bold { mono_bold.clone() } else { mono.clone() },
                color: marker_color,
                ..Default::default()
            });
        }

        job.append(rest, 0.0, egui::text::TextFormat {
            font_id: mono.clone(),
            color: text_color,
            ..Default::default()
        });

        job.append("\n", 0.0, egui::text::TextFormat {
            font_id: mono.clone(),
            color: palette::TEXT,
            ..Default::default()
        });
    }

    job
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _: &mut eframe::Frame) {
        let ctx = &ui.ctx().clone();

        if !self.theme_applied {
            apply_theme(ctx);
            self.theme_applied = true;
        }

        let busy = self.busy.load(Relaxed);
        let mtr_on = self.mtr_run.load(Relaxed);
        let working = busy || mtr_on || self.sending.load(Relaxed);

        // ── Диалог обновления ─────────────────────────────────────
        let pending = PENDING.lock().unwrap().clone();
        if let Some((info, f)) = pending {
            egui::Window::new(
                egui::RichText::new("🔄 Обновление").size(16.0).color(palette::TEXT),
            )
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
                    .size(14.0)
                    .color(palette::TEXT),
                );
                if !info.notes.is_empty() {
                    ui.add_space(4.0);
                    ui.label(
                        egui::RichText::new(&info.notes)
                            .size(13.0)
                            .color(palette::TEXT_DIM),
                    );
                }
                ui.add_space(8.0);
                if self.updating.load(Relaxed) {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label(
                            egui::RichText::new("Загрузка и проверка…")
                                .color(palette::TEXT_DIM),
                        );
                    });
                } else {
                    ui.horizontal(|ui| {
                        if styled_button(ui, "Обновить", "primary", true) {
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
                        if styled_button(ui, "Позже", "secondary", true) {
                            *PENDING.lock().unwrap() = None;
                        }
                    });
                }
            });
        }

        // ── Верхняя панель ────────────────────────────────────────
        egui::Panel::top("top")
            .frame(egui::Frame::new().fill(palette::BG_PANEL).inner_margin(egui::Margin::symmetric(12, 0)))
            .show(ui, |ui| {
                ui.add_space(8.0);

                // Заголовок приложения
                ui.horizontal(|ui| {
                    ui.add_space(4.0);
                    ui.label(
                        egui::RichText::new("🌐")
                            .size(20.0),
                    );
                    ui.label(
                        egui::RichText::new(format!("{} — диагностика", ISP_NAME))
                            .size(17.0)
                            .color(palette::TEXT)
                            .strong(),
                    );
                    ui.label(
                        egui::RichText::new(format!("v{}", update::VERSION))
                            .size(12.0)
                            .color(palette::TEXT_DIM),
                    );
                });

                ui.add_space(6.0);
                separator_line(ui);
                ui.add_space(2.0);

                // Логин
                section_label(ui, "👤", "Логин");
                ui.add_space(2.0);
                let r = ui.add(
                    egui::TextEdit::singleline(&mut self.login)
                        .hint_text(
                            egui::RichText::new("Ваш логин в системе провайдера")
                                .color(palette::TEXT_HINT),
                        )
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

                ui.add_space(6.0);

                // Проблемные сайты
                section_label(ui, "🔗", "Сайты, которые не открываются");
                ui.add_space(2.0);
                ui.add(
                    egui::TextEdit::multiline(&mut self.extra)
                        .hint_text(
                            egui::RichText::new("example.com\nother-site.org")
                                .color(palette::TEXT_HINT),
                        )
                        .text_color(palette::TEXT)
                        .desired_rows(3)
                        .desired_width(f32::INFINITY),
                );

                ui.add_space(4.0);

                // Комментарий
                section_label(ui, "💬", "Комментарий");
                ui.add_space(2.0);
                ui.add(
                    egui::TextEdit::singleline(&mut self.note)
                        .hint_text(
                            egui::RichText::new("Опишите проблему (необязательно)")
                                .color(palette::TEXT_HINT),
                        )
                        .text_color(palette::TEXT)
                        .desired_width(f32::INFINITY),
                );

                ui.add_space(8.0);
                separator_line(ui);
                ui.add_space(2.0);

                // Кнопки действий
                ui.horizontal(|ui| {
                    if styled_button(ui, "▶  Начать проверку", "primary", !busy && !mtr_on) {
                        self.start_check();
                    }
                    let can = self.can_send.load(Relaxed) && !working;
                    if styled_button(ui, "📤  Отправить отчёт", "secondary", can) {
                        self.send();
                    }
                    if styled_button(ui, "📋  Копировать", "secondary", true) {
                        ctx.copy_text(report());
                    }
                    if working {
                        ui.add_space(4.0);
                        ui.spinner();
                        ui.label(
                            egui::RichText::new(if busy {
                                "Проверка…"
                            } else if mtr_on {
                                "MTR…"
                            } else {
                                "Отправка…"
                            })
                            .size(13.0)
                            .color(palette::TEXT_DIM),
                        );
                    }
                });

                ui.add_space(6.0);

                // MTR секция
                ui.horizontal(|ui| {
                    ui.label(
                        egui::RichText::new("📡")
                            .size(14.0),
                    );
                    ui.label(
                        egui::RichText::new("MTR до:")
                            .size(13.0)
                            .color(palette::TEXT_DIM),
                    );
                    ui.add(
                        egui::TextEdit::singleline(&mut self.mtr_host)
                            .text_color(palette::TEXT)
                            .desired_width(160.0),
                    );
                    ui.label(
                        egui::RichText::new("циклов (0 = ∞):")
                            .size(13.0)
                            .color(palette::TEXT_DIM),
                    );
                    ui.add(
                        egui::TextEdit::singleline(&mut self.mtr_cycles)
                            .text_color(palette::TEXT)
                            .desired_width(50.0),
                    );
                    let stopping = mtr_on && self.mtr_stop.load(Relaxed);
                    let (label, kind) = if mtr_on {
                        ("⏹  Остановить", "danger")
                    } else {
                        ("▶  Запустить MTR", "secondary")
                    };
                    if styled_button(ui, label, kind, !busy && !stopping) {
                        self.toggle_mtr();
                    }
                });

                ui.add_space(8.0);
            });

        // ── Центральная панель: лог ───────────────────────────────
        egui::CentralPanel::default()
            .show(ui, |ui| {
                ui.style_mut().visuals.extreme_bg_color = palette::BG_DARK;
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
    let opts = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([780.0, 680.0])
            .with_title(format!("{ISP_NAME} — проверка соединения v{}", update::VERSION)),
        ..Default::default()
    };
    eframe::run_native("ispcheck", opts, Box::new(|cc| Ok(Box::new(App::new(cc)))))
}
