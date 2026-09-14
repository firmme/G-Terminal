//! Explicit ZMODEM transfers over a dedicated binary SSH exec channel.
//!
//! Both directions run the same poll/submit loop; every step reports into a
//! shared [`Handle`] so the UI can show progress and cancel, and a conflict in
//! the receive path is resolved by asking the UI instead of failing.
use anyhow::{Context, Result, bail, ensure};
use std::{
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncSeekExt, AsyncWrite, AsyncWriteExt};
use zmodem2::{Action, Event, FileInfo, Position, Receiver, Sender};

/// How often the loop re-checks the cancel flag while waiting for the peer.
const IDLE_POLL: Duration = Duration::from_millis(200);
/// Silence tolerated before the protocol's own retry timer is advanced.
const SILENCE_LIMIT: Duration = Duration::from_secs(10);
/// lrzsz leaves ZMODEM mode and returns to its prompt after eight CAN bytes.
const CANCEL_WIRE: [u8; 8] = [0x18; 8];
/// Suffix of the scratch file a download is assembled in.
const PART_SUFFIX: &str = ".zmodem.part";

/// Live counters for one transfer, shared with the UI.
#[derive(Clone, Debug, Default)]
pub struct Progress {
    pub name: String,
    pub done: u64,
    pub total: u64,
    pub message: String,
}

impl Progress {
    pub fn fraction(&self) -> f32 {
        if self.total == 0 {
            0.0
        } else {
            (self.done as f64 / self.total as f64) as f32
        }
    }
}

/// Something the transfer cannot decide alone, e.g. a name already on disk.
#[derive(Clone, Debug)]
pub struct Question {
    pub target: PathBuf,
    /// Size of the file already at `target`, when one is there.
    pub existing: Option<u64>,
    /// Size of a resumable partial download, when one is present.
    pub partial: Option<u64>,
    /// Size the incoming file will have, when the sender declared it.
    pub total: u64,
}

impl Question {
    pub fn name(&self) -> String {
        self.target
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned()
    }
    pub fn can_resume(&self) -> bool {
        self.partial.is_some()
    }
}

#[derive(Clone, Debug)]
pub enum Decision {
    /// Replace the file already at the target once the bytes are complete.
    Overwrite,
    /// Continue where a previous partial download stopped.
    Resume,
    /// Save under another name in the same directory.
    Rename(String),
    Cancel,
}

/// The UI end of a transfer: where questions are asked and answers arrive.
pub struct Ask {
    question: tokio::sync::mpsc::UnboundedSender<Question>,
    answer: tokio::sync::Mutex<tokio::sync::mpsc::UnboundedReceiver<Decision>>,
}

/// Creates the two ends of the question/answer link. The [`Ask`] half travels
/// with the transfer task, the two channels stay with the UI.
pub fn ask_channel() -> (
    Ask,
    tokio::sync::mpsc::UnboundedReceiver<Question>,
    tokio::sync::mpsc::UnboundedSender<Decision>,
) {
    let (question_tx, question_rx) = tokio::sync::mpsc::unbounded_channel();
    let (answer_tx, answer_rx) = tokio::sync::mpsc::unbounded_channel();
    (
        Ask {
            question: question_tx,
            answer: tokio::sync::Mutex::new(answer_rx),
        },
        question_rx,
        answer_tx,
    )
}

impl Ask {
    async fn ask(&self, question: Question) -> Result<Decision> {
        self.question
            .send(question)
            .map_err(|_| anyhow::anyhow!("文件窗口已关闭"))?;
        self.answer
            .lock()
            .await
            .recv()
            .await
            .ok_or_else(|| anyhow::anyhow!("文件窗口已关闭"))
    }
}

/// Transfer-wide state: progress readout, cancel flag and the optional UI link.
pub struct Handle {
    pub progress: Arc<Mutex<Progress>>,
    pub cancel: Arc<AtomicBool>,
    ask: Option<Ask>,
}

