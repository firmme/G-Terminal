//! macOS: `libproc` lists a process's descriptors, and a serial port is one of
//! them as a vnode whose path is the device. Enumerating it that way is what
//! `lsof` does. Another user's descriptors need root, so those processes are
//! counted as hidden rather than missed. There is no unprivileged way to force
//! a USB serial device to restart, so `release` is not offered here.

use super::{Owner, Report};
use std::ffi::c_void;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

pub const SUPPORTED: bool = true;
pub const CAN_RELEASE: bool = false;
/// There is no prompt-free way to raise privileges here, and an osascript one would need the user's password for a task the app cannot check.
pub const CAN_ELEVATE: bool = false;

const PROC_PIDLISTFDS: i32 = 1;
const PROC_PIDFDVNODEPATHINFO: i32 = 2;
const PROX_FDTYPE_VNODE: u32 = 1;
const MAXPATHLEN: usize = 1024;
const SIGTERM: i32 = 15;
const SIGKILL: i32 = 9;

#[link(name = "proc")]
unsafe extern "C" {
    fn proc_listallpids(buffer: *mut c_void, size: i32) -> i32;
    fn proc_pidinfo(pid: i32, flavor: i32, arg: u64, buffer: *mut c_void, size: i32) -> i32;
    fn proc_pidfdinfo(pid: i32, fd: i32, flavor: i32, buffer: *mut c_void, size: i32) -> i32;
    fn proc_pidpath(pid: i32, buffer: *mut c_void, size: u32) -> i32;
    fn proc_name(pid: i32, buffer: *mut c_void, size: u32) -> i32;
}

/// The two libc calls beyond `libproc`, kept in their own namespace so `kill`
/// does not collide with the function the UI calls.
mod c {
    unsafe extern "C" {
        pub(super) fn geteuid() -> u32;
        pub(super) fn kill(pid: i32, sig: i32) -> i32;
    }
}

/// One entry of `PROC_PIDLISTFDS`.
#[repr(C)]
#[derive(Clone, Copy)]
struct ProcFdInfo {
    fd: i32,
    fd_type: u32,
}

/// `struct vinfo_stat` is read as bytes: only its size matters here, and
/// spelling out its thirty fields would only invite a mismatch.
#[repr(C)]
#[derive(Clone, Copy)]
struct VinfoStat {
    bytes: [u8; 136],
}

#[repr(C)]
#[derive(Clone, Copy)]
struct VnodeInfo {
    stat: VinfoStat,
    kind: i32,
    pad: i32,
    fsid: [i32; 2],
}

/// `struct proc_fileinfo`, read as bytes: only its size matters here.
#[repr(C)]
#[derive(Clone, Copy)]
struct ProcFileInfo {
    bytes: [u8; 24],
}

/// `struct vnode_fdinfowithpath`: `proc_fileinfo`, then the vnode info, then the
/// path. `PROC_PIDFDVNODEPATHINFO` fills *this* — despite the name it is not a
/// bare `vnode_info_path`, and a 1176-byte buffer is refused with no write.
#[repr(C)]
#[derive(Clone, Copy)]
struct VnodeFdInfoWithPath {
    file: ProcFileInfo,
    vnode: VnodeInfo,
    path: [u8; MAXPATHLEN],
}

const VNODE_INFO_PATH_SIZE: usize = std::mem::size_of::<VnodeFdInfoWithPath>();
/// The path must land 24 bytes past the vnode info, which is what the kernel
/// writes; the asserts keep the two in step.
const _: () = assert!(std::mem::size_of::<VnodeInfo>() == 152);
const _: () = assert!(VNODE_INFO_PATH_SIZE == 24 + 152 + MAXPATHLEN);

pub fn elevated() -> bool {
    let uid = unsafe { c::geteuid() };
    uid == 0
}

pub fn port_is_free(port: &str) -> Result<bool, String> {
    // A second open succeeds here too, so the descriptor scan answers; libproc
    // makes it cheap enough to run before a connect.
    Ok(scan(port)?.owners.iter().all(Owner::is_self))
}

