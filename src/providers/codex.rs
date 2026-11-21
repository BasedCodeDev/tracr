//! Read-only Codex adapter for metadata and searchable history. Never starts a turn.
use super::{Cursor, ProviderError, SessionPage, SessionProvider};
use crate::{config::SourceConfig, domain::*};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::{BTreeSet, VecDeque},
    path::Path,
    process::Stdio,
    sync::{Arc, Mutex as StdMutex},
    time::Duration,
};
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
    process::{Child, ChildStdin, ChildStdout, Command},
    sync::Mutex,
    task::JoinHandle,
    time::timeout,
};

pub(crate) const SOURCE_KINDS: &[&str] = &[
    "cli",
    "vscode",
    "exec",
    "appServer",
    "subAgent",
    "subAgentReview",
    "subAgentCompact",
    "subAgentThreadSpawn",
    "subAgentOther",
    "unknown",
];
// The API groups Guardian with unrelated "other" subagents. Filter the exact
// metadata marker instead of excluding that entire category or matching titles.
pub(crate) fn is_guardian(thread: &Value) -> bool {
    ["/source/subAgent/other", "/source/subagent/other"]
        .iter()
        .any(|pointer| thread.pointer(pointer).and_then(Value::as_str) == Some("guardian"))
}

const DEADLINE: Duration = Duration::from_secs(20);

