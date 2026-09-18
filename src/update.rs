//! Manual update check against the project's GitHub releases, plus the
//! self-replace that follows it.
//!
//! `curl` does the HTTP work rather than a new dependency: it ships with macOS,
//! Windows 10+ and every Linux this runs on, and the release assets are already
//! plain archives.

use serde::Deserialize;
use std::path::{Path, PathBuf};
use std::process::Command;

const REPO: &str = "firmme/G-Terminal";

/// What the 检查更新 window is showing.
#[derive(Clone, Debug, PartialEq)]
pub enum Status {
    Checking,
    UpToDate { current: String },
    Available(Release),
    Downloading,
    Ready,
    Failed(String),
}

/// A newer release, with the download that fits this machine when there is one.
#[derive(Clone, Debug, PartialEq)]
pub struct Release {
    pub version: String,
    pub url: String,
    pub notes: String,
    pub asset: Option<String>,
    pub asset_name: Option<String>,
}

pub fn current_version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

/// Looks up the latest published release and compares it with this build.
pub fn check() -> Result<Status, String> {
    let body = curl(&format!(
        "https://api.github.com/repos/{REPO}/releases/latest"
    ))?;
    parse_latest(&body, current_version())
}

/// Splits the release JSON into a status. Kept apart from the request so the
/// comparison and asset choice can be tested without a network.
fn parse_latest(json: &str, current: &str) -> Result<Status, String> {
    #[derive(Deserialize)]
    struct Asset {
        name: String,
        browser_download_url: String,
    }
    #[derive(Deserialize)]
    struct Latest {
        tag_name: String,
        html_url: String,
        #[serde(default)]
        body: Option<String>,
        #[serde(default)]
        assets: Vec<Asset>,
    }

    let latest: Latest =
        serde_json::from_str(json).map_err(|e| format!("解析发布信息失败：{e}"))?;
    let version = latest.tag_name.trim_start_matches('v').to_string();
    if !is_newer(&version, current) {
        return Ok(Status::UpToDate {
            current: current.to_string(),
        });
    }

    let asset = asset_suffix().and_then(|suffix| {
        let expected = format!("G-Terminal-{version}-{suffix}");
        latest
            .assets
            .into_iter()
            .find(|asset| asset.name == expected)
    });
    Ok(Status::Available(Release {
        version,
        url: latest.html_url,
        notes: latest.body.unwrap_or_default(),
        asset: asset
            .as_ref()
            .map(|asset| asset.browser_download_url.clone()),
        asset_name: asset.map(|asset| asset.name),
    }))
}

/// Whether `candidate` is a later `major.minor.patch` than `current`. A tag
/// that does not parse is treated as not newer, so a stray tag cannot offer a
/// downgrade.
fn is_newer(candidate: &str, current: &str) -> bool {
    match (version_tuple(candidate), version_tuple(current)) {
        (Some(candidate), Some(current)) => candidate > current,
        _ => false,
    }
}

fn version_tuple(version: &str) -> Option<(u64, u64, u64)> {
    let mut parts = version
        .trim_start_matches('v')
        .split('.')
        .map(|part| part.parse::<u64>().ok());
    let major = parts.next().flatten()?;
    let minor = parts.next().flatten().unwrap_or(0);
    let patch = parts.next().flatten().unwrap_or(0);
    Some((major, minor, patch))
}

/// The asset name suffix this platform publishes, matching
/// `scripts/package-macos.sh` and `scripts/package.ps1`.
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

/// Downloads an asset into a per-run directory under the temp dir and returns
/// the file path.
pub fn download(url: &str, name: &str) -> Result<PathBuf, String> {
    let dir = update_dir()?;
    let path = dir.join(name);
    let status = Command::new("curl")
        .args(["-fsSL", "-o"])
        .arg(&path)
        .arg(url)
        .status()
        .map_err(|e| format!("无法运行 curl：{e}"))?;
    if !status.success() {
        return Err(format!("下载失败（curl {status}）"));
    }
    Ok(path)
}

