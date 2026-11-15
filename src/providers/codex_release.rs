//! Explicit, bounded release for confirmed relocations. Never used by passive monitoring.
use super::{codex::Rpc, codex_live::OperationalClient};
use crate::{config::SourceConfig, domain::RuntimeStatus, relocation as r};
use serde_json::{Value, json};
use std::{collections::HashSet, path::Path, time::Duration};
use tokio::time::{sleep, timeout};

/// Native release identity; independent of any proposed destination.
struct ReleaseContext {
    key: crate::domain::SessionKey,
    old: std::path::PathBuf,
    recovery_target: Option<std::path::PathBuf>,
    archived: bool,
    created_at: i64,
    version: String,
    fingerprint: Value,
    profile: crate::relocation_profile::Profile,
}
impl ReleaseContext {
    fn matches(&self, thread: &Value) -> bool {
        thread["id"].as_str() == Some(&self.key.native_id)
            && thread["createdAt"].as_i64() == Some(self.created_at)
    }
}

/// Explicit move-dialog operation. No destination, journal, or filesystem mutation.
pub(crate) async fn release_idle(
    binary: &Path,
    source: &SourceConfig,
    session: &crate::domain::SessionSummary,
) -> Result<ReleaseOutcome, String> {
    timeout(Duration::from_secs(30), async {
        let mut rpc = Rpc::start(binary, &source.home)
            .await
            .map_err(|e| e.to_string())?;
        let (thread, archived) = r::listed(&mut rpc, &session.key.native_id).await?;
        let old = session.cwd.clone().ok_or("Missing session directory")?;
        if thread["cwd"].as_str() != old.to_str()
            || thread["createdAt"].as_i64() != Some(session.created_at)
        {
            return Err("Session changed before release".into());
        }
        let detail = r::read(&mut rpc, &session.key.native_id).await?;
        let context = ReleaseContext {
            key: session.key.clone(),
            old,
            recovery_target: None,
            archived,
            created_at: session.created_at,
            version: r::version(binary).await?,
            fingerprint: r::fingerprint(&thread),
            profile: r::recorded_profile(source, &detail).await?,
        };
        rpc.close().await;
        release_inner(binary, source, &context).await
    })
    .await
    .map_err(|_| "Idle release check timed out".to_owned())?
}

#[derive(Debug, PartialEq, Eq)]
pub enum ReleaseOutcome {
    Released,
    AlreadyUnloaded,
    StillLoaded,
    Busy,
    Terminals(super::codex_terminals::Blocker),
    /// No connection was established; exclusive native writer acquisition is still required.
    Unavailable,
}

pub(crate) async fn prepare(
    binary: &Path,
    source: &SourceConfig,
    preview: &r::Preview,
    recovery: bool,
) -> Result<(), r::Failure> {
    match release(binary, source, preview, recovery).await? {
        ReleaseOutcome::Released
        | ReleaseOutcome::AlreadyUnloaded
        | ReleaseOutcome::Unavailable => Ok(()),
        ReleaseOutcome::StillLoaded => Err(r::Failure {
            kind: r::FailureKind::StillLoaded,
            detail: r::STILL_LOADED_DETAIL.into(),
            terminals: None,
        }),
        ReleaseOutcome::Terminals(blocker) => Err(blocker.failure()),
        ReleaseOutcome::Busy => Err(
            "This session is active or its state cannot be verified. Close it before moving."
                .into(),
        ),
    }
}

pub async fn release(
    binary: &Path,
    source: &SourceConfig,
    preview: &r::Preview,
    recovery: bool,
) -> Result<ReleaseOutcome, String> {
    timeout(
        Duration::from_secs(30),
        release_inner(
            binary,
            source,
            &ReleaseContext {
                key: preview.key.clone(),
                old: preview.old.clone(),
                recovery_target: recovery.then(|| preview.target.clone()),
                archived: preview.archived,
                created_at: preview.created_at,
                version: preview.version.clone(),
                fingerprint: preview.fingerprint.clone(),
                profile: preview.profile,
            },
        ),
    )
    .await
    .map_err(|_| {
        "Timed out releasing the idle session; exclusive ownership was not acquired.".to_owned()
    })?
}

async fn loaded(client: &mut OperationalClient, id: &str) -> Result<bool, String> {
    let mut cursor = Value::Null;
    let mut seen = HashSet::new();
    loop {
        let page = client
            .call("thread/loaded/list", json!({"limit":100,"cursor":cursor}))
            .await?;
        let ids = page["data"]
            .as_array()
            .ok_or("Invalid loaded session list")?;
        for value in ids {
            let value = value.as_str().ok_or("Invalid loaded session ID")?;
            if value == id {
                return Ok(true);
            }
        }
        cursor = page["nextCursor"].clone();
        if cursor.is_null() {
            return Ok(false);
        }
        if !cursor.is_string() || !seen.insert(cursor.to_string()) {
            return Err("Invalid or repeated loaded session cursor".into());
        }
    }
}

