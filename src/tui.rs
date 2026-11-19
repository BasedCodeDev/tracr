use crate::browser::{Browser, Discovery, FolderFilter, safe_text};
use crossterm::{
    event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers},
    execute,
    terminal::{self, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{
    Frame, Terminal,
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{
        Block, BorderType, Borders, Clear, List, ListItem, ListState, Paragraph, Scrollbar,
        ScrollbarState, Wrap,
    },
};
use std::{
    io::{self, IsTerminal},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

pub struct RelinkModal {
    pub session: crate::domain::SessionSummary,
    pub input: String,
    pub picker: Option<crate::path_picker::Picker>,
    pub preview: Option<crate::relocation::Preview>,
    pub recovery: bool,
    pub busy: bool,
    pub executing: bool,
    pub error: Option<String>,
}
pub fn relink_error(
    session: &str,
    old: &str,
    target: &str,
    failure: &crate::relocation::Failure,
    needs_recovery: bool,
) -> crate::error_dialog::ErrorDialog {
    let title = if needs_recovery {
        "Relink needs recovery"
    } else {
        "Couldn't relink session"
    };
    let explanation = if failure.kind == crate::relocation::FailureKind::PermissionProfile {
        "This session uses a permission profile that tracr does not yet support for relinking. Permission profiles control what a session can access."
    } else {
        &failure.detail
    };
    let outcome = if needs_recovery {
        "The folder may already have changed. Close this dialog and press u to inspect/recover the operation."
    } else if failure.kind == crate::relocation::FailureKind::PermissionProfile {
        "Its folder was not changed. Your session remains available, but tracr cannot move it with its current permission settings."
    } else {
        "tracr did not change the session's folder during this attempt."
    };
    crate::error_dialog::ErrorDialog::new(
        title,
        "! Relink failed",
        format!(
            "{explanation}\n\n{outcome}\n\nSession: {session}\nOriginal folder: {old}\nRequested destination: {target}\n\nTechnical details: {}",
            failure.detail
        ),
    )
}
fn show_relink_failure(view: &mut View, failure: crate::relocation::Failure, needs_recovery: bool) {
    view.terminal_recovery = None;
    if !needs_recovery {
        if let Some(modal) = view
            .relink
            .as_mut()
            .filter(|modal| !modal.recovery && modal.preview.is_some())
        {
            let old = modal
                .session
                .cwd
                .as_ref()
                .map(|p| p.display().to_string())
                .unwrap_or_default();
            let dialog = if failure.kind == crate::relocation::FailureKind::StillLoaded {
                Some(crate::terminal_recovery::Dialog::still_loaded(
                    &modal.session.title,
                    &old,
                    &modal.input,
                ))
            } else {
                failure.terminals.clone().map(|blocker| {
                    crate::terminal_recovery::Dialog::new(
                        blocker,
                        &modal.session.title,
                        &old,
                        &modal.input,
                    )
                })
            };
            if let Some(dialog) = dialog {
                modal.busy = false;
                modal.executing = false;
                view.terminal_recovery = Some(dialog);
                return;
            }
        }
    }

    if let Some(modal) = view.relink.take() {
        view.error_dialog = Some(relink_error(
            &modal.session.title,
            &modal
                .session
                .cwd
                .map(|p| p.display().to_string())
                .unwrap_or_default(),
            &modal.input,
            &failure,
            needs_recovery,
        ));
    }
}
fn show_folder_failure(view: &mut View, message: String, recovery: bool) {
    if let Some(modal) = view.folder_move.take() {
        let target = modal
            .plan
            .as_ref()
            .map(|p| p.target.clone())
            .unwrap_or_else(|| {
                modal
                    .target()
                    .unwrap_or_else(|_| std::path::PathBuf::from(&modal.input))
            });
        let count = modal
            .plan
            .as_ref()
            .map(|p| p.entries.len().to_string())
            .unwrap_or_else(|| "Not yet determined".into());
        let title = if recovery {
            "Move needs recovery"
        } else {
            "Folder was not moved"
        };
        let outcome = if recovery {
            "Files may already be at the destination. Close this dialog and press u to inspect and finish the move."
        } else {
            "tracr did not move the folder during this attempt."
        };
        view.error_dialog = Some(crate::error_dialog::ErrorDialog::new(
            title,
            "! Folder move failed",
            format!(
                "{outcome}\n\nOriginal folder: {}\nRequested destination: {}\nAffected sessions: {count}\n\nTechnical details:\n{message}",
                modal.old.display(),
                target.display()
            ),
        ));
    }
}
pub struct WorktreePicker {
    repository: std::path::PathBuf,
    oid: String,
    matches: Vec<crate::git::Worktree>,
    selected: usize,
}

fn matching_worktrees(
    browser: &Browser,
    repository: &std::path::Path,
    oid: &str,
) -> Result<Vec<crate::git::Worktree>, String> {
    if browser.repository_errors.contains_key(repository) {
        return Err("Worktree information is stale or refreshing. Try again after refresh.".into());
    }
    let repo = browser
        .repositories
        .get(repository)
        .ok_or("Worktree information is unavailable. Refresh and try again.")?;
    let mut matches: Vec<_> = repo
        .worktrees
        .iter()
        .filter(|w| !w.bare && w.head.as_deref() == Some(oid))
        .cloned()
        .collect();
    matches.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(matches)
}

fn select_git_worktree(browser: &mut Browser, view: &mut View, picker: &WorktreePicker) -> Action {
    let Some(target) = picker.matches.get(picker.selected) else {
        return Action::Continue;
    };
    match matching_worktrees(browser, &picker.repository, &picker.oid) {
        Ok(matches) if matches.iter().any(|w| w.path == target.path) => {
            // Linked worktrees are displayed beneath the main checkout, even when
            // their filesystem paths live elsewhere.
            if let Some(main) = browser
                .repositories
                .get(&picker.repository)
                .and_then(|repo| repo.worktrees.first())
            {
                browser
                    .expanded
                    .extend(main.path.ancestors().map(std::path::PathBuf::from));
            }
            browser.open_folder(target.path.clone());
            view.git_focus = false;
            view.focus = 0;
            view.folders_hidden = false;
            view.message = None;
            return enter_folder(browser, view);
        }
        Ok(_) => {
            view.message = Some("Worktree changed or was removed. Select the commit again.".into())
        }
        Err(message) => view.message = Some(message),
    }
    Action::Continue
}

#[derive(Default)]
pub struct View {
    pub layout_mode: std::cell::Cell<LayoutMode>,
    pub terminals: std::cell::RefCell<crate::inline_terminal::Manager>,
    pub terminal_recovery: Option<crate::terminal_recovery::Dialog>,
    pub history: crate::git_history::History,
    pub git_sessions: std::cell::RefCell<crate::git_sessions::Presentation>,
    pub git_hidden: bool,
    pub folders_hidden: bool,
    pub git_size_limited: bool,
    pub git_watch_error: Option<String>,
    pub git_focus: bool,
    pub focus: usize,
    pub editing: bool,
    pub search_query: String,
    pub search_selected: Option<crate::domain::SessionKey>,
    pub help: bool,
    pub help_selected: usize,
    pub help_entries: Option<Vec<MenuEntry>>,
    pub help_control: usize,
    pub search_control: usize,
    pub source_control: usize,
    pub relink_back: bool,
    pub relink_scroll: u16,
    pub relink_scroll_limit: std::cell::Cell<u16>,
    pub help_page_size: usize,
    pub info: Option<InfoModal>,
    pub commit_info: Option<crate::commit_info::Dialog>,
    pub message: Option<String>,
    pub preparing: bool,
    pub source_picker: Option<usize>,
    pub source_fullscreen: bool,
    pub worktree_picker: Option<WorktreePicker>,
    pub relink: Option<RelinkModal>,
    pub folder_move: Option<crate::folder_move_ui::Modal>,
    pub worktree_create: Option<crate::worktree_create_ui::Modal>,
    pub recovery_index: usize,
    pub recovery_available: bool,
    pub error_dialog: Option<crate::error_dialog::ErrorDialog>,
}
impl View {
    fn modal_open(&self) -> bool {
        self.terminal_recovery.is_some()
            || self.error_dialog.is_some()
            || self.worktree_create.is_some()
            || self.folder_move.is_some()
            || self.relink.is_some()
            || self.info.is_some()
            || self.commit_info.is_some()
            || self.source_picker.is_some()
            || self.worktree_picker.is_some()
            || self.help
            || self.editing
            || self.preparing
    }

    fn terminal_focused(&self) -> bool {
        self.context() == 1 && !self.modal_open()
    }

    fn context(&self) -> usize {
        if self.git_focus { 2 } else { self.focus }
    }
}

fn refresh_recovery(state: Option<&std::path::Path>, view: &mut View) -> Option<(usize, bool)> {
    view.recovery_available = false;
    let state = state?;
    let result = (|| {
        let relocations = crate::relocation::pending(state)
            .map_err(|e| format!("Cannot inspect relocation journals: {e}"))?;
        let folder = crate::folder_move::pending(state)
            .map_err(|e| format!("Cannot inspect folder-move journal: {e}"))?;
        Ok::<_, String>((relocations.len(), folder.is_some()))
    })();
    match result {
        Ok((count, folder)) => {
            view.recovery_available = count > 0 || folder;
            Some((count, folder))
        }
        Err(error) => {
            view.message = Some(error);
            None
        }
    }
}

fn selection_style(focused: bool) -> Style {
    Style::default().add_modifier(if focused {
        Modifier::BOLD
    } else {
        Modifier::DIM
    })
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum LayoutMode {
    Columns,
    Stacked,
    #[default]
    Single,
}

impl LayoutMode {
    fn for_size(width: u16, height: u16) -> Self {
        if width >= 160 && height >= 12 {
            Self::Columns
        } else if width >= 80 && height >= 48 {
            Self::Stacked
        } else {
            Self::Single
        }
    }
}

#[derive(Debug)]
struct PanelLayout {
    mode: LayoutMode,
    folders: Option<Rect>,
    sessions: Option<Rect>,
    git: Option<Rect>,
    graph_visible: bool,
}

// Shared by drawing, input navigation, Git scheduling, and terminal sizing.
// Total dimensions choose the mode so footer messages cannot change it.
fn panel_layout(area: Rect, terminal_height: u16, view: &View, present: bool) -> PanelLayout {
    let mode = LayoutMode::for_size(area.width, terminal_height);
    view.layout_mode.set(mode);
    let mut result = PanelLayout {
        mode,
        folders: None,
        sessions: None,
        git: None,
        graph_visible: false,
    };
    if mode == LayoutMode::Single {
        match view.context() {
            0 => result.folders = Some(area),
            2 if present => result.git = Some(area),
            _ => result.sessions = Some(area),
        }
    } else {
        let stacked = mode == LayoutMode::Stacked;
        let rail = if stacked { 1 } else { 2 };
        // Keep a 1:2 sidebar ratio while reserving at least 40% for Sessions.
        let stacked_folder_height = (area.height / 5).min(12);
        let folder_size = if view.folders_hidden {
            rail
        } else if stacked {
            stacked_folder_height
        } else {
            (area.width / 6).clamp(24, 32)
        };
        let git_size = if !present {
            0
        } else if view.git_hidden {
            rail
        } else if stacked {
            stacked_folder_height * 2
        } else {
            (area.width / 4).clamp(36, 48)
        };
        let regions = Layout::default()
            .direction(if stacked {
                Direction::Vertical
            } else {
                Direction::Horizontal
            })
            .constraints([
                Constraint::Length(folder_size),
                Constraint::Min(0),
                Constraint::Length(git_size),
            ])
            .split(area);
        result.folders = Some(regions[0]);
        result.sessions = Some(regions[1]);
        result.git = present.then_some(regions[2]);
    }
    result.graph_visible =
        !view.git_hidden && result.git.is_some_and(|r| r.height >= 4 && r.width >= 12);
    result
}

fn draw_rail(frame: &mut Frame<'_>, area: Rect, mode: LayoutMode, title: &str) {
    let mut rail = Block::default()
        .borders(if mode == LayoutMode::Columns {
            Borders::LEFT
        } else {
            Borders::TOP
        })
        .style(Style::default().fg(Color::DarkGray));
    if mode != LayoutMode::Columns {
        rail = rail.title(title);
    }
    frame.render_widget(rail, area);
}

static ENHANCED_INPUT: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
fn reset_input() {
    if ENHANCED_INPUT.swap(false, std::sync::atomic::Ordering::Relaxed) {
        let _ = execute!(io::stdout(), crossterm::style::Print("\x1b[<u"));
    }
}
fn restore() {
    reset_input();
    let _ = terminal::disable_raw_mode();
    let _ = execute!(
        io::stdout(),
        event::DisableMouseCapture,
        event::DisableBracketedPaste,
        event::DisableFocusChange,
        LeaveAlternateScreen,
        crossterm::cursor::Show
    );
}
// A direct flag is observed by the UI loop even if the async signal driver stalls.
struct ShutdownSignals {
    terminate: std::sync::Arc<std::sync::atomic::AtomicBool>,
    interrupt: std::sync::Arc<std::sync::atomic::AtomicBool>,
    resize: std::sync::Arc<std::sync::atomic::AtomicBool>,
    registrations: Vec<signal_hook::SigId>,
}
impl ShutdownSignals {
    fn new() -> io::Result<Self> {
        let mut guard = Self {
            terminate: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
            interrupt: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
            resize: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
            registrations: Vec::new(),
        };
        for (signal, flag) in [
            (signal_hook::consts::SIGTERM, guard.terminate.clone()),
            (signal_hook::consts::SIGINT, guard.interrupt.clone()),
            (signal_hook::consts::SIGWINCH, guard.resize.clone()),
        ] {
            guard
                .registrations
                .push(signal_hook::flag::register(signal, flag)?);
        }
        Ok(guard)
    }
}
impl Drop for ShutdownSignals {
    fn drop(&mut self) {
        for registration in self.registrations.drain(..) {
            signal_hook::low_level::unregister(registration);
        }
    }
}

struct TerminalGuard {
    active: bool,
    mouse: bool,
}
impl TerminalGuard {
    fn enhanced(&mut self, supported: bool) -> io::Result<()> {
        if supported && !ENHANCED_INPUT.swap(true, std::sync::atomic::Ordering::Relaxed) {
            execute!(io::stdout(), crossterm::style::Print("\x1b[>31u"))?;
        }
        Ok(())
    }
    fn mouse_capture(&mut self, enabled: bool) -> io::Result<()> {
        if enabled != self.mouse {
            if enabled {
                execute!(io::stdout(), event::EnableMouseCapture)?;
            } else {
                execute!(io::stdout(), event::DisableMouseCapture)?;
            }
            self.mouse = enabled;
        }
        Ok(())
    }
    fn activate(&mut self) -> io::Result<()> {
        terminal::enable_raw_mode()?;
        self.active = true;
        execute!(
            io::stdout(),
            EnterAlternateScreen,
            event::EnableBracketedPaste,
            event::EnableFocusChange,
            crossterm::style::Print("\x1b[?u\x1b[c")
        )
    }
    fn suspend(&mut self) -> io::Result<()> {
        reset_input();
        terminal::disable_raw_mode()?;
        execute!(
            io::stdout(),
            event::DisableMouseCapture,
            event::DisableBracketedPaste,
            event::DisableFocusChange,
            LeaveAlternateScreen,
            crossterm::cursor::Show
        )?;
        self.active = false;
        self.mouse = false;
        Ok(())
    }
}
impl Drop for TerminalGuard {
    fn drop(&mut self) {
        if self.active {
            restore();
        }
    }
}
fn block(title: &str, focused: bool) -> Block<'_> {
    Block::default()
        .borders(Borders::ALL)
        .title(title)
        .border_type(if focused {
            BorderType::Thick
        } else {
            BorderType::Plain
        })
        .border_style(Style::default().fg(if focused {
            Color::Cyan
        } else {
            Color::DarkGray
        }))
}

fn pane_block(title: &str, focused: bool) -> Block<'_> {
    block(title, focused).padding(ratatui::widgets::Padding::new(0, 0, 1, 0))
}

#[derive(Default)]
struct IdleGate {
    dirty: bool,
    deadline: Option<std::time::Instant>,
}
impl IdleGate {
    fn wake(&mut self) {
        self.dirty = true;
    }
    fn ready(&mut self, now: std::time::Instant, activity: bool) -> bool {
        let ready = self.dirty || activity || self.deadline.is_some_and(|deadline| now >= deadline);
        if ready {
            self.dirty = false;
            self.deadline = None;
        }
        ready
    }
}
fn age_delay(updated: i64, now: i64) -> u64 {
    let elapsed = now.saturating_sub(updated).max(0);
    if elapsed < 60 {
        return updated
            .saturating_add(60)
            .saturating_sub(now)
            .clamp(1, 86_400) as u64;
    }
    let units = [31_536_000, 2_592_000, 604_800, 86_400, 3_600, 60];
    let unit = units.into_iter().find(|unit| elapsed >= *unit).unwrap();
    let next_bucket = units
        .into_iter()
        .filter(|unit| *unit > elapsed)
        .map(|unit| unit - elapsed)
        .min()
        .unwrap_or(i64::MAX);
    (unit - elapsed % unit).min(next_bucket) as u64
}
fn next_age_update(browser: &Browser, view: &View, height: u16) -> Option<std::time::Instant> {
    if view.editing
        || view.help
        || view.error_dialog.is_some()
        || view.relink.is_some()
        || view.folder_move.is_some()
    {
        return None;
    }
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
        .min(i64::MAX as u64) as i64;
    let sessions = browser.visible();
    let selected = sessions
        .iter()
        .position(|session| Some(&session.key) == browser.selected.as_ref())
        .unwrap_or(0);
    let count = usize::from(height / 3).max(1);
    let start = selected.saturating_sub(count.saturating_sub(1));
    sessions
        .iter()
        .skip(start)
        .take(count)
        .map(|session| age_delay(session.updated_at, now))
        .min()
        .map(|seconds| std::time::Instant::now() + Duration::from_secs(seconds))
}

fn relative_time(updated_at: i64, now: i64) -> String {
    let elapsed = now.saturating_sub(updated_at).max(0);
    if elapsed < 60 {
        return "just now".into();
    }
    for (seconds, unit) in [
        (31_536_000, "year"),
        (2_592_000, "month"),
        (604_800, "week"),
        (86_400, "day"),
        (3_600, "hour"),
        (60, "minute"),
    ] {
        if elapsed >= seconds {
            let count = elapsed / seconds;
            return format!("{count} {unit}{} ago", if count == 1 { "" } else { "s" });
        }
    }
    unreachable!("elapsed is at least one minute")
}

fn footer_shortcuts(width: u16, browser: &Browser, view: &View) -> (String, String) {
    let entries = menu_entries(browser, view);
    let fit = |context: bool| {
        let hints: Vec<_> = entries
            .iter()
            .filter(|e| (e.section != "General" && e.section != "Navigation") == context)
            .filter_map(|e| e.footer)
            .collect();
        let full = hints.iter().map(|h| h.0).collect::<Vec<_>>().join(" · ");
        if unicode_width::UnicodeWidthStr::width(full.as_str()) <= usize::from(width) {
            return full;
        }
        let mut line = String::new();
        for (_, short) in hints {
            let short = if width < 15 {
                match short {
                    "? Help" => "?",
                    "q Quit" => "q",
                    _ => short,
                }
            } else {
                short
            };
            let candidate = if line.is_empty() {
                short.to_owned()
            } else {
                format!("{line} · {short}")
            };
            if unicode_width::UnicodeWidthStr::width(candidate.as_str()) > usize::from(width) {
                break;
            }
            line = candidate;
        }
        line
    };
    (fit(false), fit(true))
}

fn draw_footer(
    frame: &mut Frame<'_>,
    area: Rect,
    shared: &str,
    context: &str,
    message: &str,
    separate_message: bool,
) {
    use ratatui::layout::Alignment;
    let row = |offset| Rect::new(area.x, area.y + offset, area.width, 1);
    let shared_width =
        unicode_width::UnicodeWidthStr::width(shared).min(area.width as usize) as u16;
    let controls = Rect::new(area.right() - shared_width, area.y, shared_width, 1);
    frame.render_widget(Paragraph::new(shared).alignment(Alignment::Right), controls);
    frame.render_widget(Paragraph::new(context).alignment(Alignment::Right), row(1));
    let feedback = if separate_message {
        row(2)
    } else {
        Rect::new(
            area.x,
            area.y,
            area.width.saturating_sub(shared_width + 2),
            1,
        )
    };
    frame.render_widget(Paragraph::new(message), feedback);
}

fn is_unlinked(browser: &Browser, session: &crate::domain::SessionSummary) -> bool {
    session.cwd.as_ref().is_some_and(|path| {
        browser.folders.get(path) == Some(&crate::domain::FolderStatus::Unlinked)
    })
}
fn session_metadata(
    browser: &Browser,
    session: &crate::domain::SessionSummary,
    now: i64,
) -> Line<'static> {
    let mut spans = Vec::new();
    if browser
        .details
        .get(&session.key)
        .is_some_and(|detail| matches!(detail, Ok(Some(detail)) if detail.cwd != session.cwd))
    {
        spans.push(Span::styled(
            "Folder changed · i Info · ",
            Style::default().fg(Color::Rgb(255, 165, 0)),
        ));
    }
    if is_unlinked(browser, session) {
        spans.push(Span::styled(
            "Unlinked",
            Style::default()
                .fg(Color::Rgb(255, 165, 0))
                .add_modifier(Modifier::BOLD),
        ));
        spans.push(Span::raw(" · "));
    }
    spans.push(Span::styled(
        format!(
            "{} · {}",
            safe_text(&session.key.provider.0),
            relative_time(session.updated_at, now)
        ),
        Style::default().add_modifier(Modifier::DIM),
    ));
    Line::from(spans)
}

fn browser_footer(area: Rect, browser: &Browser, view: &View) -> (String, String, String, u16) {
    if view.terminal_focused() && view.terminals.borrow().active.is_some() {
        return (
            "Ctrl+B Background · Alt+F Full-screen · Ctrl+C Interrupt Codex".into(),
            browser.focus_root.as_ref().map_or_else(
                || "Keyboard input goes to the terminal".into(),
                |root| {
                    format!(
                        "Keyboard input goes to the terminal · Focused: {} · Alt+Z in Folders zooms out",
                        safe_text(&root.display().to_string())
                    )
                },
            ),
            view.message.clone().unwrap_or_default(),
            2,
        );
    }
    let mut issues: Vec<_> = browser
        .diagnostics()
        .into_iter()
        .filter(|s| !s.complete)
        .map(|s| {
            format!(
                "{}: {}",
                safe_text(&s.source),
                safe_text(&s.error.unwrap_or_else(|| if s.warnings.is_empty() {
                    "refreshing".into()
                } else {
                    format!(
                        "{} ({} warnings; inventory incomplete)",
                        s.warnings[0],
                        s.warnings.len()
                    )
                }))
            )
        })
        .collect();
    if view.history.present() && !view.git_hidden {
        if let Some(notice) = view.history.notice() {
            issues.insert(0, safe_text(&notice));
        }
    }
    if let Some(error) = &view.git_watch_error {
        issues.insert(
            0,
            format!(
                "Git auto-update unavailable — r to refresh: {}",
                safe_text(error)
            ),
        );
    }
    for (source, spec) in browser.sources.iter().zip(&browser.specs) {
        if source.live_error.is_some() {
            issues.push(format!(
                "{}: live status unavailable",
                safe_text(&spec.label())
            ));
        }
    }
    let message = if view.preparing {
        "Preparing resume… Esc cancels".into()
    } else {
        view.message
            .as_ref()
            .map(|m| safe_text(m))
            .unwrap_or_else(|| issues.join(" | "))
    };
    let message =
        if view.git_watch_error.is_some() && !message.contains("Git auto-update unavailable") {
            format!("{} | {message}", issues[0])
        } else {
            message
        };
    let message = if let Some(root) = &browser.focus_root {
        format!(
            "{}Focused: {} · Alt+Z in Folders zooms out",
            if message.is_empty() {
                String::new()
            } else {
                format!("{message} | ")
            },
            safe_text(&root.display().to_string())
        )
    } else {
        message
    };
    let (first, second) = footer_shortcuts(area.width, browser, view);
    let separate_message = !message.is_empty()
        && unicode_width::UnicodeWidthStr::width(message.as_str())
            + unicode_width::UnicodeWidthStr::width(first.as_str())
            + 2
            > usize::from(area.width);
    (first, second, message, if separate_message { 3 } else { 2 })
}

fn selected_git_session(browser: &Browser, view: &View) -> Option<crate::git_sessions::Annotation> {
    let mut presentation = view.git_sessions.borrow_mut();
    presentation.sync(browser, &view.history);
    presentation.session().cloned()
}

fn git_session_status(status: crate::domain::RuntimeStatus) -> &'static str {
    use crate::domain::RuntimeStatus;
    match status {
        RuntimeStatus::Idle => "Idle",
        RuntimeStatus::Running => "Running",
        RuntimeStatus::Error => "Error",
        RuntimeStatus::Waiting {
            approval: true,
            input: true,
        } => "Waiting",
        RuntimeStatus::Waiting { approval: true, .. } => "Approval",
        RuntimeStatus::Waiting { input: true, .. } => "Input",
        _ => "",
    }
}

fn git_session_name_width(presentation: &crate::git_sessions::Presentation) -> usize {
    presentation
        .rows
        .iter()
        .filter_map(|row| match row {
            crate::git_sessions::Row::Session { annotation, .. } => {
                Some(unicode_width::UnicodeWidthStr::width(
                    safe_text(&presentation.annotations[*annotation].session.title).as_str(),
                ))
            }
            _ => None,
        })
        .max()
        .unwrap_or(0)
}

fn git_session_spans(
    view: &View,
    anchor: usize,
    connector: Option<usize>,
    annotation: &crate::git_sessions::Annotation,
    last: bool,
    name_width: usize,
) -> Vec<Span<'static>> {
    let mut spans = view.history.session_spans(anchor, connector);
    // Reserve the longest status label even when no session currently uses it,
    // so names stay aligned as live statuses change.
    let status = git_session_status(annotation.status);
    let status_width: usize = 8;
    spans.push(Span::raw(" ".repeat(
        status_width.saturating_sub(unicode_width::UnicodeWidthStr::width(status)),
    )));
    spans.push(Span::styled(
        status.to_owned(),
        activity_color(annotation.status)
            .map_or(Style::default(), |color| Style::default().fg(color)),
    ));
    spans.push(Span::styled(
        if last { " └─ " } else { " ├─ " },
        Style::default().fg(Color::White),
    ));
    let title = safe_text(&annotation.session.title);
    let padding = name_width.saturating_sub(unicode_width::UnicodeWidthStr::width(title.as_str()));
    spans.push(Span::styled(
        title,
        Style::default().add_modifier(Modifier::BOLD),
    ));
    spans.push(Span::raw(" ".repeat(padding)));
    spans.push(Span::raw(format!(
        " · {}",
        safe_text(&annotation.session.key.provider.0)
    )));
    if let Some(label) = &annotation.label {
        spans.push(Span::styled(
            format!(" · {}", safe_text(label)),
            Style::default().fg(Color::DarkGray),
        ));
    }
    spans
}

