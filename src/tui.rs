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
}
fn restore() {
    let _ = terminal::disable_raw_mode();
    let _ = execute!(io::stdout(), LeaveAlternateScreen, crossterm::cursor::Show);
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
        execute!(io::stdout(), EnterAlternateScreen)
    }
    fn suspend(&mut self) -> io::Result<()> {
        terminal::disable_raw_mode()?;
        execute!(io::stdout(), LeaveAlternateScreen, crossterm::cursor::Show)?;
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

pub fn draw(frame: &mut Frame<'_>, browser: &Browser, view: &View) {
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
    frame.render_widget(
        Paragraph::new(format!(
            "←/→ panes · Enter toggle/resume · / search · a archives · r refresh · ? help · q quit\n{}",
            if view.preparing {
                "Preparing resume… Esc cancels".into()
            } else {
                view.message
                    .as_ref()
                    .map(|m| safe_text(m))
                    .unwrap_or_else(|| issues.join(" | "))
            }
        ))
        .wrap(Wrap { trim: false }),
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
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Command {
    PreviousPane,
    NextPane,
    PreviousItem,
    NextItem,
    ToggleFolder,
    Resume,
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
            _ => "Waiting",
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
                .fg(activity_color(status).expect("active label"))
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
                        Line::styled(
                            format!(
                                "{} · {}",
                                safe_text(&s.key.provider.0),
                                relative_time(s.updated_at, now)
                            ),
                            Style::default().add_modifier(Modifier::DIM),
                        ),
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
                let extra = format!("{extra}\n\nResume: {}", resume_hint(browser, &s));
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
    Continue,
    Quit,
    Refresh,
    ResumeSelected,
    CancelResume,
    CreateNew(usize),
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
    "Enter to resume in this terminal; the destination is checked again before launch.".into()
}

pub fn handle_key(browser: &mut Browser, view: &mut View, key: KeyEvent) -> Action {
    if key.kind == KeyEventKind::Release {
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
        Command::Resume => return Action::ResumeSelected,
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
    let mut last_folder = browser.initial_folder.clone();
    let mut created: Option<(
        crate::domain::SourceId,
        std::path::PathBuf,
        std::collections::BTreeSet<crate::domain::SessionKey>,
    )> = None;
    let outcome: Result<(), Box<dyn std::error::Error>> = async {
        loop {
            if signals.terminate.load(std::sync::atomic::Ordering::Relaxed)
                || signals.interrupt.load(std::sync::atomic::Ordering::Relaxed)
            {
                break;
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
            discovery.check_folders(&browser);
            if pending.is_none() {
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
                if let Event::Key(key) = event::read()? {
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
