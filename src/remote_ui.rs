use crate::theme::Palette;
use eframe::egui::{self, RichText};
use g_terminal::{
    config::RemoteProfile,
    remote::{self, ConnectJob, Connection, Credentials, Direction, Transfer},
};
use std::{
    path::PathBuf,
    sync::{Arc, Mutex},
};

pub fn wake(ctx: &egui::Context) -> remote::Wake {
    let ctx = ctx.clone();
    Arc::new(move || ctx.request_repaint())
}
pub fn hint(text: &str, p: Palette) -> RichText {
    RichText::new(text).color(p.muted)
}

pub struct Login {
    pub profile: RemoteProfile,
    credentials: Credentials,
    job: Option<ConnectJob>,
    error: Option<String>,
}
impl Login {
    pub fn new(profile: RemoteProfile) -> Self {
        Self {
            profile,
            credentials: Credentials::default(),
            job: None,
            error: None,
        }
    }
    pub fn show(
        &mut self,
        ctx: &egui::Context,
        open: &mut bool,
        p: Palette,
    ) -> Option<Arc<Connection>> {
        let mut ready = None;
        egui::Window::new(format!("SSH · {}", self.profile.label()))
            .open(open)
            .collapsible(false)
            .resizable(false)
            .default_width(410.0)
            .show(ctx, |ui| {
                ui.label(format!(
                    "{}:{}",
                    self.profile.destination(),
                    self.profile.port
                ));
                if let Some(job) = &self.job {
                    let mut state = job.state.lock().unwrap();
                    if let Some(connection) = state.ready.take() {
                        ready = Some(connection);
                    }
                    if let Some(error) = &state.error {
                        self.error = Some(error.clone());
                    }
                    if let Some(trust) = &mut state.trust {
                        ui.colored_label(p.accent, "首次连接：确认服务器公钥指纹");
                        ui.label(&trust.host);
                        ui.monospace(&trust.fingerprint);
                        ui.label(hint(
                            "核对指纹后将保存到 known_hosts。主机密钥变化时拒绝连接。",
                            p,
                        ));
                        ui.horizontal(|ui| {
                            if ui.button("信任并继续").clicked()
                                && let Some(answer) = trust.answer.take()
                            {
                                let _ = answer.send(true);
                            }
                            if ui.button("拒绝").clicked()
                                && let Some(answer) = trust.answer.take()
                            {
                                let _ = answer.send(false);
                            }
                        });
                    } else if state.error.is_none() {
                        ui.horizontal(|ui| {
                            ui.spinner();
                            ui.label(&state.message);
                        });
                    }
                }
                if self.job.is_none() || self.error.is_some() {
                    egui::Grid::new("credentials")
                        .num_columns(2)
                        .show(ui, |ui| {
                            ui.label("用户名");
                            ui.add(
                                egui::TextEdit::singleline(&mut self.profile.user)
                                    .hint_text(hint("系统用户名", p)),
                            );
                            ui.end_row();
                            ui.label("密码");
                            ui.add(
                                egui::TextEdit::singleline(&mut *self.credentials.password)
                                    .password(true)
                                    .hint_text(hint("仅保留在内存中", p)),
                            );
                            ui.end_row();
                            ui.label("私钥路径");
                            ui.add(
                                egui::TextEdit::singleline(&mut self.profile.identity)
                                    .hint_text(hint("可选，完整路径", p)),
                            );
                            ui.end_row();
                            ui.label("私钥口令");
                            ui.add(
                                egui::TextEdit::singleline(&mut *self.credentials.passphrase)
                                    .password(true)
                                    .hint_text(hint("未加密私钥可留空", p)),
                            );
                            ui.end_row();
                            if self.profile.jump.is_some() {
                                ui.label("跳板机密码");
                                ui.add(
                                    egui::TextEdit::singleline(
                                        &mut *self.credentials.jump_password,
                                    )
                                    .password(true),
                                );
                                ui.end_row();
                            }
                        });
                    if let Some(error) = &self.error {
                        ui.colored_label(egui::Color32::LIGHT_RED, error);
                    }
                    if ui.button("连接").clicked() {
                        self.error = None;
                        self.job = Some(remote::connect(
                            self.profile.clone(),
                            std::mem::take(&mut self.credentials),
                            wake(ctx),
                        ));
                    }
                }
            });
        ready
    }
}