async fn release_inner(
    binary: &Path,
    source: &SourceConfig,
    p: &ReleaseContext,
) -> Result<ReleaseOutcome, String> {
    if p.key.source_id != source.id || p.key.provider.0 != "codex" {
        return Err("Release source does not match the session".into());
    }
    let mut client = match OperationalClient::connect(binary, source.clone()).await {
        Ok(client) => client,
        Err(_) => return Ok(ReleaseOutcome::Unavailable),
    };
    let id = &p.key.native_id;
    if !loaded(&mut client, id).await? {
        return Ok(ReleaseOutcome::AlreadyUnloaded);
    }
    if r::version(binary).await? != p.version {
        return Err("Codex version changed".into());
    }
    let live = client
        .call("thread/read", json!({"threadId":id,"includeTurns":false}))
        .await?;
    let thread = &live["thread"];
    if !p.matches(thread) {
        return Err("Release session identity changed".into());
    }
    match RuntimeStatus::from_codex(thread.get("status")) {
        RuntimeStatus::Unloaded => return Ok(ReleaseOutcome::AlreadyUnloaded),
        RuntimeStatus::Idle => (),
        _ => return Ok(ReleaseOutcome::Busy),
    }
    let cwd = thread["cwd"].as_str().ok_or("Missing release directory")?;
    if cwd != p.old.to_string_lossy()
        && !p
            .recovery_target
            .as_ref()
            .is_some_and(|path| cwd == path.to_string_lossy())
    {
        return Err("Session directory changed before release".into());
    }
    let mut metadata = Rpc::start(binary, &source.home)
        .await
        .map_err(|e| e.to_string())?;
    let (before, archived) = r::listed(&mut metadata, id).await?;
    if archived || p.archived {
        return Err("Automatic release of loaded archived sessions is not yet verified. Close the session and retry once it is unloaded.".into());
    }
    if !p.matches(&before)
        || before["cwd"] != thread["cwd"]
        || (p.recovery_target.is_none() && r::fingerprint(&before) != p.fingerprint)
    {
        return Err("Session changed before release".into());
    }
    if p.profile == crate::relocation_profile::Profile::Unknown
        || r::recorded_profile(source, thread).await? != p.profile
    {
        return Err("Permission profile changed before release".into());
    }
    match super::codex_terminals::list(&mut client, id).await {
        Ok(terminals) if terminals.is_empty() => {}
        Ok(terminals) => {
            return Ok(ReleaseOutcome::Terminals(
                super::codex_terminals::Blocker::Commands(terminals),
            ));
        }
        Err(error) => {
            return Ok(ReleaseOutcome::Terminals(
                super::codex_terminals::Blocker::Unavailable(error),
            ));
        }
    }
    // Codex replaces only a detached idle runtime. An attached or newly active runtime
    // ignores these overrides. Disconnecting this client never disconnects other clients.
    let resumed = client
        .call(
            "thread/resume",
            json!({"threadId":id,"excludeTurns":true,"config":{"thread_unload_delay_secs":0}}),
        )
        .await?;
    // Unsubscribe even if validation fails; dropping the connection also handles cancellation.
    let validation = if !p.matches(&resumed["thread"]) || resumed["cwd"].as_str() != Some(cwd) {
        Err("Session changed during release".to_owned())
    } else {
        r::simple_permissions(&resumed, Path::new(cwd), p.profile).map_err(|e| e.to_string())
    };
    client
        .call("thread/unsubscribe", json!({"threadId":id}))
        .await?;
    validation?;
    let unloaded = timeout(Duration::from_secs(2), async {
        loop {
            if !loaded(&mut client, id).await? {
                return Ok::<(), String>(());
            }
            sleep(Duration::from_millis(50)).await;
        }
    })
    .await;
    match unloaded {
        Ok(result) => result?,
        Err(_) => return Ok(ReleaseOutcome::StillLoaded),
    }
    let (after, after_archived) = r::listed(&mut metadata, id).await?;
    let detail = r::read(&mut metadata, id).await?;
    if after_archived != archived
        || r::fingerprint(&before) != r::fingerprint(&after)
        || after["cwd"] != before["cwd"]
        || r::recorded_profile(source, &detail).await? != p.profile
    {
        return Err("Session metadata changed during release; refresh and review again.".into());
    }
    metadata.close().await;
    Ok(ReleaseOutcome::Released)
}
