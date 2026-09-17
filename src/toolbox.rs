//! The rarely used server toolbox: a handful of one-off initialisation steps
//! that are sent to the focused SSH session as a shell script.
//!
//! Nothing runs without being ticked, the exact payload is shown before it is
//! sent, and it is written to a temporary file on the remote and executed with
//! `sh` — so the quoting in a prompt or a public key cannot leak into the
//! wrapper that carries it.

use crate::{remote_ui::hint, theme::Palette};
use eframe::egui::{self, RichText};
use g_terminal::config::RemoteProfile;
use std::path::{Path, PathBuf};

/// The two prompt presets, kept verbatim so what runs is what was asked for.
/// The outer heredoc is quoted, so none of this is expanded on the way out.
const FULL_PROMPT: &str = r##"${debian_chroot:+($debian_chroot)}\[\033[01;33m\][\t]\[\033[00m\]\[\033[01;32m\]\u@\h\[\033[00m\]:\[\033[01;33m\]\#\[\033[00m\] \[\033[01;34m\]\w\[\033[00m\]\$ "##;
const MINIMAL_PROMPT: &str = r##"\[\e[32m\]\u@\h:\[\e[34m\]\w\[\e[0m\]\$ "##;

const DEFAULT_TIMEZONE: &str = "Asia/Shanghai";

/// The file the payload is uploaded to before it runs.
const REMOTE_SCRIPT: &str = "/tmp/g-terminal-toolbox.sh";

#[derive(Clone, Copy, PartialEq, Eq)]
enum Prompt {
    Keep,
    Full,
    Minimal,
}

impl Prompt {
    fn text(self) -> Option<&'static str> {
        match self {
            Self::Keep => None,
            Self::Full => Some(FULL_PROMPT),
            Self::Minimal => Some(MINIMAL_PROMPT),
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Tool {
    Htop,
    Vim,
    Lrzsz,
    Git,
    Node,
    Nginx,
    Hermes,
    Tailscale,
    Frp,
}

impl Tool {
    const ALL: [Tool; 9] = [
        Tool::Htop,
        Tool::Vim,
        Tool::Lrzsz,
        Tool::Git,
        Tool::Node,
        Tool::Nginx,
        Tool::Hermes,
        Tool::Tailscale,
        Tool::Frp,
    ];

    fn label(self) -> &'static str {
        match self {
            Self::Htop => "htop",
            Self::Vim => "vim",
            Self::Lrzsz => "lrzsz",
            Self::Git => "git",
            Self::Node => "node",
            Self::Nginx => "nginx",
            Self::Hermes => "Hermes",
            Self::Tailscale => "tailscale",
            Self::Frp => "frp",
        }
    }

    /// Distribution packages, or empty when the tool needs its own installer.
    fn packages(self) -> &'static str {
        match self {
            Self::Htop => "htop",
            Self::Vim => "vim",
            Self::Lrzsz => "lrzsz",
            Self::Git => "git",
            Self::Node => "nodejs npm",
            Self::Nginx => "nginx",
            Self::Hermes | Self::Tailscale | Self::Frp => "",
        }
    }
}

pub struct Toolbox {
    aliases: bool,
    key_login: bool,
    /// The local public key to install, when one was found.
    key: Option<String>,
    prompt: Prompt,
    timezone_on: bool,
    timezone: String,
    ipv6: bool,
    tools: Vec<(Tool, bool)>,
}

impl Toolbox {
    pub fn new(profile: &RemoteProfile) -> Self {
        Self {
            aliases: false,
            key_login: false,
            key: find_public_key(&profile.identity),
            prompt: Prompt::Keep,
            timezone_on: false,
            timezone: DEFAULT_TIMEZONE.into(),
            ipv6: false,
            tools: Tool::ALL.iter().map(|tool| (*tool, false)).collect(),
            // Nothing ticked yet, so nothing to preview.
        }
    }

    fn selected_tools(&self) -> Vec<Tool> {
        self.tools
            .iter()
            .filter(|(_, checked)| *checked)
            .map(|(tool, _)| *tool)
            .collect()
    }