impl Handle {
    pub fn new(name: impl Into<String>, ask: Option<Ask>) -> Self {
        Self {
            progress: Arc::new(Mutex::new(Progress {
                name: name.into(),
                ..Default::default()
            })),
            cancel: Arc::new(AtomicBool::new(false)),
            ask,
        }
    }

    /// A handle with no UI: conflicts are resolved by policy, nobody cancels.
    pub fn detached() -> Self {
        Self::new(String::new(), None)
    }

    fn shared(&self) -> std::sync::MutexGuard<'_, Progress> {
        self.progress.lock().unwrap_or_else(|e| e.into_inner())
    }
    fn begin(&self, total: u64, message: &str) {
        let mut p = self.shared();
        p.done = 0;
        p.total = total;
        p.message = message.into();
    }
    fn set(&self, done: u64, total: u64, message: &str) {
        let mut p = self.shared();
        p.done = done;
        p.total = total;
        p.message = message.into();
    }
    fn message(&self, message: &str) {
        self.shared().message = message.into();
    }
    fn cancelled(&self) -> bool {
        self.cancel.load(Ordering::Relaxed)
    }
}

/// Scratch file a download is assembled in before it takes the target name.
fn partial_path(target: &Path) -> PathBuf {
    let mut name = target.file_name().unwrap_or_default().to_os_string();
    name.push(PART_SUFFIX);
    target.with_file_name(name)
}

/// Tells the peer to abandon the session, so its shell prompt comes back
/// instead of waiting out the ZMODEM retry timer.
async fn abort_wire<S: AsyncWrite + Unpin>(stream: &mut S) {
    let _ = stream.write_all(&CANCEL_WIRE).await;
    let _ = stream.flush().await;
}

async fn flush_wire<S: AsyncWrite + Unpin>(stream: &mut S, bytes: &[u8]) -> Result<()> {
    stream.write_all(bytes).await?;
    stream.flush().await?;
    Ok(())
}

pub async fn send<S: AsyncRead + AsyncWrite + Unpin>(
    stream: &mut S,
    path: &Path,
    handle: &Handle,
) -> Result<()> {
    let mut file = tokio::fs::File::open(path).await?;
    let size = file.metadata().await?.len();
    ensure!(size <= u32::MAX as u64, "ZMODEM 单文件最大 4 GiB");
    let name = path
        .file_name()
        .ok_or_else(|| anyhow::anyhow!("无文件名"))?
        .to_string_lossy()
        .into_owned();
    handle.shared().name = name.clone();
    handle.begin(size, "等待接收方响应");
    let mut sender = Sender::new()?;
    sender.start_file(FileInfo::new(
        name.as_bytes(),
        Some(Position::new(size as u32)),
    ))?;
    let outcome = send_loop(stream, &mut sender, &mut file, size, handle).await;
    match &outcome {
        Ok(()) => handle.set(size, size, "发送完成"),
        Err(e) => {
            sender.abort();
            handle.message(&format!("发送失败：{e}"));
            abort_wire(stream).await;
        }
    }
    outcome
}

