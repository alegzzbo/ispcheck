//! Раскладка для телефона: фиксированная шапка, дальше прокручиваемая колонка.
use super::*;

impl App {
    pub(super) fn ui_mobile(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        if !self.themed {
            apply_theme(&ctx);
            ctx.style_mut_of(egui::Theme::Dark, |s| s.spacing.interact_size.y = 44.0);
            self.themed = true;
        }
        let busy = self.busy.load(Relaxed);
        let mtr_on = self.mtr_run.load(Relaxed);
        let sending = self.sending.load(Relaxed);
        let working = busy || mtr_on || sending;

        // системные отступы (статус-бар, вырез, навигация, клавиатура): пиксели -> точки
        let ppp = ctx.pixels_per_point().max(0.5);
        let [il, it, ir, ib] = crate::sys::insets().map(|v| v / ppp);
        let m = |v: f32, extra: f32| (v + extra).round().clamp(0.0, 127.0) as i8;
        let frame = egui::Frame::new().fill(palette::BG_BASE).inner_margin(egui::Margin {
            left: m(il, 12.0),
            right: m(ir, 12.0),
            top: m(it, 8.0),
            bottom: m(ib, 6.0),
        });

        egui::CentralPanel::default().frame(frame).show(ui, |ui| {
            // шапка: название слева, версия справа
            ui.horizontal(|ui| {
                ui.add(
                    egui::Label::new(egui::RichText::new(format!("{ISP_NAME} — диагностика")).size(17.0).strong())
                        .truncate(),
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(egui::RichText::new(format!("v{}", update::VERSION)).size(11.5).color(palette::TEXT_HINT));
                });
            });
            ui.add_space(6.0);

            egui::ScrollArea::vertical().auto_shrink([false; 2]).stick_to_bottom(working).show(ui, |ui| {
                let (link, ip4, down, up, dns_bad) = {
                    let g = st();
                    (g.summ.link.clone(), g.summ.ip4.clone(), g.summ.down, g.summ.up, !g.summ.dns_problem.is_empty())
                };

                // карточка сети: строки друг под другом, IP не сжимается
                if !link.is_empty() || !ip4.is_empty() || down > 0.0 {
                    card_frame().show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        if !link.is_empty() {
                            section_title(ui, "Сеть");
                            ui.label(egui::RichText::new(&link).size(13.5).color(palette::ACCENT));
                        }
                        if !ip4.is_empty() {
                            ui.add_space(6.0);
                            ui.horizontal(|ui| {
                                ui.label(egui::RichText::new("IP").size(12.0).color(palette::TEXT_HINT));
                                let r = egui::Frame::new()
                                    .fill(palette::BG_INPUT)
                                    .stroke(egui::Stroke::new(1.0, palette::BORDER))
                                    .corner_radius(10.0)
                                    .inner_margin(egui::Margin::symmetric(10, 4))
                                    .show(ui, |ui| {
                                        ui.add(
                                            egui::Label::new(egui::RichText::new(&ip4).size(13.0).color(palette::TEXT))
                                                .wrap_mode(egui::TextWrapMode::Extend)
                                                .sense(egui::Sense::click()),
                                        )
                                    })
                                    .inner;
                                if r.clicked() {
                                    copy_to_clipboard(&ctx, ip4.clone());
                                }
                                ui.label(egui::RichText::new("нажмите, чтобы скопировать").size(11.0).color(palette::TEXT_HINT));
                            });
                        }
                        if down > 0.0 || up > 0.0 {
                            ui.add_space(6.0);
                            ui.label(
                                egui::RichText::new(format!("↓ {down:.0}   ↑ {up:.0} Мбит/с")).size(13.0).color(palette::GREEN),
                            );
                        }
                        if dns_bad {
                            ui.add_space(4.0);
                            ui.label(egui::RichText::new("Проблема с DNS").size(12.5).color(palette::YELLOW));
                        }
                    });
                    ui.add_space(8.0);
                }

                card_frame().show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    section_title(ui, "Логин");
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
                    ui.add_space(8.0);
                    section_title(ui, "Сайты, которые не открываются");
                    ui.add(
                        egui::TextEdit::multiline(&mut self.extra)
                            .hint_text("По одному в строке, например: example.com")
                            .desired_rows(3)
                            .desired_width(f32::INFINITY),
                    );
                    ui.add_space(8.0);
                    section_title(ui, "Комментарий");
                    ui.add(
                        egui::TextEdit::multiline(&mut self.note)
                            .hint_text("Опишите проблему (необязательно)")
                            .desired_rows(1)
                            .desired_width(f32::INFINITY),
                    );
                });
                ui.add_space(8.0);

