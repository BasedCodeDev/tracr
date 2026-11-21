//! Owned PTYs. Output is parsed, never written to the outer terminal.
use crate::domain::{LaunchCommand, SessionKey};
use crossterm::event::{
    Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use portable_pty::{CommandBuilder, MasterPty, PtySize};
use ratatui::{
    Frame,
    layout::{Constraint, Rect},
    style::{Color, Modifier, Style},
    widgets::{
        Block, BorderType, Borders, Clear, Padding, Paragraph, Row, Table, TableState, Wrap,
    },
};
use std::{
    io::{self, Read, Write},
    os::fd::AsRawFd,
    sync::mpsc,
    time::{Duration, Instant},
};

type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;

fn io_ready(fd: std::os::fd::RawFd, events: i16) -> bool {
    let mut descriptor = libc::pollfd {
        fd,
        events,
        revents: 0,
    };
    // SAFETY: fd is owned by the worker and descriptor is a valid single entry.
    unsafe { libc::poll(&mut descriptor, 1, 100) > 0 }
}

/// Presentation data resolved from cached discovery, never from Git during drawing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct WorktreeLabel {
    name: String,
    dirty: bool,
}
impl WorktreeLabel {
    pub(crate) fn for_folder(
        browser: &crate::browser::Browser,
        folder: &std::path::Path,
    ) -> Option<Self> {
        let crate::git_discovery::Outcome::Repository {
            repository,
            root: Some(root),
        } = browser.ownership.get(folder)?.effective()?
        else {
            return None;
        };
        let worktree = browser
            .repositories
            .get(&repository.common_dir)?
            .worktrees
            .iter()
            .skip(1)
            .find(|w| !w.bare && &w.path == root)?;
        let name = worktree
            .branch
            .as_deref()
            .map(|b| b.strip_prefix("refs/heads/").unwrap_or(b))
            .map(str::to_owned)
            .unwrap_or_else(|| {
                root.file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .into_owned()
            });
        Some(Self {
            name: crate::browser::safe_text(&name),
            dirty: browser
                .worktree_status
                .get(root)
                .and_then(|s| s.as_ref().ok())
                .is_some_and(|s| s.dirty()),
        })
    }

    fn line(&self, width: u16) -> ratatui::text::Line<'static> {
        use ratatui::text::{Line, Span};
        use unicode_segmentation::UnicodeSegmentation;
        use unicode_width::UnicodeWidthStr;
        let suffix = if self.dirty { " *" } else { "" };
        let available = usize::from(width).saturating_sub(suffix.len());
        let mut name = String::new();
        let clipped = self.name.width() > available;
        let budget = available.saturating_sub(usize::from(clipped));
        for grapheme in self.name.graphemes(true) {
            if name.width() + grapheme.width() > budget {
                break;
            }
            name.push_str(grapheme);
        }
        if clipped && available > 0 {
            name.push('…');
        }
        Line::from(vec![
            Span::styled(name, Style::default().fg(crate::git_render::BRANCH_COLOR)),
            Span::styled(
                if self.dirty && width == 1 {
                    "*"
                } else if width == 0 {
                    ""
                } else {
                    suffix
                },
                Style::default().fg(crate::git_render::DIRTY_COLOR),
            ),
        ])
    }
}

#[derive(PartialEq)]
enum Ownership {
    Owned,
    Preview,
}

