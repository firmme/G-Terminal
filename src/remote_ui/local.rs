//! Everything that touches the local filesystem: listings, entry properties,
//! permission changes, the editor round-trip and the timestamp formatting.

use super::*;

pub fn format_size(n: u64) -> String {
    if n >= 1024 * 1024 {
        format!("{:.1} MiB", n as f64 / 1048576.0)
    } else if n >= 1024 {
        format!("{:.1} KiB", n as f64 / 1024.0)
    } else {
        format!("{n} B")
    }
}

/// `ls -F` suffixes: `/` for directories, `@` for symlinks.
pub(super) fn entry_suffix(directory: bool, symlink: bool) -> &'static str {
    if directory {
        "/"
    } else if symlink {
        "@"
    } else {
        ""
    }
}

/// Which glyph stands for a row.
///
/// A name's extension decides; a directory or an executable overrides it, because
/// those are what the entry *is* rather than what it is called. An unknown
/// extension is a plain page rather than a guess.
pub(super) fn file_icon(name: &str, directory: bool, executable: bool) -> crate::icons::Icon {
    use crate::icons::Icon;
    if directory {
        return Icon::Folder;
    }
    if executable {
        return Icon::FileBinary;
    }
    let extension = file_extension(name);
    match extension.as_str() {
        // Source, and the files that configure it.
        "rs" | "go" | "c" | "h" | "cc" | "cpp" | "cxx" | "hpp" | "hh" | "py" | "pyi" | "js"
        | "mjs" | "cjs" | "jsx" | "ts" | "tsx" | "java" | "kt" | "kts" | "rb" | "php" | "sh"
        | "bash" | "zsh" | "fish" | "ps1" | "bat" | "cmd" | "lua" | "sql" | "html" | "htm"
        | "css" | "scss" | "sass" | "less" | "vue" | "svelte" | "swift" | "scala" | "hs" | "ml"
        | "ex" | "exs" | "erl" | "clj" | "dart" | "pl" | "vim" | "el" | "lisp" | "scm" | "asm"
        | "groovy" | "gradle" | "cmake" | "mk" | "json" | "yaml" | "yml" | "toml" | "ini"
        | "cfg" | "conf" | "xml" => Icon::FileCode,
        // Documents.
        "txt" | "md" | "markdown" | "rst" | "log" | "tex" | "rtf" | "doc" | "docx" | "odt"
        | "pdf" => Icon::FileText,
        "png" | "jpg" | "jpeg" | "gif" | "bmp" | "svg" | "webp" | "ico" | "tif" | "tiff"
        | "psd" | "xcf" | "raw" | "heic" | "avif" => Icon::FileImage,
        "zip" | "tar" | "gz" | "tgz" | "bz2" | "xz" | "zst" | "7z" | "rar" | "lz" | "lz4"
        | "deb" | "rpm" | "apk" | "jar" | "war" | "iso" | "cab" | "msix" | "nupkg" => {
            Icon::FileArchive
        }
        // Audio and video. `.ts` is not here: in a shell it is far more often
        // TypeScript than an MPEG transport stream.
        "mp3" | "wav" | "flac" | "ogg" | "oga" | "m4a" | "aac" | "opus" | "wma" | "mid"
        | "midi" | "mp4" | "mkv" | "avi" | "mov" | "webm" | "wmv" | "flv" | "m4v" | "mpg"
        | "mpeg" | "3gp" => Icon::FileMedia,
        // Things that are not meant to be read.
        "exe" | "dll" | "so" | "dylib" | "o" | "a" | "lib" | "obj" | "bin" | "com" | "msi"
        | "app" | "elf" | "ko" | "sys" | "wasm" => Icon::FileBinary,
        _ => Icon::File,
    }
}

/// The lowercased extension of a name, or empty when there is none.
///
/// A leading dot is part of the name, not an extension: `.bashrc` is a dotfile,
/// not a file of type "bashrc".
pub(super) fn file_extension(name: &str) -> String {
    match name.rsplit_once('.') {
        Some((stem, ext)) if !stem.is_empty() && !ext.is_empty() => ext.to_ascii_lowercase(),
        _ => String::new(),
    }
}

/// Joins a name onto a POSIX directory path without doubling the root slash.
pub(super) fn join_path(parent: &str, name: &str) -> String {
    format!("{}/{}", parent.trim_end_matches('/'), name)
}

