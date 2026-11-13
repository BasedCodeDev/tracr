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
    text::Line,
    widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph, Wrap},
};
use std::{
    io::{self, IsTerminal},
    time::Duration,
};

#[derive(Default)]
pub struct View {
    pub focus: usize,
    pub editing: bool,
    pub search_query: String,
    pub search_selected: Option<crate::domain::SessionKey>,
    pub help: bool,
    pub details_scroll: u16,
    pub message: Option<String>,
    pub preparing: bool,
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
        .border_style(Style::default().fg(if focused {
            Color::Cyan
        } else {
            Color::DarkGray
        }))
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
    let issues: Vec<_> = browser
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
        let popup = Rect {
            x: area.x + 1,
            y: area.y + 1,
            width: area.width.saturating_sub(2),
            height: area.height.saturating_sub(2),
        };
        frame.render_widget(Clear, popup);
        frame.render_widget(Paragraph::new("Up / Down or j,k: navigate\nLeft / Right or Tab / Shift-Tab: switch panes\nEnter: toggle folder or resume selected active session\n/: search modal; type to filter, Up/Down select\nEnter in search: open folder and select session (not resume)\na: toggle archived sessions\nr: refresh\nEsc: previous pane (close help or cancel preparation)\nCtrl-U in search: clear query\nq / Ctrl-C: quit (q inserts text in search)\n\nSearch matches all whitespace-separated terms across title, ID, folder, provider and Git branch. Esc closes search without changing the selected folder.\n\nDirectory mismatches show listed and resume folders separately. Active Codex sessions resume in this terminal. Archived and fixture sessions are browse-only. Relinking is not available.").block(block("Help — Esc to close",true)).wrap(Wrap {trim:false}),popup);
    }
    if view.editing {
        draw_search(frame, browser, view);
    }
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
                Line::styled(
                    format!(
                        "{}{}",
                        if s.archived { "[archived] " } else { "" },
                        safe_text(&s.title)
                    ),
                    Style::default().add_modifier(Modifier::BOLD),
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
            .highlight_style(Style::default().fg(Color::Cyan))
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
                })
                .collect();
            let mut state = ListState::default()
                .with_selected(folders.iter().position(|r| r.filter == browser.folder));
            frame.render_stateful_widget(
                List::new(items)
                    .block(block("Folders", view.focus == 0))
                    .highlight_style(Style::default().fg(Color::Cyan))
                    .highlight_symbol("> "),
                area,
                &mut state,
            );
        }
        1 => {
            let sessions = browser.visible();
            let scope = if !browser.query.is_empty() {
                Some("Global search".to_owned())
            } else if matches!(browser.folder, FolderFilter::Path(_)) {
                None
            } else {
                Some("Sessions · activity in Unix seconds".to_owned())
            };
            if sessions.is_empty() {
                let message = if browser.loading() {
                    "Loading sessions…"
                } else if browser.diagnostics().iter().any(|source| !source.complete) {
                    "No matching sessions found; inventory is incomplete. Check source errors below."
                } else {
                    "No matching sessions. Press a to include archives, / to search globally, or Tab to choose another folder."
                };
                frame.render_widget(
                    Paragraph::new(match &scope {
                        Some(scope) => format!("{scope}\n\n{message}"),
                        None => message.to_owned(),
                    })
                    .wrap(Wrap { trim: false })
                    .block(block("Sessions", view.focus == 1)),
                    area,
                );
                return;
            }
            let area = if let Some(scope) = scope {
                let panes = Layout::default()
                    .direction(Direction::Vertical)
                    .constraints([Constraint::Length(3), Constraint::Min(1)])
                    .split(area);
                frame.render_widget(Paragraph::new(scope).wrap(Wrap { trim: false }), panes[0]);
                panes[1]
            } else {
                area
            };
            let items: Vec<_> = sessions
                .iter()
                .map(|s| {
                    ListItem::new(vec![
                        Line::styled(
                            format!(
                                "{}{}",
                                if s.archived { "[archived] " } else { "" },
                                safe_text(&s.title),
                            ),
                            Style::default().add_modifier(Modifier::BOLD),
                        ),
                        Line::styled(
                            format!("{} · {}", safe_text(&s.key.provider.0), s.updated_at),
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
                    .highlight_style(Style::default().fg(Color::Cyan))
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
    if view.help {
        if matches!(key.code, KeyCode::Esc | KeyCode::Char('?')) {
            view.help = false;
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
    match key.code {
        KeyCode::Char('q') => return Action::Quit,
        KeyCode::Char('?') => view.help = true,
        KeyCode::Char('/') => {
            view.editing = true;
            view.search_query.clear();
            view.search_selected = None;
        }
        KeyCode::Char('r') => return Action::Refresh,
        KeyCode::Char('a') => browser.include_archived = !browser.include_archived,
        KeyCode::Tab | KeyCode::Right => view.focus = (view.focus + 1) % 3,
        KeyCode::BackTab | KeyCode::Left | KeyCode::Esc => {
            view.focus = (view.focus + 2) % 3;
            view.message = None;
        }
        KeyCode::Up | KeyCode::Char('k') | KeyCode::Down | KeyCode::Char('j') => {
            let delta = if matches!(key.code, KeyCode::Up | KeyCode::Char('k')) {
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
        KeyCode::Enter if view.focus == 0 => {
            if let FolderFilter::Path(path) = &browser.folder {
                let expand = !browser.expanded.contains(path);
                if expand {
                    browser.expanded.insert(path.clone());
                } else {
                    browser.expanded.remove(path);
                }
            }
        }
        KeyCode::Enter => return Action::ResumeSelected,
        _ => {}
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
    discovery.refresh(&mut browser);
    let mut view = View {
        focus: usize::from(browser.initial_folder.is_some()),
        ..View::default()
    };
    let signals = ShutdownSignals::new()?;
    let mut pending: Option<PendingResume> = None;
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
                            guard.suspend()?;
                            let result = crate::resume::run(&command, &signals.terminate).await;
                            signals
                                .interrupt
                                .store(false, std::sync::atomic::Ordering::Relaxed);
                            guard.activate()?;
                            terminal.clear()?;
                            terminal.autoresize()?;
                            view.message = match result {
                                Ok(status) if status.success() => None,
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
            terminal.draw(|frame| draw(frame, &browser, &view))?;
            if event::poll(Duration::from_millis(40))? {
                if let Event::Key(key) = event::read()? {
                    match handle_key(&mut browser, &mut view, key) {
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