pub(crate) struct Rpc {
    child: Child,
    input: Option<ChildStdin>,
    output: BufReader<ChildStdout>,
    sequence: u64,
    stderr: Arc<StdMutex<VecDeque<u8>>>,
    stderr_task: JoinHandle<()>,
    failed: bool,
}
impl Drop for Rpc {
    fn drop(&mut self) {
        self.stderr_task.abort();
        // kill_on_drop also handles cancellation during an in-flight request.
    }
}
impl Rpc {
    pub(crate) async fn start(binary: &Path, home: &Path) -> Result<Self, ProviderError> {
        if !home.is_dir() {
            return Err(ProviderError::Unavailable(format!(
                "Codex home is not an accessible directory: {}",
                home.display()
            )));
        }
        let mut child = Command::new(binary)
            .args(["app-server", "--stdio"])
            .env("CODEX_HOME", home)
            .current_dir(home)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()?;
        let input = child.stdin.take();
        let output = BufReader::new(child.stdout.take().expect("piped stdout"));
        let mut errors = child.stderr.take().expect("piped stderr");
        let stderr = Arc::new(StdMutex::new(VecDeque::new()));
        let captured = stderr.clone();
        let stderr_task = tokio::spawn(async move {
            let mut buffer = [0; 1024];
            while let Ok(n) = errors.read(&mut buffer).await {
                if n == 0 {
                    break;
                }
                if let Ok(mut tail) = captured.lock() {
                    tail.extend(&buffer[..n]);
                    while tail.len() > 8192 {
                        tail.pop_front();
                    }
                }
            }
        });
        let mut rpc = Self {
            child,
            input,
            output,
            sequence: 0,
            stderr,
            stderr_task,
            failed: false,
        };
        rpc.call(
            "initialize",
            json!({"clientInfo":{"name":"tracr", "version":env!("CARGO_PKG_VERSION")},"capabilities":{"experimentalApi":true}}),
        )
        .await?;
        rpc.write(json!({"method":"initialized"})).await?;
        Ok(rpc)
    }
    async fn write(&mut self, value: Value) -> Result<(), ProviderError> {
        let mut bytes =
            serde_json::to_vec(&value).map_err(|e| ProviderError::InvalidData(e.to_string()))?;
        bytes.push(b'\n');
        let input = self
            .input
            .as_mut()
            .ok_or_else(|| ProviderError::Unavailable("server closed".into()))?;
        input.write_all(&bytes).await?;
        input.flush().await?;
        Ok(())
    }
    async fn exchange(&mut self, method: &str, params: Value) -> Result<Value, ProviderError> {
        self.sequence += 1;
        let id = self.sequence;
        self.write(json!({"id":id,"method":method,"params":params}))
            .await?;
        loop {
            let mut line = String::new();
            // Bound individual protocol frames; an oversized record fails the source.
            let n = (&mut self.output)
                .take(8 * 1024 * 1024 + 1)
                .read_line(&mut line)
                .await?;
            if n == 0 {
                return Err(ProviderError::Unavailable("Codex server exited".into()));
            }
            if n > 8 * 1024 * 1024 {
                return Err(ProviderError::InvalidData("oversized RPC frame".into()));
            }
            let message: Value = serde_json::from_str(&line)
                .map_err(|e| ProviderError::InvalidData(e.to_string()))?;
            if message.get("method").is_some() {
                if message.get("id").is_some() {
                    return Err(ProviderError::Unavailable(
                        "unexpected server request during metadata browsing".into(),
                    ));
                }
                continue; // Notifications are not responses.
            }
            if message.get("id").and_then(Value::as_u64) != Some(id) {
                return Err(ProviderError::InvalidData(
                    "RPC response ID mismatch".into(),
                ));
            }
            if let Some(error) = message.get("error") {
                return Err(ProviderError::Unavailable(error.to_string()));
            }
            return message
                .get("result")
                .cloned()
                .ok_or_else(|| ProviderError::InvalidData("RPC response has no result".into()));
        }
    }
    pub(crate) async fn call(
        &mut self,
        method: &str,
        params: Value,
    ) -> Result<Value, ProviderError> {
        if self.failed {
            return Err(ProviderError::Unavailable(
                "server connection failed; refresh to reconnect".into(),
            ));
        }
        let result = timeout(DEADLINE, self.exchange(method, params))
            .await
            .unwrap_or_else(|_| {
                Err(ProviderError::Unavailable(format!(
                    "{method} timed out after 20 seconds"
                )))
            });
        if let Err(error) = result {
            self.failed = true;
            let _ = self.child.start_kill();
            let tail = self
                .stderr
                .lock()
                .map(|s| {
                    String::from_utf8_lossy(&s.iter().copied().collect::<Vec<_>>()).into_owned()
                })
                .unwrap_or_default();
            return Err(ProviderError::Unavailable(format!(
                "{error}{}",
                if tail.is_empty() {
                    String::new()
                } else {
                    format!("; stderr: {tail}")
                }
            )));
        }
        result
    }
    /// An exact native not-found response is positive readback after hard deletion.
    pub(crate) async fn thread_is_absent(&mut self, id: &str) -> Result<bool, ProviderError> {
        let result = timeout(
            DEADLINE,
            self.exchange("thread/read", json!({"threadId":id,"includeTurns":false})),
        )
        .await
        .map_err(|_| ProviderError::Unavailable("Deletion readback timed out".into()))?;
        match result {
            Ok(_) => Ok(false),
            Err(ProviderError::Unavailable(raw))
                if serde_json::from_str::<Value>(&raw).is_ok_and(|packet| {
                    packet["code"].as_i64() == Some(-32600)
                        && packet["message"].as_str() == Some(&format!("thread not loaded: {id}"))
                }) =>
            {
                Ok(true)
            }
            Err(error) => Err(error),
        }
    }

    pub(crate) async fn close(&mut self) {
        self.input.take();
        if timeout(Duration::from_secs(2), self.child.wait())
            .await
            .is_err()
        {
            let _ = self.child.kill().await;
        }
        self.stderr_task.abort();
    }
}

pub struct CodexProvider {
    source: SourceConfig,
    binary: std::path::PathBuf,
    rpc: Mutex<Rpc>,
    listed: StdMutex<BTreeSet<SessionKey>>,
    startup_trace: crate::startup_trace::Trace,
}
#[derive(Serialize, Deserialize)]
struct Position {
    #[serde(default)]
    fallback: bool,
    archived: bool,
    cursor: Option<String>,
}