/// The parent of a POSIX path, computed rather than appended, so the address bar
/// never ends up showing a `..` component.
pub(super) fn parent_path(path: &str) -> String {
    let trimmed = path.trim_end_matches('/');
    match trimmed.rfind('/') {
        Some(0) => "/".to_string(),
        Some(index) => trimmed[..index].to_string(),
        None if trimmed.is_empty() => "/".to_string(),
        None => trimmed.to_string(),
    }
}

/// A concrete height for a pane's list. In an auto-sizing container
/// `available_height` can be unbounded, which would let the list grow forever.
pub(super) fn list_height(ui: &egui::Ui) -> f32 {
    let available = ui.available_height();
    if available.is_finite() {
        available.max(80.0)
    } else {
        280.0
    }
}

/// `YYYY-MM-DD HH:MM` in UTC from Unix seconds. Hand-rolled so a single format
/// string does not pull a date crate into the build and the licence manifest.
pub(super) fn format_time(unix_seconds: u64) -> String {
    // Short enough for a narrow column, the way `ls -l` picks a form that fits:
    // the time of day for this year, the year for anything older. The full value
    // is one hover away.
    let (year, month, day, hour, minute) = civil_parts(unix_seconds);
    if year == current_year() {
        format!("{month:02}-{day:02} {hour:02}:{minute:02}")
    } else {
        format!("{year:04}-{month:02}-{day:02}")
    }
}

/// The timestamp in full, for the tooltip over a shortened one.
pub(super) fn format_time_full(unix_seconds: u64) -> String {
    let (year, month, day, hour, minute) = civil_parts(unix_seconds);
    format!("{year:04}-{month:02}-{day:02} {hour:02}:{minute:02}")
}

pub(super) fn civil_parts(unix_seconds: u64) -> (i64, u32, u32, u64, u64) {
    let (year, month, day) = civil_from_days((unix_seconds / 86_400) as i64);
    let seconds = unix_seconds % 86_400;
    (year, month, day, seconds / 3600, (seconds % 3600) / 60)
}

/// The year a timestamp is compared against when choosing a short form. Taken
/// from the clock so a file written this year reads as recent.
pub(super) fn current_year() -> i64 {
    let seconds = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or(0);
    let (year, ..) = civil_parts(seconds);
    year
}

/// Howard Hinnant's `civil_from_days`: days since 1970-01-01 to a civil date.
/// <https://howardhinnant.github.io/date_algorithms.html>
pub(super) fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let year = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if month <= 2 { year + 1 } else { year }, month, day)
}

/// Opens a path with whatever the OS associates with it.
pub(super) fn reveal(path: &Path) -> Result<(), String> {
    open_command(path)
        .spawn()
        .map(|_| ())
        .map_err(|e| format!("无法打开 {}：{e}", path.display()))
}

/// The command that hands a path to the OS default handler. On Windows the empty
/// argument is required, otherwise `start` reads the path as a window title.
pub(super) fn open_command(path: &Path) -> Command {
    let mut command = if cfg!(windows) {
        let mut c = Command::new("cmd");
        c.args(["/C", "start", ""]);
        c
    } else if cfg!(target_os = "macos") {
        Command::new("open")
    } else {
        Command::new("xdg-open")
    };
    command.arg(path);
    command
}

/// Opens a URL in the OS default browser.
pub(crate) fn open_url(url: &str) -> Result<(), String> {
    let mut command = if cfg!(windows) {
        let mut c = Command::new("cmd");
        c.args(["/C", "start", ""]);
        c
    } else if cfg!(target_os = "macos") {
        Command::new("open")
    } else {
        Command::new("xdg-open")
    };
    command
        .arg(url)
        .spawn()
        .map(|_| ())
        .map_err(|e| format!("无法打开浏览器：{e}"))
}

/// Opens a path in a text editor rather than in its associated application.
pub(super) fn edit_with(path: &Path) -> Result<(), String> {
    let Some(program) = editor_program() else {
        // Windows always has Notepad. macOS has no single editor path, so ask
        // the system for its default text editor rather than for whatever the
        // file type is associated with.
        #[cfg(target_os = "macos")]
        {
            return Command::new("open")
                .arg("-t")
                .arg(path)
                .spawn()
                .map(|_| ())
                .map_err(|e| format!("无法启动编辑器：{e}"));
        }
        #[cfg(not(target_os = "macos"))]
        return reveal(path);
    };
    Command::new(&program)
        .arg(path)
        .spawn()
        .map(|_| ())
        .map_err(|e| format!("无法启动编辑器 {program}：{e}"))
}

