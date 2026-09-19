//! Serial-port discovery and connection bookkeeping.
//!
//! Opening a COM port is exclusive: a second opener fails while the first is
//! alive. That shapes `auto`. The choice is made from the enumerated device
//! list plus the set of ports this process already holds — never by trying to
//! open candidates, because a successful probe would have to become the
//! connection, and a failed one can take seconds to time out. A port held by
//! another program is simply reported as a failed open.
//!
//! Bluetooth SPP ports are real serial ports, but they are usually the wrong
//! choice when a cable is plugged in, so they sort last and `auto` only falls
//! back to them when every physical port is taken.
//!
//! Port names differ by platform and so does their order. Windows numbers COM
//! ports; POSIX systems name devices, where a natural order is needed so
//! `usbmodem2` precedes `usbmodem10`. On macOS each device appears twice, as
//! `/dev/cu.*` (call-out) and `/dev/tty.*` (dial-in); only the call-out is used
//! for an outgoing connection, so the dial-in twin is dropped from the list.

use anyhow::{Context, Result, bail};
use serial2::SerialPort;
use std::{
    collections::HashSet,
    sync::{
        Arc, Mutex, OnceLock,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread,
    time::Duration,
};

/// An example device name for the current platform, shown in validation errors
/// and the connection editor's placeholder.
pub const PORT_EXAMPLE: &str = if cfg!(windows) {
    "COM3"
} else if cfg!(target_os = "macos") {
    "/dev/cu.usbserial-0001"
} else {
    "/dev/ttyUSB0"
};

/// A blocking read waits this long for a byte before reporting a timeout. The
/// reader loop uses the timeout to notice a shutdown request promptly and to
/// release the device.
const READ_TIMEOUT: Duration = Duration::from_millis(50);
const WRITE_TIMEOUT: Duration = Duration::from_millis(50);

/// One entry of the device list shown in the UI.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SerialPortInfo {
    pub name: String,
    /// Windows exposes Bluetooth SPP ports through the `BthModem` driver.
    pub bluetooth: bool,
}

/// Ports this process currently has open, keyed by upper-cased device name.
fn in_use() -> &'static Mutex<HashSet<String>> {
    static IN_USE: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();
    IN_USE.get_or_init(|| Mutex::new(HashSet::new()))
}

fn key(name: &str) -> String {
    name.trim().to_ascii_uppercase()
}

/// Holds a port's claim until dropped. The session owns one for as long as its
/// workers run, so `auto` never picks a device this process already has open.
pub struct Guard {
    key: String,
}

impl Drop for Guard {
    fn drop(&mut self) {
        if let Ok(mut set) = in_use().lock() {
            set.remove(&self.key);
        }
    }
}

pub fn occupy(name: &str) -> Guard {
    let key = key(name);
    if let Ok(mut set) = in_use().lock() {
        set.insert(key.clone());
    }
    Guard { key }
}

/// Whether this process already has `name` open. Checking here turns a second
/// connect to the same device into an immediate, clear failure instead of a
/// driver-level collision.
pub fn held(name: &str) -> bool {
    in_use()
        .lock()
        .map(|set| set.contains(&key(name)))
        .unwrap_or(false)
}

fn snapshot() -> HashSet<String> {
    in_use().lock().map(|set| set.clone()).unwrap_or_default()
}

/// Physical ports first, then Bluetooth, each in ascending port order.
pub fn sort_ports(ports: &mut [SerialPortInfo]) {
    ports.sort_by(|a, b| {
        a.bluetooth
            .cmp(&b.bluetooth)
            .then_with(|| compare_names(&a.name, &b.name))
    });
}

/// Whether a name belongs to a Bluetooth serial port. Windows reads this from
/// the driver key, but POSIX only has the name: `Bluetooth-*` on macOS and
/// `rfcomm*` on Linux.
#[cfg(not(windows))]
fn is_bluetooth(name: &str) -> bool {
    let name = name.to_ascii_lowercase();
    name.contains("bluetooth") || name.contains("rfcomm")
}

