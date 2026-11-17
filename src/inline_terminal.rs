//! Owned PTYs. Output is parsed, never written to the outer terminal.
use crate::domain::{LaunchCommand, SessionKey};
use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use portable_pty::{CommandBuilder, MasterPty, PtySize};
use ratatui::{
    Frame,
    layout::Rect,
    style::{Color, Modifier, Style},
    widgets::{Block, BorderType, Borders, Clear, Paragraph},
};
use std::{
    fs::{File, OpenOptions},
    hash::{Hash, Hasher},
    io::{self, Read, Write},
    os::{
        fd::AsRawFd,
        unix::fs::{DirBuilderExt, OpenOptionsExt},
    },
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

struct Session {
    id: u64,
    key: Option<SessionKey>,
    title: String,
    master: Box<dyn MasterPty + Send>,
    child: Box<dyn portable_pty::Child + Send + Sync>,
    writer: mpsc::SyncSender<Vec<u8>>,
    output: mpsc::Receiver<Vec<u8>>,
    parser: vt100::Parser,
    queries: vte::Parser,
    pid: Option<u32>,
    size: (u16, u16),
    stopping: Option<Instant>,
    exit: Option<Instant>,
    io_stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
    output_closed: bool,
    _lock: Option<SessionLock>,
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
        self.writer
            .try_send(bytes)
            .map_err(|e| io::Error::other(format!("Terminal input unavailable: {e}")))
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

fn ownership_path(home: Option<std::path::PathBuf>, key: &SessionKey) -> std::path::PathBuf {
    let mut hash = std::collections::hash_map::DefaultHasher::new();
    home.hash(&mut hash);
    key.provider.hash(&mut hash);
    key.native_id.hash(&mut hash);
    // SAFETY: getuid has no preconditions.
    std::path::PathBuf::from("/tmp")
        .join(format!("tracr-terminals-{}", unsafe { libc::getuid() }))
        .join(format!("{:016x}.lock", hash.finish()))
}

/// Read-only ownership hint. Launch still acquires the lock atomically.
pub fn open_elsewhere(home: &std::path::Path, key: &SessionKey) -> bool {
    let Ok(home) = std::fs::canonicalize(home) else {
        return false;
    };
    let Ok(file) = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(ownership_path(Some(home), key))
    else {
        return false;
    };
    // SAFETY: descriptor is live. Never unlock another owner's file description.
    if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0 {
        drop(SessionLock(file));
        false
    } else {
        io::Error::last_os_error().raw_os_error() == Some(libc::EWOULDBLOCK)
    }
}

struct SessionLock(File);
impl Drop for SessionLock {
    fn drop(&mut self) {
        // SAFETY: the file remains live. Explicit unlock also releases references
        // briefly inherited by concurrent forks before their close-on-exec.
        unsafe {
            libc::flock(self.0.as_raw_fd(), libc::LOCK_UN);
        }
    }
}

/// Kernel locks survive neither process exit nor exec into the child. Never unlink
/// lock files: unlinking would allow two owners to lock different inodes.
fn session_lock(command: &LaunchCommand, key: &SessionKey) -> io::Result<SessionLock> {
    let home = command
        .env
        .iter()
        .find(|(k, _)| k == "CODEX_HOME")
        .map(|(_, v)| std::path::PathBuf::from(v));
    let home = home.map(std::fs::canonicalize).transpose()?;
    let path = ownership_path(home, key);
    let root = path.parent().expect("ownership directory");
    match std::fs::DirBuilder::new().mode(0o700).create(root) {
        Ok(()) => (),
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => (),
        Err(e) => return Err(e),
    }
    use std::os::unix::fs::MetadataExt;
    let metadata = std::fs::symlink_metadata(root)?;
    if !metadata.is_dir()
        || metadata.uid() != unsafe { libc::getuid() }
        || metadata.mode() & 0o077 != 0
    {
        return Err(io::Error::other("Unsafe terminal ownership directory"));
    }
    let mut file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)?;
    // SAFETY: descriptor is live and owned by file.
    if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
        return Err(io::Error::other(
            "Session is open in another tracr instance. Return to that instance to use or close its terminal.",
        ));
    }
    file.set_len(0)?;
    write!(file, "{}", std::process::id())?;
    Ok(SessionLock(file))
}

