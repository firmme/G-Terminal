//! The SSH login form: credentials, the passwordless first attempt, and the
//! key-only fallback.

use super::*;
use g_terminal::LockExt;

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
            crate::icons::Icon::Host,
            p,
        )
        .open(open)
        .collapsible(false)
        .resizable(false)
        .default_width(420.0)
        .show(ctx, |ui| {
            ui.horizontal(|ui| {
                let (rect, _) =
                    ui.allocate_exact_size(egui::vec2(16.0, 16.0), egui::Sense::hover());
                crate::icons::draw(
                    ui.painter(),
                    rect,
                    crate::icons::Icon::Connect,
                    p.muted,
                    1.3,
                );
                ui.label(hint(
                    &format!("{}:{}", self.profile.destination(), self.profile.port),
                    p,
                ));
            });
            ui.add_space(2.0);
            if let Some(job) = &self.job {
                let mut state = job.state.locked();
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
                                .locked()
                                .note_error(&format!("[错误] connect failed: {error}"));
                        }
                    }
                }
                if let Some(trust) = &mut state.trust {
                    egui::Frame::new()
                        .fill(p.warn.gamma_multiply(0.08))
                        .stroke(egui::Stroke::new(1.0_f32, p.warn.gamma_multiply(0.5)))
                        .corner_radius(8)
                        .inner_margin(12)
                        .show(ui, |ui| {
                            ui.set_min_width(ui.available_width());
                            ui.horizontal(|ui| {
                                let (rect, _) = ui.allocate_exact_size(
                                    egui::vec2(18.0, 18.0),
                                    egui::Sense::hover(),
                                );
                                crate::icons::draw(
                                    ui.painter(),
                                    rect,
                                    crate::icons::Icon::Key,
                                    p.warn,
                                    1.4,
                                );
                                ui.label(egui::RichText::new("首次连接，请核对主机指纹").strong());
                            });
                            ui.label(hint(&trust.host, p));
                            ui.add(
                                egui::Label::new(
                                    egui::RichText::new(&trust.fingerprint).monospace(),
                                )
                                .wrap(),
                            );
                            ui.label(hint(
                                "信任后写入 known_hosts；之后主机密钥一旦变化将拒绝连接。",
                                p,
                            ));
                        });
                    ui.add_space(6.0);
                    crate::dialog::footer(ui, |ui| {
                        let reject = crate::dialog::secondary(ui, "拒绝");
                        editing::focus_once(ui, &reject, &mut self.focus_trust);
                        if reject.clicked()
                            && let Some(answer) = trust.answer.take()
                        {
                            let _ = answer.send(false);
                        }
                        if crate::dialog::primary(ui, "信任并连接", p).clicked()
                            && let Some(answer) = trust.answer.take()
                        {
                            let _ = answer.send(true);
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
                    ui.add_space(4.0);
                    let cancel = crate::dialog::secondary(ui, "取消");
                    editing::focus_once(ui, &cancel, &mut self.focus_cancel);
                    if cancel.clicked() {
                        abort = true;
                    }
                }
            }
            if self.job.is_none() || self.error.is_some() {
                ui.spacing_mut().item_spacing.y = 8.0;
                let username = crate::dialog::icon_field(
                    ui,
                    crate::icons::Icon::User,
                    "用户名",
                    &mut self.profile.user,
                    false,
                    p,
                );
                editing::focus_once(ui, &username, &mut self.focus_username);
                crate::dialog::icon_field(
                    ui,
                    crate::icons::Icon::Lock,
                    "密码",
                    &mut self.credentials.password,
                    true,
                    p,
                );
                crate::dialog::private_key_field(
                    ui,
                    &mut self.profile.identity,
                    ui.available_width(),
                    p,
                );
                crate::dialog::icon_field(
                    ui,
                    crate::icons::Icon::Lock,
                    "私钥口令（可选）",
                    &mut self.credentials.passphrase,
                    true,
                    p,
                );
                if let Some(jump) = &mut self.profile.jump {
                    ui.label(hint("跳板机私钥", p));
                    crate::dialog::private_key_field(
                        ui,
                        &mut jump.identity,
                        ui.available_width(),
                        p,
                    );
                    crate::dialog::icon_field(
                        ui,
                        crate::icons::Icon::Host,
                        "跳板机密码",
                        &mut self.credentials.jump_password,
                        true,
                        p,
                    );
                }
                if self.quiet {
                    // The passwordless attempt failing usually just means the
                    // server wants credentials, so say so — but the failure
                    // beneath it is still an error and still reads as one.
                    ui.label(hint("需要密码或私钥", p));
                }
                if let Some(error) = &self.error {
                    ui.horizontal_wrapped(|ui| {
                        let (rect, _) =
                            ui.allocate_exact_size(egui::vec2(16.0, 16.0), egui::Sense::hover());
                        crate::icons::draw(
                            ui.painter(),
                            rect,
                            crate::icons::Icon::Warning,
                            p.danger,
                            1.3,
                        );
                        ui.colored_label(p.danger, error);
                    });
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
