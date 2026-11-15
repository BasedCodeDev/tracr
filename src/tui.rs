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
        "The folder may already have changed. Close this dialog and press R to inspect/recover the operation."
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
            .unwrap_or_else(|| std::path::Path::new(&modal.input).join(&modal.name));
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
            "Files may already be at the destination. Close this dialog and press R to inspect and finish the move."
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
#[derive(Default)]
pub struct View {
    pub focus: usize,
    pub editing: bool,
    pub search_query: String,
    pub search_selected: Option<crate::domain::SessionKey>,
    pub help: bool,
    pub help_selected: usize,
    pub help_page_size: usize,
    pub details_scroll: u16,
    pub message: Option<String>,
    pub preparing: bool,
    pub source_picker: Option<usize>,
    pub relink: Option<RelinkModal>,
    pub folder_move: Option<crate::folder_move_ui::Modal>,
    pub recovery_index: usize,
    pub error_dialog: Option<crate::error_dialog::ErrorDialog>,
}
fn restore() {
    let _ = terminal::disable_raw_mode();
    let _ = execute!(
        io::stdout(),
        event::DisableBracketedPaste,
        LeaveAlternateScreen,
        crossterm::cursor::Show
    );
}
// A direct flag is observed by the UI loop even if the async signal driver stalls.
struct ShutdownSignals {
    terminate: std::sync::Arc<std::sync::atomic::AtomicBool>,
    interrupt: std::sync::Arc<std::sync::atomic::AtomicBool>,
    registrations: Vec<signal_hook::SigId>,
}
impl ShutdownSignals {
    fn new() -> io::Result<Self> {
        let mut guard = Self {
            terminate: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
            interrupt: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
            registrations: Vec::new(),
        };
        for (signal, flag) in [
            (signal_hook::consts::SIGTERM, guard.terminate.clone()),
            (signal_hook::consts::SIGINT, guard.interrupt.clone()),
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
}
impl TerminalGuard {
    fn activate(&mut self) -> io::Result<()> {
        terminal::enable_raw_mode()?;
        self.active = true;
        execute!(
            io::stdout(),
            EnterAlternateScreen,
            event::EnableBracketedPaste
        )
    }
    fn suspend(&mut self) -> io::Result<()> {
        terminal::disable_raw_mode()?;
        execute!(
            io::stdout(),
            event::DisableBracketedPaste,
            LeaveAlternateScreen,
            crossterm::cursor::Show
        )?;
        self.active = false;
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

fn footer_shortcuts(width: u16) -> (&'static str, &'static str) {
    const FULL: &str = "←/→ panes · Enter toggle/resume · m move folder · l relink · R recover · / search · a archives · r refresh · ? help · q quit";
    if unicode_width::UnicodeWidthStr::width(FULL) <= usize::from(width) {
        (FULL, "")
    } else if width >= 60 {
        (
            "l relink · R recover · m move folder · ←/→ panes",
            "/ search · a archives · r refresh · ? help · q quit",
        )
    } else if width >= 30 {
        ("l relink · R recover", "/ search · r refresh · ? help")
    } else if width >= 20 {
        ("l relink · R recover", "? help · q quit")
    } else {
        ("l relink", "R recover")
    }
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

pub fn draw(frame: &mut Frame<'_>, browser: &Browser, view: &View) {
    if let Some(dialog) = &view.error_dialog {
        dialog.draw(frame);
        return;
    }
    if let Some(modal) = &view.folder_move {
        modal.draw(frame);
        return;
    }
    let area = frame.area();
    if area.width < 12 || area.height < 4 {
        frame.render_widget(Paragraph::new("tracr: enlarge terminal"), area);
        return;
    }
    let layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(1), Constraint::Length(3)])
        .split(area);
    if area.width >= 100 {
        let panes = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([
                Constraint::Percentage(25),
                Constraint::Percentage(40),
                Constraint::Percentage(35),
            ])
            .split(layout[0]);
        for (index, pane) in panes.iter().enumerate() {
            draw_pane(frame, *pane, index, browser, view);
        }
    } else {
        draw_pane(frame, layout[0], view.focus, browser, view);
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
    let (first, second) = footer_shortcuts(area.width);
    // Explicit rows keep messages separate and prevent wrapped hints hiding l/R.
    frame.render_widget(
        Paragraph::new(vec![
            Line::from(first),
            Line::from(second),
            Line::from(message),
        ]),
        layout[1],
    );
    if view.help {
        draw_help(frame, browser, view);
    }
    if let Some(selected) = view.source_picker {
        let popup = Rect::new(
            area.x + 1,
            area.y + 1,
            area.width.saturating_sub(2),
            area.height.saturating_sub(2).min(12),
        );
        frame.render_widget(Clear, popup);
        let items: Vec<_> = creation_sources(browser)
            .iter()
            .map(|&i| ListItem::new(safe_text(&browser.specs[i].label())))
            .collect();
        frame.render_stateful_widget(
            List::new(items)
                .block(block(
                    "Codex home — ↑/↓ select · Enter creates · Esc cancels",
                    true,
                ))
                .highlight_symbol("> ")
                .highlight_style(Style::default().add_modifier(Modifier::BOLD)),
            popup,
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
        let area = frame.area();
        let width = area.width.saturating_sub(2).min(76);
        let height = area.height.saturating_sub(2).min(21);
        let popup = Rect::new(
            area.x + (area.width - width) / 2,
            area.y + (area.height - height) / 2,
            width,
            height,
        );
        frame.render_widget(Clear, popup);
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
                "Permissions: {}\nOther session settings are preserved.\nOnly this session changes folders. Project files stay where they are.\nDetached idle sessions are released automatically after confirmation.\n\n",
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
        let outer = block(
            if modal.recovery {
                "Inspect/recover relocation"
            } else {
                "Relink session"
            },
            true,
        );
        let inner = outer.inner(popup);
        frame.render_widget(outer, popup);
        let sections = Layout::vertical([Constraint::Min(1), Constraint::Length(1)]).split(inner);
        frame.render_widget(Paragraph::new(text).wrap(Wrap { trim: false }), sections[0]);
        frame.render_widget(Paragraph::new(controls), sections[1]);
    }
}

fn draw_path_picker(frame: &mut Frame<'_>, modal: &RelinkModal) {
    render_picker(
        frame,
        modal.picker.as_ref().expect("path picker"),
        &modal.input,
        PickerText {
            title: "Relink session",
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
}
pub(crate) fn render_picker(
    frame: &mut Frame<'_>,
    picker: &crate::path_picker::Picker,
    input: &str,
    text: PickerText<'_>,
) {
    let area = frame.area();
    let width = area.width.saturating_sub(2).min(84);
    let height = area.height.saturating_sub(2).min(28);
    let popup = Rect::new(
        area.x + (area.width - width) / 2,
        area.y + (area.height - height) / 2,
        width,
        height,
    );
    frame.render_widget(Clear, popup);
    let outer = block(text.title, true);
    let inner = outer.inner(popup);
    frame.render_widget(outer, popup);
    if inner.height < 10 {
        frame.render_widget(
            Paragraph::new("Enlarge terminal to choose a folder. Esc cancels."),
            inner,
        );
        return;
    }
    let parts = Layout::vertical([
        Constraint::Length(2),
        Constraint::Length(3),
        Constraint::Min(1),
        Constraint::Length(1),
        Constraint::Length(2),
    ])
    .split(inner);
    frame.render_widget(
        Paragraph::new(vec![
            Line::from(safe_text(text.heading)),
            Line::from(format!("Previous folder: {}", safe_text(text.old))),
        ]),
        parts[0],
    );
    // Keep the caret visible in long paths without altering the input value.
    let caret = picker.caret.min(input.len());
    let available = usize::from(parts[1].width.saturating_sub(3));
    let mut start = 0;
    while unicode_width::UnicodeWidthStr::width(&input[start..caret]) >= available.max(1)
        && start < caret
    {
        start += input[start..].chars().next().unwrap().len_utf8();
    }
    let rendered_input = safe_text(&input[start..]);
    frame.render_widget(
        Paragraph::new(rendered_input).block(block(text.field, picker.focus == 0)),
        parts[1],
    );
    if picker.focus == 0 && !text.busy && parts[1].width > 2 {
        let offset = unicode_width::UnicodeWidthStr::width(&input[start..caret]) as u16;
        frame.set_cursor_position((parts[1].x + 1 + offset, parts[1].y + 1));
    }
    let items: Vec<_> = picker
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
    let selected = picker
        .rows
        .iter()
        .position(|r| Some(&r.path) == picker.selected.as_ref());
    frame.render_stateful_widget(
        List::new(items)
            .block(block("Folders", picker.focus == 1))
            .highlight_symbol("> ")
            .highlight_style(Style::default().add_modifier(Modifier::BOLD)),
        parts[2],
        &mut ListState::default().with_selected(selected),
    );
    let ready = picker.valid && !picker.loading && !text.busy;
    frame.render_widget(
        Paragraph::new(if picker.focus == 2 {
            "> [ Use this folder ]"
        } else {
            "  [ Use this folder ]"
        })
        .style(if ready {
            Style::default().add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::DarkGray)
        }),
        parts[3],
    );
    let hint = if picker.focus == 1 {
        "↑/↓ select · Enter expand/collapse"
    } else {
        "↑/↓ select · Enter uses folder"
    };
    let status = if text.busy {
        "Checking native state… Esc cancels".into()
    } else if let Some(error) = text.error.or(picker.error.as_deref()) {
        safe_text(error)
    } else if picker.loading {
        "Loading folders…".into()
    } else {
        "Tab switch controls · Esc cancel".into()
    };
    frame.render_widget(
        Paragraph::new(vec![Line::from(hint), Line::from(status)]),
        parts[4],
    );
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Command {
    PreviousPane,
    NextPane,
    PreviousItem,
    NextItem,
    ToggleFolder,
    Resume,
    Relink,
    MoveFolder,
    Recover,
    Create,
    Archives,
    Search,
    Refresh,
    Quit,
}
struct MenuEntry {
    command: Command,
    section: &'static str,
    shortcut: &'static str,
    label: &'static str,
}
fn menu_entries(include_archived: bool) -> Vec<MenuEntry> {
    use Command::*;
    [
        (
            PreviousPane,
            "Navigation",
            "← / Shift-Tab / Esc",
            "Previous pane",
        ),
        (NextPane, "Navigation", "→ / Tab", "Next pane"),
        (
            PreviousItem,
            "Navigation",
            "↑ / k",
            "Previous item / scroll up",
        ),
        (NextItem, "Navigation", "↓ / j", "Next item / scroll down"),
        (
            ToggleFolder,
            "Sessions",
            "Enter (Folders)",
            "Expand / collapse folder",
        ),
        (
            Resume,
            "Sessions",
            "Enter (Sessions)",
            "Resume selected session",
        ),
        (
            Create,
            "Sessions",
            "Enter (empty folder)",
            "Create new session",
        ),
        (
            Archives,
            "Sessions",
            "a",
            if include_archived {
                "Hide archived sessions"
            } else {
                "Show archived sessions"
            },
        ),
        (Relink, "Sessions", "l", "Relink session"),
        (MoveFolder, "Folders", "m", "Move folder"),
        (Recover, "Sessions", "R", "Inspect/recover relocation"),
        (Search, "Search", "/", "Search all sessions"),
        (Refresh, "General", "r", "Refresh sessions"),
        (Quit, "General", "q / Ctrl-C", "Quit tracr"),
    ]
    .into_iter()
    .map(|(command, section, shortcut, label)| MenuEntry {
        command,
        section,
        shortcut,
        label,
    })
    .collect()
}
fn unavailable(command: Command, browser: &Browser) -> Option<String> {
    match command {
        Command::ToggleFolder if !matches!(browser.folder, FolderFilter::Path(_)) => {
            Some("Select a concrete folder first.".into())
        }
        Command::Relink => relink_unavailable(browser),
        Command::MoveFolder => crate::folder_move_ui::unavailable(browser),
        Command::Create => {
            if browser.loading() {
                Some("Wait for discovery to finish.".into())
            } else if creation_sources(browser).is_empty() {
                Some("Fixture data is browse-only.".into())
            } else if !matches!(browser.folder, FolderFilter::Path(_)) {
                Some("Select a concrete folder first.".into())
            } else if !browser.visible().is_empty() {
                Some("Creation is available when no sessions are shown.".into())
            } else {
                None
            }
        }
        Command::Resume => {
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
    let area = frame.area();
    let width = area.width.saturating_sub(2).min(76);
    let height = area.height.saturating_sub(2).min(22);
    let popup = Rect::new(
        area.x + (area.width - width) / 2,
        area.y + (area.height - height) / 2,
        width,
        height,
    );
    frame.render_widget(Clear, popup);
    let border = block("Actions & shortcuts", true);
    let inner = border.inner(popup);
    frame.render_widget(border, popup);
    if inner.width < 26 || inner.height < 6 {
        frame.render_widget(
            Paragraph::new("Enlarge terminal. Esc closes.").wrap(Wrap { trim: false }),
            inner,
        );
        return;
    }
    let inner = Rect::new(
        inner.x + 1,
        inner.y,
        inner.width.saturating_sub(2),
        inner.height,
    );
    let sections = Layout::vertical([
        Constraint::Min(1),
        Constraint::Length(2),
        Constraint::Length(1),
    ])
    .split(inner);
    let entries = menu_entries(browser.include_archived);
    let selected = view.help_selected.min(entries.len() - 1);
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
        let disabled = unavailable(entry.command, browser).is_some();
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
            .highlight_style(Style::default().add_modifier(Modifier::REVERSED)),
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
    if let Some(reason) = unavailable(entries[selected].command, browser) {
        frame.render_widget(
            Paragraph::new(reason).wrap(Wrap { trim: false }),
            sections[1],
        );
    }
    frame.render_widget(
        Paragraph::new(if inner.width >= 43 {
            "↑/↓ select · Enter activate · Esc close"
        } else if inner.width >= 32 {
            "↑↓ select · Enter run · Esc close"
        } else {
            "↑↓ · Enter run · Esc close"
        }),
        sections[2],
    );
}

fn activity_color(status: crate::domain::RuntimeStatus) -> Option<Color> {
    match status {
        crate::domain::RuntimeStatus::Running => Some(Color::Green),
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
    let width = area.width.saturating_sub(2).min(100);
    let height = area.height.saturating_sub(2).min(30);
    let popup = Rect::new(
        area.x + (area.width - width) / 2,
        area.y + (area.height - height) / 2,
        width,
        height,
    );
    frame.render_widget(Clear, popup);
    let sections = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(3), Constraint::Min(1)])
        .split(popup);
    let query = safe_text(&view.search_query);
    let query_width = Line::from(query.clone()).width();
    let input_width = sections[0].width.saturating_sub(2) as usize;
    let offset = query_width.saturating_sub(input_width.saturating_sub(1));
    frame.render_widget(
        Paragraph::new(query)
            .scroll((0, offset.min(u16::MAX as usize) as u16))
            .block(block("Search sessions — Esc cancels", true)),
        sections[0],
    );
    if input_width > 0 && sections[0].height >= 3 {
        frame.set_cursor_position((
            sections[0].x + 1 + query_width.saturating_sub(offset) as u16,
            sections[0].y + 1,
        ));
    }
    let results = browser.search_results(&view.search_query);
    if results.is_empty() {
        frame.render_widget(
            Paragraph::new(if browser.loading() {
                "Searching…"
            } else {
                "No matching sessions"
            })
            .block(block("Results", false)),
            sections[1],
        );
        return;
    }
    let index = results
        .iter()
        .position(|s| Some(&s.key) == view.search_selected.as_ref())
        .unwrap_or(0);
    let items: Vec<_> = results
        .iter()
        .map(|s| {
            ListItem::new(vec![
                session_title(
                    s,
                    browser.runtime_status(&s.key),
                    sections[1].width.saturating_sub(4),
                ),
                Line::from(safe_text(
                    &s.cwd
                        .as_ref()
                        .map(|p| p.display().to_string())
                        .unwrap_or_else(|| "Unknown folder".into()),
                )),
                Line::default(),
            ])
        })
        .collect();
    frame.render_stateful_widget(
        List::new(items)
            .block(block("Results — ↑/↓ select · Enter opens folder", false))
            .highlight_style(Style::default().add_modifier(Modifier::BOLD))
            .highlight_symbol("> "),
        sections[1],
        &mut ListState::default().with_selected(Some(index)),
    );
}

fn draw_pane(frame: &mut Frame<'_>, area: Rect, pane: usize, browser: &Browser, view: &View) {
    match pane {
        0 => {
            let folders = browser.folder_rows();
            let items: Vec<_> = folders
                .iter()
                .map(|row| {
                    ListItem::new(format!(
                        "{}{} ({})",
                        "  ".repeat(row.depth.min(20)),
                        safe_text(&row.label),
                        row.count
                    ))
                    .style(
                        activity_color(browser.folder_activity(&row.filter))
                            .map_or(Style::default(), |color| Style::default().fg(color)),
                    )
                })
                .collect();
            let mut state = ListState::default()
                .with_selected(folders.iter().position(|r| r.filter == browser.folder));
            frame.render_stateful_widget(
                List::new(items)
                    .block(block("Folders", view.focus == 0))
                    .highlight_style(Style::default().add_modifier(Modifier::BOLD))
                    .highlight_symbol("> "),
                area,
                &mut state,
            );
        }
        1 => {
            let sessions = browser.visible();
            let now = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs()
                .min(i64::MAX as u64) as i64;
            if sessions.is_empty() {
                let message = if browser.loading() {
                    "Loading sessions…"
                } else if browser.diagnostics().iter().any(|source| !source.complete) {
                    "No matching sessions found; inventory is incomplete. Check source errors below."
                } else {
                    "No matching sessions. Press a to include archives, / to search globally, or Tab to choose another folder."
                };
                let message = if can_create(browser) {
                    format!(
                        "No sessions currently shown. Press a to include archives.\n\n{}Create new session — Enter",
                        if view.focus == 1 { "> " } else { "" }
                    )
                } else {
                    message.to_owned()
                };
                frame.render_widget(
                    Paragraph::new(message)
                        .wrap(Wrap { trim: false })
                        .block(block("Sessions", view.focus == 1)),
                    area,
                );
                return;
            }
            let items: Vec<_> = sessions
                .iter()
                .map(|s| {
                    ListItem::new(vec![
                        session_title(
                            s,
                            browser.runtime_status(&s.key),
                            area.width.saturating_sub(4),
                        ),
                        session_metadata(browser, s, now),
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
                    .block(block("Sessions", view.focus == 1))
                    .highlight_style(Style::default().add_modifier(Modifier::BOLD))
                    .highlight_symbol("> "),
                area,
                &mut state,
            );
        }
        _ => {
            let selected = browser
                .all()
                .into_iter()
                .find(|s| Some(&s.key) == browser.selected.as_ref());
            let text = if let Some(s) = selected {
                let owner = browser
                    .sources
                    .iter()
                    .zip(&browser.specs)
                    .find(|(_, spec)| match spec {
                        crate::browser::SourceSpec::Codex { source, .. } => {
                            source.id == s.key.source_id
                        }
                        crate::browser::SourceSpec::Fixture(_) => true,
                    })
                    .map(|(_, spec)| spec.label())
                    .unwrap_or_default();
                let path = s
                    .cwd
                    .as_ref()
                    .map(|p| p.display().to_string())
                    .unwrap_or_else(|| "unknown".into());
                let status = s.cwd.as_ref().and_then(|p| browser.folders.get(p));
                let extra = match browser.details.get(&s.key) {
                    Some(Ok(Some(detail))) if detail.cwd != s.cwd => format!(
                        "\nDIRECTORY MISMATCH\nListed folder: {path}\nResume folder: {}",
                        detail
                            .cwd
                            .as_ref()
                            .map(|p| p.display().to_string())
                            .unwrap_or_else(|| "unknown".into())
                    ),
                    Some(Err(error)) => format!("\nDetails error: {error}"),
                    Some(Ok(None)) => "\nSession no longer available".into(),
                    Some(Ok(Some(_))) => String::new(),
                    None => "\nFetching metadata…".into(),
                };
                let runtime = browser.runtime_status(&s.key);
                let availability = match runtime {
                    crate::domain::RuntimeStatus::Idle => {
                        "Codex has this session cached. Relinking will try to release it after confirmation."
                    }
                    crate::domain::RuntimeStatus::Unloaded => {
                        "Released at the latest status check. Relinking can be attempted subject to folder and ownership checks."
                    }
                    crate::domain::RuntimeStatus::Unknown => {
                        "Availability could not be confirmed. Relinking checks native ownership before changing the folder."
                    }
                    _ => {
                        "Codex reports this session as loaded; relinking must wait until it is released."
                    }
                };
                let extra = format!(
                    "{extra}\n\nStatus: {}\n{availability}\n\nResume: {}",
                    runtime.label().unwrap_or("Status unknown"),
                    resume_hint(browser, &s)
                );
                format!(
                    "{}\n\nID: {}\nProvider: {}\nSource home: {}\nFolder: {}\nFolder status: {:?}\nCreated: {}\nLast activity: {}\nSource: {}\nArchived: {}\nGit: {:?}{}",
                    s.title,
                    s.key.native_id,
                    s.key.provider.0,
                    owner,
                    path,
                    status,
                    s.created_at,
                    s.updated_at,
                    s.source_kind.as_deref().unwrap_or("unknown"),
                    s.archived,
                    s.git,
                    extra
                )
            } else {
                "No session selected".into()
            };
            frame.render_widget(
                Paragraph::new(
                    text.split('\n')
                        .map(safe_text)
                        .collect::<Vec<_>>()
                        .join("\n"),
                )
                .block(block("Details", view.focus == 2))
                .wrap(Wrap { trim: false })
                .scroll((view.details_scroll, 0)),
                area,
            );
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum Action {
    CopyErrorReport,
    Continue,
    OpenRelink,
    OpenFolderMove,
    PreviewFolderMove,
    ApplyFolderMove,
    CloseFolderMove,
    CancelRelink,
    PreviewRelink,
    ApplyRelink,
    RecoverRelink,
    Quit,
    Refresh,
    ResumeSelected,
    CancelResume,
    CreateNew(usize),
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
    Preview(Result<crate::relocation::Preview, crate::relocation::Failure>),
    Applied(crate::relocation::Outcome),
}
fn creation_sources(browser: &Browser) -> Vec<usize> {
    browser
        .specs
        .iter()
        .enumerate()
        .filter_map(|(i, spec)| {
            matches!(spec, crate::browser::SourceSpec::Codex { .. }).then_some(i)
        })
        .collect()
}
fn can_create(browser: &Browser) -> bool {
    !browser.loading()
        && matches!(browser.folder, FolderFilter::Path(_))
        && browser.visible().is_empty()
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
            .unwrap_or_else(|| "Folder missing — Enter or l to relink.".into());
    }
    "Enter to resume in this terminal; the destination is checked again before launch.".into()
}

pub fn handle_key(browser: &mut Browser, view: &mut View, key: KeyEvent) -> Action {
    if key.kind == KeyEventKind::Release {
        return Action::Continue;
    }
    if let Some(dialog) = view.error_dialog.as_mut() {
        match dialog.key(key) {
            crate::error_dialog::Event::Copy => return Action::CopyErrorReport,
            crate::error_dialog::Event::Close => view.error_dialog = None,
            crate::error_dialog::Event::None => {}
        }
        return Action::Continue;
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
            KeyCode::Char('R') if modal.recovery => return Action::RecoverRelink,
            KeyCode::Enter => {
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
        let sources = creation_sources(browser);
        match key.code {
            KeyCode::Esc => view.source_picker = None,
            KeyCode::Up => *selected = selected.saturating_sub(1),
            KeyCode::Down => *selected = (*selected + 1).min(sources.len().saturating_sub(1)),
            KeyCode::Enter => {
                let source = sources.get(*selected).copied();
                view.source_picker = None;
                if let Some(source) = source {
                    return Action::CreateNew(source);
                }
            }
            _ => {}
        }
        return Action::Continue;
    }
    if view.help {
        let entries = menu_entries(browser.include_archived);
        view.help_selected = view.help_selected.min(entries.len() - 1);
        let step = view.help_page_size.max(1);
        match key.code {
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
            KeyCode::Enter if key.kind != KeyEventKind::Repeat => {
                let command = entries[view.help_selected].command;
                if unavailable(command, browser).is_none() {
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
            KeyCode::Esc => view.editing = false,
            KeyCode::Up | KeyCode::Down => {
                let next = if key.code == KeyCode::Up {
                    index.saturating_sub(1)
                } else {
                    (index + 1).min(results.len().saturating_sub(1))
                };
                view.search_selected = results.get(next).map(|s| s.key.clone());
            }
            KeyCode::Enter => {
                if let Some(session) = results.get(index) {
                    browser.query.clear();
                    if let Some(path) = session.cwd.as_ref().filter(|path| path.is_absolute()) {
                        browser.open_folder(path.clone());
                    } else {
                        browser.folder = FolderFilter::Unknown;
                    }
                    browser.selected = Some(session.key.clone());
                    browser.reconcile(0);
                    view.focus = 1;
                    view.details_scroll = 0;
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
                if !key
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
    let command = match key.code {
        KeyCode::Char('?') => {
            view.help = true;
            view.help_selected = menu_entries(browser.include_archived)
                .iter()
                .position(|entry| unavailable(entry.command, browser).is_none())
                .unwrap_or(0);
            return Action::Continue;
        }
        KeyCode::Char('q') => Command::Quit,
        KeyCode::Char('/') => Command::Search,
        KeyCode::Char('r') => Command::Refresh,
        KeyCode::Char('l') if view.focus == 1 => Command::Relink,
        KeyCode::Char('m') if view.focus == 0 => Command::MoveFolder,
        KeyCode::Char('R') => Command::Recover,
        KeyCode::Char('a') => Command::Archives,
        KeyCode::Tab | KeyCode::Right => Command::NextPane,
        KeyCode::BackTab | KeyCode::Left | KeyCode::Esc => Command::PreviousPane,
        KeyCode::Up | KeyCode::Char('k') => Command::PreviousItem,
        KeyCode::Down | KeyCode::Char('j') => Command::NextItem,
        KeyCode::Enter if view.focus == 0 => Command::ToggleFolder,
        KeyCode::Enter if view.focus == 1 && can_create(browser) => Command::Create,
        KeyCode::Enter => Command::Resume,
        _ => return Action::Continue,
    };
    execute_command(command, browser, view)
}

fn execute_command(command: Command, browser: &mut Browser, view: &mut View) -> Action {
    match command {
        Command::Quit => return Action::Quit,
        Command::Relink => return Action::OpenRelink,
        Command::MoveFolder => return Action::OpenFolderMove,
        Command::Recover => return Action::RecoverRelink,
        Command::Search => {
            view.editing = true;
            view.search_query.clear();
            view.search_selected = None;
        }
        Command::Refresh => return Action::Refresh,
        Command::Archives => browser.include_archived = !browser.include_archived,
        Command::NextPane => view.focus = (view.focus + 1) % 3,
        Command::PreviousPane => {
            view.focus = (view.focus + 2) % 3;
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
                        browser.folder = row.filter.clone();
                    }
                }
                1 => browser.move_selection(delta),
                _ => view.details_scroll = view.details_scroll.saturating_add_signed(delta as i16),
            }
        }
        Command::ToggleFolder => {
            if let FolderFilter::Path(path) = &browser.folder {
                if !browser.expanded.remove(path) {
                    browser.expanded.insert(path.clone());
                }
            }
        }
        Command::Create => {
            view.focus = 1;
            let sources = creation_sources(browser);
            if sources.len() == 1 {
                return Action::CreateNew(sources[0]);
            }
            view.source_picker = Some(0);
        }
        Command::Resume => {
            if selected_session(browser).is_some_and(|s| {
                crate::domain::classify_folder(s.cwd.as_deref())
                    == crate::domain::FolderStatus::Unlinked
            }) {
                return Action::OpenRelink;
            }
            return Action::ResumeSelected;
        }
    }
    if view.focus != 2 {
        view.details_scroll = 0;
    }
    browser.reconcile(0);
    Action::Continue
}

struct PendingResume {
    task: tokio::task::JoinHandle<Result<crate::domain::ResumeCommand, String>>,
    observed_cwd: Option<std::path::PathBuf>,
}
impl Drop for PendingResume {
    fn drop(&mut self) {
        self.task.abort();
    }
}

fn save_folder(browser: &mut Browser, folder: Option<&std::path::Path>, view: &mut View) {
    if let (Some(file), Some(folder)) = (&browser.state_path, folder) {
        if let Err(error) = crate::state::save(file, folder) {
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
    let mut guard = TerminalGuard { active: false };
    guard.activate()?;
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        restore();
        previous(info);
    }));
    let mut terminal = Terminal::new(CrosstermBackend::new(io::stdout()))?;
    let mut discovery = Discovery::new();
    discovery.enable_live();
    discovery.refresh(&mut browser);
    let mut view = View {
        focus: usize::from(browser.initial_folder.is_some()),
        message: browser.state_warning.take(),
        ..View::default()
    };
    let signals = ShutdownSignals::new()?;
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
    if let Some(state) = &relocation_state {
        match crate::relocation::pending(state) {
            Ok(journals) if !journals.is_empty() => {
                view.message = Some(format!(
                    "{} unfinished relocation(s). Press R to inspect/recover.",
                    journals.len()
                ))
            }
            Err(error) => {
                view.message = Some(format!("Cannot inspect relocation journals: {error}"))
            }
            _ => {}
        }
    }
    if let Some(state) = &relocation_state {
        match crate::folder_move::pending(state) {
            Ok(Some(_)) => {
                view.message = Some("Unfinished folder move. Press R to inspect/recover.".into())
            }
            Err(e) => view.message = Some(format!("Cannot inspect folder-move journal: {e}")),
            _ => {}
        }
    }
    let mut relocated: Option<crate::relocation::Preview> = None;
    let mut last_folder = browser.initial_folder.clone();
    let mut created: Option<(
        crate::domain::SourceId,
        std::path::PathBuf,
        std::collections::BTreeSet<crate::domain::SessionKey>,
    )> = None;
    let outcome: Result<(), Box<dyn std::error::Error>> = async {
        loop {
            // Finish the bounded operation and archive restoration before exiting.
            if (signals.terminate.load(std::sync::atomic::Ordering::Relaxed)
                || signals.interrupt.load(std::sync::atomic::Ordering::Relaxed)) && relink_task.is_none() && folder_task.is_none() {
                break;
            }
            while let Ok(message)=relink_rx.try_recv() {
                if let Some(modal)=view.relink.as_mut(){modal.error=Some(message);}
            }
            while let Ok(message)=folder_rx.try_recv() {
                if let Some(modal)=view.folder_move.as_mut(){modal.message=Some(message);}
            }
            if let Some(modal)=view.folder_move.as_mut(){modal.tick();}
            if folder_task.as_ref().is_some_and(|t|t.is_finished()) {
                let result=folder_task.take().expect("finished folder move").await;
                while event::poll(Duration::ZERO)?{let _=event::read()?;}
                match result {
                    Ok(FolderMoveResult::Preview(Ok(plan)))=>{
                        if let Some(modal)=view.folder_move.as_mut(){modal.busy=false;modal.message=None;modal.plan=Some(plan);}
                    },
                    Ok(FolderMoveResult::Applied(crate::folder_move::Outcome::Complete(plan)))=>{
                        folder_relocated=Some((plan.target.clone(),browser.selected.clone()));
                        browser.open_folder(plan.target);view.folder_move=None;view.focus=0;
                        view.message=Some("Folder and sessions moved and verified.".into());discovery.refresh(&mut browser);
                    },
                    other=>{
                        let (message,recovery)=match other {
                            Ok(FolderMoveResult::Preview(Err(e)))=>(e,false),
                            Ok(FolderMoveResult::Applied(crate::folder_move::Outcome::Failed{message,recovery}))=>(message,recovery),
                            Err(e)=>(format!("Folder move stopped: {e}"),true),
                            _=>unreachable!(),
                        };
                        show_folder_failure(&mut view,message,recovery);
                        discovery.refresh(&mut browser);
                    }
                }
            }
            if let Some(dialog)=view.error_dialog.as_mut(){dialog.tick().await;}
            if relink_task.as_ref().is_some_and(|t|t.is_finished()) {
                let task=relink_task.take().expect("finished relocation");
                while event::poll(Duration::ZERO)?{let _=event::read()?;}
                match task.await {
                    Ok(RelinkResult::Preview(result))=>match result {
                        Ok(p)=>if let Some(modal)=view.relink.as_mut(){modal.busy=false;modal.input=p.target.display().to_string();modal.preview=Some(p);},
                        Err(e)=>show_relink_failure(&mut view,e,false),
                    },
                    Ok(RelinkResult::Applied(crate::relocation::Outcome::Verified(p)))=>{
                        view.relink=None;view.focus=1;if p.archived {browser.include_archived=true;}browser.open_folder(p.target.clone());
                        browser.selected=Some(p.key.clone());relocated=Some(p);
                        view.message=Some("Session relinked and verified.".into());discovery.refresh(&mut browser);
                    },
                    Ok(RelinkResult::Applied(crate::relocation::Outcome::Rejected(e)))=>{
                        show_relink_failure(&mut view,e,false);discovery.refresh(&mut browser);
                    },
                    Ok(RelinkResult::Applied(crate::relocation::Outcome::Unresolved(e)))=>{
                        show_relink_failure(&mut view,e,true);discovery.refresh(&mut browser);
                    },
                    Err(e)=>{show_relink_failure(&mut view,format!("Relocation stopped: {e}").into(),true);}
                }
            }
            for _ in 0..100 {
                match discovery.rx.try_recv() {
                    Ok(update) => browser.apply(update),
                    Err(_) => break,
                }
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
                if let Some((source, path, before)) = created.take() {
                    if let Some(session) = browser.visible().into_iter().find(|s| {
                        s.key.source_id == source
                            && s.cwd.as_ref() == Some(&path)
                            && !before.contains(&s.key)
                    }) {
                        browser.selected = Some(session.key);
                    }
                }
            }
            if let Some(modal)=view.relink.as_mut(){
                if !modal.busy && modal.preview.is_none(){if let Some(picker)=modal.picker.as_mut(){picker.tick(&mut modal.input);}}
            }
            discovery.check_folders(&browser);
            if pending.is_none() && relink_task.is_none() {
                discovery.detail(&browser);
            }
            if pending.as_ref().is_some_and(|p| p.task.is_finished()) {
                let mut ready = pending.take().expect("finished preparation");
                view.preparing = false;
                // An Enter queued during preparation must not approve a changed folder.
                while event::poll(Duration::ZERO)? {
                    let _ = event::read()?;
                }
                match (&mut ready.task).await {
                    Ok(Ok(command)) => {
                        browser.details.insert(
                            command.session.key.clone(),
                            Ok(Some(command.session.clone())),
                        );
                        if ready.observed_cwd.as_ref() != Some(&command.cwd) {
                            view.focus = 2;
                            view.message = Some(format!(
                                "Resume folder changed to {}. Review it, then press Enter again.",
                                command.cwd.display()
                            ));
                        } else {
                            discovery.close(&mut browser).await;
                            // Discard pending metadata events before starting a fresh generation.
                            while discovery.rx.try_recv().is_ok() {}
                            save_folder(&mut browser, last_folder.as_deref(), &mut view);
                            guard.suspend()?;
                            let result = crate::resume::run(&command, &signals.terminate).await;
                            signals
                                .interrupt
                                .store(false, std::sync::atomic::Ordering::Relaxed);
                            guard.activate()?;
                            terminal.clear()?;
                            terminal.autoresize()?;
                            view.message = match result {
                                Ok(status) if status.success() => view.message.take(),
                                Ok(status) => Some(format!(
                                    "Codex exited with {status}. You can retry or refresh."
                                )),
                                Err(error) => Some(format!("Could not resume Codex: {error}")),
                            };
                            discovery.refresh(&mut browser);
                            // Drop queued key repeats so one Enter cannot launch a second child.
                            while event::poll(Duration::ZERO)? {
                                let _ = event::read()?;
                            }
                        }
                    }
                    Ok(Err(error)) => view.message = Some(error),
                    Err(error) => {
                        view.message = Some(format!("Resume preparation failed: {error}"))
                    }
                }
            }
            let size = terminal.size()?;
            view.help_page_size =
                usize::from(size.height.saturating_sub(2).min(22).saturating_sub(7))
                    / if size.width < 58 { 2 } else { 1 };
            terminal.draw(|frame| draw(frame, &browser, &view))?;
            if event::poll(Duration::from_millis(40))? {
                let incoming=event::read()?;
                if let Event::Paste(text)=&incoming {
                    if let Some(modal)=view.folder_move.as_mut(){modal.paste(text);}
                    if let Some(modal)=view.relink.as_mut(){if !modal.busy && modal.preview.is_none(){if let Some(picker)=modal.picker.as_mut(){picker.paste(&mut modal.input,text);}}}
                }
                if let Event::Key(key) = incoming {
                    let was_help = view.help;
                    let action = handle_key(&mut browser, &mut view, key);
                    if was_help && !view.help && key.code == KeyCode::Enter {
                        // An activation must not also accept a newly opened modal or
                        // resume the underlying session through queued Enter repeats.
                        while event::poll(Duration::ZERO)? {
                            let _ = event::read()?;
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
                            if let Some(task)=folder_task.take(){task.abort();}
                            view.folder_move=None;
                        },
                        Action::PreviewFolderMove=>{
                            let Some(modal)=view.folder_move.as_mut() else {continue};
                            let Some(state)=relocation_state.clone() else {continue};
                            let parent=match crate::relocation::destination(&modal.input,&launch_dir) {
                                Ok(p)=>p,Err(e)=>{modal.message=Some(e);continue;}
                            };
                            let target=match crate::folder_move::destination(&parent,&modal.name) {
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
                            let Some(plan)=modal.plan.clone() else {continue};
                            let Some(state)=relocation_state.clone() else {continue};
                            let sources=crate::folder_move_ui::sources(&browser);
                            let recovery=modal.recovery;modal.busy=true;modal.executing=true;
                            let tx=folder_tx.clone();
                            folder_task=Some(tokio::spawn(async move{
                                FolderMoveResult::Applied(crate::folder_move::execute(plan,state,sources,recovery,tx).await)
                            }));
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
                            if let Some(session)=selected_session(&browser){let input=session.cwd.as_ref().map(|p|p.display().to_string()).unwrap_or_default();view.relink=Some(RelinkModal{session,input,picker:Some(crate::path_picker::Picker::new(launch_dir.clone())),preview:None,recovery:false,busy:false,executing:false,error:None});}
                        }
                        Action::RecoverRelink => {
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
                                    view.relink=Some(RelinkModal{session,input:p.target.display().to_string(),picker:None,preview:Some(p),recovery:true,busy:false,executing:false,error:Some(format!("Recorded phase: {}. Enter attempts guarded completion. R cycles through {} unfinished operation(s).",j.phase,journals.len()))});
                                }else{view.message=Some("No unfinished relocations.".into());},
                                Err(e)=>view.message=Some(e),
                            }
                        }
                        Action::PreviewRelink | Action::ApplyRelink => {
                            let Some(modal)=view.relink.as_mut() else{continue};
                            let Some((binary,source))=browser.specs.iter().find_map(|s|match s{crate::browser::SourceSpec::Codex{binary,source} if source.id==modal.session.key.source_id=>Some((binary.clone(),source.clone())),_=>None}) else {modal.error=Some("Session source is no longer configured.".into());continue};
                            if action==Action::PreviewRelink {
                                let input=modal.input.clone();let launch=launch_dir.clone();
                                let session=modal.session.clone();modal.busy=true;modal.error=None;
                                relink_task=Some(tokio::spawn(async move {
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
                        Action::Quit => break,
                        Action::Refresh => {
                            view.message = None;
                            discovery.refresh(&mut browser);
                        }
                        Action::CancelResume => {
                            pending.take();
                            view.preparing = false;
                            view.message = Some("Resume cancelled.".into());
                            // Cancellation may interrupt an RPC frame. Reconnect rather than reuse it.
                            discovery.refresh(&mut browser);
                        }
                        Action::CreateNew(index) => {
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
                            let crate::browser::SourceSpec::Codex { source, .. } = spec else {
                                continue;
                            };
                            created = Some((
                                source.id.clone(),
                                path,
                                browser.all().into_iter().map(|s| s.key).collect(),
                            ));
                            discovery.close(&mut browser).await;
                            while discovery.rx.try_recv().is_ok() {}
                            save_folder(&mut browser, last_folder.as_deref(), &mut view);
                            guard.suspend()?;
                            let result = crate::resume::run(&command, &signals.terminate).await;
                            signals
                                .interrupt
                                .store(false, std::sync::atomic::Ordering::Relaxed);
                            guard.activate()?;
                            terminal.clear()?;
                            terminal.autoresize()?;
                            view.message = match result {
                                Ok(status) if status.success() => view.message.take(),
                                Ok(status) => Some(format!("Codex exited with {status}.")),
                                Err(error) => Some(format!("Could not start Codex: {error}")),
                            };
                            discovery.refresh(&mut browser);
                            while event::poll(Duration::ZERO)? {
                                let _ = event::read()?;
                            }
                        }
                        Action::ResumeSelected => {
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
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
        Ok(())
    }
    .await;
    drop(pending);
    discovery.close(&mut browser).await;
    save_folder(&mut browser, last_folder.as_deref(), &mut view);
    guard.suspend()?;
    if let Some(warning) = browser.state_warning.as_ref() {
        eprintln!("tracr: {}", safe_text(warning));
    }
    outcome
}

#[cfg(test)]
mod cleanup_tests {
    use super::*;

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
                    None
                );
            }
        }
    }

    // Invoked only by the parent test in a fresh controlling terminal.
    #[test]
    #[ignore]
    fn guard_child() {
        let result = std::panic::catch_unwind(|| -> io::Result<()> {
            let mut guard = TerminalGuard { active: false };
            guard.activate()?;
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
