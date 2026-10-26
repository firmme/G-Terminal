//! OneDrive synchronisation. Only saved connections and groups cross devices.
//! Network and credential-store work runs on the UI's background worker.

mod auth;
mod drive;
mod model;
pub use auth::{check_login, client_id, disconnect, login};
pub use drive::{Outcome, synchronise};
pub use model::{Catalog, Preference, ensure_ids};

pub(crate) const MAX_BYTES: u64 = 4 * 1024 * 1024;

pub(crate) fn http_client() -> anyhow::Result<reqwest::blocking::Client> {
    Ok(reqwest::blocking::Client::builder()
        .connect_timeout(std::time::Duration::from_secs(10))
        .timeout(std::time::Duration::from_secs(30))
        .redirect(reqwest::redirect::Policy::none())
        .user_agent(concat!("G-Terminal/", env!("CARGO_PKG_VERSION")))
        .build()?)
}

/// Never expose a request URL/body containing credentials in a UI error.
pub(crate) fn network_error(_: reqwest::Error) -> anyhow::Error {
    anyhow::anyhow!("无法连接微软服务，请检查网络后重试")
}

#[cfg(test)]
mod tests;
