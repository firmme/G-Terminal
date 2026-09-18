//! The copy engine behind a transfer: conflict resolution, resuming and the
//! per-direction loops.

use super::*;

/// Where a transfer will actually land, once any conflict is settled.
pub(super) struct Resolved {
    local: PathBuf,
    remote: String,
    /// An existing target is being replaced, so a partial left by an earlier
    /// attempt has to be discarded rather than resumed against.
    replacing: bool,
}

/// Decides the destination, asking the user when something is already there.
///
/// Runs before anything is opened: a rename has to move the `.part` sibling with
/// it, so this cannot wait until the transfer is under way.
pub(super) async fn resolve_destination(
    sftp: &Arc<russh_sftp::client::SftpSession>,
    local: &Path,
    remote: &str,
    direction: Direction,
    control: &TransferControl,
    wake: &Wake,
) -> Result<Resolved> {
    use std::sync::atomic::Ordering;
    let mut local = local.to_path_buf();
    let mut remote = remote.to_string();
    // A rename can land on another existing name; give up rather than spin.
    for _ in 0..8 {
        let (overwrite_all, rename_all, skip_all, cancelled) = {
            let policy = control.policy.lock().unwrap();
            (
                policy.overwrite_all,
                policy.rename_all,
                policy.skip_all,
                policy.cancelled,
            )
        };
        ensure!(!cancelled, "已取消");
        let Some(existing) = existing_size(sftp, &local, &remote, direction).await? else {
            return Ok(Resolved {
                local,
                remote,
                replacing: false,
            });
        };
        // From here on the target exists.
        if control.overwrite || overwrite_all {
            return Ok(Resolved {
                local,
                remote,
                replacing: true,
            });
        }
        if skip_all {
            control.skipped.store(true, Ordering::Release);
            bail!("已跳过");
        }
        if rename_all {
            rename_to_free_name(sftp, direction, &mut local, &mut remote).await?;
            continue;
        }
        let (send, receive) = oneshot::channel();
        let incoming = incoming_size(sftp, &local, &remote, direction).await?;
        if control
            .conflicts
            .send(Conflict {
                name: remote_file_name(&remote),
                existing,
                incoming,
                batch_size: control.batch_size,
                batch: control.batch,
                answer: Some(send),
            })
            .is_err()
        {
            bail!("文件窗口已关闭，无法确认是否覆盖");
        }
        (*wake)();
        let choice = receive.await.context("文件窗口已关闭")?;
        match choice {
            ConflictChoice::Overwrite => {
                return Ok(Resolved {
                    local,
                    remote,
                    replacing: true,
                });
            }
            ConflictChoice::OverwriteAll => {
                control.policy.lock().unwrap().overwrite_all = true;
                return Ok(Resolved {
                    local,
                    remote,
                    replacing: true,
                });
            }
            ConflictChoice::Rename => {
                rename_to_free_name(sftp, direction, &mut local, &mut remote).await?;
            }
            ConflictChoice::RenameAll => {
                control.policy.lock().unwrap().rename_all = true;
                rename_to_free_name(sftp, direction, &mut local, &mut remote).await?;
            }
            ConflictChoice::Skip => {
                control.skipped.store(true, Ordering::Release);
                bail!("已跳过");
            }
            ConflictChoice::SkipAll => {
                control.policy.lock().unwrap().skip_all = true;
                control.skipped.store(true, Ordering::Release);
                bail!("已跳过");
            }
            ConflictChoice::CancelRemaining => {
                control.policy.lock().unwrap().cancel_remaining();
                control.cancel.store(true, Ordering::Release);
                bail!("已取消");
            }
        }
    }
    bail!("同名文件过多，请手动换个名字")
}