pub fn scan(port: &str) -> Result<Report, String> {
    let target = fs::canonicalize(port).map_err(|e| format!("无法读取 {port}：{e}"))?;
    let me = unsafe { c::geteuid() };
    let mut owners = Vec::new();
    let mut hidden = 0usize;

    for pid in all_pids() {
        if pid <= 0 || pid == std::process::id() as i32 {
            continue;
        }
        let process = Process::new(pid);
        // A process whose name cannot be read at all is a system task with no
        // descriptors of its own, not a gap in the answer.
        if process.name.is_none() && process.exe.is_none() {
            continue;
        }
        let Some(fds) = process.descriptors() else {
            if me != 0 {
                hidden += 1;
            }
            continue;
        };
        let holds = fds.iter().any(|fd| {
            fd.fd_type == PROX_FDTYPE_VNODE
                && process
                    .vnode_path(fd.fd)
                    .is_some_and(|path| matches(&path, &target))
        });
        if holds {
            let name = process.name.clone().unwrap_or_else(|| format!("PID {pid}"));
            owners.push(Owner {
                pid: pid as u32,
                name,
                path: process.exe.clone(),
                // Only processes whose descriptors this one can read are
                // listed, and those belong to the same user (or we are root).
                killable: true,
            });
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

pub fn elevate_kill(_pid: u32, _report: &std::path::Path) -> Result<(), String> {
    Err("macOS 上请以 root 重新运行本程序后再结束该进程".into())
}

pub fn elevate_release(_port: &str, _report: &std::path::Path) -> Result<(), String> {
    Err("macOS 上请以 root 重新运行本程序".into())
}

pub fn release(_port: &str) -> Result<String, String> {
    Err("macOS 上无法单独重启串口设备，请结束占用它的进程".into())
}

/// One process's libproc view, read once per scan.
struct Process {
    pid: i32,
    name: Option<String>,
    exe: Option<PathBuf>,
}

impl Process {
    fn new(pid: i32) -> Self {
        let exe = string_from(|buffer, size| unsafe { proc_pidpath(pid, buffer, size) })
            .map(PathBuf::from)
            .filter(|path| path.is_absolute());
        let name = string_from(|buffer, size| unsafe { proc_name(pid, buffer, size) });
        Self { pid, name, exe }
    }

    /// The descriptor table, or `None` when it cannot be read: kernel tasks
    /// have none, and another user's needs root.
    fn descriptors(&self) -> Option<Vec<ProcFdInfo>> {
        let size = std::mem::size_of::<ProcFdInfo>() as i32;
        let needed = unsafe { proc_pidinfo(self.pid, PROC_PIDLISTFDS, 0, std::ptr::null_mut(), 0) };
        if needed <= 0 {
            return None;
        }
        let mut fds = vec![ProcFdInfo { fd: 0, fd_type: 0 }; needed as usize / size as usize + 8];
        let written = unsafe {
            proc_pidinfo(
                self.pid,
                PROC_PIDLISTFDS,
                0,
                fds.as_mut_ptr() as *mut c_void,
                needed,
            )
        };
        if written <= 0 {
            return None;
        }
        fds.truncate(written as usize / size as usize);
        Some(fds)
    }

    fn vnode_path(&self, fd: i32) -> Option<String> {
        // `VnodeInfoPath` opens with 64-bit fields, so the buffer has to be
        // aligned for them.
        let mut buffer = vec![0u64; VNODE_INFO_PATH_SIZE.div_ceil(8)];
        let info = buffer.as_mut_ptr() as *mut VnodeFdInfoWithPath;
        let written = unsafe {
            proc_pidfdinfo(
                self.pid,
                fd,
                PROC_PIDFDVNODEPATHINFO,
                info as *mut c_void,
                VNODE_INFO_PATH_SIZE as i32,
            )
        };
        if written < VNODE_INFO_PATH_SIZE as i32 {
            return None;
        }
        let path = unsafe { &(*info).path };
        let end = path.iter().position(|byte| *byte == 0)?;
        Some(String::from_utf8_lossy(&path[..end]).into_owned())
    }
}

/// Calls a libproc function that fills a C string, returning it as a `String`.
fn string_from(call: impl Fn(*mut c_void, u32) -> i32) -> Option<String> {
    let mut buffer = [0u8; 4096];
    if call(buffer.as_mut_ptr() as *mut c_void, buffer.len() as u32) <= 0 {
        return None;
    }
    let end = buffer.iter().position(|byte| *byte == 0)?;
    Some(String::from_utf8_lossy(&buffer[..end]).into_owned())
}

/// Whether a descriptor's path is the port, following symlinks when the two
/// spellings differ (`/dev/serial/by-id/...` against `/dev/cu....`).
fn matches(open_path: &str, target: &Path) -> bool {
    let open = Path::new(open_path);
    open == target || fs::canonicalize(open).is_ok_and(|resolved| resolved == target)
}

fn all_pids() -> Vec<i32> {
    let count = unsafe { proc_listallpids(std::ptr::null_mut(), 0) };
    if count <= 0 {
        return Vec::new();
    }
    // Room to spare: processes can start between the two calls.
    let mut pids = vec![0i32; count as usize + 64];
    let written =
        unsafe { proc_listallpids(pids.as_mut_ptr() as *mut c_void, pids.len() as i32 * 4) };
    if written <= 0 {
        return Vec::new();
    }
    pids.truncate(written as usize);
    pids
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
