//! Selection coordinates refer to retained history, not the moving viewport.
use std::time::Instant;
use unicode_segmentation::UnicodeSegmentation;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Point(pub u64, pub u16);
#[derive(Clone)]
pub struct Selection {
    pub anchor: Point,
    pub head: Point,
    pub dragging: bool,
    pub unit: u8,
    pub edge: i8,
    pub tick: Instant,
    pub alternate: bool,
    pub text: String,
}
impl Selection {
    pub fn new(screen: &vt100::Screen, point: Point, unit: u8) -> Self {
        let mut selection = Self {
            anchor: point,
            head: point,
            dragging: true,
            unit,
            edge: 0,
            tick: Instant::now(),
            alternate: screen.alternate_screen(),
            text: String::new(),
        };
        selection.update(screen, point);
        selection
    }
    fn span(&self, screen: &vt100::Screen, point: Point) -> (Point, Point) {
        let cols = screen.size().1;
        let mut point = point;
        if screen
            .history_cell(point.0, point.1)
            .is_some_and(|c| c.is_wide_continuation())
        {
            point.1 = point.1.saturating_sub(1);
        }
        let width = if screen
            .history_cell(point.0, point.1)
            .is_some_and(|c| c.is_wide())
        {
            2
        } else {
            1
        };
        let mut first = point.0;
        let mut last = point.0;
        if self.unit >= 2 {
            while first > screen.history_start() && screen.history_wrapped(first - 1) {
                first -= 1;
            }
            while last + 1 < screen.history_end() && screen.history_wrapped(last) {
                last += 1;
            }
        }
        if self.unit == 3 {
            return (Point(first, 0), Point(last, cols));
        }
        if self.unit == 2 {
            let mut text = String::new();
            let mut cells = Vec::new();
            for line in first..=last {
                for col in 0..cols {
                    if let Some(cell) = screen.history_cell(line, col) {
                        if cell.is_wide_continuation() {
                            continue;
                        }
                        let value = cell.contents();
                        let start = text.len();
                        text.push_str(if value.is_empty() { " " } else { &value });
                        cells.push((
                            start,
                            text.len(),
                            Point(line, col),
                            Point(line, col + if cell.is_wide() { 2 } else { 1 }),
                        ));
                    }
                }
            }
            if let Some((byte, _, _, _)) = cells
                .iter()
                .find(|(_, _, col, end)| point >= *col && point < *end)
            {
                for (start, word) in text.split_word_bound_indices() {
                    let end = start + word.len();
                    if *byte >= start && *byte < end {
                        let left = cells.iter().find(|(_, b, _, _)| *b > start).unwrap().2;
                        let right = cells.iter().rev().find(|(a, _, _, _)| *a < end).unwrap().3;
                        return (left, right);
                    }
                }
            }
        }
        (point, Point(point.0, (point.1 + width).min(cols)))
    }
    pub fn bounds(&self, screen: &vt100::Screen) -> (Point, Point) {
        let a = self.span(screen, self.anchor);
        let b = self.span(screen, self.head);
        (a.0.min(b.0), a.1.max(b.1))
    }
    #[cfg(test)]
    pub fn contains(&self, screen: &vt100::Screen, point: Point) -> bool {
        let (start, end) = self.bounds(screen);
        point >= start && point < end
    }
    pub fn valid(&self, screen: &vt100::Screen) -> bool {
        self.alternate == screen.alternate_screen()
            && self.anchor.0.min(self.head.0) >= screen.history_start()
            && self.anchor.0.max(self.head.0) < screen.history_end()
    }
    pub fn update(&mut self, screen: &vt100::Screen, point: Point) {
        self.head = point;
        let (start, end) = self.bounds(screen);
        let mut text = String::new();
        for line in start.0..=end.0 {
            let mut row = String::new();
            let left = if line == start.0 { start.1 } else { 0 };
            let right = if line == end.0 {
                end.1
            } else {
                screen.size().1
            };
            for col in left..right {
                if let Some(cell) = screen.history_cell(line, col) {
                    if cell.is_wide_continuation() {
                        continue;
                    }
                    let value = cell.contents();
                    row.push_str(if value.is_empty() { " " } else { &value });
                }
            }
            text.push_str(if screen.history_wrapped(line) {
                &row
            } else {
                row.trim_end_matches(' ')
            });
            if line < end.0 && !screen.history_wrapped(line) {
                text.push('\n');
            }
        }
        self.text = text;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn unicode_and_soft_wrap_selection_copy_plain_text() {
        let mut parser = vt100::Parser::new(3, 6, 20);
        parser.process("a界e\u{301}bcXYZ".as_bytes());
        let mut selection = Selection::new(parser.screen(), Point(0, 0), 1);
        selection.update(parser.screen(), Point(1, 2));
        assert_eq!(selection.text, "a界e\u{301}bcXYZ");
        let wide = Selection::new(parser.screen(), Point(0, 2), 1);
        assert_eq!(wide.text, "界");
        let logical = Selection::new(parser.screen(), Point(1, 1), 3);
        assert_eq!(logical.text, "a界e\u{301}bcXYZ");
    }
    #[test]
    fn word_selection_crosses_wraps_and_keeps_wrap_spaces() {
        let mut parser = vt100::Parser::new(3, 4, 20);
        parser.process(b"abcdefgh");
        assert_eq!(
            Selection::new(parser.screen(), Point(1, 1), 2).text,
            "abcdefgh"
        );
        let mut parser = vt100::Parser::new(3, 4, 20);
        parser.process(b"foo bar");
        assert_eq!(
            Selection::new(parser.screen(), Point(0, 1), 3).text,
            "foo bar"
        );
    }
    #[test]
    fn history_selection_survives_scrolling_until_eviction() {
        let mut parser = vt100::Parser::new(2, 12, 3);
        parser.process(b"alpha beta\r\nnext");
        let selection = Selection::new(parser.screen(), Point(0, 7), 2);
        assert_eq!(selection.text, "beta");
        parser.process(b"\r\nthird\r\nfourth");
        assert!(selection.valid(parser.screen()));
        parser.set_scrollback(2);
        assert_eq!(parser.screen().viewport_start(), 0);
        assert!(selection.contains(parser.screen(), Point(0, 6)));
        parser.process(b"\r\nfifth\r\nsixth");
        assert!(!selection.valid(parser.screen()));
    }
}
