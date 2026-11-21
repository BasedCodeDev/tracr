//! Transient confirmation UI; command details are never persisted.
use crate::{
    browser::safe_text,
    providers::codex_terminals::{Blocker, Terminal},
};
use crossterm::event::{KeyCode, KeyEvent};
use ratatui::{
    Frame,
    layout::Rect,
    widgets::{Block, Borders, Clear, Padding, Paragraph, Wrap},
};

enum Reason {
    Terminals(Blocker),
    StillLoaded,
}

pub struct Dialog {
    reason: Reason,
    pub busy: bool,
    pub progress: String,
    choice: usize,
    scroll: u16,
    max_scroll: std::cell::Cell<u16>,
    context: String,
}
#[derive(Debug, PartialEq, Eq)]
pub enum Event {
    None,
    Cancel,
    Refresh,
    Stop,
    Retry,
}
impl Dialog {
    pub fn new(blocker: Blocker, session: &str, old: &str, target: &str) -> Self {
        Self::for_reason(Reason::Terminals(blocker), session, old, target)
    }
    pub fn still_loaded(session: &str, old: &str, target: &str) -> Self {
        Self::for_reason(Reason::StillLoaded, session, old, target)
    }
    fn for_reason(reason: Reason, session: &str, old: &str, target: &str) -> Self {
        Self {
            reason,
            busy: false,
            progress: "Checking/stopping commands… Please wait.".into(),
            choice: 0,
            scroll: 0,
            max_scroll: std::cell::Cell::new(0),
            context: format!(
                "Session: {}\nFrom: {}\nDestination: {}",
                safe_text(session),
                safe_text(old),
                safe_text(target)
            ),
        }
    }
    pub fn reviewed(&self) -> Option<Vec<Terminal>> {
        match &self.reason {
            Reason::Terminals(Blocker::Commands(commands)) if !commands.is_empty() => {
                Some(commands.clone())
            }
            _ => None,
        }
    }
    pub fn key(&mut self, key: KeyEvent) -> Event {
        if self.busy {
            return Event::None;
        }
        let count = if matches!(self.reason, Reason::Terminals(Blocker::Commands(_))) {
            3
        } else {
            2
        };
        match key.code {
            KeyCode::Esc => return Event::Cancel,
            KeyCode::Tab | KeyCode::Right => self.choice = (self.choice + 1) % count,
            KeyCode::BackTab | KeyCode::Left => self.choice = (self.choice + count - 1) % count,
            KeyCode::Up => self.scroll = self.scroll.saturating_sub(1),
            KeyCode::Down => self.scroll = self.scroll.saturating_add(1).min(self.max_scroll.get()),
            KeyCode::PageUp => self.scroll = self.scroll.saturating_sub(8),
            KeyCode::PageDown => {
                self.scroll = self.scroll.saturating_add(8).min(self.max_scroll.get())
            }
            KeyCode::Enter => {
                return match self.choice {
                    0 => Event::Cancel,
                    1 if matches!(self.reason, Reason::StillLoaded) => Event::Retry,
                    1 => Event::Refresh,
                    _ if self.reviewed().is_some() => Event::Stop,
                    _ => Event::Retry,
                };
            }
            _ => {}
        }
        Event::None
    }
    pub fn draw(&self, frame: &mut Frame<'_>) {
        let area = frame.area();
        if area.width < 24 || area.height < 14 {
            frame.render_widget(Clear, area);
            frame.render_widget(Paragraph::new("Enlarge terminal · Esc cancels"), area);
            return;
        }
        let width = area.width.min(100);
        let height = area.height.min(30);
        let popup = Rect::new(
            area.x + (area.width - width) / 2,
            area.y + (area.height - height) / 2,
            width,
            height,
        );
        frame.render_widget(Clear, popup);
        let mut text = format!("{}\n\n", self.context);
        match &self.reason {
            Reason::StillLoaded => text.push_str(crate::relocation::STILL_LOADED_DETAIL),
            Reason::Terminals(Blocker::Commands(commands)) if commands.is_empty() => text.push_str("No background commands remain. Retry repeats all relocation checks."),
            Reason::Terminals(Blocker::Commands(commands)) => {
                text.push_str("The agent is idle, but this session has background commands. They must stop before moving. Stopping commands may interrupt builds, servers or unsaved work.\n\n");
                for (i,command) in commands.iter().enumerate() {
                    text.push_str(&format!("{}. {}\n   Folder: {}\n\n",i+1,safe_text(&command.command),safe_text(&command.cwd)));
                }
                text.push_str("Only these listed commands will be stopped. The move will be retried if its context is unchanged. If an earlier stop attempt failed, some commands may already have stopped.");
            }
            Reason::Terminals(Blocker::Unavailable(reason)) => text.push_str(&format!("Background command state is unavailable. No move was attempted.\n\n{}\n\nRefresh to inspect again; commands cannot be stopped from an unknown state.",safe_text(reason))),
        }
        let block = Block::default()
            .borders(Borders::ALL)
            .padding(Padding::uniform(1))
            .title(if matches!(self.reason, Reason::StillLoaded) {
                "Session is still loaded"
            } else {
                "Background commands block relocation"
            });
        let inner = block.inner(popup);
        frame.render_widget(block, popup);
        let body = Rect::new(
            inner.x,
            inner.y,
            inner.width,
            inner
                .height
                .saturating_sub(if inner.width < 60 { 8 } else { 4 }),
        );
        let paragraph = Paragraph::new(text).wrap(Wrap { trim: false });
        self.max_scroll.set(crate::scrollbar::paragraph(
            frame,
            body,
            paragraph,
            self.scroll,
        ));
        let mut buttons = if matches!(self.reason, Reason::StillLoaded) {
            vec!["Cancel", "Retry"]
        } else {
            vec!["Cancel", "Refresh"]
        };
        if matches!(&self.reason,Reason::Terminals(Blocker::Commands(commands)) if commands.is_empty())
        {
            buttons.push("Retry move");
        } else if self.reviewed().is_some() {
            buttons.push("Stop listed commands and retry move");
        }
        let controls = if self.busy {
            safe_text(&self.progress)
        } else {
            format!(
                "{}\nTab/←/→ choose · Enter confirms · Esc cancels · ↑/↓ scroll",
                buttons
                    .iter()
                    .enumerate()
                    .map(|(i, b)| if i == self.choice {
                        format!("[{b}]")
                    } else {
                        b.to_string()
                    })
                    .collect::<Vec<_>>()
                    .join("  ")
            )
        };
        frame.render_widget(
            Paragraph::new(controls).wrap(Wrap { trim: false }),
            Rect::new(
                inner.x,
                body.bottom(),
                inner.width,
                inner.height.saturating_sub(body.height),
            ),
        );
    }
}
