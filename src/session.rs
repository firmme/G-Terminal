use crate::{config::RemoteProfile, terminal::Terminal};
use anyhow::{Context, Result, bail};
use portable_pty::{ChildKiller, CommandBuilder, PtySize, native_pty_system};
use std::{
    io::{Read, Write},
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, SyncSender},
    },
    thread,
    time::Duration,
};

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub enum SessionKind {
    Local(String),
    Ssh(RemoteProfile),
    Sftp(RemoteProfile),
}

impl SessionKind {
    pub fn label(&self) -> String {
        match self {
            Self::Local(shell) => match shell.as_str() {
                "powershell" => "PowerShell".into(),
                "pwsh" => "PowerShell 7".into(),
                "cmd" => "Command Prompt".into(),
                "wsl" => "WSL".into(),
                _ => "Shell".into(),
            },
            Self::Ssh(p) => p.label(),
            Self::Sftp(p) => format!("SFTP · {}", p.label()),
        }
    }

    pub fn command(&self) -> Result<CommandBuilder> {
        let mut cmd = match self {
            Self::Local(shell) => {
                #[cfg(windows)]
                {
                    match shell.as_str() {
                        "powershell" => {
                            let mut c = CommandBuilder::new("powershell.exe");
                            c.arg("-NoLogo");
                            c
                        }
                        "pwsh" => {
                            let mut c = CommandBuilder::new("pwsh.exe");
                            c.arg("-NoLogo");
                            c
                        }
                        "cmd" => CommandBuilder::new("cmd.exe"),
                        "wsl" => CommandBuilder::new("wsl.exe"),
                        _ => bail!("未知 Shell: {shell}"),
                    }
                }
                #[cfg(not(windows))]
                {
                    let _ = shell;
                    CommandBuilder::new(std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".into()))
                }
            }
            Self::Ssh(profile) | Self::Sftp(profile) => {
                profile.validate()?;
                let sftp = matches!(self, Self::Sftp(_));
                let mut c = CommandBuilder::new(if sftp { "sftp" } else { "ssh" });
                if !sftp {
                    c.arg("-tt");
                }
                c.arg(if sftp { "-P" } else { "-p" });
                c.arg(profile.port.to_string());
                c.args([
                    "-o",
                    "ServerAliveInterval=30",
                    "-o",
                    "ServerAliveCountMax=3",
                ]);
                if !profile.identity.trim().is_empty() {
                    c.arg("-i");
                    c.arg(&profile.identity);
                }
                // Arguments are passed directly to OpenSSH, never interpolated into a shell.
                c.arg("--");
                c.arg(profile.destination());
                c
            }
        };
        cmd.env("TERM", "xterm-256color");
        cmd.env("COLORTERM", "truecolor");
        cmd.env("TERM_PROGRAM", "G-Terminal");
        if let Some(home) = directories::UserDirs::new().map(|d| PathBuf::from(d.home_dir())) {
            cmd.cwd(home);
        }
        Ok(cmd)
    }
}

enum Control {
    Write(Vec<u8>),
    Resize(PtySize),
}

#[derive(Clone, Debug, Default)]
pub struct SessionStatus {
    pub exit_code: Option<u32>,
    pub error: Option<String>,
    pub eof: bool,
}

pub struct Session {
    pub terminal: Arc<Mutex<Terminal>>,
    pub status: Arc<Mutex<SessionStatus>>,
    pub kind: SessionKind,
    pub pid: Option<u32>,
    pub size: (u16, u16),
    pub remote: Option<Arc<crate::remote::Connection>>,
    input: SyncSender<Control>,
    stop: Arc<AtomicBool>,
    killer: Option<Box<dyn ChildKiller + Send + Sync>>,
}

impl Session {
    pub fn disconnected(kind: SessionKind, scrollback: usize) -> Self {
        let (input, _) = mpsc::sync_channel(1);
        Self {
            terminal: Arc::new(Mutex::new(Terminal::new(30, 100, scrollback))),
            status: Arc::new(Mutex::new(SessionStatus {
                exit_code: Some(0),
                eof: true,
                error: None,
            })),
            kind,
            pid: None,
            size: (30, 100),
            remote: None,
            input,
            stop: Arc::new(AtomicBool::new(true)),
            killer: None,
        }
    }
    pub fn spawn(
        kind: SessionKind,
        scrollback: usize,
        wake: Arc<dyn Fn() + Send + Sync>,
    ) -> Result<Self> {
        let size = PtySize {
            rows: 30,
            cols: 100,
            pixel_width: 0,
            pixel_height: 0,
        };
        let pair = native_pty_system()
            .openpty(size)
            .context("无法创建系统 PTY（Windows 需要 10 1809 或更新版本）")?;
        let mut reader = pair.master.try_clone_reader()?;
        let mut writer = pair.master.take_writer()?;
        let mut child = pair.slave.spawn_command(kind.command()?).with_context(|| {
            format!(
                "无法启动 {}，请检查程序是否已安装并在 PATH 中",
                kind.label()
            )
        })?;
        let pid = child.process_id();
        let killer = child.clone_killer();
        drop(pair.slave);
        let terminal = Arc::new(Mutex::new(Terminal::new(size.rows, size.cols, scrollback)));
        let status = Arc::new(Mutex::new(SessionStatus::default()));
        let stop = Arc::new(AtomicBool::new(false));
        // Bounded input; output is parsed immediately into a bounded screen on a worker.
        let (input, receiver) = mpsc::sync_channel::<Control>(64);
        let output_terminal = terminal.clone();
        let output_status = status.clone();
        let output_wake = wake.clone();
        let reply_sender = input.clone();
        thread::spawn(move || {
            let mut bytes = [0u8; 16 * 1024];
            loop {
                match reader.read(&mut bytes) {
                    Ok(0) => break,
                    Ok(n) => {
                        let replies = {
                            let mut terminal =
                                output_terminal.lock().unwrap_or_else(|e| e.into_inner());
                            terminal.process(&bytes[..n]);
                            std::mem::take(&mut terminal.parser.callbacks_mut().replies)
                        };
                        if !replies.is_empty() {
                            let _ = reply_sender.try_send(Control::Write(replies));
                        }
                        output_wake();
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                    Err(e) => {
                        // Unix PTYs may report EIO on a normal slave close.
                        if e.raw_os_error() != Some(5) {
                            output_status.lock().unwrap().error = Some(e.to_string());
                        }
                        break;
                    }
                }
            }
            output_status.lock().unwrap().eof = true;
            output_wake();
        });
        let control_stop = stop.clone();
        let control_status = status.clone();
        let control_terminal = terminal.clone();
        let control_wake = wake.clone();
        thread::spawn(move || {
            let master = pair.master;
            while !control_stop.load(Ordering::Acquire) {
                match receiver.recv_timeout(Duration::from_millis(100)) {
                    Ok(control) => {
                        let result = match control {
                            Control::Write(bytes) => writer
                                .write_all(&bytes)
                                .and_then(|_| writer.flush())
                                .map_err(anyhow::Error::from),
                            Control::Resize(size) => master.resize(size).map(|_| {
                                control_terminal
                                    .lock()
                                    .unwrap()
                                    .resize(size.rows, size.cols)
                            }),
                        };
                        if let Err(e) = result {
                            control_status.lock().unwrap().error = Some(e.to_string());
                            control_wake();
                        }
                    }
                    Err(mpsc::RecvTimeoutError::Disconnected) => break,
                    Err(mpsc::RecvTimeoutError::Timeout) => {}
                }
            }
            drop(writer);
            drop(master);
        });
        let wait_status = status.clone();
        thread::spawn(move || {
            match child.wait() {
                Ok(exit) => wait_status.lock().unwrap().exit_code = Some(exit.exit_code()),
                Err(e) => wait_status.lock().unwrap().error = Some(e.to_string()),
            }
            wake();
        });
        Ok(Self {
            terminal,
            status,
            kind,
            pid,
            size: (size.rows, size.cols),
            input,
            stop,
            killer: Some(killer),
            remote: None,
        })
    }

    pub fn from_remote(
        connection: Arc<crate::remote::Connection>,
        scrollback: usize,
        wake: crate::remote::Wake,
    ) -> Self {
        let terminal = Arc::new(Mutex::new(Terminal::new(30, 100, scrollback)));
        let status = Arc::new(Mutex::new(SessionStatus::default()));
        let stop = Arc::new(AtomicBool::new(false));
        let (input, receiver) = mpsc::sync_channel::<Control>(64);
        let (out, state, stopped, connection_task) = (
            terminal.clone(),
            status.clone(),
            stop.clone(),
            connection.clone(),
        );
        crate::remote::runtime().spawn(async move {
            let result: Result<()> = async {
                let receiver = receiver;
                let mut channel = connection_task.handle.channel_open_session().await?;
                channel.request_pty(true, "xterm-256color", 100, 30, 0, 0, &[]).await?;
                channel.request_shell(true).await?;
                let mut tick = tokio::time::interval(Duration::from_millis(10));
                loop {
                    tokio::select! {
                        _ = tick.tick() => {
                            if stopped.load(Ordering::Acquire) { channel.close().await?; break; }
                            for _ in 0..16 {
                                match receiver.try_recv() {
                                    Ok(Control::Write(bytes)) => channel.data(&bytes[..]).await?,
                                    Ok(Control::Resize(size)) => { channel.window_change(size.cols as u32, size.rows as u32, 0, 0).await?; out.lock().unwrap().resize(size.rows, size.cols); },
                                    Err(_) => break,
                                }
                            }
                        }
                        message = channel.wait() => match message {
                            Some(russh::ChannelMsg::Data { data }) | Some(russh::ChannelMsg::ExtendedData { data, .. }) => {
                                let replies = { let mut t=out.lock().unwrap(); t.process(&data); std::mem::take(&mut t.parser.callbacks_mut().replies) };
                                if !replies.is_empty() { channel.data(&replies[..]).await?; }
                                wake();
                            }
                            Some(russh::ChannelMsg::ExitStatus { exit_status }) => { state.lock().unwrap().exit_code=Some(exit_status); wake(); }
                            Some(russh::ChannelMsg::Close) | None => break,
                            _ => {}
                        }
                    }
                }
                Ok(())
            }.await;
            let mut status = state.lock().unwrap();
            status.eof = true;
            if let Err(e) = result { status.error=Some(format!("{e:#}")); }
            if status.exit_code.is_none() { status.exit_code=Some(255); }
            wake();
        });
        Self {
            terminal,
            status,
            kind: SessionKind::Ssh(connection.profile.clone()),
            pid: None,
            size: (30, 100),
            input,
            stop,
            killer: None,
            remote: Some(connection),
        }
    }

    pub fn write(&self, bytes: Vec<u8>) -> Result<()> {
        if bytes.len() > 1024 * 1024 {
            bail!("单次粘贴不能超过 1 MiB");
        }
        if self.status.lock().unwrap().exit_code.is_some() {
            bail!("会话已退出，请重新启动");
        }
        self.input
            .try_send(Control::Write(bytes))
            .context("终端输入队列已满或会话已关闭，请稍后重试")
    }

    pub fn resize(&mut self, rows: u16, cols: u16) -> Result<()> {
        let size = (rows.max(1), cols.max(1));
        if self.size != size {
            self.input.try_send(Control::Resize(PtySize {
                rows: size.0,
                cols: size.1,
                pixel_width: 0,
                pixel_height: 0,
            }))?;
            self.size = size;
        }
        Ok(())
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(killer) = &mut self.killer {
            let _ = killer.kill();
        }
        // The control worker closes ConPTY while the reader keeps draining it;
        // ClosePseudoConsole must never block the UI thread.
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn remote_options_cannot_be_injected_through_destination() {
        for host in [
            "-oProxyCommand=calc",
            "host name",
            "a\ncommand",
            "user@host",
        ] {
            let p = RemoteProfile {
                host: host.into(),
                ..Default::default()
            };
            assert!(SessionKind::Ssh(p).command().is_err());
        }
        let p = RemoteProfile {
            host: "::1".into(),
            user: "test".into(),
            ..Default::default()
        };
        assert!(SessionKind::Ssh(p).command().is_ok());
    }
}
