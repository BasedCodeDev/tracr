//! Terminal input transport. One stdin reader preserves rich key data before UI adaptation.
use crossterm::event::{
    Event, KeyCode, KeyEvent, KeyEventKind, KeyEventState, KeyModifiers, MouseButton, MouseEvent,
    MouseEventKind,
};
use std::{
    collections::VecDeque,
    io::{self, Read},
    time::{Duration, Instant},
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Key {
    pub code: u32,
    pub shifted: Option<u32>,
    pub base: Option<u32>,
    /// Kitty modifier mask, including lock state.
    pub modifiers: u16,
    pub kind: KeyEventKind,
    pub text: Vec<u32>,
}
#[derive(Clone, Debug)]
pub struct Input {
    pub event: Event,
    pub key: Option<Key>,
}
impl From<Event> for Input {
    fn from(event: Event) -> Self {
        Self { event, key: None }
    }
}
impl Key {
    pub fn ui(&self) -> KeyEvent {
        let modifiers = modifiers(self.modifiers);
        let mut code = key_code(self.code);
        if code == KeyCode::Tab && modifiers.contains(KeyModifiers::SHIFT) {
            code = KeyCode::BackTab;
        }
        if let Some(c) = self
            .shifted
            .and_then(char::from_u32)
            .filter(|_| modifiers.contains(KeyModifiers::SHIFT))
        {
            code = KeyCode::Char(c);
        } else if let Some(c) = self
            .text
            .first()
            .copied()
            .and_then(char::from_u32)
            .filter(|_| {
                matches!(code, KeyCode::Char(_))
                    && !modifiers
                        .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT | KeyModifiers::SUPER)
            })
        {
            code = KeyCode::Char(c);
        }
        let mut state = KeyEventState::NONE;
        state.set(KeyEventState::CAPS_LOCK, self.modifiers & 64 != 0);
        state.set(KeyEventState::NUM_LOCK, self.modifiers & 128 != 0);
        state.set(KeyEventState::KEYPAD, (57399..=57427).contains(&self.code));
        KeyEvent::new_with_kind_and_state(code, modifiers, self.kind, state)
    }
    pub fn from_event(event: KeyEvent) -> Self {
        Self {
            code: codepoint(event.code),
            shifted: None,
            base: None,
            modifiers: modifier_mask(event.modifiers)
                | if event.code == KeyCode::BackTab { 1 } else { 0 }
                | if event.state.contains(KeyEventState::CAPS_LOCK) {
                    64
                } else {
                    0
                }
                | if event.state.contains(KeyEventState::NUM_LOCK) {
                    128
                } else {
                    0
                },
            kind: event.kind,
            text: vec![],
        }
    }
}
fn modifiers(mask: u16) -> KeyModifiers {
    let mut out = KeyModifiers::NONE;
    for (bit, flag) in [
        (1, KeyModifiers::SHIFT),
        (2, KeyModifiers::ALT),
        (4, KeyModifiers::CONTROL),
        (8, KeyModifiers::SUPER),
        (16, KeyModifiers::HYPER),
        (32, KeyModifiers::META),
    ] {
        if mask & bit != 0 {
            out.insert(flag);
        }
    }
    out
}
fn modifier_mask(mods: KeyModifiers) -> u16 {
    [
        (1, KeyModifiers::SHIFT),
        (2, KeyModifiers::ALT),
        (4, KeyModifiers::CONTROL),
        (8, KeyModifiers::SUPER),
        (16, KeyModifiers::HYPER),
        (32, KeyModifiers::META),
    ]
    .into_iter()
    .filter(|(_, f)| mods.contains(*f))
    .map(|(bit, _)| bit)
    .sum()
}
fn key_code(code: u32) -> KeyCode {
    match code {
        27 => KeyCode::Esc,
        13 => KeyCode::Enter,
        9 => KeyCode::Tab,
        127 => KeyCode::Backspace,
        57348 => KeyCode::Insert,
        57349 => KeyCode::Delete,
        57350 => KeyCode::Left,
        57351 => KeyCode::Right,
        57352 => KeyCode::Up,
        57353 => KeyCode::Down,
        57354 => KeyCode::PageUp,
        57355 => KeyCode::PageDown,
        57356 => KeyCode::Home,
        57357 => KeyCode::End,
        57358 => KeyCode::CapsLock,
        57359 => KeyCode::ScrollLock,
        57360 => KeyCode::NumLock,
        57361 => KeyCode::PrintScreen,
        57362 => KeyCode::Pause,
        57363 => KeyCode::Menu,
        57364..=57398 => KeyCode::F((code - 57363) as u8),
        57399..=57408 => KeyCode::Char(char::from(b'0' + (code - 57399) as u8)),
        57409 => KeyCode::Char('.'),
        57410 => KeyCode::Char('/'),
        57411 => KeyCode::Char('*'),
        57412 => KeyCode::Char('-'),
        57413 => KeyCode::Char('+'),
        57414 => KeyCode::Enter,
        57415 => KeyCode::Char('='),
        57416 => KeyCode::Char(','),
        57417 => KeyCode::Left,
        57418 => KeyCode::Right,
        57419 => KeyCode::Up,
        57420 => KeyCode::Down,
        57421 => KeyCode::PageUp,
        57422 => KeyCode::PageDown,
        57423 => KeyCode::Home,
        57424 => KeyCode::End,
        57425 => KeyCode::Insert,
        57426 => KeyCode::Delete,
        57427 => KeyCode::KeypadBegin,
        57344..=63743 => KeyCode::Null,
        _ => char::from_u32(code)
            .map(KeyCode::Char)
            .unwrap_or(KeyCode::Null),
    }
}
fn codepoint(code: KeyCode) -> u32 {
    match code {
        KeyCode::Char(c) => c as u32,
        KeyCode::Esc => 27,
        KeyCode::Enter => 13,
        KeyCode::Tab | KeyCode::BackTab => 9,
        KeyCode::Backspace => 127,
        KeyCode::Insert => 57348,
        KeyCode::Delete => 57349,
        KeyCode::Left => 57350,
        KeyCode::Right => 57351,
        KeyCode::Up => 57352,
        KeyCode::Down => 57353,
        KeyCode::PageUp => 57354,
        KeyCode::PageDown => 57355,
        KeyCode::Home => 57356,
        KeyCode::End => 57357,
        KeyCode::F(n) => 57363 + u32::from(n),
        KeyCode::CapsLock => 57358,
        KeyCode::ScrollLock => 57359,
        KeyCode::NumLock => 57360,
        KeyCode::PrintScreen => 57361,
        KeyCode::Pause => 57362,
        KeyCode::Menu => 57363,
        KeyCode::KeypadBegin => 57427,
        _ => 0,
    }
}

