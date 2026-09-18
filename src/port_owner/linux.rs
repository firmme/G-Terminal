//! Linux: a process holding a port has an open descriptor for it, so the
//! answer is a walk of `/proc/<pid>/fd` with each descriptor's device number
//! compared against the port's. That is what `fuser` and `lsof` do. Descriptors
//! of other users need root, so an unprivileged scan counts them as hidden
//! instead of silently missing them.

use super::{Owner, Report};
use std::fs;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

pub const SUPPORTED: bool = true;
/// Restarting a USB serial device writes to sysfs, which needs root.
pub const CAN_RELEASE: bool = true;

const SIGTERM: i32 = 15;
const SIGKILL: i32 = 9;

/// The two libc calls this module needs, kept in their own namespace so
/// `kill` does not collide with the function the UI calls.
mod c {
    unsafe extern "C" {
        pub(super) fn geteuid() -> u32;
        pub(super) fn kill(pid: i32, sig: i32) -> i32;
    }
}

pub fn elevated() -> bool {
    let uid = unsafe { c::geteuid() };
    uid == 0
}

pub fn scan(port: &str) -> Result<Report, String> {
    let target = fs::metadata(port).map_err(|e| format!("无法读取 {port}：{e}"))?;
    let device = target.rdev();
    let me = unsafe { c::geteuid() };
    let mut owners = Vec::new();
    let mut hidden = 0usize;

    let entries = fs::read_dir("/proc").map_err(|e| format!("无法读取 /proc：{e}"))?;
    for entry in entries.flatten() {
        let Some(pid) = entry
            .file_name()
            .to_str()
            .and_then(|name| name.parse::<u32>().ok())
        else {
            continue;
        };
        let process = entry.path();
        let descriptors = match fs::read_dir(process.join("fd")) {
            Ok(descriptors) => descriptors,
            // Kernel threads have no descriptor table, and another user's is
            // unreadable. Only the second is a gap in the answer.
            Err(_) => {
                if process_uid(&process) != me && me != 0 {
                    hidden += 1;
                }
                continue;
            }
        };
        let holds = descriptors.flatten().any(|descriptor| {
            // `stat` on the /proc link follows it to the open file, so the
            // device number is the port's own however it was opened.
            fs::metadata(descriptor.path()).is_ok_and(|opened| opened.rdev() == device)
        });
        if holds {
            let uid = process_uid(&process);
            owners.push(owner(pid, uid == me || me == 0));
        }
    }
    Ok(Report {
        port: String::new(),
        owners,
        hidden,
    })
}

pub fn kill(pid: u32) -> Result<String, String> {
    if pid == std::process::id() {
        return Err("那是本程序自己，不能结束".into());
    }
    unsafe { c::kill(pid as i32, SIGTERM) };
    if wait_gone(pid, Duration::from_millis(1500)) {
        return Ok("已发送 SIGTERM，进程已退出".into());
    }
    unsafe { c::kill(pid as i32, SIGKILL) };
    if wait_gone(pid, Duration::from_millis(1000)) {
        Ok("进程忽略 SIGTERM，已用 SIGKILL 强制结束".into())
    } else {
        Err("进程仍未退出，请用 root 权限重试".into())
    }
}

pub fn release(port: &str) -> Result<String, String> {
    let tty = Path::new(port)
        .file_name()
        .ok_or_else(|| format!("无法从 {port} 取出设备名"))?
        .to_string_lossy()
        .into_owned();
    let device = fs::canonicalize(format!("/sys/class/tty/{tty}/device"))
        .map_err(|e| format!("{port} 不是可重启的串口设备（{e}）"))?;
    let interface = device
        .parent()
        .ok_or_else(|| "无法定位 USB 接口".to_string())?;
    let name = interface
        .file_name()
        .ok_or_else(|| "无法定位 USB 接口名".to_string())?
        .to_string_lossy()
        .into_owned();
    let driver = fs::read_link(interface.join("driver"))
        .map_err(|_| format!("{port} 不是 USB 转串口设备，只能结束占用它的进程"))?;
    let driver = driver
        .file_name()
        .ok_or_else(|| "无法识别 USB 驱动".to_string())?
        .to_string_lossy()
        .into_owned();
    let base = Path::new("/sys/bus/usb/drivers").join(&driver);
    // Unbinding drops every open descriptor, which is what forces the port
    // free; binding it again restores the device immediately.
    fs::write(base.join("unbind"), format!("{name}\n")).map_err(|e| describe("解绑", &e))?;
    std::thread::sleep(Duration::from_millis(400));
    fs::write(base.join("bind"), format!("{name}\n")).map_err(|e| describe("重新绑定", &e))?;
    Ok(format!("已重启 {name}（驱动 {driver}），现在可以重新连接"))
}

fn describe(action: &str, error: &std::io::Error) -> String {
    if error.kind() == std::io::ErrorKind::PermissionDenied {
        format!("{action}设备需要 root 权限，请以 root 运行或改用结束进程")
    } else {
        format!("{action}设备失败：{error}")
    }
}

fn process_uid(process: &Path) -> u32 {
    fs::metadata(process)
        .map(|meta| meta.uid())
        .unwrap_or(u32::MAX)
}

fn wait_gone(pid: u32, timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    loop {
        // Signal 0 only asks whether the process is still there.
        if unsafe { c::kill(pid as i32, 0) } != 0 {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn owner(pid: u32, killable: bool) -> Owner {
    let exe = fs::read_link(format!("/proc/{pid}/exe")).ok();
    let name = exe
        .as_ref()
        .and_then(|path| path.file_name())
        .map(|name| name.to_string_lossy().into_owned())
        .or_else(|| {
            fs::read_to_string(format!("/proc/{pid}/comm"))
                .ok()
                .map(|name| name.trim().to_string())
        })
        .unwrap_or_else(|| format!("PID {pid}"));
    Owner {
        pid,
        name,
        path: exe.map(PathBuf::from).filter(|path| path.is_absolute()),
        killable,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_missing_device_is_reported_not_ignored() {
        let error = scan("/dev/definitely-not-a-port").expect_err("must fail");
        assert!(error.contains("无法读取"), "{error}");
    }

    #[test]
    fn a_process_we_can_see_is_killable_unless_it_is_ours() {
        let owner = owner(std::process::id(), true);
        assert_eq!(owner.pid, std::process::id());
        assert!(!owner.name.is_empty());
    }
}
