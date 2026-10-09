//! The SSH login form: credentials, the passwordless first attempt, and the
//! key-only fallback.

use super::*;

pub struct Login {
    pub profile: RemoteProfile,
    credentials: Credentials,
    job: Option<ConnectJob>,
    error: Option<String>,
    /// The passwordless first attempt has not been made yet. Running it here is
    /// what lets a key-only server connect without a click; if it fails, the
    /// credentials simply appear.
    auto: bool,
    /// The current failure came from that automatic attempt, so it is reported as
    /// "needs a password" rather than as an error — needing one is the normal
    /// case, not a fault.
    quiet: bool,
    /// The pane console this connection's failures are mirrored into, so a
    /// message that outlives the dialog still says which session it belongs to.
    console: Option<Arc<Mutex<Terminal>>>,
    /// The last failure already mirrored there.
    noted: Option<String>,
    focus_username: bool,
    focus_cancel: bool,
    focus_trust: bool,
}
impl Login {
    pub fn new(profile: RemoteProfile, console: Option<Arc<Mutex<Terminal>>>) -> Self {
        Self {
            profile,
            credentials: Credentials::default(),
            job: None,
            error: None,
            auto: true,
            quiet: false,
            console,
            noted: None,
            focus_username: true,
            focus_cancel: true,
            focus_trust: true,
        }
    }
    pub fn show(
        &mut self,
        ctx: &egui::Context,
        open: &mut bool,
        p: Palette,
    ) -> Option<Arc<Connection>> {
        // Try once without credentials, so a server that needs none connects on
        // the first click. `ctx` is only in hand here, which is why this is not
        // done in `new`.
        if self.auto {
            self.auto = false;
            self.quiet = self.credentials.password.is_empty();
            self.job = Some(remote::connect(
                self.profile.clone(),
                std::mem::take(&mut self.credentials),
                wake(ctx),
            ));
        }
        let mut ready = None;
        let mut abort = false;
        let mut cancel = false;
        crate::dialog::Dialog::new(
            format!("SSH · {}", self.profile.label()),
            crate::icons::Icon::Connect,
            p,
        )
        .open(open)
        .collapsible(false)
        .resizable(false)
        .default_width(420.0)
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
                    // Mirror a real failure into the pane. The passwordless
                    // first attempt failing just means credentials are
                    // needed, which is not an error worth writing down, and
                    // `noted` keeps one failure from being written twice.
                    if !self.quiet && self.noted.as_deref() != Some(error.as_str()) {
                        self.noted = Some(error.clone());
                        if let Some(console) = &self.console {
                            console
                                .lock()
                                .unwrap_or_else(|e| e.into_inner())
                                .note_error(&format!("[错误] connect failed: {error}"));
                        }
                    }
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
                        let reject = ui.button("拒绝");
                        editing::focus_once(ui, &reject, &mut self.focus_trust);
                        if reject.clicked()
                            && let Some(answer) = trust.answer.take()
                        {
                            let _ = answer.send(false);
                        }
                    });
                } else if state.error.is_none() {
                    // A connect can take seconds, and the spinner only turns if
                    // something keeps asking for repaints.
                    ctx.request_repaint_after(std::time::Duration::from_millis(150));
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label(&state.message);
                    });
                    let cancel = ui.small_button("取消");
                    editing::focus_once(ui, &cancel, &mut self.focus_cancel);
                    if cancel.clicked() {
                        abort = true;
                    }
                }
            }
            if self.job.is_none() || self.error.is_some() {
                crate::dialog::card(
                    ui,
                    p,
                    crate::icons::Icon::Host,
                    "身份验证",
                    "使用账号密码或 SSH 私钥登录",
                    |ui| {
                        egui::Grid::new("credentials")
                            .num_columns(2)
                            .spacing([12.0, 6.0])
                            .min_col_width(72.0)
                            .show(ui, |ui| {
                                ui.label("用户名");
                                let username =
                                    editing::field_with(ui, &mut self.profile.user, |edit| {
                                        edit.desired_width(230.0)
                                    });
                                editing::focus_once(ui, &username, &mut self.focus_username);
                                ui.end_row();
                                ui.label("密码");
                                editing::field_with(ui, &mut self.credentials.password, |edit| {
                                    edit.password(true).desired_width(230.0)
                                });
                                ui.end_row();
                                ui.label("私钥路径");
                                editing::field_with(ui, &mut self.profile.identity, |edit| {
                                    edit.desired_width(230.0)
                                });
                                ui.end_row();
                                ui.label("私钥口令");
                                editing::field_with(ui, &mut self.credentials.passphrase, |edit| {
                                    edit.password(true).desired_width(230.0)
                                });
                                ui.end_row();
                                if self.profile.jump.is_some() {
                                    ui.label("跳板机密码");
                                    editing::field_with(
                                        ui,
                                        &mut self.credentials.jump_password,
                                        |edit| edit.password(true).desired_width(230.0),
                                    );
                                    ui.end_row();
                                }
                            });
                    },
                );
                if self.quiet {
                    // The passwordless attempt failing usually just means the
                    // server wants credentials, so say so — but the failure
                    // beneath it is still an error and still reads as one.
                    ui.label(hint("需要密码或私钥", p));
                }
                if let Some(error) = &self.error {
                    ui.colored_label(p.danger, error);
                }
                // Enter submits, matching the other connection dialogs. A
                // combo/popup consumes Enter for itself first.
                let enter = !egui::Popup::is_any_open(ui.ctx())
                    && !editing::ime_composing(ui.ctx())
                    && ui.input(|i| i.key_pressed(egui::Key::Enter));
                let mut connect = false;
                crate::dialog::footer(ui, |ui| {
                    cancel = crate::dialog::secondary(ui, "取消").clicked();
                    connect = crate::dialog::primary(ui, "连接", p).clicked();
                });
                if !cancel && (connect || enter) {
                    self.focus_username = true;
                    self.focus_cancel = true;
                    self.focus_trust = true;
                    self.error = None;
                    self.quiet = false;
                    self.job = Some(remote::connect(
                        self.profile.clone(),
                        std::mem::take(&mut self.credentials),
                        wake(ctx),
                    ));
                }
            }
        });
        if abort {
            // Dropping the job aborts its task.
            self.job = None;
        }
        if cancel {
            *open = false;
        }
        ready
    }

    pub(crate) fn prepare_quick(
        &mut self,
        password: Option<zeroize::Zeroizing<String>>,
        prompt: bool,
    ) {
        self.credentials.password = password.unwrap_or_default();
        self.auto = !prompt;
    }
}
