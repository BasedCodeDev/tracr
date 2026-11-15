//! Recoverable directory rename plus native Codex session updates.
use crate::{config::SourceConfig, domain::*, providers::codex::Rpc, relocation as r};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    os::{
        fd::AsRawFd,
        unix::{
            ffi::OsStrExt,
            fs::{MetadataExt, OpenOptionsExt},
        },
    },
    path::{Path, PathBuf},
};
use tokio::sync::mpsc::UnboundedSender;

type Result<T> = std::result::Result<T, String>;
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Source {
    pub binary: PathBuf,
    pub home: PathBuf,
    pub id: SourceId,
}
impl Source {
    fn config(&self) -> SourceConfig {
        SourceConfig {
            id: self.id.clone(),
            home: self.home.clone(),
        }
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Identity {
    dev: u64,
    ino: u64,
}
fn identity(path: &Path) -> Result<Identity> {
    let meta = fs::symlink_metadata(path).map_err(|e| format!("{}: {e}", path.display()))?;
    if !meta.is_dir() {
        return Err(format!(
            "{} is not a directory (symlink aliases are not supported).",
            path.display()
        ));
    }
    Ok(Identity {
        dev: meta.dev(),
        ino: meta.ino(),
    })
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Entry {
    pub session: r::Preview,
    pub title: String,
    pub directory: Identity,
    pub settings: Option<Value>,
    pub archive_touched: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Plan {
    pub old: PathBuf,
    pub target: PathBuf,
    pub directory: Identity,
    pub parent: Identity,
    pub sources: Vec<Source>,
    pub entries: Vec<Entry>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Journal {
    pub format: u32,
    pub plan: Plan,
    pub phase: String,
    pub error: Option<String>,
}
#[derive(Debug)]
pub enum Outcome {
    Complete(Plan),
    Failed { message: String, recovery: bool },
}

fn journal_path(state: &Path) -> PathBuf {
    state.join("folder-moves").join("current.json")
}
pub fn pending(state: &Path) -> Result<Option<Journal>> {
    let path = journal_path(state);
    let data = match fs::read(path) {
        Ok(data) => data,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e.to_string()),
    };
    let j: Journal = serde_json::from_slice(&data).map_err(|e| e.to_string())?;
    if j.format != 1 {
        return Err("Unsupported folder-move journal format.".into());
    }
    Ok((j.phase != "complete" && j.phase != "rejected").then_some(j))
}
pub(crate) struct CoordinationLock(fs::File);
impl Drop for CoordinationLock {
    fn drop(&mut self) {
        // A concurrently spawned child can briefly inherit an open file
        // description before exec closes it. Release the flock explicitly,
        // rather than depending on the last inherited descriptor closing.
        unsafe {
            libc::flock(self.0.as_raw_fd(), libc::LOCK_UN);
        }
    }
}
pub(crate) fn lock(state: &Path) -> Result<CoordinationLock> {
    fs::create_dir_all(state).map_err(|e| e.to_string())?;
    let file = fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .open(state.join("relocation.lock"))
        .map_err(|e| e.to_string())?;
    if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
        return Err("Another relocation is running. Wait for it to finish.".into());
    }
    Ok(CoordinationLock(file))
}
fn save(state: &Path, j: &Journal) -> Result<()> {
    r::persist(&journal_path(state), j)
}
fn absent(path: &Path) -> Result<()> {
    match fs::symlink_metadata(path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Ok(_) => Err(format!(
            "Destination already exists: {}. Choose another name.",
            path.display()
        )),
        Err(e) => Err(e.to_string()),
    }
}
// Resolve existing ancestors without creating the state directory.
fn resolved(path: &Path) -> Result<PathBuf> {
    if path.exists() {
        return fs::canonicalize(path).map_err(|e| e.to_string());
    }
    let parent = path.parent().ok_or("Cannot resolve protected path")?;
    Ok(resolved(parent)?.join(path.file_name().ok_or("Invalid protected path")?))
}
/// Resolve the final path, preserving a trailing slash as an explicit nesting request.
pub fn resolve_destination(input: &str, old: &Path, launch: &Path) -> Result<PathBuf> {
    if input.is_empty() {
        return Err("Enter a destination folder.".into());
    }
    let raw = if input == "~" {
        dirs::home_dir().ok_or("Home directory unavailable")?
    } else if let Some(rest) = input.strip_prefix("~/") {
        dirs::home_dir()
            .ok_or("Home directory unavailable")?
            .join(rest)
    } else {
        let path = PathBuf::from(input);
        if path.is_absolute() {
            path
        } else {
            launch.join(path)
        }
    };
    let target = if input.ends_with('/') {
        raw.join(old.file_name().ok_or("Cannot move a filesystem root")?)
    } else {
        raw
    };
    if matches!(input.rsplit('/').next(), Some(".") | Some("..")) {
        return Err("Destination must include a new folder name.".into());
    }
    let parent = target
        .parent()
        .ok_or("Destination must have a parent folder")?;
    let parent =
        fs::canonicalize(parent).map_err(|e| format!("Destination parent is unavailable: {e}"))?;
    identity(&parent)?;
    Ok(parent.join(
        target
            .file_name()
            .ok_or("Destination must include a folder name")?,
    ))
}

pub fn destination(parent: &Path, name: &str) -> Result<PathBuf> {
    if name.is_empty() || name == "." || name == ".." || name.contains('/') || name.contains('\0') {
        return Err("Folder name must be one nonempty name, without slashes.".into());
    }
    let parent = fs::canonicalize(parent).map_err(|e| e.to_string())?;
    identity(&parent)?;
    let target = parent.join(name);
    absent(&target)?;
    Ok(target)
}
fn validate_paths(
    old: &Path,
    target: &Path,
    state: &Path,
    sources: &[Source],
) -> Result<(Identity, Identity)> {
    let dir = identity(old)?;
    fs::read_dir(old).map_err(|e| format!("Source directory is inaccessible: {e}"))?;
    if fs::canonicalize(old).map_err(|e| e.to_string())? != old || old.parent().is_none() {
        return Err("Choose a concrete folder, not a filesystem root or symlink alias.".into());
    }
    let parent = target.parent().ok_or("Invalid destination")?;
    if !target.is_absolute() || fs::canonicalize(parent).map_err(|e| e.to_string())? != parent {
        return Err("Destination parent changed or is a symlink alias.".into());
    }
    if target.starts_with(old) {
        return Err("A folder cannot move into itself.".into());
    }
    absent(target)?;
    let dest = identity(parent)?;
    if dir.dev != dest.dev {
        return Err("Moving between filesystems is not supported yet. Choose a folder on the same filesystem.".into());
    }
    if identity(old.parent().ok_or("Cannot move a filesystem root")?)?.dev != dir.dev {
        return Err("Cannot move a filesystem mount root.".into());
    }
    for protected in std::iter::once(state).chain(sources.iter().map(|s| s.home.as_path())) {
        if resolved(protected)?.starts_with(old) {
            return Err(format!(
                "Cannot move a folder containing Codex storage or tracr recovery state: {}",
                protected.display()
            ));
        }
    }
    Ok((dir, dest))
}
async fn inventory(sources: &[Source]) -> Result<BTreeMap<SessionKey, (Value, bool)>> {
    let mut all = BTreeMap::new();
    for source in sources {
        let mut rpc = Rpc::start(&source.binary, &source.home)
            .await
            .map_err(|e| e.to_string())?;
        let result:Result<()> = async {
            for archived in [false,true] {
                let mut cursor=Value::Null;
                let mut seen=BTreeSet::new();
                loop {
                    let page=rpc.call("thread/list",json!({"useStateDbOnly":true,"limit":100,"archived":archived,"cursor":cursor,"modelProviders":[],"sourceKinds":[]})).await.map_err(|e|e.to_string())?;
                    for thread in page["data"].as_array().ok_or("Incomplete session inventory")? {
                        let id=thread["id"].as_str().ok_or("Session inventory has no ID")?;
                        let cwd=thread["cwd"].as_str().ok_or("Session inventory has no folder")?;
                        if !Path::new(cwd).is_absolute() {return Err("Session inventory has a relative folder".into());}
                        let key=SessionKey{provider:ProviderId("codex".into()),source_id:source.id.clone(),native_id:id.into()};
                        if all.insert(key,(thread.clone(),archived)).is_some() {return Err("Duplicate session in inventory".into());}
                    }
                    cursor=page.get("nextCursor").ok_or("Incomplete session pagination")?.clone();
                    if cursor.is_null(){break;}
                    if !seen.insert(cursor.to_string()){return Err("Repeated inventory cursor".into());}
                }
            }
            Ok(())
        }.await;
        rpc.close().await;
        result.map_err(|e| format!("{}: {e}", source.home.display()))?;
    }
    Ok(all)
}
fn affected(thread: &Value, old: &Path) -> bool {
    thread["cwd"]
        .as_str()
        .is_some_and(|p| Path::new(p).starts_with(old))
}
pub async fn preview(
    sources: Vec<Source>,
    old: PathBuf,
    target: PathBuf,
    state: &Path,
) -> Result<Plan> {
    let (directory, parent) = validate_paths(&old, &target, state, &sources)?;
    if pending(state)?.is_some() || !r::pending(state)?.is_empty() {
        return Err("Recover unfinished relocations before moving a folder.".into());
    }
    let records = inventory(&sources).await?;
    let mut entries = vec![];
    let mut blockers = vec![];
    for source in &sources {
        let version = r::version(&source.binary).await?;
        // Live telemetry is advisory; native writer acquisition remains authoritative.
        let live = match crate::providers::codex_live::LiveClient::connect(
            &source.binary,
            source.config(),
        )
        .await
        {
            Ok(mut client) => client.snapshot().await.ok(),
            Err(_) => None,
        };
        let mut rpc = Rpc::start(&source.binary, &source.home)
            .await
            .map_err(|e| e.to_string())?;
        for (key, (thread, archived)) in &records {
            if key.source_id != source.id || !affected(thread, &old) {
                continue;
            }
            let result: Result<Entry> = async {
                if live.as_ref().is_some_and(|m| m.get(key).is_some_and(|s| *s != crate::domain::RuntimeStatus::Idle)) {
                    return Err("Session is active or its loaded state is uncertain; close it before moving.".into());
                }
                if !matches!(thread["historyMode"].as_str(), Some("legacy" | "paginated"))
                    || thread["canAcceptDirectInput"] == false
                    || crate::providers::codex::is_guardian(thread)
                {
                    return Err("This session type cannot be moved.".into());
                }
                let cwd = PathBuf::from(thread["cwd"].as_str().ok_or("Missing folder")?);
                let directory = identity(&cwd)?;
                fs::read_dir(&cwd)
                    .map_err(|e| format!("Session directory is inaccessible: {e}"))?;
                if fs::canonicalize(&cwd).map_err(|e| e.to_string())? != cwd {
                    return Err(
                        "Session folder uses a symlink alias; relink it separately first.".into(),
                    );
                }
                let detail = r::read(&mut rpc, &key.native_id).await?;
                if detail["cwd"] != thread["cwd"] {
                    return Err("Session changed during discovery.".into());
                }
                let profile = r::recorded_profile(&source.config(), &detail).await?;
                Ok(Entry {
                    session: r::Preview {
                        key: key.clone(),
                        old: cwd.clone(),
                        target: if cwd == old {
                            target.clone()
                        } else {
                            target.join(cwd.strip_prefix(&old).map_err(|e| e.to_string())?)
                        },
                        archived: *archived,
                        created_at: thread["createdAt"]
                            .as_i64()
                            .ok_or("Missing session creation time")?,
                        version: version.clone(),
                        fingerprint: r::fingerprint(thread),
                        profile,
                    },
                    title: thread["name"].as_str().unwrap_or(&key.native_id).into(),
                    directory,
                    settings: None,
                    archive_touched: false,
                })
            }
            .await;
            match result {
                Ok(e) => entries.push(e),
                Err(e) => blockers.push(format!("{}: {e}", key.native_id)),
            }
        }
        rpc.close().await;
    }
    if !blockers.is_empty() {
        return Err(format!(
            "Folder was not moved. Resolve these blockers:\n{}",
            blockers.join("\n")
        ));
    }
    entries.sort_by(|a, b| a.session.key.cmp(&b.session.key));
    Ok(Plan {
        old,
        target,
        directory,
        parent,
        sources,
        entries,
    })
}
fn source_for<'a>(plan: &'a Plan, key: &SessionKey) -> Result<&'a Source> {
    plan.sources
        .iter()
        .find(|s| s.id == key.source_id)
        .ok_or_else(|| "Session source no longer configured".into())
}
fn notify(tx: &UnboundedSender<String>, message: impl Into<String>) {
    let _ = tx.send(message.into());
}
fn renamed(plan: &Plan) -> Result<bool> {
    let old = fs::symlink_metadata(&plan.old);
    let target = fs::symlink_metadata(&plan.target);
    if identity(&plan.old).ok().as_ref() == Some(&plan.directory)
        && matches!(&target,Err(e) if e.kind()==std::io::ErrorKind::NotFound)
    {
        return Ok(false);
    }
    if identity(&plan.target).ok().as_ref() == Some(&plan.directory)
        && matches!(&old,Err(e) if e.kind()==std::io::ErrorKind::NotFound)
    {
        return Ok(true);
    }
    Err("Folder identities conflict with the journal. Neither directory was changed; inspect the source and destination before recovery.".into())
}
fn rename_noreplace(old: &Path, target: &Path) -> Result<()> {
    let a = std::ffi::CString::new(old.as_os_str().as_bytes()).map_err(|e| e.to_string())?;
    let b = std::ffi::CString::new(target.as_os_str().as_bytes()).map_err(|e| e.to_string())?;
    // musl does not export a renameat2 wrapper. Use the Linux syscall on
    // both supported architectures, retaining atomic no-overwrite semantics.
    let result = unsafe {
        libc::syscall(
            libc::SYS_renameat2,
            libc::AT_FDCWD,
            a.as_ptr(),
            libc::AT_FDCWD,
            b.as_ptr(),
            libc::RENAME_NOREPLACE,
        )
    };
    if result != 0 {
        return Err(format!(
            "Folder rename failed without overwriting the destination: {}",
            std::io::Error::last_os_error()
        ));
    }
    for parent in [old.parent(), target.parent()].into_iter().flatten() {
        fs::File::open(parent)
            .and_then(|f| f.sync_all())
            .map_err(|e| e.to_string())?;
    }
    Ok(())
}
async fn check_inventory(plan: &Plan, moved: bool, recovery: bool) -> Result<()> {
    let records = inventory(&plan.sources).await?;
    let keys: BTreeSet<_> = plan.entries.iter().map(|e| e.session.key.clone()).collect();
    for (key, (thread, _)) in &records {
        if (affected(thread, &plan.old) || (moved && affected(thread, &plan.target)))
            && !keys.contains(key)
        {
            return Err(
                "Session inventory changed; an additional session now belongs to this folder."
                    .into(),
            );
        }
    }
    for entry in &plan.entries {
        let p = &entry.session;
        let (thread, archived) = records
            .get(&p.key)
            .ok_or_else(|| format!("Session disappeared from inventory: {}", p.key.native_id))?;
        let cwd = thread["cwd"].as_str();
        if !r::matches_thread(thread, p)
            || (cwd != p.old.to_str() && !(recovery && cwd == p.target.to_str()))
        {
            return Err(format!(
                "{}: session identity or folder changed",
                entry.title
            ));
        }
        let mut observed = r::fingerprint(thread);
        let mut expected = p.fingerprint.clone();
        // Native unarchive updates activity time on some supported versions.
        // Identity, directory, permissions and all other metadata remain checked.
        if entry.archive_touched {
            observed.as_object_mut().unwrap().remove("updatedAt");
            expected.as_object_mut().unwrap().remove("updatedAt");
        }
        if !recovery && observed != expected {
            return Err(format!("{}: preview is stale", entry.title));
        }
        if *archived != p.archived && !(entry.archive_touched && p.archived && !archived) {
            return Err("Session archive state changed".into());
        }
    }
    Ok(())
}
pub async fn execute(
    plan: Plan,
    state: PathBuf,
    sources: Vec<Source>,
    recovery: bool,
    tx: UnboundedSender<String>,
) -> Outcome {
    let result = execute_inner(plan.clone(), &state, &sources, recovery, &tx).await;
    match result {
        Ok(()) => Outcome::Complete(plan),
        Err(message) => Outcome::Failed {
            message,
            recovery: pending(&state).map_or(true, |p| p.is_some()),
        },
    }
}
async fn execute_inner(
    plan: Plan,
    state: &Path,
    sources: &[Source],
    recovery: bool,
    tx: &UnboundedSender<String>,
) -> Result<()> {
    let _lock = lock(state)?;
    if plan.sources != sources {
        return Err("Configured Codex sources changed. Restore them before recovery or create a fresh preview.".into());
    }
    if !r::pending(state)?.is_empty() {
        return Err("Recover unfinished single-session relocations first.".into());
    }
    let mut j = if recovery {
        let saved = pending(state)?.ok_or("No unfinished folder move")?;
        if serde_json::to_value(&saved.plan).map_err(|e| e.to_string())?
            != serde_json::to_value(&plan).map_err(|e| e.to_string())?
        {
            return Err("Folder-move journal changed; inspect it again.".into());
        }
        saved
    } else {
        if pending(state)?.is_some() {
            return Err("Recover the unfinished folder move first.".into());
        }
        let (dir, parent) = validate_paths(&plan.old, &plan.target, state, sources)?;
        if dir != plan.directory || parent != plan.parent {
            return Err("Folder or destination parent changed since preview.".into());
        }
        Journal {
            format: 1,
            plan,
            phase: "preparing".into(),
            error: None,
        }
    };
    let moved = renamed(&j.plan)?;
    if moved && j.plan.entries.iter().any(|e| e.settings.is_none()) {
        return Err(
            "Folder moved without complete saved permission checks; manual inspection is required."
                .into(),
        );
    }
    if recovery && !moved && j.phase == "complete" {
        return Err("Completed move has conflicting paths".into());
    }
    if identity(j.plan.target.parent().ok_or("Invalid target")?)? != j.plan.parent {
        return Err("Destination parent changed".into());
    }
    check_inventory(&j.plan, moved, recovery).await?;
    // Every directory must still be the same object, including descendants.
    for entry in &j.plan.entries {
        let path = if moved {
            &entry.session.target
        } else {
            &entry.session.old
        };
        if identity(path)? != entry.directory {
            return Err(format!("Session directory changed: {}", path.display()));
        }
    }
    for source in sources {
        let version = r::version(&source.binary).await?;
        if j.plan
            .entries
            .iter()
            .any(|e| e.session.key.source_id == source.id && e.session.version != version)
        {
            return Err("Codex version changed since preview.".into());
        }
    }
    save(state, &j)?;
    let mut owners: Vec<Option<Rpc>> = (0..j.plan.entries.len()).map(|_| None).collect();
    let mut changed = moved;
    let result: Result<()> = async {
        let mut blockers = vec![];
        for (i, owner) in owners.iter_mut().enumerate() {
            notify(
                tx,
                format!("Checking session {}/{}", i + 1, j.plan.entries.len()),
            );
            let p = j.plan.entries[i].session.clone();
            let source = source_for(&j.plan, &p.key)?.clone();
            let acquire: Result<Rpc> = async {
                let mut rpc = Rpc::start(&source.binary, &source.home)
                    .await
                    .map_err(|e| e.to_string())?;
                let detail = r::read(&mut rpc, &p.key.native_id).await?;
                if r::recorded_profile(&source.config(), &detail).await? != p.profile {
                    return Err("Permission profile changed".into());
                }
                notify(
                    tx,
                    format!("Releasing idle session {}/{}…", i + 1, j.plan.entries.len()),
                );
                crate::providers::codex_release::prepare(
                    &source.binary,
                    &source.config(),
                    &p,
                    recovery,
                )
                .await
                .map_err(|error| error.to_string())?;
                if p.archived && r::listed(&mut rpc, &p.key.native_id).await?.1 {
                    j.plan.entries[i].archive_touched = true;
                    save(state, &j)?;
                    rpc.call("thread/unarchive", json!({"threadId":p.key.native_id}))
                        .await
                        .map_err(|e| e.to_string())?;
                }
                let current = r::resume(&mut rpc, &p.key.native_id).await?;
                let cwd = current["cwd"].as_str().ok_or("Missing current folder")?;
                if !r::matches_thread(&current["thread"], &p)
                    || (cwd != p.old.to_string_lossy()
                        && !(recovery && cwd == p.target.to_string_lossy()))
                {
                    return Err("Session changed before writer acquisition".into());
                }
                r::simple_permissions(&current, Path::new(cwd), p.profile)
                    .map_err(|e| e.to_string())?;
                let settings = r::settings(&current);
                if j.plan.entries[i]
                    .settings
                    .as_ref()
                    .is_some_and(|s| s != &settings)
                {
                    return Err("Session settings changed since move began".into());
                }
                j.plan.entries[i].settings = Some(settings);
                save(state, &j)?;
                Ok(rpc)
            }
            .await;
            match acquire {
                Ok(rpc) => *owner = Some(rpc),
                Err(e) => blockers.push(format!("{}: {e}", j.plan.entries[i].title)),
            }
        }
        if !blockers.is_empty() {
            return Err(format!(
                "Resolve these session blockers:\n{}",
                blockers.join("\n")
            ));
        }
        check_inventory(&j.plan, moved, recovery).await?;
        if !moved {
            let (dir, parent) = validate_paths(&j.plan.old, &j.plan.target, state, sources)?;
            if dir != j.plan.directory || parent != j.plan.parent {
                return Err("Folder changed before rename".into());
            }
            for entry in &j.plan.entries {
                if identity(&entry.session.old)? != entry.directory {
                    return Err("A session directory changed before rename.".into());
                }
            }
            j.phase = "renaming".into();
            save(state, &j)?;
            notify(tx, "Moving folder and files…");
            // Mark uncertainty before the syscall: even an fsync error must require recovery.
            changed = true;
            rename_noreplace(&j.plan.old, &j.plan.target)?;
        }
        if !renamed(&j.plan)? {
            return Err("Folder rename was not verified.".into());
        }
        j.phase = "updating-sessions".into();
        save(state, &j)?;
        for (i, owner) in owners.iter_mut().enumerate() {
            notify(
                tx,
                format!("Updating session {}/{}", i + 1, j.plan.entries.len()),
            );
            let p = &j.plan.entries[i].session;
            if identity(&p.target)? != j.plan.entries[i].directory {
                return Err("Destination directory changed".into());
            }
            let rpc = owner.as_mut().ok_or("Missing native writer")?;
            rpc.call(
                "thread/settings/update",
                json!({"threadId":p.key.native_id,"cwd":p.target}),
            )
            .await
            .map_err(|e| e.to_string())?;
            let mut verified = false;
            for _ in 0..40 {
                let current = r::resume(rpc, &p.key.native_id).await?;
                if current["cwd"].as_str() == p.target.to_str() {
                    r::simple_permissions(&current, &p.target, p.profile)
                        .map_err(|e| e.to_string())?;
                    if j.plan.entries[i].settings.as_ref() != Some(&r::settings(&current)) {
                        return Err("Session settings changed during folder move".into());
                    }
                    verified = true;
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            }
            if !verified {
                return Err("Native directory update did not settle".into());
            }
            j.phase = format!("updated-{}", i + 1);
            save(state, &j)?;
        }
        // Release each owned process and verify persisted settings in a fresh process.
        for (i, owner) in owners.iter_mut().enumerate() {
            notify(
                tx,
                format!("Verifying session {}/{}", i + 1, j.plan.entries.len()),
            );
            if let Some(mut rpc) = owner.take() {
                rpc.close().await;
            }
            let p = &j.plan.entries[i].session;
            let source = source_for(&j.plan, &p.key)?;
            let mut rpc = Rpc::start(&source.binary, &source.home)
                .await
                .map_err(|e| e.to_string())?;
            let verify: Result<()> = async {
                let current = r::resume(&mut rpc, &p.key.native_id).await?;
                let (listed, _) = r::listed(&mut rpc, &p.key.native_id).await?;
                let detail = r::read(&mut rpc, &p.key.native_id).await?;
                if current["cwd"].as_str() != p.target.to_str()
                    || listed["cwd"].as_str() != p.target.to_str()
                    || detail["cwd"].as_str() != p.target.to_str()
                    || !r::matches_thread(&listed, p)
                {
                    return Err("Native destination verification failed".into());
                }
                r::simple_permissions(&current, &p.target, p.profile).map_err(|e| e.to_string())?;
                if j.plan.entries[i].settings.as_ref() != Some(&r::settings(&current)) {
                    return Err("Settings changed after restart".into());
                }
                Ok(())
            }
            .await;
            *owner = Some(rpc);
            verify?;
        }
        Ok(())
    }
    .await;
    // Restore archives on every exit. If a competing writer owns a session,
    // reacquisition fails and the journal remains recoverable.
    let mut cleanup_errors = vec![];
    for (i, owner) in owners.iter_mut().enumerate() {
        if let Some(mut rpc) = owner.take() {
            rpc.close().await;
        }
        if j.plan.entries[i].archive_touched {
            let p = &j.plan.entries[i].session;
            let source = source_for(&j.plan, &p.key)?;
            let restore: Result<()> = async {
                if owner.is_none() {
                    *owner = Some(
                        Rpc::start(&source.binary, &source.home)
                            .await
                            .map_err(|e| e.to_string())?,
                    );
                }
                let rpc = owner.as_mut().ok_or("Missing archive writer")?;
                let current = r::resume(rpc, &p.key.native_id).await?;
                if !r::matches_thread(&current["thread"], p)
                    || (current["cwd"].as_str() != p.old.to_str()
                        && current["cwd"].as_str() != p.target.to_str())
                {
                    return Err("Archive restoration conflicts with changed session".into());
                }
                rpc.call("thread/archive", json!({"threadId":p.key.native_id}))
                    .await
                    .map_err(|e| e.to_string())?;
                if !r::listed(rpc, &p.key.native_id).await?.1 {
                    return Err("Archive restoration failed".into());
                }
                Ok(())
            }
            .await;
            match restore {
                Ok(()) => j.plan.entries[i].archive_touched = false,
                Err(e) => cleanup_errors.push(e),
            }
        }
        if let Some(mut rpc) = owner.take() {
            rpc.close().await;
        }
    }
    let result = if cleanup_errors.is_empty() {
        result
    } else {
        Err(format!(
            "{}\nArchive recovery: {}",
            result.err().unwrap_or_default(),
            cleanup_errors.join("\n")
        ))
    };
    match result {
        Ok(()) => {
            j.phase = "complete".into();
            j.error = None;
            save(state, &j)
        }
        Err(error) => {
            j.error = Some(error.clone());
            if !changed && !recovery && cleanup_errors.is_empty() {
                j.phase = "rejected".into();
            }
            save(state, &j)?;
            Err(error)
        }
    }
}

#[cfg(test)]
mod rename_tests {
    use super::*;

    #[test]
    fn atomic_rename_preserves_an_existing_destination() {
        let root = tempfile::tempdir().unwrap();
        let old = root.path().join("old");
        let target = root.path().join("target");
        fs::create_dir(&old).unwrap();
        fs::create_dir(&target).unwrap();
        fs::write(old.join("source"), "source").unwrap();
        fs::write(target.join("existing"), "existing").unwrap();
        assert!(rename_noreplace(&old, &target).is_err());
        assert_eq!(fs::read_to_string(old.join("source")).unwrap(), "source");
        assert_eq!(
            fs::read_to_string(target.join("existing")).unwrap(),
            "existing"
        );
        fs::remove_file(target.join("existing")).unwrap();
        fs::remove_dir(&target).unwrap();
        rename_noreplace(&old, &target).unwrap();
        assert!(!old.exists());
        assert_eq!(fs::read_to_string(target.join("source")).unwrap(), "source");
    }
}
