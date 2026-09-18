//! The windows the app opens over itself: settings, the connection form, the
//! groups, the toolbox, the serial picker, the update window and the exit
//! confirmation.

use super::*;

impl App {
    pub(super) fn dialogs(&mut self, ctx: &egui::Context, action: &mut Option<Action>) {
        let p = self.palette;
        let mut open = self.settings_open;
        let mut changed = false;
        let auto_hide_before = self.settings.auto_hide_sidebar;
        egui::Window::new("偏好设置")
            .open(&mut open)
            .collapsible(false)
            .default_width(440.0)
            .show(ctx, |ui| {
                changed |= ui
                    .add(egui::Slider::new(&mut self.settings.font_size, 10.0..=28.0).text("字号"))
                    .changed();
                changed |= ui
                    .add(
                        egui::Slider::new(&mut self.settings.scrollback, 100..=50_000)
                            .logarithmic(true)
                            .text("历史行数（新会话）"),
                    )
                    .changed();
                ui.horizontal(|ui| {
                    changed |= ui
                        .checkbox(&mut self.settings.light_theme, "浅色主题")
                        .changed();
                    changed |= ui.checkbox(&mut self.settings.sidebar, "导航栏").changed();
                });
                changed |= ui
                    .checkbox(&mut self.settings.copy_on_select, "选中文本后自动复制")
                    .changed();
                changed |= ui
                    .checkbox(
                        &mut self.settings.hide_dotfiles,
                        "文件窗口默认隐藏 . 开头文件",
                    )
                    .changed();
                changed |= ui
                    .checkbox(
                        &mut self.settings.restore_tabs,
                        "启动时恢复上次关闭的标签（不重连）",
                    )
                    .changed();
                changed |= ui
                    .checkbox(
                        &mut self.settings.confirm_on_exit,
                        "关闭窗口时确认（有活动会话）",
                    )
                    .changed();
                changed |= ui
                    .checkbox(
                        &mut self.settings.auto_hide_sidebar,
                        "自动隐藏导航栏（鼠标移到左侧时悬浮展开）",
                    )
                    .changed();
                ui.label(hint(
                    "鼠标中键：粘贴；Shift+鼠标：绕过应用鼠标协议选择文本。",
                    p,
                ));
                ui.label(hint(
                    "恢复只还原标签，全部显示为断开；点「重新连接」后才连上。",
                    p,
                ));
                ui.horizontal(|ui| {
                    ui.label("默认 Shell");
                    egui::ComboBox::from_id_salt("default-shell")
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
                });
                ui.horizontal(|ui| {
                    ui.label("浏览器搜索");
                    egui::ComboBox::from_id_salt("search-engine")
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
                });
                ui.separator();
                ui.label(hint(Settings::path().to_string_lossy().as_ref(), p));
            });
        self.settings_open = open;
        if self.settings.auto_hide_sidebar != auto_hide_before {
            // Turning auto-hide on starts with the bar hidden; turning it off
            // leaves it docked.
            self.sidebar_pinned = !self.settings.auto_hide_sidebar;
        }
        if changed {
            self.palette = Palette::new(self.settings.light_theme);
            self.palette.apply(ctx, self.settings.light_theme);
            self.persist();
        }
        let mut open = self.remote_open;
        let mut save = false;
        let mut connect = false;
        let mut refresh_ports = false;
        egui::Window::new("连接配置")
            .open(&mut open)
            .collapsible(false)
            .default_width(480.0)
            .show(ctx, |ui| {
                // Rows come in two widths so the form reads as a grid rather
                // than as a ragged column: long values span the form, short ones
                // (ports, groups, rates) take half.
                let full = 300.0;
                let half = 150.0;
                ui.horizontal(|ui| {
                    ui.label("类型");
                    ui.selectable_value(&mut self.profile_kind, ProfileKind::Ssh, "SSH");
                    ui.selectable_value(&mut self.profile_kind, ProfileKind::Serial, "串口");
                });
                ui.separator();
                match self.profile_kind {
                    ProfileKind::Ssh => {
                        egui::Grid::new("connection-form")
                            .spacing([12.0, 6.0])
                            .min_col_width(72.0)
                            .show(ui, |ui| {
                                ui.label("主机 / IP");
                                editing::field_with(ui, &mut self.remote.host, |edit| {
                                    edit.desired_width(full)
                                });
                                ui.end_row();
                                ui.label("端口");
                                ui.add_sized(
                                    egui::vec2(half, 20.0),
                                    egui::DragValue::new(&mut self.remote.port).range(1..=65535),
                                );
                                ui.end_row();
                                ui.label("连接名称");
                                editing::field_with(ui, &mut self.remote.name, |edit| {
                                    edit.desired_width(full)
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
                                    edit.desired_width(full)
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
                        egui::CollapsingHeader::new("ProxyJump 跳板机").show(ui, |ui| {
                            let mut enabled = self.remote.jump.is_some();
                            if ui.checkbox(&mut enabled, "启用一级跳板机").changed() {
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
                                            egui::DragValue::new(&mut j.port).range(1..=65535),
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
                                            egui::DragValue::new(&mut f.bind_port).range(1..=65535),
                                        );
                                        ui.label("→");
                                        editing::field_with(ui, &mut f.target_host, |edit| {
                                            edit.desired_width(half)
                                        });
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
                        ui.label(hint("认证在连接时进行，密码不写入配置。", p));
                    }
                    ProfileKind::Serial => {
                        egui::Grid::new("serial-form")
                            .spacing([12.0, 6.0])
                            .min_col_width(72.0)
                            .show(ui, |ui| {
                                ui.label("串口");
                                // Typing a device name and picking one from the
                                // list are both allowed; the field is what is
                                // actually stored.
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
                        ui.label(hint(
                            "串口填 auto 时，连接优先选普通串口，并跳过本程序已占用的串口。",
                            p,
                        ));
                    }
                }
                ui.horizontal(|ui| {
                    if ui.button("保存").clicked() {
                        save = true;
                    }
                    if ui.button("保存并连接").clicked() {
                        save = true;
                        connect = true;
                    }
                });
                ui.label(hint("回车保存，Ctrl+回车保存并连接。", p));
                // A popup consumes Enter itself (picking from a combo), so this
                // only fires while the form has the keyboard.
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
                if !egui::Popup::is_any_open(ui.ctx()) && !editing::ime_composing(ui.ctx()) && ui.input(|i| i.key_pressed(Key::Enter)) {
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
                // Edited apart from the group list so the colours map is not
                // borrowed while the names are.
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
                        if editing::field_with(ui, group, |edit| edit.desired_width(150.0)).changed()
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
                    // The group's colour is keyed by name, so it moves too.
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
                ui.label(hint(
                    "标签颜色：连接自身的颜色优先于分组；都为空时不着色。删除分组后，连接移到未分组。",
                    p,
                ));
            });
        self.groups_open = open;
        if modified {
            self.persist();
        }
        if let Some(mut toolbox) = self.toolbox.take() {
            // The script is typed into the focused session, so the target is
            // resolved once, right before it runs.
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
            let mut cancel = false;
            let mut refresh = false;
            if let Some(picker) = &mut self.serial_picker {
                egui::Window::new("连接串口")
                    .open(&mut open)
                    .collapsible(false)
                    .resizable(false)
                    .default_width(340.0)
                    .show(ctx, |ui| {
                        // The list can be empty (nothing attached) and a device
                        // may need a name the system did not enumerate, so the
                        // port is always typable as well as selectable.
                        ui.horizontal(|ui| {
                            ui.label("串口");
                            editing::field_with(ui, &mut picker.port, |edit| {
                                edit.desired_width(200.0)
                                    .hint_text(format!("{} 或 auto", serial::PORT_EXAMPLE))
                            });
                        });
                        if picker.ports.is_empty() {
                            ui.label(hint("未检测到串口设备，可手动输入或填 auto。", p));
                        } else {
                            ui.label(hint("普通串口在前，蓝牙串口在后。", p));
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
        egui::Window::new("关于 / 快捷键")
            .open(&mut self.help_open)
            .default_width(520.0)
            .show(ctx, |ui| {
                ui.label(
                    RichText::new(concat!(
                        "G-Terminal ",
                        env!("CARGO_PKG_VERSION"),
                        " · Native. Fast. Yours."
                    ))
                    .color(p.accent),
                );
                // Ctrl+C stays the terminal interrupt on every platform; only
                // the clipboard accelerators follow the Cmd / Ctrl split.
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
                ui.label(hint(
                    "Agent 仅预留协议。终端兼容性范围和待实现项见 README。",
                    p,
                ));
            });
        if let Some(login) = &mut self.login {
            let mut open = true;
            let ready = login.show(ctx, &mut open, p);
            if let Some(c) = ready {
                self.login = None;
                self.add_connection(c, ctx);
            } else if !open {
                self.login = None;
                // Backing out leaves the pre-created pane, marked so it still
                // offers a retry.
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
            // A running transfer has to keep ticking so its progress and any
            // conflict prompt show up while the user is idle.
            if files.busy() {
                ctx.request_repaint_after(std::time::Duration::from_millis(150));
            }
        }
        self.update_window(ctx, p);
    }

    /// The close request the platform raises while sessions are still live
    /// becomes this dialog; the setting may have skipped it.
    pub(super) fn exit_confirm(&mut self, ctx: &egui::Context) {
        if self.confirm_exit {
            egui::Window::new("确认退出")
                .collapsible(false)
                .resizable(false)
                // Above the file window and anything else that can be raised.
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
