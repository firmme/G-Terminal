//! Shared window chrome and form styling for application dialogs.

use eframe::egui::{self, Color32, RichText, Stroke};

use crate::{
    icons::{self, Icon},
    theme::Palette,
};

pub(crate) struct Dialog<'a> {
    window: egui::Window<'a>,
    title: String,
    icon: Icon,
    palette: Palette,
    open: Option<&'a mut bool>,
    width: f32,
    resizable: bool,
    height: Option<f32>,
}

impl<'a> Dialog<'a> {
    pub(crate) fn new(title: impl Into<String>, icon: Icon, palette: Palette) -> Self {
        let title = title.into();
        Self {
            window: egui::Window::new(&title)
                .title_bar(false)
                .collapsible(false)
                .resizable(false),
            title,
            icon,
            palette,
            open: None,
            width: 420.0,
            resizable: false,
            height: None,
        }
    }

    pub(crate) fn open(mut self, open: &'a mut bool) -> Self {
        self.open = Some(open);
        self
    }
    pub(crate) fn collapsible(self, _: bool) -> Self {
        self
    }
    pub(crate) fn resizable(mut self, value: bool) -> Self {
        self.resizable = value;
        self.window = self.window.resizable(value);
        self
    }
    pub(crate) fn default_width(mut self, width: f32) -> Self {
        self.width = width;
        self.window = self.window.default_width(width);
        self
    }
    pub(crate) fn default_height(mut self, height: f32) -> Self {
        self.height = Some(height);
        self.window = self.window.default_height(height);
        self
    }
    pub(crate) fn default_size(mut self, size: impl Into<egui::Vec2>) -> Self {
        let size = size.into();
        self.width = size.x;
        self.height = Some(size.y);
        self.window = self.window.default_size(size);
        self
    }
    pub(crate) fn min_width(mut self, width: f32) -> Self {
        self.window = self.window.min_width(width);
        self
    }
    pub(crate) fn min_height(mut self, height: f32) -> Self {
        self.window = self.window.min_height(height);
        self
    }
    pub(crate) fn order(mut self, order: egui::Order) -> Self {
        self.window = self.window.order(order);
        self
    }
    pub(crate) fn anchor(mut self, align: egui::Align2, offset: impl Into<egui::Vec2>) -> Self {
        self.window = self.window.anchor(align, offset);
        self
    }

    pub(crate) fn show<R>(
        self,
        ctx: &egui::Context,
        body: impl FnOnce(&mut egui::Ui) -> R,
    ) -> Option<egui::InnerResponse<Option<R>>> {
        if self.open.as_deref() == Some(&false) {
            return None;
        }
        let p = self.palette;
        let screen = ctx.content_rect().shrink(16.0);
        // Native minimize/restore can briefly report a zero-size viewport.
        // Do not let that overwrite a dialog's remembered layout.
        if screen.width() < 120.0 || screen.height() < 120.0 {
            return None;
        }
        let width = self.width.min(screen.width()).max(240.0);
        let height_limit = if self.resizable {
            self.height.unwrap_or(480.0).min(screen.height())
        } else {
            self.height
                .unwrap_or(620.0)
                .min(ctx.content_rect().height() * 0.8)
                .min(screen.height())
        };
        let height_key = egui::Id::new(("dialog-content-height", &self.title));
        let content_height = ctx
            .data(|data| data.get_temp::<f32>(height_key))
            .unwrap_or(0.0);
        let mut close = false;
        let window = if self.resizable {
            self.window
        } else {
            self.window
                .min_width(width)
                .max_width(width)
                .min_height((content_height + 76.0).min(height_limit).max(100.0))
                .max_height(height_limit)
        };
        let result = window
            .default_width(width)
            .default_height(height_limit.max(100.0))
            .default_pos(egui::pos2(
                screen.center().x - width / 2.0,
                screen.top() + 42.0,
            ))
            .constrain_to(screen)
            .frame(frame(p))
            .show(ctx, |ui| {
                form_style(ui, p);
                ui.set_min_width(width.min(ui.available_width()));
                close = header(ui, &self.title, self.icon, p, self.open.is_some());
                egui::Frame::new()
                    .inner_margin(12)
                    .show(ui, |ui| {
                        // The resizable file browser lays out two panes against
                        // a finite height and has its own table scroll areas.
                        if self.resizable {
                            ui.spacing_mut().interact_size.y = 24.0;
                            ui.spacing_mut().button_padding = egui::vec2(8.0, 4.0);
                            ui.spacing_mut().item_spacing = egui::vec2(6.0, 5.0);
                            return body(ui);
                        }
                        let scroll = egui::ScrollArea::vertical()
                            .max_height((height_limit - 76.0).max(24.0))
                            .auto_shrink([false, true])
                            .show(ui, body);
                        ctx.data_mut(|data| data.insert_temp(height_key, scroll.content_size.y));
                        scroll.inner
                    })
                    .inner
            });
        if close && let Some(open) = self.open {
            *open = false;
        }
        result
    }
}