async fn send_loop<S: AsyncRead + AsyncWrite + Unpin>(
    stream: &mut S,
    sender: &mut Sender,
    file: &mut tokio::fs::File,
    size: u64,
    handle: &Handle,
) -> Result<()> {
    let mut buffer = vec![0; 8192];
    let mut pending = vec![];
    let mut retries = 0u32;
    let mut silence = Duration::ZERO;
    loop {
        if handle.cancelled() {
            bail!("已取消发送");
        }
        match sender.poll() {
            Action::WriteWire(bytes) => {
                let bytes = bytes.to_vec();
                flush_wire(stream, &bytes).await?;
                sender.wire_written(bytes.len());
            }
            Action::ReadFile { offset, max_len } => {
                file.seek(std::io::SeekFrom::Start(offset.get() as u64))
                    .await?;
                let count = max_len.min(buffer.len());
                let n = file.read(&mut buffer[..count]).await?;
                sender.submit_file(&buffer[..n])?;
                handle.set(offset.get() as u64 + n as u64, size, "发送中");
            }
            Action::Event(Event::FileCompleted) => sender.finish()?,
            Action::Event(Event::SessionCompleted) => {
                while let Action::WriteWire(bytes) = sender.poll() {
                    let bytes = bytes.to_vec();
                    flush_wire(stream, &bytes).await?;
                    sender.wire_written(bytes.len());
                }
                return Ok(());
            }
            Action::Event(Event::Aborted) => bail!("ZMODEM 已取消"),
            Action::Idle => {
                if !pending.is_empty() {
                    let n = sender.submit_wire(&pending)?;
                    if n > 0 {
                        pending.drain(..n);
                        continue;
                    }
                }
                match tokio::time::timeout(IDLE_POLL, stream.read(&mut buffer)).await {
                    Ok(Ok(0)) => bail!("ZMODEM 连接提前关闭"),
                    Ok(Ok(n)) => {
                        ensure!(pending.len() + n < 65536, "ZMODEM 缓冲超过上限");
                        pending.extend_from_slice(&buffer[..n]);
                        silence = Duration::ZERO;
                    }
                    Ok(Err(e)) => return Err(e.into()),
                    Err(_) => {
                        silence += IDLE_POLL;
                        if silence >= SILENCE_LIMIT {
                            silence = Duration::ZERO;
                            retries += 1;
                            ensure!(retries <= 6, "ZMODEM 超时：对端无响应");
                            sender.timeout()?;
                        }
                    }
                }
            }
            _ => {}
        }
    }
}

#[derive(Clone, Copy, Default, PartialEq)]
enum Commit {
    /// Link the scratch file into place, refusing to clobber.
    #[default]
    Create,
    /// Replace whatever sits at the target.
    Replace,
}

/// Everything the receive path tracks about the file it is assembling.
#[derive(Default)]
struct Incoming {
    target: PathBuf,
    file: Option<tokio::fs::File>,
    partial: Option<PathBuf>,
    commit: Commit,
    /// Set when the bytes are complete but committing them failed, so the
    /// scratch file must survive for the user to pick up by hand.
    keep_partial: bool,
}

pub async fn receive<S: AsyncRead + AsyncWrite + Unpin>(
    stream: &mut S,
    path: &Path,
    handle: &Handle,
) -> Result<PathBuf> {
    let mut incoming = Incoming {
        // When handed a directory the file name comes from the ZMODEM header.
        target: if path.is_dir() {
            PathBuf::new()
        } else {
            path.to_path_buf()
        },
        ..Default::default()
    };
    let outcome = receive_loop(stream, path, handle, &mut incoming).await;
    if let Err(e) = &outcome {
        drop(incoming.file.take());
        let keep = incoming.keep_partial;
        if let Some(part) = incoming.partial.take()
            && !keep
        {
            let _ = tokio::fs::remove_file(&part).await;
        }
        if !keep {
            // Hand the remote shell back its prompt instead of leaving the
            // peer retrying a session nobody is listening to.
            abort_wire(stream).await;
        }
        handle.message(&format!("接收失败：{e}"));
    }
    outcome
}

