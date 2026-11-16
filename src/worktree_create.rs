//! Reviewed, non-destructive creation of relative-linked Git worktrees.
use std::{
    fs,
    os::unix::fs::{MetadataExt, OpenOptionsExt},
    path::{Path, PathBuf},
    process::Stdio,
    time::Duration,
};
use tokio::{io::AsyncReadExt, process::Command};

type Result<T> = std::result::Result<T, String>;
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Context {
    pub source: PathBuf,
    pub main: PathBuf,
    pub common: PathBuf,
    pub commit: String,
    pub subject: String,
    identity: (u64, u64),
}
#[derive(Clone, Debug)]
pub struct Plan {
    pub context: Context,
    pub branch: String,
    pub destination: PathBuf,
    pub outside: bool,
    pub exclusion: Option<String>,
    exclude_before: Option<Vec<u8>>,
    ignore_root: Option<PathBuf>,
}

async fn drain(mut pipe: impl tokio::io::AsyncRead + Unpin) -> std::io::Result<Vec<u8>> {
    let mut result = Vec::new();
    let mut buf = [0; 8192];
    loop {
        let n = pipe.read(&mut buf).await?;
        if n == 0 {
            break;
        }
        if result.len() + n > 2 * 1024 * 1024 {
            return Err(std::io::Error::other("Git output exceeded 2 MiB"));
        }
        result.extend_from_slice(&buf[..n]);
    }
    Ok(result)
}
struct ProcessGroup(Option<u32>);
impl Drop for ProcessGroup {
    fn drop(&mut self) {
        if let Some(pid) = self.0 {
            unsafe {
                libc::kill(-(pid as i32), libc::SIGKILL);
            }
        }
    }
}
async fn run(root: &Path, args: &[&str], seconds: u64) -> Result<(bool, String)> {
    let mut command = Command::new("git");
    command
        .arg("-C")
        .arg(root)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    for name in [
        "GIT_DIR",
        "GIT_WORK_TREE",
        "GIT_COMMON_DIR",
        "GIT_INDEX_FILE",
        "GIT_OBJECT_DIRECTORY",
        "GIT_ALTERNATE_OBJECT_DIRECTORIES",
        "GIT_PREFIX",
    ] {
        command.env_remove(name);
    }
    command.process_group(0);
    let mut child = command
        .spawn()
        .map_err(|e| format!("Could not run Git: {e}"))?;
    let pid = child.id().unwrap();
    let mut group = ProcessGroup(Some(pid));
    let out = child.stdout.take().unwrap();
    let err = child.stderr.take().unwrap();
    let result = tokio::time::timeout(Duration::from_secs(seconds), async {
        let (status, stdout, stderr) = tokio::try_join!(child.wait(), drain(out), drain(err))?;
        Ok::<_, std::io::Error>((
            status.success(),
            if status.success() { stdout } else { stderr },
        ))
    })
    .await;
    match result {
        Ok(Ok((success, bytes))) => {
            group.0 = None;
            Ok((
                success,
                String::from_utf8(bytes).map_err(|_| "Git returned non-UTF-8 output")?,
            ))
        }
        failure => {
            // Reap owned Git and any hooks/filters when a deadline or pipe fails.
            unsafe {
                libc::kill(-(pid as i32), libc::SIGKILL);
            }
            let _ = child.wait().await;
            Err(match failure {
                Ok(Err(e)) => format!("Git command failed: {e}"),
                _ => "Git command timed out; inspect the reported worktree state before retrying."
                    .into(),
            })
        }
    }
}
async fn git(root: &Path, args: &[&str]) -> Result<String> {
    let (success, text) = run(root, args, 30).await?;
    if success {
        Ok(text.trim_end_matches('\n').to_owned())
    } else {
        Err(crate::browser::safe_text(text.trim()))
    }
}
fn utf8(path: &Path) -> Result<&str> {
    path.to_str()
        .ok_or_else(|| "Worktree creation requires UTF-8 paths.".into())
}
fn identity(path: &Path) -> Result<(u64, u64)> {
    let m = fs::metadata(path).map_err(|e| e.to_string())?;
    Ok((m.dev(), m.ino()))
}

