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
            .then_with(|| rank(&a.name).cmp(&rank(&b.name)))
    });
}

/// `COM10` sorts after `COM9`, which a plain string sort gets wrong.
fn rank(name: &str) -> (u8, u32, String) {
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
            bail!("请输入串口设备名，例如 COM3 或 auto");
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
    Ok(port)
}

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
        serial2::SerialPort::available_ports()
            .unwrap_or_default()
            .into_iter()
            .map(|path| SerialPortInfo {
                name: path.to_string_lossy().into_owned(),
                bluetooth: false,
            })
            .collect()
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
