//! Demand-driven replacement of complete Git graph prefixes.
use crate::git::{CommitRecord, GraphRow};
use crate::git_discovery::BackendFuture;
use std::sync::atomic::{AtomicBool, Ordering};
use std::{cell::Cell, path::PathBuf, sync::Arc};
use tokio::task::JoinHandle;

pub trait Backend: Send + Sync {
    fn references(&self, _path: PathBuf) -> BackendFuture<Result<crate::git::References, String>> {
        Box::pin(async { Ok(crate::git::References::new()) })
    }

    fn head(&self, path: PathBuf) -> BackendFuture<Result<crate::git::Head, String>> {
        Box::pin(async move { crate::git::head(&path).await })
    }
    fn history_at(
        &self,
        path: PathBuf,
        limit: usize,
        _head: Option<String>,
    ) -> BackendFuture<Result<Vec<CommitRecord>, String>> {
        self.history(path, limit)
    }

    fn history(
        &self,
        path: PathBuf,
        limit: usize,
    ) -> BackendFuture<Result<Vec<CommitRecord>, String>>;
}
struct Native;
impl Backend for Native {
    fn references(&self, path: PathBuf) -> BackendFuture<Result<crate::git::References, String>> {
        Box::pin(async move { crate::git::references(&path).await })
    }

    fn history_at(
        &self,
        path: PathBuf,
        limit: usize,
        head: Option<String>,
    ) -> BackendFuture<Result<Vec<CommitRecord>, String>> {
        Box::pin(async move { crate::git::history_at(&path, limit, head.as_deref()).await })
    }

    fn history(
        &self,
        path: PathBuf,
        limit: usize,
    ) -> BackendFuture<Result<Vec<CommitRecord>, String>> {
        Box::pin(async move { crate::git::history(&path, limit).await })
    }
}
pub enum Movement {
    Up,
    Down,
    PageUp,
    PageDown,
    Home,
    End,
}
type ReadResult = (
    Result<Vec<CommitRecord>, String>,
    Option<Result<crate::git::References, String>>,
);
type ReadTask = (u64, usize, JoinHandle<ReadResult>);

