//! The navigation sidebar: local shells, saved connections and their groups,
//! plus the floating overlay that auto-hide mode uses.

use super::*;

impl App {
    pub(super) fn sidebar(&mut self, ctx: &egui::Context, action: &mut Option<Action>) {
        let p = self.palette;
        egui::SidePanel::left("navigation")
            .default_width(185.0)
            .width_range(140.0..=320.0)
            .frame(egui::Frame::new().fill(p.panel).inner_margin(5))
            .show(ctx, |ui| self.sidebar_contents(ui, action));
    }
    /// The navigation bar's body, shared by the docked panel and the floating
    /// overlay that auto-hide uses.
    pub(super) fn sidebar_contents(&mut self, ui: &mut egui::Ui, action: &mut Option<Action>) {
        let p = self.palette;
        egui::ScrollArea::vertical().show(ui, |ui| {
            egui::CollapsingHeader::new(RichText::new("本地 Shell").color(p.muted))
                .default_open(true)
                .show(ui, |ui| {
                    for shell in local_shells() {
                        // Double-click, like every other row in the
                        // sidebar, so a stray click cannot open a pane.
                        let row = icons::icon_row(ui, icons::Icon::Terminal, &shell.label, None, p);
                        if row.double_clicked() {
                            *action = Some(Action::New(SessionKind::Local(shell.value.clone())));
                        }
                        // The executable path matters on Unix, where
                        // several shells can share a name.
                        row.on_hover_text(if shell.value.contains('/') {
                            format!("双击打开 · {}", shell.value)
                        } else {
                            "双击打开".to_string()
                        });
                    }
                    ui.separator();
                    let serial = icons::icon_row(ui, icons::Icon::Host, "连接串口…", None, p);
                    if serial.double_clicked() {
                        *action = Some(Action::SerialPicker);
                    }
                    serial.on_hover_text("双击打开串口选择");
                });
            ui.horizontal(|ui| {
                let (rect, _) = ui.allocate_exact_size(egui::vec2(15.0, 15.0), Sense::hover());
                icons::draw(ui.painter(), rect, icons::Icon::Host, p.muted, 1.2);
                ui.label(hint("连接", p));
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if icons::icon_button(ui, icons::Icon::Group, p, icons::Size::Row)
                        .on_hover_text("分组管理")
                        .clicked()
                    {
                        self.groups_open = true;
                    }
                    if icons::icon_button(ui, icons::Icon::Plus, p, icons::Size::Row)
                        .on_hover_text("新建连接")
                        .clicked()
                    {
                        *action = Some(Action::Remote);
                    }
                });
            });
            let mut groups = self.settings.groups.clone();
            for group in self
                .settings
                .profiles
                .iter()
                .map(|p| &p.group)
                .chain(self.settings.serial_profiles.iter().map(|p| &p.group))
            {
                if !group.is_empty() && !groups.contains(group) {
                    groups.push(group.clone());
                }
            }
            groups.insert(0, String::new());
            let any_profiles =
                !self.settings.profiles.is_empty() || !self.settings.serial_profiles.is_empty();
            for group in groups {
                let count = self
                    .settings
                    .profiles
                    .iter()
                    .filter(|p| p.group == group)
                    .count()
                    + self
                        .settings
                        .serial_profiles
                        .iter()
                        .filter(|p| p.group == group)
                        .count();
                if group.is_empty() && count == 0 && any_profiles {
                    continue;
                }
                egui::CollapsingHeader::new(format!(
                    "{} ({count})",
                    if group.is_empty() {
                        "未分组"
                    } else {
                        &group
                    }
                ))
                .id_salt((&group, "group"))
                .default_open(true)
                .show(ui, |ui| {
                    for (index, profile) in self
                        .settings
                        .profiles
                        .iter()
                        .enumerate()
                        .filter(|(_, p)| p.group == group)
                    {
                        let r = icons::icon_row(ui, icons::Icon::Host, &profile.label(), None, p);
                        if r.double_clicked() {
                            *action = Some(Action::New(SessionKind::Ssh(profile.clone())));
                        }
                        r.on_hover_text(format!(
                            "{}:{} · 双击连接 / 右键管理",
                            profile.destination(),
                            profile.port
                        ))
                        .context_menu(|ui| {
                            for (text, a) in [
                                ("连接 SSH", Action::New(SessionKind::Ssh(profile.clone()))),
                                ("编辑 / 跳板机 / 转发", Action::Edit(index)),
                                ("复制连接", Action::Duplicate(index)),
                                ("删除连接", Action::Remove(index)),
                            ] {
                                if ui.button(text).clicked() {
                                    *action = Some(a);
                                    ui.close();
                                }
                            }
                        });
                    }
                    for (index, profile) in self
                        .settings
                        .serial_profiles
                        .iter()
                        .enumerate()
                        .filter(|(_, p)| p.group == group)
                    {
                        let r =
                            icons::icon_row(ui, icons::Icon::Terminal, &profile.label(), None, p);
                        if r.double_clicked() {
                            *action = Some(Action::New(SessionKind::Serial(profile.clone())));
                        }
                        r.on_hover_text(format!(
                            "串口 {} · {} bps · 双击连接 / 右键管理",
                            if profile.auto() {
                                "auto".to_string()
                            } else {
                                profile.port.clone()
                            },
                            profile.baud
                        ))
                        .context_menu(|ui| {
                            for (text, a) in [
                                (
                                    "连接串口",
                                    Action::New(SessionKind::Serial(profile.clone())),
                                ),
                                ("编辑串口连接", Action::EditSerial(index)),
                                ("复制连接", Action::DuplicateSerial(index)),
                                ("删除连接", Action::RemoveSerial(index)),
                            ] {
                                if ui.button(text).clicked() {
                                    *action = Some(a);
                                    ui.close();
                                }
                            }
                        });
                    }
                });
            }
            // Hosts imported from ~/.ssh/config. They live outside the
            // saved groups because the file, not settings.json, owns
            // them; "保存到连接" copies one over when it is worth keeping.
            if !self.settings.ssh_config_profiles.is_empty() {
                let imported = self.settings.ssh_config_profiles.clone();
                let count = imported.len();
                egui::CollapsingHeader::new(format!("SSH-CONFIG ({count})"))
                    .id_salt("ssh-config")
                    .default_open(true)
                    .show(ui, |ui| {
                        for profile in imported {
                            let r =
                                icons::icon_row(ui, icons::Icon::Host, &profile.label(), None, p);
                            if r.double_clicked() {
                                *action = Some(Action::New(SessionKind::Ssh(profile.clone())));
                            }
                            r.on_hover_text(format!(
                                "{}:{} · 双击连接",
                                profile.destination(),
                                profile.port
                            ))
                            .context_menu(|ui| {
                                if ui.button("连接 SSH").clicked() {
                                    *action = Some(Action::New(SessionKind::Ssh(profile.clone())));
                                    ui.close();
                                }
                                if ui.button("保存到连接").clicked() {
                                    *action = Some(Action::AdoptSshConfig(profile.clone()));
                                    ui.close();
                                }
                            });
                        }
                    });
            }
        });
    }
    /// Auto-hide mode: the bar is out of the layout and drops in from the left
    /// when the pointer reaches a slim strip at the window edge.
    pub(super) fn sidebar_overlay(&mut self, ctx: &egui::Context, action: &mut Option<Action>) {
        let p = self.palette;
        const STRIP_WIDTH: f32 = 9.0;
        const PANEL_WIDTH: f32 = 185.0;
        let avail = ctx.available_rect();
        let pointer = ctx.input(|i| i.pointer.hover_pos());
        let strip = Rect::from_min_size(
            egui::pos2(avail.left(), avail.top()),
            egui::vec2(STRIP_WIDTH, avail.height()),
        );
        let over_strip = pointer.is_some_and(|pos| strip.contains(pos));
        let over_panel = pointer.is_some_and(|pos| {
            self.sidebar_panel_rect
                .is_some_and(|rect| rect.contains(pos))
        });
        // A context menu opened from the bar counts as still hovering it.
        let over_menu = self.sidebar_reveal && egui::Popup::is_any_open(ctx);
        let reveal = over_strip || over_panel || over_menu;
        self.sidebar_reveal = reveal;
        if reveal {
            let rect = Rect::from_min_size(avail.min, egui::vec2(PANEL_WIDTH, avail.height()));
            self.sidebar_panel_rect = Some(rect);
            egui::Area::new(egui::Id::new("navigation-overlay"))
                .order(egui::Order::Foreground)
                .fixed_pos(rect.min)
                .show(ctx, |ui| {
                    ui.set_width(PANEL_WIDTH);
                    ui.set_max_height(avail.height());
                    egui::Frame::new()
                        .fill(p.panel)
                        .stroke(egui::Stroke::new(1.0_f32, p.muted.gamma_multiply(0.4)))
                        .inner_margin(5)
                        .show(ui, |ui| {
                            ui.set_width(PANEL_WIDTH - 10.0);
                            ui.set_max_height(avail.height() - 10.0);
                            self.sidebar_contents(ui, action);
                        });
                });
        } else {
            self.sidebar_panel_rect = None;
            let painter = ctx.layer_painter(egui::LayerId::new(
                egui::Order::Foreground,
                egui::Id::new("navigation-strip"),
            ));
            let center = egui::pos2(avail.left() + 4.0, avail.center().y);
            let tab = Rect::from_center_size(center, egui::vec2(9.0, 42.0));
            painter.rect_filled(tab, 3.0, p.panel.gamma_multiply(0.65));
            icons::draw(
                &painter,
                Rect::from_center_size(center, egui::vec2(9.0, 9.0)),
                icons::Icon::ChevronRight,
                p.muted,
                1.4,
            );
        }
    }
}
