//! Provider-independent browser state and generation-scoped discovery.
use crate::{
    config::SourceConfig,
    domain::*,
    providers::{SessionProvider, codex::CodexProvider, fixture::FixtureProvider},
};
use serde::Serialize;
use std::{
    collections::{BTreeMap, BTreeSet, HashSet},
    path::PathBuf,
    sync::Arc,
};
use tokio::{
    sync::{Semaphore, mpsc},
    task::JoinHandle,
};

#[derive(Clone)]
pub enum SourceSpec {
    Codex {
        binary: PathBuf,
        source: SourceConfig,
    },
    Fixture(PathBuf),
}
impl SourceSpec {
    /// A configured source that can launch a fresh session.
    pub fn creation_source_id(&self) -> Option<&SourceId> {
        match self {
            Self::Codex { source, .. } => Some(&source.id),
            Self::Fixture(_) => None,
        }
    }

    pub fn prepare_new(&self, cwd: &std::path::Path) -> Result<LaunchCommand, String> {
        match self {
            Self::Codex { binary, source } => {
                crate::providers::codex::prepare_new(binary, source, cwd).map_err(|e| e.to_string())
            }
            Self::Fixture(_) => Err("Fixture data is browse-only.".into()),
        }
    }
    pub fn label(&self) -> String {
        match self {
            Self::Codex { source, .. } => source.home.display().to_string(),
            Self::Fixture(path) => path.display().to_string(),
        }
    }
    async fn open(&self) -> Result<Provider, String> {
        match self {
            Self::Codex { binary, source } => CodexProvider::connect(binary, source.clone())
                .await
                .map(|provider| Provider::Codex(Box::new(provider)))
                .map_err(|e| e.to_string()),
            Self::Fixture(path) => FixtureProvider::open(path)
                .map(Provider::Fixture)
                .map_err(|e| e.to_string()),
        }
    }
}
pub enum Provider {
    Codex(Box<CodexProvider>),
    Fixture(FixtureProvider),
}
impl Provider {
    pub async fn prepare_resume(&self, session: &SessionSummary) -> Result<ResumeCommand, String> {
        match self {
            Self::Codex(p) => p.prepare_resume(session).await,
            Self::Fixture(p) => p.prepare_resume(session).await,
        }
        .map_err(|e| e.to_string())
    }

    async fn page(
        &self,
        cursor: Option<crate::providers::Cursor>,
    ) -> Result<crate::providers::SessionPage, String> {
        match self {
            Self::Codex(p) => p.list(cursor).await,
            Self::Fixture(p) => p.list(cursor).await,
        }
        .map_err(|e| e.to_string())
    }
    async fn read(&self, key: &SessionKey) -> Result<Option<SessionSummary>, String> {
        match self {
            Self::Codex(p) => p.read(key).await,
            Self::Fixture(p) => p.read(key).await,
        }
        .map_err(|e| e.to_string())
    }
    async fn close(&self) {
        if let Self::Codex(p) = self {
            p.close().await;
        }
    }
}

pub enum Update {
    Live {
        generation: u64,
        source: usize,
        statuses: BTreeMap<SessionKey, RuntimeStatus>,
        error: Option<String>,
    },
    Ready {
        generation: u64,
        source: usize,
        provider: Arc<Provider>,
    },
    Page {
        generation: u64,
        source: usize,
        sessions: Vec<SessionSummary>,
        warnings: Vec<String>,
    },
    Finished {
        generation: u64,
        source: usize,
        error: Option<String>,
    },
    Folder {
        generation: u64,
        path: PathBuf,
        status: FolderStatus,
    },
    Detail {
        generation: u64,
        key: SessionKey,
        value: Box<Result<Option<SessionSummary>, String>>,
    },
}
impl Update {
    fn generation(&self) -> u64 {
        match self {
            Self::Live { generation, .. }
            | Self::Ready { generation, .. }
            | Self::Page { generation, .. }
            | Self::Finished { generation, .. }
            | Self::Folder { generation, .. }
            | Self::Detail { generation, .. } => *generation,
        }
    }
}

