//! Every other platform: the question is a good one, but there is no code here
//! that knows how to answer it, so the UI is told nothing is available.

use super::Report;

pub const SUPPORTED: bool = false;
pub const CAN_RELEASE: bool = false;

pub fn elevated() -> bool {
    false
}

pub fn scan(_port: &str) -> Result<Report, String> {
    Err("此平台尚不支持查找串口占用程序".into())
}

pub fn kill(_pid: u32) -> Result<String, String> {
    Err("此平台尚不支持结束占用串口的进程".into())
}

pub fn release(_port: &str) -> Result<String, String> {
    Err("此平台尚不支持重启串口设备".into())
}