/// Size of whatever is already at the destination, or None when nothing is.
pub(super) async fn existing_size(
    sftp: &Arc<russh_sftp::client::SftpSession>,
    local: &Path,
    remote: &str,
    direction: Direction,
) -> Result<Option<u64>> {
    match direction {
        Direction::Upload => {
            if !sftp.try_exists(remote).await? {
                return Ok(None);
            }
            Ok(sftp.metadata(remote).await?.size)
        }
        Direction::Download => Ok(tokio::fs::metadata(local).await.ok().map(|m| m.len())),
    }
}

/// Size of the incoming file, for the conflict dialog.
pub(super) async fn incoming_size(
    sftp: &Arc<russh_sftp::client::SftpSession>,
    local: &Path,
    remote: &str,
    direction: Direction,
) -> Result<u64> {
    Ok(match direction {
        Direction::Upload => tokio::fs::metadata(local).await?.len(),
        Direction::Download => sftp.metadata(remote).await?.size.unwrap_or(0),
    })
}

/// Renames the receiving side to the first free numbered name. Only that side
/// moves: the sending side is the source and keeps its name, or the transfer goes
/// looking for a file nobody wrote.
pub(super) async fn rename_to_free_name(
    sftp: &Arc<russh_sftp::client::SftpSession>,
    direction: Direction,
    local: &mut PathBuf,
    remote: &mut String,
) -> Result<()> {
    let candidate = free_name(sftp, direction, local, remote).await?;
    match direction {
        Direction::Upload => *remote = join_remote(&parent_remote(remote.as_str()), &candidate),
        Direction::Download => {
            *local = local.parent().unwrap_or(Path::new("")).join(&candidate);
        }
    }
    Ok(())
}

/// The first `name (n).ext` that is still free on the receiving side.
pub(super) async fn free_name(
    sftp: &Arc<russh_sftp::client::SftpSession>,
    direction: Direction,
    local: &Path,
    remote: &str,
) -> Result<String> {
    let name = remote_file_name(remote);
    for index in 1..10_000u32 {
        let candidate = numbered(&name, index);
        let taken = match direction {
            Direction::Upload => {
                sftp.try_exists(join_remote(&parent_remote(remote), &candidate))
                    .await?
            }
            // An unreadable local path counts as taken, which errs toward asking
            // for the next name rather than clobbering something.
            Direction::Download => {
                tokio::fs::try_exists(local.parent().unwrap_or(Path::new("")).join(&candidate))
                    .await
                    .unwrap_or(true)
            }
        };
        if !taken {
            return Ok(candidate);
        }
    }
    bail!("同名文件过多，无法自动重命名")
}

/// `report.txt` at 2 becomes `report (2).txt`. A leading dot starts the name, not
/// an extension, so `.bashrc` stays whole.
pub(super) fn numbered(name: &str, index: u32) -> String {
    match name.rsplit_once('.') {
        Some((stem, ext)) if !stem.is_empty() && !ext.is_empty() => {
            format!("{stem} ({index}).{ext}")
        }
        _ => format!("{name} ({index})"),
    }
}

pub(super) fn remote_file_name(path: &str) -> String {
    path.rsplit('/').next().unwrap_or(path).to_string()
}

/// The directory holding an absolute POSIX path.
pub(super) fn parent_remote(path: &str) -> String {
    match path.rfind('/') {
        Some(0) | None => "/".to_string(),
        Some(index) => path[..index].to_string(),
    }
}

