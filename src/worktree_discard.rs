//! Permanent feature discard, with a journal outside the checkout and native Codex deletion.
use crate::{
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

async fn source_chats(source: &Source, root: &Path) -> Result<Vec<Chat>> {
    let mut rpc = Rpc::start(&source.binary, &source.home)
        .await
        .map_err(error)?;
    let all = catalog(&mut rpc, &source.home).await?;
    let selected = select_chats(&mut rpc, &all, root).await?;
    for chat in &selected {
        ensure_unloaded(&mut rpc, chat).await?;
    }
    rpc.close().await;
    // The independent app-server's native writer locks protect other servers.
    // Also check the shared daemon when reachable, including its unsaved threads.
    let config = crate::config::SourceConfig {
        id: source.id.clone(),
        home: source.home.clone(),
    };
    if let Ok(mut live) =
        crate::providers::codex_live::LiveClient::connect(&source.binary, config).await
    {
        for (key, session) in live.session_snapshot().await? {
            let matches_path = match session.cwd {
                Some(cwd) => under(&cwd, root)?,
                None => return Err("A loaded Codex conversation has no reported worktree.".into()),
            };
            if matches_path || selected.iter().any(|chat| chat.id == key.native_id) {
                return Err(format!(
                    "Close loaded Codex conversation {} before discarding.",
                    key.native_id
                ));
            }
        }
    }
    Ok(selected)
}

async fn ensure_unloaded(rpc: &mut Rpc, chat: &Chat) -> Result<()> {
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
            "Codex conversation {} changed. Review the worktree again.",
            chat.id
        ));
    }
    if thread["status"]["type"].as_str() != Some("notLoaded") {
        return Err(format!(
            "Close loaded Codex conversation {} before discarding.",
            chat.id
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

async fn conversations(sources: &[Source], root: &Path) -> Result<Vec<Conversations>> {
    if sources.is_empty() {
        return Err("No local Codex sources are configured for verification.".into());
    }
    let mut homes = BTreeSet::new();
    let mut groups = Vec::new();
    for source in sources {
        let home = fs::canonicalize(&source.home).map_err(error)?;
        let home_identity = directory_identity(&home)?;
        if !homes.insert(home_identity) {
            return Err("Multiple Codex sources refer to the same home.".into());
        }
        let chats = tokio::time::timeout(Duration::from_secs(60), source_chats(source, root))
            .await
            .map_err(|_| format!("Codex inventory timed out for {}.", source.home.display()))?
            .map_err(|err| format!("{}: {err}", source.home.display()))?;
        if directory_identity(&source.home)? != home_identity {
            return Err("A configured Codex home changed during inventory.".into());
        }
        groups.push(Conversations {
            source: source.clone(),
            home_identity,
            chats,
        });
    }
    Ok(groups)
}

async fn inspect(
    state: &Path,
    sources: &[Source],
    target: Target,
    previous: Option<&Journal>,
) -> Result<Plan> {
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
    let conversations = conversations(sources, &target.path).await?;
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
    Ok(Plan {
        target,
        checkout,
        main_tip,
        unique_commits,
        conversations,
    })
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
) -> Result<Plan> {
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
    ensure_unloaded(&mut rpc, expected).await?;
    let result = rpc
        .call("thread/delete", json!({"threadId":expected.id}))
        .await
        .map_err(error)?;
    if result != json!({}) {
        return Err("Codex returned an invalid deletion acknowledgment.".into());
    }
    rpc.close().await;
    verify_deleted(source, expected).await
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
    let fresh = preview(state, sources, Some(target.path.clone()), previous.as_ref()).await?;
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
            let current = tokio::time::timeout(
                Duration::from_secs(60),
                source_chats(&group.source, &target.path),
            )
            .await
            .map_err(|_| "Codex revalidation timed out.")??;
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

    let verified = inspect(state, sources, target.clone(), Some(&journal)).await?;
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