#[derive(Default)]
pub struct Decoder {
    bytes: Vec<u8>,
    ready: VecDeque<Input>,
    pub enhanced: bool,
    pub detected: bool,
    since: Option<Instant>,
    button: Option<MouseButton>,
}
impl Decoder {
    pub fn feed(&mut self, bytes: &[u8]) {
        self.bytes.extend_from_slice(bytes);
        self.since = Some(Instant::now());
        self.decode(false);
    }
    fn key(&mut self, key: Key) {
        self.ready.push_back(Input {
            event: Event::Key(key.ui()),
            key: Some(key),
        });
    }
    fn simple(&mut self, code: KeyCode, modifiers: KeyModifiers) {
        self.key(Key::from_event(KeyEvent::new(code, modifiers)));
    }
    fn decode(&mut self, timeout: bool) {
        while !self.bytes.is_empty() {
            if self.bytes.starts_with(b"\x1b[200~") {
                let Some(end) = self.bytes[6..].windows(6).position(|w| w == b"\x1b[201~") else {
                    break;
                };
                let text = String::from_utf8_lossy(&self.bytes[6..6 + end]).into_owned();
                self.bytes.drain(..end + 12);
                self.ready.push_back(Event::Paste(text).into());
                continue;
            }
            if self.bytes[0] != 27 {
                let b = self.bytes[0];
                if b < 32 || b == 127 {
                    self.bytes.remove(0);
                    match b {
                        13 => self.simple(KeyCode::Enter, KeyModifiers::NONE),
                        9 => self.simple(KeyCode::Tab, KeyModifiers::NONE),
                        127 => self.simple(KeyCode::Backspace, KeyModifiers::NONE),
                        0 => self.simple(KeyCode::Char(' '), KeyModifiers::CONTROL),
                        1..=26 => self.simple(
                            KeyCode::Char(char::from(b'a' + b - 1)),
                            KeyModifiers::CONTROL,
                        ),
                        28..=31 => self.simple(
                            KeyCode::Char(char::from(b'\\' + b - 28)),
                            KeyModifiers::CONTROL,
                        ),
                        _ => (),
                    }
                } else {
                    let n = match b {
                        0..=127 => 1,
                        194..=223 => 2,
                        224..=239 => 3,
                        240..=244 => 4,
                        _ => 1,
                    };
                    if self.bytes.len() < n {
                        break;
                    }
                    let c = std::str::from_utf8(&self.bytes[..n])
                        .ok()
                        .and_then(|s| s.chars().next())
                        .unwrap_or('\u{fffd}');
                    self.bytes.drain(..n);
                    self.simple(KeyCode::Char(c), KeyModifiers::NONE);
                }
                continue;
            }
            if self.bytes.len() == 1 {
                if timeout {
                    self.bytes.remove(0);
                    self.simple(KeyCode::Esc, KeyModifiers::NONE);
                }
                break;
            }
            if self.bytes.starts_with(b"\x1b[M") {
                let mut values = Vec::new();
                let mut end = 3;
                while values.len() < 3 && end < self.bytes.len() {
                    let byte = self.bytes[end];
                    let length = match byte {
                        194..=223 => 2,
                        224..=239 => 3,
                        _ => 1,
                    };
                    if end + length > self.bytes.len() {
                        break;
                    }
                    let (value, length) = if length > 1 {
                        std::str::from_utf8(&self.bytes[end..end + length])
                            .ok()
                            .and_then(|s| s.chars().next())
                            .map_or((u16::from(byte), 1), |c| (c as u16, length))
                    } else {
                        (u16::from(byte), 1)
                    };
                    values.push(value.saturating_sub(32));
                    end += length;
                }
                if values.len() < 3 {
                    break;
                }
                self.bytes.drain(..end);
                self.mouse(values[0], values[1], values[2], false);
                continue;
            }
            if matches!(self.bytes[1], b'[' | b'O') {
                let Some(end) = self.bytes[2..]
                    .iter()
                    .position(|b| (0x40..=0x7e).contains(b))
                    .map(|n| n + 2)
                else {
                    if timeout && self.bytes.len() == 2 {
                        let c = char::from(self.bytes[1]);
                        self.bytes.clear();
                        self.simple(KeyCode::Char(c), KeyModifiers::ALT);
                    }
                    break;
                };
                let final_byte = self.bytes[end];
                let body = String::from_utf8_lossy(&self.bytes[2..end]).into_owned();
                let ss3 = self.bytes[1] == b'O';
                self.bytes.drain(..=end);
                self.sequence(&body, final_byte, ss3);
                continue;
            }
            // Decode exactly one Alt-prefixed character; never recursively parse an
            // arbitrary run of escapes or consume subsequent input into this key.
            let b = self.bytes[1];
            if b == 27 {
                self.bytes.drain(..2);
                self.simple(KeyCode::Esc, KeyModifiers::ALT);
                continue;
            }
            let n = match b {
                0..=127 => 1,
                194..=223 => 2,
                224..=239 => 3,
                240..=244 => 4,
                _ => 1,
            };
            if self.bytes.len() < n + 1 {
                break;
            }
            let mut nested = Decoder::default();
            nested.feed(&self.bytes[1..=n]);
            if let Some(mut input) = nested.ready.pop_front() {
                if let Some(key) = input.key.as_mut() {
                    key.modifiers |= 2;
                    input.event = Event::Key(key.ui());
                }
                self.ready.push_back(input);
            }
            self.bytes.drain(..n + 1);
        }
        if self.bytes.is_empty() {
            self.since = None;
        }
    }
    fn mouse(&mut self, b: u16, x: u16, y: u16, release: bool) {
        let button = match b & 3 {
            0 => MouseButton::Left,
            1 => MouseButton::Middle,
            2 => MouseButton::Right,
            _ => self.button.unwrap_or(MouseButton::Left),
        };
        let kind = if b & 64 != 0 {
            match b & 3 {
                0 => MouseEventKind::ScrollUp,
                1 => MouseEventKind::ScrollDown,
                2 => MouseEventKind::ScrollLeft,
                _ => MouseEventKind::ScrollRight,
            }
        } else if release || b & 3 == 3 && b & 32 == 0 {
            MouseEventKind::Up(button)
        } else if b & 32 != 0 {
            if b & 3 == 3 {
                MouseEventKind::Moved
            } else {
                MouseEventKind::Drag(button)
            }
        } else {
            MouseEventKind::Down(button)
        };
        match kind {
            MouseEventKind::Down(button) => self.button = Some(button),
            MouseEventKind::Up(_) => self.button = None,
            _ => (),
        }
        let modifiers = modifiers((b >> 2) & 7);
        self.ready.push_back(
            Event::Mouse(MouseEvent {
                kind,
                column: x.saturating_sub(1),
                row: y.saturating_sub(1),
                modifiers,
            })
            .into(),
        );
    }
    fn sequence(&mut self, body: &str, last: u8, ss3: bool) {
        if last == b'u' && body.starts_with('?') {
            self.enhanced = true;
            self.detected = true;
            return;
        }
        if last == b'c' && body.starts_with('?') {
            self.detected = true;
            return;
        }
        if body.is_empty() && matches!(last, b'I' | b'O') && !ss3 {
            self.ready.push_back(
                if last == b'I' {
                    Event::FocusGained
                } else {
                    Event::FocusLost
                }
                .into(),
            );
            return;
        }
        if let Some(mouse) = body
            .strip_prefix('<')
            .filter(|_| matches!(last, b'M' | b'm'))
        {
            let p: Vec<_> = mouse
                .split(';')
                .filter_map(|v| v.parse::<u16>().ok())
                .collect();
            if p.len() == 3 {
                self.mouse(p[0], p[1], p[2], last == b'm');
            }
            return;
        }
        let fields: Vec<_> = body.split(';').collect();
        let number = |s: &str| s.parse::<u32>().ok();
        let first = fields
            .first()
            .and_then(|s| number(s.split(':').next().unwrap_or("")))
            .unwrap_or(1);
        let mut modifiers = fields
            .get(1)
            .and_then(|s| number(s.split(':').next().unwrap_or("")))
            .unwrap_or(1)
            .saturating_sub(1)
            .min(255) as u16;
        let kind = match fields.get(1).and_then(|s| s.split(':').nth(1)) {
            Some("2") => KeyEventKind::Repeat,
            Some("3") => KeyEventKind::Release,
            _ => KeyEventKind::Press,
        };
        if last == b'u' {
            let keys: Vec<_> = fields[0].split(':').map(number).collect();
            if let Some(Some(code)) = keys.first() {
                self.key(Key {
                    code: *code,
                    shifted: keys.get(1).copied().flatten(),
                    base: keys.get(2).copied().flatten(),
                    modifiers,
                    kind,
                    text: fields
                        .get(2)
                        .map(|s| s.split(':').filter_map(number).collect())
                        .unwrap_or_default(),
                });
            }
            return;
        }
        if last == b'~' && first == 27 && fields.len() == 3 {
            // xterm modifyOtherKeys
            if let Some(code) = number(fields[2]) {
                self.key(Key {
                    code,
                    shifted: None,
                    base: None,
                    modifiers,
                    kind,
                    text: vec![],
                });
            }
            return;
        }
        let code = match last {
            b'A' => 57352,
            b'B' => 57353,
            b'C' => 57351,
            b'D' => 57350,
            b'H' => 57356,
            b'F' => 57357,
            b'Z' => {
                modifiers |= 1;
                9
            }
            b'P'..=b'S' => 57364 + u32::from(last - b'P'),
            b'~' => match first {
                1 | 7 => 57356,
                2 => 57348,
                3 => 57349,
                4 | 8 => 57357,
                5 => 57354,
                6 => 57355,
                11..=15 => 57364 + first - 11,
                17..=21 => 57369 + first - 17,
                23..=26 => 57374 + first - 23,
                28..=29 => 57378 + first - 28,
                31..=34 => 57380 + first - 31,
                _ => return,
            },
            b'p'..=b'y' if ss3 => 57399 + u32::from(last - b'p'),
            b'n' if ss3 => 57409,
            b'o' if ss3 => 57410,
            b'j' if ss3 => 57411,
            b'm' if ss3 => 57412,
            b'k' if ss3 => 57413,
            b'M' if ss3 => 57414,
            b'X' if ss3 => 57415,
            b'l' if ss3 => 57416,
            b'E' => 57427,
            _ => return,
        };
        self.key(Key {
            code,
            shifted: None,
            base: None,
            modifiers,
            kind,
            text: vec![],
        });
    }
}

