//! Read-only Git discovery and graph access. Commands never invoke a shell.
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;
use tokio::process::Command;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Worktree {
    pub path: PathBuf,
    pub head: Option<String>,
    pub branch: Option<String>,
    pub bare: bool,
    pub locked: bool,
    pub prunable: bool,
}
#[derive(Clone, Debug)]
pub struct Repository {
    pub common_dir: PathBuf,
    pub git_dir: PathBuf,
    pub worktrees: Vec<Worktree>,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GraphRow {
    pub graph: String,
    pub graph_spans: Vec<GraphSpan>,
    pub oid: Option<String>,
    pub subject: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GraphSpan {
    pub text: String,
    pub color: Option<u8>,
}
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum RefKind {
    Local,
    Remote,
    Tag,
}
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct RefLabel {
    pub kind: RefKind,
    pub name: String,
}
pub type References = std::collections::BTreeMap<String, Vec<RefLabel>>;
pub fn parse_references(bytes: &[u8]) -> Result<References, String> {
    let mut refs = std::collections::BTreeMap::new();
    let mut peeled = std::collections::BTreeMap::new();
    for line in String::from_utf8_lossy(bytes).lines() {
        let (oid, name) = line.split_once(' ').ok_or("Malformed Git reference")?;
        if ![40, 64].contains(&oid.len()) || !oid.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err("Invalid reference object ID".into());
        }
        if let Some(name) = name.strip_suffix("^{}") {
            peeled.insert(name.to_owned(), oid.to_owned());
        } else {
            refs.insert(name.to_owned(), oid.to_owned());
        }
    }
    let mut result = References::new();
    for (name, oid) in refs {
        let kind_name = [
            ("refs/heads/", RefKind::Local),
            ("refs/remotes/", RefKind::Remote),
            ("refs/tags/", RefKind::Tag),
        ]
        .into_iter()
        .find_map(|(prefix, kind)| name.strip_prefix(prefix).map(|short| (kind, short)));
        if let Some((kind, short)) = kind_name {
            let target = peeled.get(&name).cloned().unwrap_or(oid);
            result.entry(target).or_default().push(RefLabel {
                kind,
                name: crate::browser::safe_text(short),
            });
        }
    }
    for labels in result.values_mut() {
        labels.sort();
        labels.dedup();
    }
    Ok(result)
}
pub async fn references(path: &Path) -> Result<References, String> {
    match read(path, &["show-ref", "--dereference"]).await {
        Ok(bytes) => parse_references(&bytes),
        Err(error) if error.is_empty() => Ok(References::new()),
        Err(error) => Err(error),
    }
}
fn graph_segments(bytes: &[u8]) -> Vec<GraphSpan> {
    let mut spans: Vec<GraphSpan> = Vec::new();
    let mut color = None;
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == 27 {
            i += 1;
            if bytes.get(i) == Some(&b'[') {
                i += 1;
                let start = i;
                while i < bytes.len() && !(0x40..=0x7e).contains(&bytes[i]) {
                    i += 1;
                }
                if bytes.get(i) == Some(&b'm') {
                    color = None;
                    for code in String::from_utf8_lossy(&bytes[start..i]).split(';') {
                        if let Ok(code @ 31..=36) = code.parse::<u8>() {
                            color = Some(code);
                        } else if code == "0" || code == "39" {
                            color = None;
                        }
                    }
                }
                i = (i + 1).min(bytes.len());
            } else if bytes.get(i) == Some(&b']') {
                i += 1;
                while i < bytes.len()
                    && bytes[i] != 7
                    && !(bytes[i] == 27 && bytes.get(i + 1) == Some(&b'\\'))
                {
                    i += 1;
                }
                i = (i + if bytes.get(i) == Some(&27) { 2 } else { 1 }).min(bytes.len());
            } else {
                i = (i + 1).min(bytes.len());
            }
            continue;
        }
        let byte = bytes[i];
        i += 1;
        // Native graph output is ASCII. Never retain terminal control bytes.
        if !b" *|/\\_-.".contains(&byte) {
            continue;
        }
        if let Some(span) = spans.last_mut().filter(|span| span.color == color) {
            span.text.push(byte as char);
        } else {
            spans.push(GraphSpan {
                text: (byte as char).to_string(),
                color,
            });
        }
    }
    spans
}