pub fn draw(frame: &mut Frame<'_>, browser: &Browser, view: &View) {
    view.terminals.borrow_mut().visible = false;
    if let Some(dialog) = &view.terminal_recovery {
        dialog.draw(frame);
        return;
    }

    if let Some(dialog) = &view.error_dialog {
        dialog.draw(frame);
        return;
    }
    if let Some(modal) = &view.worktree_create {
        modal.draw(frame);
        return;
    }
    if let Some(modal) = &view.folder_move {
        modal.draw(frame, browser);
        return;
    }
    if let Some(info) = &view.commit_info {
        info.draw(frame);
        return;
    }
    if let Some(info) = &view.info {
        info.dialog.draw(frame);
        return;
    }
    let area = frame.area();
    view.layout_mode
        .set(LayoutMode::for_size(area.width, area.height));
    if view.terminals.borrow().fullscreen
        && view.terminals.borrow().active.is_some()
        && !view.modal_open()
    {
        view.terminals.borrow_mut().draw(frame, area, true);
        return;
    }
    if area.width < 12 || area.height < 4 {
        frame.render_widget(Paragraph::new("tracr: enlarge terminal"), area);
        return;
    }
    let (first, second, message, footer_height) = browser_footer(area, browser, view);
    let separate_message = footer_height == 3;
    let layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(1), Constraint::Length(footer_height)])
        .split(area);
    let panels = panel_layout(layout[0], area.height, view, view.history.present());
    let git_area = panels.git;
    let expanded = panels.graph_visible;
    if let Some(git_area) = git_area {
        if expanded {
            let mut presentation = view.git_sessions.borrow_mut();
            presentation.sync(browser, &view.history);
            let range = presentation.range(usize::from(git_area.height.saturating_sub(3)));
            let more_above = range.start > 0;
            let more_below = range.end < presentation.rows.len()
                || (!view.history.exhausted && !presentation.rows.is_empty());
            let selected = presentation.selected.saturating_sub(range.start);
            // Use all loaded rows, so scrolling does not shift the provider column.
            let name_width = git_session_name_width(&presentation);
            let width = usize::from(git_area.width.saturating_sub(4));
            // Preserve existing horizontal bounds, extending them for annotation text.
            let requested = view.history.horizontal.get();
            view.history.clamp_horizontal(width);
            let session_max = presentation
                .rows
                .iter()
                .filter_map(|row| match row {
                    crate::git_sessions::Row::Session {
                        anchor,
                        annotation,
                        connector,
                    } => Some(
                        git_session_spans(
                            view,
                            *anchor,
                            *connector,
                            &presentation.annotations[*annotation],
                            presentation.is_last_session(*annotation),
                            name_width,
                        )
                        .iter()
                        .map(|s| unicode_width::UnicodeWidthStr::width(s.content.as_ref()))
                        .sum::<usize>()
                        .saturating_sub(width),
                    ),
                    _ => None,
                })
                .max()
                .unwrap_or(0);
            view.history.horizontal.set(
                view.history
                    .horizontal
                    .get()
                    .max(requested.min(session_max)),
            );
            let items: Vec<_> = presentation.rows[range]
                .iter()
                .map(|row| {
                    let spans = match row {
                        crate::git_sessions::Row::Graph(i) => {
                            view.history.row_spans(&view.history.rows[*i])
                        }
                        crate::git_sessions::Row::Session {
                            anchor,
                            annotation,
                            connector,
                        } => git_session_spans(
                            view,
                            *anchor,
                            *connector,
                            &presentation.annotations[*annotation],
                            presentation.is_last_session(*annotation),
                            name_width,
                        ),
                    };
                    ListItem::new(graph_viewport(spans, view.history.horizontal.get(), width))
                })
                .collect();
            frame.render_stateful_widget(
                List::new(items)
                    .block(pane_block("Git Worktrees", view.git_focus))
                    .highlight_symbol("> ")
                    .highlight_style(selection_style(view.context() == 2)),
                git_area,
                &mut ListState::default().with_selected(Some(selected)),
            );
            for (visible, y, marker) in [
                (more_above, git_area.y, " ↑ "),
                (more_below, git_area.bottom() - 1, " ↓ "),
            ] {
                if visible {
                    frame.render_widget(
                        Paragraph::new(marker).style(
                            Style::default()
                                .fg(Color::Cyan)
                                .add_modifier(Modifier::BOLD),
                        ),
                        Rect::new(git_area.right() - 4, y, 3, 1),
                    );
                }
            }
        } else {
            draw_rail(frame, git_area, panels.mode, " Git Worktrees ");
        }
    }
    if let Some(area) = panels.folders {
        if view.folders_hidden {
            draw_rail(frame, area, panels.mode, " Folders ");
        } else {
            draw_pane(frame, area, 0, browser, view);
        }
    }
    if let Some(area) = panels.sessions {
        draw_pane(frame, area, 1, browser, view);
    }
    draw_footer(
        frame,
        layout[1],
        &first,
        &second,
        &message,
        separate_message,
    );
    if let Some(picker) = &view.worktree_picker {
        let popup = Rect::new(
            area.x + 1,
            area.y + 1,
            area.width.saturating_sub(2),
            area.height.saturating_sub(2).min(16),
        );
        let items: Vec<_> = picker
            .matches
            .iter()
            .map(|w| {
                let branch = w
                    .branch
                    .as_deref()
                    .map(|b| b.trim_start_matches("refs/heads/"))
                    .unwrap_or("detached");
                ListItem::new(vec![
                    Line::from(safe_text(branch)),
                    Line::from(safe_text(&w.path.display().to_string())),
                ])
            })
            .collect();
        frame.render_widget(Clear, popup);
        frame.render_stateful_widget(
            List::new(items)
                .block(block(
                    "Choose worktree — ↑/↓ · Enter selects · Esc cancels",
                    true,
                ))
                .highlight_symbol("> ")
                .highlight_style(Style::default().add_modifier(Modifier::BOLD)),
            popup,
            &mut ListState::default().with_selected(Some(picker.selected)),
        );
    }
    if view.help {
        draw_help(frame, browser, view);
    }
    if let Some(selected) = view.source_picker {
        let inner = crate::modal::shell(frame, "Choose harness", 76, 18);
        let regions = crate::modal::regions(inner);
        modal_actions(
            frame,
            regions.actions,
            "Cancel",
            "Create session",
            view.source_control,
            !creation_sources(browser).is_empty(),
            true,
        );
        frame.render_widget(
            Paragraph::new("↑/↓ select · Enter creates · Esc cancels"),
            regions.hints,
        );
        let items: Vec<_> = creation_sources(browser)
            .iter()
            .map(|&i| ListItem::new(safe_text(&browser.specs[i].label())))
            .collect();
        frame.render_stateful_widget(
            List::new(items)
                .highlight_symbol("> ")
                .highlight_style(selection_style(view.source_control == 0)),
            regions.body,
            &mut ListState::default().with_selected(Some(selected)),
        );
    }
    if view.editing {
        draw_search(frame, browser, view);
    }
    if let Some(modal) = &view.relink {
        if modal.preview.is_none() && modal.picker.is_some() {
            draw_path_picker(frame, modal);
            return;
        }
        let mut text = format!(
            "{}\nSource: {}\nFrom: {}\nArchived: {}\n\nDestination: {}\n\n",
            safe_text(&modal.session.title),
            safe_text(&browser.specs.iter().find(|spec|matches!(spec,crate::browser::SourceSpec::Codex{source,..} if source.id==modal.session.key.source_id)).map(|spec|spec.label()).unwrap_or_else(||modal.session.key.source_id.0.clone())),
            safe_text(
                &modal
                    .session
                    .cwd
                    .as_ref()
                    .map(|p| p.display().to_string())
                    .unwrap_or_default()
            ),
            modal.session.archived,
            safe_text(&modal.input)
        );
        if let Some(preview) = &modal.preview {
            text.push_str(&format!(
                "Permissions: {}\nOther session settings are preserved.\nOnly this session changes folders. Project files stay where they are.\nDetached idle sessions are released automatically after confirmation.\nIdle describes agent activity; background commands may still block moving.\n\n",
                preview.profile.label()
            ));
        }
        let controls = if modal.busy {
            modal
                .error
                .as_deref()
                .unwrap_or("Checking native state… Please wait.")
        } else if modal.preview.is_some() {
            "Enter confirms • Esc cancels"
        } else {
            "Type an existing folder • Enter previews • Esc cancels"
        };
        if let Some(error) = modal.error.as_ref().filter(|_| !modal.busy) {
            text.push_str(&format!("\n\n{}", safe_text(error)));
        }
        let inner = crate::modal::shell(
            frame,
            if modal.recovery {
                "Inspect/recover relocation"
            } else {
                "Move session"
            },
            84,
            28,
        );
        let regions = crate::modal::regions(inner);
        view.relink_scroll_limit.set(crate::modal::body(
            frame,
            regions.body,
            text,
            view.relink_scroll,
        ));
        frame.render_widget(
            Paragraph::new(controls).wrap(Wrap { trim: false }),
            regions.status,
        );
        modal_actions(
            frame,
            regions.actions,
            "Cancel",
            if modal.recovery {
                "Continue recovery"
            } else {
                "Move session"
            },
            if view.relink_back { 1 } else { 2 },
            !modal.busy,
            !modal.executing,
        );
    }
}

fn modal_actions(
    frame: &mut Frame<'_>,
    area: Rect,
    secondary: &'static str,
    primary: &'static str,
    focus: usize,
    enabled: bool,
    cancellable: bool,
) {
    use crate::modal::Button;
    crate::modal::buttons(
        frame,
        area,
        &[
            Button {
                action: 0,
                label: secondary,
                enabled: cancellable,
                primary: false,
            },
            Button {
                action: 1,
                label: primary,
                enabled,
                primary: true,
            },
        ],
        focus.checked_sub(1),
    );
}

fn draw_path_picker(frame: &mut Frame<'_>, modal: &RelinkModal) {
    render_picker(
        frame,
        modal.picker.as_ref().expect("path picker"),
        &modal.input,
        PickerText {
            title: "Move session",
            heading: &modal.session.title,
            old: &modal
                .session
                .cwd
                .as_ref()
                .map(|p| p.display().to_string())
                .unwrap_or_default(),
            field: "New folder",
            busy: modal.busy,
            error: modal.error.as_deref(),
            sidebar: None,
        },
    );
}
pub(crate) struct PickerText<'a> {
    pub title: &'a str,
    pub heading: &'a str,
    pub old: &'a str,
    pub field: &'a str,
    pub busy: bool,
    pub error: Option<&'a str>,
    pub sidebar: Option<(&'a [Line<'static>], u16, &'a std::cell::Cell<u16>)>,
}
pub(crate) fn render_picker(
    frame: &mut Frame<'_>,
    picker: &crate::path_picker::Picker,
    input: &str,
    text: PickerText<'_>,
) {
    let inner = crate::modal::shell(
        frame,
        text.title,
        if text.sidebar.is_some() { 120 } else { 84 },
        30,
    );
    if inner.height < 10 {
        frame.render_widget(
            Paragraph::new("Enlarge terminal to choose a folder. Esc cancels."),
            inner,
        );
        return;
    }
    let heading = if picker.is_folder_move() {
        format!(
            "Previous folder: {}\n{}",
            safe_text(text.old),
            safe_text(text.heading)
        )
    } else {
        format!(
            "{}\nPrevious folder: {}",
            safe_text(text.heading),
            safe_text(text.old)
        )
    };
    let heading = Paragraph::new(heading).wrap(Wrap { trim: false });
    let heading_height = heading
        .line_count(inner.width)
        .min(usize::from(inner.height.saturating_sub(8)))
        .max(2) as u16;
    let parts = Layout::vertical([
        Constraint::Length(heading_height),
        Constraint::Length(3),
        Constraint::Min(1),
        Constraint::Length(2),
        Constraint::Length(1),
        Constraint::Length(1),
    ])
    .split(inner);
    frame.render_widget(heading, parts[0]);
    crate::modal::text_field(
        frame,
        parts[1],
        text.field,
        input,
        picker.caret,
        picker.focus == 0,
        !text.busy,
    );
    let worktrees = picker.worktrees.as_ref().filter(|w| w.active);
    let (items, selected): (Vec<ListItem>, Option<usize>) = if let Some(w) = worktrees {
        let items = w
            .entries
            .iter()
            .map(|row| {
                let name = row
                    .path
                    .file_name()
                    .unwrap_or(row.path.as_os_str())
                    .to_string_lossy();
                let label = format!(
                    "{}{} · {}",
                    name,
                    if row.main { " (main)" } else { "" },
                    row.branch.as_deref().unwrap_or("Detached HEAD")
                );
                let width = usize::from(parts[2].width.saturating_sub(4));
                let mut lines: Vec<_> = crate::info_content::wrap(&label, width)
                    .into_iter()
                    .map(Line::from)
                    .collect();
                lines.extend(
                    crate::info_content::wrap(&row.path.display().to_string(), width)
                        .into_iter()
                        .map(Line::from),
                );
                if let Some(error) = &row.unavailable {
                    lines.extend(
                        crate::info_content::wrap(error, width)
                            .into_iter()
                            .map(Line::from),
                    );
                }
                ListItem::new(lines).style(if row.unavailable.is_some() {
                    Style::default().fg(Color::DarkGray)
                } else {
                    Style::default()
                })
            })
            .collect();
        (
            items,
            w.entries
                .iter()
                .position(|r| Some(&r.path) == w.selected.as_ref()),
        )
    } else {
        let items = picker
            .rows
            .iter()
            .map(|row| {
                let mut label = crate::path_picker::tree_label(&row.path, row.depth, row.expanded);
                if let Some(error) = &row.error {
                    label.push_str(&format!(" [{error}]"));
                }
                ListItem::new(safe_text(&label))
            })
            .collect();
        (
            items,
            picker
                .rows
                .iter()
                .position(|r| Some(&r.path) == picker.selected.as_ref()),
        )
    };
    let tree_area = if let Some((lines, scroll, limit)) = text.sidebar {
        let regions = if parts[2].width >= 90 {
            Layout::horizontal([Constraint::Percentage(55), Constraint::Percentage(45)])
                .split(parts[2])
        } else {
            Layout::vertical([Constraint::Percentage(55), Constraint::Percentage(45)])
                .split(parts[2])
        };
        let panel = block("Sessions · PgUp/PgDn", false);
        let body = panel.inner(regions[1]);
        frame.render_widget(panel, regions[1]);
        let sections = Layout::vertical([Constraint::Min(0), Constraint::Length(2)]).split(body);
        let content = lines
            .get(..lines.len().saturating_sub(2))
            .unwrap_or_default();
        let paragraph = Paragraph::new(content.to_vec()).wrap(Wrap { trim: false });
        let max = paragraph
            .line_count(sections[0].width)
            .saturating_sub(sections[0].height as usize)
            .min(u16::MAX as usize) as u16;
        limit.set(max);
        frame.render_widget(paragraph.scroll((scroll.min(max), 0)), sections[0]);
        if let Some(message) = lines.last() {
            frame.render_widget(
                Paragraph::new(message.clone()).wrap(Wrap { trim: false }),
                sections[1],
            );
        }
        regions[0]
    } else {
        parts[2]
    };
    let title = if picker.worktrees.is_some() {
        if worktrees.is_some() {
            "Folders   [Worktrees]"
        } else {
            "[Folders]   Worktrees"
        }
    } else {
        "Folders"
    };
    let state = worktrees.map_or(&picker.list_state, |w| &w.list_state);
    let mut state = state.borrow_mut();
    state.select(selected);
    frame.render_stateful_widget(
        List::new(items)
            .block(block(title, picker.focus == 1))
            .highlight_symbol("> ")
            .highlight_style(selection_style(picker.focus == 1)),
        tree_area,
        &mut state,
    );
    let ready = picker.valid && !picker.loading && !text.busy;
    modal_actions(
        frame,
        parts[4],
        "Cancel",
        "Use this folder",
        match picker.focus {
            3 => 1,
            2 => 2,
            _ => 0,
        },
        ready,
        true,
    );
    let hint = if picker.focus == 1 && worktrees.is_some() {
        "←/→ views · ↑/↓ select · Enter fills path"
    } else if picker.focus == 1 && picker.worktrees.is_some() {
        "←/→ views · ↑/↓ select · Enter expand"
    } else if picker.focus == 1 {
        "↑/↓ select · Enter expand/collapse"
    } else {
        "Enter focuses Use this folder · Tab controls"
    };
    let status = if text.busy {
        "Checking native state… Esc cancels".into()
    } else if let Some(error) = text.error.or(picker.error.as_deref()) {
        safe_text(error)
    } else if picker.loading {
        if worktrees.is_some() {
            "Loading worktrees…".into()
        } else {
            "Loading folders…".into()
        }
    } else {
        if picker.is_folder_move() {
            "End with / to keep the current folder name.".into()
        } else {
            "Tab switch controls · Esc cancel".into()
        }
    };
    frame.render_widget(Paragraph::new(status).wrap(Wrap { trim: false }), parts[3]);
    frame.render_widget(Paragraph::new(hint), parts[5]);
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Command {
    OpenTerminals,
    CloseTerminal,
    CreateWorktree,
    GitToggle,
    FolderToggle,
    GitReturn,
    GitUp,
    GitDown,
    GitLeft,
    GitRight,
    GitPageUp,
    GitPageDown,
    GitHome,
    GitEnd,
    GitFollow,
    GitSelectWorktree,
    Help,
    PreviousPane,
    EnterFolder,
    CyclePane,
    CyclePaneBack,
    PaneUp,
    PaneDown,
    PaneLeft,
    PaneRight,
    PreviousItem,
    NextItem,
    FolderFocus,
    FolderLeft,
    FolderRight,
    Resume,
    ResumeFullscreen,
    ToggleTerminalFullscreen,
    Info,
    CommitInfo,
    Relink,
    MoveFolder,
    Recover,
    Create,
    CreateFullscreen,
    Archives,
    Search,
    Refresh,
    Quit,
}
fn available_when_folders_hidden(command: Command) -> bool {
    matches!(
        command,
        Command::FolderToggle
            | Command::OpenTerminals
            | Command::ToggleTerminalFullscreen
            | Command::Help
            | Command::Quit
            | Command::Refresh
            | Command::CyclePane
            | Command::CyclePaneBack
            | Command::PaneUp
            | Command::PaneDown
            | Command::PaneLeft
            | Command::PaneRight
            | Command::EnterFolder
            | Command::PreviousPane
    )
}

#[derive(Clone)]
pub struct MenuEntry {
    command: Command,
    section: &'static str,
    shortcut: &'static str,
    label: &'static str,
    footer: Option<(&'static str, &'static str)>,
}
fn open_help(browser: &Browser, view: &mut View) {
    view.help = false;
    view.help_entries = Some(menu_entries(browser, view));
    view.help = true;
    view.help_control = 0;
}
fn menu_entries(browser: &Browser, view: &View) -> Vec<MenuEntry> {
    // Keep action identities stable while a menu is open. Availability is still
    // checked on every render and activation, so a refresh cannot redirect Enter.
    if view.help {
        if let Some(entries) = &view.help_entries {
            return entries.clone();
        }
    }
    use Command::*;
    let mut entries = vec![
        (
            Help,
            "General",
            "?",
            "Help / actions",
            Some(("? Help", "? Help")),
        ),
        (
            Quit,
            "General",
            "q / Ctrl-C",
            "Quit tracr",
            Some(("q Quit", "q Quit")),
        ),
        (
            Search,
            "General",
            "/",
            if browser.focus_root.is_some() {
                "Search within focused folder"
            } else {
                "Search all sessions"
            },
            Some(("/ Search", "/ Search")),
        ),
        (
            Refresh,
            "General",
            "r",
            "Refresh sessions",
            Some(("r Refresh", "r Refresh")),
        ),
        (
            Archives,
            "General",
            "a",
            if browser.include_archived {
                "Hide archived sessions"
            } else {
                "Show archived sessions"
            },
            Some(("a Archives", "a Archives")),
        ),
        (
            Recover,
            "General",
            "u",
            "Inspect/recover relocation",
            Some(("u Recover", "u Recover")),
        ),
        (
            CyclePane,
            "Navigation",
            "Tab",
            "Next pane",
            Some(("Tab Next pane", "Tab Next pane")),
        ),
        (
            CyclePaneBack,
            "Navigation",
            "Shift-Tab",
            "Previous pane",
            Some(("Shift-Tab Previous pane", "Shift-Tab Previous pane")),
        ),
        (
            PreviousPane,
            "Navigation",
            "← / Esc / Backspace",
            "Select Folders",
            None,
        ),
        (
            EnterFolder,
            "Folders",
            "Enter",
            "Open session or select Sessions",
            Some(("Enter Sessions", "Enter Sessions")),
        ),
        (
            PreviousItem,
            "Navigation",
            "↑ / k",
            "Previous item / scroll up",
            None,
        ),
        (
            NextItem,
            "Navigation",
            "↓ / j",
            "Next item / scroll down",
            None,
        ),
    ];
    entries.retain(|entry| {
        (entry.0 != Recover || view.recovery_available)
            && (entry.0 != EnterFolder || view.context() == 0)
            && (entry.0 != PreviousPane || view.context() == 1)
    });
    if view.context() == 0 {
        entries.push((
            FolderToggle,
            "Folders",
            "b",
            if view.folders_hidden {
                "Expand Folders pane"
            } else {
                "Collapse Folders pane"
            },
            Some(if view.folders_hidden {
                ("b Expand", "b Expand")
            } else {
                ("b Collapse", "b Collapse")
            }),
        ));
        entries.extend([
            (
                CreateFullscreen,
                "Folders",
                "n",
                "Create new session",
                Some(("n New session", "n New")),
            ),
            (
                FolderLeft,
                "Folders",
                "← / h",
                "Collapse folder / select parent",
                None,
            ),
            (
                FolderRight,
                "Folders",
                "→ / l",
                "Expand folder / focus Sessions",
                Some(if matches!(&browser.folder, FolderFilter::Path(path) if browser.expanded.contains(path)) {
                    ("→/l Sessions", "→/l Sessions")
                } else {
                    ("→/l Expand", "→/l Expand")
                }),
            ),
            (
                MoveFolder,
                "Folders",
                "m",
                "Move folder",
                Some(("m Move folder", "m Move")),
            ),
        ]);
    } else {
        entries.push(if can_create(browser) && browser.visible().is_empty() {
            (
                Create,
                "Sessions",
                "Enter / n",
                "Create new session",
                Some(("Enter/n New session", "Enter/n New")),
            )
        } else {
            (
                Resume,
                "Sessions",
                "Enter",
                "Open session embedded",
                Some(("Enter Embedded", "Enter Open")),
            )
        });
        if !browser.visible().is_empty() || !can_create(browser) {
            entries.push((
                Create,
                "Sessions",
                "n",
                "Create new session",
                Some(("n New session", "n New")),
            ));
        }
        entries.extend([
            (
                Info,
                "Sessions",
                "i",
                "Session information",
                Some(("i Info", "i Info")),
            ),
            (
                Relink,
                "Sessions",
                "m",
                "Move session",
                Some(("m Move", "m Move")),
            ),
        ]);
    }
    if view.context() == 0 && selected_checkout(browser).is_some() {
        entries.push((
            Info,
            "Folders",
            "i",
            "Worktree information",
            Some(("i Info", "i Info")),
        ));
    }
    if view.context() == 0 {
        entries.push((
            CreateWorktree,
            "Folders",
            "w",
            "Create worktree",
            Some(("w Worktree", "w Worktree")),
        ));
    }
    if view.context() == 2 {
        entries
            .retain(|entry| matches!(entry.0, Help | Quit | Refresh | CyclePane | CyclePaneBack));
        entries.push((
            GitToggle,
            "Git",
            "b",
            if view.git_hidden {
                "Expand Git history"
            } else {
                "Collapse Git history"
            },
            Some(if view.git_hidden {
                ("b Expand", "b Expand")
            } else {
                ("b Collapse", "b Collapse")
            }),
        ));
        if !view.git_hidden && !view.git_size_limited {
            entries.push((
                CommitInfo,
                "Git",
                "i",
                "Commit information",
                Some(("i Commit info", "i Info")),
            ));
            entries.push((
                GitSelectWorktree,
                "Git",
                "Enter",
                if selected_git_session(browser, view).is_some() {
                    "Resume session"
                } else if view
                    .history
                    .rows
                    .get(view.history.selected)
                    .is_some_and(|row| row.stash.is_some())
                {
                    "Describe saved stash"
                } else {
                    "Select worktree folder"
                },
                Some(if selected_git_session(browser, view).is_some() {
                    ("Enter Resume", "Enter Resume")
                } else if view
                    .history
                    .rows
                    .get(view.history.selected)
                    .is_some_and(|row| row.stash.is_some())
                {
                    ("Enter Stash", "Enter Stash")
                } else {
                    ("Enter Folder", "Enter Folder")
                }),
            ));
            entries.push((
                CreateWorktree,
                "Git",
                "w",
                "Create worktree from selected commit",
                Some(("w Worktree", "w Worktree")),
            ));
            entries.extend([
                (
                    GitUp,
                    "Git",
                    "↑ / k",
                    "Previous commit/session",
                    Some(("↑/↓ Scroll", "↑/↓ Scroll")),
                ),
                (GitDown, "Git", "↓ / j", "Next commit/session", None),
                (
                    GitLeft,
                    "Git",
                    "← / h",
                    "Scroll left",
                    Some(("←/→ Scroll", "←/→ Scroll")),
                ),
                (GitRight, "Git", "→ / l", "Scroll right", None),
                (GitPageUp, "Git", "PageUp", "Previous page", None),
                (GitPageDown, "Git", "PageDown", "Next page", None),
                (GitHome, "Git", "Home", "First loaded commit", None),
                (GitEnd, "Git", "End", "Last loaded commit", None),
            ]);
        }
        entries.push((GitReturn, "Navigation", "Esc", "Return to Sessions", None));
    }
    let navigation_index = entries
        .iter()
        .position(|entry| entry.1 == "Navigation")
        .unwrap_or(entries.len());
    let mode = view.layout_mode.get();
    let previous = match view.context() {
        1 => "Focus Folders",
        _ => "Focus Sessions",
    };
    let next = match view.context() {
        0 => "Focus Sessions",
        _ => "Focus Git Worktrees",
    };
    let last = if view.history.present() { 2 } else { 1 };
    for (back, forward, back_key, forward_key) in [
        (PaneLeft, PaneRight, "Alt+Left", "Alt+Right"),
        (PaneUp, PaneDown, "Alt+Up", "Alt+Down"),
    ] {
        let horizontal = back == PaneLeft;
        if mode == LayoutMode::Single || horizontal == (mode == LayoutMode::Columns) {
            if view.context() > 0 {
                entries.insert(
                    navigation_index,
                    (back, "Navigation", back_key, previous, None),
                );
            }
            if view.context() < last {
                entries.insert(
                    navigation_index,
                    (forward, "Navigation", forward_key, next, None),
                );
            }
        }
    }
    entries
        .retain(|entry| entry.0 != CreateWorktree || worktree_unavailable(browser, view).is_none());
    if browser
        .specs
        .iter()
        .any(|spec| matches!(spec, crate::browser::SourceSpec::Codex { .. }))
        && (view.context() == 1 || !view.terminals.borrow().is_empty())
    {
        entries.push((
            OpenTerminals,
            "Terminals",
            "g",
            "Open terminals",
            Some(("g Terminals", "g Terminals")),
        ));
        entries.push((CloseTerminal, "Terminals", "x", "Close terminal", None));
        if view.terminals.borrow().active.is_some() {
            entries.push((
                ToggleTerminalFullscreen,
                "Terminals",
                "Alt+F",
                "Toggle terminal full-screen",
                Some(("Alt+F Full-screen", "Alt+F Full")),
            ));
        } else if view.context() == 1 {
            entries.push((
                ResumeFullscreen,
                "Sessions",
                "Alt+F",
                "Open session full-screen",
                Some(("Alt+F Full-screen", "Alt+F Full")),
            ));
        }
    }
    if view.context() == 0 {
        let index = entries
            .iter()
            .position(|e| e.0 == CreateWorktree)
            .unwrap_or(entries.len());
        entries.insert(
            index,
            (
                FolderFocus,
                "Folders",
                "Alt+Z",
                if browser.focus_root.is_some() {
                    "Zoom out of folder"
                } else {
                    "Zoom into selected folder"
                },
                Some(("Alt+Z Zoom", "Alt+Z Zoom")),
            ),
        );
    }
    if view.context() == 0 && view.folders_hidden {
        entries.retain(|entry| available_when_folders_hidden(entry.0));
    }
    entries
        .into_iter()
        .map(|(command, section, shortcut, label, footer)| MenuEntry {
            command,
            section,
            shortcut,
            label,
            footer,
        })
        .collect()
}

fn selected_commit(browser: &Browser, view: &View) -> Option<(std::path::PathBuf, String)> {
    if !view.git_focus
        || view.git_hidden
        || view.git_size_limited
        || selected_git_session(browser, view).is_some()
    {
        return None;
    }
    Some((
        view.history.repository()?,
        view.history.rows.get(view.history.selected)?.oid.clone()?,
    ))
}

fn worktree_unavailable(browser: &Browser, view: &View) -> Option<String> {
    if view.git_focus
        && view
            .history
            .rows
            .get(view.history.selected)
            .is_some_and(|row| row.stash.is_some())
    {
        return Some("Select a commit rather than a stash to create a worktree.".into());
    }
    if view.git_focus && selected_git_session(browser, view).is_some() {
        return Some("Select a commit to create a worktree.".into());
    }
    if creation_sources(browser).is_empty() {
        return Some("Fixture data is browse-only.".into());
    }
    let FolderFilter::Path(path) = &browser.folder else {
        return Some("Select a Git checkout first.".into());
    };
    let Some(owner) = browser.ownership.get(path) else {
        return Some("Wait for Git discovery to finish.".into());
    };
    if owner.generation != browser.git_generation {
        return Some("Wait for Git discovery to finish.".into());
    }
    match &owner.outcome {
        Some(crate::git_discovery::Outcome::Repository { repository, .. }) => {
            if view.git_focus && view.history.repository().as_ref() != Some(&repository.common_dir)
            {
                return Some("Wait for history of the selected repository to load.".into());
            }
        }
        Some(crate::git_discovery::Outcome::NotRepository) => {
            return Some("The selected folder is not a Git checkout.".into());
        }
        _ => return Some("Git repository information is unavailable or still loading.".into()),
    }
    if view.git_focus
        && (view.git_hidden
            || view.git_size_limited
            || view
                .history
                .rows
                .get(view.history.selected)
                .and_then(|row| row.oid.as_ref())
                .is_none())
    {
        return Some("Select a visible Git commit first.".into());
    }
    None
}

fn unavailable(command: Command, browser: &Browser, view: &View) -> Option<String> {
    match command {
        Command::FolderFocus
            if browser.focus_root.is_none() && !matches!(browser.folder, FolderFilter::Path(_)) =>
        {
            Some("Select a concrete folder first.".into())
        }
        Command::CreateWorktree => worktree_unavailable(browser, view),
        Command::CommitInfo if selected_commit(browser, view).is_none() => {
            Some("Select a visible Git commit first.".into())
        }
        Command::FolderLeft | Command::FolderRight
            if !matches!(browser.folder, FolderFilter::Path(_)) =>
        {
            Some("Select a concrete folder first.".into())
        }
        Command::Info if view.focus == 0 && selected_checkout(browser).is_none() => {
            Some("Select a checkout folder first.".into())
        }
        Command::Info if view.focus == 1 && selected_session(browser).is_none() => {
            Some("Select a session first.".into())
        }
        Command::Relink => relink_unavailable(browser),
        Command::MoveFolder => crate::folder_move_ui::unavailable(browser),
        Command::Create | Command::CreateFullscreen => {
            if browser.loading() {
                Some("Wait for discovery to finish.".into())
            } else if creation_sources(browser).is_empty() {
                Some("Fixture data is browse-only.".into())
            } else if !matches!(browser.folder, FolderFilter::Path(_)) {
                Some("Select a concrete folder first.".into())
            } else {
                None
            }
        }
        Command::Resume | Command::ResumeFullscreen => {
            let session = browser
                .visible()
                .into_iter()
                .find(|s| Some(&s.key) == browser.selected.as_ref());
            let Some(session) = session else {
                return Some("Select a session first.".into());
            };
            if creation_sources(browser).is_empty() {
                return Some("Fixture data is browse-only.".into());
            }
            if let Some(reason) = crate::domain::resume_blocker(&session) {
                return Some(reason.into());
            }
            match browser.details.get(&session.key) {
                Some(Ok(Some(detail))) => crate::domain::resume_blocker(detail).map(str::to_owned),
                Some(Ok(None)) => Some("Session no longer available; refresh the list.".into()),
                Some(Err(error)) => Some(error.clone()),
                None => None,
            }
        }
        _ => None,
    }
}
fn draw_help(frame: &mut Frame<'_>, browser: &Browser, view: &View) {
    let inner = crate::modal::shell(frame, "Actions & shortcuts", 80, 28);
    if inner.width < 26 || inner.height < 6 {
        frame.render_widget(
            Paragraph::new("Enlarge terminal. Esc closes.").wrap(Wrap { trim: false }),
            inner,
        );
        return;
    }
    let regions = crate::modal::regions(inner);
    let sections = [regions.body, regions.status, regions.hints];
    let entries = menu_entries(browser, view);
    let selected = view.help_selected.min(entries.len() - 1);
    modal_actions(
        frame,
        regions.actions,
        "Close",
        "Run action",
        view.help_control,
        unavailable(entries[selected].command, browser, view).is_none(),
        true,
    );
    let mut items = Vec::new();
    let mut last_section = "";
    let mut selected_row = 0;
    for (i, entry) in entries.iter().enumerate() {
        if entry.section != last_section {
            items.push(ListItem::new(Line::styled(
                entry.section,
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            )));
            last_section = entry.section;
        }
        if i == selected {
            selected_row = items.len();
        }
        let disabled = unavailable(entry.command, browser, view).is_some();
        let style = if disabled {
            Style::default().fg(Color::DarkGray)
        } else {
            Style::default()
        };
        let lines = if sections[0].width >= 52 {
            vec![Line::from(vec![
                Span::styled(
                    format!("{:<22}", entry.shortcut),
                    style.add_modifier(Modifier::BOLD),
                ),
                Span::styled(entry.label, style),
            ])]
        } else {
            let mut lines = vec![Line::styled(
                entry.shortcut,
                style.add_modifier(Modifier::BOLD),
            )];
            let mut line = String::new();
            for word in entry.label.split_whitespace() {
                if !line.is_empty()
                    && line.len() + 1 + word.len()
                        > usize::from(sections[0].width.saturating_sub(3))
                {
                    lines.push(Line::styled(std::mem::take(&mut line), style));
                }
                if !line.is_empty() {
                    line.push(' ');
                }
                line.push_str(word);
            }
            lines.push(Line::styled(line, style));
            lines
        };
        items.push(ListItem::new(lines));
    }
    let heights: Vec<_> = items.iter().map(ListItem::height).collect();
    let total: usize = heights.iter().sum();
    let scrolling = total > usize::from(sections[0].height);
    let body = Rect::new(
        sections[0].x,
        sections[0].y,
        sections[0].width.saturating_sub(u16::from(scrolling)),
        sections[0].height,
    );
    let mut state = ListState::default().with_selected(Some(selected_row));
    frame.render_stateful_widget(
        List::new(items)
            .highlight_symbol("> ")
            .highlight_style(selection_style(view.help_control == 0)),
        body,
        &mut state,
    );
    if scrolling {
        let offset = heights.iter().take(state.offset()).sum();
        frame.render_stateful_widget(
            Scrollbar::default(),
            sections[0],
            &mut ScrollbarState::new(total)
                .position(offset)
                .viewport_content_length(usize::from(body.height)),
        );
    }
    if let Some(reason) = unavailable(entries[selected].command, browser, view) {
        frame.render_widget(
            Paragraph::new(reason).wrap(Wrap { trim: false }),
            sections[1],
        );
    }
    frame.render_widget(
        Paragraph::new(if inner.width >= 43 {
            "↑/↓ select · Enter activate · Esc close"
        } else if inner.width >= 32 {
            "Esc close · ↑↓ select · Enter run"
        } else {
            "Esc close · ↑↓ · Enter run"
        }),
        sections[2],
    );
}

pub(crate) fn activity_color(status: crate::domain::RuntimeStatus) -> Option<Color> {
    match status {
        crate::domain::RuntimeStatus::Running => Some(Color::Green),
        crate::domain::RuntimeStatus::Idle => Some(Color::Rgb(255, 255, 170)),
        crate::domain::RuntimeStatus::Waiting { .. } => Some(Color::Rgb(255, 165, 0)),
        _ => None,
    }
}
fn session_title(
    session: &crate::domain::SessionSummary,
    status: crate::domain::RuntimeStatus,
    width: u16,
) -> Line<'static> {
    use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};
    let title = format!(
        "{}{}",
        if session.archived { "[archived] " } else { "" },
        safe_text(&session.title)
    );
    let Some(label) = status.label() else {
        return Line::styled(title, Style::default().add_modifier(Modifier::BOLD));
    };
    let label = if usize::from(width) < label.width() + 12 {
        match status {
            crate::domain::RuntimeStatus::Running => "Running",
            crate::domain::RuntimeStatus::Waiting { .. } => "Waiting",
            _ => label,
        }
    } else {
        label
    };
    let available = usize::from(width).saturating_sub(label.width() + 1);
    let mut clipped = String::new();
    let mut used = 0;
    if title.width() > available {
        for c in title.chars() {
            let size = c.width().unwrap_or(0);
            if used + size > available.saturating_sub(1) {
                break;
            }
            clipped.push(c);
            used += size;
        }
        if available > 0 {
            clipped.push('…');
        }
    } else {
        clipped = title;
    }
    let padding = usize::from(width).saturating_sub(clipped.width() + label.width());
    Line::from(vec![
        Span::styled(clipped, Style::default().add_modifier(Modifier::BOLD)),
        Span::raw(" ".repeat(padding)),
        Span::styled(
            label,
            Style::default()
                .fg(activity_color(status).unwrap_or(Color::Reset))
                .add_modifier(Modifier::BOLD),
        ),
    ])
}

