//! Shared visual scrollbars. Metrics are rendered lines, never selected-item indices.
use ratatui::{
    Frame,
    layout::Rect,
    style::{Color, Style},
    widgets::{ListItem, Paragraph, Scrollbar, ScrollbarState},
};

/// A border's right edge, restricted to the independently scrolling body.
pub(crate) fn border_track(border: Rect, body: Rect) -> Rect {
    Rect::new(
        border.right().saturating_sub(1),
        body.y,
        u16::from(border.width > 0),
        body.height,
    )
}

/// Reserve a column only when an unbordered body overflows. Callers that wrap
/// text must recalculate its line count at the returned width.
pub(crate) fn content_area(area: Rect, total: usize) -> Rect {
    Rect {
        width: area.width.saturating_sub(u16::from(
            area.width > 1 && total > usize::from(area.height),
        )),
        ..area
    }
}

pub(crate) fn draw(frame: &mut Frame<'_>, track: Rect, total: usize, visible: usize, top: usize) {
    if track.is_empty() || visible == 0 || total <= visible {
        return;
    }
    let max = total - visible;
    frame.render_stateful_widget(
        Scrollbar::default()
            .begin_symbol(None)
            .end_symbol(None)
            .track_symbol(Some("│"))
            .track_style(Style::default().fg(Color::DarkGray))
            .thumb_symbol("▐")
            .thumb_style(Style::default().fg(Color::Cyan)),
        track,
        // Ratatui expects the number of possible viewport positions, not rows.
        &mut ScrollbarState::new(max.saturating_add(1))
            .position(top.min(max))
            .viewport_content_length(visible),
    );
}

/// Render a wrapped or unwrapped paragraph with a private scrollbar column.
/// Return the clamped maximum for the existing u16 keyboard scroll state.
pub(crate) fn paragraph(frame: &mut Frame<'_>, area: Rect, text: Paragraph<'_>, top: u16) -> u16 {
    let full_total = text.line_count(area.width);
    let body = content_area(area, full_total);
    let total = if body.width == area.width {
        full_total
    } else {
        text.line_count(body.width)
    };
    let max = total
        .saturating_sub(usize::from(body.height))
        .min(u16::MAX as usize) as u16;
    let top = top.min(max);
    frame.render_widget(text.scroll((top, 0)), body);
    if body.width < area.width {
        draw(
            frame,
            border_track(area, body),
            total,
            usize::from(body.height),
            usize::from(top),
        );
    }
    max
}

