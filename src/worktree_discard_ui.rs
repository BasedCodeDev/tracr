use crate::{
    browser::{Browser, FolderFilter, SourceSpec, safe_text},
    folder_move::Source,
    modal::{self, Button},
    tui::Action,
    worktree_discard::{self as discard, Journal, Plan},
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
    Review(JoinHandle<Result<Plan, String>>),
    Discard(JoinHandle<Result<PathBuf, String>>),
}

pub struct Modal {
    state: PathBuf,
    sources: Vec<Source>,
    previous: Option<Journal>,
    plan: Option<Plan>,
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
    ) -> Self {
        let task = tokio::spawn({
            let sources = sources.clone();
            let state = state.clone();
            let previous = previous.clone();
            async move { discard::preview(&state, &sources, selected, previous.as_ref()).await }
        });
        Self {
            state,
            sources,
            previous,
            plan: None,
            task: Some(Task::Review(task)),
            error: None,
            input: String::new(),
            caret: 0,
            focus: 1,
            scroll: 0,
            limit: Cell::default(),
            fits: Cell::default(),
        }
    }

    pub(crate) fn ready(&self) -> bool {
        match &self.task {
            Some(Task::Review(task)) => task.is_finished(),
            Some(Task::Discard(task)) => task.is_finished(),
            None => false,
        }
    }

    pub(crate) fn executing(&self) -> bool {
        matches!(self.task, Some(Task::Discard(_)))
    }

    pub(crate) fn start(&mut self) {
        if self.task.is_some() || !self.confirmed() {
            return;
        }
        let plan = self.plan.take().expect("confirmed plan");
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
            Task::Review(task) => {
                match task.await.unwrap_or_else(|e| Err(e.to_string())) {
                    Ok(plan) => self.plan = Some(plan),
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
                .plan
                .as_ref()
                .is_some_and(|plan| self.input == plan.branch())
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
        if self.plan.is_none() {
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
        if self.plan.is_some() && self.focus == 0 && self.fits.get() {
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
        if let Some(plan) = &self.plan {
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
            self.limit
                .set(modal::body(frame, parts[0], plan.review(), self.scroll));
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
        } else {
            let message = if let Some(error) = &self.error {
                format!("{}\n\n{progress}", safe_text(error))
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
        if let Some(Task::Review(task)) = self.task.take() {
            task.abort();
        }
    }
}