#[derive(Default)]
pub struct Reader {
    decoder: Decoder,
}
impl Reader {
    pub fn enhanced(&self) -> bool {
        self.decoder.enhanced
    }
    pub fn poll(&mut self, timeout: Duration) -> io::Result<bool> {
        let deadline = Instant::now() + timeout;
        loop {
            if self
                .decoder
                .since
                .is_some_and(|t| t.elapsed() >= Duration::from_millis(25))
            {
                self.decoder.decode(true);
            }
            if !self.decoder.ready.is_empty() {
                return Ok(true);
            }
            let mut wait = deadline.saturating_duration_since(Instant::now());
            if let Some(t) = self
                .decoder
                .since
                .filter(|t| t.elapsed() < Duration::from_millis(25))
            {
                wait = wait.min(Duration::from_millis(25).saturating_sub(t.elapsed()));
            }
            let mut fd = libc::pollfd {
                fd: 0,
                events: libc::POLLIN,
                revents: 0,
            };
            // SAFETY: fd points to a live pollfd; stdin is read only by this reader.
            let result =
                unsafe { libc::poll(&mut fd, 1, wait.as_millis().min(i32::MAX as u128) as i32) };
            if result < 0 {
                let e = io::Error::last_os_error();
                if e.kind() != io::ErrorKind::Interrupted {
                    return Err(e);
                }
            }
            if fd.revents & (libc::POLLIN | libc::POLLHUP) != 0 {
                let mut bytes = [0; 8192];
                let n = io::stdin().read(&mut bytes)?;
                if n == 0 {
                    return Err(io::Error::new(
                        io::ErrorKind::UnexpectedEof,
                        "terminal input closed",
                    ));
                }
                self.decoder.feed(&bytes[..n]);
            }
            if Instant::now() >= deadline {
                return Ok(!self.decoder.ready.is_empty());
            }
        }
    }
    pub fn read(&mut self) -> io::Result<Input> {
        while !self.poll(Duration::from_secs(1))? {}
        Ok(self.decoder.ready.pop_front().expect("queued input"))
    }
}

