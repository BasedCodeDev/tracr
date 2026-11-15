//! On-demand terminal inspection and explicitly confirmed single-session cleanup.
use super::{codex::Rpc, codex_live::OperationalClient};
use crate::{
    config::SourceConfig,
    domain::RuntimeStatus,
    relocation::{self as r, Failure, Preview},
};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{collections::HashSet, path::Path, time::Duration};

#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Terminal {
    pub process_id: String,
    pub item_id: String,
    pub command: String,
    pub cwd: String,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Blocker {
    Commands(Vec<Terminal>),
    Unavailable(String),
}
impl Blocker {
    pub fn failure(self) -> Failure {
        let detail = match &self {
            Self::Commands(_) => "The agent is idle, but this session has background commands. Stop them before moving.".to_owned(),
            Self::Unavailable(reason) => format!("Cannot verify background commands: {reason}. Retry before moving."),
        };
        Failure {
            kind: r::FailureKind::Other,
            detail,
            terminals: Some(self),
        }
    }
}

pub(crate) async fn list(
    client: &mut OperationalClient,
    id: &str,
) -> Result<Vec<Terminal>, String> {
    let mut cursor = Value::Null;
    let mut cursors = HashSet::new();
    let mut ids = HashSet::new();
    let mut result = Vec::new();
    loop {
        let page = client
            .call(
                "thread/backgroundTerminals/list",
                json!({"threadId":id,"limit":100,"cursor":cursor}),
            )
            .await?;
        let data = page
            .get("data")
            .and_then(Value::as_array)
            .ok_or("Malformed terminal list")?;
        for item in data {
            let terminal: Terminal =
                serde_json::from_value(item.clone()).map_err(|_| "Malformed terminal entry")?;
            if terminal.process_id.is_empty()
                || terminal.item_id.is_empty()
                || !ids.insert(terminal.process_id.clone())
            {
                return Err("Invalid or repeated terminal identity".into());
            }
            result.push(terminal);
        }
        cursor = page.get("nextCursor").cloned().unwrap_or(Value::Null);
        if cursor.is_null() {
            return Ok(result);
        }
        let next = cursor
            .as_str()
            .filter(|s| !s.is_empty())
            .ok_or("Malformed terminal cursor")?;
        if !cursors.insert(next.to_owned()) {
            return Err("Repeated terminal cursor".into());
        }
    }
}

async fn validate(
    binary: &Path,
    source: &SourceConfig,
    p: &Preview,
    client: &mut OperationalClient,
) -> Result<(), Failure> {
    if p.key.provider.0 != "codex" || p.key.source_id != source.id || p.archived {
        return Err(
            "Terminal recovery requires an unarchived session from the approved source.".into(),
        );
    }
    if r::version(binary).await? != p.version
        || std::fs::canonicalize(&p.target).ok().as_ref() != Some(&p.target)
        || !p.target.is_dir()
        || !matches!(
            crate::domain::classify_folder(Some(&p.old)),
            crate::domain::FolderStatus::Available | crate::domain::FolderStatus::Unlinked
        )
    {
        return Err("Move context changed; create a fresh relink preview.".into());
    }
    let response = client
        .call(
            "thread/read",
            json!({"threadId":p.key.native_id,"includeTurns":false}),
        )
        .await?;
    let live = &response["thread"];
    if !r::matches_thread(live, p)
        || live["cwd"].as_str() != p.old.to_str()
        || RuntimeStatus::from_codex(live.get("status")) != RuntimeStatus::Idle
    {
        return Err("Session changed or is no longer idle; no further commands were stopped. Review the move again.".into());
    }
    let mut rpc = Rpc::start(binary, &source.home)
        .await
        .map_err(|e| e.to_string())?;
    let result = async {
        let (thread, archived) = r::listed(&mut rpc, &p.key.native_id).await?;
        let detail = r::read(&mut rpc, &p.key.native_id).await?;
        if !r::matches_thread(&thread, p)
            || thread["cwd"].as_str() != p.old.to_str()
            || detail["cwd"].as_str() != p.old.to_str()
            || archived != p.archived
            || r::fingerprint(&thread) != p.fingerprint
            || p.profile == crate::relocation_profile::Profile::Unknown
            || r::recorded_profile(source, &detail).await? != p.profile
        {
            return Err("Move context changed; create a fresh relink preview.".into());
        }
        Ok(())
    }
    .await;
    rpc.close().await;
    result
}

pub async fn inspect(
    binary: &Path,
    source: &SourceConfig,
    p: &Preview,
) -> Result<Vec<Terminal>, Failure> {
    tokio::time::timeout(Duration::from_secs(30), async {
        let mut client = OperationalClient::connect(binary, source.clone())
            .await
            .map_err(|e| Blocker::Unavailable(e).failure())?;
        validate(binary, source, p, &mut client).await?;
        list(&mut client, &p.key.native_id)
            .await
            .map_err(|e| Blocker::Unavailable(e).failure())
    })
    .await
    .unwrap_or_else(|_| Err(Blocker::Unavailable("Terminal inspection timed out".into()).failure()))
}

/// Stops only the reviewed identities. Never invokes bulk cleanup or OS signals.
pub async fn stop_reviewed(
    binary: &Path,
    source: &SourceConfig,
    p: &Preview,
    reviewed: &[Terminal],
) -> Result<(), Failure> {
    let outcome = tokio::time::timeout(Duration::from_secs(30), async {
        let mut client = OperationalClient::connect(binary,source.clone()).await.map_err(|e|Blocker::Unavailable(e).failure())?;
        for terminal in reviewed {
            validate(binary,source,p,&mut client).await?;
            let current = list(&mut client,&p.key.native_id).await.map_err(|e|Blocker::Unavailable(e).failure())?;
            if current.iter().any(|item| !reviewed.contains(item)) { return Err(Blocker::Commands(current).failure()); }
            if !current.contains(terminal) { continue; }
            let result = client.call("thread/backgroundTerminals/terminate",json!({"threadId":p.key.native_id,"processId":terminal.process_id})).await;
            let remaining = list(&mut client,&p.key.native_id).await.map_err(|e|Blocker::Unavailable(format!("Some commands may have stopped. {e}")).failure())?;
            if result.as_ref().ok().and_then(|r|r.get("terminated")).and_then(Value::as_bool).is_none() {
                return Err(Blocker::Unavailable("Termination could not be verified; some commands may have stopped. Refresh to inspect the remaining commands.".into()).failure());
            }
            if remaining.iter().any(|item|item.process_id == terminal.process_id) || remaining.iter().any(|item|!reviewed.contains(item)) {
                return Err(Blocker::Commands(remaining).failure());
            }
        }
        validate(binary,source,p,&mut client).await?;
        let remaining = list(&mut client,&p.key.native_id).await.map_err(|e|Blocker::Unavailable(e).failure())?;
        if !remaining.is_empty() { return Err(Blocker::Commands(remaining).failure()); }
        Ok(())
    }).await.unwrap_or_else(|_|Err(Blocker::Unavailable("Stopping commands timed out; some may have stopped. Refresh to inspect them.".into()).failure()));
    outcome.map_err(|mut failure| {
        failure.detail.push_str(" Some commands may already have stopped; the session folder was not changed by cleanup.");
        failure
    })
}