/// macOS exposes every device twice, as `/dev/cu.X` (call-out) and `/dev/tty.X`
/// (dial-in). Only `cu` is usable for an outgoing connection, so a `tty` entry
/// is dropped when its `cu` twin exists. A `tty` with no `cu` counterpart is
/// kept rather than hidden.
#[cfg(target_os = "macos")]
fn drop_shadowed_dialin(ports: &mut Vec<SerialPortInfo>) {
    let callouts: HashSet<String> = ports
        .iter()
        .filter(|port| port.name.starts_with("/dev/cu."))
        .map(|port| port.name.clone())
        .collect();
    ports.retain(|port| match port.name.strip_prefix("/dev/tty.") {
        Some(suffix) => !callouts.contains(&format!("/dev/cu.{suffix}")),
        None => true,
    });
}

/// Port naming differs by platform, so the order does too.
fn compare_names(a: &str, b: &str) -> std::cmp::Ordering {
    if cfg!(windows) {
        rank_com(a).cmp(&rank_com(b))
    } else {
        rank_posix(a).cmp(&rank_posix(b))
    }
}

/// `COM10` sorts after `COM9`, which a plain string sort gets wrong.
fn rank_com(name: &str) -> (u8, u32, String) {
    match name
        .trim()
        .to_ascii_uppercase()
        .strip_prefix("COM")
        .and_then(|n| n.parse::<u32>().ok())
    {
        Some(n) => (0, n, String::new()),
        None => (1, 0, name.trim().to_ascii_uppercase()),
    }
}

/// POSIX paths. `/dev/cu.*` (call-out) sorts before `/dev/tty.*` (dial-in,
/// which waits for carrier); within a group the order is natural.
fn rank_posix(name: &str) -> (u8, String) {
    let name = name.trim();
    let group = if name.starts_with("/dev/cu.") {
        0
    } else if name.starts_with("/dev/tty.") {
        1
    } else {
        2
    };
    (group, natural_key(name))
}

/// Zero-pads every run of digits so a plain string compare orders them
/// numerically. POSIX device names embed counter numbers, where a lexicographic
/// compare would put `usbmodem10` before `usbmodem2`.
fn natural_key(name: &str) -> String {
    let mut key = String::with_capacity(name.len() + 16);
    let mut digits = String::new();
    for ch in name.chars() {
        if ch.is_ascii_digit() {
            digits.push(ch);
        } else {
            flush_digits(&mut key, &mut digits);
            key.push(ch);
        }
    }
    flush_digits(&mut key, &mut digits);
    key
}

/// Writes a digit run into `key` padded to a fixed width, so `2` sorts before
/// `10` under a lexicographic compare.
fn flush_digits(key: &mut String, digits: &mut String) {
    if digits.is_empty() {
        return;
    }
    for _ in digits.len()..10 {
        key.push('0');
    }
    key.push_str(digits);
    digits.clear();
}

/// Enumerated ports, ready for display. Failures yield an empty list rather
/// than an error: not having discovered any port is a normal state.
pub fn available_ports() -> Vec<SerialPortInfo> {
    let mut ports = platform::enumerate();
    sort_ports(&mut ports);
    ports
}

/// The device `auto` should open: the first port this process does not already
/// hold. `ports` is expected to be sorted physical-first, so a Bluetooth port
/// is only chosen once every physical one is busy.
pub fn pick_auto(ports: &[SerialPortInfo], held: &HashSet<String>) -> Option<String> {
    ports
        .iter()
        .find(|port| !held.contains(&key(&port.name)))
        .map(|port| port.name.clone())
}

/// Resolves a profile's `port` field. `auto` chooses a device; anything else is
/// used as written, even if it is not currently attached.
pub fn resolve(requested: &str) -> Result<String> {
    let requested = requested.trim();
    if !requested.eq_ignore_ascii_case("auto") {
        if requested.is_empty() {
            bail!("请输入串口设备名，例如 {PORT_EXAMPLE} 或 auto");
        }
        return Ok(requested.to_string());
    }
    let ports = available_ports();
    if ports.is_empty() {
        bail!("没有检测到可用串口");
    }
    pick_auto(&ports, &snapshot()).context("所有串口都被本程序占用")
}

/// How long the UI is willing to wait for a port to open before giving up.
/// `CreateFile` on a busy or half-dead device can block for a long time in the
/// driver, and the caller runs on the UI thread.
pub const OPEN_TIMEOUT: Duration = Duration::from_millis(2000);