async fn receive_loop<S: AsyncRead + AsyncWrite + Unpin>(
    stream: &mut S,
    path: &Path,
    handle: &Handle,
    incoming: &mut Incoming,
) -> Result<PathBuf> {
    let mut receiver = Receiver::new()?;
    receiver.set_manual_file_accept(true);
    let mut buffer = vec![0; 8192];
    let mut pending = vec![];
    let mut accepted = false;
    let mut completed = false;
    let mut written = 0u64;
    let mut total = 0u64;
    let mut retries = 0u32;
    let mut silence = Duration::ZERO;
    loop {
        if handle.cancelled() {
            bail!("已取消接收");
        }
        match receiver.poll() {
            Action::WriteWire(bytes) => {
                let bytes = bytes.to_vec();
                flush_wire(stream, &bytes).await?;
                receiver.wire_written(bytes.len());
            }
            Action::WriteFile(bytes) => {
                ensure!(accepted, "服务器未声明文件");
                let bytes = bytes.to_vec();
                incoming
                    .file
                    .as_mut()
                    .ok_or_else(|| anyhow::anyhow!("文件未打开"))?
                    .write_all(&bytes)
                    .await?;
                written += bytes.len() as u64;
                receiver.file_written(bytes.len())?;
                handle.set(written, total, "接收中");
            }
            Action::Event(Event::FileStarted(info)) => {
                ensure!(!accepted, "一次仅允许接收一个文件");
                ensure!(
                    !info.name.contains(&b'/') && !info.name.contains(&b'\\') && info.name != b"..",
                    "非法远程文件名"
                );
                if incoming.target.as_os_str().is_empty() {
                    incoming.target = path.join(String::from_utf8_lossy(info.name).trim());
                }
                ensure!(!incoming.target.as_os_str().is_empty(), "无文件名");
                total = info.size.map_or(0, |s| s.get() as u64);
                handle.shared().name = incoming
                    .target
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .into_owned();
                handle.begin(total, "等待确认保存位置");
                let start = resolve_conflict(handle, incoming, total).await?;
                let part = partial_path(&incoming.target);
                incoming.file = Some(if start > 0 {
                    ensure!(
                        tokio::fs::try_exists(&part).await.unwrap_or(false),
                        "续传文件已被移除"
                    );
                    tokio::fs::OpenOptions::new().append(true).open(&part).await?
                } else {
                    let _ = tokio::fs::remove_file(&part).await;
                    tokio::fs::OpenOptions::new()
                        .write(true)
                        .create_new(true)
                        .open(&part)
                        .await?
                });
                incoming.partial = Some(part);
                written = start;
                accepted = true;
                accept(&mut receiver, stream, start as u32).await?;
                handle.set(written, total, "接收中");
            }
            Action::Event(Event::FileCompleted) => completed = true,
            Action::Event(Event::SessionCompleted) => {
                while let Action::WriteWire(bytes) = receiver.poll() {
                    let bytes = bytes.to_vec();
                    flush_wire(stream, &bytes).await?;
                    receiver.wire_written(bytes.len());
                }
                ensure!(completed, "ZMODEM 未完成文件");
                let mut file = incoming
                    .file
                    .take()
                    .ok_or_else(|| anyhow::anyhow!("文件未打开"))?;
                file.flush().await?;
                file.sync_all().await?;
                drop(file);
                let part = incoming
                    .partial
                    .take()
                    .ok_or_else(|| anyhow::anyhow!("文件未打开"))?;
                if let Err(e) = commit(incoming, &part).await {
                    // The bytes are good; keep the scratch file so a manual
                    // rename can still rescue the download.
                    incoming.partial = Some(part.clone());
                    incoming.keep_partial = true;
                    bail!("{e:#}；完整数据保留在 {}", part.display());
                }
                handle.set(written, written, "接收完成");
                return Ok(incoming.target.clone());
            }
            Action::Event(Event::Aborted) => bail!("ZMODEM 已取消"),
            Action::Idle => {
                if !pending.is_empty() {
                    let n = receiver.submit_wire(&pending)?;
                    if n > 0 {
                        pending.drain(..n);
                        continue;
                    }
                }
                match tokio::time::timeout(IDLE_POLL, stream.read(&mut buffer)).await {
                    Ok(Ok(0)) => bail!("ZMODEM 连接提前关闭"),
                    Ok(Ok(n)) => {
                        ensure!(pending.len() + n < 65536, "ZMODEM 缓冲超过上限");
                        pending.extend_from_slice(&buffer[..n]);
                        silence = Duration::ZERO;
                    }
                    Ok(Err(e)) => return Err(e.into()),
                    Err(_) => {
                        silence += IDLE_POLL;
                        if silence >= SILENCE_LIMIT {
                            silence = Duration::ZERO;
                            retries += 1;
                            ensure!(retries <= 6, "ZMODEM 超时：对端无响应");
                            receiver.timeout()?;
                        }
                    }
                }
            }
            _ => {}
        }
    }
}

