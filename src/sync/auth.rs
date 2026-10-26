use super::{http_client, network_error};
use anyhow::{Context, Result, bail};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use reqwest::{Url, blocking::Client};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::{
    io::{Read, Write},
    net::TcpListener,
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, Instant},
};
use zeroize::Zeroizing;

const AUTHORITY: &str = "https://login.microsoftonline.com/consumers/oauth2/v2.0";
const SCOPE: &str = "https://graph.microsoft.com/Files.ReadWrite.AppFolder offline_access";
const DEFAULT_CLIENT_ID: &str = "bfc1025c-64f7-434f-bdf9-6a3cee653a55";

/// Developer configuration only, never an end-user setting. No client secret
/// belongs in a desktop app. The runtime override also supports local testing.
pub fn client_id() -> Option<String> {
    std::env::var("GTERMINAL_ONEDRIVE_CLIENT_ID")
        .ok()
        .or_else(|| option_env!("GTERMINAL_ONEDRIVE_CLIENT_ID").map(str::to_owned))
        .or_else(|| Some(DEFAULT_CLIENT_ID.to_owned()))
        .filter(|value| uuid::Uuid::parse_str(value).is_ok())
}

fn credential() -> Result<keyring::Entry> {
    let id = client_id().context("此版本尚未配置 OneDrive 应用 ID，请联系发布者")?;
    keyring::Entry::new("G-Terminal.OneDrive", &id).context("无法打开系统凭据库")
}

pub fn disconnect() -> Result<()> {
    match credential()?.delete_credential() {
        Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
        Err(_) => bail!("无法删除系统凭据，请重试"),
    }
}

#[derive(Deserialize)]
struct Token {
    access_token: String,
    refresh_token: Option<String>,
}

impl Drop for Token {
    fn drop(&mut self) {
        use zeroize::Zeroize;
        self.access_token.zeroize();
        self.refresh_token.zeroize();
    }
}

fn exchange(client: &Client, fields: &[(&str, &str)]) -> Result<Token> {
    exchange_at(client, &format!("{AUTHORITY}/token"), fields)
}

fn exchange_at(client: &Client, endpoint: &str, fields: &[(&str, &str)]) -> Result<Token> {
    let response = client
        .post(endpoint)
        .form(fields)
        .send()
        .map_err(network_error)?;
    let status = response.status();
    let mut body = Zeroizing::new(Vec::new());
    response
        .take(64 * 1024 + 1)
        .read_to_end(&mut body)
        .context("无法读取微软登录响应")?;
    if body.len() > 64 * 1024 {
        bail!("微软登录响应超过大小限制");
    }
    if !status.is_success() {
        bail!("{}", token_error(status.as_u16(), &body, fields));
    }
    serde_json::from_slice(&body).map_err(|_| anyhow::anyhow!("微软返回了无效的登录响应"))
}

#[derive(Default, Deserialize)]
#[serde(default)]
struct TokenError {
    error: String,
    error_description: String,
    error_codes: Vec<u64>,
}

/// Only expose the OAuth error fields, never dump response bodies or request
/// parameters. Some identity providers echo rejected parameters in descriptions.
fn token_error(status: u16, body: &[u8], fields: &[(&str, &str)]) -> String {
    let phase = if fields.contains(&("grant_type", "refresh_token")) {
        "刷新登录令牌"
    } else {
        "用授权码换取令牌"
    };
    let Ok(error) = serde_json::from_slice::<TokenError>(body) else {
        return format!("{phase}失败（HTTP {status}，微软未返回有效的 OAuth 错误信息）");
    };
    let mut detail = format!("{}：{}", error.error, error.error_description);
    for (key, value) in fields {
        if !value.is_empty()
            && matches!(
                *key,
                "code"
                    | "code_verifier"
                    | "refresh_token"
                    | "client_secret"
                    | "client_assertion"
                    | "password"
            )
        {
            let mut encoded = Url::parse("https://localhost/").unwrap();
            encoded.query_pairs_mut().append_pair("secret", value);
            let encoded = encoded.query().unwrap().strip_prefix("secret=").unwrap();
            detail = detail
                .replace(value, "[已隐藏]")
                .replace(encoded, "[已隐藏]");
        }
    }
    let detail: String = detail
        .chars()
        .filter(|c| !c.is_control())
        .take(1200)
        .collect();
    let hint = if error.error_codes.contains(&7000218) || error.error == "invalid_client" {
        "请检查应用的桌面公共客户端配置；桌面程序不应配置客户端密码。"
    } else if error.error == "invalid_grant" {
        "授权码或刷新凭据被拒绝；请重新发起登录，并按微软错误说明检查回调、PKCE 和账号配置。"
    } else {
        "请根据微软返回的错误代码检查应用配置。"
    };
    let codes = error
        .error_codes
        .iter()
        .take(8)
        .map(|code| format!("AADSTS{code}"))
        .collect::<Vec<_>>()
        .join(", ");
    format!("{phase}失败（HTTP {status}）\n{detail}\n{codes}\n{hint}")
}

