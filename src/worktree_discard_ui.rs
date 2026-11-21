use crate::{
    browser::{Browser, FolderFilter, SourceSpec, safe_text},
    folder_move::Source,
    modal::{self, Button, CachedBody},
    tui::Action,
    worktree_discard::{self as discard, Blocker, Journal, Plan, Preview},
};
use crossterm::event::{KeyCode, KeyEvent, KeyEventKind};
use ratatui::{
    Frame,
    layout::{Constraint, Layout},
    style::{Color, Style},
    widgets::{Paragraph, Wrap},
};
use std::{
    cell::Cell,
    path::{Path, PathBuf},
};
use tokio::task::JoinHandle;

pub(crate) fn selected(browser: &Browser) -> Option<&Path> {
    let FolderFilter::Path(path) = &browser.folder else {
        return None;
    };
    browser.repositories.values().find_map(|repo| {
        let mut linked = repo.worktrees.iter().skip(1);
        linked
            .find(|row| {
                row.path == *path
                    && !row.bare
                    && !row.locked
                    && !row.prunable
                    && row.branch.as_deref().is_some_and(|branch| {
                        !matches!(branch, "refs/heads/main" | "refs/heads/master")
                    })
            })
            .map(|row| row.path.as_path())
    })
}

pub(crate) fn unavailable(browser: &Browser) -> Option<&'static str> {
    if browser.state_path.is_none() {
        Some("Persistent state is required for discard recovery.")
    } else if browser
        .specs
        .iter()
        .any(|source| matches!(source, SourceSpec::Fixture(_)))
    {
        Some("Fixture data is browse-only.")
    } else {
        None
    }
}

enum Task {
    Review {
        task: JoinHandle<Result<(Preview, String), String>>,
        releasing: bool,
    },
    Discard(JoinHandle<Result<PathBuf, String>>),
}

pub struct Modal {
    state: PathBuf,
    sources: Vec<Source>,
    previous: Option<Journal>,
    selected: Option<PathBuf>,
    blocked: Option<(Vec<Blocker>, CachedBody)>,
    review: Option<(Plan, CachedBody)>,
    task: Option<Task>,
    error: Option<String>,
    input: String,
    caret: usize,
    focus: usize,
    scroll: u16,
    limit: Cell<u16>,
    fits: Cell<bool>,
}

impl Modal {
    pub(crate) fn new(
        state: PathBuf,
        sources: Vec<Source>,
        selected: Option<PathBuf>,
        previous: Option<Journal>,
        local_terminals: Vec<String>,
    ) -> Self {
        let mut modal = Self {
            state,
            sources,
            previous,
            selected,
            blocked: None,
            review: None,
            task: None,
            error: None,
            input: String::new(),
            caret: 0,
            focus: 1,
            scroll: 0,
            limit: Cell::default(),
            fits: Cell::default(),
        };
        modal.refresh(local_terminals, false);
        modal
    }

    pub(crate) fn root(&self) -> Option<&Path> {
        self.previous
            .as_ref()
            .map(Journal::root)
            .or(self.selected.as_deref())
    }

    pub(crate) fn refresh(&mut self, local_terminals: Vec<String>, release: bool) {
        if local_terminals.is_empty() {
            self.check(release);
        } else if self.task.is_none() {
            self.reset_review();
            let text = format!(
                "Close these terminals in Tracr, then retry.\nPress Esc to return and use the Terminals menu.\n\n{}",
                local_terminals.join("\n\n")
            );
            self.blocked = Some((Vec::new(), CachedBody::new(text)));
        }
    }

    fn reset_review(&mut self) {
        self.review = None;
        self.error = None;
        self.scroll = 0;
        self.limit.set(0);
        self.input.clear();
        self.caret = 0;
        self.focus = 1;
    }

    fn check(&mut self, release: bool) {
        if self.task.is_some() {
            return;
        }
        if release && !self.can_release() {
            return;
        }
        let approved = self
            .blocked
            .take()
            .map(|(blockers, _)| blockers)
            .unwrap_or_default();
        self.reset_review();
        let state = self.state.clone();
        let sources = self.sources.clone();
        let selected = self.selected.clone();
        let previous = self.previous.clone();
        let task = tokio::spawn(async move {
            let (preview, report) = if release {
                discard::release_idle(&state, &sources, selected, previous.as_ref(), approved)
                    .await?
            } else {
                (
                    discard::preview(&state, &sources, selected, previous.as_ref()).await?,
                    String::new(),
                )
            };
            let text = match &preview {
                Preview::Ready(plan) => plan.review(),
                Preview::Blocked(blockers) => {
                    let mut text = Blocker::review(blockers);
                    if !report.is_empty() {
                        text = format!("Release results\n{report}\n{text}");
                    }
                    text
                }
            };
            Ok((preview, text))
        });
        self.task = Some(Task::Review {
            task,
            releasing: release,
        });
    }