struct Session {
    ownership: Ownership,
    id: u64,
    key: Option<SessionKey>,
    title: String,
    folder: std::path::PathBuf,
    master: Box<dyn MasterPty + Send>,
    child: Box<dyn portable_pty::Child + Send + Sync>,
    writer: mpsc::SyncSender<Vec<u8>>,
    pending_input: std::cell::RefCell<std::collections::VecDeque<Vec<u8>>>,
    queued_bytes: std::sync::Arc<std::sync::atomic::AtomicUsize>,
    modes: crate::terminal_input::Modes,
    focused: Option<bool>,
    selection: Option<crate::terminal_selection::Selection>,
    click: Option<(Instant, crate::terminal_selection::Point, u8)>,
    mouse_down: bool,
    delivery_failed: bool,
    writer_status: mpsc::Receiver<String>,
    output: mpsc::Receiver<Vec<u8>>,
    parser: vt100::Parser,
    queries: vte::Parser,
    alternate_scroll: bool,
    pid: Option<u32>,
    size: (u16, u16),
    stopping: Option<Instant>,
    exit: Option<Instant>,
    io_stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
    output_closed: bool,
}
impl Session {
    fn signal(&self, signal: i32) {
        if let Some(pid) = self.pid {
            // SAFETY: portable-pty starts the owned child in a new session/process group.
            unsafe {
                libc::kill(-(pid as i32), signal);
            }
        }
    }
    fn stop(&mut self) {
        if self.stopping.is_none() {
            self.signal(libc::SIGTERM);
            self.signal(libc::SIGCONT);
            self.stopping = Some(Instant::now());
        }
    }
    fn write(&self, bytes: Vec<u8>) -> io::Result<()> {
        self.queued_bytes
            .fetch_add(bytes.len(), std::sync::atomic::Ordering::Relaxed);
        self.pending_input.borrow_mut().push_back(bytes);
        self.flush_input()
    }
    fn flush_input(&self) -> io::Result<()> {
        let mut pending = self.pending_input.borrow_mut();
        while let Some(bytes) = pending.pop_front() {
            match self.writer.try_send(bytes) {
                Ok(()) => (),
                Err(mpsc::TrySendError::Full(bytes)) => {
                    pending.push_front(bytes);
                    break;
                }
                Err(mpsc::TrySendError::Disconnected(_)) => {
                    return Err(io::Error::other("Terminal input writer disconnected"));
                }
            }
        }
        Ok(())
    }
}
impl Drop for Session {
    fn drop(&mut self) {
        self.io_stop
            .store(true, std::sync::atomic::Ordering::Relaxed);
        self.signal(libc::SIGKILL);
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[derive(Default)]
pub struct Manager {
    sessions: Vec<Session>,
    folder_views: std::collections::BTreeMap<std::path::PathBuf, u64>,
    next_id: u64,
    pub active: Option<u64>,
    pub picker: bool,
    selected: usize,
    picker_page_size: std::cell::Cell<usize>,
    confirmation: Option<Confirmation>,
    pub area: Rect,
    pub visible: bool,
    pub copy_request: Option<String>,
    pub input_error: Option<String>,
}
enum Confirmation {
    Quit,
    Close(u64),
}
#[derive(Default)]
pub(crate) struct OverlayState {
    picker: bool,
    selected: usize,
    confirmation: Option<Confirmation>,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct WorktreeTerminal {
    pub(crate) id: u64,
    pub(crate) title: String,
    pub(crate) folder: std::path::PathBuf,
    pub(crate) key: Option<SessionKey>,
    pub(crate) owned: bool,
}

pub struct ExitNotice {
    pub(crate) id: u64,
    pub(crate) exit_verified: bool,
    pub message: String,
    pub failed: bool,
    pub was_active: bool,
}
pub enum Input {
    Pass,
    Consumed,
    Quit,
}
impl Manager {
    pub(crate) fn swap_overlay(&mut self, other: &mut OverlayState) {
        std::mem::swap(&mut self.picker, &mut other.picker);
        std::mem::swap(&mut self.selected, &mut other.selected);
        std::mem::swap(&mut self.confirmation, &mut other.confirmation);
    }

    /// Return the displayed folder to its session list without stopping its process.
    pub fn background(&mut self) {
        if let Some(folder) = self.active_folder() {
            self.folder_views.remove(&folder);
        }
        self.active = None;
    }

    pub fn is_empty(&self) -> bool {
        self.sessions.is_empty()
    }
    pub(crate) fn worktree_terminals(&self, root: &std::path::Path) -> Vec<WorktreeTerminal> {
        self.sessions
            .iter()
            .filter(|session| {
                session.folder.starts_with(root)
                    || std::fs::canonicalize(&session.folder)
                        .is_ok_and(|path| path.starts_with(root))
            })
            .map(|session| WorktreeTerminal {
                id: session.id,
                title: session.title.clone(),
                folder: session.folder.clone(),
                key: session.key.clone(),
                owned: session.ownership == Ownership::Owned,
            })
            .collect()
    }

    pub(crate) fn close_worktree_terminals(
        &mut self,
        root: &std::path::Path,
        reviewed: &[WorktreeTerminal],
    ) -> std::result::Result<Vec<WorktreeTerminal>, Vec<WorktreeTerminal>> {
        let mut current = self.worktree_terminals(root);
        if current.iter().any(|terminal| !reviewed.contains(terminal)) {
            return Err(current);
        }
        current.retain(|terminal| terminal.owned);
        for terminal in &current {
            if let Some(session) = self
                .sessions
                .iter_mut()
                .find(|session| session.id == terminal.id)
            {
                session.stop();
            }
        }
        Ok(current)
    }

    pub fn owns(&self, key: &SessionKey) -> bool {
        self.sessions.iter().any(|s| s.key.as_ref() == Some(key))
    }
    /// Restore a folder's view without moving keyboard focus or restarting its PTY.
    pub fn sync_folder(&mut self, folder: Option<&std::path::Path>) {
        self.sync_preview(folder, None);
    }
    /// Temporarily display an owned session without changing any folder's saved view.
    pub fn sync_preview(&mut self, folder: Option<&std::path::Path>, key: Option<&SessionKey>) {
        self.active = key
            .and_then(|key| self.sessions.iter().find(|s| s.key.as_ref() == Some(key)))
            .map(|session| session.id)
            .or_else(|| folder.and_then(|path| self.folder_views.get(path)).copied());
    }
    /// Hide and stop disposable clients without touching a daemon or another manager.
    pub(crate) fn stop_preview(&mut self) {
        self.active = None;
        self.visible = false;
        self.folder_views.clear();
        for session in &mut self.sessions {
            if session.ownership == Ownership::Preview {
                session.stop();
            }
        }
    }

    pub(crate) fn launch_preview(
        &mut self,
        command: &LaunchCommand,
        key: SessionKey,
        title: String,
    ) -> Result<()> {
        self.launch(command, Some(key), title)?;
        self.sessions
            .last_mut()
            .expect("launched preview")
            .ownership = Ownership::Preview;
        self.folder_views.clear();
        Ok(())
    }

    /// Transfer the same PTY into normal ownership; no resume or restart occurs.
    pub(crate) fn promote_into(&mut self, owned: &mut Self, key: &SessionKey) -> bool {
        let Some(index) = self.sessions.iter().position(|s| {
            s.ownership == Ownership::Preview && s.key.as_ref() == Some(key) && s.stopping.is_none()
        }) else {
            return false;
        };
        let mut session = self.sessions.remove(index);
        session.ownership = Ownership::Owned;
        owned.next_id += 1;
        session.id = owned.next_id;
        owned
            .folder_views
            .insert(session.folder.clone(), session.id);
        owned.active = Some(session.id);
        owned.sessions.push(session);
        self.active = None;
        self.visible = false;
        self.folder_views.clear();
        true
    }

    pub fn active_folder(&self) -> Option<std::path::PathBuf> {
        self.sessions
            .iter()
            .find(|s| Some(s.id) == self.active)
            .map(|s| s.folder.clone())
    }
    fn activate(&mut self, id: u64) {
        if let Some(s) = self.sessions.iter().find(|s| s.id == id) {
            self.folder_views.insert(s.folder.clone(), id);
            self.active = Some(id);
        }
    }
    pub fn attach(&mut self, key: &SessionKey) -> bool {
        if let Some(s) = self.sessions.iter().find(|s| s.key.as_ref() == Some(key)) {
            self.activate(s.id);
            true
        } else {
            false
        }
    }
    pub fn launch(
        &mut self,
        command: &LaunchCommand,
        key: Option<SessionKey>,
        title: String,
    ) -> Result<()> {
        let rows = self.area.height.max(1);
        let cols = self.area.width.max(1);
        let pair = portable_pty::native_pty_system().openpty(PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        })?;
        let mut cmd = CommandBuilder::new(&command.executable);
        cmd.args(&command.args);
        cmd.cwd(&command.cwd);
        for (k, v) in &command.env {
            cmd.env(k, v);
        }
        cmd.env("TERM", "xterm-256color");
        let fd = pair
            .master
            .as_raw_fd()
            .ok_or_else(|| io::Error::other("PTY has no file descriptor"))?;
        // SAFETY: master owns fd throughout duplication and fcntl. Nonblocking
        // master I/O lets workers notice shutdown even if a descendant keeps a slave.
        let readiness = std::sync::Arc::new(
            unsafe { std::os::fd::BorrowedFd::borrow_raw(fd) }.try_clone_to_owned()?,
        );
        let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
        if flags == -1 || unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) } == -1
        {
            return Err(io::Error::last_os_error().into());
        }
        let io_stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let mut reader = pair.master.try_clone_reader()?;
        let mut writer = pair.master.take_writer()?;
        let child = pair.slave.spawn_command(cmd)?;
        drop(pair.slave);
        let pid = child.process_id();
        let (tx, output) = mpsc::sync_channel(32);
        let read_stop = io_stop.clone();
        let read_ready = readiness.clone();
        std::thread::spawn(move || {
            let mut buffer = [0; 8192];
            while !read_stop.load(std::sync::atomic::Ordering::Relaxed) {
                if !io_ready(read_ready.as_raw_fd(), libc::POLLIN) {
                    continue;
                }
                match reader.read(&mut buffer) {
                    Err(error)
                        if matches!(
                            error.kind(),
                            io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted
                        ) =>
                    {
                        continue;
                    }
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        if tx.send(buffer[..n].to_vec()).is_err() {
                            break;
                        }
                    }
                }
            }
        });
        let (input, rx) = mpsc::sync_channel::<Vec<u8>>(64);
        let write_stop = io_stop.clone();
        let queued_bytes = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let written_bytes = queued_bytes.clone();
        let (write_error, writer_status) = mpsc::channel();
        std::thread::spawn(move || {
            while let Ok(bytes) = rx.recv() {
                let mut remaining = bytes.as_slice();
                while !remaining.is_empty() {
                    if write_stop.load(std::sync::atomic::Ordering::Relaxed) {
                        return;
                    }
                    if !io_ready(readiness.as_raw_fd(), libc::POLLOUT) {
                        continue;
                    }
                    match writer.write(remaining) {
                        Ok(0) => {
                            let _ =
                                write_error.send("Terminal input closed before delivery".into());
                            return;
                        }
                        Ok(n) => {
                            written_bytes.fetch_sub(n, std::sync::atomic::Ordering::Relaxed);
                            remaining = &remaining[n..];
                        }
                        Err(error)
                            if matches!(
                                error.kind(),
                                io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted
                            ) =>
                        {
                            continue;
                        }
                        Err(error) => {
                            let _ = write_error
                                .send(format!("Terminal input delivery failed: {error}"));
                            return;
                        }
                    }
                }
            }
        });
        self.next_id += 1;
        self.active = Some(self.next_id);
        self.folder_views.insert(command.cwd.clone(), self.next_id);
        self.sessions.push(Session {
            ownership: Ownership::Owned,
            id: self.next_id,
            key,
            title,
            folder: command.cwd.clone(),
            master: pair.master,
            child,
            writer: input,
            pending_input: Default::default(),
            queued_bytes,
            modes: Default::default(),
            focused: None,
            selection: None,
            click: None,
            mouse_down: false,
            delivery_failed: false,
            writer_status,
            output,
            parser: vt100::Parser::new(rows, cols, 2000),
            queries: vte::Parser::new(),
            alternate_scroll: true,
            pid,
            size: (rows, cols),
            stopping: None,
            exit: None,
            io_stop,
            output_closed: false,
        });
        Ok(())
    }
    pub fn poll(&mut self) -> (bool, Vec<ExitNotice>) {
        let mut changed = false;
        let mut exited = Vec::new();
        self.sessions.retain_mut(|s| {
            if let Ok(error) = s.writer_status.try_recv() {
                self.input_error = Some(error);
                s.delivery_failed = true;
                changed = true;
            }
            if let Err(error) = s.flush_input() {
                if !s.delivery_failed {
                    self.input_error = Some(error.to_string());
                    changed = true;
                }
                s.delivery_failed = true;
            }
            if let Some(selection) = s.selection.as_mut().filter(|selection| {
                selection.dragging
                    && selection.edge != 0
                    && self.active == Some(s.id)
                    && self.visible
                    && selection.tick.elapsed() >= Duration::from_millis(60)
            }) {
                let offset = s.parser.screen().scrollback();
                s.parser.set_scrollback(if selection.edge < 0 {
                    offset.saturating_add(1)
                } else {
                    offset.saturating_sub(1)
                });
                let screen = s.parser.screen();
                let line = screen.viewport_start()
                    + if selection.edge < 0 {
                        0
                    } else {
                        u64::from(s.size.0.saturating_sub(1))
                    };
                selection.update(
                    screen,
                    crate::terminal_selection::Point(line, selection.head.1),
                );
                selection.tick = Instant::now();
                changed = true;
            }
            let mut received_output = false;
            for _ in 0..32 {
                let bytes = match s.output.try_recv() {
                    Ok(bytes) => bytes,
                    Err(mpsc::TryRecvError::Disconnected) => {
                        s.output_closed = true;
                        break;
                    }
                    Err(mpsc::TryRecvError::Empty) => break,
                };
                changed = true;
                received_output = true;
                for byte in bytes {
                    s.parser.process(&[byte]);
                    let mut queries = Queries {
                        cursor: s.parser.screen().cursor_position(),
                        replies: Vec::new(),
                        alternate_scroll: s.alternate_scroll,
                        modes: std::mem::take(&mut s.modes),
                        reset: false,
                    };
                    s.queries.advance(&mut queries, byte);
                    s.alternate_scroll = queries.alternate_scroll;
                    s.modes = queries.modes;
                    if queries.reset
                        || s.selection
                            .as_ref()
                            .is_some_and(|selection| !selection.valid(s.parser.screen()))
                    {
                        s.selection = None;
                    }
                    for reply in queries.replies {
                        let _ = s.write(reply);
                    }
                }
            }
            if let Some(selection) = s.selection.as_ref().filter(|_| received_output) {
                let mut current = selection.clone();
                current.update(s.parser.screen(), selection.head);
                if current.text != selection.text {
                    s.selection = None;
                }
            }
            if s.stopping
                .is_some_and(|at| at.elapsed() >= Duration::from_secs(5))
            {
                s.signal(libc::SIGKILL);
            }
            match s.child.try_wait() {
                Ok(Some(status)) => {
                    if s.exit.is_none() {
                        s.signal(libc::SIGKILL);
                        s.pid = None;
                        s.exit = Some(Instant::now());
                    }
                    if !s.output_closed
                        && s.exit
                            .is_some_and(|at| at.elapsed() < Duration::from_millis(250))
                    {
                        return true;
                    }
                    let details = if status.success() || s.stopping.is_some() {
                        String::new()
                    } else {
                        format!(
                            " — {}",
                            s.parser
                                .screen()
                                .contents()
                                .chars()
                                .take(1500)
                                .collect::<String>()
                        )
                    };
                    exited.push(ExitNotice {
                        id: s.id,
                        exit_verified: true,
                        message: format!("{} exited: {status}{details}", s.title),
                        failed: !status.success() && s.stopping.is_none(),
                        was_active: self.active == Some(s.id),
                    });
                    if self.active == Some(s.id) {
                        self.active = None;
                    }
                    false
                }
                Err(error) => {
                    exited.push(ExitNotice {
                        id: s.id,
                        exit_verified: false,
                        message: format!("{}: {error}", s.title),
                        failed: true,
                        was_active: self.active == Some(s.id),
                    });
                    if self.active == Some(s.id) {
                        self.active = None;
                    }
                    false
                }
                Ok(None) => true,
            }
        });
        self.folder_views
            .retain(|_, id| self.sessions.iter().any(|s| s.id == *id));
        self.selected = self.selected.min(self.sessions.len().saturating_sub(1));
        (changed || !exited.is_empty(), exited)
    }
    pub async fn shutdown(&mut self) {
        for s in &mut self.sessions {
            s.stop();
        }
        while !self.sessions.is_empty() {
            self.poll();
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    }
    pub fn request_quit(&mut self) -> bool {
        if self.is_empty() {
            true
        } else {
            self.active = None;
            self.confirmation = Some(Confirmation::Quit);
            false
        }
    }
    pub fn request_close(&mut self, key: Option<&SessionKey>) {
        if let Some(s) =
            key.and_then(|key| self.sessions.iter().find(|s| s.key.as_ref() == Some(key)))
        {
            self.confirmation = Some(Confirmation::Close(s.id));
        } else {
            self.picker = true;
        }
    }
    pub fn sync_focus(&mut self, focused: bool) -> io::Result<()> {
        let overlay = self.has_overlay();
        for s in &mut self.sessions {
            let focus = focused && !overlay && self.visible && self.active == Some(s.id);
            if !focus {
                if let Some(selection) = s.selection.as_mut() {
                    selection.dragging = false;
                    selection.edge = 0;
                }
            }
            if s.modes.focus {
                if s.focused != Some(focus) {
                    s.write(if focus { b"\x1b[I" } else { b"\x1b[O" }.to_vec())?;
                }
                s.focused = Some(focus);
            } else {
                s.focused = None;
            }
        }
        Ok(())
    }
    pub fn input_backlogged(&self) -> bool {
        self.sessions.iter().any(|s| {
            !s.delivery_failed
                && s.queued_bytes.load(std::sync::atomic::Ordering::Relaxed) > 4 * 1024 * 1024
        })
    }
    pub fn has_overlay(&self) -> bool {
        self.picker || self.confirmation.is_some()
    }
    pub fn mouse(&mut self, mut mouse: MouseEvent) -> io::Result<Input> {
        use crate::terminal_selection::{Point, Selection};
        if !self.visible || self.has_overlay() || self.area.is_empty() {
            return Ok(Input::Pass);
        }
        let inside = self.area.contains((mouse.column, mouse.row).into());
        let Some(s) = self.sessions.iter_mut().find(|s| Some(s.id) == self.active) else {
            return Ok(Input::Pass);
        };
        let selecting = s
            .selection
            .as_ref()
            .is_some_and(|selection| selection.dragging);
        if !inside && !selecting && !s.mouse_down {
            return Ok(Input::Pass);
        }
        let edge = if mouse.row < self.area.y {
            -1
        } else if mouse.row >= self.area.bottom() {
            1
        } else {
            0
        };
        mouse.column = mouse.column.clamp(self.area.x, self.area.right() - 1);
        mouse.row = mouse.row.clamp(self.area.y, self.area.bottom() - 1);
        let screen = s.parser.screen();
        let local = selecting
            || mouse.modifiers.contains(KeyModifiers::SHIFT)
            || screen.mouse_protocol_mode() == vt100::MouseProtocolMode::None;
        let point = Point(
            screen.viewport_start() + u64::from(mouse.row - self.area.y),
            mouse.column - self.area.x,
        );
        if local {
            match mouse.kind {
                MouseEventKind::Down(MouseButton::Left) => {
                    let clicks = s
                        .click
                        .filter(|(at, previous, _)| {
                            at.elapsed() < Duration::from_millis(400) && *previous == point
                        })
                        .map_or(1, |(_, _, count)| count % 3 + 1);
                    s.click = Some((Instant::now(), point, clicks));
                    s.selection = Some(Selection::new(screen, point, clicks));
                }
                MouseEventKind::Drag(MouseButton::Left) | MouseEventKind::Up(MouseButton::Left) => {
                    if let Some(selection) =
                        s.selection.as_mut().filter(|selection| selection.dragging)
                    {
                        selection.update(screen, point);
                        selection.edge = edge;
                        if matches!(mouse.kind, MouseEventKind::Up(_)) {
                            selection.dragging = false;
                            selection.edge = 0;
                        }
                    }
                }
                MouseEventKind::ScrollUp | MouseEventKind::ScrollDown => return self.wheel(mouse),
                _ => (),
            }
        } else {
            let mode = screen.mouse_protocol_mode();
            let send = match mouse.kind {
                MouseEventKind::Moved => mode == vt100::MouseProtocolMode::AnyMotion,
                MouseEventKind::Drag(_) => matches!(
                    mode,
                    vt100::MouseProtocolMode::AnyMotion | vt100::MouseProtocolMode::ButtonMotion
                ),
                MouseEventKind::Up(_) => mode != vt100::MouseProtocolMode::Press,
                _ => true,
            };
            if matches!(mouse.kind, MouseEventKind::Down(_)) {
                s.mouse_down = true;
            }
            if matches!(mouse.kind, MouseEventKind::Up(_)) {
                s.mouse_down = false;
            }
            if send {
                s.write(encode_mouse(
                    mouse,
                    self.area,
                    screen.mouse_protocol_encoding(),
                ))?;
            }
        }
        Ok(Input::Consumed)
    }
    pub fn wheel(&mut self, mouse: MouseEvent) -> io::Result<Input> {
        if !self.visible
            || self.has_overlay()
            || !self.area.contains((mouse.column, mouse.row).into())
        {
            return Ok(Input::Pass);
        }
        let up = match mouse.kind {
            MouseEventKind::ScrollUp => true,
            MouseEventKind::ScrollDown => false,
            _ => return Ok(Input::Pass),
        };
        let Some(s) = self.sessions.iter_mut().find(|s| Some(s.id) == self.active) else {
            return Ok(Input::Pass);
        };
        let screen = s.parser.screen();
        if screen.mouse_protocol_mode() != vt100::MouseProtocolMode::None
            && !mouse.modifiers.contains(KeyModifiers::SHIFT)
        {
            let bytes = encode_mouse(mouse, self.area, screen.mouse_protocol_encoding());
            if !bytes.is_empty() {
                s.write(bytes)?;
            }
        } else if screen.alternate_screen() {
            if s.alternate_scroll && !mouse.modifiers.contains(KeyModifiers::SHIFT) {
                let code = if up { KeyCode::Up } else { KeyCode::Down };
                s.write(
                    encode_key(
                        KeyEvent::new(code, KeyModifiers::NONE),
                        screen.application_cursor(),
                    )
                    .repeat(3),
                )?;
            }
        } else {
            let offset = screen.scrollback();
            s.parser.set_scrollback(if up {
                offset.saturating_add(3)
            } else {
                offset.saturating_sub(3)
            });
        }
        Ok(Input::Consumed)
    }
    pub fn input(&mut self, event: &Event) -> io::Result<Input> {
        self.input_rich(event, None)
    }
    pub fn input_rich(
        &mut self,
        event: &Event,
        rich: Option<&crate::terminal_input::Key>,
    ) -> io::Result<Input> {
        if let Some(confirmation) = &self.confirmation {
            if let Event::Key(key) = event {
                if key.kind == KeyEventKind::Release {
                    return Ok(Input::Consumed);
                }
                match key.code {
                    KeyCode::Esc | KeyCode::Char('n') => self.confirmation = None,
                    KeyCode::Char('y') => {
                        match confirmation {
                            Confirmation::Quit => return Ok(Input::Quit),
                            Confirmation::Close(id) => {
                                if let Some(s) = self.sessions.iter_mut().find(|s| s.id == *id) {
                                    s.stop();
                                }
                            }
                        }
                        self.confirmation = None;
                    }
                    _ => (),
                }
            }
            return Ok(Input::Consumed);
        }
        if self.picker {
            if let Event::Key(key) = event {
                if key.kind == KeyEventKind::Release {
                    return Ok(Input::Consumed);
                }
                match key.code {
                    KeyCode::Esc | KeyCode::Char('g') => self.picker = false,
                    KeyCode::PageUp | KeyCode::PageDown | KeyCode::Home | KeyCode::End => {
                        self.selected = crate::tui::list_index(
                            key.code,
                            self.selected,
                            self.sessions.len(),
                            self.picker_page_size.get(),
                        );
                    }
                    KeyCode::Up | KeyCode::Char('k') => {
                        self.selected = self.selected.saturating_sub(1)
                    }
                    KeyCode::Down | KeyCode::Char('j') => {
                        self.selected =
                            (self.selected + 1).min(self.sessions.len().saturating_sub(1))
                    }
                    KeyCode::Enter if key.kind == KeyEventKind::Press => {
                        if let Some(s) = self.sessions.get(self.selected) {
                            self.activate(s.id);
                            self.picker = false;
                        }
                    }
                    KeyCode::Char('x') => {
                        if let Some(s) = self.sessions.get(self.selected) {
                            self.confirmation = Some(Confirmation::Close(s.id));
                        }
                    }
                    _ => (),
                }
            }
            return Ok(Input::Consumed);
        }
        let Some(id) = self.active else {
            return Ok(Input::Pass);
        };
        if matches!(event, Event::Key(k) if k.kind != KeyEventKind::Release && k.code == KeyCode::Char('b') && k.modifiers.contains(KeyModifiers::CONTROL))
        {
            if matches!(event, Event::Key(key) if key.kind == KeyEventKind::Press) {
                self.background();
            }
            return Ok(Input::Consumed);
        }
        if let Some(s) = self.sessions.iter_mut().find(|s| s.id == id) {
            if matches!(event, Event::Key(key) if matches!(key.code,KeyCode::Char('c'|'C')) && (key.modifiers==KeyModifiers::ALT || key.modifiers==KeyModifiers::CONTROL|KeyModifiers::SHIFT))
            {
                if let Some(selection) = &s.selection {
                    if matches!(event,Event::Key(key) if key.kind!=KeyEventKind::Release) {
                        self.copy_request = Some(selection.text.clone());
                    }
                    return Ok(Input::Consumed);
                }
            }
            let bytes = match event {
                Event::Key(key) => s.modes.encode(
                    &rich
                        .cloned()
                        .unwrap_or_else(|| crate::terminal_input::Key::from_event(*key)),
                    s.parser.screen().application_cursor(),
                    s.parser.screen().application_keypad(),
                ),
                Event::Paste(text) => {
                    if s.parser.screen().bracketed_paste() {
                        format!("\x1b[200~{text}\x1b[201~").into_bytes()
                    } else {
                        text.as_bytes().to_vec()
                    }
                }
                _ => Vec::new(),
            };
            if !bytes.is_empty() {
                s.write(bytes)?;
                s.parser.set_scrollback(0);
                s.selection = None;
            }
        }
        Ok(Input::Consumed)
    }
    pub fn draw(&mut self, frame: &mut Frame<'_>, area: Rect, focused: bool, framed: bool) -> bool {
        self.draw_with_worktree(frame, area, focused, framed, None)
    }

    pub(crate) fn draw_with_worktree(
        &mut self,
        frame: &mut Frame<'_>,
        area: Rect,
        focused: bool,
        framed: bool,
        worktree: Option<&WorktreeLabel>,
    ) -> bool {
        let border = Block::default()
            .borders(Borders::ALL)
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
            .title(" Codex ");
        self.area = if framed { border.inner(area) } else { area };
        let Some(s) = self.sessions.iter_mut().find(|s| Some(s.id) == self.active) else {
            return false;
        };
        if framed {
            frame.render_widget(border, area);
        }
        let inner = self.area;
        if inner.width == 0 || inner.height == 0 {
            return true;
        }
        if s.size != (inner.height, inner.width)
            && s.master
                .resize(PtySize {
                    rows: inner.height,
                    cols: inner.width,
                    pixel_width: 0,
                    pixel_height: 0,
                })
                .is_ok()
        {
            s.selection = None;
            s.parser.set_size(inner.height, inner.width);
            s.size = (inner.height, inner.width);
        }
        self.visible = true;
        let screen = s.parser.screen();
        let selected_bounds = s
            .selection
            .as_ref()
            .map(|selection| selection.bounds(screen));
        for y in 0..inner.height {
            for x in 0..inner.width {
                if let Some(cell) = screen.cell(y, x) {
                    if cell.is_wide_continuation() {
                        continue;
                    }
                    let text = cell.contents();
                    let mut style = Style::default()
                        .fg(color(cell.fgcolor()))
                        .bg(color(cell.bgcolor()));
                    for (enabled, modifier) in [
                        (cell.bold(), Modifier::BOLD),
                        (cell.italic(), Modifier::ITALIC),
                        (cell.underline(), Modifier::UNDERLINED),
                        (cell.inverse(), Modifier::REVERSED),
                    ] {
                        if enabled {
                            style = style.add_modifier(modifier);
                        }
                    }
                    let point =
                        crate::terminal_selection::Point(screen.viewport_start() + u64::from(y), x);
                    if selected_bounds.is_some_and(|(start, end)| point >= start && point < end) {
                        style = style.bg(Color::Blue).fg(Color::White);
                    }
                    frame.buffer_mut().set_stringn(
                        inner.x + x,
                        inner.y + y,
                        if text.is_empty() { " " } else { &text },
                        usize::from(inner.width - x),
                        style,
                    );
                }
            }
        }
        let (row, col) = screen.cursor_position();
        let top = if framed {
            Rect::new(inner.x, area.y, inner.width, 1)
        } else {
            Rect::new(area.x, area.y, area.width, 1)
        };
        let worktree_line = worktree.map(|label| label.line(top.width));
        let label_width = worktree_line.as_ref().map_or(0, |line| line.width() as u16);
        if let Some(line) = worktree_line {
            if top.width > label_width {
                frame.render_widget(
                    Paragraph::new(" ").style(Style::default().bg(Color::Black)),
                    Rect::new(top.right() - label_width - 1, top.y, 1, 1),
                );
            }
            frame.render_widget(
                Paragraph::new(line).style(Style::default().bg(Color::Black)),
                Rect::new(
                    top.right().saturating_sub(label_width),
                    top.y,
                    label_width,
                    1,
                ),
            );
        }
        if screen.scrollback() > 0 {
            let label = format!(" ↑ {} lines · type to return ", screen.scrollback());
            let right = top
                .right()
                .saturating_sub(label_width + u16::from(label_width > 0));
            let width = (label.len() as u16).min(right.saturating_sub(top.x));
            frame.render_widget(
                Paragraph::new(label).style(Style::default().fg(Color::Yellow).bg(Color::Black)),
                Rect::new(right.saturating_sub(width), top.y, width, 1),
            );
        }
        if focused
            && screen.scrollback() == 0
            && !screen.hide_cursor()
            && row < inner.height
            && col < inner.width
        {
            frame.set_cursor_position((inner.x + col, inner.y + row));
        }
        true
    }
    fn draw_terminal_dialog(&self, frame: &mut Frame<'_>, browser: &crate::browser::Browser) {
        let screen = frame.area();
        let width = screen
            .width
            .saturating_sub(4)
            .min(110)
            .max(screen.width.min(12));
        let sessions: Vec<_> = self.sessions.iter().filter(|session| {
            !matches!(self.confirmation, Some(Confirmation::Close(id)) if id != session.id)
        }).collect();
        let warning = match self.confirmation {
            Some(Confirmation::Quit) => {
                "Close all owned terminals and quit? Running work will stop."
            }
            Some(Confirmation::Close(_)) => "Close this terminal? Running work will stop.",
            None => "",
        };
        let warning = Paragraph::new(warning)
            .wrap(Wrap { trim: true })
            .style(Style::default().add_modifier(Modifier::BOLD));
        let padding_x = if width >= 30 {
            2
        } else if width >= 24 {
            1
        } else {
            0
        };
        let warning_height = if self.confirmation.is_some() {
            warning
                .line_count(width.saturating_sub(2 + padding_x * 2).max(1))
                .min(u16::MAX as usize) as u16
        } else {
            0
        };
        let max_height = screen.height.saturating_sub(2).max(screen.height.min(3));
        let content_height = (sessions
            .len()
            .saturating_add(4)
            .saturating_add(usize::from(warning_height))
            .min(u16::MAX as usize) as u16)
            .max(5);
        let padding_y = u16::from(max_height >= content_height.saturating_add(2));
        let height = content_height.saturating_add(padding_y * 2).min(max_height);
        let area = Rect::new(
            screen.x + (screen.width - width) / 2,
            screen.y + (screen.height - height) / 2,
            width,
            height,
        );
        frame.render_widget(Clear, area);
        let block = Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Thick)
            .border_style(Style::default().fg(Color::Cyan))
            .padding(Padding::new(
                padding_x,
                padding_x.max(1),
                padding_y,
                padding_y,
            ))
            .title(match self.confirmation {
                Some(Confirmation::Quit) => {
                    format!(" Close terminals and quit ({}) ", sessions.len())
                }
                Some(Confirmation::Close(_)) => " Close terminal ".to_owned(),
                None => format!(" Open terminals ({}) ", sessions.len()),
            });
        let inner = block.inner(area);
        frame.render_widget(block, area);
        let warning_height = warning_height.min(inner.height.saturating_sub(1));
        if warning_height > 0 {
            frame.render_widget(
                warning,
                Rect::new(inner.x, inner.y, inner.width, warning_height),
            );
        }
        let rows_area = Rect::new(
            inner.x,
            inner.y + warning_height,
            inner.width,
            inner.height.saturating_sub(1 + warning_height),
        );
        self.picker_page_size.set(usize::from(rows_area.height));
        if sessions.is_empty() {
            frame.render_widget(
                Paragraph::new("No open terminals").style(Style::default().fg(Color::DarkGray)),
                rows_area,
            );
        } else {
            let rows = sessions.iter().enumerate().map(|(index, s)| {
                let branch = if index + 1 == sessions.len() {
                    "└─"
                } else {
                    "├─"
                };
                let marker = if self.active == Some(s.id) {
                    "◆"
                } else {
                    "○"
                };
                let title = format!(
                    "{branch} {marker} #{} {}",
                    s.id,
                    crate::browser::safe_text(&s.title)
                );
                let provider = s
                    .key
                    .as_ref()
                    .map_or("terminal", |key| key.provider.0.as_str());
                let status = if s.stopping.is_some() {
                    "closing"
                } else {
                    match s.key.as_ref().map(|key| browser.runtime_status(key)) {
                        Some(crate::domain::RuntimeStatus::Idle) => "idle",
                        Some(crate::domain::RuntimeStatus::Running) => "working",
                        Some(crate::domain::RuntimeStatus::Waiting { .. }) => "waiting",
                        Some(crate::domain::RuntimeStatus::Error) => "error",
                        _ => "open",
                    }
                };
                let cells = if inner.width >= 45 {
                    vec![
                        title,
                        crate::browser::safe_text(provider),
                        status.to_owned(),
                    ]
                } else {
                    vec![title]
                };
                Row::new(cells)
            });
            let widths = if inner.width >= 45 {
                vec![
                    Constraint::Min(1),
                    Constraint::Length(12),
                    Constraint::Length(8),
                ]
            } else {
                vec![Constraint::Min(1)]
            };
            let table = Table::new(rows, widths)
                .column_spacing(2)
                .highlight_symbol("> ")
                .row_highlight_style(Style::default().add_modifier(Modifier::BOLD));
            let mut state = TableState::default()
                .with_selected(self.confirmation.is_none().then_some(self.selected));
            frame.render_stateful_widget(table, rows_area, &mut state);
            crate::scrollbar::draw(
                frame,
                crate::scrollbar::inside_border_track(area, rows_area),
                sessions.len(),
                usize::from(rows_area.height),
                state.offset(),
            );
        }
        if inner.height > 0 {
            frame.render_widget(
                Paragraph::new(if self.confirmation.is_some() {
                    "y Confirm · Esc/n Cancel"
                } else {
                    "↑/↓ j/k Select · Enter Open · x Close · g/Esc Back"
                })
                .style(Style::default().fg(Color::DarkGray)),
                Rect::new(inner.x, inner.bottom() - 1, inner.width, 1),
            );
        }
    }

    pub fn draw_overlay(&self, frame: &mut Frame<'_>, browser: &crate::browser::Browser) {
        if !self.picker && self.confirmation.is_none() {
            return;
        }
        self.draw_terminal_dialog(frame, browser);
    }
}
fn color(c: vt100::Color) -> Color {
    match c {
        vt100::Color::Default => Color::Reset,
        vt100::Color::Idx(n) => Color::Indexed(n),
        vt100::Color::Rgb(r, g, b) => Color::Rgb(r, g, b),
    }
}

