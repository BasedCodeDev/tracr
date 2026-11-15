//! Shared copyable, scrollable error presentation; operation reports are supplied by callers.
use crate::browser::safe_text;
use crossterm::event::{KeyCode, KeyEvent, KeyEventKind};
use ratatui::{
    Frame,
    layout::{Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    widgets::{Block, BorderType, Borders, Clear, Paragraph, Wrap},
};
use std::cell::Cell;

pub enum Event {
    None,
    Copy,
    Close,
}
pub struct ErrorDialog {
    pub title: String,
    pub summary: String,
    body: String,
    pub scroll: u16,
    pub scroll_limit: Cell<u16>,
    pub copy_status: Option<String>,
    pub copy_task: Option<tokio::task::JoinHandle<Result<(), String>>>,
}
impl ErrorDialog {
    pub fn new(
        title: impl Into<String>,
        summary: impl Into<String>,
        body: impl Into<String>,
    ) -> Self {
        Self {
            title: title.into(),
            summary: summary.into(),
            body: body.into(),
            scroll: 0,
            scroll_limit: Cell::new(0),
            copy_status: None,
            copy_task: None,
        }
    }
    pub fn report(&self) -> String {
        format!("{}\n\n{}", self.title, self.body)
    }
    pub fn key(&mut self, key: KeyEvent) -> Event {
        if key.kind == KeyEventKind::Release {
            return Event::None;
        }
        self.scroll = self.scroll.min(self.scroll_limit.get());
        match key.code {
            KeyCode::Char('c') if self.copy_task.is_none() => return Event::Copy,
            KeyCode::Enter | KeyCode::Esc => return Event::Close,
            KeyCode::Up => self.scroll = self.scroll.saturating_sub(1),
            KeyCode::Down => {
                self.scroll = self.scroll.saturating_add(1).min(self.scroll_limit.get())
            }
            KeyCode::PageUp => self.scroll = self.scroll.saturating_sub(10),
            KeyCode::PageDown => {
                self.scroll = self.scroll.saturating_add(10).min(self.scroll_limit.get())
            }
            KeyCode::Home => self.scroll = 0,
            _ => {}
        }
        Event::None
    }
    pub fn start_copy(&mut self) {
        if self.copy_task.is_none() {
            self.copy_status = Some("Copying message…".into());
            self.scroll = 0;
            self.copy_task = Some(tokio::spawn(crate::clipboard::copy(self.report())));
        }
    }
    pub async fn tick(&mut self) {
        if self.copy_task.as_ref().is_some_and(|t| t.is_finished()) {
            let result = self.copy_task.take().expect("finished copy").await;
            self.copy_status = Some(match result {
                Ok(Ok(())) => "Message copied".into(),
                Ok(Err(e)) => e,
                Err(e) => format!("Copy failed: {e}"),
            });
            self.scroll = 0;
        }
    }
    pub fn draw(&self, frame: &mut Frame<'_>) {
        let area = frame.area();
        let width = area.width.min(76);
        let height = area.height.min(24);
        let popup = Rect::new(
            area.x + (area.width - width) / 2,
            area.y + (area.height - height) / 2,
            width,
            height,
        );
        frame.render_widget(Clear, popup);
        let outer = Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Thick)
            .title(safe_text(&self.title));
        let inner = outer.inner(popup);
        frame.render_widget(outer, popup);
        let parts = Layout::vertical([
            Constraint::Length(1),
            Constraint::Min(1),
            Constraint::Length(3),
        ])
        .split(inner);
        frame.render_widget(
            Paragraph::new(safe_text(&self.summary))
                .style(Style::default().fg(Color::Red).add_modifier(Modifier::BOLD)),
            parts[0],
        );
        let text = if let Some(status) = &self.copy_status {
            format!("{status}\n\n{}", self.body)
        } else {
            self.body.clone()
        };
        let text = text
            .split('\n')
            .map(safe_text)
            .collect::<Vec<_>>()
            .join("\n");
        let body = Paragraph::new(text).wrap(Wrap { trim: false });
        let max = body
            .line_count(parts[1].width)
            .saturating_sub(parts[1].height as usize)
            .min(u16::MAX as usize) as u16;
        self.scroll_limit.set(max);
        frame.render_widget(body.scroll((self.scroll.min(max), 0)), parts[1]);
        frame.render_widget(
            Paragraph::new("c Copy message\nEnter/Esc close\n↑/↓ scroll · Home top"),
            parts[2],
        );
    }
}
impl Drop for ErrorDialog {
    fn drop(&mut self) {
        if let Some(task) = self.copy_task.take() {
            task.abort();
        }
    }
}
