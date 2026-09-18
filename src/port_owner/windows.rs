//! Windows: a serial port is held by whichever process owns a handle to its
//! device object. Nothing reports that directly, so the whole system handle
//! table is walked and every handle's name is queried, which is how
//! Sysinternals' `handle.exe` answers the same question.
//!
//! Two things make that awkward. Querying a synchronous pipe's name can block
//! forever, so the names are collected by a small pool of workers under a
//! deadline rather than on the caller's thread: one stuck handle costs a
//! quarter of the pool, not the whole answer. And another user's handles cannot
//! be duplicated without elevation, so those processes are reported as hidden
//! instead of silently missing.

use super::{Owner, Report};
use std::collections::{HashMap, HashSet};
use std::ffi::c_void;
use std::os::windows::ffi::OsStringExt;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant};
use winreg::RegKey;
use winreg::enums::HKEY_LOCAL_MACHINE;
use winreg::types::FromRegValue;

pub const SUPPORTED: bool = true;
pub const CAN_RELEASE: bool = true;

type Handle = *mut c_void;

const PROCESS_TERMINATE: u32 = 0x0001;
const PROCESS_DUP_HANDLE: u32 = 0x0040;
const PROCESS_QUERY_LIMITED_INFORMATION: u32 = 0x1000;
const DUPLICATE_SAME_ACCESS: u32 = 0x0002;
/// Only handles opened for I/O carry these; the filter keeps the scan from
/// duplicating every event and registry key in the system.
const FILE_TYPE_CHAR: u32 = 0x0002;
const FILE_READ_DATA: u32 = 0x0001;
const FILE_WRITE_DATA: u32 = 0x0002;
const WAIT_TIMEOUT: u32 = 0x0000_0102;
const WM_CLOSE: u32 = 0x0010;
const TOKEN_QUERY: u32 = 0x0008;
const TOKEN_ELEVATION_CLASS: u32 = 20;
/// `STATUS_INFO_LENGTH_MISMATCH`, the buffer was too small.
const STATUS_INFO_LENGTH_MISMATCH: i32 = 0xC000_0004u32 as i32;
const SYSTEM_EXTENDED_HANDLE_INFORMATION: i32 = 64;
const OBJECT_NAME_INFORMATION: i32 = 1;
/// How long the name queries get before the scan answers with what it has.
const NAME_DEADLINE: Duration = Duration::from_millis(4000);
const NAME_WORKERS: usize = 4;

const DIGCF_PRESENT: u32 = 0x0000_0002;
const DIGCF_ALLCLASSES: u32 = 0x0000_0004;
const SPDRP_FRIENDLYNAME: u32 = 0x0000_000C;
const DIREG_DEV: u32 = 1;
const KEY_READ: u32 = 0x0002_0019;
const DIF_PROPERTYCHANGE: u32 = 18;
const DICS_FLAG_GLOBAL: u32 = 1;
/// Stop, then start again: the device's holders lose their handles.
const DICS_PROPCHANGE: u32 = 3;

#[link(name = "kernel32")]
unsafe extern "system" {
    fn OpenProcess(access: u32, inherit: i32, pid: u32) -> Handle;
    fn CloseHandle(handle: Handle) -> i32;
    fn GetCurrentProcess() -> Handle;
    fn GetFileType(handle: Handle) -> u32;
    fn DuplicateHandle(
        source_process: Handle,
        source: Handle,
        target_process: Handle,
        target: *mut Handle,
        access: u32,
        inherit: i32,
        options: u32,
    ) -> i32;
    fn QueryFullProcessImageNameW(
        process: Handle,
        flags: u32,
        exe: *mut u16,
        size: *mut u32,
    ) -> i32;
    fn TerminateProcess(process: Handle, code: u32) -> i32;
    fn WaitForSingleObject(handle: Handle, milliseconds: u32) -> u32;
    fn EnumWindows(
        callback: unsafe extern "system" fn(*mut c_void, isize) -> i32,
        param: isize,
    ) -> i32;
    fn GetWindowThreadProcessId(window: *mut c_void, pid: *mut u32) -> u32;
    fn IsWindowVisible(window: *mut c_void) -> i32;
    fn PostMessageW(window: *mut c_void, message: u32, wparam: usize, lparam: isize) -> i32;
}