fn encode_mouse(mouse: MouseEvent, area: Rect, encoding: vt100::MouseProtocolEncoding) -> Vec<u8> {
    let release = matches!(mouse.kind, MouseEventKind::Up(_));
    let button: u16 = match mouse.kind {
        MouseEventKind::ScrollUp => 64,
        MouseEventKind::ScrollDown => 65,
        MouseEventKind::ScrollLeft => 66,
        MouseEventKind::ScrollRight => 67,
        MouseEventKind::Moved => 35,
        MouseEventKind::Down(button)
        | MouseEventKind::Up(button)
        | MouseEventKind::Drag(button) => {
            let button = match button {
                MouseButton::Left => 0,
                MouseButton::Middle => 1,
                MouseButton::Right => 2,
            };
            if release && encoding != vt100::MouseProtocolEncoding::Sgr {
                3
            } else {
                button
                    + if matches!(mouse.kind, MouseEventKind::Drag(_)) {
                        32
                    } else {
                        0
                    }
            }
        }
    } + if mouse.modifiers.contains(KeyModifiers::SHIFT) {
        4
    } else {
        0
    } + if mouse.modifiers.contains(KeyModifiers::ALT) {
        8
    } else {
        0
    } + if mouse.modifiers.contains(KeyModifiers::CONTROL) {
        16
    } else {
        0
    };
    let x = mouse.column - area.x + 1;
    let y = mouse.row - area.y + 1;
    match encoding {
        vt100::MouseProtocolEncoding::Sgr => {
            format!("\x1b[<{button};{x};{y}{}", if release { 'm' } else { 'M' }).into_bytes()
        }
        vt100::MouseProtocolEncoding::Default => {
            if x > 223 || y > 223 {
                return vec![];
            }
            vec![
                27,
                b'[',
                b'M',
                (button + 32) as u8,
                (x + 32) as u8,
                (y + 32) as u8,
            ]
        }
        vt100::MouseProtocolEncoding::Utf8 => {
            if x > 2015 || y > 2015 {
                return vec![];
            }
            format!(
                "\x1b[M{}{}{}",
                char::from_u32(u32::from(button) + 32).unwrap(),
                char::from_u32(u32::from(x) + 32).unwrap(),
                char::from_u32(u32::from(y) + 32).unwrap()
            )
            .into_bytes()
        }
    }
}