#[derive(Default)]
pub struct Manager {
    sessions: Vec<Session>,
    next_id: u64,
    pub active: Option<u64>,
    pub fullscreen: bool,
    pub picker: bool,
    selected: usize,
    confirmation: Option<Confirmation>,
    pub area: Rect,
}
enum Confirmation {
    Quit,
    Close(u64),
}
pub struct ExitNotice {
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
    pub fn is_empty(&self) -> bool {
        self.sessions.is_empty()
    }
    pub fn owns(&self, key: &SessionKey) -> bool {
        self.sessions.iter().any(|s| s.key.as_ref() == Some(key))
    }
    pub fn attach(&mut self, key: &SessionKey) -> bool {
        if let Some(s) = self.sessions.iter().find(|s| s.key.as_ref() == Some(key)) {
            self.fullscreen = false;
            self.active = Some(s.id);
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
        let lock = key.as_ref().map(|k| session_lock(command, k)).transpose()?;
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
                        Ok(0) => return,
                        Ok(n) => remaining = &remaining[n..],
                        Err(error)
                            if matches!(
                                error.kind(),
                                io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted
                            ) =>
                        {
                            continue;
                        }
                        Err(_) => return,
                    }
                }
            }
        });
        self.next_id += 1;
        self.fullscreen = false;
        self.active = Some(self.next_id);
        self.sessions.push(Session {
            id: self.next_id,
            key,
            title,
            master: pair.master,
            child,
            writer: input,
            output,
            parser: vt100::Parser::new(rows, cols, 2000),
            queries: vte::Parser::new(),
            pid,
            size: (rows, cols),
            stopping: None,
            exit: None,
            io_stop,
            output_closed: false,
            _lock: lock,
        });
        Ok(())
    }
    pub fn poll(&mut self) -> (bool, Vec<ExitNotice>) {
        let mut changed = false;
        let mut exited = Vec::new();
        self.sessions.retain_mut(|s| {
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
                for byte in bytes {
                    s.parser.process(&[byte]);
                    let mut queries = Queries {
                        cursor: s.parser.screen().cursor_position(),
                        replies: Vec::new(),
                    };
                    s.queries.advance(&mut queries, byte);
                    for reply in queries.replies {
                        let _ = s.write(reply);
                    }
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
                        message: format!("{} exited: {status}{details}", s.title),
                        failed: !status.success() && s.stopping.is_none(),
                        was_active: self.active == Some(s.id),
                    });
                    if self.active == Some(s.id) {
                        self.fullscreen = false;
                        self.active = None;
                    }
                    false
                }
                Err(error) => {
                    exited.push(ExitNotice {
                        message: format!("{}: {error}", s.title),
                        failed: true,
                        was_active: self.active == Some(s.id),
                    });
                    if self.active == Some(s.id) {
                        self.fullscreen = false;
                        self.active = None;
                    }
                    false
                }
                Ok(None) => true,
            }
        });
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
            self.fullscreen = false;
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
    pub fn has_overlay(&self) -> bool {
        self.picker || self.confirmation.is_some()
    }
    pub fn input(&mut self, event: &Event) -> io::Result<Input> {
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
                    KeyCode::Esc | KeyCode::Char('b') => self.picker = false,
                    KeyCode::Up => self.selected = self.selected.saturating_sub(1),
                    KeyCode::Down => {
                        self.selected =
                            (self.selected + 1).min(self.sessions.len().saturating_sub(1))
                    }
                    KeyCode::Enter => {
                        if let Some(s) = self.sessions.get(self.selected) {
                            self.fullscreen = false;
                            self.active = Some(s.id);
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
        // Pane navigation belongs to tracr even while the child has input focus.
        if matches!(event, Event::Key(key) if key.modifiers == KeyModifiers::ALT && matches!(key.code, KeyCode::Left | KeyCode::Right | KeyCode::Up | KeyCode::Down))
        {
            self.fullscreen = false;
            return Ok(Input::Pass);
        }
        let Some(id) = self.active else {
            return Ok(Input::Pass);
        };
        if matches!(event, Event::Key(key) if key.code == KeyCode::Char('f') && key.modifiers == KeyModifiers::ALT)
        {
            if matches!(event, Event::Key(key) if key.kind == KeyEventKind::Press) {
                self.fullscreen = !self.fullscreen;
            }
            return Ok(Input::Consumed);
        }

        if matches!(event, Event::Key(k) if k.kind != KeyEventKind::Release && k.code == KeyCode::Char('b') && k.modifiers.contains(KeyModifiers::CONTROL))
        {
            self.fullscreen = false;
            self.active = None;
            return Ok(Input::Consumed);
        }
        if let Some(s) = self.sessions.iter().find(|s| s.id == id) {
            let bytes = match event {
                Event::Key(key) => encode_key(*key, s.parser.screen().application_cursor()),
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
            }
        }
        Ok(Input::Consumed)
    }
    pub fn draw(&mut self, frame: &mut Frame<'_>, area: Rect, focused: bool) -> bool {
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
            .title(" Codex · Ctrl+B background · Alt+arrows panels ");
        self.area = if self.fullscreen {
            area
        } else {
            border.inner(area)
        };
        let Some(s) = self.sessions.iter_mut().find(|s| Some(s.id) == self.active) else {
            return false;
        };
        if !self.fullscreen {
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
            s.parser.set_size(inner.height, inner.width);
            s.size = (inner.height, inner.width);
        }
        let screen = s.parser.screen();
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
        if focused && !screen.hide_cursor() && row < inner.height && col < inner.width {
            frame.set_cursor_position((inner.x + col, inner.y + row));
        }
        true
    }
    pub fn draw_overlay(&self, frame: &mut Frame<'_>) {
        if !self.picker && self.confirmation.is_none() {
            return;
        }
        let area = frame.area();
        let area = Rect::new(
            area.x + area.width / 10,
            area.y + area.height / 10,
            area.width * 8 / 10,
            area.height * 8 / 10,
        );
        frame.render_widget(Clear, area);
        let mut lines = Vec::new();
        if let Some(c) = &self.confirmation {
            lines.push(match c {
                Confirmation::Quit => {
                    "Close all owned terminals and quit? Running work will stop.".into()
                }
                Confirmation::Close(_) => "Close this terminal? Running work will stop.".into(),
            });
            lines.push("y Confirm · Esc Cancel".into());
        } else {
            lines.push("Enter Attach · x Close terminal · Esc Back".into());
        }
        for (index, s) in self.sessions.iter().enumerate() {
            if matches!(&self.confirmation, Some(Confirmation::Close(id)) if *id != s.id) {
                continue;
            }
            lines.push(format!(
                "{} #{} {}{}",
                if index == self.selected { ">" } else { " " },
                s.id,
                crate::browser::safe_text(&s.title),
                if s.stopping.is_some() {
                    " (closing)"
                } else {
                    ""
                }
            ));
        }
        if self.sessions.is_empty() {
            lines.push("No open terminals".into());
        }
        let offset = self
            .selected
            .saturating_sub(usize::from(area.height.saturating_sub(5)));
        if self.confirmation.is_none() && offset > 0 {
            lines.drain(1..=offset);
        }
        frame.render_widget(
            Paragraph::new(lines.join("\n")).block(
                Block::default()
                    .title(" Open terminals ")
                    .borders(Borders::ALL),
            ),
            area,
        );
    }
}
fn color(c: vt100::Color) -> Color {
    match c {
        vt100::Color::Default => Color::Reset,
        vt100::Color::Idx(n) => Color::Indexed(n),
        vt100::Color::Rgb(r, g, b) => Color::Rgb(r, g, b),
    }
}

fn encode_key(key: KeyEvent, application: bool) -> Vec<u8> {
    if key.kind == KeyEventKind::Release {
        return vec![];
    }
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    let mut text = match key.code {
        KeyCode::Char(c) if ctrl && c.is_ascii() => vec![(c.to_ascii_uppercase() as u8) & 0x1f],
        KeyCode::Char(c) => c.to_string().into_bytes(),
        KeyCode::Enter if key.modifiers.contains(KeyModifiers::SHIFT) => vec![b'\n'],
        KeyCode::Enter => vec![b'\r'],
        KeyCode::Tab => vec![b'\t'],
        KeyCode::BackTab => b"\x1b[Z".to_vec(),
        KeyCode::Backspace => vec![127],
        KeyCode::Esc => vec![27],
        KeyCode::Up
        | KeyCode::Down
        | KeyCode::Right
        | KeyCode::Left
        | KeyCode::Home
        | KeyCode::End => {
            let suffix = match key.code {
                KeyCode::Up => 'A',
                KeyCode::Down => 'B',
                KeyCode::Right => 'C',
                KeyCode::Left => 'D',
                KeyCode::Home => 'H',
                _ => 'F',
            };
            let modifier = 1
                + u8::from(key.modifiers.contains(KeyModifiers::SHIFT))
                + 2 * u8::from(key.modifiers.contains(KeyModifiers::ALT))
                + 4 * u8::from(ctrl);
            if modifier > 1 {
                format!("\x1b[1;{modifier}{suffix}").into_bytes()
            } else {
                format!("\x1b{}{suffix}", if application { 'O' } else { '[' }).into_bytes()
            }
        }
        KeyCode::Delete => b"\x1b[3~".to_vec(),
        KeyCode::Insert => b"\x1b[2~".to_vec(),
        KeyCode::PageUp => b"\x1b[5~".to_vec(),
        KeyCode::PageDown => b"\x1b[6~".to_vec(),
        KeyCode::F(n @ 1..=4) => format!("\x1bO{}", char::from(b'P' + n - 1)).into_bytes(),
        KeyCode::F(n @ 5..=12) => format!(
            "\x1b[{}~",
            [15, 17, 18, 19, 20, 21, 23, 24][usize::from(n - 5)]
        )
        .into_bytes(),
        _ => vec![],
    };
    if key.modifiers.contains(KeyModifiers::ALT)
        && matches!(
            key.code,
            KeyCode::Char(_) | KeyCode::Enter | KeyCode::Backspace
        )
    {
        text.insert(0, 27);
    }
    text
}
struct Queries {
    cursor: (u16, u16),
    replies: Vec<Vec<u8>>,
}
impl vte::Perform for Queries {
    fn csi_dispatch(&mut self, params: &vte::Params, intermediates: &[u8], _: bool, action: char) {
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
            ('u', _, [b'?']) => "\x1b[?0u".into(),
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
    fn key(code: KeyCode, modifiers: KeyModifiers) -> Event {
        Event::Key(KeyEvent::new(code, modifiers))
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
            assert!(Instant::now() < deadline, "terminal condition timed out");
            std::thread::sleep(Duration::from_millis(10));
        }
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
        assert!(manager.fullscreen);
        manager
            .input(&Event::Key(KeyEvent::new_with_kind(
                KeyCode::Char('f'),
                KeyModifiers::ALT,
                KeyEventKind::Repeat,
            )))
            .unwrap();
        assert!(manager.fullscreen);
        manager
            .input(&key(KeyCode::Char('f'), KeyModifiers::ALT))
            .unwrap();
        assert!(!manager.fullscreen);
        assert_eq!(manager.sessions[0].pid, pid);
        for direction in [KeyCode::Left, KeyCode::Right, KeyCode::Up, KeyCode::Down] {
            assert!(matches!(
                manager.input(&key(direction, KeyModifiers::ALT)).unwrap(),
                Input::Pass
            ));
            assert!(manager.active.is_some());
        }

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
                manager.draw(frame, Rect::new(10, 2, 30, 12), true);
            })
            .unwrap();
        assert_eq!(manager.sessions[0].size, (10, 28));
        assert_eq!(terminal.backend().buffer()[(0, 0)].symbol(), "O");
        assert_eq!(terminal.backend().buffer()[(10, 2)].symbol(), "┏");
        assert_eq!(terminal.backend().buffer()[(10, 2)].fg, Color::Cyan);
        terminal
            .draw(|frame| {
                manager.draw(frame, Rect::new(10, 2, 30, 12), false);
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
        manager.fullscreen = true;
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
        assert!(!manager.fullscreen);
    }

    #[test]
    fn ownership_lock_and_failed_spawn_release() {
        let key = SessionKey {
            provider: crate::domain::ProviderId("test".into()),
            source_id: crate::domain::SourceId("test".into()),
            native_id: format!("lock-{}", std::process::id()),
        };
        let cmd = command("pass");
        let lock = session_lock(&cmd, &key).unwrap();
        assert!(session_lock(&cmd, &key).is_err());
        drop(lock);
        assert!(session_lock(&cmd, &key).is_ok());
        let mut missing = cmd.clone();
        missing.executable = "/no-such-tracr-test-program".into();
        assert!(
            Manager::default()
                .launch(&missing, Some(key.clone()), "missing".into())
                .is_err()
        );
        assert!(session_lock(&cmd, &key).is_ok());
    }
}
