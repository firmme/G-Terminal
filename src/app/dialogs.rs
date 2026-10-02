//! Application dialogs.

use super::*;

#[derive(Clone, Copy, PartialEq, Eq)]
enum SettingsSection {
    Appearance,
    Terminal,
    Connection,
    Files,
    Startup,
}

impl SettingsSection {
    const ALL: [Self; 5] = [
        Self::Appearance,
        Self::Terminal,
        Self::Files,
        Self::Connection,
        Self::Startup,
    ];
}

impl App {
    pub(super) fn dialogs(&mut self, ctx: &egui::Context, action: &mut Option<Action>) {
        let p = self.palette;
        let mut open = self.settings_open;
        let mut changed = false;
        let auto_hide_before = self.settings.auto_hide_sidebar;
        egui::Window::new("偏好设置")
            .open(&mut open)
            .collapsible(false)
            .default_width(680.0)
            .show(ctx, |ui| {
                ui.columns(2, |columns| {
                    for (index, section) in SettingsSection::ALL.into_iter().enumerate() {
                        let column = &mut columns[index % 2];
                        column.group(|ui| {
                            ui.set_min_width(290.0);
                            match section {
                                SettingsSection::Appearance => {
                                    ui.label(RichText::new("外观与导航").strong().color(p.accent));
                                    changed |= ui
                                        .add(
                                            egui::Slider::new(
                                                &mut self.settings.font_size,
                                                10.0..=28.0,
                                            )
                                            .text("字号"),
                                        )
                                        .changed();
                                    ui.horizontal(|ui| {
                                        changed |= ui
                                            .checkbox(&mut self.settings.light_theme, "浅色主题")
                                            .changed();
                                        changed |= ui
                                            .checkbox(&mut self.settings.sidebar, "导航栏")
                                            .changed();
                                    });
                                    changed |= ui
                                        .checkbox(
                                            &mut self.settings.auto_hide_sidebar,
                                            "自动隐藏导航栏",
                                        )
                                        .on_hover_text("鼠标移到左侧边缘时展开")
                                        .changed();
                                }
                                SettingsSection::Terminal => {
                                    ui.label(RichText::new("终端").strong().color(p.accent));
                                    changed |= ui
                                        .add(
                                            egui::Slider::new(
                                                &mut self.settings.scrollback,
                                                100..=50_000,
                                            )
                                            .logarithmic(true)
                                            .text("历史行数（新会话）"),
                                        )
                                        .changed();
                                    changed |= ui
                                        .checkbox(
                                            &mut self.settings.copy_on_select,
                                            "选中文本后自动复制",
                                        )
                                        .changed();
                                    ui.label(hint("中键粘贴 · Shift+鼠标强制选择", p));
                                }
                                SettingsSection::Connection => {
                                    ui.label(RichText::new("连接").strong().color(p.accent));
                                    ui.horizontal(|ui| {
                                        ui.label("默认 Shell");
                                        egui::ComboBox::from_id_salt("default-shell")
                                            .selected_text(shell_label(
                                                &self.settings.default_shell,
                                            ))
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
                                    });
                                    changed |= ui
                                        .checkbox(
                                            &mut self.settings.serial_background_timeout_enabled,
                                            "串口在后台超时后自动断开",
                                        )
                                        .changed();
                                    ui.add_enabled_ui(
                                        self.settings.serial_background_timeout_enabled,
                                        |ui| {
                                            ui.horizontal(|ui| {
                                                ui.label("后台超时");
                                                changed |= ui
                                                    .add(
                                                        egui::DragValue::new(
                                                            &mut self
                                                                .settings
                                                                .serial_background_timeout_minutes,
                                                        )
                                                        .range(1..=1440)
                                                        .suffix(" 分钟"),
                                                    )
                                                    .changed();
                                            });
                                        },
                                    );
                                    ui.label(hint("切到后台后开始计时，返回时重置", p));
                                }
                                SettingsSection::Files => {
                                    ui.label(RichText::new("文件与搜索").strong().color(p.accent));
                                    ui.horizontal(|ui| {
                                        ui.label("浏览器搜索");
                                        egui::ComboBox::from_id_salt("search-engine")
                                            .selected_text(search_engine_label(
                                                &self.settings.search_engine,
                                            ))
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
                                    });
                                    changed |= ui
                                        .checkbox(
                                            &mut self.settings.hide_dotfiles,
                                            "文件窗口默认隐藏 . 开头文件",
                                        )
                                        .changed();
                                }
                                SettingsSection::Startup => {
                                    ui.label(RichText::new("启动行为").strong().color(p.accent));
                                    changed |= ui
                                        .checkbox(
                                            &mut self.settings.restore_tabs,
                                            "恢复上次标签（不自动重连）",
                                        )
                                        .changed();
                                    changed |= ui
                                        .checkbox(
                                            &mut self.settings.confirm_on_exit,
                                            "退出时确认活动会话",
                                        )
                                        .changed();
                                }
                            }
                        });
                        column.add_space(8.0);
                    }
                });
                egui::CollapsingHeader::new("配置文件位置")
                    .default_open(false)
                    .show(ui, |ui| {
                        ui.label(hint(Settings::path().to_string_lossy().as_ref(), p));
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
        let mut save = false;
        let mut connect = false;
        let mut refresh_ports = false;
        egui::Window::new("连接配置")
            .open(&mut open)
            .collapsible(false)
            .default_width(420.0)
            .show(ctx, |ui| {
                let full = 260.0;
                let half = 130.0;
                ui.horizontal(|ui| {
                    ui.label("类型");
                    ui.selectable_value(&mut self.profile_kind, ProfileKind::Ssh, "SSH");
                    ui.selectable_value(&mut self.profile_kind, ProfileKind::Serial, "串口");
                });
                ui.separator();
                match self.profile_kind {
                    ProfileKind::Ssh => {
                        ui.group(|ui| {
                            ui.label(RichText::new("连接信息").strong().color(p.accent));
                            egui::Grid::new("connection-form")
                                .spacing([12.0, 6.0])
                                .min_col_width(72.0)
                                .show(ui, |ui| {
                                    ui.label("主机 / IP");
                                    editing::field_with(ui, &mut self.remote.host, |edit| {
                                        edit.hint_text("example.com 或 192.168.1.10")
                                            .desired_width(full)
                                    });
                                    ui.end_row();
                                    ui.label("端口");
                                    ui.add_sized(
                                        egui::vec2(half, 20.0),
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
                                        .width(half)
                                        .selected_text(if self.remote.group.is_empty() {
                                            "未分组"
                                        } else {
                                            &self.remote.group
                                        })
                                        .show_ui(ui, |ui| {
                                            ui.selectable_value(
                                                &mut self.remote.group,
                                                String::new(),
                                                "未分组",
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
                                    editing::field_with(ui, &mut self.remote.user, |edit| {
                                        edit.hint_text("留空时连接时输入").desired_width(full)
                                    });
                                    ui.end_row();
                                    ui.label("私钥路径");
                                    editing::field_with(ui, &mut self.remote.identity, |edit| {
                                        edit.desired_width(full)
                                    });
                                    ui.end_row();
                                    ui.label("标签颜色");
                                    ui.horizontal(|ui| {
                                        color_picker(ui, &mut self.remote.color, p);
                                    });
                                    ui.end_row();
                                });
                        });
                        egui::CollapsingHeader::new("高级设置")
                            .default_open(false)
                            .show(ui, |ui| {
                                egui::CollapsingHeader::new("ProxyJump 跳板机").show(ui, |ui| {
                                    let mut enabled = self.remote.jump.is_some();
                                    if ui.checkbox(&mut enabled, "启用一级跳板机").changed()
                                    {
                                        self.remote.jump = if enabled {
                                            Some(Box::new(RemoteProfile::default()))
                                        } else {
                                            None
                                        };
                                    }
                                    if let Some(j) = &mut self.remote.jump {
                                        egui::Grid::new("jump-form")
                                            .spacing([12.0, 6.0])
                                            .min_col_width(48.0)
                                            .show(ui, |ui| {
                                                ui.label("地址");
                                                editing::field_with(ui, &mut j.host, |edit| {
                                                    edit.desired_width(full)
                                                });
                                                ui.end_row();
                                                ui.label("端口");
                                                ui.add_sized(
                                                    egui::vec2(half, 20.0),
                                                    egui::DragValue::new(&mut j.port)
                                                        .range(1..=65535),
                                                );
                                                ui.end_row();
                                                ui.label("用户名");
                                                editing::field_with(ui, &mut j.user, |edit| {
                                                    edit.desired_width(full)
                                                });
                                                ui.end_row();
                                                ui.label("私钥");
                                                editing::field_with(ui, &mut j.identity, |edit| {
                                                    edit.desired_width(full)
                                                });
                                                ui.end_row();
                                            });
                                    }
                                });
                                egui::CollapsingHeader::new("本地端口转发（监听 127.0.0.1）").show(
                                    ui,
                                    |ui| {
                                        let mut remove = None;
                                        for (i, f) in self.remote.forwards.iter_mut().enumerate() {
                                            ui.horizontal(|ui| {
                                                ui.add(
                                                    egui::DragValue::new(&mut f.bind_port)
                                                        .range(1..=65535),
                                                );
                                                ui.label("→");
                                                editing::field_with(
                                                    ui,
                                                    &mut f.target_host,
                                                    |edit| edit.desired_width(half),
                                                );
                                                ui.add(
                                                    egui::DragValue::new(&mut f.target_port)
                                                        .range(1..=65535),
                                                );
                                                if ui.small_button("×").clicked() {
                                                    remove = Some(i);
                                                }
                                            });
                                        }
                                        if let Some(i) = remove {
                                            self.remote.forwards.remove(i);
                                        }
                                        if ui.small_button("+ 转发规则").clicked() {
                                            self.remote.forwards.push(Forward {
                                                bind_port: 8080,
                                                target_host: "127.0.0.1".into(),
                                                target_port: 80,
                                            });
                                        }
                                    },
                                );
                                ui.label(hint("密码在连接时输入，不保存。", p));
                            });
                    }
                    ProfileKind::Serial => {
                        ui.group(|ui| {
                            ui.label(RichText::new("串口连接").strong().color(p.accent));
                            egui::Grid::new("serial-form")
                                .spacing([12.0, 6.0])
                                .min_col_width(72.0)
                                .show(ui, |ui| {
                                    ui.label("串口");
                                    editing::field_with(ui, &mut self.serial.port, |edit| {
                                        edit.desired_width(full)
                                            .hint_text(format!("{} 或 auto", serial::PORT_EXAMPLE))
                                    });
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
                                        if ui.small_button("刷新").clicked() {
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
                                            "未分组"
                                        } else {
                                            &self.serial.group
                                        })
                                        .show_ui(ui, |ui| {
                                            ui.selectable_value(
                                                &mut self.serial.group,
                                                String::new(),
                                                "未分组",
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
                        ui.label(hint("auto 优先选未占用的普通串口", p));
                    }
                }
                ui.horizontal(|ui| {
                    if ui.button("保存").clicked() {
                        save = true;
                    }
                    if ui
                        .add(
                            egui::Button::new(RichText::new("保存并连接").color(p.panel))
                                .fill(p.accent),
                        )
                        .clicked()
                    {
                        save = true;
                        connect = true;
                    }
                });
                ui.label(hint("Enter 保存 · Ctrl+Enter 保存并连接", p));
                if !egui::Popup::is_any_open(ui.ctx())
                    && !editing::ime_composing(ui.ctx())
                    && ui.input(|i| i.key_pressed(Key::Enter))
                {
                    save = true;
                    connect = ui.input(|i| i.modifiers.command);
                }
            });
        self.remote_open = open;
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
        egui::Window::new("连接分组")
            .open(&mut open)
            .resizable(false)
            .show(ctx, |ui| {
                let mut add_group = false;
                ui.horizontal(|ui| {
                    editing::field_with(ui, &mut self.new_group, |edit| edit.desired_width(180.0));
                    if ui.button("添加").clicked() {
                        add_group = true;
                    }
                });
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
                    ui.horizontal(|ui| {
                        if editing::field_with(ui, group, |edit| edit.desired_width(150.0))
                            .changed()
                        {
                            rename = Some((old.clone(), group.clone()));
                        }
                        ui.label(hint("颜色", p));
                        if color_picker(ui, &mut colors[i], p) {
                            recolor = true;
                        }
                        if ui.small_button("删除").clicked() {
                            remove = Some(i);
                        }
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
                ui.label(hint("连接颜色优先于分组；删除分组不删除连接。", p));
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
        if self.serial_picker.is_some() {
            let mut open = true;
            let mut connect = false;
            let mut inspect = false;
            let mut cancel = false;
            let mut refresh = false;
            if let Some(picker) = &mut self.serial_picker {
                egui::Window::new("连接串口")
                    .open(&mut open)
                    .collapsible(false)
                    .resizable(false)
                    .default_width(340.0)
                    .show(ctx, |ui| {
                        ui.horizontal(|ui| {
                            ui.label("串口");
                            editing::field_with(ui, &mut picker.port, |edit| {
                                edit.desired_width(200.0)
                                    .hint_text(format!("{} 或 auto", serial::PORT_EXAMPLE))
                            });
                        });
                        if picker.ports.is_empty() {
                            ui.label(hint("未检测到设备，可输入端口或 auto。", p));
                        } else {
                            egui::ScrollArea::vertical()
                                .max_height(220.0)
                                .show(ui, |ui| {
                                    for port in &picker.ports {
                                        let label = if port.bluetooth {
                                            format!("{}（蓝牙）", port.name)
                                        } else {
                                            port.name.clone()
                                        };
                                        ui.radio_value(&mut picker.port, port.name.clone(), label);
                                    }
                                });
                        }
                        ui.horizontal(|ui| {
                            ui.label("波特率");
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
                        ui.horizontal(|ui| {
                            if ui.button("连接").clicked() {
                                connect = true;
                            }
                            if ui.button("刷新").clicked() {
                                refresh = true;
                            }
                            if ui
                                .button("占用排查")
                                .on_hover_text("查看哪个程序占着这个端口")
                                .clicked()
                            {
                                inspect = true;
                            }
                            if ui.button("取消").clicked() {
                                cancel = true;
                            }
                        });
                        if !egui::Popup::is_any_open(ui.ctx())
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
        egui::Window::new("快捷键帮助")
            .open(&mut self.help_open)
            .default_width(520.0)
            .default_height(520.0)
            .show(ctx, |ui| {
                egui::ScrollArea::vertical().show(ui, |ui| {
                    ui.label(
                        RichText::new(concat!(
                            "G-Terminal ",
                            env!("CARGO_PKG_VERSION"),
                            " · Native. Fast. Yours."
                        ))
                        .color(p.accent),
                    );
                    ui.hyperlink_to(
                        "GitHub · 查看项目与帮助文档",
                        "https://github.com/firmme/G-Terminal",
                    );
                    ui.add_space(6.0);
                    let copy_paste = if ACCEL == "Cmd" {
                        "Cmd+C / Cmd+V".to_string()
                    } else {
                        accel("Ctrl+Shift+C / V")
                    };
                    for (key, description) in [
                        (accel("Ctrl+Shift+T / W"), "新建标签 / 关闭窗格"),
                        (accel("Ctrl+Shift+D / E"), "左右 / 上下分屏"),
                        (
                            format!("Ctrl+Tab / {}", alt_accel("Right")),
                            "切换标签 / 窗格",
                        ),
                        (copy_paste, "复制 / 粘贴"),
                        ("Ctrl+C".to_string(), "终端中断"),
                        (
                            format!("{} / {}", alt_accel("C"), alt_accel("R")),
                            "断开 / 重连当前会话",
                        ),
                        (accel("Ctrl+Shift+F"), "全部保留历史查找"),
                        (accel("Ctrl+Shift+B / Ctrl+,"), "导航栏 / 设置"),
                        ("中键 / Shift+鼠标".to_string(), "粘贴 / 强制选择"),
                        (accel("Ctrl+Plus / Minus / 0"), "字号放大 / 缩小 / 重置"),
                    ] {
                        ui.horizontal(|ui| {
                            ui.monospace(key);
                            ui.label(description);
                        });
                    }
                    ui.separator();
                    if let Some(tab) = self.tabs.get(self.active) {
                        let session = &tab.panes[tab.focused].session;
                        let mut shell = shortcut_shell(&session.kind);
                        ui.label(
                            RichText::new(format!("当前会话：{}", session.kind.label()))
                                .strong()
                                .color(p.accent),
                        );
                        if matches!(shell, ShortcutShell::Unknown | ShortcutShell::Ssh) {
                            if shell == ShortcutShell::Unknown {
                                ui.label(hint("选择当前使用的 Shell", p));
                            } else {
                                ui.label(hint("SSH 默认使用 Unix 快捷键；可切换远端 Shell。", p));
                            }
                            let id = egui::Id::new(("help-shell", tab.panes[tab.focused].id));
                            let mut selected = ui
                                .ctx()
                                .data_mut(|data| data.get_temp::<ShortcutShell>(id))
                                .unwrap_or(shell);
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
                                });
                            ui.ctx().data_mut(|data| data.insert_temp(id, selected));
                            shell = selected;
                        }
                        if shell != ShortcutShell::Unknown {
                            show_shell_shortcuts(ui, shell, p);
                        }
                    }
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
        if self.confirm_exit {
            egui::Window::new("确认退出")
                .collapsible(false)
                .resizable(false)
                .order(egui::Order::Foreground)
                .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
                .show(ctx, |ui| {
                    ui.label("仍有未断开的连接，确定要退出吗？");
                    ui.horizontal(|ui| {
                        if ui.button("退出").clicked() {
                            self.exit_confirmed = true;
                            self.confirm_exit = false;
                            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                        }
                        if ui.button("取消").clicked() {
                            self.confirm_exit = false;
                        }
                    });
                });
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

fn shortcut_rows(ui: &mut egui::Ui, rows: &[(&str, &str)]) {
    egui::Grid::new(ui.next_auto_id())
        .spacing([16.0, 5.0])
        .show(ui, |ui| {
            for &(key, description) in rows {
                ui.monospace(key);
                ui.label(description);
                ui.end_row();
            }
        });
}

fn shortcut_group(ui: &mut egui::Ui, title: &str, rows: &[(&str, &str)], p: Palette) {
    ui.add_space(8.0);
    ui.label(RichText::new(title).strong().color(p.accent));
    shortcut_rows(ui, rows);
}

fn show_shell_shortcuts(ui: &mut egui::Ui, shell: ShortcutShell, p: Palette) {
    match shell {
        ShortcutShell::Unknown => {}
        ShortcutShell::Ssh | ShortcutShell::Unix => {
            ui.label(
                RichText::new(if shell == ShortcutShell::Ssh {
                    "SSH · Bash / Zsh / Ksh"
                } else {
                    "Bash / Zsh / Ksh"
                })
                .strong(),
            );
            ui.label(hint(
                "适用于常见 Emacs/Readline 编辑模式；Vi 模式或自定义键位可能不同。",
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
            ui.label(RichText::new("PowerShell（PSReadLine）").strong());
            shortcut_group(
                ui,
                "光标移动",
                &[
                    ("Home / Ctrl+A", "跳到行首"),
                    ("End / Ctrl+E", "跳到行尾"),
                    ("Ctrl+Left / Right", "向左 / 向右跳一个词"),
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
                    ("Ctrl+Home / End", "删到行首 / 行尾"),
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
            ui.label(RichText::new("Windows CMD").strong());
            shortcut_group(
                ui,
                "光标移动与编辑",
                &[
                    ("Home / End", "跳到行首 / 行尾"),
                    ("Ctrl+Left / Right", "向左 / 向右跳一个词"),
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
            ui.label(RichText::new("串口终端").strong());
            ui.label(hint(
                "串口没有统一的命令行快捷键；编辑行为由串口设备决定。",
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
                    ("Shift+PageUp / Down", "滚动终端历史"),
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
