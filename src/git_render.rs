//! Pure Unicode drawing over the deterministic lane model.
//!
//! Each logical transition occupies its own immutable block. Separate routing
//! rows avoid horizontal overlaps. Active vertical lanes cannot be moved without
//! revising earlier blocks; an intervening lane is therefore crossed with `⌢`,
//! which carries two independent paths, never a junction. Arrows establish the
//! direction of horizontal ancestry at joins. No terminal escapes are emitted.
use std::{collections::BTreeMap, sync::Arc};

use crate::{
    git::GraphSpan,
    git_layout::{LaneColor, Layout, LayoutRow},
};

pub const LANE_PITCH: usize = 4;
pub const BRIDGE: char = '⌢';

/// Quiet, consistent accent for linked-worktree markers in both panes.
pub const DIRTY_COLOR: ratatui::style::Color = ratatui::style::Color::Rgb(184, 151, 94);

pub const WORKTREE_COLOR: ratatui::style::Color = ratatui::style::Color::Rgb(129, 151, 181);

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RenderedRow {
    pub spans: Vec<GraphSpan>,
    /// Only commit rows are selectable. Connector and boundary rows have no OID.
    pub oid: Option<Arc<str>>,
}
impl RenderedRow {
    pub fn text(&self) -> String {
        self.spans.iter().map(|span| span.text.as_str()).collect()
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RenderedGraph {
    /// Completed blocks, without global-width padding or boundary decoration.
    pub rows: Vec<RenderedRow>,
    pub positions: BTreeMap<Arc<str>, usize>,
    /// Open ancestry, replaced on expansion; not part of the stable row prefix.
    pub boundary: Option<RenderedRow>,
    /// Terminal cells, not UTF-8 bytes. Includes the boundary if present.
    pub width: usize,
}

impl RenderedGraph {
    /// Trailing rows owned by each commit, excluding the open-history boundary.
    pub fn commit_tail_ranges(&self) -> BTreeMap<usize, std::ops::Range<usize>> {
        let mut starts: Vec<_> = self.positions.values().copied().collect();
        starts.sort_unstable();
        starts
            .iter()
            .enumerate()
            .map(|(i, &start)| {
                (
                    start,
                    start + 1..starts.get(i + 1).copied().unwrap_or(self.rows.len()),
                )
            })
            .collect()
    }
}

fn color(color: LaneColor) -> u8 {
    match color {
        LaneColor::Cyan => 36,
        LaneColor::Magenta => 35,
        LaneColor::Yellow => 33,
        LaneColor::Green => 32,
        LaneColor::Blue => 34,
        LaneColor::Red => 31,
    }
}

#[derive(Default)]
struct Line(Vec<(char, Option<u8>)>);
impl Line {
    fn set(&mut self, x: usize, glyph: char, tint: LaneColor) {
        self.0.resize(self.0.len().max(x + 1), (' ', None));
        self.0[x] = (glyph, Some(color(tint)));
    }
    fn verticals(lanes: &BTreeMap<usize, LaneColor>) -> Self {
        let mut line = Self::default();
        for (&column, &tint) in lanes {
            line.set(column * LANE_PITCH, '│', tint);
        }
        line
    }
    fn finish(self, oid: Option<Arc<str>>) -> RenderedRow {
        let mut spans: Vec<GraphSpan> = Vec::new();
        for (glyph, color) in self.0 {
            if let Some(last) = spans.last_mut().filter(|s| s.color == color) {
                last.text.push(glyph);
            } else {
                spans.push(GraphSpan {
                    text: glyph.to_string(),
                    color,
                });
            }
        }
        RenderedRow { spans, oid }
    }
}

fn block(
    row: &LayoutRow,
    output: &mut Vec<RenderedRow>,
    cancelled: &impl Fn() -> bool,
) -> Option<()> {
    let mut active: BTreeMap<_, _> = row.incoming.iter().map(|l| (l.column, l.color)).collect();
    let source = row.node.column * LANE_PITCH;
    let mut node = Line::verticals(&active);
    node.set(source, '●', row.node.color);
    output.push(node.finish(Some(row.node.oid.clone())));
    active.remove(&row.node.column);

    let continues = row
        .parents
        .iter()
        .any(|p| p.target_column == row.node.column);
    let sideways: Vec<_> = row
        .parents
        .iter()
        .filter(|p| p.target_column != row.node.column)
        .collect();
    // New lanes in this block have only this commit as their source, so they
    // may fork again without introducing ancestry for another child. Never use
    // a pre-existing target as a routing fork: that would create false parents.
    let mut opened = Vec::new();
    let mut routes = Vec::new();
    for edge in sideways {
        if cancelled() {
            return None;
        }
        let target = edge.target_column;
        let anchor = opened
            .iter()
            .copied()
            .filter(|column| {
                *column > row.node.column.min(target) && *column < row.node.column.max(target)
            })
            .min_by_key(|column: &usize| column.abs_diff(target))
            .unwrap_or(row.node.column);
        routes.push((edge, anchor));
        if !active.contains_key(&target) {
            opened.push(target);
        }
    }
    if continues || !routes.is_empty() {
        active.insert(row.node.column, row.node.color);
    }
    for (index, &(edge, anchor)) in routes.iter().enumerate() {
        if cancelled() {
            return None;
        }
        let source = anchor * LANE_PITCH;
        let target = edge.target_column * LANE_PITCH;
        let right = target > source;
        let trunk_continues = anchor != row.node.column
            || continues
            || routes[index + 1..]
                .iter()
                .any(|(_, column)| *column == anchor);
        let mut line = Line::verticals(&active);
        for x in source.min(target) + 1..source.max(target) {
            if x % 256 == 0 && cancelled() {
                return None;
            }
            let crossing = x % LANE_PITCH == 0 && active.contains_key(&(x / LANE_PITCH));
            line.set(x, if crossing { BRIDGE } else { '─' }, edge.color);
        }
        // A directed join cannot be mistaken for a new fork of an existing lane.
        line.set(
            if right { source + 1 } else { source - 1 },
            if right { '→' } else { '←' },
            edge.color,
        );
        line.set(
            source,
            match (right, trunk_continues) {
                (true, true) => '├',
                (false, true) => '┤',
                (true, false) => '╰',
                (false, false) => '╯',
            },
            active[&anchor],
        );
        line.set(
            target,
            match (right, active.contains_key(&edge.target_column)) {
                (true, true) => '┤',
                (false, true) => '├',
                (true, false) => '╮',
                (false, false) => '╭',
            },
            edge.color,
        );
        output.push(line.finish(None));
        if !trunk_continues {
            active.remove(&anchor);
        }
        active.insert(edge.target_column, edge.color);
    }
    if row.parents.is_empty() {
        // A later, unrelated tip may reuse this column. A blank at the terminated
        // node prevents adjacent dots from implying a continuation into it.
        output.push(Line::verticals(&active).finish(None));
    }
    Some(())
}

/// Render a model produced by `git_layout::Builder` or `git_layout::layout`.
/// Rows are independent of viewport width and future commits. The caller owns
/// caching, cancellation and adapting graph rows into its history presentation.
pub fn render(layout: &Layout) -> RenderedGraph {
    render_cancellable(layout, || false).expect("uncancelled rendering")
}

pub fn render_cancellable(layout: &Layout, cancelled: impl Fn() -> bool) -> Option<RenderedGraph> {
    let mut result = RenderedGraph::default();
    for row in &layout.rows {
        if cancelled() {
            return None;
        }
        result
            .positions
            .insert(row.node.oid.clone(), result.rows.len());
        block(row, &mut result.rows, &cancelled)?;
    }
    if !layout.unresolved.is_empty() {
        let mut line = Line::default();
        for lane in &layout.unresolved {
            line.set(lane.column * LANE_PITCH, '┊', lane.color);
        }
        result.boundary = Some(line.finish(None));
    }
    result.width = result
        .rows
        .iter()
        .chain(result.boundary.iter())
        .map(|row| row.spans.iter().map(|s| s.text.chars().count()).sum())
        .max()
        .unwrap_or(0);
    Some(result)
}

/// Vertical lanes immediately below a commit, before its parent routing rows.
/// Annotation rows stretch these edges without adding nodes or new connections.
pub fn annotation_continuation(row: &crate::git::GraphRow) -> Vec<GraphSpan> {
    row.graph_spans
        .iter()
        .map(|span| GraphSpan {
            text: span
                .text
                .replace('●', if row.parents.is_empty() { " " } else { "│" }),
            color: span.color,
        })
        .collect()
}