impl CodexProvider {
    pub async fn connect(binary: &Path, source: SourceConfig) -> Result<Self, ProviderError> {
        Self::connect_with_trace(binary, source, Default::default()).await
    }
    pub(crate) async fn connect_with_trace(
        binary: &Path,
        source: SourceConfig,
        startup_trace: crate::startup_trace::Trace,
    ) -> Result<Self, ProviderError> {
        let initialize = startup_trace.span("codex_initialize");
        let rpc = Rpc::start(binary, &source.home).await;
        initialize.finish(if rpc.is_ok() { "ok" } else { "error" }, None);
        Ok(Self {
            source,
            binary: binary.to_owned(),
            rpc: Mutex::new(rpc?),
            listed: StdMutex::new(BTreeSet::new()),
            startup_trace,
        })
    }
    pub async fn close(&self) {
        self.rpc.lock().await.close().await;
    }
    fn summary(&self, value: Value, archived: bool) -> Result<SessionSummary, ProviderError> {
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct Thread {
            id: String,
            name: Option<String>,
            preview: Option<String>,
            cwd: Option<std::path::PathBuf>,
            created_at: i64,
            updated_at: i64,
            source: Option<Value>,
            git_info: Option<Value>,
            can_accept_direct_input: Option<bool>,
        }
        let thread: Thread =
            serde_json::from_value(value).map_err(|e| ProviderError::InvalidData(e.to_string()))?;
        if thread.id.is_empty() {
            return Err(ProviderError::InvalidData("empty thread ID".into()));
        }
        let title = thread
            .name
            .filter(|s| !s.trim().is_empty())
            .or_else(|| thread.preview.filter(|s| !s.trim().is_empty()))
            .unwrap_or_else(|| thread.id.chars().take(12).collect());
        let git = thread.git_info.map(|v| GitMetadata {
            branch: v["branch"].as_str().map(str::to_owned),
            origin_url: v["originUrl"].as_str().map(str::to_owned),
            commit: v["sha"].as_str().map(str::to_owned),
        });
        Ok(SessionSummary {
            key: SessionKey {
                provider: ProviderId("codex".into()),
                source_id: self.source.id.clone(),
                native_id: thread.id,
            },
            title,
            cwd: thread.cwd,
            created_at: thread.created_at,
            updated_at: thread.updated_at,
            archived,
            can_accept_direct_input: thread.can_accept_direct_input,
            source_kind: thread.source.map(|v| {
                v.as_str()
                    .map(str::to_owned)
                    .unwrap_or_else(|| v.to_string())
            }),
            git,
        })
    }
}
impl SessionProvider for CodexProvider {
    async fn search_content(
        &self,
        key: &SessionKey,
    ) -> Result<Vec<crate::search::Content>, ProviderError> {
        if key.provider.0 != "codex" || key.source_id != self.source.id {
            return Err(ProviderError::InvalidData("Session source mismatch".into()));
        }
        let mut rpc = self.rpc.lock().await;
        let mut cursor: Option<String> = None;
        let mut seen = BTreeSet::new();
        let mut content = Vec::new();
        loop {
            let response = rpc
                .call(
                    "thread/turns/list",
                    json!({
                        "threadId": key.native_id, "itemsView": "full", "limit": 20,
                        "sortDirection": "asc", "cursor": cursor
                    }),
                )
                .await;
            let response = match response {
                Err(error) if cursor.is_none() && error.to_string().contains("-32601") => {
                    // RPC errors retire the connection. Compatibility reads use a fresh one.
                    *rpc = Rpc::start(&self.binary, &self.source.home).await?;
                    let value = rpc
                        .call(
                            "thread/read",
                            json!({"threadId":key.native_id,"includeTurns":true}),
                        )
                        .await?;
                    if value["thread"]["id"].as_str() != Some(&key.native_id) {
                        return Err(ProviderError::InvalidData(
                            "Session identity mismatch".into(),
                        ));
                    }
                    return crate::search::extract_turns(&value["thread"]["turns"]);
                }
                other => other?,
            };
            content.extend(crate::search::extract_turns(&response["data"])?);
            cursor = match response.get("nextCursor") {
                None | Some(Value::Null) => return Ok(content),
                Some(Value::String(next)) if seen.insert(next.clone()) => Some(next.clone()),
                _ => {
                    return Err(ProviderError::InvalidData(
                        "Invalid or repeated history cursor".into(),
                    ));
                }
            };
        }
    }
    async fn prepare_resume(
        &self,
        session: &SessionSummary,
    ) -> Result<ResumeCommand, ProviderError> {
        if session.key.provider.0 != "codex" || session.key.source_id != self.source.id {
            return Err(ProviderError::Unavailable(
                "Session belongs to a different Codex source.".into(),
            ));
        }
        if let Some(reason) = resume_blocker(session) {
            return Err(ProviderError::Unavailable(reason.into()));
        }
        let fresh = self.read(&session.key).await?.ok_or_else(|| {
            ProviderError::Unavailable(
                "Session is no longer available; refresh the inventory.".into(),
            )
        })?;
        if fresh.key != session.key {
            return Err(ProviderError::InvalidData(
                "Codex returned a different session identity.".into(),
            ));
        }
        if let Some(reason) = resume_blocker(&fresh) {
            return Err(ProviderError::Unavailable(reason.into()));
        }
        let cwd = fresh
            .cwd
            .as_ref()
            .filter(|p| p.is_absolute())
            .cloned()
            .ok_or_else(|| {
                ProviderError::Unavailable(
                    "No usable local working directory is recorded for this session.".into(),
                )
            })?;
        let path = cwd.clone();
        let status = tokio::task::spawn_blocking(move || classify_folder(Some(&path)))
            .await
            .map_err(|e| ProviderError::Unavailable(e.to_string()))?;
        match status {
            FolderStatus::Available => {}
            FolderStatus::Unlinked => {
                return Err(ProviderError::Unavailable(format!(
                    "Folder no longer exists: {}. Select Move session (m).",
                    cwd.display()
                )));
            }
            FolderStatus::Inaccessible { reason } => {
                return Err(ProviderError::Unavailable(format!(
                    "Cannot access {}: {reason}",
                    cwd.display()
                )));
            }
            FolderStatus::Unknown => {
                return Err(ProviderError::Unavailable(
                    "Unknown working directory.".into(),
                ));
            }
        }
        Ok(ResumeCommand {
            launch: LaunchCommand {
                executable: self.binary.clone(),
                args: vec![
                    "resume".into(),
                    "--cd".into(),
                    cwd.clone().into_os_string(),
                    "--".into(),
                    fresh.key.native_id.clone().into(),
                ],
                cwd,
                env: vec![(
                    "CODEX_HOME".into(),
                    self.source.home.clone().into_os_string(),
                )],
            },
            session: fresh,
        })
    }