fn draw_search(frame: &mut Frame<'_>, browser: &Browser, view: &View) {
    let area = frame.area();
    let inner = crate::modal::shell(
        frame,
        if browser.focus_root.is_some() {
            "Search within folder"
        } else {
            "Search sessions"
        },
        area.width
            .saturating_sub(if area.width >= 28 { 4 } else { 0 }),
        area.height
            .saturating_sub(if area.height >= 12 { 2 } else { 0 }),
    );
    let compact = inner.height < 18;
    let short = inner.height < 12;
    let show_progress = browser.deep_search && !view.search_query.trim().is_empty();
    let mode_height = if compact { 1 } else { 2 };
    let sections = Layout::vertical([
        Constraint::Length(u16::from(browser.focus_root.is_some())),
        Constraint::Length(if short { 1 } else { 3 }),
        Constraint::Length(mode_height),
        Constraint::Min(1),
        Constraint::Length(u16::from(show_progress)),
        Constraint::Length(1),
        Constraint::Length(1),
    ])
    .split(inner);
    if let Some(root) = &browser.focus_root {
        let scope = safe_text(&format!("Within: {}", root.display()));
        let clipped =
            unicode_width::UnicodeWidthStr::width(scope.as_str()) > usize::from(sections[0].width);
        let mut line = graph_viewport(
            vec![Span::raw(scope)],
            0,
            usize::from(sections[0].width.saturating_sub(u16::from(clipped))),
        );
        if clipped && sections[0].width > 0 {
            line.spans.push(Span::raw("…"));
        }
        frame.render_widget(Paragraph::new(line), sections[0]);
    }
    let mode_style = if view.search_control == 3 {
        Style::default()
            .fg(Color::Black)
            .bg(Color::Cyan)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default()
            .fg(if browser.deep_search {
                Color::Green
            } else {
                Color::Yellow
            })
            .add_modifier(Modifier::BOLD)
    };
    let instructions = if inner.width >= 70 {
        "   Tab selects · Space toggles · Ctrl+D shortcut"
    } else {
        " · Ctrl+D"
    };
    let mut mode_lines = vec![Line::from(vec![
        Span::styled(
            if browser.deep_search {
                "[x] Deep search: ON"
            } else {
                "[ ] Deep search: OFF"
            },
            mode_style,
        ),
        Span::raw(instructions),
    ])];
    if !compact {
        mode_lines.push(Line::from(if inner.width < 70 {
            "Tab selects · Space toggles"
        } else if browser.deep_search {
            "Searches messages, plans, final replies, and metadata."
        } else {
            "Metadata only. Enable deep search to find conversation content."
        }));
    }
    if short {
        let text = safe_text(&format!("Search: {}", view.search_query));
        let width = usize::from(sections[1].width);
        let text_width = unicode_width::UnicodeWidthStr::width(text.as_str());
        let offset = text_width.saturating_sub(width.saturating_sub(1));
        frame.render_widget(
            Paragraph::new(graph_viewport(vec![Span::raw(text)], offset, width)),
            sections[1],
        );
        if view.search_control == 0 && sections[1].width > 0 && sections[1].height > 0 {
            frame.set_cursor_position((
                sections[1].x + text_width.min(width - 1) as u16,
                sections[1].y,
            ));
        }
    } else {
        crate::modal::text_field(
            frame,
            sections[1],
            "Search",
            &view.search_query,
            view.search_query.len(),
            view.search_control == 0,
            true,
        );
    }
    frame.render_widget(Paragraph::new(mode_lines), sections[2]);
    if show_progress {
        frame.render_widget(
            Paragraph::new(crate::search::progress(browser)),
            sections[4],
        );
    }
    modal_actions(
        frame,
        sections[5],
        "Cancel",
        "Open",
        view.search_control,
        !browser.search_results(&view.search_query).is_empty(),
        true,
    );
    frame.render_widget(
        Paragraph::new("↑/↓ results · Enter opens · Esc cancels"),
        sections[6],
    );
    let result_block = if sections[3].height >= 4 {
        block("Results — ↑/↓ select · Enter opens folder", false)
    } else {
        Block::default()
    }
    .padding(ratatui::widgets::Padding::horizontal(1));
    let results = browser.search_results(&view.search_query);
    if results.is_empty() {
        frame.render_widget(
            Paragraph::new(
                if browser.loading()
                    || (browser.deep_search
                        && !view.search_query.trim().is_empty()
                        && crate::search::pending(browser))
                {
                    "Searching…"
                } else {
                    "No matching sessions"
                },
            )
            .block(result_block),
            sections[3],
        );
        return;
    }
    let index = results
        .iter()
        .position(|s| Some(&s.key) == view.search_selected.as_ref())
        .unwrap_or(0);
    let result_height = usize::from(result_block.inner(sections[3]).height);
    let items: Vec<_> = results
        .iter()
        .map(|s| {
            let mut lines = vec![
                session_title(
                    s,
                    browser.runtime_status(&s.key),
                    sections[3].width.saturating_sub(6),
                ),
                Line::from(safe_text(
                    &s.cwd
                        .as_ref()
                        .map(|p| p.display().to_string())
                        .unwrap_or_else(|| "Unknown folder".into()),
                )),
            ];
            if let Some(excerpt) = crate::search::excerpt(browser, s, &view.search_query) {
                lines.push(Line::from(excerpt));
            }
            if !compact {
                lines.push(Line::default());
            }
            lines.truncate(result_height.max(1));
            ListItem::new(lines)
        })
        .collect();
    frame.render_stateful_widget(
        List::new(items)
            .block(result_block)
            .highlight_style(selection_style(view.help_control == 0))
            .highlight_symbol("> "),
        sections[3],
        &mut ListState::default().with_selected(Some(index)),
    );
}

/// Slice terminal columns without splitting graphemes or wide glyphs.
fn graph_viewport(spans: Vec<Span<'static>>, offset: usize, width: usize) -> Line<'static> {
    use unicode_segmentation::UnicodeSegmentation;
    use unicode_width::UnicodeWidthStr;
    let end = offset.saturating_add(width);
    let mut column = 0;
    let mut clipped = Vec::new();
    for span in spans {
        for grapheme in span.content.graphemes(true) {
            let next = column + grapheme.width();
            if column >= end {
                return Line::from(clipped);
            }
            if next > offset {
                let text = if column >= offset && next <= end {
                    grapheme.to_owned()
                } else {
                    " ".repeat(next.min(end).saturating_sub(column.max(offset)))
                };
                clipped.push(Span::styled(text, span.style));
            }
            column = next;
        }
    }
    Line::from(clipped)
}

/// Draw connectors from the complete displayed hierarchy, including projected
/// worktrees. Filesystem parents and viewport boundaries are not tree endings.
fn folder_tree_labels(rows: &[crate::browser::FolderRow]) -> Vec<String> {
    let mut later_sibling = vec![false; rows.len()];
    let mut depths = Vec::new();
    for (index, row) in rows.iter().enumerate().rev() {
        while depths.last().is_some_and(|depth| *depth > row.depth) {
            depths.pop();
        }
        later_sibling[index] = depths.last() == Some(&row.depth);
        if !later_sibling[index] {
            depths.push(row.depth);
        }
    }
    let mut ancestors = Vec::new();
    rows.iter()
        .enumerate()
        .map(|(index, row)| {
            ancestors.resize(row.depth + 1, false);
            ancestors[row.depth] = later_sibling[index];
            let mut label = String::new();
            if row.depth > 0 {
                for &continues in ancestors.iter().take(row.depth.min(20)).skip(1) {
                    label.push_str(if continues { "│  " } else { "   " });
                }
                label.push_str(if later_sibling[index] {
                    "├─ "
                } else {
                    "└─ "
                });
            }
            label.push_str(&safe_text(&row.label));
            label.push_str(&format!(" ({})", row.count));
            label
        })
        .collect()
}

fn folder_tree_line(
    row: &crate::browser::FolderRow,
    label: &str,
    status: crate::domain::RuntimeStatus,
    offset: usize,
    width: usize,
) -> Line<'static> {
    let text = format!("{} ({})", safe_text(&row.label), row.count);
    let prefix = &label[..label.len() - text.len()];
    let text_style =
        activity_color(status).map_or(Style::default(), |color| Style::default().fg(color));
    let mut spans = vec![Span::styled(
        prefix.to_owned(),
        Style::default().fg(Color::White),
    )];
    let safe_label = safe_text(&row.label);
    let (name, dirty) = safe_label
        .strip_suffix(" *")
        .map_or((safe_label.as_str(), false), |s| (s, true));
    if let Some(name) = name.strip_prefix(crate::git::WORKTREE_MARKER) {
        spans.push(Span::styled(
            "◇",
            Style::default().fg(crate::git_render::WORKTREE_COLOR),
        ));
        spans.push(Span::styled(format!(" {name}"), text_style));
    } else {
        spans.push(Span::styled(name.to_owned(), text_style));
    }
    if dirty {
        spans.push(Span::styled(" ", text_style));
        spans.push(Span::styled(
            "*",
            Style::default().fg(crate::git_render::DIRTY_COLOR),
        ));
    }
    spans.push(Span::styled(format!(" ({})", row.count), text_style));
    graph_viewport(spans, offset, width)
}

fn draw_pane(frame: &mut Frame<'_>, area: Rect, pane: usize, browser: &Browser, view: &View) {
    match pane {
        0 => {
            let folders = browser.folder_rows();
            let selected = folders.iter().position(|row| row.filter == browser.folder);
            let labels = folder_tree_labels(&folders);
            // Borders and the fixed selection marker occupy four columns.
            let width = usize::from(area.width.saturating_sub(4));
            let offset = selected
                .map(|index| {
                    unicode_width::UnicodeWidthStr::width(labels[index].as_str())
                        .saturating_sub(width)
                })
                .unwrap_or(0);
            let items: Vec<_> = folders
                .iter()
                .zip(&labels)
                .map(|(row, label)| {
                    ListItem::new(folder_tree_line(
                        row,
                        label,
                        browser.folder_activity(&row.filter),
                        offset,
                        width,
                    ))
                })
                .collect();
            // Folder rows occupy one line each. Keep the selection centered,
            // clamping at the ends so the viewport stays filled.
            let block = pane_block(
                if browser.focus_root.is_some() {
                    "Folders · Focused"
                } else {
                    "Folders"
                },
                view.context() == 0,
            );
            let visible_rows = usize::from(block.inner(area).height);
            let top = selected
                .filter(|_| visible_rows > 0)
                .map(|index| {
                    index
                        .saturating_sub(visible_rows / 2)
                        .min(folders.len().saturating_sub(visible_rows))
                })
                .unwrap_or(0);
            let mut state = ListState::default()
                .with_offset(top)
                .with_selected(selected);
            frame.render_stateful_widget(
                List::new(items)
                    .block(block)
                    .highlight_style(selection_style(view.context() == pane))
                    .highlight_symbol("> "),
                area,
                &mut state,
            );
        }
        1 => {
            if view
                .terminals
                .borrow_mut()
                .draw(frame, area, view.terminal_focused())
            {
                return;
            }
            let sessions = browser.visible();
            // Preserve eight columns for essential status labels in tiny panes.
            let right_padding = area.width.saturating_sub(12).min(2);
            let now = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs()
                .min(i64::MAX as u64) as i64;
            if sessions.is_empty() {
                draw_empty_sessions(frame, area, browser, view.context() == 1);
                return;
            }
            let items: Vec<_> = sessions
                .iter()
                .map(|s| {
                    ListItem::new(vec![
                        session_title(
                            s,
                            browser.runtime_status(&s.key),
                            area.width.saturating_sub(4 + right_padding),
                        ),
                        if view.terminals.borrow().owns(&s.key) {
                            Line::from("Open here · Enter Embedded · Alt+F Full-screen · x Close terminal")
                        } else if browser.specs.iter().any(|spec| matches!(spec, crate::browser::SourceSpec::Codex { source, .. } if source.id == s.key.source_id && crate::inline_terminal::open_elsewhere(&source.home, &s.key))) {
                            Line::from("Open in another tracr")
                        } else {
                            session_metadata(browser, s, now)
                        },
                        Line::default(),
                    ])
                })
                .collect();
            let mut state = ListState::default().with_selected(
                sessions
                    .iter()
                    .position(|s| Some(&s.key) == browser.selected.as_ref()),
            );
            frame.render_stateful_widget(
                List::new(items)
                    .block(
                        pane_block(&browser.scope_label(), view.context() == 1)
                            .padding(ratatui::widgets::Padding::new(0, right_padding, 1, 0)),
                    )
                    .highlight_style(selection_style(view.context() == pane))
                    .highlight_symbol("> "),
                area,
                &mut state,
            );
        }
        _ => {}
    }
}

fn draw_empty_sessions(frame: &mut Frame, area: Rect, browser: &Browser, focused: bool) {
    let title = browser.scope_label();
    let margin = area.width.saturating_sub(10).min(4) / 2;
    let border =
        pane_block(&title, focused).padding(ratatui::widgets::Padding::new(margin, margin, 1, 0));
    let inner = border.inner(area);
    frame.render_widget(border, area);
    if inner.width == 0 || inner.height == 0 {
        return;
    }
    let status = if browser.loading() {
        "Loading sessions…"
    } else if browser.diagnostics().iter().any(|source| !source.complete) {
        "Session inventory incomplete.\nSee source errors below."
    } else {
        "No sessions currently shown"
    };
    let primary_style = Style::default()
        .add_modifier(Modifier::BOLD)
        .fg(if focused { Color::Cyan } else { Color::Reset });
    let muted = Style::default().fg(Color::DarkGray);
    let width = inner.width.min(40);
    let create = can_create(browser);
    let mut gaps = true;
    let mut hints = true;
    let mut compact = false;
    let paragraph = loop {
        let mut lines: Vec<Line<'_>> = status.lines().map(Line::from).collect();
        if create {
            if gaps {
                lines.push(Line::default());
            }
            if compact {
                lines.push(Line::styled("Enter: new session", primary_style));
            } else {
                lines.push(Line::styled("Create new session", primary_style));
                lines.push(Line::styled("Press Enter", primary_style));
            }
        }
        if hints {
            if gaps {
                lines.push(Line::default());
            }
            if !browser.include_archived {
                lines.push(Line::styled("a  Show archived sessions", muted));
            }
            lines.push(Line::styled(
                if browser.focus_root.is_some() {
                    "/  Search within focused folder"
                } else {
                    "/  Search all sessions"
                },
                muted,
            ));
        }
        let paragraph = Paragraph::new(lines)
            .alignment(ratatui::layout::Alignment::Center)
            .wrap(Wrap { trim: true });
        if paragraph.line_count(width) <= usize::from(inner.height) {
            break paragraph;
        }
        if gaps {
            gaps = false;
        } else if hints {
            hints = false;
        } else if create && !compact {
            compact = true;
        } else {
            break paragraph;
        }
    };
    let height = paragraph.line_count(width).min(usize::from(inner.height)) as u16;
    let spare_rows = inner.height - height;
    // The pane already supplies one blank row; add three more when space permits.
    let top_offset = spare_rows.min(3);
    let content = Rect::new(
        inner.x + (inner.width - width) / 2,
        inner.y + top_offset,
        width,
        height,
    );
    frame.render_widget(paragraph, content);
}

fn selected_checkout(browser: &Browser) -> Option<&std::path::Path> {
    let FolderFilter::Path(path) = &browser.folder else {
        return None;
    };
    browser
        .repositories
        .values()
        .flat_map(|r| &r.worktrees)
        .find(|w| !w.bare && &w.path == path)
        .map(|w| w.path.as_path())
}

fn worktree_status_description(browser: &Browser, path: &std::path::Path) -> String {
    match browser
        .worktree_status
        .iter()
        .filter(|(root, _)| path.starts_with(root))
        .max_by_key(|(root, _)| root.components().count())
        .map(|(_, s)| s)
    {
        Some(Ok(counts)) => counts.description(),
        Some(Err(error)) => format!("Unavailable: {error}"),
        None => "Not yet available".into(),
    }
}

fn session_information(
    browser: &Browser,
    s: &crate::domain::SessionSummary,
) -> crate::info_content::Information {
    use crate::domain::{FolderStatus, RuntimeStatus};
    use crate::info_content::{Information, Row, Section, local_date};
    let value = |value: Option<&str>| {
        value
            .filter(|s| !s.is_empty())
            .unwrap_or("Not recorded")
            .to_owned()
    };
    let owner = browser
        .specs
        .iter()
        .find_map(|spec| match spec {
            crate::browser::SourceSpec::Codex { source, .. } if source.id == s.key.source_id => {
                Some(source.home.display().to_string())
            }
            _ => None,
        })
        .unwrap_or_else(|| "Not recorded".into());
    let path = s
        .cwd
        .as_ref()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|| "Not recorded".into());
    let folder_state = match s.cwd.as_ref().and_then(|p| browser.folders.get(p)) {
        Some(FolderStatus::Available) => "Available".into(),
        Some(FolderStatus::Unlinked) => "Missing".into(),
        Some(FolderStatus::Inaccessible { reason }) => format!("Inaccessible: {reason}"),
        _ => "Unknown".into(),
    };
    let mut notices = vec![];
    let mut location = vec![Row::new("Folder", path.clone())];
    match browser.details.get(&s.key) {
        Some(Ok(Some(detail))) if detail.cwd != s.cwd => {
            notices.push("Folder differs from Codex's current resume destination.".into());
            location = vec![
                Row::new("Listed folder", path),
                Row::new(
                    "Resume folder",
                    detail
                        .cwd
                        .as_ref()
                        .map(|p| p.display().to_string())
                        .unwrap_or_else(|| "Not recorded".into()),
                ),
            ];
        }
        Some(Err(error)) => notices.push(format!("Could not load session information: {error}")),
        Some(Ok(None)) => notices.push("Session no longer available".into()),
        None => notices.push("Fetching metadata…".into()),
        _ => (),
    }
    if is_unlinked(browser, s) || crate::domain::resume_blocker(s).is_some() {
        notices.push(resume_hint(browser, s));
    }
    let runtime = browser.runtime_status(&s.key);
    let mut status = Row::new(
        "Status",
        if runtime == RuntimeStatus::Unknown {
            "Unknown"
        } else {
            runtime.label().unwrap_or("Unknown")
        },
    );
    status.color = activity_color(runtime);
    status.note = Some(
        match runtime {
            RuntimeStatus::Idle => "Cached by Codex; release is attempted when moving.",
            RuntimeStatus::Unloaded => "Released at the last check; moving still checks ownership.",
            RuntimeStatus::Running => "Working in Codex; close the session before moving.",
            RuntimeStatus::Waiting { .. } => "Waiting in Codex; close the session before moving.",
            RuntimeStatus::Unknown => "Live state unavailable; moving checks native ownership.",
            RuntimeStatus::Error => "Codex reported an error; moving checks native ownership.",
        }
        .into(),
    );
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64);
    let activity = if s.updated_at == 0 {
        "Not recorded".into()
    } else {
        format!(
            "{}\n{}",
            relative_time(s.updated_at, now),
            local_date(s.updated_at)
        )
    };
    if let Some(path) = s.cwd.as_ref() {
        location.push(Row::new(
            "Uncommitted changes",
            worktree_status_description(browser, path),
        ));
    }
    location.extend([
        Row::new("Folder state", folder_state),
        Row::new(
            "Git branch",
            value(s.git.as_ref().and_then(|g| g.branch.as_deref())),
        ),
        Row::new(
            "Repository",
            value(s.git.as_ref().and_then(|g| g.origin_url.as_deref())),
        ),
    ]);
    Information {
        title: s.title.clone(),
        notices,
        sections: vec![
            Section {
                title: "Session",
                rows: vec![
                    status,
                    Row::new("Last activity", activity),
                    Row::new("Created", local_date(s.created_at)),
                    Row::new(
                        "Agent",
                        if s.key.provider.0 == "codex" {
                            "Codex".into()
                        } else {
                            s.key.provider.0.clone()
                        },
                    ),
                    Row::new("Archived", if s.archived { "Yes" } else { "No" }),
                ],
            },
            Section {
                title: "Location",
                rows: location,
            },
            Section {
                title: "Technical details",
                rows: vec![
                    Row::new("Session ID", s.key.native_id.clone()),
                    Row::new("Codex home", owner),
                    Row::new(
                        "Session source",
                        if s.source_kind.as_deref() == Some("cli") {
                            "CLI".into()
                        } else {
                            value(s.source_kind.as_deref())
                        },
                    ),
                    Row::new(
                        "Git commit",
                        value(s.git.as_ref().and_then(|g| g.commit.as_deref())),
                    ),
                ],
            },
        ],
    }
}