pub async fn context(origin: &Path, commit: Option<&str>) -> Result<Context> {
    let version = git(origin, &["--version"]).await?;
    let mut numbers = version
        .split_whitespace()
        .nth(2)
        .unwrap_or("")
        .split('.')
        .filter_map(|n| n.parse::<u32>().ok());
    if (numbers.next().unwrap_or(0), numbers.next().unwrap_or(0)) < (2, 48) {
        return Err(
            "Creating portable worktrees requires Git 2.48 or newer. Browsing remains available."
                .into(),
        );
    }
    // Capability is checked independently of the reported version (vendor builds).
    let help = run(origin, &["worktree", "add", "-h"], 30).await?.1;
    if !help.contains("relative-paths") {
        return Err("This Git does not support relative worktree links.".into());
    }
    if git(origin, &["rev-parse", "--is-inside-work-tree"]).await? != "true" {
        return Err("Select an existing non-bare Git checkout.".into());
    }
    let source = PathBuf::from(git(origin, &["rev-parse", "--show-toplevel"]).await?)
        .canonicalize()
        .map_err(|e| e.to_string())?;
    if !git(&source, &["rev-parse", "--show-superproject-working-tree"])
        .await?
        .is_empty()
    {
        return Err("Creating worktrees of submodules is not supported yet.".into());
    }
    let repo = crate::git::discover(&source).await?;
    let first = repo
        .worktrees
        .first()
        .ok_or("Main checkout was not found.")?;
    if first.bare {
        return Err("Bare repositories are not supported for worktree creation.".into());
    }
    let main = first
        .path
        .canonicalize()
        .map_err(|_| "The main checkout is missing or inaccessible.")?;
    let common = repo.common_dir.canonicalize().map_err(|e| e.to_string())?;
    if !main
        .join(".git")
        .symlink_metadata()
        .is_ok_and(|m| m.is_dir() && !m.file_type().is_symlink())
        || main.join(".git").canonicalize().ok().as_ref() != Some(&common)
    {
        return Err("The main checkout must contain its Git data in .git; external Git directories are not supported yet.".into());
    }
    if let Some(oid) = commit {
        if ![40, 64].contains(&oid.len()) || !oid.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err("Select a valid history commit.".into());
        }
    }
    let revision = format!("{}^{{commit}}", commit.unwrap_or("HEAD"));
    let commit = git(&source, &["rev-parse", "--verify", &revision])
        .await
        .map_err(|_| "The selected checkout has no available starting commit.")?;
    let tree = git(&source, &["ls-tree", "-r", "-z", &commit]).await?;
    let index = git(&source, &["ls-files", "--stage", "-z"]).await?;
    let main_index = git(&main, &["ls-files", "--stage", "-z"]).await?;
    if [&tree, &index, &main_index]
        .iter()
        .any(|data| data.split('\0').any(|entry| entry.starts_with("160000 ")))
    {
        return Err(
            "Repositories containing submodules are not supported for worktree creation yet."
                .into(),
        );
    }
    let subject = git(&source, &["show", "-s", "--format=%s", &commit]).await?;
    Ok(Context {
        source,
        main,
        identity: identity(&common)?,
        common,
        commit,
        subject,
    })
}

