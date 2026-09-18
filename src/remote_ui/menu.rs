//! The file-pane context menu: its rows, the actions they dispatch, and the
//! batch rules that decide which actions make sense for a selection.

use super::*;

/// One row of a file-pane context menu. An entry that does not apply to the row is
/// greyed out, rather than left clickable and failing afterwards.
pub(super) struct MenuItem {
    label: &'static str,
    enabled: bool,
    /// None marks a separator.
    action: Option<MenuAction>,
}

pub(super) fn menu_item(label: &'static str, enabled: bool, action: MenuAction) -> MenuItem {
    MenuItem {
        label,
        enabled,
        action: Some(action),
    }
}

pub(super) fn separator() -> MenuItem {
    MenuItem {
        label: "",
        enabled: false,
        action: None,
    }
}

pub(super) fn context_menu(ui: &mut egui::Ui, menu: &mut Option<MenuAction>, items: Vec<MenuItem>) {
    for item in items {
        let Some(action) = item.action else {
            ui.separator();
            continue;
        };
        if ui
            .add_enabled(item.enabled, egui::Button::new(item.label))
            .clicked()
        {
            *menu = Some(action);
            ui.close();
        }
    }
}

pub(super) fn direction_glyph(direction: Direction) -> &'static str {
    match direction {
        Direction::Upload => "↑",
        Direction::Download => "↓",
    }
}

/// The settled queue entries to drop, oldest first. Split from the queue itself so
/// it can be checked without a live connection to build a `Files` around.
pub(super) fn settled_to_drop(settled: &[usize], keep: usize) -> &[usize] {
    &settled[..settled.len().saturating_sub(keep)]
}

/// Every entry of a batch moves the same way, so the first one speaks for all.
pub(super) fn batch_direction(members: &[&Transfer]) -> Direction {
    members.first().map_or(Direction::Upload, |t| t.direction)
}

/// One file's queue row: name, progress, and whatever control applies right now.
pub(super) fn transfer_row(
    ui: &mut egui::Ui,
    transfer: &Transfer,
    p: Palette,
    ctx: &egui::Context,
    nested: bool,
) {
    let s = transfer.state.lock().unwrap().clone();
    ui.horizontal(|ui| {
        let name = file_name_of(&transfer.local);
        // A nested row names just the file: the batch header above it already
        // carries the direction and the directory.
        if nested {
            ui.label(hint(&name, p));
        } else {
            ui.label(format!("{} {name}", direction_glyph(transfer.direction)));
        }
        ui.add(
            egui::ProgressBar::new(if s.total == 0 {
                0.0
            } else {
                s.done as f32 / s.total as f32
            })
            .desired_width(if nested { 120.0 } else { 160.0 }),
        );
        ui.label(hint(
            &format!(
                "{} / {} · {}",
                format_size(s.done),
                format_size(s.total),
                s.message
            ),
            p,
        ));
        if s.running && ui.small_button("暂停").clicked() {
            transfer
                .pause
                .store(true, std::sync::atomic::Ordering::Release);
        }
        if s.running && ui.small_button("取消").clicked() {
            transfer.cancel();
        }
        if !s.running && !s.finished && ui.small_button("继续").clicked() {
            transfer.start(wake(ctx));
        }
    });
}

impl Files {
    /// Applies a context-menu choice. Runs outside the row loops, where `&mut self`
    /// is free of the borrows the panes hold.
    pub(super) fn dispatch(&mut self, action: MenuAction, ctx: &egui::Context) {
        match action {
            MenuAction::OpenLocal(path) => self.report(reveal(&path)),
            MenuAction::EditLocal(path) => self.report(edit_with(&path)),
            MenuAction::OpenRemote(target) => self.open_remote(target, false, ctx),
            MenuAction::EditRemote(target) => self.open_remote(target, true, ctx),
            MenuAction::Upload(path) => {
                let directory = path.is_dir();
                let remote = self.remote_child(&file_name_of(&path));
                self.transfer(path, remote, Direction::Upload, directory, ctx);
            }
            MenuAction::Download(target) => {
                if !remote::safe_local_name(&target.entry.name) {
                    self.error = Some("服务器返回了非法文件名".into());
                    return;
                }
                let directory = target.entry.directory;
                let local = PathBuf::from(&self.local_path).join(&target.entry.name);
                self.transfer(local, target.path, Direction::Download, directory, ctx);
            }
            MenuAction::RenameLocal(path) => {
                self.renaming = Some(Rename {
                    name: file_name_of(&path),
                    local: Some(path),
                    remote: None,
                });
            }
            MenuAction::RenameRemote(target) => {
                self.renaming = Some(Rename {
                    name: target.entry.name.clone(),
                    local: None,
                    remote: Some(target),
                });
            }
            MenuAction::DeleteLocal(path) => {
                if path.is_dir() && !dir_is_empty_locally(&path) {
                    self.confirm = Some(Confirm {
                        message: format!(
                            "目录 {} 非空，将连同其中所有内容一并删除。",
                            file_name_of(&path)
                        ),
                        action: ConfirmAction::DeleteLocal(path),
                    });
                } else {
                    self.delete_local(path);
                }
            }
            MenuAction::DeleteRemote(target) => {
                if !target.entry.directory {
                    let loaded = self.simple_operation();
                    spawn_delete_remote(self.connection.clone(), target, loaded, ctx);
                } else {
                    // Whether a directory is empty decides whether this needs
                    // confirming, and only the server can say.
                    let slot = self.simple_operation();
                    let connection = self.connection.clone();
                    let probe = target.clone();
                    let wake = wake(ctx);
                    remote::runtime().spawn(async move {
                        let result = async {
                            let sftp = connection.sftp("检查目录").await?;
                            remote::directory_is_empty(&sftp, &probe.path).await
                        }
                        .await;
                        *slot.lock().unwrap() = Some(match result {
                            Ok(true) => Ok(String::new()),
                            Ok(false) => Ok("非空".into()),
                            Err(e) => Err(format!("{e:#}")),
                        });
                        wake();
                    });
                    self.deleting = Some(target);
                }
            }
            MenuAction::CopyPath(text) => {
                ctx.copy_text(text);
                self.notice = Some("已复制路径".into());
            }
            MenuAction::Properties(properties) => self.properties = Some(properties),
        }
    }
}