pub struct InfoModal {
    pub session: crate::domain::SessionSummary,
    pub dialog: crate::error_dialog::ErrorDialog,
    warning: Option<String>,
}
impl InfoModal {
    pub fn new(browser: &Browser, session: crate::domain::SessionSummary) -> Self {
        let mut dialog = crate::error_dialog::ErrorDialog::information("Session information", "");
        dialog.set_information(session_information(browser, &session));
        Self {
            session,
            dialog,
            warning: None,
        }
    }
    pub fn refresh(&mut self, browser: &Browser) {
        let current = browser
            .all()
            .into_iter()
            .find(|s| s.key == self.session.key);
        let missing = current.is_none() && !browser.loading();
        if let Some(current) = current {
            self.session = current;
        }
        let mut body = session_information(browser, &self.session);
        if missing {
            body.notices.insert(
                0,
                "Session no longer available in the current inventory.".into(),
            );
        }
        if let Some(warning) = &self.warning {
            body.notices.insert(0, warning.clone());
        }
        self.dialog.set_information(body);
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum Action {
    CopyErrorReport,
    CopyInformation,
    CloseInformation,
    Continue,
    OpenRelink,
    OpenFolderMove,
    PreviewFolderMove,
    ApplyFolderMove,
    CloseFolderMove,
    CancelRelink,
    RefreshTerminals,
    StopTerminals,
    PreviewRelink,
    ApplyRelink,
    RecoverRelink,
    Quit,
    Refresh,
    ResumeSelected,
    ResumeSelectedFullscreen,
    CancelResume,
    CreateNew(usize),
    CreateNewFullscreen(usize),
}

fn selected_session(browser: &Browser) -> Option<crate::domain::SessionSummary> {
    browser
        .visible()
        .into_iter()
        .find(|s| Some(&s.key) == browser.selected.as_ref())
}
fn relink_unavailable(browser: &Browser) -> Option<String> {
    let session = match selected_session(browser) {
        Some(s) => s,
        None => return Some("Select a session first.".into()),
    };
    if browser.loading() {
        return Some("Wait for discovery to finish.".into());
    }
    let index=browser.specs.iter().position(|s|matches!(s,crate::browser::SourceSpec::Codex{source,..} if source.id==session.key.source_id));
    let Some(index) = index else {
        return Some("Fixture data is browse-only.".into());
    };
    if browser.sources[index].error.is_some() || !browser.sources[index].warnings.is_empty() {
        return Some("Resolve source discovery warnings before relinking.".into());
    }
    if browser.sources[index].live.contains_key(&session.key)
        && browser.runtime_status(&session.key) != crate::domain::RuntimeStatus::Idle
    {
        use crate::domain::RuntimeStatus;
        return Some(match browser.runtime_status(&session.key) {
            RuntimeStatus::Running => "This session is running in Codex. Close it and wait for Codex to release it before relinking.",
            RuntimeStatus::Waiting { .. } => "This session is waiting in Codex. Close it and wait for Codex to release it before relinking.",
            _ => "Codex still reports this session as loaded. Wait for it to be released before relinking.",
        }.into());
    }
    if !matches!(
        crate::domain::classify_folder(session.cwd.as_deref()),
        crate::domain::FolderStatus::Available | crate::domain::FolderStatus::Unlinked
    ) {
        return Some("Relinking requires a known original folder that is not inaccessible.".into());
    }
    if browser.state_path.is_none() {
        return Some("Persistent state is required for relocation recovery.".into());
    }
    None
}
enum FolderMoveResult {
    Preview(Result<crate::folder_move::Plan, String>),
    Applied(crate::folder_move::Outcome),
}
enum RelinkResult {
    Terminals(Result<Vec<crate::providers::codex_terminals::Terminal>, crate::relocation::Failure>),
    Preview(Result<crate::relocation::Preview, crate::relocation::Failure>),
    Applied(crate::relocation::Outcome),
}
fn creation_sources(browser: &Browser) -> Vec<usize> {
    browser
        .specs
        .iter()
        .enumerate()
        .filter_map(|(i, spec)| spec.creation_source_id().map(|_| i))
        .collect()
}
fn creation_action(source: usize, fullscreen: bool) -> Action {
    if fullscreen {
        Action::CreateNewFullscreen(source)
    } else {
        Action::CreateNew(source)
    }
}

/// Enter only a known directory inventory, never a transient search result.
fn enter_folder(browser: &mut Browser, view: &mut View) -> Action {
    view.focus = 1;
    view.git_focus = false;
    if !matches!(browser.folder, FolderFilter::Path(_)) {
        return Action::Continue;
    }
    browser.query.clear();
    browser.reconcile(0);
    if browser.loading() || browser.diagnostics().iter().any(|source| !source.complete) {
        return Action::Continue;
    }
    let sessions = browser.visible();
    match sessions.as_slice() {
        [] => execute_command(Command::Create, browser, view),
        [session] => {
            browser.selected = Some(session.key.clone());
            execute_command(Command::Resume, browser, view)
        }
        _ => Action::Continue,
    }
}

fn can_create(browser: &Browser) -> bool {
    !browser.loading()
        && matches!(browser.folder, FolderFilter::Path(_))
        && !creation_sources(browser).is_empty()
}

fn resume_hint(browser: &Browser, session: &crate::domain::SessionSummary) -> String {
    if browser
        .specs
        .iter()
        .all(|s| matches!(s, crate::browser::SourceSpec::Fixture(_)))
    {
        return "Fixture data is browse-only.".into();
    }
    if let Some(reason) = crate::domain::resume_blocker(session) {
        return reason.into();
    }
    if let Some(Ok(Some(detail))) = browser.details.get(&session.key) {
        if let Some(reason) = crate::domain::resume_blocker(detail) {
            return reason.into();
        }
    }
    if is_unlinked(browser, session) {
        return relink_unavailable(browser)
            .unwrap_or_else(|| "Folder missing — Enter or m to move session.".into());
    }
    "Enter opens embedded · Alt+F opens full-screen; the destination is checked again before launch.".into()
}

pub fn handle_key(browser: &mut Browser, view: &mut View, key: KeyEvent) -> Action {
    if key.kind == KeyEventKind::Release {
        return Action::Continue;
    }
    if key.modifiers == KeyModifiers::ALT && key.code == KeyCode::Char('f') {
        if key.kind == KeyEventKind::Press
            && !view.modal_open()
            && !view.terminals.borrow().has_overlay()
        {
            return execute_command(Command::ToggleTerminalFullscreen, browser, view);
        }
        return Action::Continue;
    }
    if key.modifiers == KeyModifiers::ALT {
        let command = match key.code {
            KeyCode::Up => Some(Command::PaneUp),
            KeyCode::Down => Some(Command::PaneDown),
            KeyCode::Left => Some(Command::PaneLeft),
            KeyCode::Right => Some(Command::PaneRight),
            _ => None,
        };
        if let Some(command) = command {
            // These keys belong to pane navigation, never to a dialog's arrows.
            if view.modal_open() {
                return Action::Continue;
            }
            return execute_command(command, browser, view);
        }
    }
    if let Some(dialog) = view.terminal_recovery.as_mut() {
        use crate::terminal_recovery::Event;
        return match dialog.key(key) {
            Event::None => Action::Continue,
            Event::Cancel => {
                view.terminal_recovery = None;
                view.relink = None;
                Action::Continue
            }
            Event::Refresh => Action::RefreshTerminals,
            Event::Stop => Action::StopTerminals,
            Event::Retry => {
                view.terminal_recovery = None;
                Action::ApplyRelink
            }
        };
    }
    if let Some(dialog) = view.error_dialog.as_mut() {
        match dialog.key(key) {
            crate::error_dialog::Event::Copy => return Action::CopyErrorReport,
            crate::error_dialog::Event::Close => view.error_dialog = None,
            crate::error_dialog::Event::None => {}
        }
        return Action::Continue;
    }
    if let Some(mut picker) = view.worktree_picker.take() {
        match key.code {
            KeyCode::Esc => return Action::Continue,
            KeyCode::Up | KeyCode::Char('k') => picker.selected = picker.selected.saturating_sub(1),
            KeyCode::Down | KeyCode::Char('j') => {
                picker.selected = (picker.selected + 1).min(picker.matches.len().saturating_sub(1))
            }
            KeyCode::Enter if key.kind != KeyEventKind::Repeat => {
                return select_git_worktree(browser, view, &picker);
            }
            _ => {}
        }
        view.worktree_picker = Some(picker);
        return Action::Continue;
    }
    if let Some(modal) = view.worktree_create.as_mut() {
        if modal.key(key) {
            view.worktree_create = None;
        }
        return Action::Continue;
    }
    if let Some(info) = view.commit_info.as_mut() {
        match info.key(key) {
            crate::error_dialog::Event::Copy => info.copy_report(),
            crate::error_dialog::Event::Close => view.commit_info = None,
            crate::error_dialog::Event::None => {}
        }
        return Action::Continue;
    }
    if let Some(info) = view.info.as_mut() {
        match info.dialog.key(key) {
            crate::error_dialog::Event::Copy => return Action::CopyInformation,
            crate::error_dialog::Event::Close => {
                view.info = None;
                return Action::CloseInformation;
            }
            crate::error_dialog::Event::None => return Action::Continue,
        }
    }
    if let Some(modal) = view.folder_move.as_mut() {
        return modal.key(key);
    }
    if let Some(modal) = view.relink.as_mut() {
        if modal.busy {
            return if !modal.executing && key.code == KeyCode::Esc {
                Action::CancelRelink
            } else {
                Action::Continue
            };
        }
        if modal.preview.is_none()
            && modal.picker.as_ref().is_some_and(|p| p.focus == 3)
            && matches!(key.code, KeyCode::Enter | KeyCode::Char(' '))
        {
            return Action::CancelRelink;
        }
        if modal.preview.is_some() {
            match key.code {
                KeyCode::Up | KeyCode::PageUp => {
                    view.relink_scroll = view.relink_scroll.saturating_sub(1);
                    return Action::Continue;
                }
                KeyCode::Down | KeyCode::PageDown => {
                    view.relink_scroll = view
                        .relink_scroll
                        .saturating_add(1)
                        .min(view.relink_scroll_limit.get());
                    return Action::Continue;
                }
                KeyCode::Home => {
                    view.relink_scroll = 0;
                    return Action::Continue;
                }
                KeyCode::End => {
                    view.relink_scroll = view.relink_scroll_limit.get();
                    return Action::Continue;
                }
                _ => {}
            }
        }
        if modal.preview.is_some() && matches!(key.code, KeyCode::Tab | KeyCode::BackTab) {
            view.relink_back = !view.relink_back;
            return Action::Continue;
        }
        if modal.preview.is_some()
            && view.relink_back
            && matches!(key.code, KeyCode::Enter | KeyCode::Char(' '))
        {
            return Action::CancelRelink;
        }
        if modal.preview.is_none() && key.code != KeyCode::Esc {
            if let Some(picker) = modal.picker.as_mut() {
                modal.error = None;
                return if picker.key(&mut modal.input, key) {
                    Action::PreviewRelink
                } else {
                    Action::Continue
                };
            }
        }
        match key.code {
            KeyCode::Esc => view.relink = None,
            KeyCode::Char('u') if modal.recovery => return Action::RecoverRelink,
            KeyCode::Enter | KeyCode::Char(' ')
                if key.code == KeyCode::Enter || modal.preview.is_some() =>
            {
                return if modal.preview.is_some() {
                    Action::ApplyRelink
                } else {
                    Action::PreviewRelink
                };
            }
            KeyCode::Backspace if modal.preview.is_none() => {
                modal.input.pop();
            }
            KeyCode::Char(c)
                if modal.preview.is_none() && !key.modifiers.contains(KeyModifiers::CONTROL) =>
            {
                modal.input.push(c)
            }
            _ => {}
        }
        return Action::Continue;
    }
    if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
        return Action::Quit;
    }
    if view.preparing {
        return match key.code {
            KeyCode::Esc => Action::CancelResume,
            KeyCode::Char('q') => Action::Quit,
            _ => Action::Continue,
        };
    }
    if let Some(selected) = view.source_picker.as_mut() {
        if key.kind == KeyEventKind::Repeat && key.code == KeyCode::Enter {
            return Action::Continue;
        }
        let sources = creation_sources(browser);
        match key.code {
            KeyCode::Tab | KeyCode::BackTab => crate::modal::cycle(
                &mut view.source_control,
                &[true, true, !sources.is_empty()],
                key.code == KeyCode::BackTab,
            ),
            KeyCode::Enter | KeyCode::Char(' ') if view.source_control == 1 => {
                view.source_picker = None
            }
            KeyCode::Char(' ') if view.source_control == 2 => {
                let source = sources.get(*selected).copied();
                view.source_picker = None;
                if let Some(source) = source {
                    return creation_action(source, view.source_fullscreen);
                }
            }
            KeyCode::Esc => view.source_picker = None,
            KeyCode::Up => *selected = selected.saturating_sub(1),
            KeyCode::Down => *selected = (*selected + 1).min(sources.len().saturating_sub(1)),
            KeyCode::Enter => {
                let source = sources.get(*selected).copied();
                view.source_picker = None;
                if let Some(source) = source {
                    return creation_action(source, view.source_fullscreen);
                }
            }
            _ => {}
        }
        return Action::Continue;
    }
    if view.help {
        let entries = menu_entries(browser, view);
        view.help_selected = view.help_selected.min(entries.len() - 1);
        let step = view.help_page_size.max(1);
        let action_enabled =
            unavailable(entries[view.help_selected].command, browser, view).is_none();
        match key.code {
            KeyCode::Tab | KeyCode::BackTab => crate::modal::cycle(
                &mut view.help_control,
                &[true, true, action_enabled],
                key.code == KeyCode::BackTab,
            ),
            KeyCode::Enter | KeyCode::Char(' ') if view.help_control == 1 => view.help = false,
            KeyCode::Esc | KeyCode::Char('?') => view.help = false,
            KeyCode::Up | KeyCode::Char('k') => {
                view.help_selected = view.help_selected.saturating_sub(1)
            }
            KeyCode::Down | KeyCode::Char('j') => {
                view.help_selected = (view.help_selected + 1).min(entries.len() - 1)
            }
            KeyCode::PageUp => view.help_selected = view.help_selected.saturating_sub(step),
            KeyCode::PageDown => {
                view.help_selected = (view.help_selected + step).min(entries.len() - 1)
            }
            KeyCode::Home => view.help_selected = 0,
            KeyCode::End => view.help_selected = entries.len() - 1,
            KeyCode::Enter | KeyCode::Char(' ') if key.kind != KeyEventKind::Repeat => {
                let command = entries[view.help_selected].command;
                if unavailable(command, browser, view).is_none() {
                    view.help = false;
                    return execute_command(command, browser, view);
                }
            }
            _ => {}
        }
        return Action::Continue;
    }
    if view.editing {
        let results = browser.search_results(&view.search_query);
        let index = results
            .iter()
            .position(|s| Some(&s.key) == view.search_selected.as_ref())
            .unwrap_or(0);
        match key.code {
            KeyCode::Tab | KeyCode::BackTab => {
                // Follow the visual order: query, deep search, Cancel, Open.
                let controls = [0, 3, 1, 2];
                let mut index = controls
                    .iter()
                    .position(|c| *c == view.search_control)
                    .unwrap_or(0);
                crate::modal::cycle(
                    &mut index,
                    &[true, true, true, !results.is_empty()],
                    key.code == KeyCode::BackTab || key.modifiers.contains(KeyModifiers::SHIFT),
                );
                view.search_control = controls[index];
            }
            KeyCode::Char('d') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                browser.deep_search = !browser.deep_search;
                view.search_selected = None;
            }
            KeyCode::Enter | KeyCode::Char(' ') if view.search_control == 3 => {
                browser.deep_search = !browser.deep_search;
                view.search_selected = None;
            }
            KeyCode::Enter | KeyCode::Char(' ') if view.search_control == 1 => view.editing = false,
            KeyCode::Esc => view.editing = false,
            KeyCode::Up | KeyCode::Down => {
                let next = if key.code == KeyCode::Up {
                    index.saturating_sub(1)
                } else {
                    (index + 1).min(results.len().saturating_sub(1))
                };
                view.search_selected = results.get(next).map(|s| s.key.clone());
            }
            KeyCode::Enter | KeyCode::Char(' ')
                if key.code == KeyCode::Enter || view.search_control == 2 =>
            {
                if let Some(session) = results.get(index) {
                    browser.query.clear();
                    if let Some(path) = session.cwd.as_ref().filter(|path| path.is_absolute()) {
                        browser.open_folder(path.clone());
                    } else {
                        browser.select_folder(FolderFilter::Unknown);
                    }
                    browser.selected = Some(session.key.clone());
                    browser.reconcile(0);
                    view.focus = 1;

                    view.message = None;
                    view.editing = false;
                }
            }
            KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                view.search_query.clear();
                view.search_selected = None;
            }
            KeyCode::Backspace => {
                view.search_query.pop();
                view.search_selected = None;
            }
            KeyCode::Char(c)
                if view.search_control == 0
                    && !key
                        .modifiers
                        .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
            {
                view.search_query.push(c);
                view.search_selected = None;
            }
            _ => {}
        }
        return Action::Continue;
    }
    if matches!(key.code, KeyCode::Tab | KeyCode::BackTab) {
        let command = if key.code == KeyCode::BackTab || key.modifiers.contains(KeyModifiers::SHIFT)
        {
            Command::CyclePaneBack
        } else {
            Command::CyclePane
        };
        return execute_command(command, browser, view);
    }
    if view.git_focus {
        let command = match key.code {
            KeyCode::Char('b') => Command::GitToggle,
            KeyCode::Esc => Command::GitReturn,
            KeyCode::Char('g') => Command::OpenTerminals,
            KeyCode::Char('x') if view.focus == 1 => Command::CloseTerminal,
            KeyCode::Char('q') => Command::Quit,
            KeyCode::Char('r') => Command::Refresh,
            KeyCode::Char('?') => {
                view.help_selected = 0;
                Command::Help
            }
            _ if view.git_hidden || view.git_size_limited => return Action::Continue,
            KeyCode::Enter if key.kind != KeyEventKind::Repeat => Command::GitSelectWorktree,
            KeyCode::Char('w') => Command::CreateWorktree,
            KeyCode::Char('i') => Command::CommitInfo,
            KeyCode::Up | KeyCode::Char('k') => Command::GitUp,
            KeyCode::Down | KeyCode::Char('j') => Command::GitDown,
            KeyCode::Left | KeyCode::Char('h') => Command::GitLeft,
            KeyCode::Right | KeyCode::Char('l') => Command::GitRight,
            KeyCode::PageUp => Command::GitPageUp,
            KeyCode::PageDown => Command::GitPageDown,
            KeyCode::Home => Command::GitHome,
            KeyCode::End => Command::GitEnd,
            KeyCode::Char('f') => Command::GitFollow,
            _ => return Action::Continue,
        };
        return execute_command(command, browser, view);
    }
    let command = match key.code {
        KeyCode::Char('?') => {
            open_help(browser, view);
            view.help_selected = menu_entries(browser, view)
                .iter()
                .position(|entry| unavailable(entry.command, browser, view).is_none())
                .unwrap_or(0);
            return Action::Continue;
        }
        KeyCode::Char('g') => Command::OpenTerminals,
        KeyCode::Char('x') if view.focus == 1 => Command::CloseTerminal,
        KeyCode::Char('q') => Command::Quit,
        KeyCode::Char('/') => Command::Search,
        KeyCode::Char('r') => Command::Refresh,
        KeyCode::Char('i') if !view.git_focus => Command::Info,
        KeyCode::Char('n') if view.focus == 1 => Command::Create,
        KeyCode::Char('n') if view.focus == 0 => Command::CreateFullscreen,
        KeyCode::Char('m') if view.focus == 1 => Command::Relink,
        KeyCode::Char('b') if view.focus == 0 => Command::FolderToggle,
        KeyCode::Char('m') if view.focus == 0 => Command::MoveFolder,
        KeyCode::Char('w') if view.focus == 0 => Command::CreateWorktree,
        KeyCode::Char('u') => Command::Recover,
        KeyCode::Char('a') => Command::Archives,
        KeyCode::Right | KeyCode::Char('l') if view.focus == 0 => Command::FolderRight,
        KeyCode::Left | KeyCode::Char('h') if view.focus == 0 => Command::FolderLeft,
        KeyCode::Left | KeyCode::Backspace if view.focus == 1 => Command::PreviousPane,
        KeyCode::Esc => Command::PreviousPane,
        KeyCode::Up | KeyCode::Char('k') => Command::PreviousItem,
        KeyCode::Down | KeyCode::Char('j') => Command::NextItem,
        KeyCode::Enter if key.kind == KeyEventKind::Repeat => return Action::Continue,
        KeyCode::Enter if view.focus == 0 => Command::EnterFolder,
        KeyCode::Enter
            if view.focus == 1 && can_create(browser) && browser.visible().is_empty() =>
        {
            Command::Create
        }
        KeyCode::Char('z') if key.modifiers == KeyModifiers::ALT && view.context() == 0 => {
            if key.kind != KeyEventKind::Press {
                return Action::Continue;
            }
            Command::FolderFocus
        }
        KeyCode::Enter => Command::Resume,
        _ => return Action::Continue,
    };
    execute_command(command, browser, view)
}

fn execute_command(command: Command, browser: &mut Browser, view: &mut View) -> Action {
    if view.context() == 0 && view.folders_hidden && !available_when_folders_hidden(command) {
        return Action::Continue;
    }
    let previous_context = view.context();
    match command {
        Command::ToggleTerminalFullscreen => {
            if view.terminals.borrow().active.is_none()
                && view.context() == 1
                && selected_session(browser).is_some()
            {
                return execute_command(Command::ResumeFullscreen, browser, view);
            }
            let mut terminals = view.terminals.borrow_mut();
            if terminals.active.is_some() {
                terminals.fullscreen = !terminals.fullscreen;
                view.focus = 1;
                view.git_focus = false;
            }
        }
        Command::GitSelectWorktree => {
            if view.git_hidden || view.git_size_limited {
                return Action::Continue;
            }
            if let Some(annotation) = selected_git_session(browser, view) {
                let session = annotation.session;
                if let Some(cwd) = session.cwd {
                    if let Some(main) = view
                        .history
                        .repository()
                        .as_ref()
                        .and_then(|id| browser.repositories.get(id))
                        .and_then(|r| r.worktrees.first())
                    {
                        browser
                            .expanded
                            .extend(main.path.ancestors().map(std::path::PathBuf::from));
                    }
                    browser.query.clear();
                    browser.open_folder(cwd);
                    browser.selected = Some(session.key);
                    browser.reconcile(0);
                    view.git_focus = false;
                    view.focus = 1;
                    view.message = None;
                    return execute_command(Command::Resume, browser, view);
                }
                return Action::Continue;
            }
            if let Some(stash) = view
                .history
                .rows
                .get(view.history.selected)
                .and_then(|row| row.stash.as_ref())
            {
                view.message = Some(format!(
                    "Saved stash {} · based on {} · shown read-only",
                    stash.selectors.join(", "),
                    &stash.base[..stash.base.len().min(7)]
                ));
                return Action::Continue;
            }
            let Some(repository) = view.history.repository() else {
                return Action::Continue;
            };
            let Some(oid) = view
                .history
                .rows
                .get(view.history.selected)
                .and_then(|row| row.oid.clone())
            else {
                return Action::Continue;
            };
            match matching_worktrees(browser, &repository, &oid) {
                Ok(matches) if matches.is_empty() => {
                    view.message = Some("No worktree is checked out at this commit.".into())
                }
                Ok(matches) => {
                    let picker = WorktreePicker {
                        repository,
                        oid,
                        matches,
                        selected: 0,
                    };
                    if picker.matches.len() == 1 {
                        return select_git_worktree(browser, view, &picker);
                    } else {
                        view.message = None;
                        view.worktree_picker = Some(picker);
                    }
                }
                Err(message) => view.message = Some(message),
            }
        }
        Command::CreateWorktree => {
            if worktree_unavailable(browser, view).is_some() {
                return Action::Continue;
            }
            if let FolderFilter::Path(path) = &browser.folder {
                let commit = if view.git_focus {
                    view.history
                        .rows
                        .get(view.history.selected)
                        .and_then(|row| row.oid.clone())
                } else {
                    None
                };
                view.worktree_create =
                    Some(crate::worktree_create_ui::Modal::new(path.clone(), commit));
            }
        }

        Command::FolderToggle => {
            view.folders_hidden = !view.folders_hidden;
            if view.folders_hidden {
                view.focus = 1;
                view.git_focus = false;
            }
        }
        Command::GitToggle => {
            view.git_hidden = !view.git_hidden;
            if view.git_hidden {
                view.focus = 1;
                view.git_focus = false;
            }
        }
        Command::GitReturn => {
            view.git_focus = false;
            view.focus = 1;
        }
        Command::GitLeft => view.history.scroll_horizontal(false),
        Command::GitRight => view.history.scroll_horizontal(true),
        Command::GitFollow => {
            view.git_sessions.borrow_mut().follow();
            view.history.follow();
        }
        Command::GitUp
        | Command::GitDown
        | Command::GitPageUp
        | Command::GitPageDown
        | Command::GitHome
        | Command::GitEnd => {
            use crate::git_history::Movement;
            let mut presentation = view.git_sessions.borrow_mut();
            presentation.sync(browser, &view.history);
            presentation.navigate(
                match command {
                    Command::GitUp => Movement::Up,
                    Command::GitDown => Movement::Down,
                    Command::GitPageUp => Movement::PageUp,
                    Command::GitPageDown => Movement::PageDown,
                    Command::GitHome => Movement::Home,
                    _ => Movement::End,
                },
                &mut view.history,
            );
        }

        Command::Help => {
            open_help(browser, view);
        }
        Command::CommitInfo => {
            if let Some((repository, oid)) = selected_commit(browser, view) {
                view.commit_info = Some(crate::commit_info::Dialog::new(repository, oid));
            } else {
                view.message = Some("Select a visible Git commit first.".into());
            }
        }
        Command::Info => {
            if view.focus == 0 {
                if let Some(path) = selected_checkout(browser) {
                    view.error_dialog = Some(crate::error_dialog::ErrorDialog::information(
                        "Worktree information",
                        format!(
                            "{}\n\nUncommitted changes: {}\n\n* marks staged, unstaged, or untracked changes. Ignored files are excluded.",
                            path.display(),
                            worktree_status_description(browser, path)
                        ),
                    ));
                }
            } else if let Some(session) = selected_session(browser) {
                view.info = Some(InfoModal::new(browser, session));
            }
        }
        Command::OpenTerminals => view.terminals.borrow_mut().picker = true,
        Command::CloseTerminal => view
            .terminals
            .borrow_mut()
            .request_close(browser.selected.as_ref()),
        Command::Quit => return Action::Quit,
        Command::Relink => return Action::OpenRelink,
        Command::MoveFolder => return Action::OpenFolderMove,
        Command::Recover => {
            if view.recovery_available {
                return Action::RecoverRelink;
            }
        }
        Command::Search => {
            view.editing = true;
            view.search_control = 0;
            view.search_query.clear();
            view.search_selected = None;
        }
        Command::Refresh => return Action::Refresh,
        Command::Archives => browser.include_archived = !browser.include_archived,
        Command::PaneUp | Command::PaneDown | Command::PaneLeft | Command::PaneRight => {
            let horizontal = matches!(command, Command::PaneLeft | Command::PaneRight);
            let mode = view.layout_mode.get();
            if mode == LayoutMode::Single || horizontal == (mode == LayoutMode::Columns) {
                let last = if view.history.present() { 2 } else { 1 };
                let current = view.context();
                let next = if matches!(command, Command::PaneLeft | Command::PaneUp) {
                    current.saturating_sub(1)
                } else {
                    (current + 1).min(last)
                };
                if next != current {
                    view.git_focus = next == 2;
                    if next == 2 {
                        view.git_hidden = false;
                    } else {
                        view.focus = next;
                    }
                }
            }
        }
        Command::CyclePane | Command::CyclePaneBack => {
            let count = if view.history.present() { 3 } else { 2 };
            let current = if view.git_focus && count == 3 {
                2
            } else {
                view.focus
            };
            let step = if command == Command::CyclePaneBack {
                count - 1
            } else {
                1
            };
            let next = (current + step) % count;
            view.git_focus = next == 2;
            if view.git_focus {
                // Keep the entry pane for Escape; Tab follows the fixed cycle.
                view.git_hidden = false;
            } else {
                view.focus = next;
            }
            if command == Command::CyclePaneBack {
                view.message = None;
            }
        }
        Command::EnterFolder => return enter_folder(browser, view),
        Command::PreviousPane => {
            view.focus = 0;
            view.message = None;
        }
        Command::PreviousItem | Command::NextItem => {
            let delta = if command == Command::PreviousItem {
                -1
            } else {
                1
            };
            match view.focus {
                0 => {
                    let rows = browser.folder_rows();
                    let i = rows
                        .iter()
                        .position(|r| r.filter == browser.folder)
                        .unwrap_or(0);
                    if let Some(row) = rows.get(
                        i.saturating_add_signed(delta)
                            .min(rows.len().saturating_sub(1)),
                    ) {
                        browser.select_folder(row.filter.clone());
                    }
                }
                1 => browser.move_selection(delta),
                _ => {}
            }
        }
        Command::FolderFocus => browser.toggle_folder_focus(),
        Command::FolderLeft | Command::FolderRight => {
            let FolderFilter::Path(path) = browser.folder.clone() else {
                return Action::Continue;
            };
            let rows = browser.folder_rows();
            let Some(index) = rows.iter().position(|row| row.filter == browser.folder) else {
                return Action::Continue;
            };
            let depth = rows[index].depth;
            if command == Command::FolderRight {
                if !browser.expanded.insert(path) {
                    view.focus = 1;
                }
            } else if !browser.expanded.remove(&path) {
                if let Some(parent) = rows[..index]
                    .iter()
                    .rev()
                    .find(|row| row.depth < depth && matches!(row.filter, FolderFilter::Path(_)))
                {
                    browser.select_folder(parent.filter.clone());
                }
            }
        }
        Command::Create | Command::CreateFullscreen => {
            if let Some(reason) = unavailable(Command::Create, browser, view) {
                view.message = Some(reason);
                return Action::Continue;
            }
            view.focus = 1;
            view.source_fullscreen = command == Command::CreateFullscreen;
            let sources = creation_sources(browser);
            if sources.len() == 1 {
                return creation_action(sources[0], view.source_fullscreen);
            }
            view.source_picker = Some(0);
            view.source_control = 0;
        }
        Command::Resume | Command::ResumeFullscreen => {
            if selected_session(browser).is_some_and(|s| {
                crate::domain::classify_folder(s.cwd.as_deref())
                    == crate::domain::FolderStatus::Unlinked
            }) {
                return Action::OpenRelink;
            }
            return if command != Command::ResumeFullscreen {
                Action::ResumeSelected
            } else {
                Action::ResumeSelectedFullscreen
            };
        }
    }
    if previous_context != 0 && view.context() == 0 {
        view.folders_hidden = false;
    }
    browser.reconcile(0);
    Action::Continue
}

