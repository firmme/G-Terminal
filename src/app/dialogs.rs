//! Application dialogs.

use super::*;
use crate::icons::Icon;

impl App {
    pub(super) fn dialogs(&mut self, ctx: &egui::Context, action: &mut Option<Action>) {
        let p = self.palette;
        let mut open = self.settings_open;
        let settings_focus = editing::window_focus(ctx, "settings", open);
        let mut changed = false;
        let auto_hide_before = self.settings.auto_hide_sidebar;
        crate::dialog::Dialog::new("偏好设置", Icon::Settings, p)
            .open(&mut open)
            .collapsible(false)
            .default_width(620.0)
            .show(ctx, |ui| {
                ui.spacing_mut().interact_size.y = 24.0;
                ui.spacing_mut().button_padding.y = 4.0;
                ui.spacing_mut().item_spacing.y = 2.0;
                ui.spacing_mut().slider_width = 110.0;
                ui.columns(2, |columns| {
                    let left = &mut columns[0];
                    crate::dialog::group_title(left, "外观", p);
                    crate::dialog::setting_row(left, Icon::Sun, "主题", None, p, |ui| {
                        let mut index = usize::from(self.settings.light_theme);
                        if crate::dialog::segmented(
                            ui,
                            &[(Icon::Moon, "深色"), (Icon::Sun, "浅色")],
                            &mut index,
                            p,
                        ) {
                            self.settings.light_theme = index == 1;
                            changed = true;
                        }
                    });
                    crate::dialog::setting_row(left, Icon::FileText, "字号", None, p, |ui| {
                        let font_size = ui.add(
                            egui::Slider::new(&mut self.settings.font_size, 10.0..=28.0)
                                .fixed_decimals(0),
                        );
                        settings_focus.request(ui, &font_size);
                        changed |= font_size.changed();
                    });
                    crate::dialog::setting_row(left, Icon::Sidebar, "导航栏", None, p, |ui| {
                        changed |=
                            crate::dialog::switch(ui, &mut self.settings.sidebar, p).changed();
                    });
                    let sidebar = self.settings.sidebar;
                    left.add_enabled_ui(sidebar, |ui| {
                        crate::dialog::setting_row(
                            ui,
                            Icon::ChevronLeft,
                            "自动隐藏",
                            Some("导航栏平时收起，鼠标移到窗口左边缘时展开"),
                            p,
                            |ui| {
                                changed |= crate::dialog::switch(
                                    ui,
                                    &mut self.settings.auto_hide_sidebar,
                                    p,
                                )
                                .changed();
                            },
                        );
                    });
                    crate::dialog::group_title(left, "终端", p);
                    crate::dialog::setting_row(
                        left,
                        Icon::History,
                        "历史行数",
                        Some("每个终端保留的回滚行数"),
                        p,
                        |ui| {
                            changed |= ui
                                .add(
                                    egui::Slider::new(&mut self.settings.scrollback, 100..=50_000)
                                        .logarithmic(true),
                                )
                                .changed();
                        },
                    );
                    crate::dialog::setting_row(
                        left,
                        Icon::Copy,
                        "选中即复制",
                        Some("中键粘贴；按住 Shift 可在程序接管鼠标时强制选择"),
                        p,
                        |ui| {
                            changed |=
                                crate::dialog::switch(ui, &mut self.settings.copy_on_select, p)
                                    .changed();
                        },
                    );
                    crate::dialog::setting_row(
                        left,
                        Icon::Search,
                        "搜索选中文字",
                        Some("终端右键「搜索」使用的搜索引擎"),
                        p,
                        |ui| {
                            egui::ComboBox::from_id_salt("search-engine")
                                .width(110.0)
                                .selected_text(search_engine_label(&self.settings.search_engine))
                                .show_ui(ui, |ui| {
                                    for (key, label, _) in SEARCH_ENGINES {
                                        changed |= ui
                                            .selectable_value(
                                                &mut self.settings.search_engine,
                                                key.to_string(),
                                                label,
                                            )
                                            .changed();
                                    }
                                });
                        },
                    );

                    let right = &mut columns[1];
                    crate::dialog::group_title(right, "连接", p);
                    crate::dialog::setting_row(
                        right,
                        Icon::Terminal,
                        "默认 Shell",
                        None,
                        p,
                        |ui| {
                            egui::ComboBox::from_id_salt("default-shell")
                                .width(130.0)
                                .selected_text(shell_label(&self.settings.default_shell))
                                .show_ui(ui, |ui| {
                                    for shell in local_shells() {
                                        changed |= ui
                                            .selectable_value(
                                                &mut self.settings.default_shell,
                                                shell.value.clone(),
                                                &shell.label,
                                            )
                                            .changed();
                                    }
                                });
                        },
                    );
                    crate::dialog::setting_row(
                        right,
                        Icon::Serial,
                        "串口后台断开",
                        Some("标签在后台或窗口失焦超过设定时间后断开串口；切回时重新计时"),
                        p,
                        |ui| {
                            changed |= crate::dialog::switch(
                                ui,
                                &mut self.settings.serial_background_timeout_enabled,
                                p,
                            )
                            .changed();
                            ui.add_enabled_ui(
                                self.settings.serial_background_timeout_enabled,
                                |ui| {
                                    changed |= ui
                                        .add(
                                            egui::DragValue::new(
                                                &mut self
                                                    .settings
                                                    .serial_background_timeout_minutes,
                                            )
                                            .range(1..=1440)
                                            .suffix(" 分"),
                                        )
                                        .changed();
                                },
                            );
                        },
                    );
                    crate::dialog::group_title(right, "文件", p);
                    crate::dialog::setting_row(
                        right,
                        Icon::Folder,
                        "隐藏点文件",
                        Some("文件窗口默认不显示以 . 开头的文件"),
                        p,
                        |ui| {
                            changed |=
                                crate::dialog::switch(ui, &mut self.settings.hide_dotfiles, p)
                                    .changed();
                        },
                    );
                    crate::dialog::group_title(right, "启动与退出", p);
                    crate::dialog::setting_row(
                        right,
                        Icon::Restart,
                        "恢复上次标签",
                        Some("只恢复标签布局，不会自动连接"),
                        p,
                        |ui| {
                            changed |=
                                crate::dialog::switch(ui, &mut self.settings.restore_tabs, p)
                                    .changed();
                        },
                    );
                    crate::dialog::setting_row(
                        right,
                        Icon::Power,
                        "退出前确认",
                        Some("仍有连接中的会话时，关闭窗口前先确认"),
                        p,
                        |ui| {
                            changed |=
                                crate::dialog::switch(ui, &mut self.settings.confirm_on_exit, p)
                                    .changed();
                        },
                    );
                });
                ui.add_space(8.0);
                ui.separator();
                self.cloud_sync_settings(ui, ctx, p);
                ui.separator();
                ui.horizontal(|ui| {
                    let path = Settings::path().to_string_lossy().into_owned();
                    let (rect, _) = ui.allocate_exact_size(egui::vec2(16.0, 16.0), Sense::hover());
                    icons::draw(ui.painter(), rect, Icon::FileCode, p.muted, 1.3);
                    ui.add(egui::Label::new(hint(&path, p)).truncate());
                    if icons::icon_button(ui, Icon::Copy, p, icons::Size::Row)
                        .on_hover_text("复制配置文件路径")
                        .clicked()
                    {
                        ui.ctx().copy_text(path);
                    }
                });
            });
        self.settings_open = open;
        if self.settings.auto_hide_sidebar != auto_hide_before {
            self.sidebar_pinned = !self.settings.auto_hide_sidebar;
        }
        if changed {
            self.palette = Palette::new(self.settings.light_theme);
            self.palette.apply(ctx, self.settings.light_theme);
            self.settings_dirty_at = Some(std::time::Instant::now());
        }
        let mut open = self.remote_open;
        let ssh_focus = editing::window_focus(
            ctx,
            "connection-config-ssh",
            open && self.profile_kind == ProfileKind::Ssh,
        );
        let serial_focus = editing::window_focus(
            ctx,
            "connection-config-serial",
            open && self.profile_kind == ProfileKind::Serial,
        );
        let focus_host = self.remote.host.is_empty();
        let mut save = false;
        let mut connect = false;
        let mut refresh_ports = false;
        let mut cancel_config = false;
        crate::dialog::Dialog::new("连接配置", Icon::Host, p)
            .open(&mut open)
            .collapsible(false)
            .default_width(480.0)
            .show(ctx, |ui| {
                let full = (ui.available_width() - 132.0).max(120.0);
                let half = 130.0;
                ui.horizontal(|ui| {
                    if crate::dialog::tab(
                        ui,
                        Icon::Host,
                        "SSH",
                        self.profile_kind == ProfileKind::Ssh,
                        p,
                    )
                    .clicked()
                    {
                        self.profile_kind = ProfileKind::Ssh;
                    }
                    if crate::dialog::tab(
                        ui,
                        Icon::Serial,
                        "串口",
                        self.profile_kind == ProfileKind::Serial,
                        p,
                    )
                    .clicked()
                    {
                        self.profile_kind = ProfileKind::Serial;
                    }
                });
                ui.separator();
                match self.profile_kind {
                    ProfileKind::Ssh => {
                        crate::dialog::panel(ui, p, |ui| {
                            egui::Grid::new("connection-form")
                                .spacing([12.0, 6.0])
                                .min_col_width(80.0)
                                .show(ui, |ui| {
                                    ui.label("主机 / IP");
                                    let host =
                                        editing::field_with(ui, &mut self.remote.host, |edit| {
                                            edit.hint_text("example.com 或 192.168.1.10")
                                                .desired_width(full)
                                        });
                                    if focus_host {
                                        ssh_focus.request(ui, &host);
                                    }
                                    ui.end_row();
                                    ui.label("端口");
                                    ui.add_sized(
                                        egui::vec2(half, 28.0),
                                        egui::DragValue::new(&mut self.remote.port)
                                            .range(1..=65535),
                                    );
                                    ui.end_row();
                                    ui.label("连接名称");
                                    editing::field_with(ui, &mut self.remote.name, |edit| {
                                        edit.hint_text("留空时显示主机名").desired_width(full)
                                    });
                                    ui.end_row();
                                    ui.label("分组");
                                    egui::ComboBox::from_id_salt("profile-group")
                                        .width(full + 16.0)
                                        .selected_text(if self.remote.group.is_empty() {
                                            "默认分组"
                                        } else {
                                            &self.remote.group
                                        })
                                        .show_ui(ui, |ui| {
                                            ui.selectable_value(
                                                &mut self.remote.group,
                                                String::new(),
                                                "默认分组",
                                            );
                                            for group in &self.settings.groups {
                                                ui.selectable_value(
                                                    &mut self.remote.group,
                                                    group.clone(),
                                                    group,
                                                );
                                            }
                                        });
                                    ui.end_row();
                                    ui.label("用户名");
                                    let username =
                                        editing::field_with(ui, &mut self.remote.user, |edit| {
                                            edit.hint_text("留空时连接时输入").desired_width(full)
                                        });
                                    if !focus_host {
                                        ssh_focus.request(ui, &username);
                                    }
                                    ui.end_row();
                                    ui.label("私钥路径");
                                    crate::dialog::private_key_field(
                                        ui,
                                        &mut self.remote.identity,
                                        full,
                                        p,
                                    );
                                    ui.end_row();
                                    ui.label("标签颜色");
                                    ui.horizontal(|ui| {
                                        color_picker(ui, &mut self.remote.color, p);
                                    });
                                    ui.end_row();
                                });
                        });
                        crate::dialog::panel(ui, p, |ui| {
                            ui.spacing_mut().item_spacing.y = 4.0;
                            let mut enabled = self.remote.jump.is_some();
                            crate::dialog::subheading(ui, Icon::Host, "跳板机", p, |ui| {
                                if crate::dialog::switch(ui, &mut enabled, p)
                                    .on_hover_text("经由一台跳板机连接（ProxyJump，一级）")
                                    .changed()
                                {
                                    self.remote.jump = if enabled {
                                        Some(Box::new(RemoteProfile::default()))
                                    } else {
                                        None
                                    };
                                }
                            });
                            if let Some(j) = &mut self.remote.jump {
                                egui::Grid::new("jump-form")
                                    .spacing([12.0, 6.0])
                                    .min_col_width(80.0)
                                    .show(ui, |ui| {
                                        ui.label("地址");
                                        ui.horizontal(|ui| {
                                            editing::field_with(ui, &mut j.host, |edit| {
                                                edit.hint_text("主机")
                                                    .desired_width(full - half - 8.0)
                                            });
                                            ui.add_sized(
                                                egui::vec2(half - 50.0, 28.0),
                                                egui::DragValue::new(&mut j.port).range(1..=65535),
                                            )
                                            .on_hover_text("端口");
                                        });
                                        ui.end_row();
                                        ui.label("用户名");
                                        editing::field_with(ui, &mut j.user, |edit| {
                                            edit.desired_width(full)
                                        });
                                        ui.end_row();
                                        ui.label("私钥");
                                        crate::dialog::private_key_field(
                                            ui,
                                            &mut j.identity,
                                            full,
                                            p,
                                        );
                                        ui.end_row();
                                    });
                            }
                            ui.separator();
                            let add = crate::dialog::subheading(
                                ui,
                                Icon::Forward,
                                "本地端口转发",
                                p,
                                |ui| {
                                    icons::icon_button(ui, Icon::Plus, p, icons::Size::Row)
                                        .on_hover_text("添加转发：本机 127.0.0.1:端口 → 远端地址")
                                        .clicked()
                                },
                            );
                            let mut remove = None;
                            for (i, f) in self.remote.forwards.iter_mut().enumerate() {
                                ui.horizontal(|ui| {
                                    ui.add_sized(
                                        egui::vec2(64.0, 26.0),
                                        egui::DragValue::new(&mut f.bind_port).range(1..=65535),
                                    )
                                    .on_hover_text("本机端口（127.0.0.1）");
                                    let (rect, _) = ui.allocate_exact_size(
                                        egui::vec2(16.0, 16.0),
                                        Sense::hover(),
                                    );
                                    icons::draw(
                                        ui.painter(),
                                        rect,
                                        Icon::ChevronRight,
                                        p.muted,
                                        1.4,
                                    );
                                    editing::field_with(ui, &mut f.target_host, |edit| {
                                        edit.hint_text("目标主机").desired_width(half)
                                    });
                                    ui.add_sized(
                                        egui::vec2(64.0, 26.0),
                                        egui::DragValue::new(&mut f.target_port).range(1..=65535),
                                    )
                                    .on_hover_text("目标端口");
                                    if icons::close_button(ui, p, "删除这条转发").clicked() {
                                        remove = Some(i);
                                    }
                                });
                            }
                            if let Some(i) = remove {
                                self.remote.forwards.remove(i);
                            }
                            if add {
                                self.remote.forwards.push(Forward {
                                    bind_port: 8080,
                                    target_host: "127.0.0.1".into(),
                                    target_port: 80,
                                });
                            }
                        });
                        ui.horizontal(|ui| {
                            let (rect, _) =
                                ui.allocate_exact_size(egui::vec2(14.0, 14.0), Sense::hover());
                            icons::draw(ui.painter(), rect, Icon::Lock, p.muted, 1.3);
                            ui.label(hint("密码不保存，连接时输入", p));
                        });
                    }
                    ProfileKind::Serial => {
                        crate::dialog::panel(ui, p, |ui| {
                            egui::Grid::new("serial-form")
                                .spacing([12.0, 6.0])
                                .min_col_width(80.0)
                                .show(ui, |ui| {
                                    ui.label("串口");
                                    let port =
                                        editing::field_with(ui, &mut self.serial.port, |edit| {
                                            edit.desired_width(full).hint_text(format!(
                                                "{} 或 auto",
                                                serial::PORT_EXAMPLE
                                            ))
                                        });
                                    let port = port
                                        .on_hover_text("auto：连接时自动选择一个未占用的普通串口");
                                    serial_focus.request(ui, &port);
                                    ui.end_row();
                                    ui.label("选择端口");
                                    ui.horizontal(|ui| {
                                        egui::ComboBox::from_id_salt("serial-port-pick")
                                            .selected_text("选择端口")
                                            .width(half)
                                            .show_ui(ui, |ui| {
                                                ui.selectable_value(
                                                    &mut self.serial.port,
                                                    "auto".to_string(),
                                                    "auto（自动）",
                                                );
                                                for port in &self.serial_ports {
                                                    let label = if port.bluetooth {
                                                        format!("{}（蓝牙）", port.name)
                                                    } else {
                                                        port.name.clone()
                                                    };
                                                    ui.selectable_value(
                                                        &mut self.serial.port,
                                                        port.name.clone(),
                                                        label,
                                                    );
                                                }
                                                if self.serial_ports.is_empty() {
                                                    ui.label(hint("未检测到串口", p));
                                                }
                                            });
                                        if icons::icon_button(
                                            ui,
                                            Icon::Refresh,
                                            p,
                                            icons::Size::Row,
                                        )
                                        .on_hover_text("重新检测串口")
                                        .clicked()
                                        {
                                            refresh_ports = true;
                                        }
                                    });
                                    ui.end_row();
                                    ui.label("波特率");
                                    egui::ComboBox::from_id_salt("serial-baud")
                                        .width(half)
                                        .selected_text(self.serial.baud.to_string())
                                        .show_ui(ui, |ui| {
                                            for baud in BAUD_RATES {
                                                ui.selectable_value(
                                                    &mut self.serial.baud,
                                                    baud,
                                                    baud.to_string(),
                                                );
                                            }
                                        });
                                    ui.end_row();
                                    ui.label("连接名称");
                                    editing::field_with(ui, &mut self.serial.name, |edit| {
                                        edit.desired_width(full)
                                    });
                                    ui.end_row();
                                    ui.label("分组");
                                    egui::ComboBox::from_id_salt("serial-group")
                                        .width(half)
                                        .selected_text(if self.serial.group.is_empty() {
                                            "默认分组"
                                        } else {
                                            &self.serial.group
                                        })
                                        .show_ui(ui, |ui| {
                                            ui.selectable_value(
                                                &mut self.serial.group,
                                                String::new(),
                                                "默认分组",
                                            );
                                            for group in &self.settings.groups {
                                                ui.selectable_value(
                                                    &mut self.serial.group,
                                                    group.clone(),
                                                    group,
                                                );
                                            }
                                        });
                                    ui.end_row();
                                    ui.label("标签颜色");
                                    ui.horizontal(|ui| {
                                        color_picker(ui, &mut self.serial.color, p);
                                    });
                                    ui.end_row();
                                });
                        });
                    }
                }
                crate::dialog::footer(ui, |ui| {
                    if crate::dialog::secondary(ui, "取消").clicked() {
                        cancel_config = true;
                    }
                    if crate::dialog::primary(ui, "保存并连接", p)
                        .on_hover_text(accel("Ctrl+Enter"))
                        .clicked()
                    {
                        save = true;
                        connect = true;
                    }
                    ui.add_space((ui.available_width() - 80.0).max(0.0));
                    if crate::dialog::secondary(ui, "保存")
                        .on_hover_text("Enter")
                        .clicked()
                    {
                        save = true;
                    }
                });
                if !cancel_config
                    && !egui::Popup::is_any_open(ui.ctx())
                    && !editing::ime_composing(ui.ctx())
                    && ui.input(|i| i.key_pressed(Key::Enter))
                {
                    save = true;
                    connect = ui.input(|i| i.modifiers.command);
                }
            });
        self.remote_open = open && !cancel_config;
        if refresh_ports {
            self.serial_ports = serial::available_ports();
        }
        if save {
            let saved = match self.profile_kind {
                ProfileKind::Ssh => self.remote.validate().map(|()| {
                    if let Some(i) = self.editing_profile {
                        self.settings.profiles[i] = self.remote.clone();
                    } else {
                        self.settings.profiles.push(self.remote.clone());
                    }
                }),
                ProfileKind::Serial => self.serial.validate().map(|()| {
                    if let Some(i) = self.editing_serial {
                        self.settings.serial_profiles[i] = self.serial.clone();
                    } else {
                        self.settings.serial_profiles.push(self.serial.clone());
                    }
                }),
            };
            match saved {
                Ok(()) => {
                    self.remote_open = false;
                    self.persist();
                    if connect {
                        *action = Some(match self.profile_kind {
                            ProfileKind::Ssh => Action::New(SessionKind::Ssh(self.remote.clone())),
                            ProfileKind::Serial => {
                                Action::New(SessionKind::Serial(self.serial.clone()))
                            }
                        });
                    }
                }
                Err(e) => self.error = Some(e.to_string()),
            }
        }
        let mut open = self.groups_open;
        let mut modified = false;
        let group_focus = editing::window_focus(ctx, "connection-groups", open);
        crate::dialog::Dialog::new("连接分组", Icon::Folder, p)
            .open(&mut open)
            .default_width(540.0)
            .resizable(false)
            .show(ctx, |ui| {
                let mut add_group = false;
                ui.horizontal(|ui| {
                    ui.allocate_ui(egui::vec2(ui.available_width() - 42.0, 32.0), |ui| {
                        let group = crate::dialog::icon_field(
                            ui,
                            Icon::NewFolder,
                            "新分组名称",
                            &mut self.new_group,
                            false,
                            p,
                        );
                        group_focus.request(ui, &group);
                    });
                    if crate::dialog::primary_icon(ui, Icon::Plus, p)
                        .on_hover_text("添加分组 (Enter)")
                        .clicked()
                    {
                        add_group = true;
                    }
                });
                ui.add_space(4.0);
                if !egui::Popup::is_any_open(ui.ctx())
                    && !editing::ime_composing(ui.ctx())
                    && ui.input(|i| i.key_pressed(Key::Enter))
                {
                    add_group = true;
                }
                if add_group {
                    let name = self.new_group.trim().to_string();
                    if !name.is_empty() && !self.settings.groups.contains(&name) {
                        self.settings.groups.push(name);
                        modified = true;
                        self.new_group.clear();
                    }
                }
                let mut colors: Vec<String> = self
                    .settings
                    .groups
                    .iter()
                    .map(|group| {
                        self.settings
                            .group_colors
                            .get(group)
                            .cloned()
                            .unwrap_or_default()
                    })
                    .collect();
                let mut remove = None;
                let mut rename = None;
                let mut recolor = false;
                for (i, group) in self.settings.groups.iter_mut().enumerate() {
                    let old = group.clone();
                    ui.push_id(i, |ui| {
                        ui.horizontal(|ui| {
                            ui.set_min_height(34.0);
                            let (rect, _) =
                                ui.allocate_exact_size(egui::vec2(18.0, 18.0), Sense::hover());
                            icons::draw(
                                ui.painter(),
                                rect,
                                Icon::Folder,
                                parse_tag_color(&colors[i]).unwrap_or(p.muted),
                                1.4,
                            );
                            if editing::field_with(ui, group, |edit| edit.desired_width(150.0))
                                .changed()
                            {
                                rename = Some((old.clone(), group.clone()));
                            }
                            if color_picker(ui, &mut colors[i], p) {
                                recolor = true;
                            }
                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                if icons::icon_button(ui, Icon::Trash, p, icons::Size::Row)
                                    .on_hover_text("删除分组（其中的连接移到默认分组）")
                                    .clicked()
                                {
                                    remove = Some(i);
                                }
                            });
                        });
                    });
                }
                if recolor {
                    for (group, color) in self.settings.groups.iter().zip(colors.iter()) {
                        if color.is_empty() {
                            self.settings.group_colors.remove(group);
                        } else {
                            self.settings
                                .group_colors
                                .insert(group.clone(), color.clone());
                        }
                    }
                    modified = true;
                }
                if let Some((old, new)) = rename {
                    for p in &mut self.settings.profiles {
                        if p.group == old {
                            p.group = new.clone();
                        }
                    }
                    for p in &mut self.settings.serial_profiles {
                        if p.group == old {
                            p.group = new.clone();
                        }
                    }
                    if let Some(color) = self.settings.group_colors.remove(&old) {
                        self.settings.group_colors.insert(new.clone(), color);
                    }
                    modified = true;
                }
                if let Some(i) = remove {
                    let group = self.settings.groups.remove(i);
                    self.settings.group_colors.remove(&group);
                    for p in &mut self.settings.profiles {
                        if p.group == group {
                            p.group.clear();
                        }
                    }
                    for p in &mut self.settings.serial_profiles {
                        if p.group == group {
                            p.group.clear();
                        }
                    }
                    modified = true;
                }
                if self.settings.groups.is_empty() {
                    ui.label(hint("还没有分组", p));
                }
                ui.add_space(4.0);
                ui.label(hint("连接自身的颜色优先于分组颜色", p));
            });
        self.groups_open = open;
        if modified {
            self.persist();
        }
        if let Some(mut toolbox) = self.toolbox.take() {
            let target = self.focused_ssh().map(|(_, target)| target);
            let mut open = true;
            let script = match &target {
                Some(target) => toolbox.show(ctx, &mut open, p, target),
                None => {
                    self.error = Some("会话已断开，服务器工具箱不可用".into());
                    None
                }
            };
            if let Some(script) = script {
                open = false;
                let count = script.chars().count();
                let sent = self
                    .tabs
                    .get_mut(self.active)
                    .and_then(|t| t.panes.get_mut(t.focused))
                    .map(|pane| pane.session.write(script.into_bytes()));
                match sent {
                    Some(Ok(())) => self.notify(format!("已发送服务器初始化脚本（{count} 字符）")),
                    Some(Err(e)) => self.error = Some(format!("{e:#}")),
                    None => {}
                }
            }
            if open && target.is_some() {
                self.toolbox = Some(toolbox);
            }
        }
        let picker_focus =
            editing::window_focus(ctx, "serial-picker", self.serial_picker.is_some());
        if self.serial_picker.is_some() {
            let mut open = true;
            let mut connect = false;
            let mut inspect = false;
            let mut cancel = false;
            let mut refresh = false;
            if let Some(picker) = &mut self.serial_picker {
                crate::dialog::Dialog::new("连接串口", Icon::Serial, p)
                    .open(&mut open)
                    .collapsible(false)
                    .resizable(false)
                    .default_width(400.0)
                    .show(ctx, |ui| {
                        let port = crate::dialog::icon_field(
                            ui,
                            Icon::Serial,
                            &format!("端口，如 {} 或 auto", serial::PORT_EXAMPLE),
                            &mut picker.port,
                            false,
                            p,
                        );
                        picker_focus.request(ui, &port);
                        ui.add_space(4.0);
                        crate::dialog::panel(ui, p, |ui| {
                            ui.spacing_mut().item_spacing.y = 2.0;
                            let mut rows =
                                vec![("auto".to_string(), "auto · 自动选择".to_string())];
                            rows.extend(picker.ports.iter().map(|port| {
                                (
                                    port.name.clone(),
                                    if port.bluetooth {
                                        format!("{} · 蓝牙", port.name)
                                    } else {
                                        port.name.clone()
                                    },
                                )
                            }));
                            egui::ScrollArea::vertical()
                                .max_height(200.0)
                                .show(ui, |ui| {
                                    for (name, label) in rows {
                                        let row = ui
                                            .push_id(&name, |ui| {
                                                icons::navigation_row(
                                                    ui,
                                                    Icon::Serial,
                                                    &label,
                                                    picker.port.eq_ignore_ascii_case(&name),
                                                    None,
                                                    p,
                                                )
                                            })
                                            .inner;
                                        if row.clicked() {
                                            picker.port = name.clone();
                                        }
                                        if row.double_clicked() {
                                            picker.port = name;
                                            connect = true;
                                        }
                                    }
                                });
                            if picker.ports.is_empty() {
                                ui.label(hint("未检测到串口设备", p));
                            }
                        });
                        ui.add_space(4.0);
                        crate::dialog::setting_row(ui, Icon::Forward, "波特率", None, p, |ui| {
                            egui::ComboBox::from_id_salt("picker-baud")
                                .width(120.0)
                                .selected_text(picker.baud.to_string())
                                .show_ui(ui, |ui| {
                                    for baud in BAUD_RATES {
                                        ui.selectable_value(
                                            &mut picker.baud,
                                            baud,
                                            baud.to_string(),
                                        );
                                    }
                                });
                        });
                        crate::dialog::footer(ui, |ui| {
                            if crate::dialog::secondary(ui, "取消").clicked() {
                                cancel = true;
                            }
                            if crate::dialog::primary(ui, "连接", p).clicked() {
                                connect = true;
                            }
                            ui.add_space((ui.available_width() - 60.0).max(0.0));
                            if icons::icon_button(ui, Icon::Search, p, icons::Size::Button)
                                .on_hover_text("占用排查：查看哪个程序占着这个端口")
                                .clicked()
                            {
                                inspect = true;
                            }
                            if icons::icon_button(ui, Icon::Refresh, p, icons::Size::Button)
                                .on_hover_text("重新检测串口")
                                .clicked()
                            {
                                refresh = true;
                            }
                        });
                        if !cancel
                            && !egui::Popup::is_any_open(ui.ctx())
                            && !editing::ime_composing(ui.ctx())
                            && ui.input(|i| i.key_pressed(Key::Enter))
                        {
                            connect = true;
                        }
                    });
                if refresh {
                    picker.refresh();
                }
            }
            if cancel {
                open = false;
            }
            if inspect && let Some(picker) = &self.serial_picker {
                *action = Some(Action::FindPortOwner(picker.port.clone()));
            }
            if connect && let Some(picker) = &self.serial_picker {
                *action = Some(Action::New(SessionKind::Serial(SerialProfile {
                    sync_id: String::new(),
                    name: picker.port.clone(),
                    port: picker.port.clone(),
                    baud: picker.baud,
                    group: String::new(),
                    color: String::new(),
                })));
                open = false;
            }
            if !open {
                self.serial_picker = None;
            }
        }
        crate::dialog::Dialog::new("快捷键", Icon::Keyboard, p)
            .open(&mut self.help_open)
            .default_width(620.0)
            .default_height(520.0)
            .show(ctx, |ui| {
                egui::ScrollArea::vertical().show(ui, |ui| {
                    let copy_paste = if ACCEL == "Cmd" {
                        "Cmd+C / Cmd+V".to_string()
                    } else {
                        accel("Ctrl+Shift+C / Ctrl+Shift+V")
                    };
                    let rows: [(Icon, &str, String); 12] = [
                        (Icon::Plus, "新建标签", accel("Ctrl+Shift+T")),
                        (Icon::ClosePane, "关闭窗格", accel("Ctrl+Shift+W")),
                        (Icon::SplitHorizontal, "左右分屏", accel("Ctrl+Shift+D")),
                        (Icon::SplitVertical, "上下分屏", accel("Ctrl+Shift+E")),
                        (Icon::ChevronRight, "下一个标签", "Ctrl+Tab".to_string()),
                        (Icon::ChevronRight, "下一个窗格", alt_accel("Right")),
                        (Icon::Close, "中断程序", "Ctrl+C".to_string()),
                        (Icon::Disconnect, "断开", alt_accel("C")),
                        (Icon::Reconnect, "重新连接", alt_accel("R")),
                        (Icon::Search, "查找输出", accel("Ctrl+Shift+F")),
                        (Icon::Sidebar, "导航栏", accel("Ctrl+Shift+B")),
                        (Icon::Settings, "偏好设置", accel("Ctrl+,")),
                    ];
                    ui.columns(2, |columns| {
                        for (index, (icon, description, keys)) in rows.iter().enumerate() {
                            key_row(&mut columns[index % 2], Some(*icon), description, keys, p);
                        }
                    });
                    // The long combinations get the full width.
                    key_row(ui, Some(Icon::Copy), "复制 / 粘贴", &copy_paste, p);
                    key_row(
                        ui,
                        Some(Icon::FileText),
                        "字号 放大 / 缩小 / 还原",
                        &accel("Ctrl+Plus / Ctrl+Minus / Ctrl+0"),
                        p,
                    );
                    ui.columns(2, |columns| {
                        key_row(
                            &mut columns[0],
                            Some(Icon::Copy),
                            "中键粘贴",
                            "MiddleClick",
                            p,
                        );
                        key_row(
                            &mut columns[1],
                            Some(Icon::Terminal),
                            "强制选择",
                            "Shift+Drag",
                            p,
                        );
                    });
                    if let Some(tab) = self.tabs.get(self.active) {
                        let session = &tab.panes[tab.focused].session;
                        let mut shell = shortcut_shell(&session.kind);
                        ui.add_space(6.0);
                        ui.separator();
                        ui.horizontal(|ui| {
                            let (rect, _) =
                                ui.allocate_exact_size(egui::vec2(18.0, 18.0), Sense::hover());
                            icons::draw(
                                ui.painter(),
                                rect,
                                sidebar::kind_icon(&session.kind),
                                p.accent,
                                1.4,
                            );
                            ui.label(RichText::new(session.kind.label()).strong().color(p.text));
                            if matches!(shell, ShortcutShell::Unknown | ShortcutShell::Ssh) {
                                let id = egui::Id::new(("help-shell", tab.panes[tab.focused].id));
                                let mut selected = ui
                                    .ctx()
                                    .data_mut(|data| data.get_temp::<ShortcutShell>(id))
                                    .unwrap_or(shell);
                                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                    egui::ComboBox::from_id_salt(id)
                                        .selected_text(selected.label())
                                        .show_ui(ui, |ui| {
                                            if shell == ShortcutShell::Ssh {
                                                ui.selectable_value(
                                                    &mut selected,
                                                    ShortcutShell::Ssh,
                                                    ShortcutShell::Ssh.label(),
                                                );
                                            }
                                            for candidate in [
                                                ShortcutShell::Unix,
                                                ShortcutShell::PowerShell,
                                                ShortcutShell::Cmd,
                                            ] {
                                                ui.selectable_value(
                                                    &mut selected,
                                                    candidate,
                                                    candidate.label(),
                                                );
                                            }
                                        })
                                        .response
                                        .on_hover_text("远端或当前 Shell");
                                });
                                ui.ctx().data_mut(|data| data.insert_temp(id, selected));
                                shell = selected;
                            }
                        });
                        if shell != ShortcutShell::Unknown {
                            show_shell_shortcuts(ui, shell, p);
                        }
                    }
                    ui.add_space(8.0);
                    ui.separator();
                    ui.horizontal(|ui| {
                        ui.label(hint(concat!("G-Terminal ", env!("CARGO_PKG_VERSION")), p));
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            ui.hyperlink_to("GitHub", "https://github.com/firmme/G-Terminal");
                        });
                    });
                });
            });
        if let Some(login) = &mut self.login {
            let mut open = true;
            let ready = login.show(ctx, &mut open, p);
            if let Some(c) = ready {
                self.login = None;
                self.add_connection(c, ctx);
            } else if !open {
                self.login = None;
                if let Some((tab_id, pane_id)) = self.login_target.take() {
                    self.note_error(tab_id, pane_id, "connect cancelled");
                    if let Some((tab, index)) = self.locate(tab_id, pane_id) {
                        let kind = self.tabs[tab].panes[index].session.kind.clone();
                        let terminal = self.tabs[tab].panes[index].session.terminal.clone();
                        self.tabs[tab].panes[index] = Pane::new(
                            pane_id,
                            Session::disconnected_reusing(
                                kind,
                                self.settings.scrollback,
                                Some(terminal),
                            ),
                        );
                    }
                }
            }
        }
        if let Some(files) = &mut self.files {
            files.show(ctx, &mut self.files_open, p, self.settings.hide_dotfiles);
            if files.busy() {
                ctx.request_repaint_after(std::time::Duration::from_millis(150));
            }
        }
        self.update_window(ctx, p);
        if let Some(window) = &mut self.port_owner {
            let outcome = window.show(ctx, p);
            if outcome.reconnect {
                *action = Some(Action::Restart);
            }
            if !outcome.keep_open || outcome.reconnect {
                self.port_owner = None;
            }
        }
    }

    pub(super) fn exit_confirm(&mut self, ctx: &egui::Context) {
        let focus = editing::window_focus(ctx, "exit-confirm", self.confirm_exit);
        if self.confirm_exit {
            let mut open = true;
            let mut keep_tabs_changed = false;
            crate::dialog::Dialog::new("确认退出", Icon::Warning, self.palette)
                .open(&mut open)
                .default_width(360.0)
                .collapsible(false)
                .resizable(false)
                .order(egui::Order::Foreground)
                .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
                .show(ctx, |ui| {
                    ui.label("仍有未断开的连接，确定要退出吗？");
                    ui.add_space(8.0);
                    keep_tabs_changed = ui
                        .checkbox(&mut self.settings.restore_tabs, "保留当前选项卡")
                        .on_hover_text("下次启动恢复选项卡和分屏，连接保持断开")
                        .changed();
                    ui.add_space(12.0);
                    crate::dialog::footer(ui, |ui| {
                        if crate::dialog::primary(ui, "退出", self.palette).clicked() {
                            self.exit_confirmed = true;
                            self.confirm_exit = false;
                            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                        }
                        let cancel = crate::dialog::secondary(ui, "取消");
                        focus.request(ui, &cancel);
                        if cancel.clicked() {
                            self.confirm_exit = false;
                        }
                    });
                });
            if keep_tabs_changed {
                self.persist();
            }
            if !open {
                self.confirm_exit = false;
            }
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ShortcutShell {
    Unknown,
    Ssh,
    Unix,
    PowerShell,
    Cmd,
    Serial,
}

impl ShortcutShell {
    fn label(self) -> &'static str {
        match self {
            Self::Unknown => "选择 Shell",
            Self::Ssh => "SSH（常见 Unix Shell）",
            Self::Unix => "Bash / Zsh / Ksh",
            Self::PowerShell => "PowerShell",
            Self::Cmd => "Windows CMD",
            Self::Serial => "串口",
        }
    }
}

fn shortcut_shell(kind: &SessionKind) -> ShortcutShell {
    match kind {
        SessionKind::Serial(_) => ShortcutShell::Serial,
        SessionKind::Ssh(_) => ShortcutShell::Ssh,
        SessionKind::Sftp(_) => ShortcutShell::Unknown,
        SessionKind::Local(value) => {
            let name = if value == "shell" || value.is_empty() {
                std::env::var("SHELL").unwrap_or_default()
            } else {
                value.clone()
            };
            let name = name
                .rsplit(['/', '\\'])
                .next()
                .unwrap_or(&name)
                .to_ascii_lowercase();
            match name.trim_end_matches(".exe") {
                "powershell" | "pwsh" => ShortcutShell::PowerShell,
                "cmd" => ShortcutShell::Cmd,
                "bash" | "zsh" | "ksh" => ShortcutShell::Unix,
                _ => ShortcutShell::Unknown,
            }
        }
    }
}

/// A shortcut: what it does on the left, its keys drawn as keycaps on the
/// right. `keys` is written `Ctrl+Shift+T`, alternatives separated by ` / `.
fn key_row(ui: &mut egui::Ui, icon: Option<Icon>, description: &str, keys: &str, p: Palette) {
    let width = ui.available_width();
    ui.allocate_ui_with_layout(
        egui::vec2(width, 28.0),
        Layout::left_to_right(Align::Center),
        |ui| {
            ui.set_min_size(egui::vec2(width, 28.0));
            if let Some(icon) = icon {
                let (rect, _) = ui.allocate_exact_size(egui::vec2(16.0, 16.0), Sense::hover());
                icons::draw(ui.painter(), rect, icon, p.muted, 1.3);
            }
            ui.label(RichText::new(description).color(p.text));
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.spacing_mut().item_spacing.x = 3.0;
                // Laid out right to left, so the pieces go in reverse.
                let alternatives: Vec<&str> = keys.split(" / ").collect();
                for (index, alternative) in alternatives.iter().rev().enumerate() {
                    if index > 0 {
                        ui.label(RichText::new("/").size(12.0).color(p.muted));
                    }
                    for key in alternative.split('+').rev() {
                        keycap(ui, key, p);
                    }
                }
            });
        },
    );
}