pub(super) async fn transfer_file(
    sftp: &Arc<russh_sftp::client::SftpSession>,
    local: &Path,
    remote: &str,
    direction: Direction,
    state: &Arc<Mutex<TransferState>>,
    control: &TransferControl,
    wake: &Wake,
) -> Result<()> {
    use russh_sftp::protocol::OpenFlags;
    use std::io::SeekFrom;
    // Decide the target before anything is opened. A rename changes not just the
    // destination but the `.part` sibling derived from it, so resolving later
    // would leave the partial file under the name the user just declined.
    let decided = resolve_destination(sftp, local, remote, direction, control, wake).await?;
    let local = &decided.local;
    let remote = decided.remote.as_str();
    let mut buffer = vec![0u8; 64 * 1024];
    let mut verify = vec![0u8; 64 * 1024];
    let partial_remote = format!("{remote}.gterminal.part");
    let partial_local = PathBuf::from(format!("{}.gterminal.part", local.display()));
    control.checkpoint()?;
    let (mut source, mut destination, total, offset): (
        Box<dyn ReadSeek>,
        Box<dyn ReadWriteSeek>,
        u64,
        u64,
    ) = match direction {
        Direction::Upload => {
            if decided.replacing {
                // Replacing means different bytes, so a partial left by an earlier
                // pass must not be resumed against — its prefix would not match and
                // the transfer would refuse to continue.
                let _ = sftp.remove_file(&partial_remote).await;
            }
            let source = tokio::fs::File::open(local).await?;
            let total = source.metadata().await?.len();
            let file = sftp
                .open_with_flags(
                    &partial_remote,
                    OpenFlags::CREATE | OpenFlags::READ | OpenFlags::WRITE,
                )
                .await?;
            let offset = file.metadata().await?.size.unwrap_or(0);
            (Box::new(source), Box::new(file), total, offset)
        }
        Direction::Download => {
            if decided.replacing {
                let _ = tokio::fs::remove_file(&partial_local).await;
            }
            let source = sftp.open(remote).await?;
            let total = source.metadata().await?.size.unwrap_or(0);
            let file = tokio::fs::OpenOptions::new()
                .create(true)
                .truncate(false)
                .read(true)
                .write(true)
                .open(&partial_local)
                .await?;
            let offset = file.metadata().await?.len();
            (Box::new(source), Box::new(file), total, offset)
        }
    };
    ensure!(
        offset <= total,
        "断点文件大于源文件，请清理 .gterminal.part 后重试"
    );
    {
        let mut s = state.lock().unwrap();
        s.total = total;
        s.done = 0;
        s.message = "校验断点前缀".into();
    }
    let mut checked = 0;
    while checked < offset {
        control.checkpoint()?;
        let n = (offset - checked).min(buffer.len() as u64) as usize;
        source.read_exact(&mut buffer[..n]).await?;
        destination.read_exact(&mut verify[..n]).await?;
        ensure!(
            buffer[..n] == verify[..n],
            "断点内容与源文件不一致，拒绝续传；请清理 .gterminal.part"
        );
        checked += n as u64;
    }
    source.seek(SeekFrom::Start(offset)).await?;
    destination.seek(SeekFrom::Start(offset)).await?;
    let mut done = offset;
    while done < total {
        control.checkpoint()?;
        let max = (total - done).min(buffer.len() as u64) as usize;
        let n = source.read(&mut buffer[..max]).await?;
        ensure!(n > 0, "源文件提前结束");
        destination.write_all(&buffer[..n]).await?;
        done += n as u64;
        {
            let mut s = state.lock().unwrap();
            s.done = done;
            s.message = "传输中".into();
        }
        wake();
    }
    destination.flush().await?;
    destination.shutdown().await?;
    drop(destination);
    drop(source);
    match direction {
        Direction::Upload => {
            if decided.replacing {
                // Some servers refuse to rename onto an existing path, which is
                // exactly the case here, so clear the target explicitly.
                let _ = sftp.remove_file(remote).await;
            } else {
                ensure!(
                    !sftp.try_exists(remote).await?,
                    "目标在传输期间已出现，保留临时文件"
                );
            }
            sftp.rename(partial_remote, remote).await?;
        }
        Direction::Download => {
            if decided.replacing {
                // `hard_link` refuses an existing target, so clear the one the user
                // agreed to replace before committing.
                let _ = tokio::fs::remove_file(local).await;
            }
            tokio::fs::hard_link(&partial_local, local)
                .await
                .context("提交下载失败，保留断点文件（目标必须不存在且文件系统支持硬链接）")?;
            tokio::fs::remove_file(partial_local).await?;
        }
    }
    Ok(())
}