#[derive(Default)]
pub struct SourceState {
    pub live: BTreeMap<SessionKey, RuntimeStatus>,
    pub live_error: Option<String>,
    pub live_checked: bool,
    committed: BTreeMap<SessionKey, SessionSummary>,
    staging: BTreeMap<SessionKey, SessionSummary>,
    pub loading: bool,
    pub error: Option<String>,
    pub warnings: Vec<String>,
    pub provider: Option<Arc<Provider>>,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FolderFilter {
    All,
    Unlinked,
    Unknown,
    Path(PathBuf),
}
#[derive(Clone, Debug)]
pub struct FolderRow {
    pub filter: FolderFilter,
    pub label: String,
    pub depth: usize,
    pub count: usize,
}
#[derive(Serialize)]
pub struct SourceDiagnostic {
    pub source: String,
    pub complete: bool,
    pub error: Option<String>,
    pub warnings: Vec<String>,
}

pub struct Browser {
    pub ownership: BTreeMap<PathBuf, crate::git_discovery::Ownership>,
    pub git_generation: u64,
    retired_repositories: BTreeSet<PathBuf>,
    repository_candidates: BTreeMap<PathBuf, BTreeSet<PathBuf>>,
    pub repository_errors: BTreeMap<PathBuf, String>,
    pub repositories: BTreeMap<PathBuf, crate::git::Repository>,
    worktree_order: BTreeMap<PathBuf, (u64, BTreeMap<String, usize>)>,
    remembered_sessions: BTreeMap<PathBuf, SessionKey>,
    pub state_path: Option<PathBuf>,
    pub state_warning: Option<String>,
    pub generation: u64,
    pub sources: Vec<SourceState>,
    pub specs: Vec<SourceSpec>,
    pub folders: BTreeMap<PathBuf, FolderStatus>,
    pub folder: FolderFilter,
    pub initial_folder: Option<PathBuf>,
    pub expanded: BTreeSet<PathBuf>,
    pub query: String,
    pub include_archived: bool,
    pub selected: Option<SessionKey>,
    pub details: BTreeMap<SessionKey, Result<Option<SessionSummary>, String>>,
}
impl Browser {
    pub fn new(specs: Vec<SourceSpec>) -> Self {
        Self {
            ownership: BTreeMap::new(),
            git_generation: 0,
            retired_repositories: BTreeSet::new(),
            repository_candidates: BTreeMap::new(),
            repository_errors: BTreeMap::new(),
            repositories: BTreeMap::new(),
            worktree_order: BTreeMap::new(),
            remembered_sessions: BTreeMap::new(),
            state_path: None,
            state_warning: None,
            generation: 0,
            sources: (0..specs.len()).map(|_| SourceState::default()).collect(),
            specs,
            folders: BTreeMap::new(),
            folder: FolderFilter::All,
            initial_folder: None,
            expanded: BTreeSet::from([PathBuf::from("/")]),
            query: String::new(),
            include_archived: false,
            selected: None,
            details: BTreeMap::new(),
        }
    }
    pub fn select_folder(&mut self, next: FolderFilter) {
        if let (FolderFilter::Path(path), Some(key)) = (&self.folder, &self.selected) {
            self.remembered_sessions.insert(path.clone(), key.clone());
        }
        self.selected = match &next {
            FolderFilter::Path(p) => self.remembered_sessions.get(p).cloned(),
            _ => None,
        };
        self.folder = next;
        self.reconcile(0);
    }
    pub fn install_repository(&mut self, repo: crate::git::Repository) {
        if !self.repositories.contains_key(&repo.common_dir) {
            if let Some(main) = repo.worktrees.first() {
                self.expanded
                    .extend(main.path.ancestors().map(PathBuf::from));
            }
        }
        self.repositories.insert(repo.common_dir.clone(), repo);
    }
    pub fn apply_inventory(
        &mut self,
        generation: u64,
        id: PathBuf,
        result: Result<crate::git::Repository, String>,
    ) -> Vec<PathBuf> {
        if generation != self.git_generation || self.retired_repositories.contains(&id) {
            return vec![];
        }
        let repo = match result {
            Ok(repo) => repo,
            Err(error) => {
                self.repository_errors.insert(id, error);
                return vec![];
            }
        };
        self.repository_errors.remove(&id);
        let removed: Vec<_> = self
            .repositories
            .get(&id)
            .into_iter()
            .flat_map(|r| &r.worktrees)
            .filter(|old| !repo.worktrees.iter().any(|new| new.path == old.path))
            .map(|w| w.path.clone())
            .collect();
        let invalid: Vec<_> = self.ownership.iter().filter(|(_, record)| matches!(record.effective(),
            Some(crate::git_discovery::Outcome::Repository { repository, root: Some(root) }) if repository.common_dir == id && removed.contains(root)))
            .map(|(path,_)| path.clone()).collect();
        for path in &invalid {
            self.ownership.remove(path);
        }
        self.install_repository(repo);
        invalid
    }
    /// Keep unrelated repositories current during a native metadata refresh.
    pub fn invalidate_git_repository(
        &mut self,
        id: &std::path::Path,
        topology: bool,
    ) -> Vec<PathBuf> {
        self.repository_errors
            .insert(id.to_owned(), "Refreshing worktree information".into());
        let mut paths = Vec::new();
        for (path, record) in &mut self.ownership {
            if (topology || record.stale())
                && matches!(record.effective(), Some(crate::git_discovery::Outcome::Repository { repository, .. }) if repository.common_dir == id)
            {
                paths.push(path.clone());
                record.previous = record.effective().cloned();
                record.outcome = None;
            }
        }
        paths
    }
    pub fn invalidate_git(&mut self) {
        self.git_generation += 1;
        self.retired_repositories.clear();
        for id in self.repositories.keys() {
            self.repository_errors
                .insert(id.clone(), "Refreshing worktree information".into());
        }
        for record in self.ownership.values_mut() {
            record.previous = record.effective().cloned();
            record.outcome = None;
            record.generation = self.git_generation;
        }
    }
    pub fn apply_ownership(
        &mut self,
        generation: u64,
        path: PathBuf,
        outcome: crate::git_discovery::Outcome,
    ) {
        if generation != self.git_generation {
            return;
        }
        if let crate::git_discovery::Outcome::Repository { repository, .. } = &outcome {
            if self.retired_repositories.contains(&repository.common_dir) {
                return;
            }
            self.repository_candidates
                .entry(repository.common_dir.clone())
                .or_default()
                .insert(path.clone());
        }
        let previous = self
            .ownership
            .get(&path)
            .and_then(|r| r.effective())
            .cloned();
        self.ownership.insert(
            path,
            crate::git_discovery::Ownership {
                generation,
                outcome: Some(outcome),
                previous,
            },
        );
        self.reconcile(0);
    }
    pub fn retire_repositories(&mut self) -> Vec<PathBuf> {
        self.retire_repositories_except(&BTreeSet::new())
    }
    pub fn retire_repositories_except(&mut self, pending: &BTreeSet<PathBuf>) -> Vec<PathBuf> {
        use crate::git_discovery::Outcome;
        let retired: Vec<_> = self.repositories.keys().filter(|id| {
            !pending.contains(*id) && self.repository_candidates.get(*id).is_some_and(|paths| !paths.is_empty() && paths.iter().all(|path| {
                self.ownership.get(path).is_some_and(|record| record.generation == self.git_generation && match &record.outcome {
                    Some(Outcome::NotRepository) => true,
                    Some(Outcome::Repository { repository, .. }) => &repository.common_dir != *id,
                    _ => false,
                })
            })) && !self.ownership.values().any(|r| matches!(&r.outcome, Some(Outcome::Repository { repository, .. }) if &repository.common_dir == *id))
        }).cloned().collect();
        for id in &retired {
            self.repositories.remove(id);
            self.worktree_order.remove(id);
            self.repository_errors.remove(id);
            self.repository_candidates.remove(id);
            self.retired_repositories.insert(id.clone());
        }
        retired
    }
    pub fn navigation_ancestors(&self) -> Vec<FolderFilter> {
        let mut stack: Vec<(usize, FolderFilter)> = Vec::new();
        for row in self.folder_rows() {
            while stack.last().is_some_and(|(depth, _)| *depth >= row.depth) {
                stack.pop();
            }
            if row.filter == self.folder {
                return stack.into_iter().rev().map(|(_, filter)| filter).collect();
            }
            stack.push((row.depth, row.filter));
        }
        vec![]
    }
    pub fn recover_navigation(&mut self, ancestors: &[FolderFilter], selected: Option<SessionKey>) {
        let missing = matches!(&self.folder, FolderFilter::Path(path)
            if matches!(self.folders.get(path), Some(FolderStatus::Unlinked))
            && !self.repositories.values().any(|repo| repo.worktrees.iter().any(|worktree| &worktree.path == path)));
        let mut rows = self.folder_rows();
        if missing || !rows.iter().any(|r| r.filter == self.folder) {
            if let FolderFilter::Path(path) = &self.folder {
                self.expanded.extend(path.ancestors().map(PathBuf::from));
                rows = self.folder_rows();
            }
            if missing || !rows.iter().any(|r| r.filter == self.folder) {
                if let Some(target) = ancestors
                    .iter()
                    .find(|a| rows.iter().any(|r| &r.filter == *a))
                {
                    self.folder = target.clone();
                } else {
                    self.folder = if missing {
                        FolderFilter::Unlinked
                    } else {
                        rows.first()
                            .map(|r| r.filter.clone())
                            .unwrap_or(FolderFilter::Unknown)
                    };
                }
            }
        }
        self.selected = selected;
        self.reconcile(0);
    }
    fn checkout_root(&self, path: &std::path::Path) -> Option<&std::path::Path> {
        match self.ownership.get(path).and_then(|r| r.effective()) {
            Some(crate::git_discovery::Outcome::Repository { root, .. }) => root.as_deref(),
            _ => None,
        }
    }
    pub fn scope_label(&self) -> String {
        "Sessions".into()
    }
    fn subtree_matches(&self, session: &SessionSummary, filter: &FolderFilter) -> bool {
        let Some(cwd) = &session.cwd else {
            return self.matches_folder(session, filter);
        };
        let linked = self.repositories.values().find_map(|repo| {
            repo.worktrees
                .iter()
                .skip(1)
                .filter(|w| cwd.starts_with(&w.path))
                .max_by_key(|w| w.path.components().count())
                .map(|w| (repo, w))
        });
        match filter {
            FolderFilter::Path(path) => {
                if let Some((repo, worktree)) = linked {
                    if path.starts_with(&worktree.path) {
                        return cwd.starts_with(path);
                    }
                    return repo
                        .worktrees
                        .first()
                        .is_some_and(|main| main.path.starts_with(path));
                }
                cwd.starts_with(path)
            }
            _ => self.matches_folder(session, filter),
        }
    }
    /// Select an absolute, lexically normalized folder before discovery begins.
    pub fn open_folder(&mut self, path: PathBuf) {
        self.expanded.extend(path.ancestors().map(|p| p.to_owned()));
        self.select_folder(FolderFilter::Path(path.clone()));
        self.initial_folder = Some(path);
        self.reconcile(0);
    }