type Operation = Arc<Mutex<Option<Result<String, String>>>>;
pub struct Files {
    pub connection: Arc<Connection>,
    directory: Arc<Mutex<remote::DirectoryState>>,
    remote_path: String,
    local_path: String,
    selected_local: Option<PathBuf>,
    selected_remote: Option<remote::Entry>,
    local_entries: Vec<(String, bool, u64, bool)>,
    name: String,
    error: Option<String>,
    pub transfers: Vec<Transfer>,
    operation: Option<Operation>,
    sudo_destination: String,
    split_ratio: f32,
    show_hidden: bool,
}
impl Files {
    pub fn new(
        connection: Arc<Connection>,
        ctx: &egui::Context,
        hide_dotfiles: bool,
    ) -> Self {
        let local_path = directories::UserDirs::new()
            .map(|d| d.home_dir().display().to_string())
            .unwrap_or_else(|| ".".into());
        let mut this = Self {
            directory: remote::list_directory(connection.clone(), ".".into(), wake(ctx)),
            connection,
            remote_path: ".".into(),
            local_path,
            selected_local: None,
            selected_remote: None,
            local_entries: vec![],
            name: String::new(),
            error: None,
            transfers: vec![],
            operation: None,
            sudo_destination: String::new(),
            split_ratio: 0.5,
            show_hidden: !hide_dotfiles,
        };
        this.refresh_local();
        this
    }
    fn refresh_local(&mut self) {
        self.local_entries.clear();
        self.selected_local = None;
        match std::fs::read_dir(&self.local_path) {
            Ok(entries) => {
                self.local_entries = entries
                    .filter_map(Result::ok)
                    .map(|e| {
                        let metadata = e.metadata().ok();
                        let name = e.file_name().to_string_lossy().into_owned();
                        let symlink = e.file_type().ok().is_some_and(|t| t.is_symlink());
                        (
                            name,
                            metadata.as_ref().is_some_and(|m| m.is_dir()),
                            metadata.map(|m| m.len()).unwrap_or(0),
                            symlink,
                        )
                    })
                    .collect();
                self.local_entries.sort_by(|a, b| {
                    b.1.cmp(&a.1)
                        .then_with(|| a.0.to_lowercase().cmp(&b.0.to_lowercase()))
                });
            }
            Err(e) => self.error = Some(e.to_string()),
        }
    }
    fn refresh_remote(&mut self, ctx: &egui::Context) {
        self.directory =
            remote::list_directory(self.connection.clone(), self.remote_path.clone(), wake(ctx));
        self.selected_remote = None;
    }
    fn remote_child(&self, name: &str) -> String {
        format!(
            "{}/{}",
            self.directory.lock().unwrap().path.trim_end_matches('/'),
            name
        )
    }