struct PendingResume {
    origin: FolderFilter,
    task: tokio::task::JoinHandle<Result<crate::domain::ResumeCommand, String>>,
    observed_cwd: Option<std::path::PathBuf>,
    fullscreen: bool,
}
impl Drop for PendingResume {
    fn drop(&mut self) {
        self.task.abort();
    }
}

fn save_folder(browser: &mut Browser, folder: Option<&std::path::Path>, view: &mut View) {
    if let (Some(file), Some(folder)) = (&browser.state_path, folder) {
        if let Err(error) = crate::state::save_context(
            file,
            folder,
            browser
                .launch_folder
                .as_deref()
                .map(|launch| (launch, browser.focus_root.as_deref())),
        ) {
            let warning = format!("Could not save browser state: {error}");
            view.message = Some(warning.clone());
            browser.state_warning = Some(warning);
        }
    }
}

pub async fn run(mut browser: Browser) -> Result<(), Box<dyn std::error::Error>> {
    if !io::stdin().is_terminal() || !io::stdout().is_terminal() {
        return Err("interactive browsing requires a terminal; use 'tracr list'".into());
    }
    let mut guard = TerminalGuard {
        active: false,
        mouse: false,
    };
    guard.activate()?;
    let mut input_reader = crate::terminal_input::Reader::default();
    let mut outer_focused = true;
    let mut terminal_copy: Option<tokio::task::JoinHandle<std::result::Result<(), String>>> = None;
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        restore();
        previous(info);
    }));
    let mut terminal = Terminal::new(CrosstermBackend::new(io::stdout()))?;
    let mut content_search = crate::search::Search::default();
    let mut discovery = Discovery::new();
    discovery.enable_live();
    browser.invalidate_git();
    discovery.refresh(&mut browser);
    let mut view = View {
        focus: 1,
        message: browser.state_warning.take(),
        ..View::default()
    };
    let signals = ShutdownSignals::new()?;
    let mut dev_reload = crate::dev_reload::DevReload::new()?;
    let mut reloading = false;
    let mut pending: Option<PendingResume> = None;
    let mut relink_task: Option<tokio::task::JoinHandle<RelinkResult>> = None;
    let mut folder_task: Option<tokio::task::JoinHandle<FolderMoveResult>> = None;
    let (relink_tx, mut relink_rx) = tokio::sync::mpsc::unbounded_channel::<String>();
    let (folder_tx, mut folder_rx) = tokio::sync::mpsc::unbounded_channel::<String>();
    let mut folder_relocated: Option<(std::path::PathBuf, Option<crate::domain::SessionKey>)> =
        None;
    let launch_dir = std::env::current_dir()?;
    let relocation_state = browser
        .state_path
        .as_ref()
        .and_then(|p| p.parent())
        .map(|p| p.to_path_buf());
    if let Some((count, folder)) = refresh_recovery(relocation_state.as_deref(), &mut view) {
        if folder {
            view.message = Some("Unfinished folder move. Press u to inspect/recover.".into());
        } else if count > 0 {
            view.message = Some(format!(
                "{count} unfinished relocation(s). Press u to inspect/recover."
            ));
        }
    }
    let mut relocated: Option<crate::relocation::Preview> = None;
    let mut last_folder = browser.initial_folder.clone();
    let mut git_discovery = crate::git_discovery::Controller::default();
    let mut status_monitor = crate::worktree_status::Monitor::default();
    let mut git_watch = crate::git_watch::Monitor::default();
    let mut history_folder = None;
    let mut candidates_dirty = true;
    let mut candidate_context = None;
    let mut idle = IdleGate::default();
    idle.wake();
    let mut modal_deadline = std::time::Instant::now();
    let mut folder_checks_dirty = true;
    let outcome: Result<(), Box<dyn std::error::Error>> = async {
        loop {
            if let Some(notice) = browser.focus_notice.take() { view.message = Some(notice); idle.wake(); }
            if terminal_copy.as_ref().is_some_and(|task|task.is_finished()) {
                view.message=Some(match terminal_copy.take().unwrap().await {Ok(Ok(()))=>"Terminal selection copied".into(),Ok(Err(error))=>error,Err(error)=>format!("Copy failed: {error}")});
                idle.wake();
            }
            if terminal_copy.is_none() {
                if let Some(text)=view.terminals.borrow_mut().copy_request.take() {terminal_copy=Some(tokio::spawn(crate::clipboard::copy(text)));}
            }
            let (terminal_changed, terminal_exits) = view.terminals.borrow_mut().poll();
            if terminal_changed { idle.wake(); }
            if let Some(error)=view.terminals.borrow_mut().input_error.take() {view.message=Some(error);}
            if !terminal_exits.is_empty() {
                view.message = Some(terminal_exits.iter().map(|notice| notice.message.as_str()).collect::<Vec<_>>().join("; "));
                if let Some(notice) = terminal_exits.iter().find(|notice| notice.failed && notice.was_active) {
                    view.error_dialog = Some(crate::error_dialog::ErrorDialog::new("Terminal exited", "The session remains available to resume.", notice.message.clone()));
                }
                browser.invalidate_git();
                discovery.refresh(&mut browser);
            }
            for _ in 0..100 {
                match discovery.rx.try_recv() {
                    Ok(update) => {
                        if browser.update_changes(&update) {
                            let paths_changed = matches!(&update, crate::browser::Update::Page { .. } | crate::browser::Update::Finished { .. });
                            candidates_dirty |= paths_changed; folder_checks_dirty |= paths_changed;
                            browser.apply(update); idle.wake();
                        }
                    }
                    Err(_) => break,
                }
            }
            let search_active = view.editing && browser.deep_search && !view.search_query.trim().is_empty();
            if content_search.tick(&mut browser, search_active) {
                if view.editing && view.search_selected.is_none() {
                    view.search_selected = browser.search_results(&view.search_query).first().map(|s| s.key.clone());
                }
                idle.wake();
            }
            if status_monitor.tick(&mut browser).await { idle.wake(); }
            let now = std::time::Instant::now();
            if let Some(change) = git_watch.poll(now, git_discovery.pending() || view.history.loading()) {
                git_watch.reconcile();
                if let Some(id) = git_watch.repository() {
                    git_discovery.refresh_repository(id, change == crate::git_watch::Change::Topology);
                    let retry_paths = browser.invalidate_git_repository(id, change == crate::git_watch::Change::Topology);
                    git_discovery.forget(&retry_paths);
                    view.history.request_refresh();
                    idle.wake();
                }
            }
            if view.git_watch_error != git_watch.error { view.git_watch_error = git_watch.error.clone(); idle.wake(); }
            let copy_ready = view.error_dialog.as_ref().is_some_and(|dialog| dialog.copy_task.as_ref().is_some_and(|task| task.is_finished())) || view.info.as_ref().is_some_and(|info| info.dialog.copy_task.as_ref().is_some_and(|task| task.is_finished()));
            let picker_ready = view.relink.as_ref().is_some_and(|modal| modal.picker.as_ref().is_some_and(|picker| picker.needs_tick()));
            let modal_timer = view.folder_move.is_some() && now >= modal_deadline;
            if modal_timer { modal_deadline = now + Duration::from_millis(100); }
            let resized = signals.resize.swap(false, std::sync::atomic::Ordering::Relaxed);
            let reload_requested = dev_reload.requested();
            let activity = view.commit_info.as_ref().is_some_and(|info| info.ready()) || reload_requested || view.worktree_create.as_ref().is_some_and(|modal| modal.ready()) || resized || candidates_dirty || git_discovery.needs_poll() || view.history.needs_poll() || copy_ready || picker_ready || modal_timer
                || pending.as_ref().is_some_and(|p| p.task.is_finished())
                || relink_task.as_ref().is_some_and(|task| task.is_finished()) || folder_task.as_ref().is_some_and(|task| task.is_finished())
                || !relink_rx.is_empty() || !folder_rx.is_empty()
                || signals.terminate.load(std::sync::atomic::Ordering::Relaxed) || signals.interrupt.load(std::sync::atomic::Ordering::Relaxed);
            if idle.ready(now, activity) {
            git_discovery.generation(browser.git_generation);
            let context_changed = candidate_context.as_ref().is_none_or(|(generation, folder, expanded, archived)| *generation != browser.git_generation || folder != &browser.folder || expanded != &browser.expanded || *archived != browser.include_archived);
            if candidates_dirty || context_changed {
                let mut candidates = Vec::new();
                if let FolderFilter::Path(path) = &browser.folder { candidates.push(path.clone()); }
                candidates.extend(browser.folder_rows().into_iter().filter_map(|r| match r.filter { FolderFilter::Path(p) => Some(p), _ => None }));
                candidates.extend(browser.all().into_iter().filter_map(|s| s.cwd));
                candidates.extend(browser.repositories.values().flat_map(|r| r.worktrees.iter().map(|w| w.path.clone())));
                git_discovery.set_candidates(candidates);
                candidate_context = Some((browser.git_generation, browser.folder.clone(), browser.expanded.clone(), browser.include_archived));
                candidates_dirty = false;
            }
            let updates = git_discovery.poll().await;
            candidates_dirty |= !updates.is_empty();
            git_discovery.apply_batch(&mut browser, updates);
            let context = match &browser.folder {
                FolderFilter::Path(path) => browser.ownership.get(path).and_then(|r| r.effective()),
                _ => None,
            };
            let pending_context = match &browser.folder {
                FolderFilter::Path(path) => browser.ownership.get(path).is_none_or(|record| record.outcome.is_none() && record.previous.is_none()),
                _ => false,
            };
            let target = match context {
                Some(crate::git_discovery::Outcome::Repository { repository, .. }) => Some(repository.common_dir.clone()),
                _ => None,
            };
            let checkout = match context { Some(crate::git_discovery::Outcome::Repository { root, .. }) => root.clone(), _ => None };
            let selected_folder = match &browser.folder { FolderFilter::Path(path) => Some(path.clone()), _ => None };
            let root_selected = selected_folder != history_folder && checkout.is_some() && selected_folder == checkout;
            if !pending_context {
                history_folder = selected_folder;
                view.history.checkout(checkout, root_selected).await;
            }
            let target = if pending_context { view.history.repository() } else { target };
            let size = terminal.size()?;
            let area = Rect::new(0, 0, size.width, size.height);
            view.layout_mode.set(LayoutMode::for_size(size.width, size.height));
            let (_, _, _, footer_height) = browser_footer(area, &browser, &view);
            let content_height = size.height.saturating_sub(footer_height);
            let panels = panel_layout(Rect::new(0, 0, size.width, content_height), size.height, &view, target.is_some());
            view.git_size_limited = panels.git.is_none_or(|area| area.height < 4 || area.width < 12);
            let graph_visible = panels.graph_visible;
            let page = usize::from(panels.git.map_or(0, |area| area.height.saturating_sub(3))).max(1);
            git_watch.context(target.clone(), browser.git_generation, graph_visible);
            view.history.tick(target, browser.git_generation, graph_visible, page).await;
            if let Some(repository) = view.history.repository().filter(|_| !view.history.rows.is_empty()) {
                browser.order_worktrees(repository, view.history.revision(), view.history.rows.iter().filter_map(|row| row.oid.as_deref()));
            }
            let displayed_repository = view.history.repository();
            view.history.set_worktrees(displayed_repository.as_ref().and_then(|id| browser.repositories.get(id)));
            view.history.set_status(displayed_repository.as_ref().and_then(|id| browser.repositories.get(id)), &browser.worktree_status);
            if !view.history.present() && view.git_focus { view.git_focus = false; view.focus = 1; }
            {
                let mut presentation = view.git_sessions.borrow_mut();
                presentation.sync(&browser, &view.history);
                if presentation.session().is_some() {
                    if let Some(anchor) = presentation.anchor() { view.history.selected = anchor; }
                }
            }
            // Finish the bounded operation and archive restoration before exiting.
            if (signals.terminate.load(std::sync::atomic::Ordering::Relaxed)
                || signals.interrupt.load(std::sync::atomic::Ordering::Relaxed)) && relink_task.is_none() && folder_task.is_none() && !view.worktree_create.as_ref().is_some_and(|modal| modal.executing) {
                break;
            }
            // Reload is distinct from termination: never interrupt interactive Codex.
            // Background terminals and mutations must also settle first.
            if reload_requested && view.terminals.borrow().is_empty() && relink_task.is_none() && folder_task.is_none()
                && !view.worktree_create.as_ref().is_some_and(|modal| modal.executing) {
                reloading = true;
                break;
            }
            let worktree_ready = view.worktree_create.as_ref().is_some_and(|modal| modal.ready());
            let worktree_result = if let Some(modal) = view.worktree_create.as_mut() { modal.tick().await } else { None };
            if worktree_ready {
                // Discard queued input after worktree checks or creation complete.
                while input_reader.poll(Duration::ZERO)? { let _ = input_reader.read()?; }
            }
            if let Some(result) = worktree_result {
                browser.invalidate_git();
                candidates_dirty = true;
                match result {
                    Ok(path) => {
                        browser.open_folder(path);
                        view.worktree_create = None;
                        view.git_focus = false;
                        view.focus = 0;
                        view.folders_hidden = false;
                        view.message = Some("Worktree created. Select Sessions to start a new session.".into());
                        discovery.refresh(&mut browser);
                    }
                    Err(error) => view.error_dialog = Some(crate::error_dialog::ErrorDialog::new("Couldn't create worktree", "Inspect creation state before retrying", error)),
                }
            }
            while let Ok(message)=relink_rx.try_recv() {
                if let Some(dialog)=view.terminal_recovery.as_mut(){dialog.progress=message.clone();}
                if let Some(modal)=view.relink.as_mut(){modal.error=Some(message);}
            }
            while let Ok(message)=folder_rx.try_recv() {
                if let Some(modal)=view.folder_move.as_mut(){modal.message=Some(message);}
            }
            if let Some(modal)=view.folder_move.as_mut(){modal.tick();}
            if folder_task.as_ref().is_some_and(|t|t.is_finished()) {
                let result=folder_task.take().expect("finished folder move").await;
                while input_reader.poll(Duration::ZERO)?{let _=input_reader.read()?;}
                match result {
                    Ok(FolderMoveResult::Preview(Ok(plan)))=>{
                        if let Some(modal)=view.folder_move.as_mut(){modal.busy=false;modal.message=None;modal.plan=Some(plan);}
                    },
                    Ok(FolderMoveResult::Applied(crate::folder_move::Outcome::Complete(plan)))=>{
                        folder_relocated=Some((plan.target.clone(),browser.selected.clone()));
                        browser.open_folder(plan.target);view.folder_move=None;view.focus=0;
                        view.message=Some("Folder and sessions moved and verified.".into());browser.invalidate_git(); discovery.refresh(&mut browser);
                    },
                    other=>{
                        let (message,recovery)=match other {
                            Ok(FolderMoveResult::Preview(Err(e)))=>(e,false),
                            Ok(FolderMoveResult::Applied(crate::folder_move::Outcome::Failed{message,recovery}))=>(message,recovery),
                            Err(e)=>(format!("Folder move stopped: {e}"),true),
                            _=>unreachable!(),
                        };
                        show_folder_failure(&mut view,message,recovery);
                        browser.invalidate_git(); discovery.refresh(&mut browser);
                    }
                }
                refresh_recovery(relocation_state.as_deref(), &mut view);
            }
            if let Some(info)=view.commit_info.as_mut(){info.tick().await;}
            if let Some(dialog)=view.error_dialog.as_mut(){dialog.tick().await;}
            if let Some(info)=view.info.as_mut(){info.refresh(&browser);info.dialog.tick().await;}
            if relink_task.as_ref().is_some_and(|t|t.is_finished()) {
                let task=relink_task.take().expect("finished relocation");
                while input_reader.poll(Duration::ZERO)?{let _=input_reader.read()?;}
                match task.await {
                    Ok(RelinkResult::Terminals(result)) => {
                        let failure = match result {
                            Ok(commands) => crate::providers::codex_terminals::Blocker::Commands(commands).failure(),
                            Err(error) => error,
                        };
                        show_relink_failure(&mut view,failure,false);
                    }
                    Ok(RelinkResult::Preview(result))=>match result {
                        Ok(p)=>if let Some(modal)=view.relink.as_mut(){modal.busy=false;modal.input=p.target.display().to_string();modal.preview=Some(p);view.relink_back=false;view.relink_scroll=0;},
                        Err(e)=>show_relink_failure(&mut view,e,false),
                    },
                    Ok(RelinkResult::Applied(crate::relocation::Outcome::Verified(p)))=>{
                        view.terminal_recovery=None;view.relink=None;view.focus=1;if p.archived {browser.include_archived=true;}browser.open_folder(p.target.clone());
                        browser.selected=Some(p.key.clone());relocated=Some(p);
                        view.message=Some("Session relinked and verified.".into());browser.invalidate_git(); discovery.refresh(&mut browser);
                    },
                    Ok(RelinkResult::Applied(crate::relocation::Outcome::Rejected(e)))=>{
                        show_relink_failure(&mut view,e,false);browser.invalidate_git(); discovery.refresh(&mut browser);
                    },
                    Ok(RelinkResult::Applied(crate::relocation::Outcome::Unresolved(e)))=>{
                        show_relink_failure(&mut view,e,true);browser.invalidate_git(); discovery.refresh(&mut browser);
                    },
                    Err(e)=>{show_relink_failure(&mut view,format!("Relocation stopped: {e}").into(),true);}
                }
                refresh_recovery(relocation_state.as_deref(), &mut view);
            }
            if let FolderFilter::Path(path) = &browser.folder {
                last_folder = Some(path.clone());
            }
            if !browser.loading() {
                if let Some((path,selected))=folder_relocated.take(){
                    browser.open_folder(path);
                    if selected.as_ref().is_some_and(|k|browser.visible().iter().any(|s|&s.key==k)){browser.selected=selected;}
                }
                if let Some(p)=relocated.take(){browser.open_folder(p.target);browser.selected=Some(p.key);}

            }
            if let Some(modal)=view.relink.as_mut(){
                if !modal.busy && modal.preview.is_none(){if let Some(picker)=modal.picker.as_mut(){picker.tick(&mut modal.input);}}
            }
            if let Some(modal)=view.folder_move.as_mut(){ modal.monitor(&browser); }
            if folder_checks_dirty || browser.loading() { discovery.check_folders(&browser); folder_checks_dirty = false; }
            if pending.is_none() && relink_task.is_none() {
                discovery.detail(&browser);
            }
            if pending.as_ref().is_some_and(|p| p.task.is_finished()) {
                let mut ready = pending.take().expect("finished preparation");
                view.preparing = false;
                // An Enter queued during preparation must not approve a changed folder.
                while input_reader.poll(Duration::ZERO)? {
                    let _ = input_reader.read()?;
                }
                match (&mut ready.task).await {
                    Ok(Ok(command)) => {
                        browser.details.insert(
                            command.session.key.clone(),
                            Ok(Some(command.session.clone())),
                        );
                        if ready.observed_cwd.as_ref() != Some(&command.cwd) {
                            view.focus = 1;
                            let mut info = InfoModal::new(&browser, command.session.clone());
                            info.dialog.title = "Session folder changed".into();
                            info.warning = Some(format!("Previously displayed folder: {}\nCurrent resume folder: {}\n\nResume was stopped. Close this dialog, then press Enter in Sessions to request resume in the updated folder. The destination will be checked again.", ready.observed_cwd.as_ref().map(|p|p.display().to_string()).unwrap_or_else(||"Unknown".into()), command.cwd.display()));
                            info.refresh(&browser);
                            view.info = Some(info);
                            view.message = Some(format!(
                                "Resume folder changed to {}. Review it, then press Enter again.",
                                command.cwd.display()
                            ));
                        } else {
                            save_folder(&mut browser, last_folder.as_deref(), &mut view);
                            if ready.fullscreen {
                                let size = terminal.size()?;
                                view.terminals.borrow_mut().area = Rect::new(0, 0, size.width, size.height);
                            }
                            let result = view.terminals.borrow_mut().launch(&command.launch, Some(command.session.key.clone()), command.session.title.clone());
                            if result.is_ok() && browser.folder == ready.origin {
                                browser.open_folder(command.cwd.clone());
                                view.terminals.borrow_mut().sync_folder(Some(&command.cwd));
                                view.terminals.borrow_mut().fullscreen = ready.fullscreen;
                                view.focus = 1;
                                view.git_focus = false;
                            }
                            if let Err(error) = result { view.message = Some(format!("Could not resume Codex: {error}")); }
                        }
                    }
                    Ok(Err(error)) => view.message = Some(error),
                    Err(error) => {
                        view.message = Some(format!("Resume preparation failed: {error}"))
                    }
                }
            }
            view.terminals.borrow_mut().sync_folder(match &browser.folder {
                FolderFilter::Path(path) => Some(path.as_path()),
                _ => None,
            });
            let size = terminal.size()?;
            view.help_page_size =
                usize::from(size.height.saturating_sub(2).min(22).saturating_sub(7))
                    / if size.width < 58 { 2 } else { 1 };
            terminal.draw(|frame| { draw(frame, &browser, &view); view.terminals.borrow().draw_overlay(frame, &browser); })?;
            guard.mouse_capture(view.terminals.borrow().active.is_some())?;
            idle.deadline = next_age_update(&browser, &view, size.height);
            }
            let busy = terminal_copy.is_some() || !view.terminals.borrow().is_empty() || browser.loading() || git_discovery.pending() || view.history.loading() || pending.is_some() || relink_task.is_some() || folder_task.is_some()
                || view.relink.is_some() || view.folder_move.is_some() || view.worktree_create.is_some()
                || view.error_dialog.as_ref().is_some_and(|dialog| dialog.copy_task.is_some())
                || view.info.as_ref().is_some_and(|info| info.dialog.copy_task.is_some());
            let mut wait = Duration::from_millis(if busy { 40 } else { 250 });
            if let Some(deadline) = idle.deadline { wait = wait.min(deadline.saturating_duration_since(std::time::Instant::now())); }
            if !git_discovery.pending() && !view.history.loading() {
                if let Some(deadline) = git_watch.deadline() { wait = wait.min(deadline.saturating_duration_since(std::time::Instant::now())); }
            }
            guard.enhanced(input_reader.enhanced())?;
            if let Err(error) = view.terminals.borrow_mut().sync_focus(outer_focused && view.terminal_focused()) { view.message = Some(error.to_string()); }
            if view.terminals.borrow().input_backlogged() {tokio::time::sleep(wait).await;}
            else if input_reader.poll(wait)? {
                idle.wake();
                let packet=input_reader.read()?;
                let incoming=packet.event;
                if matches!(incoming, Event::FocusGained | Event::FocusLost) {
                    outer_focused = matches!(incoming, Event::FocusGained);
                    continue;
                }
                if let Event::Mouse(mouse) = &incoming {
                    if !view.modal_open() {
                        if matches!(mouse.kind,crossterm::event::MouseEventKind::Down(_)) && view.terminals.borrow().visible && view.terminals.borrow().area.contains((mouse.column,mouse.row).into()) {view.focus=1;view.git_focus=false;}
                        if let Err(error) = view.terminals.borrow_mut().mouse(*mouse) {
                            view.message = Some(error.to_string());
                        }
                    }
                    continue;
                }
                let previous_terminal = view.terminals.borrow().active;
                let terminal_overlay = view.terminals.borrow().has_overlay();
                let terminal_input = if terminal_overlay || view.terminal_focused() {
                    view.terminals.borrow_mut().input_rich(&incoming, packet.key.as_ref())
                } else { Ok(crate::inline_terminal::Input::Pass) };
                match terminal_input {
                    Ok(crate::inline_terminal::Input::Quit) => break,
                    Ok(crate::inline_terminal::Input::Consumed) => {
                        let active = view.terminals.borrow().active;
                        if previous_terminal != active || (terminal_overlay && active.is_some() && !view.terminals.borrow().has_overlay() && matches!(&incoming, Event::Key(key) if key.kind == KeyEventKind::Press && (key.code == KeyCode::Enter || key.code == KeyCode::Char('f') && key.modifiers == KeyModifiers::ALT))) {
                            let folder = view.terminals.borrow().active_folder();
                            if let Some(folder) = folder {
                                browser.open_folder(folder.clone());
                                let mut terminals = view.terminals.borrow_mut();
                                let fullscreen = terminals.fullscreen;
                                terminals.sync_folder(Some(&folder));
                                terminals.fullscreen = fullscreen;
                            }
                            view.focus = 1; view.git_focus = false;
                        }
                        continue;
                    },
                    Ok(crate::inline_terminal::Input::Pass) => (),
                    Err(error) => { view.message = Some(error.to_string()); continue; }
                }
                if let Event::Paste(text)=&incoming {
                    if let Some(modal)=view.worktree_create.as_mut(){modal.paste(text);}
                    if let Some(modal)=view.folder_move.as_mut(){modal.paste(text);}
                    if let Some(modal)=view.relink.as_mut(){if !modal.busy && modal.preview.is_none(){if let Some(picker)=modal.picker.as_mut(){picker.paste(&mut modal.input,text);}}}
                }
                if let Event::Key(key) = incoming {
                    let was_help = view.help;
                    let action = handle_key(&mut browser, &mut view, key);
                    if was_help && !view.help && key.code == KeyCode::Enter {
                        // An activation must not also accept a newly opened modal or
                        // resume the underlying session through queued Enter repeats.
                        while input_reader.poll(Duration::ZERO)? {
                            let _ = input_reader.read()?;
                        }
                    }
                    match action {
                        Action::OpenFolderMove=>{
                            if let Some(reason)=crate::folder_move_ui::unavailable(&browser){view.message=Some(reason);continue;}
                            if let FolderFilter::Path(path)=&browser.folder {
                                view.folder_move=Some(crate::folder_move_ui::Modal::new(path.clone(),launch_dir.clone()));
                            }
                        },
                        Action::CloseFolderMove=>{
                            if let Some(modal)=view.folder_move.as_mut(){modal.stop_release().await;}
                            if let Some(task)=folder_task.take(){task.abort();}
                            view.folder_move=None;
                        },
                        Action::PreviewFolderMove=>{
                            let Some(modal)=view.folder_move.as_mut() else {continue};
                            let Some(state)=relocation_state.clone() else {continue};
                            modal.stop_release().await;
                            let target=match modal.target() {
                                Ok(p)=>p,Err(e)=>{modal.message=Some(e);continue;}
                            };
                            let sources=crate::folder_move_ui::sources(&browser);
                            let old=modal.old.clone();modal.busy=true;
                            modal.message=Some("Checking all configured Codex sources…".into());
                            folder_task=Some(tokio::spawn(async move{
                                FolderMoveResult::Preview(crate::folder_move::preview(sources,old,target,&state).await)
                            }));
                        },
                        Action::ApplyFolderMove=>{
                            let Some(modal)=view.folder_move.as_mut() else {continue};
                            modal.stop_release().await;
                            let Some(plan)=modal.plan.clone() else {continue};
                            let Some(state)=relocation_state.clone() else {continue};
                            let sources=crate::folder_move_ui::sources(&browser);
                            let recovery=modal.recovery;modal.busy=true;modal.executing=true;
                            let tx=folder_tx.clone();
                            folder_task=Some(tokio::spawn(async move{
                                FolderMoveResult::Applied(crate::folder_move::execute(plan,state,sources,recovery,tx).await)
                            }));
                        },
                        Action::CloseInformation => {
                            while input_reader.poll(Duration::ZERO)? { let _ = input_reader.read()?; }
                        },
                        Action::CopyInformation => {
                            if let Some(info)=view.info.as_mut(){ info.dialog.start_copy(); }
                        },
                        Action::CopyErrorReport => {
                            if let Some(dialog)=view.error_dialog.as_mut(){dialog.start_copy();}
                        }
                        Action::CancelRelink => {
                            if let Some(task)=relink_task.take(){task.abort();}
                            view.relink=None;
                        }
                        Action::OpenRelink => {
                            if let Some(reason)=relink_unavailable(&browser){view.message=Some(reason);continue;}
                            if let Some(session)=selected_session(&browser){let input=session.cwd.as_ref().map(|p|p.display().to_string()).unwrap_or_default();let picker=crate::path_picker::Picker::for_relink(launch_dir.clone(),session.cwd.clone());view.relink=Some(RelinkModal{session,input,picker:Some(picker),preview:None,recovery:false,busy:false,executing:false,error:None});}
                        }
                        Action::RecoverRelink => {
                            refresh_recovery(relocation_state.as_deref(), &mut view);
                            if !view.recovery_available { continue; }
                            let Some(state)=&relocation_state else{view.message=Some("No persistent state directory.".into());continue};
                            let folder=match crate::folder_move::pending(state){Ok(j)=>j,Err(e)=>{view.message=Some(e);continue;}};
                            let count=match crate::relocation::pending(state){Ok(j)=>j.len(),Err(e)=>{view.message=Some(e);continue;}};
                            let total=count+usize::from(folder.is_some());
                            let index=view.recovery_index%total.max(1);
                            view.recovery_index=(index+1)%total.max(1);
                            view.folder_move=None;view.relink=None;
                            if index==count {
                                if let Some(j)=folder{view.folder_move=Some(crate::folder_move_ui::Modal::recover(j));continue;}
                            }
                            match crate::relocation::pending(state){
                                Ok(journals)=>if let Some(j)=journals.get(index){
                                    let p=j.preview.clone();
                                    let session=crate::domain::SessionSummary{key:p.key.clone(),title:"Unfinished relocation".into(),cwd:Some(p.old.clone()),created_at:p.created_at,updated_at:0,archived:p.archived,can_accept_direct_input:None,source_kind:None,git:None};
                                    view.relink=Some(RelinkModal{session,input:p.target.display().to_string(),picker:None,preview:Some(p),recovery:true,busy:false,executing:false,error:Some(format!("Recorded phase: {}. Enter attempts guarded completion. u cycles through {} unfinished operation(s).",j.phase,journals.len()))});
                                }else{view.message=Some("No unfinished relocations.".into());},
                                Err(e)=>view.message=Some(e),
                            }
                        }
                        Action::RefreshTerminals | Action::StopTerminals => {
                            let Some(modal)=view.relink.as_mut() else {continue};
                            let Some(p)=modal.preview.clone() else {continue};
                            if modal.recovery {continue;}
                            let Some(dialog)=view.terminal_recovery.as_mut() else {continue};
                            let reviewed=dialog.reviewed();
                            let Some((binary,source))=browser.specs.iter().find_map(|s|match s{crate::browser::SourceSpec::Codex{binary,source} if source.id==p.key.source_id=>Some((binary.clone(),source.clone())),_=>None}) else {continue};
                            let Some(state)=relocation_state.clone() else {continue};
                            if action==Action::StopTerminals && reviewed.is_none() {continue;}
                            dialog.busy=true;modal.busy=true;modal.executing=true;
                            let tx=relink_tx.clone();
                            relink_task=Some(tokio::spawn(async move {
                                if action==Action::RefreshTerminals {
                                    RelinkResult::Terminals(crate::providers::codex_terminals::inspect(&binary,&source,&p).await)
                                } else {
                                    match crate::providers::codex_terminals::stop_reviewed(&binary,&source,&p,&reviewed.unwrap()).await {
                                        Ok(()) => RelinkResult::Applied(crate::relocation::execute_with_progress(&binary,&source,&state,p,false,Some(tx)).await),
                                        Err(error) => RelinkResult::Terminals(Err(error)),
                                    }
                                }
                            }));
                        }
                        Action::PreviewRelink | Action::ApplyRelink => {
                            let Some(modal)=view.relink.as_mut() else{continue};
                            let Some((binary,source))=browser.specs.iter().find_map(|s|match s{crate::browser::SourceSpec::Codex{binary,source} if source.id==modal.session.key.source_id=>Some((binary.clone(),source.clone())),_=>None}) else {modal.error=Some("Session source is no longer configured.".into());continue};
                            if action==Action::PreviewRelink {
                                let input=modal.input.clone();let launch=launch_dir.clone();
                                let worktree=modal.picker.as_ref().and_then(|p|p.worktree_validation(&input));
                                let session=modal.session.clone();modal.busy=true;modal.error=None;
                                relink_task=Some(tokio::spawn(async move {
                                    if let Some((origin,target))=worktree { if let Err(error)=crate::worktrees::validate(&origin,&target).await { return RelinkResult::Preview(Err(error.into())); } }
                                    let destination=tokio::task::spawn_blocking(move||crate::relocation::destination(&input,&launch)).await;
                                    let result=match destination {
                                        Ok(Ok(target))=>crate::relocation::preview(&binary,&source,&session,target).await,
                                        Ok(Err(error))=>Err(error.into()),Err(error)=>Err(error.to_string().into()),
                                    };
                                    RelinkResult::Preview(result)
                                }));
                            } else {
                                let Some(p)=modal.preview.clone() else{continue};
                                let Some(state)=relocation_state.clone() else{continue};
                                let recovery=modal.recovery;modal.busy=true;modal.executing=true;modal.error=None;
                                let tx=relink_tx.clone();
                                relink_task=Some(tokio::spawn(async move {RelinkResult::Applied(crate::relocation::execute_with_progress(&binary,&source,&state,p,recovery,Some(tx)).await)}));
                            }
                        }
                        Action::Quit => { if view.terminals.borrow_mut().request_quit() { break; } },
                        Action::Refresh => {
                            view.message = None;
                            refresh_recovery(relocation_state.as_deref(), &mut view);
                            browser.invalidate_git(); discovery.refresh(&mut browser);
                        }
                        Action::CancelResume => {
                            pending.take();
                            view.preparing = false;
                            view.message = Some("Resume cancelled.".into());
                            // Cancellation may interrupt an RPC frame. Reconnect rather than reuse it.
                            browser.invalidate_git(); discovery.refresh(&mut browser);
                        }
                        Action::CreateNew(index) | Action::CreateNewFullscreen(index) => {
                            let fullscreen = matches!(action, Action::CreateNewFullscreen(_));
                            if !can_create(&browser) {
                                continue;
                            }
                            let FolderFilter::Path(path) = browser.folder.clone() else {
                                continue;
                            };
                            let spec = &browser.specs[index];
                            let command = match spec.prepare_new(&path) {
                                Ok(command) => command,
                                Err(error) => {
                                    view.message = Some(error);
                                    continue;
                                }
                            };
                            let title = format!("New session · {}", path.display());
                            save_folder(&mut browser, last_folder.as_deref(), &mut view);
                            let result = view.terminals.borrow_mut().launch(&command, None, title);
                            view.focus = 1;
                            view.git_focus = false;
                            match result {
                                Ok(()) => view.terminals.borrow_mut().fullscreen = fullscreen,
                                Err(error) => view.message = Some(format!("Could not start session: {error}")),
                            }
                        }
                        Action::ResumeSelected | Action::ResumeSelectedFullscreen => {
                            let fullscreen = action == Action::ResumeSelectedFullscreen;
                            if browser.selected.as_ref().is_some_and(|key| view.terminals.borrow_mut().attach(key)) {
                                if let Some(folder) = view.terminals.borrow().active_folder() { browser.open_folder(folder); }
                                let mut terminals = view.terminals.borrow_mut();
                                let folder = terminals.active_folder();
                                terminals.sync_folder(folder.as_deref());
                                terminals.fullscreen = fullscreen;
                                view.focus = 1; view.git_focus = false; continue;
                            }
                            if let Some(session) = browser
                                .visible()
                                .into_iter()
                                .find(|s| Some(&s.key) == browser.selected.as_ref())
                            {
                                if let Some(reason) = crate::domain::resume_blocker(&session) {
                                    view.message = Some(reason.into());
                                    continue;
                                }
                                match browser.resume_provider(&session.key) {
                                    Ok(provider) => {
                                        let observed_cwd = browser
                                            .details
                                            .get(&session.key)
                                            .and_then(|r| r.as_ref().ok())
                                            .and_then(|s| s.as_ref())
                                            .map(|s| s.cwd.clone())
                                            .unwrap_or_else(|| session.cwd.clone());
                                        view.preparing = true;
                                        view.message = None;
                                        pending = Some(PendingResume {
                                            origin: browser.folder.clone(),
                                            fullscreen,
                                            observed_cwd,
                                            task: tokio::spawn(async move {
                                                provider.prepare_resume(&session).await
                                            }),
                                        });
                                    }
                                    Err(error) => view.message = Some(error),
                                }
                            }
                        }
                        Action::Continue => {}
                    }
                }
            }
            // Let the runtime service I/O and timers between blocking terminal polls.
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
        Ok(())
    }
    .await;
    view.terminals.get_mut().shutdown().await;
    git_watch.stop();
    view.history.shutdown().await;
    status_monitor.shutdown().await;
    git_discovery.shutdown().await;
    view.commit_info = None;
    crate::git::drain_reads().await;
    drop(pending);
    discovery.close(&mut browser).await;
    save_folder(&mut browser, last_folder.as_deref(), &mut view);
    guard.suspend()?;
    if let Some(warning) = browser.state_warning.as_ref() {
        eprintln!("tracr: {}", safe_text(warning));
    }
    if reloading && outcome.is_ok() {
        dev_reload.acknowledge()?;
    }
    outcome
}