    pub fn begin_refresh(&mut self) {
        self.generation += 1;
        self.folders.clear();
        self.details.clear();
        for source in &mut self.sources {
            // Preserve partial first-load observations through a cancelled refresh.
            source.committed.append(&mut source.staging);
            source.live.clear();
            source.live_checked = false;
            source.live_error = None;
            source.loading = true;
            source.error = None;
            source.warnings.clear();
            source.provider = None;
        }
    }
    pub fn resume_provider(&self, key: &SessionKey) -> Result<Arc<Provider>, String> {
        let source = self
            .sources
            .iter()
            .find(|s| s.committed.contains_key(key) || s.staging.contains_key(key))
            .ok_or("Session is no longer in the inventory; refresh first.")?;
        if source.loading {
            return Err("Wait for this source to finish loading before resuming.".into());
        }
        source
            .provider
            .clone()
            .ok_or_else(|| "Source is unavailable; refresh before resuming.".into())
    }
    pub fn mode_label(&self) -> &'static str {
        if self
            .specs
            .iter()
            .all(|s| matches!(s, SourceSpec::Fixture(_)))
        {
            "Fixture data"
        } else {
            "Live Codex sessions"
        }
    }
    pub fn loading(&self) -> bool {
        self.sources.iter().any(|s| s.loading)
    }
    pub fn diagnostics(&self) -> Vec<SourceDiagnostic> {
        self.sources
            .iter()
            .zip(&self.specs)
            .map(|(s, spec)| SourceDiagnostic {
                source: spec.label(),
                complete: !s.loading && s.error.is_none() && s.warnings.is_empty(),
                error: s.error.clone(),
                warnings: s.warnings.clone(),
            })
            .collect()
    }
    pub fn runtime_status(&self, key: &SessionKey) -> RuntimeStatus {
        for (source, spec) in self.sources.iter().zip(&self.specs) {
            if let Some(status) = source.live.get(key) {
                return *status;
            }
            if matches!(spec, SourceSpec::Codex { source: config, .. }
                if key.provider.0 == "codex" && config.id == key.source_id)
            {
                return if source.live_checked {
                    RuntimeStatus::Unloaded
                } else {
                    RuntimeStatus::Unknown
                };
            }
        }
        RuntimeStatus::Unknown
    }
    pub fn folder_activity(&self, filter: &FolderFilter) -> RuntimeStatus {
        let mut status = RuntimeStatus::Unknown;
        for session in self
            .all()
            .into_iter()
            .filter(|s| self.include_archived || !s.archived)
        {
            let matches = self.subtree_matches(&session, filter);
            if !matches {
                continue;
            }
            match self.runtime_status(&session.key) {
                waiting @ RuntimeStatus::Waiting { .. } => return waiting,
                RuntimeStatus::Running => status = RuntimeStatus::Running,
                RuntimeStatus::Idle if status != RuntimeStatus::Running => {
                    status = RuntimeStatus::Idle
                }
                _ => {}
            }
        }
        status
    }
    pub fn all(&self) -> Vec<SessionSummary> {
        let mut records = BTreeMap::new();
        for source in &self.sources {
            for session in source.committed.values().chain(source.staging.values()) {
                records.insert(session.key.clone(), session.clone());
            }
        }
        records.into_values().collect()
    }
    pub fn visible(&self) -> Vec<SessionSummary> {
        self.matching_sessions(&self.query, !self.query.trim().is_empty())
    }
    pub fn search_results(&self, query: &str) -> Vec<SessionSummary> {
        self.matching_sessions(query, true)
    }
    fn matching_sessions(&self, query: &str, searching: bool) -> Vec<SessionSummary> {
        let terms: Vec<String> = query.split_whitespace().map(str::to_lowercase).collect();
        let mut sessions: Vec<_> = self
            .all()
            .into_iter()
            .filter(|s| {
                (self.include_archived || !s.archived)
                    && if searching {
                        let text = format!(
                            "{} {} {} {} {}",
                            s.title,
                            s.key.native_id,
                            s.cwd
                                .as_ref()
                                .map(|p| p.to_string_lossy())
                                .unwrap_or_default(),
                            s.key.provider.0,
                            s.git
                                .as_ref()
                                .and_then(|g| g.branch.as_deref())
                                .unwrap_or("")
                        )
                        .to_lowercase();
                        terms.iter().all(|term| text.contains(term))
                    } else {
                        self.matches_folder(s, &self.folder)
                    }
            })
            .collect();
        crate::app::visible_sessions(&mut sessions, self.include_archived);
        sessions
    }
    fn matches_folder(&self, session: &SessionSummary, folder: &FolderFilter) -> bool {
        match folder {
            FolderFilter::All => true,
            FolderFilter::Unlinked => session
                .cwd
                .as_ref()
                .is_some_and(|p| self.folders.get(p) == Some(&FolderStatus::Unlinked)),
            FolderFilter::Unknown => session.cwd.as_ref().is_none_or(|p| !p.is_absolute()),
            FolderFilter::Path(path) => session.cwd.as_ref().is_some_and(|p| {
                if self.checkout_root(path) == Some(path.as_path()) {
                    self.checkout_root(p) == Some(path.as_path())
                } else {
                    p == path
                }
            }),
        }
    }
    /// Remember graph order per repository, including while another repository is viewed.
    pub fn order_worktrees<'a>(
        &mut self,
        repository: PathBuf,
        revision: u64,
        commits: impl Iterator<Item = &'a str>,
    ) {
        if self
            .worktree_order
            .get(&repository)
            .is_some_and(|(seen, _)| *seen == revision)
        {
            return;
        }
        let ranks = commits
            .enumerate()
            .map(|(rank, oid)| (oid.to_owned(), rank))
            .collect();
        self.worktree_order.insert(repository, (revision, ranks));
    }

    pub fn folder_rows(&self) -> Vec<FolderRow> {
        let sessions: Vec<_> = self
            .all()
            .into_iter()
            .filter(|s| self.include_archived || !s.archived)
            .collect();
        let count = |f: &FolderFilter| {
            sessions
                .iter()
                // Tree totals include descendants; the session pane uses exact matches.
                .filter(|s| self.subtree_matches(s, f))
                .count()
        };
        let mut rows = vec![
            FolderRow {
                filter: FolderFilter::Unlinked,
                label: "Unlinked folders".into(),
                depth: 0,
                count: count(&FolderFilter::Unlinked),
            },
            FolderRow {
                filter: FolderFilter::Unknown,
                label: "Unknown folders".into(),
                depth: 0,
                count: count(&FolderFilter::Unknown),
            },
        ];
        let mut paths = BTreeSet::new();
        if let Some(path) = &self.initial_folder {
            paths.extend(path.ancestors().map(|p| p.to_owned()));
        }
        for session in &sessions {
            if let Some(path) = session.cwd.as_ref().filter(|p| p.is_absolute()) {
                paths.extend(
                    path.ancestors()
                        .filter(|p| !p.as_os_str().is_empty())
                        .map(|p| p.to_owned()),
                );
            }
        }
        for repo in self.repositories.values() {
            if let Some(main) = repo.worktrees.first() {
                paths.extend(main.path.ancestors().map(PathBuf::from));
            }
            for worktree in &repo.worktrees {
                paths.insert(worktree.path.clone());
            }
        }
        let all_paths = paths.clone();
        let linked: Vec<_> = self
            .repositories
            .values()
            .flat_map(|r| r.worktrees.iter().skip(1))
            .collect();
        for path in paths {
            if linked.iter().any(|w| path.starts_with(&w.path)) {
                continue;
            }
            if path
                .ancestors()
                .skip(1)
                .filter(|p| !p.as_os_str().is_empty())
                .any(|p| !self.expanded.contains(p))
            {
                continue;
            }
            let status = match self.folders.get(&path) {
                Some(FolderStatus::Unlinked) => " [unlinked]",
                Some(FolderStatus::Inaccessible { .. }) => " [inaccessible]",
                _ => "",
            };
            let filter = FolderFilter::Path(path.clone());
            rows.push(FolderRow {
                count: count(&filter),
                filter,
                depth: path.components().count().saturating_sub(1),
                label: format!(
                    "{}{status}",
                    path.file_name()
                        .unwrap_or(path.as_os_str())
                        .to_string_lossy()
                ),
            });
            for repo in self.repositories.values().filter(|r| {
                r.worktrees.first().is_some_and(|w| w.path == path) && r.worktrees.len() > 1
            }) {
                if !self.expanded.contains(&path) {
                    continue;
                }
                let depth = path.components().count();
                let mut worktrees: Vec<_> = repo.worktrees.iter().skip(1).collect();
                if let Some((_, ranks)) = self.worktree_order.get(&repo.common_dir) {
                    // Stable sorting preserves inventory order for shared or unloaded heads.
                    worktrees.sort_by_key(|worktree| {
                        worktree
                            .head
                            .as_ref()
                            .and_then(|head| ranks.get(head))
                            .copied()
                            .unwrap_or(usize::MAX)
                    });
                }
                for worktree in worktrees {
                    let filter = FolderFilter::Path(worktree.path.clone());
                    rows.push(FolderRow {
                        count: count(&filter),
                        filter,
                        depth,
                        label: format!(
                            "{}{}{}",
                            worktree
                                .branch
                                .as_deref()
                                .map(|branch| branch.trim_start_matches("refs/heads/").to_owned())
                                .unwrap_or_else(|| {
                                    worktree
                                        .head
                                        .as_ref()
                                        .map(|head| {
                                            format!(
                                                "detached HEAD {}",
                                                head.chars().take(7).collect::<String>()
                                            )
                                        })
                                        .unwrap_or_else(|| "detached HEAD".into())
                                }),
                            if !matches!(
                                self.folders.get(&worktree.path),
                                Some(FolderStatus::Unlinked | FolderStatus::Inaccessible { .. })
                            ) {
                                ""
                            } else {
                                " [unavailable]"
                            },
                            if self.repository_errors.contains_key(&repo.common_dir) {
                                " [stale]"
                            } else {
                                ""
                            }
                        ),
                    });
                    if self.expanded.contains(&worktree.path) {
                        for child in all_paths
                            .iter()
                            .filter(|p| **p != worktree.path && p.starts_with(&worktree.path))
                        {
                            if child
                                .ancestors()
                                .skip(1)
                                .take_while(|p| *p != worktree.path)
                                .any(|p| !self.expanded.contains(p))
                            {
                                continue;
                            }
                            let filter = FolderFilter::Path(child.clone());
                            rows.push(FolderRow {
                                count: count(&filter),
                                filter,
                                depth: depth
                                    + child
                                        .strip_prefix(&worktree.path)
                                        .unwrap()
                                        .components()
                                        .count(),
                                label: child
                                    .file_name()
                                    .unwrap_or_default()
                                    .to_string_lossy()
                                    .into_owned(),
                            });
                        }
                    }
                }
            }
        }
        rows
    }
    pub fn move_selection(&mut self, delta: isize) {
        let visible = self.visible();
        if visible.is_empty() {
            self.selected = None;
            return;
        }
        let current = visible
            .iter()
            .position(|s| Some(&s.key) == self.selected.as_ref())
            .unwrap_or(0);
        let next = current.saturating_add_signed(delta).min(visible.len() - 1);
        self.selected = Some(visible[next].key.clone());
    }
    pub fn reconcile(&mut self, old_index: usize) {
        let rows = self.visible();
        if !rows.iter().any(|s| Some(&s.key) == self.selected.as_ref()) {
            self.selected = rows
                .get(old_index.min(rows.len().saturating_sub(1)))
                .map(|s| s.key.clone());
        }
    }
    /// Cheaply reject repeated provider snapshots before rebuilding selection.
    pub fn update_changes(&self, update: &Update) -> bool {
        if update.generation() != self.generation {
            return false;
        }
        match update {
            Update::Live {
                source,
                statuses,
                error,
                ..
            } => {
                let state = &self.sources[*source];
                state.live_checked != error.is_none()
                    || state.live_error != *error
                    || if error.is_some() {
                        !state.live.is_empty()
                    } else {
                        state.live != *statuses
                    }
            }
            Update::Folder { path, status, .. } => self.folders.get(path) != Some(status),
            Update::Detail { key, value, .. } => self.details.get(key) != Some(value.as_ref()),
            Update::Page {
                source,
                sessions,
                warnings,
                ..
            } => {
                !warnings.is_empty()
                    || sessions.iter().any(|session| {
                        self.sources[*source]
                            .staging
                            .get(&session.key)
                            .is_none_or(|old| old.updated_at < session.updated_at)
                    })
            }
            Update::Finished { source, error, .. } => {
                let state = &self.sources[*source];
                state.loading || !state.staging.is_empty() || state.error != *error
            }
            Update::Ready {
                source, provider, ..
            } => self.sources[*source]
                .provider
                .as_ref()
                .is_none_or(|old| !Arc::ptr_eq(old, provider)),
        }
    }
    pub fn apply(&mut self, update: Update) {
        if !self.update_changes(&update) {
            return;
        }
        let old_index = self
            .visible()
            .iter()
            .position(|s| Some(&s.key) == self.selected.as_ref())
            .unwrap_or(0);
        match update {
            Update::Live {
                source,
                statuses,
                error,
                ..
            } => {
                self.sources[source].live_checked = error.is_none();
                self.sources[source].live = if error.is_none() {
                    statuses
                } else {
                    BTreeMap::new()
                };
                self.sources[source].live_error = error;
            }
            Update::Ready {
                source, provider, ..
            } => self.sources[source].provider = Some(provider),
            Update::Page {
                source,
                sessions,
                warnings,
                ..
            } => {
                let state = &mut self.sources[source];
                state.warnings.extend(warnings);
                for session in sessions {
                    if state
                        .staging
                        .get(&session.key)
                        .is_none_or(|old| old.updated_at < session.updated_at)
                    {
                        state.staging.insert(session.key.clone(), session);
                    }
                }
            }
            Update::Finished { source, error, .. } => {
                let state = &mut self.sources[source];
                state.loading = false;
                state.error = error;
                if state.error.is_none() && state.warnings.is_empty() {
                    state.committed = std::mem::take(&mut state.staging);
                } else {
                    state.committed.append(&mut state.staging);
                }
            }
            Update::Folder { path, status, .. } => {
                self.folders.insert(path, status);
            }
            Update::Detail { key, value, .. } => {
                self.details.insert(key, *value);
            }
        }
        self.reconcile(old_index);
    }
}