    pub(crate) fn ready(&self) -> bool {
        match &self.task {
            Some(Task::Review { task, .. }) => task.is_finished(),
            Some(Task::Discard(task)) => task.is_finished(),
            None => false,
        }
    }

    pub(crate) fn executing(&self) -> bool {
        matches!(
            self.task,
            Some(
                Task::Discard(_)
                    | Task::Review {
                        releasing: true,
                        ..
                    }
            )
        )
    }

    pub(crate) fn start(&mut self) {
        if self.task.is_some() || !self.confirmed() {
            return;
        }
        let (plan, _) = self.review.take().expect("confirmed plan");
        let state = self.state.clone();
        let sources = self.sources.clone();
        let previous = self.previous.clone();
        self.task = Some(Task::Discard(tokio::spawn(async move {
            discard::execute(&state, &sources, plan, previous).await
        })));
    }

    pub(crate) async fn tick(&mut self) -> Option<Result<PathBuf, String>> {
        if !self.ready() {
            return None;
        }
        match self.task.take().expect("ready task") {
            Task::Review { task, .. } => {
                match task.await.unwrap_or_else(|e| Err(e.to_string())) {
                    Ok((Preview::Ready(plan), text)) => {
                        self.review = Some((*plan, CachedBody::new(text)))
                    }
                    Ok((Preview::Blocked(blockers), text)) => {
                        self.blocked = Some((blockers, CachedBody::new(text)))
                    }
                    Err(error) => self.error = Some(error),
                }
                None
            }
            Task::Discard(task) => Some(task.await.unwrap_or_else(|e| Err(e.to_string()))),
        }
    }

    fn confirmed(&self) -> bool {
        self.fits.get()
            && self
                .review
                .as_ref()
                .is_some_and(|(plan, _)| self.input == plan.branch())
    }

    fn buttons(&self) -> [Button<Action>; 2] {
        [
            Button {
                action: Action::CloseDiscard,
                label: "Cancel",
                enabled: true,
                primary: false,
            },
            Button {
                action: Action::ApplyDiscard,
                label: "Discard worktree",
                enabled: self.confirmed(),
                primary: true,
            },
        ]
    }

    fn can_release(&self) -> bool {
        self.blocked
            .as_ref()
            .is_some_and(|(blockers, _)| blockers.iter().any(Blocker::can_release))
    }

    fn blocker_buttons(&self, compact: bool) -> [Button<Action>; 3] {
        [
            Button {
                action: Action::CloseDiscard,
                label: "Cancel",
                enabled: true,
                primary: false,
            },
            Button {
                action: Action::RetryDiscard,
                label: "Retry",
                enabled: true,
                primary: true,
            },
            Button {
                action: Action::ReleaseDiscardIdle,
                label: if compact {
                    "Release"
                } else {
                    "Release idle conversations"
                },
                enabled: self.can_release(),
                primary: false,
            },
        ]
    }

    pub(crate) fn key(&mut self, key: KeyEvent) -> Action {
        if self.executing() || key.kind == KeyEventKind::Release {
            return Action::Continue;
        }
        if key.code == KeyCode::Esc {
            return Action::CloseDiscard;
        }
        if matches!(
            key.code,
            KeyCode::Up | KeyCode::Down | KeyCode::PageUp | KeyCode::PageDown
        ) {
            let amount = if matches!(key.code, KeyCode::PageUp | KeyCode::PageDown) {
                5
            } else {
                1
            };
            self.scroll = if matches!(key.code, KeyCode::Up | KeyCode::PageUp) {
                self.scroll.saturating_sub(amount)
            } else {
                self.scroll.saturating_add(amount).min(self.limit.get())
            };
            return Action::Continue;
        }
        if self.blocked.is_some() {
            if !self.fits.get() {
                return Action::Continue;
            }
            let buttons = self.blocker_buttons(false);
            let buttons = &buttons[..2 + usize::from(self.can_release())];
            let selected = self.focus - 1;
            if matches!(key.code, KeyCode::Tab | KeyCode::BackTab) {
                let mut selected = selected;
                modal::cycle(
                    &mut selected,
                    &[true; 3][..buttons.len()],
                    key.code == KeyCode::BackTab,
                );
                self.focus = selected + 1;
            } else if let Some(action) = modal::activate(key, buttons, selected) {
                return action;
            }
            return Action::Continue;
        }
        if self.review.is_none() {
            if self.error.is_some() && key.kind == KeyEventKind::Press && key.code == KeyCode::Enter
            {
                return Action::CloseDiscard;
            }
            return Action::Continue;
        }
        if !self.fits.get() {
            return Action::Continue;
        }
        match key.code {
            KeyCode::Tab | KeyCode::BackTab => {
                let enabled = [true, true, self.confirmed()];
                modal::cycle(&mut self.focus, &enabled, key.code == KeyCode::BackTab);
            }
            _ if self.focus == 0 => {
                modal::edit(&mut self.input, &mut self.caret, key);
            }
            _ => {
                if let Some(action) = modal::activate(key, &self.buttons(), self.focus - 1) {
                    return action;
                }
            }
        }
        Action::Continue
    }

