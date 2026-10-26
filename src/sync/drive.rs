use super::{Catalog, MAX_BYTES, Preference, auth, http_client, network_error};
use anyhow::{Context, Result, bail};
use reqwest::{
    StatusCode,
    blocking::{Client, Response},
};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::{io::Read, path::PathBuf};

const GRAPH: &str = "https://graph.microsoft.com/v1.0";
const FILE: &str = "connections-v1.json";

pub struct Outcome {
    pub input: Catalog,
    pub merged: Catalog,
    baseline_path: PathBuf,
}

impl Outcome {
    /// Commit the sync base only AFTER the UI has applied and saved the result.
    /// A crash before that point must download remote changes again, not delete them.
    pub fn commit(&self) -> Result<()> {
        let parent = self.baseline_path.parent().context("无效同步缓存路径")?;
        std::fs::create_dir_all(parent)?;
        let temp = parent.join(format!("{}.tmp", uuid::Uuid::new_v4()));
        std::fs::write(&temp, serde_json::to_vec(&self.merged)?)?;
        std::fs::rename(&temp, &self.baseline_path).context("无法保存同步基线")
    }
}

pub fn synchronise(local: Catalog, preference: Preference) -> Result<Outcome> {
    local.validate()?;
    let client = http_client()?;
    let token = auth::refresh(&client)?;
    let folder: Folder = checked(
        client
            .get(format!("{GRAPH}/me/drive/special/approot"))
            .bearer_auth(token.as_str())
            .send()
            .map_err(network_error)?,
    )?
    .json()
    .map_err(network_error)?;
    let drive_id = folder.parent_reference.drive_id;
    let account_hash = Sha256::digest(format!("{}:{drive_id}", auth::client_id().unwrap()));
    let baseline_path =
        crate::config::Settings::path().with_file_name(format!("onedrive-{account_hash:x}.json"));
    let base = match std::fs::read(&baseline_path) {
        Ok(raw) => {
            let base: Catalog =
                serde_json::from_slice(&raw).context("同步基线损坏，已停止同步以保护数据")?;
            base.validate()?;
            base
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Catalog::default(),
        Err(error) => return Err(error).context("无法读取同步基线"),
    };
    let store = Drive {
        client,
        token: token.as_str(),
        folder_id: folder.id,
        graph: GRAPH.into(),
    };
    let merged = merge_and_upload(&store, &base, &local, preference)?;
    Ok(Outcome {
        input: local,
        merged,
        baseline_path,
    })
}

#[derive(Deserialize)]
struct Folder {
    id: String,
    #[serde(rename = "parentReference")]
    parent_reference: Parent,
}
#[derive(Deserialize)]
struct Parent {
    #[serde(rename = "driveId")]
    drive_id: String,
}

#[derive(Deserialize)]
struct Item {
    id: String,
    #[serde(rename = "eTag")]
    etag: String,
    size: u64,
}

pub(super) struct Snapshot {
    pub catalog: Catalog,
    pub revision: Option<(String, String)>,
}

pub(super) trait Store {
    fn read(&self) -> Result<Snapshot>;
    /// False means another device won the conditional write; fetch and merge again.
    fn write(&self, revision: Option<&(String, String)>, catalog: &Catalog) -> Result<bool>;
}

pub(super) fn merge_and_upload(
    store: &impl Store,
    base: &Catalog,
    local: &Catalog,
    preference: Preference,
) -> Result<Catalog> {
    for _ in 0..3 {
        let remote = store.read()?;
        if remote.revision.is_none() && base != &Catalog::default() && preference == Preference::Ask
        {
            bail!(
                "同步冲突：云端同步文件已被删除。使用本机将重新上传；使用云端将清空本机同步列表。"
            );
        }
        let merged = if remote.revision.is_none() && base != &Catalog::default() {
            if preference == Preference::Cloud {
                Catalog::default()
            } else {
                local.clone()
            }
        } else {
            Catalog::merge(base, local, &remote.catalog, preference)?
        };
        if merged == remote.catalog && remote.revision.is_some() {
            return Ok(merged);
        }
        if store.write(remote.revision.as_ref(), &merged)? {
            return Ok(merged);
        }
    }
    bail!("其他设备正在修改连接列表，请稍后重试")
}

struct Drive<'a> {
    client: Client,
    token: &'a str,
    folder_id: String,
    graph: String,
}