pub(super) fn refresh(client: &Client) -> Result<Zeroizing<String>> {
    let id = client_id().context("此版本尚未配置 OneDrive 应用 ID")?;
    let stored = Zeroizing::new(
        credential()?
            .get_secret()
            .map_err(|_| anyhow::anyhow!("未找到 OneDrive 登录凭据，请重新登录"))?,
    );
    let token = exchange(
        client,
        &[
            ("client_id", &id),
            ("grant_type", "refresh_token"),
            (
                "refresh_token",
                std::str::from_utf8(&stored).context("无效登录凭据，请重新登录")?,
            ),
            ("scope", SCOPE),
        ],
    )?;
    if let Some(refresh) = &token.refresh_token {
        credential()?
            .set_secret(refresh.as_bytes())
            .map_err(|_| anyhow::anyhow!("无法更新系统凭据库，已停止同步"))?;
    }
    Ok(Zeroizing::new(token.access_token.clone()))
}

/// Developer diagnostic: validate the stored refresh credential without touching
/// the connection list. The access token is dropped and zeroized immediately.
pub fn check_login() -> Result<()> {
    refresh(&http_client()?).map(|_| ())
}

/// Browser OAuth with PKCE and a random-port IPv4 loopback. The listener only
/// accepts a bounded GET with the matching state; unrelated requests are ignored.
pub fn login(open_browser: impl FnOnce(&str) -> Result<()>, cancel: &AtomicBool) -> Result<()> {
    let id = client_id().context("此版本尚未配置 OneDrive 应用 ID")?;
    let listener = TcpListener::bind("127.0.0.1:0").context("无法创建本地登录回调")?;
    listener.set_nonblocking(true)?;
    let redirect = format!("http://localhost:{}/", listener.local_addr()?.port());
    let verifier = Zeroizing::new(format!(
        "{}{}",
        uuid::Uuid::new_v4().simple(),
        uuid::Uuid::new_v4().simple()
    ));
    let state = uuid::Uuid::new_v4().to_string();
    let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
    let mut url = Url::parse(&format!("{AUTHORITY}/authorize"))?;
    url.query_pairs_mut().extend_pairs([
        ("client_id", id.as_str()),
        ("response_type", "code"),
        ("redirect_uri", redirect.as_str()),
        ("response_mode", "query"),
        ("scope", SCOPE),
        ("state", state.as_str()),
        ("code_challenge", challenge.as_str()),
        ("code_challenge_method", "S256"),
        ("prompt", "select_account"),
    ]);
    open_browser(url.as_str())?;
    let code = Zeroizing::new(wait_for_code(listener, &state, cancel)?);
    if cancel.load(Ordering::Relaxed) {
        bail!("已取消登录");
    }
    let token = exchange(
        &http_client()?,
        &[
            ("client_id", &id),
            ("grant_type", "authorization_code"),
            ("code", &code),
            ("redirect_uri", &redirect),
            ("code_verifier", &verifier),
        ],
    )?;
    let refresh = token
        .refresh_token
        .as_ref()
        .context("微软未返回离线授权，请重新授权")?;
    if cancel.load(Ordering::Relaxed) {
        bail!("已取消登录");
    }
    credential()?
        .set_secret(refresh.as_bytes())
        .map_err(|_| anyhow::anyhow!("无法保存到系统凭据库，登录未保存"))?;
    Ok(())
}

fn wait_for_code(listener: TcpListener, state: &str, cancel: &AtomicBool) -> Result<String> {
    let deadline = Instant::now() + Duration::from_secs(180);
    while Instant::now() < deadline && !cancel.load(Ordering::Relaxed) {
        let (mut stream, _) = match listener.accept() {
            Ok(stream) => stream,
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::sleep(Duration::from_millis(100));
                continue;
            }
            Err(error) => return Err(error.into()),
        };
        stream.set_read_timeout(Some(Duration::from_secs(2)))?;
        stream.set_write_timeout(Some(Duration::from_secs(2)))?;
        let mut bytes = Vec::new();
        let mut buf = [0; 1024];
        // Stop at the header terminator; do not wait for a browser to close.
        while bytes.len() < 16 * 1024 && !bytes.windows(4).any(|w| w == b"\r\n\r\n") {
            match stream.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(count) => bytes.extend_from_slice(&buf[..count]),
            }
        }
        let request = String::from_utf8_lossy(&bytes);
        let result = callback_code(&request, state);
        let accepted = result.is_ok();
        let message = if accepted {
            "已收到微软授权回调，请返回 G-Terminal 查看登录结果。收到回调不代表登录已完成。"
        } else {
            "授权回调校验失败，请返回 G-Terminal 重新登录。"
        };
        let status = if accepted {
            "200 OK"
        } else {
            "400 Bad Request"
        };
        let _ = write!(
            stream,
            "HTTP/1.1 {status}\r\nContent-Type: text/plain; charset=utf-8\r\nCache-Control: no-store\r\nConnection: close\r\nContent-Length: {}\r\n\r\n{message}",
            message.len()
        );
        if accepted {
            return result;
        }
        // An explicit denial with the right state is terminal; strangers and
        // favicon requests cannot abort an in-progress login.
        if request.contains("error=") && matching_state(&request, state) {
            bail!("用户拒绝了微软授权");
        }
    }
    bail!("登录已取消或超时，请重新登录")
}

