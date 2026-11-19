//! Passive monitoring and a separate operational connection to an existing Codex daemon.
//! Both own only their byte proxy, never the shared daemon.
use crate::{
    config::SourceConfig,
    domain::{ProviderId, RuntimeStatus, SessionKey},
};
use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, HashSet},
    path::{Path, PathBuf},
    process::Stdio,
    time::Duration,
};
use tokio::{
    io::{Join, join},
    process::{Child, ChildStdin, ChildStdout, Command},
    time::timeout,
};
use tokio_tungstenite::{
    WebSocketStream, client_async_with_config,
    tungstenite::{Message, protocol::WebSocketConfig},
};

type Socket = WebSocketStream<Join<ChildStdout, ChildStdin>>;
#[derive(Clone, Debug)]
pub struct LiveSession {
    pub title: String,
    pub cwd: Option<PathBuf>,
    pub status: RuntimeStatus,
}

pub struct LiveClient {
    socket: Socket,
    // kill_on_drop affects only our byte proxy, never the shared daemon.
    _proxy: Option<Child>,
    sequence: u64,
    initialization: Value,
    source: SourceConfig,
}
impl Drop for LiveClient {
    fn drop(&mut self) {
        if let Some(mut proxy) = self._proxy.take() {
            let _ = proxy.start_kill();
            if let Ok(runtime) = tokio::runtime::Handle::try_current() {
                runtime.spawn(async move {
                    let _ = proxy.wait().await;
                });
            }
        }
    }
}
impl LiveClient {
    pub async fn connect(binary: &Path, source: SourceConfig) -> Result<Self, String> {
        timeout(Duration::from_secs(5), Self::open(binary, source, false))
            .await
            .map_err(|_| "live status connection timed out".to_owned())?
    }
    async fn open(binary: &Path, source: SourceConfig, operational: bool) -> Result<Self, String> {
        let mut proxy = Command::new(binary)
            .args(["app-server", "proxy"])
            .env("CODEX_HOME", &source.home)
            .current_dir(&source.home)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .map_err(|e| e.to_string())?;
        let stream = join(
            proxy.stdout.take().expect("piped stdout"),
            proxy.stdin.take().expect("piped stdin"),
        );
        let config = WebSocketConfig::default()
            .max_message_size(Some(8 * 1024 * 1024))
            .max_frame_size(Some(8 * 1024 * 1024));
        let (socket, _) = client_async_with_config("ws://localhost/", stream, Some(config))
            .await
            .map_err(|e| e.to_string())?;
        let mut client = Self {
            socket,
            _proxy: Some(proxy),
            sequence: 0,
            initialization: Value::Null,
            source,
        };
        client.initialization = client
            .call(
                "initialize",
                json!({"clientInfo":{"name":if operational {"tracr_release"} else {"tracr_live"}, "version":env!("CARGO_PKG_VERSION")},"capabilities":{"experimentalApi":operational}}),
            )
            .await?;
        client.send(json!({"method":"initialized"})).await?;
        Ok(client)
    }
    async fn send(&mut self, value: Value) -> Result<(), String> {
        self.socket
            .send(Message::Text(value.to_string().into()))
            .await
            .map_err(|e| e.to_string())
    }
    async fn call(&mut self, method: &str, params: Value) -> Result<Value, String> {
        self.sequence += 1;
        let id = self.sequence;
        self.send(json!({"id":id,"method":method,"params":params}))
            .await?;
        while let Some(frame) = self.socket.next().await {
            let frame = frame.map_err(|e| e.to_string())?;
            match frame {
                Message::Text(text) => {
                    let value: Value = serde_json::from_str(&text).map_err(|e| e.to_string())?;
                    if value.get("method").is_some() {
                        if value.get("id").is_some() {
                            return Err(
                                "unexpected server request on passive status connection".into()
                            );
                        }
                        continue;
                    }
                    if value.get("id").and_then(Value::as_u64) != Some(id) {
                        return Err("live status response ID mismatch".into());
                    }
                    if let Some(error) = value.get("error") {
                        return Err(format!("{method}: {error}"));
                    }
                    return value
                        .get("result")
                        .cloned()
                        .ok_or_else(|| "missing live status result".into());
                }
                Message::Ping(_) => self.socket.flush().await.map_err(|e| e.to_string())?,
                Message::Close(_) => break,
                Message::Pong(_) => {}
                _ => return Err("unexpected live status frame".into()),
            }
        }
        Err("live status connection closed".into())
    }
    pub async fn snapshot(&mut self) -> Result<BTreeMap<SessionKey, RuntimeStatus>, String> {
        Ok(self
            .session_snapshot()
            .await?
            .into_iter()
            .map(|(key, session)| (key, session.status))
            .collect())
    }
    /// Read loaded session metadata without loading history or resuming a thread.
    pub async fn session_snapshot(&mut self) -> Result<BTreeMap<SessionKey, LiveSession>, String> {
        timeout(Duration::from_secs(20), self.read_snapshot())
            .await
            .map_err(|_| "live status poll timed out".to_owned())?
    }
    async fn read_snapshot(&mut self) -> Result<BTreeMap<SessionKey, LiveSession>, String> {
        let mut ids = HashSet::new();
        let mut cursors = HashSet::new();
        let mut cursor = None;
        loop {
            let page = self
                .call("thread/loaded/list", json!({"limit":100,"cursor":cursor}))
                .await?;
            for id in page
                .get("data")
                .and_then(Value::as_array)
                .ok_or("invalid loaded thread list")?
            {
                ids.insert(
                    id.as_str()
                        .filter(|id| !id.is_empty())
                        .ok_or("invalid loaded thread ID")?
                        .to_owned(),
                );
            }
            match page.get("nextCursor") {
                None | Some(Value::Null) => break,
                Some(Value::String(next)) if cursors.insert(next.clone()) => {
                    cursor = Some(next.clone())
                }
                _ => return Err("invalid or repeated live status cursor".into()),
            }
        }
        let mut result = BTreeMap::new();
        for id in ids {
            let response = self
                .call("thread/read", json!({"threadId":id,"includeTurns":false}))
                .await?;
            let thread = response.get("thread").ok_or("missing live thread")?;
            if thread.get("id").and_then(Value::as_str) != Some(&id) {
                return Err("live thread identity mismatch".into());
            }
            if super::codex::is_guardian(thread) {
                continue;
            }
            let status = RuntimeStatus::from_codex(thread.get("status"));
            if status == RuntimeStatus::Unloaded {
                continue;
            }
            result.insert(
                SessionKey {
                    provider: ProviderId("codex".into()),
                    source_id: self.source.id.clone(),
                    native_id: id.clone(),
                },
                LiveSession {
                    title: thread
                        .get("name")
                        .and_then(Value::as_str)
                        .filter(|s| !s.trim().is_empty())
                        .or_else(|| {
                            thread
                                .get("preview")
                                .and_then(Value::as_str)
                                .filter(|s| !s.trim().is_empty())
                        })
                        .unwrap_or(&id)
                        .to_owned(),
                    cwd: thread.get("cwd").and_then(Value::as_str).map(PathBuf::from),
                    status,
                },
            );
        }
        Ok(result)
    }
}

