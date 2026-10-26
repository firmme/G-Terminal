//! Dismissible errors float above the workspace; they never reserve terminal rows.
use super::*;

impl App {
    pub(super) fn error_notification(&mut self, ctx: &egui::Context, p: Palette) {
        let Some(error) = self.error.clone() else {
            return;
        };
        let first = error.lines().next().unwrap_or("操作失败");
        let summary: String = first.chars().take(140).collect();
        let shortened = first.chars().count() > 140;
        let summary = if shortened {
            format!("{summary}…")
        } else {
            summary
        };
        let has_details = shortened || error.lines().count() > 1;
        let screen = ctx.content_rect();
        let status_height = ctx
            .data(|data| data.get_temp::<f32>(egui::Id::new("status-bar-height")))
            .unwrap_or(32.0);
        let width = 440.0_f32.min((screen.width() - 32.0).max(160.0));
        let mut dismiss = false;
        egui::Area::new(egui::Id::new("error-notification"))
            .anchor(
                egui::Align2::RIGHT_BOTTOM,
                egui::vec2(-16.0, -status_height - 12.0),
            )
            .order(egui::Order::Foreground)
            .movable(false)
            .show(ctx, |ui| {
                egui::Frame::new()
                    .fill(p.panel)
                    .stroke(egui::Stroke::new(1.0_f32, p.line))
                    .corner_radius(8.0)
                    .inner_margin(12.0)
                    .shadow(egui::epaint::Shadow {
                        offset: [0, 4],
                        blur: 12,
                        spread: 0,
                        color: egui::Color32::from_black_alpha(50),
                    })
                    .show(ui, |ui| {
                        ui.set_width((width - 24.0).max(120.0));
                        ui.spacing_mut().item_spacing = egui::vec2(8.0, 8.0);
                        ui.horizontal_top(|ui| {
                            let (rect, _) =
                                ui.allocate_exact_size(egui::vec2(16.0, 20.0), Sense::hover());
                            icons::draw(ui.painter(), rect, icons::Icon::Warning, p.danger, 1.3);
                            let text_width = (ui.available_width() - 58.0).max(60.0);
                            ui.allocate_ui_with_layout(
                                egui::vec2(text_width, 0.0),
                                Layout::top_down(Align::Min),
                                |ui| {
                                    ui.set_min_width(text_width);
                                    ui.add(
                                        egui::Label::new(
                                            RichText::new(&summary).size(13.0).color(p.text),
                                        )
                                        .wrap(),
                                    );
                                },
                            );
                            if icons::icon_button(ui, icons::Icon::Copy, p, icons::Size::Row)
                                .on_hover_text("复制错误详情")
                                .clicked()
                            {
                                ctx.copy_text(error.clone());
                            }
                            let close = icons::close_button(ui, p, "关闭提示");
                            #[cfg(test)]
                            ctx.data_mut(|data| {
                                data.insert_temp(
                                    egui::Id::new("error-notification-close"),
                                    close.rect,
                                )
                            });
                            dismiss = close.clicked();
                        });
                        if has_details {
                            egui::CollapsingHeader::new("详细信息")
                                .id_salt("error-details")
                                .show(ui, |ui| {
                                    egui::ScrollArea::vertical()
                                        .max_height((screen.height() * 0.25).clamp(60.0, 180.0))
                                        .show(ui, |ui| {
                                            ui.add(
                                                egui::Label::new(
                                                    RichText::new(&error).size(12.0).color(p.muted),
                                                )
                                                .wrap()
                                                .selectable(true),
                                            );
                                        });
                                });
                        }
                    });
            });
        if dismiss {
            self.error = None;
        }
    }
}