struct Snapshot {
    rows: Vec<GraphRow>,
    positions: std::collections::HashMap<String, usize>,
    selectable: Vec<usize>,
    width: usize,
}
fn construct(records: Vec<CommitRecord>, cancel: &AtomicBool) -> Result<Option<Snapshot>, String> {
    let cancelled = || cancel.load(Ordering::Relaxed);
    let mut builder = crate::git_layout::Builder::new();
    for record in &records {
        if cancelled() {
            return Ok(None);
        }
        builder
            .append_cancellable(
                &crate::git_layout::Commit {
                    oid: record.oid.clone(),
                    parents: record.parents.clone(),
                },
                &cancelled,
            )
            .map_err(|error| format!("Graph layout failed: {error:?}"))?;
    }
    let layout = builder.into_layout();
    let Some(graph) = crate::git_render::render_cancellable(&layout, cancelled) else {
        return Ok(None);
    };
    drop(layout);
    let width = graph.width;
    let mut records = records.into_iter();
    let mut rows = Vec::with_capacity(graph.rows.len() + usize::from(graph.boundary.is_some()));
    let mut positions = std::collections::HashMap::new();
    let mut selectable = Vec::new();
    for rendered in graph.rows.into_iter().chain(graph.boundary) {
        if cancelled() {
            return Ok(None);
        }
        let record = if rendered.oid.is_some() {
            records.next()
        } else {
            None
        };
        let (oid, parents, subject) = match record {
            Some(record) => {
                positions.insert(record.oid.clone(), rows.len());
                selectable.push(rows.len());
                (Some(record.oid), record.parents, record.subject)
            }
            None => (None, Vec::new(), String::new()),
        };
        rows.push(GraphRow {
            graph: rendered.text(),
            graph_spans: rendered.spans,
            oid,
            parents,
            subject,
        });
    }
    Ok(Some(Snapshot {
        rows,
        positions,
        selectable,
        width,
    }))
}
struct BuildTask {
    token: u64,
    limit: usize,
    cancel: Arc<AtomicBool>,
    handle: JoinHandle<Result<Option<Snapshot>, String>>,
}
impl Drop for BuildTask {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

pub struct History {
    build: Option<BuildTask>,
    #[cfg(test)]
    builds_started: usize,
    positions: std::collections::HashMap<String, usize>,
    selectable: Vec<usize>,
    graph_width: usize,
    backend: Arc<dyn Backend>,
    checkout: Option<PathBuf>,
    pub head: Option<crate::git::Head>,
    pub following: bool,
    head_row: Option<usize>,
    resolve_head: bool,
    restart: bool,
    cancel_seek: bool,
    head_task: Option<JoinHandle<Result<crate::git::Head, String>>>,
    pub rows: Vec<GraphRow>,
    pub references: crate::git::References,
    pub reference_error: Option<String>,
    pub horizontal: Cell<usize>,
    max_width: Cell<Option<usize>>,
    pub selected: usize,
    pub top: Cell<usize>,
    pub page: Cell<usize>,
    pub error: Option<String>,
    pub exhausted: bool,
    pub limit: usize,
    path: Option<PathBuf>,
    generation: u64,
    token: u64,
    task: Option<ReadTask>,
    active: bool,
    refresh: bool,
    extend: bool,
    loaded: bool,
}
impl Default for History {
    fn default() -> Self {
        Self::with_backend(Arc::new(Native))
    }
}
impl History {
    pub fn with_backend(backend: Arc<dyn Backend>) -> Self {
        Self {
            build: None,
            #[cfg(test)]
            builds_started: 0,
            positions: Default::default(),
            selectable: Vec::new(),
            graph_width: 0,
            backend,
            checkout: None,
            head: None,
            following: false,
            head_row: None,
            resolve_head: false,
            restart: false,
            cancel_seek: false,
            head_task: None,
            rows: vec![],
            references: crate::git::References::new(),
            reference_error: None,
            horizontal: Cell::new(0),
            max_width: Cell::new(None),
            selected: 0,
            top: Cell::new(0),
            page: Cell::new(1),
            error: None,
            exhausted: false,
            limit: 0,
            path: None,
            generation: 0,
            token: 0,
            task: None,
            active: false,
            refresh: false,
            extend: false,
            loaded: false,
        }
    }
    pub async fn checkout(&mut self, checkout: Option<PathBuf>, root_selected: bool) {
        if checkout != self.checkout {
            self.shutdown().await;
            self.checkout = checkout;
            self.horizontal.set(0);
            self.head = None;
            self.head_row = None;
            self.max_width.set(None);
            self.following = self.checkout.is_some();
            self.resolve_head = self.checkout.is_some();
            self.refresh = true;
            self.error = None;
        } else if root_selected {
            self.follow();
        }
    }
    /// Refresh metadata without rearming follow mode or resetting the viewport.
    pub fn request_refresh(&mut self) {
        self.restart = true;
        self.resolve_head = self.checkout.is_some();
        self.refresh = true;
        self.error = None;
    }
    pub fn follow(&mut self) {
        self.horizontal.set(0);
        if self.checkout.is_some() {
            self.following = true;
            self.resolve_head = true;
            self.refresh = true;
            self.restart = true;
            self.error = None;
        }
    }
    pub fn is_head(&self, row: &GraphRow) -> bool {
        row.oid.is_some()
            && row.oid.as_ref() == self.head.as_ref().and_then(|head| head.oid.as_ref())
    }
    pub fn needs_poll(&self) -> bool {
        self.build
            .as_ref()
            .is_some_and(|task| task.handle.is_finished())
            || self
                .head_task
                .as_ref()
                .is_some_and(|task| task.is_finished())
            || self
                .task
                .as_ref()
                .is_some_and(|(_, _, task)| task.is_finished())
    }
    fn seeking(&self) -> bool {
        self.following
            && self
                .head
                .as_ref()
                .and_then(|head| head.oid.as_ref())
                .is_some()
            && self.head_row.is_none()
    }
    pub fn scroll_horizontal(&mut self, right: bool) {
        if self.following {
            self.cancel_seek = self.head_task.is_some() || self.seeking();
            self.following = false;
        }
        self.horizontal.set(if right {
            self.horizontal.get().saturating_add(4)
        } else {
            self.horizontal.get().saturating_sub(4)
        });
    }
    pub fn row_spans(&self, row: &GraphRow) -> Vec<ratatui::text::Span<'static>> {
        use ratatui::{
            style::{Color, Modifier, Style},
            text::Span,
        };
        let mut spans: Vec<_> = row
            .graph_spans
            .iter()
            .map(|span| {
                let color = match span.color {
                    Some(31) => Color::Red,
                    Some(32) => Color::Green,
                    Some(33) => Color::Yellow,
                    Some(34) => Color::Blue,
                    Some(35) => Color::Magenta,
                    Some(36) => Color::Cyan,
                    _ => Color::Reset,
                };
                Span::styled(span.text.clone(), Style::default().fg(color))
            })
            .collect();
        if let Some(oid) = &row.oid {
            use unicode_width::UnicodeWidthStr;
            spans.push(Span::raw(
                " ".repeat(self.graph_width.saturating_sub(row.graph.width()) + 1),
            ));
            spans.push(Span::raw(format!(
                "{} ",
                oid.chars().take(7).collect::<String>()
            )));
            if self.is_head(row) {
                spans.push(Span::styled(
                    "[HEAD] ",
                    Style::default()
                        .fg(Color::Magenta)
                        .add_modifier(Modifier::BOLD),
                ));
            }
            if let Some(labels) = self.references.get(oid) {
                for label in labels {
                    let (prefix, color) = match label.kind {
                        crate::git::RefKind::Local => ("", Color::Green),
                        crate::git::RefKind::Remote => ("remote: ", Color::Cyan),
                        crate::git::RefKind::Tag => ("tag: ", Color::Yellow),
                    };
                    spans.push(Span::styled(
                        format!("[{prefix}{}] ", label.name),
                        Style::default().fg(color),
                    ));
                }
            }
            spans.push(Span::raw(row.subject.clone()));
        }
        spans
    }
    pub fn clamp_horizontal(&self, width: usize) {
        use unicode_width::UnicodeWidthStr;
        let max = self
            .max_width
            .get()
            .unwrap_or_else(|| {
                let max = self
                    .rows
                    .iter()
                    .map(|row| {
                        let mut width = if row.oid.is_some() {
                            self.graph_width + 1
                        } else {
                            row.graph.width()
                        };
                        if let Some(oid) = &row.oid {
                            width +=
                                8 + row.subject.width() + if self.is_head(row) { 7 } else { 0 };
                            if let Some(labels) = self.references.get(oid) {
                                for label in labels {
                                    width += label.name.width()
                                        + 3
                                        + match label.kind {
                                            crate::git::RefKind::Local => 0,
                                            crate::git::RefKind::Remote => 8,
                                            crate::git::RefKind::Tag => 5,
                                        };
                                }
                            }
                        }
                        width
                    })
                    .max()
                    .unwrap_or(0);
                self.max_width.set(Some(max));
                max
            })
            .saturating_sub(width);
        self.horizontal.set(self.horizontal.get().min(max));
    }
    pub fn repository(&self) -> Option<PathBuf> {
        self.path.clone()
    }
    pub fn present(&self) -> bool {
        self.path.is_some()
    }
    pub fn loading(&self) -> bool {
        self.task.is_some() || self.build.is_some() || self.head_task.is_some()
    }
    pub fn title(&self) -> String {
        if let Some(error) = &self.error {
            return format!("Git history — {error} · r retry (previous data may be stale)");
        }
        let status = if self.reference_error.is_some() {
            "references unavailable/stale · r retry"
        } else if self.head.as_ref().is_some_and(|head| head.oid.is_none()) {
            "no commits yet"
        } else if self.following
            && (self.resolve_head || self.head_task.is_some() || self.seeking())
        {
            "locating HEAD…"
        } else if self.loading() && !self.loaded {
            "loading…"
        } else if self.loading() {
            "loading more…"
        } else if self.loaded && self.rows.is_empty() {
            "empty"
        } else if self.exhausted {
            "end of history"
        } else {
            "Tab cycle panes"
        };
        let context = self
            .head
            .as_ref()
            .map(|head| {
                crate::browser::safe_text(head.branch.as_deref().unwrap_or("detached HEAD"))
            })
            .unwrap_or_default();
        if context.is_empty() {
            format!("Git history — {status}")
        } else {
            format!(
                "Git history — {context} · {status} · {}",
                if self.following {
                    "following HEAD"
                } else {
                    "f follow HEAD"
                }
            )
        }
    }
    pub fn visible_range(&self, height: usize) -> std::ops::Range<usize> {
        self.page.set(height.max(1));
        let mut top = self.top.get().min(self.rows.len().saturating_sub(height));
        if self.selected < top {
            top = self.selected;
        }
        if self.selected >= top + height.max(1) {
            top = self.selected + 1 - height.max(1);
        }
        self.top.set(top);
        top..(top + height).min(self.rows.len())
    }
    /// Scroll affordances derived only from the cached, clamped viewport.
    pub fn scroll_availability(&self, visible: &std::ops::Range<usize>) -> (bool, bool) {
        if !self.loaded || self.selectable.is_empty() || visible.is_empty() {
            return (false, false);
        }
        let above = self.selectable[0] < visible.start;
        let below = self
            .selectable
            .last()
            .is_some_and(|index| *index >= visible.end)
            || !self.exhausted;
        (above, below)
    }

