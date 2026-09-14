//! Reserved Agent protocol. No deployment or remote execution is performed.
//! Transport: a future authenticated SSH subsystem; length-prefixed JSON,
//! 4-byte big-endian length, maximum 1 MiB, explicit version negotiation.
use serde::{Deserialize, Serialize};
pub const VERSION: u32 = 1;
pub const MAX_FRAME: usize = 1024 * 1024;

#[derive(Debug, Serialize, Deserialize)]
pub struct Request {
    pub id: u64,
    pub version: u32,
    pub method: Method,
}
#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "method", content = "params", rename_all = "snake_case")]
pub enum Method {
    Hello,
    SystemInfo,
    Cancel { request_id: u64 },
}
#[derive(Debug, Serialize, Deserialize)]
pub struct Response {
    pub id: u64,
    pub version: u32,
    pub result: Option<serde_json::Value>,
    pub error: Option<String>,
}

pub fn encode(request: &Request) -> anyhow::Result<Vec<u8>> {
    anyhow::ensure!(request.version == VERSION, "Unsupported Agent version");
    let bytes = serde_json::to_vec(request)?;
    anyhow::ensure!(bytes.len() <= MAX_FRAME, "Agent frame exceeds limit");
    let mut frame = (bytes.len() as u32).to_be_bytes().to_vec();
    frame.extend(bytes);
    Ok(frame)
}
