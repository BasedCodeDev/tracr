//! Live annotations over immutable Git geometry. This module performs no I/O.
use crate::{
    browser::Browser,
    domain::{RuntimeStatus, SessionKey, SessionSummary},
    git::{GraphRow, RefKind},
    git_discovery::Outcome,
    git_history::{History, Movement},
};
use ratatui::{style::Color, text::Line};
use std::{collections::BTreeMap, path::PathBuf};

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Selection {
    Commit(String),
    Session(SessionKey),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Annotation {
    pub session: SessionSummary,
    pub status: RuntimeStatus,
    pub oid: String,
    pub root: PathBuf,
    pub label: Option<String>,
    branch: Option<String>,
}

fn priority(status: RuntimeStatus) -> u8 {
    match status {
        RuntimeStatus::Waiting { .. } => 0,
        RuntimeStatus::Error => 1,
        RuntimeStatus::Running => 2,
        RuntimeStatus::Idle => 3,
        _ => 4,
    }
}

pub fn annotations(browser: &Browser, history: &History) -> Vec<Annotation> {
    let Some(id) = history.repository() else {
        return vec![];
    };
    let Some(repo) = browser.repositories.get(&id) else {
        return vec![];
    };
    let mut result = Vec::new();
    for session in browser.all() {
        let status = browser.runtime_status(&session.key);
        if priority(status) == 4 || (session.archived && !browser.include_archived) {
            continue;
        }
        let Some(cwd) = &session.cwd else {
            continue;
        };
        let Some(owner) = browser.ownership.get(cwd) else {
            continue;
        };
        if owner.generation != browser.git_generation {
            continue;
        }
        let Some(Outcome::Repository {
            repository,
            root: Some(root),
        }) = owner.effective()
        else {
            continue;
        };
        if repository.common_dir != id {
            continue;
        }
        let Some(worktree) = repo.worktrees.iter().find(|w| !w.bare && &w.path == root) else {
            continue;
        };
        let Some(oid) = &worktree.head else {
            continue;
        };
        let peers: Vec<_> = repo
            .worktrees
            .iter()
            .filter(|w| !w.bare && w.head.as_ref() == Some(oid))
            .collect();
        let name = |w: &crate::git::Worktree| {
            w.branch
                .as_deref()
                .map(|b| b.trim_start_matches("refs/heads/"))
                .unwrap_or("detached")
                .to_string()
        };
        let label = (peers.len() > 1).then(|| {
            let branch = name(worktree);
            if peers.iter().filter(|w| name(w) == branch).count() > 1 {
                // Full paths distinguish worktrees even when basenames coincide.
                format!("{branch} · {}", root.display())
            } else {
                branch
            }
        });
        result.push(Annotation {
            session,
            status,
            oid: oid.clone(),
            root: root.clone(),
            label,
            branch: worktree
                .branch
                .as_deref()
                .and_then(|name| name.strip_prefix("refs/heads/"))
                .map(crate::browser::safe_text),
        });
    }
    result.sort_by(|a, b| {
        a.oid
            .cmp(&b.oid)
            .then(priority(a.status).cmp(&priority(b.status)))
            .then(b.session.created_at.cmp(&a.session.created_at))
            .then(a.session.key.cmp(&b.session.key))
    });
    result
}

#[derive(Clone, Debug)]
pub enum Row {
    Graph(usize),
    References {
        anchor: usize,
        line: Line<'static>,
    },
    Session {
        anchor: usize,
        annotation: usize,
        connector: Option<usize>,
        heading: Option<usize>,
        last: bool,
    },
}

pub enum CommitSection {
    References(Vec<Line<'static>>),
    Branch {
        heading: Vec<Line<'static>>,
        sessions: Vec<usize>,
    },
    Sessions(Vec<usize>),
}

/// Share the first connector rows with the final sessions in each commit block.
/// Branch headings and earlier sessions extend the lanes before routing begins.
pub fn compose_rows(
    rows: &[GraphRow],
    tails: &BTreeMap<usize, std::ops::Range<usize>>,
    session_oids: &[&str],
    sections: impl Fn(&str, &[usize]) -> Vec<CommitSection>,
) -> Vec<Row> {
    let mut attached: BTreeMap<&str, Vec<usize>> = BTreeMap::new();
    for (index, oid) in session_oids.iter().enumerate() {
        attached.entry(oid).or_default().push(index);
    }
    let mut result = Vec::new();
    let mut i = 0;
    while i < rows.len() {
        result.push(Row::Graph(i));
        if let Some(oid) = rows[i].oid.as_deref() {
            let indices = attached.get(oid).map_or(&[][..], Vec::as_slice);
            for section in sections(oid, indices) {
                let (heading, sessions) = match section {
                    CommitSection::References(lines) => {
                        result.extend(
                            lines
                                .into_iter()
                                .map(|line| Row::References { anchor: i, line }),
                        );
                        continue;
                    }
                    CommitSection::Branch { heading, sessions } => {
                        let start = result.len();
                        result.extend(
                            heading
                                .into_iter()
                                .map(|line| Row::References { anchor: i, line }),
                        );
                        (Some(start), sessions)
                    }
                    CommitSection::Sessions(sessions) => (None, sessions),
                };
                let len = sessions.len();
                for (position, annotation) in sessions.into_iter().enumerate() {
                    result.push(Row::Session {
                        anchor: i,
                        annotation,
                        connector: None,
                        heading,
                        last: position + 1 == len,
                    });
                }
            }
            let tail = tails.get(&i).cloned().unwrap_or(i + 1..i + 1);
            let shared = result
                .iter()
                .rev()
                .take(tail.len())
                .take_while(|row| matches!(row, Row::Session { .. }))
                .count();
            let start = result.len() - shared;
            for (row, index) in result[start..].iter_mut().zip(tail) {
                if let Row::Session { connector, .. } = row {
                    *connector = Some(index);
                }
            }
            i += shared;
        }
        i += 1;
    }
    result
}

fn commit_sections(
    history: &History,
    oid: &str,
    width: usize,
    indices: &[usize],
    annotations: &[Annotation],
    branch_colors: &BTreeMap<&str, (u8, Color)>,
) -> Vec<CommitSection> {
    let labels = history.reference_labels(oid);
    let local_branches: std::collections::BTreeSet<_> = labels
        .iter()
        .filter(|label| label.kind == RefKind::Local)
        .map(|label| label.name.as_str())
        .collect();
    let mut attached: BTreeMap<&str, Vec<usize>> = BTreeMap::new();
    let mut unmatched = Vec::new();
    for &index in indices {
        match annotations[index].branch.as_deref() {
            Some(branch) if local_branches.contains(branch) => {
                attached.entry(branch).or_default().push(index)
            }
            _ => unmatched.push(index),
        }
    }

    let mut sections = Vec::new();
    let mut feature_groups = Vec::new();
    let mut compact = Vec::new();
    for label in labels {
        let local = (label.kind == RefKind::Local).then_some(label.name.as_str());
        let color = local
            .and_then(|name| branch_colors.get(name))
            .map(|&(_, color)| color);
        let line = history.reference_label(label, color);
        let Some(sessions) = local.and_then(|name| attached.remove(name)) else {
            compact.push(line);
            continue;
        };
        let group = CommitSection::Branch {
            heading: history.wrap_reference_labels(width, [line]),
            sessions,
        };
        if History::primary_branch_rank(label).is_some() {
            if !compact.is_empty() {
                sections.push(CommitSection::References(
                    history.wrap_reference_labels(width, std::mem::take(&mut compact)),
                ));
            }
            sections.push(group);
        } else {
            feature_groups.push(group);
        }
    }
    if !compact.is_empty() {
        sections.push(CommitSection::References(
            history.wrap_reference_labels(width, compact),
        ));
    }
    sections.extend(feature_groups);
    if !unmatched.is_empty() {
        sections.push(CommitSection::Sessions(unmatched));
    }
    sections
}

#[derive(Default)]
pub struct Presentation {
    pub rows: Vec<Row>,
    pub annotations: Vec<Annotation>,
    pub selected: usize,
    selectable: Vec<usize>,
    positions: BTreeMap<Selection, usize>,
    pub top: usize,
    selection: Option<Selection>,
    history_oid: Option<String>,
    anchor_oid: Option<String>,
    repository: Option<PathBuf>,
    geometry: (u64, usize, usize),
    pub width: Option<usize>,
    reference_layout: (Option<usize>, u64),
    reference_offset: usize,
    height: usize,
}
impl Presentation {
    pub fn sync(&mut self, browser: &Browser, history: &History) {
        let repository = history.repository();
        let history_oid = history
            .rows
            .get(history.selected)
            .and_then(|r| r.oid.clone());
        let geometry = (
            history.revision(),
            history.rows.as_ptr() as usize,
            history.rows.len(),
        );
        let annotations = annotations(browser, history);
        let old_offset = self.cursor().saturating_sub(self.top);
        let old_selection = self.selection.clone();
        let old_anchor = self.anchor_oid.clone();
        let repository_changed = self.repository != repository;
        if repository_changed
            || history.following
            || (self.history_oid != history_oid
                && !matches!(self.selection, Some(Selection::Session(_))))
        {
            self.selection = history_oid.clone().map(Selection::Commit);
        }
        let reference_layout = (self.width, history.reference_revision);
        let rebuild = repository_changed
            || self.geometry != geometry
            || self.annotations != annotations
            || self.reference_layout != reference_layout;
        if rebuild {
            self.annotations = annotations;
            let oids: Vec<_> = self.annotations.iter().map(|a| a.oid.as_str()).collect();
            let mut branch_colors = BTreeMap::new();
            for annotation in &self.annotations {
                if let (Some(branch), Some(color)) = (
                    annotation.branch.as_deref(),
                    crate::tui::activity_color(annotation.status),
                ) {
                    let rank = priority(annotation.status);
                    let entry = branch_colors.entry(branch).or_insert((rank, color));
                    if rank < entry.0 {
                        *entry = (rank, color);
                    }
                }
            }
            self.rows = compose_rows(&history.rows, &history.tails, &oids, |oid, indices| {
                commit_sections(
                    history,
                    oid,
                    self.width.unwrap_or(usize::MAX),
                    indices,
                    &self.annotations,
                    &branch_colors,
                )
            });
            self.positions = self
                .rows
                .iter()
                .enumerate()
                .filter_map(|(i, row)| self.key(row, history).map(|key| (key, i)))
                .collect();
            self.selectable = self
                .rows
                .iter()
                .enumerate()
                .filter_map(|(i, row)| self.key(row, history).map(|_| i))
                .collect();
            self.geometry = geometry;
            self.reference_layout = reference_layout;
            self.repository = repository;
        }
        let locate = |key: &Selection| self.positions.get(key).copied();
        self.selected = self
            .selection
            .as_ref()
            .and_then(locate)
            .or_else(|| {
                old_anchor
                    .as_ref()
                    .and_then(|oid| locate(&Selection::Commit(oid.clone())))
            })
            .or_else(|| {
                history_oid
                    .as_ref()
                    .and_then(|oid| locate(&Selection::Commit(oid.clone())))
            })
            .unwrap_or(0);
        self.selection = self
            .rows
            .get(self.selected)
            .and_then(|row| self.key(row, history));
        if rebuild || self.selection != old_selection {
            self.reference_offset = if self.selection == old_selection {
                self.reference_offset
                    .min(self.reference_end() - self.selected)
            } else {
                0
            };
            self.top = self.cursor().saturating_sub(old_offset);
        }
        self.anchor_oid = self
            .anchor()
            .and_then(|i| history.rows.get(i))
            .and_then(|r| r.oid.clone());
        self.history_oid = history_oid;
    }
    fn key(&self, row: &Row, history: &History) -> Option<Selection> {
        match row {
            Row::Graph(i) => history.rows.get(*i)?.oid.clone().map(Selection::Commit),
            Row::References { .. } => None,
            Row::Session { annotation, .. } => Some(Selection::Session(
                self.annotations[*annotation].session.key.clone(),
            )),
        }
    }
    pub fn session(&self) -> Option<&Annotation> {
        match self.rows.get(self.selected)? {
            Row::Session { annotation, .. } => self.annotations.get(*annotation),
            _ => None,
        }
    }
    pub fn anchor(&self) -> Option<usize> {
        match self.rows.get(self.selected)? {
            Row::Graph(i) => Some(*i),
            Row::References { anchor, .. } | Row::Session { anchor, .. } => Some(*anchor),
        }
    }
    pub fn cursor(&self) -> usize {
        self.selected + self.reference_offset
    }
    fn reference_end(&self) -> usize {
        self.rows
            .iter()
            .skip(self.selected + 1)
            .take_while(|row| matches!(row, Row::References { .. }))
            .count()
            + self.selected
    }
    pub fn range(&mut self, height: usize) -> std::ops::Range<usize> {
        self.height = height;
        self.top = self.top.min(self.rows.len().saturating_sub(height));
        let cursor = self.cursor();
        let mut first = cursor;
        let mut bottom = cursor;
        if self.reference_offset == 0 {
            match self.rows.get(self.selected) {
                Some(Row::Session {
                    heading: Some(heading),
                    ..
                }) if cursor - heading < height => {
                    first = *heading;
                }
                Some(Row::Graph(_)) => {
                    // Keep the commit subject and its leading references together when they fit.
                    let end = self.reference_end();
                    if end - self.selected < height {
                        bottom = end;
                    }
                }
                _ => {}
            }
        }
        self.top = self
            .top
            .min(first)
            .max((bottom + 1).saturating_sub(height.max(1)));
        self.top..(self.top + height).min(self.rows.len())
    }
    pub fn navigate(&mut self, movement: Movement, history: &mut History) {
        let selectable = &self.selectable;
        if selectable.is_empty() {
            return;
        }
        let current = selectable
            .partition_point(|i| *i < self.selected)
            .min(selectable.len() - 1);
        let end = self.reference_end();
        let reading_refs = match movement {
            Movement::Down
                if self.height > 0 && end > self.cursor() && end >= self.top + self.height =>
            {
                let next_page_end = self.top + self.height * 2 - 1;
                self.reference_offset = end.min(next_page_end) - self.selected;
                true
            }
            Movement::Up if self.reference_offset > 0 => {
                self.reference_offset = 0;
                true
            }
            _ => false,
        };
        let target = if reading_refs {
            self.selected
        } else {
            let target = match movement {
                Movement::Up => selectable[current.saturating_sub(1)],
                Movement::Down => selectable[(current + 1).min(selectable.len() - 1)],
                Movement::Home => selectable[0],
                Movement::End => *selectable.last().unwrap(),
                Movement::PageUp => {
                    selectable[selectable
                        .partition_point(|i| *i < self.selected.saturating_sub(history.page.get()))
                        .min(selectable.len() - 1)]
                }
                Movement::PageDown => {
                    selectable[selectable
                        .partition_point(|i| *i < self.selected.saturating_add(history.page.get()))
                        .min(selectable.len() - 1)]
                }
            };
            if target != self.selected || !matches!(movement, Movement::Down) {
                self.reference_offset = 0;
            }
            target
        };
        self.selected = target;
        self.selection = self.key(&self.rows[target], history);
        if let Some(anchor) = self.anchor() {
            history.select_row(anchor);
        }
        self.history_oid = history
            .rows
            .get(history.selected)
            .and_then(|r| r.oid.clone());
        self.anchor_oid = self.history_oid.clone();
    }
    pub fn follow(&mut self) {
        self.selection = None;
        self.history_oid = None;
        self.reference_offset = 0;
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::{
        browser::{SourceSpec, Update},
        domain::{ProviderId, SourceId},
        git::{Directory, GraphRow, Repository, Worktree},
    };

    fn is_last_session(view: &Presentation, index: usize) -> bool {
        view.rows.iter().any(|row| {
            matches!(
                row,
                Row::Session { annotation, last: true, .. } if *annotation == index
            )
        })
    }

    pub fn add_session(
        browser: &mut Browser,
        name: &str,
        cwd: &str,
        root: &str,
        oid_repo: &str,
        status: RuntimeStatus,
        created: i64,
    ) -> SessionKey {
        let key = SessionKey {
            provider: ProviderId("codex".into()),
            source_id: SourceId("test".into()),
            native_id: name.into(),
        };
        browser.apply(Update::Page {
            source: 0,
            generation: browser.generation,
            warnings: vec![],
            sessions: vec![SessionSummary {
                key: key.clone(),
                title: name.into(),
                cwd: Some(cwd.into()),
                created_at: created,
                updated_at: created,
                archived: false,
                can_accept_direct_input: None,
                source_kind: None,
                git: None,
            }],
        });
        browser.sources[0].live.insert(key.clone(), status);
        browser.apply_ownership(
            browser.git_generation,
            cwd.into(),
            Outcome::Repository {
                repository: Directory {
                    common_dir: oid_repo.into(),
                    git_dir: oid_repo.into(),
                },
                root: Some(root.into()),
            },
        );
        key
    }

    pub async fn fixture() -> (Browser, History) {
        let mut browser = Browser::new(vec![SourceSpec::Fixture("/fixture".into())]);
        browser.install_repository(Repository {
            common_dir: "/repo/.git".into(),
            git_dir: "/repo/.git".into(),
            worktrees: [("/repo", "main"), ("/worktree", "feature")]
                .into_iter()
                .map(|(path, branch)| Worktree {
                    path: path.into(),
                    head: Some("head".into()),
                    branch: Some(format!("refs/heads/{branch}")),
                    bare: false,
                    locked: false,
                    prunable: false,
                })
                .collect(),
        });
        let mut history = History::default();
        history.tick(Some("/repo/.git".into()), 0, false, 4).await;
        history.rows = ["head", "older"]
            .into_iter()
            .map(|oid| GraphRow {
                stash: None,
                graph: "●".into(),
                graph_spans: vec![crate::git::GraphSpan {
                    text: "●".into(),
                    color: Some(36),
                }],
                oid: Some(oid.into()),
                parents: if oid == "head" {
                    vec!["older".into()]
                } else {
                    vec![]
                },
                subject: oid.into(),
            })
            .collect();
        history.exhausted = true;
        (browser, history)
    }

    #[tokio::test]
    async fn shared_parent_gap_disappears_and_session_navigation_survives_updates() {
        use crate::{
            git::CommitRecord,
            tui::{View, draw, handle_key},
        };
        use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
        let (mut browser, mut history) = fixture().await;
        crate::tui::select_git_fixture(&mut browser, "/repo/.git");
        history.replace_records_for_test(vec![
            CommitRecord {
                stash: None,
                oid: "left".into(),
                parents: vec!["base".into()],
                subject: "left tip".into(),
            },
            CommitRecord {
                stash: None,
                oid: "right".into(),
                parents: vec!["base".into()],
                subject: "right tip".into(),
            },
            CommitRecord {
                stash: None,
                oid: "base".into(),
                parents: vec![],
                subject: "shared base".into(),
            },
        ]);
        let repo = browser
            .repositories
            .get_mut(std::path::Path::new("/repo/.git"))
            .unwrap();
        repo.worktrees[0].head = Some("left".into());
        repo.worktrees[1].head = Some("right".into());
        add_session(
            &mut browser,
            "session-left",
            "/repo",
            "/repo",
            "/repo/.git",
            RuntimeStatus::Idle,
            1,
        );
        let key = add_session(
            &mut browser,
            "session-right",
            "/worktree",
            "/worktree",
            "/repo/.git",
            RuntimeStatus::Running,
            1,
        );
        let right = history
            .rows
            .iter()
            .position(|r| r.oid.as_deref() == Some("right"))
            .unwrap();
        let connector = history.tails[&right].start;
        let original = history.rows.clone();
        let revision = history.revision();
        let mut view = View {
            drawer_open: true,
            drawer_git: true,
            dialog_area: true,
            history,
            git_focus: true,
            ..View::default()
        };
        for _ in 0..3 {
            handle_key(
                &mut browser,
                &mut view,
                KeyEvent::new(KeyCode::Down, KeyModifiers::NONE),
            );
        }
        {
            let presentation = view.git_sessions.borrow();
            assert_eq!(presentation.session().unwrap().session.key, key);
            assert!(
                matches!(presentation.rows[presentation.selected], Row::Session { connector: Some(i), .. } if i == connector)
            );
            assert!(
                matches!(presentation.rows[presentation.selected + 1], Row::Graph(i) if view.history.rows[i].oid.as_deref() == Some("base"))
            );
        }
        let mut terminal =
            ratatui::Terminal::new(ratatui::backend::TestBackend::new(120, 40)).unwrap();
        terminal.draw(|frame| draw(frame, &browser, &view)).unwrap();
        let buffer = terminal.backend().buffer();
        let lines: Vec<String> = (0..40)
            .map(|y| (0..120).map(|x| buffer[(x, y)].symbol()).collect())
            .collect();
        let session_row = lines
            .iter()
            .rposition(|s| s.contains("session-right"))
            .unwrap();
        assert!(lines[session_row + 1].contains("shared base"));
        let plain: String = view
            .history
            .session_spans(right, Some(connector))
            .iter()
            .map(|s| s.content.as_ref())
            .collect();
        assert!(lines[session_row].contains(plain.trim_end()));
        let extra = add_session(
            &mut browser,
            "newer-right",
            "/worktree",
            "/worktree",
            "/repo/.git",
            RuntimeStatus::Waiting {
                approval: true,
                input: false,
            },
            2,
        );
        {
            let mut presentation = view.git_sessions.borrow_mut();
            presentation.sync(&browser, &view.history);
            assert_eq!(presentation.session().unwrap().session.key, key);
            assert!(
                matches!(presentation.rows[presentation.selected], Row::Session { connector: Some(i), .. } if i == connector)
            );
            assert!(matches!(
                presentation.rows[presentation.selected - 1],
                Row::Session {
                    connector: None,
                    ..
                }
            ));
            browser.sources[0]
                .live
                .insert(key.clone(), RuntimeStatus::Unloaded);
            presentation.sync(&browser, &view.history);
            assert!(presentation.session().is_none());
            assert_eq!(presentation.anchor(), Some(right));
            assert!(presentation.rows.iter().any(|r| matches!(r, Row::Session { connector: Some(i), annotation, .. } if *i == connector && presentation.annotations[*annotation].session.key == extra)));
        }
        assert_eq!(view.history.rows, original);
        assert_eq!(view.history.revision(), revision);
        handle_key(
            &mut browser,
            &mut view,
            KeyEvent::new(KeyCode::Down, KeyModifiers::NONE),
        );
        handle_key(
            &mut browser,
            &mut view,
            KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
        );
        assert_eq!(browser.selected, Some(extra));
        assert_eq!(view.focus, 1);
        assert!(!view.git_focus);
        assert!(!view.drawer_open);
    }

    #[tokio::test]
    async fn ownership_states_age_order_and_ambiguous_labels() {
        let (mut browser, history) = fixture().await;
        use RuntimeStatus::*;
        for (name, root, status, created) in [
            ("old-running", "/repo", Running, 1),
            ("new-running", "/worktree", Running, 9),
            (
                "waiting",
                "/repo",
                Waiting {
                    approval: true,
                    input: false,
                },
                2,
            ),
            ("error", "/worktree", Error, 1),
            ("idle", "/repo", Idle, 10),
            ("unloaded", "/repo", Unloaded, 20),
            ("unknown", "/repo", Unknown, 30),
        ] {
            add_session(
                &mut browser,
                name,
                &format!("{root}/{name}"),
                root,
                "/repo/.git",
                status,
                created,
            );
        }
        add_session(
            &mut browser,
            "nested",
            "/repo/nested",
            "/repo/nested",
            "/repo/nested/.git",
            Running,
            40,
        );
        add_session(
            &mut browser,
            "unmapped",
            "/repo/unmapped",
            "/not-registered",
            "/repo/.git",
            Running,
            50,
        );
        let result = annotations(&browser, &history);
        assert_eq!(
            result
                .iter()
                .map(|s| s.session.title.as_str())
                .collect::<Vec<_>>(),
            ["waiting", "error", "new-running", "old-running", "idle"]
        );
        assert_eq!(result[0].label.as_deref(), Some("main"));
        assert_eq!(result[2].label.as_deref(), Some("feature"));
        browser.query = "does not match".into();
        browser.folder = crate::browser::FolderFilter::Unknown;
        assert_eq!(result, annotations(&browser, &history));
        let repo = browser
            .repositories
            .get_mut(std::path::Path::new("/repo/.git"))
            .unwrap();
        repo.worktrees[1].head = Some("older".into());
        repo.worktrees[1].branch = None;
        assert!(
            annotations(&browser, &history)
                .iter()
                .all(|a| a.label.is_none())
        );
        assert!(
            annotations(&browser, &history)
                .iter()
                .filter(|a| a.root == std::path::Path::new("/worktree"))
                .all(|a| a.oid == "older")
        );
        let mut archived = browser
            .all()
            .into_iter()
            .find(|s| s.title == "waiting")
            .unwrap();
        archived.archived = true;
        archived.updated_at += 1;
        browser.apply(Update::Page {
            source: 0,
            generation: browser.generation,
            sessions: vec![archived],
            warnings: vec![],
        });
        assert!(
            !annotations(&browser, &history)
                .iter()
                .any(|a| a.session.title == "waiting")
        );
        browser.include_archived = true;
        assert!(
            annotations(&browser, &history)
                .iter()
                .any(|a| a.session.title == "waiting")
        );
    }

    #[tokio::test]
    async fn branch_labels_follow_live_activity_in_their_worktrees() {
        use crate::git::{RefKind, RefLabel};
        use ratatui::style::Color;
        let (mut browser, mut history) = fixture().await;
        let running = add_session(
            &mut browser,
            "running",
            "/repo",
            "/repo",
            "/repo/.git",
            RuntimeStatus::Running,
            1,
        );
        let waiting = add_session(
            &mut browser,
            "waiting",
            "/repo/src",
            "/repo",
            "/repo/.git",
            RuntimeStatus::Waiting {
                input: true,
                approval: false,
            },
            2,
        );
        add_session(
            &mut browser,
            "feature",
            "/worktree",
            "/worktree",
            "/repo/.git",
            RuntimeStatus::Running,
            3,
        );
        history.set_worktrees(browser.repositories.get(std::path::Path::new("/repo/.git")));
        history.set_references(BTreeMap::from([(
            "head".into(),
            [
                (RefKind::Local, "main"),
                (RefKind::Local, "feature"),
                (RefKind::Local, "unattached"),
                (RefKind::Remote, "origin/main"),
                (RefKind::Tag, "main"),
            ]
            .into_iter()
            .map(|(kind, name)| RefLabel {
                kind,
                name: name.into(),
            })
            .collect(),
        )]));
        let mut view = Presentation {
            width: Some(80),
            ..Default::default()
        };
        let color = |view: &Presentation, text: &str| {
            view.rows
                .iter()
                .filter_map(|row| match row {
                    Row::References { line, .. } => Some(&line.spans),
                    _ => None,
                })
                .flatten()
                .find(|span| span.content == text)
                .and_then(|span| span.style.fg)
        };
        view.sync(&browser, &history);
        assert_eq!(color(&view, "[main]"), Some(Color::Rgb(255, 165, 0)));
        assert_eq!(color(&view, "feature"), Some(Color::Green));
        assert_eq!(color(&view, "◇"), Some(crate::git_render::WORKTREE_COLOR));
        assert_eq!(
            color(&view, "[unattached]"),
            Some(crate::git_render::BRANCH_COLOR)
        );
        assert_eq!(color(&view, "[remote: origin/main]"), Some(Color::Cyan));
        assert_eq!(color(&view, "[tag: main]"), Some(Color::Yellow));

        browser.sources[0]
            .live
            .insert(waiting, RuntimeStatus::Unloaded);
        view.sync(&browser, &history);
        assert_eq!(color(&view, "[main]"), Some(Color::Green));
        browser.sources[0]
            .live
            .insert(running.clone(), RuntimeStatus::Idle);
        view.sync(&browser, &history);
        assert_eq!(color(&view, "[main]"), Some(Color::Rgb(255, 255, 170)));
        browser.sources[0]
            .live
            .insert(running, RuntimeStatus::Unloaded);
        view.sync(&browser, &history);
        assert_eq!(
            color(&view, "[main]"),
            Some(crate::git_render::BRANCH_COLOR)
        );
    }

    #[tokio::test]
    async fn reference_reflow_preserves_sessions_and_graph_lanes() {
        use crate::git::{RefKind, RefLabel};
        let (mut browser, mut history) = fixture().await;
        browser
            .repositories
            .get_mut(std::path::Path::new("/repo/.git"))
            .unwrap()
            .worktrees[0]
            .branch = Some("refs/heads/feature/branch".into());
        let key = add_session(
            &mut browser,
            "attached",
            "/repo/src",
            "/repo",
            "/repo/.git",
            RuntimeStatus::Idle,
            1,
        );
        let mut view = Presentation {
            width: Some(40),
            ..Default::default()
        };
        view.sync(&browser, &history);
        view.navigate(Movement::Down, &mut history);
        assert_eq!(view.session().unwrap().session.key, key);

        history.set_references(BTreeMap::from([(
            "head".into(),
            vec![RefLabel {
                kind: RefKind::Local,
                name: "feature/branch".into(),
            }],
        )]));
        view.sync(&browser, &history);
        assert_eq!(view.session().unwrap().session.key, key);
        assert!(view.range(2).contains(&1));
        assert!(
            matches!(&view.rows[1], Row::References { anchor: 0, line } if line.to_string() == "[feature/branch]")
        );
        assert_eq!(history.continuation_spans(0)[0].content, "│");

        view.width = Some(10);
        view.sync(&browser, &history);
        assert!(view.selected > 2);
        assert_eq!(view.session().unwrap().session.key, key);

        view.navigate(Movement::Home, &mut history);
        let mut range = view.range(2);
        for _ in 0..view.rows.len() {
            view.navigate(Movement::Down, &mut history);
            let next = view.range(2);
            assert!(range.end >= next.start);
            range = next;
            if view.session().is_some() {
                break;
            }
        }
        assert_eq!(view.session().unwrap().session.key, key);
        assert!(range.contains(&view.selected));
    }

    #[tokio::test]
    async fn live_reordering_head_movement_and_removal_preserve_identity_and_geometry() {
        let (mut browser, mut history) = fixture().await;
        let first = add_session(
            &mut browser,
            "first",
            "/repo/src",
            "/repo",
            "/repo/.git",
            RuntimeStatus::Running,
            1,
        );
        let second = add_session(
            &mut browser,
            "second",
            "/worktree",
            "/worktree",
            "/repo/.git",
            RuntimeStatus::Idle,
            2,
        );
        let original = history.rows.clone();
        let revision = history.revision();
        let allocation = history.rows.as_ptr();
        let mut view = Presentation::default();
        view.sync(&browser, &history);
        assert!(!is_last_session(&view, 0));
        assert!(is_last_session(&view, 1));
        view.navigate(Movement::Down, &mut history);
        assert_eq!(view.session().unwrap().session.key, first);
        view.top = view.selected;
        browser.sources[0].live.insert(
            second,
            RuntimeStatus::Waiting {
                approval: false,
                input: true,
            },
        );
        view.sync(&browser, &history);
        assert_eq!(view.session().unwrap().session.key, first);
        assert_eq!(view.top, view.selected);
        let Row::Session { annotation, .. } = view.rows[view.selected] else {
            panic!("expected selected session")
        };
        assert!(is_last_session(&view, annotation));
        browser
            .repositories
            .get_mut(std::path::Path::new("/repo/.git"))
            .unwrap()
            .worktrees[0]
            .head = Some("older".into());
        view.sync(&browser, &history);
        assert_eq!(view.session().unwrap().session.key, first);
        assert_eq!(view.anchor(), Some(1));
        // Each commit now owns a single session, so both branches terminate.
        assert!(is_last_session(&view, 0));
        assert!(is_last_session(&view, 1));
        browser.sources[0]
            .live
            .insert(first, RuntimeStatus::Unloaded);
        view.sync(&browser, &history);
        assert!(view.session().is_none());
        assert_eq!(view.selection, Some(Selection::Commit("older".into())));
        assert_eq!(history.rows, original);
        assert_eq!(history.rows.as_ptr(), allocation);
        assert_eq!(history.revision(), revision);
        assert!(!history.needs_poll());
        browser.sources[0].live.clear();
        view.sync(&browser, &history);
        assert_eq!(view.rows.len(), history.rows.len());
    }

    #[tokio::test]
    async fn session_heavy_pages_follow_and_unloaded_anchors() {
        let (mut browser, mut history) = fixture().await;
        for i in 0..12 {
            add_session(
                &mut browser,
                &format!("session-{i}"),
                "/repo",
                "/repo",
                "/repo/.git",
                RuntimeStatus::Running,
                i,
            );
        }
        let mut view = Presentation::default();
        view.sync(&browser, &history);
        history.page.set(4);
        view.navigate(Movement::PageDown, &mut history);
        assert_eq!(view.selected, 4);
        assert!(view.session().is_some());
        assert!(view.range(4).contains(&view.selected));
        let range = view.range(4);
        let Row::Session { annotation, .. } = view.rows[range.end - 1] else {
            panic!("expected a clipped session group")
        };
        assert!(!is_last_session(&view, annotation));
        assert!(is_last_session(&view, 11));
        view.navigate(Movement::End, &mut history);
        assert_eq!(view.selection, Some(Selection::Commit("older".into())));
        view.navigate(Movement::Up, &mut history);
        assert!(view.session().is_some());
        view.navigate(Movement::Home, &mut history);
        assert_eq!(view.selected, 0);
        view.navigate(Movement::Down, &mut history);
        history.following = true;
        view.sync(&browser, &history);
        assert_eq!(view.selected, 0);
        browser
            .repositories
            .get_mut(std::path::Path::new("/repo/.git"))
            .unwrap()
            .worktrees[0]
            .head = Some("not-loaded".into());
        view.sync(&browser, &history);
        assert_eq!(view.rows.len(), 2);
    }
}
