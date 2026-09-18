//! Moving bytes to and from the remote: the file window, drag-and-drop, and
//! the ZMODEM stream a terminal-initiated transfer takes over.

use super::*;

impl App {
    pub(super) fn begin_terminal_zmodem(
        &mut self,
        upload: bool,
        local: std::path::PathBuf,
        connection: &Arc<Connection>,
        ctx: &egui::Context,
    ) {
        if let Some(files) = &self.files
            && !Arc::ptr_eq(&files.connection, connection)
            && files
                .transfers
                .iter()
                .any(|t| t.state.lock().unwrap().running)
        {
            self.error = Some("文件窗口还有其他连接的传输任务，请完成后再试".into());
            return;
        }
        if self.files.as_ref().is_some_and(Files::busy) {
            self.error = Some("文件窗口还有 ZMODEM 传输任务，请完成或取消后再试".into());
            return;
        }
        if self
            .files
            .as_ref()
            .is_none_or(|f| !Arc::ptr_eq(&f.connection, connection))
        {
            self.files = Some(Files::new(
                connection.clone(),
                ctx,
                self.settings.hide_dotfiles,
            ));
        }
        // The file window is not opened. A transfer started from the terminal has
        // nothing to do with the file browser, and the status bar already carries
        // the progress strip for it.
        let grab = self.tabs[self.active].panes[self.tabs[self.active].focused]
            .session
            .begin_terminal_zmodem();
        match grab {
            Ok(stream) => {
                if let Some(files) = self.files.as_mut() {
                    files.start_terminal_zmodem(stream, upload, local, ctx);
                }
            }
            Err(e) => self.error = Some(format!("{e:#}")),
        }
    }
    /// Opens (or switches to) the file window for the focused SSH connection.
    /// The menu and drag-and-drop both call this; on failure `self.error` says
    /// why.
    pub(super) fn ensure_files(&mut self, ctx: &egui::Context) -> bool {
        let Some(connection) = self
            .tabs
            .get(self.active)
            .and_then(|t| t.panes[t.focused].session.remote.clone())
        else {
            self.error = Some("请先连接内置 SSH 会话".into());
            return false;
        };
        if self
            .files
            .as_ref()
            .is_none_or(|f| !Arc::ptr_eq(&f.connection, &connection))
        {
            if self
                .files
                .as_ref()
                .is_some_and(|f| f.transfers.iter().any(|t| t.state.lock().unwrap().running))
            {
                self.error = Some("文件窗口还有传输任务，请完成或暂停后切换连接".into());
                return false;
            }
            self.files = Some(Files::new(connection, ctx, self.settings.hide_dotfiles));
        }
        self.files_open = true;
        true
    }

    /// Uploads files dropped onto the window through the file window's queue.
    pub(super) fn handle_dropped_files(&mut self, ctx: &egui::Context) {
        let dropped: Vec<std::path::PathBuf> = ctx.input(|i| {
            i.raw
                .dropped_files
                .iter()
                .filter_map(|file| file.path.clone())
                .collect()
        });
        if dropped.is_empty() {
            return;
        }
        if !self.ensure_files(ctx) {
            return;
        }
        if let Some(files) = self.files.as_mut() {
            for path in dropped {
                files.upload_path(path, ctx);
            }
        }
    }

    /// Dims the window and names the target while files are dragged over it.
    pub(super) fn drop_overlay(&self, ctx: &egui::Context, p: Palette) {
        let hovering = ctx.input(|i| i.raw.hovered_files.len());
        if hovering == 0 {
            return;
        }
        let directory = self
            .files
            .as_ref()
            .map(|files| files.remote_dir())
            .unwrap_or_default();
        let rect = ctx.content_rect();
        let painter = ctx.layer_painter(egui::LayerId::new(
            egui::Order::Foreground,
            egui::Id::new("drop-overlay"),
        ));
        painter.rect_filled(rect, 0, p.bg.gamma_multiply(0.72));
        painter.rect_stroke(
            rect.shrink(8.0),
            8.0,
            egui::Stroke::new(2.0_f32, p.accent),
            egui::StrokeKind::Inside,
        );
        let text = if directory.is_empty() {
            format!("松开上传 {hovering} 个文件（先连接 SSH 会话）")
        } else {
            format!("松开上传 {hovering} 个文件到 {directory}")
        };
        painter.text(
            rect.center(),
            egui::Align2::CENTER_CENTER,
            text,
            egui::FontId::proportional(16.0),
            p.text,
        );
    }

    /// A terminal-initiated `rz`/`sz` shows up as a flag on the terminal;
    /// the picker it opens is the confirmation.
    pub(super) fn poll_zmodem_offer(&mut self, ctx: &egui::Context) {
        if let Some(t) = self.tabs.get(self.active)
            && let Some(c) = t.panes[t.focused].session.remote.clone()
        {
            // The ZMODEM frame type decides the direction, so the old screen-text
            // guess ("waiting to receive") is gone — and with it the reason `rz`
            // never raised an offer at all.
            let offer = self.tabs[self.active].panes[self.tabs[self.active].focused]
                .session
                .terminal
                .lock()
                .unwrap()
                .zmodem_offer;
            if let Some(upload) = offer {
                // The picker opens straight away. The confirmation that used to sit
                // in front of it decided nothing: if you type `rz` or `sz` you have
                // already said what you want, and the picker's own cancel button is
                // the way to back out.
                self.tabs[self.active].panes[self.tabs[self.active].focused]
                    .session
                    .terminal
                    .lock()
                    .unwrap()
                    .zmodem_offer = None;
                let local = if upload {
                    rfd::FileDialog::new().pick_file()
                } else {
                    rfd::FileDialog::new().pick_folder()
                };
                match local {
                    Some(local) => self.begin_terminal_zmodem(upload, local, &c, ctx),
                    // Backing out of the picker is the only way left to refuse, so
                    // abort the lrzsz that is waiting on the other end.
                    None => {
                        let _ = self.tabs[self.active].panes[self.tabs[self.active].focused]
                            .session
                            .write(vec![0x18; 8]);
                    }
                }
            }
        }
    }
}
