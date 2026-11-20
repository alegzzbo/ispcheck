//! Вертикальная раскладка для телефона: всё в одной прокручиваемой колонке.
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

        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(palette::BG_BASE).inner_margin(egui::Margin::symmetric(10, 8)))
            .show(ui, |ui| {
                egui::ScrollArea::vertical().auto_shrink([false; 2]).stick_to_bottom(working).show(ui, |ui| {
                    ui.horizontal_wrapped(|ui| {
                        ui.label(egui::RichText::new(format!("{ISP_NAME} — диагностика")).size(18.0).strong());
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
                        section_title(ui, "Сайты, которые не открываются (по одному в строке)");
                        ui.add(
                            egui::TextEdit::multiline(&mut self.extra)
                                .hint_text("example.com\nother-site.org")
                                .desired_rows(3)
                                .desired_width(f32::INFINITY),
                        );
                        ui.add_space(8.0);
                        section_title(ui, "Комментарий");
                        ui.add(
                            egui::TextEdit::singleline(&mut self.note)
                                .hint_text("Опишите проблему (необязательно)")
                                .desired_width(f32::INFINITY),
                        );
                    });
                    ui.add_space(8.0);

                    ui.horizontal_wrapped(|ui| {
                        if btn(ui, "▶  Начать проверку", 1, !busy && !mtr_on) {
                            self.start_check();
                        }
                        let can = self.can_send.load(Relaxed) && !working;
                        if SEND_ENABLED && btn(ui, "↑  Отправить отчёт", 0, can) {
                            self.send();
                        }
                        let copied = self.copied_at.map_or(false, |t| t.elapsed().as_millis() < 1500);
                        if btn(ui, if copied { "✓  Скопировано" } else { "Копировать отчёт" }, 0, true) {
                            copy_to_clipboard(&ctx, text());
                            self.copied_at = Some(std::time::Instant::now());
                            ctx.request_repaint_after(std::time::Duration::from_millis(1600));
                        }
                    });
                    ui.add_space(6.0);

                    card_frame().show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        section_title(ui, "MTR до узла / циклов (0 = бесконечно)");
                        ui.add(egui::TextEdit::singleline(&mut self.mtr_host).desired_width(f32::INFINITY));
                        ui.add(egui::TextEdit::singleline(&mut self.mtr_cycles).desired_width(80.0));
                        let stopping = mtr_on && self.mtr_stop.load(Relaxed);
                        if mtr_on {
                            if btn(ui, "■  Остановить", 2, !stopping) {
                                self.toggle_mtr();
                            }
                        } else if btn(ui, "▶  Запустить MTR", 0, !busy) {
                            self.toggle_mtr();
                        }
                    });

                    ui.add_space(6.0);
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