    /// Whether the timezone field holds something worth sending.
    fn timezone_ok(&self) -> bool {
        valid_timezone(&self.timezone)
    }

    fn anything_selected(&self) -> bool {
        self.aliases
            || (self.key_login && self.key.is_some())
            || self.prompt != Prompt::Keep
            || (self.timezone_on && self.timezone_ok())
            || self.ipv6
            || !self.selected_tools().is_empty()
    }

    /// The whole payload: a script uploaded to the remote and then run. `sh`
    /// reads it from a file, so the commands are expanded remotely and the
    /// literal pieces (prompt, aliases) stay literal.
    pub fn script(&self) -> String {
        let mut body = String::from(
            "echo '=== G-Terminal 服务器初始化 ==='\n\
             [ \"$(id -u)\" = 0 ] || echo '提示：当前不是 root，部分操作可能失败（可先 sudo -i 后重试）'\n",
        );
        if self.aliases {
            body.push_str(ALIASES);
        }
        if self.key_login
            && let Some(key) = &self.key
        {
            body.push_str(&key_section(key));
        }
        if let Some(prompt) = self.prompt.text() {
            body.push_str(&prompt_section(prompt));
        }
        if self.timezone_on && self.timezone_ok() {
            body.push_str(&timezone_section(self.timezone.trim()));
        }
        if self.ipv6 {
            body.push_str(IPV6);
        }
        let tools = self.selected_tools();
        if !tools.is_empty() {
            body.push_str(&tools_section(&tools));
        }
        format!(
            "cat > {REMOTE_SCRIPT} <<'GT_TOOLBOX'\n{body}GT_TOOLBOX\n\
             sh {REMOTE_SCRIPT}\nrm -f {REMOTE_SCRIPT}\n"
        )
    }

    /// Draws the window, returning the script to send when 执行 is pressed.
    pub fn show(
        &mut self,
        ctx: &egui::Context,
        open: &mut bool,
        p: Palette,
        target: &str,
    ) -> Option<String> {
        let mut run = false;
        let mut cancel = false;
        let mut copy = false;
        egui::Window::new("服务器工具箱")
            .open(open)
            .collapsible(false)
            .default_width(540.0)
            .show(ctx, |ui| {
                ui.label(RichText::new(format!("目标：{target}")).color(p.accent));
                ui.label(hint(
                    "以下命令以当前登录用户的身份在该会话中执行；未勾选的项不会运行。",
                    p,
                ));
                ui.separator();
                ui.label(RichText::new("初始化服务器").strong());
                ui.checkbox(&mut self.aliases, "配置常用命令别名");
                ui.horizontal(|ui| {
                    let has_key = self.key.is_some();
                    ui.add_enabled_ui(has_key, |ui| {
                        ui.checkbox(&mut self.key_login, "免密登录（写入本机公钥）");
                    });
                    if !has_key {
                        ui.label(hint("未找到本机公钥（~/.ssh/id_ed25519.pub 等）", p));
                    }
                });
                ui.horizontal(|ui| {
                    ui.label("命令提示符");
                    ui.selectable_value(&mut self.prompt, Prompt::Keep, "不修改");
                    ui.selectable_value(&mut self.prompt, Prompt::Full, "完整");
                    ui.selectable_value(&mut self.prompt, Prompt::Minimal, "精简");
                });
                ui.horizontal(|ui| {
                    ui.checkbox(&mut self.timezone_on, "设置时区");
                    ui.add_enabled(
                        self.timezone_on,
                        egui::TextEdit::singleline(&mut self.timezone).desired_width(160.0),
                    );
                });
                ui.checkbox(&mut self.ipv6, "关闭 IPv6");
                ui.separator();
                ui.label(RichText::new("安装常用工具").strong());
                ui.horizontal_wrapped(|ui| {
                    for (tool, checked) in &mut self.tools {
                        ui.checkbox(checked, tool.label());
                    }
                });
                if self.timezone_on && !self.timezone_ok() {
                    ui.colored_label(p.danger, "时区格式不合法，示例：Asia/Shanghai");
                }
                ui.separator();
                egui::CollapsingHeader::new("查看将要执行的命令")
                    .default_open(false)
                    .show(ui, |ui| {
                        egui::ScrollArea::both().max_height(220.0).show(ui, |ui| {
                            ui.monospace(self.script());
                        });
                    });
                ui.label(hint(
                    "命令会写入远端临时文件后执行，可直接复制到别处复核。",
                    p,
                ));
                ui.horizontal(|ui| {
                    let ready =
                        self.anything_selected() && !(self.timezone_on && !self.timezone_ok());
                    ui.add_enabled_ui(ready, |ui| {
                        if ui.button("执行").clicked() {
                            run = true;
                        }
                    });
                    if ui.button("复制命令").clicked() {
                        copy = true;
                    }
                    if ui.button("取消").clicked() {
                        cancel = true;
                    }
                });
            });
        if copy {
            ctx.copy_text(self.script());
        }
        if cancel {
            *open = false;
        }
        if run { Some(self.script()) } else { None }
    }
}