/// One key, drawn as a small raised cap.
fn keycap(ui: &mut egui::Ui, key: &str, p: Palette) {
    let label = match key {
        "MiddleClick" => "中键",
        "Drag" => "拖动",
        "Plus" => "+",
        "Minus" => "−",
        "Right" => "→",
        "Left" => "←",
        "Up" => "↑",
        "Down" => "↓",
        other => other,
    };
    let galley =
        ui.painter()
            .layout_no_wrap(label.to_owned(), egui::FontId::monospace(11.5), p.text);
    let size = egui::vec2((galley.size().x + 10.0).max(20.0), 20.0);
    let (rect, _) = ui.allocate_exact_size(size, Sense::hover());
    ui.painter()
        .rect_filled(rect.translate(egui::vec2(0.0, 1.5)), 4, p.line);
    ui.painter().rect_filled(rect, 4, p.raised);
    ui.painter().rect_stroke(
        rect,
        4,
        egui::Stroke::new(1.0_f32, p.line),
        egui::StrokeKind::Inside,
    );
    ui.painter()
        .galley(rect.center() - galley.size() * 0.5, galley, p.text);
}

fn shortcut_rows(ui: &mut egui::Ui, rows: &[(&str, &str)], p: Palette) {
    ui.columns(2, |columns| {
        for (index, &(keys, description)) in rows.iter().enumerate() {
            key_row(&mut columns[index % 2], None, description, keys, p);
        }
    });
}

