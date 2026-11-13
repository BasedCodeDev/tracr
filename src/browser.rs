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
            Self::Ready { generation, .. }
            | Self::Page { generation, .. }
            | Self::Finished { generation, .. }
            | Self::Folder { generation, .. }
            | Self::Detail { generation, .. } => *generation,
        }
    }
}

#[derive(Default)]
pub struct SourceState {
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
    /// Select an absolute, lexically normalized folder before discovery begins.
    pub fn open_folder(&mut self, path: PathBuf) {
        self.expanded.extend(path.ancestors().map(|p| p.to_owned()));
        self.folder = FolderFilter::Path(path.clone());
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
        let terms: Vec<String> = self
            .query
            .split_whitespace()
            .map(str::to_lowercase)
            .collect();
        let searching = !terms.is_empty();
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
            FolderFilter::Path(path) => session.cwd.as_ref().is_some_and(|p| p == path),
        }
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
                .filter(|s| self.matches_folder(s, f))
                .count()
        };
        let mut rows = vec![
            FolderRow {
                filter: FolderFilter::All,
                label: "All sessions".into(),
                depth: 0,
                count: sessions.len(),
            },
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
        for path in paths {
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
                    "{} {}{status}",
                    if self.expanded.contains(&path) {
                        "▾"
                    } else {
                        "▸"
                    },
                    path.file_name()
                        .unwrap_or(path.as_os_str())
                        .to_string_lossy()
                ),
            });
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
    pub fn apply(&mut self, update: Update) {
        if update.generation() != self.generation {
            return;
        }
        let old_index = self
            .visible()
            .iter()
            .position(|s| Some(&s.key) == self.selected.as_ref())
            .unwrap_or(0);
        match update {
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
        }
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
