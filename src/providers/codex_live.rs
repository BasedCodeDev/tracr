//! Passive WebSocket client for an existing Codex daemon. Never owns the daemon.
use crate::{
    config::SourceConfig,
    domain::{ProviderId, RuntimeStatus, SessionKey},
};
use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, HashSet},
    path::Path,
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
pub struct LiveClient {
    socket: Socket,
    // kill_on_drop affects only our byte proxy, never the shared daemon.
    _proxy: Child,
    sequence: u64,
    source: SourceConfig,
}
impl LiveClient {
    pub async fn connect(binary: &Path, source: SourceConfig) -> Result<Self, String> {
        timeout(Duration::from_secs(5), Self::open(binary, source))
            .await
            .map_err(|_| "live status connection timed out".to_owned())?
    }
    async fn open(binary: &Path, source: SourceConfig) -> Result<Self, String> {
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
            _proxy: proxy,
            sequence: 0,
            source,
        };
        client
            .call(
                "initialize",
                json!({"clientInfo":{"name":"tracr_live", "version":env!("CARGO_PKG_VERSION")}}),
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
        timeout(Duration::from_secs(20), self.read_snapshot())
            .await
            .map_err(|_| "live status poll timed out".to_owned())?
    }
    async fn read_snapshot(&mut self) -> Result<BTreeMap<SessionKey, RuntimeStatus>, String> {
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
            result.insert(
                SessionKey {
                    provider: ProviderId("codex".into()),
                    source_id: self.source.id.clone(),
                    native_id: id,
                },
                status,
            );
        }
        Ok(result)
    }
}