pub struct Discovery {
    pub rx: mpsc::UnboundedReceiver<Update>,
    tx: mpsc::UnboundedSender<Update>,
    tasks: Vec<JoinHandle<()>>,
    checked: HashSet<PathBuf>,
    requested: HashSet<SessionKey>,
    folder_limit: Arc<Semaphore>,
    live_enabled: bool,
}
impl Default for Discovery {
    fn default() -> Self {
        Self::new()
    }
}
impl Discovery {
    pub fn new() -> Self {
        let (tx, rx) = mpsc::unbounded_channel();
        Self {
            rx,
            tx,
            tasks: Vec::new(),
            checked: HashSet::new(),
            requested: HashSet::new(),
            folder_limit: Arc::new(Semaphore::new(8)),
            live_enabled: false,
        }
    }
    pub fn enable_live(&mut self) {
        self.live_enabled = true;
    }
    pub fn refresh(&mut self, browser: &mut Browser) {
        for task in self.tasks.drain(..) {
            task.abort();
        }
        self.checked.clear();
        self.requested.clear();
        browser.begin_refresh();
        let limit = Arc::new(Semaphore::new(4));
        for (source, spec) in browser.specs.clone().into_iter().enumerate() {
            if self.live_enabled {
                if let SourceSpec::Codex {
                    binary,
                    source: config,
                } = spec.clone()
                {
                    let tx = self.tx.clone();
                    let generation = browser.generation;
                    self.tasks.push(tokio::spawn(async move {
                        loop {
                            let error = match crate::providers::codex_live::LiveClient::connect(
                                &binary,
                                config.clone(),
                            )
                            .await
                            {
                                Ok(mut client) => loop {
                                    match client.snapshot().await {
                                        Ok(statuses) => {
                                            if tx
                                                .send(Update::Live {
                                                    generation,
                                                    source,
                                                    statuses,
                                                    error: None,
                                                })
                                                .is_err()
                                            {
                                                return;
                                            }
                                            tokio::time::sleep(std::time::Duration::from_secs(2))
                                                .await;
                                        }
                                        Err(error) => break error,
                                    }
                                },
                                Err(error) => error,
                            };
                            if tx
                                .send(Update::Live {
                                    generation,
                                    source,
                                    statuses: BTreeMap::new(),
                                    error: Some(error),
                                })
                                .is_err()
                            {
                                return;
                            }
                            tokio::time::sleep(std::time::Duration::from_secs(5)).await;
                        }
                    }));
                }
            }
            let tx = self.tx.clone();
            let generation = browser.generation;
            let limit = limit.clone();
            self.tasks.push(tokio::spawn(async move {
                let _permit = limit
                    .acquire_owned()
                    .await
                    .expect("discovery semaphore open");
                let result = async {
                    let provider = Arc::new(spec.open().await?);
                    let _ = tx.send(Update::Ready {
                        generation,
                        source,
                        provider: provider.clone(),
                    });
                    let mut cursor = None;
                    let mut seen = HashSet::new();
                    loop {
                        let page = provider.page(cursor).await?;
                        if tx
                            .send(Update::Page {
                                generation,
                                source,
                                sessions: page.sessions,
                                warnings: page.warnings,
                            })
                            .is_err()
                        {
                            return Ok::<(), String>(());
                        }
                        match page.next {
                            None => break,
                            Some(next) => {
                                if !seen.insert(next.0.clone()) {
                                    return Err("provider repeated pagination cursor".into());
                                }
                                cursor = Some(next);
                            }
                        }
                    }
                    Ok(())
                }
                .await;
                let _ = tx.send(Update::Finished {
                    generation,
                    source,
                    error: result.err(),
                });
            }));
        }
    }
    pub fn check_folders(&mut self, browser: &Browser) {
        self.tasks.retain(|t| !t.is_finished());
        for path in browser
            .all()
            .into_iter()
            .filter_map(|s| s.cwd)
            .filter(|p| p.is_absolute())
        {
            if !self.checked.insert(path.clone()) {
                continue;
            }
            let tx = self.tx.clone();
            let generation = browser.generation;
            let limit = self.folder_limit.clone();
            self.tasks.push(tokio::spawn(async move {
                let permit = limit.acquire_owned().await.expect("folder semaphore open");
                let result = tokio::task::spawn_blocking(move || {
                    // Keep the permit in the blocking task even if the waiter is cancelled.
                    let _permit = permit;
                    let status = classify_folder(Some(&path));
                    (path, status)
                })
                .await;
                if let Ok((path, status)) = result {
                    let _ = tx.send(Update::Folder {
                        generation,
                        path,
                        status,
                    });
                }
            }));
        }
    }
    pub fn detail(&mut self, browser: &Browser) {
        let Some(key) = browser.selected.clone() else {
            return;
        };
        if self.requested.contains(&key) {
            return;
        }
        let owner = browser
            .sources
            .iter()
            .find(|s| s.committed.contains_key(&key) || s.staging.contains_key(&key));
        // Wait for this source's discovery so a detail request cannot delay its pagination.
        let Some(provider) = owner
            .filter(|s| !s.loading)
            .and_then(|s| s.provider.clone())
        else {
            return;
        };
        self.requested.insert(key.clone());
        let tx = self.tx.clone();
        let generation = browser.generation;
        self.tasks.push(tokio::spawn(async move {
            let value = provider.read(&key).await;
            let _ = tx.send(Update::Detail {
                generation,
                key,
                value: Box::new(value),
            });
        }));
    }
    pub async fn close(&mut self, browser: &mut Browser) {
        for task in self.tasks.drain(..) {
            task.abort();
            let _ = task.await;
        }
        let mut closing = tokio::task::JoinSet::new();
        for source in &mut browser.sources {
            if let Some(provider) = source.provider.take() {
                closing.spawn(async move {
                    provider.close().await;
                });
            }
        }
        while closing.join_next().await.is_some() {}
    }
}
impl Drop for Discovery {
    fn drop(&mut self) {
        for task in &self.tasks {
            task.abort();
        }
    }
}

pub fn safe_text(text: &str) -> String {
    text.chars()
        .flat_map(|c| {
            if c.is_control() || matches!(c, '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}') {
                c.escape_default().collect::<Vec<_>>()
            } else {
                vec![c]
            }
        })
        .collect()
}
