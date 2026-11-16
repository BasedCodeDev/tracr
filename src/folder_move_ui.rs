use crate::{
    browser::{Browser, FolderFilter, SourceSpec, safe_text},
    folder_move,
    tui::{Action, PickerText, render_picker},
};
use crossterm::event::{KeyCode, KeyEvent, KeyEventKind};
use ratatui::{
    Frame,
    style::Style,
    text::{Line, Span},
    widgets::Paragraph,
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
    if let FolderFilter::Path(path) = &browser.folder {
        if browser.repositories.values().any(|repo| {
            repo.worktrees.len() > 1 && repo.worktrees.iter().any(|w| w.path.starts_with(path))
        }) {
            return Some("Git-aware worktree moves are not supported yet.".into());
        }
    }
    match &browser.folder {
        FolderFilter::Path(p) if p.is_dir() => None,
        _ => Some("Select an existing folder in the Folders pane.".into()),
    }
}
pub struct Modal {
    pub old: PathBuf,
    pub input: String,
    pub picker: Option<crate::path_picker::Picker>,
    launch: PathBuf,
    pub plan: Option<folder_move::Plan>,
    pub recovery: bool,
    pub busy: bool,
    pub executing: bool,
    pub message: Option<String>,
    pub scroll: u16,
    action_back: bool,
    scroll_limit: std::cell::Cell<u16>,
    release_task: Option<tokio::task::JoinHandle<()>>,
    release_attempts: std::collections::BTreeMap<crate::domain::SessionKey, std::time::Instant>,
}
impl Drop for Modal {
    fn drop(&mut self) {
        if let Some(task) = self.release_task.take() {
            task.abort();
        }
    }
}
impl Modal {
    pub fn new(old: PathBuf, launch: PathBuf) -> Self {
        let mut input = old.parent().unwrap_or(Path::new("/")).display().to_string();
        if !input.ends_with('/') {
            input.push('/');
        }
        let picker = crate::path_picker::Picker::for_move(launch.clone(), old.clone(), &input);
        Self {
            input,
            old,
            picker: Some(picker),
            launch,
            plan: None,
            recovery: false,
            busy: false,
            executing: false,
            message: None,
            scroll: 0,
            action_back: false,
            scroll_limit: std::cell::Cell::new(0),
            release_task: None,
            release_attempts: Default::default(),
        }
    }
    pub fn recover(j: folder_move::Journal) -> Self {
        Self {
            old: j.plan.old.clone(),
            input: j.plan.target.display().to_string(),
            picker: None,
            launch: PathBuf::from("/"),
            plan: Some(j.plan),
            recovery: true,
            busy: false,
            executing: false,
            message: Some(format!(
                "Recorded phase: {}\n{}\nEnter attempts guarded completion. u cycles through unfinished operations.",
                j.phase,
                j.error.unwrap_or_default()
            )),
            scroll: 0,
            action_back: false,
            scroll_limit: std::cell::Cell::new(0),
            release_task: None,
            release_attempts: Default::default(),
        }
    }
    pub async fn stop_release(&mut self) {
        if let Some(task) = self.release_task.take() {
            task.abort();
            let _ = task.await;
        }
    }
    pub fn monitor(&mut self, browser: &Browser) {
        if self
            .release_task
            .as_ref()
            .is_some_and(|task| task.is_finished())
        {
            self.release_task = None;
        }
        if self.busy || self.recovery || self.plan.is_some() || self.release_task.is_some() {
            return;
        }
        let now = std::time::Instant::now();
        let candidate = browser
            .all()
            .into_iter()
            .filter(|s| {
                !s.archived
                    && s.cwd.as_ref().is_some_and(|p| p.starts_with(&self.old))
                    && browser.runtime_status(&s.key) == crate::domain::RuntimeStatus::Idle
            })
            .filter(|s| {
                self.release_attempts.get(&s.key).is_none_or(|last| {
                    now.duration_since(*last) >= std::time::Duration::from_secs(5)
                })
            })
            .min_by_key(|s| self.release_attempts.get(&s.key).copied());
        let Some(session) = candidate else {
            return;
        };
        let Some((binary, source)) = browser.specs.iter().find_map(|spec| match spec {
            SourceSpec::Codex { binary, source } if source.id == session.key.source_id => {
                Some((binary.clone(), source.clone()))
            }
            _ => None,
        }) else {
            return;
        };
        self.release_attempts.insert(session.key.clone(), now);
        self.release_task = Some(tokio::spawn(async move {
            let _ = crate::providers::codex_release::release_idle(&binary, &source, &session).await;
            // Verify with a fresh passive snapshot; the normal live feed remains the
            // display authority, so older dialog results cannot overwrite newer state.
            if let Ok(mut client) =
                crate::providers::codex_live::LiveClient::connect(&binary, source).await
            {
                let _ = client.snapshot().await;
            }
        }));
    }
    pub fn target(&self) -> Result<PathBuf, String> {
        self.plan
            .as_ref()
            .map(|p| Ok(p.target.clone()))
            .unwrap_or_else(|| {
                folder_move::resolve_destination(&self.input, &self.old, &self.launch)
            })
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
        if self.plan.is_none()
            && self.picker.as_ref().is_some_and(|p| p.focus == 3)
            && matches!(key.code, KeyCode::Enter | KeyCode::Char(' '))
        {
            return Action::CloseFolderMove;
        }
        if self.plan.is_some() && matches!(key.code, KeyCode::Tab | KeyCode::BackTab) {
            self.action_back = !self.action_back;
            return Action::Continue;
        }
        let back = self.action_back && matches!(key.code, KeyCode::Enter | KeyCode::Char(' '));
        if key.code == KeyCode::Char('u') && self.recovery {
            return Action::RecoverRelink;
        }
        if key.code == KeyCode::Esc || back {
            if self.plan.is_some() && !self.recovery {
                self.plan = None;
                self.action_back = false;
                self.message = None;
                self.scroll = 0;
            } else {
                return Action::CloseFolderMove;
            }
        } else if self.plan.is_some() {
            if matches!(key.code, KeyCode::Enter | KeyCode::Char(' ')) {
                return Action::ApplyFolderMove;
            }
            match key.code {
                KeyCode::Down => self.scroll = self.scroll.saturating_add(1),
                KeyCode::Up => self.scroll = self.scroll.saturating_sub(1),
                _ => {}
            }
        } else if key.code == KeyCode::PageDown {
            self.scroll = self
                .scroll
                .min(self.scroll_limit.get())
                .saturating_add(5)
                .min(self.scroll_limit.get());
        } else if key.code == KeyCode::PageUp {
            self.scroll = self.scroll.min(self.scroll_limit.get()).saturating_sub(5);
        } else if let Some(picker) = self.picker.as_mut() {
            if picker.key(&mut self.input, key) {
                self.message = None;
                return Action::PreviewFolderMove;
            }
        }
        Action::Continue
    }
    pub fn paste(&mut self, text: &str) {
        if self.busy || self.plan.is_some() {
            return;
        }
        if let Some(p) = self.picker.as_mut() {
            p.paste(&mut self.input, text);
        }
    }
    pub fn tick(&mut self) {
        if !self.busy && self.plan.is_none() {
            if let Some(p) = self.picker.as_mut() {
                p.tick(&mut self.input);
            }
        }
    }
    pub fn draw(&self, frame: &mut Frame<'_>, browser: &Browser) {
        if self.plan.is_none() && !self.busy {
            if let Some(p) = &self.picker {
                let heading = format!(
                    "Final path: {}",
                    self.target()
                        .map(|p| p.display().to_string())
                        .unwrap_or_else(|_| "Enter a valid destination".into())
                );
                let inner_width = frame
                    .area()
                    .width
                    .saturating_sub(2)
                    .min(120)
                    .saturating_sub(2);
                let column_width = if inner_width >= 90 {
                    inner_width * 45 / 100
                } else {
                    inner_width
                };
                let readiness = move_readiness(
                    browser,
                    &self.old,
                    column_width.saturating_sub(2),
                    self.release_task.is_some(),
                );
                render_picker(
                    frame,
                    p,
                    &self.input,
                    PickerText {
                        title: "Move folder",
                        heading: &heading,
                        old: &self.old.display().to_string(),
                        field: "Destination folder",
                        busy: false,
                        error: self.message.as_deref(),
                        sidebar: Some((&readiness, self.scroll, &self.scroll_limit)),
                    },
                );
                return;
            }
        }
        let inner = crate::modal::shell(
            frame,
            if self.recovery {
                "Inspect/recover folder move"
            } else {
                "Move folder"
            },
            90,
            30,
        );
        let regions = crate::modal::regions(inner);
        let target = self
            .target()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|e| e);
        let mut text = format!(
            "Previous folder: {}\nFinal path: {}\n\n",
            self.old.display(),
            target
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
            "↑/↓ scroll · Enter confirms · Esc returns"
        } else {
            "Enter previews · Esc cancels"
        };
        self.scroll_limit
            .set(crate::modal::body(frame, regions.body, text, self.scroll));
        frame.render_widget(Paragraph::new(controls), regions.status);
        use crate::modal::Button;
        crate::modal::buttons(
            frame,
            regions.actions,
            &[
                Button {
                    action: 0,
                    label: if self.recovery { "Close" } else { "Back" },
                    enabled: !self.executing,
                    primary: false,
                },
                Button {
                    action: 1,
                    label: if self.recovery {
                        "Continue recovery"
                    } else {
                        "Move folder"
                    },
                    enabled: !self.busy,
                    primary: true,
                },
            ],
            Some(usize::from(!self.action_back)),
        );
        frame.render_widget(
            Paragraph::new("Tab buttons · Enter activate · Esc back"),
            regions.hints,
        );
    }
}