pub(crate) fn frame(p: Palette) -> egui::Frame {
    egui::Frame::new()
        .fill(p.panel)
        .stroke(Stroke::new(1.0_f32, p.line.linear_multiply(1.35)))
        .corner_radius(10)
        .inner_margin(0)
        .shadow(egui::epaint::Shadow {
            offset: [0, 10],
            blur: 32,
            spread: 2,
            color: Color32::from_black_alpha(115),
        })
}

pub(crate) fn form_style(ui: &mut egui::Ui, p: Palette) {
    let style = ui.style_mut();
    style.spacing.item_spacing = egui::vec2(6.0, 6.0);
    style.spacing.button_padding = egui::vec2(10.0, 5.0);
    style.spacing.interact_size.y = 28.0;
    style.spacing.text_edit_width = 240.0;
    style.spacing.combo_width = 140.0;
    style.visuals.widgets.inactive.bg_stroke = Stroke::new(1.0_f32, p.line.linear_multiply(1.4));
    style.visuals.widgets.noninteractive.bg_stroke = Stroke::new(1.0_f32, p.line);
    style
        .text_styles
        .insert(egui::TextStyle::Body, egui::FontId::proportional(14.0));
    style
        .text_styles
        .insert(egui::TextStyle::Button, egui::FontId::proportional(14.0));
}

pub(crate) fn header(
    ui: &mut egui::Ui,
    title: &str,
    icon: Icon,
    p: Palette,
    closable: bool,
) -> bool {
    let (rect, _) =
        ui.allocate_exact_size(egui::vec2(ui.available_width(), 44.0), egui::Sense::hover());
    ui.painter().rect_filled(
        rect,
        egui::CornerRadius {
            nw: 10,
            ne: 10,
            sw: 0,
            se: 0,
        },
        p.raised.gamma_multiply(0.65),
    );
    ui.painter()
        .hline(rect.x_range(), rect.bottom(), Stroke::new(1.0_f32, p.line));
    let glyph = egui::Rect::from_center_size(
        egui::pos2(rect.left() + 24.0, rect.center().y),
        egui::Vec2::splat(20.0),
    );
    icons::draw(
        ui.painter(),
        glyph,
        icon,
        if icon == Icon::Warning {
            p.warn
        } else {
            p.accent
        },
        1.9,
    );
    ui.painter().text(
        egui::pos2(rect.left() + 46.0, rect.center().y),
        egui::Align2::LEFT_CENTER,
        title,
        egui::FontId::proportional(18.0),
        p.text,
    );
    if closable {
        let rect = egui::Rect::from_center_size(
            egui::pos2(rect.right() - 24.0, rect.center().y),
            egui::vec2(28.0, 28.0),
        );
        let response = ui
            .interact(rect, ui.id().with("dialog-close"), egui::Sense::click())
            .on_hover_text("关闭");
        if response.hovered() {
            ui.painter().rect_filled(rect, 6, p.raised);
        }
        icons::draw(ui.painter(), rect.shrink(7.0), Icon::Close, p.muted, 1.5);
        response.clicked()
    } else {
        false
    }
}