#[link(name = "ntdll")]
unsafe extern "system" {
    fn NtQuerySystemInformation(
        class: i32,
        info: *mut c_void,
        length: u32,
        needed: *mut u32,
    ) -> i32;
    fn NtQueryObject(
        handle: Handle,
        class: i32,
        info: *mut c_void,
        length: u32,
        needed: *mut u32,
    ) -> i32;
}

#[link(name = "advapi32")]
unsafe extern "system" {
    fn OpenProcessToken(process: Handle, access: u32, token: *mut Handle) -> i32;
    fn GetTokenInformation(
        token: Handle,
        class: u32,
        info: *mut c_void,
        length: u32,
        needed: *mut u32,
    ) -> i32;
    fn RegQueryValueExW(
        key: *mut c_void,
        name: *const u16,
        reserved: *mut u32,
        kind: *mut u32,
        data: *mut u8,
        length: *mut u32,
    ) -> i32;
    fn RegCloseKey(key: *mut c_void) -> i32;
}

#[link(name = "setupapi")]
unsafe extern "system" {
    fn SetupDiGetClassDevsW(
        class: *const c_void,
        enumerator: *const u16,
        parent: *mut c_void,
        flags: u32,
    ) -> *mut c_void;
    fn SetupDiEnumDeviceInfo(set: *mut c_void, index: u32, data: *mut SpDevInfoData) -> i32;
    fn SetupDiGetDeviceRegistryPropertyW(
        set: *mut c_void,
        data: *const SpDevInfoData,
        property: u32,
        kind: *mut u32,
        buffer: *mut u8,
        size: u32,
        needed: *mut u32,
    ) -> i32;
    fn SetupDiOpenDevRegKey(
        set: *mut c_void,
        data: *const SpDevInfoData,
        scope: u32,
        profile: u32,
        key_type: u32,
        access: u32,
    ) -> *mut c_void;
    fn SetupDiSetClassInstallParamsW(
        set: *mut c_void,
        data: *const SpDevInfoData,
        params: *const SpClassInstallHeader,
        size: u32,
    ) -> i32;
    fn SetupDiCallClassInstaller(
        function: u32,
        set: *mut c_void,
        data: *const SpDevInfoData,
    ) -> i32;
    fn SetupDiDestroyDeviceInfoList(set: *mut c_void) -> i32;
}

/// The layout `NtQuerySystemInformation` uses for the extended handle table:
/// a count, a reserved word, then the entries. Getting the reserved word wrong
/// shifts the whole array, so it stays named.
#[repr(C)]
struct SystemHandleEntry {
    object: *mut c_void,
    pid: usize,
    handle: usize,
    access: u32,
    creator_backtrace: u16,
    object_type: u16,
    attributes: u32,
    reserved: u32,
}

