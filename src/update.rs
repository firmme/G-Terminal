//! Checks GitHub releases and prepares a verified update for the next restart.

use reqwest::blocking::{Client, Response};
use semver::Version;
use serde::Deserialize;
use std::io::Read;
use std::time::Duration;

#[cfg(any(windows, target_os = "macos"))]
use std::{
    fs::{self, File},
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

const REPO: &str = "firmme/G-Terminal";
const METADATA_TIMEOUT: Duration = Duration::from_secs(20);

#[derive(Debug)]
pub enum Status {
    Idle,
    Checking,
    UpToDate { current: String },
    Available(Release),
    Downloading,
    Ready(PreparedUpdate),
    Failed(String),
}

#[derive(Debug)]
pub struct Release {
    pub version: String,
    pub url: String,
    pub notes: String,
    pub download: Download,
}

#[derive(Debug)]
pub enum Download {
    Available(Package),
    MissingChecksum,
    Unavailable,
}

#[cfg_attr(not(any(windows, target_os = "macos")), allow(dead_code))]
#[derive(Clone, Debug)]
pub struct Package {
    name: String,
    url: String,
    checksum_url: String,
}

pub fn current_version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

pub fn check() -> Result<Status, String> {
    let url = format!("https://api.github.com/repos/{REPO}/releases/latest");
    let response = client()?
        .get(url)
        .header(reqwest::header::ACCEPT, "application/vnd.github+json")
        .timeout(METADATA_TIMEOUT)
        .send()
        .and_then(Response::error_for_status)
        .map_err(|error| format!("检查更新失败：{error}"))?;
    parse_latest(
        &read_metadata(response, 1024 * 1024)?,
        current_version(),
        asset_suffix(),
    )
}

fn parse_latest(json: &str, current: &str, suffix: Option<&str>) -> Result<Status, String> {
    #[derive(Deserialize)]
    struct Asset {
        name: String,
        browser_download_url: String,
    }
    #[derive(Deserialize)]
    struct Latest {
        tag_name: String,
        html_url: String,
        body: Option<String>,
        #[serde(default)]
        assets: Vec<Asset>,
    }

    let latest: Latest =
        serde_json::from_str(json).map_err(|error| format!("解析发布信息失败：{error}"))?;
    let version = latest
        .tag_name
        .strip_prefix('v')
        .unwrap_or(&latest.tag_name);
    if !is_newer(version, current)? {
        return Ok(Status::UpToDate {
            current: current.to_string(),
        });
    }

    let download = suffix
        .and_then(|suffix| {
            let name = format!("G-Terminal-{version}-{suffix}");
            latest.assets.iter().find(|asset| asset.name == name)
        })
        .map_or(Download::Unavailable, |asset| {
            let checksum_name = format!("{}.sha256", asset.name);
            latest
                .assets
                .iter()
                .find(|item| item.name == checksum_name)
                .map_or(Download::MissingChecksum, |checksum| {
                    Download::Available(Package {
                        name: asset.name.clone(),
                        url: asset.browser_download_url.clone(),
                        checksum_url: checksum.browser_download_url.clone(),
                    })
                })
        });

    Ok(Status::Available(Release {
        version: version.to_string(),
        url: latest.html_url,
        notes: latest.body.unwrap_or_default(),
        download,
    }))
}

fn is_newer(candidate: &str, current: &str) -> Result<bool, String> {
    let parse = |version: &str| {
        Version::parse(version).map_err(|error| format!("无效的版本号 {version}：{error}"))
    };
    Ok(parse(candidate)?.cmp_precedence(&parse(current)?).is_gt())
}

fn asset_suffix() -> Option<&'static str> {
    if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
        Some("macos-arm64.tar.gz")
    } else if cfg!(all(target_os = "macos", target_arch = "x86_64")) {
        Some("macos-x86_64.tar.gz")
    } else if cfg!(all(target_os = "windows", target_arch = "x86_64")) {
        Some("windows-x64.zip")
    } else {
        None
    }
}