static READ_WORKERS: std::sync::Mutex<Vec<tokio::task::JoinHandle<()>>> =
    std::sync::Mutex::new(Vec::new());

/// Call after all discovery and history request tasks have been cancelled/joined.
pub async fn drain_reads() {
    let workers = std::mem::take(&mut *READ_WORKERS.lock().expect("Git worker registry"));
    for worker in workers {
        let _ = worker.await;
    }
}

async fn read(path: &Path, args: &[&str]) -> Result<Vec<u8>, String> {
    let mut command = Command::new("git");
    command
        .arg("--no-pager")
        .arg("-C")
        .arg(path)
        .args(args)
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_COMMON_DIR")
        .env_remove("GIT_INDEX_FILE")
        .env_remove("GIT_OBJECT_DIRECTORY")
        .env_remove("GIT_ALTERNATE_OBJECT_DIRECTORIES")
        .env("LC_ALL", "C")
        .env("GIT_OPTIONAL_LOCKS", "0")
        .env("GIT_NO_LAZY_FETCH", "1")
        .stdin(Stdio::null())
        .kill_on_drop(true);
    command.stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = command
        .spawn()
        .map_err(|e| format!("Could not run Git: {e}"))?;
    let mut stdout = child.stdout.take().expect("piped stdout");
    let mut stderr = child.stderr.take().expect("piped stderr");
    let (mut sender, receiver) = tokio::sync::oneshot::channel();
    // The worker owns the process independently of the requesting future, so
    // cancellation can kill AND reap it before terminal handoff.
    let worker = tokio::spawn(async move {
        use tokio::io::AsyncReadExt;
        let mut out = Vec::new();
        let mut err = Vec::new();
        let result = {
            let output = async {
                let (status, _, _) = tokio::try_join!(
                    child.wait(),
                    stdout.read_to_end(&mut out),
                    stderr.read_to_end(&mut err)
                )?;
                Ok::<_, std::io::Error>(status)
            };
            tokio::select! {
                _ = sender.closed() => None,
                result = tokio::time::timeout(Duration::from_secs(30), output) => Some(result),
            }
        };
        let result = match result {
            Some(Ok(Ok(status))) => Ok(std::process::Output {
                status,
                stdout: out,
                stderr: err,
            }),
            other => {
                let _ = child.kill().await;
                let _ = child.wait().await;
                match other {
                    None => return,
                    Some(Err(_)) => Err("Git command timed out".to_string()),
                    Some(Ok(Err(error))) => Err(format!("Could not read Git: {error}")),
                    _ => unreachable!(),
                }
            }
        };
        let _ = sender.send(result);
    });
    {
        let mut workers = READ_WORKERS.lock().expect("Git worker registry");
        workers.retain(|worker| !worker.is_finished());
        workers.push(worker);
    }
    let output = receiver
        .await
        .map_err(|_| "Git reader stopped".to_string())??;
    if !output.status.success() {
        return Err(crate::browser::safe_text(&String::from_utf8_lossy(
            &output.stderr,
        )));
    }
    Ok(output.stdout)
}

fn path_bytes(bytes: &[u8]) -> PathBuf {
    use std::os::unix::ffi::OsStringExt;
    std::ffi::OsString::from_vec(bytes.to_vec()).into()
}
fn output_path(mut bytes: Vec<u8>) -> PathBuf {
    if bytes.last() == Some(&b'\n') {
        bytes.pop();
    }
    path_bytes(&bytes)
}