fn encode_key(key: KeyEvent, application: bool) -> Vec<u8> {
    crate::terminal_input::Modes::default().encode(
        &crate::terminal_input::Key::from_event(key),
        application,
        false,
    )
}

struct Queries {
    reset: bool,
    modes: crate::terminal_input::Modes,
    alternate_scroll: bool,
    cursor: (u16, u16),
    replies: Vec<Vec<u8>>,
}
impl vte::Perform for Queries {
    fn esc_dispatch(&mut self, _: &[u8], _: bool, byte: u8) {
        if byte == b'c' {
            self.modes = Default::default();
            self.reset = true;
            self.alternate_scroll = true;
        }
    }
    fn csi_dispatch(&mut self, params: &vte::Params, intermediates: &[u8], _: bool, action: char) {
        if let Some(reply) = self.modes.csi(params, intermediates, action) {
            self.replies.push(reply);
        }
        if intermediates == b"?"
            && matches!(action, 'h' | 'l')
            && params.iter().any(|p| p == [1007])
        {
            self.alternate_scroll = action == 'h';
        }
        if intermediates == b"!" && action == 'p' {
            self.alternate_scroll = true;
        }
        let first = params
            .iter()
            .next()
            .and_then(|p| p.first())
            .copied()
            .unwrap_or(0);
        let reply = match (action, first, intermediates) {
            ('n', 6, []) => format!("\x1b[{};{}R", self.cursor.0 + 1, self.cursor.1 + 1),
            ('n', 5, []) => "\x1b[0n".into(),
            ('c', 0, []) => "\x1b[?1;2c".into(),
            ('c', 0, [b'>']) => "\x1b[>0;0;0c".into(),

            _ => return,
        };
        self.replies.push(reply.into_bytes());
    }
    fn osc_dispatch(&mut self, params: &[&[u8]], _: bool) {
        if params.len() == 2 && params[1] == b"?" {
            let reply = match params[0] {
                b"10" => "\x1b]10;rgb:dddd/dddd/dddd\x1b\\",
                b"11" => "\x1b]11;rgb:0000/0000/0000\x1b\\",
                _ => return,
            };
            self.replies.push(reply.as_bytes().to_vec());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn worktree_label_resolves_launch_directory_and_cached_status() {
        use crate::git_discovery::{Outcome, Ownership};
        let mut browser = crate::browser::Browser::new(vec![]);
        let common = std::path::PathBuf::from("/project/.git");
        let main = std::path::PathBuf::from("/project");
        let linked = std::path::PathBuf::from("/feature-folder");
        let nested = linked.join("src");
        for (folder, root) in [(&main, &main), (&linked, &linked), (&nested, &linked)] {
            browser.ownership.insert(
                folder.clone(),
                Ownership {
                    generation: 0,
                    previous: None,
                    outcome: Some(Outcome::Repository {
                        repository: crate::git::Directory {
                            common_dir: common.clone(),
                            git_dir: common.clone(),
                        },
                        root: Some(root.clone()),
                    }),
                },
            );
        }
        browser.repositories.insert(
            common.clone(),
            crate::git::Repository {
                common_dir: common.clone(),
                git_dir: common.clone(),
                worktrees: [(&main, "main"), (&linked, "feature/overlay")]
                    .into_iter()
                    .map(|(path, branch)| crate::git::Worktree {
                        path: path.clone(),
                        head: None,
                        branch: Some(format!("refs/heads/{branch}")),
                        bare: false,
                        locked: false,
                        prunable: false,
                    })
                    .collect(),
            },
        );
        assert!(WorktreeLabel::for_folder(&browser, &main).is_none());
        assert!(WorktreeLabel::for_folder(&browser, std::path::Path::new("/elsewhere")).is_none());
        assert_eq!(
            WorktreeLabel::for_folder(&browser, &nested),
            Some(WorktreeLabel {
                name: "feature/overlay".into(),
                dirty: false
            })
        );
        for counts in [
            crate::worktree_status::Counts {
                staged: 1,
                ..Default::default()
            },
            crate::worktree_status::Counts {
                modified: 1,
                ..Default::default()
            },
            crate::worktree_status::Counts {
                untracked: 1,
                ..Default::default()
            },
        ] {
            browser.worktree_status.insert(linked.clone(), Ok(counts));
            assert!(WorktreeLabel::for_folder(&browser, &nested).unwrap().dirty);
        }
        browser
            .worktree_status
            .insert(linked.clone(), Err("unavailable".into()));
        assert!(!WorktreeLabel::for_folder(&browser, &nested).unwrap().dirty);
        browser.repositories.get_mut(&common).unwrap().worktrees[1].branch = None;
        assert_eq!(
            WorktreeLabel::for_folder(&browser, &nested).unwrap().name,
            "feature-folder"
        );
    }

    #[test]
    fn worktree_label_clips_unicode_and_preserves_dirty_marker() {
        let label = WorktreeLabel {
            name: "界e\u{301}/long".into(),
            dirty: true,
        };
        for width in 0..20 {
            let line = label.line(width);
            assert!(line.width() <= usize::from(width));
            if width > 0 {
                assert!(line.to_string().ends_with('*'));
            }
        }
        assert_eq!(label.line(6).to_string(), "界e\u{301}… *");
        let line = label.line(30);
        assert_eq!(
            line.spans[0].style.fg,
            Some(crate::git_render::BRANCH_COLOR)
        );
        assert_eq!(line.spans[1].style.fg, Some(crate::git_render::DIRTY_COLOR));
    }

    #[test]
    fn worktree_overlay_aligns_without_resizing_and_coexists_with_scrollback() {
        let mut manager = Manager {
            area: Rect::new(0, 0, 40, 10),
            ..Default::default()
        };
        manager
            .launch(&command("import time; time.sleep(30)"), None, "test".into())
            .unwrap();
        let label = WorktreeLabel {
            name: "feature".into(),
            dirty: true,
        };
        let mut terminal =
            ratatui::Terminal::new(ratatui::backend::TestBackend::new(40, 10)).unwrap();
        for framed in [false, true] {
            terminal
                .draw(|frame| {
                    manager.draw_with_worktree(frame, frame.area(), true, framed, Some(&label));
                })
                .unwrap();
            let right = if framed { 39 } else { 40 };
            let buffer = terminal.backend().buffer();
            assert_eq!(buffer[(right - 1, 0)].symbol(), "*");
            assert_eq!(buffer[(right - 1, 0)].fg, crate::git_render::DIRTY_COLOR);
            assert_eq!(buffer[(right - 9, 0)].symbol(), "f");
            assert_eq!(buffer[(right - 9, 0)].fg, crate::git_render::BRANCH_COLOR);
            assert_eq!(
                manager.sessions[0].size,
                if framed { (8, 38) } else { (10, 40) }
            );
        }
        manager.sessions[0]
            .parser
            .process(&b"output\r\n".repeat(40));
        manager.sessions[0].parser.set_scrollback(5);
        terminal
            .draw(|frame| {
                manager.draw_with_worktree(frame, frame.area(), true, false, Some(&label));
            })
            .unwrap();
        let buffer = terminal.backend().buffer();
        let row: String = (0..40).map(|x| buffer[(x, 0)].symbol()).collect();
        assert!(row.contains("lines"));
        assert!(row.ends_with("feature *"));
    }

    fn key(code: KeyCode, modifiers: KeyModifiers) -> Event {
        Event::Key(KeyEvent::new(code, modifiers))
    }
    #[tokio::test]
    async fn temporary_preview_promotes_same_pty_and_restores_owned_folder() {
        use crate::domain::{ProviderId, SourceId};
        let key = SessionKey {
            provider: ProviderId("codex".into()),
            source_id: SourceId("test".into()),
            native_id: "external".into(),
        };
        let mut owned = Manager::default();
        owned
            .launch(
                &command("import time; time.sleep(30)"),
                None,
                "original".into(),
            )
            .unwrap();
        let original = owned.active;
        let original_views = owned.folder_views.clone();
        let mut preview = Manager {
            area: Rect::new(0, 0, 80, 24),
            ..Manager::default()
        };
        preview
            .launch_preview(
                &command("import time; print('NATIVE_PREVIEW',flush=True); time.sleep(30)"),
                key.clone(),
                "preview".into(),
            )
            .unwrap();
        wait(&mut preview, |m| {
            m.sessions[0]
                .parser
                .screen()
                .contents()
                .contains("NATIVE_PREVIEW")
        });
        assert!(preview.folder_views.is_empty());
        assert_eq!(owned.active, original);
        assert_eq!(owned.folder_views, original_views);
        let pid = preview.sessions[0].pid;
        assert!(preview.promote_into(&mut owned, &key));
        preview.stop_preview();
        assert!(preview.is_empty());
        assert_eq!(owned.sessions.last().unwrap().pid, pid);
        assert!(owned.owns(&key));
        assert_ne!(owned.sessions[0].id, owned.sessions[1].id);
        owned.shutdown().await;
    }

    #[tokio::test]
    async fn temporary_preview_closes_and_reaps_without_touching_owned_client() {
        use crate::domain::{ProviderId, SourceId};
        let key = SessionKey {
            provider: ProviderId("codex".into()),
            source_id: SourceId("test".into()),
            native_id: "external".into(),
        };
        let mut owned = Manager::default();
        owned
            .launch(
                &command("import time; time.sleep(30)"),
                None,
                "original".into(),
            )
            .unwrap();
        let mut preview = Manager {
            area: Rect::new(0, 0, 80, 24),
            ..Manager::default()
        };
        preview
            .launch_preview(
                &command("import time; time.sleep(30)"),
                key,
                "preview".into(),
            )
            .unwrap();
        preview.stop_preview();
        assert!(preview.active.is_none());
        wait(&mut preview, Manager::is_empty);
        assert!(owned.sessions[0].child.try_wait().unwrap().is_none());
        owned.shutdown().await;
    }

    fn command(script: &str) -> LaunchCommand {
        LaunchCommand {
            executable: "/usr/bin/python3".into(),
            args: vec!["-u".into(), "-c".into(), script.into()],
            cwd: std::env::temp_dir(),
            env: vec![],
        }
    }
    fn wait(manager: &mut Manager, predicate: impl Fn(&Manager) -> bool) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while !predicate(manager) {
            manager.poll();
            assert!(
                Instant::now() < deadline,
                "terminal condition timed out: {:?}",
                manager
                    .sessions
                    .iter()
                    .map(|s| s.parser.screen().contents())
                    .collect::<Vec<_>>()
            );
            std::thread::sleep(Duration::from_millis(10));
        }
    }
    #[tokio::test]
    async fn closing_worktree_terminals_requires_a_current_review_and_preserves_other_folders() {
        let fixture = tempfile::tempdir().unwrap();
        let target = fixture.path().join("feature");
        let nested = target.join("src");
        let other = fixture.path().join("feature-other");
        std::fs::create_dir_all(&nested).unwrap();
        std::fs::create_dir(&other).unwrap();
        let mut manager = Manager::default();
        let mut launch = command("import time; time.sleep(30)");
        launch.cwd = target.clone();
        manager.launch(&launch, None, "Root".into()).unwrap();
        launch.cwd = other;
        manager.launch(&launch, None, "Outside".into()).unwrap();

        let first_review = manager.worktree_terminals(&target);
        assert_eq!(first_review.len(), 1);
        launch.cwd = nested;
        manager.launch(&launch, None, "Nested".into()).unwrap();

        let updated = manager
            .close_worktree_terminals(&target, &first_review)
            .unwrap_err();
        assert_eq!(updated.len(), 2);
        manager.sessions[0].title = "Updated root".into();
        let reviewed = manager
            .close_worktree_terminals(&target, &updated)
            .unwrap_err();
        assert!(
            manager
                .sessions
                .iter()
                .all(|terminal| terminal.stopping.is_none())
        );

        let closing = manager
            .close_worktree_terminals(&target, &reviewed)
            .unwrap();
        assert_eq!(closing, reviewed);
        wait(&mut manager, |manager| manager.sessions.len() == 1);
        assert_eq!(manager.sessions[0].title, "Outside");
        assert!(manager.sessions[0].stopping.is_none());
        manager.shutdown().await;
    }

    #[test]
    fn session_previews_preserve_folder_choices_and_processes() {
        let temp = tempfile::tempdir().unwrap();
        let mut manager = Manager::default();
        let mut launch = command("import time; time.sleep(60)");
        launch.cwd = temp.path().into();
        let key = |name: &str| SessionKey {
            provider: crate::domain::ProviderId("preview-test".into()),
            source_id: crate::domain::SourceId("local".into()),
            native_id: name.into(),
        };
        // Assign keys after launch so this test needs no external ownership locks.
        for name in ["first", "second", "third"] {
            manager.launch(&launch, None, name.into()).unwrap();
            manager.sessions.last_mut().unwrap().key = Some(key(name));
        }
        let original = manager.active;
        let pids: Vec<_> = manager.sessions.iter().map(|s| s.pid).collect();
        for name in ["first", "second", "first"] {
            manager.sync_preview(Some(temp.path()), Some(&key(name)));
            let expected = manager
                .sessions
                .iter()
                .find(|s| s.key == Some(key(name)))
                .unwrap()
                .id;
            assert_eq!(manager.active, Some(expected));
            manager.sync_preview(Some(temp.path()), Some(&key(name)));
            assert_eq!(manager.active, Some(expected));
            assert_eq!(
                manager.sessions.iter().map(|s| s.pid).collect::<Vec<_>>(),
                pids
            );
        }
        manager.sync_preview(Some(temp.path()), Some(&key("external")));
        assert_eq!(manager.active, original);
        manager.sync_folder(Some(temp.path()));
        assert_eq!(manager.active, original);
        manager.background();
        manager.sync_preview(Some(temp.path()), Some(&key("first")));
        assert!(manager.active.is_some());
        manager.sync_folder(Some(temp.path()));
        assert_eq!(manager.active, None);
        manager.sync_preview(None, Some(&key("second")));
        assert!(manager.active.is_some());
        manager.sync_folder(None);
        assert_eq!(manager.active, None);
        assert!(manager.attach(&key("third")));
        manager.sync_preview(Some(temp.path()), Some(&key("first")));
        manager.sessions[0].stop();
        wait(&mut manager, |m| m.sessions.len() == 2);
        manager.sync_preview(Some(temp.path()), Some(&key("first")));
        assert_eq!(manager.active, original);
        assert!(manager.attach(&key("second")));
        let selected = manager.active;
        manager.sync_folder(Some(temp.path()));
        assert_eq!(manager.active, selected);
    }

    #[test]
    fn real_readline_and_neovim_accept_nested_editing() {
        let temp = tempfile::tempdir().unwrap();
        let mut manager = Manager {
            area: Rect::new(0, 0, 80, 24),
            ..Manager::default()
        };
        let shell = LaunchCommand {
            executable: "/usr/bin/bash".into(),
            args: vec!["--noprofile".into(), "--norc".into()],
            cwd: temp.path().into(),
            env: vec![("PS1".into(), "TRACR_READY> ".into())],
        };
        manager.launch(&shell, None, "readline".into()).unwrap();
        wait(&mut manager, |m| {
            m.sessions[0]
                .parser
                .screen()
                .contents()
                .contains("TRACR_READY>")
        });
        manager
            .input(&Event::Paste("printf 'READLINE:%s\\n' alpha beta".into()))
            .unwrap();
        manager
            .input(&key(KeyCode::Backspace, KeyModifiers::ALT))
            .unwrap();
        manager
            .input(&key(KeyCode::Enter, KeyModifiers::NONE))
            .unwrap();
        wait(&mut manager, |m| {
            m.sessions[0]
                .parser
                .screen()
                .contents()
                .contains("READLINE:alpha")
        });
        assert!(
            !manager.sessions[0]
                .parser
                .screen()
                .contents()
                .contains("READLINE:beta")
        );
        if !std::path::Path::new("/usr/bin/nvim").exists() {
            return;
        }
        let path = temp.path().join("edited.txt");
        let editor = LaunchCommand {
            executable: "/usr/bin/nvim".into(),
            args: vec![
                "--clean".into(),
                "-i".into(),
                "NONE".into(),
                "-n".into(),
                path.as_os_str().to_owned(),
            ],
            cwd: temp.path().into(),
            env: vec![
                (
                    "NVIM_LOG_FILE".into(),
                    temp.path().join("nvim.log").into_os_string(),
                ),
                ("XDG_STATE_HOME".into(), temp.path().into()),
            ],
        };
        manager.launch(&editor, None, "neovim".into()).unwrap();
        wait(&mut manager, |m| {
            m.sessions
                .last()
                .unwrap()
                .parser
                .screen()
                .contents()
                .contains("edited.txt")
        });
        manager
            .input(&key(KeyCode::Char('i'), KeyModifiers::NONE))
            .unwrap();
        wait(&mut manager, |m| {
            m.sessions
                .last()
                .unwrap()
                .parser
                .screen()
                .contents()
                .contains("INSERT")
        });
        manager.input(&Event::Paste("alpha beta".into())).unwrap();
        wait(&mut manager, |m| {
            m.sessions
                .last()
                .unwrap()
                .parser
                .screen()
                .contents()
                .contains("alpha beta")
        });
        manager
            .input(&key(KeyCode::Char('w'), KeyModifiers::CONTROL))
            .unwrap();
        wait(&mut manager, |m| {
            !m.sessions
                .last()
                .unwrap()
                .parser
                .screen()
                .contents()
                .contains("beta")
        });
        manager.input(&Event::Paste("gamma".into())).unwrap();
        wait(&mut manager, |m| {
            m.sessions
                .last()
                .unwrap()
                .parser
                .screen()
                .contents()
                .contains("alpha gamma")
        });
        manager
            .input(&key(KeyCode::Esc, KeyModifiers::NONE))
            .unwrap();
        for c in ":wq".chars() {
            manager
                .input(&key(KeyCode::Char(c), KeyModifiers::NONE))
                .unwrap();
        }
        manager
            .input(&key(KeyCode::Enter, KeyModifiers::NONE))
            .unwrap();
        wait(&mut manager, |_| {
            std::fs::read_to_string(&path).is_ok_and(|text| text == "alpha gamma\n")
        });
        assert_eq!(std::fs::read_to_string(path).unwrap(), "alpha gamma\n");
    }

    #[test]
    #[ignore = "requires tmux and permission to create a local socket"]
    fn nested_tmux_keeps_readline_editing() {
        if !std::path::Path::new("/usr/bin/tmux").exists() {
            return;
        }
        let temp = tempfile::tempdir().unwrap();
        let socket = temp.path().join("tmux.sock");
        struct Server(std::path::PathBuf);
        impl Drop for Server {
            fn drop(&mut self) {
                let _ = std::process::Command::new("/usr/bin/tmux")
                    .arg("-S")
                    .arg(&self.0)
                    .arg("kill-server")
                    .output();
            }
        }
        let _server = Server(socket.clone());
        let command = LaunchCommand {
            executable: "/usr/bin/tmux".into(),
            args: vec![
                "-S".into(),
                socket.into_os_string(),
                "-f".into(),
                "/dev/null".into(),
                "new-session".into(),
                "/usr/bin/bash --noprofile --norc".into(),
            ],
            cwd: temp.path().into(),
            env: vec![("PS1".into(), "NESTED_READY> ".into())],
        };
        let mut manager = Manager {
            area: Rect::new(0, 0, 80, 24),
            ..Manager::default()
        };
        manager.launch(&command, None, "tmux".into()).unwrap();
        wait(&mut manager, |m| {
            m.sessions[0].parser.screen().contents().contains("bash-")
        });
        manager
            .input(&Event::Paste("printf 'NESTED:%s\\n' alpha beta".into()))
            .unwrap();
        manager
            .input(&key(KeyCode::Backspace, KeyModifiers::ALT))
            .unwrap();
        manager
            .input(&key(KeyCode::Enter, KeyModifiers::NONE))
            .unwrap();
        wait(&mut manager, |m| {
            m.sessions[0]
                .parser
                .screen()
                .contents()
                .contains("NESTED:alpha")
        });
        assert!(
            !manager.sessions[0]
                .parser
                .screen()
                .contents()
                .contains("NESTED:beta")
        );
    }

    #[test]
    fn input_queue_preserves_order_under_backpressure_and_reports_disconnect() {
        let mut manager = Manager::default();
        manager
            .launch(
                &command("import time; time.sleep(60)"),
                None,
                "queue".into(),
            )
            .unwrap();
        let (writer, receiver) = mpsc::sync_channel(1);
        let session = &mut manager.sessions[0];
        session.writer = writer;
        for i in 0u32..256 {
            session.write(i.to_le_bytes().to_vec()).unwrap();
        }
        for i in 0u32..256 {
            assert_eq!(
                receiver.recv_timeout(Duration::from_secs(1)).unwrap(),
                i.to_le_bytes()
            );
            session.flush_input().unwrap();
        }
        assert!(session.pending_input.borrow().is_empty());
        drop(receiver);
        assert!(session.write(vec![1]).is_err());
    }

    #[test]
    fn negotiated_pty_receives_modified_keys_paste_mouse_and_focus() {
        let mut manager = Manager {
            area: Rect::new(10, 5, 80, 12),
            visible: true,
            ..Manager::default()
        };
        manager
            .launch(
                &command(
                    r#"
import os,tty,time
tty.setraw(0)
os.write(1,b'\x1b[>1u\x1b[?1004h\x1b[?1000h\x1b[?1006h\x1b[?2004hREADY')
expected=b'\x1b[127;5u\x1b[200~hello\nworld\x1b[201~\x1b[<0;3;2M\x1b[<0;3;2m\x1b[I\x1b[O'
data=b''
while len(data)<len(expected): data+=os.read(0,len(expected)-len(data))
os.write(1,b'\r\nMATCH' if data==expected else b'\r\nMISMATCH:'+data.hex().encode())
time.sleep(60)
"#,
                ),
                None,
                "protocol peer".into(),
            )
            .unwrap();
        wait(&mut manager, |m| {
            m.sessions[0].parser.screen().contents().contains("READY")
        });
        manager
            .input(&key(KeyCode::Backspace, KeyModifiers::CONTROL))
            .unwrap();
        manager.input(&Event::Paste("hello\nworld".into())).unwrap();
        for kind in [
            MouseEventKind::Down(MouseButton::Left),
            MouseEventKind::Up(MouseButton::Left),
        ] {
            manager
                .mouse(MouseEvent {
                    kind,
                    column: 12,
                    row: 6,
                    modifiers: KeyModifiers::NONE,
                })
                .unwrap();
        }
        manager.sync_focus(true).unwrap();
        manager.sync_focus(false).unwrap();
        wait(&mut manager, |m| {
            m.sessions[0].parser.screen().contents().contains("MATCH")
        });
        assert!(
            !manager.sessions[0]
                .parser
                .screen()
                .contents()
                .contains("MISMATCH")
        );
    }

    #[test]
    fn selection_copy_is_explicit_and_ctrl_c_still_reaches_child() {
        let mut manager = Manager {
            area: Rect::new(0, 0, 40, 8),
            visible: true,
            ..Manager::default()
        };
        manager
            .launch(
                &command("import time; print('alpha beta',flush=True); time.sleep(60)"),
                None,
                "selection".into(),
            )
            .unwrap();
        wait(&mut manager, |m| {
            m.sessions[0]
                .parser
                .screen()
                .contents()
                .contains("alpha beta")
        });
        let mouse = MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: 0,
            row: 0,
            modifiers: KeyModifiers::NONE,
        };
        manager.mouse(mouse).unwrap();
        manager
            .mouse(MouseEvent {
                kind: MouseEventKind::Up(MouseButton::Left),
                column: 4,
                ..mouse
            })
            .unwrap();
        assert!(manager.copy_request.is_none());
        manager
            .input(&key(KeyCode::Char('c'), KeyModifiers::ALT))
            .unwrap();
        assert_eq!(manager.copy_request.take().unwrap(), "alpha");
        let (writer, receiver) = mpsc::sync_channel(16);
        manager.sessions[0].writer = writer;
        manager
            .input(&key(KeyCode::Char('c'), KeyModifiers::CONTROL))
            .unwrap();
        assert_eq!(receiver.recv_timeout(Duration::from_secs(1)).unwrap(), [3]);
        assert!(manager.sessions[0].selection.is_none());
    }

