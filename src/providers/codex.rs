//! Metadata-only Codex adapter. Never resumes a thread or starts a turn.
use super::{Cursor, ProviderError, SessionPage, SessionProvider};
use crate::{config::SourceConfig, domain::*};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::VecDeque,
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

const SOURCE_KINDS: &[&str] = &[
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
pub(super) fn is_guardian(thread: &Value) -> bool {
    ["/source/subAgent/other", "/source/subagent/other"]
        .iter()
        .any(|pointer| thread.pointer(pointer).and_then(Value::as_str) == Some("guardian"))
}

const DEADLINE: Duration = Duration::from_secs(20);

struct Rpc {
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
    async fn start(binary: &Path, home: &Path) -> Result<Self, ProviderError> {
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
            json!({"clientInfo":{"name":"tracr", "version":env!("CARGO_PKG_VERSION")}}),
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
    async fn call(&mut self, method: &str, params: Value) -> Result<Value, ProviderError> {
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
    async fn close(&mut self) {
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
}
#[derive(Serialize, Deserialize)]
struct Position {
    archived: bool,
    cursor: Option<String>,
}

impl CodexProvider {
    pub async fn connect(binary: &Path, source: SourceConfig) -> Result<Self, ProviderError> {
        let rpc = Rpc::start(binary, &source.home).await?;
        Ok(Self {
            source,
            binary: binary.to_owned(),
            rpc: Mutex::new(rpc),
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
                    "Folder no longer exists: {}. Relinking is not available yet.",
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
            relocation: Capability::Unsupported {
                reason: "Codex 0.167.2 lists the old folder after relocation".into(),
            },
        }
    }
    async fn list(&self, cursor: Option<Cursor>) -> Result<SessionPage, ProviderError> {
        let position = match cursor {
            None => Position {
                archived: false,
                cursor: None,
            },
            Some(c) => serde_json::from_str(&c.0).map_err(|_| ProviderError::InvalidCursor)?,
        };
        let response = self.rpc.lock().await.call("thread/list", json!({"limit":100,"archived":position.archived,"cursor":position.cursor,"modelProviders":[],"sourceKinds":SOURCE_KINDS,"sortKey":"updated_at","sortDirection":"desc"})).await?;
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
                Ok(summary) => sessions.push(summary),
                Err(error) => warnings.push(error.to_string()),
            }
        }
        let next = match response.get("nextCursor") {
            Some(Value::String(cursor)) => Some(Position {
                archived: position.archived,
                cursor: Some(cursor.clone()),
            }),
            None | Some(Value::Null) if !position.archived => Some(Position {
                archived: true,
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
        args: vec!["--cd".into(), cwd.as_os_str().to_owned()],
        cwd: cwd.to_owned(),
        env: vec![("CODEX_HOME".into(), source.home.as_os_str().to_owned())],
    })
}
