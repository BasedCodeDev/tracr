//! Native single-session relinking. Journals contain metadata, never transcript text.
use crate::relocation_profile::Profile;
use crate::{config::SourceConfig, domain::*, providers::codex::Rpc};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    fs,
    io::Write,
    os::{
        fd::AsRawFd,
        unix::fs::{OpenOptionsExt, PermissionsExt},
    },
    path::{Path, PathBuf},
};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Preview {
    pub key: SessionKey,
    pub old: PathBuf,
    pub target: PathBuf,
    pub archived: bool,
    pub created_at: i64,
    pub version: String,
    pub fingerprint: Value,
    #[serde(default)]
    pub profile: Profile,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Journal {
    pub format: u32,
    pub preview: Preview,
    pub phase: String,
    pub settings: Option<Value>,
    #[serde(default)]
    pub permission_validation: Option<PermissionValidation>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failure: Option<Failure>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PermissionValidation {
    pub version: u32,
    pub profile: Profile,
}
pub const STILL_LOADED_DETAIL: &str = "Codex still reports this session as loaded. It may still be releasing, or another client may be attached. Close any client using this session, then retry. If already closed, you can retry now.\n\nThe session's folder has not changed.";

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum FailureKind {
    PermissionProfile,
    StillLoaded,
    Other,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Failure {
    #[serde(skip)]
    pub terminals: Option<crate::providers::codex_terminals::Blocker>,
    pub kind: FailureKind,
    pub detail: String,
}
impl From<String> for Failure {
    fn from(detail: String) -> Self {
        Self {
            kind: FailureKind::Other,
            terminals: None,
            detail,
        }
    }
}
impl From<&str> for Failure {
    fn from(detail: &str) -> Self {
        detail.to_owned().into()
    }
}
impl std::fmt::Display for Failure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.detail.fmt(f)
    }
}
#[derive(Debug)]
pub enum Outcome {
    Verified(Preview),
    Rejected(Failure),
    Unresolved(Failure),
}
fn err(e: impl std::fmt::Display) -> String {
    e.to_string()
}

pub fn destination(input: &str, launch: &Path) -> Result<PathBuf, String> {
    let path = if input == "~" {
        dirs::home_dir().ok_or("Home directory unavailable")?
    } else if let Some(rest) = input.strip_prefix("~/") {
        dirs::home_dir()
            .ok_or("Home directory unavailable")?
            .join(rest)
    } else {
        let p = PathBuf::from(input);
        if p.is_absolute() { p } else { launch.join(p) }
    };
    if input.is_empty() || classify_folder(Some(&path)) != FolderStatus::Available {
        return Err("Choose an existing accessible directory.".into());
    }
    fs::canonicalize(path).map_err(err)
}
fn journal_path(state: &Path, key: &SessionKey) -> PathBuf {
    // Stable bounded filename; journals themselves retain and verify the full identity.
    let hash =
        key.source_id.0.as_bytes().iter().fold(
            0x6c62272e07bb014262b821756295c58du128,
            |hash, byte| {
                (hash ^ u128::from(*byte)).wrapping_mul(0x0000000001000000000000000000013b)
            },
        );
    state.join("relocations").join(format!("{hash:032x}.json"))
}
pub(crate) fn persist<T: Serialize>(path: &Path, journal: &T) -> Result<(), String> {
    let parent = path.parent().ok_or("Invalid journal path")?;
    fs::create_dir_all(parent).map_err(err)?;
    fs::set_permissions(parent, fs::Permissions::from_mode(0o700)).map_err(err)?;
    let tmp = path.with_extension(format!("{}.tmp", std::process::id()));
    let result = (|| {
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&tmp)
            .map_err(err)?;
        file.write_all(&serde_json::to_vec(journal).map_err(err)?)
            .map_err(err)?;
        file.sync_all().map_err(err)?;
        fs::rename(&tmp, path).map_err(err)?;
        fs::File::open(parent)
            .and_then(|f| f.sync_all())
            .map_err(err)
    })();
    let _ = fs::remove_file(tmp);
    result
}
pub fn pending(state: &Path) -> Result<Vec<Journal>, String> {
    let dir = state.join("relocations");
    if !dir.exists() {
        return Ok(vec![]);
    }
    let mut result = vec![];
    for entry in fs::read_dir(dir).map_err(err)? {
        let path = entry.map_err(err)?.path();
        if path.extension().is_some_and(|e| e == "json") {
            let j: Journal = serde_json::from_slice(&fs::read(&path).map_err(err)?).map_err(err)?;
            if j.format != 1 {
                return Err("Unsupported relocation journal format".into());
            }
            if j.phase != "complete" && j.phase != "rejected" {
                result.push(j)
            }
        }
    }
    result.sort_by(|a, b| a.preview.key.cmp(&b.preview.key));
    Ok(result)
}
pub(crate) async fn version(binary: &Path) -> Result<String, String> {
    let output = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        tokio::process::Command::new(binary)
            .arg("--version")
            .output(),
    )
    .await
    .map_err(err)?
    .map_err(err)?;
    let v = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    if !output.status.success() || !matches!(v.as_str(), "codex-cli 0.167.2" | "codex-cli 0.169.0")
    {
        return Err(format!(
            "Relinking is not verified for {v}. Supported: 0.167.2, 0.169.0."
        ));
    }
    Ok(v)
}
pub(crate) async fn listed(rpc: &mut Rpc, id: &str) -> Result<(Value, bool), String> {
    for archived in [false, true] {
        let mut cursor = Value::Null;
        let mut seen = std::collections::BTreeSet::new();
        loop {
            let page=rpc.call("thread/list",json!({"useStateDbOnly":true,"limit":100,"archived":archived,"cursor":cursor,"modelProviders":[],"sourceKinds":[]})).await.map_err(err)?;
            for thread in page["data"]
                .as_array()
                .ok_or("Invalid directory inventory")?
            {
                if thread["id"].as_str() == Some(id) {
                    return Ok((thread.clone(), archived));
                }
            }
            cursor = page["nextCursor"].clone();
            if cursor.is_null() {
                break;
            }
            if !seen.insert(cursor.to_string()) {
                return Err("Repeated directory inventory cursor".into());
            }
        }
    }
    Err("Session no longer appears in native database listing.".into())
}
pub(crate) fn matches_thread(thread: &Value, p: &Preview) -> bool {
    thread["id"].as_str() == Some(&p.key.native_id)
        && thread["createdAt"].as_i64() == Some(p.created_at)
}
pub(crate) fn fingerprint(thread: &Value) -> Value {
    let mut fields = serde_json::Map::new();
    for name in [
        "id",
        "createdAt",
        "updatedAt",
        "model",
        "modelProvider",
        "gitInfo",
        "historyMode",
        "projectId",
    ] {
        fields.insert(name.into(), thread[name].clone());
    }
    Value::Object(fields)
}
pub(crate) fn settings(response: &Value) -> Value {
    let mut fields = serde_json::Map::new();
    for name in [
        "model",
        "modelProvider",
        "approvalPolicy",
        "approvalsReviewer",
        "sandbox",
        "activePermissionProfile",
    ] {
        fields.insert(name.into(), response[name].clone());
    }
    Value::Object(fields)
}
pub(crate) async fn read(rpc: &mut Rpc, id: &str) -> Result<Value, String> {
    let response = rpc
        .call("thread/read", json!({"threadId":id,"includeTurns":false}))
        .await
        .map_err(err)?;
    Ok(response["thread"].clone())
}
pub(crate) async fn recorded_profile(
    source: &SourceConfig,
    thread: &Value,
) -> Result<Profile, String> {
    let path = thread["path"]
        .as_str()
        .ok_or("Codex did not provide a path for permission metadata verification.")?;
    let path = PathBuf::from(path);
    let home = source.home.clone();
    let id = thread["id"]
        .as_str()
        .ok_or("Missing session identity")?
        .to_owned();
    tokio::time::timeout(
        std::time::Duration::from_secs(5),
        tokio::task::spawn_blocking(move || crate::relocation_profile::read(&home, &path, &id)),
    )
    .await
    .map_err(|_| "Timed out reading permission metadata".to_owned())?
    .map_err(err)?
}
fn validate_original(old: &Path) -> Result<(), String> {
    if matches!(
        classify_folder(Some(old)),
        FolderStatus::Available | FolderStatus::Unlinked
    ) {
        Ok(())
    } else {
        Err("The original folder must have a known path and must not be inaccessible.".into())
    }
}
fn validate_distinct(old: &Path, target: &Path) -> Result<(), String> {
    if old == target
        || fs::canonicalize(old)
            .ok()
            .is_some_and(|p| fs::canonicalize(target).ok().as_ref() == Some(&p))
    {
        Err("This session is already in that folder. Choose a different destination.".into())
    } else {
        Ok(())
    }
}
pub async fn preview(
    binary: &Path,
    source: &SourceConfig,
    session: &SessionSummary,
    target: PathBuf,
) -> Result<Preview, Failure> {
    if session.key.provider.0 != "codex" || session.key.source_id != source.id {
        return Err("Wrong session source".into());
    }
    let old = session.cwd.clone().ok_or("Session has no directory")?;
    validate_original(&old)?;
    validate_distinct(&old, &target)?;
    if classify_folder(Some(&target)) != FolderStatus::Available {
        return Err("Destination is unavailable".into());
    }
    let version = version(binary).await?;
    let mut rpc = Rpc::start(binary, &source.home).await.map_err(err)?;
    let result = async {
        let (thread, archived) = listed(&mut rpc, &session.key.native_id).await?;
        let detail = read(&mut rpc, &session.key.native_id).await?;
        if thread["cwd"].as_str() != old.to_str()
            || detail["cwd"].as_str() != old.to_str()
            || archived != session.archived
        {
            return Err("Session changed; refresh before relinking.".into());
        }
        if !matches!(thread["historyMode"].as_str(), Some("legacy" | "paginated")) {
            return Err("This history format is not verified for relinking.".into());
        }
        if thread["canAcceptDirectInput"] == false || crate::providers::codex::is_guardian(&thread)
        {
            return Err("This session cannot be relocated.".into());
        }
        Ok(Preview {
            key: session.key.clone(),
            old,
            target,
            archived,
            created_at: session.created_at,
            version,
            fingerprint: fingerprint(&thread),
            profile: recorded_profile(source, &detail).await?,
        })
    }
    .await;
    rpc.close().await;
    result
}
pub(crate) fn simple_permissions(
    response: &Value,
    cwd: &Path,
    expected: Profile,
) -> Result<(), Failure> {
    let active =
        Profile::from_native(&response["activePermissionProfile"]).map_err(|detail| Failure {
            kind: FailureKind::PermissionProfile,
            terminals: None,
            detail,
        })?;
    if expected == Profile::Unknown || active != expected {
        return Err("The permission profile changed or could not be verified. Refresh and review the session again; profile fallback is not accepted.".into());
    }
    if expected != Profile::Legacy {
        let sandbox = &response["sandbox"];
        let compatible = match expected {
            Profile::Workspace => {
                sandbox["type"] == "workspaceWrite"
                    && sandbox["networkAccess"] == false
                    && sandbox["excludeTmpdirEnvVar"] == false
                    && sandbox["excludeSlashTmp"] == false
            }
            Profile::ReadOnly => sandbox["type"] == "readOnly" && sandbox["networkAccess"] == false,
            _ => false,
        };
        if !compatible {
            return Err(
                "The built-in permission profile has an unexpected sandbox configuration.".into(),
            );
        }
    }
    let roots = response["runtimeWorkspaceRoots"]
        .as_array()
        .ok_or("Cannot verify workspace roots")?;
    if roots.len() != 1 || roots[0].as_str() != cwd.to_str() {
        return Err("Multi-root workspaces are not yet supported for relinking.".into());
    }
    match response["sandbox"]["type"].as_str() {
        Some("readOnly") => Ok(()),
        Some("workspaceWrite")
            if response["sandbox"]["writableRoots"]
                .as_array()
                .is_some_and(|roots| roots.is_empty()) =>
        {
            Ok(())
        }
        _ => Err(
            "Custom permissions or explicit writable roots are not yet verified for relinking."
                .into(),
        ),
    }
}
pub(crate) async fn resume(rpc: &mut Rpc, id: &str) -> Result<Value, String> {
    rpc.call("thread/resume", json!({"threadId":id,"excludeTurns":true}))
        .await
        .map_err(err)
}