/// A `UNICODE_STRING` header followed by the name's own bytes.
#[repr(C)]
struct UnicodeString {
    length: u16,
    maximum_length: u16,
    buffer: *mut u16,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct Guid {
    data1: u32,
    data2: u16,
    data3: u16,
    data4: [u8; 8],
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct SpDevInfoData {
    cb_size: u32,
    class_guid: Guid,
    dev_inst: u32,
    reserved: usize,
}

#[repr(C)]
struct SpClassInstallHeader {
    cb_size: u32,
    install_function: u32,
}

#[repr(C)]
struct SpPropChangeParams {
    header: SpClassInstallHeader,
    state_change: u32,
    scope: u32,
    hw_profile: u32,
}

thread_local! {
    /// `EnumWindows` cannot capture, so the pid to message and the number of
    /// windows sent to live here for the duration of one call.
    static CLOSE_TARGET: std::cell::Cell<(u32, u32)> = const { std::cell::Cell::new((0, 0)) };
}

pub fn elevated() -> bool {
    let mut token: Handle = std::ptr::null_mut();
    unsafe {
        if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) == 0 {
            return false;
        }
        let mut value: u32 = 0;
        let mut needed = 0u32;
        let ok = GetTokenInformation(
            token,
            TOKEN_ELEVATION_CLASS,
            &mut value as *mut u32 as *mut c_void,
            std::mem::size_of::<u32>() as u32,
            &mut needed,
        );
        CloseHandle(token);
        ok != 0 && value != 0
    }
}

pub fn scan(port: &str) -> Result<Report, String> {
    let targets = device_names(port);
    let candidates = handles_for_io()?;
    let (names, unreadable) = names_for(candidates);
    let mut owners: HashMap<u32, Owner> = HashMap::new();
    let mut hidden: HashSet<u32> = HashSet::new();
    for (pid, name) in names {
        match name {
            Some(name) if targets.iter().any(|target| name_matches(&name, target)) => {
                owners.entry(pid).or_insert_with(|| owner_of(pid));
            }
            None if unreadable.contains(&pid) => {
                hidden.insert(pid);
            }
            _ => {}
        }
    }
    Ok(Report {
        port: String::new(),
        owners: owners.into_values().collect(),
        // Only a process this one can identify but not inspect is a real gap;
        // system processes are not something the user is meant to close.
        hidden: hidden
            .into_iter()
            .filter(|pid| process_image(*pid).1.is_some())
            .count(),
    })
}

pub fn kill(pid: u32) -> Result<String, String> {
    if pid == std::process::id() {
        return Err("那是本程序自己，不能结束".into());
    }
    let handle = unsafe {
        OpenProcess(
            PROCESS_TERMINATE | PROCESS_QUERY_LIMITED_INFORMATION,
            0,
            pid,
        )
    };
    if handle.is_null() {
        return Err("没有权限结束该进程，请以管理员身份重新运行本程序".into());
    }
    // Ask first: a program with an editor open gets the chance to save or to
    // prompt, and only a refusal falls through to force.
    let windows = close_windows(pid);
    let stopped = unsafe { WaitForSingleObject(handle, 1500) } != WAIT_TIMEOUT;
    let message = if stopped {
        format!("已关闭 {windows} 个窗口，进程已退出")
    } else if unsafe { TerminateProcess(handle, 1) } != 0 {
        format!("已发送关闭请求，随后强制结束了进程（{windows} 个窗口）")
    } else {
        unsafe { CloseHandle(handle) };
        return Err("结束进程失败，请以管理员身份重新运行本程序".into());
    };
    unsafe { CloseHandle(handle) };
    Ok(message)
}

pub fn release(port: &str) -> Result<String, String> {
    unsafe {
        let set = SetupDiGetClassDevsW(
            std::ptr::null(),
            std::ptr::null(),
            std::ptr::null_mut(),
            DIGCF_ALLCLASSES | DIGCF_PRESENT,
        );
        if set.is_null() || set as isize == -1 {
            return Err("无法枚举设备（需要管理员权限）".into());
        }
        let mut index = 0u32;
        loop {
            let mut data = SpDevInfoData {
                cb_size: std::mem::size_of::<SpDevInfoData>() as u32,
                ..Default::default()
            };
            if SetupDiEnumDeviceInfo(set, index, &mut data) == 0 {
                break;
            }
            index += 1;
            if !is_port_device(set, &data, port) {
                continue;
            }
            // The class installer reads the request from these params, so they
            // have to be installed before the call rather than passed to it.
            let params = SpPropChangeParams {
                header: SpClassInstallHeader {
                    cb_size: std::mem::size_of::<SpPropChangeParams>() as u32,
                    install_function: DIF_PROPERTYCHANGE,
                },
                state_change: DICS_PROPCHANGE,
                scope: DICS_FLAG_GLOBAL,
                hw_profile: 0,
            };
            SetupDiSetClassInstallParamsW(
                set,
                &data,
                &params.header,
                std::mem::size_of::<SpPropChangeParams>() as u32,
            );
            let restarted = SetupDiCallClassInstaller(DIF_PROPERTYCHANGE, set, &data) != 0;
            SetupDiDestroyDeviceInfoList(set);
            return if restarted {
                Ok(format!("已重启 {port} 对应的设备，现在可以重新连接"))
            } else {
                Err("重启设备失败，请以管理员身份重新运行本程序".into())
            };
        }
        SetupDiDestroyDeviceInfoList(set);
        Err(format!("没有找到 {port} 对应的设备"))
    }
}

/// The names a handle to this port can carry: the device object the driver map
/// points at, plus the DOS name a caller may have opened instead.
fn device_names(port: &str) -> Vec<String> {
    let mut names = vec![
        format!("\\??\\{port}"),
        format!("\\\\.\\{port}"),
        format!("\\\\?\\{port}"),
    ];
    if let Ok(key) =
        RegKey::predef(HKEY_LOCAL_MACHINE).open_subkey(r"HARDWARE\DEVICEMAP\SERIALCOMM")
    {
        for (device, value) in key.enum_values().flatten() {
            let Ok(name) = String::from_reg_value(&value) else {
                continue;
            };
            if name.trim().eq_ignore_ascii_case(port) {
                names.push(device.clone());
                // A volume-less device object can also appear under the
                // global root, which is a spelling of the same thing.
                names.push(format!("\\??\\GLOBALROOT{device}"));
            }
        }
    }
    names
}

/// A handle's name is usually the device object, but a driver may append an
/// instance or a path, so a suffix match counts as well.
fn name_matches(name: &str, target: &str) -> bool {
    name.eq_ignore_ascii_case(target)
        || (name.len() > target.len()
            && name.is_char_boundary(name.len() - target.len())
            && name[name.len() - target.len()..].eq_ignore_ascii_case(target))
}

/// The (pid, handle) pairs that are files: the serial port's handle is a file
/// object, and so is every pipe, socket and ordinary file. The object type is
/// not a constant (it depends on how many types the kernel has registered), so
/// it is learned from a file handle this process opens itself.
fn handles_for_io() -> Result<Vec<(u32, usize)>, String> {
    let probe = std::env::temp_dir().join("g-terminal-port-owner-probe");
    let file = std::fs::File::create(&probe).map_err(|e| format!("无法建立探测文件：{e}"))?;
    let mine = std::os::windows::io::AsRawHandle::as_raw_handle(&file) as usize;
    let our_pid = std::process::id();
    let table = read_table()?;
    drop(file);
    let _ = std::fs::remove_file(&probe);

    let file_type = table
        .iter()
        .find(|entry| entry.pid == our_pid && entry.handle == mine)
        .map(|entry| entry.object_type)
        .ok_or_else(|| "无法确定文件对象的类型编号".to_string())?;

    let handles = table
        .into_iter()
        .filter(|entry| {
            entry.pid != 0
                && entry.object_type == file_type
                && entry.access & (FILE_READ_DATA | FILE_WRITE_DATA) != 0
        })
        .map(|entry| (entry.pid, entry.handle))
        .collect();
    Ok(handles)
}

/// One row of the system handle table.
struct TableEntry {
    pid: u32,
    handle: usize,
    access: u32,
    object_type: u16,
}

/// Reads the system handle table. The buffer grows until it fits; the table can
/// be large (six figures of handles on a busy desktop).
fn read_table() -> Result<Vec<TableEntry>, String> {
    let mut length: usize = 1 << 20;
    loop {
        // `usize` keeps the buffer 8-byte aligned for the entry struct.
        let mut buffer = vec![0usize; length / std::mem::size_of::<usize>()];
        let mut needed = 0u32;
        let status = unsafe {
            NtQuerySystemInformation(
                SYSTEM_EXTENDED_HANDLE_INFORMATION,
                buffer.as_mut_ptr() as *mut c_void,
                length as u32,
                &mut needed,
            )
        };
        if status == STATUS_INFO_LENGTH_MISMATCH {
            length = (needed as usize).max(length * 2);
            if length > (1 << 30) {
                return Err("系统句柄表异常地大，已放弃查找".into());
            }
            continue;
        }
        if status < 0 {
            return Err(format!("读取系统句柄表失败（0x{status:08X}）"));
        }
        let count = buffer[0];
        // The entries start after the count and the reserved word.
        let base = unsafe { buffer.as_ptr().add(2) as *const SystemHandleEntry };
        let max = (length.saturating_sub(2 * std::mem::size_of::<usize>()))
            / std::mem::size_of::<SystemHandleEntry>();
        let mut table = Vec::with_capacity(count.min(max));
        for index in 0..count.min(max) {
            let entry = unsafe { &*base.add(index) };
            table.push(TableEntry {
                pid: entry.pid as u32,
                handle: entry.handle,
                access: entry.access,
                object_type: entry.object_type,
            });
        }
        return Ok(table);
    }
}

/// Queries the object name of every candidate handle on a small worker pool.
///
/// Names come back as `Option`: `None` means the handle could not be duplicated
/// or named, which for a process this one can identify means a permission gap.
/// A worker that blocks on a pipe stays blocked, so the deadline ends the
/// collection with whatever the other workers produced.
fn names_for(candidates: Vec<(u32, usize)>) -> (Vec<(u32, Option<String>)>, HashSet<u32>) {
    let queue = Arc::new(Mutex::new(candidates.into_iter()));
    let (sender, receiver) = mpsc::channel();
    let mut workers = Vec::new();
    for _ in 0..NAME_WORKERS {
        let queue = queue.clone();
        let sender = sender.clone();
        workers.push(std::thread::spawn(move || {
            let mut processes: HashMap<u32, Handle> = HashMap::new();
            loop {
                let next = queue.lock().map(|mut queue| queue.next()).unwrap_or(None);
                let Some((pid, value)) = next else {
                    break;
                };
                let process = match processes.get(&pid) {
                    Some(process) => *process,
                    None => {
                        let process = unsafe { OpenProcess(PROCESS_DUP_HANDLE, 0, pid) };
                        if process.is_null() {
                            let _ = sender.send((pid, None));
                            continue;
                        }
                        processes.insert(pid, process);
                        process
                    }
                };
                let mut copy: Handle = std::ptr::null_mut();
                let duplicated = unsafe {
                    DuplicateHandle(
                        process,
                        value as Handle,
                        GetCurrentProcess(),
                        &mut copy,
                        0,
                        0,
                        DUPLICATE_SAME_ACCESS,
                    )
                };
                if duplicated == 0 {
                    let _ = sender.send((pid, None));
                    continue;
                }
                // Only character devices can be serial ports, and asking a
                // pipe's name is what blocks, so everything else is dropped
                // before the query.
                if unsafe { GetFileType(copy) } != FILE_TYPE_CHAR {
                    unsafe { CloseHandle(copy) };
                    continue;
                }
                let name = object_name(copy);
                unsafe { CloseHandle(copy) };
                if name.is_none() {
                    let _ = sender.send((pid, None));
                } else if let Some(name) = name {
                    let _ = sender.send((pid, Some(name)));
                }
            }
            for process in processes.into_values() {
                unsafe { CloseHandle(process) };
            }
        }));
    }
    drop(sender);

    let deadline = Instant::now() + NAME_DEADLINE;
    let mut names = Vec::new();
    let mut unreadable = HashSet::new();
    while let Some(remaining) = deadline.checked_duration_since(Instant::now()) {
        match receiver.recv_timeout(remaining) {
            Ok((pid, name)) => {
                if name.is_none() {
                    unreadable.insert(pid);
                }
                names.push((pid, name));
            }
            // Every worker finished, or the deadline passed.
            Err(_) => break,
        }
    }
    (names, unreadable)
}

/// The object name behind a duplicated handle, e.g. `\Device\Serial0`.
fn object_name(handle: Handle) -> Option<String> {
    let mut buffer = vec![0usize; (std::mem::size_of::<UnicodeString>() + 2048) / 8];
    let mut needed = 0u32;
    let status = unsafe {
        NtQueryObject(
            handle,
            OBJECT_NAME_INFORMATION,
            buffer.as_mut_ptr() as *mut c_void,
            (buffer.len() * std::mem::size_of::<usize>()) as u32,
            &mut needed,
        )
    };
    if status < 0 {
        return None;
    }
    let string = unsafe { &*(buffer.as_ptr() as *const UnicodeString) };
    if string.buffer.is_null() || string.length == 0 {
        return None;
    }
    let characters =
        unsafe { std::slice::from_raw_parts(string.buffer, (string.length / 2) as usize) };
    Some(String::from_utf16_lossy(characters))
}

fn owner_of(pid: u32) -> Owner {
    let (name, path) = process_image(pid);
    let killable = pid != std::process::id() && {
        let handle = unsafe { OpenProcess(PROCESS_TERMINATE, 0, pid) };
        if handle.is_null() {
            false
        } else {
            unsafe { CloseHandle(handle) };
            true
        }
    };
    Owner {
        pid,
        name,
        path,
        killable,
    }
}

fn process_image(pid: u32) -> (String, Option<PathBuf>) {
    let handle = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
    if handle.is_null() {
        return (format!("PID {pid}"), None);
    }
    let mut buffer = vec![0u16; 32768];
    let mut length = buffer.len() as u32;
    let ok = unsafe { QueryFullProcessImageNameW(handle, 0, buffer.as_mut_ptr(), &mut length) };
    unsafe { CloseHandle(handle) };
    if ok == 0 {
        return (format!("PID {pid}"), None);
    }
    let path = PathBuf::from(std::ffi::OsString::from_wide(&buffer[..length as usize]));
    let name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| format!("PID {pid}"));
    (name, Some(path))
}

