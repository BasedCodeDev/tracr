//! On-demand commit details with a compact overview and dedicated file/metadata views.
use crate::{
    error_dialog::{ErrorDialog, Event},
    info_content::{local_date, wrap},
};
use crossterm::event::{KeyCode, KeyEvent, KeyEventKind};
use ratatui::{
    Frame,
    layout::{Constraint, Layout},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::Paragraph,
};
use std::path::{Path, PathBuf};
use tokio::task::JoinHandle;

struct FileChange {
    path: String,
    added: Option<u64>,
    removed: Option<u64>,
}
struct Details {
    repository: PathBuf,
    oid: String,
    parents: String,
    author: String,
    authored: String,
    committer: String,
    committed: String,
    refs: String,
    message: String,
    files: Vec<FileChange>,
}
impl Details {
    fn subject(&self) -> &str {
        self.message.lines().next().unwrap_or("Untitled commit")
    }
    fn totals(&self) -> (u64, u64) {
        self.files.iter().fold((0, 0), |(a, r), f| {
            (
                a.saturating_add(f.added.unwrap_or(0)),
                r.saturating_add(f.removed.unwrap_or(0)),
            )
        })
    }
    fn metadata(&self) -> Vec<(&str, String)> {
        vec![
            ("Commit", self.oid.clone()),
            (
                "Parents",
                if self.parents.is_empty() {
                    "None · root commit".into()
                } else {
                    self.parents.replace(' ', "\n")
                },
            ),
            ("Author", self.author.clone()),
            ("Author date", self.authored.clone()),
            ("Committer", self.committer.clone()),
            ("Commit date", self.committed.clone()),
            (
                "Refs",
                if self.refs.is_empty() {
                    "No named refs".into()
                } else {
                    self.refs.clone()
                },
            ),
            ("Repository", self.repository.display().to_string()),
        ]
    }
    fn report(&self) -> String {
        let mut report = self
            .metadata()
            .into_iter()
            .map(|(k, v)| format!("{k}: {v}"))
            .collect::<Vec<_>>()
            .join("\n");
        report.push_str(&format!(
            "\n\n{}\n\nFiles changed: {}\n",
            self.message,
            self.files.len()
        ));
        for file in &self.files {
            report.push_str(&format!(
                "{}  {}\n",
                file.path,
                match (file.added, file.removed) {
                    (Some(a), Some(r)) => format!("+{a} -{r}"),
                    _ => "Binary".into(),
                }
            ));
        }
        if self.parents.split_whitespace().count() > 1 {
            report.push_str("Changes compared with the first parent.\n");
        }
        report
    }
    fn lines(&self, tab: usize, width: usize) -> Vec<Line<'static>> {
        let mut lines = Vec::new();
        let muted = Style::default().add_modifier(Modifier::DIM);
        match tab {
            0 => {
                section(&mut lines, "MESSAGE");
                let body = self
                    .message
                    .split_once('\n')
                    .map_or("", |(_, body)| body.trim_start());
                // Preserve long subjects in the scrollable body as well as the fixed heading.
                let body = if wrap(self.subject(), width).len() > 3 {
                    &self.message
                } else {
                    body
                };
                append(
                    &mut lines,
                    if body.is_empty() {
                        "No extended commit message."
                    } else {
                        body
                    },
                    width,
                    if body.is_empty() {
                        muted
                    } else {
                        Style::default()
                    },
                );
                lines.push(Line::default());
                section(&mut lines, "AUTHORED");
                append(&mut lines, &self.author, width, Style::default());
                append(&mut lines, &self.authored, width, muted);
                if !self.refs.is_empty() {
                    lines.push(Line::default());
                    section(&mut lines, "REFERENCES");
                    append(
                        &mut lines,
                        &self.refs,
                        width,
                        Style::default().fg(crate::git_render::BRANCH_COLOR),
                    );
                }
            }
            1 => {
                if self.parents.split_whitespace().count() > 1 {
                    append(&mut lines, "Compared with the first parent", width, muted);
                    lines.push(Line::default());
                }
                if self.files.is_empty() {
                    append(&mut lines, "No file changes in this commit.", width, muted);
                }
                let table = width >= 60
                    && self.files.iter().all(|file| {
                        file.added.unwrap_or(0) < 10_000_000
                            && file.removed.unwrap_or(0) < 10_000_000
                    });
                let path_width = width.saturating_sub(20);
                if table && !self.files.is_empty() {
                    lines.push(Line::styled(
                        format!("{:<path_width$}  {:>8}  {:>8}", "FILE", "ADDED", "REMOVED"),
                        muted,
                    ));
                    lines.push(Line::default());
                }
                for file in &self.files {
                    if table {
                        for (index, path) in wrap(&file.path, path_width).into_iter().enumerate() {
                            let padding = path_width.saturating_sub(
                                unicode_width::UnicodeWidthStr::width(path.as_str()),
                            );
                            let mut spans =
                                vec![Span::raw(format!("{path}{}", " ".repeat(padding)))];
                            if index == 0 {
                                match (file.added, file.removed) {
                                    (Some(a), Some(r)) => spans.extend([
                                        Span::styled(
                                            format!("  {:>8}", format!("+{a}")),
                                            Style::default().fg(Color::Green),
                                        ),
                                        Span::styled(
                                            format!("  {:>8}", format!("−{r}")),
                                            Style::default().fg(Color::Red),
                                        ),
                                    ]),
                                    _ => spans.push(Span::styled("    Binary file", muted)),
                                }
                            }
                            lines.push(Line::from(spans));
                        }
                        lines.push(Line::default());
                        continue;
                    }
                    append(
                        &mut lines,
                        &file.path,
                        width,
                        Style::default().add_modifier(Modifier::BOLD),
                    );
                    match (file.added, file.removed) {
                        (Some(a), Some(r)) => lines.push(Line::from(vec![
                            Span::styled(format!("+{a}"), Style::default().fg(Color::Green)),
                            Span::raw("  "),
                            Span::styled(format!("−{r}"), Style::default().fg(Color::Red)),
                        ])),
                        _ => lines.push(Line::styled("Binary file", muted)),
                    }
                    lines.push(Line::default());
                }
            }
            _ => {
                for (label, value) in self.metadata() {
                    let value_style = if label == "Refs" {
                        Style::default().fg(crate::git_render::BRANCH_COLOR)
                    } else if label == "Commit" && width >= 60 {
                        Style::default().fg(Color::Cyan)
                    } else {
                        Style::default()
                    };
                    if width >= 60 {
                        for (index, value) in wrap(&value, width - 14).into_iter().enumerate() {
                            lines.push(Line::from(vec![
                                Span::styled(
                                    format!("{:<14}", if index == 0 { label } else { "" }),
                                    muted,
                                ),
                                Span::styled(value, value_style),
                            ]));
                        }
                        if label == "Parents" || label == "Commit date" {
                            lines.push(Line::default());
                        }
                        continue;
                    }
                    section(&mut lines, &label.to_uppercase());
                    append(&mut lines, &value, width, value_style);
                    lines.push(Line::default());
                }
            }
        }
        lines
    }
}
fn section(lines: &mut Vec<Line<'static>>, text: &str) {
    lines.push(Line::styled(
        text.to_owned(),
        Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD),
    ));
}
fn append(lines: &mut Vec<Line<'static>>, text: &str, width: usize, style: Style) {
    lines.extend(
        wrap(text, width)
            .into_iter()
            .map(|text| Line::styled(text, style)),
    );
}