pub(crate) fn card<R>(
    ui: &mut egui::Ui,
    p: Palette,
    icon: Icon,
    title: &str,
    description: &str,
    body: impl FnOnce(&mut egui::Ui) -> R,
) -> R {
    egui::Frame::new()
        .fill(p.bg.gamma_multiply(0.8))
        .stroke(Stroke::new(1.0_f32, p.line))
        .corner_radius(8)
        .inner_margin(12)
        .show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            ui.horizontal(|ui| {
                let (rect, _) =
                    ui.allocate_exact_size(egui::vec2(28.0, 28.0), egui::Sense::hover());
                ui.painter().rect_filled(rect, 7, p.raised);
                icons::draw(ui.painter(), rect.shrink(6.0), icon, p.accent, 1.4);
                ui.vertical(|ui| {
                    ui.spacing_mut().item_spacing.y = 3.0;
                    ui.label(RichText::new(title).strong().size(15.0).color(p.text));
                    if !description.is_empty() {
                        ui.label(RichText::new(description).size(12.0).color(p.muted));
                    }
                });
            });
            ui.separator();
            body(ui)
        })
        .inner
}

pub(crate) fn primary(ui: &mut egui::Ui, label: &str, p: Palette) -> egui::Response {
    ui.add(
        egui::Button::new(RichText::new(label).color(if p.panel.r() > 128 {
            Color32::WHITE
        } else {
            Color32::from_rgb(7, 25, 32)
        }))
        .fill(p.accent)
        .min_size(egui::vec2(96.0, 32.0)),
    )
}

pub(crate) fn icon_button(ui: &mut egui::Ui, icon: Icon, p: Palette) -> egui::Response {
    let response = ui.add_sized([28.0, 28.0], egui::Button::new(""));
    icons::draw(ui.painter(), response.rect.shrink(6.0), icon, p.muted, 1.3);
    response
}

pub(crate) fn secondary(ui: &mut egui::Ui, label: &str) -> egui::Response {
    ui.add(egui::Button::new(label).min_size(egui::vec2(80.0, 32.0)))
}

pub(crate) fn footer<R>(ui: &mut egui::Ui, body: impl FnOnce(&mut egui::Ui) -> R) -> R {
    ui.add_space(4.0);
    // Limit the row height before changing direction. A free-standing
    // horizontal layout would center its buttons in the entire scroll body.
    ui.horizontal(|ui| {
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), body)
            .inner
    })
    .inner
}

pub(crate) fn tab(ui: &mut egui::Ui, label: &str, selected: bool, p: Palette) -> egui::Response {
    let response = ui.add_sized(
        [76.0, 30.0],
        egui::Button::new(RichText::new(label).size(15.0).color(if selected {
            p.accent
        } else {
            p.muted
        }))
        .frame(false),
    );
    if selected {
        ui.painter().hline(
            response.rect.x_range(),
            response.rect.bottom(),
            Stroke::new(3.0_f32, p.accent),
        );
    }
    response
}

#[cfg(test)]
mod tests {
    use super::*;