                // основная кнопка на всю ширину
                let w = ui.available_width();
                if busy {
                    btn_w(ui, "Идёт проверка…", 1, false, w);
                } else if btn_w(ui, "▶  Начать проверку", 1, !mtr_on, w) {
                    self.start_check();
                }
                ui.add_space(6.0);

                // «Отправить» и «Копировать» строго 50/50
                ui.columns(2, |cols| {
                    let can = self.can_send.load(Relaxed) && !working;
                    let w0 = cols[0].available_width();
                    if SEND_ENABLED && btn_w(&mut cols[0], "↑  Отправить", 0, can, w0) {
                        self.send();
                    }
                    let copied = self.copied_at.map_or(false, |t| t.elapsed().as_millis() < 1500);
                    let w1 = cols[1].available_width();
                    if btn_w(&mut cols[1], if copied { "✓  Скопировано" } else { "Копировать" }, 0, true, w1) {
                        copy_to_clipboard(&ctx, text());
                        self.copied_at = Some(std::time::Instant::now());
                        ctx.request_repaint_after(std::time::Duration::from_millis(1600));
                    }
                });
                ui.add_space(8.0);

                // MTR: свёрнут по умолчанию
                egui::CollapsingHeader::new(egui::RichText::new("MTR (для специалистов)").size(13.0).color(palette::TEXT_SEC))
                    .default_open(false)
                    .show(ui, |ui| {
                        card_frame().show(ui, |ui| {
                            ui.set_width(ui.available_width());
                            let total = ui.available_width();
                            let sp = ui.spacing().item_spacing.x;
                            let (wh, wc) = ((total - sp) * 0.7, (total - sp) * 0.3);
                            ui.horizontal_top(|ui| {
                                ui.vertical(|ui| {
                                    ui.set_width(wh);
                                    section_title(ui, "Узел / IP");
                                    ui.add(egui::TextEdit::singleline(&mut self.mtr_host).desired_width(wh));
                                });
                                ui.vertical(|ui| {
                                    ui.set_width(wc);
                                    section_title(ui, "Циклы");
                                    ui.add(
                                        egui::TextEdit::singleline(&mut self.mtr_cycles).hint_text("0 = ∞").desired_width(wc),
                                    );
                                });
                            });
                            ui.add_space(6.0);
                            let w = ui.available_width();
                            let stopping = mtr_on && self.mtr_stop.load(Relaxed);
                            if mtr_on {
                                if btn_w(ui, "■  Остановить MTR", 2, !stopping, w) {
                                    self.toggle_mtr();
                                }
                            } else if btn_w(ui, "▶  Запустить MTR", 0, !busy, w) {
                                self.toggle_mtr();
                            }
                        });
                    });

                ui.add_space(8.0);
                // статус и индикатор выполнения
                let (dot, txt) = if busy {
                    (palette::YELLOW, "Идёт проверка…")
                } else if mtr_on {
                    (palette::YELLOW, "Идёт MTR…")
                } else if sending {
                    (palette::ACCENT, "Отправка отчёта…")
                } else if self.can_send.load(Relaxed) {
                    (palette::GREEN, if SEND_ENABLED { "Готово. Отчёт можно отправить" } else { "Готово" })
                } else {
                    (palette::TEXT_HINT, "Ожидание запуска")
                };
                if working {
                    let t = ui.input(|i| i.time);
                    ui.add(
                        egui::ProgressBar::new(((t * 0.5) % 1.0) as f32)
                            .desired_height(4.0)
                            .fill(palette::ACCENT),
                    );
                    ctx.request_repaint();
                }
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new("●").size(11.0).color(dot));
                    ui.label(egui::RichText::new(txt).size(12.0).color(palette::TEXT_SEC));
                });
                ui.add_space(4.0);

                egui::ScrollArea::horizontal().auto_shrink([false, true]).show(ui, |ui| render_log(ui));
                ui.add_space(12.0);
            });
        });
    }
}