fn callback_url(request: &str) -> Option<Url> {
    let mut parts = request.lines().next()?.split_whitespace();
    if parts.next()? != "GET" {
        return None;
    }
    let path = parts.next()?;
    if !path.starts_with("/?") {
        return None;
    }
    Url::parse(&format!("http://localhost{path}")).ok()
}

fn matching_state(request: &str, state: &str) -> bool {
    callback_url(request).is_some_and(|url| {
        let values: Vec<_> = url
            .query_pairs()
            .filter(|(k, _)| k == "state")
            .map(|(_, v)| v.into_owned())
            .collect();
        values == [state]
    })
}

pub(super) fn callback_code(request: &str, state: &str) -> Result<String> {
    if !matching_state(request, state) {
        bail!("登录回调校验失败");
    }
    let url = callback_url(request).context("无效登录回调")?;
    let codes: Vec<_> = url
        .query_pairs()
        .filter(|(k, _)| k == "code")
        .map(|(_, v)| v.into_owned())
        .collect();
    if codes.len() != 1 || codes[0].is_empty() || url.query_pairs().any(|(k, _)| k == "error") {
        bail!("微软授权失败");
    }
    Ok(codes[0].clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_failures_keep_microsoft_reason_and_hide_credentials() {
        let fields = [
            ("grant_type", "authorization_code"),
            ("code", "sensitive+code"),
            ("code_verifier", "sensitive-verifier"),
        ];
        let body = br#"{"error":"invalid_grant","error_description":"AADSTS50148: PKCE mismatch sensitive+code sensitive%2Bcode sensitive-verifier","error_codes":[50148],"access_token":"never-display-this"}"#;
        let error = token_error(400, body, &fields);
        assert!(error.contains("PKCE mismatch"));
        assert!(error.contains("AADSTS50148"));
        for secret in [
            "sensitive+code",
            "sensitive%2Bcode",
            "sensitive-verifier",
            "never-display-this",
        ] {
            assert!(!error.contains(secret), "leaked {secret}");
        }
        assert!(!token_error(500, b"not-json secret", &fields).contains("not-json secret"));
    }

    #[test]
    fn token_request_posts_form_encoded_code_redirect_and_verifier() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let endpoint = format!("http://{}/token", listener.local_addr().unwrap());
        let peer = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            let mut bytes = Vec::new();
            let mut buffer = [0; 1024];
            let header_end = loop {
                let count = stream.read(&mut buffer).unwrap();
                assert!(count > 0);
                bytes.extend_from_slice(&buffer[..count]);
                if let Some(end) = bytes.windows(4).position(|w| w == b"\r\n\r\n") {
                    break end + 4;
                }
            };
            let headers = String::from_utf8_lossy(&bytes[..header_end]).to_lowercase();
            assert!(headers.contains("content-type: application/x-www-form-urlencoded"));
            let length = headers
                .lines()
                .find_map(|line| line.strip_prefix("content-length: "))
                .unwrap()
                .parse::<usize>()
                .unwrap();
            while bytes.len() < header_end + length {
                let count = stream.read(&mut buffer).unwrap();
                assert!(count > 0);
                bytes.extend_from_slice(&buffer[..count]);
            }
            let body = std::str::from_utf8(&bytes[header_end..]).unwrap();
            let mut url = Url::parse("https://localhost").unwrap();
            url.set_query(Some(body));
            let values: std::collections::BTreeMap<_, _> = url.query_pairs().into_owned().collect();
            assert_eq!(values["code"], "test+code");
            assert_eq!(values["code_verifier"], "test-verifier");
            assert_eq!(values["redirect_uri"], "http://localhost:12345/");
            assert!(!values.contains_key("client_secret"));
            let body = r#"{"error":"invalid_request","error_description":"The request is missing a required parameter."}"#;
            write!(stream, "HTTP/1.1 400 Bad Request\r\nContent-Type: application/json\r\nConnection: close\r\nContent-Length: {}\r\n\r\n{body}", body.len()).unwrap();
        });
        let error = exchange_at(
            &Client::builder().no_proxy().build().unwrap(),
            &endpoint,
            &[
                ("grant_type", "authorization_code"),
                ("code", "test+code"),
                ("code_verifier", "test-verifier"),
                ("redirect_uri", "http://localhost:12345/"),
            ],
        )
        .err()
        .unwrap();
        assert!(error.to_string().contains("missing a required parameter"));
        peer.join().unwrap();
    }
}