pub fn suggested(context: &Context, branch: &str) -> PathBuf {
    context.main.join(".worktrees").join(branch)
}
pub fn destination(context: &Context, input: &str) -> Result<PathBuf> {
    let expanded = if let Some(tail) = input.strip_prefix("~/") {
        dirs::home_dir()
            .ok_or("Home directory unavailable.")?
            .join(tail)
    } else {
        PathBuf::from(input)
    };
    if input.chars().any(char::is_control) {
        return Err("Destination must not contain control characters.".into());
    }
    crate::config::absolute_path(&expanded, &context.source).map_err(|e| e.to_string())
}
fn check_path(path: &Path) -> Result<()> {
    for ancestor in path.ancestors() {
        match ancestor.symlink_metadata() {
            Ok(m) if m.file_type().is_symlink() => {
                return Err("Destination and its parents must not be symlinks.".into());
            }
            Ok(m) if !m.is_dir() => return Err("A destination parent is not a directory.".into()),
            Ok(_) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(format!("Cannot inspect destination: {e}")),
        }
    }
    if path.symlink_metadata().is_ok() {
        return Err("Destination already exists. Choose a new directory.".into());
    }
    Ok(())
}
fn exclude_bytes(common: &Path) -> Result<Option<Vec<u8>>> {
    let info = common.join("info");
    if info
        .symlink_metadata()
        .is_ok_and(|m| m.file_type().is_symlink() || !m.is_dir())
    {
        return Err("Git info directory must be an ordinary directory.".into());
    }
    let path = info.join("exclude");
    match path.symlink_metadata() {
        Ok(m) if !m.is_file() || m.file_type().is_symlink() => {
            Err("Git exclude file must be an ordinary file.".into())
        }
        Ok(_) => fs::read(path).map(Some).map_err(|e| e.to_string()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e.to_string()),
    }
}
fn pattern(relative: &Path) -> Result<String> {
    let mut result = String::from("/");
    for c in utf8(relative)?.chars() {
        if matches!(c, '\\' | '*' | '?' | '[' | ']' | ' ' | '!' | '#') {
            result.push('\\');
        }
        result.push(c);
    }
    result.push('/');
    Ok(result)
}
async fn ignored(root: &Path, destination: &Path) -> Result<bool> {
    let path = format!("{}/", utf8(destination)?);
    let (ok, error) = run(root, &["check-ignore", "-q", "--", &path], 30).await?;
    if !ok && !error.is_empty() {
        return Err(error);
    }
    Ok(ok)
}
pub async fn prepare(context: Context, branch: String, destination: PathBuf) -> Result<Plan> {
    let fresh = self::context(&context.source, Some(&context.commit)).await?;
    if fresh != context {
        return Err(
            "Repository changed since opening the dialog. Reopen it and review again.".into(),
        );
    }
    if branch.starts_with('-') || branch.starts_with('@') || branch.chars().any(char::is_control) {
        return Err("Choose a valid new branch name.".into());
    }
    git(&context.source, &["check-ref-format", "--branch", &branch])
        .await
        .map_err(|_| "Choose a valid new branch name.")?;
    let reference = format!("refs/heads/{branch}");
    git(&context.source, &["check-ref-format", &reference])
        .await
        .map_err(|_| "Choose a valid new branch name.")?;
    if run(
        &context.source,
        &["show-ref", "--verify", "--quiet", &reference],
        30,
    )
    .await?
    .0
    {
        return Err("That branch already exists. Choose a new branch name.".into());
    }
    check_path(&destination)?;
    let repo = crate::git::discover(&context.source).await?;
    if repo.worktrees.iter().any(|w| w.path == destination) {
        return Err("That destination is already registered as a worktree.".into());
    }
    if destination.starts_with(&context.common) {
        return Err("A worktree cannot be created inside Git's administrative directory.".into());
    }
    let mut parent = destination
        .parent()
        .ok_or("Destination needs a parent directory.")?;
    while !parent.exists() {
        parent = parent.parent().ok_or("No existing parent directory.")?;
    }
    let (inside, _) = run(parent, &["rev-parse", "--is-inside-work-tree"], 30).await?;
    let mut ignore_root = None;
    let mut exclusion = None;
    let mut exclude_before = None;
    if inside {
        let owner = crate::git::resolve_directory(parent).await?;
        if owner.common_dir.canonicalize().map_err(|e| e.to_string())? != context.common {
            return Err(
                "Destination is inside an unrelated repository. Choose another location.".into(),
            );
        }
        let root = PathBuf::from(git(parent, &["rev-parse", "--show-toplevel"]).await?);
        let relative = destination
            .strip_prefix(&root)
            .map_err(|_| "Destination ownership changed.")?;
        let ignored_relative =
            if destination.starts_with(context.main.join(".worktrees")) && root == context.main {
                Path::new(".worktrees")
            } else {
                relative
            };
        let files = git(&root, &["ls-files", "-z"]).await?;
        if files
            .split('\0')
            .filter(|p| !p.is_empty())
            .any(|p| Path::new(p).starts_with(ignored_relative))
        {
            return Err("The proposed worktree container contains tracked content. Choose another destination.".into());
        }
        if !ignored(&root, &destination).await? {
            exclusion = Some(pattern(ignored_relative)?);
            exclude_before = exclude_bytes(&context.common)?;
        }
        ignore_root = Some(root);
    }
    Ok(Plan {
        outside: !destination.starts_with(&context.main),
        context,
        branch,
        destination,
        exclusion,
        exclude_before,
        ignore_root,
    })
}