fn client() -> Result<Client, String> {
    Client::builder()
        .user_agent(concat!("G-Terminal/", env!("CARGO_PKG_VERSION")))
        .https_only(true)
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(300))
        .build()
        .map_err(|error| format!("无法初始化更新请求：{error}"))
}

fn read_metadata(response: impl Read, limit: u64) -> Result<String, String> {
    let mut text = String::new();
    response
        .take(limit + 1)
        .read_to_string(&mut text)
        .map_err(|error| format!("无法读取发布信息：{error}"))?;
    if text.len() as u64 > limit {
        return Err("发布信息超过大小限制".into());
    }
    Ok(text)
}

#[derive(Debug)]
pub struct PreparedUpdate {
    #[cfg(any(windows, target_os = "macos"))]
    dir: tempfile::TempDir,
    #[cfg(any(windows, target_os = "macos"))]
    current: PathBuf,
    #[cfg(any(windows, target_os = "macos"))]
    staged: PathBuf,
}

impl PreparedUpdate {
    #[cfg(any(windows, target_os = "macos"))]
    pub fn prepare(package: &Package) -> Result<Self, String> {
        let current = install_target()?;
        let parent = current.parent().ok_or("无法找到安装目录")?;
        // Keep the staged update and backup on the same filesystem as the installation.
        let dir = tempfile::Builder::new()
            .prefix(".gterminal-update-")
            .tempdir_in(parent)
            .map_err(|error| format!("无法在安装目录准备更新：{error}。请从发布页手动更新。"))?;
        let archive = dir.path().join(&package.name);
        download(package, &archive)?;
        let member = package_member(&package.name)?;
        extract(&archive, dir.path(), &member)?;
        let staged = dir.path().join(member);
        let metadata = fs::symlink_metadata(&staged)
            .map_err(|error| format!("下载包缺少应用文件：{error}"))?;
        #[cfg(windows)]
        let valid = metadata.is_file();
        #[cfg(target_os = "macos")]
        let valid = metadata.is_dir() && staged.join("Contents/MacOS/g-terminal").is_file();
        if !valid {
            return Err("下载包里的应用文件格式不正确".into());
        }
        fs::remove_file(&archive).map_err(|error| format!("无法清理下载文件：{error}"))?;
        Ok(Self {
            dir,
            current,
            staged,
        })
    }

    #[cfg(any(windows, target_os = "macos"))]
    pub fn launch(&mut self) -> Result<(), String> {
        let script = self.dir.path().join(INSTALLER_NAME);
        fs::write(&script, INSTALLER).map_err(|error| format!("无法写入更新程序：{error}"))?;
        installer_command(&script)?
            .env("GTERMINAL_UPDATE_PARENT", std::process::id().to_string())
            .env("GTERMINAL_UPDATE_DIR", self.dir.path())
            .env("GTERMINAL_UPDATE_CURRENT", &self.current)
            .env("GTERMINAL_UPDATE_STAGED", &self.staged)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|error| format!("无法启动更新程序：{error}"))?;
        // The installer owns the staging directory once it has been started.
        self.dir.disable_cleanup(true);
        Ok(())
    }

    #[cfg(not(any(windows, target_os = "macos")))]
    pub fn prepare(_package: &Package) -> Result<Self, String> {
        Err("当前平台请在发布页手动更新".into())
    }

    #[cfg(not(any(windows, target_os = "macos")))]
    pub fn launch(&mut self) -> Result<(), String> {
        Err("当前平台请在发布页手动更新".into())
    }
}