/// Asks the receiver to start at `offset`, draining wire bytes when the state
/// machine still has some queued (it then reports backpressure).
async fn accept<S: AsyncRead + AsyncWrite + Unpin>(
    receiver: &mut Receiver,
    stream: &mut S,
    offset: u32,
) -> Result<()> {
    loop {
        match receiver.accept_file_at(offset) {
            Ok(()) => return Ok(()),
            Err(zmodem2::Error::Backpressure) => {
                let mut drained = false;
                while let Action::WriteWire(bytes) = receiver.poll() {
                    let bytes = bytes.to_vec();
                    flush_wire(stream, &bytes).await?;
                    receiver.wire_written(bytes.len());
                    drained = true;
                }
                ensure!(drained, "ZMODEM 状态机阻塞");
            }
            Err(e) => return Err(e.into()),
        }
    }
}

/// Puts the finished scratch file in place.
async fn commit(incoming: &Incoming, part: &Path) -> Result<()> {
    match incoming.commit {
        Commit::Replace => Ok(tokio::fs::rename(part, &incoming.target).await?),
        Commit::Create => {
            tokio::fs::hard_link(part, &incoming.target)
                .await
                .context("目标在传输期间出现，或文件系统不支持硬链接")?;
            tokio::fs::remove_file(part).await?;
            Ok(())
        }
    }
}