/// A timezone path cannot smuggle anything into the script.
fn valid_timezone(value: &str) -> bool {
    let value = value.trim();
    !value.is_empty()
        && value.len() <= 64
        && value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '/' | '_' | '-' | '+'))
}

/// The public half of the key this profile would log in with, so it can be
/// installed on the server. An explicit identity wins, then the usual names.
fn find_public_key(identity: &str) -> Option<String> {
    let mut candidates = Vec::new();
    let identity = identity.trim();
    if !identity.is_empty() {
        if identity.ends_with(".pub") {
            candidates.push(expand_home(identity));
        } else {
            candidates.push(expand_home(&format!("{identity}.pub")));
        }
    }
    if let Some(home) = directories::UserDirs::new().map(|d| d.home_dir().to_path_buf()) {
        for name in ["id_ed25519.pub", "id_ecdsa.pub", "id_rsa.pub"] {
            candidates.push(home.join(".ssh").join(name));
        }
    }
    for path in candidates {
        if let Some(key) = read_public_key(&path) {
            return Some(key);
        }
    }
    None
}

fn expand_home(path: &str) -> PathBuf {
    let rest = path
        .strip_prefix("~/")
        .or_else(|| path.strip_prefix("~\\"))
        .map(PathBuf::from);
    match (rest, directories::UserDirs::new()) {
        (Some(rest), Some(dirs)) => dirs.home_dir().join(rest),
        _ => PathBuf::from(path),
    }
}

/// One line, and safe to drop inside single quotes in the remote script.
fn read_public_key(path: &Path) -> Option<String> {
    let text = std::fs::read_to_string(path).ok()?;
    let key = text.lines().next()?.trim().to_string();
    let looks_like_key = key.starts_with("ssh-") || key.starts_with("ecdsa-");
    (looks_like_key && !key.contains('\'')).then_some(key)
}

// ---- script sections -------------------------------------------------------

const ALIASES: &str = "\
cat >> \"$HOME/.bashrc\" <<'GT_ALIAS'

# G-Terminal 工具箱：常用别名
alias ll='ls -alF'
alias la='ls -A'
alias l='ls -CF'
alias ..='cd ..'
alias ...='cd ../..'
alias g='grep --color=auto'
alias ports='ss -tulnp 2>/dev/null || netstat -tulnp'
alias dfh='df -h'
alias duh='du -sh'
alias freeh='free -h'
GT_ALIAS
echo '已写入常用别名到 ~/.bashrc'
";

fn key_section(key: &str) -> String {
    format!(
        "mkdir -p \"$HOME/.ssh\" && chmod 700 \"$HOME/.ssh\"\n\
         touch \"$HOME/.ssh/authorized_keys\"\n\
         grep -qxF '{key}' \"$HOME/.ssh/authorized_keys\" 2>/dev/null || echo '{key}' >> \"$HOME/.ssh/authorized_keys\"\n\
         chmod 600 \"$HOME/.ssh/authorized_keys\"\n\
         echo '已写入公钥到 ~/.ssh/authorized_keys'\n"
    )
}

