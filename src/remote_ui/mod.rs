use crate::editing;
use crate::theme::Palette;
use eframe::egui::{self, RichText};
use g_terminal::{
    config::RemoteProfile,
    remote::{self, ConnectJob, Connection, Credentials, Direction, Transfer},
    terminal::Terminal,
    ztransfer,
};
use std::{
    path::{Path, PathBuf},
    process::Command,
    sync::{Arc, Mutex},
};

pub fn wake(ctx: &egui::Context) -> remote::Wake {
    let ctx = ctx.clone();
    Arc::new(move || ctx.request_repaint())
}
pub fn hint(text: &str, p: Palette) -> RichText {
    RichText::new(text).color(p.muted)
}

mod dialogs;
mod local;
mod login;
mod menu;
mod panes;
mod table;
#[cfg(test)]
mod tests;
mod transfer;
pub use local::format_size;
pub(crate) use local::open_url;
use local::*;
pub use login::Login;
use menu::*;
use table::*;

/// Where a background operation leaves its final message.
type Outcome = Arc<Mutex<Option<Result<String, String>>>>;

/// A ZMODEM transfer the file window keeps on screen while it runs.
struct Operation {
    progress: Arc<Mutex<ztransfer::Progress>>,
    cancel: Arc<std::sync::atomic::AtomicBool>,
    result: Outcome,
}

impl Operation {
    fn state(&self) -> ztransfer::Progress {
        self.progress
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }
    fn outcome(&self) -> Option<Result<String, String>> {
        self.result.lock().unwrap().clone()
    }
    fn running(&self) -> bool {
        self.result.lock().unwrap().is_none()
    }
}

/// One row of the local file table.
struct LocalEntry {
    name: String,
    directory: bool,
    size: u64,
    symlink: bool,
    /// Modification time in Unix seconds; None when unavailable.
    mtime: Option<u64>,
    /// POSIX mode from `lstat`, or None on platforms that have no such bits.
    perms: Option<u32>,
}

/// The local file's POSIX mode. Windows has no equivalent, so the local table
/// and the 属性 dialog leave permissions out there rather than inventing them.
#[cfg(unix)]
fn local_mode(metadata: &std::fs::Metadata) -> Option<u32> {
    use std::os::unix::fs::PermissionsExt;
    Some(metadata.permissions().mode())
}

#[cfg(not(unix))]
fn local_mode(_metadata: &std::fs::Metadata) -> Option<u32> {
    None
}

/// The local listing, as the worker thread leaves it.
#[derive(Default)]
struct LocalState {
    entries: Vec<LocalEntry>,
    /// Which directory the entries describe, so a slow read cannot overwrite the
    /// result of a later one.
    path: String,
    loading: bool,
    error: Option<String>,
}