    #[test]
    fn picker_enter_reattaches_without_restarting() {
        let mut manager = Manager::default();
        manager
            .launch(
                &command("import time; time.sleep(60)"),
                None,
                "picker".into(),
            )
            .unwrap();
        let id = manager.active;
        let pid = manager.sessions[0].pid;
        manager.background();
        manager.picker = true;
        manager
            .input(&key(KeyCode::Char('f'), KeyModifiers::ALT))
            .unwrap();
        assert!(manager.picker);
        assert!(manager.active.is_none());
        for kind in [KeyEventKind::Repeat, KeyEventKind::Release] {
            manager
                .input(&Event::Key(KeyEvent::new_with_kind(
                    KeyCode::Enter,
                    KeyModifiers::NONE,
                    kind,
                )))
                .unwrap();
            assert!(manager.picker);
        }
        manager
            .input(&key(KeyCode::Enter, KeyModifiers::NONE))
            .unwrap();
        assert!(!manager.picker);
        assert_eq!(manager.active, id);
        assert_eq!(manager.sessions[0].pid, pid);
    }

    #[test]
    fn folder_views_keep_processes_and_explicit_background_choices() {
        let temp = tempfile::tempdir().unwrap();
        let a = temp.path().join("a");
        let b = temp.path().join("b");
        std::fs::create_dir_all(&a).unwrap();
        std::fs::create_dir_all(&b).unwrap();
        let mut manager = Manager {
            area: Rect::new(0, 0, 40, 10),
            ..Manager::default()
        };
        let mut launch = command("import time; print('READY', flush=True); time.sleep(60)");
        launch.cwd = a.clone();
        manager.sync_folder(Some(&a));
        manager.launch(&launch, None, "A".into()).unwrap();
        let first = manager.active.unwrap();
        let pid = manager.sessions[0].pid;
        manager.sync_folder(Some(&b));
        assert_eq!(manager.active, None);
        wait(&mut manager, |m| {
            m.sessions[0].parser.screen().contents().contains("READY")
        });
        launch.cwd = b.clone();
        manager.launch(&launch, None, "B".into()).unwrap();
        let second = manager.active;
        manager.sync_folder(Some(&a));
        assert_eq!(manager.active, Some(first));
        assert_eq!(manager.sessions[0].pid, pid);
        manager
            .input(&key(KeyCode::Char('b'), KeyModifiers::CONTROL))
            .unwrap();
        manager.sync_folder(Some(&b));
        assert_eq!(manager.active, second);
        manager.sync_folder(Some(&a));
        assert_eq!(manager.active, None); // Ctrl+B is remembered.
        manager.activate(first);

        manager.sync_folder(Some(&a));

        manager.sync_folder(None);
        assert_eq!(manager.active, None);
        manager.sync_folder(Some(&a));
        assert_eq!(manager.active, Some(first));

        // A delayed launch in B must not replace A's view.
        manager.launch(&launch, None, "B newer".into()).unwrap();
        let newer = manager.active;
        manager.sync_folder(Some(&a));
        assert_eq!(manager.active, Some(first));
        manager.sync_folder(Some(&b));
        assert_eq!(manager.active, newer);
        manager.sessions.last_mut().unwrap().stop();
        wait(&mut manager, |m| m.sessions.len() == 2);
        manager.sync_folder(Some(&a));
        manager.sync_folder(Some(&b));
        assert_eq!(manager.active, None); // Do not pick another terminal after exit.
        // The running-terminal picker explicitly restores its owner's view.
        manager.picker = true;
        manager.selected = 0;
        manager
            .input(&key(KeyCode::Enter, KeyModifiers::NONE))
            .unwrap();
        assert_eq!(manager.active_folder(), Some(a));
    }