fn prompt_section(prompt: &str) -> String {
    format!(
        "sed -i '/# G-Terminal 工具箱：命令提示符$/,+1d' \"$HOME/.bashrc\" 2>/dev/null\n\
         cat >> \"$HOME/.bashrc\" <<'GT_PROMPT'\n\n\
         # G-Terminal 工具箱：命令提示符\n\
         PS1='{prompt}'\n\
         GT_PROMPT\n\
         echo '已更新命令提示符（重新登录或 source ~/.bashrc 后生效）'\n"
    )
}

fn timezone_section(timezone: &str) -> String {
    format!(
        "if command -v timedatectl >/dev/null 2>&1; then\n\
         \x20 timedatectl set-timezone {timezone} && echo '时区已设为 {timezone}'\n\
         else\n\
         \x20 ln -sf \"/usr/share/zoneinfo/{timezone}\" /etc/localtime && echo '{timezone}' > /etc/timezone && echo '时区已设为 {timezone}'\n\
         fi\n"
    )
}

const IPV6: &str = "\
if ! grep -q 'G-Terminal 工具箱：关闭 IPv6' /etc/sysctl.conf 2>/dev/null; then
cat >> /etc/sysctl.conf <<'GT_SYSCTL'

# G-Terminal 工具箱：关闭 IPv6
net.ipv6.conf.all.disable_ipv6 = 1
net.ipv6.conf.default.disable_ipv6 = 1
net.ipv6.conf.lo.disable_ipv6 = 1
GT_SYSCTL
fi
sysctl -p >/dev/null 2>&1 && echo 'IPv6 已关闭'
";