    fn render(
        ctx: &egui::Context,
        open: &mut bool,
        events: Vec<egui::Event>,
        size: egui::Vec2,
    ) -> Option<egui::Rect> {
        let mut rect = None;
        let _ = ctx.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, size)),
                events,
                ..Default::default()
            },
            |ctx| {
                rect = Dialog::new("窗口交互", Icon::Settings, Palette::new(false))
                    .open(open)
                    .default_width(420.0)
                    .show(ctx, |ui| {
                        ui.label("可以拖动标题栏，并关闭弹窗。");
                        for _ in 0..8 {
                            ui.label("表单内容");
                        }
                        secondary(ui, "取消");
                    })
                    .map(|shown| shown.response.rect);
            },
        );
        rect
    }

    fn pointer(pos: egui::Pos2, pressed: bool) -> egui::Event {
        egui::Event::PointerButton {
            pos,
            pressed,
            button: egui::PointerButton::Primary,
            modifiers: egui::Modifiers::NONE,
        }
    }

    #[test]
    fn custom_header_can_drag_and_close_the_window() {
        let ctx = egui::Context::default();
        let mut open = true;
        let size = egui::vec2(1280.0, 800.0);
        let mut rect = egui::Rect::NOTHING;
        for _ in 0..3 {
            rect = render(&ctx, &mut open, vec![], size).unwrap();
        }
        assert!(
            rect.height() < 500.0,
            "a short dialog acquired empty space: {rect:?}"
        );
        let start = rect.min + egui::vec2(260.0, 22.0);
        render(
            &ctx,
            &mut open,
            vec![egui::Event::PointerMoved(start), pointer(start, true)],
            size,
        );
        let end = start + egui::vec2(60.0, 24.0);
        for _ in 0..3 {
            rect = render(&ctx, &mut open, vec![egui::Event::PointerMoved(end)], size).unwrap();
        }
        render(&ctx, &mut open, vec![pointer(end, false)], size);
        assert!(
            (rect.left() - (start.x - 260.0 + 60.0)).abs() < 2.0,
            "title drag did not move the window: {rect:?}"
        );
        let close = egui::pos2(rect.right() - 24.0, rect.top() + 22.0);
        render(
            &ctx,
            &mut open,
            vec![egui::Event::PointerMoved(close), pointer(close, true)],
            size,
        );
        render(&ctx, &mut open, vec![pointer(close, false)], size);
        assert!(!open, "the custom close control must close the window");
    }

    #[test]
    fn resizable_browser_receives_a_finite_body_height() {
        let ctx = egui::Context::default();
        let mut open = true;
        for _ in 0..3 {
            let _ = ctx.run(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(1280.0, 800.0),
                    )),
                    ..Default::default()
                },
                |ctx| {
                    Dialog::new("文件布局", Icon::Folder, Palette::new(false))
                        .open(&mut open)
                        .resizable(true)
                        .default_size([900.0, 560.0])
                        .show(ctx, |ui| {
                            let size = ui.available_size();
                            assert!(
                                size.y.is_finite() && size.y < 600.0,
                                "file panes received unbounded height: {size:?}"
                            );
                            ui.allocate_space(size);
                        });
                },
            );
        }
    }

    #[test]
    fn long_forms_scroll_inside_a_bounded_window() {
        for size in [egui::vec2(1280.0, 800.0), egui::vec2(900.0, 600.0)] {
            let ctx = egui::Context::default();
            let mut rect = egui::Rect::NOTHING;
            for _ in 0..12 {
                let _ = ctx.run(
                    egui::RawInput {
                        screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, size)),
                        ..Default::default()
                    },
                    |ctx| {
                        rect = Dialog::new("长表单", Icon::Settings, Palette::new(false))
                            .show(ctx, |ui| {
                                for _ in 0..100 {
                                    ui.label("可滚动的表单内容");
                                }
                            })
                            .unwrap()
                            .response
                            .rect;
                    },
                );
            }
            assert!(
                rect.height() <= size.y * 0.8 + 4.0,
                "a long form must leave room around the window: {rect:?}"
            );
        }
    }

    #[test]
    fn minimized_viewport_does_not_shrink_the_remembered_form() {
        let ctx = egui::Context::default();
        let mut open = true;
        let size = egui::vec2(1280.0, 800.0);
        let mut before = egui::Rect::NOTHING;
        for _ in 0..3 {
            before = render(&ctx, &mut open, vec![], size).unwrap();
        }
        assert!(render(&ctx, &mut open, vec![], egui::Vec2::ZERO).is_none());
        let mut after = egui::Rect::NOTHING;
        for _ in 0..3 {
            after = render(&ctx, &mut open, vec![], size).unwrap();
        }
        assert!(
            (before.height() - after.height()).abs() < 2.0,
            "{before:?} became {after:?}"
        );
    }
}
