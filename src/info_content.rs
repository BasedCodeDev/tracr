//! Structured session information: one source for the table and clipboard text.
use crate::browser::safe_text;
use ratatui::{
    Frame,
    layout::{Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Paragraph, Wrap},
};
use unicode_width::UnicodeWidthStr;

pub struct Row {
    pub label: &'static str,
    pub value: String,
    pub color: Option<Color>,
    pub note: Option<String>,
}
impl Row {
    pub fn new(label: &'static str, value: impl Into<String>) -> Self {
        Self {
            label,
            value: value.into(),
            color: None,
            note: None,
        }
    }
}
pub struct Section {
    pub title: &'static str,
    pub rows: Vec<Row>,
}
pub struct Information {
    pub title: String,
    pub notices: Vec<String>,
    pub sections: Vec<Section>,
}
impl Information {
    pub fn report(&self) -> String {
        let mut lines = vec![self.title.clone()];
        for notice in &self.notices {
            lines.push(String::new());
            lines.push(notice.clone());
        }
        for section in &self.sections {
            lines.push(String::new());
            lines.push(section.title.into());
            for row in &section.rows {
                lines.push(format!("{}: {}", row.label, row.value));
                if let Some(note) = &row.note {
                    lines.push(note.clone());
                }
            }
        }
        lines.join("\n")
    }
    fn lines(&self, width: usize, stacked: bool) -> Vec<Line<'static>> {
        let mut lines = Vec::new();
        let muted = Style::default().add_modifier(Modifier::DIM);
        for notice in &self.notices {
            for line in wrap(notice, width) {
                lines.push(Line::styled(line, Style::default().fg(Color::Yellow)));
            }
            lines.push(Line::default());
        }
        for section in &self.sections {
            lines.push(Line::styled(
                section.title,
                Style::default().add_modifier(Modifier::BOLD),
            ));
            for row in &section.rows {
                let style = row
                    .color
                    .map_or(Style::default(), |color| Style::default().fg(color));
                if stacked {
                    for line in wrap(row.label, width) {
                        lines.push(Line::styled(line, muted));
                    }
                    for line in wrap(&row.value, width) {
                        lines.push(Line::styled(line, style));
                    }
                } else {
                    for (i, line) in wrap(&row.value, width.saturating_sub(16))
                        .into_iter()
                        .enumerate()
                    {
                        let label = if i == 0 {
                            format!("{:<16}", row.label)
                        } else {
                            " ".repeat(16)
                        };
                        lines.push(Line::from(vec![
                            Span::styled(label, muted),
                            Span::styled(line, style),
                        ]));
                    }
                }
                if let Some(note) = &row.note {
                    let indent = if stacked { 0 } else { 16 };
                    for line in wrap(note, width.saturating_sub(indent)) {
                        lines.push(Line::styled(format!("{}{line}", " ".repeat(indent)), muted));
                    }
                }
                if stacked {
                    lines.push(Line::default());
                }
            }
            lines.push(Line::default());
        }
        lines
    }
    pub fn draw(
        &self,
        frame: &mut Frame<'_>,
        inner: Rect,
        scroll: u16,
        _feedback: Option<&str>,
    ) -> u16 {
        let title = Paragraph::new(safe_text(&self.title))
            .style(Style::default().add_modifier(Modifier::BOLD))
            .wrap(Wrap { trim: false });
        let title_height = title.line_count(inner.width).min(3) as u16;
        let parts = Layout::vertical([Constraint::Length(title_height + 1), Constraint::Min(1)])
            .split(inner);
        frame.render_widget(title, parts[0]);
        let build_lines = |width: usize| {
            let mut lines = self.lines(width, inner.width + 2 < 50);
            // Extremely long titles remain available in full in the scrolling content.
            if wrap(&self.title, width).len() > 3 {
                let mut full = wrap(&self.title, width)
                    .into_iter()
                    .map(Line::from)
                    .collect::<Vec<_>>();
                full.push(Line::default());
                full.append(&mut lines);
                lines = full;
            }
            lines
        };
        let mut lines = build_lines(usize::from(parts[1].width));
        let body = crate::scrollbar::content_area(parts[1], lines.len());
        if body.width < parts[1].width {
            lines = build_lines(usize::from(body.width));
        }
        let total = lines.len();
        let max = lines
            .len()
            .saturating_sub(parts[1].height as usize)
            .min(u16::MAX as usize) as u16;
        frame.render_widget(Paragraph::new(lines).scroll((scroll.min(max), 0)), body);
        if body.width < parts[1].width {
            crate::scrollbar::draw(
                frame,
                crate::scrollbar::border_track(parts[1], body),
                total,
                usize::from(body.height),
                usize::from(scroll.min(max)),
            );
        }
        max
    }
}

