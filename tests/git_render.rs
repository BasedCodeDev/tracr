#[path = "support/git_graph_fixtures.rs"]
mod fixtures;
#[path = "support/git_graph_oracle.rs"]
mod oracle;
use tracr::{
    git_layout::layout,
    git_render::{BRIDGE, render},
};
use unicode_width::UnicodeWidthChar;

#[test]
fn all_glyphs_are_single_terminal_cells() {
    for c in "●│─╭╮╰╯├┤→←┊".chars().chain([BRIDGE]) {
        assert_eq!(c.width(), Some(1), "{c}");
    }
}

#[test]
fn corpus_and_every_prefix_preserve_displayed_topology() {
    for (name, commits) in fixtures::fixtures() {
        let full = render(&layout(&commits).unwrap());
        for len in 0..=commits.len() {
            let model = layout(&commits[..len]).unwrap();
            let graph = render(&model);
            oracle::check(&commits[..len], &model, &graph).unwrap_or_else(|e| {
                panic!(
                    "{name}/{len}: {e}\n{}",
                    graph
                        .rows
                        .iter()
                        .chain(graph.boundary.iter())
                        .map(|r| r.text())
                        .collect::<Vec<_>>()
                        .join("\n")
                )
            });
            assert_eq!(graph.rows, full.rows[..graph.rows.len()], "{name}/{len}");
            assert_eq!(graph, render(&model));
            for (oid, pos) in &graph.positions {
                assert_eq!(full.positions[oid], *pos);
            }
        }
    }
}

#[test]
fn large_prefixes_keep_styled_rows_and_positions() {
    let commits = fixtures::wide(600, 8);
    let full = render(&layout(&commits).unwrap());
    for count in [128, 256, 512] {
        let model = layout(&commits[..count]).unwrap();
        let graph = render(&model);
        oracle::check(&commits[..count], &model, &graph).unwrap();
        assert_eq!(graph.rows, full.rows[..graph.rows.len()]);
        for (oid, pos) in &graph.positions {
            assert_eq!(full.positions[oid], *pos);
        }
    }
}

#[test]
fn oracle_rejects_missing_edges_false_joins_and_wrong_mapping() {
    let (_, commits) = fixtures::fixtures()
        .into_iter()
        .find(|(n, _)| *n == "crossing")
        .unwrap();
    let model = layout(&commits).unwrap();
    let graph = render(&model);
    assert!(graph.rows.iter().any(|r| r.text().contains(BRIDGE)));
    let mut broken = graph.clone();
    for row in &mut broken.rows {
        for span in &mut row.spans {
            span.text = span.text.replace(BRIDGE, "├");
        }
    }
    assert!(oracle::check(&commits, &model, &broken).is_err());
    let mut broken = graph.clone();
    broken.rows[1].spans.clear();
    assert!(oracle::check(&commits, &model, &broken).is_err());
    let mut broken = graph;
    *broken.positions.values_mut().next().unwrap() = 999;
    assert!(oracle::check(&commits, &model, &broken).is_err());
}

#[test]
#[ignore = "local release performance probe"]
fn rendering_cost_probe() {
    for (count, width) in [(128, 4), (512, 4), (2048, 4), (10000, 4), (2048, 128)] {
        let commits = fixtures::wide(count, width);
        let model = layout(&commits).unwrap();
        let mut times = Vec::new();
        for _ in 0..7 {
            let start = std::time::Instant::now();
            let graph = render(std::hint::black_box(&model));
            times.push(start.elapsed());
            std::hint::black_box(graph);
        }
        times.sort();
        let graph = render(&model);
        let storage: usize = graph
            .rows
            .iter()
            .chain(graph.boundary.iter())
            .map(|r| {
                std::mem::size_of_val(r)
                    + r.spans
                        .iter()
                        .map(|s| std::mem::size_of_val(s) + s.text.len())
                        .sum::<usize>()
            })
            .sum();
        println!(
            "commits={count} lanes={width} rows={} width={} median={:?} row_bytes={storage} (excludes capacity, OIDs, map, model)",
            graph.rows.len() + usize::from(graph.boundary.is_some()),
            graph.width,
            times[3]
        );
    }
}

#[test]
fn generated_dags_preserve_every_edge_and_boundary() {
    let mut seed = 0x12345678u64;
    for case in 0..128 {
        let commits: Vec<_> = (0..20)
            .map(|i| {
                let mut parents = Vec::new();
                for p in i + 1..24 {
                    seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
                    if seed >> 60 < 3 {
                        parents.push(p.to_string());
                    }
                }
                if case % 2 == 0 {
                    parents.reverse();
                }
                tracr::git_layout::Commit {
                    oid: i.to_string(),
                    parents,
                }
            })
            .collect();
        let full = render(&layout(&commits).unwrap());
        for count in [1, 4, 12, 20] {
            let model = layout(&commits[..count]).unwrap();
            let graph = render(&model);
            oracle::check(&commits[..count], &model, &graph)
                .unwrap_or_else(|e| panic!("case {case}, prefix {count}: {e}"));
            assert_eq!(graph.rows, full.rows[..graph.rows.len()]);
        }
    }
}

#[test]
fn compact_linear_rows_and_shared_merge_forks() {
    let cases = fixtures::fixtures();
    let (_, linear) = cases.iter().find(|(name, _)| *name == "linear").unwrap();
    let linear = render(&layout(linear).unwrap());
    assert_eq!(linear.rows.len(), 5); // four commits and the root's separation row
    assert_eq!(linear.width, 1);
    assert!(linear.boundary.is_none());
    let (_, octopus) = cases.iter().find(|(name, _)| *name == "octopus").unwrap();
    let graph = render(&layout(octopus).unwrap());
    assert!(!graph.rows.iter().any(|r| r.text().contains(BRIDGE)));
    assert_eq!(graph.rows[2].text(), "│   ├→──╮");
    assert_eq!(graph.rows[3].text(), "│   │   ├→──╮");
    let (_, disconnected) = cases
        .iter()
        .find(|(name, _)| *name == "disconnected")
        .unwrap();
    let graph = render(&layout(disconnected).unwrap());
    assert_eq!(graph.rows[2].text(), "");
    assert!(graph.rows[2].oid.is_none());
}

#[test]
fn cancellation_interrupts_wide_routing_without_returning_partial_output() {
    let commits = fixtures::wide(600, 128);
    let model = layout(&commits).unwrap();
    let checks = std::cell::Cell::new(0);
    let result = tracr::git_render::render_cancellable(&model, || {
        checks.set(checks.get() + 1);
        checks.get() > 40
    });
    assert!(result.is_none());
    assert_eq!(checks.get(), 41);
}