#[cfg(any(windows, target_os = "macos"))]
fn download(package: &Package, path: &Path) -> Result<(), String> {
    let client = client()?;
    let checksum = client
        .get(&package.checksum_url)
        .timeout(METADATA_TIMEOUT)
        .send()
        .and_then(Response::error_for_status)
        .map_err(|error| format!("无法下载校验文件：{error}"))?;
    let expected = parse_checksum(&read_metadata(checksum, 4096)?).ok_or("校验文件格式不正确")?;
    {
        let mut file =
            File::create_new(path).map_err(|error| format!("无法创建下载文件：{error}"))?;
        client
            .get(&package.url)
            .send()
            .and_then(Response::error_for_status)
            .and_then(|mut response| response.copy_to(&mut file))
            .map_err(|error| format!("下载更新失败：{error}"))?;
    }
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    File::open(path)
        .and_then(|mut file| std::io::copy(&mut file, &mut hasher))
        .map_err(|error| format!("无法读取下载文件：{error}"))?;
    if format!("{:x}", hasher.finalize()) != expected {
        return Err("下载文件 SHA-256 校验失败".into());
    }
    Ok(())
}

#[cfg(any(windows, target_os = "macos", test))]
fn parse_checksum(text: &str) -> Option<String> {
    let digest = text.split_whitespace().next()?.to_ascii_lowercase();
    (digest.len() == 64 && digest.bytes().all(|b| b.is_ascii_hexdigit())).then_some(digest)
}

#[cfg(any(windows, target_os = "macos"))]
fn package_member(name: &str) -> Result<String, String> {
    #[cfg(windows)]
    let (extension, member) = (".zip", "g-terminal.exe");
    #[cfg(target_os = "macos")]
    let (extension, member) = (".tar.gz", "G-Terminal.app");
    let root = name.strip_suffix(extension).ok_or("更新包扩展名不正确")?;
    Ok(format!("{root}/{member}"))
}

#[cfg(any(windows, target_os = "macos"))]
fn extract(archive: &Path, dir: &Path, member: &str) -> Result<(), String> {
    #[cfg(windows)]
    let mut tar = hidden_windows_command(windows_system_dir()?.join("tar.exe"));
    #[cfg(target_os = "macos")]
    let mut tar = Command::new("/usr/bin/tar");
    let output = tar
        .arg("-xf")
        .arg(archive)
        .arg("-C")
        .arg(dir)
        .arg(member)
        .output()
        .map_err(|error| format!("无法解压更新包：{error}"))?;
    if !output.status.success() {
        let error = String::from_utf8_lossy(&output.stderr);
        return Err(format!("解压更新包失败：{}", error.trim()));
    }
    Ok(())
}

#[cfg(windows)]
fn install_target() -> Result<PathBuf, String> {
    std::env::current_exe().map_err(|error| format!("无法找到当前程序：{error}"))
}

#[cfg(target_os = "macos")]
fn install_target() -> Result<PathBuf, String> {
    let executable =
        std::env::current_exe().map_err(|error| format!("无法找到当前程序：{error}"))?;
    let bundle = executable.ancestors().nth(3).filter(|bundle| {
        bundle
            .extension()
            .is_some_and(|extension| extension == "app")
            && executable
                .strip_prefix(bundle)
                .is_ok_and(|path| path == Path::new("Contents/MacOS/g-terminal"))
    });
    bundle
        .map(Path::to_path_buf)
        .ok_or_else(|| "当前程序从开发构建运行，请在发布页手动更新".into())
}

#[cfg(windows)]
const INSTALLER_NAME: &str = "apply.ps1";
#[cfg(windows)]
const INSTALLER: &str = include_str!("update/apply.ps1");
#[cfg(target_os = "macos")]
const INSTALLER_NAME: &str = "apply.sh";
#[cfg(target_os = "macos")]
const INSTALLER: &str = include_str!("update/apply.sh");

#[cfg(windows)]
fn windows_system_dir() -> Result<PathBuf, String> {
    std::env::var_os("SystemRoot")
        .map(|root| PathBuf::from(root).join("System32"))
        .ok_or_else(|| "无法找到 Windows 系统目录".into())
}

#[cfg(windows)]
fn hidden_windows_command(program: PathBuf) -> Command {
    use std::os::windows::process::CommandExt;
    use windows::Win32::System::Threading::CREATE_NO_WINDOW;
    let mut command = Command::new(program);
    command.creation_flags(CREATE_NO_WINDOW.0);
    command
}