#[cfg(test)]
mod cleanup_tests {
    use super::*;

    fn press(browser: &mut Browser, view: &mut View, code: KeyCode) {
        handle_key(browser, view, KeyEvent::new(code, KeyModifiers::NONE));
    }

    fn entry_fixture(count: usize, sources: usize) -> Browser {
        use crate::browser::{SourceSpec, Update};
        let mut browser = Browser::new(
            (0..sources)
                .map(|i| SourceSpec::Codex {
                    binary: "codex".into(),
                    source: crate::config::SourceConfig {
                        id: crate::domain::SourceId(format!("source-{i}")),
                        home: "/tmp".into(),
                    },
                })
                .collect(),
        );
        browser.begin_refresh();
        for source in 0..sources {
            let sessions = if source == 0 {
                (0..count).map(|i| serde_json::from_value(serde_json::json!({
                    "key": {"provider":"codex","source_id":"source-0","native_id":format!("session-{i}")},
                    "title":format!("Session {i}"), "cwd":"/tmp",
                    "created_at":1,"updated_at":2,"archived":false,"source_kind":null,"git":null
                })).unwrap()).collect()
            } else {
                vec![]
            };
            browser.apply(Update::Page {
                generation: 1,
                source,
                sessions,
                warnings: vec![],
            });
            browser.apply(Update::Finished {
                generation: 1,
                source,
                error: None,
            });
        }
        browser.open_folder("/tmp".into());
        browser
    }

    fn enter(browser: &mut Browser, view: &mut View) -> Action {
        handle_key(
            browser,
            view,
            KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
        )
    }

    #[test]
    fn folder_new_session_shortcut_and_menu_ignore_session_count() {
        for count in 0..=2 {
            let mut browser = entry_fixture(count, 1);
            let mut view = View::default();
            let entry = menu_entries(&browser, &view)
                .into_iter()
                .find(|entry| entry.command == Command::CreateFullscreen)
                .unwrap();
            assert_eq!(entry.shortcut, "n");
            assert_eq!(entry.label, "Create new session");
            assert!(
                footer_shortcuts(240, &browser, &view)
                    .1
                    .contains("n New session")
            );
            assert_eq!(
                handle_key(
                    &mut browser,
                    &mut view,
                    KeyEvent::new(KeyCode::Char('n'), KeyModifiers::NONE)
                ),
                Action::CreateNewFullscreen(0)
            );
            assert_eq!(view.focus, 1);
            assert_eq!(
                handle_key(
                    &mut browser,
                    &mut view,
                    KeyEvent::new(KeyCode::Char('n'), KeyModifiers::NONE)
                ),
                Action::CreateNew(0)
            );

            view.focus = 0;
            open_help(&browser, &mut view);
            view.help_selected = menu_entries(&browser, &view)
                .iter()
                .position(|entry| entry.command == Command::CreateFullscreen)
                .unwrap();
            assert_eq!(
                enter(&mut browser, &mut view),
                Action::CreateNewFullscreen(0)
            );
        }
    }

