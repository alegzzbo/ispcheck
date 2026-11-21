//! Раскладка для телефона: шапка, две вкладки («Параметры» без прокрутки и «Результат»).
use super::*;

fn save_login(login: &str) {
    let p = login_path();
    if let Some(d) = p.parent() {
        let _ = std::fs::create_dir_all(d);
    }
    let _ = std::fs::write(p, login.trim());
}

const FOOTER_H: f32 = 56.0;

impl App {
    pub(super) fn ui_mobile(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        if !self.themed {
            apply_theme(&ctx);
            ctx.style_mut_of(egui::Theme::Dark, |s| {
                s.spacing.interact_size.y = 40.0;
                s.spacing.item_spacing.y = 6.0;
            });
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
            top: m(it, 6.0),
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

            // вкладки
            ui.columns(2, |cols| {
                for (i, label) in ["Параметры", if working { "Результат  ●" } else { "Результат" }].iter().enumerate() {
                    let sel = self.tab as usize == i;
                    let w = cols[i].available_width();
                    let r = cols[i].add(
                        egui::Button::new(
                            egui::RichText::new(*label)
                                .size(14.0)
                                .color(if sel { palette::ACCENT } else { palette::TEXT_SEC }),
                        )
                        .frame(false)
                        .truncate()
                        .min_size(egui::vec2(w, 38.0)),
                    );
                    let y = r.rect.bottom();
                    let (col, th) = if sel { (palette::ACCENT, 2.5) } else { (palette::BORDER, 1.0) };
                    cols[i].painter().hline(r.rect.x_range(), y, egui::Stroke::new(th, col));
                    if r.clicked() {
                        self.tab = i as u8;
                    }
                }
            });
            ui.add_space(4.0);

            self.ime_now = None;
            if self.tab == 0 {
                self.tab_params(ui, &ctx, busy, mtr_on);
            } else {
                self.tab_result(ui, &ctx, busy, mtr_on, sending, working);
            }
        });
        self.sync_ime(&ctx);