/// Word-wrap normal text, but split long paths/IDs without losing any characters.
pub(crate) fn wrap(text: &str, width: usize) -> Vec<String> {
    if width == 0 {
        return vec![];
    }
    let mut lines = vec![];
    for original in text.split('\n') {
        let clean = safe_text(original);
        let mut rest = clean.as_str();
        while rest.width() > width {
            let mut end = 0;
            let mut space = None;
            for (index, ch) in rest.char_indices() {
                let next = index + ch.len_utf8();
                if rest[..next].width() > width {
                    break;
                }
                end = next;
                if ch.is_whitespace() && index > 0 {
                    space = Some(index);
                }
            }
            if end == 0 {
                end = rest.chars().next().unwrap().len_utf8();
            }
            let split = space.unwrap_or(end);
            lines.push(rest[..split].to_owned());
            rest = &rest[split..];
            if space.is_some() {
                rest = rest.trim_start();
            }
        }
        lines.push(rest.to_owned());
    }
    lines
}

pub fn local_date(seconds: i64) -> String {
    if seconds == 0 {
        return "Not recorded".into();
    }
    let time = seconds as libc::time_t;
    let mut local = std::mem::MaybeUninit::<libc::tm>::uninit();
    // localtime_r owns its output and strftime writes into a bounded buffer.
    unsafe {
        if libc::localtime_r(&time, local.as_mut_ptr()).is_null() {
            return "Unknown".into();
        }
        let mut output = [0u8; 128];
        let length = libc::strftime(
            output.as_mut_ptr().cast(),
            output.len(),
            c"%d %b %Y, %H:%M %Z (%z)".as_ptr(),
            local.as_ptr(),
        );
        if length == 0 {
            "Unknown".into()
        } else {
            String::from_utf8_lossy(&output[..length]).into_owned()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn example() -> Information {
        Information {
            title: "A session with a readable title".into(),
            notices: vec!["Notice about this session".into()],
            sections: vec![Section {
                title: "Location",
                rows: vec![
                    Row::new(
                        "Folder",
                        "/home/用户/a-long-folder-with-no-spaces/another-long-folder",
                    ),
                    Row::new("Repository", "Not recorded"),
                ],
            }],
        }
    }
    #[test]
    fn unicode_paths_wrap_without_losing_characters() {
        for text in [
            "/home/用户/長いパス/abcdefghi",
            "e\u{301}e\u{301}e\u{301}/👩‍💻/folder",
            "1234567890123456789012345678901234567890",
        ] {
            for width in [8, 16, 34, 60] {
                let lines = wrap(text, width);
                assert_eq!(lines.concat(), text);
                assert!(lines.iter().all(|line| line.width() <= width), "{lines:?}");
            }
        }
    }
    #[test]
    fn readable_dates_include_timezone_and_handle_missing_or_invalid_values() {
        assert_eq!(local_date(0), "Not recorded");
        assert_eq!(local_date(i64::MAX), "Unknown");
        let date = local_date(1_700_000_000);
        assert!(date.contains("2023"), "{date}");
        assert!(date.contains('(') && date.ends_with(')'), "{date}");
        assert!(date.contains('+') || date.contains('-'), "{date}");
    }
    #[test]
    fn table_and_stacked_layouts_preserve_values_and_report_has_no_decoration() {
        let info = example();
        for (width, stacked) in [(78, false), (48, true), (22, true)] {
            let lines = info.lines(width, stacked);
            assert!(lines.iter().all(|line| line.width() <= width));
            let rendered = lines
                .iter()
                .map(|line| {
                    line.spans
                        .iter()
                        .map(|span| span.content.as_ref())
                        .collect::<String>()
                })
                .collect::<Vec<_>>();
            assert!(rendered.iter().any(|line| line.contains("Folder")));
            if !stacked {
                assert!(
                    rendered
                        .iter()
                        .any(|line| line.starts_with("Folder          /home/"))
                );
            }
        }
        let report = info.report();
        assert!(
            report.contains("Folder: /home/用户/a-long-folder-with-no-spaces/another-long-folder")
        );
        assert!(!report.contains('┃'));
        assert!(!report.contains("Some("));
    }
    #[test]
    fn content_keeps_heading_and_leaves_actions_to_dialog() {
        let info = example();
        let mut terminal =
            ratatui::Terminal::new(ratatui::backend::TestBackend::new(36, 12)).unwrap();
        terminal
            .draw(|frame| {
                info.draw(frame, frame.area(), u16::MAX, Some("Message copied"));
            })
            .unwrap();
        let text: String = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect();
        assert!(text.contains("A session with a readable title"));
        assert!(!text.contains("Message copied"));
        assert!(!text.contains("c Copy"));
    }
}
