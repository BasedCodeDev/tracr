//! Permanent feature discard, with a journal outside the checkout and native Codex deletion.
use crate::{
    config::SourceConfig,
    domain::{RuntimeStatus, SessionSummary},
    folder_move::Source,
    git,
    providers::codex::{Rpc, SOURCE_KINDS},
    relocation,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    io::{self, BufRead, BufReader, Read},
    os::unix::fs::MetadataExt,
    path::{Path, PathBuf},
    time::Duration,
};

type Result<T> = std::result::Result<T, String>;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct FileStamp {
    device: u64,
    inode: u64,
    mode: u32,
    size: u64,
    modified: (i64, i64),
    changed: (i64, i64),
}

impl From<fs::Metadata> for FileStamp {
    fn from(metadata: fs::Metadata) -> Self {
        Self {
            device: metadata.dev(),
            inode: metadata.ino(),
            mode: metadata.mode(),
            size: metadata.size(),
            modified: (metadata.mtime(), metadata.mtime_nsec()),
            changed: (metadata.ctime(), metadata.ctime_nsec()),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct Checkout {
    directory: (u64, u64),
    git_dir: PathBuf,
    status: Vec<String>,
    index: String,
    files: BTreeMap<PathBuf, FileStamp>,
    #[serde(default)]
    nested_repositories: BTreeMap<PathBuf, NestedRepository>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
enum NestedRepository {
    Bare,
    Checkout,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct Target {
    path: PathBuf,
    main: PathBuf,
    common_dir: PathBuf,
    branch: String,
    tip: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct Chat {
    id: String,
    cwd: PathBuf,
    title: String,
    archived: bool,
    created: i64,
    updated: i64,
    attached_to: Option<String>,
    spawned_by: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct Conversations {
    source: Source,
    home_identity: (u64, u64),
    // Children precede parents, including side chats that Codex requires us to delete first.
    chats: Vec<Chat>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Plan {
    target: Target,
    checkout: Option<Checkout>,
    main_tip: String,
    unique_commits: u64,
    conversations: Vec<Conversations>,
}

pub(crate) enum Preview {
    Ready(Box<Plan>),
    Blocked(Vec<Blocker>),
}

#[derive(Clone)]
pub(crate) struct SessionTarget {
    pub source: Source,
    pub session: SessionSummary,
}

#[derive(PartialEq, Eq)]
pub(crate) struct SessionPlan {
    chat: Chat,
    home_identity: (u64, u64),
}

impl SessionPlan {
    pub(crate) fn review(&self, target: &SessionTarget) -> String {
        use crate::browser::safe_text;

        format!(
            "Permanently delete this Codex session?\n\n{}\nFolder  {}\nSession  {}\nCodex home  {}\nStatus  {}",
            safe_text(&self.chat.title),
            safe_text(&self.chat.cwd.display().to_string()),
            self.chat.id,
            safe_text(&target.source.home.display().to_string()),
            if self.chat.archived {
                "Archived"
            } else {
                "Current"
            },
        )
    }
}

impl Preview {
    fn require_clear(self) -> Result<Plan> {
        match self {
            Self::Ready(plan) => Ok(*plan),
            Self::Blocked(blockers) => Err(Blocker::review(&blockers)),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Blocker {
    source: Source,
    home_identity: (u64, u64),
    id: String,
    title: String,
    cwd: PathBuf,
    status: RuntimeStatus,
    archive: bool,
    release: std::result::Result<SessionSummary, &'static str>,
}

impl Blocker {
    pub(crate) fn review(blockers: &[Self]) -> String {
        use std::fmt::Write;

        let mut text = String::from("Codex still has these conversations loaded.\n");
        if !blockers.iter().any(Self::can_release) {
            text.push_str("No loaded conversation can be released from Tracr right now.\n");
        }
        text.push_str("Close the client or finish the active turn, then choose Retry. Codex may keep idle conversations loaded briefly after closing.\n");
        for blocker in blockers {
            let label = blocker.status.label().unwrap_or("Status unknown");
            let archive = if blocker.archive { " · Archived" } else { "" };
            let _ = writeln!(
                text,
                "\n{}  ·  {label}{archive}",
                crate::browser::safe_text(&blocker.title),
            );
            if let Err(reason) = blocker.release {
                let _ = writeln!(text, "{reason}");
            }
            let _ = writeln!(
                text,
                "Folder  {}\nSession  {}\nCodex home  {}",
                crate::browser::safe_text(&blocker.cwd.display().to_string()),
                blocker.id,
                crate::browser::safe_text(&blocker.source.home.display().to_string()),
            );
        }
        text
    }

    pub(crate) fn can_release(&self) -> bool {
        self.release.is_ok()
    }
}

impl Plan {
    pub(crate) fn branch(&self) -> &str {
        self.target
            .branch
            .strip_prefix("refs/heads/")
            .expect("validated branch")
    }

    pub(crate) fn review(&self) -> String {
        use crate::browser::safe_text;
        use std::fmt::Write;

        let mut body = format!(
            "This permanently deletes the items below.\n\nWorktree  {}\nLocal branch  {}\n{} commits absent from the main checkout\n",
            safe_text(&self.target.path.display().to_string()),
            safe_text(self.branch()),
            self.unique_commits,
        );
        if let Some(checkout) = &self.checkout {
            if !checkout.nested_repositories.is_empty() {
                body.push_str(
                    "\nNested repositories and their Git history will be permanently deleted.\n",
                );
                for (path, kind) in &checkout.nested_repositories {
                    let label = match kind {
                        NestedRepository::Bare => "Bare repository",
                        NestedRepository::Checkout => "Repository",
                    };
                    let _ = writeln!(
                        body,
                        "  {label}  {}",
                        safe_text(&path.display().to_string())
                    );
                }
            }
            let mut local_changes = checkout
                .status
                .iter()
                .map(|record| record.split_at(3))
                .filter(|(code, path)| {
                    !matches!(*code, "?? " | "!! ")
                        || !checkout
                            .nested_repositories
                            .keys()
                            .any(|repo| Path::new(path).starts_with(repo))
                })
                .peekable();
            if local_changes.peek().is_some() {
                body.push_str("\nLocal changes and files\n");
                for (code, path) in local_changes {
                    let label = match code {
                        "?? " => "Untracked",
                        "!! " => "Ignored",
                        _ => match (code.as_bytes()[0] != b' ', code.as_bytes()[1] != b' ') {
                            (true, true) => "Staged and modified",
                            (true, false) => "Staged",
                            _ => "Modified",
                        },
                    };
                    let _ = writeln!(body, "  {label}  {}", safe_text(path));
                }
            } else if checkout.nested_repositories.is_empty() {
                body.push_str("\nNo staged, modified, untracked, or ignored files.\n");
            }
        } else {
            body.push_str("\nWorktree removal already verified.\n");
        }
        let count: usize = self
            .conversations
            .iter()
            .map(|source| source.chats.len())
            .sum();
        let _ = writeln!(body, "\n{count} Codex conversations remaining");
        for source in &self.conversations {
            if !source.chats.is_empty() {
                let _ = writeln!(
                    body,
                    "  {}",
                    safe_text(&source.source.home.display().to_string())
                );
            }
            for chat in &source.chats {
                let archive = if chat.archived { " [archived]" } else { "" };
                let _ = writeln!(
                    body,
                    "    {}{archive} ({})",
                    safe_text(&chat.title),
                    chat.id
                );
            }
        }
        body
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
enum Phase {
    Conversations,
    RemovingWorktree,
    RemovingBranch,
    Complete,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Journal {
    format: u32,
    approved: Plan,
    phase: Phase,
    verified: BTreeSet<(PathBuf, String)>,
}

impl Journal {
    pub(crate) fn root(&self) -> &Path {
        &self.approved.target.path
    }

    pub(crate) fn progress(&self) -> String {
        let git = match self.phase {
            Phase::Conversations => "Worktree and branch remain.",
            Phase::RemovingWorktree => {
                "Worktree removal needs verification. Branch cleanup is pending."
            }
            Phase::RemovingBranch => {
                "Worktree removal verified. Branch cleanup needs verification."
            }
            Phase::Complete => "Worktree and local branch removal verified.",
        };
        format!(
            "{} conversation deletions verified. {git}",
            self.verified.len()
        )
    }
}

fn path(state: &Path) -> PathBuf {
    state.join("worktree-discard").join("current.json")
}

pub(crate) fn pending(state: &Path) -> Result<Option<Journal>> {
    match fs::read(path(state)) {
        Ok(data) => {
            let journal: Journal = serde_json::from_slice(&data).map_err(error)?;
            if journal.format != 1 {
                return Err("Unsupported worktree discard journal.".into());
            }
            Ok((journal.phase != Phase::Complete).then_some(journal))
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(error(e)),
    }
}

fn error(e: impl std::fmt::Display) -> String {
    e.to_string()
}

fn utf8(path: &Path) -> Result<&str> {
    path.to_str()
        .ok_or_else(|| format!("Unsupported non-UTF-8 path {}", path.display()))
}

fn missing(path: &Path) -> Result<bool> {
    match fs::symlink_metadata(path) {
        Ok(_) => Ok(false),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(true),
        Err(e) => Err(error(e)),
    }
}

async fn identify(path: PathBuf) -> Result<Target> {
    if fs::canonicalize(&path).map_err(error)? != path {
        return Err("Select the registered checkout path. Symlink aliases are unsupported.".into());
    }
    let repo = git::discover(&path).await?;
    let main = repo
        .worktrees
        .first()
        .ok_or("No surviving checkout found.")?;
    let row = repo
        .worktrees
        .iter()
        .find(|row| row.path == path)
        .ok_or("Select the root of a registered feature worktree.")?;
    let branch = row
        .branch
        .clone()
        .ok_or("Detached worktrees cannot be discarded.")?;
    if main.path == path
        || main.bare
        || row.bare
        || row.locked
        || row.prunable
        || main.branch.as_ref() == Some(&branch)
        || matches!(branch.as_str(), "refs/heads/main" | "refs/heads/master")
        || !branch.starts_with("refs/heads/")
    {
        return Err("This checkout is protected from discard.".into());
    }
    let actual_branch =
        String::from_utf8(git::read(&path, &["symbolic-ref", "HEAD"]).await?).map_err(error)?;
    if actual_branch.strip_suffix('\n') != Some(&branch) {
        return Err("The checkout branch changed or cannot be represented.".into());
    }
    if repo
        .worktrees
        .iter()
        .any(|other| other.path != path && other.branch.as_ref() == Some(&branch))
    {
        return Err("The feature branch is checked out in another worktree.".into());
    }
    if repo
        .worktrees
        .iter()
        .any(|other| other.path != path && other.path.starts_with(&path))
    {
        return Err("Another registered worktree is inside this checkout.".into());
    }
    let tip = row
        .head
        .clone()
        .ok_or("The feature branch has no commit.")?;
    Ok(Target {
        path,
        main: main.path.clone(),
        common_dir: repo.common_dir,
        branch,
        tip,
    })
}

#[cfg(target_os = "linux")]
fn check_mounts(root: &Path) -> Result<()> {
    use std::os::unix::ffi::OsStrExt;

    // Device IDs alone miss a bind mount on the same filesystem.
    let mounts = fs::read("/proc/self/mountinfo").map_err(error)?;
    for record in mounts
        .split(|byte| *byte == b'\n')
        .filter(|record| !record.is_empty())
    {
        let encoded = record
            .split(|byte| *byte == b' ')
            .nth(4)
            .ok_or("Cannot read mount information.")?;
        let mut path = Vec::new();
        let mut remaining = encoded;
        while let Some((&first, rest)) = remaining.split_first() {
            if first != b'\\' {
                path.push(first);
                remaining = rest;
                continue;
            }
            let escaped = match rest.get(..3) {
                Some(b"040") => b' ',
                Some(b"011") => b'\t',
                Some(b"012") => b'\n',
                Some(b"134") => b'\\',
                _ => return Err("Invalid path in mount information.".into()),
            };
            path.push(escaped);
            remaining = &rest[3..];
        }
        let mount = Path::new(std::ffi::OsStr::from_bytes(&path));
        if mount.starts_with(root) {
            return Err(format!(
                "A mounted filesystem is inside the worktree at {}.",
                mount.display()
            ));
        }
    }
    Ok(())
}

fn scan_files(root: &Path) -> Result<BTreeMap<PathBuf, FileStamp>> {
    #[cfg(target_os = "linux")]
    check_mounts(root)?;
    let device = fs::symlink_metadata(root).map_err(error)?.dev();
    let mut files = BTreeMap::new();
    let mut directories = vec![root.to_path_buf()];
    while let Some(dir) = directories.pop() {
        for entry in fs::read_dir(&dir).map_err(error)? {
            let entry = entry.map_err(error)?;
            let path = entry.path();
            if dir == root && entry.file_name() == ".git" {
                continue;
            }
            let metadata = fs::symlink_metadata(&path).map_err(error)?;
            if metadata.dev() != device {
                return Err(format!(
                    "A mounted filesystem is inside the worktree at {}",
                    path.display()
                ));
            }
            if metadata.is_dir() {
                directories.push(path.clone());
            }
            let relative = path.strip_prefix(root).map_err(error)?;
            utf8(relative)?;
            files.insert(relative.to_owned(), metadata.into());
        }
    }
    Ok(files)
}

async fn nested_repositories(
    root: &Path,
    files: &BTreeMap<PathBuf, FileStamp>,
) -> Result<BTreeMap<PathBuf, NestedRepository>> {
    let mut repositories = BTreeMap::new();
    for (path, stamp) in files {
        let directory = stamp.mode & libc::S_IFMT == libc::S_IFDIR;
        if path.ends_with(".git") {
            if !directory {
                return Err(format!(
                    "Linked checkout or external Git metadata at {}.",
                    root.join(path).display()
                ));
            }
            let parent = path.parent().expect("relative .git path has a parent");
            repositories.insert(parent.to_path_buf(), NestedRepository::Checkout);
        } else if directory
            && [
                ("HEAD", libc::S_IFREG),
                ("objects", libc::S_IFDIR),
                ("refs", libc::S_IFDIR),
            ]
            .iter()
            .all(|(name, kind)| {
                files.get(&path.join(name)).is_some_and(|stamp| {
                    let mode = stamp.mode & libc::S_IFMT;
                    mode == *kind || mode == libc::S_IFLNK
                })
            })
        {
            repositories
                .entry(path.clone())
                .or_insert(NestedRepository::Bare);
        }
    }
    for (path, kind) in &repositories {
        let directory = root.join(path);
        let git_dir = match kind {
            NestedRepository::Bare => path.clone(),
            NestedRepository::Checkout => path.join(".git"),
        };
        verify_nested_repository(root, &directory, &git_dir, *kind, files)
            .await
            .map_err(|reason| {
                format!("Nested Git repository at {}: {reason}", directory.display())
            })?;
    }
    Ok(repositories)
}

async fn verify_nested_repository(
    root: &Path,
    directory: &Path,
    git_dir: &Path,
    kind: NestedRepository,
    files: &BTreeMap<PathBuf, FileStamp>,
) -> Result<()> {
    let metadata = root.join(git_dir);
    // Inspect Git storage links before asking Git to resolve the repository.
    for (path, stamp) in files.iter().filter(|(path, _)| path.starts_with(git_dir)) {
        if stamp.mode & libc::S_IFMT == libc::S_IFLNK {
            let destination = fs::canonicalize(root.join(path))
                .map_err(|e| format!("Cannot resolve Git storage link {}: {e}.", path.display()))?;
            if !destination.starts_with(directory) {
                return Err(format!(
                    "Git storage link {} points outside the repository.",
                    path.display()
                ));
            }
        }
    }
    for path in [
        "commondir",
        "objects/info/alternates",
        "objects/info/http-alternates",
    ] {
        if !missing(&metadata.join(path))? {
            return Err(format!(
                "Shared or alternate Git storage declared in {path}."
            ));
        }
    }

    // Git discovery must resolve to the directory we inspected, even if a
    // malformed nested repository would otherwise fall back to its parent.
    let actual = git::resolve_directory(directory).await?;
    if actual.git_dir != metadata || actual.common_dir != metadata {
        return Err("Git metadata belongs to a different repository.".into());
    }
    let rows = git::parse_worktrees(
        &git::read(directory, &["worktree", "list", "--porcelain", "-z"]).await?,
    )?;
    let bare = kind == NestedRepository::Bare;
    if !matches!(rows.as_slice(), [row] if row.path == directory && row.bare == bare) {
        return Err(
            "Repository has linked worktrees or a working directory outside its root.".into(),
        );
    }
    if !bare {
        // Compare the complete byte sequence so paths containing newlines work.
        let top_level = git::read(directory, &["rev-parse", "--show-toplevel"]).await?;
        if top_level.strip_suffix(b"\n") != Some(directory.as_os_str().as_encoded_bytes()) {
            return Err("Repository uses a working directory outside its root.".into());
        }
        let index = git::read(directory, &["ls-files", "--stage", "-z"]).await?;
        if has_submodules(&index) {
            return Err("Repository contains submodules.".into());
        }
    }
    Ok(())
}

fn has_submodules(index: &[u8]) -> bool {
    index
        .split(|byte| *byte == 0)
        .any(|item| item.starts_with(b"160000 "))
}

async fn checkout(target: &Target) -> Result<Checkout> {
    let directory = fs::symlink_metadata(&target.path).map_err(error)?;
    let git_dir = git::resolve_directory(&target.path).await?.git_dir;
    let index = String::from_utf8(git::read(&target.path, &["ls-files", "--stage", "-z"]).await?)
        .map_err(error)?;
    if has_submodules(index.as_bytes()) {
        return Err("Worktrees with submodules cannot be discarded.".into());
    }
    let status = String::from_utf8(
        git::read(
            &target.path,
            &[
                "status",
                "--porcelain=v1",
                "-z",
                "--no-renames",
                "--untracked-files=all",
                "--ignored",
                "--ignore-submodules=none",
            ],
        )
        .await?,
    )
    .map_err(error)?;
    let status = status
        .split_terminator('\0')
        .map(|record| {
            if !record
                .as_bytes()
                .get(..3)
                .is_some_and(|prefix| prefix.is_ascii() && prefix[2] == b' ')
            {
                return Err("Invalid Git status record.".into());
            }
            Ok(record.to_owned())
        })
        .collect::<Result<_>>()?;
    let root = target.path.clone();
    let files = tokio::task::spawn_blocking(move || scan_files(&root))
        .await
        .map_err(error)??;
    let nested_repositories = nested_repositories(&target.path, &files).await?;
    Ok(Checkout {
        directory: (directory.dev(), directory.ino()),
        git_dir,
        status,
        index,
        files,
        nested_repositories,
    })
}

async fn page_threads(
    rpc: &mut Rpc,
    filter: Option<(&str, &str)>,
    db_only: bool,
) -> Result<Vec<(Value, bool)>> {
    let mut rows = Vec::new();
    for archived in [false, true] {
        let mut cursor = Value::Null;
        let mut seen = BTreeSet::new();
        loop {
            let mut params = json!({
                "archived": archived, "cursor": cursor, "limit": 100,
                "modelProviders": [], "useStateDbOnly": db_only,
            });
            if let Some((key, id)) = filter {
                params[key] = json!(id);
            } else {
                params["sourceKinds"] = json!(SOURCE_KINDS);
            }
            let response = rpc.call("thread/list", params).await.map_err(error)?;
            let data = response["data"]
                .as_array()
                .ok_or("Incomplete Codex conversation list.")?;
            rows.extend(data.iter().cloned().map(|thread| (thread, archived)));
            cursor = response
                .get("nextCursor")
                .ok_or("Missing Codex pagination cursor.")?
                .clone();
            match &cursor {
                Value::Null => break,
                Value::String(next) if !next.is_empty() && seen.insert(next.clone()) => {}
                _ => return Err("Invalid or repeated Codex pagination cursor.".into()),
            }
        }
    }
    Ok(rows)
}

fn uuid(id: &str) -> bool {
    id.len() == 36
        && id.bytes().enumerate().all(|(i, byte)| {
            if matches!(i, 8 | 13 | 18 | 23) {
                byte == b'-'
            } else {
                byte.is_ascii_hexdigit()
            }
        })
}

fn parse_chat(thread: &Value, archived: bool) -> Result<Chat> {
    let id = thread["id"]
        .as_str()
        .filter(|id| uuid(id))
        .ok_or("Invalid Codex thread ID.")?;
    let cwd = thread["cwd"]
        .as_str()
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .ok_or("Codex reported a conversation without an absolute folder.")?;
    let title = ["name", "preview"]
        .into_iter()
        .filter_map(|field| thread[field].as_str())
        .find(|value| !value.trim().is_empty())
        .unwrap_or(id);
    Ok(Chat {
        id: id.into(),
        cwd,
        title: title.chars().take(160).collect(),
        archived,
        created: thread["createdAt"]
            .as_i64()
            .ok_or("Missing conversation creation time.")?,
        updated: thread["updatedAt"]
            .as_i64()
            .ok_or("Missing conversation update time.")?,
        attached_to: thread["attachedToThreadId"].as_str().map(str::to_owned),
        spawned_by: thread["parentThreadId"].as_str().map(str::to_owned),
    })
}

fn under(cwd: &Path, root: &Path) -> Result<bool> {
    // Include missing descendants and aliases to paths that still exist.
    let resolved = crate::folder_move::resolved(cwd)?;
    if cwd.starts_with(root) && !resolved.starts_with(root) {
        return Err(format!(
            "A conversation folder points outside this worktree through {}.",
            cwd.display()
        ));
    }
    Ok(resolved.starts_with(root))
}

// Ordinary thread/list skips saved rollouts without a user message. Discover
// those IDs from the first metadata record and let Codex resolve their current state.
fn rollout_ids(home: &Path) -> Result<BTreeMap<String, bool>> {
    let mut ids = BTreeMap::new();
    for (folder, archived) in [("sessions", false), ("archived_sessions", true)] {
        let root = home.join(folder);
        if missing(&root)? {
            continue;
        }
        let mut directories = vec![root];
        while let Some(dir) = directories.pop() {
            if !fs::symlink_metadata(&dir).map_err(error)?.is_dir() {
                return Err(format!(
                    "Invalid Codex rollout directory {}.",
                    dir.display()
                ));
            }
            for entry in fs::read_dir(dir).map_err(error)? {
                let entry = entry.map_err(error)?;
                let file_type = entry.file_type().map_err(error)?;
                if file_type.is_symlink() {
                    return Err(format!(
                        "Unexpected symlink in Codex storage at {}.",
                        entry.path().display()
                    ));
                }
                if file_type.is_dir() {
                    directories.push(entry.path());
                    continue;
                }
                let name = entry.file_name();
                if !name.as_encoded_bytes().starts_with(b"rollout-")
                    || !name.as_encoded_bytes().ends_with(b".jsonl")
                {
                    continue;
                }
                if !file_type.is_file() {
                    return Err(format!("Invalid Codex rollout {}.", entry.path().display()));
                }
                let file = fs::File::open(entry.path()).map_err(error)?;
                let mut line = Vec::new();
                let length = BufReader::new(file)
                    .take(8 * 1024 * 1024 + 1)
                    .read_until(b'\n', &mut line)
                    .map_err(error)?;
                if length > 8 * 1024 * 1024 || line.last() != Some(&b'\n') {
                    return Err(format!(
                        "Cannot read Codex rollout metadata at {}.",
                        entry.path().display()
                    ));
                }
                let row: Value = serde_json::from_slice(&line).map_err(|_| {
                    format!(
                        "Invalid Codex rollout metadata at {}.",
                        entry.path().display()
                    )
                })?;
                let id = row["payload"]["id"]
                    .as_str()
                    .filter(|id| uuid(id))
                    .ok_or("Codex rollout metadata has an invalid thread ID.")?;
                if row["type"].as_str() != Some("session_meta")
                    || !name
                        .as_encoded_bytes()
                        .ends_with(format!("-{id}.jsonl").as_bytes())
                    || ids.insert(id.to_owned(), archived).is_some()
                {
                    return Err(format!(
                        "Conflicting Codex rollout identity at {}.",
                        entry.path().display()
                    ));
                }
            }
        }
    }
    Ok(ids)
}

async fn catalog(rpc: &mut Rpc, home: &Path) -> Result<BTreeMap<String, Chat>> {
    let mut chats = BTreeMap::new();
    for (thread, archived) in page_threads(rpc, None, false).await? {
        let chat = parse_chat(&thread, archived)?;
        if chats.insert(chat.id.clone(), chat).is_some() {
            return Err("Duplicate conversation in Codex inventory.".into());
        }
    }
    let home = home.to_path_buf();
    let rollouts = tokio::task::spawn_blocking(move || rollout_ids(&home))
        .await
        .map_err(error)??;
    for (id, archived) in rollouts {
        if let Some(chat) = chats.get(&id) {
            if chat.archived != archived {
                return Err(format!("Codex archive state changed for {id}."));
            }
        } else {
            let record = rpc
                .call("thread/read", json!({"threadId":id,"includeTurns":false}))
                .await
                .map_err(error)?;
            let chat = parse_chat(&record["thread"], archived)?;
            if chat.id != id {
                return Err("Codex rollout and native thread identities disagree.".into());
            }
            chats.insert(id, chat);
        }
    }
    let mut parents: Vec<_> = chats.keys().cloned().collect();
    while let Some(parent) = parents.pop() {
        for (thread, archived) in
            page_threads(rpc, Some(("attachedToThreadId", &parent)), true).await?
        {
            let chat = parse_chat(&thread, archived)?;
            if chat.attached_to.as_deref() != Some(&parent) || chat.id == parent {
                return Err("Codex did not honor the side chat filter.".into());
            }
            match chats.get(&chat.id) {
                Some(existing) if existing == &chat => {}
                Some(_) => return Err("Conflicting side chat in Codex inventory.".into()),
                None => {
                    parents.push(chat.id.clone());
                    chats.insert(chat.id.clone(), chat);
                }
            }
        }
    }
    Ok(chats)
}

async fn select_chats(
    rpc: &mut Rpc,
    all: &BTreeMap<String, Chat>,
    root: &Path,
) -> Result<Vec<Chat>> {
    let mut selected = BTreeMap::new();
    for chat in all.values() {
        if under(&chat.cwd, root)? {
            selected.insert(chat.id.clone(), chat.clone());
        }
    }

    // Confirm native spawn edges rather than relying solely on ordinary listing metadata.
    // In particular, the unfiltered catalog can omit internal descendant source kinds.
    let mut queue: Vec<_> = selected.keys().cloned().collect();
    while let Some(id) = queue.pop() {
        for (thread, archived) in page_threads(rpc, Some(("parentThreadId", &id)), true).await? {
            let mut chat = parse_chat(&thread, archived)?;
            if chat.spawned_by.as_deref() != Some(&id) {
                return Err("Codex did not honor the spawned conversation filter.".into());
            }
            if let Some(existing) = selected.get(&chat.id) {
                if existing == &chat {
                    continue;
                }
                // A side chat can be visible through both its attachment and spawn edge.
                chat.attached_to = chat.attached_to.or_else(|| existing.attached_to.clone());
            }
            if !under(&chat.cwd, root)? {
                return Err(format!(
                    "Conversation {} would also be deleted outside this worktree at {}.",
                    chat.id,
                    chat.cwd.display()
                ));
            }
            if selected.insert(chat.id.clone(), chat.clone()).is_none() {
                queue.push(chat.id);
            }
        }
        if let Some(chat) = all.values().find(|chat| {
            chat.attached_to.as_deref() == Some(&id) && !selected.contains_key(&chat.id)
        }) {
            return Err(format!(
                "Side chat {} would also be affected outside this worktree at {}.",
                chat.id,
                chat.cwd.display()
            ));
        }
    }

    let mut ordered = Vec::new();
    while !selected.is_empty() {
        let parents: BTreeSet<_> = selected
            .values()
            .flat_map(|chat| [&chat.attached_to, &chat.spawned_by])
            .flatten()
            .cloned()
            .collect();
        let leaves: Vec<_> = selected
            .keys()
            .filter(|id| !parents.contains(*id))
            .cloned()
            .collect();
        if leaves.is_empty() {
            return Err("Codex reported a cycle in conversation ownership.".into());
        }
        ordered.extend(leaves.iter().filter_map(|id| selected.remove(id)));
    }
    Ok(ordered)
}

fn release_candidate<'a>(
    saved: Option<&'a Chat>,
    status: RuntimeStatus,
    shared_idle_folder: Option<&Path>,
) -> std::result::Result<&'a Chat, &'static str> {
    match status {
        RuntimeStatus::Idle => {}
        RuntimeStatus::Running | RuntimeStatus::Waiting { .. } => {
            return Err("Finish the turn or close its Codex client.");
        }
        _ => return Err("Tracr could not verify that this conversation is idle."),
    }
    let chat = saved.ok_or("This conversation has no saved record to verify.")?;
    if chat.archived {
        return Err("Tracr can release only unarchived conversations.");
    }
    if chat.attached_to.is_some() || chat.spawned_by.is_some() {
        return Err("Tracr cannot release side chats or spawned conversations.");
    }
    let cwd = shared_idle_folder
        .ok_or("The shared Codex server could not confirm that this conversation is idle.")?;
    if chat.cwd != cwd {
        return Err("The loaded folder differs from the saved folder. Close the client and retry.");
    }
    Ok(chat)
}

async fn source_chats(
    source: &Source,
    root: &Path,
    home_identity: (u64, u64),
) -> Result<(Vec<Chat>, Vec<Blocker>)> {
    let mut rpc = Rpc::start(&source.binary, &source.home)
        .await
        .map_err(error)?;
    let all = catalog(&mut rpc, &source.home).await?;
    let selected = select_chats(&mut rpc, &all, root).await?;
    let mut blockers = BTreeMap::new();
    for chat in &selected {
        let status = read_status(&mut rpc, chat).await?;
        if status != RuntimeStatus::Unloaded {
            blockers.insert(
                chat.id.clone(),
                Blocker {
                    source: source.clone(),
                    home_identity,
                    id: chat.id.clone(),
                    title: chat.title.clone(),
                    cwd: chat.cwd.clone(),
                    status,
                    archive: chat.archived,
                    release: Err(release_candidate(Some(chat), status, None)
                        .expect_err("the shared server has not been checked")),
                },
            );
        }
    }
    rpc.close().await;
    // Independent app-servers still have to acquire Codex's native writer locks.
    // The shared daemon can also know about conversations with no saved rollout.
    let config = SourceConfig {
        id: source.id.clone(),
        home: source.home.clone(),
    };
    if let Ok(mut live) =
        crate::providers::codex_live::LiveClient::connect(&source.binary, config).await
    {
        for (key, session) in live.session_snapshot().await? {
            let cwd = session
                .cwd
                .ok_or("A loaded Codex conversation has no reported worktree.")?;
            let saved = selected.iter().find(|chat| chat.id == key.native_id);
            if !under(&cwd, root)? && saved.is_none() {
                continue;
            }
            let status = match blockers.get(&key.native_id) {
                Some(previous) if previous.status != RuntimeStatus::Idle => previous.status,
                _ => session.status,
            };
            let shared_idle_folder =
                (session.status == RuntimeStatus::Idle).then_some(cwd.as_path());
            let release =
                release_candidate(saved, status, shared_idle_folder).map(|chat| SessionSummary {
                    key: key.clone(),
                    title: chat.title.clone(),
                    cwd: Some(chat.cwd.clone()),
                    created_at: chat.created,
                    updated_at: chat.updated,
                    archived: false,
                    can_accept_direct_input: None,
                    source_kind: None,
                    git: None,
                });
            blockers.insert(
                key.native_id.clone(),
                Blocker {
                    source: source.clone(),
                    home_identity,
                    id: key.native_id,
                    title: session.title,
                    cwd,
                    status,
                    archive: saved.is_some_and(|chat| chat.archived),
                    release,
                },
            );
        }
    }
    Ok((selected, blockers.into_values().collect()))
}

async fn read_status(rpc: &mut Rpc, chat: &Chat) -> Result<RuntimeStatus> {
    let record = rpc
        .call(
            "thread/read",
            json!({"threadId":chat.id,"includeTurns":false}),
        )
        .await
        .map_err(error)?;
    let thread = &record["thread"];
    let actual = parse_chat(thread, chat.archived)?;
    if actual.id != chat.id
        || actual.cwd != chat.cwd
        || actual.created != chat.created
        || actual.updated != chat.updated
        || actual.attached_to != chat.attached_to
        || actual.spawned_by != chat.spawned_by
    {
        return Err(format!(
            "Codex conversation {} changed. Review it again.",
            chat.id
        ));
    }
    Ok(RuntimeStatus::from_codex(thread.get("status")))
}

async fn ensure_unloaded(rpc: &mut Rpc, chat: &Chat) -> Result<()> {
    let status = read_status(rpc, chat).await?;
    if status != RuntimeStatus::Unloaded {
        return Err(format!(
            "Codex conversation {} became loaded ({}). Close it and retry.",
            crate::browser::safe_text(&chat.title),
            status.label().unwrap_or("Status unknown")
        ));
    }
    Ok(())
}

fn relocation_clear(state: &Path) -> Result<()> {
    if crate::folder_move::pending(state)?.is_some() || !relocation::pending(state)?.is_empty() {
        return Err("Recover unfinished relocations before discarding a worktree.".into());
    }
    Ok(())
}

fn directory_identity(path: &Path) -> Result<(u64, u64)> {
    let metadata = fs::metadata(path).map_err(error)?;
    if !metadata.is_dir() {
        return Err(format!("Expected a directory at {}.", path.display()));
    }
    Ok((metadata.dev(), metadata.ino()))
}

async fn conversations(
    sources: &[Source],
    root: &Path,
) -> Result<(Vec<Conversations>, Vec<Blocker>)> {
    if sources.is_empty() {
        return Err("No local Codex sources are configured for verification.".into());
    }
    let mut homes = BTreeSet::new();
    let mut groups = Vec::new();
    let mut blockers = Vec::new();
    for source in sources {
        let home = fs::canonicalize(&source.home).map_err(error)?;
        let home_identity = directory_identity(&home)?;
        if !homes.insert(home_identity) {
            return Err("Multiple Codex sources refer to the same home.".into());
        }
        let (chats, loaded) = tokio::time::timeout(
            Duration::from_secs(60),
            source_chats(source, root, home_identity),
        )
        .await
        .map_err(|_| format!("Codex inventory timed out for {}.", source.home.display()))?
        .map_err(|err| format!("{}: {err}", source.home.display()))?;
        if directory_identity(&source.home)? != home_identity {
            return Err("A configured Codex home changed during inventory.".into());
        }
        blockers.extend(loaded);
        groups.push(Conversations {
            source: source.clone(),
            home_identity,
            chats,
        });
    }
    Ok((groups, blockers))
}

async fn inspect(
    state: &Path,
    sources: &[Source],
    target: Target,
    previous: Option<&Journal>,
) -> Result<Preview> {
    relocation_clear(state)?;
    for location in [&target.common_dir, &target.main, state]
        .into_iter()
        .chain(sources.iter().map(|source| source.home.as_path()))
    {
        if crate::folder_move::resolved(location)?.starts_with(&target.path) {
            return Err(format!(
                "The worktree contains protected storage at {}.",
                location.display()
            ));
        }
    }
    let repo = git::discover(&target.main).await?;
    if repo.common_dir != target.common_dir
        || repo.worktrees.first().map(|main| &main.path) != Some(&target.main)
    {
        return Err("The surviving checkout changed.".into());
    }
    let branch = branch_tip(&target).await?;
    let present = repo.worktrees.iter().any(|row| row.path == target.path);
    let checkout = if present {
        if previous.is_some_and(|old| old.phase == Phase::RemovingBranch) {
            return Err("The worktree path became registered again. Recovery stopped.".into());
        }
        if identify(target.path.clone()).await? != target || branch.as_deref() != Some(&target.tip)
        {
            return Err("The registered worktree or feature branch changed.".into());
        }
        let checkout = checkout(&target).await?;
        if let Some(original) = previous.and_then(|old| old.approved.checkout.as_ref()) {
            if checkout.directory != original.directory || checkout.git_dir != original.git_dir {
                return Err("The worktree directory was replaced. Recovery stopped.".into());
            }
        }
        Some(checkout)
    } else {
        if !previous.is_some_and(|old| old.phase != Phase::Conversations) || !missing(&target.path)?
        {
            return Err("Cannot verify removal of the registered worktree.".into());
        }
        if repo
            .worktrees
            .iter()
            .any(|row| row.branch.as_ref() == Some(&target.branch))
        {
            return Err("The local branch is now checked out in another worktree.".into());
        }
        if branch.as_deref() != Some(&target.tip)
            && !(branch.is_none() && previous.is_some_and(|old| old.phase == Phase::RemovingBranch))
        {
            return Err(
                "The local branch changed after worktree removal. Recovery stopped.".into(),
            );
        }
        None
    };
    let main_tip = git_text(&target.main, &["rev-parse", "--verify", "HEAD"]).await?;
    let unique_commits = git_text(
        &target.main,
        &[
            "rev-list",
            "--count",
            &format!("{main_tip}..{}", target.tip),
        ],
    )
    .await?
    .parse()
    .map_err(error)?;
    let (conversations, blockers) = conversations(sources, &target.path).await?;
    if previous.is_some_and(|journal| {
        !journal
            .approved
            .conversations
            .iter()
            .map(|group| group.home_identity)
            .eq(conversations.iter().map(|group| group.home_identity))
    }) {
        return Err("A configured Codex home was replaced. Recovery stopped.".into());
    }
    if checkout.is_none() && conversations.iter().any(|source| !source.chats.is_empty()) {
        return Err(
            "A conversation now refers to the removed worktree. Branch cleanup stopped.".into(),
        );
    }
    if !blockers.is_empty() {
        return Ok(Preview::Blocked(blockers));
    }
    Ok(Preview::Ready(Box::new(Plan {
        target,
        checkout,
        main_tip,
        unique_commits,
        conversations,
    })))
}

async fn git_text(path: &Path, args: &[&str]) -> Result<String> {
    let bytes = git::read(path, args).await?;
    Ok(String::from_utf8(bytes)
        .map_err(error)?
        .trim_end_matches('\n')
        .to_owned())
}

async fn branch_tip(target: &Target) -> Result<Option<String>> {
    let refs = git_text(
        &target.main,
        &[
            "for-each-ref",
            "--format=%(refname)%00%(objectname)%00%(symref)",
            &target.branch,
        ],
    )
    .await?;
    for line in refs.lines() {
        let mut fields = line.split('\0');
        if fields.next() == Some(target.branch.as_str()) {
            let tip = fields.next().ok_or("Git omitted the local branch tip.")?;
            if fields.next() != Some("") {
                return Err("The feature branch is a symbolic ref.".into());
            }
            return Ok(Some(tip.to_owned()));
        }
    }
    Ok(None)
}

pub(crate) async fn preview(
    state: &Path,
    sources: &[Source],
    selected: Option<PathBuf>,
    previous: Option<&Journal>,
) -> Result<Preview> {
    let journal = pending(state)?;
    if journal.as_ref() != previous {
        return Err("Discard recovery changed. Reopen it to review the current state.".into());
    }
    let target = match previous {
        Some(previous) => {
            if !previous
                .approved
                .conversations
                .iter()
                .map(|group| &group.source)
                .eq(sources.iter())
            {
                return Err(
                    "Configured Codex sources changed. Restore them before continuing discard."
                        .into(),
                );
            }
            previous.approved.target.clone()
        }
        None => identify(selected.ok_or("Select a linked feature worktree.")?).await?,
    };
    inspect(state, sources, target, previous).await
}

fn session_operations_clear(state: &Path) -> Result<()> {
    if pending(state)?.is_some() {
        return Err("Press D to review the unfinished worktree discard first.".into());
    }
    if crate::folder_move::pending(state)?.is_some() || !relocation::pending(state)?.is_empty() {
        return Err("Recover unfinished relocations before deleting a session.".into());
    }
    Ok(())
}

async fn inspect_session(
    state: &Path,
    target: &SessionTarget,
    rpc: &mut Rpc,
    home_identity: (u64, u64),
) -> Result<std::result::Result<SessionPlan, Blocker>> {
    session_operations_clear(state)?;
    let SessionTarget { source, session } = target;
    let id = &session.key.native_id;
    if session.key.provider.0 != "codex" || session.key.source_id != source.id || !uuid(id) {
        return Err("The selected session has an invalid Codex identity.".into());
    }

    let thread = relocation::read(rpc, id).await?;
    let chat = parse_chat(&thread, session.archived)?;
    if chat.id != *id
        || Some(&chat.cwd) != session.cwd.as_ref()
        || chat.created != session.created_at
    {
        return Err("The selected session changed. Refresh and review it again.".into());
    }
    let attachment = chat
        .attached_to
        .as_deref()
        .map(|parent| ("attachedToThreadId", parent));
    let listed = page_threads(rpc, attachment, attachment.is_some()).await?;
    let mut matches = listed
        .iter()
        .filter(|(thread, _)| thread["id"].as_str() == Some(id));
    let current = matches
        .next()
        .ok_or("The selected session is missing from Codex. Refresh the list.")?;
    if matches.next().is_some() || parse_chat(&current.0, current.1)? != chat {
        return Err("The selected session changed. Refresh and review it again.".into());
    }

    let mut status = RuntimeStatus::from_codex(thread.get("status"));
    let config = SourceConfig {
        id: source.id.clone(),
        home: source.home.clone(),
    };
    let mut shared_idle_folder = None;
    if let Ok(mut live) =
        crate::providers::codex_live::LiveClient::connect(&source.binary, config).await
        && let Some(loaded) = live.session_snapshot().await?.remove(&session.key)
    {
        if matches!(status, RuntimeStatus::Unloaded | RuntimeStatus::Idle) {
            status = loaded.status;
        }
        if loaded.status == RuntimeStatus::Idle {
            shared_idle_folder = loaded.cwd;
        }
    }

    for (filter, field) in [
        ("attachedToThreadId", "side chat"),
        ("parentThreadId", "spawned conversation"),
    ] {
        let dependents = page_threads(rpc, Some((filter, id)), true).await?;
        for (dependent, archived) in &dependents {
            let dependent = parse_chat(dependent, *archived)?;
            let parent = if filter == "attachedToThreadId" {
                dependent.attached_to
            } else {
                dependent.spawned_by
            };
            if parent.as_deref() != Some(id) || dependent.id == *id {
                return Err(format!("Codex did not honor the {field} filter."));
            }
        }
        if !dependents.is_empty() {
            return Err(format!(
                "This session has a {field}. Delete its dependent conversations first."
            ));
        }
    }

    if directory_identity(&source.home)? != home_identity {
        return Err("The configured Codex home changed. Review the session again.".into());
    }
    if status == RuntimeStatus::Unloaded {
        return Ok(Ok(SessionPlan {
            chat,
            home_identity,
        }));
    }
    let release =
        release_candidate(Some(&chat), status, shared_idle_folder.as_deref()).map(|chat| {
            SessionSummary {
                updated_at: chat.updated,
                ..session.clone()
            }
        });
    Ok(Err(Blocker {
        source: source.clone(),
        home_identity,
        id: chat.id,
        title: chat.title,
        cwd: chat.cwd,
        status,
        archive: chat.archived,
        release,
    }))
}

pub(crate) async fn preview_session(
    state: &Path,
    target: &SessionTarget,
) -> Result<std::result::Result<SessionPlan, Blocker>> {
    tokio::time::timeout(Duration::from_secs(60), async {
        let source = &target.source;
        let home_identity = directory_identity(&source.home)?;
        let mut rpc = Rpc::start(&source.binary, &source.home)
            .await
            .map_err(error)?;
        let preview = inspect_session(state, target, &mut rpc, home_identity).await;
        rpc.close().await;
        preview
    })
    .await
    .map_err(|_| "Codex session checks timed out.")?
}

pub(crate) async fn release_session(
    state: &Path,
    target: &SessionTarget,
    approved: &[Blocker],
) -> Result<(std::result::Result<SessionPlan, Blocker>, String)> {
    let _lock = crate::folder_move::lock(state)?;
    let fresh = preview_session(state, target).await?;
    let Err(blocker) = &fresh else {
        return Ok((fresh, String::new()));
    };
    if !approved.contains(blocker) || !blocker.can_release() {
        return Ok((
            fresh,
            "The session changed. Review its current status before releasing.\n".into(),
        ));
    }
    let report = release_blocker(blocker).await?;
    Ok((preview_session(state, target).await?, report))
}

pub(crate) async fn execute_session(
    state: &Path,
    target: &SessionTarget,
    reviewed: SessionPlan,
) -> Result<()> {
    let _lock = crate::folder_move::lock(state)?;
    let source = &target.source;
    if directory_identity(&source.home)? != reviewed.home_identity {
        return Err("The configured Codex home changed. Review the session again.".into());
    }
    let mut rpc = Rpc::start(&source.binary, &source.home)
        .await
        .map_err(error)?;
    let fresh = tokio::time::timeout(
        Duration::from_secs(60),
        inspect_session(state, target, &mut rpc, reviewed.home_identity),
    )
    .await
    .map_err(|_| "Codex session checks timed out.")??
    .map_err(|blocker| Blocker::review(&[blocker]))?;
    if fresh != reviewed {
        return Err("The reviewed session changed. Open a new review.".into());
    }
    delete_chat(&mut rpc, &fresh.chat).await?;
    rpc.close().await;
    verify_deleted(source, &fresh.chat)
        .await
        .map_err(|error| format!("Codex deletion could not be verified.\n{error}"))?;
    if directory_identity(&source.home)? != fresh.home_identity {
        return Err("Codex deletion could not be verified because its home changed.".into());
    }
    Ok(())
}

/// Release only the idle records the user reviewed. A fresh preview verifies
/// both worktree eligibility and the conversation set before any release.
pub(crate) async fn release_idle(
    state: &Path,
    sources: &[Source],
    selected: Option<PathBuf>,
    previous: Option<&Journal>,
    approved: Vec<Blocker>,
) -> Result<(Preview, String)> {
    use std::fmt::Write;

    let _lock = crate::folder_move::lock(state)?;
    let fresh = preview(state, sources, selected.clone(), previous).await?;
    let Preview::Blocked(blockers) = fresh else {
        return Ok((fresh, String::new()));
    };
    let mut report = String::new();
    for blocker in &approved {
        let Ok(session) = &blocker.release else {
            continue;
        };
        if !blockers.contains(blocker) {
            let _ = writeln!(
                report,
                "{} changed. Review its current status before releasing.",
                session.title
            );
            continue;
        }
        report.push_str(&release_blocker(blocker).await?);
    }
    let fresh = preview(state, sources, selected, previous).await?;
    Ok((fresh, report))
}

async fn release_blocker(blocker: &Blocker) -> Result<String> {
    use crate::providers::{codex_release::ReleaseOutcome, codex_terminals};

    let session = blocker.release.as_ref().map_err(|reason| *reason)?;
    let source = &blocker.source;
    if directory_identity(&source.home)? != blocker.home_identity {
        return Err("A configured Codex home was replaced.".into());
    }
    let config = SourceConfig {
        id: source.id.clone(),
        home: source.home.clone(),
    };
    let message = match crate::providers::codex_release::release_discard_idle(
        &source.binary,
        &config,
        session,
    )
    .await
    {
        Ok(ReleaseOutcome::Released | ReleaseOutcome::AlreadyUnloaded) => {
            return Ok(String::new());
        }
        Ok(ReleaseOutcome::StillLoaded) => {
            "Still loaded. Close any Codex client using this conversation and retry.".into()
        }
        Ok(ReleaseOutcome::Busy) => {
            "Conversation became active or its status could not be verified.".into()
        }
        Ok(ReleaseOutcome::Terminals(codex_terminals::Blocker::Commands(_))) => {
            "Background terminals are still running. Close them in Codex before retrying.".into()
        }
        Ok(ReleaseOutcome::Terminals(codex_terminals::Blocker::Unavailable(err))) => {
            format!("Could not verify background terminals. {err}")
        }
        Ok(ReleaseOutcome::Unavailable) => "Could not connect to the shared Codex server.".into(),
        Err(error) => error,
    };
    Ok(format!(
        "{}  ·  {}\n",
        crate::browser::safe_text(&session.title),
        crate::browser::safe_text(&message)
    ))
}

async fn verify_deleted(source: &Source, chat: &Chat) -> Result<()> {
    // A native read supplements listing: a thread that disappeared from the
    // index alone may still have a saved rollout.
    let mut rpc = Rpc::start(&source.binary, &source.home)
        .await
        .map_err(error)?;
    let result = rpc.thread_is_absent(&chat.id).await;
    rpc.close().await;
    if result.map_err(error)? {
        Ok(())
    } else {
        Err(format!("Codex conversation {} is still readable.", chat.id))
    }
}

async fn delete_one(source: &Source, expected: &Chat) -> Result<()> {
    let mut rpc = Rpc::start(&source.binary, &source.home)
        .await
        .map_err(error)?;
    delete_chat(&mut rpc, expected).await?;
    rpc.close().await;
    verify_deleted(source, expected).await
}

async fn delete_chat(rpc: &mut Rpc, expected: &Chat) -> Result<()> {
    ensure_unloaded(rpc, expected).await?;
    let result = rpc
        .call("thread/delete", json!({"threadId":expected.id}))
        .await
        .map_err(error)?;
    if result != json!({}) {
        return Err("Codex returned an invalid deletion acknowledgment.".into());
    }
    Ok(())
}

fn save(state: &Path, journal: &Journal) -> Result<()> {
    relocation::persist(&path(state), journal)
}

pub(crate) async fn execute(
    state: &Path,
    sources: &[Source],
    plan: Plan,
    previous: Option<Journal>,
) -> Result<PathBuf> {
    let _lock = crate::folder_move::lock(state)?;
    let target = &plan.target;
    let fresh = preview(state, sources, Some(target.path.clone()), previous.as_ref())
        .await?
        .require_clear()?;
    if fresh != plan {
        return Err(
            "The reviewed worktree or its conversations changed. Open a new preview.".into(),
        );
    }

    let mut journal = Journal {
        format: 1,
        approved: plan.clone(),
        phase: previous
            .as_ref()
            .map_or(Phase::Conversations, |old| old.phase),
        verified: previous
            .as_ref()
            .map(|old| old.verified.clone())
            .unwrap_or_default(),
    };
    if let Some(previous) = &previous {
        for (old, current) in previous
            .approved
            .conversations
            .iter()
            .zip(&plan.conversations)
        {
            for chat in old
                .chats
                .iter()
                .filter(|chat| !current.chats.iter().any(|new| new.id == chat.id))
            {
                verify_deleted(&old.source, chat).await?;
                journal
                    .verified
                    .insert((old.source.home.clone(), chat.id.clone()));
            }
        }
    }
    save(state, &journal)?;

    for group in &plan.conversations {
        for (index, chat) in group.chats.iter().enumerate() {
            let (current, blockers) = tokio::time::timeout(
                Duration::from_secs(60),
                source_chats(&group.source, &target.path, group.home_identity),
            )
            .await
            .map_err(|_| "Codex revalidation timed out.")??;
            if !blockers.is_empty() {
                return Err(Blocker::review(&blockers));
            }
            if current != group.chats[index..] {
                return Err(
                    "The reviewed conversation set changed. Review the remaining discard.".into(),
                );
            }
            if identify(target.path.clone()).await? != *target
                || Some(checkout(target).await?) != plan.checkout
            {
                return Err("The reviewed worktree changed. Review the remaining discard.".into());
            }
            if directory_identity(&group.source.home)? != group.home_identity {
                return Err("A configured Codex home was replaced. Discard stopped.".into());
            }
            delete_one(&group.source, chat).await?;
            journal
                .verified
                .insert((group.source.home.clone(), chat.id.clone()));
            save(state, &journal)?;
        }
    }

    let verified = inspect(state, sources, target.clone(), Some(&journal))
        .await?
        .require_clear()?;
    if verified
        .conversations
        .iter()
        .any(|group| !group.chats.is_empty())
    {
        return Err("A conversation still refers to this worktree. Git cleanup stopped.".into());
    }
    if verified.checkout != plan.checkout || verified.main_tip != plan.main_tip {
        return Err("Git state changed before cleanup. Review the remaining discard.".into());
    }
    if let Some(reviewed) = &plan.checkout {
        if identify(target.path.clone()).await? != *target || &checkout(target).await? != reviewed {
            return Err(
                "The worktree changed before removal. Review the remaining discard.".into(),
            );
        }
        journal.phase = Phase::RemovingWorktree;
        save(state, &journal)?;
        let mut args = vec!["worktree", "remove"];
        if !reviewed.status.is_empty() || !reviewed.nested_repositories.is_empty() {
            // A single force flag cannot remove a locked worktree.
            args.push("--force");
        }
        args.extend(["--", utf8(&target.path)?]);
        git::read(&target.main, &args).await?;
    }

    let repo = git::discover(&target.main).await?;
    if repo.common_dir != target.common_dir
        || !missing(&target.path)?
        || repo
            .worktrees
            .iter()
            .any(|row| row.path == target.path || row.branch.as_ref() == Some(&target.branch))
    {
        return Err(
            "Could not verify worktree removal and branch ownership. Branch cleanup stopped."
                .into(),
        );
    }
    let branch = branch_tip(target).await?;
    if branch.as_deref() == Some(&target.tip) {
        journal.phase = Phase::RemovingBranch;
        save(state, &journal)?;
        git::read(
            &target.main,
            &[
                "update-ref",
                "--no-deref",
                "-d",
                &target.branch,
                &target.tip,
            ],
        )
        .await?;
    } else if branch.is_some() || journal.phase != Phase::RemovingBranch {
        return Err(
            "The local branch changed after worktree removal. Branch cleanup stopped.".into(),
        );
    }
    if branch_tip(target).await?.is_some() {
        return Err("The local branch still exists after deletion.".into());
    }
    journal.phase = Phase::Complete;
    save(state, &journal)?;
    Ok(target.main.clone())
}
