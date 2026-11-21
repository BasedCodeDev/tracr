//! Scriptable relocation. Live moves update the owning daemon, never acquire a second writer.
use crate::{
    browser::{Browser, Discovery, SourceSpec},
    config::SourceConfig,
    domain::SessionSummary,
    providers::codex_live::{LiveClient, OperationalClient},
    relocation::{self, Outcome},
};
use serde::Serialize;
use serde_json::{Value, json};
use std::{
    path::{Path, PathBuf},
    time::Duration,
};

#[derive(Debug, Serialize)]
pub struct Report {
    pub schema_version: u32,
    pub status: String,
    pub session: Option<SessionSummary>,
    pub destination: PathBuf,
    pub journal: Option<PathBuf>,
    pub error: Option<String>,
}
impl Report {
    pub fn failure(destination: PathBuf, error: String) -> Self {
        Self {
            schema_version: 1,
            status: "rejected".into(),
            session: None,
            destination,
            journal: None,
            error: Some(error),
        }
    }
    pub fn success(&self) -> bool {
        self.error.is_none()
    }
}

pub struct Request {
    pub id: String,
    pub target: PathBuf,
    pub from: Option<PathBuf>,
    pub live: bool,
    pub dry_run: bool,
}

pub async fn run(
    binary: &Path,
    sources: Vec<SourceConfig>,
    state: &Path,
    request: Request,
) -> Report {
    let mut report = Report::failure(request.target.clone(), String::new());
    let result = tokio::time::timeout(
        Duration::from_secs(150),
        perform(binary, sources, state, &request, &mut report),
    )
    .await;
    match result {
        Ok(Ok(())) => report.error = None,
        Ok(Err(error)) => report.error = Some(error),
        Err(_) => report.error = Some("Move timed out. Inspect the reported journal and native session directory before retrying.".into()),
    }
    report
}

async fn select(
    binary: &Path,
    sources: &[SourceConfig],
    id: &str,
) -> Result<SessionSummary, String> {
    let mut browser = Browser::new(
        sources
            .iter()
            .map(|source| SourceSpec::Codex {
                binary: binary.to_owned(),
                source: source.clone(),
            })
            .collect(),
    );
    browser.include_archived = true;
    let mut discovery = Discovery::new();
    discovery.refresh(&mut browser);
    while browser.loading() {
        let update = discovery
            .rx
            .recv()
            .await
            .ok_or("Session discovery stopped")?;
        browser.apply(update);
    }
    let diagnostics = browser.diagnostics();
    let sessions: Vec<_> = browser
        .all()
        .into_iter()
        .filter(|s| s.key.native_id == id)
        .collect();
    discovery.close(&mut browser).await;
    if diagnostics.iter().any(|s| !s.complete) {
        return Err("Incomplete session inventory; cannot select a unique session. Check tracr list --json or explicitly select --codex-home.".into());
    }
    match sessions.as_slice() {
        [session] => Ok(session.clone()),
        [] => Err("Session not found in the configured Codex sources.".into()),
        _ => Err("Session ID is ambiguous across sources; select --codex-home explicitly.".into()),
    }
}

async fn perform(
    binary: &Path,
    sources: Vec<SourceConfig>,
    state: &Path,
    request: &Request,
    report: &mut Report,
) -> Result<(), String> {
    let target = std::fs::canonicalize(&request.target).map_err(|e| format!("Destination: {e}"))?;
    if !target.is_dir() {
        return Err("Destination must be an existing directory.".into());
    }
    // RPC paths are JSON strings: never silently replace non-UTF-8 bytes.
    target.to_str().ok_or("Destination is not UTF-8")?;
    report.destination = target.clone();
    let session = select(binary, &sources, &request.id).await?;
    report.session = Some(session.clone());
    let source = sources
        .iter()
        .find(|s| s.id == session.key.source_id)
        .ok_or("Session source disappeared")?;
    let old = session.cwd.as_deref().ok_or("Session has no directory")?;
    if let Some(from) = &request.from {
        let same = from == old
            || std::fs::canonicalize(from)
                .ok()
                .zip(std::fs::canonicalize(old).ok())
                .is_some_and(|(a, b)| a == b);
        // Repeated invocations may verify an already-applied move.
        if !same && old != target {
            return Err("Session is no longer in --from; refusing to move it.".into());
        }
    }
    if request.live {
        live(binary, source, state, request, report, &session, &target).await
    } else {
        if old == target {
            report.status = "already_at_destination".into();
            return Ok(());
        }
        let preview = relocation::preview(binary, source, &session, target)
            .await
            .map_err(|e| e.to_string())?;
        if request.dry_run {
            report.status = "preview".into();
            return Ok(());
        }
        report.status = "unresolved".into();
        report.journal = Some(relocation::journal_path(state, &session.key));
        match relocation::execute(binary, source, state, preview, false).await {
            Outcome::Verified(_) => {
                report.status = "moved".into();
                Ok(())
            }
            Outcome::Rejected(e) => {
                report.status = "rejected".into();
                report.journal = None;
                Err(e.to_string())
            }
            Outcome::Unresolved(e) => {
                report.status = "unresolved".into();
                Err(e.to_string())
            }
        }
    }
}

