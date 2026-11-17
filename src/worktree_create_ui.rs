//! Shared worktree dialog for checkout HEAD and selected history commits.
use crate::worktree_create::{self as backend, Context, Plan};
use crossterm::event::{KeyCode, KeyEvent, KeyEventKind};
use ratatui::{
    Frame,
    layout::{Constraint, Layout},
    style::{Color, Style},
    text::Line,
    widgets::{Paragraph, Wrap},
};
use std::path::PathBuf;

enum ResultStage {
    Context(Context),
    Prepared(Plan),
    Created(PathBuf),
}
pub struct Modal {
    pub context: Option<Context>,
    pub branch: String,
    pub destination: String,
    pub focus: usize,
    pub error: Option<String>,
    pub executing: bool,
    task: Option<tokio::task::JoinHandle<Result<ResultStage, String>>>,
    edited_destination: bool,
    caret: [usize; 2],
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
            error: None,
            executing: false,
            edited_destination: false,
            caret: [0, 0],
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
            Ok(ResultStage::Prepared(plan)) => {
                self.executing = true;
                self.task = Some(tokio::spawn(async move {
                    backend::execute(plan).await.map(ResultStage::Created)
                }));
            }
            Ok(ResultStage::Created(path)) => {
                self.executing = false;
                return Some(Ok(path));
            }
            Err(error) => {
                if self.executing {
                    self.executing = false;
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
        if self.task.is_some() || self.focus > 1 {
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
            return true;
        }
        if self.task.is_some() || self.context.is_none() {
            if matches!(key.code, KeyCode::Tab | KeyCode::BackTab) {
                self.focus = 2;
            }
            return self.focus == 2 && matches!(key.code, KeyCode::Enter | KeyCode::Char(' '));
        }
        match key.code {
            KeyCode::Tab => crate::modal::cycle(&mut self.focus, &[true, true, true, true], false),
            KeyCode::BackTab => {
                crate::modal::cycle(&mut self.focus, &[true, true, true, true], true)
            }
            KeyCode::Enter if self.focus < 2 && key.kind != KeyEventKind::Repeat => {
                self.focus = if self.focus == 0 { 1 } else { 3 };
            }
            KeyCode::Enter | KeyCode::Char(' ')
                if self.focus >= 2 && key.kind != KeyEventKind::Repeat =>
            {
                if self.focus == 2 {
                    return true;
                }
                let context = self.context.clone().unwrap();
                match backend::destination(&context, &self.destination) {
                    Ok(destination) => {
                        let branch = self.branch.clone();
                        self.error = None;
                        self.task = Some(tokio::spawn(async move {
                            backend::prepare(context, branch, destination)
                                .await
                                .map(ResultStage::Prepared)
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
                if crate::modal::edit(input, &mut self.caret[self.focus], key) {
                    self.changed();
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
        let inner = crate::modal::shell(frame, "Create worktree", 96, 28);
        let sections = crate::modal::regions(inner);
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
        let summary = lines
            .iter()
            .map(|line| {
                line.spans
                    .iter()
                    .map(|span| span.content.as_ref())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n");
        let notes = "Committed content only; uncommitted changes stay in the source.\nEnables relative links: this repository will require Git 2.48+.\nIn-project destinations are ignored locally; outside paths stay separate.";
        let notes_height = Paragraph::new(notes)
            .wrap(Wrap { trim: false })
            .line_count(sections.body.width) as u16;
        let gap = u16::from(sections.body.height >= 12 + notes_height);
        let summary_height = Paragraph::new(summary.clone())
            .wrap(Wrap { trim: false })
            .line_count(sections.body.width)
            .min(4) as u16;
        if sections.body.height < summary_height + 6 + 2 * gap + notes_height {
            frame.render_widget(Paragraph::new("Create worktree: enlarge terminal"), area);
            return;
        }
        let fields = Layout::vertical([
            Constraint::Length(summary_height),
            Constraint::Length(gap),
            Constraint::Length(3),
            Constraint::Length(gap),
            Constraint::Length(3),
            Constraint::Min(notes_height),
        ])
        .split(sections.body);
        crate::modal::body(frame, fields[0], summary, 0);
        crate::modal::text_field(
            frame,
            fields[2],
            "New branch",
            &self.branch,
            self.caret[0],
            self.focus == 0,
            self.task.is_none(),
        );
        crate::modal::text_field(
            frame,
            fields[4],
            "Destination",
            &self.destination,
            self.caret[1],
            self.focus == 1,
            self.task.is_none(),
        );
        crate::modal::body(frame, fields[5], notes, 0);
        let status = if self.executing {
            "Creating worktree… Please wait. Cancellation is disabled."
        } else if self.task.is_some() {
            "Checking repository… Esc cancels"
        } else {
            self.error
                .as_deref()
                .unwrap_or("Relative destinations use the source checkout root.")
        };
        frame.render_widget(
            Paragraph::new(status)
                .wrap(Wrap { trim: false })
                .style(Style::default().fg(if self.error.is_some() {
                    Color::Red
                } else {
                    Color::Gray
                })),
            sections.status,
        );
        use crate::modal::Button;
        let buttons = [
            Button {
                action: 0,
                label: "Cancel",
                enabled: !self.executing,
                primary: false,
            },
            Button {
                action: 1,
                label: "Create worktree",
                enabled: self.task.is_none() && self.context.is_some(),
                primary: true,
            },
        ];
        crate::modal::buttons(frame, sections.actions, &buttons, self.focus.checked_sub(2));
        frame.render_widget(
            Paragraph::new("Tab controls · Enter activate/next · Esc cancel"),
            sections.hints,
        );
    }
}
