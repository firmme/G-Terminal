//! Explicit ZMODEM transfers over a dedicated binary SSH exec channel.
use anyhow::{Result, bail, ensure};
use std::{path::Path, time::Duration};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncSeekExt, AsyncWrite, AsyncWriteExt};
use zmodem2::{Action, Event, FileInfo, Position, Receiver, Sender};

pub async fn send<S: AsyncRead + AsyncWrite + Unpin>(stream: &mut S, path: &Path) -> Result<()> {
    let mut file = tokio::fs::File::open(path).await?;
    let size = file.metadata().await?.len();
    ensure!(size <= u32::MAX as u64, "ZMODEM 单文件最大 4 GiB");
    let name = path
        .file_name()
        .ok_or_else(|| anyhow::anyhow!("无文件名"))?
        .to_string_lossy();
    let mut sender = Sender::new()?;
    sender.start_file(FileInfo::new(
        name.as_bytes(),
        Some(Position::new(size as u32)),
    ))?;
    let mut buffer = vec![0; 8192];
    let mut pending = vec![];
    let mut retries = 0;
    loop {
        match sender.poll() {
            Action::WriteWire(bytes) => {
                let bytes = bytes.to_vec();
                stream.write_all(&bytes).await?;
                stream.flush().await?;
                sender.wire_written(bytes.len());
            }
            Action::ReadFile { offset, max_len } => {
                file.seek(std::io::SeekFrom::Start(offset.get() as u64))
                    .await?;
                let count = max_len.min(buffer.len());
                let n = file.read(&mut buffer[..count]).await?;
                sender.submit_file(&buffer[..n])?;
            }
            Action::Event(Event::FileCompleted) => sender.finish()?,
            Action::Event(Event::SessionCompleted) => {
                while let Action::WriteWire(bytes) = sender.poll() {
                    let bytes = bytes.to_vec();
                    stream.write_all(&bytes).await?;
                    stream.flush().await?;
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
                match tokio::time::timeout(Duration::from_secs(10), stream.read(&mut buffer)).await
                {
                    Ok(Ok(0)) => bail!("ZMODEM 连接提前关闭"),
                    Ok(Ok(n)) => {
                        ensure!(pending.len() + n < 65536, "ZMODEM 缓冲超过上限");
                        pending.extend_from_slice(&buffer[..n]);
                    }
                    Ok(Err(e)) => return Err(e.into()),
                    Err(_) => {
                        retries += 1;
                        ensure!(retries <= 6, "ZMODEM 超时");
                        sender.timeout()?;
                    }
                }
            }
            _ => {}
        }
    }
}

pub async fn receive<S: AsyncRead + AsyncWrite + Unpin>(stream: &mut S, path: &Path) -> Result<()> {
    ensure!(!path.exists(), "目标已存在");
    let partial = path.with_extension("zmodem.part");
    let mut file = tokio::fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&partial)
        .await?;
    let mut receiver = Receiver::new()?;
    receiver.set_manual_file_accept(true);
    let mut buffer = vec![0; 8192];
    let mut pending = vec![];
    let mut accepted = false;
    let mut completed = false;
    let mut retries = 0;
    loop {
        match receiver.poll() {
            Action::WriteWire(bytes) => {
                let bytes = bytes.to_vec();
                stream.write_all(&bytes).await?;
                stream.flush().await?;
                receiver.wire_written(bytes.len());
            }
            Action::WriteFile(bytes) => {
                ensure!(accepted, "服务器未声明文件");
                let bytes = bytes.to_vec();
                file.write_all(&bytes).await?;
                receiver.file_written(bytes.len())?;
            }
            Action::Event(Event::FileStarted(info)) => {
                ensure!(!accepted, "一次仅允许接收一个文件");
                ensure!(
                    !info.name.contains(&b'/') && !info.name.contains(&b'\\') && info.name != b"..",
                    "非法远程文件名"
                );
                accepted = true;
                receiver.accept_file_at(0)?;
            }
            Action::Event(Event::FileCompleted) => completed = true,
            Action::Event(Event::SessionCompleted) => {
                while let Action::WriteWire(bytes) = receiver.poll() {
                    let bytes = bytes.to_vec();
                    stream.write_all(&bytes).await?;
                    stream.flush().await?;
                    receiver.wire_written(bytes.len());
                }
                ensure!(completed, "ZMODEM 未完成文件");
                file.flush().await?;
                file.sync_all().await?;
                drop(file);
                tokio::fs::hard_link(&partial, path).await?;
                tokio::fs::remove_file(partial).await?;
                return Ok(());
            }
            Action::Event(Event::Aborted) => bail!("ZMODEM 已取消，保留临时文件"),
            Action::Idle => {
                if !pending.is_empty() {
                    let n = receiver.submit_wire(&pending)?;
                    if n > 0 {
                        pending.drain(..n);
                        continue;
                    }
                }
                match tokio::time::timeout(Duration::from_secs(10), stream.read(&mut buffer)).await
                {
                    Ok(Ok(0)) => bail!("ZMODEM 连接提前关闭"),
                    Ok(Ok(n)) => {
                        ensure!(pending.len() + n < 65536, "ZMODEM 缓冲超过上限");
                        pending.extend_from_slice(&buffer[..n]);
                    }
                    Ok(Err(e)) => return Err(e.into()),
                    Err(_) => {
                        retries += 1;
                        ensure!(retries <= 6, "ZMODEM 超时");
                        receiver.timeout()?;
                    }
                }
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn zmodem_roundtrip_uses_real_protocol_and_preserves_binary() {
        crate::remote::runtime().block_on(async {
            let dir = std::env::temp_dir().join(format!("gterminal-zmodem-{}", std::process::id()));
            std::fs::create_dir_all(&dir).unwrap();
            let source = dir.join("source.bin");
            let target = dir.join("received.bin");
            let bytes: Vec<u8> = (0..20000).map(|i| (i % 256) as u8).collect();
            std::fs::write(&source, &bytes).unwrap();
            let (mut a, mut b) = tokio::io::duplex(8192);
            let result = tokio::time::timeout(Duration::from_secs(15), async {
                tokio::try_join!(send(&mut a, &source), receive(&mut b, &target))
            })
            .await
            .unwrap();
            result.unwrap();
            assert_eq!(std::fs::read(&target).unwrap(), bytes);
            std::fs::remove_file(source).unwrap();
            std::fs::remove_file(target).unwrap();
            std::fs::remove_dir(dir).unwrap();
        });
    }
}