    fn history(parser: &mut vt100::Parser, count: usize) {
        // Codex-style history above a two-line composer: scroll top region only.
        parser.process(b"\x1b[5;1HINPUT\x1b[1;4r\x1b[4;1H");
        for n in 0..count {
            parser.process(format!("\r\n\x1b[31mrow{n:04}界\x1b[0m").as_bytes());
        }
        parser.process(b"\x1b[r\x1b[5;6H");
    }

    #[test]
    fn partial_region_history_is_retained_without_composer_duplicates() {
        let mut parser = vt100::Parser::new(6, 24, 2000);
        history(&mut parser, 30);
        assert!(parser.screen().contents().contains("INPUT"));
        parser.set_scrollback(10);
        assert!(!parser.screen().contents().contains("INPUT"));
        assert!(parser.screen().contents().contains("row0016界"));
        assert_eq!(
            parser.screen().cell(0, 0).unwrap().fgcolor(),
            vt100::Color::Idx(1)
        );
        let before = parser.screen().contents();
        parser.process(b"\x1b[1;4r\x1b[4;1H\r\nnew\x1b[r");
        assert_eq!(parser.screen().contents(), before);
        assert_eq!(parser.screen().scrollback(), 11);
        parser.set_scrollback(0);
        history(&mut parser, 2100);
        parser.set_scrollback(usize::MAX);
        assert_eq!(parser.screen().scrollback(), 2000);
        parser.set_size(8, 30);
        assert_eq!(parser.screen().scrollback(), 2000);
        parser.set_scrollback(0);
        parser.process(b"\x1b[?1049h");
        history(&mut parser, 20);
        parser.set_scrollback(100);
        assert_eq!(parser.screen().scrollback(), 0);
        parser.process(b"\x1b[?1049l");
        parser.set_scrollback(usize::MAX);
        assert_eq!(parser.screen().scrollback(), 2000);
        let mut interior = vt100::Parser::new(6, 24, 2000);
        interior.process(b"\x1b[2;4r\x1b[4;1H");
        interior.process(b"x\r\n".repeat(20).as_slice());
        interior.set_scrollback(100);
        assert_eq!(interior.screen().scrollback(), 0);
    }