/// Child-selected keyboard modes, independent for the primary and alternate screen.
#[derive(Default)]
pub struct Modes {
    stacks: [Vec<u8>; 2],
    pub alternate: bool,
    pub modify_other_keys: u8,
    pub focus: bool,
}
impl Modes {
    pub fn flags(&self) -> u8 {
        self.stacks[usize::from(self.alternate)]
            .last()
            .copied()
            .unwrap_or(0)
    }
    pub fn csi(
        &mut self,
        params: &vte::Params,
        intermediate: &[u8],
        action: char,
    ) -> Option<Vec<u8>> {
        let p: Vec<_> = params.iter().map(|v| v[0]).collect();
        let first = p.first().copied().unwrap_or(0);
        if intermediate == b"?" && matches!(action, 'h' | 'l') {
            for n in &p {
                match n {
                    47 | 1047 | 1049 => self.alternate = action == 'h',
                    1004 => self.focus = action == 'h',
                    _ => (),
                }
            }
        }
        if intermediate == b">" && action == 'm' && first == 4 {
            self.modify_other_keys = p.get(1).copied().unwrap_or(0).min(2) as u8;
        }
        if intermediate == b">" && action == 'n' && first == 4 {
            self.modify_other_keys = 0;
        }
        if action != 'u' {
            return None;
        }
        let stack = &mut self.stacks[usize::from(self.alternate)];
        match intermediate {
            b"?" => return Some(format!("\x1b[?{}u", self.flags()).into_bytes()),
            b">" => {
                if stack.len() >= 32 {
                    stack.remove(0);
                }
                stack.push((first as u8) & 31);
            }
            b"<" => {
                let n = if first == 0 { 1 } else { first as usize };
                stack.truncate(stack.len().saturating_sub(n));
            }
            b"=" => {
                let previous = stack.last().copied().unwrap_or(0);
                let bits = (first as u8) & 31;
                let value = match p.get(1).copied().unwrap_or(1) {
                    2 => previous | bits,
                    3 => previous & !bits,
                    _ => bits,
                };
                if let Some(last) = stack.last_mut() {
                    *last = value;
                } else {
                    stack.push(value);
                }
            }
            _ => (),
        }
        None
    }
    pub fn encode(&self, key: &Key, application: bool, keypad: bool) -> Vec<u8> {
        let flags = self.flags();
        if (57441..=57454).contains(&key.code) && flags & 8 == 0 {
            return vec![];
        }
        let plain_control = matches!(key.code, 9 | 13 | 127) && key.modifiers & 63 == 0;
        let text_key = (key.code == 0 && !key.text.is_empty()
            || key.code >= 32 && !(57344..=63743).contains(&key.code) && key.code != 127)
            && key.modifiers & 62 == 0;
        if flags & 8 == 0 && (plain_control || text_key) && key.kind == KeyEventKind::Release {
            return vec![];
        }
        if key.kind == KeyEventKind::Release && flags & 2 == 0 {
            return vec![];
        }
        // Legacy Enter/Tab/Backspace retain their bytes unless modified or all-key reporting is requested.
        let extended = flags & 8 != 0
            || flags & 1 != 0 && (key.modifiers & 62 != 0 || !text_key && !plain_control)
            || flags & 2 != 0 && !plain_control && !text_key && key.kind != KeyEventKind::Press;
        if extended {
            let mut code = key.code.to_string();
            if flags & 4 != 0 && (key.shifted.is_some() || key.base.is_some()) {
                code.push(':');
                if let Some(n) = key.shifted {
                    code.push_str(&n.to_string());
                }
                if let Some(n) = key.base {
                    code.push(':');
                    code.push_str(&n.to_string());
                }
            }
            let mut modifiers = (key.modifiers + 1).to_string();
            if flags & 2 != 0 {
                modifiers.push_str(match key.kind {
                    KeyEventKind::Press => "",
                    KeyEventKind::Repeat => ":2",
                    KeyEventKind::Release => ":3",
                });
            }
            let text = if flags & 16 != 0 && flags & 8 != 0 && !key.text.is_empty() {
                format!(
                    ";{}",
                    key.text
                        .iter()
                        .map(u32::to_string)
                        .collect::<Vec<_>>()
                        .join(":")
                )
            } else {
                String::new()
            };
            let functional = match key.code {
                57348 => Some((2, '~')),
                57349 => Some((3, '~')),
                57350 => Some((1, 'D')),
                57351 => Some((1, 'C')),
                57352 => Some((1, 'A')),
                57353 => Some((1, 'B')),
                57354 => Some((5, '~')),
                57355 => Some((6, '~')),
                57356 => Some((1, 'H')),
                57357 => Some((1, 'F')),
                57364 => Some((1, 'P')),
                57365 => Some((1, 'Q')),
                57366 => Some((13, '~')),
                57367 => Some((1, 'S')),
                57427 => Some((1, 'E')),
                57368..=57375 => Some((
                    [15, 17, 18, 19, 20, 21, 23, 24][(key.code - 57368) as usize],
                    '~',
                )),
                _ => None,
            };
            if let Some((number, suffix)) = functional {
                return format!("\x1b[{number};{modifiers}{suffix}").into_bytes();
            }
            return format!("\x1b[{code};{modifiers}{text}u").into_bytes();
        }
        if key.kind == KeyEventKind::Release {
            return vec![];
        }
        if flags == 0
            && self.modify_other_keys > 0
            && key.modifiers & 63 != 0
            && key.code < 57344
            && (self.modify_other_keys == 2
                || !matches!(key.code, 9 | 13 | 27 | 127) && key.code > 32)
        {
            return format!("\x1b[27;{};{}~", key.modifiers + 1, key.code).into_bytes();
        }
        legacy(key, application, keypad)
    }
}
fn legacy(key: &Key, application: bool, keypad: bool) -> Vec<u8> {
    if key.modifiers & 56 != 0
        || key.modifiers & 5 == 5 && key.code >= 32 && key.code != 127 && key.code < 57344
    {
        return format!("\x1b[{};{}u", key.code, (key.modifiers & 63) + 1).into_bytes();
    }
    let modifiers = key.modifiers & 7;
    let modified = modifiers != 0;
    let prefix = if application && !modified { "O" } else { "[" };
    let suffix = match key.code {
        57350 => Some('D'),
        57351 => Some('C'),
        57352 => Some('A'),
        57353 => Some('B'),
        57356 => Some('H'),
        57357 => Some('F'),
        57364..=57367 => Some(char::from(b'P' + (key.code - 57364) as u8)),
        _ => None,
    };
    if let Some(suffix) = suffix {
        return if modified {
            format!("\x1b[1;{}{suffix}", modifiers + 1)
        } else if (57364..=57367).contains(&key.code) {
            format!("\x1bO{suffix}")
        } else {
            format!("\x1b{prefix}{suffix}")
        }
        .into_bytes();
    }
    let tilde = match key.code {
        57348 => Some(2),
        57349 => Some(3),
        57354 => Some(5),
        57355 => Some(6),
        57368..=57383 => Some(
            [
                15, 17, 18, 19, 20, 21, 23, 24, 25, 26, 28, 29, 31, 32, 33, 34,
            ][(key.code - 57368) as usize],
        ),
        _ => None,
    };
    if let Some(n) = tilde {
        return if modified {
            format!("\x1b[{n};{}~", modifiers + 1)
        } else {
            format!("\x1b[{n}~")
        }
        .into_bytes();
    }
    if (57399..=57427).contains(&key.code) {
        if keypad && !modified {
            let suffix = match key.code {
                57399..=57408 => b'p' + (key.code - 57399) as u8,
                57409 => b'n',
                57410 => b'o',
                57411 => b'j',
                57412 => b'm',
                57413 => b'k',
                57414 => b'M',
                57415 => b'X',
                57416 => b'l',
                57417 => b't',
                57418 => b'v',
                57419 => b'x',
                57420 => b'r',
                57421 => b'y',
                57422 => b's',
                57423 => b'w',
                57424 => b'q',
                57425 => b'p',
                57426 => b'n',
                _ => b'u',
            };
            return vec![27, b'O', suffix];
        }
        let normal = key_code(key.code);
        if normal != KeyCode::KeypadBegin {
            let mut key = key.clone();
            key.code = codepoint(normal);
            return legacy(&key, application, false);
        }
        return b"\x1b[E".to_vec();
    }
    if key.code == 9 && modifiers & 1 != 0 {
        return if modifiers == 1 {
            b"\x1b[Z".to_vec()
        } else {
            format!("\x1b[1;{}Z", modifiers + 1).into_bytes()
        };
    }
    let code = if key.modifiers & 1 != 0 {
        key.shifted.unwrap_or_else(|| {
            char::from_u32(key.code)
                .map(|c| c.to_ascii_uppercase() as u32)
                .unwrap_or(key.code)
        })
    } else {
        key.code
    };
    let mut bytes = match code {
        13 => vec![13],
        9 => vec![9],
        27 => vec![27],
        127 => vec![if modifiers & 4 != 0 { 8 } else { 127 }],
        n if modifiers & 4 != 0 => match char::from_u32(n).map(|c| c.to_ascii_uppercase()) {
            Some('A'..='Z') => vec![(n as u8).to_ascii_uppercase() - b'A' + 1],
            Some(' ' | '@' | '2') => vec![0],
            Some('[' | '3') => vec![27],
            Some('\\' | '4') => vec![28],
            Some(']' | '5') => vec![29],
            Some('^' | '6') => vec![30],
            Some('_' | '/' | '7') => vec![31],
            Some('?' | '8') => vec![127],
            Some(c) => c.to_string().into_bytes(),
            None => return vec![],
        },
        _ if !key.text.is_empty() => key
            .text
            .iter()
            .filter_map(|n| char::from_u32(*n))
            .collect::<String>()
            .into_bytes(),
        n if !(57344..=63743).contains(&n) => char::from_u32(n)
            .map(|c| c.to_string().into_bytes())
            .unwrap_or_default(),
        _ => vec![],
    };
    if modifiers & 2 != 0 {
        bytes.insert(0, 27);
    }
    bytes
}

