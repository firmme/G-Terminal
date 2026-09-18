//! The prompts the file window raises: overwrite conflicts, delete and rename
//! confirmation, properties and the permission editor.

use super::*;

/// A checkbox for one permission bit.
pub(super) fn bit_box(ui: &mut egui::Ui, mode: &mut u32, label: &str, mask: u32) {
    let mut on = *mode & mask != 0;
    if ui.checkbox(&mut on, label).changed() {
        if on {
            *mode |= mask;
        } else {
            *mode &= !mask;
        }
    }
}

/// The permission editor, drawn inside the 属性 window.
///
/// Returns `(apply, count)`: `count` asks for a recursive walk first, `apply`
/// asks for the change to go ahead. A recursive change always counts first, so the
/// confirmation can say how much it covers.
pub(super) fn permission_editor(
    ui: &mut egui::Ui,
    edit: &mut PermissionEdit,
    p: Palette,
) -> (bool, bool) {
    let mut apply = false;
    let mut count = false;

    egui::Grid::new("permission-bits")
        .num_columns(4)
        .spacing([12.0, 4.0])
        .min_col_width(40.0)
        .show(ui, |ui| {
            ui.label(hint("", p));
            for heading in ["读", "写", "执行"] {
                ui.label(hint(heading, p));
            }
            ui.end_row();
            for (row, (who, read, write, execute)) in [
                ("所有者", 0o400, 0o200, 0o100),
                ("用户组", 0o040, 0o020, 0o010),
                ("其他", 0o004, 0o002, 0o001),
            ]
            .into_iter()
            .enumerate()
            {
                ui.label(hint(who, p));
                for (bit, mask) in [("读", read), ("写", write), ("执行", execute)] {
                    // Scoped, so three boxes labelled 读 in one grid stay distinct.
                    ui.push_id((row, bit), |ui| {
                        bit_box(ui, &mut edit.mode, bit, mask);
                    });
                }
                ui.end_row();
            }
        });
    ui.horizontal(|ui| {
        bit_box(ui, &mut edit.mode, "setuid", 0o4000);
        bit_box(ui, &mut edit.mode, "setgid", 0o2000);
        bit_box(ui, &mut edit.mode, "sticky", 0o1000);
    });
    // Shown live, because the checkboxes and the number are the same fact told two
    // ways and seeing both is how you catch a mis-click.
    ui.label(hint(&format!("八进制：{:04o}", edit.mode), p));

    // Ownership is an SFTP-side change: locally it would need root, and the
    // names are resolved against the server for a remote path.
    if !matches!(edit.target, PermTarget::Local(_)) {
        ui.horizontal(|ui| {
            ui.label(hint("所有者", p));
            editing::field_with(ui, &mut edit.owner, |field| {
                field.desired_width(96.0).hint_text("uid 或用户名")
            });
            ui.label(hint("用户组", p));
            editing::field_with(ui, &mut edit.group, |field| {
                field.desired_width(96.0).hint_text("gid 或组名")
            });
        });
    }
    ui.checkbox(&mut edit.recursive, "递归应用到该目录下所有内容");

    match edit.counted {
        Some(count) => {
            ui.colored_label(p.warn, format!("将修改 {count} 个条目，确认吗？"));
            ui.horizontal(|ui| {
                if ui.button("确认修改").clicked() {
                    apply = true;
                }
                if ui.button("取消").clicked() {
                    edit.counted = None;
                }
            });
        }
        None => {
            if ui.button("应用权限").clicked() {
                if edit.recursive {
                    count = true;
                } else {
                    apply = true;
                }
            }
        }
    }
    (apply, count)
}

impl Files {
    /// Picks up a conflict a running transfer is blocked on.
    pub(super) fn poll_question(&mut self) {
        if let Some(questions) = &mut self.questions {
            while let Ok(question) = questions.try_recv() {
                self.rename = question.name();
                self.question = Some(question);
            }
        }
    }