    #[test]
    fn wheel_scrolls_history_without_child_keys_and_preserves_folder_position() {
        let mut manager = Manager {
            area: Rect::new(10, 2, 24, 6),
            ..Manager::default()
        };
        manager
            .launch(
                &command("import time; time.sleep(60)"),
                None,
                "scroll".into(),
            )
            .unwrap();
        let folder = manager.active_folder().unwrap();
        manager.sync_folder(Some(&folder));
        manager.visible = true;
        history(&mut manager.sessions[0].parser, 30);
        let (writer, input) = mpsc::sync_channel(16);
        manager.sessions[0].writer = writer;
        let mouse = MouseEvent {
            kind: MouseEventKind::ScrollUp,
            column: 12,
            row: 3,
            modifiers: KeyModifiers::NONE,
        };
        manager.wheel(mouse).unwrap();
        assert_eq!(manager.sessions[0].parser.screen().scrollback(), 3);
        manager.sync_folder(None);
        manager.sync_folder(Some(&folder));
        assert_eq!(manager.sessions[0].parser.screen().scrollback(), 3);
        manager.wheel(MouseEvent { column: 1, ..mouse }).unwrap();
        assert_eq!(manager.sessions[0].parser.screen().scrollback(), 3);
        manager.picker = true;
        manager.wheel(mouse).unwrap();
        assert_eq!(manager.sessions[0].parser.screen().scrollback(), 3);
        manager.picker = false;
        manager
            .wheel(MouseEvent {
                kind: MouseEventKind::ScrollDown,
                ..mouse
            })
            .unwrap();
        assert_eq!(manager.sessions[0].parser.screen().scrollback(), 0);
        manager.wheel(mouse).unwrap();
        assert!(input.try_recv().is_err()); // Wheel browsing sends no child input.
        manager.input(&Event::Paste("hello".into())).unwrap();
        assert_eq!(input.try_recv().unwrap(), b"hello");
        assert_eq!(manager.sessions[0].parser.screen().scrollback(), 0);
        manager.wheel(mouse).unwrap();
        manager
            .input(&key(KeyCode::Char('a'), KeyModifiers::NONE))
            .unwrap();
        assert_eq!(manager.sessions[0].parser.screen().scrollback(), 0);
        assert_eq!(input.try_recv().unwrap(), b"a");
        manager.sessions[0]
            .parser
            .process(b"\x1b[?1000h\x1b[?1006h");
        manager.wheel(mouse).unwrap();
        assert_eq!(input.try_recv().unwrap(), b"\x1b[<64;3;2M");
        manager.sessions[0]
            .parser
            .process(b"\x1b[?1000l\x1b[?1049h");
        manager.sessions[0].alternate_scroll = false;
        manager.wheel(mouse).unwrap();
        assert!(input.try_recv().is_err());
        manager.sessions[0].alternate_scroll = true;
        manager.wheel(mouse).unwrap();
        assert_eq!(input.try_recv().unwrap(), b"\x1b[A\x1b[A\x1b[A");
    }

    #[test]
    fn wheel_protocol_coordinates_modifiers_and_alternate_scroll_modes() {
        let area = Rect::new(10, 4, 400, 20);
        let mouse = MouseEvent {
            kind: MouseEventKind::ScrollUp,
            column: 12,
            row: 5,
            modifiers: KeyModifiers::CONTROL,
        };
        assert_eq!(
            encode_mouse(mouse, area, vt100::MouseProtocolEncoding::Sgr),
            b"\x1b[<80;3;2M"
        );
        assert_eq!(
            encode_mouse(mouse, area, vt100::MouseProtocolEncoding::Default),
            b"\x1b[Mp#\""
        );
        assert_eq!(
            encode_mouse(mouse, area, vt100::MouseProtocolEncoding::Utf8),
            b"\x1b[Mp#\""
        );
        assert!(
            encode_mouse(
                MouseEvent {
                    column: 300,
                    ..mouse
                },
                area,
                vt100::MouseProtocolEncoding::Default
            )
            .is_empty()
        );
        let mut parser = vte::Parser::new();
        let mut queries = Queries {
            modes: Default::default(),
            reset: false,
            alternate_scroll: true,
            cursor: (0, 0),
            replies: vec![],
        };
        for byte in b"\x1b[?1007l" {
            parser.advance(&mut queries, *byte);
        }
        assert!(!queries.alternate_scroll);
        for byte in b"\x1b[?1;1007h" {
            parser.advance(&mut queries, *byte);
        }
        assert!(queries.alternate_scroll);
    }

    #[test]
    fn keys_preserve_child_controls() {
        for (code, mods, expected) in [
            (
                KeyCode::Char('c'),
                KeyModifiers::CONTROL,
                b"\x03".as_slice(),
            ),
            (KeyCode::Esc, KeyModifiers::NONE, b"\x1b"),
            (KeyCode::Tab, KeyModifiers::NONE, b"\t"),
            (KeyCode::Enter, KeyModifiers::ALT, b"\x1b\r"),
            (KeyCode::Up, KeyModifiers::NONE, b"\x1b[A"),
            (KeyCode::Left, KeyModifiers::CONTROL, b"\x1b[1;5D"),
            (KeyCode::Char('é'), KeyModifiers::NONE, "é".as_bytes()),
        ] {
            assert_eq!(encode_key(KeyEvent::new(code, mods), false), expected);
        }
        assert_eq!(
            encode_key(KeyEvent::new(KeyCode::Up, KeyModifiers::NONE), true),
            b"\x1bOA"
        );
    }
    #[test]
    fn terminal_queries_fragmented_and_confined() {
        let mut parser = vte::Parser::new();
        let mut queries = Queries {
            modes: Default::default(),
            reset: false,
            alternate_scroll: true,
            cursor: (3, 7),
            replies: vec![],
        };
        for byte in b"\x1b[6n\x1b]11;?\x07\x1b[?u" {
            parser.advance(&mut queries, *byte);
        }
        assert_eq!(queries.replies[0], b"\x1b[4;8R");
        assert_eq!(queries.replies[1], b"\x1b]11;rgb:0000/0000/0000\x1b\\");
        assert_eq!(queries.replies[2], b"\x1b[?0u");
    }
    #[test]
    fn navigator_highlights_scrolls_and_keeps_close_confirmation() {
        let browser = crate::browser::Browser::new(vec![]);
        let mut manager = Manager {
            picker: true,
            ..Manager::default()
        };
        for title in ["First", "Second", "Last 界"] {
            manager
                .launch(&command("import time; time.sleep(30)"), None, title.into())
                .unwrap();
        }
        manager
            .input(&key(KeyCode::Char('j'), KeyModifiers::NONE))
            .unwrap();
        manager
            .input(&key(KeyCode::Char('j'), KeyModifiers::NONE))
            .unwrap();
        assert_eq!(manager.selected, 2);
        for (width, height) in [(100, 12), (35, 7), (12, 5), (1, 1)] {
            let mut terminal =
                ratatui::Terminal::new(ratatui::backend::TestBackend::new(width, height)).unwrap();
            terminal
                .draw(|frame| manager.draw_overlay(frame, &browser))
                .unwrap();
            let buffer = terminal.backend().buffer();
            if width >= 35 {
                let text = buffer
                    .content
                    .iter()
                    .map(|cell| cell.symbol())
                    .collect::<String>();
                assert!(text.contains("Last 界"));
                let highlighted: Vec<_> = buffer
                    .content
                    .iter()
                    .filter(|cell| cell.modifier.contains(Modifier::BOLD))
                    .collect();
                assert!(!highlighted.is_empty());
                assert!(highlighted.iter().all(|cell| cell.fg == Color::Reset));
                assert!(highlighted.iter().any(|cell| cell.symbol() == ">"));
                assert!(buffer.content.iter().all(|cell| cell.bg == Color::Reset));
                assert!(buffer.content.iter().all(|cell| if cell.symbol() == "🮇" {
                    matches!(cell.fg, Color::Rgb(56, 56, 56) | Color::Rgb(112, 112, 112))
                } else {
                    matches!(cell.fg, Color::Reset | Color::Cyan | Color::DarkGray)
                }));
                assert!(
                    buffer
                        .content
                        .iter()
                        .any(|cell| cell.symbol() == "┏" && cell.fg == Color::Cyan)
                );
                assert!(
                    buffer
                        .content
                        .iter()
                        .any(|cell| cell.symbol() == "┓" && cell.fg == Color::Cyan)
                );
                if width >= 100 {
                    assert!(text.contains("terminal"));
                    assert!(text.contains("open"));
                }
            }
        }
        manager
            .input(&key(KeyCode::Char('k'), KeyModifiers::NONE))
            .unwrap();
        assert_eq!(manager.selected, 1);
        manager
            .input(&key(KeyCode::Char('x'), KeyModifiers::NONE))
            .unwrap();
        assert!(matches!(manager.confirmation, Some(Confirmation::Close(_))));
        for quit in [false, true] {
            if quit {
                manager.confirmation = Some(Confirmation::Quit);
            }
            for (width, height) in [(100, 12), (35, 12), (12, 5), (1, 1)] {
                let mut terminal =
                    ratatui::Terminal::new(ratatui::backend::TestBackend::new(width, height))
                        .unwrap();
                terminal
                    .draw(|frame| manager.draw_overlay(frame, &browser))
                    .unwrap();
                let buffer = terminal.backend().buffer();
                if width >= 35 {
                    let text = buffer
                        .content
                        .iter()
                        .map(|cell| cell.symbol())
                        .collect::<String>();
                    assert!(text.contains("Second"));
                    assert_eq!(text.contains("First"), quit);
                    assert_eq!(text.contains("Last 界"), quit);
                    assert!(text.contains("y Confirm · Esc/n Cancel"));
                    assert!(
                        buffer
                            .content
                            .iter()
                            .any(|cell| cell.symbol() == "┏" && cell.fg == Color::Cyan)
                    );
                    assert!(buffer.content.iter().all(|cell| cell.bg == Color::Reset));
                    if width == 100 {
                        assert!(text.contains("Running work will stop."));
                        assert!(text.contains("terminal"));
                        assert!(text.contains("open"));
                    }
                }
            }
        }
        manager
            .input(&key(KeyCode::Esc, KeyModifiers::NONE))
            .unwrap();
        assert!(manager.confirmation.is_none() && manager.picker);
        manager
            .input(&key(KeyCode::Char('g'), KeyModifiers::NONE))
            .unwrap();
        assert!(!manager.picker);
        manager.picker = true;
        manager
            .input(&key(KeyCode::Enter, KeyModifiers::NONE))
            .unwrap();
        assert_eq!(manager.active, Some(manager.sessions[1].id));
        assert!(!manager.picker);
    }