/// Opens a port in raw 8N1 mode. A port held by another program fails here,
/// which is exactly the error the user should see.
pub fn open(name: &str, baud: u32) -> Result<SerialPort> {
    let mut port = SerialPort::open(name, baud)
        .with_context(|| format!("无法打开串口 {name}（可能被其它程序占用）"))?;
    port.set_read_timeout(READ_TIMEOUT)?;
    port.set_write_timeout(WRITE_TIMEOUT)?;
    // Ask the tty layer for exclusive use, so a later opener fails instead of
    // silently splitting the incoming bytes with us. macOS and Linux both let a
    // device be opened twice by default. This does not disturb a program that
    // opened the port *before* us — that one is caught by the owner scan before
    // connecting.
    set_exclusive(&port);
    Ok(port)
}

/// Sets `TIOCEXCL` on the port. The request number differs by platform; the
/// call is best-effort, because a device that refuses it still works, just
/// without the guarantee.
#[cfg(unix)]
fn set_exclusive(port: &SerialPort) {
    use std::os::unix::io::AsRawFd;
    #[cfg(target_os = "macos")]
    const TIOCEXCL: std::ffi::c_ulong = 0x2000_740d;
    #[cfg(not(target_os = "macos"))]
    const TIOCEXCL: std::ffi::c_ulong = 0x540c;
    unsafe extern "C" {
        fn ioctl(fd: std::ffi::c_int, request: std::ffi::c_ulong, ...) -> std::ffi::c_int;
    }
    unsafe {
        ioctl(port.as_raw_fd(), TIOCEXCL);
    }
}

#[cfg(not(unix))]
fn set_exclusive(_port: &SerialPort) {}

/// Opens a port on a worker so a wedged driver cannot freeze the UI. The caller
/// waits at most `timeout`; if the open is still stuck, the worker's eventual
/// handle is dropped as soon as it arrives, closing the port again.
pub fn open_bounded(name: &str, baud: u32, timeout: Duration) -> Result<SerialPort> {
    let (sender, receiver) = mpsc::channel();
    let abandoned = Arc::new(AtomicBool::new(false));
    let flag = abandoned.clone();
    let owned = name.to_string();
    thread::spawn(move || match open(&owned, baud) {
        Ok(port) if !flag.load(Ordering::Acquire) => {
            let _ = sender.send(Ok(port));
        }
        // Abandoned: dropping the port here releases the device.
        Ok(_) => {}
        Err(e) => {
            let _ = sender.send(Err(e));
        }
    });
    match receiver.recv_timeout(timeout) {
        Ok(result) => result,
        Err(_) => {
            abandoned.store(true, Ordering::Release);
            bail!("打开串口 {name} 超时，端口可能被占用或设备无响应")
        }
    }
}

#[cfg(windows)]
mod platform {
    use super::SerialPortInfo;
    use winreg::{RegKey, enums::HKEY_LOCAL_MACHINE, types::FromRegValue};

    /// `HARDWARE\DEVICEMAP\SERIALCOMM` maps a driver device name to the port it
    /// backs, e.g. `\Device\BthModem0` -> `COM5`. The device name is what tells
    /// a Bluetooth SPP port apart from a physical one; the public port
    /// enumeration throws it away.
    pub fn enumerate() -> Vec<SerialPortInfo> {
        let Ok(key) =
            RegKey::predef(HKEY_LOCAL_MACHINE).open_subkey(r"HARDWARE\DEVICEMAP\SERIALCOMM")
        else {
            // The key does not exist until a serial device has been present.
            return Vec::new();
        };
        let mut ports = Vec::new();
        for (device, value) in key.enum_values().flatten() {
            let Ok(name) = String::from_reg_value(&value) else {
                continue;
            };
            let name = name.trim().to_string();
            if name.is_empty() {
                continue;
            }
            let bluetooth = device.to_ascii_uppercase().contains("BTHMODEM");
            ports.push(SerialPortInfo { name, bluetooth });
        }
        ports
    }
}

#[cfg(not(windows))]
mod platform {
    use super::SerialPortInfo;