    pub(crate) fn paste(&mut self, value: &str) {
        if self.review.is_some() && self.focus == 0 && self.fits.get() {
            let filtered: String = value.chars().filter(|c| !c.is_control()).collect();
            self.input.insert_str(self.caret, &filtered);
            self.caret += filtered.len();
        }
    }

    pub(crate) fn draw(&self, frame: &mut Frame<'_>) {
        let inner = modal::shell(frame, "Discard worktree", 84, 27);
        let regions = modal::regions(inner);
        let fits = inner.width >= 33 && inner.height >= 10;
        self.fits.set(fits);
        let progress = self
            .previous
            .as_ref()
            .map(Journal::progress)
            .unwrap_or_default();
        if let Some((_, body)) = &self.review {
            if !fits {
                frame.render_widget(
                    Paragraph::new("Enlarge terminal to review and confirm. Esc cancels.")
                        .wrap(Wrap { trim: true }),
                    inner,
                );
                return;
            }
            let parts =
                Layout::vertical([Constraint::Min(1), Constraint::Length(3)]).split(regions.body);
            self.limit.set(body.draw(frame, parts[0], self.scroll));
            modal::text_field(
                frame,
                parts[1],
                "Type the branch name",
                &self.input,
                self.caret,
                self.focus == 0,
                true,
            );
            frame.render_widget(
                Paragraph::new(progress).wrap(Wrap { trim: true }),
                regions.status,
            );
            modal::buttons(
                frame,
                regions.actions,
                &self.buttons(),
                self.focus.checked_sub(1),
            );
            frame.render_widget(
                Paragraph::new("↑↓ Scroll  •  Tab Focus  •  Esc Cancel")
                    .style(Style::default().fg(Color::DarkGray)),
                regions.hints,
            );
        } else if let Some((_, body)) = &self.blocked {
            let can_release = self.can_release();
            let min_width = if can_release { 35 } else { 22 };
            let fits = inner.width >= min_width && inner.height >= 8;
            self.fits.set(fits);
            if !fits {
                frame.render_widget(
                    Paragraph::new("Enlarge terminal for actions. Esc cancels.")
                        .wrap(Wrap { trim: true }),
                    inner,
                );
                return;
            }
            self.limit.set(body.draw(frame, regions.body, self.scroll));
            frame.render_widget(
                Paragraph::new(progress).wrap(Wrap { trim: true }),
                regions.status,
            );
            let buttons = self.blocker_buttons(inner.width < 54);
            modal::buttons(
                frame,
                regions.actions,
                &buttons[..2 + usize::from(can_release)],
                self.focus.checked_sub(1),
            );
            frame.render_widget(
                Paragraph::new("↑↓ Scroll  •  Tab Focus  •  Esc Cancel")
                    .style(Style::default().fg(Color::DarkGray)),
                regions.hints,
            );
        } else {
            let message = if let Some(error) = &self.error {
                format!("{}\n\n{progress}", safe_text(error))
            } else if matches!(
                self.task,
                Some(Task::Review {
                    releasing: true,
                    ..
                })
            ) {
                "Releasing reviewed idle conversations and checking again…".into()
            } else if self.executing() {
                "Deleting reviewed conversations and checking Git cleanup…".into()
            } else {
                format!("Checking worktree files and local Codex sources…\n\n{progress}")
            };
            self.limit
                .set(modal::body(frame, regions.body, message, self.scroll));
            if !self.executing() {
                frame.render_widget(
                    Paragraph::new(if self.error.is_some() {
                        "Esc / Enter Close"
                    } else {
                        "Esc Cancel"
                    }),
                    regions.hints,
                );
            }
        }
    }
}

impl Drop for Modal {
    fn drop(&mut self) {
        if let Some(Task::Review { task, .. }) = self.task.take() {
            task.abort();
        }
    }
}