impl Drive<'_> {
    fn metadata(&self) -> Result<Option<Item>> {
        let response = self
            .client
            .get(format!(
                "{}/me/drive/items/{}:/{FILE}",
                self.graph, self.folder_id
            ))
            .bearer_auth(self.token)
            .send()
            .map_err(network_error)?;
        if response.status() == StatusCode::NOT_FOUND {
            return Ok(None);
        }
        Ok(Some(checked(response)?.json().map_err(network_error)?))
    }
}

impl Store for Drive<'_> {
    fn read(&self) -> Result<Snapshot> {
        for _ in 0..3 {
            let Some(item) = self.metadata()? else {
                return Ok(Snapshot {
                    catalog: Catalog::default(),
                    revision: None,
                });
            };
            if item.size > MAX_BYTES {
                bail!("云端连接列表超过 4 MiB，已停止同步");
            }
            let response = self
                .client
                .get(format!("{}/me/drive/items/{}/content", self.graph, item.id))
                .bearer_auth(self.token)
                .send()
                .map_err(network_error)?;
            // Graph redirects content to a preauthenticated URL. Never forward
            // the Graph bearer token to the download host or follow plain HTTP.
            let response = if response.status() == StatusCode::FOUND {
                let location = response
                    .headers()
                    .get(reqwest::header::LOCATION)
                    .context("微软未返回下载地址")?
                    .to_str()
                    .context("无效下载地址")?;
                let url = reqwest::Url::parse(location)?;
                if url.scheme() != "https" {
                    bail!("拒绝不安全的云端下载地址");
                }
                self.client.get(url).send().map_err(network_error)?
            } else {
                response
            };
            let mut bytes = Vec::new();
            checked(response)?
                .take(MAX_BYTES + 1)
                .read_to_end(&mut bytes)?;
            if bytes.len() as u64 > MAX_BYTES {
                bail!("云端连接列表超过 4 MiB");
            }
            let catalog: Catalog =
                serde_json::from_slice(&bytes).context("云端连接文件损坏，未覆盖任何数据")?;
            catalog.validate()?;
            if self.metadata()?.is_some_and(|next| next.etag == item.etag) {
                return Ok(Snapshot {
                    catalog,
                    revision: Some((item.id, item.etag)),
                });
            }
        }
        bail!("云端文件正在变化，请稍后重试")
    }

    fn write(&self, revision: Option<&(String, String)>, catalog: &Catalog) -> Result<bool> {
        let bytes = serde_json::to_vec_pretty(catalog)?;
        if bytes.len() as u64 > MAX_BYTES {
            bail!("连接列表超过 4 MiB");
        }
        let request = if let Some((id, etag)) = revision {
            self.client
                .put(format!("{}/me/drive/items/{id}/content", self.graph))
                .header(reqwest::header::IF_MATCH, etag)
        } else {
            self.client
                .put(format!(
                    "{}/me/drive/items/{}:/{FILE}:/content",
                    self.graph, self.folder_id
                ))
                .query(&[("@microsoft.graph.conflictBehavior", "fail")])
                .header(reqwest::header::IF_NONE_MATCH, "*")
        };
        let response = request
            .bearer_auth(self.token)
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .body(bytes)
            .send()
            .map_err(network_error)?;
        if matches!(
            response.status(),
            StatusCode::PRECONDITION_FAILED | StatusCode::CONFLICT
        ) {
            return Ok(false);
        }
        checked(response)?;
        Ok(true)
    }
}

fn checked(response: Response) -> Result<Response> {
    let status = response.status();
    if status.is_success() {
        return Ok(response);
    }
    match status {
        StatusCode::UNAUTHORIZED => bail!("微软授权已失效，请重新登录"),
        StatusCode::FORBIDDEN => bail!("OneDrive 权限不足，请重新授权应用文件夹访问权限"),
        StatusCode::TOO_MANY_REQUESTS => bail!("微软服务暂时限流，请稍后重试"),
        _ => bail!("OneDrive 请求失败（HTTP {}）", status.as_u16()),
    }
}