fn update_dir() -> Result<PathBuf, String> {
    let dir = std::env::temp_dir().join(format!("gterminal-update-{}", std::process::id()));
    std::fs::create_dir_all(&dir).map_err(|e| format!("无法创建临时目录：{e}"))?;
    Ok(dir)
}

/// Extracts the archive, then hands the swap to a detached script so it can run
/// after this process exits — the bundle cannot be replaced from inside itself.
pub fn install(archive: &Path) -> Result<(), String> {
    let dir = archive
        .parent()
        .ok_or_else(|| "下载文件没有所在目录".to_string())?;
    extract(archive, dir)?;
    swap(dir)
}

#[cfg(target_os = "macos")]
fn extract(archive: &Path, dir: &Path) -> Result<(), String> {
    let status = Command::new("tar")
        .arg("-xzf")
        .arg(archive)
        .arg("-C")
        .arg(dir)
        .status()
        .map_err(|e| format!("无法解压：{e}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("解压失败（tar {status}）"))
    }
}

#[cfg(not(target_os = "macos"))]
fn extract(archive: &Path, dir: &Path) -> Result<(), String> {
    let status = Command::new("tar")
        .arg("-xf")
        .arg(archive)
        .arg("-C")
        .arg(dir)
        .status()
        .map_err(|e| format!("无法解压：{e}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("解压失败（tar {status}）"))
    }
}

#[cfg(target_os = "macos")]
fn swap(dir: &Path) -> Result<(), String> {
    let new_app =
        find_with_extension(dir, "app").ok_or_else(|| "下载包里没有找到 .app".to_string())?;
    let bundle = bundle_root().ok_or_else(|| {
        "当前不是从 .app 运行的（开发构建），请用「打开发布页」手动更新".to_string()
    })?;
    let backup = dir.join("replaced.app");
    let script = dir.join("apply.sh");
    let quoted = |path: &Path| format!("'{}'", path.display());
    let body = format!(
        "#!/bin/sh\n\
         sleep 1\n\
         rm -rf {backup}\n\
         mv {bundle} {backup} 2>/dev/null\n\
         if mv {new_app} {bundle}; then\n\
         \topen {bundle}\n\
         \trm -rf {backup}\n\
         else\n\
         \tmv {backup} {bundle} 2>/dev/null\n\
         fi\n",
        backup = quoted(&backup),
        bundle = quoted(&bundle),
        new_app = quoted(&new_app),
    );
    std::fs::write(&script, body).map_err(|e| format!("无法写入更新脚本：{e}"))?;
    let mut permissions = std::fs::metadata(&script)
        .map_err(|e| e.to_string())?
        .permissions();
    #[allow(clippy::permissions_set_readonly_false)]
    {
        use std::os::unix::fs::PermissionsExt;
        permissions.set_mode(0o755);
    }
    std::fs::set_permissions(&script, permissions).map_err(|e| e.to_string())?;
    Command::new("sh")
        .arg(&script)
        .spawn()
        .map_err(|e| format!("无法启动更新脚本：{e}"))?;
    Ok(())
}

#[cfg(target_os = "windows")]
fn swap(dir: &Path) -> Result<(), String> {
    let new_exe = find_named(dir, "g-terminal.exe")
        .ok_or_else(|| "下载包里没有找到 g-terminal.exe".to_string())?;
    let current = std::env::current_exe().map_err(|e| e.to_string())?;
    let batch = dir.join("apply.bat");
    let body = format!(
        "@echo off\r\n\
         ping 127.0.0.1 -n 3 >nul\r\n\
         move /y \"{}\" \"{}\"\r\n\
         start \"\" \"{}\"\r\n",
        new_exe.display(),
        current.display(),
        current.display(),
    );
    std::fs::write(&batch, body).map_err(|e| format!("无法写入更新脚本：{e}"))?;
    Command::new("cmd")
        .args(["/C", "start", "", "/b"])
        .arg(&batch)
        .spawn()
        .map_err(|e| format!("无法启动更新脚本：{e}"))?;
    Ok(())
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
fn swap(_dir: &Path) -> Result<(), String> {
    Err("当前平台不支持自动替换，请用「打开发布页」手动更新".to_string())
}

/// The `.app` this executable lives in, if it was launched from a bundle.
#[cfg(target_os = "macos")]
fn bundle_root() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let contents = exe.parent()?.parent()?;
    let bundle = contents.parent()?;
    (bundle.extension()? == "app").then(|| bundle.to_path_buf())
}

/// First entry under `dir` (recursively) whose extension matches.
#[cfg(target_os = "macos")]
fn find_with_extension(dir: &Path, extension: &str) -> Option<PathBuf> {
    for entry in std::fs::read_dir(dir).ok()?.flatten() {
        let path = entry.path();
        if entry.file_type().ok()?.is_dir() {
            if path.extension().is_some_and(|value| value == extension) {
                return Some(path);
            }
            if let Some(found) = find_with_extension(&path, extension) {
                return Some(found);
            }
        }
    }
    None
}

/// First file under `dir` (recursively) named `name`.
#[cfg(target_os = "windows")]
fn find_named(dir: &Path, name: &str) -> Option<PathBuf> {
    for entry in std::fs::read_dir(dir).ok()?.flatten() {
        let path = entry.path();
        if path.is_dir() {
            if let Some(found) = find_named(&path, name) {
                return Some(found);
            }
        } else if path.file_name().is_some_and(|value| value == name) {
            return Some(path);
        }
    }
    None
}

/// Runs `curl` with the headers the GitHub API requires and returns stdout.
fn curl(url: &str) -> Result<String, String> {
    let output = Command::new("curl")
        .args([
            "-fsSL",
            "-H",
            "Accept: application/vnd.github+json",
            "-A",
            "G-Terminal",
        ])
        .arg(url)
        .output()
        .map_err(|e| format!("无法运行 curl：{e}"))?;
    if !output.status.success() {
        return Err(format!("请求 GitHub 失败（curl {}）", output.status));
    }
    String::from_utf8(output.stdout).map_err(|e| format!("响应不是文本：{e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_compare_numerically() {
        assert!(is_newer("0.4.10", "0.4.9"));
        assert!(is_newer("v0.5.0", "0.4.9"));
        assert!(!is_newer("0.4.1", "0.4.10"));
        assert!(!is_newer("0.4.1", "0.4.1"));
        assert!(!is_newer("nightly", "0.4.1"));
    }

    #[test]
    fn a_newer_release_picks_the_platform_asset() {
        let json = r#"{
            "tag_name": "v9.9.9",
            "html_url": "https://example.com/release",
            "body": "notes",
            "assets": [
                {"name": "G-Terminal-9.9.9-windows-x64.zip", "browser_download_url": "https://example.com/win.zip"},
                {"name": "G-Terminal-9.9.9-macos-arm64.tar.gz", "browser_download_url": "https://example.com/mac.tar.gz"}
            ]
        }"#;
        let status = parse_latest(json, "0.4.1").unwrap();
        match status {
            Status::Available(release) => {
                assert_eq!(release.version, "9.9.9");
                assert_eq!(release.url, "https://example.com/release");
                // The chosen asset is the one for this machine.
                if cfg!(target_os = "macos") {
                    assert!(release.asset_name.unwrap().contains("macos-"));
                }
            }
            other => panic!("expected an available release, got {other:?}"),
        }
    }

    #[test]
    fn an_older_tag_is_not_an_update() {
        let json = r#"{"tag_name":"v0.4.0","html_url":"x","assets":[]}"#;
        assert_eq!(
            parse_latest(json, "0.4.1").unwrap(),
            Status::UpToDate {
                current: "0.4.1".into()
            }
        );
    }

    #[test]
    fn a_tag_without_a_platform_asset_still_reports_the_release() {
        let json = r#"{"tag_name":"v9.9.9","html_url":"x","assets":[]}"#;
        match parse_latest(json, "0.4.1").unwrap() {
            Status::Available(release) => {
                assert!(release.asset.is_none());
                assert!(release.asset_name.is_none());
            }
            other => panic!("expected an available release, got {other:?}"),
        }
    }
}