    /// Draws the overwrite / rename / resume prompt for an incoming file.
    pub fn show_question(&mut self, ctx: &egui::Context, p: Palette) {
        self.poll_question();
        let Some(question) = self.question.clone() else {
            return;
        };
        let mut answer = None;
        egui::Window::new("ZMODEM 文件冲突")
            .collapsible(false)
            .resizable(false)
            .default_width(400.0)
            // A prompt the transfer is waiting on must never end up behind the
            // file window, which is an ordinary window and can be raised.
            .order(egui::Order::Foreground)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                ui.label(format!(
                    "服务器正在发送 {}（{}）",
                    question.name(),
                    format_size(question.total)
                ));
                if let Some(size) = question.existing {
                    ui.colored_label(
                        p.warn,
                        format!("目标位置已有同名文件，{}", format_size(size)),
                    );
                }
                if let Some(size) = question.partial {
                    ui.label(hint(
                        &format!("存在未完成的下载，可续传（已完成 {}）", format_size(size)),
                        p,
                    ));
                }
                ui.add_space(4.0);
                ui.horizontal_wrapped(|ui| {
                    if question.can_resume() && ui.button("续传").clicked() {
                        answer = Some(ztransfer::Decision::Resume);
                    }
                    if ui
                        .button("覆盖")
                        .on_hover_text("传输完成后替换同名文件；中途失败不会破坏原文件")
                        .clicked()
                    {
                        answer = Some(ztransfer::Decision::Overwrite);
                    }
                    editing::field_with(ui, &mut self.rename, |edit| edit.desired_width(150.0));
                    if ui.button("重命名").clicked() {
                        answer = Some(ztransfer::Decision::Rename(self.rename.clone()));
                    }
                    if ui.button("取消传输").clicked() {
                        answer = Some(ztransfer::Decision::Cancel);
                    }
                });
            });
        if let Some(answer) = answer
            && let Some(answers) = &self.answers
        {
            let _ = answers.send(answer);
            self.question = None;
        }
    }

    /// Deletes a local file or tree on a worker thread. `remove_dir_all` over a
    /// large tree used to run inline and freeze the window for its duration.
    pub(super) fn delete_local(&mut self, path: PathBuf) {
        let state = self.simple_operation();
        let wake = self.wake.clone();
        remote::runtime().spawn_blocking(move || {
            let outcome = if path.is_dir() {
                std::fs::remove_dir_all(&path)
            } else {
                std::fs::remove_file(&path)
            };
            *state.lock().unwrap() = Some(match outcome {
                Ok(()) => Ok(format!("已删除 {}", file_name_of(&path))),
                Err(e) => Err(format!("删除失败：{e}")),
            });
            wake();
        });
    }

    pub(super) fn delete_remote(&mut self, target: RemoteTarget, ctx: &egui::Context) {
        let loaded = self.simple_operation();
        spawn_delete_remote(self.connection.clone(), target, loaded, ctx);
    }

    /// Fetches a remote file into a scratch directory and opens it. In edit mode the
    /// copy is watched, and every save is sent back over the original.
    pub(super) fn open_remote(&mut self, target: RemoteTarget, edit: bool, ctx: &egui::Context) {
        if target.entry.directory {
            self.error = Some("目录不能打开；双击进入即可".into());
            return;
        }
        if !remote::safe_local_name(&target.entry.name) {
            self.error = Some("服务器返回了非法文件名".into());
            return;
        }
        let Some(dir) = scratch_dir(self.edits.len()) else {
            self.error = Some("无法创建临时目录".into());
            return;
        };
        let local = dir.join(&target.entry.name);
        let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
        if edit {
            self.edits.push(EditSession {
                stop: stop.clone(),
                dir: dir.clone(),
            });
        }
        let connection = self.connection.clone();
        let remote = target.path.clone();
        let state = self.simple_operation();
        let wake = wake(ctx);
        remote::runtime().spawn(async move {
            let fail = |state: &Outcome, message: String| {
                *state.lock().unwrap() = Some(Err(message));
            };
            if let Err(e) = remote::fetch(&connection, &remote, &local).await {
                fail(&state, format!("{e:#}"));
                wake();
                return;
            }
            *state.lock().unwrap() = Some(Ok(if edit {
                "已打开，保存后自动回传".into()
            } else {
                "已下载并打开".into()
            }));
            wake();
            let opened = if edit {
                edit_with(&local)
            } else {
                reveal(&local)
            };
            if let Err(e) = opened {
                fail(&state, e);
                wake();
                return;
            }
            if !edit {
                return;
            }
            // Watch the scratch copy and push every save back. The staleness check
            // runs again after a settle delay so a half-written file is not sent.
            let mut last = save_stamp(&local);
            while !stop.load(std::sync::atomic::Ordering::Acquire) {
                tokio::time::sleep(std::time::Duration::from_millis(1000)).await;
                let now = save_stamp(&local);
                if now == last {
                    continue;
                }
                tokio::time::sleep(std::time::Duration::from_millis(700)).await;
                last = save_stamp(&local);
                let outcome = remote::put(&connection, &remote, &local).await;
                *state.lock().unwrap() = Some(match outcome {
                    Ok(()) => Ok("已回传".into()),
                    Err(e) => Err(format!("回传失败：{e:#}")),
                });
                wake();
            }
        });
    }

    /// Walks the tree to count what a recursive change would touch, so the
    /// confirmation can say how much it covers. The walk is the one the change
    /// itself would do, so the number is not an estimate.
    pub(super) fn count_permissions(&mut self, properties: &mut Properties) {
        let Some(edit) = &properties.editable else {
            return;
        };
        let slot: Arc<Mutex<Option<Result<usize, String>>>> = Arc::new(Mutex::new(None));
        let state = slot.clone();
        let wake = self.wake.clone();
        match &edit.target {
            PermTarget::Local(path) => {
                let path = path.clone();
                std::thread::spawn(move || {
                    let result = walk_local(&path)
                        .map(|paths| paths.len())
                        .map_err(|e| format!("{e:#}"));
                    *state.lock().unwrap() = Some(result);
                    wake();
                });
            }
            PermTarget::Remote(target) => {
                let (connection, path) = (self.connection.clone(), target.path.clone());
                remote::runtime().spawn(async move {
                    let result: anyhow::Result<usize> = async {
                        let sftp = connection.sftp("统计目录").await?;
                        Ok(remote::walk_remote(&sftp, &path).await?.len())
                    }
                    .await;
                    *state.lock().unwrap() = Some(result.map_err(|e| format!("{e:#}")));
                    wake();
                });
            }
        }
        self.counting = Some(slot);
    }

    /// Picks up a finished count and puts it in front of the user.
    pub(super) fn poll_count(&mut self) {
        let Some(slot) = &self.counting else {
            return;
        };
        let landed = slot.lock().unwrap().clone();
        let Some(landed) = landed else {
            return;
        };
        self.counting = None;
        let Some(properties) = &mut self.properties else {
            return;
        };
        let Some(edit) = &mut properties.editable else {
            return;
        };
        match landed {
            Ok(count) => edit.counted = Some(count),
            Err(e) => {
                self.error = Some(e);
                edit.counted = None;
            }
        }
    }

    /// Applies the mode, and the ownership if one was given.
    pub(super) fn apply_permissions(&mut self, properties: &Properties) {
        let Some(edit) = &properties.editable else {
            return;
        };
        let (mode, recursive) = (edit.mode, edit.recursive);
        let (owner, group) = (edit.owner.clone(), edit.group.clone());
        let (progress, outcome) = self.simple_progress("修改权限");
        let wake = self.wake.clone();
        match &edit.target {
            PermTarget::Local(path) => {
                let path = path.clone();
                std::thread::spawn(move || {
                    let result = apply_local_mode(&path, mode, recursive, &progress, &wake);
                    *outcome.lock().unwrap() = Some(result.map_err(|e| format!("{e:#}")));
                    wake();
                });
            }
            PermTarget::Remote(target) => {
                let path = target.path.clone();
                let connection = self.connection.clone();
                remote::runtime().spawn(async move {
                    let result: anyhow::Result<String> = async {
                        let uid = match owner.trim() {
                            "" => None,
                            name => Some(remote::resolve_uid(&connection, name).await?),
                        };
                        let gid = match group.trim() {
                            "" => None,
                            name => Some(remote::resolve_gid(&connection, name).await?),
                        };
                        let sftp = connection.sftp("修改权限").await?;
                        let paths = if recursive {
                            remote::walk_remote(&sftp, &path).await?
                        } else {
                            vec![path.clone()]
                        };
                        let total = paths.len().max(1);
                        // Hoisted out of the loop: `||` cannot be mixed into a
                        // let-chain, and nesting the two conditions only to satisfy
                        // clippy would read worse than naming the question once.
                        let owner_wanted = uid.is_some() || gid.is_some();
                        // A refusal of the ownership half is recorded, not raised:
                        // the mode change is the part most servers allow, and losing
                        // it because the user is not root would be the wrong trade.
                        let mut refused = None;
                        for (index, target) in paths.iter().enumerate() {
                            remote::set_permissions(&sftp, target, mode).await?;
                            if owner_wanted
                                && let Err(e) = remote::set_owner(&sftp, target, uid, gid).await
                            {
                                refused.get_or_insert(format!("{e:#}"));
                            }
                            if let Ok(mut state) = progress.lock() {
                                state.done = index as u64 + 1;
                                state.total = total as u64;
                                state.message = "修改中".into();
                            }
                            wake();
                        }
                        Ok(match refused {
                            Some(e) => format!("权限已修改，所有者未修改：{e}"),
                            None if recursive => format!("已修改 {} 个条目", paths.len()),
                            None => "权限已修改".into(),
                        })
                    }
                    .await;
                    *outcome.lock().unwrap() = Some(result.map_err(|e| format!("{e:#}")));
                    wake();
                });
            }
        }
    }

    /// The 属性 / 确认删除 / 重命名 / 覆盖冲突 windows.
    pub(super) fn dialogs(&mut self, ctx: &egui::Context, p: Palette) {
        self.show_conflict(ctx, p);
        // Taken out of `self` rather than cloned, so the permission editor can
        // write back what the user typed, and put back unless the window closed.
        if let Some(mut properties) = self.properties.take() {
            let mut open = true;
            let mut apply = false;
            let mut count = false;
            egui::Window::new("属性")
                .collapsible(false)
                .resizable(false)
                .default_width(460.0)
                .order(egui::Order::Foreground)
                .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
                .open(&mut open)
                .show(ctx, |ui| {
                    egui::Grid::new("properties")
                        .num_columns(2)
                        .spacing([12.0, 6.0])
                        .min_col_width(56.0)
                        // Cap the value column: without it the grid sizes to the
                        // longest value, so a deep path makes the window as wide as
                        // the path is long.
                        .max_col_width(320.0)
                        .show(ui, |ui| {
                            for (label, value) in [
                                ("名称", properties.name.clone()),
                                ("路径", properties.path.clone()),
                                ("类型", properties.kind.to_string()),
                                (
                                    "大小",
                                    properties.size.map_or_else(|| "—".to_string(), format_size),
                                ),
                                (
                                    "修改时间",
                                    properties.mtime.clone().unwrap_or_else(|| "—".into()),
                                ),
                            ] {
                                ui.label(hint(label, p));
                                // Wrapped rather than allowed to stretch the row.
                                ui.add(egui::Label::new(value).wrap());
                                ui.end_row();
                            }
                        });
                    if let Some(edit) = &mut properties.editable {
                        ui.separator();
                        let (asked_apply, asked_count) = permission_editor(ui, edit, p);
                        apply |= asked_apply;
                        count |= asked_count;
                    } else if let Some(perms) = &properties.perms {
                        ui.label(hint(&format!("权限：{perms}"), p));
                    }
                });
            // A recursive change is counted first, so the confirmation can say how
            // much it covers. The walk is the same one the change would do.
            if count {
                self.count_permissions(&mut properties);
            }
            if apply {
                self.apply_permissions(&properties);
            }
            if open {
                self.properties = Some(properties);
            }
        }

        if let Some(rename) = &mut self.renaming {
            let mut open = true;
            let mut submit = false;
            egui::Window::new("重命名")
                .collapsible(false)
                .resizable(false)
                .default_width(360.0)
                .order(egui::Order::Foreground)
                .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
                .show(ctx, |ui| {
                    let edit =
                        editing::field_with(ui, &mut rename.name, |edit| edit.desired_width(320.0));
                    edit.request_focus();
                    let valid = !rename.name.is_empty()
                        && !rename.name.contains(['/', '\\'])
                        && rename.name != "..";
                    if !valid {
                        ui.colored_label(p.danger, "名称不能为空，也不能包含斜杠或 ..");
                    }
                    ui.horizontal(|ui| {
                        if ui.add_enabled(valid, egui::Button::new("确定")).clicked()
                            || (valid
                                && edit.lost_focus()
                                && !editing::ime_composing(ui.ctx())
                                && ui.input(|i| i.key_pressed(egui::Key::Enter)))
                        {
                            submit = true;
                        }
                        if ui.button("取消").clicked() {
                            open = false;
                        }
                    });
                });
            if submit {
                let taken = self.renaming.take();
                if let Some(mut rename) = taken {
                    let name = std::mem::take(&mut rename.name);
                    if let Some(path) = rename.local {
                        rename_local(&path, &name, &mut self.error, &mut self.notice);
                    } else if let Some(target) = rename.remote {
                        let to = join_path(&parent_path(&target.path), &name);
                        let loaded = self.simple_operation();
                        spawn_rename_remote(self.connection.clone(), target, to, loaded, ctx);
                    }
                }
                self.refresh_local();
                self.refresh_remote(ctx);
            } else if !open {
                self.renaming = None;
            }
        }

        if let Some(confirm) = &self.confirm {
            let message = confirm.message.clone();
            let mut answer = None;
            egui::Window::new("确认删除")
                .collapsible(false)
                .resizable(false)
                .default_width(400.0)
                .order(egui::Order::Foreground)
                .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
                .show(ctx, |ui| {
                    ui.add(egui::Label::new(RichText::new(message).color(p.warn)).wrap());
                    ui.add_space(4.0);
                    ui.horizontal(|ui| {
                        if ui.button("删除").clicked() {
                            answer = Some(true);
                        }
                        if ui.button("取消").clicked() {
                            answer = Some(false);
                        }
                    });
                });
            match answer {
                Some(true) => {
                    let action = self.confirm.take().map(|c| c.action);
                    match action {
                        Some(ConfirmAction::DeleteLocal(path)) => self.delete_local(path),
                        Some(ConfirmAction::DeleteRemote(target)) => {
                            self.delete_remote(target, ctx)
                        }
                        None => {}
                    }
                }
                Some(false) => self.confirm = None,
                None => {}
            }
        }

        // A directory delete waits here for the server to report whether the
        // directory holds anything, which decides if it needs confirming.
        if let Some(target) = self.deleting.clone() {
            let probe = self.operation.as_ref().and_then(Operation::outcome);
            match probe {
                Some(Ok(verdict)) => {
                    self.deleting = None;
                    self.operation = None;
                    if verdict.is_empty() {
                        self.delete_remote(target, ctx);
                    } else {
                        self.confirm = Some(Confirm {
                            message: format!(
                                "目录 {} 非空，将连同其中所有内容一并删除。",
                                target.entry.name
                            ),
                            action: ConfirmAction::DeleteRemote(target),
                        });
                    }
                }
                Some(Err(e)) => {
                    self.deleting = None;
                    self.operation = None;
                    self.error = Some(e);
                }
                None => {}
            }
        }
    }
}