    #[test]
    fn folder_new_session_selects_source_and_cancels() {
        let mut browser = entry_fixture(2, 2);
        let mut view = View::default();
        press(&mut browser, &mut view, KeyCode::Char('n'));
        assert_eq!(view.source_picker, Some(0));
        press(&mut browser, &mut view, KeyCode::Down);
        assert_eq!(
            enter(&mut browser, &mut view),
            Action::CreateNewFullscreen(1)
        );

        view.focus = 0;
        press(&mut browser, &mut view, KeyCode::Char('n'));
        assert_eq!(
            handle_key(
                &mut browser,
                &mut view,
                KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)
            ),
            Action::Continue
        );
        assert!(view.source_picker.is_none());
    }

    #[test]
    fn folder_new_session_respects_unavailable_states() {
        for mode in 0..3 {
            let mut browser = entry_fixture(1, 1);
            match mode {
                0 => browser.sources[0].loading = true,
                1 => browser.specs.clear(),
                _ => browser.folder = FolderFilter::Unknown,
            }
            let mut view = View::default();
            assert!(unavailable(Command::CreateFullscreen, &browser, &view).is_some());
            assert!(
                menu_entries(&browser, &view)
                    .iter()
                    .any(|entry| entry.command == Command::CreateFullscreen)
            );
            assert_eq!(
                handle_key(
                    &mut browser,
                    &mut view,
                    KeyEvent::new(KeyCode::Char('n'), KeyModifiers::NONE)
                ),
                Action::Continue
            );
            assert!(view.message.is_some());
            assert!(view.source_picker.is_none());
        }
    }

    #[test]
    fn folder_entry_dispatches_by_count_and_clears_search() {
        for count in 0..=2 {
            let mut browser = entry_fixture(count, 1);
            browser.query = "does not match".into();
            let mut view = View::default();
            let action = enter(&mut browser, &mut view);
            assert_eq!(
                action,
                match count {
                    0 => Action::CreateNew(0),
                    1 => Action::ResumeSelected,
                    _ => Action::Continue,
                }
            );
            assert_eq!(view.focus, 1);
            assert!(browser.query.is_empty());
            if count == 1 {
                assert_eq!(browser.selected, Some(browser.visible()[0].key.clone()));
            }
        }
    }

    #[test]
    fn folder_entry_waits_for_complete_inventory() {
        for count in 0..=1 {
            for mode in 0..3 {
                let mut browser = entry_fixture(count, 1);
                match mode {
                    0 => browser.sources[0].loading = true,
                    1 => browser.sources[0].error = Some("offline".into()),
                    _ => browser.sources[0].warnings.push("incomplete".into()),
                }
                let mut view = View::default();
                assert_eq!(enter(&mut browser, &mut view), Action::Continue);
                assert_eq!(view.focus, 1);
                assert!(view.source_picker.is_none());
            }
        }
    }

    #[test]
    fn folder_entry_preserves_creation_intent_through_source_picker() {
        let mut browser = entry_fixture(0, 2);
        let mut view = View::default();
        assert_eq!(enter(&mut browser, &mut view), Action::Continue);
        assert_eq!(view.source_picker, Some(0));
        press(&mut browser, &mut view, KeyCode::Down);
        assert_eq!(enter(&mut browser, &mut view), Action::CreateNew(1));

        view.focus = 0;
        enter(&mut browser, &mut view);
        press(&mut browser, &mut view, KeyCode::Esc);
        assert!(view.source_picker.is_none());
        assert_eq!(
            execute_command(Command::Create, &mut browser, &mut view),
            Action::Continue
        );
        assert_eq!(enter(&mut browser, &mut view), Action::CreateNew(0));
        let mut browser = entry_fixture(0, 1);
        assert_eq!(
            execute_command(Command::Create, &mut browser, &mut view),
            Action::CreateNew(0)
        );
    }

    #[test]
    fn folder_entry_respects_archives_aggregate_rows_and_repeat() {
        let mut browser = entry_fixture(1, 1);
        let mut session = browser.all().remove(0);
        session.archived = true;
        browser.begin_refresh();
        browser.apply(crate::browser::Update::Page {
            generation: 2,
            source: 0,
            sessions: vec![session],
            warnings: vec![],
        });
        browser.apply(crate::browser::Update::Finished {
            generation: 2,
            source: 0,
            error: None,
        });
        let mut view = View::default();
        assert_eq!(enter(&mut browser, &mut view), Action::CreateNew(0));
        browser.include_archived = true;
        view.focus = 0;
        assert_eq!(enter(&mut browser, &mut view), Action::ResumeSelected);
        browser.folder = FolderFilter::All;
        view.focus = 0;
        assert_eq!(enter(&mut browser, &mut view), Action::Continue);
        browser.open_folder("/tmp".into());
        view.focus = 0;
        assert_eq!(
            handle_key(
                &mut browser,
                &mut view,
                KeyEvent::new_with_kind(KeyCode::Enter, KeyModifiers::NONE, KeyEventKind::Repeat)
            ),
            Action::Continue
        );
        assert_eq!(view.focus, 0);
    }

    #[tokio::test]
    async fn git_worktree_entry_propagates_session_actions() {
        for picker in [false, true] {
            for count in 0..=2 {
                let (navigation, mut view) = navigation_fixture(if picker {
                    &["selected-oid", "selected-oid"]
                } else {
                    &["selected-oid"]
                })
                .await;
                let mut browser = entry_fixture(count, 1);
                for mut repository in navigation.repositories.into_values() {
                    for worktree in &mut repository.worktrees {
                        worktree.path = "/tmp".into();
                    }
                    browser.install_repository(repository);
                }
                let mut action = enter(&mut browser, &mut view);
                if picker {
                    assert_eq!(action, Action::Continue);
                    assert!(view.worktree_picker.is_some());
                    action = enter(&mut browser, &mut view);
                }
                assert_eq!(
                    action,
                    match count {
                        0 => Action::CreateNew(0),
                        1 => Action::ResumeSelected,
                        _ => Action::Continue,
                    }
                );
                assert_eq!(view.context(), 1);
            }
        }
    }

    #[tokio::test]
    async fn git_session_rows_render_and_enter_relinks_missing_folder() {
        use crate::git_sessions::tests::{add_session, fixture};
        let (mut browser, history) = fixture().await;
        let key = add_session(
            &mut browser,
            "Implement login 界",
            "/worktree/src",
            "/worktree",
            "/repo/.git",
            crate::domain::RuntimeStatus::Waiting {
                approval: false,
                input: true,
            },
            1,
        );
        let mut view = View {
            history,
            git_focus: true,
            ..View::default()
        };
        browser.query = "blocking query".into();
        browser.expanded.clear();
        press(&mut browser, &mut view, KeyCode::Down);
        assert!(selected_git_session(&browser, &view).is_some());
        assert!(
            !menu_entries(&browser, &view)
                .iter()
                .any(|e| e.command == Command::CreateWorktree)
        );
        assert!(
            menu_entries(&browser, &view)
                .iter()
                .any(|e| e.label == "Resume session")
        );
        let mut annotation = selected_git_session(&browser, &view).unwrap();
        let mut name_column = None;
        for status in [
            crate::domain::RuntimeStatus::Idle,
            crate::domain::RuntimeStatus::Running,
            crate::domain::RuntimeStatus::Error,
            crate::domain::RuntimeStatus::Waiting {
                approval: true,
                input: false,
            },
            crate::domain::RuntimeStatus::Waiting {
                approval: false,
                input: true,
            },
            crate::domain::RuntimeStatus::Waiting {
                approval: true,
                input: true,
            },
        ] {
            annotation.status = status;
            let spans = git_session_spans(&view, 0, None, &annotation, true, 0);
            assert_eq!(
                spans
                    .iter()
                    .find(|span| span.content.contains('└'))
                    .unwrap()
                    .style
                    .fg,
                Some(Color::White)
            );
            let line: String = spans.iter().map(|span| span.content.as_ref()).collect();
            let title_start = line.find(&annotation.session.title).unwrap();
            let column = unicode_width::UnicodeWidthStr::width(&line[..title_start]);
            let expected = match status {
                crate::domain::RuntimeStatus::Idle => "Idle",
                crate::domain::RuntimeStatus::Running => "Running",
                crate::domain::RuntimeStatus::Error => "Error",
                crate::domain::RuntimeStatus::Waiting {
                    approval: true,
                    input: true,
                } => "Waiting",
                crate::domain::RuntimeStatus::Waiting { approval: true, .. } => "Approval",
                _ => "Input",
            };
            assert!(line[..title_start].ends_with(&format!("{expected:>8} └─ ")));
            let continuing: String = git_session_spans(&view, 0, None, &annotation, false, 0)
                .iter()
                .map(|span| span.content.as_ref())
                .collect();
            assert_eq!(continuing, line.replace(" └─ ", " ├─ "));
            let prefix: String = view
                .history
                .session_spans(0, None)
                .iter()
                .map(|span| span.content.as_ref())
                .collect();
            assert_eq!(
                column,
                unicode_width::UnicodeWidthStr::width(prefix.as_str()) + 12
            );
            assert_eq!(*name_column.get_or_insert(column), column);
        }
        for (width, height) in [(120, 40), (80, 24), (60, 20), (40, 16), (12, 9)] {
            let mut terminal =
                Terminal::new(ratatui::backend::TestBackend::new(width, height)).unwrap();
            terminal.draw(|frame| draw(frame, &browser, &view)).unwrap();
            if width == 120 {
                let text: String = terminal
                    .backend()
                    .buffer()
                    .content
                    .iter()
                    .map(|c| c.symbol())
                    .collect();
                assert!(text.contains("Implement login 界"));
                assert!(text.contains("codex · feature"));
                assert!(text.contains("Input └─ Implement login 界"));
            }
        }
        assert!(matches!(
            handle_key(
                &mut browser,
                &mut view,
                KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)
            ),
            Action::OpenRelink
        ));
        assert_eq!(view.context(), 1);
        assert_eq!(browser.folder, FolderFilter::Path("/worktree/src".into()));
        assert_eq!(browser.selected, Some(key));
        assert!(browser.query.is_empty());
        assert!(browser.expanded.contains(std::path::Path::new("/repo")));
        assert!(
            browser
                .folder_rows()
                .iter()
                .any(|r| r.filter == browser.folder)
        );
    }

    #[tokio::test]
    async fn git_session_enter_resumes_selected_session_in_one_step() {
        use crate::git_sessions::tests::{add_session, fixture};
        let cwd = tempfile::tempdir().unwrap();
        for status in [
            crate::domain::RuntimeStatus::Idle,
            crate::domain::RuntimeStatus::Running,
            crate::domain::RuntimeStatus::Waiting {
                approval: false,
                input: true,
            },
        ] {
            let (mut browser, history) = fixture().await;
            let key = add_session(
                &mut browser,
                "Direct resume",
                cwd.path().to_str().unwrap(),
                "/worktree",
                "/repo/.git",
                status,
                1,
            );
            let mut view = View {
                history,
                git_focus: true,
                ..View::default()
            };
            press(&mut browser, &mut view, KeyCode::Down);
            assert!(matches!(
                handle_key(
                    &mut browser,
                    &mut view,
                    KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)
                ),
                Action::ResumeSelected
            ));
            assert_eq!(browser.selected, Some(key));
            assert_eq!(browser.folder, FolderFilter::Path(cwd.path().into()));
            assert_eq!(view.context(), 1);
        }
    }

    #[tokio::test]
    async fn git_session_provider_column_aligns_unicode_names_across_commits_and_scroll() {
        use crate::git_sessions::tests::{add_session, fixture};
        let (mut browser, history) = fixture().await;
        for (title, root) in [
            ("Short", "/repo"),
            ("Longer 界界 name", "/worktree"),
            ("e\u{301} task", "/repo"),
        ] {
            add_session(
                &mut browser,
                title,
                root,
                root,
                "/repo/.git",
                crate::domain::RuntimeStatus::Running,
                1,
            );
        }
        browser
            .repositories
            .get_mut(std::path::Path::new("/repo/.git"))
            .unwrap()
            .worktrees[1]
            .head = Some("older".into());
        let view = View {
            history,
            ..View::default()
        };
        let mut presentation = view.git_sessions.borrow_mut();
        presentation.sync(&browser, &view.history);
        let width = git_session_name_width(&presentation);
        assert_eq!(
            width,
            unicode_width::UnicodeWidthStr::width("Longer 界界 name")
        );
        let mut provider_column = None;
        for row in &presentation.rows {
            if let crate::git_sessions::Row::Session {
                anchor,
                annotation,
                connector,
            } = row
            {
                let spans = git_session_spans(
                    &view,
                    *anchor,
                    *connector,
                    &presentation.annotations[*annotation],
                    presentation.is_last_session(*annotation),
                    width,
                );
                let text: String = spans.iter().map(|s| s.content.as_ref()).collect();
                let column =
                    unicode_width::UnicodeWidthStr::width(&text[..text.find("codex").unwrap()]);
                assert_eq!(*provider_column.get_or_insert(column), column);
            }
        }
        presentation.selected = presentation.rows.len() - 1;
        presentation.range(1);
        assert_eq!(git_session_name_width(&presentation), width);
    }

    #[tokio::test]
    async fn commit_info_shortcut_opens_modal_and_preserves_selection_on_close() {
        let (mut browser, mut view) = navigation_fixture(&["selected-oid"]).await;
        assert!(
            menu_entries(&browser, &view)
                .iter()
                .any(|entry| entry.command == Command::CommitInfo)
        );
        let key = |code| KeyEvent::new(code, KeyModifiers::NONE);
        handle_key(&mut browser, &mut view, key(KeyCode::Char('i')));
        assert!(view.commit_info.is_some());
        assert!(view.modal_open());
        handle_key(&mut browser, &mut view, key(KeyCode::Down));
        assert_eq!(view.history.selected, 0);
        handle_key(&mut browser, &mut view, key(KeyCode::Esc));
        assert!(view.commit_info.is_none());
        assert!(view.git_focus);
        handle_key(&mut browser, &mut view, key(KeyCode::Char('i')));
        handle_key(&mut browser, &mut view, key(KeyCode::Char('i')));
        assert!(view.commit_info.is_none());
        view.history.rows.clear();
        handle_key(&mut browser, &mut view, key(KeyCode::Char('i')));
        assert!(view.commit_info.is_none());
    }

    async fn navigation_fixture(heads: &[&str]) -> (Browser, View) {
        let mut browser = Browser::new(vec![]);
        let repository = std::path::PathBuf::from("/project/.git");
        browser.install_repository(crate::git::Repository {
            common_dir: repository.clone(),
            git_dir: repository.clone(),
            worktrees: heads
                .iter()
                .enumerate()
                .map(|(i, head)| crate::git::Worktree {
                    path: if i == 0 {
                        "/project".into()
                    } else {
                        format!("/elsewhere/worktree-{i}").into()
                    },
                    head: Some((*head).into()),
                    branch: (i == 0).then(|| "refs/heads/main".into()),
                    bare: false,
                    locked: false,
                    prunable: false,
                })
                .collect(),
        });
        browser.expanded.clear();
        let mut view = View {
            git_focus: true,
            focus: 1,
            ..View::default()
        };
        view.history.tick(Some(repository), 0, false, 10).await;
        view.history.rows = vec![crate::git::GraphRow {
            stash: None,
            oid: Some("selected-oid".into()),
            graph: String::new(),
            graph_spans: vec![],
            parents: vec![],
            subject: "Selected commit".into(),
        }];
        (browser, view)
    }

    #[tokio::test]
    async fn stash_selection_is_read_only_and_cannot_create_a_worktree() {
        let (mut browser, mut view) = navigation_fixture(&["selected-oid"]).await;
        view.history.rows[0].stash = Some(crate::git::Stash {
            selectors: vec!["stash@{0}".into()],
            base: "a".repeat(40),
        });
        assert!(
            worktree_unavailable(&browser, &view)
                .unwrap()
                .contains("rather than a stash")
        );
        assert!(matches!(
            execute_command(Command::GitSelectWorktree, &mut browser, &mut view),
            Action::Continue
        ));
        assert!(
            view.message
                .as_ref()
                .unwrap()
                .contains("Saved stash stash@{0}")
        );
        assert!(view.git_focus);
        assert!(view.worktree_picker.is_none());
        assert!(
            footer_shortcuts(120, &browser, &view)
                .1
                .contains("Enter Stash")
        );
    }

    #[tokio::test]
    async fn git_enter_reveals_linked_folder_and_enters_sessions() {
        let (mut browser, mut view) = navigation_fixture(&["other", "selected-oid"]).await;
        press(&mut browser, &mut view, KeyCode::Enter);
        assert_eq!(
            browser.folder,
            FolderFilter::Path("/elsewhere/worktree-1".into())
        );
        assert!(
            browser
                .folder_rows()
                .iter()
                .any(|row| row.filter == browser.folder)
        );
        assert!(browser.expanded.contains(std::path::Path::new("/project")));
        assert_eq!(view.context(), 1);
        assert!(browser.visible().is_empty());
        assert!(browser.selected.is_none());
        assert_eq!(
            view.history.rows[view.history.selected].oid.as_deref(),
            Some("selected-oid")
        );
    }

    #[tokio::test]
    async fn git_enter_chooser_confirms_cancels_and_renders() {
        let (mut browser, mut view) = navigation_fixture(&["selected-oid", "selected-oid"]).await;
        let original = browser.folder.clone();
        press(&mut browser, &mut view, KeyCode::Enter);
        assert_eq!(browser.folder, original);
        assert!(view.worktree_picker.is_some());
        for width in [40, 100] {
            let mut terminal =
                Terminal::new(ratatui::backend::TestBackend::new(width, 24)).unwrap();
            terminal.draw(|frame| draw(frame, &browser, &view)).unwrap();
            let rendered: String = terminal
                .backend()
                .buffer()
                .content
                .iter()
                .map(|cell| cell.symbol())
                .collect();
            assert!(rendered.contains("Choose worktree"));
            assert!(rendered.contains("/elsewhere/worktree-1"));
            assert!(rendered.contains("detached"));
            assert!(rendered.contains("/project"));
        }
        press(&mut browser, &mut view, KeyCode::Esc);
        assert!(view.worktree_picker.is_none());
        assert_eq!(browser.folder, original);
        assert!(view.git_focus);
        press(&mut browser, &mut view, KeyCode::Enter);
        press(&mut browser, &mut view, KeyCode::Char('j'));
        press(&mut browser, &mut view, KeyCode::Enter);
        assert_eq!(browser.folder, FolderFilter::Path("/project".into()));
        assert_eq!(view.context(), 1);
    }

    #[tokio::test]
    async fn git_enter_revalidates_chooser_and_rejects_stale_inventory() {
        let (mut browser, mut view) = navigation_fixture(&["selected-oid", "selected-oid"]).await;
        let original = browser.folder.clone();
        press(&mut browser, &mut view, KeyCode::Enter);
        let repo = browser.repositories.values_mut().next().unwrap();
        repo.worktrees[1].head = Some("changed".into());
        press(&mut browser, &mut view, KeyCode::Enter);
        assert_eq!(browser.folder, original);
        assert!(view.git_focus);
        assert!(view.message.as_ref().unwrap().contains("changed"));
        browser
            .repository_errors
            .insert("/project/.git".into(), "refreshing".into());
        press(&mut browser, &mut view, KeyCode::Enter);
        assert_eq!(browser.folder, original);
        assert!(view.message.as_ref().unwrap().contains("stale"));
        browser.repository_errors.clear();
        browser.repositories.clear();
        press(&mut browser, &mut view, KeyCode::Enter);
        assert!(view.message.as_ref().unwrap().contains("unavailable"));
    }

    #[tokio::test]
    async fn git_enter_ignores_unmatched_bare_hidden_and_repeat_events() {
        let (mut browser, mut view) = navigation_fixture(&["other"]).await;
        let original = browser.folder.clone();
        let mut unrelated = browser.repositories.values().next().unwrap().clone();
        unrelated.common_dir = "/unrelated/.git".into();
        unrelated.worktrees[0].head = Some("selected-oid".into());
        browser.install_repository(unrelated);
        press(&mut browser, &mut view, KeyCode::Enter);
        assert!(view.message.as_ref().unwrap().contains("No worktree"));
        let repo = browser
            .repositories
            .get_mut(std::path::Path::new("/project/.git"))
            .unwrap();
        repo.worktrees[0].head = Some("selected-oid".into());
        repo.worktrees[0].bare = true;
        press(&mut browser, &mut view, KeyCode::Enter);
        assert_eq!(browser.folder, original);
        browser
            .repositories
            .get_mut(std::path::Path::new("/project/.git"))
            .unwrap()
            .worktrees[0]
            .bare = false;
        view.message = None;
        view.git_hidden = true;
        press(&mut browser, &mut view, KeyCode::Enter);
        view.git_hidden = false;
        view.git_size_limited = true;
        press(&mut browser, &mut view, KeyCode::Enter);
        view.git_size_limited = false;
        handle_key(
            &mut browser,
            &mut view,
            KeyEvent::new_with_kind(KeyCode::Enter, KeyModifiers::NONE, KeyEventKind::Repeat),
        );
        assert_eq!(browser.folder, original);
        assert!(view.git_focus);
        assert!(view.message.is_none());
        view.history.rows[0].oid = None;
        press(&mut browser, &mut view, KeyCode::Enter);
        assert_eq!(browser.folder, original);
    }

    #[test]
    fn right_expands_then_focuses_sessions_and_left_follows_displayed_parent() {
        for (left, right) in [
            (KeyCode::Left, KeyCode::Right),
            (KeyCode::Char('h'), KeyCode::Char('l')),
        ] {
            let mut browser = Browser::new(vec![]);
            browser.open_folder("/project/child/leaf".into());
            browser.select_folder(FolderFilter::Path("/project".into()));
            let mut view = View::default();
            let press = |browser: &mut Browser, view: &mut View, code| {
                handle_key(browser, view, KeyEvent::new(code, KeyModifiers::NONE))
            };
            press(&mut browser, &mut view, left);
            assert!(!browser.expanded.contains(std::path::Path::new("/project")));
            assert!(
                browser
                    .expanded
                    .contains(std::path::Path::new("/project/child"))
            );
            let selected = browser.selected.clone();
            press(&mut browser, &mut view, right);
            assert_eq!(browser.folder, FolderFilter::Path("/project".into()));
            assert!(browser.expanded.contains(std::path::Path::new("/project")));
            assert_eq!(view.focus, 0);
            assert!(
                footer_shortcuts(240, &browser, &view)
                    .1
                    .contains("→/l Sessions")
            );
            press(&mut browser, &mut view, right);
            assert_eq!(browser.folder, FolderFilter::Path("/project".into()));
            assert_eq!(browser.selected, selected);
            assert_eq!(view.focus, 1);
            press(&mut browser, &mut view, KeyCode::Esc);
            press(&mut browser, &mut view, KeyCode::Down);
            assert_eq!(browser.folder, FolderFilter::Path("/project/child".into()));
            press(&mut browser, &mut view, left);
            assert_eq!(browser.folder, FolderFilter::Path("/project/child".into()));
            press(&mut browser, &mut view, left);
            assert_eq!(browser.folder, FolderFilter::Path("/project".into()));
            browser.open_folder("/project/child/leaf".into());
            browser
                .expanded
                .remove(std::path::Path::new("/project/child/leaf"));
            assert!(
                footer_shortcuts(240, &browser, &view)
                    .1
                    .contains("→/l Expand")
            );
            press(&mut browser, &mut view, right);
            assert!(
                browser
                    .expanded
                    .contains(std::path::Path::new("/project/child/leaf"))
            );
            assert_eq!(view.focus, 0);
            assert_eq!(
                browser.folder,
                FolderFilter::Path("/project/child/leaf".into())
            );
            press(&mut browser, &mut view, right);
            assert_eq!(view.focus, 1);
            assert_eq!(
                browser.folder,
                FolderFilter::Path("/project/child/leaf".into())
            );
            press(&mut browser, &mut view, KeyCode::Esc);
            press(&mut browser, &mut view, left);
            press(&mut browser, &mut view, left);
            assert_eq!(browser.folder, FolderFilter::Path("/project/child".into()));
            browser.select_folder(FolderFilter::Path("/".into()));
            press(&mut browser, &mut view, left);
            press(&mut browser, &mut view, left);
            assert_eq!(browser.folder, FolderFilter::Path("/".into()));
            for filter in [FolderFilter::Unlinked, FolderFilter::Unknown] {
                browser.select_folder(filter.clone());
                let expanded = browser.expanded.clone();
                press(&mut browser, &mut view, left);
                press(&mut browser, &mut view, right);
                assert_eq!(browser.folder, filter);
                assert_eq!(browser.expanded, expanded);
                press(&mut browser, &mut view, KeyCode::Enter);
                assert_eq!(view.focus, 1);
                press(&mut browser, &mut view, KeyCode::Esc);
                assert_eq!(view.focus, 0);
            }
        }
    }

    #[test]
    fn worktree_parent_is_displayed_repository_and_enter_repeat_is_ignored() {
        let mut browser = Browser::new(vec![]);
        browser.open_folder("/project".into());
        let worktree = |path: &str| crate::git::Worktree {
            path: path.into(),
            head: None,
            branch: None,
            bare: false,
            locked: false,
            prunable: false,
        };
        browser.install_repository(crate::git::Repository {
            common_dir: "/project/.git".into(),
            git_dir: "/project/.git".into(),
            worktrees: vec![worktree("/project"), worktree("/elsewhere/feature")],
        });
        let mut view = View::default();
        let key = |code| KeyEvent::new(code, KeyModifiers::NONE);
        handle_key(&mut browser, &mut view, key(KeyCode::Down));
        assert_eq!(
            browser.folder,
            FolderFilter::Path("/elsewhere/feature".into())
        );
        handle_key(&mut browser, &mut view, key(KeyCode::Left));
        assert_eq!(browser.folder, FolderFilter::Path("/project".into()));
        let expanded = browser.expanded.clone();
        handle_key(&mut browser, &mut view, key(KeyCode::Enter));
        assert_eq!(view.focus, 1);
        assert_eq!(browser.expanded, expanded);
        assert_eq!(
            handle_key(
                &mut browser,
                &mut view,
                KeyEvent::new_with_kind(KeyCode::Enter, KeyModifiers::NONE, KeyEventKind::Repeat)
            ),
            Action::Continue
        );
        assert_eq!(
            handle_key(&mut browser, &mut view, key(KeyCode::Char('m'))),
            Action::OpenRelink
        );
        assert_eq!(
            handle_key(&mut browser, &mut view, key(KeyCode::Char('r'))),
            Action::Refresh
        );
        for code in [KeyCode::Right, KeyCode::Char('h'), KeyCode::Char('l')] {
            assert_eq!(
                handle_key(&mut browser, &mut view, key(code)),
                Action::Continue
            );
            assert_eq!(view.focus, 1);
        }
        for code in [KeyCode::Left, KeyCode::Esc, KeyCode::Backspace] {
            view.focus = 1;
            handle_key(&mut browser, &mut view, key(code));
            assert_eq!(view.focus, 0);
            assert_eq!(browser.folder, FolderFilter::Path("/project".into()));
            assert_eq!(browser.expanded, expanded);
        }
    }

    #[test]
    fn worktree_action_tracks_selected_repository_availability() {
        use crate::git_discovery::{Outcome, Ownership};
        let path = std::path::PathBuf::from("/project");
        let mut browser = Browser::new(vec![crate::browser::SourceSpec::Codex {
            binary: "codex".into(),
            source: crate::config::SourceConfig {
                id: crate::domain::SourceId("test".into()),
                home: "/codex".into(),
            },
        }]);
        browser.folder = FolderFilter::Path(path.clone());
        let repository = Outcome::Repository {
            repository: crate::git::Directory {
                common_dir: path.join(".git"),
                git_dir: path.join(".git"),
            },
            root: Some(path.clone()),
        };
        let mut view = View::default();
        for (outcome, stale, available) in [
            (None, false, false),
            (Some(Outcome::NotRepository), false, false),
            (Some(Outcome::Unavailable("offline".into())), false, false),
            (Some(repository.clone()), true, false),
            (Some(repository.clone()), false, true),
            (Some(Outcome::NotRepository), false, false),
        ] {
            browser.ownership.insert(
                path.clone(),
                Ownership {
                    generation: browser.git_generation + u64::from(stale),
                    outcome,
                    previous: Some(repository.clone()),
                },
            );
            assert_eq!(
                menu_entries(&browser, &view)
                    .iter()
                    .any(|entry| entry.command == Command::CreateWorktree),
                available
            );
            assert_eq!(
                footer_shortcuts(240, &browser, &view)
                    .1
                    .contains("w Worktree"),
                available
            );
            if !available {
                view.message = Some("unchanged".into());
                handle_key(
                    &mut browser,
                    &mut view,
                    KeyEvent::new(KeyCode::Char('w'), KeyModifiers::NONE),
                );
                assert!(view.worktree_create.is_none());
                assert_eq!(view.message.as_deref(), Some("unchanged"));
            }
        }
        browser.ownership.get_mut(&path).unwrap().outcome = Some(repository.clone());
        open_help(&browser, &mut view);
        view.help_selected = menu_entries(&browser, &view)
            .iter()
            .position(|entry| entry.command == Command::CreateWorktree)
            .unwrap();
        assert_eq!(
            menu_entries(&browser, &view)[view.help_selected].command,
            Command::CreateWorktree
        );
        browser.git_generation += 1;
        assert_eq!(
            menu_entries(&browser, &view)[view.help_selected].command,
            Command::CreateWorktree
        );
        press(&mut browser, &mut view, KeyCode::Enter);
        assert!(view.help);
        assert!(view.error_dialog.is_none());
        assert!(view.worktree_create.is_none());
        browser.ownership.get_mut(&path).unwrap().generation = browser.git_generation;
        assert!(worktree_unavailable(&browser, &view).is_none());
        press(&mut browser, &mut view, KeyCode::Esc);
        browser.folder = FolderFilter::All;
        assert!(worktree_unavailable(&browser, &view).is_some());
        browser.folder = FolderFilter::Path(path.clone());
        browser.ownership.get_mut(&path).unwrap().outcome = Some(repository);
        view.git_focus = true;
        assert!(
            !menu_entries(&browser, &view)
                .iter()
                .any(|entry| entry.command == Command::CreateWorktree)
        );
        view.git_focus = false;
        browser.specs.clear();
        assert!(
            !menu_entries(&browser, &view)
                .iter()
                .any(|entry| entry.command == Command::CreateWorktree)
        );
    }

    #[test]
    fn recovery_menu_and_shortcut_follow_availability() {
        let mut browser = Browser::new(vec![]);
        let mut view = View::default();
        for available in [false, true, false] {
            view.recovery_available = available;
            for focus in [0, 1] {
                view.focus = focus;
                assert_eq!(
                    menu_entries(&browser, &view)
                        .iter()
                        .any(|e| e.command == Command::Recover),
                    available
                );
                assert_eq!(
                    footer_shortcuts(240, &browser, &view)
                        .0
                        .contains("u Recover"),
                    available
                );
                assert_eq!(
                    matches!(
                        handle_key(
                            &mut browser,
                            &mut view,
                            KeyEvent::new(KeyCode::Char('u'), KeyModifiers::NONE)
                        ),
                        Action::RecoverRelink
                    ),
                    available
                );
            }
        }
    }

    #[test]
    fn recovery_refresh_disables_missing_or_unreadable_state() {
        let mut view = View {
            recovery_available: true,
            ..View::default()
        };
        assert_eq!(refresh_recovery(None, &mut view), None);
        assert!(!view.recovery_available);
        let state = tempfile::tempdir().unwrap();
        assert_eq!(
            refresh_recovery(Some(state.path()), &mut view),
            Some((0, false))
        );
        assert!(!view.recovery_available);
        std::fs::create_dir(state.path().join("folder-moves")).unwrap();
        std::fs::write(state.path().join("folder-moves/current.json"), "invalid").unwrap();
        view.recovery_available = true;
        assert_eq!(refresh_recovery(Some(state.path()), &mut view), None);
        assert!(!view.recovery_available);
        assert!(
            view.message
                .as_deref()
                .unwrap()
                .contains("Cannot inspect folder-move journal")
        );
    }

    #[tokio::test]
    async fn session_opening_shortcuts_choose_mode_and_ignore_repeats() {
        use crate::git_sessions::tests::{add_session, fixture};
        let cwd = tempfile::tempdir().unwrap();
        let (mut browser, _) = fixture().await;
        let session = add_session(
            &mut browser,
            "Open mode",
            cwd.path().to_str().unwrap(),
            "/worktree",
            "/repo/.git",
            crate::domain::RuntimeStatus::Idle,
            1,
        );
        browser.open_folder(cwd.path().into());
        browser.selected = Some(session);
        let mut view = View {
            focus: 1,
            ..View::default()
        };
        for (code, modifiers, fullscreen) in [
            (KeyCode::Enter, KeyModifiers::NONE, false),
            (KeyCode::Char('f'), KeyModifiers::ALT, true),
        ] {
            let key = KeyEvent::new(code, modifiers);
            let action = handle_key(&mut browser, &mut view, key);
            assert!(if fullscreen {
                matches!(action, Action::ResumeSelectedFullscreen)
            } else {
                matches!(action, Action::ResumeSelected)
            });
            assert!(view.terminals.borrow().active.is_none());
            for kind in [KeyEventKind::Repeat, KeyEventKind::Release] {
                assert!(matches!(
                    handle_key(&mut browser, &mut view, KeyEvent { kind, ..key }),
                    Action::Continue
                ));
            }
            view.preparing = true;
            assert!(matches!(
                handle_key(&mut browser, &mut view, key),
                Action::Continue
            ));
            view.preparing = false;
        }
        assert!(matches!(
            execute_command(Command::Resume, &mut browser, &mut view),
            Action::ResumeSelected
        ));
        assert!(matches!(
            execute_command(Command::ResumeFullscreen, &mut browser, &mut view),
            Action::ResumeSelectedFullscreen
        ));
    }

    #[test]
    fn alt_f_only_toggles_a_displayed_terminal_from_any_pane() {
        for (focus, git_focus, folders_hidden) in [
            (0, false, false),
            (1, false, false),
            (0, true, false),
            (1, true, false),
            (0, false, true),
        ] {
            let mut browser = Browser::new(vec![]);
            let mut view = View {
                focus,
                git_focus,
                folders_hidden,
                ..View::default()
            };
            let key = KeyEvent::new(KeyCode::Char('f'), KeyModifiers::ALT);
            let following = view.history.following;
            assert!(matches!(
                handle_key(&mut browser, &mut view, key),
                Action::Continue
            ));
            assert!(!view.terminals.borrow().fullscreen);
            assert_eq!((view.focus, view.git_focus), (focus, git_focus));
            assert_eq!(view.history.following, following);
            view.terminals.borrow_mut().active = Some(1);
            view.help = true;
            handle_key(&mut browser, &mut view, key);
            assert!(!view.terminals.borrow().fullscreen);
            view.help = false;
            handle_key(
                &mut browser,
                &mut view,
                KeyEvent {
                    kind: KeyEventKind::Repeat,
                    ..key
                },
            );
            assert!(!view.terminals.borrow().fullscreen);
            handle_key(&mut browser, &mut view, key);
            assert!(view.terminals.borrow().fullscreen);
            assert_eq!((view.focus, view.git_focus), (1, false));
            handle_key(&mut browser, &mut view, key);
            assert!(!view.terminals.borrow().fullscreen);
        }
    }

    #[tokio::test]
    async fn responsive_navigation_preserves_selection_and_scroll() {
        for has_git in [false, true] {
            let mut browser = Browser::new(vec![]);
            browser.open_folder("/synthetic/repo/child".into());
            let mut view = View::default();
            if has_git {
                view.history
                    .tick(Some("/synthetic/repo".into()), 0, false, 1)
                    .await;
            }
            view.history.selected = 7;
            view.history.top.set(3);
            view.history.horizontal.set(4);
            let folder = browser.folder.clone();
            let selected = browser.selected.clone();
            let expanded = browser.expanded.clone();
            for mode in [LayoutMode::Columns, LayoutMode::Stacked, LayoutMode::Single] {
                view.layout_mode.set(mode);
                for current in 0..if has_git { 3 } else { 2 } {
                    for (key, forward, horizontal) in [
                        (KeyCode::Left, false, true),
                        (KeyCode::Right, true, true),
                        (KeyCode::Up, false, false),
                        (KeyCode::Down, true, false),
                    ] {
                        view.focus = current.min(1);
                        view.git_focus = current == 2;
                        view.folders_hidden = current != 0;
                        view.git_hidden = current != 2;
                        let active = mode == LayoutMode::Single
                            || horizontal == (mode == LayoutMode::Columns);
                        let expected = if !active {
                            current
                        } else if forward {
                            (current + 1).min(if has_git { 2 } else { 1 })
                        } else {
                            current.saturating_sub(1)
                        };
                        handle_key(
                            &mut browser,
                            &mut view,
                            KeyEvent::new(key, KeyModifiers::ALT),
                        );
                        assert_eq!(view.context(), expected);
                        if expected == 0 {
                            assert!(!view.folders_hidden);
                        }
                        if expected == 2 {
                            assert!(!view.git_hidden);
                        }
                        assert_eq!(browser.folder, folder);
                        assert_eq!(browser.selected, selected);
                        assert_eq!(browser.expanded, expanded);
                        assert_eq!(
                            (
                                view.history.selected,
                                view.history.top.get(),
                                view.history.horizontal.get()
                            ),
                            (7, 3, 4)
                        );
                    }
                }
            }
            view.focus = 1;
            view.git_focus = false;
            for modal in 0..4 {
                view.help = modal == 0;
                view.editing = modal == 1;
                view.preparing = modal == 2;
                view.source_picker = (modal == 3).then_some(0);
                for key in [KeyCode::Left, KeyCode::Right, KeyCode::Up, KeyCode::Down] {
                    handle_key(
                        &mut browser,
                        &mut view,
                        KeyEvent::new(key, KeyModifiers::ALT),
                    );
                    assert_eq!(view.context(), 1);
                }
            }
        }
    }

    #[tokio::test]
    async fn directional_menu_matches_layout_and_returns_to_sessions() {
        let mut browser = Browser::new(vec![]);
        let mut view = View {
            git_focus: true,
            folders_hidden: true,
            ..View::default()
        };
        view.history
            .tick(Some("/synthetic/repo".into()), 0, false, 1)
            .await;
        for (mode, command) in [
            (LayoutMode::Columns, Command::PaneLeft),
            (LayoutMode::Stacked, Command::PaneUp),
        ] {
            view.layout_mode.set(mode);
            view.git_focus = true;
            open_help(&browser, &mut view);
            view.help_selected = menu_entries(&browser, &view)
                .iter()
                .position(|e| e.command == command)
                .unwrap();
            press(&mut browser, &mut view, KeyCode::Enter);
            assert_eq!(view.context(), 1);
            assert!(view.folders_hidden);
            assert!(!view.help);
            view.git_focus = true;
            press(&mut browser, &mut view, KeyCode::Esc);
            assert_eq!(view.context(), 1);
        }
    }

    #[test]
    fn deep_search_toggle_is_keyboard_accessible_and_renders_small_terminals() {
        let mut browser = Browser::new(vec![]);
        let mut view = View {
            editing: true,
            search_query: "AltTab".into(),
            ..View::default()
        };
        handle_key(
            &mut browser,
            &mut view,
            KeyEvent::new(KeyCode::Char('d'), KeyModifiers::CONTROL),
        );
        assert!(browser.deep_search);
        assert_eq!(view.search_query, "AltTab");
        handle_key(
            &mut browser,
            &mut view,
            KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE),
        );
        assert_eq!(view.search_control, 3);
        handle_key(
            &mut browser,
            &mut view,
            KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE),
        );
        assert!(!browser.deep_search);
        let mut terminal = Terminal::new(ratatui::backend::TestBackend::new(100, 30)).unwrap();
        terminal
            .draw(|frame| draw_search(frame, &browser, &view))
            .unwrap();
        let screen: String = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect();
        assert!(screen.contains("Deep search: OFF"));
        assert!(screen.contains("Enable deep search"));
        assert!(screen.contains("Space toggles"));
        handle_key(
            &mut browser,
            &mut view,
            KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
        );
        assert!(browser.deep_search);
        assert!(view.editing);
        for (width, height) in [(120, 35), (40, 10), (15, 5)] {
            let mut terminal =
                Terminal::new(ratatui::backend::TestBackend::new(width, height)).unwrap();
            terminal
                .draw(|frame| draw_search(frame, &browser, &view))
                .unwrap();
            if width >= 40 {
                let screen: String = terminal
                    .backend()
                    .buffer()
                    .content
                    .iter()
                    .map(|cell| cell.symbol())
                    .collect();
                assert!(screen.contains("Deep search: ON"));
            }
        }
        handle_key(
            &mut browser,
            &mut view,
            KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE),
        );
        assert!(!view.editing);
        assert!(browser.deep_search);
    }

    #[tokio::test]
    async fn tab_cycles_available_panes_in_both_directions() {
        for has_git in [false, true] {
            let mut browser = Browser::new(vec![]);
            browser.open_folder("/synthetic/repo/child".into());
            let mut view = View::default();
            if has_git {
                view.history
                    .tick(Some("/synthetic/repo".into()), 0, false, 1)
                    .await;
            }
            let count = if has_git { 3 } else { 2 };
            let folder = browser.folder.clone();
            let expanded = browser.expanded.clone();
            for limited in [false, true] {
                view.git_size_limited = limited;
                for current in 0..count {
                    for (event, backwards) in [
                        (KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE), false),
                        (KeyEvent::new(KeyCode::BackTab, KeyModifiers::SHIFT), true),
                        (KeyEvent::new(KeyCode::Tab, KeyModifiers::SHIFT), true),
                    ] {
                        view.focus = current.min(1);
                        view.git_focus = current == 2;
                        view.git_hidden = true;
                        let expected = (current + if backwards { count - 1 } else { 1 }) % count;
                        assert_eq!(handle_key(&mut browser, &mut view, event), Action::Continue);
                        assert_eq!(view.context(), expected);
                        assert_eq!(view.git_hidden, expected != 2);
                        assert_eq!(browser.folder, folder);
                        assert_eq!(browser.expanded, expanded);
                        if expected == 2 {
                            handle_key(
                                &mut browser,
                                &mut view,
                                KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE),
                            );
                            assert_eq!(view.context(), 1);
                            assert!(!view.git_hidden);
                        }
                    }
                }
            }
            // Search owns Tab and Backspace even with Git available.
            view.git_focus = false;
            view.focus = 1;
            view.editing = true;
            view.search_control = 0;
            view.search_query = "ab".into();
            handle_key(
                &mut browser,
                &mut view,
                KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE),
            );
            assert_eq!(view.search_query, "a");
            handle_key(
                &mut browser,
                &mut view,
                KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE),
            );
            assert!(view.editing);
            assert_eq!(view.context(), 1);
            assert_eq!(view.search_control, 3);
            view.editing = false;
            // The action menu uses the same pane-cycle behavior as the key.
            view.help = true;
            view.help_control = 0;
            view.help_selected = menu_entries(&browser, &view)
                .iter()
                .position(|entry| entry.command == Command::CyclePane)
                .unwrap();
            handle_key(
                &mut browser,
                &mut view,
                KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
            );
            assert!(!view.help);
            assert_eq!(view.context(), if has_git { 2 } else { 0 });
        }
    }

    #[test]
    fn g_opens_navigator_from_each_pane_including_collapsed_ones() {
        for (focus, git_focus) in [(0, false), (1, false), (0, true)] {
            let mut browser = Browser::new(vec![]);
            let mut view = View {
                focus,
                git_focus,
                folders_hidden: true,
                git_hidden: true,
                ..View::default()
            };
            press(&mut browser, &mut view, KeyCode::Char('g'));
            assert!(view.terminals.borrow().picker);
            assert!(view.folders_hidden && view.git_hidden);
            assert_eq!((view.focus, view.git_focus), (focus, git_focus));
        }
    }

    #[tokio::test]
    async fn sidebars_collapse_to_sessions_and_navigation_reopens_them() {
        let mut browser = Browser::new(vec![]);
        browser.open_folder("/parent/child".into());
        let mut view = View::default();
        view.history
            .tick(Some("/synthetic/repo".into()), 0, false, 1)
            .await;
        let folder = browser.folder.clone();
        let expanded = browser.expanded.clone();
        view.history.selected = 7;
        view.history.horizontal.set(3);
        for mode in [LayoutMode::Columns, LayoutMode::Stacked, LayoutMode::Single] {
            view.layout_mode.set(mode);
            for side in [0, 2] {
                view.focus = side.min(1);
                view.git_focus = side == 2;
                view.folders_hidden = false;
                view.git_hidden = false;
                press(&mut browser, &mut view, KeyCode::Char('b'));
                assert_eq!(view.context(), 1);
                assert_eq!(view.folders_hidden, side == 0);
                assert_eq!(view.git_hidden, side == 2);
                press(
                    &mut browser,
                    &mut view,
                    if side == 0 {
                        KeyCode::BackTab
                    } else {
                        KeyCode::Tab
                    },
                );
                assert_eq!(view.context(), side);
                assert!(!view.folders_hidden && !view.git_hidden);
                assert_eq!(browser.folder, folder);
                assert_eq!(browser.expanded, expanded);
                assert_eq!(
                    (view.history.selected, view.history.horizontal.get()),
                    (7, 3)
                );
            }
        }
        press(&mut browser, &mut view, KeyCode::Char('l'));
        assert_eq!(view.history.horizontal.get(), 7);
        press(&mut browser, &mut view, KeyCode::Char('h'));
        assert_eq!(view.history.horizontal.get(), 3);
        view.git_size_limited = true;
        press(&mut browser, &mut view, KeyCode::Char('l'));
        assert_eq!(view.history.horizontal.get(), 3);
    }

    #[tokio::test]
    async fn responsive_layout_renders_panels_and_collapsed_rails() {
        let mut browser = Browser::new(vec![]);
        browser.open_folder("/project/界é-long-folder-name".into());
        let mut view = View {
            focus: 1,
            ..View::default()
        };
        view.history
            .tick(Some("/synthetic/repo".into()), 0, false, 1)
            .await;
        for (width, height) in [(160, 40), (100, 60), (80, 24)] {
            for folders_hidden in [false, true] {
                for git_hidden in [false, true] {
                    view.folders_hidden = folders_hidden;
                    view.git_hidden = git_hidden;
                    let mut terminal =
                        Terminal::new(ratatui::backend::TestBackend::new(width, height)).unwrap();
                    terminal.draw(|frame| draw(frame, &browser, &view)).unwrap();
                    let (_, _, _, footer) =
                        browser_footer(Rect::new(0, 0, width, height), &browser, &view);
                    let panels =
                        panel_layout(Rect::new(0, 0, width, height - footer), height, &view, true);
                    let sessions = panels.sessions.unwrap();
                    assert_eq!(
                        terminal.backend().buffer()[(sessions.x, sessions.y)].symbol(),
                        "┏"
                    );
                    for (area, hidden) in
                        [(panels.folders, folders_hidden), (panels.git, git_hidden)]
                    {
                        if let Some(area) = area {
                            let cell = &terminal.backend().buffer()[(area.x, area.y)];
                            assert_eq!(cell.fg, Color::DarkGray);
                            if !hidden {
                                assert_eq!(cell.symbol(), "┌");
                            }
                        }
                    }
                }
            }
        }
    }

    #[tokio::test]
    async fn short_terminals_show_only_the_focused_pane() {
        let mut browser = Browser::new(vec![]);
        browser.open_folder("/synthetic/repo/child".into());
        let mut view = View::default();
        view.history
            .tick(Some("/synthetic/repo".into()), 0, false, 1)
            .await;
        let selected = browser.selected.clone();
        let folder = browser.folder.clone();
        for width in [80, 120] {
            let mut terminal =
                Terminal::new(ratatui::backend::TestBackend::new(width, 24)).unwrap();
            for height in [22, 23, 24, 23] {
                for context in 0..3 {
                    view.focus = context.min(1);
                    view.git_focus = context == 2;
                    terminal.backend_mut().resize(width, height);
                    terminal.resize(Rect::new(0, 0, width, height)).unwrap();
                    terminal.draw(|frame| draw(frame, &browser, &view)).unwrap();
                    let (_, _, _, footer) =
                        browser_footer(Rect::new(0, 0, width, height), &browser, &view);
                    let text: String = terminal
                        .backend()
                        .buffer()
                        .content
                        .iter()
                        .take(usize::from(width * (height - footer)))
                        .map(|cell| cell.symbol())
                        .collect();
                    assert_eq!(text.contains("Git Worktrees"), context == 2);
                    if width < 160 {
                        assert_eq!(text.contains("Folders"), context == 0);
                        assert_eq!(text.contains("Sessions"), context == 1);
                        assert_eq!(terminal.backend().buffer()[(width - 1, 0)].symbol(), "┓");
                    }
                    assert_eq!(view.context(), context);
                    assert_eq!(browser.selected, selected);
                    assert_eq!(browser.folder, folder);
                }
            }
        }
    }

    #[tokio::test]
    async fn short_terminal_hides_collapsed_rails_and_cycles_to_expanded_panes() {
        let mut browser = Browser::new(vec![]);
        let mut view = View {
            folders_hidden: true,
            git_hidden: true,
            focus: 1,
            ..View::default()
        };
        view.history
            .tick(Some("/synthetic/repo".into()), 0, false, 1)
            .await;
        let mut terminal = Terminal::new(ratatui::backend::TestBackend::new(120, 22)).unwrap();
        for expected in [1, 2, 0] {
            terminal.draw(|frame| draw(frame, &browser, &view)).unwrap();
            assert_eq!(view.context(), expected);
            assert_eq!(terminal.backend().buffer()[(0, 0)].symbol(), "┏");
            assert_eq!(terminal.backend().buffer()[(119, 0)].symbol(), "┓");
            press(&mut browser, &mut view, KeyCode::Tab);
        }
        assert!(!view.folders_hidden);
        assert!(!view.git_hidden);
        press(&mut browser, &mut view, KeyCode::BackTab);
        assert_eq!(view.context(), 0);
        press(&mut browser, &mut view, KeyCode::Char('b'));
        terminal.draw(|frame| draw(frame, &browser, &view)).unwrap();
        assert_eq!(view.context(), 1);
        assert!(view.folders_hidden);
        assert_eq!(terminal.backend().buffer()[(0, 0)].symbol(), "┏");
    }

    #[test]
    fn responsive_geometry_boundaries_and_visibility() {
        for (width, height, mode) in [
            (160, 12, LayoutMode::Columns),
            (159, 47, LayoutMode::Single),
            (160, 11, LayoutMode::Single),
            (80, 48, LayoutMode::Stacked),
            (79, 60, LayoutMode::Single),
            (159, 48, LayoutMode::Stacked),
            (160, 48, LayoutMode::Columns),
            (140, 60, LayoutMode::Stacked),
        ] {
            let view = View {
                focus: 1,
                ..View::default()
            };
            for footer in [1, 2, 3] {
                assert_eq!(
                    panel_layout(Rect::new(0, 0, width, height - footer), height, &view, true).mode,
                    mode
                );
            }
        }
        for (width, height) in [
            (160, 24),
            (160, 40),
            (240, 60),
            (100, 100),
            (100, 60),
            (80, 48),
        ] {
            for present in [false, true] {
                for folders_hidden in [false, true] {
                    for git_hidden in [false, true] {
                        let view = View {
                            focus: 1,
                            folders_hidden,
                            git_hidden,
                            ..View::default()
                        };
                        let area = Rect::new(0, 0, width, height - 3);
                        let p = panel_layout(area, height, &view, present);
                        let f = p.folders.unwrap();
                        let s = p.sessions.unwrap();
                        assert_eq!(p.git.is_some(), present);
                        assert_eq!(p.graph_visible, present && !git_hidden);
                        if p.mode == LayoutMode::Columns {
                            assert_eq!(f.right(), s.x);
                            assert!(s.width >= 80);
                            assert_eq!(
                                f.width,
                                if folders_hidden {
                                    2
                                } else {
                                    (width / 6).clamp(24, 32)
                                }
                            );
                            assert_eq!(
                                p.git.map_or(s.right(), |g| {
                                    assert_eq!(s.right(), g.x);
                                    g.right()
                                }),
                                area.right()
                            );
                        } else {
                            assert_eq!(f.bottom(), s.y);
                            assert!(s.height > f.height);
                            assert!(s.height >= area.height * 2 / 5);
                            assert_eq!(
                                f.height,
                                if folders_hidden {
                                    1
                                } else {
                                    (area.height / 5).min(12)
                                }
                            );
                            if let Some(g) = p.git {
                                assert_eq!(
                                    g.height,
                                    if git_hidden {
                                        1
                                    } else {
                                        (area.height / 5).min(12) * 2
                                    }
                                );
                                if !folders_hidden && !git_hidden {
                                    assert_eq!(g.height, f.height * 2);
                                }
                            }
                            assert_eq!(
                                p.git.map_or(s.bottom(), |g| {
                                    assert_eq!(s.bottom(), g.y);
                                    g.bottom()
                                }),
                                area.bottom()
                            );
                        }
                    }
                }
            }
        }
        let view = View {
            git_focus: true,
            ..View::default()
        };
        assert!(panel_layout(Rect::new(0, 0, 80, 4), 6, &view, true).graph_visible);
        assert!(!panel_layout(Rect::new(0, 0, 80, 3), 5, &view, true).graph_visible);
        assert!(
            panel_layout(Rect::new(0, 0, 80, 20), 24, &view, false)
                .sessions
                .is_some()
        );
    }

    #[test]
    fn folder_selection_stays_centered_in_both_directions() {
        let mut browser = Browser::new(vec![]);
        let path = format!(
            "/{}",
            (0..20)
                .map(|i| format!("node{i:02}"))
                .collect::<Vec<_>>()
                .join("/")
        );
        browser.open_folder(path.into());
        let view = View::default();
        let rows = browser.folder_rows();
        let last = rows.len() - 1;
        let mut terminal = Terminal::new(ratatui::backend::TestBackend::new(160, 9)).unwrap();
        // Six interior rows: the lower middle is screen row five. At the end,
        // moving up reaches the middle before the viewport starts scrolling.
        for (selected, top, cursor) in [
            (0, 0, 2),
            (3, 0, 5),
            (10, 7, 5),
            (11, 8, 5),
            (10, 7, 5),
            (last, last - 5, 7),
            (last - 1, last - 5, 6),
            (last - 2, last - 5, 5),
            (last - 3, last - 6, 5),
        ] {
            browser.folder = rows[selected].filter.clone();
            terminal
                .draw(|frame| draw_pane(frame, frame.area(), 0, &browser, &view))
                .unwrap();
            let buffer = terminal.backend().buffer();
            assert_eq!(buffer[(1, cursor)].symbol(), ">");
            let first: String = (0..160).map(|x| buffer[(x, 2)].symbol()).collect();
            assert!(first.contains(&rows[top].label), "{first}");
            assert_eq!(browser.folder, rows[selected].filter);
        }
        // Resize the same selection, including zero/one usable row.
        browser.folder = rows[10].filter.clone();
        for (height, cursor) in [
            (8, Some(4)),
            (13, Some(7)),
            (5, Some(3)),
            (4, Some(2)),
            (3, None),
            (9, Some(5)),
        ] {
            terminal.backend_mut().resize(160, height);
            terminal.resize(Rect::new(0, 0, 160, height)).unwrap();
            terminal
                .draw(|frame| draw_pane(frame, frame.area(), 0, &browser, &view))
                .unwrap();
            if let Some(y) = cursor {
                assert_eq!(terminal.backend().buffer()[(1, y)].symbol(), ">");
            }
            assert_eq!(browser.folder, rows[10].filter);
        }
        // Collapsing the selected ancestor shortens the tree; expanding it
        // restores centering without retaining an obsolete viewport offset.
        if let FolderFilter::Path(path) = &browser.folder {
            browser.expanded.remove(path);
        }
        terminal
            .draw(|frame| draw_pane(frame, frame.area(), 0, &browser, &view))
            .unwrap();
        assert_eq!(terminal.backend().buffer()[(1, 7)].symbol(), ">");
        if let FolderFilter::Path(path) = &browser.folder {
            browser.expanded.insert(path.clone());
        }
        terminal
            .draw(|frame| draw_pane(frame, frame.area(), 0, &browser, &view))
            .unwrap();
        assert_eq!(terminal.backend().buffer()[(1, 5)].symbol(), ">");
        // A short list fits without artificial empty space above its rows.
        browser.expanded.clear();
        browser.folder = FolderFilter::Unlinked;
        terminal
            .draw(|frame| draw_pane(frame, frame.area(), 0, &browser, &view))
            .unwrap();
        assert_eq!(terminal.backend().buffer()[(1, 2)].symbol(), ">");
    }

    #[test]
    fn folder_layout_stays_stable_across_expansion_selection_and_viewport() {
        let mut browser = Browser::new(vec![]);
        let path = std::path::PathBuf::from(format!("/parent/{}", "long-folder-".repeat(8)));
        browser.open_folder(path.clone());
        let mut view = View {
            focus: 0,
            ..View::default()
        };
        let divider = |browser: &Browser, view: &View, width, height| {
            let mut terminal =
                Terminal::new(ratatui::backend::TestBackend::new(width, height)).unwrap();
            terminal.draw(|frame| draw(frame, browser, view)).unwrap();
            (0..width)
                .find(|&x| terminal.backend().buffer()[(x, 0)].symbol() == "┓")
                .unwrap()
                + 1
        };
        assert_eq!(divider(&browser, &view, 99, 24), 99);
        assert_eq!(divider(&browser, &view, 100, 24), 100);
        assert_eq!(divider(&browser, &view, 160, 24), 26);
        browser.folder = FolderFilter::Unlinked;
        assert_eq!(divider(&browser, &view, 160, 6), 160);
        browser.expanded.remove(std::path::Path::new("/parent"));
        let collapsed = divider(&browser, &view, 160, 24);
        assert_eq!(collapsed, 26);
        browser.expanded.insert("/parent".into());
        assert_eq!(divider(&browser, &view, 160, 24), 26);
        view.focus = 1;
        let mut terminal = Terminal::new(ratatui::backend::TestBackend::new(99, 20)).unwrap();
        terminal.draw(|frame| draw(frame, &browser, &view)).unwrap();
        assert_eq!(terminal.backend().buffer()[(98, 0)].symbol(), "┓");
    }

    #[test]
    fn availability_labels_and_relink_gate_follow_live_updates() {
        use crate::{
            browser::{SourceSpec, Update},
            config::SourceConfig,
            domain::*,
        };
        use std::collections::BTreeMap;
        let tmp = tempfile::tempdir().unwrap();
        let source = SourceConfig {
            id: SourceId("test".into()),
            home: tmp.path().to_owned(),
        };
        let mut browser = Browser::new(vec![SourceSpec::Codex {
            binary: "codex".into(),
            source,
        }]);
        browser.state_path = Some(tmp.path().join("state.json"));
        let session: SessionSummary = serde_json::from_value(serde_json::json!({
            "key": {"provider":"codex","source_id":"test","native_id":"one"},
            "title":"Example session", "cwd":tmp.path().join("missing"),
            "created_at":1,"updated_at":2,"archived":false,"source_kind":null,"git":null
        }))
        .unwrap();
        browser.begin_refresh();
        browser.apply(Update::Page {
            generation: 1,
            source: 0,
            sessions: vec![session.clone()],
            warnings: vec![],
        });
        browser.apply(Update::Finished {
            generation: 1,
            source: 0,
            error: None,
        });
        browser.selected = Some(session.key.clone());
        for (status, label) in [
            (RuntimeStatus::Running, "Running"),
            (RuntimeStatus::Idle, "Idle"),
            (RuntimeStatus::Unloaded, "Unloaded"),
            (RuntimeStatus::Unknown, "Status unknown"),
        ] {
            let statuses = if status == RuntimeStatus::Unloaded || status == RuntimeStatus::Unknown
            {
                BTreeMap::new()
            } else {
                BTreeMap::from([(session.key.clone(), status)])
            };
            browser.apply(Update::Live {
                generation: 1,
                source: 0,
                statuses,
                error: (status == RuntimeStatus::Unknown).then(|| "offline".into()),
            });
            assert_eq!(browser.runtime_status(&session.key), status);
            for width in [18, 80] {
                let line = session_title(&session, status, width);
                let text: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
                assert!(text.contains(label), "{text}");
                assert_eq!(
                    line.spans.last().unwrap().style.fg,
                    Some(activity_color(status).unwrap_or(Color::Reset))
                );
            }
            let reason = relink_unavailable(&browser);
            match status {
                RuntimeStatus::Idle => assert!(reason.is_none()),
                RuntimeStatus::Running => assert!(reason.unwrap().contains("running in Codex")),
                _ => assert!(reason.is_none(), "{reason:?}"),
            }
            if matches!(status, RuntimeStatus::Idle | RuntimeStatus::Unloaded) {
                assert_eq!(
                    activity_color(browser.folder_activity(&crate::browser::FolderFilter::All)),
                    if status == RuntimeStatus::Idle {
                        Some(Color::Rgb(255, 255, 170))
                    } else {
                        None
                    }
                );
            }
        }
    }

    // Invoked only by the parent test in a fresh controlling terminal.
    #[test]
    #[ignore]
    fn guard_child() {
        let result = std::panic::catch_unwind(|| -> io::Result<()> {
            let mut guard = TerminalGuard {
                active: false,
                mouse: false,
            };
            guard.activate()?;
            guard.enhanced(true)?;
            guard.suspend()?;
            guard.activate()?;
            guard.enhanced(true)?;
            if std::env::var("TRACR_TEST_CLEANUP").as_deref() == Ok("panic") {
                panic!("simulated failure after terminal activation");
            }
            Err(io::Error::other("simulated setup failure after activation"))
        });
        assert!(!matches!(result, Ok(Ok(()))));
    }

    #[test]
    fn setup_error_and_panic_restore_terminal() {
        let status = std::process::Command::new("python3")
            .arg(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/tests/fixtures/app-server/guard_check.py"
            ))
            .arg(std::env::current_exe().unwrap())
            .status()
            .unwrap();
        assert!(status.success());
    }
}

