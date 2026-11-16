//! Shared presentation and keyboard primitives. Workflow actions stay with the caller.
use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::{
    Frame,
    layout::{Alignment, Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, Clear, Padding, Paragraph, Wrap},
};

pub struct Regions {
    pub body: Rect,
    pub status: Rect,
    pub actions: Rect,
    pub hints: Rect,
}
pub fn shell(frame: &mut Frame<'_>, title: &str, width: u16, height: u16) -> Rect {
    let area = frame.area();
    let margin_x = if area.width > width + 4 { 2 } else { 0 };
    let margin_y = if area.height > height + 2 { 1 } else { 0 };
    let width = width.min(area.width.saturating_sub(margin_x * 2));
    let height = height.min(area.height.saturating_sub(margin_y * 2));
    let popup = Rect::new(
        area.x + (area.width - width) / 2,
        area.y + (area.height - height) / 2,
        width,
        height,
    );
    frame.render_widget(Clear, popup);
    let padding = if width >= 60 && height >= 20 {
        Padding::new(2, 2, 1, 1)
    } else if width >= 24 {
        Padding::horizontal(1)
    } else {
        Padding::ZERO
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(Color::Cyan))
        .title(format!(" {} ", crate::browser::safe_text(title)))
        .padding(padding);
    let inner = block.inner(popup);
    frame.render_widget(block, popup);
    inner
}
pub fn regions(inner: Rect) -> Regions {
    let parts = Layout::vertical([
        Constraint::Min(1),
        Constraint::Length(2),
        Constraint::Length(1),
        Constraint::Length(1),
    ])
    .split(inner);
    Regions {
        body: parts[0],
        status: parts[1],
        actions: parts[2],
        hints: parts[3],
    }
}
#[derive(Clone, Copy)]
pub struct Button<A> {
    pub action: A,
    pub label: &'static str,
    pub enabled: bool,
    pub primary: bool,
}
pub fn buttons<A: Copy>(
    frame: &mut Frame<'_>,
    area: Rect,
    buttons: &[Button<A>],
    selected: Option<usize>,
) {
    let mut spans = vec![];
    for (i, b) in buttons.iter().enumerate() {
        if i > 0 {
            spans.push(Span::raw("  "));
        }
        let focused = selected == Some(i) && b.enabled;
        let style = if !b.enabled {
            Style::default().fg(Color::DarkGray)
        } else if focused {
            Style::default()
                .fg(Color::Black)
                .bg(Color::Cyan)
                .add_modifier(Modifier::BOLD)
        } else if b.primary {
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default()
        };
        spans.push(Span::styled(
            format!("{}[ {} ]", if focused { ">" } else { "" }, b.label),
            style,
        ));
    }
    let line = Line::from(spans);
    if line.width() > usize::from(area.width) {
        frame.render_widget(Paragraph::new("Enlarge terminal for actions"), area);
    } else {
        frame.render_widget(Paragraph::new(line).alignment(Alignment::Right), area);
    }
}
pub fn cycle(focus: &mut usize, enabled: &[bool], backwards: bool) {
    if enabled.is_empty() {
        return;
    }
    for _ in 0..enabled.len() {
        *focus = (*focus + if backwards { enabled.len() - 1 } else { 1 }) % enabled.len();
        if enabled[*focus] {
            break;
        }
    }
}
pub fn activate<A: Copy>(key: KeyEvent, buttons: &[Button<A>], selected: usize) -> Option<A> {
    if key.kind != KeyEventKind::Press || !matches!(key.code, KeyCode::Enter | KeyCode::Char(' ')) {
        return None;
    }
    buttons
        .get(selected)
        .filter(|b| b.enabled)
        .map(|b| b.action)
}
pub fn text_field(
    frame: &mut Frame<'_>,
    area: Rect,
    label: &str,
    value: &str,
    caret: usize,
    focused: bool,
    enabled: bool,
) {
    let style = Style::default().fg(if !enabled {
        Color::DarkGray
    } else if focused {
        Color::Cyan
    } else {
        Color::Gray
    });
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(if focused {
            BorderType::Thick
        } else {
            BorderType::Plain
        })
        .title(format!(" {label} "))
        .border_style(style);
    let inner = block.inner(area);
    let mut caret = caret.min(value.len());
    while !value.is_char_boundary(caret) {
        caret -= 1;
    }
    let mut start = 0;
    while unicode_width::UnicodeWidthStr::width(&value[start..caret])
        >= usize::from(inner.width).max(1)
        && start < caret
    {
        start += value[start..].chars().next().unwrap().len_utf8();
    }
    frame.render_widget(block, area);
    frame.render_widget(
        Paragraph::new(crate::browser::safe_text(&value[start..])),
        inner,
    );
    if focused && enabled && inner.width > 0 && inner.height > 0 {
        frame.set_cursor_position((
            inner.x + unicode_width::UnicodeWidthStr::width(&value[start..caret]) as u16,
            inner.y,
        ));
    }
}
pub fn edit(value: &mut String, caret: &mut usize, key: KeyEvent) -> bool {
    if key.kind == KeyEventKind::Release {
        return false;
    }
    *caret = (*caret).min(value.len());
    while !value.is_char_boundary(*caret) {
        *caret -= 1;
    }
    let previous = value[..*caret].char_indices().last().map_or(0, |(i, _)| i);
    let next = value[*caret..]
        .chars()
        .next()
        .map_or(*caret, |c| *caret + c.len_utf8());
    match key.code {
        KeyCode::Left => *caret = previous,
        KeyCode::Right => *caret = next,
        KeyCode::Home => *caret = 0,
        KeyCode::End => *caret = value.len(),
        KeyCode::Backspace if *caret > 0 => {
            value.replace_range(previous..*caret, "");
            *caret = previous;
            return true;
        }
        KeyCode::Delete if next > *caret => {
            value.replace_range(*caret..next, "");
            return true;
        }
        KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            value.clear();
            *caret = 0;
            return true;
        }
        KeyCode::Char(c)
            if !c.is_control()
                && !key
                    .modifiers
                    .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
        {
            value.insert(*caret, c);
            *caret += c.len_utf8();
            return true;
        }
        _ => {}
    }
    false
}
pub fn body(frame: &mut Frame<'_>, area: Rect, text: impl Into<String>, scroll: u16) -> u16 {
    let text = text
        .into()
        .split('\n')
        .map(crate::browser::safe_text)
        .collect::<Vec<_>>()
        .join("\n");
    let paragraph = Paragraph::new(text).wrap(Wrap { trim: false });
    let mut content = area;
    if paragraph.line_count(area.width) > usize::from(area.height) && area.width > 1 {
        content.width -= 1;
    }
    let max = paragraph
        .line_count(content.width)
        .saturating_sub(area.height as usize)
        .min(u16::MAX as usize) as u16;
    frame.render_widget(paragraph.scroll((scroll.min(max), 0)), content);
    if max > 0 && area.width > 0 && area.height > 0 {
        frame.render_widget(
            Paragraph::new(if scroll < max { "↓" } else { "↑" })
                .style(Style::default().fg(Color::Cyan)),
            Rect::new(area.right() - 1, area.bottom() - 1, 1, 1),
        );
    }
    max
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn layouts_keep_actions_inside_and_fields_have_visible_focus() {
        for (width, height) in [(120, 40), (80, 24), (60, 20), (40, 16), (12, 4), (1, 1)] {
            let mut terminal =
                ratatui::Terminal::new(ratatui::backend::TestBackend::new(width, height)).unwrap();
            terminal
                .draw(|frame| {
                    let inner = shell(frame, "Example", 80, 24);
                    let r = regions(inner);
                    for area in [r.body, r.status, r.actions, r.hints] {
                        assert!(area.right() <= width && area.bottom() <= height);
                    }
                    if r.body.height >= 3 {
                        text_field(
                            frame,
                            Rect::new(r.body.x, r.body.y, r.body.width, 3),
                            "Branch",
                            "feature/日本語",
                            17,
                            true,
                            true,
                        );
                    }
                    buttons(
                        frame,
                        r.actions,
                        &[
                            Button {
                                action: (),
                                label: "Cancel",
                                enabled: true,
                                primary: false,
                            },
                            Button {
                                action: (),
                                label: "Review",
                                enabled: true,
                                primary: true,
                            },
                        ],
                        Some(1),
                    );
                })
                .unwrap();
            if width >= 40 {
                let text = terminal
                    .backend()
                    .buffer()
                    .content
                    .iter()
                    .map(|cell| cell.symbol())
                    .collect::<String>();
                assert!(text.contains("[ Review ]"));
                assert!(text.contains("Branch"));
            }
        }
    }
    #[test]
    fn editing_is_unicode_safe_and_focus_skips_disabled_actions() {
        let mut value = "a界é".to_owned();
        let mut caret = value.len();
        assert!(edit(
            &mut value,
            &mut caret,
            KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE)
        ));
        assert_eq!(value, "a界");
        edit(
            &mut value,
            &mut caret,
            KeyEvent::new(KeyCode::Left, KeyModifiers::NONE),
        );
        assert_eq!(caret, 1);
        assert!(edit(
            &mut value,
            &mut caret,
            KeyEvent::new(KeyCode::Delete, KeyModifiers::NONE)
        ));
        assert_eq!(value, "a");
        let mut focus = 0;
        cycle(&mut focus, &[true, false, true], false);
        assert_eq!(focus, 2);
        cycle(&mut focus, &[true, false, true], true);
        assert_eq!(focus, 0);
        let b = [Button {
            action: 7,
            label: "Run",
            enabled: false,
            primary: true,
        }];
        assert_eq!(
            activate(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE), &b, 0),
            None
        );
        let b = [Button {
            enabled: true,
            ..b[0]
        }];
        assert_eq!(
            activate(KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE), &b, 0),
            Some(7)
        );
    }
}
