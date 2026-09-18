//! The two file panes: the local listing and the remote one, drawn from the
//! same table helpers.

use super::*;

impl Files {
    pub(super) fn entry_color(
        directory: bool,
        symlink: bool,
        executable: bool,
        p: Palette,
    ) -> eframe::egui::Color32 {
        // Blue, cyan, green — the `ls` convention, and the terminal's own colours,
        // so the list and `ls` agree. Directories used to be the accent green,
        // which reads as "executable" to anyone who has used `ls --color`.
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
            // Laid out from the right so the field takes exactly what is left.
            // Sizing it from the left pushed 刷新 past the pane edge, where the
            // divider or the window edge cut it off.
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let refresh =
                    crate::icons::icon_label_button(ui, crate::icons::Icon::Refresh, "刷新", p);
                // The two icons come after the field in this right-to-left layout,
                // so their room has to be held back: otherwise the field takes the
                // lot and they land at negative x, over the pane to the left.
                let icons =
                    crate::icons::Size::Button.button().x * 2.0 + ui.spacing().item_spacing.x * 2.0;
                let room = ui.available_width();
                // They are shortcuts; the field is not. In a pane too narrow for
                // both, the shortcuts go, so that nothing is ever pushed outside —
                // a floor on the field's width would guarantee that it was.
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
        // Holding the Arc rather than borrowing `self`, so the row handlers below
        // stay free to write `selected_local`.
        let local = self.local.clone();
        let local = local.lock().unwrap();
        if local.loading {
            ui.spinner();
        }
        if let Some(e) = &local.error {
            ui.colored_label(p.danger, e);
        }
        // The header lives inside the scroller, so it travels sideways with the
        // rows instead of drifting out of alignment with them.
        egui::ScrollArea::both()
            .id_salt("local-files")
            .auto_shrink([false, false])
            .max_height(list_height(ui))
            .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::VisibleWhenNeeded)
            .show(ui, |ui| {
                // Wider than the pane when the pane is narrow: the columns keep
                // their widths and the overflow becomes a horizontal scrollbar,
                // rather than every column being squeezed into a truncation.
                let width = ui.available_width().max(MIN_TABLE_WIDTH);
                table_header(ui, p, &LOCAL_COLUMNS, &LOCAL_HEADERS, width);
                for entry in &local.entries {
                    if !self.show_hidden && entry.name.starts_with('.') {
                        continue;
                    }
                    let path = PathBuf::from(&self.local_path).join(&entry.name);
                    // A set executable bit (any of the three) is what `ls`
                    // highlights, and what makes the icon a script rather than a
                    // document. A symlink's own mode says nothing about its
                    // target, so links never count as executable here; Windows
                    // reports no mode at all.
                    let executable = !entry.symlink && entry.perms.is_some_and(|m| m & 0o111 != 0);
                    let mut cells = vec![
                        Cell {
                            text: format!(
                                "{}{}",
                                entry.name,
                                entry_suffix(entry.directory, entry.symlink)
                            ),
                            right: false,
                            color: Self::entry_color(entry.directory, entry.symlink, executable, p),
                            full: None,
                            icon: Some(file_icon(&entry.name, entry.directory, executable)),
                            link: entry.symlink,
                        },
                        Cell {
                            // A directory has no meaningful size of its own.
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
                        // The column shows a shortened form; hovering recovers
                        // the exact timestamp.
                        full: entry.mtime.map(format_time_full),
                        icon: None,
                        link: false,
                    });
                    let selected = self.selected_local.as_ref() == Some(&path);
                    let (r, _) = table_row(ui, &cells, &LOCAL_COLUMNS, selected, width);
                    // A right-click selects the row first, the way a file manager
                    // does, so the menu always acts on the row under the cursor
                    // rather than on whatever happened to be selected.
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
                            // A file opens in whatever the OS associates with it,
                            // the same as the menu's 打开.
                            *menu = Some(MenuAction::OpenLocal(path.clone()));
                        }
                    }
                    r.context_menu(|ui| {
                        context_menu(
                            ui,
                            menu,
                            vec![
                                menu_item("打开", true, MenuAction::OpenLocal(path.clone())),
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
                                menu_item("重命名", true, MenuAction::RenameLocal(path.clone())),
                                menu_item("删除", true, MenuAction::DeleteLocal(path.clone())),
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
    }
    pub(super) fn show_remote_pane(
        &mut self,
        ui: &mut egui::Ui,
        p: Palette,
        next: &mut Option<String>,
        menu: &mut Option<MenuAction>,
    ) {
        // Snapshot under the lock, then release it before the row handlers below
        // start mutating other `self` fields.
        let (path, loading, error, entries) = {
            let state = self.directory.lock().unwrap();
            (
                state.path.clone(),
                state.loading,
                state.error.clone(),
                state.entries.clone(),
            )
        };
        // The first listing resolves `.` to the server's home directory, which is
        // what the home button jumps back to.
        if self.remote_home.is_none() && !loading && !path.is_empty() {
            self.remote_home = Some(path.clone());
        }
        ui.horizontal(|ui| {
            // Right to left, same as the local pane: the field takes what is left
            // and the buttons cannot be pushed out of the pane.
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let refresh =
                    crate::icons::icon_label_button(ui, crate::icons::Icon::Refresh, "刷新", p);
                // Same reservation as the local pane: the icons follow the field,
                // and they are dropped rather than the field overflowing.
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
                // Show the path the server actually resolved, but never overwrite
                // what the user is in the middle of typing.
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
        // The header lives inside the scroller, so it travels sideways with the
        // rows instead of drifting out of alignment with them.
        egui::ScrollArea::both()
            .id_salt("remote-files")
            .auto_shrink([false, false])
            .max_height(list_height(ui))
            .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::VisibleWhenNeeded)
            .show(ui, |ui| {
                let width = ui.available_width().max(MIN_TABLE_WIDTH);
                table_header(
                    ui,
                    p,
                    &REMOTE_COLUMNS,
                    &["名称", "大小", "权限", "修改时间"],
                    width,
                );
                for entry in &entries {
                    if !self.show_hidden && entry.name.starts_with('.') {
                        continue;
                    }
                    let executable = entry.perms.is_some_and(|m| m & 0o111 != 0);
                    let cells = [
                        Cell {
                            text: format!(
                                "{}{}",
                                entry.name,
                                entry_suffix(entry.directory, entry.symlink)
                            ),
                            right: false,
                            color: Self::entry_color(entry.directory, entry.symlink, executable, p),
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
                            // A remote file is fetched to the scratch directory and
                            // then opened, exactly as the menu's 打开 does.
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
                        // Everything but the metadata actions needs a regular file:
                        // a directory is entered by double-clicking, not opened.
                        let file = !entry.directory;
                        context_menu(
                            ui,
                            menu,
                            vec![
                                menu_item("打开", file, MenuAction::OpenRemote(target())),
                                menu_item("编辑", file, MenuAction::EditRemote(target())),
                                menu_item("← 下载", file, MenuAction::Download(target())),
                                separator(),
                                menu_item("重命名", true, MenuAction::RenameRemote(target())),
                                menu_item("删除", true, MenuAction::DeleteRemote(target())),
                                separator(),
                                menu_item("复制路径", true, MenuAction::CopyPath(target().path)),
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
    }
}