pub fn parse_worktrees(bytes: &[u8]) -> Result<Vec<Worktree>, String> {
    let mut result = Vec::new();
    let mut current: Option<Worktree> = None;
    for field in bytes.split(|b| *b == 0) {
        if field.is_empty() {
            if let Some(item) = current.take() {
                result.push(item);
            }
        } else if let Some(path) = field.strip_prefix(b"worktree ") {
            if current.is_some() {
                return Err("Unterminated Git worktree record".into());
            }
            current = Some(Worktree {
                path: path_bytes(path),
                head: None,
                branch: None,
                bare: false,
                locked: false,
                prunable: false,
            });
        } else {
            let item = current.as_mut().ok_or("Git attribute without worktree")?;
            if let Some(head) = field.strip_prefix(b"HEAD ") {
                item.head = Some(String::from_utf8(head.to_vec()).map_err(|_| "Invalid Git HEAD")?);
            } else if let Some(branch) = field.strip_prefix(b"branch ") {
                item.branch = Some(String::from_utf8_lossy(branch).into_owned());
            } else if field == b"bare" {
                item.bare = true;
            } else if field == b"locked" || field.starts_with(b"locked ") {
                item.locked = true;
            } else if field == b"prunable" || field.starts_with(b"prunable ") {
                item.prunable = true;
            }
        }
    }
    if let Some(item) = current {
        result.push(item);
    }
    Ok(result)
}

#[derive(Clone, Debug)]
pub struct Directory {
    pub common_dir: PathBuf,
    pub git_dir: PathBuf,
}
pub async fn resolve_directory(path: &Path) -> Result<Directory, String> {
    let common_dir = output_path(
        read(
            path,
            &["rev-parse", "--path-format=absolute", "--git-common-dir"],
        )
        .await?,
    );
    let git_dir = output_path(read(path, &["rev-parse", "--absolute-git-dir"]).await?);
    Ok(Directory {
        common_dir,
        git_dir,
    })
}
pub async fn inventory(common_dir: &Path) -> Result<Repository, String> {
    let worktrees =
        parse_worktrees(&read(common_dir, &["worktree", "list", "--porcelain", "-z"]).await?)?;
    Ok(Repository {
        common_dir: common_dir.to_owned(),
        git_dir: common_dir.to_owned(),
        worktrees,
    })
}
pub async fn discover(path: &Path) -> Result<Repository, String> {
    let directory = resolve_directory(path).await?;
    inventory(&directory.common_dir).await
}

pub fn parse_graph(bytes: &[u8]) -> Result<Vec<GraphRow>, String> {
    bytes
        .split(|b| *b == b'\n')
        .filter(|line| !line.is_empty())
        .map(|line| {
            let fields: Vec<_> = line.split(|b| *b == 0).collect();
            let graph_spans = graph_segments(fields[0]);
            let graph = graph_spans.iter().map(|span| span.text.as_str()).collect();
            if fields.len() == 1 {
                return Ok(GraphRow {
                    graph,
                    graph_spans,
                    oid: None,
                    subject: String::new(),
                });
            }
            if fields.len() != 4
                || !fields[3].is_empty()
                || ![40, 64].contains(&fields[1].len())
                || !fields[1].iter().all(u8::is_ascii_hexdigit)
            {
                return Err("Malformed Git graph record".into());
            }
            Ok(GraphRow {
                graph,
                graph_spans,
                oid: Some(String::from_utf8_lossy(fields[1]).into_owned()),
                subject: crate::browser::safe_text(&String::from_utf8_lossy(fields[2])),
            })
        })
        .collect()
}

