//! Local and remote file panes.

use super::*;

impl Files {
    pub(super) fn entry_color(
        directory: bool,
        symlink: bool,
        executable: bool,
        p: Palette,
    ) -> eframe::egui::Color32 {
        if symlink {
            p.symlink
        } else if directory {
            p.directory
        } else if executable {
            p.executable
        } else {
            p.text
        }
    }
    pub(super) fn perms_string(perms: Option<u32>) -> String {
        perms.map_or_else(String::new, |m| {
            format!(
                "{}{}{}{}{}{}{}{}{}{}",
                if m & 0o400 != 0 { 'r' } else { '-' },
                if m & 0o200 != 0 { 'w' } else { '-' },
                if m & 0o100 != 0 { 'x' } else { '-' },
                if m & 0o040 != 0 { 'r' } else { '-' },
                if m & 0o020 != 0 { 'w' } else { '-' },
                if m & 0o010 != 0 { 'x' } else { '-' },
                if m & 0o004 != 0 { 'r' } else { '-' },
                if m & 0o002 != 0 { 'w' } else { '-' },
                if m & 0o001 != 0 { 'x' } else { '-' },
                if m & 0o1000 != 0 { 't' } else { ' ' },
            )
        })
    }
    pub(super) fn show_local_pane(
        &mut self,
        ui: &mut egui::Ui,
        p: Palette,
        next: &mut Option<String>,
        menu: &mut Option<MenuAction>,
    ) {
        ui.horizontal(|ui| {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let refresh =
                    crate::icons::icon_label_button(ui, crate::icons::Icon::Refresh, "刷新", p);
                let icons =
                    crate::icons::Size::Button.button().x * 2.0 + ui.spacing().item_spacing.x * 2.0;
                let room = ui.available_width();
                let shortcuts = room - icons >= MIN_FIELD_WIDTH;
                let edit = editing::field_with(ui, &mut self.local_path, |edit| {
                    edit.desired_width(if shortcuts { room - icons } else { room })
                });
                if shortcuts {
                    if crate::icons::icon_button(
                        ui,
                        crate::icons::Icon::Home,
                        p,
                        crate::icons::Size::Button,
                    )
                    .on_hover_text("主目录")
                    .clicked()
                    {
                        *next = Some(self.local_home.display().to_string());
                    }
                    if crate::icons::icon_button(
                        ui,
                        crate::icons::Icon::FolderUp,
                        p,
                        crate::icons::Size::Button,
                    )
                    .on_hover_text("上一级")
                    .clicked()
                    {
                        *next = PathBuf::from(&self.local_path)
                            .parent()
                            .map(|p| p.display().to_string());
                    }
                }
                if refresh.clicked()
                    || (edit.lost_focus()
                        && !editing::ime_composing(ui.ctx())
                        && ui.input(|i| i.key_pressed(egui::Key::Enter)))
                {
                    *next = Some(self.local_path.clone());
                }
            });
        });
        let local = self.local.clone();
        let local = local.lock().unwrap();
        if local.loading {
            ui.spinner();
        }
        if let Some(e) = &local.error {
            ui.colored_label(p.danger, e);
        }
        let width = ui.available_width().max(MIN_TABLE_WIDTH);
        let height = list_height(ui);
        let visible: Vec<_> = local
            .entries
            .iter()
            .filter(|entry| self.show_hidden || !entry.name.starts_with('.'))
            .collect();
        egui::ScrollArea::horizontal()
            .id_salt("local-files")
            .auto_shrink([false, false])
            .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::VisibleWhenNeeded)
            .show(ui, |ui| {
                ui.set_min_width(width);
                table_header(ui, p, &LOCAL_COLUMNS, &LOCAL_HEADERS, width);
                egui::ScrollArea::vertical()
                    .id_salt("local-file-rows")
                    .auto_shrink([false, false])
                    .max_height((height - 24.0).max(50.0))
                    .show_rows(ui, ROW_HEIGHT, visible.len(), |ui, range| {
                        for entry in visible[range].iter().copied() {
                            let path = PathBuf::from(&self.local_path).join(&entry.name);
                            let executable =
                                !entry.symlink && entry.perms.is_some_and(|m| m & 0o111 != 0);
                            let mut cells = vec![
                                Cell {
                                    text: format!(
                                        "{}{}",
                                        entry.name,
                                        entry_suffix(entry.directory, entry.symlink)
                                    ),
                                    right: false,
                                    color: Self::entry_color(
                                        entry.directory,
                                        entry.symlink,
                                        executable,
                                        p,
                                    ),
                                    full: None,
                                    icon: Some(file_icon(&entry.name, entry.directory, executable)),
                                    link: entry.symlink,
                                },
                                Cell {
                                    text: if entry.directory {
                                        String::new()
                                    } else {
                                        format_size(entry.size)
                                    },
                                    right: true,
                                    color: p.muted,
                                    full: None,
                                    icon: None,
                                    link: false,
                                },
                            ];
                            #[cfg(unix)]
                            cells.push(Cell {
                                text: Self::perms_string(entry.perms),
                                right: true,
                                color: p.muted,
                                full: None,
                                icon: None,
                                link: false,
                            });
                            cells.push(Cell {
                                text: entry.mtime.map_or_else(String::new, format_time),
                                right: true,
                                color: p.muted,
                                full: entry.mtime.map(format_time_full),
                                icon: None,
                                link: false,
                            });
                            let selected = self.selected_local.as_ref() == Some(&path);
                            let (r, _) = table_row(ui, &cells, &LOCAL_COLUMNS, selected, width);
                            if r.secondary_clicked() {
                                self.selected_local = Some(path.clone());
                            }
                            if r.clicked() {
                                self.selected_local = Some(path.clone());
                            }
                            if r.double_clicked() {
                                if entry.directory {
                                    *next = Some(path.display().to_string());
                                } else {
                                    *menu = Some(MenuAction::OpenLocal(path.clone()));
                                }
                            }
                            r.context_menu(|ui| {
                                context_menu(
                                    ui,
                                    menu,
                                    vec![
                                        menu_item(
                                            "打开",
                                            true,
                                            MenuAction::OpenLocal(path.clone()),
                                        ),
                                        menu_item(
                                            "编辑",
                                            !entry.directory,
                                            MenuAction::EditLocal(path.clone()),
                                        ),
                                        separator(),
                                        menu_item(
                                            "上传 →",
                                            !entry.directory,
                                            MenuAction::Upload(path.clone()),
                                        ),
                                        separator(),
                                        menu_item(
                                            "重命名",
                                            true,
                                            MenuAction::RenameLocal(path.clone()),
                                        ),
                                        menu_item(
                                            "删除",
                                            true,
                                            MenuAction::DeleteLocal(path.clone()),
                                        ),
                                        separator(),
                                        menu_item(
                                            "复制路径",
                                            true,
                                            MenuAction::CopyPath(path.display().to_string()),
                                        ),
                                        menu_item(
                                            "属性",
                                            true,
                                            MenuAction::Properties(local_properties(entry, &path)),
                                        ),
                                    ],
                                );
                            });
                        }
                    });
            });
    }
    pub(super) fn show_remote_pane(
        &mut self,
        ui: &mut egui::Ui,
        p: Palette,
        next: &mut Option<String>,
        menu: &mut Option<MenuAction>,
    ) {
        let (path, loading, error, entries) = {
            let state = self.directory.lock().unwrap();
            (
                state.path.clone(),
                state.loading,
                state.error.clone(),
                state.entries.clone(),
            )
        };
        if self.remote_home.is_none() && !loading && !path.is_empty() {
            self.remote_home = Some(path.clone());
        }
        ui.horizontal(|ui| {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let refresh =
                    crate::icons::icon_label_button(ui, crate::icons::Icon::Refresh, "刷新", p);
                let icons =
                    crate::icons::Size::Button.button().x * 2.0 + ui.spacing().item_spacing.x * 2.0;
                let room = ui.available_width();
                let shortcuts = room - icons >= MIN_FIELD_WIDTH;
                let edit = editing::field_with(ui, &mut self.remote_path, |edit| {
                    edit.desired_width(if shortcuts { room - icons } else { room })
                });
                let home = self.remote_home.clone();
                if shortcuts {
                    let home_button = crate::icons::icon_button(
                        ui,
                        crate::icons::Icon::Home,
                        p,
                        crate::icons::Size::Button,
                    )
                    .on_hover_text("主目录");
                    if let Some(home) = home
                        && home_button.clicked()
                    {
                        *next = Some(home);
                    }
                    if crate::icons::icon_button(
                        ui,
                        crate::icons::Icon::FolderUp,
                        p,
                        crate::icons::Size::Button,
                    )
                    .on_hover_text("上一级")
                    .clicked()
                    {
                        *next = Some(parent_path(&path));
                    }
                }
                if !loading && !path.is_empty() && !edit.has_focus() {
                    self.remote_path.clone_from(&path);
                }
                if refresh.clicked()
                    || (edit.lost_focus()
                        && !editing::ime_composing(ui.ctx())
                        && ui.input(|i| i.key_pressed(egui::Key::Enter)))
                {
                    *next = Some(self.remote_path.clone());
                }
            });
        });
        if loading {
            ui.spinner();
        }
        if let Some(e) = &error {
            ui.colored_label(p.danger, e);
        }
        let width = ui.available_width().max(MIN_TABLE_WIDTH);
        let height = list_height(ui);
        let visible: Vec<_> = entries
            .iter()
            .filter(|entry| self.show_hidden || !entry.name.starts_with('.'))
            .collect();
        egui::ScrollArea::horizontal()
            .id_salt("remote-files")
            .auto_shrink([false, false])
            .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::VisibleWhenNeeded)
            .show(ui, |ui| {
                ui.set_min_width(width);
                table_header(
                    ui,
                    p,
                    &REMOTE_COLUMNS,
                    &["名称", "大小", "权限", "修改时间"],
                    width,
                );
                egui::ScrollArea::vertical()
                    .id_salt("remote-file-rows")
                    .auto_shrink([false, false])
                    .max_height((height - 24.0).max(50.0))
                    .show_rows(ui, ROW_HEIGHT, visible.len(), |ui, range| {
                        for entry in visible[range].iter().copied() {
                            let executable = entry.perms.is_some_and(|m| m & 0o111 != 0);
                            let cells = [
                                Cell {
                                    text: format!(
                                        "{}{}",
                                        entry.name,
                                        entry_suffix(entry.directory, entry.symlink)
                                    ),
                                    right: false,
                                    color: Self::entry_color(
                                        entry.directory,
                                        entry.symlink,
                                        executable,
                                        p,
                                    ),
                                    full: None,
                                    icon: Some(file_icon(&entry.name, entry.directory, executable)),
                                    link: entry.symlink,
                                },
                                Cell {
                                    text: if entry.directory {
                                        String::new()
                                    } else {
                                        format_size(entry.size)
                                    },
                                    right: true,
                                    color: p.muted,
                                    full: None,
                                    icon: None,
                                    link: false,
                                },
                                Cell {
                                    text: Self::perms_string(entry.perms),
                                    right: true,
                                    color: p.muted,
                                    full: None,
                                    icon: None,
                                    link: false,
                                },
                                Cell {
                                    text: entry
                                        .mtime
                                        .map_or_else(String::new, |t| format_time(u64::from(t))),
                                    right: true,
                                    color: p.muted,
                                    full: entry.mtime.map(|t| format_time_full(u64::from(t))),
                                    icon: None,
                                    link: false,
                                },
                            ];
                            let selected = self
                                .selected_remote
                                .as_ref()
                                .is_some_and(|e| e.name == entry.name);
                            let (r, _) = table_row(ui, &cells, &REMOTE_COLUMNS, selected, width);
                            if r.secondary_clicked() {
                                self.selected_remote = Some(entry.clone());
                            }
                            if r.clicked() {
                                self.selected_remote = Some(entry.clone());
                                self.name = entry.name.clone();
                            }
                            if r.double_clicked() {
                                if entry.directory {
                                    *next = Some(join_path(&path, &entry.name));
                                } else {
                                    *menu = Some(MenuAction::OpenRemote(RemoteTarget {
                                        entry: entry.clone(),
                                        path: join_path(&path, &entry.name),
                                    }));
                                }
                            }
                            r.context_menu(|ui| {
                                let target = || RemoteTarget {
                                    entry: entry.clone(),
                                    path: join_path(&path, &entry.name),
                                };
                                let file = !entry.directory;
                                context_menu(
                                    ui,
                                    menu,
                                    vec![
                                        menu_item("打开", file, MenuAction::OpenRemote(target())),
                                        menu_item("编辑", file, MenuAction::EditRemote(target())),
                                        menu_item("← 下载", file, MenuAction::Download(target())),
                                        separator(),
                                        menu_item(
                                            "重命名",
                                            true,
                                            MenuAction::RenameRemote(target()),
                                        ),
                                        menu_item("删除", true, MenuAction::DeleteRemote(target())),
                                        separator(),
                                        menu_item(
                                            "复制路径",
                                            true,
                                            MenuAction::CopyPath(target().path),
                                        ),
                                        menu_item(
                                            "属性",
                                            true,
                                            MenuAction::Properties(remote_properties(
                                                &target(),
                                                Self::perms_string(entry.perms),
                                            )),
                                        ),
                                    ],
                                );
                            });
                        }
                    });
            });
    }
}