#[cfg(test)]
mod tests {
    use super::*;
    fn decode(bytes: &[u8]) -> Vec<Input> {
        let mut decoder = Decoder::default();
        for byte in bytes {
            decoder.feed(&[*byte]);
        }
        decoder.decode(true);
        decoder.ready.into_iter().collect()
    }
    fn mode(bytes: &[u8]) -> Modes {
        struct Handler(Modes);
        impl vte::Perform for Handler {
            fn csi_dispatch(&mut self, p: &vte::Params, i: &[u8], _: bool, a: char) {
                self.0.csi(p, i, a);
            }
        }
        let mut handler = Handler(Modes::default());
        let mut parser = vte::Parser::new();
        for b in bytes {
            parser.advance(&mut handler, *b);
        }
        handler.0
    }
    #[test]
    fn fragmented_keys_preserve_modifiers_alternates_text_and_events() {
        let events = decode(b"\x1b[97:65:113;198:2;65:769u\x1b[127;5u\x1b[1;3:3D\x1b[27;5;127~");
        let key = events[0].key.as_ref().unwrap();
        assert_eq!(
            (key.code, key.shifted, key.base, key.modifiers, key.kind),
            (97, Some(65), Some(113), 197, KeyEventKind::Repeat)
        );
        assert_eq!(key.text, [65, 769]);
        assert!(
            key.ui()
                .modifiers
                .contains(KeyModifiers::CONTROL | KeyModifiers::SHIFT)
        );
        assert_eq!(
            events[1].event,
            Event::Key(KeyEvent::new(KeyCode::Backspace, KeyModifiers::CONTROL))
        );
        assert_eq!(events[2].key.as_ref().unwrap().kind, KeyEventKind::Release);
        assert_eq!(events[3].key.as_ref().unwrap().modifiers, 4);
    }
    #[test]
    fn utf8_paste_escape_mouse_and_focus_do_not_become_keys() {
        let events =
            decode("é𐐀\x1b[200~a\n\x1b[127;5u\x1b[201~\x1bx\x1b[I\x1b[<20;4;6m\x1b".as_bytes());
        assert_eq!(events.len(), 7);
        assert_eq!(events[1].key.as_ref().unwrap().code, '𐐀' as u32);
        assert_eq!(events[2].event, Event::Paste("a\n\x1b[127;5u".into()));
        assert_eq!(events[3].key.as_ref().unwrap().modifiers, 2);
        assert_eq!(events[4].event, Event::FocusGained);
        assert!(
            matches!(&events[5].event,Event::Mouse(m) if m.column==3 && m.row==5 && m.modifiers==KeyModifiers::CONTROL|KeyModifiers::SHIFT)
        );
        assert_eq!(events[6].key.as_ref().unwrap().code, 27);
    }
    #[test]
    fn child_modes_are_isolated_and_stack_queries_are_honest() {
        let modes = mode(b"\x1b[>1u\x1b[>31u\x1b[<u\x1b[?1049h\x1b[>8u\x1b[?1049l");
        assert_eq!(modes.flags(), 1);
        assert_eq!(mode(b"\x1b[>1u\x1b[=2;2u\x1b[=1;3u").flags(), 2);
        assert_eq!(mode(b"\x1b[>31u\x1b[<99u").flags(), 0);
    }
    #[test]
    fn negotiated_and_legacy_encodings_match_terminal_protocols() {
        let modern = mode(b"\x1b[>31u");
        let minimal = mode(b"\x1b[>1u");
        let legacy = Modes::default();
        for (input, modern_expected, legacy_expected) in [
            (
                b"\x1b[127;5u".as_slice(),
                b"\x1b[127;5u".as_slice(),
                b"\x08".as_slice(),
            ),
            (b"\x1b[3;6~", b"\x1b[3;6~", b"\x1b[3;6~"),
            (b"\x1b[1;3D", b"\x1b[1;3D", b"\x1b[1;3D"),
            (b"\x1b[97:65;2;65u", b"\x1b[97:65;2;65u", b"A"),
            (b"\x1b[13;3u", b"\x1b[13;3u", b"\x1b\r"),
        ] {
            let key = decode(input).remove(0).key.unwrap();
            assert_eq!(modern.encode(&key, false, false), modern_expected);
            assert_eq!(legacy.encode(&key, false, false), legacy_expected);
        }
        let key = Key::from_event(KeyEvent::new(KeyCode::Backspace, KeyModifiers::CONTROL));
        assert_eq!(minimal.encode(&key, false, false), b"\x1b[127;5u");
        let key = Key::from_event(KeyEvent::new(KeyCode::Char('𐐀'), KeyModifiers::NONE));
        assert_eq!(legacy.encode(&key, false, false), "𐐀".as_bytes());
        assert_eq!(
            mode(b"\x1b[>4;2m").encode(
                &Key::from_event(KeyEvent::new(KeyCode::Enter, KeyModifiers::SHIFT)),
                false,
                false
            ),
            b"\x1b[27;2;13~"
        );
    }
    #[test]
    fn ime_text_and_modifier_keys_do_not_leak_control_codes() {
        let key = decode(b"\x1b[0;;20320:22909u").remove(0).key.unwrap();
        assert_eq!(
            mode(b"\x1b[>1u").encode(&key, false, false),
            "你好".as_bytes()
        );
        assert_eq!(
            Modes::default().encode(&key, false, false),
            "你好".as_bytes()
        );
        let modifier = decode(b"\x1b[57441;2u").remove(0).key.unwrap();
        assert_eq!(modifier.ui().code, KeyCode::Null);
        assert!(mode(b"\x1b[>1u").encode(&modifier, false, false).is_empty());
        assert!(
            !mode(b"\x1b[>31u")
                .encode(&modifier, false, false)
                .is_empty()
        );
    }
    #[test]
    fn releases_only_reach_children_that_requested_them() {
        for code in [
            KeyCode::Char('a'),
            KeyCode::Enter,
            KeyCode::Backspace,
            KeyCode::Tab,
        ] {
            let key = Key::from_event(KeyEvent::new_with_kind(
                code,
                KeyModifiers::NONE,
                KeyEventKind::Release,
            ));
            assert!(mode(b"\x1b[>3u").encode(&key, false, false).is_empty());
            assert!(!mode(b"\x1b[>31u").encode(&key, false, false).is_empty());
            assert!(Modes::default().encode(&key, false, false).is_empty());
        }
    }
}
