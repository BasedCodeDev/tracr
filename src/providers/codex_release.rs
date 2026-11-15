//! Explicit, bounded release for confirmed relocations. Never used by passive monitoring.
use super::{codex::Rpc, codex_live::OperationalClient};
use crate::{config::SourceConfig, domain::RuntimeStatus, relocation as r};
use serde_json::{Value, json};
use std::{collections::HashSet, path::Path, time::Duration};
use tokio::time::{sleep, timeout};

#[derive(Debug, PartialEq, Eq)]
pub enum ReleaseOutcome {
    Released,
    AlreadyUnloaded,
    StillLoaded,
    Busy,
    /// No connection was established; exclusive native writer acquisition is still required.
    Unavailable,
}

pub(crate) async fn prepare(
    binary: &Path,
    source: &SourceConfig,
    preview: &r::Preview,
    recovery: bool,
) -> Result<(), String> {
    match release(binary, source, preview, recovery).await? {
        ReleaseOutcome::Released | ReleaseOutcome::AlreadyUnloaded | ReleaseOutcome::Unavailable => Ok(()),
        ReleaseOutcome::StillLoaded => Err("Codex kept this session loaded. Another client may still be attached. Close it and retry.".into()),
        ReleaseOutcome::Busy => Err("This session is active or its state cannot be verified. Close it before moving.".into()),
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
        release_inner(binary, source, preview, recovery),
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
    p: &r::Preview,
    recovery: bool,
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
    if !r::matches_thread(thread, p) {
        return Err("Release session identity changed".into());
    }
    match RuntimeStatus::from_codex(thread.get("status")) {
        RuntimeStatus::Unloaded => return Ok(ReleaseOutcome::AlreadyUnloaded),
        RuntimeStatus::Idle => (),
        _ => return Ok(ReleaseOutcome::Busy),
    }
    let cwd = thread["cwd"].as_str().ok_or("Missing release directory")?;
    if cwd != p.old.to_string_lossy() && !(recovery && cwd == p.target.to_string_lossy()) {
        return Err("Session directory changed before release".into());
    }
    let mut metadata = Rpc::start(binary, &source.home)
        .await
        .map_err(|e| e.to_string())?;
    let (before, archived) = r::listed(&mut metadata, id).await?;
    if archived || p.archived {
        return Err("Automatic release of loaded archived sessions is not yet verified. Close the session and retry once it is unloaded.".into());
    }
    if !r::matches_thread(&before, p)
        || before["cwd"] != thread["cwd"]
        || (!recovery && r::fingerprint(&before) != p.fingerprint)
    {
        return Err("Session changed before release".into());
    }
    if p.profile == crate::relocation_profile::Profile::Unknown
        || r::recorded_profile(source, thread).await? != p.profile
    {
        return Err("Permission profile changed before release".into());
    }
    let terminals = client
        .call(
            "thread/backgroundTerminals/list",
            json!({"threadId":id,"limit":1}),
        )
        .await
        .map_err(|e| {
            format!("Cannot verify background terminals; automatic release was not attempted: {e}")
        })?;
    if !terminals["data"]
        .as_array()
        .is_some_and(|items| items.is_empty())
        || !terminals["nextCursor"].is_null()
    {
        return Err("This session has background terminals, or their state is unknown. Stop them in Codex before moving.".into());
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
    let validation =
        if !r::matches_thread(&resumed["thread"], p) || resumed["cwd"].as_str() != Some(cwd) {
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