    #[test]
    fn tabs_left_arrow_and_alt_f_reach_child() {
        let mut manager = Manager {
            area: Rect::new(0, 0, 80, 24),
            ..Manager::default()
        };
        let script = "import os,tty\ntty.setraw(0)\nprint('READY',flush=True)\nbuf=b''\nwhile True:\n b=os.read(0,1)\n buf+=b\n if b==b'\\r':\n  os.write(1,buf.hex().encode()+b'\\r\\n')\n  buf=b''";
        manager
            .launch(&command(script), None, "test".into())
            .unwrap();
        wait(&mut manager, |m| {
            m.sessions[0].parser.screen().contents().contains("READY")
        });
        let active = manager.active;
        let pid = manager.sessions[0].pid;
        manager.input(&Event::Paste("draft".into())).unwrap();
        let mut expected = b"draft".to_vec();
        manager
            .input(&key(KeyCode::Char('f'), KeyModifiers::ALT))
            .unwrap();
        expected.extend_from_slice(b"\x1bf");
        manager
            .input(&key(KeyCode::Left, KeyModifiers::NONE))
            .unwrap();
        expected.extend_from_slice(b"\x1b[D");
        {
            for (code, modifiers) in [
                (KeyCode::Tab, KeyModifiers::NONE),
                (KeyCode::Tab, KeyModifiers::SHIFT),
                (KeyCode::BackTab, KeyModifiers::SHIFT),
                (KeyCode::BackTab, KeyModifiers::NONE),
            ] {
                for kind in [
                    KeyEventKind::Release,
                    KeyEventKind::Press,
                    KeyEventKind::Repeat,
                ] {
                    let result = manager
                        .input(&Event::Key(KeyEvent::new_with_kind(code, modifiers, kind)))
                        .unwrap();
                    assert!(matches!(result, Input::Consumed));
                    if kind != KeyEventKind::Release {
                        expected.extend_from_slice(
                            if code == KeyCode::Tab && modifiers == KeyModifiers::NONE {
                                b"\t"
                            } else {
                                b"\x1b[Z"
                            },
                        );
                    }

                    assert_eq!(manager.active, active);
                    assert_eq!(manager.sessions[0].pid, pid);
                }
            }
        }
        manager
            .input(&key(KeyCode::Enter, KeyModifiers::NONE))
            .unwrap();
        expected.push(b'\r');
        let expected = expected
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>();
        wait(&mut manager, |m| {
            m.sessions[0].parser.screen().contents().contains(&expected)
        });
        for modifiers in [KeyModifiers::CONTROL, KeyModifiers::ALT] {
            assert!(matches!(
                manager.input(&key(KeyCode::Tab, modifiers)).unwrap(),
                Input::Consumed
            ));
        }
        manager.picker = true;
        assert!(matches!(
            manager
                .input(&key(KeyCode::Tab, KeyModifiers::NONE))
                .unwrap(),
            Input::Consumed
        ));
        assert!(manager.picker);
    }

    #[test]
    fn background_output_reattach_input_resize_and_render() {
        let mut manager = Manager {
            area: Rect::new(0, 0, 40, 10),
            ..Manager::default()
        };
        let script = "import os,tty,time\ntty.setraw(0)\nprint('READY',flush=True)\ntime.sleep(.1)\nprint('BACKGROUND',flush=True)\nwhile True:\n b=os.read(0,1024)\n os.write(1,b.hex().encode()+b'\\r\\n')";
        let session_key = SessionKey {
            provider: crate::domain::ProviderId("test".into()),
            source_id: crate::domain::SourceId("test".into()),
            native_id: format!("test-{}", std::process::id()),
        };
        manager
            .launch(&command(script), Some(session_key.clone()), "test".into())
            .unwrap();
        wait(&mut manager, |m| {
            m.sessions[0].parser.screen().contents().contains("READY")
        });
        let pid = manager.sessions[0].pid;
        assert!(matches!(
            manager
                .input(&key(KeyCode::Char('f'), KeyModifiers::ALT))
                .unwrap(),
            Input::Consumed
        ));

        manager
            .input(&Event::Key(KeyEvent::new_with_kind(
                KeyCode::Char('f'),
                KeyModifiers::ALT,
                KeyEventKind::Repeat,
            )))
            .unwrap();

        for (direction, expected) in [
            (KeyCode::Left, "1b5b313b3344"),
            (KeyCode::Right, "1b5b313b3343"),
            (KeyCode::Up, "1b5b313b3341"),
            (KeyCode::Down, "1b5b313b3342"),
        ] {
            assert!(matches!(
                manager.input(&key(direction, KeyModifiers::ALT)).unwrap(),
                Input::Consumed
            ));

            assert!(manager.active.is_some());
            assert_eq!(manager.sessions[0].pid, pid);
            wait(&mut manager, |m| {
                m.sessions[0].parser.screen().contents().contains(expected)
            });
        }
        manager
            .input(&key(KeyCode::Char('f'), KeyModifiers::ALT))
            .unwrap();

        assert_eq!(manager.sessions[0].pid, pid);
        for direction in [KeyCode::Left, KeyCode::Right, KeyCode::Up, KeyCode::Down] {
            assert!(matches!(
                manager.input(&key(direction, KeyModifiers::ALT)).unwrap(),
                Input::Consumed
            ));
            assert!(manager.active.is_some());
        }

        manager
            .input(&key(KeyCode::Char('f'), KeyModifiers::ALT))
            .unwrap();

        assert!(matches!(
            manager
                .input(&key(KeyCode::Char('b'), KeyModifiers::CONTROL))
                .unwrap(),
            Input::Consumed
        ));
        assert_eq!(manager.active, None);

        wait(&mut manager, |m| {
            m.sessions[0]
                .parser
                .screen()
                .contents()
                .contains("BACKGROUND")
        });
        assert!(manager.attach(&session_key));
        assert_eq!(manager.sessions[0].pid, pid);
        manager
            .input(&key(KeyCode::Char('c'), KeyModifiers::CONTROL))
            .unwrap();
        wait(&mut manager, |m| {
            m.sessions[0].parser.screen().contents().contains("03")
        });
        manager.sessions[0].parser.process(b"\x1b[?2004h");
        manager.input(&Event::Paste("a\nb".into())).unwrap();
        wait(&mut manager, |m| {
            m.sessions[0]
                .parser
                .screen()
                .contents()
                .contains("1b5b3230307e610a621b5b3230317e")
        });
        let mut terminal =
            ratatui::Terminal::new(ratatui::backend::TestBackend::new(50, 20)).unwrap();
        terminal
            .draw(|frame| {
                frame.render_widget(Paragraph::new("OUTSIDE"), Rect::new(0, 0, 10, 1));
                manager.draw(frame, Rect::new(10, 2, 30, 12), true, true);
            })
            .unwrap();
        assert_eq!(manager.sessions[0].size, (10, 28));
        assert_eq!(terminal.backend().buffer()[(0, 0)].symbol(), "O");
        assert_eq!(terminal.backend().buffer()[(10, 2)].symbol(), "┏");
        assert_eq!(terminal.backend().buffer()[(10, 2)].fg, Color::Cyan);
        terminal
            .draw(|frame| {
                manager.draw(frame, Rect::new(10, 2, 30, 12), false, true);
            })
            .unwrap();
        assert_eq!(terminal.backend().buffer()[(10, 2)].symbol(), "┌");
        assert_eq!(terminal.backend().buffer()[(10, 2)].fg, Color::DarkGray);

        assert!(!manager.request_quit());
        assert!(matches!(
            manager
                .input(&key(KeyCode::Esc, KeyModifiers::NONE))
                .unwrap(),
            Input::Consumed
        ));
        assert!(!manager.is_empty());
        manager.request_close(Some(&session_key));
        manager
            .input(&key(KeyCode::Char('y'), KeyModifiers::NONE))
            .unwrap();
        wait(&mut manager, |m| m.is_empty());
    }
    #[test]
    fn fast_failure_keeps_final_diagnostic_output() {
        let mut manager = Manager {
            area: Rect::new(0, 0, 60, 10),
            ..Manager::default()
        };
        manager
            .launch(
                &command("import sys; print('specific startup failure', flush=True); sys.exit(7)"),
                None,
                "failed".into(),
            )
            .unwrap();

        let deadline = Instant::now() + Duration::from_secs(5);
        let notices = loop {
            let (_, notices) = manager.poll();
            if !notices.is_empty() {
                break notices;
            }
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(10));
        };
        assert!(notices[0].failed && notices[0].was_active);
        assert!(notices[0].message.contains("specific startup failure"));
        assert!(manager.is_empty());
    }

    #[test]
    fn independent_managers_can_open_the_same_session() {
        let key = SessionKey {
            provider: crate::domain::ProviderId("test".into()),
            source_id: crate::domain::SourceId("test".into()),
            native_id: "shared-session".into(),
        };
        let cmd =
            command("print('READY', flush=True); input(); print('ALIVE', flush=True); input()");
        let mut first = Manager::default();
        let mut second = Manager::default();
        for manager in [&mut first, &mut second] {
            manager.area = Rect::new(0, 0, 80, 24);
            manager
                .launch(&cmd, Some(key.clone()), "shared".into())
                .unwrap();
            wait(manager, |m| {
                m.sessions[0].parser.screen().contents().contains("READY")
            });
        }
        assert_ne!(first.sessions[0].pid, second.sessions[0].pid);
        let pid = first.sessions[0].pid;
        assert!(first.attach(&key));
        assert_eq!(first.sessions.len(), 1);
        assert_eq!(first.sessions[0].pid, pid);
        drop(second);
        first.sessions[0].write(b"\r".to_vec()).unwrap();
        wait(&mut first, |m| {
            m.sessions[0].parser.screen().contents().contains("ALIVE")
        });
    }

    #[test]
    fn failed_spawn_does_not_prevent_retry() {
        let key = SessionKey {
            provider: crate::domain::ProviderId("test".into()),
            source_id: crate::domain::SourceId("test".into()),
            native_id: "retry-session".into(),
        };
        let cmd = command("print('READY', flush=True); input()");
        let mut missing = cmd.clone();
        missing.executable = "/no-such-tracr-test-program".into();
        let mut manager = Manager {
            area: Rect::new(0, 0, 80, 24),
            ..Manager::default()
        };
        assert!(
            manager
                .launch(&missing, Some(key.clone()), "missing".into())
                .is_err()
        );
        assert!(manager.is_empty());
        assert!(manager.active.is_none());
        manager.launch(&cmd, Some(key), "retry".into()).unwrap();
        wait(&mut manager, |m| {
            m.sessions[0].parser.screen().contents().contains("READY")
        });
    }
}
