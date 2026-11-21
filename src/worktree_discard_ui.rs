use crate::{
    browser::{Browser, FolderFilter, SourceSpec, safe_text},
    domain::SessionKey,
    folder_move::Source,
    inline_terminal::{DiscardTerminal, ExitNotice, Manager},
    modal::{self, Button, CachedBody},
    tui::Action,
    worktree_discard::{
        self as discard, Blocker, Journal, Plan, Preview, SessionPlan, SessionTarget,
    },
};
use crossterm::event::{KeyCode, KeyEvent, KeyEventKind};
use ratatui::{
    Frame,
    layout::{Constraint, Layout},
    style::{Color, Modifier, Style},
    widgets::{Paragraph, Wrap},
};
use std::{
    cell::Cell,
    path::{Path, PathBuf},
    time::{Duration, Instant},
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

#[derive(Clone)]
enum Target {
    Worktree {
        sources: Vec<Source>,
        selected: Option<PathBuf>,
        previous: Option<Box<Journal>>,
    },
    Session(Box<SessionTarget>),
}

enum Review {
    Worktree(Box<Plan>),
    Session(Box<SessionPlan>),
}

pub(crate) enum Discarded {
    Worktree(PathBuf),
    Session(SessionKey),
}

type ReviewResult = Result<(Result<Review, Vec<Blocker>>, String), String>;

enum Task {
    Review {
        task: JoinHandle<ReviewResult>,
        releasing: bool,
    },
    Discard(JoinHandle<Result<Discarded, String>>),
}

#[derive(Clone, Copy)]
enum TerminalPhase {
    Review,
    Confirm,
    Closing {
        deadline: Instant,
        delayed: bool,
        unverified: bool,
    },
}

struct TerminalBlock {
    terminals: Vec<DiscardTerminal>,
    phase: TerminalPhase,
    list_changed: bool,
    body: CachedBody,
}

impl TerminalBlock {
    fn new(terminals: Vec<DiscardTerminal>, phase: TerminalPhase, list_changed: bool) -> Self {
        let mut block = Self {
            terminals,
            phase,
            list_changed,
            body: CachedBody::new(String::new()),
        };
        block.set_phase(phase);
        block
    }

    fn shown_terminals(&self) -> impl Iterator<Item = &DiscardTerminal> {
        self.terminals
            .iter()
            .filter(|terminal| !matches!(self.phase, TerminalPhase::Confirm) || terminal.owned)
    }

    fn set_phase(&mut self, phase: TerminalPhase) {
        self.phase = phase;
        let text = self
            .shown_terminals()
            .map(|terminal| {
                let title = safe_text(&terminal.title);
                let folder = safe_text(&terminal.folder.display().to_string());
                let mut text = format!("{title}\n  {folder}");
                if !terminal.owned {
                    text.push_str("\n  Preview terminal");
                }
                if let Some(key) = &terminal.key {
                    text.push_str(&format!("\n  Session  {}", safe_text(&key.native_id)));
                }
                text
            })
            .collect::<Vec<_>>()
            .join("\n\n");
        self.body = CachedBody::new(text);
    }

    fn buttons(&self, compact: bool) -> Vec<Button<Action>> {
        let close = |label, action| Button {
            action,
            label,
            enabled: true,
            primary: true,
        };
        let cancel = Button {
            action: Action::CloseDiscard,
            label: "Cancel",
            enabled: true,
            primary: false,
        };
        match self.phase {
            TerminalPhase::Review => {
                let mut buttons = vec![cancel, close("Retry", Action::RetryDiscard)];
                if self.terminals.iter().any(|terminal| terminal.owned) {
                    buttons.push(close(
                        if compact {
                            "Close…"
                        } else {
                            "Close listed terminals…"
                        },
                        Action::ReviewDiscardTerminals,
                    ));
                }
                buttons
            }
            TerminalPhase::Confirm => vec![
                cancel,
                close("Close terminals", Action::ConfirmDiscardTerminals),
            ],
            TerminalPhase::Closing { .. } => Vec::new(),
        }
    }

    fn heading(&self) -> (String, &str) {
        let count = self.shown_terminals().count();
        let heading = if count == 1 {
            "1 terminal".to_owned()
        } else {
            format!("{count} terminals")
        };
        match self.phase {
            TerminalPhase::Review if self.list_changed => (
                format!("{heading} in Tracr"),
                "The terminal list changed. Review it before closing.",
            ),
            TerminalPhase::Review => (
                format!("{heading} in Tracr"),
                "Close the blocking terminals to continue discard.",
            ),
            TerminalPhase::Confirm => (
                format!("Close {heading}?"),
                "Running work in these terminals will stop.",
            ),
            TerminalPhase::Closing {
                unverified: true, ..
            } => (
                format!("{heading} remaining"),
                "Tracr could not verify that a terminal exited. Discard is paused.",
            ),
            TerminalPhase::Closing { delayed: true, .. } => (
                format!("{heading} remaining"),
                "Still waiting for terminal shutdown. Discard is paused.",
            ),
            TerminalPhase::Closing { .. } => (
                format!("Closing {heading}…"),
                "Discard checks will resume after the terminals exit.",
            ),
        }
    }
}

pub struct Modal {
    state: PathBuf,
    target: Target,
    blocked: Option<(Vec<Blocker>, CachedBody)>,
    local: Option<TerminalBlock>,
    review: Option<(Review, CachedBody)>,
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
        local_terminals: Vec<DiscardTerminal>,
    ) -> Self {
        Self::open(
            state,
            Target::Worktree {
                sources,
                selected,
                previous: previous.map(Box::new),
            },
            local_terminals,
        )
    }

    pub(crate) fn for_session(
        state: PathBuf,
        target: SessionTarget,
        local_terminals: Vec<DiscardTerminal>,
    ) -> Self {
        Self::open(state, Target::Session(Box::new(target)), local_terminals)
    }

    fn open(state: PathBuf, target: Target, local_terminals: Vec<DiscardTerminal>) -> Self {
        let mut modal = Self {
            state,
            target,
            blocked: None,
            local: None,
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

    pub(crate) fn session_key(&self) -> Option<&SessionKey> {
        match &self.target {
            Target::Session(target) => Some(&target.session.key),
            Target::Worktree { .. } => None,
        }
    }

    pub(crate) fn terminals(&self, manager: &Manager) -> Vec<DiscardTerminal> {
        match &self.target {
            Target::Session(target) => manager.session_terminals(&target.session.key),
            Target::Worktree {
                selected, previous, ..
            } => previous
                .as_deref()
                .map(Journal::root)
                .or(selected.as_deref())
                .map(|root| manager.worktree_terminals(root))
                .unwrap_or_default(),
        }
    }

    pub(crate) fn refresh(&mut self, terminals: Vec<DiscardTerminal>, release: bool) {
        if self.task.is_some() || self.is_closing_terminals() {
            return;
        }
        if terminals.is_empty() {
            self.local = None;
            self.check(release);
        } else {
            self.show_local(terminals, false);
        }
    }

    fn show_local(&mut self, terminals: Vec<DiscardTerminal>, list_changed: bool) {
        self.reset_review();
        self.blocked = None;
        self.local = Some(TerminalBlock::new(
            terminals,
            TerminalPhase::Review,
            list_changed,
        ));
    }

    fn set_terminal_phase(&mut self, phase: TerminalPhase) {
        if let Some(local) = &mut self.local {
            local.set_phase(phase);
        }
        self.focus = 1;
        self.scroll = 0;
        self.limit.set(0);
    }

    pub(crate) fn review_terminals(&mut self) {
        if self
            .local
            .as_ref()
            .is_some_and(|local| matches!(local.phase, TerminalPhase::Review))
        {
            self.set_terminal_phase(TerminalPhase::Confirm);
        }
    }

    pub(crate) fn confirm_terminals(&mut self, manager: &mut Manager) {
        let Some(local) = self
            .local
            .as_ref()
            .filter(|local| matches!(local.phase, TerminalPhase::Confirm))
        else {
            return;
        };
        let closing = match &self.target {
            Target::Session(target) => {
                manager.close_session_terminals(&target.session.key, &local.terminals)
            }
            Target::Worktree {
                selected, previous, ..
            } => {
                let Some(root) = previous
                    .as_deref()
                    .map(Journal::root)
                    .or(selected.as_deref())
                else {
                    return;
                };
                manager.close_worktree_terminals(root, &local.terminals)
            }
        };
        match closing {
            Ok(closing) if closing.is_empty() => {
                let remaining = self.terminals(manager);
                self.refresh(remaining, false);
            }
            Ok(closing) => {
                let phase = TerminalPhase::Closing {
                    deadline: Instant::now() + Duration::from_secs(8),
                    delayed: false,
                    unverified: false,
                };
                self.reset_review();
                self.local = Some(TerminalBlock::new(closing, phase, false));
            }
            Err(current) => self.show_local(current, true),
        }
    }

    fn is_closing_terminals(&self) -> bool {
        self.local
            .as_ref()
            .is_some_and(|local| matches!(local.phase, TerminalPhase::Closing { .. }))
    }

    pub(crate) fn tracks_terminal(&self, id: u64) -> bool {
        self.is_closing_terminals()
            && self
                .local
                .as_ref()
                .is_some_and(|local| local.terminals.iter().any(|terminal| terminal.id == id))
    }

    /// Called after the manager polls its children. Only a verified exit clears
    /// a requested terminal, even if manager polling drops a failed child.
    pub(crate) fn poll_terminals(
        &mut self,
        manager: &Manager,
        exits: &[ExitNotice],
        now: Instant,
    ) -> bool {
        let Some(local) = &mut self.local else {
            return false;
        };
        let TerminalPhase::Closing {
            deadline,
            delayed,
            unverified,
        } = local.phase
        else {
            return false;
        };
        let waiting = now >= deadline;
        if exits.is_empty() && waiting == delayed {
            return false;
        }
        let mut failed = unverified;
        let mut remaining = local.terminals.clone();
        remaining.retain(
            |terminal| match exits.iter().find(|exit| exit.id == terminal.id) {
                Some(exit) if exit.exit_verified => false,
                Some(_) => {
                    failed = true;
                    true
                }
                None => true,
            },
        );
        if remaining.is_empty() {
            self.local = None;
            let current = self.terminals(manager);
            self.refresh(current, false);
            return true;
        }
        if waiting != delayed || failed != unverified || remaining != local.terminals {
            let phase = TerminalPhase::Closing {
                deadline,
                delayed: waiting,
                unverified: failed,
            };
            self.local = Some(TerminalBlock::new(remaining, phase, false));
            return true;
        }
        false
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
        let target = self.target.clone();
        let task = tokio::spawn(async move {
            let (review, report) = match target {
                Target::Worktree {
                    sources,
                    selected,
                    previous,
                } => {
                    let (preview, report) = if release {
                        discard::release_idle(
                            &state,
                            &sources,
                            selected,
                            previous.as_deref(),
                            approved,
                        )
                        .await?
                    } else {
                        (
                            discard::preview(&state, &sources, selected, previous.as_deref())
                                .await?,
                            String::new(),
                        )
                    };
                    let review = match preview {
                        Preview::Ready(plan) => {
                            let body = plan.review();
                            Ok((Review::Worktree(plan), body))
                        }
                        Preview::Blocked(blockers) => Err(blockers),
                    };
                    (review, report)
                }
                Target::Session(target) => {
                    let (preview, report) = if release {
                        discard::release_session(&state, &target, &approved).await?
                    } else {
                        (
                            discard::preview_session(&state, &target).await?,
                            String::new(),
                        )
                    };
                    let review = match preview {
                        Ok(plan) => {
                            let body = plan.review(&target);
                            Ok((Review::Session(Box::new(plan)), body))
                        }
                        Err(blocker) => Err(vec![blocker]),
                    };
                    (review, report)
                }
            };
            match review {
                Ok((plan, body)) => Ok((Ok(plan), body)),
                Err(blockers) => {
                    let mut text = Blocker::review(&blockers);
                    if !report.is_empty() {
                        text = format!("Release results\n{report}\n{text}");
                    }
                    Ok((Err(blockers), text))
                }
            }
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
        let target = self.target.clone();
        self.task = Some(Task::Discard(tokio::spawn(async move {
            match (target, plan) {
                (
                    Target::Worktree {
                        sources, previous, ..
                    },
                    Review::Worktree(plan),
                ) => discard::execute(&state, &sources, *plan, previous.map(|journal| *journal))
                    .await
                    .map(Discarded::Worktree),
                (Target::Session(target), Review::Session(plan)) => {
                    discard::execute_session(&state, &target, *plan)
                        .await
                        .map(|()| Discarded::Session(target.session.key))
                }
                _ => unreachable!("review and discard target must match"),
            }
        })));
    }

    pub(crate) async fn tick(&mut self) -> Option<Result<Discarded, String>> {
        if !self.ready() {
            return None;
        }
        match self.task.take().expect("ready task") {
            Task::Review { task, .. } => {
                match task.await.unwrap_or_else(|e| Err(e.to_string())) {
                    Ok((Ok(plan), text)) => self.review = Some((plan, CachedBody::new(text))),
                    Ok((Err(blockers), text)) => {
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
            && self.review.as_ref().is_some_and(|(plan, _)| match plan {
                Review::Worktree(plan) => self.input == plan.branch(),
                Review::Session(_) => true,
            })
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
                label: if self.session_key().is_some() {
                    "Delete session"
                } else {
                    "Discard worktree"
                },
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
            if self
                .local
                .as_ref()
                .is_some_and(|local| matches!(local.phase, TerminalPhase::Confirm))
            {
                self.set_terminal_phase(TerminalPhase::Review);
                return Action::Continue;
            }
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
        if let Some(local) = &self.local {
            if !self.fits.get() || self.is_closing_terminals() {
                return Action::Continue;
            }
            let buttons = local.buttons(false);
            let mut selected = self.focus - 1;
            if matches!(key.code, KeyCode::Tab | KeyCode::BackTab) {
                modal::cycle(
                    &mut selected,
                    &[true; 3][..buttons.len()],
                    key.code == KeyCode::BackTab,
                );
                self.focus = selected + 1;
            } else {
                match modal::activate(key, &buttons, selected) {
                    Some(Action::CloseDiscard) if matches!(local.phase, TerminalPhase::Confirm) => {
                        self.set_terminal_phase(TerminalPhase::Review);
                    }
                    Some(action) => return action,
                    None => {}
                }
            }
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
                let enabled = [self.session_key().is_none(), true, self.confirmed()];
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
        let session = self.session_key().is_some();
        let title = if session {
            "Delete session"
        } else {
            "Discard worktree"
        };
        let inner = modal::shell(frame, title, 84, 27);
        let regions = modal::regions(inner);
        let fits = inner.width >= 33 && inner.height >= 10;
        self.fits.set(fits);
        let progress = match &self.target {
            Target::Worktree {
                previous: Some(journal),
                ..
            } => journal.progress(),
            _ => String::new(),
        };
        if let Some(local) = &self.local {
            self.draw_local(frame, inner, regions, local);
        } else if let Some((_, body)) = &self.review {
            if !fits {
                frame.render_widget(
                    Paragraph::new("Enlarge terminal to review and confirm. Esc cancels.")
                        .wrap(Wrap { trim: true }),
                    inner,
                );
                return;
            }
            let body_area = if session {
                regions.body
            } else {
                let parts = Layout::vertical([Constraint::Min(1), Constraint::Length(3)])
                    .split(regions.body);
                modal::text_field(
                    frame,
                    parts[1],
                    "Type the branch name",
                    &self.input,
                    self.caret,
                    self.focus == 0,
                    true,
                );
                parts[0]
            };
            self.limit.set(body.draw(frame, body_area, self.scroll));
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
            } else if self.executing() && session {
                "Deleting the reviewed Codex session…".into()
            } else if self.executing() {
                "Deleting reviewed conversations and checking Git cleanup…".into()
            } else if session {
                "Checking the selected Codex session…".into()
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
    fn draw_local(
        &self,
        frame: &mut Frame<'_>,
        inner: ratatui::layout::Rect,
        regions: modal::Regions,
        local: &TerminalBlock,
    ) {
        let fits = inner.width >= 34 && inner.height >= 9;
        self.fits.set(fits);
        if !fits {
            frame.render_widget(
                Paragraph::new("Enlarge terminal for actions. Esc goes back.")
                    .wrap(Wrap { trim: true }),
                inner,
            );
            return;
        }
        let (heading, message) = local.heading();
        let warning = matches!(
            local.phase,
            TerminalPhase::Confirm
                | TerminalPhase::Closing {
                    unverified: true,
                    ..
                }
        );
        let header = Layout::vertical([
            Constraint::Length(1),
            Constraint::Length(2),
            Constraint::Min(1),
        ])
        .split(regions.body);
        frame.render_widget(
            Paragraph::new(heading).style(
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ),
            header[0],
        );
        let style = if warning {
            Style::default().fg(Color::Yellow)
        } else {
            Style::default()
        };
        frame.render_widget(
            Paragraph::new(message)
                .style(style)
                .wrap(Wrap { trim: true }),
            header[1],
        );
        self.limit
            .set(local.body.draw(frame, header[2], self.scroll));
        let buttons = local.buttons(inner.width < 52);
        modal::buttons(frame, regions.actions, &buttons, self.focus.checked_sub(1));
        let hints = if self.is_closing_terminals() {
            "↑↓ Scroll  •  Esc Close dialog"
        } else {
            "↑↓ Scroll  •  Tab Focus  •  Esc Back"
        };
        frame.render_widget(
            Paragraph::new(hints).style(Style::default().fg(Color::DarkGray)),
            regions.hints,
        );
    }
}

impl Drop for Modal {
    fn drop(&mut self) {
        if let Some(Task::Review { task, .. }) = self.task.take() {
            task.abort();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn delayed_or_unverified_terminal_exit_keeps_discard_blocked() {
        let terminal = DiscardTerminal {
            id: 1,
            title: "Terminal".into(),
            folder: "/feature".into(),
            key: None,
            owned: true,
        };
        let mut modal = Modal::new(
            "state".into(),
            Vec::new(),
            Some("/feature".into()),
            None,
            vec![terminal],
        );
        let deadline = Instant::now();
        modal.set_terminal_phase(TerminalPhase::Closing {
            deadline,
            delayed: false,
            unverified: false,
        });
        let manager = Manager::default();
        assert!(modal.poll_terminals(&manager, &[], deadline));
        assert!(
            modal
                .local
                .as_ref()
                .unwrap()
                .heading()
                .1
                .contains("Still waiting")
        );
        assert!(!modal.ready());

        let exit = ExitNotice {
            id: 1,
            exit_verified: false,
            failed: true,
            was_active: false,
            message: "Wait failed".into(),
        };
        assert!(modal.poll_terminals(&manager, &[exit], deadline));
        assert!(
            modal
                .local
                .as_ref()
                .unwrap()
                .heading()
                .1
                .contains("could not verify")
        );
        assert!(modal.tracks_terminal(1));
        assert!(!modal.ready());
    }
}