#[cfg(test)]
mod folder_viewport_tests {
    fn folder_viewport(text: &str, offset: usize, width: usize) -> String {
        super::graph_viewport(
            vec![ratatui::text::Span::raw(text.to_owned())],
            offset,
            width,
        )
        .to_string()
    }

    #[test]
    fn folder_connectors_stay_white_while_text_keeps_activity_color() {
        use crate::{
            browser::{FolderFilter, FolderRow},
            domain::RuntimeStatus,
        };
        use ratatui::style::Color;
        let row = FolderRow {
            filter: FolderFilter::Path("/project".into()),
            label: "◇ project *".into(),
            depth: 2,
            count: 1,
        };
        for status in [
            RuntimeStatus::Running,
            RuntimeStatus::Idle,
            RuntimeStatus::Waiting {
                approval: true,
                input: false,
            },
        ] {
            for offset in [0, 1, 3, 4, 6] {
                let line =
                    super::folder_tree_line(&row, "│  └─ ◇ project * (1)", status, offset, 40);
                for span in line.spans {
                    if span.content.contains(['│', '└', '─']) {
                        assert_eq!(span.style.fg, Some(Color::White));
                    } else if span.content == "*" {
                        assert_eq!(span.style.fg, Some(crate::git_render::DIRTY_COLOR));
                    } else if span.content == "◇" {
                        assert_eq!(span.style.fg, Some(crate::git_render::WORKTREE_COLOR));
                    } else if span.content.chars().any(char::is_alphabetic) {
                        assert_eq!(span.style.fg, super::activity_color(status));
                    }
                }
            }
        }
    }

    #[test]
    fn folder_branches_follow_displayed_hierarchy_and_collapsed_siblings() {
        use crate::browser::{FolderFilter, FolderRow};
        let row = |label: &str, depth| FolderRow {
            label: label.into(),
            depth,
            count: 0,
            filter: FolderFilter::Path(std::path::PathBuf::from(label)),
        };
        // The linked worktree's filesystem path is irrelevant to this hierarchy.
        let mut rows = vec![
            row("/", 0),
            row("project", 1),
            row("feature", 2),
            row("src", 3),
            row("other", 1),
            row("child", 2),
        ];
        let labels = super::folder_tree_labels(&rows);
        assert_eq!(
            labels,
            [
                "/ (0)",
                "├─ project (0)",
                "│  └─ feature (0)",
                "│     └─ src (0)",
                "└─ other (0)",
                "   └─ child (0)"
            ]
        );
        // Clipping before the later sibling must not turn project into a corner.
        assert_eq!(&labels[1..3], ["├─ project (0)", "│  └─ feature (0)"]);
        rows.drain(2..4);
        assert_eq!(
            super::folder_tree_labels(&rows),
            ["/ (0)", "├─ project (0)", "└─ other (0)", "   └─ child (0)"]
        );
    }

    #[test]
    fn clips_columns_without_splitting_graphemes() {
        assert_eq!(folder_viewport("ab界cd", 3, 3), " cd");
        assert_eq!(folder_viewport("ab界cd", 0, 3), "ab ");
        assert_eq!(folder_viewport("ae\u{301}z", 1, 1), "e\u{301}");
        assert_eq!(folder_viewport("a👩‍💻z", 2, 2), " z");
        assert_eq!(folder_viewport("short", 10, 4), "");
        assert_eq!(folder_viewport("short", 0, 0), "");
    }
}

#[cfg(test)]
mod graph_viewport_tests {
    use super::*;
    #[test]
    fn styled_unicode_clips_by_columns() {
        let line = graph_viewport(
            vec![
                Span::styled("ab界", Style::default().fg(Color::Cyan)),
                Span::styled("éz", Style::default().fg(Color::Yellow)),
            ],
            3,
            3,
        );
        assert_eq!(
            line.spans
                .iter()
                .map(|s| s.content.as_ref())
                .collect::<String>(),
            " éz"
        );
        assert_eq!(line.spans[0].style.fg, Some(Color::Cyan));
        assert_eq!(line.spans[1].style.fg, Some(Color::Yellow));
    }
    #[test]
    fn graph_styles_and_fixed_selection_survive_horizontal_clipping() {
        use ratatui::{Terminal, backend::TestBackend};
        let mut terminal = Terminal::new(TestBackend::new(40, 8)).unwrap();
        let spans = vec![
            Span::styled("| * ", Style::default().fg(Color::Cyan)),
            Span::raw("abcdef0 "),
            Span::styled(
                "[main] ",
                Style::default().fg(crate::git_render::BRANCH_COLOR),
            ),
            Span::styled("[remote: origin/main] ", Style::default().fg(Color::Cyan)),
            Span::styled("[tag: v1] ", Style::default().fg(Color::Yellow)),
            Span::raw("long subject 界界"),
        ];
        for offset in [0, 4, 20] {
            terminal
                .draw(|frame| {
                    frame.render_stateful_widget(
                        List::new(vec![ListItem::new(graph_viewport(
                            spans.clone(),
                            offset,
                            36,
                        ))])
                        .block(block("Git history", true))
                        .highlight_symbol("> ")
                        .highlight_style(Style::default().add_modifier(Modifier::BOLD)),
                        frame.area(),
                        &mut ListState::default().with_selected(Some(0)),
                    )
                })
                .unwrap();
            let buffer = terminal.backend().buffer();
            assert_eq!(buffer[(1, 1)].symbol(), ">");
            assert_eq!(buffer[(39, 1)].symbol(), "┃");
            assert!(buffer.content.iter().any(|cell| cell.fg == Color::Cyan));
            if offset == 0 {
                assert!(
                    buffer
                        .content
                        .iter()
                        .any(|cell| cell.symbol() == "m"
                            && cell.fg == crate::git_render::BRANCH_COLOR)
                );
            }
            if offset == 20 {
                assert!(
                    buffer
                        .content
                        .iter()
                        .any(|cell| cell.symbol() == "t" && cell.fg == Color::Yellow)
                );
            }
        }
    }
}

#[cfg(test)]
mod idle_tests {
    use super::*;
    #[test]
    fn idle_gate_does_no_repeated_work_and_wakes_for_changes_or_deadlines() {
        let now = std::time::Instant::now();
        let mut gate = IdleGate::default();
        gate.wake();
        assert!(gate.ready(now, false));
        for _ in 0..10_000 {
            assert!(!gate.ready(now, false));
        }
        assert!(gate.ready(now, true));
        assert!(!gate.ready(now, false));
        gate.deadline = Some(now + Duration::from_secs(60));
        assert!(!gate.ready(now + Duration::from_secs(59), false));
        assert!(gate.ready(now + Duration::from_secs(60), false));
        assert!(!gate.ready(now + Duration::from_secs(61), false));
        gate.wake();
        assert!(gate.ready(now, false));
    }
    #[test]
    fn watch_failure_remains_visible_alongside_transient_messages() {
        let browser = Browser::new(vec![]);
        let view = View {
            git_watch_error: Some("native notifications unavailable".into()),
            message: Some("Session returned".into()),
            ..View::default()
        };
        let (_, _, message, _) = browser_footer(Rect::new(0, 0, 100, 30), &browser, &view);
        assert!(message.contains("Git auto-update unavailable — r to refresh"));
        assert!(message.contains("native notifications unavailable"));
        assert!(message.contains("Session returned"));
    }
    #[test]
    fn age_deadlines_track_displayed_text_boundaries() {
        for elapsed in [
            0, 1, 59, 60, 61, 3599, 3600, 86399, 86400, 604_799, 604_800, 2_591_999, 2_592_000,
            31_104_000, 31_535_999, 31_536_000,
        ] {
            let delay = age_delay(100, 100 + elapsed);
            let current = relative_time(100, 100 + elapsed);
            assert_eq!(
                relative_time(100, 100 + elapsed + delay as i64 - 1),
                current
            );
            assert_ne!(relative_time(100, 100 + elapsed + delay as i64), current);
        }
        assert_eq!(age_delay(i64::MAX, 0), 86_400);
    }
}

#[cfg(test)]
mod empty_sessions_tests {
    use super::*;
    use crate::{browser::SourceSpec, config::SourceConfig, domain::SourceId};
    use ratatui::backend::TestBackend;

    #[test]
    fn empty_sessions_top_align_and_keep_status_when_space_is_limited() {
        let mut browser = Browser::new(vec![SourceSpec::Codex {
            binary: "codex".into(),
            source: SourceConfig {
                id: SourceId("empty-test".into()),
                home: "/unused".into(),
            },
        }]);
        browser.folder = FolderFilter::Path("/project".into());
        let render = |browser: &Browser, width, height| {
            let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
            terminal
                .draw(|frame| draw_empty_sessions(frame, frame.area(), browser, true))
                .unwrap();
            (0..height)
                .map(|y| {
                    (0..width)
                        .map(|x| terminal.backend().buffer()[(x, y)].symbol())
                        .collect::<String>()
                })
                .collect::<Vec<_>>()
        };
        let lines = render(&browser, 60, 20);
        assert!(lines[5].contains("No sessions currently shown"));
        assert!(lines[7].contains("Create new session"));
        assert!(lines[8].contains("Press Enter"));
        for height in [14, 30, 50] {
            let resized = render(&browser, 60, height);
            assert!(resized[5].contains("No sessions currently shown"));
            for row in &resized[1..5] {
                assert!(row.chars().skip(1).take(58).all(|c| c == ' '));
            }
        }
        let status = &lines[5];
        let left = status.chars().position(|c| c == 'N').unwrap();
        let right = 59 - (left + "No sessions currently shown".len());
        assert!(left.abs_diff(right + 1) <= 1);
        assert!(
            lines
                .iter()
                .any(|line| line.contains("Show archived sessions"))
        );
        browser.include_archived = true;
        assert!(
            !render(&browser, 60, 20)
                .join("\n")
                .contains("Show archived")
        );
        let short = render(&browser, 60, 6).join("\n");
        assert!(short.contains("No sessions currently shown"));
        assert!(short.contains("Press Enter"));
        assert!(!short.contains("Search all sessions"));
        browser.begin_refresh();
        let loading = render(&browser, 60, 20).join("\n");
        assert!(loading.contains("Loading sessions"));
        assert!(!loading.contains("Create new session"));
        for (width, height) in [(20, 12), (8, 4), (1, 1)] {
            render(&browser, width, height);
        }
    }
}