/// Reads one directory into sorted rows. Blocking, so it runs off the UI thread.
fn read_local(path: &str) -> Result<Vec<LocalEntry>, String> {
    let mut rows: Vec<LocalEntry> = std::fs::read_dir(path)
        .map_err(|e| e.to_string())?
        .filter_map(Result::ok)
        .map(|entry| {
            let metadata = entry.metadata().ok();
            LocalEntry {
                name: entry.file_name().to_string_lossy().into_owned(),
                directory: metadata.as_ref().is_some_and(|m| m.is_dir()),
                size: metadata.as_ref().map(|m| m.len()).unwrap_or(0),
                symlink: entry.file_type().ok().is_some_and(|t| t.is_symlink()),
                mtime: metadata
                    .as_ref()
                    .and_then(|m| m.modified().ok())
                    .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                    .map(|d| d.as_secs()),
                perms: metadata.as_ref().and_then(local_mode),
            }
        })
        .collect();
    rows.sort_by(|a, b| {
        b.directory
            .cmp(&a.directory)
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
    Ok(rows)
}

/// A remote row plus its full path, which the row itself does not carry.
#[derive(Clone)]
struct RemoteTarget {
    entry: remote::Entry,
    path: String,
}

/// Everything the 属性 dialog shows. Assembled from a row already in hand, so
/// opening it costs no round trip.
#[derive(Clone)]
struct Properties {
    name: String,
    path: String,
    kind: &'static str,
    size: Option<u64>,
    mtime: Option<String>,
    perms: Option<String>,
    /// Present when the mode (and, remotely, the ownership) can be changed.
    /// Local files only get one on platforms with POSIX bits.
    editable: Option<PermissionEdit>,
}

/// A recursive walk's result, left for the UI thread to collect.
type CountSlot = Arc<Mutex<Option<Result<usize, String>>>>;

/// What a permission change applies to: a local path, or a remote entry reached
/// over SFTP.
#[derive(Clone)]
enum PermTarget {
    Local(PathBuf),
    Remote(RemoteTarget),
}

/// The permission editor's state, kept alongside the dialog because egui rebuilds
/// the window every frame.
#[derive(Clone)]
struct PermissionEdit {
    target: PermTarget,
    mode: u32,
    owner: String,
    group: String,
    recursive: bool,
    /// Set once a recursive walk has counted the entries, which is what the
    /// confirmation is about.
    counted: Option<usize>,
}

/// A destructive action held until the user answers.
struct Confirm {
    message: String,
    action: ConfirmAction,
}

/// What a confirmation will carry out. Kept separate from `MenuAction` so that
/// confirming performs the work directly instead of re-entering the menu dispatch,
/// which would just ask for confirmation again.
enum ConfirmAction {
    DeleteLocal(PathBuf),
    DeleteRemote(RemoteTarget),
}

/// A rename in progress: the name in an editable box plus what it applies to.
struct Rename {
    name: String,
    local: Option<PathBuf>,
    remote: Option<RemoteTarget>,
}

/// One user action that put files in the queue: a single file, or a directory.
struct BatchMeta {
    id: u64,
    /// What the user picked, used as the queue group's label.
    name: String,
    /// A directory pick, drawn as a tree with aggregate progress.
    directory: bool,
}

/// Where a directory walk leaves its result for the UI thread to collect.
type PlanSlot = Arc<Mutex<Option<Result<Vec<remote::PlannedFile>, String>>>>;

/// A directory pick whose walk is still running.
struct PendingBatch {
    id: u64,
    name: String,
    direction: Direction,
    plan: PlanSlot,
}

/// One remote file opened in an external editor, polled for saves to send back.
struct EditSession {
    stop: Arc<std::sync::atomic::AtomicBool>,
    /// The scratch copy's directory, removed when the session ends. Without this
    /// every edited file would leave a directory behind under the temp folder.
    dir: PathBuf,
}

/// What a context-menu item asked for. The row loop only records it; the work
/// happens afterwards, where `&mut self` is free and a spawned task can own what
/// it needs.
enum MenuAction {
    OpenLocal(PathBuf),
    OpenRemote(RemoteTarget),
    EditLocal(PathBuf),
    EditRemote(RemoteTarget),
    Upload(PathBuf),
    Download(RemoteTarget),
    RenameLocal(PathBuf),
    RenameRemote(RemoteTarget),
    DeleteLocal(PathBuf),
    DeleteRemote(RemoteTarget),
    /// Copies text and says so, since a clipboard write has no visible effect.
    CopyPath(String),
    Properties(Properties),
}

pub struct Files {
    pub connection: Arc<Connection>,
    directory: Arc<Mutex<remote::DirectoryState>>,
    remote_path: String,
    local_path: String,
    /// Where the pane-level home buttons jump to.
    local_home: PathBuf,
    /// Captured from the first listing, which resolves `.` to the server's home.
    remote_home: Option<String>,
    selected_local: Option<PathBuf>,
    selected_remote: Option<remote::Entry>,
    /// The local listing, filled on a worker thread.
    local: Arc<Mutex<LocalState>>,
    /// How the worker tells the UI there is something new to show.
    wake: remote::Wake,
    name: String,
    error: Option<String>,
    /// Transient confirmation, e.g. "已复制路径".
    notice: Option<String>,
    pub transfers: Vec<Transfer>,
    operation: Option<Operation>,
    /// Conflict a running ZMODEM transfer is waiting on an answer for.
    question: Option<ztransfer::Question>,
    questions: Option<tokio::sync::mpsc::UnboundedReceiver<ztransfer::Question>>,
    answers: Option<tokio::sync::mpsc::UnboundedSender<ztransfer::Decision>>,
    /// Conflicts raised by queued transfers, and the slot the dialog answers from.
    /// One slot is enough: the transfer gate lets only one file move at a time.
    conflicts: tokio::sync::mpsc::UnboundedSender<remote::Conflict>,
    conflict_rx: tokio::sync::mpsc::UnboundedReceiver<remote::Conflict>,
    conflict: Option<remote::Conflict>,
    /// One entry per user action, so the queue can group entries and aggregate.
    batches: Vec<BatchMeta>,
    batch_seq: u64,
    /// A directory pick whose walk is still running.
    pending_batch: Option<PendingBatch>,
    rename: String,
    split_ratio: f32,
    /// Height the footer rows occupied last frame. The panes reserve it so the
    /// window's content never exceeds its own height.
    footer_height: f32,
    show_hidden: bool,
    properties: Option<Properties>,
    /// A recursive-permission walk in flight, counting what it would touch.
    counting: Option<CountSlot>,
    confirm: Option<Confirm>,
    renaming: Option<Rename>,
    /// A directory delete waiting on the server's emptiness verdict.
    deleting: Option<RemoteTarget>,
    /// Live editor round-trips. Dropping `Files` stops them, so a closed window
    /// cannot leave a task holding the connection open.
    edits: Vec<EditSession>,
}

impl Drop for Files {
    fn drop(&mut self) {
        for session in &self.edits {
            session
                .stop
                .store(true, std::sync::atomic::Ordering::Release);
            // The scratch copy has served its purpose; a closed window should not
            // leave one directory per edited file behind.
            let _ = std::fs::remove_dir_all(&session.dir);
        }
    }
}
impl Files {
    pub fn new(connection: Arc<Connection>, ctx: &egui::Context, hide_dotfiles: bool) -> Self {
        let dirs = directories::UserDirs::new();
        let home = dirs.as_ref().map(|d| d.home_dir().to_path_buf());
        // Downloads is where a file browser's traffic lands; the home button still
        // jumps to the home directory, which is a different destination.
        let local_path = dirs
            .as_ref()
            .and_then(|d| d.download_dir().map(|d| d.to_path_buf()))
            .or_else(|| home.clone())
            .map(|d| d.display().to_string())
            .unwrap_or_else(|| ".".into());
        let (conflicts, conflict_rx) = tokio::sync::mpsc::unbounded_channel();
        let mut this = Self {
            directory: remote::list_directory(connection.clone(), ".".into(), wake(ctx)),
            connection,
            remote_path: ".".into(),
            local_path,
            local_home: home.unwrap_or_else(|| PathBuf::from(".")),
            remote_home: None,
            selected_local: None,
            selected_remote: None,
            local: Arc::new(Mutex::new(LocalState::default())),
            wake: wake(ctx),
            name: String::new(),
            error: None,
            notice: None,
            transfers: vec![],
            operation: None,
            question: None,
            questions: None,
            answers: None,
            conflicts,
            conflict_rx,
            conflict: None,
            batches: vec![],
            batch_seq: 0,
            pending_batch: None,
            rename: String::new(),
            split_ratio: 0.5,
            // A first-frame estimate, so the window does not jump before the real
            // footer has been measured once.
            footer_height: 150.0,
            show_hidden: !hide_dotfiles,
            properties: None,
            counting: None,
            confirm: None,
            renaming: None,
            deleting: None,
            edits: vec![],
        };
        this.refresh_local();
        this
    }

    /// True while a ZMODEM transfer wants the window on screen.
    pub fn busy(&self) -> bool {
        self.operation.as_ref().is_some_and(Operation::running)
    }
    /// True while a directory listing is in flight. Opening the window on a server
    /// without SFTP waits out the subsystem probe here, and the spinner needs
    /// repaints to keep turning for that whole time.
    pub fn loading(&self) -> bool {
        // Either pane still filling counts: both drive their own repaints.
        self.directory.lock().unwrap().loading || self.local.lock().unwrap().loading
    }

    /// Asks a running transfer to stop; it aborts the peer and cleans up.
    pub fn cancel(&self) {
        if let Some(operation) = &self.operation {
            operation
                .cancel
                .store(true, std::sync::atomic::Ordering::Release);
        }
    }

    /// Re-reads the local directory on a worker thread.
    ///
    /// This used to run inline, and it is one `read_dir` plus a `stat` and a
    /// `file_type` per entry: pointing the address bar at something like
    /// `C:\Windows\WinSxS` froze the window for as long as that took.
    fn refresh_local(&mut self) {
        let path = self.local_path.clone();
        let state = self.local.clone();
        let wake = self.wake.clone();
        {
            let mut state = state.lock().unwrap();
            state.loading = true;
            state.error = None;
            state.path = path.clone();
        }
        // The selected row belongs to the listing being replaced.
        self.selected_local = None;
        remote::runtime().spawn_blocking(move || {
            let result = read_local(&path);
            let mut state = state.lock().unwrap();
            // A slower listing for an older directory must not overwrite this one.
            if state.path == path {
                match result {
                    Ok(entries) => state.entries = entries,
                    Err(e) => state.error = Some(e),
                }
                state.loading = false;
            }
            wake();
        });
    }
    fn refresh_remote(&mut self, ctx: &egui::Context) {
        self.directory =
            remote::list_directory(self.connection.clone(), self.remote_path.clone(), wake(ctx));
        self.selected_remote = None;
    }
    fn remote_child(&self, name: &str) -> String {
        join_path(&self.directory.lock().unwrap().path, name)
    }

    /// The remote directory the listing is showing, for the drag-and-drop hint.
    pub fn remote_dir(&self) -> String {
        self.directory
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .path
            .clone()
    }

    /// Queues a local file or directory for upload into the directory the
    /// window is showing. Used by files dropped onto the app window.
    pub fn upload_path(&mut self, path: PathBuf, ctx: &egui::Context) {
        let name = file_name_of(&path);
        if name.is_empty() {
            self.error = Some("拖入的路径没有文件名".into());
            return;
        }
        if self.remote_dir().is_empty() {
            self.error = Some("远程目录还没加载完，请稍后再拖入".into());
            return;
        }
        let directory = path.is_dir();
        let remote = self.remote_child(&name);
        self.transfer(path, remote, Direction::Upload, directory, ctx);
    }

    /// Reserves the single operation slot for a short SFTP-side action.
    fn simple(&mut self, name: &str) -> Outcome {
        self.simple_progress(name).1
    }

    /// Like [`Self::simple`], but also hands back the progress slot so a long walk
    /// can report how far it has got. The footer and the top bar both read that
    /// slot, so the bar appears without any further plumbing.
    fn simple_progress(&mut self, name: &str) -> (Arc<Mutex<ztransfer::Progress>>, Outcome) {
        let result = Arc::new(Mutex::new(None));
        let progress = Arc::new(Mutex::new(ztransfer::Progress {
            name: name.into(),
            message: "进行中".into(),
            ..Default::default()
        }));
        self.operation = Some(Operation {
            progress: progress.clone(),
            cancel: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            result: result.clone(),
        });
        (progress, result)
    }

    /// Reserves the single ZMODEM slot and wires up its question channel.
    /// Refuses while another transfer still owns the slot.
    fn begin(&mut self, name: &str) -> Option<(ztransfer::Handle, Outcome)> {
        if self.busy() {
            self.error = Some("已有传输正在进行，请等待完成或先取消".into());
            return None;
        }
        let (ask, questions, answers) = ztransfer::ask_channel();
        self.questions = Some(questions);
        self.answers = Some(answers);
        self.question = None;
        self.rename.clear();
        let result = Arc::new(Mutex::new(None));
        let handle = ztransfer::Handle::new(name, Some(ask));
        self.operation = Some(Operation {
            progress: handle.progress.clone(),
            cancel: handle.cancel.clone(),
            result: result.clone(),
        });
        Some((handle, result))
    }

    /// Answers and progress of a running ZMODEM transfer, for the status bar.
    pub fn operation_state(&self) -> Option<ztransfer::Progress> {
        self.operation.as_ref().map(Operation::state)
    }

    fn report(&mut self, result: Result<(), String>) {
        if let Err(e) = result {
            self.error = Some(e);
        }
    }

    /// Reserves the single operation slot for a one-off remote action, the same way
    /// `simple` does for the footer's mkdir/rename row.
    fn simple_operation(&mut self) -> Outcome {
        self.simple("文件操作")
    }

    pub fn show(&mut self, ctx: &egui::Context, open: &mut bool, p: Palette, hide_dotfiles: bool) {
        self.show_hidden = self.show_hidden || !hide_dotfiles;
        self.poll_question();
        self.poll_conflict();
        self.poll_batch(ctx);
        self.poll_transfers(ctx);
        self.poll_count();
        egui::Window::new(format!("文件 · {}", self.connection.profile.label()))
            .open(open)
            .default_size([900.0, 560.0])
            .min_width(560.0)
            .min_height(360.0)
            .show(ctx, |ui| {
                ui.horizontal_wrapped(|ui| {
                    ui.label(RichText::new("SFTP").color(p.accent));
                    if ui.checkbox(&mut self.show_hidden, "显示隐藏文件").changed() {
                        self.refresh_local();
                    }
                });
                let mut local_next = None;
                let mut remote_next = None;
                // Collected by the row loops and applied below, where `&mut self`
                // is free of the borrows the panes hold.
                let mut menu = None;
                let available = ui.available_width();
                let area = ui.available_rect_before_wrap();
                // The panes take whatever the footer left last frame — measured,
                // not guessed. Without this the content would be taller than the
                // window, and egui's `Resize` refuses to shrink a window below its
                // content (`resize.rs`: `desired_size.max(last_content_size)`), so
                // dragging the window shorter would spring straight back.
                let panes = egui::Rect::from_min_max(
                    area.min,
                    egui::pos2(
                        area.right(),
                        (area.bottom() - self.footer_height).max(area.top() + 80.0),
                    ),
                );
                // Side by side needs room for two readable columns; a narrow
                // window stacks them instead so it stays usable when shrunk.
                let stacked = available < 520.0;
                let pane = egui::Layout::top_down(egui::Align::LEFT);
                let (local_rect, remote_rect, divider) = if stacked {
                    let half = (panes.height() * self.split_ratio.clamp(0.2, 0.8)).round();
                    let cut = panes.top() + half;
                    (
                        egui::Rect::from_min_max(panes.min, egui::pos2(panes.right(), cut - 3.0)),
                        egui::Rect::from_min_max(egui::pos2(panes.left(), cut + 3.0), panes.max),
                        egui::Rect::from_min_max(
                            egui::pos2(panes.left(), cut - 3.0),
                            egui::pos2(panes.right(), cut + 3.0),
                        ),
                    )
                } else {
                    let left =
                        panes.left() + (available * self.split_ratio.clamp(0.25, 0.75)).round();
                    // A gap rather than a hairline butt-joint: with nothing drawn
                    // between them the two tables read as one continuous band of
                    // text, and the remote names look like they are sitting on the
                    // local timestamps.
                    const GAP: f32 = 7.0;
                    (
                        egui::Rect::from_min_max(panes.min, egui::pos2(left - GAP, panes.bottom())),
                        egui::Rect::from_min_max(egui::pos2(left + GAP, panes.top()), panes.max),
                        egui::Rect::from_min_max(
                            egui::pos2(left - 3.0, panes.top()),
                            egui::pos2(left + 3.0, panes.bottom()),
                        ),
                    )
                };
                let mut local_ui =
                    ui.new_child(egui::UiBuilder::new().max_rect(local_rect).layout(pane));
                self.show_local_pane(&mut local_ui, p, &mut local_next, &mut menu);
                let mut remote_ui =
                    ui.new_child(egui::UiBuilder::new().max_rect(remote_rect).layout(pane));
                self.show_remote_pane(&mut remote_ui, p, &mut remote_next, &mut menu);
                let drag = ui.interact(divider, ui.id().with("files-split"), egui::Sense::drag());
                if drag.dragged()
                    && let Some(pos) = drag.interact_pointer_pos()
                {
                    self.split_ratio = if stacked {
                        ((pos.y - panes.top()) / panes.height().max(1.0)).clamp(0.2, 0.8)
                    } else {
                        ((pos.x - panes.left()) / available.max(1.0)).clamp(0.25, 0.75)
                    };
                }
                drag.on_hover_cursor(if stacked {
                    egui::CursorIcon::ResizeVertical
                } else {
                    egui::CursorIcon::ResizeHorizontal
                });
                // A visible seam. Painted after the panes so it lands on top.
                let seam = egui::Stroke::new(1.0_f32, p.line);
                if stacked {
                    ui.painter()
                        .hline(divider.x_range(), divider.center().y, seam);
                } else {
                    ui.painter()
                        .vline(divider.center().x, divider.y_range(), seam);
                }
                ui.allocate_rect(panes, egui::Sense::hover());
                if let Some(path) = local_next {
                    self.local_path = path;
                    self.refresh_local();
                }
                if let Some(path) = remote_next {
                    self.remote_path = path;
                    self.refresh_remote(ctx);
                }
                if let Some(action) = menu {
                    self.dispatch(action, ctx);
                }
                ui.separator();
                ui.horizontal_wrapped(|ui| {
                    // Plain labels rather than painted glyphs: the arrow already
                    // says which way the file goes, and at this size a drawn icon
                    // competes with the text instead of helping it.
                    if ui
                        .add(egui::Button::new("上传 →").min_size(egui::vec2(80.0, 0.0)))
                        .clicked()
                        && let Some(path) = self.selected_local.clone()
                    {
                        let directory = path.is_dir();
                        let remote = self.remote_child(&file_name_of(&path));
                        self.transfer(path, remote, Direction::Upload, directory, ctx);
                    }
                    if ui
                        .add(egui::Button::new("← 下载").min_size(egui::vec2(80.0, 0.0)))
                        .clicked()
                        && let Some(e) = self.selected_remote.clone()
                    {
                        if !remote::safe_local_name(&e.name) {
                            self.error = Some("服务器返回了非法文件名".into());
                        } else {
                            let directory = e.directory;
                            self.transfer(
                                PathBuf::from(&self.local_path).join(&e.name),
                                self.remote_child(&e.name),
                                Direction::Download,
                                directory,
                                ctx,
                            );
                        }
                    }
                    ui.separator();
                    editing::field_with(ui, &mut self.name, |edit| edit.desired_width(160.0));
                    let mut op = None;
                    if ui
                        .add(egui::Button::new("建目录").min_size(egui::vec2(80.0, 0.0)))
                        .clicked()
                    {
                        op = Some(0);
                    }
                    if ui
                        .add(egui::Button::new("重命名").min_size(egui::vec2(80.0, 0.0)))
                        .clicked()
                    {
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
                            let state = self.simple("新建目录 / 重命名");
                            let wake = wake(ctx);
                            remote::runtime().spawn(async move {
                                let result: anyhow::Result<()> = async {
                                    let sftp = connection.sftp("新建目录或重命名").await?;
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
                if let Some(operation) = &self.operation {
                    let state = operation.state();
                    ui.horizontal_wrapped(|ui| {
                        ui.label(RichText::new(state.name.clone()).color(p.accent));
                        if state.total > 0 {
                            ui.add(
                                egui::ProgressBar::new(state.fraction())
                                    .desired_width(160.0)
                                    .show_percentage(),
                            );
                            ui.label(hint(
                                &format!(
                                    "{} / {} · {}",
                                    format_size(state.done),
                                    format_size(state.total),
                                    state.message
                                ),
                                p,
                            ));
                        } else {
                            ui.label(hint(&state.message, p));
                        }
                        if operation.running() && ui.small_button("取消").clicked() {
                            operation
                                .cancel
                                .store(true, std::sync::atomic::Ordering::Release);
                        }
                    });
                    match operation.outcome() {
                        Some(Ok(message)) => {
                            ui.colored_label(p.accent, message);
                        }
                        Some(Err(e)) => {
                            ui.colored_label(p.danger, e);
                        }
                        None => {}
                    }
                }
                if let Some(error) = &self.error {
                    ui.colored_label(p.danger, error);
                }
                if let Some(notice) = self.notice.clone() {
                    ui.horizontal(|ui| {
                        ui.colored_label(p.accent, &notice);
                        if ui.small_button("×").clicked() {
                            self.notice = None;
                        }
                    });
                }
                ui.label("传输队列");
                egui::ScrollArea::vertical()
                    .id_salt("transfers")
                    .max_height(150.0)
                    .show(ui, |ui| {
                        for batch in &self.batches {
                            let members: Vec<&Transfer> = self
                                .transfers
                                .iter()
                                .filter(|t| t.batch == batch.id)
                                .collect();
                            let Some(first) = members.first() else {
                                continue;
                            };
                            if !batch.directory {
                                transfer_row(ui, first, p, ctx, false);
                                continue;
                            }
                            // A directory batch shows its own aggregate, then only
                            // the file actually moving. Queued and finished files
                            // are deliberately absent: a long tree would otherwise
                            // bury the live one.
                            let planned: u64 = members.iter().map(|t| t.planned).sum();
                            let settled: u64 = members
                                .iter()
                                .map(|t| {
                                    let s = t.state.lock().unwrap();
                                    // A settled entry counts its planned bytes,
                                    // whether it moved them or was skipped, so the
                                    // bar reads "how much of this batch is done
                                    // with" and still reaches the end.
                                    if s.finished { t.planned } else { s.done }
                                })
                                .sum();
                            let finished = members
                                .iter()
                                .filter(|t| t.state.lock().unwrap().finished)
                                .count();
                            ui.horizontal(|ui| {
                                ui.label(RichText::new(format!(
                                    "{} {}/",
                                    direction_glyph(batch_direction(&members)),
                                    batch.name
                                )));
                                ui.add(
                                    egui::ProgressBar::new(if planned == 0 {
                                        0.0
                                    } else {
                                        settled as f32 / planned as f32
                                    })
                                    .desired_width(160.0)
                                    .show_percentage(),
                                );
                                ui.label(hint(
                                    &format!(
                                        "{} / {} · {}/{} 个文件",
                                        format_size(settled),
                                        format_size(planned),
                                        finished,
                                        members.len()
                                    ),
                                    p,
                                ));
                            });
                            if let Some(active) = members
                                .iter()
                                .find(|t| t.active.load(std::sync::atomic::Ordering::Acquire))
                            {
                                ui.indent(batch.id, |ui| {
                                    transfer_row(ui, active, p, ctx, true);
                                });
                            }
                        }
                    });
                // Measured, not guessed. The panes above reserved exactly
                // `area − footer_height`, so the two together add up to the
                // window's own content box and `Resize` is free to shrink it.
                self.footer_height = (ui.min_rect().bottom() - panes.bottom()).max(0.0);
            });
        // Outside the window closure, where `self` is free to move again.
        self.dialogs(ctx, p);
    }
}
/// The body of the overwrite conflict dialog.
///
/// A header, the file it is about, the sizes, then the choices — laid out like
/// a dialog rather than a list of sentences, because this one blocks a transfer
/// until it is answered.
pub(crate) fn conflict_body(
    ui: &mut egui::Ui,
    name: &str,
    existing: u64,
    incoming: u64,
    batch_size: usize,
    p: Palette,
) -> Option<remote::ConflictChoice> {
    let mut answer = None;
    ui.set_width(470.0);
    // A tinted header band, so the dialog announces itself before anything else
    // is read — the same size for every line is what made it look like a list.
    egui::Frame::new()
        .fill(p.warn.gamma_multiply(0.18))
        .inner_margin(egui::Margin::symmetric(12, 9))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(
                RichText::new("目标已存在")
                    .color(p.warn)
                    .size(17.0)
                    .strong(),
            );
        });
    ui.add_space(12.0);
    ui.label(RichText::new(name).color(p.text).strong());
    ui.add_space(4.0);
    ui.label(
        RichText::new(format!(
            "已有 {} · 本次 {} · 本批共 {} 个文件",
            format_size(existing),
            format_size(incoming),
            batch_size
        ))
        .color(p.muted),
    );
    ui.add_space(14.0);
    // The 全部 buttons only mean something when the batch holds more than the
    // file that asked.
    let more = batch_size > 1;
    ui.spacing_mut().item_spacing = egui::vec2(8.0, 8.0);
    // The choices are grouped by what they affect: this file, the whole batch,
    // then the one that stops everything.
    ui.horizontal(|ui| {
        if ui.button("覆盖").clicked() {
            answer = Some(remote::ConflictChoice::Overwrite);
        }
        if ui.button("自动重命名").clicked() {
            answer = Some(remote::ConflictChoice::Rename);
        }
        if ui.button("跳过这个文件").clicked() {
            answer = Some(remote::ConflictChoice::Skip);
        }
    });
    ui.add_space(2.0);
    ui.horizontal(|ui| {
        if ui
            .add_enabled(more, egui::Button::new("本批全部覆盖"))
            .clicked()
        {
            answer = Some(remote::ConflictChoice::OverwriteAll);
        }
        if ui
            .add_enabled(more, egui::Button::new("本批全部重命名"))
            .clicked()
        {
            answer = Some(remote::ConflictChoice::RenameAll);
        }
        if ui
            .add_enabled(more, egui::Button::new("本批全部跳过"))
            .clicked()
        {
            answer = Some(remote::ConflictChoice::SkipAll);
        }
    });
    ui.add_space(14.0);
    ui.separator();
    ui.add_space(8.0);
    ui.horizontal(|ui| {
        if ui.button("取消剩余任务").clicked() {
            answer = Some(remote::ConflictChoice::CancelRemaining);
        }
        ui.label(hint("文件不会被改动", p));
    });
    answer
}