    pub fn navigate(&mut self, movement: Movement) {
        if self.following {
            self.cancel_seek = self.head_task.is_some() || self.seeking();
            self.following = false;
        }
        let commits = &self.selectable;
        if commits.is_empty() {
            return;
        }
        let target = match movement {
            Movement::Up => commits
                .partition_point(|i| *i < self.selected)
                .checked_sub(1)
                .map(|i| commits[i]),
            Movement::Down => commits
                .get(commits.partition_point(|i| *i <= self.selected))
                .copied(),
            Movement::PageUp => Some(
                commits[commits
                    .partition_point(|i| *i <= self.selected.saturating_sub(self.page.get()))
                    .saturating_sub(1)],
            ),
            Movement::PageDown => Some(
                commits[commits
                    .partition_point(|i| *i < self.selected.saturating_add(self.page.get()))
                    .min(commits.len() - 1)],
            ),
            Movement::Home => commits.first().copied(),
            Movement::End => commits.last().copied(),
        };
        if let Some(target) = target {
            self.selected = target;
        }
        // Only a navigation event arms expansion; completion cannot arm another.
        self.extend = self.error.is_none()
            && !self.exhausted
            && commits.len() - commits.partition_point(|i| *i <= self.selected) < self.page.get();
    }
    pub async fn shutdown(&mut self) {
        self.token += 1;
        if let Some(task) = self.head_task.take() {
            task.abort();
            let _ = task.await;
        }
        if let Some((_, _, task)) = self.task.take() {
            task.abort();
            let _ = task.await;
        }
        if let Some(mut build) = self.build.take() {
            build.cancel.store(true, Ordering::Relaxed);
            let _ = (&mut build.handle).await;
        }
        self.extend = false;
    }
    #[cfg(test)]
    fn replace(&mut self, rows: Vec<GraphRow>, limit: usize) {
        let positions = rows
            .iter()
            .enumerate()
            .filter_map(|(i, r)| r.oid.clone().map(|id| (id, i)))
            .collect();
        let selectable = rows
            .iter()
            .enumerate()
            .filter_map(|(i, r)| r.oid.as_ref().map(|_| i))
            .collect();
        let width = rows
            .iter()
            .map(|r| unicode_width::UnicodeWidthStr::width(r.graph.as_str()))
            .max()
            .unwrap_or(0);
        self.install(
            Snapshot {
                rows,
                positions,
                selectable,
                width,
            },
            limit,
        );
    }
    fn install(&mut self, snapshot: Snapshot, limit: usize) {
        let Snapshot {
            rows,
            positions,
            selectable,
            width,
        } = snapshot;
        let offset = self.selected.saturating_sub(self.top.get());
        let selected = self
            .rows
            .get(self.selected)
            .and_then(|row| row.oid.as_ref());
        let locate = |id: &String| positions.get(id.as_str()).copied();
        let preserved = selected.and_then(locate).or_else(|| {
            self.rows
                .iter()
                .enumerate()
                .filter(|(_, row)| row.oid.is_some())
                .filter_map(|(i, row)| {
                    locate(row.oid.as_ref().unwrap())
                        .map(|new| ((i.abs_diff(self.selected), i < self.selected), new))
                })
                .min_by_key(|(distance, _)| *distance)
                .map(|(_, index)| index)
        });
        self.selected = preserved
            .or_else(|| rows.iter().position(|row| row.oid.is_some()))
            .unwrap_or(0);
        self.top.set(self.selected.saturating_sub(offset));
        self.exhausted = selectable.len() < limit;
        self.positions = positions;
        self.selectable = selectable;
        self.graph_width = width;
        self.rows = rows;
        self.head_row = self
            .head
            .as_ref()
            .and_then(|head| head.oid.as_ref())
            .and_then(|oid| self.positions.get(oid).copied());
        self.max_width.set(None);
        self.loaded = true;
        self.limit = limit;
        self.error = None;
        if self.following {
            if let Some(index) = self.head_row {
                self.selected = index;
            } else if self.seeking() && self.exhausted {
                self.error = Some("Selected HEAD is unavailable in history".into());
            }
        }
    }
    pub async fn tick(
        &mut self,
        path: Option<PathBuf>,
        generation: u64,
        active: bool,
        initial_page: usize,
    ) {
        if self.restart || self.cancel_seek {
            let cancel = self.cancel_seek;
            let extend = self.extend;
            self.shutdown().await;
            self.extend = extend;
            self.restart = false;
            self.cancel_seek = false;
            if cancel {
                self.refresh = false;
                self.resolve_head = false;
            }
        }
        if path != self.path || generation != self.generation {
            let extend = self.extend && path == self.path;
            self.shutdown().await;
            self.extend = extend;
            if path != self.path {
                self.following = self.checkout.is_some();
                self.rows.clear();
                self.positions.clear();
                self.selectable.clear();
                self.graph_width = 0;
                self.head_row = None;
                self.max_width.set(None);
                self.references.clear();
                self.reference_error = None;
                self.horizontal.set(0);
                self.selected = 0;
                self.top.set(0);
                self.limit = 0;
                self.loaded = false;
                self.exhausted = false;
            }
            self.path = path;
            self.generation = generation;
            self.refresh = true;
            self.resolve_head = self.checkout.is_some();
            self.error = None;
        }
        if !active {
            if self.active {
                self.shutdown().await;
            }
            self.active = false;
            return;
        }
        if !self.active {
            self.refresh = true;
            self.resolve_head = self.checkout.is_some();
        }
        self.active = true;
        if self.resolve_head {
            if let Some(checkout) = self.checkout.clone() {
                if self.head_task.is_none() {
                    self.head_task = Some(tokio::spawn(self.backend.head(checkout)));
                }
                if !self.head_task.as_ref().unwrap().is_finished() {
                    return;
                }
                let result = self
                    .head_task
                    .take()
                    .unwrap()
                    .await
                    .unwrap_or_else(|error| Err(error.to_string()));
                self.resolve_head = false;
                match result {
                    Ok(head) => {
                        self.head = Some(head);
                        self.head_row = self
                            .head
                            .as_ref()
                            .and_then(|head| head.oid.as_ref())
                            .and_then(|oid| self.positions.get(oid).copied());
                        self.max_width.set(None);
                        self.refresh = true;
                    }
                    Err(error) => {
                        self.error = Some(format!("Checkout HEAD unavailable: {error}"));
                        self.refresh = false;
                        self.extend = false;
                        return;
                    }
                }
            } else {
                self.resolve_head = false;
            }
        }
        if self
            .task
            .as_ref()
            .is_some_and(|(_, _, task)| task.is_finished())
        {
            let (token, limit, task) = self.task.take().unwrap();
            let (result, references) = task
                .await
                .unwrap_or_else(|error| (Err(error.to_string()), None));
            if token == self.token {
                if let Some(references) = references {
                    match references {
                        Ok(refs) => {
                            self.references = refs;
                            self.max_width.set(None);
                            self.reference_error = None;
                        }
                        Err(error) => self.reference_error = Some(error),
                    }
                }
                match result {
                    Ok(records)
                        if self.loaded
                            && records.len() == self.selectable.len()
                            && records.iter().zip(&self.selectable).all(|(record, index)| {
                                let row = &self.rows[*index];
                                row.oid.as_ref() == Some(&record.oid)
                                    && row.parents == record.parents
                                    && row.subject == record.subject
                            }) =>
                    {
                        // Ref/HEAD-only changes reuse geometry and its row map.
                        self.limit = limit;
                        self.exhausted = records.len() < limit;
                        self.error = None;
                        if self.following {
                            if let Some(index) = self.head_row {
                                self.selected = index;
                            }
                        }
                    }
                    Ok(records) => {
                        #[cfg(test)]
                        {
                            self.builds_started += 1;
                        }
                        let cancel = Arc::new(AtomicBool::new(false));
                        let signal = cancel.clone();
                        self.build = Some(BuildTask {
                            token,
                            limit,
                            cancel,
                            handle: tokio::task::spawn_blocking(move || {
                                construct(records, &signal)
                            }),
                        });
                    }
                    Err(error) => {
                        self.limit = limit;
                        self.error = Some(error);
                    }
                }
            }
        }
        if self
            .build
            .as_ref()
            .is_some_and(|build| build.handle.is_finished())
        {
            let mut build = self.build.take().unwrap();
            let result = (&mut build.handle)
                .await
                .unwrap_or_else(|error| Err(error.to_string()));
            if build.token == self.token {
                match result {
                    Ok(Some(snapshot)) => {
                        self.install(snapshot, build.limit);
                    }
                    Ok(None) => {}
                    Err(error) => {
                        self.error = Some(error);
                        self.limit = build.limit;
                    }
                }
            }
        }
        let seek = self.error.is_none() && self.seeking() && !self.exhausted;
        // Navigation during a refresh retains its demand, but a completed
        // expansion may already have moved the boundary beyond the selection.
        if self.extend && self.task.is_none() && self.build.is_none() {
            self.extend = self.error.is_none()
                && !self.exhausted
                && self.selectable.len() - self.selectable.partition_point(|i| *i <= self.selected)
                    < self.page.get();
        }
        if self.task.is_none() && self.build.is_none() && (self.refresh || self.extend || seek) {
            if let Some(path) = self.path.clone() {
                let limit = if self.refresh {
                    self.limit.max(128).max(if self.loaded {
                        0
                    } else {
                        initial_page.saturating_mul(4)
                    })
                } else {
                    self.limit.saturating_mul(2)
                };
                self.token += 1;
                let refs = self.refresh.then(|| self.backend.references(path.clone()));
                let history = self.backend.history_at(
                    path,
                    limit,
                    self.head.as_ref().and_then(|head| head.oid.clone()),
                );
                self.task = Some((
                    self.token,
                    limit,
                    tokio::spawn(async move {
                        let refs = match refs {
                            Some(future) => Some(future.await),
                            None => None,
                        };
                        (history.await, refs)
                    }),
                ));
                self.error = None;
            }
            if !self.refresh {
                self.extend = false;
            }
            self.refresh = false;
        }
    }
}
impl Drop for History {
    fn drop(&mut self) {
        if let Some(task) = self.head_task.take() {
            task.abort();
        }
        if let Some((_, _, task)) = self.task.take() {
            task.abort();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::sync::{mpsc, oneshot};
    type Reply = oneshot::Sender<Result<Vec<GraphRow>, String>>;
    struct Controlled(mpsc::UnboundedSender<(usize, Reply)>);
    impl Backend for Controlled {
        fn history(
            &self,
            _: PathBuf,
            limit: usize,
        ) -> BackendFuture<Result<Vec<CommitRecord>, String>> {
            let (tx, rx) = oneshot::channel();
            self.0.send((limit, tx)).unwrap();
            Box::pin(async move { rx.await.unwrap().map(input_records) })
        }
    }
    fn input_records(rows: Vec<GraphRow>) -> Vec<CommitRecord> {
        rows.into_iter()
            .filter_map(|row| {
                row.oid.map(|oid| CommitRecord {
                    oid,
                    parents: row.parents,
                    subject: row.subject,
                })
            })
            .collect()
    }
    fn rows(n: usize) -> Vec<GraphRow> {
        (0..n)
            .map(|i| GraphRow {
                graph: "●".into(),
                graph_spans: vec![crate::git::GraphSpan {
                    text: "●".into(),
                    color: Some(36),
                }],
                oid: Some(format!("{i:040x}")),
                parents: vec![format!("{:040x}", i + 1)],
                subject: format!("commit {i}"),
            })
            .collect()
    }
    async fn tick(h: &mut History) {
        h.tick(Some("/repo".into()), 0, true, 20).await;
    }
    async fn finish(h: &mut History) {
        tokio::time::timeout(std::time::Duration::from_secs(2), async {
            while h.loading() {
                tokio::task::yield_now().await;
                tick(h).await;
            }
        })
        .await
        .unwrap();
    }
    #[tokio::test]
    async fn demand_doubles_preserves_selection_and_stops_at_exhaustion() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let mut h = History::with_backend(Arc::new(Controlled(tx)));
        tick(&mut h).await;
        let (limit, reply) = rx.recv().await.unwrap();
        assert_eq!(limit, 128);
        tick(&mut h).await;
        assert!(rx.try_recv().is_err());
        reply.send(Ok(rows(128))).unwrap();
        finish(&mut h).await;
        assert!(!h.exhausted);
        h.visible_range(10);
        h.navigate(Movement::End);
        h.visible_range(10);
        let old_top = h.top.get();
        tick(&mut h).await;
        let (limit, reply) = rx.recv().await.unwrap();
        assert_eq!(limit, 256);
        reply.send(Ok(rows(256))).unwrap();
        finish(&mut h).await;
        assert_eq!(h.selected, 127);
        assert_eq!(h.top.get(), old_top);
        for _ in 0..3 {
            tick(&mut h).await;
        }
        assert!(rx.try_recv().is_err());
        h.navigate(Movement::End);
        tick(&mut h).await;
        let (limit, reply) = rx.recv().await.unwrap();
        assert_eq!(limit, 512);
        reply.send(Ok(rows(300))).unwrap();
        finish(&mut h).await;
        assert!(h.exhausted);
        h.navigate(Movement::End);
        tick(&mut h).await;
        assert!(!h.loading());
    }
    #[tokio::test]
    async fn navigation_during_refresh_retains_expansion_without_chaining() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let mut h = History::with_backend(Arc::new(Controlled(tx)));
        tick(&mut h).await;
        rx.recv().await.unwrap().1.send(Ok(rows(128))).unwrap();
        finish(&mut h).await;
        h.visible_range(10);
        h.request_refresh();
        tick(&mut h).await;
        let (limit, reply) = rx.recv().await.unwrap();
        assert_eq!(limit, 128);
        h.navigate(Movement::End);
        reply.send(Ok(rows(128))).unwrap();
        let (limit, reply) = tokio::time::timeout(std::time::Duration::from_secs(2), async {
            loop {
                tick(&mut h).await;
                if let Ok(request) = rx.try_recv() {
                    break request;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        assert_eq!(limit, 256);
        // Repeated navigation at the old boundary while expansion is pending
        // must not cause another expansion after this request completes.
        h.navigate(Movement::End);
        reply.send(Ok(rows(256))).unwrap();
        finish(&mut h).await;
        assert_eq!(h.selected, 127);
        assert!(rx.try_recv().is_err());
    }
    #[tokio::test]
    async fn cancelled_head_lookup_preserves_navigation_demand() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let mut h = History::with_backend(Arc::new(Controlled(tx)));
        tick(&mut h).await;
        rx.recv().await.unwrap().1.send(Ok(rows(128))).unwrap();
        finish(&mut h).await;
        h.visible_range(10);
        h.following = true;
        h.resolve_head = true;
        h.head_task = Some(tokio::spawn(std::future::pending()));
        h.navigate(Movement::End);
        tick(&mut h).await;
        let (limit, reply) = tokio::time::timeout(std::time::Duration::from_secs(2), rx.recv())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(limit, 256);
        assert!(h.head_task.is_none());
        reply.send(Ok(rows(256))).unwrap();
        finish(&mut h).await;
        assert_eq!(h.selected, 127);
        assert!(rx.try_recv().is_err());
    }
    #[tokio::test]
    async fn failure_requires_retry_and_hide_generation_and_shutdown_cancel() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let mut h = History::with_backend(Arc::new(Controlled(tx)));
        tick(&mut h).await;
        rx.recv().await.unwrap().1.send(Ok(rows(128))).unwrap();
        finish(&mut h).await;
        h.navigate(Movement::End);
        tick(&mut h).await;
        rx.recv()
            .await
            .unwrap()
            .1
            .send(Err("offline".into()))
            .unwrap();
        finish(&mut h).await;
        assert_eq!(h.selectable.len(), 128);
        assert!(h.error.is_some());
        h.navigate(Movement::End);
        tick(&mut h).await;
        assert!(!h.loading());
        h.tick(Some("/repo".into()), 1, true, 20).await;
        let (limit, reply) = rx.recv().await.unwrap();
        assert_eq!(limit, 256);
        h.tick(Some("/repo".into()), 1, false, 20).await;
        assert!(reply.is_closed());
        h.tick(Some("/repo".into()), 1, true, 20).await;
        let (_, reply) = rx.recv().await.unwrap();
        reply.send(Ok(rows(200))).unwrap();
        // Even a completed result must not cross a generation/repository change.
        tokio::task::yield_now().await;
        h.tick(Some("/other".into()), 2, true, 40).await;
        assert!(h.rows.is_empty());
        let (limit, reply) = rx.recv().await.unwrap();
        assert_eq!(limit, 160);
        h.shutdown().await;
        assert!(reply.is_closed());
    }
    #[test]
    fn replacement_fallback_and_navigation_skip_connectors() {
        let mut h = History::default();
        h.replace(rows(8), 8);
        h.selected = 3;
        h.top.set(2);
        let mut next = rows(8);
        next.remove(3);
        h.replace(next, 8);
        assert_eq!(h.rows[h.selected].subject, "commit 4");
        assert_eq!(h.selected - h.top.get(), 1);
        h.rows.insert(
            0,
            GraphRow {
                graph: "| ".into(),
                graph_spans: vec![],
                oid: None,
                parents: Vec::new(),
                subject: String::new(),
            },
        );
        h.selectable = h
            .rows
            .iter()
            .enumerate()
            .filter_map(|(i, r)| r.oid.as_ref().map(|_| i))
            .collect();
        h.navigate(Movement::Home);
        assert_eq!(h.selected, 1);
        h.navigate(Movement::Up);
        assert_eq!(h.selected, 1);
        h.visible_range(2);
        h.navigate(Movement::PageDown);
        assert_eq!(h.selected, 3);
        h.navigate(Movement::PageUp);
        assert_eq!(h.selected, 1);
        h.replace(vec![], 128);
        assert!(h.exhausted);
        assert!(h.title().contains("empty"));
    }
    #[tokio::test]
    async fn merge_prefixes_preserve_commits_and_connectors() {
        let dir = tempfile::tempdir().unwrap();
        let run = |args: &[&str]| {
            let out = std::process::Command::new("git")
                .arg("-C")
                .arg(dir.path())
                .args(args)
                .env("GIT_CONFIG_GLOBAL", "/dev/null")
                .env("GIT_CONFIG_NOSYSTEM", "1")
                .output()
                .unwrap();
            assert!(
                out.status.success(),
                "{}",
                String::from_utf8_lossy(&out.stderr)
            );
        };
        run(&["init", "-qb", "main"]);
        let commit = || {
            run(&[
                "-c",
                "user.name=Test",
                "-c",
                "user.email=test@example.invalid",
                "commit",
                "--allow-empty",
                "-qm",
                "commit",
            ])
        };
        commit();
        run(&["branch", "side"]);
        for _ in 0..80 {
            commit();
        }
        run(&["checkout", "-q", "side"]);
        for _ in 0..80 {
            run(&[
                "-c",
                "user.name=Test",
                "-c",
                "user.email=test@example.invalid",
                "commit",
                "--allow-empty",
                "-qm",
                "side commit",
            ]);
        }
        run(&["checkout", "-q", "main"]);
        run(&[
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@example.invalid",
            "merge",
            "--no-ff",
            "-qm",
            "merge",
            "side",
        ]);
        let short = crate::git::history(dir.path(), 128).await.unwrap();
        let full = crate::git::history(dir.path(), 256).await.unwrap();
        assert!(full.len() > 128);
        assert_eq!(short.len(), 128);
        assert_eq!(short, full[..128]);
        let short = construct(short, &AtomicBool::new(false)).unwrap().unwrap();
        let full = construct(full, &AtomicBool::new(false)).unwrap().unwrap();
        assert_eq!(
            short.rows[..short.rows.len() - 1],
            full.rows[..short.rows.len() - 1]
        );
        assert!(full.rows.iter().any(|row| row.oid.is_none()));
    }
    #[tokio::test]
    async fn rendered_states_viewport_resize_and_graph_shortcuts() {
        use crate::tui::{Action, View, draw, handle_key};
        use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
        use ratatui::{Terminal, backend::TestBackend};
        let (tx, mut rx) = mpsc::unbounded_channel();
        let mut view = View {
            history: History::with_backend(Arc::new(Controlled(tx))),
            git_focus: true,
            ..View::default()
        };
        let mut browser = crate::browser::Browser::new(vec![]);
        tick(&mut view.history).await;
        let mut terminal = Terminal::new(TestBackend::new(140, 40)).unwrap();
        let text = |terminal: &Terminal<TestBackend>| -> String {
            terminal
                .backend()
                .buffer()
                .content
                .iter()
                .map(|cell| cell.symbol())
                .collect()
        };
        terminal.draw(|frame| draw(frame, &browser, &view)).unwrap();
        assert!(text(&terminal).contains("loading…"));
        rx.recv().await.unwrap().1.send(Ok(rows(128))).unwrap();
        finish(&mut view.history).await;
        view.history.navigate(Movement::End);
        terminal.draw(|frame| draw(frame, &browser, &view)).unwrap();
        assert!(text(&terminal).contains("commit 127"));
        assert!(!text(&terminal).contains("commit 0 "));
        let selected = view.history.rows[view.history.selected].oid.clone();
        terminal.backend_mut().resize(100, 18);
        terminal
            .resize(ratatui::layout::Rect::new(0, 0, 100, 18))
            .unwrap();
        terminal.draw(|frame| draw(frame, &browser, &view)).unwrap();
        assert_eq!(view.history.rows[view.history.selected].oid, selected);
        assert!(text(&terminal).contains("commit 127"));
        assert!(matches!(
            handle_key(
                &mut browser,
                &mut view,
                KeyEvent::new(KeyCode::Char('r'), KeyModifiers::NONE)
            ),
            Action::Refresh
        ));
        handle_key(
            &mut browser,
            &mut view,
            KeyEvent::new(KeyCode::Char('?'), KeyModifiers::NONE),
        );
        assert!(view.help);
        view.help = false;
        view.history.error = Some("offline".into());
        terminal.draw(|frame| draw(frame, &browser, &view)).unwrap();
        assert!(text(&terminal).contains("offline"));
        assert!(text(&terminal).contains("commit 127"));
        view.history.replace(vec![], 128);
        terminal.draw(|frame| draw(frame, &browser, &view)).unwrap();
        assert!(text(&terminal).contains("Git history — empty"));
        view.history.head = Some(crate::git::Head {
            oid: None,
            branch: Some("new-branch".into()),
        });
        terminal.draw(|frame| draw(frame, &browser, &view)).unwrap();
        assert!(text(&terminal).contains("no commits yet"));
        view.history.replace(rows(8), 128);
        view.history.head = Some(crate::git::Head {
            oid: Some(format!("{:040x}", 3)),
            branch: None,
        });
        view.history.selected = 2;
        terminal
            .resize(ratatui::layout::Rect::new(0, 0, 140, 40))
            .unwrap();
        terminal.backend_mut().resize(140, 40);
        terminal.draw(|frame| draw(frame, &browser, &view)).unwrap();
        let screen = text(&terminal);
        assert!(screen.contains("detached HEAD"));
        assert!(screen.contains("[HEAD] commit 3"));
        assert!(screen.contains("> ● 0000000 commit 2"));
        assert_eq!(screen.matches("[HEAD]").count(), 1);
    }
    type HeadReply = oneshot::Sender<Result<crate::git::Head, String>>;
    struct CheckoutBackend {
        heads: mpsc::UnboundedSender<HeadReply>,
        reads: mpsc::UnboundedSender<(usize, Option<String>, Reply)>,
    }
    impl Backend for CheckoutBackend {
        fn head(&self, _: PathBuf) -> BackendFuture<Result<crate::git::Head, String>> {
            let (tx, rx) = oneshot::channel();
            self.heads.send(tx).unwrap();
            Box::pin(async move { rx.await.unwrap() })
        }
        fn history(
            &self,
            path: PathBuf,
            limit: usize,
        ) -> BackendFuture<Result<Vec<CommitRecord>, String>> {
            self.history_at(path, limit, None)
        }
        fn history_at(
            &self,
            _: PathBuf,
            limit: usize,
            head: Option<String>,
        ) -> BackendFuture<Result<Vec<CommitRecord>, String>> {
            let (tx, rx) = oneshot::channel();
            self.reads.send((limit, head, tx)).unwrap();
            Box::pin(async move { rx.await.unwrap().map(input_records) })
        }
    }
    async fn pump(h: &mut History) {
        tokio::task::yield_now().await;
        tick(h).await;
        tokio::time::timeout(std::time::Duration::from_secs(2), async {
            while h.build.is_some() {
                tokio::task::yield_now().await;
                tick(h).await;
            }
        })
        .await
        .unwrap();
    }
    #[tokio::test]
    async fn follows_old_head_manual_refresh_and_resume() {
        let (htx, mut heads) = mpsc::unbounded_channel();
        let (rtx, mut reads) = mpsc::unbounded_channel();
        let mut h = History::with_backend(Arc::new(CheckoutBackend {
            heads: htx,
            reads: rtx,
        }));
        let oid = format!("{:040x}", 180);
        let head = crate::git::Head {
            oid: Some(oid.clone()),
            branch: Some("feature".into()),
        };
        h.checkout(Some("/checkout".into()), false).await;
        tick(&mut h).await;
        heads.recv().await.unwrap().send(Ok(head.clone())).unwrap();
        pump(&mut h).await;
        let (limit, pin, reply) = reads.recv().await.unwrap();
        assert_eq!(limit, 128);
        assert_eq!(pin, Some(oid.clone()));
        reply.send(Ok(rows(128))).unwrap();
        pump(&mut h).await;
        let (limit, _, reply) = reads.recv().await.unwrap();
        assert_eq!(limit, 256);
        assert!(h.title().contains("locating HEAD"));
        reply.send(Ok(rows(220))).unwrap();
        finish(&mut h).await;
        assert_eq!(h.selected, 180);
        assert!(h.is_head(&h.rows[180]));
        h.navigate(Movement::Up);
        assert!(!h.following);
        assert_eq!(h.selected, 179);
        h.checkout(Some("/checkout".into()), false).await;
        assert!(!h.following);
        h.tick(Some("/repo".into()), 0, false, 20).await;
        assert!(!h.loading());
        h.tick(Some("/repo".into()), 0, true, 20).await;
        heads.recv().await.unwrap().send(Ok(head.clone())).unwrap();
        pump(&mut h).await;
        reads.recv().await.unwrap().2.send(Ok(rows(220))).unwrap();
        finish(&mut h).await;
        assert_eq!(h.selected, 179);
        assert!(!h.following);
        h.tick(Some("/repo".into()), 1, true, 20).await;
        heads.recv().await.unwrap().send(Ok(head.clone())).unwrap();
        tokio::task::yield_now().await;
        h.tick(Some("/repo".into()), 1, true, 20).await;
        reads.recv().await.unwrap().2.send(Ok(rows(220))).unwrap();
        tokio::task::yield_now().await;
        h.tick(Some("/repo".into()), 1, true, 20).await;
        assert_eq!(h.selected, 179);
        assert!(!h.following);
        h.follow();
        h.tick(Some("/repo".into()), 1, true, 20).await;
        heads.recv().await.unwrap().send(Ok(head)).unwrap();
        tokio::task::yield_now().await;
        h.tick(Some("/repo".into()), 1, true, 20).await;
        reads.recv().await.unwrap().2.send(Ok(rows(220))).unwrap();
        tokio::task::yield_now().await;
        h.tick(Some("/repo".into()), 1, true, 20).await;
        while h.build.is_some() {
            tokio::task::yield_now().await;
            h.tick(Some("/repo".into()), 1, true, 20).await;
        }
        assert_eq!(h.selected, 180);
        assert!(h.following);
        h.navigate(Movement::Home);
        h.checkout(Some("/checkout".into()), true).await;
        assert!(h.following);
    }
    #[tokio::test]
    async fn checkout_switch_and_manual_navigation_cancel_head_search() {
        let (htx, mut heads) = mpsc::unbounded_channel();
        let (rtx, mut reads) = mpsc::unbounded_channel();
        let mut h = History::with_backend(Arc::new(CheckoutBackend {
            heads: htx,
            reads: rtx,
        }));
        h.checkout(Some("/one".into()), false).await;
        tick(&mut h).await;
        let reply = heads.recv().await.unwrap();
        h.checkout(Some("/two".into()), false).await;
        assert!(reply.is_closed());
        tick(&mut h).await;
        heads
            .recv()
            .await
            .unwrap()
            .send(Ok(crate::git::Head {
                oid: Some(format!("{:040x}", 180)),
                branch: None,
            }))
            .unwrap();
        pump(&mut h).await;
        reads.recv().await.unwrap().2.send(Ok(rows(128))).unwrap();
        pump(&mut h).await;
        let (_, _, reply) = reads.recv().await.unwrap();
        h.navigate(Movement::Home);
        tick(&mut h).await;
        assert!(reply.is_closed());
        assert!(!h.following);
        assert_eq!(h.selectable.len(), 128);
        tick(&mut h).await;
        assert!(reads.try_recv().is_err());
        h.follow();
        tick(&mut h).await;
        heads
            .recv()
            .await
            .unwrap()
            .send(Err("missing checkout".into()))
            .unwrap();
        pump(&mut h).await;
        assert!(h.error.is_some());
        assert_eq!(h.selectable.len(), 128);
        assert!(!h.loading());
        tick(&mut h).await;
        assert!(heads.try_recv().is_err());
    }
    #[tokio::test]
    async fn native_head_handles_unborn_bare_and_unreachable_detached_commit() {
        let dir = tempfile::tempdir().unwrap();
        let run = |args: &[&str]| {
            let output = std::process::Command::new("git")
                .arg("-C")
                .arg(dir.path())
                .args(args)
                .env("GIT_CONFIG_GLOBAL", "/dev/null")
                .env("GIT_CONFIG_NOSYSTEM", "1")
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
        };
        run(&["init", "-qb", "main"]);
        assert_eq!(
            crate::git::head(dir.path()).await.unwrap(),
            crate::git::Head {
                oid: None,
                branch: Some("main".into())
            }
        );
        run(&[
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@example.invalid",
            "commit",
            "--allow-empty",
            "-qm",
            "root",
        ]);
        let linked = dir.path().join("linked");
        run(&[
            "worktree",
            "add",
            "-q",
            "--detach",
            linked.to_str().unwrap(),
        ]);
        let result = std::process::Command::new("git")
            .arg("-C")
            .arg(&linked)
            .args([
                "-c",
                "user.name=Test",
                "-c",
                "user.email=test@example.invalid",
                "-c",
                "commit.gpgsign=false",
                "commit",
                "--allow-empty",
                "-qm",
                "detached-only",
            ])
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .output()
            .unwrap();
        assert!(result.status.success());
        let head = crate::git::head(&linked).await.unwrap();
        assert!(head.branch.is_none());
        let refs = std::process::Command::new("git")
            .arg("-C")
            .arg(dir.path())
            .args(["for-each-ref", "--format=%(objectname)"])
            .output()
            .unwrap();
        assert!(
            !String::from_utf8_lossy(&refs.stdout)
                .lines()
                .any(|oid| Some(oid) == head.oid.as_deref())
        );
        assert!(
            crate::git::history_at(dir.path(), 128, head.oid.as_deref())
                .await
                .unwrap()
                .iter()
                .any(|r| Some(&r.oid) == head.oid.as_ref())
        );
        let bare = dir.path().join("bare.git");
        run(&["init", "--bare", "-q", bare.to_str().unwrap()]);
        assert!(crate::git::checkout_path(&bare).await.unwrap().is_none());
    }
    struct RefBackend {
        refs: mpsc::UnboundedSender<oneshot::Sender<Result<crate::git::References, String>>>,
        reads: mpsc::UnboundedSender<(usize, Reply)>,
    }
    impl Backend for RefBackend {
        fn references(&self, _: PathBuf) -> BackendFuture<Result<crate::git::References, String>> {
            let (tx, rx) = oneshot::channel();
            self.refs.send(tx).unwrap();
            Box::pin(async move { rx.await.unwrap() })
        }
        fn history(
            &self,
            _: PathBuf,
            limit: usize,
        ) -> BackendFuture<Result<Vec<CommitRecord>, String>> {
            let (tx, rx) = oneshot::channel();
            self.reads.send((limit, tx)).unwrap();
            Box::pin(async move { rx.await.unwrap().map(input_records) })
        }
    }
    #[tokio::test]
    async fn references_cache_expand_fail_recover_and_reject_obsolete() {
        let (tx, mut refs) = mpsc::unbounded_channel();
        let (reads_tx, mut reads) = mpsc::unbounded_channel();
        let mut h = History::with_backend(Arc::new(RefBackend {
            refs: tx,
            reads: reads_tx,
        }));
        let oid = format!("{:040x}", 0);
        let labels = crate::git::References::from([(
            oid.clone(),
            vec![crate::git::RefLabel {
                kind: crate::git::RefKind::Local,
                name: "main".into(),
            }],
        )]);
        tick(&mut h).await;
        refs.recv().await.unwrap().send(Ok(labels.clone())).unwrap();
        reads.recv().await.unwrap().1.send(Ok(rows(128))).unwrap();
        finish(&mut h).await;
        h.navigate(Movement::End);
        tick(&mut h).await;
        assert!(refs.try_recv().is_err());
        reads.recv().await.unwrap().1.send(Ok(rows(200))).unwrap();
        finish(&mut h).await;
        assert_eq!(h.references, labels);
        h.tick(Some("/repo".into()), 1, true, 20).await;
        refs.recv()
            .await
            .unwrap()
            .send(Err("offline".into()))
            .unwrap();
        reads.recv().await.unwrap().1.send(Ok(rows(200))).unwrap();
        tokio::task::yield_now().await;
        h.tick(Some("/repo".into()), 1, true, 20).await;
        assert_eq!(h.references, labels);
        assert!(h.title().contains("references unavailable/stale"));
        assert_eq!(h.selectable.len(), 200);
        h.tick(Some("/repo".into()), 2, true, 20).await;
        refs.recv()
            .await
            .unwrap()
            .send(Ok(crate::git::References::new()))
            .unwrap();
        reads.recv().await.unwrap().1.send(Ok(rows(200))).unwrap();
        tokio::task::yield_now().await;
        h.tick(Some("/repo".into()), 2, true, 20).await;
        assert!(h.references.is_empty());
        assert!(h.reference_error.is_none());
        h.tick(Some("/repo".into()), 3, true, 20).await;
        let old = refs.recv().await.unwrap();
        let old_read = reads.recv().await.unwrap().1;
        h.tick(Some("/new".into()), 4, true, 20).await;
        assert!(old.is_closed());
        assert!(old_read.is_closed());
        assert!(h.references.is_empty());
        h.shutdown().await;
    }
    #[test]
    fn horizontal_scrolling_preserves_selection_and_styles() {
        use crate::git::{RefKind, RefLabel};
        let mut h = History::default();
        h.replace(rows(2), 128);
        h.rows[0].subject = "wide 界 text ".repeat(30);
        h.head = Some(crate::git::Head {
            oid: h.rows[0].oid.clone(),
            branch: Some("main".into()),
        });
        h.following = true;
        h.references.insert(
            h.rows[0].oid.clone().unwrap(),
            vec![
                RefLabel {
                    kind: RefKind::Local,
                    name: "main".into(),
                },
                RefLabel {
                    kind: RefKind::Remote,
                    name: "origin/main".into(),
                },
                RefLabel {
                    kind: RefKind::Tag,
                    name: "v1".into(),
                },
            ],
        );
        let spans = h.row_spans(&h.rows[0]);
        let text = spans.iter().map(|s| s.content.as_ref()).collect::<String>();
        assert!(text.contains("[HEAD] [main] [remote: origin/main] [tag: v1]"));
        assert!(spans.iter().any(|s|s.content=="[main] " && s.style.fg==Some(ratatui::style::Color::Green)));
        h.scroll_horizontal(true);
        h.clamp_horizontal(50);
        assert_eq!(h.horizontal.get(), 4);
        assert!(!h.following);
        assert_eq!(h.selected, 0);
        h.scroll_horizontal(false);
        assert_eq!(h.horizontal.get(), 0);
        h.scroll_horizontal(true);
        h.clamp_horizontal(10000);
        assert_eq!(h.horizontal.get(), 0);
        h.scroll_horizontal(true);
        h.follow();
        assert_eq!(h.horizontal.get(), 0);
    }
    #[tokio::test]
    async fn cached_graph_is_reused_for_navigation_resize_and_idle() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let mut h = History::with_backend(Arc::new(Controlled(tx)));
        tick(&mut h).await;
        rx.recv().await.unwrap().1.send(Ok(rows(100))).unwrap();
        finish(&mut h).await;
        let builds = h.builds_started;
        let pointer = h.rows.as_ptr();
        for width in [20, 80, 200].into_iter().cycle().take(30) {
            h.navigate(Movement::Down);
            h.navigate(Movement::Up);
            h.scroll_horizontal(true);
            h.clamp_horizontal(width);
            for i in h.visible_range(12) {
                let _ = h.row_spans(&h.rows[i]);
            }
            tick(&mut h).await;
        }
        h.request_refresh();
        tick(&mut h).await;
        rx.recv().await.unwrap().1.send(Ok(rows(100))).unwrap();
        finish(&mut h).await;
        assert_eq!(h.builds_started, builds);
        assert_eq!(h.rows.as_ptr(), pointer);
        assert!(rx.try_recv().is_err());
    }

    #[tokio::test]
    async fn shutdown_joins_construction_and_never_publishes_obsolete_snapshot() {
        let mut h = History::default();
        let cancel = Arc::new(AtomicBool::new(false));
        let signal = cancel.clone();
        let exited = Arc::new(AtomicBool::new(false));
        let finished = exited.clone();
        let (started_tx, started_rx) = oneshot::channel();
        let handle = tokio::task::spawn_blocking(move || {
            let _ = started_tx.send(());
            while !signal.load(Ordering::Relaxed) {
                std::thread::yield_now();
            }
            // Even a completed result from obsolete work must be discarded.
            finished.store(true, Ordering::Relaxed);
            construct(input_records(rows(3)), &AtomicBool::new(false))
        });
        h.build = Some(BuildTask {
            token: h.token,
            limit: 128,
            cancel,
            handle,
        });
        started_rx.await.unwrap();
        h.shutdown().await;
        assert!(exited.load(Ordering::Relaxed));
        assert!(h.rows.is_empty());
        assert!(!h.loading());
    }

    #[test]
    fn cancelled_snapshot_is_not_published() {
        assert!(
            construct(input_records(rows(100)), &AtomicBool::new(true))
                .unwrap()
                .is_none()
        );
    }
    #[test]
    fn scroll_indicators_distinguish_commits_boundaries_and_loadable_history() {
        let mut h = History::default();
        assert_eq!(h.scroll_availability(&(0..10)), (false, false));
        h.replace(rows(20), 128);
        assert_eq!(h.scroll_availability(&(0..5)), (false, true));
        assert_eq!(h.scroll_availability(&(5..10)), (true, true));
        assert_eq!(h.scroll_availability(&(15..20)), (true, false));
        assert_eq!(h.scroll_availability(&(0..20)), (false, false));
        h.exhausted = false;
        assert_eq!(h.scroll_availability(&(0..20)), (false, true));
        h.error = Some("offline".into());
        assert_eq!(h.scroll_availability(&(15..20)), (true, true));
        h.exhausted = true;
        // Unresolved ancestry and connectors are not selectable commits.
        h.rows.push(GraphRow {
            graph: "┊".into(),
            graph_spans: vec![],
            oid: None,
            parents: vec![],
            subject: String::new(),
        });
        assert_eq!(h.scroll_availability(&(0..20)), (false, false));
        h.selected = 15;
        let range = h.visible_range(5);
        assert_eq!(h.scroll_availability(&range), (true, true));
        assert_eq!(h.selected, 15);
        assert_eq!(h.builds_started, 0);
        h.replace(vec![], 128);
        assert_eq!(h.scroll_availability(&(0..5)), (false, false));
    }

    #[tokio::test]
    async fn border_arrows_survive_long_titles_resize_and_clear_at_boundaries() {
        use crate::tui::{View, draw};
        use ratatui::{
            Terminal,
            backend::TestBackend,
            layout::Rect,
            style::{Color, Modifier},
        };
        let (tx, mut rx) = mpsc::unbounded_channel();
        let mut view = View {
            history: History::with_backend(Arc::new(Controlled(tx))),
            ..View::default()
        };
        let browser = crate::browser::Browser::new(vec![]);
        tick(&mut view.history).await;
        rx.recv().await.unwrap().1.send(Ok(rows(80))).unwrap();
        finish(&mut view.history).await;
        view.history.head = Some(crate::git::Head {
            oid: view.history.rows[40].oid.clone(),
            branch: Some("界long-branch".repeat(40)),
        });
        view.history.following = true;
        let mut terminal = Terminal::new(TestBackend::new(140, 40)).unwrap();
        for (width, height, focused) in [(140, 40, false), (100, 18, true), (40, 18, false)] {
            terminal.backend_mut().resize(width, height);
            terminal.resize(Rect::new(0, 0, width, height)).unwrap();
            view.git_focus = focused;
            for (selected, expected) in
                [(40, (true, true)), (0, (false, true)), (79, (true, false))]
            {
                view.history.selected = selected;
                view.history.top.set(selected.saturating_sub(2));
                terminal.draw(|frame| draw(frame, &browser, &view)).unwrap();
                let buffer = terminal.backend().buffer();
                let arrows: Vec<_> = buffer
                    .content
                    .iter()
                    .enumerate()
                    .filter(|(_, cell)| {
                        matches!(cell.symbol(), "↑" | "↓")
                            && cell.fg == Color::Cyan
                            && cell.modifier.contains(Modifier::BOLD)
                    })
                    .collect();
                assert_eq!(arrows.iter().any(|(_, c)| c.symbol() == "↑"), expected.0);
                assert_eq!(arrows.iter().any(|(_, c)| c.symbol() == "↓"), expected.1);
                for (index, cell) in arrows {
                    assert_eq!(index % usize::from(width), usize::from(width - 3));
                    assert_eq!(cell.fg, Color::Cyan);
                    assert!(cell.modifier.contains(Modifier::BOLD));
                    let row = index / usize::from(width);
                    assert_ne!(
                        buffer.content[row * usize::from(width) + usize::from(width - 1)].symbol(),
                        " "
                    );
                }
                assert_eq!(view.history.selected, selected);
                assert!(view.history.following);
            }
        }
        view.history.replace(rows(1), 128);
        terminal.draw(|frame| draw(frame, &browser, &view)).unwrap();
        assert!(
            !terminal
                .backend()
                .buffer()
                .content
                .iter()
                .any(|c| matches!(c.symbol(), "↑" | "↓")
                    && c.fg == Color::Cyan
                    && c.modifier.contains(Modifier::BOLD))
        );
        assert_eq!(view.history.builds_started, 1);
        assert!(rx.try_recv().is_err());
    }
}
