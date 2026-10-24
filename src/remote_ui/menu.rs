//! The file-pane context menu: its rows, the actions they dispatch, and the
//! batch rules that decide which actions make sense for a selection.

use super::*;
use g_terminal::LockExt;

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

pub(super) fn context_menu(
    ui: &mut egui::Ui,
    menu: &mut Option<MenuAction>,
    items: Vec<MenuItem>,
    p: Palette,
) {
    ui.set_min_width(200.0);
    for item in items {
        let Some(action) = item.action else {
            ui.separator();
            continue;
        };
        let icon = menu_icon(&action);
        if ui
            .add_enabled_ui(item.enabled, |ui| {
                crate::icons::icon_row(ui, icon, item.label, None, p)
            })
            .inner
            .clicked()
            && item.enabled
        {
            *menu = Some(action);
            ui.close();
        }
    }
}

/// The glyph that leads a file action in the context menu.
fn menu_icon(action: &MenuAction) -> crate::icons::Icon {
    use crate::icons::Icon;
    match action {
        MenuAction::OpenLocal(_) | MenuAction::OpenRemote(_) => Icon::File,
        MenuAction::EditLocal(_) | MenuAction::EditRemote(_) => Icon::FileText,
        MenuAction::Upload(_) => Icon::ArrowUp,
        MenuAction::Download(_) => Icon::ArrowDown,
        MenuAction::RenameLocal(_) | MenuAction::RenameRemote(_) => Icon::Edit,
        MenuAction::DeleteLocal(_) | MenuAction::DeleteRemote(_) => Icon::Trash,
        MenuAction::CopyPath(_) => Icon::Copy,
        MenuAction::Properties(_) => Icon::Info,
    }
}

pub(super) fn direction_icon(direction: Direction) -> crate::icons::Icon {
    match direction {
        Direction::Upload => crate::icons::Icon::ArrowUp,
        Direction::Download => crate::icons::Icon::ArrowDown,
    }
}

/// The upload / download arrow before a queue row.
pub(super) fn direction_mark(ui: &mut egui::Ui, direction: Direction, p: Palette) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(16.0, 16.0), egui::Sense::hover());
    crate::icons::draw(
        ui.painter(),
        rect,
        direction_icon(direction),
        match direction {
            Direction::Upload => p.warn,
            Direction::Download => p.ok,
        },
        1.5,
    );
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
    let s = transfer.state.locked().clone();
    ui.horizontal(|ui| {
        let name = file_name_of(&transfer.local);
        // A nested row names just the file: the batch header above it already
        // carries the direction and the directory.
        if nested {
            ui.add_space(20.0);
            ui.label(hint(&name, p));
        } else {
            direction_mark(ui, transfer.direction, p);
            ui.label(name);
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
        use crate::icons::{Icon, Size, icon_button};
        if s.running
            && icon_button(ui, Icon::Pause, p, Size::Row)
                .on_hover_text("暂停")
                .clicked()
        {
            transfer
                .pause
                .store(true, std::sync::atomic::Ordering::Release);
        }
        if s.running
            && icon_button(ui, Icon::Close, p, Size::Row)
                .on_hover_text("取消")
                .clicked()
        {
            transfer.cancel();
        }
        if !s.running
            && !s.finished
            && icon_button(ui, Icon::Play, p, Size::Row)
                .on_hover_text("继续")
                .clicked()
        {
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
                        *slot.locked() = Some(match result {
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
