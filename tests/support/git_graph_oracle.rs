//! Deliberately reads final glyphs and OID positions, not renderer routing data.
use std::collections::{BTreeMap, BTreeSet, HashSet};
use tracr::{
    git_layout::{Commit, Layout},
    git_render::{LANE_PITCH, RenderedGraph},
};

const N: u8 = 1;
const E: u8 = 2;
const S: u8 = 4;
const W: u8 = 8;
fn ports(c: char) -> u8 {
    match c {
        ' ' => 0,
        '│' | '●' => N | S,
        '─' | '→' | '←' => E | W,
        '╭' => E | S,
        '╮' => W | S,
        '╰' => N | E,
        '╯' => N | W,
        '├' => N | S | E,
        '┤' => N | S | W,
        '⌢' => N | S | E | W,
        '┊' => N,
        _ => panic!("unknown graph glyph {c}"),
    }
}
fn step(x: usize, y: usize, dir: u8) -> Option<(usize, usize)> {
    match dir {
        N => y.checked_sub(1).map(|y| (x, y)),
        W => x.checked_sub(1).map(|x| (x, y)),
        E => Some((x + 1, y)),
        S => Some((x, y + 1)),
        _ => unreachable!(),
    }
}
fn opposite(dir: u8) -> u8 {
    match dir {
        N => S,
        S => N,
        E => W,
        W => E,
        _ => unreachable!(),
    }
}

pub fn check(commits: &[Commit], layout: &Layout, graph: &RenderedGraph) -> Result<(), String> {
    let grid: Vec<Vec<_>> = graph
        .rows
        .iter()
        .chain(graph.boundary.iter())
        .map(|r| r.text().chars().collect())
        .collect();
    let cell = |x: usize, y: usize| grid.get(y).and_then(|r| r.get(x)).copied().unwrap_or(' ');
    let mut nodes = BTreeMap::new();
    for (y, row) in graph.rows.iter().enumerate() {
        let dots: Vec<_> = grid[y]
            .iter()
            .enumerate()
            .filter(|(_, c)| **c == '●')
            .map(|(x, _)| x)
            .collect();
        match &row.oid {
            Some(oid) if dots.len() == 1 && graph.positions.get(oid) == Some(&y) => {
                nodes.insert((dots[0], y), oid.to_string());
            }
            None if dots.is_empty() => (),
            _ => return Err("node/OID mapping mismatch".into()),
        }
    }
    if nodes.len() != commits.len() || graph.positions.len() != commits.len() {
        return Err("commit count mismatch".into());
    }
    // Every non-node stroke must connect physically on each of its ports.
    for (y, row) in grid.iter().enumerate() {
        for (x, &c) in row.iter().enumerate() {
            if c == '●' {
                continue;
            }
            for dir in [N, E, S, W] {
                if ports(c) & dir == 0 {
                    continue;
                }
                let connected = step(x, y, dir)
                    .is_some_and(|(nx, ny)| ports(cell(nx, ny)) & opposite(dir) != 0);
                if !connected {
                    return Err(format!("dangling {c} at {x},{y} toward {dir}"));
                }
            }
        }
    }
    for commit in commits {
        let (&(sx, sy), _) = nodes
            .iter()
            .find(|(_, oid)| **oid == commit.oid)
            .ok_or("missing commit")?;
        let mut pending = vec![(sx, sy + 1, N)];
        let mut seen = HashSet::new();
        let mut reached = BTreeSet::new();
        while let Some((x, y, entry)) = pending.pop() {
            if !seen.insert((x, y, entry)) {
                continue;
            }
            let glyph = cell(x, y);
            if ports(glyph) & entry == 0 {
                continue;
            }
            if let Some(oid) = nodes.get(&(x, y)) {
                reached.insert(oid.clone());
                continue;
            }
            if glyph == '┊' {
                if y != graph.rows.len() {
                    return Err("boundary inside completed rows".into());
                }
                let lane = layout
                    .unresolved
                    .iter()
                    .find(|l| l.column * LANE_PITCH == x)
                    .ok_or("unlabelled boundary")?;
                reached.insert(lane.target.to_string());
                continue;
            }
            // Crossings keep independent axes. Arrows reject walking back into a
            // different child's ancestry when entering a destination junction.
            if (glyph == '→' && entry != W) || (glyph == '←' && entry != E) {
                continue;
            }
            for dir in [E, S, W] {
                if dir == entry || ports(glyph) & dir == 0 {
                    continue;
                }
                if glyph == '⌢' && dir != opposite(entry) {
                    continue;
                }
                if let Some((nx, ny)) = step(x, y, dir) {
                    pending.push((nx, ny, opposite(dir)));
                }
            }
        }
        let expected: BTreeSet<_> = commit.parents.iter().cloned().collect();
        if reached != expected {
            return Err(format!(
                "{}: expected {expected:?}, rendered {reached:?}",
                commit.oid
            ));
        }
    }
    Ok(())
}
