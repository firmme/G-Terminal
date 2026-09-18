//! Saved connections: SSH and serial.

use super::*;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct RemoteProfile {
    pub name: String,
    pub host: String,
    pub user: String,
    pub port: u16,
    pub identity: String,
    pub group: String,
    /// Tab tag colour, `#rrggbb`; empty means none. Beats the group's colour.
    pub color: String,
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
            color: String::new(),
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

/// Line speeds offered as presets. 115200 is the default, and is what a
/// console cable almost always expects.
pub const BAUD_RATES: [u32; 8] = [9600, 19200, 38400, 57600, 115200, 230400, 460800, 921600];

pub const DEFAULT_BAUD: u32 = 115_200;

/// A serial port the terminal can open directly, with no process on the other
/// end of it. `port` is the device name (`COM3`, `/dev/ttyUSB0`) or `auto`,
/// which picks a connected device at connect time.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct SerialProfile {
    pub name: String,
    pub port: String,
    pub baud: u32,
    pub group: String,
    /// Tab tag colour, `#rrggbb`; empty means none. Beats the group's colour.
    pub color: String,
}

impl Default for SerialProfile {
    fn default() -> Self {
        Self {
            name: String::new(),
            port: "auto".into(),
            baud: DEFAULT_BAUD,
            group: String::new(),
            color: String::new(),
        }
    }
}

impl SerialProfile {
    pub fn validate(&self) -> Result<()> {
        let port = self.port.trim();
        if port.is_empty() {
            bail!(
                "请输入串口设备名，例如 {} 或 auto",
                crate::serial::PORT_EXAMPLE
            );
        }
        if port
            .chars()
            .any(|c| c.is_whitespace() || c.is_control() || c == '"')
        {
            bail!("串口设备名不能包含空格或引号");
        }
        if self.baud == 0 || self.baud > 4_000_000 {
            bail!("波特率范围是 1–4000000");
        }
        Ok(())
    }

    /// Whether the port is chosen when connecting rather than pinned here.
    pub fn auto(&self) -> bool {
        self.port.trim().eq_ignore_ascii_case("auto")
    }

    pub fn label(&self) -> String {
        if self.name.trim().is_empty() {
            self.port.trim().to_string()
        } else {
            self.name.clone()
        }
    }
}