/// Decides where the incoming file goes, asking the UI whenever the answer is
/// not obvious. Returns the offset the sender should resume from.
async fn resolve_conflict(handle: &Handle, incoming: &mut Incoming, total: u64) -> Result<u64> {
    for _ in 0..8 {
        let existing = tokio::fs::metadata(&incoming.target)
            .await
            .ok()
            .map(|m| m.len());
        let part = partial_path(&incoming.target);
        let partial = tokio::fs::metadata(&part)
            .await
            .ok()
            .map(|m| m.len())
            .filter(|n| *n > 0);
        if existing.is_none() && partial.is_none() {
            incoming.commit = Commit::Create;
            return Ok(0);
        }
        let Some(ask) = &handle.ask else {
            // No UI attached: never clobber, but a partial is safe to resume.
            ensure!(
                existing.is_none(),
                "目标已存在：{}（不会自动覆盖）",
                incoming.target.display()
            );
            return Ok(partial.unwrap_or(0));
        };
        let question = Question {
            target: incoming.target.clone(),
            existing,
            partial,
            total,
        };
        match ask.ask(question).await? {
            Decision::Cancel => bail!("已取消接收"),
            Decision::Resume => {
                incoming.commit = Commit::Create;
                return Ok(partial.unwrap_or(0));
            }
            Decision::Overwrite => {
                if partial.is_some() {
                    let _ = tokio::fs::remove_file(&part).await;
                }
                incoming.commit = Commit::Replace;
                return Ok(0);
            }
            Decision::Rename(name) => {
                let name = name.trim().to_string();
                ensure!(
                    !name.is_empty() && !name.contains(['/', '\\']) && name != "..",
                    "文件名无效"
                );
                incoming.target = incoming.target.with_file_name(name);
            }
        }
    }
    bail!("文件名冲突未解决")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicU32;

    fn scratch(tag: &str) -> PathBuf {
        static SEQ: AtomicU32 = AtomicU32::new(0);
        let dir = std::env::temp_dir().join(format!(
            "gterminal-zmodem-{tag}-{}-{}",
            std::process::id(),
            SEQ.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn payload(len: usize) -> Vec<u8> {
        (0..len).map(|i| (i % 251) as u8).collect()
    }

    fn roundtrip(source: &Path, target: &Path) -> Result<PathBuf> {
        crate::remote::runtime().block_on(async {
            let (mut a, mut b) = tokio::io::duplex(8192);
            let (sender, receiver) = (Handle::detached(), Handle::detached());
            let (_, received) = tokio::time::timeout(Duration::from_secs(20), async {
                tokio::try_join!(
                    send(&mut a, source, &sender),
                    receive(&mut b, target, &receiver)
                )
            })
            .await
            .map_err(|_| anyhow::anyhow!("ZMODEM roundtrip timed out"))??;
            Ok::<PathBuf, anyhow::Error>(received)
        })
    }

    #[test]
    fn zmodem_roundtrip_uses_real_protocol_and_preserves_binary() {
        let dir = scratch("roundtrip");
        let source = dir.join("source.bin");
        let target = dir.join("received.bin");
        let bytes = payload(20_000);
        std::fs::write(&source, &bytes).unwrap();
        let saved = roundtrip(&source, &target).unwrap();
        assert_eq!(saved, target);
        assert_eq!(std::fs::read(&target).unwrap(), bytes);
        assert!(!partial_path(&target).exists());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn downloads_receive_into_a_directory_and_report_progress() {
        let dir = scratch("into-dir");
        let source = dir.join("payload.bin");
        let into = dir.join("inbox");
        std::fs::create_dir_all(&into).unwrap();
        let bytes = payload(9_000);
        std::fs::write(&source, &bytes).unwrap();
        let saved = roundtrip(&source, &into).unwrap();
        assert_eq!(saved, into.join("payload.bin"));
        assert_eq!(std::fs::read(&saved).unwrap(), bytes);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn an_existing_target_is_never_silently_replaced() {
        let dir = scratch("conflict");
        let source = dir.join("source.bin");
        let target = dir.join("source.bin.keep");
        std::fs::write(&source, payload(4_000)).unwrap();
        std::fs::write(&target, b"keep me").unwrap();
        let error = roundtrip(&source, &target).unwrap_err().to_string();
        assert!(error.contains("目标已存在"), "{error}");
        assert_eq!(std::fs::read(&target).unwrap(), b"keep me");
        // The scratch file must not be left behind for the next attempt to trip on.
        assert!(!partial_path(&target).exists());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_stale_partial_is_resumed_and_consumed() {
        let dir = scratch("resume");
        let source = dir.join("resume.bin");
        let into = dir.join("inbox");
        std::fs::create_dir_all(&into).unwrap();
        let bytes = payload(12_000);
        std::fs::write(&source, &bytes).unwrap();
        let target = into.join("resume.bin");
        let part = partial_path(&target);
        // A prefix left by an interrupted transfer is picked up, not discarded.
        std::fs::write(&part, &bytes[..5_000]).unwrap();
        let saved = roundtrip(&source, &into).unwrap();
        assert_eq!(saved, target);
        assert_eq!(std::fs::read(&target).unwrap(), bytes);
        assert!(!part.exists());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_renamed_download_lands_next_to_the_original() {
        let dir = scratch("rename");
        let source = dir.join("source.bin");
        let into = dir.join("inbox");
        std::fs::create_dir_all(&into).unwrap();
        let bytes = payload(6_000);
        std::fs::write(&source, &bytes).unwrap();
        std::fs::write(into.join("source.bin"), b"occupied").unwrap();
        let (ask, mut questions, answers) = ask_channel();
        let handle = Handle::new("source.bin", Some(ask));
        let saved = crate::remote::runtime().block_on(async {
            let (mut a, mut b) = tokio::io::duplex(8192);
            let answering = async {
                let question = questions
                    .recv()
                    .await
                    .ok_or_else(|| anyhow::anyhow!("传输没有提出问题"))?;
                assert!(question.existing.is_some());
                answers
                    .send(Decision::Rename("kept.bin".into()))
                    .map_err(|_| anyhow::anyhow!("文件窗口已关闭"))?;
                Ok::<(), anyhow::Error>(())
            };
            let (_, received, ()) = tokio::time::timeout(Duration::from_secs(20), async {
                tokio::try_join!(
                    send(&mut a, &source, &handle),
                    receive(&mut b, &into, &handle),
                    answering
                )
            })
            .await
            .map_err(|_| anyhow::anyhow!("ZMODEM roundtrip timed out"))??;
            Ok::<PathBuf, anyhow::Error>(received)
        })
        .unwrap();
        assert_eq!(saved, into.join("kept.bin"));
        assert_eq!(std::fs::read(into.join("kept.bin")).unwrap(), bytes);
        assert_eq!(std::fs::read(into.join("source.bin")).unwrap(), b"occupied");
        std::fs::remove_dir_all(dir).unwrap();
    }
}