fn add_exclusion(plan: &Plan) -> Result<()> {
    let Some(pattern) = &plan.exclusion else {
        return Ok(());
    };
    let info = plan.context.common.join("info");
    fs::create_dir_all(&info).map_err(|e| e.to_string())?;
    let lock = info.join("exclude.lock");
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&lock)
        .map_err(|e| format!("Cannot lock local Git exclusions: {e}"))?;
    let result = (|| {
        use std::io::Write;
        if exclude_bytes(&plan.context.common)? != plan.exclude_before {
            return Err("Local exclusions changed since review. Review again.".into());
        }
        let mut bytes = plan.exclude_before.clone().unwrap_or_default();
        if !bytes
            .split(|b| *b == b'\n')
            .any(|line| line == pattern.as_bytes())
        {
            if !bytes.is_empty() && !bytes.ends_with(b"\n") {
                bytes.push(b'\n');
            }
            bytes.extend_from_slice(pattern.as_bytes());
            bytes.push(b'\n');
        }
        if let Ok(meta) = fs::metadata(info.join("exclude")) {
            file.set_permissions(meta.permissions())
                .map_err(|e| e.to_string())?;
        }
        file.write_all(&bytes)
            .and_then(|_| file.sync_all())
            .map_err(|e| e.to_string())?;
        if exclude_bytes(&plan.context.common)? != plan.exclude_before {
            return Err(
                "Local exclusions changed while preparing the update. Review again.".into(),
            );
        }
        fs::rename(&lock, info.join("exclude")).map_err(|e| e.to_string())
    })();
    if result.is_err() {
        let _ = fs::remove_file(lock);
    }
    result
}
async fn verify(plan: &Plan) -> Result<()> {
    let repo = crate::git::discover(&plan.context.main).await?;
    let reference = format!("refs/heads/{}", plan.branch);
    if !repo.worktrees.iter().any(|w| {
        w.path == plan.destination
            && w.head.as_deref() == Some(&plan.context.commit)
            && w.branch.as_deref() == Some(&reference)
    }) {
        return Err("Git registration does not match the reviewed worktree.".into());
    }
    if git(&plan.destination, &["rev-parse", "HEAD"]).await? != plan.context.commit {
        return Err("Created worktree HEAD differs from the reviewed commit.".into());
    }
    let link = fs::read_to_string(plan.destination.join(".git")).map_err(|e| e.to_string())?;
    let target = link
        .trim()
        .strip_prefix("gitdir: ")
        .ok_or("Invalid worktree Git link.")?;
    if Path::new(target).is_absolute() {
        return Err("Git did not create relative worktree links.".into());
    }
    let admin = plan
        .destination
        .join(target)
        .canonicalize()
        .map_err(|e| e.to_string())?;
    let reverse = fs::read_to_string(admin.join("gitdir")).map_err(|e| e.to_string())?;
    if Path::new(reverse.trim()).is_absolute()
        || admin.join(reverse.trim()).canonicalize().ok()
            != plan.destination.join(".git").canonicalize().ok()
    {
        return Err("Git's reverse worktree link was not verified.".into());
    }
    Ok(())
}
/// No rollback deletes user data; failures include fresh observations of possible partial state.
pub async fn execute(plan: Plan) -> Result<PathBuf> {
    let result = async {
        let fresh = prepare(plan.context.clone(), plan.branch.clone(), plan.destination.clone()).await?;
        if fresh.exclusion != plan.exclusion || fresh.exclude_before != plan.exclude_before || fresh.ignore_root != plan.ignore_root { return Err("Local ignore rules changed since review. Review again.".into()); }
        add_exclusion(&plan)?;
        if let Some(root) = &plan.ignore_root {
            if !ignored(root, &plan.destination).await? { return Err("A higher-priority ignore rule prevents ignoring this destination. Choose another destination or adjust that rule yourself.".into()); }
        }
        check_path(&plan.destination)?;
        let (ok, output) = run(&plan.context.source, &["worktree", "add", "--relative-paths", "-b", &plan.branch, utf8(&plan.destination)?, &plan.context.commit], 300).await?;
        if !ok { return Err(output); }
        verify(&plan).await?;
        Ok(plan.destination.clone())
    }.await;
    match result {
        Ok(path) => Ok(path),
        Err(error) => {
            let branch = run(
                &plan.context.main,
                &[
                    "show-ref",
                    "--verify",
                    "--quiet",
                    &format!("refs/heads/{}", plan.branch),
                ],
                30,
            )
            .await
            .map(|r| if r.0 { "exists" } else { "not found" })
            .unwrap_or("unknown");
            let registered = crate::git::discover(&plan.context.main)
                .await
                .map(|repo| {
                    if repo.worktrees.iter().any(|w| w.path == plan.destination) {
                        "yes"
                    } else {
                        "no"
                    }
                })
                .unwrap_or("unknown");
            Err(format!(
                "{}\n\nDestination: {}\nDestination exists: {}\nBranch {}: {branch}\nRegistered worktree: {registered}\n\nNo branches or directories were automatically deleted. Local exclusion changes and Git compatibility settings may remain. Inspect this state before retrying.",
                crate::browser::safe_text(&error),
                plan.destination.display(),
                plan.destination.symlink_metadata().is_ok(),
                plan.branch
            ))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn git_deadline_is_bounded() {
        let temp = tempfile::tempdir().unwrap();
        let before = std::time::Instant::now();
        let error = run(
            temp.path(),
            &["-c", "alias.tracr-slow=!sleep 30", "tracr-slow"],
            1,
        )
        .await
        .unwrap_err();
        assert!(error.contains("timed out"));
        assert!(before.elapsed() < Duration::from_secs(5));
    }
}