fn tools_section(tools: &[Tool]) -> String {
    let mut out = String::from(
        "\necho '开始安装常用工具…'\n\
         if command -v apt-get >/dev/null 2>&1; then\n\
         \x20 apt-get update -qq >/dev/null 2>&1\n\
         \x20 PM='apt-get install -y'\n\
         elif command -v dnf >/dev/null 2>&1; then PM='dnf install -y'\n\
         elif command -v yum >/dev/null 2>&1; then PM='yum install -y'\n\
         elif command -v apk >/dev/null 2>&1; then PM='apk add'\n\
         elif command -v pacman >/dev/null 2>&1; then PM='pacman -S --noconfirm'\n\
         else PM=''; fi\n\
         install_pkgs() {\n\
         \x20 [ -n \"$PM\" ] || { echo \"未识别包管理器，跳过: $*\"; return 0; }\n\
         \x20 $PM \"$@\"\n\
         }\n",
    );
    for tool in tools {
        match tool {
            Tool::Tailscale => out.push_str(
                "if command -v tailscale >/dev/null 2>&1; then echo 'tailscale 已安装'; \
                 else curl -fsSL https://tailscale.com/install.sh | sh; fi\n",
            ),
            Tool::Frp => out.push_str(
                "ARCH=amd64; case \"$(uname -m)\" in aarch64|arm64) ARCH=arm64;; armv7l) ARCH=arm;; esac\n\
                 VER=$(curl -fsSL https://api.github.com/repos/fatedier/frp/releases/latest 2>/dev/null \
                 | sed -n 's/.*\"tag_name\": *\"\\([^\"]*\\)\".*/\\1/p' | head -n 1)\n\
                 if [ -n \"$VER\" ]; then\n\
                 \x20 curl -fL \"https://github.com/fatedier/frp/releases/download/$VER/frp_${VER#v}_linux_$ARCH.tar.gz\" -o /tmp/frp.tgz \\\n\
                 \x20   && tar -xzf /tmp/frp.tgz -C /tmp \\\n\
                 \x20   && install -m 755 /tmp/frp_*/frpc /usr/local/bin/frpc \\\n\
                 \x20   && install -m 755 /tmp/frp_*/frp /usr/local/bin/frp \\\n\
                 \x20   && echo 'frp 已安装到 /usr/local/bin' && rm -rf /tmp/frp.tgz /tmp/frp_*\n\
                 else echo 'frp：无法获取最新版本号'; fi\n",
            ),
            Tool::Hermes => out.push_str(
                "if command -v npm >/dev/null 2>&1; then npm install -g hermes-engine; \
                 else echo 'Hermes：需要先安装 node 与 npm'; fi\n",
            ),
            other => out.push_str(&format!(
                "install_pkgs {}\n",
                other.packages()
            )),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn toolbox() -> Toolbox {
        Toolbox {
            aliases: false,
            key_login: false,
            key: Some("ssh-ed25519 AAAA test@host".into()),
            prompt: Prompt::Keep,
            timezone_on: false,
            timezone: DEFAULT_TIMEZONE.into(),
            ipv6: false,
            tools: Tool::ALL.iter().map(|tool| (*tool, false)).collect(),
        }
    }

    #[test]
    fn nothing_ticked_produces_no_commands() {
        let toolbox = toolbox();
        assert!(!toolbox.anything_selected());
        let script = toolbox.script();
        assert!(script.contains("cat > /tmp/g-terminal-toolbox.sh"));
        assert!(!script.contains("alias ll="));
        assert!(!script.contains("PS1="));
        assert!(!script.contains("install_pkgs"));
    }

    #[test]
    fn every_section_lands_in_the_payload() {
        let mut toolbox = toolbox();
        toolbox.aliases = true;
        toolbox.key_login = true;
        toolbox.prompt = Prompt::Minimal;
        toolbox.timezone_on = true;
        toolbox.ipv6 = true;
        toolbox.tools.iter_mut().for_each(|(_, on)| *on = true);
        let script = toolbox.script();
        assert!(script.contains("alias ll='ls -alF'"));
        assert!(script.contains("ssh-ed25519 AAAA test@host"));
        assert!(script.contains("PS1='\\[\\e[32m\\]\\u@\\h:"));
        assert!(script.contains("timedatectl set-timezone Asia/Shanghai"));
        assert!(script.contains("disable_ipv6"));
        for package in ["htop", "vim", "lrzsz", "git", "nodejs npm", "nginx"] {
            assert!(
                script.contains(&format!("install_pkgs {package}")),
                "missing {package}"
            );
        }
        assert!(script.contains("tailscale.com/install.sh"));
        assert!(script.contains("fatedier/frp"));
        // The prompt's own characters are not expanded by the wrapper.
        assert!(script.contains("GT_TOOLBOX\n") && script.contains("GT_PROMPT"));
    }

    #[test]
    fn an_unusable_timezone_is_refused() {
        let mut toolbox = toolbox();
        toolbox.timezone_on = true;
        toolbox.timezone = "Asia/Shanghai; rm -rf /".into();
        assert!(!toolbox.timezone_ok());
        assert!(!toolbox.script().contains("timedatectl"));
    }

    #[test]
    fn the_full_prompt_is_carried_verbatim() {
        let mut toolbox = toolbox();
        toolbox.prompt = Prompt::Full;
        let script = toolbox.script();
        assert!(script.contains(FULL_PROMPT), "{script}");
    }

    /// The window has to lay out without panicking, and nothing is returned
    /// until 执行 is pressed.
    #[test]
    fn the_window_renders_and_runs_nothing_on_its_own() {
        let ctx = egui::Context::default();
        let mut toolbox = toolbox();
        toolbox.aliases = true;
        let mut open = true;
        let output = ctx.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(900.0, 700.0),
                )),
                ..Default::default()
            },
            |ctx| {
                assert!(
                    toolbox
                        .show(ctx, &mut open, Palette::new(false), "root@host:22")
                        .is_none()
                );
            },
        );
        assert!(!output.shapes.is_empty(), "the window drew nothing");
        assert!(open);
    }

    #[test]
    fn a_key_with_a_quote_is_not_used() {
        let dir = std::env::temp_dir().join("g-term-toolbox-test");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("bad.pub");
        std::fs::write(&path, "ssh-ed25519 AAAA it's-broken\n").unwrap();
        assert_eq!(read_public_key(&path), None);
        let good = dir.join("good.pub");
        std::fs::write(&good, "ssh-ed25519 AAAAC3Nza user@host\n").unwrap();
        assert_eq!(
            read_public_key(&good).as_deref(),
            Some("ssh-ed25519 AAAAC3Nza user@host")
        );
    }
}
