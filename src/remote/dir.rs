//! Remote directory listings.

use super::*;

#[derive(Clone, Debug)]
pub struct Entry {
    pub name: String,
    pub directory: bool,
    /// A symlink. `directory` is false for a link to a directory, because the
    /// listing reports the link itself rather than what it points at.
    pub symlink: bool,
    pub size: u64,
    /// POSIX permission bits, e.g. 0o644; None when unavailable.
    pub perms: Option<u32>,
    /// Numeric owner and group, which is all SFTP carries. None when the server
    /// omits them.
    pub uid: Option<u32>,
    pub gid: Option<u32>,
    /// Modification time in Unix seconds; None when the server omits it.
    pub mtime: Option<u32>,
}
#[derive(Default)]
pub struct DirectoryState {
    pub path: String,
    pub entries: Vec<Entry>,
    pub error: Option<String>,
    pub loading: bool,
}
pub fn list_directory(
    connection: Arc<Connection>,
    path: String,
    wake: Wake,
) -> Arc<Mutex<DirectoryState>> {
    let state = Arc::new(Mutex::new(DirectoryState {
        path: path.clone(),
        loading: true,
        ..Default::default()
    }));
    let output = state.clone();
    runtime().spawn(async move {
        let result: Result<_> = async {
            let sftp = connection.sftp("浏览文件").await?;
            let canonical = sftp.canonicalize(path).await?;
            let mut entries: Vec<_> = sftp
                .read_dir(&canonical)
                .await?
                .filter(|e| e.file_name() != "." && e.file_name() != "..")
                .map(|e| {
                    let metadata = e.metadata();
                    Entry {
                        name: e.file_name(),
                        directory: metadata.is_dir(),
                        // The listing reports the link itself, not its target.
                        symlink: e.file_type().is_symlink(),
                        size: metadata.size.unwrap_or(0),
                        perms: metadata.permissions,
                        uid: metadata.uid,
                        gid: metadata.gid,
                        mtime: metadata.mtime,
                    }
                })
                .collect();
            entries.sort_by(|a, b| {
                b.directory
                    .cmp(&a.directory)
                    .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
            });
            Ok((canonical, entries))
        }
        .await;
        let mut s = output.lock().unwrap();
        s.loading = false;
        match result {
            Ok((path, entries)) => {
                s.path = path;
                s.entries = entries;
            }
            Err(e) => s.error = Some(format!("{e:#}")),
        }
        wake();
    });
    state
}

/// Joins a child name onto a remote POSIX directory path.
pub(super) fn join_remote(parent: &str, name: &str) -> String {
    format!("{}/{}", parent.trim_end_matches('/'), name)
}

/// Removes a remote file, or a directory and everything beneath it.
pub async fn remove(
    sftp: &Arc<russh_sftp::client::SftpSession>,
    path: &str,
    directory: bool,
) -> Result<()> {
    if !directory {
        sftp.remove_file(path).await?;
        return Ok(());
    }
    // Collect breadth-first, then delete deepest-first: SFTP's `remove_dir` only
    // accepts an empty directory. A symlink is unlinked as a link and never
    // descended into, so a link pointing back at an ancestor cannot loop.
    let mut directories = vec![path.to_string()];
    let mut index = 0;
    while index < directories.len() {
        let dir = directories[index].clone();
        index += 1;
        for entry in sftp.read_dir(&dir).await? {
            let name = entry.file_name();
            if name == "." || name == ".." {
                continue;
            }
            let child = join_remote(&dir, &name);
            if entry.file_type().is_dir() {
                directories.push(child);
            } else {
                sftp.remove_file(&child).await?;
            }
        }
    }
    for dir in directories.iter().rev() {
        sftp.remove_dir(dir).await?;
    }
    Ok(())
}

/// Whether a remote directory is empty, so a recursive delete can be confirmed
/// before it happens rather than after.
pub async fn directory_is_empty(
    sftp: &Arc<russh_sftp::client::SftpSession>,
    path: &str,
) -> Result<bool> {
    for entry in sftp.read_dir(path).await? {
        let name = entry.file_name();
        if name != "." && name != ".." {
            return Ok(false);
        }
    }
    Ok(true)
}
