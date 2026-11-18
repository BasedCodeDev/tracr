//! One-shot, passive activity inventory for command-line consumers.
use crate::{
    config::SourceConfig,
    domain::{RuntimeStatus, SessionKey},
    git,
    providers::codex_live::LiveClient,
};
use futures_util::{StreamExt, stream};
use serde::Serialize;
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

#[derive(Serialize)]
pub struct Agent {
    pub key: SessionKey,
    pub title: String,
    pub cwd: PathBuf,
    pub status: RuntimeStatus,
}
#[derive(Serialize)]
pub struct Worktree {
    pub path: PathBuf,
    pub branch: Option<String>,
    pub head: Option<String>,
    pub bare: bool,
    pub locked: bool,
    pub prunable: bool,
    pub agents: Vec<Agent>,
}
#[derive(Serialize)]
pub struct Source {
    pub source: crate::domain::SourceId,
    pub complete: bool,
    pub errors: Vec<String>,
}
#[derive(Serialize)]
pub struct Report {
    pub schema_version: u8,
    pub repository: Option<PathBuf>,
    pub worktrees: Vec<Worktree>,
    pub complete: bool,
    pub error: Option<String>,
    pub sources: Vec<Source>,
}
impl Report {
    pub fn failure(error: String) -> Self {
        Self {
            schema_version: 1,
            repository: None,
            worktrees: vec![],
            complete: false,
            error: Some(error),
            sources: vec![],
        }
    }
    pub fn print(&self, text: bool) -> Result<(), serde_json::Error> {
        if !text {
            println!("{}", serde_json::to_string_pretty(self)?);
            return Ok(());
        }
        println!("BRANCH\tWORKTREE\tAGENTS");
        for tree in &self.worktrees {
            let agents = tree
                .agents
                .iter()
                .map(|agent| {
                    format!(
                        "{}/{}/{}: {} ({})",
                        agent.key.provider.0,
                        agent.key.source_id.0,
                        agent.key.native_id,
                        agent.title,
                        agent.status.label().unwrap_or("Unknown")
                    )
                })
                .collect::<Vec<_>>()
                .join("; ");
            println!(
                "{}\t{}\t{}",
                crate::browser::safe_text(tree.branch.as_deref().unwrap_or(if tree.bare {
                    "(bare)"
                } else {
                    "(detached)"
                })),
                crate::browser::safe_text(&tree.path.to_string_lossy()),
                if agents.is_empty() {
                    "No agents observed".into()
                } else {
                    crate::browser::safe_text(&agents)
                }
            );
        }
        if !self.complete {
            eprintln!("tracr: incomplete activity snapshot");
        }
        for error in self
            .error
            .iter()
            .chain(self.sources.iter().flat_map(|s| &s.errors))
        {
            eprintln!("tracr: {}", crate::browser::safe_text(error));
        }
        Ok(())
    }
}
fn canonical(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_owned())
}
async fn owner(path: &Path) -> Result<Option<(PathBuf, PathBuf)>, String> {
    if !path.is_absolute() || !path.is_dir() {
        return Err(format!(
            "Session directory is unavailable: {}",
            path.display()
        ));
    }
    let directory = match git::resolve_directory(path).await {
        Ok(directory) => directory,
        Err(error) if error.starts_with("fatal: not a git repository") => return Ok(None),
        Err(error) => return Err(error),
    };
    Ok(git::checkout_path(path)
        .await?
        .map(|root| (canonical(&directory.common_dir), canonical(&root))))
}
pub async fn collect(path: &Path, binary: &Path, sources: Vec<SourceConfig>) -> Report {
    let repo = match git::discover(path).await {
        Ok(repo) => repo,
        Err(error) => return Report::failure(error),
    };
    let common = canonical(&repo.common_dir);
    let mut report = Report {
        schema_version: 1,
        repository: Some(common.clone()),
        worktrees: repo
            .worktrees
            .into_iter()
            .map(|w| Worktree {
                path: w.path,
                branch: w.branch,
                head: w.head.filter(|oid| !oid.bytes().all(|b| b == b'0')),
                bare: w.bare,
                locked: w.locked,
                prunable: w.prunable,
                agents: vec![],
            })
            .collect(),
        complete: true,
        error: None,
        sources: vec![],
    };
    report.worktrees.sort_by(|a, b| a.path.cmp(&b.path));
    let roots: BTreeMap<_, _> = report
        .worktrees
        .iter()
        .enumerate()
        .filter(|(_, w)| !w.bare)
        .map(|(i, w)| (canonical(&w.path), i))
        .collect();
    let snapshots = stream::iter(sources.into_iter().map(|source| async move {
        let result = async {
            let mut client = LiveClient::connect(binary, source.clone()).await?;
            client.session_snapshot().await
        }
        .await;
        (source.id, result)
    }))
    .buffered(4)
    .collect::<Vec<_>>()
    .await;
    let mut ownership = BTreeMap::new();
    for (source, snapshot) in snapshots {
        let mut diagnostic = Source {
            source,
            complete: true,
            errors: vec![],
        };
        match snapshot {
            Err(error) => diagnostic.errors.push(error),
            Ok(sessions) => {
                for (key, session) in sessions {
                    let Some(cwd) = session.cwd else {
                        diagnostic
                            .errors
                            .push(format!("{}: missing session directory", key.native_id));
                        continue;
                    };
                    if !ownership.contains_key(&cwd) {
                        ownership.insert(cwd.clone(), owner(&cwd).await);
                    }
                    match &ownership[&cwd] {
                        Ok(Some((repository, root))) if repository == &common => {
                            if let Some(&index) = roots.get(root) {
                                report.worktrees[index].agents.push(Agent {
                                    key,
                                    title: session.title,
                                    cwd,
                                    status: session.status,
                                });
                            } else {
                                diagnostic.errors.push(format!(
                                    "{}: checkout missing from inventory",
                                    key.native_id
                                ));
                            }
                        }
                        Err(error) => diagnostic
                            .errors
                            .push(format!("{}: {error}", key.native_id)),
                        _ => {}
                    }
                }
            }
        }
        diagnostic.complete = diagnostic.errors.is_empty();
        report.complete &= diagnostic.complete;
        report.sources.push(diagnostic);
    }
    for tree in &mut report.worktrees {
        tree.agents.sort_by(|a, b| a.key.cmp(&b.key));
    }
    report
}