/// Separate connection used only by confirmed relocation operations.
pub(crate) struct OperationalClient(LiveClient);
impl OperationalClient {
    pub(crate) fn validate_server(&self, home: &Path, version: &str) -> Result<(), String> {
        let init = &self.0.initialization;
        let agent = init["userAgent"].as_str().unwrap_or_default();
        let reported_version = agent
            .split_whitespace()
            .next()
            .and_then(|s| s.rsplit_once('/'))
            .map(|(_, version)| version);
        if reported_version != Some(version) {
            return Err(format!(
                "Shared Codex daemon must be version {version}; restart the client after upgrading Codex."
            ));
        }
        let server_home = init["codexHome"]
            .as_str()
            .ok_or("Shared daemon did not report its Codex home")?;
        let actual = std::fs::canonicalize(server_home).map_err(|e| e.to_string())?;
        let expected = std::fs::canonicalize(home).map_err(|e| e.to_string())?;
        if actual != expected {
            return Err("Shared daemon belongs to a different Codex home".into());
        }
        Ok(())
    }
    pub(crate) async fn connect(binary: &Path, source: SourceConfig) -> Result<Self, String> {
        timeout(
            Duration::from_secs(5),
            LiveClient::open(binary, source, true),
        )
        .await
        .map_err(|_| "release connection timed out".to_owned())?
        .map(Self)
    }
    pub(crate) async fn call(&mut self, method: &str, params: Value) -> Result<Value, String> {
        self.0.call(method, params).await
    }
}