#[cfg(test)]
mod http_tests {
    use super::*;
    use std::{
        io::{Read, Write},
        net::TcpListener,
        thread,
    };

    /// A local HTTP peer asserts the actual wire protocol, not just the merge
    /// algorithm. It never authenticates to Microsoft or uses real credentials.
    fn server(steps: Vec<(String, u16, String)>) -> (String, thread::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let worker = thread::spawn(move || {
            for (expected, status, body) in steps {
                let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
                let mut stream = loop {
                    match listener.accept() {
                        Ok((stream, _)) => break stream,
                        Err(error)
                            if error.kind() == std::io::ErrorKind::WouldBlock
                                && std::time::Instant::now() < deadline =>
                        {
                            thread::sleep(std::time::Duration::from_millis(5))
                        }
                        Err(error) => panic!("mock HTTP accept: {error}"),
                    }
                };
                stream
                    .set_read_timeout(Some(std::time::Duration::from_secs(5)))
                    .unwrap();
                let mut bytes = Vec::new();
                let mut buf = [0; 1024];
                let header_end = loop {
                    let count = stream.read(&mut buf).unwrap();
                    assert!(count > 0);
                    bytes.extend_from_slice(&buf[..count]);
                    if let Some(end) = bytes.windows(4).position(|w| w == b"\r\n\r\n") {
                        break end + 4;
                    }
                };
                let headers = String::from_utf8_lossy(&bytes[..header_end]).to_lowercase();
                let length = headers
                    .lines()
                    .find_map(|line| line.strip_prefix("content-length: "))
                    .map(|s| s.parse::<usize>().unwrap())
                    .unwrap_or(0);
                while bytes.len() < header_end + length {
                    let count = stream.read(&mut buf).unwrap();
                    assert!(count > 0);
                    bytes.extend_from_slice(&buf[..count]);
                }
                let request = String::from_utf8_lossy(&bytes);
                for text in expected.lines() {
                    assert!(
                        request.to_lowercase().contains(&text.to_lowercase()),
                        "missing {text} in {request}"
                    );
                }
                write!(stream, "HTTP/1.1 {status} Mock\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).unwrap();
            }
        });
        (url, worker)
    }

    #[test]
    fn wire_read_checks_revision_and_update_is_conditional() {
        let catalog = Catalog::default();
        let content = serde_json::to_string(&catalog).unwrap();
        let metadata =
            serde_json::json!({"id":"file", "eTag":"\"rev1\"", "size":content.len()}).to_string();
        let (graph, worker) = server(vec![
            ("GET /me/drive/items/folder:/connections-v1.json\nauthorization: Bearer test-token".into(), 200, metadata.clone()),
            ("GET /me/drive/items/file/content".into(), 200, content),
            ("GET /me/drive/items/folder:/connections-v1.json".into(), 200, metadata),
            ("PUT /me/drive/items/file/content\nif-match: \"rev1\"\ncontent-type: application/json".into(), 412, "{}".into()),
        ]);
        let drive = Drive {
            client: Client::builder().no_proxy().build().unwrap(),
            token: "test-token",
            folder_id: "folder".into(),
            graph,
        };
        let snapshot = drive.read().unwrap();
        assert_eq!(snapshot.catalog, catalog);
        assert!(!drive.write(snapshot.revision.as_ref(), &catalog).unwrap());
        worker.join().unwrap();
    }

    #[test]
    fn wire_first_upload_requests_fail_on_conflict() {
        let (graph, worker) = server(vec![
            ("GET /me/drive/items/folder:/connections-v1.json".into(), 404, "{}".into()),
            ("PUT /me/drive/items/folder:/connections-v1.json:/content?%40microsoft.graph.conflictBehavior=fail\nif-none-match: *".into(), 201, "{}".into()),
        ]);
        let drive = Drive {
            client: Client::builder().no_proxy().build().unwrap(),
            token: "test-token",
            folder_id: "folder".into(),
            graph,
        };
        let snapshot = drive.read().unwrap();
        assert!(snapshot.revision.is_none());
        assert!(drive.write(None, &Catalog::default()).unwrap());
        worker.join().unwrap();
    }
}
