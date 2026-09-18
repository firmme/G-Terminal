//! The transfer queue: what a pane sends or receives, how a batch is planned,
//! and how conflicts and progress are polled while it runs.

use super::*;

impl Files {
    pub fn start_terminal_zmodem(
        &mut self,
        stream: g_terminal::session::BridgeStream,
        upload: bool,
        local: PathBuf,
        ctx: &egui::Context,
    ) {
        let name = local
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "ZMODEM".into());
        let Some((handle, operation)) = self.begin(&name) else {
            return;
        };
        let wake = wake(ctx);
        remote::runtime().spawn(async move {
            let result: anyhow::Result<String> = async {
                let mut stream = stream;
                if upload {
                    g_terminal::ztransfer::send(&mut stream, &local, &handle).await?;
                    Ok("ZMODEM 上传完成".into())
                } else {
                    let saved =
                        g_terminal::ztransfer::receive(&mut stream, &local, &handle).await?;
                    Ok(format!("ZMODEM 已保存到 {}", saved.display()))
                }
            }
            .await;
            *operation.lock().unwrap() = Some(result.map_err(|e| format!("{e:#}")));
            wake();
        });
    }
    pub(super) fn next_batch_id(&mut self) -> u64 {
        self.batch_seq += 1;
        self.batch_seq
    }
    pub(super) fn queue(
        &mut self,
        local: PathBuf,
        remote: String,
        direction: Direction,
        ctx: &egui::Context,
    ) {
        // The edit round-trip re-uploads the same path on every save, so it is the
        // one caller that may replace an existing target.
        self.queue_replacing(local, remote, direction, false, ctx);
    }
    pub(super) fn queue_replacing(
        &mut self,
        local: PathBuf,
        remote: String,
        direction: Direction,
        overwrite: bool,
        ctx: &egui::Context,
    ) {
        if self.is_queued(&local, &remote) {
            self.error = Some("该文件已在队列中，请使用继续按钮".into());
            return;
        }
        let planned = std::fs::metadata(&local).map(|m| m.len()).unwrap_or(0);
        let id = self.next_batch_id();
        self.batches.push(BatchMeta {
            id,
            name: file_name_of(&local),
            directory: false,
        });
        self.push_transfer(
            remote::TransferSpec {
                local,
                remote,
                direction,
                planned,
                batch: id,
                batch_size: 1,
                policy: Arc::new(Mutex::new(remote::BatchPolicy::default())),
                overwrite,
            },
            ctx,
        );
    }
    pub(super) fn is_queued(&self, local: &Path, remote: &str) -> bool {
        self.transfers
            .iter()
            .any(|t| t.local == local && t.remote == remote && !t.state.lock().unwrap().finished)
    }
    pub(super) fn push_transfer(&mut self, spec: remote::TransferSpec, ctx: &egui::Context) {
        self.transfers.push(Transfer::new(
            self.connection.clone(),
            spec,
            self.conflicts.clone(),
            wake(ctx),
        ));
    }
    /// Queues a file, or walks a directory and queues everything under it.
    pub(super) fn transfer(
        &mut self,
        local: PathBuf,
        remote: String,
        direction: Direction,
        directory: bool,
        ctx: &egui::Context,
    ) {
        if directory {
            self.start_batch(file_name_of(&local), local, remote, direction, ctx);
        } else {
            self.queue(local, remote, direction, ctx);
        }
    }
    /// Starts a directory transfer. The walk runs off the UI thread; the entries it
    /// finds are queued once it lands, which is why this returns immediately.
    pub(super) fn start_batch(
        &mut self,
        name: String,
        local_root: PathBuf,
        remote_root: String,
        direction: Direction,
        ctx: &egui::Context,
    ) {
        let id = self.next_batch_id();
        let plan: PlanSlot = Arc::new(Mutex::new(None));
        let slot = plan.clone();
        let connection = self.connection.clone();
        let wake = wake(ctx);
        remote::runtime().spawn(async move {
            let result = async {
                let sftp = connection.sftp("遍历目录").await?;
                match direction {
                    Direction::Upload => {
                        remote::plan_upload(&sftp, &local_root, &remote_root).await
                    }
                    Direction::Download => {
                        remote::plan_download(&sftp, &remote_root, &local_root).await
                    }
                }
            }
            .await;
            *slot.lock().unwrap() = Some(result.map_err(|e| format!("{e:#}")));
            wake();
        });
        self.pending_batch = Some(PendingBatch {
            id,
            name,
            direction,
            plan,
        });
    }
    /// Turns a finished directory walk into queue entries.
    pub(super) fn poll_batch(&mut self, ctx: &egui::Context) {
        let Some(pending) = &self.pending_batch else {
            return;
        };
        let planned = pending.plan.lock().unwrap().clone();
        let Some(planned) = planned else {
            return;
        };
        let Some(pending) = self.pending_batch.take() else {
            return;
        };
        let files = match planned {
            Ok(files) => files,
            Err(e) => {
                self.error = Some(e);
                return;
            }
        };
        if files.is_empty() {
            self.notice = Some(format!("{} 里没有文件", pending.name));
            return;
        }
        let size = files.len();
        self.batches.push(BatchMeta {
            id: pending.id,
            name: pending.name,
            directory: true,
        });
        // One policy for the whole batch, so an "apply to all" answer covers it and
        // stops at its edge.
        let policy = Arc::new(Mutex::new(remote::BatchPolicy::default()));
        for file in files {
            // A destination already queued is skipped silently; a whole directory
            // would otherwise raise one error per entry.
            if self.is_queued(&file.local, &file.remote) {
                continue;
            }
            self.push_transfer(
                remote::TransferSpec {
                    local: file.local,
                    remote: file.remote,
                    direction: pending.direction,
                    planned: file.size,
                    batch: pending.id,
                    batch_size: size,
                    policy: policy.clone(),
                    overwrite: false,
                },
                ctx,
            );
        }
    }
    /// Picks up a conflict a queued transfer is blocked on.
    pub(super) fn poll_conflict(&mut self) {
        while let Ok(conflict) = self.conflict_rx.try_recv() {
            // One at a time is all the transfer gate allows, so the newest is the
            // only live one.
            self.conflict = Some(conflict);
        }
    }
    /// The conflict dialog. Answers the transfer waiting on it, then forgets it.
    pub(super) fn show_conflict(&mut self, ctx: &egui::Context, p: Palette) {
        let Some(conflict) = &self.conflict else {
            return;
        };
        let (name, existing, incoming, batch_size, batch) = (
            conflict.name.clone(),
            conflict.existing,
            conflict.incoming,
            conflict.batch_size,
            conflict.batch,
        );
        let mut answer = None;
        // A modal, not a window: the transfer is blocked until this is answered,
        // and a window that shares the file window's grey made it easy to miss.
        // The backdrop dims everything behind it, so there is no doubt about
        // what is being asked or where the answer goes.
        // A modal with a real header and a warn-coloured edge: the transfer is
        // blocked until this is answered, so it has to read as one dialog rather
        // than as a heap of equally sized lines and buttons.
        let frame = egui::Frame::popup(&ctx.style())
            .stroke(egui::Stroke::new(1.0_f32, p.warn.gamma_multiply(0.7)));
        egui::Modal::new(egui::Id::new("sftp-transfer-conflict"))
            .frame(frame)
            .show(ctx, |ui| {
                answer = conflict_body(ui, &name, existing, incoming, batch_size, p);
            });
        let Some(choice) = answer else {
            return;
        };
        if let Some(mut conflict) = self.conflict.take()
            && let Some(sender) = conflict.answer.take()
        {
            let _ = sender.send(choice);
        }
        // 取消剩余 has to stop entries that have not reached the front of the queue
        // yet, not just the one that asked: the policy only applies at the next
        // checkpoint, which a queued entry has not reached.
        if choice == remote::ConflictChoice::CancelRemaining {
            for transfer in &self.transfers {
                if transfer.batch == batch {
                    transfer.cancel();
                }
            }
        }
    }
    /// Refreshes the destination pane once a transfer settles, so a drop shows up
    /// without pressing 刷新.
    pub(super) fn poll_transfers(&mut self, ctx: &egui::Context) {
        use std::sync::atomic::Ordering;
        let mut local_dir = None;
        let mut remote_dir = None;
        for transfer in &self.transfers {
            if !transfer.settled.swap(false, Ordering::AcqRel) {
                continue;
            }
            match transfer.direction {
                Direction::Upload => {
                    let dir = parent_path(&transfer.remote);
                    // Wait for anything else heading for the same directory, so a
                    // directory transfer refreshes once at the end instead of
                    // clearing and reloading the list once per file.
                    let busy = self.transfers.iter().any(|other| {
                        matches!(other.direction, Direction::Upload)
                            && parent_path(&other.remote) == dir
                            && other.state.lock().unwrap().running
                    });
                    if !busy {
                        remote_dir = Some(dir);
                    }
                }
                Direction::Download => {
                    let Some(dir) = transfer.local.parent().map(Path::to_path_buf) else {
                        continue;
                    };
                    let busy = self.transfers.iter().any(|other| {
                        matches!(other.direction, Direction::Download)
                            && other.local.parent() == Some(dir.as_path())
                            && other.state.lock().unwrap().running
                    });
                    if !busy {
                        local_dir = Some(dir);
                    }
                }
            }
        }
        // Only if the user is still looking at that directory.
        if let Some(dir) = remote_dir
            && self.directory.lock().unwrap().path == dir
        {
            self.refresh_remote(ctx);
        }
        if let Some(dir) = local_dir
            && Path::new(&self.local_path) == dir.as_path()
        {
            self.refresh_local();
        }
        self.prune_queue();
        // A settled row disappears on a timer, so the frame that notices has to be
        // asked for — an idle window would otherwise leave it on screen.
        if !self.transfers.is_empty() {
            ctx.request_repaint_after(std::time::Duration::from_secs(1));
        }
    }
    /// Drops settled queue entries once they are old news.
    ///
    /// Nothing removed them before, and each one holds an `Arc<Connection>` and a
    /// handful of `Arc`s beyond that — a directory transfer would pin thousands of
    /// them, and the connection with them, for the life of the window.
    pub(super) fn prune_queue(&mut self) {
        /// A backstop for pathological queues, regardless of age.
        const KEEP_SETTLED: usize = 40;
        /// How long a finished row stays on screen. Long enough to read the
        /// outcome, short enough that a cancelled transfer does not sit there for
        /// the rest of the session.
        const LINGER: std::time::Duration = std::time::Duration::from_secs(6);
        let now = std::time::Instant::now();
        // A batch with entries still running keeps all of them: the group's
        // progress bar sums its members, so dropping a finished one would make the
        // bar run backwards.
        let unfinished: std::collections::HashSet<u64> = self
            .transfers
            .iter()
            .filter(|transfer| !transfer.state.lock().unwrap().finished)
            .map(|transfer| transfer.batch)
            .collect();
        let finished: Vec<usize> = self
            .transfers
            .iter()
            .enumerate()
            .filter(|(_, transfer)| transfer.state.lock().unwrap().finished)
            .map(|(index, _)| index)
            .collect();
        let mut doomed: Vec<usize> = finished
            .iter()
            .copied()
            .filter(|index| {
                let transfer = &self.transfers[*index];
                if unfinished.contains(&transfer.batch) {
                    return false;
                }
                transfer
                    .settled_at
                    .lock()
                    .unwrap()
                    .is_some_and(|at| now.duration_since(at) >= LINGER)
            })
            .collect();
        // Anything past the cap goes immediately, however recently it settled.
        doomed.extend(settled_to_drop(&finished, KEEP_SETTLED));
        doomed.sort_unstable();
        doomed.dedup();
        if doomed.is_empty() {
            return;
        }
        // Removing by index in reverse keeps the remaining indices valid.
        for index in doomed.into_iter().rev() {
            self.transfers.remove(index);
        }
        // A batch with no members left would render as nothing, but its id would
        // keep the sequence growing.
        let live: std::collections::HashSet<u64> = self
            .transfers
            .iter()
            .map(|transfer| transfer.batch)
            .collect();
        self.batches.retain(|batch| live.contains(&batch.id));
    }
}
