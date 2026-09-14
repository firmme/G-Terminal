use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub font_size: f32,
    pub scrollback: usize,
    pub light_theme: bool,
    pub sidebar: bool,
    pub default_shell: String,
    pub profiles: Vec<RemoteProfile>,
    pub groups: Vec<String>,
    pub copy_on_select: bool,
    pub restore_workspace: bool,
    pub workspace: Vec<crate::layout::SavedTab>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            font_size: 15.0,
            scrollback: 10_000,
            light_theme: false,
            sidebar: true,
            default_shell: if cfg!(windows) { "powershell" } else { "shell" }.into(),
            profiles: Vec::new(),
            groups: Vec::new(),
            copy_on_select: false,
            restore_workspace: true,
            workspace: Vec::new(),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct RemoteProfile {
    pub name: String,
    pub host: String,
    pub user: String,
    pub port: u16,
    pub identity: String,
    pub group: String,
    pub jump: Option<Box<RemoteProfile>>,
    pub forwards: Vec<Forward>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Forward {
    pub bind_port: u16,
    pub target_host: String,
    pub target_port: u16,
}

impl Default for RemoteProfile {
    fn default() -> Self {
        Self {
            name: String::new(),
            host: String::new(),
            user: String::new(),
            port: 22,
            identity: String::new(),
            group: String::new(),
            jump: None,
            forwards: Vec::new(),
        }
    }
}

impl RemoteProfile {
    pub fn validate(&self) -> Result<()> {
        if self.host.is_empty()
            || self.host.starts_with('-')
            || self
                .host
                .chars()
                .any(|c| c.is_whitespace() || c.is_control() || matches!(c, '@' | '/' | '\\'))
        {
            bail!("请输入有效的主机名或 IP 地址（不能包含空格或命令参数）");
        }
        if self.user.starts_with('-')
            || self
                .user
                .chars()
                .any(|c| c.is_whitespace() || c.is_control() || matches!(c, '@' | '/' | '\\' | ':'))
        {
            bail!("用户名不能包含空格、路径或命令参数");
        }
        if self.port == 0 {
            bail!("端口范围是 1–65535");
        }
        if let Some(jump) = &self.jump {
            if jump.jump.is_some() {
                bail!("ProxyJump 当前只支持一级跳板机");
            }
            jump.validate()?;
        }
        let mut ports = std::collections::HashSet::new();
        for forward in &self.forwards {
            if forward.bind_port == 0
                || forward.target_port == 0
                || forward.target_host.is_empty()
                || forward
                    .target_host
                    .chars()
                    .any(|c| c.is_control() || c.is_whitespace())
            {
                bail!("请输入有效的转发地址和端口");
            }
            if !ports.insert(forward.bind_port) {
                bail!("同一个连接不能重复监听本地端口 {}", forward.bind_port);
            }
        }
        Ok(())
    }

    pub fn destination(&self) -> String {
        if self.user.is_empty() {
            self.host.clone()
        } else {
            format!("{}@{}", self.user, self.host)
        }
    }

    pub fn label(&self) -> String {
        if self.name.trim().is_empty() {
            self.host.clone()
        } else {
            self.name.clone()
        }
    }
}

impl Settings {
    pub fn path() -> PathBuf {
        directories::ProjectDirs::from("dev", "gterminal", "G-Terminal")
            .map(|d| d.config_dir().join("settings.json"))
            .unwrap_or_else(|| PathBuf::from("settings.json"))
    }

    pub fn load() -> Result<Self> {
        let path = Self::path();
        if !path.exists() {
            return Ok(Self::default());
        }
        let mut settings: Self = serde_json::from_slice(&std::fs::read(&path)?)
            .with_context(|| format!("无法读取设置 {}", path.display()))?;
        settings.font_size = settings.font_size.clamp(10.0, 28.0);
        settings.scrollback = settings.scrollback.clamp(100, 50_000);
        Ok(settings)
    }

    pub fn save(&self) -> Result<()> {
        let path = Self::path();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        // Stage a complete file before replacing the previous configuration.
        let temp = path.with_extension("json.tmp");
        std::fs::write(&temp, serde_json::to_vec_pretty(self)?)?;
        std::fs::rename(&temp, &path).context("无法保存设置")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn old_config_migrates_and_default_label_is_host() {
        let s: Settings = serde_json::from_str(
            r#"{"profiles":[{"name":"","host":"10.0.0.8","user":"root","port":22,"identity":""}]}"#,
        )
        .unwrap();
        assert_eq!(s.profiles[0].label(), "10.0.0.8");
        assert!(s.profiles[0].group.is_empty());
        assert!(!s.copy_on_select);
        assert!(s.restore_workspace);
    }
}