/// Expanding prefixes preserve lane context; independent --skip pages do not.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Head {
    pub oid: Option<String>,
    pub branch: Option<String>,
}
pub async fn head(path: &Path) -> Result<Head, String> {
    let branch = read(path, &["symbolic-ref", "--quiet", "HEAD"])
        .await
        .ok()
        .map(|bytes| {
            String::from_utf8_lossy(&bytes)
                .trim()
                .trim_start_matches("refs/heads/")
                .to_owned()
        });
    match read(path, &["rev-parse", "--verify", "HEAD^{commit}"]).await {
        Ok(bytes) => {
            let oid = String::from_utf8_lossy(&bytes).trim().to_owned();
            if ![40, 64].contains(&oid.len()) || !oid.bytes().all(|b| b.is_ascii_hexdigit()) {
                return Err("Invalid HEAD object ID".into());
            }
            Ok(Head {
                oid: Some(oid),
                branch,
            })
        }
        Err(error) => {
            if let Some(branch) = &branch {
                let name = format!("refs/heads/{branch}");
                let refs = read(path, &["for-each-ref", "--format=%(refname)", &name]).await?;
                if !String::from_utf8_lossy(&refs)
                    .lines()
                    .any(|line| line == name)
                {
                    return Ok(Head {
                        oid: None,
                        branch: Some(branch.clone()),
                    });
                }
            }
            Err(error)
        }
    }
}
pub async fn history(path: &Path, limit: usize) -> Result<Vec<GraphRow>, String> {
    history_at(path, limit, None).await
}
pub async fn history_at(
    path: &Path,
    limit: usize,
    head: Option<&str>,
) -> Result<Vec<GraphRow>, String> {
    let limit = format!("--max-count={}", limit.max(1));
    let mut args = vec![
        "-c",
        "log.graphColors=cyan,magenta,yellow,green,blue,red",
        "log",
        "--all",
        "--graph",
        "--topo-order",
        "--color=always",
        "--no-show-signature",
        "--no-notes",
        "--format=%x00%H%x00%s%x00",
        &limit,
    ];
    if let Some(oid) = head {
        if ![40, 64].contains(&oid.len()) || !oid.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err("Invalid HEAD object ID".into());
        }
        args.push(oid);
    }
    args.push("--");
    parse_graph(&read(path, &args).await?)
}

/// Bare repositories have no checkout root.
pub async fn checkout_path(path: &Path) -> Result<Option<PathBuf>, String> {
    if read(path, &["rev-parse", "--is-bare-repository"]).await? == b"true\n" {
        return Ok(None);
    }
    Ok(Some(output_path(
        read(path, &["rev-parse", "--show-toplevel"]).await?,
    )))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn worktrees_keep_unusual_paths_and_states() {
        let rows = parse_worktrees(b"worktree /tmp/with\nnewline\0HEAD abc\0detached\0locked reason\0\0worktree /tmp/bare\0bare\0\0").unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].path, PathBuf::from("/tmp/with\nnewline"));
        assert!(rows[0].locked);
        assert!(rows[1].bare);
    }
    #[test]
    fn graph_preserves_connectors_and_rejects_bad_framing() {
        let input = format!("* \0{}\0hello\0\n|\\\n", "a".repeat(40));
        let rows = parse_graph(input.as_bytes()).unwrap();
        assert_eq!(rows.len(), 2);
        assert!(rows[0].oid.is_some());
        assert!(rows[1].oid.is_none());
        assert!(parse_graph(b"* \0bad\0subject\0\n").is_err());
    }
    #[test]
    fn references_are_typed_sorted_and_peeled() {
        let commit = "a".repeat(40);
        let tag = "b".repeat(40);
        let data = format!(
            "{commit} refs/remotes/origin/main\n{tag} refs/tags/v1\n{commit} refs/tags/v1^{{}}\n{commit} refs/heads/z\n{commit} refs/heads/a,branch\n{commit} refs/tags/light\n{commit} refs/stash\n"
        );
        let refs = parse_references(data.as_bytes()).unwrap();
        assert_eq!(refs.len(), 1);
        assert_eq!(
            refs[&commit]
                .iter()
                .map(|r| r.name.as_str())
                .collect::<Vec<_>>(),
            vec!["a,branch", "z", "origin/main", "light", "v1"]
        );
        assert_eq!(refs[&commit][2].kind, RefKind::Remote);
        assert_eq!(refs[&commit][3].kind, RefKind::Tag);
        assert!(parse_references(b"").unwrap().is_empty());
        assert!(parse_references(b"oops refs/heads/main").is_err());
    }
    #[test]
    fn graph_colors_are_parsed_without_terminal_controls() {
        let data = format!(
            "\x1b[36m|\x1b[m \x1b[35m*\x1b[m \0{}\0unicode 界 é\0\n\x1b]0;bad title\x07\x1b[2J|/\n",
            "a".repeat(40)
        );
        let rows = parse_graph(data.as_bytes()).unwrap();
        assert_eq!(rows[0].graph, "| * ");
        assert_eq!(rows[0].graph_spans[0].color, Some(36));
        assert_eq!(rows[0].subject, "unicode 界 é");
        assert_eq!(rows[1].graph, "|/");
        assert!(!rows.iter().any(|row| row.graph.contains('\x1b')));
    }
}