async fn loaded(
    binary: &Path,
    source: &SourceConfig,
    session: &SessionSummary,
) -> Result<(), String> {
    let mut observer = LiveClient::connect(binary, source.clone()).await?;
    if !observer.snapshot().await?.contains_key(&session.key) {
        return Err(
            "Live relocation requires a session already loaded by the shared Codex daemon.".into(),
        );
    }
    Ok(())
}

async fn settings(client: &mut OperationalClient, id: &str) -> Result<Value, String> {
    client
        .call(
            "thread/resume",
            json!({"threadId": id, "excludeTurns": true}),
        )
        .await
}

fn invariant_settings(value: &Value) -> Value {
    let mut result = relocation::settings(value);
    for field in [
        "serviceTier",
        "reasoningEffort",
        "collaborationMode",
        "multiAgentMode",
        "disabledPluginIds",
    ] {
        result[field] = value[field].clone();
    }
    result
}

async fn live(
    binary: &Path,
    source: &SourceConfig,
    state: &Path,
    request: &Request,
    report: &mut Report,
    session: &SessionSummary,
    target: &Path,
) -> Result<(), String> {
    if session.archived {
        return Err("Archived sessions cannot be moved live.".into());
    }
    let _lock = crate::folder_move::lock(state)?;
    if crate::folder_move::pending(state)?.is_some() || !relocation::pending(state)?.is_empty() {
        return Err("Recover unfinished folder/session relocations before moving live.".into());
    }
    loaded(binary, source, session).await?;
    let mut client = OperationalClient::connect(binary, source.clone()).await?;
    client.validate_server_home(&source.home)?;
    // Override-free resume attaches to the existing runtime; it starts no model turn.
    let before = settings(&mut client, &request.id).await?;
    let old = session.cwd.as_deref().ok_or("Session has no directory")?;
    if before["thread"]["id"] != request.id
        || before["cwd"].as_str() != old.to_str()
        || before["thread"]["createdAt"].as_i64() != Some(session.created_at)
    {
        return Err("Session identity or directory changed during discovery.".into());
    }
    if !matches!(
        before["thread"]["historyMode"].as_str(),
        Some("legacy" | "paginated")
    ) || before["thread"]["canAcceptDirectInput"] == false
    {
        return Err("Unsupported live session history or input mode.".into());
    }
    let environments = before["thread"]["environments"]
        .as_array()
        .ok_or("Cannot verify live environments")?;
    if environments.len() != 1
        || environments[0]["environmentId"] != "local"
        || environments[0]["cwd"].as_str() != old.to_str()
    {
        return Err("Live relocation supports a single local environment only.".into());
    }
    let profile = relocation::recorded_profile(source, &before["thread"]).await?;
    relocation::simple_permissions(&before, old, profile).map_err(|e| e.to_string())?;
    if old == target {
        report.status = "already_at_destination".into();
        return Ok(());
    }
    if request.dry_run {
        report.status = "preview".into();
        return Ok(());
    }
    if std::fs::canonicalize(target).map_err(|e| e.to_string())? != target {
        return Err("Destination changed during validation.".into());
    }
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|e| e.to_string())?
        .as_nanos();
    let path = state
        .join("live-relocations")
        .join(format!("{stamp}-{}.json", std::process::id()));
    let mut journal = json!({"schema_version":1,"key":session.key,"old":old,"destination":target,"settings":relocation::settings(&before),"phase":"updating"});
    relocation::persist(&path, &journal)?;
    report.journal = Some(path.clone());
    // From this point a transport failure cannot establish that nothing changed.
    report.status = "unresolved".into();
    client
        .call(
            "thread/settings/update",
            json!({"threadId":request.id,"cwd":target}),
        )
        .await
        .map_err(|error| format!("Live directory update failed: {error}"))?;
    for _ in 0..50 {
        let after = settings(&mut client, &request.id).await?;
        if after["thread"]["id"] != request.id
            || after["thread"]["createdAt"].as_i64() != Some(session.created_at)
        {
            return Err("Session identity changed during relocation; inspect the journal.".into());
        }
        if invariant_settings(&after) != invariant_settings(&before) {
            return Err("Settings changed concurrently or during relocation; inspect the journal. No rollback was attempted.".into());
        }
        if after["cwd"].as_str() == target.to_str() {
            relocation::simple_permissions(&after, target, profile).map_err(|e| e.to_string())?;
            let read = client
                .call(
                    "thread/read",
                    json!({"threadId":request.id,"includeTurns":false}),
                )
                .await?;
            let mut rpc = crate::providers::codex::Rpc::start(binary, &source.home)
                .await
                .map_err(|e| e.to_string())?;
            let listed = relocation::listed(&mut rpc, &request.id).await;
            rpc.close().await;
            let (listed, archived) = listed?;
            if read["thread"]["id"] == request.id
                && listed["id"] == request.id
                && read["thread"]["cwd"].as_str() == target.to_str()
                && listed["cwd"].as_str() == target.to_str()
                && read["thread"]["createdAt"].as_i64() == Some(session.created_at)
                && listed["createdAt"].as_i64() == Some(session.created_at)
                && !archived
            {
                journal["phase"] = "complete".into();
                relocation::persist(&path, &journal)?;
                report.status = "moved".into();
                return Ok(());
            }
        } else if after["cwd"].as_str() != old.to_str() {
            return Err(
                "Session moved to a different directory concurrently; inspect the journal.".into(),
            );
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    Err("Native directory verification did not settle; inspect the journal before retrying.".into())
}
