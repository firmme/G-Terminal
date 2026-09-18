//! One-off SFTP operations and the quoting and naming rules they rely on.

use super::*;

pub(super) trait ReadSeek:
    tokio::io::AsyncRead + tokio::io::AsyncSeek + Unpin + Send
{
}
impl<T: tokio::io::AsyncRead + tokio::io::AsyncSeek + Unpin + Send> ReadSeek for T {}
pub(super) trait ReadWriteSeek:
    tokio::io::AsyncRead + tokio::io::AsyncWrite + tokio::io::AsyncSeek + Unpin + Send
{
}
impl<T: tokio::io::AsyncRead + tokio::io::AsyncWrite + tokio::io::AsyncSeek + Unpin + Send>
    ReadWriteSeek for T
{
}

pub fn quote_posix(value: &str) -> Result<String> {
    ensure!(!value.contains('\0'), "路径不能包含 NUL");
    Ok(format!("'{}'", value.replace('\'', "'\\''")))
}

/// Runs a command on a fresh channel and returns its stdout.
///
/// `command` is passed to the remote shell verbatim, so every caller has to quote
/// whatever it interpolates — see [`quote_posix`].
pub async fn run_capture(connection: &Arc<Connection>, command: &str) -> Result<String> {
    let mut channel = connection.handle.channel_open_session().await?;
    channel.exec(true, command).await?;
    let mut output = String::new();
    let mut code = None;
    while let Some(message) = channel.wait().await {
        match message {
            russh::ChannelMsg::Data { data } | russh::ChannelMsg::ExtendedData { data, .. } => {
                // Bounded: this is for reading a small value back, not a stream.
                if output.len() < 4096 {
                    output.push_str(&String::from_utf8_lossy(&data));
                }
            }
            russh::ChannelMsg::ExitStatus { exit_status } => code = Some(exit_status),
            _ => {}
        }
    }
    ensure!(code == Some(0), "命令失败：{}", output.trim());
    Ok(output)
}

/// Resolves a user name to its numeric id. A number is taken as-is, because SFTP
/// carries ids and not names.
pub async fn resolve_uid(connection: &Arc<Connection>, name: &str) -> Result<u32> {
    let name = name.trim();
    if let Ok(id) = name.parse() {
        return Ok(id);
    }
    ensure!(!name.is_empty(), "请填写所有者");
    let output = run_capture(connection, &format!("id -u {}", quote_posix(name)?)).await?;
    output
        .trim()
        .parse()
        .with_context(|| format!("无法解析用户 {name} 的 uid"))
}

/// Resolves a group name to its numeric id. A number is taken as-is.
///
/// `getent group` is the direct lookup; `id -g` is the fallback for a system
/// without `getent`, where the name is more likely a user whose primary group is
/// wanted.
pub async fn resolve_gid(connection: &Arc<Connection>, name: &str) -> Result<u32> {
    let name = name.trim();
    if let Ok(id) = name.parse() {
        return Ok(id);
    }
    ensure!(!name.is_empty(), "请填写用户组");
    let quoted = quote_posix(name)?;
    let command = format!("getent group {quoted} 2>/dev/null | cut -d: -f3 || id -g {quoted}");
    let output = run_capture(connection, &command).await?;
    output
        .trim()
        .parse()
        .with_context(|| format!("无法解析用户组 {name} 的 gid"))
}

/// An attribute set that asks for nothing.
///
/// `FileAttributes::default()` is **not** empty: it carries `uid: Some(0)`,
/// `gid: Some(0)`, a mode and timestamps. Spreading it into a request would make
/// a plain chmod also try to take ownership of the file, so every field that
/// should be left alone has to be named as `None` here.
pub(super) fn no_attributes() -> russh_sftp::protocol::FileAttributes {
    russh_sftp::protocol::FileAttributes {
        size: None,
        uid: None,
        user: None,
        gid: None,
        group: None,
        permissions: None,
        atime: None,
        mtime: None,
    }
}

/// Changes a remote entry's mode, and nothing else.
pub async fn set_permissions(
    sftp: &Arc<russh_sftp::client::SftpSession>,
    path: &str,
    mode: u32,
) -> Result<()> {
    let mut attrs = no_attributes();
    attrs.permissions = Some(mode);
    sftp.set_metadata(path, attrs).await?;
    Ok(())
}

/// Changes a remote entry's owner and group.
///
/// Separate from [`set_permissions`] on purpose: SFTP carries both in one SETSTAT,
/// but a server refuses the ownership half for anyone who is not root. Sent
/// together, that refusal would take the mode change down with it.
pub async fn set_owner(
    sftp: &Arc<russh_sftp::client::SftpSession>,
    path: &str,
    uid: Option<u32>,
    gid: Option<u32>,
) -> Result<()> {
    let mut attrs = no_attributes();
    attrs.uid = uid;
    attrs.gid = gid;
    sftp.set_metadata(path, attrs).await?;
    Ok(())
}

/// Every path at or below `root`, parents before their children.
///
/// A symlink is listed but never descended into. `read_dir` follows links, so
/// descending into one that points at an ancestor would walk forever — the same
/// reason the recursive delete refuses to follow them.
pub async fn walk_remote(
    sftp: &Arc<russh_sftp::client::SftpSession>,
    root: &str,
) -> Result<Vec<String>> {
    let mut paths = vec![root.to_string()];
    let mut queue = std::collections::VecDeque::from([root.to_string()]);
    while let Some(directory) = queue.pop_front() {
        // A file has no children; asking would only raise an error.
        let Ok(entries) = sftp.read_dir(&directory).await else {
            continue;
        };
        for entry in entries {
            let name = entry.file_name();
            if name == "." || name == ".." {
                continue;
            }
            let child = join_remote(&directory, &name);
            // Only what the listing calls a directory is descended into. The check
            // is on the link itself, not on what it points at.
            if entry.file_type().is_dir() {
                queue.push_back(child.clone());
            }
            paths.push(child);
        }
    }
    Ok(paths)
}

/// A remote basename must remain one ordinary local filename on Windows too.
pub fn safe_local_name(name: &str) -> bool {
    if name.is_empty()
        || name.ends_with(['.', ' '])
        || name
            .chars()
            .any(|c| c.is_control() || "<>:\"/\\|?*".contains(c))
    {
        return false;
    }
    let stem = name.split('.').next().unwrap_or("").to_ascii_uppercase();
    !matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        && !((stem.starts_with("COM") || stem.starts_with("LPT"))
            && stem.len() == 4
            && matches!(stem.as_bytes()[3], b'1'..=b'9'))
}
