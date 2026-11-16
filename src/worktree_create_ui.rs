//! Shared worktree dialog for checkout HEAD and selected history commits.
use crate::worktree_create::{self as backend, Context, Plan};
use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::{
    Frame,
    layout::{Constraint, Layout, Rect},
    style::{Color, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph, Wrap},
};
use std::path::PathBuf;

enum ResultStage {
    Context(Context),
    Preview(Plan),
    Created(PathBuf),
}
pub struct Modal {
    pub context: Option<Context>,
    pub branch: String,
    pub destination: String,
    pub focus: usize,
    pub plan: Option<Plan>,
    pub error: Option<String>,
    pub executing: bool,
    task: Option<tokio::task::JoinHandle<Result<ResultStage, String>>>,
    edited_destination: bool,
    caret: [usize; 2],
    scroll: u16,
}
impl Drop for Modal {
    fn drop(&mut self) {
        if let Some(task) = &self.task {
            if !self.executing {
                task.abort();
            }
        }
    }
}
impl Modal {
    pub fn new(origin: PathBuf, commit: Option<String>) -> Self {
        Self {
            context: None,
            branch: String::new(),
            destination: String::new(),
            focus: 0,
            plan: None,
            error: None,
            executing: false,
            edited_destination: false,
            caret: [0, 0],
            scroll: 0,
            task: Some(tokio::spawn(async move {
                backend::context(&origin, commit.as_deref())
                    .await
                    .map(ResultStage::Context)
            })),
        }
    }
    pub fn ready(&self) -> bool {
        self.task.as_ref().is_some_and(|t| t.is_finished())
    }
    pub async fn tick(&mut self) -> Option<Result<PathBuf, String>> {
        if !self.ready() {
            return None;
        }
        let result = self
            .task
            .take()
            .unwrap()
            .await
            .unwrap_or_else(|e| Err(format!("Worktree operation stopped: {e}")));
        match result {
            Ok(ResultStage::Context(context)) => {
                self.destination = backend::suggested(&context, "").display().to_string();
                self.caret[1] = self.destination.len();
                self.context = Some(context);
            }
            Ok(ResultStage::Preview(plan)) => {
                self.plan = Some(plan);
                self.scroll = 0;
            }
            Ok(ResultStage::Created(path)) => {
                self.executing = false;
                return Some(Ok(path));
            }
            Err(error) => {
                if self.executing {
                    self.executing = false;
                    self.plan = None;
                    return Some(Err(error));
                }
                self.error = Some(error);
            }
        }
        None
    }
    fn changed(&mut self) {
        self.error = None;
        if self.focus == 0 && !self.edited_destination {
            if let Some(context) = &self.context {
                self.destination = backend::suggested(context, &self.branch)
                    .display()
                    .to_string();
                self.caret[1] = self.destination.len();
            }
        } else if self.focus == 1 {
            self.edited_destination = true;
        }
    }
    pub fn paste(&mut self, text: &str) {
        if self.task.is_some() || self.plan.is_some() || self.focus > 1 {
            return;
        }
        let input = if self.focus == 0 {
            &mut self.branch
        } else {
            &mut self.destination
        };
        let clean: String = text.chars().filter(|c| !c.is_control()).collect();
        input.insert_str(self.caret[self.focus], &clean);
        self.caret[self.focus] += clean.len();
        self.changed();
    }
    /// True closes the dialog. Running creation cannot be abandoned by a key.
    pub fn key(&mut self, key: KeyEvent) -> bool {
        if key.kind == KeyEventKind::Release {
            return false;
        }
        if self.executing {
            return false;
        }
        if key.code == KeyCode::Esc {
            if self.plan.take().is_some() {
                self.scroll = 0;
                return false;
            }
            return true;
        }
        if self.task.is_some() {
            return false;
        }
        if let Some(plan) = &self.plan {
            match key.code {
                KeyCode::Enter if key.kind != KeyEventKind::Repeat => {
                    let plan = plan.clone();
                    self.executing = true;
                    self.task = Some(tokio::spawn(async move {
                        backend::execute(plan).await.map(ResultStage::Created)
                    }));
                }
                KeyCode::Down | KeyCode::PageDown => self.scroll = self.scroll.saturating_add(1),
                KeyCode::Up | KeyCode::PageUp => self.scroll = self.scroll.saturating_sub(1),
                KeyCode::Home => self.scroll = 0,
                _ => {}
            }
            return false;
        }
        if self.context.is_none() {
            return false;
        }
        match key.code {
            KeyCode::Tab => self.focus = (self.focus + 1) % 3,
            KeyCode::BackTab => self.focus = (self.focus + 2) % 3,
            KeyCode::Enter if key.kind != KeyEventKind::Repeat => {
                let context = self.context.clone().unwrap();
                match backend::destination(&context, &self.destination) {
                    Ok(destination) => {
                        let branch = self.branch.clone();
                        self.error = None;
                        self.task = Some(tokio::spawn(async move {
                            backend::prepare(context, branch, destination)
                                .await
                                .map(ResultStage::Preview)
                        }));
                    }
                    Err(error) => self.error = Some(error),
                }
            }
            _ if self.focus < 2 => {
                let input = if self.focus == 0 {
                    &mut self.branch
                } else {
                    &mut self.destination
                };
                let caret = &mut self.caret[self.focus];
                let previous = input[..*caret].char_indices().last().map_or(0, |(i, _)| i);
                let next = input[*caret..]
                    .chars()
                    .next()
                    .map_or(*caret, |c| *caret + c.len_utf8());
                match key.code {
                    KeyCode::Left => *caret = previous,
                    KeyCode::Right => *caret = next,
                    KeyCode::Home => *caret = 0,
                    KeyCode::End => *caret = input.len(),
                    KeyCode::Backspace if *caret > 0 => {
                        input.replace_range(previous..*caret, "");
                        *caret = previous;
                        self.changed();
                    }
                    KeyCode::Delete => {
                        input.replace_range(*caret..next, "");
                        self.changed();
                    }
                    KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                        input.clear();
                        *caret = 0;
                        self.changed();
                    }
                    KeyCode::Char(c)
                        if !key
                            .modifiers
                            .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
                            && !c.is_control() =>
                    {
                        input.insert(*caret, c);
                        *caret += c.len_utf8();
                        self.changed();
                    }
                    _ => {}
                }
            }
            _ => {}
        }
        false
    }
    pub fn draw(&self, frame: &mut Frame<'_>) {
        let area = frame.area();
        if area.width < 40 || area.height < 16 {
            frame.render_widget(Paragraph::new("Create worktree: enlarge terminal"), area);
            return;
        }
        let width = area.width.min(96);
        let height = area.height.min(28);
        let area = Rect::new(
            area.x + (area.width - width) / 2,
            area.y + (area.height - height) / 2,
            width,
            height,
        );
        frame.render_widget(Clear, area);
        let block = Block::default()
            .borders(Borders::ALL)
            .title(if self.plan.is_some() {
                "Review new worktree"
            } else {
                "Create worktree"
            })
            .border_style(Style::default().fg(Color::Cyan));
        let inner = block.inner(area);
        frame.render_widget(block, area);
        let sections = Layout::vertical([Constraint::Min(1), Constraint::Length(2)]).split(inner);
        let mut lines = vec![];
        if let Some(context) = &self.context {
            lines.push(Line::from(format!(
                "Source: {}",
                crate::browser::safe_text(&context.source.display().to_string())
            )));
            lines.push(Line::from(format!(
                "Starting commit: {} {}",
                &context.commit[..8],
                crate::browser::safe_text(&context.subject)
            )));
            lines.push(Line::from(""));
        }
        if let Some(plan) = &self.plan {
            let portability = if plan.outside {
                "This destination is outside the project. Moving the project alone will not move this worktree."
            } else {
                "Move the whole main checkout to keep these relative Git links intact. External moves do not update Codex session paths."
            };
            let exclusion = plan.exclusion.as_ref().map_or_else(
                || "No new local exclusion is needed.".to_owned(),
                |pattern| format!("Add local exclusion {pattern} to {}. Tracked .gitignore files stay unchanged.", plan.context.common.join("info/exclude").display()),
            );
            let details = [
                format!("New branch: {}", plan.branch),
                format!("Destination: {}", plan.destination.display()),
                String::new(),
                "Starts from committed content; uncommitted changes stay in the source checkout.".into(),
                "Relative Git links require Git 2.48+ and compatible tools. Git enables a repository extension; older Git cannot open this repository.".into(),
                portability.into(), exclusion, String::new(), "Enter: Create worktree".into(),
            ];
            for line in details {
                lines.push(Line::from(crate::browser::safe_text(&line)));
            }
        } else if self.context.is_some() {
            for (index, (label, value)) in [
                ("New branch", &self.branch),
                ("Destination", &self.destination),
            ]
            .iter()
            .enumerate()
            {
                lines.push(Line::from(*label));
                let mut value = value.to_string();
                if self.focus == index {
                    value.insert(self.caret[index], '▏');
                }
                // Scroll with the caret and account for wide Unicode characters.
                let limit = usize::from(inner.width).saturating_sub(2);
                let cursor = if self.focus == index {
                    self.caret[index]
                } else {
                    value.len()
                };
                let mut start = cursor;
                let mut columns = 1;
                for (offset, character) in value[..cursor].char_indices().rev() {
                    let width = unicode_width::UnicodeWidthChar::width(character).unwrap_or(0);
                    if columns + width > limit {
                        break;
                    }
                    columns += width;
                    start = offset;
                }
                let mut columns = 0;
                let shown: String = value[start..]
                    .chars()
                    .take_while(|character| {
                        columns += unicode_width::UnicodeWidthChar::width(*character).unwrap_or(0);
                        columns <= limit
                    })
                    .collect();
                lines.push(Line::from(Span::styled(
                    shown,
                    if self.focus == index {
                        Style::default().fg(Color::Cyan)
                    } else {
                        Style::default()
                    },
                )));
            }
            lines.push(Line::from(Span::styled(
                "[ Review ]",
                if self.focus == 2 {
                    Style::default().fg(Color::Cyan)
                } else {
                    Style::default()
                },
            )));
            lines.push(Line::from(
                "Relative destinations use the source checkout root.",
            ));
        }
        if let Some(error) = &self.error {
            lines.push(Line::from(Span::styled(
                crate::browser::safe_text(error),
                Style::default().fg(Color::Red),
            )));
        }
        frame.render_widget(
            Paragraph::new(lines)
                .wrap(Wrap { trim: false })
                .scroll((self.scroll, 0)),
            sections[0],
        );
        let controls = if self.executing {
            "Creating worktree… Please wait. Cancellation is disabled."
        } else if self.task.is_some() {
            "Checking repository… Esc cancels"
        } else if self.plan.is_some() {
            "Enter Create worktree · Esc edit · ↑/↓ scroll"
        } else {
            "Tab controls · Enter review · Ctrl-U clear · Esc cancel"
        };
        frame.render_widget(
            Paragraph::new(controls).wrap(Wrap { trim: false }),
            sections[1],
        );
    }
}