    fn capabilities(&self) -> ProviderCapabilities {
        ProviderCapabilities {
            list: Capability::Supported,
            read: Capability::Supported,
            resume: Capability::Supported,
            // Each preview verifies version and each execution verifies exclusive access.
            relocation: Capability::Supported,
        }
    }
    async fn list(&self, cursor: Option<Cursor>) -> Result<SessionPage, ProviderError> {
        let position = match cursor {
            None => {
                self.listed.lock().expect("listed lock").clear();
                Position {
                    fallback: false,
                    archived: false,
                    cursor: None,
                }
            }
            Some(c) => serde_json::from_str(&c.0).map_err(|_| ProviderError::InvalidCursor)?,
        };
        let request = self
            .startup_trace
            .span(match (position.fallback, position.archived) {
                (false, false) => "codex_database_list",
                (false, true) => "codex_database_list_archived",
                (true, false) => "codex_reconciled_list",
                (true, true) => "codex_reconciled_list_archived",
            });
        let response = self.rpc.lock().await.call("thread/list", json!({"limit":100,"archived":position.archived,"cursor":position.cursor,"modelProviders":[],"sourceKinds":SOURCE_KINDS,"sortKey":"updated_at","sortDirection":"desc","useStateDbOnly":!position.fallback})).await;
        let rows = response
            .as_ref()
            .ok()
            .and_then(|value| value["data"].as_array())
            .map(Vec::len);
        request.finish(if rows.is_some() { "ok" } else { "error" }, rows);
        let response = match response {
            Ok(response) => response,
            Err(error)
                if !position.fallback
                    && ["-32601", "-32602", "-32603"]
                        .iter()
                        .any(|code| error.to_string().contains(code)) =>
            {
                let reconnect = self.startup_trace.span("codex_reconnect");
                let rpc = Rpc::start(&self.binary, &self.source.home).await;
                reconnect.finish(if rpc.is_ok() { "ok" } else { "error" }, None);
                let rpc = rpc?;
                *self.rpc.lock().await = rpc;
                return Ok(SessionPage {
                    sessions: Vec::new(),
                    warnings: vec![format!(
                        "Database listing unavailable; using reconciled discovery: {error}"
                    )],
                    next: Some(Cursor(
                        serde_json::to_string(&Position {
                            fallback: true,
                            archived: false,
                            cursor: None,
                        })
                        .expect("position"),
                    )),
                });
            }
            Err(error) => return Err(error),
        };
        let data = response["data"]
            .as_array()
            .ok_or_else(|| ProviderError::InvalidData("thread/list missing data array".into()))?;
        let mut sessions = Vec::new();
        let mut warnings = Vec::new();
        for value in data {
            if is_guardian(value) {
                continue;
            }
            match self.summary(value.clone(), position.archived) {
                Ok(mut summary) => {
                    if self
                        .listed
                        .lock()
                        .expect("listed lock")
                        .contains(&summary.key)
                    {
                        continue;
                    }
                    if position.fallback {
                        let read = self.startup_trace.span("codex_reconcile_read");
                        let result = self.read(&summary.key).await;
                        read.finish(
                            match &result {
                                Ok(Some(_)) => "ok",
                                Ok(None) => "missing",
                                Err(_) => "error",
                            },
                            None,
                        );
                        match result {
                            Ok(Some(mut fresh)) => {
                                fresh.archived = position.archived;
                                summary = fresh;
                            }
                            Ok(None) => continue,
                            Err(error) => {
                                warnings.push(format!(
                                    "Could not reconcile {}: {error}",
                                    summary.key.native_id
                                ));
                                continue;
                            }
                        }
                    }
                    self.listed
                        .lock()
                        .expect("listed lock")
                        .insert(summary.key.clone());
                    sessions.push(summary);
                }
                Err(error) => warnings.push(error.to_string()),
            }
        }
        let next = match response.get("nextCursor") {
            Some(Value::String(cursor)) => Some(Position {
                fallback: position.fallback,
                archived: position.archived,
                cursor: Some(cursor.clone()),
            }),
            None | Some(Value::Null) if !position.archived => Some(Position {
                fallback: position.fallback,
                archived: true,
                cursor: None,
            }),
            None | Some(Value::Null) if !position.fallback => Some(Position {
                fallback: true,
                archived: false,
                cursor: None,
            }),
            None | Some(Value::Null) => None,
            _ => return Err(ProviderError::InvalidData("invalid nextCursor".into())),
        }
        .map(|p| Cursor(serde_json::to_string(&p).expect("serializable position")));
        Ok(SessionPage {
            sessions,
            next,
            warnings,
        })
    }
    async fn read(&self, key: &SessionKey) -> Result<Option<SessionSummary>, ProviderError> {
        if key.source_id != self.source.id || key.provider.0 != "codex" {
            return Ok(None);
        }
        let response = self
            .rpc
            .lock()
            .await
            .call(
                "thread/read",
                json!({"threadId":key.native_id,"includeTurns":false}),
            )
            .await?;
        if is_guardian(&response["thread"]) {
            return Ok(None);
        }
        self.summary(response["thread"].clone(), false).map(Some)
    }
}

/// Prepare an interactive session without creating or modifying stored records.
pub fn prepare_new(
    binary: &std::path::Path,
    source: &crate::config::SourceConfig,
    cwd: &std::path::Path,
) -> Result<LaunchCommand, ProviderError> {
    if !cwd.is_absolute() || classify_folder(Some(cwd)) != FolderStatus::Available {
        return Err(ProviderError::Unavailable(format!(
            "Cannot create a session: folder is missing, inaccessible, or not a directory: {}",
            cwd.display()
        )));
    }
    Ok(LaunchCommand {
        executable: binary.to_owned(),
        args: vec![
            "--cd".into(),
            cwd.as_os_str().to_owned(),
            "--approve-for-me".into(),
        ],
        cwd: cwd.to_owned(),
        env: vec![("CODEX_HOME".into(), source.home.as_os_str().to_owned())],
    })
}
