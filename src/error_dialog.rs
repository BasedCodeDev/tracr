//! Shared copyable, scrollable error presentation; operation reports are supplied by callers.
use crossterm::event::{KeyCode, KeyEvent, KeyEventKind};
use ratatui::{
    Frame,
    widgets::{Paragraph, Wrap},
};
use std::cell::Cell;

#[derive(Clone, Copy)]
pub enum Event {
    None,
    Copy,
    Close,
}
pub struct ErrorDialog {
    pub title: String,
    pub summary: String,
    body: String,
    information: bool,
    content: Option<crate::info_content::Information>,
    pub scroll: u16,
    pub button_focus: usize,
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
            information: false,
            content: None,
            scroll: 0,
            button_focus: 1,
            scroll_limit: Cell::new(0),
            copy_status: None,
            copy_task: None,
        }
    }
    pub fn information(title: impl Into<String>, body: impl Into<String>) -> Self {
        let mut dialog = Self::new(title, "", body);
        dialog.information = true;
        dialog
    }
    pub fn set_body(&mut self, body: String) {
        self.body = body;
    }
    pub fn set_information(&mut self, content: crate::info_content::Information) {
        self.content = Some(content);
    }
    pub fn report(&self) -> String {
        format!(
            "{}\n\n{}",
            self.title,
            self.content
                .as_ref()
                .map_or_else(|| self.body.clone(), |content| content.report())
        )
    }
    pub fn key(&mut self, key: KeyEvent) -> Event {
        if key.kind == KeyEventKind::Release {
            return Event::None;
        }
        self.scroll = self.scroll.min(self.scroll_limit.get());
        match key.code {
            KeyCode::Char('c') if self.copy_task.is_none() => return Event::Copy,
            KeyCode::Tab | KeyCode::BackTab => crate::modal::cycle(
                &mut self.button_focus,
                &[self.copy_task.is_none(), true],
                key.code == KeyCode::BackTab,
            ),
            KeyCode::Enter | KeyCode::Char(' ') => {
                use crate::modal::Button;
                return crate::modal::activate(
                    key,
                    &[
                        Button {
                            action: Event::Copy,
                            label: "Copy",
                            enabled: self.copy_task.is_none(),
                            primary: false,
                        },
                        Button {
                            action: Event::Close,
                            label: "Close",
                            enabled: true,
                            primary: true,
                        },
                    ],
                    self.button_focus,
                )
                .unwrap_or(Event::None);
            }
            KeyCode::Esc => return Event::Close,
            KeyCode::Char('i') if self.information => return Event::Close,
            KeyCode::Up => self.scroll = self.scroll.saturating_sub(1),
            KeyCode::Down => {
                self.scroll = self.scroll.saturating_add(1).min(self.scroll_limit.get())
            }
            KeyCode::PageUp => self.scroll = self.scroll.saturating_sub(10),
            KeyCode::PageDown => {
                self.scroll = self.scroll.saturating_add(10).min(self.scroll_limit.get())
            }
            KeyCode::Home => self.scroll = 0,
            KeyCode::End => self.scroll = self.scroll_limit.get(),
            _ => {}
        }
        Event::None
    }
    pub fn start_copy(&mut self) {
        if self.copy_task.is_none() {
            self.copy_status = Some("Copying message…".into());
            if !self.information {
                self.scroll = 0;
            }
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
            if !self.information {
                self.scroll = 0;
            }
        }
    }
    pub fn draw(&self, frame: &mut Frame<'_>) {
        let area = frame.area();
        if area.width < 12 || area.height < 4 {
            frame.render_widget(Paragraph::new("tracr: enlarge terminal"), area);
            return;
        }
        let inner = crate::modal::shell(
            frame,
            &self.title,
            if self.information { 80 } else { 76 },
            if self.information { 28 } else { 24 },
        );
        let regions = crate::modal::regions(inner);
        if let Some(content) = &self.content {
            self.scroll_limit
                .set(content.draw(frame, regions.body, self.scroll, None));
        } else {
            let text = if self.summary.is_empty() {
                self.body.clone()
            } else {
                format!("{}\n\n{}", self.summary, self.body)
            };
            self.scroll_limit
                .set(crate::modal::body(frame, regions.body, text, self.scroll));
        }
        frame.render_widget(
            Paragraph::new(self.copy_status.as_deref().unwrap_or("")).wrap(Wrap { trim: false }),
            regions.status,
        );
        use crate::modal::Button;
        crate::modal::buttons(
            frame,
            regions.actions,
            &[
                Button {
                    action: Event::Copy,
                    label: if self.information {
                        "Copy information"
                    } else {
                        "Copy message"
                    },
                    enabled: self.copy_task.is_none(),
                    primary: false,
                },
                Button {
                    action: Event::Close,
                    label: "Close",
                    enabled: true,
                    primary: true,
                },
            ],
            Some(self.button_focus),
        );
        frame.render_widget(
            Paragraph::new(if inner.width < 32 {
                "Esc close · c Copy"
            } else if inner.width < 50 {
                "c Copy message · Esc close"
            } else if self.information {
                "c Copy · Enter activate · Esc close · ↑/↓ scroll"
            } else {
                "c Copy message · Enter activate · Esc close · ↑/↓ scroll"
            }),
            regions.hints,
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