fn shortcut_group(ui: &mut egui::Ui, title: &str, rows: &[(&str, &str)], p: Palette) {
    crate::dialog::group_title(ui, title, p);
    shortcut_rows(ui, rows, p);
}

fn show_shell_shortcuts(ui: &mut egui::Ui, shell: ShortcutShell, p: Palette) {
    match shell {
        ShortcutShell::Unknown => {}
        ShortcutShell::Ssh | ShortcutShell::Unix => {
            ui.label(hint(
                "Emacs / Readline 键位；Vi 模式或自定义键位可能不同。",
                p,
            ));
            shortcut_group(
                ui,
                "光标移动",
                &[
                    ("Ctrl+A / Ctrl+E", "行首 / 行尾"),
                    ("Alt+B / Alt+F", "向左 / 向右跳一个词"),
                ],
                p,
            );
            shortcut_group(
                ui,
                "文本删除",
                &[
                    ("Ctrl+W", "向左删一个词"),
                    ("Alt+D", "向右删一个词"),
                    ("Ctrl+U / Ctrl+K", "删到行首 / 行尾"),
                    ("Ctrl+Y", "粘贴刚才删除的内容"),
                ],
                p,
            );
            shortcut_group(
                ui,
                "控制与辅助",
                &[
                    ("Ctrl+L", "清屏"),
                    ("Ctrl+R", "搜索历史命令"),
                    ("Ctrl+C", "中断进程 / 放弃当前行"),
                ],
                p,
            );
        }
        ShortcutShell::PowerShell => {
            shortcut_group(
                ui,
                "光标移动",
                &[
                    ("Home / Ctrl+A", "跳到行首"),
                    ("End / Ctrl+E", "跳到行尾"),
                    ("Ctrl+Left / Ctrl+Right", "向左 / 向右跳一个词"),
                ],
                p,
            );
            shortcut_group(
                ui,
                "文本编辑",
                &[
                    ("Ctrl+Backspace", "向左删一个词"),
                    ("Ctrl+Delete", "向右删一个词"),
                    ("Esc", "清空整行"),
                    ("Ctrl+Home / Ctrl+End", "删到行首 / 行尾"),
                    ("Shift+Enter", "换行输入"),
                ],
                p,
            );
            shortcut_group(
                ui,
                "历史与补全",
                &[
                    ("Tab", "自动补全"),
                    ("Ctrl+R", "搜索历史命令"),
                    ("Right", "接受灰色预测建议"),
                    ("F8", "匹配上一条历史命令"),
                    ("Ctrl+L", "清屏并保留当前行"),
                    ("Ctrl+C", "中断运行"),
                ],
                p,
            );
        }
        ShortcutShell::Cmd => {
            shortcut_group(
                ui,
                "光标移动与编辑",
                &[
                    ("Home / End", "跳到行首 / 行尾"),
                    ("Ctrl+Left / Ctrl+Right", "向左 / 向右跳一个词"),
                    ("Esc", "清空整行"),
                ],
                p,
            );
            shortcut_group(
                ui,
                "历史命令",
                &[
                    ("Up / Down", "翻看历史命令"),
                    ("F1 / F3", "逐字 / 完整复制上一条命令"),
                    ("F7", "打开历史命令窗口"),
                    ("F8", "根据输入前缀搜索历史"),
                    ("F9", "按编号调用历史命令"),
                    ("Ctrl+C", "中断运行"),
                ],
                p,
            );
        }
        ShortcutShell::Serial => {
            ui.label(hint(
                "编辑行为由串口设备决定，以下为 G-Terminal 自身操作。",
                p,
            ));
            let copy_paste = if ACCEL == "Cmd" {
                "Cmd+C / Cmd+V".to_string()
            } else {
                "Ctrl+Shift+C / Ctrl+Shift+V".to_string()
            };
            let connection = format!("{} / {}", alt_accel("C"), alt_accel("R"));
            shortcut_group(
                ui,
                "G-Terminal 操作",
                &[
                    ("Enter", "发送回车"),
                    (&copy_paste, "复制 / 粘贴终端文本"),
                    ("Shift+PageUp / Shift+PageDown", "滚动终端历史"),
                    (&connection, "断开 / 重连串口"),
                ],
                p,
            );
        }
    }
}

#[cfg(test)]
mod shortcut_tests {
    use super::*;

    #[test]
    fn local_shell_help_follows_the_session_command() {
        assert_eq!(
            shortcut_shell(&SessionKind::Local("pwsh".into())),
            ShortcutShell::PowerShell
        );
        assert_eq!(
            shortcut_shell(&SessionKind::Local("cmd".into())),
            ShortcutShell::Cmd
        );
        assert_eq!(
            shortcut_shell(&SessionKind::Local("/bin/zsh".into())),
            ShortcutShell::Unix
        );
        assert_eq!(
            shortcut_shell(&SessionKind::Local("wsl".into())),
            ShortcutShell::Unknown
        );
    }

    #[test]
    fn ssh_help_has_a_useful_default() {
        assert_eq!(
            shortcut_shell(&SessionKind::Ssh(RemoteProfile::default())),
            ShortcutShell::Ssh
        );
    }
}