/// Line offsets for variable-height lists, built alongside their rendered items.
pub(crate) struct ListLines(Vec<usize>);
impl ListLines {
    pub(crate) fn new(items: &[ListItem<'_>]) -> Self {
        let mut offsets = Vec::with_capacity(items.len() + 1);
        offsets.push(0);
        for item in items {
            offsets.push(offsets.last().unwrap() + item.height());
        }
        Self(offsets)
    }

    pub(crate) fn total(&self) -> usize {
        *self.0.last().unwrap()
    }

    pub(crate) fn top(&self, item: usize) -> usize {
        self.0[item.min(self.0.len() - 1)]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::{Terminal, backend::TestBackend, text::Line, widgets::Wrap};

    #[test]
    #[ignore = "manual large list and wrapped dialog rendering cost"]
    fn large_widget_render_cost() {
        use ratatui::widgets::{List, ListState};
        let rows: Vec<_> = (0..10_000)
            .map(|i| format!("Entry {i}\nmetadata 界界"))
            .collect();
        let text = "A long Unicode paragraph 界界 with enough words to wrap across several lines. "
            .repeat(1_000);
        let mut terminal = Terminal::new(TestBackend::new(160, 40)).unwrap();
        for sample in 0..6 {
            let start = std::time::Instant::now();
            for step in 0..100 {
                terminal
                    .draw(|frame| {
                        let items: Vec<_> =
                            rows.iter().map(|row| ListItem::new(row.as_str())).collect();
                        let lines = ListLines::new(&items);
                        let area = Rect::new(0, 0, 80, 40);
                        let body = content_area(area, lines.total());
                        let mut state = ListState::default().with_selected(Some(5_000 + step));
                        frame.render_stateful_widget(List::new(items), body, &mut state);
                        draw(
                            frame,
                            border_track(area, body),
                            lines.total(),
                            40,
                            lines.top(state.offset()),
                        );
                        paragraph(
                            frame,
                            Rect::new(80, 0, 80, 40),
                            Paragraph::new(text.as_str()).wrap(Wrap { trim: false }),
                            step as u16,
                        );
                    })
                    .unwrap();
            }
            eprintln!(
                "large widgets sample {sample}: {:?} per 100 frames",
                start.elapsed()
            );
        }
    }

    #[test]
    fn thumb_reaches_both_ends_and_disappears_when_content_shrinks() {
        let mut terminal = Terminal::new(TestBackend::new(8, 8)).unwrap();
        for height in [1, 2, 6] {
            let track = Rect::new(6, 1, 1, height);
            for (total, visible, top) in [
                (100, 10, 0),
                (100, 10, 45),
                (100, 10, 900),
                (4, 10, 0),
                (0, 0, 0),
            ] {
                terminal
                    .draw(|frame| draw(frame, track, total, visible, top))
                    .unwrap();
                let buffer = terminal.backend().buffer();
                let thumb: Vec<_> = (1..1 + height)
                    .filter(|y| buffer[(6, *y)].fg == Color::Cyan)
                    .collect();
                if total > visible {
                    assert!(!thumb.is_empty());
                    assert!(matches!(buffer[(6, 1)].symbol(), "│" | "▐"));
                    for y in &thumb {
                        assert_eq!(buffer[(6, *y)].symbol(), "▐");
                    }
                    if top == 0 {
                        assert_eq!(thumb[0], 1);
                    }
                    if top >= total - visible {
                        assert_eq!(*thumb.last().unwrap(), height);
                    }
                } else {
                    assert!(thumb.is_empty());
                }
                assert_eq!(buffer[(6, 0)].symbol(), " ");
                assert_eq!(buffer[(6, 7)].symbol(), " ");
                assert_eq!(buffer[(5, 1)].symbol(), " ");
            }
        }
    }

    #[test]
    fn paragraphs_rewrap_in_the_reserved_column_and_clamp_after_resize() {
        let mut terminal = Terminal::new(TestBackend::new(20, 8)).unwrap();
        let text = "界界界界界界\nabc def ghi jkl\nlast line";
        for width in [12, 6, 2, 1, 0] {
            let area = Rect::new(0, 0, width, 3);
            let mut max = 0;
            terminal
                .draw(|frame| {
                    max = paragraph(
                        frame,
                        area,
                        Paragraph::new(text).wrap(Wrap { trim: false }),
                        u16::MAX,
                    );
                })
                .unwrap();
            let original = Paragraph::new(text).wrap(Wrap { trim: false });
            let body = content_area(area, original.line_count(width));
            assert_eq!(
                usize::from(max),
                original.line_count(body.width).saturating_sub(3)
            );
            if body.width < width {
                assert_eq!(terminal.backend().buffer()[(width - 1, 2)].fg, Color::Cyan);
            }
        }
    }

    #[test]
    fn variable_list_metrics_use_lines_including_headers_and_blank_rows() {
        let items = vec![
            ListItem::new("Heading"),
            ListItem::new(vec![Line::from("one"), Line::from("two"), Line::default()]),
            ListItem::new("last"),
        ];
        let lines = ListLines::new(&items);
        assert_eq!(lines.total(), 5);
        assert_eq!(lines.top(0), 0);
        assert_eq!(lines.top(1), 1);
        assert_eq!(lines.top(2), 4);
        assert_eq!(lines.top(99), 5);
    }
}