#[cfg(windows)]
fn installer_command(script: &Path) -> Result<Command, String> {
    let mut command =
        hidden_windows_command(windows_system_dir()?.join("WindowsPowerShell/v1.0/powershell.exe"));
    command
        .args([
            "-NoLogo",
            "-NoProfile",
            "-NonInteractive",
            "-ExecutionPolicy",
            "Bypass",
            "-File",
        ])
        .arg(script);
    Ok(command)
}

#[cfg(target_os = "macos")]
fn installer_command(script: &Path) -> Result<Command, String> {
    let mut command = Command::new("/bin/sh");
    command.arg(script);
    Ok(command)
}

#[cfg(test)]
mod tests {
    use super::*;

    const WINDOWS: &str = "windows-x64.zip";
    const MAC_ARM: &str = "macos-arm64.tar.gz";
    const RELEASE: &str = r#"{
        "tag_name": "v9.9.9",
        "html_url": "https://example.com/release",
        "body": "notes",
        "assets": [
            {"name": "G-Terminal-9.9.9-windows-x64.zip", "browser_download_url": "https://example.com/win.zip"},
            {"name": "G-Terminal-9.9.9-windows-x64.zip.sha256", "browser_download_url": "https://example.com/win.zip.sha256"},
            {"name": "G-Terminal-9.9.9-macos-arm64.tar.gz", "browser_download_url": "https://example.com/mac.tar.gz"},
            {"name": "G-Terminal-9.9.9-macos-x86_64.tar.gz.sha256", "browser_download_url": "https://example.com/intel.tar.gz.sha256"}
        ]
    }"#;

    #[test]
    fn versions_follow_semver_precedence() {
        assert!(is_newer("0.4.10", "0.4.9").unwrap());
        assert!(is_newer("0.5.0", "0.5.0-rc.1").unwrap());
        assert!(!is_newer("0.5.0-rc.1", "0.5.0").unwrap());
        assert!(!is_newer("0.4.1+new", "0.4.1").unwrap());
        for malformed in ["nightly", "1.bad.0", "1.0.0.1", "1.0"] {
            assert!(is_newer(malformed, "0.4.1").is_err());
        }
    }

    #[test]
    fn a_newer_release_picks_the_platform_asset_and_its_checksum() {
        let Status::Available(release) = parse_latest(RELEASE, "0.4.1", Some(WINDOWS)).unwrap()
        else {
            panic!("expected a release")
        };
        assert_eq!(release.version, "9.9.9");
        assert_eq!(release.url, "https://example.com/release");
        let Download::Available(package) = release.download else {
            panic!("expected a package")
        };
        assert_eq!(package.name, "G-Terminal-9.9.9-windows-x64.zip");
        assert_eq!(package.url, "https://example.com/win.zip");
        assert_eq!(package.checksum_url, "https://example.com/win.zip.sha256");
    }

    #[test]
    fn an_uninstallable_release_still_links_to_the_release_page() {
        for (suffix, missing_checksum) in [
            (Some(MAC_ARM), true),
            (Some("macos-x86_64.tar.gz"), false),
            (None, false),
        ] {
            let Status::Available(release) = parse_latest(RELEASE, "0.4.1", suffix).unwrap() else {
                panic!("expected a release")
            };
            assert!(match release.download {
                Download::MissingChecksum => missing_checksum,
                Download::Unavailable => !missing_checksum,
                Download::Available(_) => false,
            });
            assert_eq!(release.url, "https://example.com/release");
        }
    }

    #[test]
    fn an_older_tag_is_not_an_update() {
        let Status::UpToDate { current } = parse_latest(RELEASE, "10.0.0", Some(WINDOWS)).unwrap()
        else {
            panic!("expected the current version")
        };
        assert_eq!(current, "10.0.0");
    }

    #[test]
    fn checksum_requires_a_full_hex_digest() {
        let digest = "AB".repeat(32);
        assert_eq!(
            parse_checksum(&format!("{digest}  G-Terminal.zip\n")),
            Some(digest.to_ascii_lowercase())
        );
        assert_eq!(parse_checksum("nothing useful"), None);
    }
}