    fn start_zmodem(&mut self, upload: bool, ctx: &egui::Context) {
        let (local, remote) = if upload {
            let Some(local) = self.selected_local.clone().filter(|p| p.is_file()) else {
                self.error = Some("请选择本地文件".into());
                return;
            };
            (local, self.directory.lock().unwrap().path.clone())
        } else {
            let Some(entry) = self.selected_remote.clone().filter(|e| !e.directory) else {
                self.error = Some("请选择远程文件".into());
                return;
            };
            if !remote::safe_local_name(&entry.name) {
                self.error = Some("非法文件名".into());
                return;
            }
            (
                PathBuf::from(&self.local_path).join(&entry.name),
                self.remote_child(&entry.name),
            )
        };
        let state = Arc::new(Mutex::new(None));
        self.operation = Some(state.clone());
        let connection = self.connection.clone();
        let wake = wake(ctx);
        remote::runtime().spawn(async move {
            let result: anyhow::Result<String> = async {
                let _permit = connection.transfer_gate.clone().acquire_owned().await?;
                let command = if upload {
                    format!(
                        "cd {} && rz --binary --protect",
                        remote::quote_posix(&remote)?
                    )
                } else {
                    format!("sz --binary -- {}", remote::quote_posix(&remote)?)
                };
                let channel = connection.handle.channel_open_session().await?;
                channel.exec(true, command).await?;
                let mut stream = channel.into_stream();
                if upload {
                    g_terminal::ztransfer::send(&mut stream, &local).await?;
                } else {
                    g_terminal::ztransfer::receive(&mut stream, &local).await?;
                }
                Ok("ZMODEM 完成".into())
            }
            .await;
            *state.lock().unwrap() = Some(result.map_err(|e| format!("{e:#}")));
            wake();
        });
    }
    pub fn start_zmodem_from_terminal(&mut self, ctx: &egui::Context) {
        let state = Arc::new(Mutex::new(None));
        self.operation = Some(state.clone());
        let connection = self.connection.clone();
        let local = PathBuf::from(&self.local_path);
        let wake = wake(ctx);
        remote::runtime().spawn(async move {
            let result: anyhow::Result<String> = async {
                let _permit = connection.transfer_gate.clone().acquire_owned().await?;
                let channel = connection.handle.channel_open_session().await?;
                channel.exec(true, "rz --binary --protect").await?;
                let mut stream = channel.into_stream();
                g_terminal::ztransfer::receive(&mut stream, &local).await?;
                Ok("ZMODEM 接收完成".into())
            }
            .await;
            *state.lock().unwrap() = Some(result.map_err(|e| format!("{e:#}")));
            wake();
        });
    }
    fn queue(&mut self, local: PathBuf, remote: String, direction: Direction, ctx: &egui::Context) {
        if self
            .transfers
            .iter()
            .any(|t| t.local == local && t.remote == remote && !t.state.lock().unwrap().finished)
        {
            self.error = Some("该文件已在队列中，请使用继续按钮".into());
            return;
        }
        self.transfers.push(Transfer::new(
            self.connection.clone(),
            local,
            remote,
            direction,
            wake(ctx),
        ));
    }
    fn entry_color(
        directory: bool,
        symlink: bool,
        executable: bool,
        p: Palette,
    ) -> eframe::egui::Color32 {
        if symlink {
            eframe::egui::Color32::from_rgb(230, 160, 60)
        } else if directory {
            p.accent
        } else if executable {
            eframe::egui::Color32::from_rgb(90, 160, 250)
        } else {
            p.text
        }
    }
    fn perms_string(perms: Option<u32>) -> String {
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
    fn show_local_pane(
        &mut self,
        ui: &mut egui::Ui,
        p: Palette,
        next: &mut Option<String>,
    ) {
        ui.horizontal(|ui| {
            if ui.button("↑").clicked() {
                *next = PathBuf::from(&self.local_path)
                    .parent()
                    .map(|p| p.display().to_string());
            }
            let edit = ui.add(
                egui::TextEdit::singleline(&mut self.local_path)
                    .desired_width((ui.available_width() - 45.0).max(100.0)),
            );
            if ui.button("刷新").clicked()
                || (edit.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)))
            {
                *next = Some(self.local_path.clone());
            }
        });
        egui::ScrollArea::vertical()
            .id_salt("local-files")
            .max_height(280.0)
            .show(ui, |ui| {
                for (name, dir, size, symlink) in &self.local_entries {
                    if !self.show_hidden && name.starts_with('.') {
                        continue;
                    }
                    let path = PathBuf::from(&self.local_path).join(name);
                    let body = if *dir {
                        name.clone()
                    } else {
                        format!("{name}   {}", format_size(*size))
                    };
                    let label = RichText::new(format!(
                        "{} {}",
                        if *symlink { "[→]" } else if *dir { "[+]" } else { "   " },
                        body
                    ))
                    .color(Self::entry_color(*dir, *symlink, false, p));
                    let r = ui.selectable_label(
                        self.selected_local.as_ref() == Some(&path),
                        label,
                    );
                    if r.clicked() {
                        self.selected_local = Some(path.clone());
                    }
                    if r.double_clicked() && *dir {
                        *next = Some(path.display().to_string());
                    }
                }
            });
    }
    fn show_remote_pane(
        &mut self,
        ui: &mut egui::Ui,
        p: Palette,
        next: &mut Option<String>,
    ) {
        ui.horizontal(|ui| {
            if ui.button("↑").clicked() {
                *next = Some(format!("{}/..", self.directory.lock().unwrap().path));
            }
            let edit = ui.add(
                egui::TextEdit::singleline(&mut self.remote_path)
                    .desired_width((ui.available_width() - 45.0).max(100.0)),
            );
            if ui.button("刷新").clicked()
                || (edit.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)))
            {
                *next = Some(self.remote_path.clone());
            }
        });
        let state = self.directory.lock().unwrap();
        if state.loading {
            ui.spinner();
        }
        if let Some(e) = &state.error {
            ui.colored_label(egui::Color32::LIGHT_RED, e);
        }
        egui::ScrollArea::vertical()
            .id_salt("remote-files")
            .max_height(280.0)
            .show(ui, |ui| {
                for entry in &state.entries {
                    if !self.show_hidden && entry.name.starts_with('.') {
                        continue;
                    }
                    let executable = entry.perms.is_some_and(|m| m & 0o111 != 0);
                    let perms = Self::perms_string(entry.perms);
                    let body = if entry.directory {
                        entry.name.clone()
                    } else {
                        format!("{}   {}", entry.name, format_size(entry.size))
                    };
                    let label = RichText::new(format!(
                        "{} {}{}",
                        if entry.directory { "[+]" } else { "   " },
                        body,
                        if perms.is_empty() {
                            String::new()
                        } else {
                            format!("  {perms}")
                        }
                    ))
                    .color(Self::entry_color(entry.directory, false, executable, p));
                    let r = ui.selectable_label(
                        self.selected_remote
                            .as_ref()
                            .is_some_and(|e| e.name == entry.name),
                        label,
                    );
                    if r.clicked() {
                        self.selected_remote = Some(entry.clone());
                        self.name = entry.name.clone();
                    }
                    if r.double_clicked() && entry.directory {
                        *next = Some(format!(
                            "{}/{}",
                            state.path.trim_end_matches('/'),
                            entry.name
                        ));
                    }
                }
            });
    }
    pub fn show(&mut self, ctx: &egui::Context, open: &mut bool, p: Palette, hide_dotfiles: bool) {
        self.show_hidden = self.show_hidden || !hide_dotfiles;
        egui::Window::new(format!("文件 · {}", self.connection.profile.label()))
            .open(open)
            .default_size([760.0, 520.0])
            .min_width(560.0)
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.label(RichText::new("SFTP").color(p.accent));
                    ui.label(hint(
                        "双击目录进入 · 文件以 .gterminal.part 续传 · 不自动覆盖同名目标",
                        p,
                    ));
                    if ui
                        .checkbox(&mut self.show_hidden, "显示隐藏文件")
                        .changed()
                    {
                        self.refresh_local();
                    }
                });
                let mut local_next = None;
                let mut remote_next = None;
                let available = ui.available_width();
                let left_width = (available * self.split_ratio.clamp(0.25, 0.75)).round();
                ui.horizontal(|ui| {
                    let mut local_rect = ui.available_rect_before_wrap();
                    local_rect.max.x = local_rect.min.x + left_width;
                    ui.scope_builder(egui::UiBuilder::new().max_rect(local_rect), |ui| {
                        self.show_local_pane(ui, p, &mut local_next)
                    });
                    let mut divider = local_rect;
                    divider.min.x = divider.max.x - 3.0;
                    let drag = ui.interact(divider, ui.id().with("files-split"), egui::Sense::drag());
                    if drag.dragged()
                        && let Some(pos) = drag.interact_pointer_pos()
                    {
                        self.split_ratio =
                            ((pos.x - local_rect.min.x) / available).clamp(0.25, 0.75);
                    }
                    drag.on_hover_cursor(egui::CursorIcon::ResizeHorizontal);
                    let remote_rect = ui.available_rect_before_wrap();
                    ui.scope_builder(egui::UiBuilder::new().max_rect(remote_rect), |ui| {
                        self.show_remote_pane(ui, p, &mut remote_next)
                    });
                });
                if let Some(path) = local_next {
                    self.local_path = path;
                    self.refresh_local();
                }
                if let Some(path) = remote_next {
                    self.remote_path = path;
                    self.refresh_remote(ctx);
                }
                ui.separator();
                ui.horizontal(|ui| {
                    if ui
                        .add_enabled(
                            self.selected_local.as_ref().is_some_and(|p| p.is_file()),
                            egui::Button::new("上传 →"),
                        )
                        .clicked()
                        && let Some(path) = self.selected_local.clone()
                    {
                        let remote =
                            self.remote_child(&path.file_name().unwrap().to_string_lossy());
                        self.queue(path, remote, Direction::Upload, ctx);
                    }
                    if ui
                        .add_enabled(
                            self.selected_remote.as_ref().is_some_and(|e| !e.directory),
                            egui::Button::new("← 下载"),
                        )
                        .clicked()
                        && let Some(e) = self.selected_remote.clone()
                    {
                        if !remote::safe_local_name(&e.name) {
                            self.error = Some("服务器返回了非法文件名".into());
                        } else {
                            self.queue(
                                PathBuf::from(&self.local_path).join(&e.name),
                                self.remote_child(&e.name),
                                Direction::Download,
                                ctx,
                            );
                        }
                    }
                    ui.separator();
                    ui.add(
                        egui::TextEdit::singleline(&mut self.name)
                            .desired_width(150.0)
                            .hint_text(hint("新目录 / 新文件名", p)),
                    );
                    let mut op = None;
                    if ui.button("建目录").clicked() {
                        op = Some(0);
                    }
                    if ui.button("重命名").clicked() {
                        op = Some(1);
                    }
                    if let Some(op) = op {
                        if self.name.is_empty()
                            || self.name.contains(['/', '\\'])
                            || self.name == ".."
                        {
                            self.error = Some("请输入有效文件名".into());
                        } else {
                            let connection = self.connection.clone();
                            let target = self.remote_child(&self.name);
                            let source = self
                                .selected_remote
                                .as_ref()
                                .map(|e| self.remote_child(&e.name));
                            let state = Arc::new(Mutex::new(None));
                            self.operation = Some(state.clone());
                            let wake = wake(ctx);
                            remote::runtime().spawn(async move {
                                let result: anyhow::Result<()> = async {
                                    let sftp = connection.sftp.clone().ok_or_else(|| {
                                        anyhow::anyhow!("服务器未提供 SFTP 子系统")
                                    })?;
                                    if op == 0 {
                                        sftp.create_dir(target).await?;
                                    } else if let Some(source) = source {
                                        sftp.rename(source, target).await?;
                                    } else {
                                        anyhow::bail!("请先选择远程文件");
                                    }
                                    Ok(())
                                }
                                .await;
                                let result = result
                                    .map(|_| "操作完成，请刷新".to_string())
                                    .map_err(|e| e.to_string());
                                *state.lock().unwrap() = Some(result);
                                wake();
                            });
                        }
                    }
                });
                ui.horizontal(|ui| {
                    ui.label(hint("sudo 安装已上传文件", p));
                    ui.add(
                        egui::TextEdit::singleline(&mut self.sudo_destination)
                            .desired_width(230.0)
                            .hint_text(hint("完整目标路径（目标须不存在）", p)),
                    );
                    if ui
                        .button("sudo -n 安装")
                        .on_hover_text("需要服务器允许免密 sudo；不会保存或注入口令")
                        .clicked()
                        && let Some(entry) = &self.selected_remote
                    {
                        let source = self.remote_child(&entry.name);
                        let target = self.sudo_destination.clone();
                        let state = Arc::new(Mutex::new(None));
                        self.operation = Some(state.clone());
                        let connection = self.connection.clone();
                        let wake = wake(ctx);
                        remote::runtime().spawn(async move {
                            let result: anyhow::Result<String> = async {
                                anyhow::ensure!(!target.is_empty(), "请输入目标路径");
                                let script = format!(
                                    "test ! -e {target} && cp --no-clobber -- {source} {target}",
                                    target = remote::quote_posix(&target)?,
                                    source = remote::quote_posix(&source)?
                                );
                                let command =
                                    format!("sudo -n sh -c {}", remote::quote_posix(&script)?);
                                let mut ch = connection.handle.channel_open_session().await?;
                                ch.exec(true, command).await?;
                                let mut output = String::new();
                                let mut code = None;
                                while let Some(msg) = ch.wait().await {
                                    match msg {
                                        russh::ChannelMsg::Data { data }
                                        | russh::ChannelMsg::ExtendedData { data, .. } => {
                                            if output.len() < 16_384 {
                                                output.push_str(&String::from_utf8_lossy(&data));
                                            }
                                        }
                                        russh::ChannelMsg::ExitStatus { exit_status } => {
                                            code = Some(exit_status)
                                        }
                                        _ => {}
                                    }
                                }
                                anyhow::ensure!(
                                    code == Some(0),
                                    "sudo 失败（需免密权限且目标不存在）：{output}"
                                );
                                Ok("sudo 安装完成".into())
                            }
                            .await;
                            *state.lock().unwrap() = Some(result.map_err(|e| format!("{e:#}")));
                            wake();
                        });
                    }
                });
                if let Some(operation) = &self.operation
                    && let Some(result) = &*operation.lock().unwrap()
                {
                    match result {
                        Ok(message) => {
                            ui.colored_label(p.accent, message);
                        }
                        Err(e) => {
                            ui.colored_label(egui::Color32::LIGHT_RED, e);
                        }
                    }
                }
                if let Some(error) = &self.error {
                    ui.colored_label(egui::Color32::LIGHT_RED, error);
                }
                ui.separator();
                ui.horizontal(|ui| {
                    ui.label(hint("ZMODEM（服务器需安装 rz / sz）", p));
                    if ui.button("上传选中文件").clicked() {
                        self.start_zmodem(true, ctx);
                    }
                    if ui.button("下载选中文件").clicked() {
                        self.start_zmodem(false, ctx);
                    }
                });
                ui.label("传输队列");
                egui::ScrollArea::vertical()
                    .id_salt("transfers")
                    .max_height(150.0)
                    .show(ui, |ui| {
                        for transfer in &self.transfers {
                            let s = transfer.state.lock().unwrap().clone();
                            ui.horizontal(|ui| {
                                ui.label(format!(
                                    "{} {}",
                                    if transfer.direction == Direction::Upload {
                                        "↑"
                                    } else {
                                        "↓"
                                    },
                                    transfer
                                        .local
                                        .file_name()
                                        .unwrap_or_default()
                                        .to_string_lossy()
                                ));
                                ui.add(
                                    egui::ProgressBar::new(if s.total == 0 {
                                        0.0
                                    } else {
                                        s.done as f32 / s.total as f32
                                    })
                                    .desired_width(120.0),
                                );
                                ui.label(format!(
                                    "{} / {} · {}",
                                    format_size(s.done),
                                    format_size(s.total),
                                    s.message
                                ));
                                if s.running && ui.small_button("暂停").clicked() {
                                    transfer
                                        .pause
                                        .store(true, std::sync::atomic::Ordering::Release);
                                }
                                if !s.running && !s.finished && ui.small_button("继续").clicked()
                                {
                                    transfer.start(wake(ctx));
                                }
                            });
                        }
                    });
            });
    }
}
fn format_size(n: u64) -> String {
    if n >= 1024 * 1024 {
        format!("{:.1} MiB", n as f64 / 1048576.0)
    } else if n >= 1024 {
        format!("{:.1} KiB", n as f64 / 1024.0)
    } else {
        format!("{n} B")
    }
}
