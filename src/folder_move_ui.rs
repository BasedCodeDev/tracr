use crate::{
    browser::{Browser, FolderFilter, SourceSpec, safe_text},
    folder_move,
    tui::{Action, PickerText, render_picker},
};
use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::{
    Frame,
    layout::{Constraint, Layout, Rect},
    widgets::{Block, Borders, Clear, Paragraph, Wrap},
};
use std::path::{Path, PathBuf};

pub fn sources(browser: &Browser) -> Vec<folder_move::Source> {
    browser
        .specs
        .iter()
        .filter_map(|s| match s {
            SourceSpec::Codex { binary, source } => Some(folder_move::Source {
                binary: binary.clone(),
                home: source.home.clone(),
                id: source.id.clone(),
            }),
            _ => None,
        })
        .collect()
}
pub fn unavailable(browser: &Browser) -> Option<String> {
    if browser.loading() {
        return Some("Wait for discovery to finish.".into());
    }
    if browser
        .specs
        .iter()
        .any(|s| matches!(s, SourceSpec::Fixture(_)))
    {
        return Some("Fixture data is browse-only.".into());
    }
    if browser.diagnostics().iter().any(|d| !d.complete) {
        return Some("Resolve source discovery warnings before moving a folder.".into());
    }
    if browser.state_path.is_none() {
        return Some("Persistent state is required for move recovery.".into());
    }
    match &browser.folder {
        FolderFilter::Path(p) if p.is_dir() => None,
        _ => Some("Select an existing folder in the Folders pane.".into()),
    }
}
pub struct Modal {
    pub old: PathBuf,
    pub input: String,
    pub name: String,
    pub picker: Option<crate::path_picker::Picker>,
    pub naming: bool,
    pub plan: Option<folder_move::Plan>,
    pub recovery: bool,
    pub busy: bool,
    pub executing: bool,
    pub message: Option<String>,
    pub scroll: u16,
}
impl Modal {
    pub fn new(old: PathBuf, launch: PathBuf) -> Self {
        Self {
            input: old.parent().unwrap_or(Path::new("/")).display().to_string(),
            name: old.file_name().unwrap_or_default().to_string_lossy().into(),
            old,
            picker: Some(crate::path_picker::Picker::new(launch)),
            naming: false,
            plan: None,
            recovery: false,
            busy: false,
            executing: false,
            message: None,
            scroll: 0,
        }
    }
    pub fn recover(j: folder_move::Journal) -> Self {
        Self {
            old: j.plan.old.clone(),
            input: j
                .plan
                .target
                .parent()
                .unwrap_or(Path::new("/"))
                .display()
                .to_string(),
            name: j
                .plan
                .target
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .into(),
            picker: None,
            naming: true,
            plan: Some(j.plan),
            recovery: true,
            busy: false,
            executing: false,
            message: Some(format!(
                "Recorded phase: {}\n{}\nEnter attempts guarded completion. R cycles through unfinished operations.",
                j.phase,
                j.error.unwrap_or_default()
            )),
            scroll: 0,
        }
    }
    pub fn key(&mut self, key: KeyEvent) -> Action {
        if key.kind == KeyEventKind::Release {
            return Action::Continue;
        }
        if self.busy {
            return if !self.executing && key.code == KeyCode::Esc {
                Action::CloseFolderMove
            } else {
                Action::Continue
            };
        }
        if key.code == KeyCode::Char('R') && self.recovery {
            return Action::RecoverRelink;
        }
        if key.code == KeyCode::Esc {
            if self.naming && self.plan.is_none() {
                self.naming = false;
            } else {
                return Action::CloseFolderMove;
            }
        } else if self.plan.is_some() {
            if key.code == KeyCode::Enter {
                return Action::ApplyFolderMove;
            }
            match key.code {
                KeyCode::Down => self.scroll = self.scroll.saturating_add(1),
                KeyCode::Up => self.scroll = self.scroll.saturating_sub(1),
                _ => {}
            }
        } else if self.naming {
            match key.code {
                KeyCode::Enter => return Action::PreviewFolderMove,
                KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    self.name.clear()
                }
                KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                    self.name.push(c)
                }
                KeyCode::Backspace => {
                    self.name.pop();
                }
                _ => {}
            }
        } else if let Some(picker) = self.picker.as_mut() {
            if picker.key(&mut self.input, key) {
                self.naming = true;
                self.message = None;
            }
        }
        Action::Continue
    }
    pub fn paste(&mut self, text: &str) {
        if self.busy || self.plan.is_some() {
            return;
        }
        if self.naming {
            self.name.extend(text.chars().filter(|c| !c.is_control()));
        } else if let Some(p) = self.picker.as_mut() {
            p.paste(&mut self.input, text);
        }
    }
    pub fn tick(&mut self) {
        if !self.busy && !self.naming && self.plan.is_none() {
            if let Some(p) = self.picker.as_mut() {
                p.tick(&mut self.input);
            }
        }
    }
    pub fn draw(&self, frame: &mut Frame<'_>) {
        if !self.naming && !self.busy {
            if let Some(p) = &self.picker {
                let heading = format!(
                    "Final path: {}",
                    Path::new(&self.input).join(&self.name).display()
                );
                render_picker(
                    frame,
                    p,
                    &self.input,
                    PickerText {
                        title: "Move folder",
                        heading: &heading,
                        old: &self.old.display().to_string(),
                        field: "Destination parent",
                        busy: false,
                        error: self.message.as_deref(),
                    },
                );
                return;
            }
        }
        let area = frame.area();
        let width = area.width.saturating_sub(2).min(86);
        let height = area.height.saturating_sub(2).min(28);
        let popup = Rect::new(
            area.x + (area.width - width) / 2,
            area.y + (area.height - height) / 2,
            width,
            height,
        );
        frame.render_widget(Clear, popup);
        let title = if self.recovery {
            "Inspect/recover folder move"
        } else {
            "Move folder"
        };
        let block = Block::default().borders(Borders::ALL).title(title);
        let inner = block.inner(popup);
        frame.render_widget(block, popup);
        let parts = Layout::vertical([Constraint::Min(1), Constraint::Length(2)]).split(inner);
        let target = self
            .plan
            .as_ref()
            .map(|p| p.target.clone())
            .unwrap_or_else(|| Path::new(&self.input).join(&self.name));
        if self.naming && self.plan.is_none() && !self.busy && inner.height >= 12 {
            let fields = Layout::vertical([
                Constraint::Length(3),
                Constraint::Length(3),
                Constraint::Min(1),
            ])
            .split(parts[0]);
            frame.render_widget(
                Paragraph::new(format!(
                    "From: {}\nParent: {}",
                    safe_text(&self.old.display().to_string()),
                    safe_text(&self.input)
                ))
                .wrap(Wrap { trim: false }),
                fields[0],
            );
            let available = usize::from(fields[1].width.saturating_sub(3));
            let mut start = 0;
            while unicode_width::UnicodeWidthStr::width(&self.name[start..]) > available
                && start < self.name.len()
            {
                start += self.name[start..].chars().next().unwrap().len_utf8();
            }
            frame.render_widget(
                Paragraph::new(safe_text(&self.name[start..]))
                    .block(Block::default().borders(Borders::ALL).title("Folder name:")),
                fields[1],
            );
            if fields[1].width > 2 {
                frame.set_cursor_position((
                    fields[1].x
                        + 1
                        + unicode_width::UnicodeWidthStr::width(&self.name[start..]) as u16,
                    fields[1].y + 1,
                ));
            }
            frame.render_widget(
                Paragraph::new(format!(
                    "Final path: {}\n{}",
                    safe_text(&target.display().to_string()),
                    safe_text(self.message.as_deref().unwrap_or(""))
                ))
                .wrap(Wrap { trim: false }),
                fields[2],
            );
            frame.render_widget(
                Paragraph::new(
                    "Edit name · Ctrl+U clear · Enter previews\nEsc returns to destination parent",
                ),
                parts[1],
            );
            return;
        }
        let mut text = format!(
            "From: {}\nDestination parent: {}\nFolder name: {}\nFinal path: {}\n\n",
            self.old.display(),
            self.input,
            self.name,
            target.display()
        );
        if let Some(plan) = &self.plan {
            let archived = plan.entries.iter().filter(|e| e.session.archived).count();
            text.push_str(&format!("{} sessions ({} archived), including subfolders.\nThis moves the folder and its files, and updates these sessions to their new locations.\nDetached idle sessions are released automatically after confirmation.\n\n",plan.entries.len(),archived));
            for entry in plan.entries.iter().filter(|_| !self.busy) {
                text.push_str(&format!(
                    "{}{} → {}\n",
                    entry.title,
                    if entry.session.archived {
                        " [archived]"
                    } else {
                        ""
                    },
                    entry.session.target.display()
                ));
            }
        }
        if let Some(message) = &self.message {
            text.push_str(message);
        }
        let controls = if self.busy {
            "Working… please wait."
        } else if self.plan.is_some() {
            "↑/↓ scroll · Enter confirms · Esc cancels"
        } else {
            "Edit folder name · Ctrl+U clear · Enter previews\nEsc returns to destination parent"
        };
        frame.render_widget(
            Paragraph::new(text.lines().map(safe_text).collect::<Vec<_>>().join("\n"))
                .wrap(Wrap { trim: false })
                .scroll((self.scroll, 0)),
            parts[0],
        );
        frame.render_widget(Paragraph::new(controls), parts[1]);
    }
}