        // свайп влево/вправо переключает вкладки
        let (pressed, released, pos) =
            ctx.input(|i| (i.pointer.primary_pressed(), i.pointer.any_released(), i.pointer.latest_pos()));
        if pressed {
            self.swipe_from = pos;
        }
        if released {
            if let (Some(a), Some(b)) = (self.swipe_from.take(), pos) {
                let (dx, dy) = (b.x - a.x, b.y - a.y);
                if dx.abs() > 90.0 && dy.abs() < 40.0 {
                    self.tab = if dx < 0.0 { 1 } else { 0 };
                }
            }
        }
    }

    fn field_mut(&mut self, i: u8) -> &mut String {
        match i {
            0 => &mut self.login,
            1 => &mut self.extra,
            2 => &mut self.note,
            3 => &mut self.mtr_host,
            _ => &mut self.mtr_cycles,
        }
    }

    /// Экранная клавиатура: фокус поля -> показать IME; текст из IME -> в поле.
    fn sync_ime(&mut self, ctx: &egui::Context) {
        let now = self.ime_now.take();
        if now != self.ime_field {
            self.ime_field = now;
            match now {
                Some(i) => {
                    let t = self.field_mut(i).clone();
                    crate::sys::ime_focus(&t);
                }
                None => crate::sys::ime_blur(),
            }
        } else if let Some(i) = now {
            if let Some(t) = crate::sys::ime_read() {
                if *self.field_mut(i) != t {
                    *self.field_mut(i) = t.clone();
                    if i == 0 {
                        save_login(&t);
                    }
                }
            }
        }
        if now.is_some() {
            ctx.request_repaint_after(std::time::Duration::from_millis(80));
        }
    }

    fn tab_params(&mut self, ui: &mut egui::Ui, ctx: &egui::Context, busy: bool, mtr_on: bool) {
        // страховка: на очень маленьком экране прокрутка появится сама, на обычном её нет
        egui::ScrollArea::vertical().auto_shrink([false; 2]).show(ui, |ui| {
            let (link, ip4, down, up, dns_bad) = {
                let g = st();
                (g.summ.link.clone(), g.summ.ip4.clone(), g.summ.down, g.summ.up, !g.summ.dns_problem.is_empty())
            };

            // карточка сети: строки друг под другом, IP не сжимается
            if !link.is_empty() || !ip4.is_empty() || down > 0.0 {
                card_frame().inner_margin(egui::Margin::symmetric(12, 8)).show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    if !link.is_empty() {
                        ui.label(egui::RichText::new(&link).size(13.0).color(palette::ACCENT));
                    }
                    if !ip4.is_empty() {
                        ui.horizontal(|ui| {
                            let r = egui::Frame::new()
                                .fill(palette::BG_INPUT)
                                .stroke(egui::Stroke::new(1.0, palette::BORDER))
                                .corner_radius(10.0)
                                .inner_margin(egui::Margin::symmetric(10, 3))
                                .show(ui, |ui| {
                                    ui.add(
                                        egui::Label::new(
                                            egui::RichText::new(format!("IP: {ip4}")).size(13.0).color(palette::TEXT),
                                        )
                                        .wrap_mode(egui::TextWrapMode::Extend)
                                        .sense(egui::Sense::click()),
                                    )
                                })
                                .inner;
                            if r.clicked() {
                                copy_to_clipboard(ctx, ip4.clone());
                            }
                            ui.add(
                                egui::Label::new(egui::RichText::new("тап — копировать").size(11.0).color(palette::TEXT_HINT))
                                    .truncate(),
                            );
                        });
                    }
                    if down > 0.0 || up > 0.0 {
                        ui.label(
                            egui::RichText::new(format!("Скорость (замер): ↓ {down:.0} загрузка, ↑ {up:.0} отдача Мбит/с"))
                                .size(12.5)
                                .color(palette::GREEN),
                        );
                    }
                    if dns_bad {
                        ui.label(egui::RichText::new("Проблема с DNS").size(12.5).color(palette::YELLOW));
                    }
                });
                ui.add_space(6.0);
            }

            card_frame().inner_margin(egui::Margin::symmetric(12, 8)).show(ui, |ui| {
                ui.set_width(ui.available_width());
                section_title(ui, "Логин");
                let r = ui.add(
                    egui::TextEdit::singleline(&mut self.login)
                        .hint_text("Ваш логин в системе провайдера")
                        .desired_width(f32::INFINITY),
                );
                if r.has_focus() {
                    self.ime_now = Some(0);
                }
                if r.changed() {
                    save_login(&self.login);
                }
                section_title(ui, "Сайты, которые не открываются");
                let r = ui.add(
                    egui::TextEdit::multiline(&mut self.extra)
                        .hint_text("По одному в строке: example.com")
                        .desired_rows(2)
                        .desired_width(f32::INFINITY),
                );
                if r.has_focus() {
                    self.ime_now = Some(1);
                }
                section_title(ui, "Комментарий");
                let r = ui.add(
                    egui::TextEdit::multiline(&mut self.note)
                        .hint_text("Опишите проблему (необязательно)")
                        .desired_rows(1)
                        .desired_width(f32::INFINITY),
                );
                if r.has_focus() {
                    self.ime_now = Some(2);
                }
            });
            ui.add_space(6.0);

            // основная кнопка на всю ширину
            let w = ui.available_width();
            if busy {
                // отмены проверки в ядре нет: кнопка неактивна до завершения
                btn_w(ui, "Идёт проверка…", 1, false, w);
            } else if btn_w(ui, "▶  Начать проверку", 1, !mtr_on, w) {
                self.start_check();
                self.tab = 1;
            }
            ui.add_space(6.0);

            // MTR: хост 70% + циклы 30%, кнопка на всю ширину
            card_frame().inner_margin(egui::Margin::symmetric(12, 8)).show(ui, |ui| {
                ui.set_width(ui.available_width());
                let total = ui.available_width();
                let sp = ui.spacing().item_spacing.x;
                let (wh, wc) = ((total - sp) * 0.7, (total - sp) * 0.3);
                ui.horizontal_top(|ui| {
                    ui.vertical(|ui| {
                        ui.set_width(wh);
                        section_title(ui, "MTR: узел / IP");
                        if ui.add(egui::TextEdit::singleline(&mut self.mtr_host).desired_width(wh)).has_focus() {
                            self.ime_now = Some(3);
                        }
                    });
                    ui.vertical(|ui| {
                        ui.set_width(wc);
                        section_title(ui, "Циклы");
                        if ui
                            .add(egui::TextEdit::singleline(&mut self.mtr_cycles).hint_text("0 = ∞").desired_width(wc))
                            .has_focus()
                        {
                            self.ime_now = Some(4);
                        }
                    });
                });
                let w = ui.available_width();
                let stopping = mtr_on && self.mtr_stop.load(Relaxed);
                if mtr_on {
                    if btn_w(ui, "■  Остановить MTR", 2, !stopping, w) {
                        self.toggle_mtr();
                    }
                } else if btn_w(ui, "▶  Запустить MTR", 0, !busy, w) {
                    self.toggle_mtr();
                    self.tab = 1;
                }
            });
        });
    }

    fn tab_result(&mut self, ui: &mut egui::Ui, ctx: &egui::Context, busy: bool, mtr_on: bool, sending: bool, working: bool) {
        // статус и индикатор выполнения
        let has_text = !text().is_empty();
        let (dot, txt) = if busy {
            (palette::YELLOW, "Идёт проверка…")
        } else if mtr_on {
            (palette::YELLOW, "Идёт MTR…")
        } else if sending {
            (palette::ACCENT, "Отправка отчёта…")
        } else if has_text {
            (palette::GREEN, "Проверка завершена")
        } else {
            (palette::TEXT_HINT, "Нет данных")
        };
        if working {
            let t = ui.input(|i| i.time);
            ui.add(egui::ProgressBar::new(((t * 0.5) % 1.0) as f32).desired_height(4.0).fill(palette::ACCENT));
            ctx.request_repaint();
        } else {
            ui.add_space(4.0);
        }
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new("●").size(11.0).color(dot));
            ui.label(egui::RichText::new(txt).size(12.0).color(palette::TEXT_SEC));
        });
        ui.add_space(2.0);

        // область вывода (прокручивается), под ней закреплённая панель действий
        let h = (ui.available_height() - FOOTER_H - ui.spacing().item_spacing.y).max(80.0);
        if !has_text && !working {
            ui.allocate_ui(egui::vec2(ui.available_width(), h), |ui| {
                ui.centered_and_justified(|ui| {
                    ui.label(
                        egui::RichText::new("Запустите проверку на вкладке «Параметры», чтобы увидеть отчёт")
                            .size(14.0)
                            .color(palette::TEXT_HINT),
                    );
                });
            });
        } else {
            egui::ScrollArea::vertical().max_height(h).auto_shrink([false, false]).stick_to_bottom(working).show(ui, |ui| {
                egui::ScrollArea::horizontal().auto_shrink([false, true]).show(ui, |ui| render_log(ui));
            });
        }

        // нижняя панель: «Копировать» и «Отправить» строго 50/50
        ui.add_space(4.0);
        ui.columns(2, |cols| {
            let copied = self.copied_at.map_or(false, |t| t.elapsed().as_millis() < 1500);
            let w0 = cols[0].available_width();
            if btn_w(&mut cols[0], if copied { "✓  Скопировано" } else { "⧉  Копировать" }, 0, has_text, w0) {
                copy_to_clipboard(ctx, text());
                self.copied_at = Some(std::time::Instant::now());
                ctx.request_repaint_after(std::time::Duration::from_millis(1600));
            }
            let can = SEND_ENABLED && self.can_send.load(Relaxed) && has_text && !working;
            let w1 = cols[1].available_width();
            if btn_w(&mut cols[1], "↑  Отправить", 0, can, w1) {
                self.send();
            }
        });
    }
}