    pub fn enumerate() -> Vec<SerialPortInfo> {
        let mut ports: Vec<SerialPortInfo> = serial2::SerialPort::available_ports()
            .unwrap_or_default()
            .into_iter()
            .map(|path| {
                let name = path.to_string_lossy().into_owned();
                let bluetooth = super::is_bluetooth(&name);
                SerialPortInfo { name, bluetooth }
            })
            .collect();
        #[cfg(target_os = "macos")]
        super::drop_shadowed_dialin(&mut ports);
        ports
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn info(name: &str, bluetooth: bool) -> SerialPortInfo {
        SerialPortInfo {
            name: name.into(),
            bluetooth,
        }
    }

    #[test]
    fn bluetooth_ports_sort_after_physical_ones_by_number() {
        let mut ports = vec![
            info("COM10", false),
            info("COM4", true),
            info("COM9", false),
            info("COM2", false),
        ];
        sort_ports(&mut ports);
        let names: Vec<_> = ports.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, ["COM2", "COM9", "COM10", "COM4"]);
    }

    #[test]
    #[cfg(not(windows))]
    fn posix_ports_sort_callouts_first_and_numbers_naturally() {
        let mut ports = vec![
            info("/dev/tty.usbserial-10", false),
            info("/dev/cu.usbmodem10", false),
            info("/dev/cu.usbmodem2", false),
            info("/dev/tty.Bluetooth-Incoming-Port", true),
        ];
        sort_ports(&mut ports);
        let names: Vec<_> = ports.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(
            names,
            [
                "/dev/cu.usbmodem2",
                "/dev/cu.usbmodem10",
                "/dev/tty.usbserial-10",
                "/dev/tty.Bluetooth-Incoming-Port",
            ]
        );
    }

    #[test]
    #[cfg(not(windows))]
    fn posix_bluetooth_ports_are_recognised_by_name() {
        assert!(is_bluetooth("/dev/cu.Bluetooth-Incoming-Port"));
        assert!(is_bluetooth("/dev/cu.Bluetooth-Modem"));
        assert!(is_bluetooth("/dev/rfcomm0"));
        assert!(!is_bluetooth("/dev/cu.usbserial-0001"));
    }

    #[test]
    #[cfg(target_os = "macos")]
    fn macos_drops_dialin_when_the_callout_exists() {
        let mut ports = vec![
            info("/dev/cu.usbserial-0001", false),
            info("/dev/tty.usbserial-0001", false),
            info("/dev/tty.only-dialin", false),
        ];
        drop_shadowed_dialin(&mut ports);
        let names: Vec<_> = ports.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, ["/dev/cu.usbserial-0001", "/dev/tty.only-dialin"]);
    }

    #[test]
    fn auto_skips_ports_this_process_holds() {
        let ports = vec![info("COM3", false), info("COM4", false), info("COM7", true)];
        let held = HashSet::from(["COM4".to_string()]);
        assert_eq!(pick_auto(&ports, &held).as_deref(), Some("COM3"));
    }

    #[test]
    fn auto_falls_back_to_bluetooth_only_when_every_physical_port_is_held() {
        let ports = vec![info("COM3", false), info("COM4", false), info("COM7", true)];
        // `held` always stores the normalised key `occupy` writes.
        let held = HashSet::from(["COM3".to_string(), "COM4".to_string()]);
        assert_eq!(pick_auto(&ports, &held).as_deref(), Some("COM7"));
        let all = HashSet::from(["COM3".to_string(), "COM4".to_string(), "COM7".to_string()]);
        assert_eq!(pick_auto(&ports, &all), None);
    }

    #[test]
    fn auto_case_insensitive_and_literal_ports_pass_through_resolution() {
        assert_eq!(pick_auto(&[], &HashSet::new()), None);
        assert_eq!(key(" com3 "), "COM3");
    }

    #[test]
    fn the_claim_on_a_port_is_released_with_its_guard() {
        assert!(!held("COM42"));
        let guard = occupy("com42");
        assert!(held("COM42"));
        drop(guard);
        assert!(!held("com42"));
    }

    #[test]
    fn bounded_open_reports_a_missing_port_without_blocking() {
        let started = std::time::Instant::now();
        let error =
            open_bounded("COM199", 115_200, OPEN_TIMEOUT).expect_err("COM199 must not exist");
        assert!(format!("{error:#}").contains("COM199"), "{error:#}");
        assert!(started.elapsed() < OPEN_TIMEOUT, "the open was not bounded");
    }
}