/// The editor to use, from the environment. Split from the lookup so it is
/// testable without launching anything or depending on this machine's variables.
pub(super) fn editor_program_from(lookup: impl Fn(&str) -> Option<String>) -> Option<String> {
    ["VISUAL", "EDITOR"]
        .into_iter()
        .find_map(&lookup)
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

pub(super) fn editor_program() -> Option<String> {
    editor_program_from(|name| std::env::var(name).ok()).or_else(|| {
        // Notepad is always present. Falling back to the file association would
        // open a .json in whatever edits JSON, which is not what 编辑 promises.
        cfg!(windows).then(|| "notepad.exe".to_string())
    })
}

/// A scratch directory for one editor round-trip. The index keeps two files that
/// share a name from colliding.
pub(super) fn scratch_dir(index: usize) -> Option<PathBuf> {
    let dir = std::env::temp_dir()
        .join("gterminal-edit")
        .join(format!("{}-{index}", std::process::id()));
    std::fs::create_dir_all(&dir).ok()?;
    Some(dir)
}

/// What the editor round-trip watches: size plus modification time.
pub(super) fn save_stamp(path: &Path) -> Option<(u64, u64)> {
    let metadata = std::fs::metadata(path).ok()?;
    let mtime = metadata
        .modified()
        .ok()?
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?
        .as_secs();
    Some((metadata.len(), mtime))
}

pub(super) fn file_name_of(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// Whether a local directory holds anything, so the caller can decide whether an
/// irreversible delete needs confirming. An unreadable directory counts as
/// non-empty, which errs toward asking.
pub(super) fn dir_is_empty_locally(path: &Path) -> bool {
    std::fs::read_dir(path)
        .map(|mut entries| entries.next().is_none())
        .unwrap_or(false)
}

pub(super) fn rename_local(
    path: &Path,
    name: &str,
    error: &mut Option<String>,
    notice: &mut Option<String>,
) {
    let Some(parent) = path.parent() else {
        *error = Some("无法确定所在目录".into());
        return;
    };
    let to = parent.join(name);
    if to.exists() {
        *error = Some("目标已存在，请换一个名字".into());
        return;
    }
    match std::fs::rename(path, &to) {
        Ok(()) => *notice = Some(format!("已重命名为 {name}")),
        Err(e) => *error = Some(format!("重命名失败：{e}")),
    }
}

pub(super) fn local_properties(entry: &LocalEntry, path: &Path) -> Properties {
    // A symlink's own mode is not changeable in a useful way — `chmod` follows
    // the link — so it is shown but not offered for editing.
    let editable = (entry.perms.is_some() && !entry.symlink).then(|| PermissionEdit {
        target: PermTarget::Local(path.to_path_buf()),
        mode: entry.perms.unwrap_or(0o644) & 0o7777,
        owner: String::new(),
        group: String::new(),
        recursive: false,
        counted: None,
    });
    Properties {
        name: entry.name.clone(),
        path: path.display().to_string(),
        kind: if entry.symlink {
            "符号链接"
        } else if entry.directory {
            "目录"
        } else {
            "文件"
        },
        // A directory's own size says nothing useful about what is inside it.
        size: (!entry.directory).then_some(entry.size),
        mtime: entry.mtime.map(format_time),
        // Windows carries no POSIX mode, so the row is left out rather than faked.
        perms: entry.perms.map(|m| Files::perms_string(Some(m))),
        editable,
    }
}

/// Walks a local tree without following symlinks, yielding the root and then
/// everything under it. The counterpart to `remote::walk_remote`, so a recursive
/// permission change behaves the same on both sides.
pub(super) fn walk_local(root: &Path) -> anyhow::Result<Vec<PathBuf>> {
    let mut paths = vec![root.to_path_buf()];
    let mut stack = vec![root.to_path_buf()];
    while let Some(directory) = stack.pop() {
        let entries = std::fs::read_dir(&directory)
            .map_err(|e| anyhow::anyhow!("无法读取 {}：{e}", directory.display()))?;
        for entry in entries.flatten() {
            let path = entry.path();
            // `symlink_metadata` does not follow the link, so a symlinked
            // directory is recorded but never descended.
            let Ok(metadata) = std::fs::symlink_metadata(&path) else {
                continue;
            };
            paths.push(path.clone());
            if metadata.is_dir() {
                stack.push(path);
            }
        }
    }
    Ok(paths)
}

/// Applies `mode` to a local path, over its tree when `recursive`. Symlinks are
/// skipped so a recursive change never follows one onto its target.
#[cfg(unix)]
pub(super) fn apply_local_mode(
    path: &Path,
    mode: u32,
    recursive: bool,
    progress: &Arc<Mutex<ztransfer::Progress>>,
    wake: &remote::Wake,
) -> anyhow::Result<String> {
    use std::os::unix::fs::PermissionsExt;
    let mut paths = if recursive {
        walk_local(path)?
    } else {
        vec![path.to_path_buf()]
    };
    // Deepest first: a directory whose new mode drops the execute bit can no
    // longer be entered, so its contents have to be changed before its own mode.
    paths.sort_by_key(|path| std::cmp::Reverse(path.components().count()));
    let total = paths.len().max(1);
    for (index, target) in paths.iter().enumerate() {
        let is_symlink = std::fs::symlink_metadata(target)
            .map(|metadata| metadata.file_type().is_symlink())
            .unwrap_or(false);
        if !is_symlink {
            std::fs::set_permissions(target, std::fs::Permissions::from_mode(mode))
                .map_err(|e| anyhow::anyhow!("无法修改 {}：{e}", target.display()))?;
        }
        if let Ok(mut state) = progress.lock() {
            state.done = index as u64 + 1;
            state.total = total as u64;
            state.message = "修改中".into();
        }
        wake();
    }
    Ok(if recursive {
        format!("已修改 {} 个条目", paths.len())
    } else {
        "权限已修改".into()
    })
}

/// Windows has no POSIX mode to apply. The editor is never offered there, so
/// this is only reached if that ever changes.
#[cfg(not(unix))]
pub(super) fn apply_local_mode(
    _path: &Path,
    _mode: u32,
    _recursive: bool,
    _progress: &Arc<Mutex<ztransfer::Progress>>,
    _wake: &remote::Wake,
) -> anyhow::Result<String> {
    anyhow::bail!("此平台不支持 POSIX 权限")
}

pub(super) fn remote_properties(target: &RemoteTarget, perms: String) -> Properties {
    // Seeded from what the listing already reported, so the dialog opens on the
    // entry's real mode rather than a guess.
    let mode = target.entry.perms.unwrap_or(0o644);
    Properties {
        name: target.entry.name.clone(),
        path: target.path.clone(),
        kind: if target.entry.directory {
            "目录"
        } else {
            "文件"
        },
        size: (!target.entry.directory).then_some(target.entry.size),
        mtime: target.entry.mtime.map(|t| format_time(u64::from(t))),
        perms: Some(perms),
        editable: Some(PermissionEdit {
            target: PermTarget::Remote(target.clone()),
            mode,
            // SFTP carries ids, not names, so these start as numbers. A name typed
            // here is resolved on the server before anything is sent.
            owner: target
                .entry
                .uid
                .map(|id| id.to_string())
                .unwrap_or_default(),
            group: target
                .entry
                .gid
                .map(|id| id.to_string())
                .unwrap_or_default(),
            recursive: false,
            counted: None,
        }),
    }
}

pub(super) fn spawn_delete_remote(
    connection: Arc<Connection>,
    target: RemoteTarget,
    state: Outcome,
    ctx: &egui::Context,
) {
    let wake = wake(ctx);
    let (path, is_dir) = (target.path, target.entry.directory);
    remote::runtime().spawn(async move {
        let result = async {
            let sftp = connection.sftp("删除").await?;
            remote::remove(&sftp, &path, is_dir).await
        }
        .await;
        *state.lock().unwrap() = Some(
            result
                .map(|_| "已删除".to_string())
                .map_err(|e| format!("{e:#}")),
        );
        wake();
    });
}

pub(super) fn spawn_rename_remote(
    connection: Arc<Connection>,
    target: RemoteTarget,
    to: String,
    state: Outcome,
    ctx: &egui::Context,
) {
    let wake = wake(ctx);
    remote::runtime().spawn(async move {
        let result = async {
            let sftp = connection.sftp("重命名").await?;
            remote::rename(&sftp, &target.path, &to).await
        }
        .await;
        *state.lock().unwrap() = Some(
            result
                .map(|_| "已重命名".to_string())
                .map_err(|e| format!("{e:#}")),
        );
        wake();
    });
}