unsafe extern "system" fn close_window(window: *mut c_void, _: isize) -> i32 {
    let (pid, count) = CLOSE_TARGET.with(|cell| cell.get());
    let mut owner = 0u32;
    unsafe { GetWindowThreadProcessId(window, &mut owner) };
    if owner == pid && unsafe { IsWindowVisible(window) } != 0 {
        unsafe { PostMessageW(window, WM_CLOSE, 0, 0) };
        CLOSE_TARGET.with(|cell| cell.set((pid, count + 1)));
    }
    1
}

fn close_windows(pid: u32) -> u32 {
    CLOSE_TARGET.with(|cell| cell.set((pid, 0)));
    unsafe { EnumWindows(close_window, 0) };
    CLOSE_TARGET.with(|cell| cell.get().1)
}

/// Matches a device to the port it backs, by its `PortName` value first and by
/// the `(COM3)` in its friendly name second.
unsafe fn is_port_device(set: *mut c_void, data: &SpDevInfoData, port: &str) -> bool {
    let key = unsafe { SetupDiOpenDevRegKey(set, data, DICS_FLAG_GLOBAL, 0, DIREG_DEV, KEY_READ) };
    if !key.is_null() && key as isize != -1 {
        let mut name: Vec<u16> = "PortName".encode_utf16().chain([0]).collect();
        let mut buffer = [0u8; 256];
        let mut length = buffer.len() as u32;
        let ok = unsafe {
            RegQueryValueExW(
                key,
                name.as_mut_ptr(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                buffer.as_mut_ptr(),
                &mut length,
            )
        };
        unsafe { RegCloseKey(key) };
        if ok == 0 {
            let value = wide_string(&buffer[..length as usize]);
            if value.eq_ignore_ascii_case(port) {
                return true;
            }
        }
    }
    // Friendly names read `USB Serial Port (COM3)`, which is what a device
    // without a readable PortName value still carries.
    let mut buffer = [0u8; 512];
    let ok = unsafe {
        SetupDiGetDeviceRegistryPropertyW(
            set,
            data,
            SPDRP_FRIENDLYNAME,
            std::ptr::null_mut(),
            buffer.as_mut_ptr(),
            512,
            std::ptr::null_mut(),
        )
    };
    if ok == 0 {
        return false;
    }
    wide_string(&buffer)
        .to_ascii_lowercase()
        .contains(&format!("({})", port.to_ascii_lowercase()))
}

/// The NUL-terminated UTF-16 text in `bytes`.
fn wide_string(bytes: &[u8]) -> String {
    let mut units = Vec::with_capacity(bytes.len() / 2);
    let mut index = 0;
    while index + 1 < bytes.len() {
        units.push(u16::from_le_bytes([bytes[index], bytes[index + 1]]));
        index += 2;
    }
    let length = units
        .iter()
        .position(|unit| *unit == 0)
        .unwrap_or(units.len());
    String::from_utf16_lossy(&units[..length])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_handle_name_may_carry_a_suffix() {
        assert!(name_matches("\\Device\\Serial0", "\\Device\\Serial0"));
        assert!(name_matches("\\device\\serial0", "\\Device\\Serial0"));
        assert!(name_matches("\\??\\COM3", "\\??\\COM3"));
        assert!(!name_matches("\\Device\\Serial10", "\\Device\\Serial1"));
        assert!(!name_matches("\\Device\\Serial", "\\Device\\Serial0"));
    }

    #[test]
    fn device_names_include_the_dos_name_even_without_a_map() {
        // The registry may not list the port at all; the DOS names are still
        // worth matching so the scan never comes back empty-handed.
        let names = device_names("COM99");
        assert!(names.iter().any(|name| name == "\\??\\COM99"));
        assert!(names.iter().any(|name| name == "\\\\.\\COM99"));
    }

    #[test]
    fn a_scan_of_an_imaginary_port_finds_nothing() {
        // The walk touches every handle in the system and a query can block, so
        // it runs on a thread a timeout can abandon rather than hanging the run.
        let (sender, receiver) = mpsc::channel();
        std::thread::spawn(move || {
            let _ = sender.send(scan("COM199"));
        });
        match receiver.recv_timeout(Duration::from_secs(30)) {
            Ok(Ok(report)) => assert!(report.owners.is_empty(), "{:?}", report.owners),
            Ok(Err(error)) => panic!("scan failed: {error}"),
            Err(_) => panic!("scan did not finish within 30s"),
        }
    }

    #[test]
    fn our_own_open_device_shows_up_with_our_pid() {
        // A real end-to-end check of the machinery without a serial port: the
        // `NUL` device is a character device this process can open, and a
        // handle to it must be found in the table.
        let device = std::fs::File::open("\\\\.\\NUL")
            .or_else(|_| std::fs::File::open("NUL"))
            .expect("the NUL device must be openable");
        let (sender, receiver) = mpsc::channel();
        std::thread::spawn(move || {
            let _ = sender.send(names_for(handles_for_io().unwrap_or_default()).0);
        });
        let names = receiver
            .recv_timeout(Duration::from_secs(30))
            .expect("the scan must finish");
        drop(device);
        assert!(
            names.iter().any(|(pid, handle)| *pid == std::process::id()
                && handle
                    .as_ref()
                    .is_some_and(|name| name.to_ascii_lowercase().ends_with("null"))),
            "our own handle to NUL was not found"
        );
    }
}