pub struct Dialog {
    pub dialog: ErrorDialog,
    task: Option<JoinHandle<Result<Details, String>>>,
    details: Option<Details>,
    error: Option<String>,
    oid: String,
    tab: usize,
    scrolls: [u16; 3],
    copied_hash: bool,
}
impl Dialog {
    pub fn new(repository: PathBuf, oid: String) -> Self {
        let selected = oid.clone();
        Self {
            dialog: ErrorDialog::information("Commit information", ""),
            task: Some(tokio::spawn(async move {
                details(&repository, &selected).await
            })),
            details: None,
            error: None,
            oid,
            tab: 0,
            scrolls: [0; 3],
            copied_hash: false,
        }
    }
    pub fn ready(&self) -> bool {
        self.task.as_ref().is_some_and(|task| task.is_finished())
            || self
                .dialog
                .copy_task
                .as_ref()
                .is_some_and(|task| task.is_finished())
    }
    pub async fn tick(&mut self) {
        if self.task.as_ref().is_some_and(|task| task.is_finished()) {
            match self.task.take().expect("finished commit details").await {
                Ok(Ok(details)) => {
                    self.dialog.set_body(details.report());
                    self.details = Some(details);
                }
                result => {
                    self.error = Some(match result {
                        Ok(Err(error)) => error,
                        Err(error) => error.to_string(),
                        _ => unreachable!(),
                    })
                }
            }
        }
        self.dialog.tick().await;
        if self.dialog.copy_status.as_deref() == Some("Message copied") {
            self.dialog.copy_status = Some(
                if self.copied_hash {
                    "Commit hash copied"
                } else {
                    "Commit details copied"
                }
                .into(),
            );
        }
    }
    pub fn copy_report(&mut self) {
        if self.details.is_some() && self.dialog.copy_task.is_none() {
            self.copied_hash = false;
            self.dialog.start_copy();
            self.dialog.copy_status = Some("Copying commit details…".into());
        }
    }
    pub fn key(&mut self, key: KeyEvent) -> Event {
        if key.kind == KeyEventKind::Release {
            return Event::None;
        }
        let tab = match key.code {
            KeyCode::Char('1') => Some(0),
            KeyCode::Char('2') => Some(1),
            KeyCode::Char('3') => Some(2),
            KeyCode::Left => Some((self.tab + 2) % 3),
            KeyCode::Right => Some((self.tab + 1) % 3),
            _ => None,
        };
        if let Some(tab) = tab {
            self.scrolls[self.tab] = self.dialog.scroll;
            self.tab = tab;
            self.dialog.scroll = self.scrolls[tab];
            return Event::None;
        }
        match key.code {
            KeyCode::Tab | KeyCode::BackTab => {
                crate::modal::cycle(
                    &mut self.dialog.button_focus,
                    &[
                        self.details.is_some() && self.dialog.copy_task.is_none(),
                        true,
                    ],
                    key.code == KeyCode::BackTab,
                );
                Event::None
            }
            KeyCode::Enter | KeyCode::Char(' ')
                if self.dialog.button_focus == 0 && self.details.is_none() =>
            {
                Event::None
            }
            KeyCode::Char('y') if self.dialog.copy_task.is_none() => {
                self.copied_hash = true;
                self.dialog.copy_status = Some("Copying commit hash…".into());
                self.dialog.copy_task =
                    Some(tokio::spawn(crate::clipboard::copy(self.oid.clone())));
                Event::None
            }
            KeyCode::Char('c') if self.details.is_none() => Event::None,
            KeyCode::Char('j') => self.dialog.key(KeyEvent::new(KeyCode::Down, key.modifiers)),
            KeyCode::Char('k') => self.dialog.key(KeyEvent::new(KeyCode::Up, key.modifiers)),
            _ => self.dialog.key(key),
        }
    }
    pub fn draw(&self, frame: &mut Frame<'_>) {
        if frame.area().width < 36 || frame.area().height < 16 {
            let inner = crate::modal::shell(frame, "Commit", 96, 34);
            frame.render_widget(
                Paragraph::new("Enlarge terminal to view details.\nEsc close · y copy hash")
                    .wrap(ratatui::widgets::Wrap { trim: false }),
                inner,
            );
            return;
        }
        let inner = crate::modal::shell(frame, "Commit information", 96, 34);
        let width = usize::from(inner.width);
        let subject = self.details.as_ref().map_or(
            if self.error.is_some() {
                "Unable to load commit"
            } else {
                "Loading commit…"
            },
            |d| d.subject(),
        );
        let title = wrap(subject, width);
        let title_height = title.len().min(3) as u16;
        let areas = Layout::vertical([
            Constraint::Length(title_height),
            Constraint::Length(2),
            Constraint::Length(2),
            Constraint::Min(1),
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(1),
        ])
        .split(inner);
        frame.render_widget(
            Paragraph::new(
                title
                    .into_iter()
                    .take(3)
                    .map(|s| Line::styled(s, Style::default().add_modifier(Modifier::BOLD)))
                    .collect::<Vec<_>>(),
            ),
            areas[0],
        );
        let mut identity = vec![Span::styled(
            self.oid.chars().take(12).collect::<String>(),
            Style::default().fg(Color::Cyan),
        )];
        if let Some(details) = &self.details {
            let (a, r) = details.totals();
            identity.extend([
                Span::raw(format!(
                    "  ·  {} {}  ",
                    details.files.len(),
                    if details.files.len() == 1 {
                        "file"
                    } else {
                        "files"
                    }
                )),
                Span::styled(format!("+{a}"), Style::default().fg(Color::Green)),
                Span::raw("  "),
                Span::styled(format!("−{r}"), Style::default().fg(Color::Red)),
            ]);
            let binary = details
                .files
                .iter()
                .filter(|file| file.added.is_none())
                .count();
            if binary > 0 {
                identity.push(Span::styled(
                    format!("  ·  {binary} binary"),
                    Style::default().add_modifier(Modifier::DIM),
                ));
            }
        }
        frame.render_widget(
            Paragraph::new(Line::from(identity)).wrap(ratatui::widgets::Wrap { trim: false }),
            areas[1],
        );
        let labels = if width < 45 {
            ["1 Info", "2 Files", "3 Details"]
        } else {
            ["1 Overview", "2 Files", "3 Details"]
        };
        let tabs = labels
            .iter()
            .enumerate()
            .flat_map(|(i, label)| {
                [
                    Span::styled(
                        format!(" {label} "),
                        if i == self.tab {
                            Style::default()
                                .fg(Color::Black)
                                .bg(Color::Cyan)
                                .add_modifier(Modifier::BOLD)
                        } else {
                            Style::default().add_modifier(Modifier::DIM)
                        },
                    ),
                    Span::raw(" "),
                ]
            })
            .collect::<Vec<_>>();
        frame.render_widget(Paragraph::new(Line::from(tabs)), areas[2]);
        let build_lines = |width: usize| {
            if let Some(details) = &self.details {
                details.lines(self.tab, width)
            } else {
                let mut lines = vec![];
                append(
                    &mut lines,
                    self.error
                        .as_deref()
                        .unwrap_or("Reading commit metadata and file changes…"),
                    width,
                    Style::default().fg(if self.error.is_some() {
                        Color::Yellow
                    } else {
                        Color::Gray
                    }),
                );
                if self.error.is_some() {
                    lines.push(Line::default());
                    append(
                        &mut lines,
                        "Close and press i to try again.",
                        width,
                        Style::default(),
                    );
                }
                lines
            }
        };
        let mut lines = build_lines(width);
        let body = crate::scrollbar::content_area(areas[3], lines.len());
        if body.width < areas[3].width {
            lines = build_lines(usize::from(body.width));
        }
        let total = lines.len();
        let max = total
            .saturating_sub(usize::from(areas[3].height))
            .min(u16::MAX as usize) as u16;
        self.dialog.scroll_limit.set(max);
        let scroll = self.dialog.scroll.min(max);
        frame.render_widget(Paragraph::new(lines).scroll((scroll, 0)), body);
        if body.width < areas[3].width {
            crate::scrollbar::draw(
                frame,
                crate::scrollbar::border_track(areas[3], body),
                total,
                usize::from(body.height),
                usize::from(scroll),
            );
        }
        let status = self.dialog.copy_status.clone().unwrap_or_else(|| {
            if max > 0 {
                format!(
                    "{}–{} of {total} lines · ↑/↓ scroll",
                    scroll + 1,
                    (usize::from(scroll) + usize::from(areas[3].height)).min(total)
                )
            } else {
                String::new()
            }
        });
        frame.render_widget(
            Paragraph::new(status).style(Style::default().fg(Color::Cyan)),
            areas[4],
        );
        crate::modal::buttons(
            frame,
            areas[5],
            &[
                crate::modal::Button {
                    action: Event::Copy,
                    label: "Copy details",
                    enabled: self.details.is_some() && self.dialog.copy_task.is_none(),
                    primary: false,
                },
                crate::modal::Button {
                    action: Event::Close,
                    label: "Close",
                    enabled: true,
                    primary: true,
                },
            ],
            Some(self.dialog.button_focus),
        );
        let hints = if width < 45 {
            "1/2/3 tabs · c copy · y hash · Esc"
        } else if width >= 78 {
            "←/→ tabs · ↑/↓ scroll · Tab actions · Enter select · c copy · y hash · Esc close"
        } else {
            "←/→ tabs · ↑/↓ scroll · c copy · y hash · Esc close"
        };
        frame.render_widget(
            Paragraph::new(hints).style(Style::default().add_modifier(Modifier::DIM)),
            areas[6],
        );
    }
}
impl Drop for Dialog {
    fn drop(&mut self) {
        if let Some(task) = self.task.take() {
            task.abort();
        }
    }
}
async fn details(repository: &Path, oid: &str) -> Result<Details, String> {
    if ![40, 64].contains(&oid.len()) || !oid.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err("Invalid commit object ID".into());
    }
    let metadata = crate::git::read(
        repository,
        &[
            "show",
            "-s",
            "--no-show-signature",
            "--format=%H%x00%P%x00%an%x00%ae%x00%at%x00%cn%x00%ce%x00%ct%x00%D%x00%B",
            oid,
            "--",
        ],
    )
    .await?;
    let fields = metadata
        .splitn(10, |b| *b == 0)
        .map(|s| String::from_utf8_lossy(s).into_owned())
        .collect::<Vec<_>>();
    if fields.len() != 10 {
        return Err("Git returned incomplete commit metadata".into());
    }
    let changes = crate::git::read(
        repository,
        &[
            "show",
            "--format=",
            "--no-show-signature",
            "--no-color",
            "--no-ext-diff",
            "--no-textconv",
            "--no-renames",
            "--numstat",
            "-z",
            "--root",
            "--diff-merges=first-parent",
            oid,
            "--",
        ],
    )
    .await?;
    let mut files = vec![];
    for record in changes.split(|b| *b == 0).filter(|s| !s.is_empty()) {
        let parts = record.splitn(3, |b| *b == b'\t').collect::<Vec<_>>();
        if parts.len() != 3 {
            return Err("Git returned incomplete file changes".into());
        }
        let number = |bytes: &[u8]| -> Result<Option<u64>, String> {
            if bytes == b"-" {
                Ok(None)
            } else {
                String::from_utf8_lossy(bytes)
                    .parse()
                    .map(Some)
                    .map_err(|_| "Git returned invalid change counts".into())
            }
        };
        files.push(FileChange {
            path: String::from_utf8_lossy(parts[2]).into_owned(),
            added: number(parts[0])?,
            removed: number(parts[1])?,
        });
    }
    let date = |s: &str| local_date(s.parse().unwrap_or(0));
    Ok(Details {
        repository: repository.into(),
        oid: fields[0].clone(),
        parents: fields[1].clone(),
        author: format!("{} <{}>", fields[2], fields[3]),
        authored: date(&fields[4]),
        committer: format!("{} <{}>", fields[5], fields[6]),
        committed: date(&fields[7]),
        refs: fields[8].replace(", ", "\n"),
        message: fields[9].trim_end().to_owned(),
        files,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn example() -> Dialog {
        let details = Details {
            repository: "/home/dev/projects/tracr/.git".into(),
            oid: "abc123def456789012345678901234567890123456".into(),
            parents: "0123456789012345678901234567890123456789".into(),
            author: "Alex Morgan <alex@example.com>".into(),
            authored: "19 Nov 2026, 14:32 AEDT (+1100)".into(),
            committer: "Alex Morgan <alex@example.com>".into(),
            committed: "19 Nov 2026, 14:32 AEDT (+1100)".into(),
            refs: "HEAD -> feature/commit-details\norigin/feature/commit-details".into(),
            message: "Make commit history easier to explore\n\nGive each commit a readable overview, a focused file list, and complete metadata.\n\nKeep keyboard navigation predictable and preserve the reader’s place when switching tabs.".into(),
            files: vec![
                FileChange { path: "src/commit_info.rs".into(), added: Some(142), removed: Some(38) },
                FileChange { path: "docs/界面/very-long-file-name-that-must-remain-readable-on-narrow-terminals.md".into(), added: Some(18), removed: Some(4) },
                FileChange { path: "assets/preview.png".into(), added: None, removed: None },
            ],
        };
        let mut dialog = ErrorDialog::information("Commit information", "");
        dialog.set_body(details.report());
        Dialog {
            dialog,
            task: None,
            oid: details.oid.clone(),
            details: Some(details),
            error: None,
            tab: 0,
            scrolls: [0; 3],
            copied_hash: false,
        }
    }

    #[test]
    fn responsive_tabs_keep_controls_visible_and_restore_scroll() {
        use crossterm::event::KeyModifiers;
        use ratatui::{Terminal, backend::TestBackend};
        let mut info = example();
        let key = |code| KeyEvent::new(code, KeyModifiers::NONE);
        for (width, height) in [(120, 40), (80, 24), (40, 18), (36, 16), (20, 8), (1, 1)] {
            let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
            for tab in 0..3 {
                info.tab = tab;
                terminal.draw(|frame| info.draw(frame)).unwrap();
                let buffer = terminal.backend().buffer();
                let text: String = buffer.content.iter().map(|cell| cell.symbol()).collect();
                if width >= 36 && height >= 16 {
                    assert!(
                        text.contains("1 ")
                            && text.contains("2 Files")
                            && text.contains("3 Details"),
                        "{width}x{height}: {text}"
                    );
                    assert!(text.contains("Close") && text.contains("y hash"), "{text}");
                }
            }
        }
        info.tab = 2;
        info.dialog.scroll_limit.set(100);
        info.key(key(KeyCode::PageDown));
        assert_eq!(info.dialog.scroll, 10);
        info.key(key(KeyCode::Char('1')));
        assert_eq!(info.dialog.scroll, 0);
        info.key(key(KeyCode::Left));
        assert_eq!(info.tab, 2);
        assert_eq!(info.dialog.scroll, 10);
        assert!(matches!(info.key(key(KeyCode::Char('c'))), Event::Copy));
        assert!(info.dialog.report().contains("assets/preview.png  Binary"));
        assert!(matches!(info.key(key(KeyCode::Esc)), Event::Close));
    }

    #[test]
    fn loading_and_error_states_keep_close_available_without_copying_partial_details() {
        use crossterm::event::KeyModifiers;
        let mut info = example();
        info.details = None;
        let key = |code| KeyEvent::new(code, KeyModifiers::NONE);
        assert!(matches!(info.key(key(KeyCode::Char('c'))), Event::None));
        info.key(key(KeyCode::Tab));
        assert_eq!(info.dialog.button_focus, 1);
        assert!(matches!(info.key(key(KeyCode::Enter)), Event::Close));
        info.error = Some("Commit object is no longer available".into());
        let mut terminal =
            ratatui::Terminal::new(ratatui::backend::TestBackend::new(80, 24)).unwrap();
        terminal.draw(|frame| info.draw(frame)).unwrap();
        let text: String = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect();
        assert!(text.contains("Unable to load commit"));
        assert!(text.contains("Close and press i to try again."));
    }

    fn git(path: &std::path::Path, args: &[&str]) -> String {
        let output = std::process::Command::new("git")
            .arg("-C")
            .arg(path)
            .args([
                "-c",
                "user.name=Commit Tester",
                "-c",
                "user.email=test@example.invalid",
                "-c",
                "commit.gpgsign=false",
            ])
            .args(args)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).unwrap().trim().into()
    }

    #[tokio::test]
    async fn details_read_selected_commit_instead_of_head_and_include_full_message() {
        let repo = tempfile::tempdir().unwrap();
        git(repo.path(), &["init", "-qb", "main"]);
        std::fs::write(repo.path().join("example.txt"), "one\n").unwrap();
        std::fs::write(repo.path().join("tab\tname.txt"), "other\n").unwrap();
        std::fs::write(repo.path().join("binary.dat"), b"\0\x01\x02").unwrap();
        git(repo.path(), &["add", "."]);
        git(
            repo.path(),
            &[
                "commit",
                "-qm",
                "First subject\n\nFull body with Unicode: café\nAnother paragraph.",
            ],
        );
        let oid = git(repo.path(), &["rev-parse", "HEAD"]);
        git(
            repo.path(),
            &["commit", "--allow-empty", "-qm", "Later commit"],
        );
        let head = git(repo.path(), &["rev-parse", "HEAD"]);
        let body = details(&repo.path().join(".git"), &oid)
            .await
            .unwrap()
            .report();
        assert!(body.contains("tab\tname.txt  +1 -0"));
        assert!(body.contains("binary.dat  Binary"));
        for expected in [
            &oid,
            "Parents:",
            "Author: Commit Tester <test@example.invalid>",
            "Author date:",
            "Committer:",
            "Commit date:",
            "First subject",
            "Full body with Unicode: café",
            "Another paragraph.",
            "example.txt",
            "+1 -0",
        ] {
            assert!(body.contains(expected), "missing {expected}: {body}");
        }
        assert!(!body.contains("Later commit"));
        assert!(
            details(repo.path(), &head)
                .await
                .unwrap()
                .report()
                .contains(&format!("Parents: {oid}"))
        );
        assert!(details(repo.path(), "--all").await.is_err());
        assert!(details(repo.path(), &"0".repeat(40)).await.is_err());
    }
}