pub async fn execute(
    binary: &Path,
    source: &SourceConfig,
    state: &Path,
    p: Preview,
    recovery: bool,
) -> Outcome {
    execute_with_progress(binary, source, state, p, recovery, None).await
}
pub async fn execute_with_progress(
    binary: &Path,
    source: &SourceConfig,
    state: &Path,
    p: Preview,
    recovery: bool,
    progress: Option<tokio::sync::mpsc::UnboundedSender<String>>,
) -> Outcome {
    let result = tokio::time::timeout(
        std::time::Duration::from_secs(120),
        execute_inner(binary, source, state, p.clone(), recovery, progress),
    )
    .await
    .unwrap_or_else(|_| Err("Operation timed out; inspect the journal before retrying".into()));
    match result {
        Ok(()) => Outcome::Verified(p),
        Err(e) => {
            if pending(state).map_or(true, |js| js.iter().any(|j| j.preview.key == p.key)) {
                Outcome::Unresolved(e)
            } else {
                Outcome::Rejected(e)
            }
        }
    }
}
async fn execute_inner(
    binary: &Path,
    source: &SourceConfig,
    state: &Path,
    mut p: Preview,
    recovery: bool,
    progress: Option<tokio::sync::mpsc::UnboundedSender<String>>,
) -> Result<(), Failure> {
    let _coordination = crate::folder_move::lock(state)?;
    if crate::folder_move::pending(state)?.is_some() {
        return Err("Recover the unfinished folder move first.".into());
    }
    if p.key.provider.0 != "codex" || p.key.source_id != source.id {
        return Err("Journal source is not configured".into());
    }
    if version(binary).await? != p.version {
        return Err("Codex version changed; review the operation again".into());
    }
    if classify_folder(Some(&p.target)) != FolderStatus::Available {
        return Err("Destination disappeared or became inaccessible".into());
    }
    let path = journal_path(state, &p.key);
    fs::create_dir_all(path.parent().unwrap()).map_err(err)?;
    let lock = fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .open(path.with_extension("lock"))
        .map_err(err)?;
    // Advisory lock protects against another tracr process; Codex's native writer lock
    // below protects the actual session across all clients.
    if unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
        return Err("Another relocation is running for this source".into());
    }
    if !recovery
        && pending(state)?
            .iter()
            .any(|j| j.preview.key.source_id == p.key.source_id)
    {
        return Err("Recover the unfinished operation for this source first".into());
    }
    if path.exists() {
        let previous: Journal =
            serde_json::from_slice(&fs::read(&path).map_err(err)?).map_err(err)?;
        if previous.preview.key.source_id != p.key.source_id {
            return Err("Journal identity collision; refusing to overwrite".into());
        }
    }
    let mut rpc = Rpc::start(binary, &source.home).await.map_err(err)?;
    let mut j = if recovery {
        let saved: Journal = serde_json::from_slice(&fs::read(&path).map_err(err)?).map_err(err)?;
        if saved.preview.key != p.key
            || saved.preview.target != p.target
            || saved.preview.old != p.old
        {
            return Err("Journal changed; inspect it again".into());
        }
        saved
    } else {
        Journal {
            format: 1,
            preview: p.clone(),
            phase: "prepared".into(),
            settings: None,
            permission_validation: None,
            failure: None,
        }
    };
    if recovery && p.profile == Profile::Unknown {
        if j.settings
            .as_ref()
            .is_some_and(|s| s.get("activePermissionProfile") == Some(&Value::Null))
        {
            p.profile = Profile::Legacy;
            j.preview.profile = Profile::Legacy;
        } else {
            return Err("This older journal has no verified permission profile. Inspect the recorded/native state before attempting another move.".into());
        }
    }
    let mut touched = false;
    let mut changing = false;
    let result: Result<(), Failure> = async {
        let (thread, archived) = listed(&mut rpc, &p.key.native_id).await?;
        let cwd = thread["cwd"].as_str().ok_or("Missing native directory")?;
        if !matches_thread(&thread, &p)
            || (cwd != p.old.to_string_lossy() && (!recovery || cwd != p.target.to_string_lossy()))
        {
            return Err("Conflict: session identity or directory changed".into());
        }
        if !recovery
            && (fingerprint(&thread) != p.fingerprint
                || archived != p.archived)
        {
            return Err("Preview is stale; review again".into());
        }
        if !recovery {
            validate_original(&p.old)?;
            validate_distinct(&p.old, &p.target)?;
        }
        if recovery && archived && !p.archived {
            return Err("Conflict: session was archived after the operation".into());
        }
        if fs::canonicalize(&p.target).map_err(err)? != p.target {
            return Err("Destination changed since preview".into());
        }
        let detail = read(&mut rpc, &p.key.native_id).await?;
        let recorded = recorded_profile(source, &detail).await?;
        if recorded != p.profile || p.profile == Profile::Unknown {
            return Err("Recorded permission profile changed or is unverified; create a fresh relink preview.".into());
        }
        if recovery && p.profile != Profile::Legacy && !j.permission_validation.as_ref().is_some_and(|v| v.version == 1 && v.profile == p.profile) {
            return Err("This journal lacks built-in permission verification. If no move occurred, start a fresh preview; otherwise this operation needs manual recovery review.".into());
        }
        if let Some(tx) = &progress { let _ = tx.send("Releasing idle session…".into()); }
        crate::providers::codex_release::prepare(binary, source, &p, recovery).await?;
        if let Some(tx) = &progress { let _ = tx.send("Acquiring exclusive ownership…".into()); }
        persist(&path, &j)?;
        touched = true;
        if archived {
            rpc.call("thread/unarchive", json!({"threadId":p.key.native_id}))
                .await
                .map_err(err)?;
        }
        j.phase = "acquiring-writer".into();
        persist(&path, &j)?;
        // No cwd override here: competing writers are rejected BEFORE any directory mutation.
        let before = resume(&mut rpc, &p.key.native_id).await?;
        if !matches_thread(&before["thread"], &p) {
            return Err("Writer acquisition returned a different session identity".into());
        }
        let current = before["cwd"].as_str().ok_or("No resolved directory")?;
        if current != p.old.to_string_lossy()
            && (!recovery || current != p.target.to_string_lossy())
        {
            return Err("Conflict: current directory changed".into());
        }
        simple_permissions(&before, Path::new(current), p.profile)?;
        if let Some(expected) = &j.settings {
            if expected != &settings(&before) {
                return Err(
                    "Conflict: permissions or model settings changed since the operation began"
                        .into(),
                );
            }
        } else {
            j.settings = Some(settings(&before));
            j.permission_validation = Some(PermissionValidation { version: 1, profile: p.profile });
            persist(&path, &j)?;
        }
        changing = true;
        j.phase = "changing-directory".into();
        persist(&path, &j)?;
        if current != p.target.to_string_lossy() {
            rpc.call(
                "thread/settings/update",
                json!({"threadId":p.key.native_id,"cwd":p.target}),
            )
            .await
            .map_err(err)?;
        }
        // Settings acknowledgement precedes application; poll the loaded settings barrier.
        let mut applied = false;
        for _ in 0..40 {
            let after = resume(&mut rpc, &p.key.native_id).await?;
            if after["cwd"].as_str() == p.target.to_str() {
                simple_permissions(&after, &p.target, p.profile)?;
                if settings(&before) != settings(&after) {
                    return Err("Permission or model settings changed during relinking; inspect recovery.".into());
                }
                applied = true;
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
        if !applied {
            return Err("Directory update did not settle".into());
        }
        j.phase = "verifying".into();
        persist(&path, &j)?;
        rpc.close().await;
        rpc = Rpc::start(binary, &source.home).await.map_err(err)?;
        let (listed, _) = listed(&mut rpc, &p.key.native_id).await?;
        let detail = read(&mut rpc, &p.key.native_id).await?;
        let resumed = resume(&mut rpc, &p.key.native_id).await?;
        if !matches_thread(&listed, &p)
            || listed["cwd"].as_str() != p.target.to_str()
            || detail["cwd"].as_str() != p.target.to_str()
            || resumed["cwd"].as_str() != p.target.to_str()
        {
            return Err("Native verification disagrees on destination".into());
        }
        simple_permissions(&resumed, &p.target, p.profile)?;
        if j.settings.as_ref() != Some(&settings(&resumed)) {
            return Err(
                "Settings changed after restart; inspect permissions and trust before recovery"
                    .into(),
            );
        }
        Ok(())
    }
    .await;
    rpc.close().await;
    // Restore archive state on success and failure without taking over another writer.
    if p.archived && touched {
        let mut restore = Rpc::start(binary, &source.home).await.map_err(err)?;
        // Never archive a session owned by a competing process.
        match resume(&mut restore, &p.key.native_id).await {
            Ok(current)
                if matches_thread(&current["thread"], &p)
                    && (current["cwd"].as_str() == p.old.to_str()
                        || current["cwd"].as_str() == p.target.to_str()) => {}
            Ok(_) => {
                restore.close().await;
                return Err(
                    "Conflict: refusing to archive a session changed by another client".into(),
                );
            }
            Err(error) => {
                restore.close().await;
                return Err(error.into());
            }
        }
        let restored = restore
            .call("thread/archive", json!({"threadId":p.key.native_id}))
            .await
            .map_err(err);
        restored?;
        let restored_state = listed(&mut restore, &p.key.native_id).await;
        restore.close().await;
        if !restored_state?.1 {
            return Err("Archive restoration was not verified".into());
        }
    }
    if let Err(failure) = &result {
        if touched {
            j.failure = Some(failure.clone());
            if !changing && !recovery {
                j.phase = "rejected".into();
            }
            persist(&path, &j)?;
        }
    }
    result?;
    j.phase = "complete".into();
    j.failure = None;
    persist(&path, &j)?;
    Ok(())
}

#[cfg(test)]
mod permission_tests {
    use super::*;
    #[test]
    fn builtin_permissions_require_matching_single_root_and_canonical_sandbox() {
        let valid = json!({"activePermissionProfile":{"id":":workspace","extends":null},"runtimeWorkspaceRoots":["/old"],"sandbox":{"type":"workspaceWrite","writableRoots":[],"networkAccess":false,"excludeTmpdirEnvVar":false,"excludeSlashTmp":false}});
        assert!(simple_permissions(&valid, Path::new("/old"), Profile::Workspace).is_ok());
        for (key, value) in [
            ("networkAccess", json!(true)),
            ("writableRoots", json!(["/extra"])),
            ("excludeSlashTmp", json!(true)),
        ] {
            let mut changed = valid.clone();
            changed["sandbox"][key] = value;
            assert!(simple_permissions(&changed, Path::new("/old"), Profile::Workspace).is_err());
        }
        let mut changed = valid.clone();
        changed["runtimeWorkspaceRoots"] = json!(["/old", "/extra"]);
        assert!(simple_permissions(&changed, Path::new("/old"), Profile::Workspace).is_err());
        assert!(simple_permissions(&valid, Path::new("/new"), Profile::Workspace).is_err());
        assert!(simple_permissions(&valid, Path::new("/old"), Profile::Unknown).is_err());
    }
}
