//! The sessions a pane can start and the shells found on this machine.

use super::*;

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub enum SessionKind {
    Local(String),
    Serial(SerialProfile),
    Ssh(RemoteProfile),
    Sftp(RemoteProfile),
}

impl SessionKind {
    pub fn label(&self) -> String {
        match self {
            Self::Local(shell) => shell_label(shell),
            Self::Serial(p) => p.label(),
            Self::Ssh(p) => p.label(),
            Self::Sftp(p) => format!("SFTP · {}", p.label()),
        }
    }

    pub fn command(&self) -> Result<CommandBuilder> {
        let mut cmd = match self {
            Self::Serial(profile) => bail!("串口会话不通过进程启动：{}", profile.label()),
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
                    // `shell` is an executable path from `local_shells`. The
                    // legacy empty/`shell` value still means "follow $SHELL".
                    let program = match shell.trim() {
                        "" | "shell" => std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".into()),
                        path => path.to_string(),
                    };
                    CommandBuilder::new(program)
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

/// A local shell the UI can offer: the value stored in the session and config,
/// and the name shown for it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LocalShell {
    pub value: String,
    pub label: String,
}

/// The name to show for a shell value. Windows values are short keys
/// (`powershell`, `cmd`, …); Unix values are executable paths, shown by their
/// file name. An empty value or `shell` means "follow `$SHELL`".
pub fn shell_label(value: &str) -> String {
    match value {
        "powershell" => "PowerShell".into(),
        "pwsh" => "PowerShell 7".into(),
        "cmd" => "Command Prompt".into(),
        "wsl" => "WSL".into(),
        "" | "shell" => "Shell".into(),
        path => std::path::Path::new(path)
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .filter(|name| !name.is_empty())
            .unwrap_or_else(|| path.to_string()),
    }
}

/// The local shells offered in the UI, in display order. Probed once: reading
/// `/etc/shells` and stat-ing candidates on every frame would be wasteful.
pub fn local_shells() -> &'static [LocalShell] {
    static SHELLS: std::sync::OnceLock<Vec<LocalShell>> = std::sync::OnceLock::new();
    SHELLS.get_or_init(enumerate_local_shells)
}

/// Windows ships a fixed set of shells; the keys match `SessionKind::command`.
#[cfg(windows)]
pub(super) fn enumerate_local_shells() -> Vec<LocalShell> {
    [
        ("powershell", "PowerShell"),
        ("pwsh", "PowerShell 7"),
        ("cmd", "CMD"),
        ("wsl", "WSL"),
    ]
    .into_iter()
    .map(|(value, label)| LocalShell {
        value: value.into(),
        label: label.into(),
    })
    .collect()
}

/// On Unix the shells are whatever the machine actually has. `/etc/shells` is
/// the system's own list of valid login shells; `$SHELL` and a few common
/// package-manager locations are added in case they are not listed there.
#[cfg(not(windows))]
pub(super) fn enumerate_local_shells() -> Vec<LocalShell> {
    fn push(paths: &mut Vec<String>, path: &str) {
        let path = path.trim();
        if path.is_empty() || paths.iter().any(|existing| existing.as_str() == path) {
            return;
        }
        if std::path::Path::new(path).is_file() {
            paths.push(path.to_string());
        }
    }

    let mut paths = Vec::new();
    if let Ok(shell) = std::env::var("SHELL") {
        push(&mut paths, &shell);
    }
    if let Ok(contents) = std::fs::read_to_string("/etc/shells") {
        for line in contents.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            push(&mut paths, line);
        }
    }
    for candidate in [
        "/bin/sh",
        "/bin/bash",
        "/bin/zsh",
        "/usr/bin/fish",
        "/usr/local/bin/fish",
        "/opt/homebrew/bin/fish",
        "/opt/homebrew/bin/bash",
        "/opt/homebrew/bin/zsh",
        "/usr/local/bin/bash",
        "/usr/local/bin/zsh",
        "/opt/homebrew/bin/pwsh",
        "/usr/local/bin/pwsh",
    ] {
        push(&mut paths, candidate);
    }

    // Two paths can share a name (`/bin/bash` and `/opt/homebrew/bin/bash`).
    // Keep the first, so the list stays unambiguous; the earlier sources
    // (`$SHELL`, `/etc/shells`) win.
    let mut shells: Vec<LocalShell> = Vec::new();
    for path in paths {
        let label = shell_label(&path);
        if shells.iter().any(|shell| shell.label == label) {
            continue;
        }
        shells.push(LocalShell { value: path, label });
    }
    shells
}