/// Advisory only: native preflight and exclusive ownership remain authoritative.
fn move_readiness(browser: &Browser, old: &Path, width: u16, checking: bool) -> Vec<Line<'static>> {
    use crate::domain::RuntimeStatus;
    use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};
    let mut sessions: Vec<_> = browser
        .all()
        .into_iter()
        .filter(|s| s.cwd.as_ref().is_some_and(|path| path.starts_with(old)))
        .filter_map(|s| {
            let status = browser.runtime_status(&s.key);
            let priority = match status {
                RuntimeStatus::Running | RuntimeStatus::Waiting { .. } | RuntimeStatus::Error => 0,
                RuntimeStatus::Idle => 1,
                RuntimeStatus::Unknown => 2,
                RuntimeStatus::Unloaded => return None,
            };
            Some((priority, s, status))
        })
        .collect();
    sessions.sort_by(|a, b| {
        a.0.cmp(&b.0)
            .then_with(|| a.1.title.cmp(&b.1.title))
            .then_with(|| a.1.key.cmp(&b.1.key))
    });
    let message = if browser.diagnostics().iter().any(|d| !d.complete) {
        "Inventory incomplete"
    } else if sessions.iter().any(|s| s.2 == RuntimeStatus::Unknown) {
        "Status unavailable — retrying…"
    } else if checking {
        "Checking idle sessions…"
    } else if sessions.is_empty() {
        "No loaded sessions."
    } else {
        "Close sessions still open in Codex."
    };
    let status_width = sessions
        .iter()
        .map(|s| s.2.label().unwrap_or("Unknown").width())
        .max()
        .unwrap_or(6)
        .max(6);
    let stacked = usize::from(width) < status_width + 16;
    let title_width = usize::from(width).saturating_sub(status_width + 2);
    let mut lines = vec![];
    for (_, session, status) in sessions {
        let title = format!(
            "{}{}",
            safe_text(&session.title),
            if session.archived { " [archived]" } else { "" }
        );
        let style =
            crate::tui::activity_color(status).map_or(Style::default(), |c| Style::default().fg(c));
        let label = status.label().unwrap_or("Unknown").to_owned();
        if stacked {
            lines.push(Line::from(title));
            lines.push(Line::styled(label, style));
            lines.push(Line::default());
        } else {
            let mut clipped = String::new();
            let mut used = 0;
            for ch in title.chars() {
                let size = ch.width().unwrap_or(0);
                if used + size > title_width.saturating_sub(1) {
                    break;
                }
                clipped.push(ch);
                used += size;
            }
            if title.width() > title_width {
                clipped.push('…');
                used += 1;
            } else {
                clipped = title;
                used = clipped.width();
            }
            lines.push(Line::from(vec![
                Span::raw(clipped),
                Span::raw(" ".repeat(title_width.saturating_sub(used) + 2)),
                Span::styled(label, style),
            ]));
        }
    }
    lines.push(Line::default());
    lines.push(Line::from(message));
    lines
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{browser::Update, domain::*};
    use ratatui::{Terminal, backend::TestBackend};

    #[test]
    fn readiness_includes_descendants_and_archives_and_updates_live() {
        let root = tempfile::tempdir().unwrap();
        let old = root.path().join("project");
        std::fs::create_dir(&old).unwrap();
        let session: SessionSummary = serde_json::from_value(serde_json::json!({
            "key":{"provider":"codex","source_id":"test","native_id":"idle"},
            "title":"Idle agent", "cwd":old.join("child"), "created_at":1,"updated_at":1,"archived":false,"source_kind":null,"git":null
        })).unwrap();
        let mut active = session.clone();
        active.key.native_id = "active".into();
        active.title = "Active agent".into();
        active.archived = true;
        let mut unrelated = session.clone();
        unrelated.key.native_id = "outside".into();
        unrelated.title = "Outside agent".into();
        unrelated.cwd = Some(root.path().join("project-other"));
        let mut browser = Browser::new(vec![SourceSpec::Fixture(root.path().join("fixture"))]);
        browser.begin_refresh();
        browser.apply(Update::Page {
            generation: browser.generation,
            source: 0,
            sessions: vec![session.clone(), active.clone(), unrelated.clone()],
            warnings: vec![],
        });
        browser.apply(Update::Finished {
            generation: browser.generation,
            source: 0,
            error: None,
        });
        for (key, status) in [
            (session.key.clone(), RuntimeStatus::Idle),
            (active.key.clone(), RuntimeStatus::Running),
            (unrelated.key.clone(), RuntimeStatus::Running),
        ] {
            browser.sources[0].live.insert(key, status);
        }
        let text = |browser: &Browser| {
            move_readiness(browser, &old, 80, false)
                .iter()
                .map(|line| line.to_string())
                .collect::<Vec<_>>()
                .join("\n")
        };
        let report = text(&browser);
        assert!(report.contains("Active agent [archived]"));
        assert!(report.contains("Idle"));
        assert!(!report.contains("Outside agent"));
        assert!(report.find("Active agent").unwrap() < report.find("Idle agent").unwrap());
        let mut modal = Modal::new(old.clone(), root.path().into());
        modal.scroll_limit.set(20);
        let input = modal.input.clone();
        assert_eq!(
            modal.key(KeyEvent::new(
                KeyCode::PageDown,
                crossterm::event::KeyModifiers::NONE
            )),
            Action::Continue
        );
        assert_eq!(modal.scroll, 5);
        assert_eq!(modal.input, input);
        assert!(modal.plan.is_none());
        modal.key(KeyEvent::new(
            KeyCode::PageUp,
            crossterm::event::KeyModifiers::NONE,
        ));
        assert_eq!(modal.scroll, 0);
        for width in [140, 80, 36] {
            let mut terminal = Terminal::new(TestBackend::new(width, 40)).unwrap();
            terminal.draw(|f| modal.draw(f, &browser)).unwrap();
            let buffer = terminal.backend().buffer();
            let rendered = buffer
                .content
                .iter()
                .map(|c| c.symbol())
                .collect::<String>();
            assert!(rendered.contains("Sessions"));
            assert!(rendered.contains("Folders"));
            assert!(
                rendered.find("Previous folder").unwrap() < rendered.find("Final path").unwrap()
            );
        }
        browser.sources[0]
            .live
            .insert(active.key.clone(), RuntimeStatus::Unloaded);
        assert!(!text(&browser).contains("Active agent"));
        browser.sources[0]
            .live
            .insert(session.key.clone(), RuntimeStatus::Unloaded);
        assert!(text(&browser).contains("No loaded sessions."));
        browser.sources[0]
            .live
            .insert(session.key, RuntimeStatus::Unknown);
        assert!(text(&browser).contains("Status unavailable"));
    }
}
