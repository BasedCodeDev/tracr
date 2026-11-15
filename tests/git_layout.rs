use tracr::git_layout::{Builder, Commit, LaneColor, LayoutError, layout};

fn c(oid: &str, parents: &[&str]) -> Commit {
    Commit {
        oid: oid.into(),
        parents: parents.iter().map(|s| (*s).into()).collect(),
    }
}

fn verify(commits: &[Commit]) {
    let full = layout(commits).unwrap();
    let mut builder = Builder::new();
    for (i, commit) in commits.iter().enumerate() {
        builder.append(commit).unwrap();
        assert_eq!(builder.snapshot(), &layout(&commits[..=i]).unwrap());
        assert_eq!(builder.snapshot().rows, full.rows[..=i]);
        let row = &full.rows[i];
        assert_eq!(full.positions[commit.oid.as_str()], i);
        assert_eq!(
            row.parents
                .iter()
                .map(|p| p.parent.as_ref())
                .collect::<Vec<_>>(),
            commit.parents
        );
        for (index, edge) in row.parents.iter().enumerate() {
            assert_eq!(edge.parent_index, index);
            assert!(row.outgoing.iter().any(|l| l.target == edge.parent
                && l.column == edge.target_column
                && l.id == edge.lane_id));
        }
        for lanes in [&row.incoming, &row.outgoing] {
            for pair in lanes.windows(2) {
                assert!(pair[0].column < pair[1].column);
            }
            let targets: std::collections::HashSet<_> = lanes.iter().map(|l| &l.target).collect();
            assert_eq!(targets.len(), lanes.len());
        }
        for lane in &row.incoming {
            if lane.target.as_ref() != commit.oid {
                assert!(
                    row.outgoing.contains(lane),
                    "unrelated lane must stay straight"
                );
            } else {
                assert_eq!(row.node.column, lane.column);
                assert_eq!(row.node.lane_id, lane.id);
            }
        }
    }
    assert_eq!(builder.into_layout(), full);
}

#[test]
fn topology_and_every_prefix_are_stable() {
    let fixtures = [
        vec![],
        vec![c("3", &["2"]), c("2", &["1"]), c("1", &[])],
        vec![
            c("m", &["a", "b"]),
            c("a", &["r"]),
            c("b", &["r"]),
            c("r", &[]),
        ],
        vec![
            c("m", &["a", "b", "c", "d"]),
            c("d", &["r"]),
            c("b", &["r"]),
            c("a", &["r"]),
            c("c", &["r"]),
            c("r", &[]),
        ],
        vec![
            c("6", &["4", "5"]),
            c("5", &["3", "2"]),
            c("4", &["2", "3"]),
            c("3", &["1"]),
            c("2", &["1"]),
            c("1", &[]),
        ],
        vec![c("a", &["b"]), c("x", &["y"]), c("b", &[]), c("y", &[])],
        vec![c("a", &["missing", "other"])],
    ];
    for commits in fixtures {
        verify(&commits);
    }
}

#[test]
fn parallel_and_interleaved_four_lane_fixtures() {
    // The same parent shapes used in the renderer/library feasibility probes.
    for parents in [
        vec![
            vec![],
            vec![1],
            vec![1],
            vec![2],
            vec![3],
            vec![4],
            vec![5],
            vec![6, 7],
            vec![4],
            vec![9],
            vec![7],
        ],
        vec![
            vec![],
            vec![1],
            vec![2],
            vec![1],
            vec![4],
            vec![1],
            vec![6],
            vec![1],
            vec![8],
        ],
    ] {
        let commits = parents
            .iter()
            .enumerate()
            .rev()
            .map(|(i, ps)| Commit {
                oid: (i + 1).to_string(),
                parents: ps.iter().map(usize::to_string).collect(),
            })
            .collect::<Vec<_>>();
        verify(&commits);
        assert!(layout(&commits).unwrap().unresolved.is_empty());
    }
}

#[test]
fn reserves_node_column_then_reuses_it_with_new_identity() {
    let result = layout(&[c("a", &["x"]), c("b", &["x", "y"]), c("c", &[])]).unwrap();
    let b = &result.rows[1];
    assert_eq!(b.node.column, 1);
    assert_eq!(
        b.parents
            .iter()
            .map(|e| e.target_column)
            .collect::<Vec<_>>(),
        [0, 2]
    );
    assert_eq!(result.rows[2].node.column, 1);
    assert_ne!(result.rows[2].node.lane_id, b.node.lane_id);
    assert_eq!(
        result
            .unresolved
            .iter()
            .map(|l| l.column)
            .collect::<Vec<_>>(),
        [0, 2]
    );
}

#[test]
fn colors_continue_first_parent_and_cycle_for_new_lanes() {
    let commits = (0..8).map(|i| c(&i.to_string(), &[])).collect::<Vec<_>>();
    let result = layout(&commits).unwrap();
    assert!(
        result
            .rows
            .iter()
            .all(|r| r.node.column == 0 && r.parents.is_empty())
    );
    assert_eq!(
        result.rows.iter().map(|r| r.node.color).collect::<Vec<_>>(),
        [
            LaneColor::Cyan,
            LaneColor::Magenta,
            LaneColor::Yellow,
            LaneColor::Green,
            LaneColor::Blue,
            LaneColor::Red,
            LaneColor::Cyan,
            LaneColor::Magenta
        ]
    );
    let linear = layout(&[c("a", &["b"]), c("b", &["c"])]).unwrap();
    assert_eq!(linear.rows[0].node.lane_id, linear.rows[1].node.lane_id);
}

#[test]
fn invalid_append_is_atomic() {
    let mut builder = Builder::new();
    builder.append(&c("a", &["b"])).unwrap();
    let cases = [
        (c("a", &[]), LayoutError::DuplicateCommit("a".into())),
        (c("b", &["b"]), LayoutError::SelfParent("b".into())),
        (
            c("b", &["c", "c"]),
            LayoutError::RepeatedParent {
                oid: "b".into(),
                parent: "c".into(),
            },
        ),
        (
            c("b", &["c", "a"]),
            LayoutError::ParentAlreadyProcessed {
                oid: "b".into(),
                parent: "a".into(),
            },
        ),
    ];
    for (commit, error) in cases {
        let before = builder.clone();
        assert_eq!(builder.append(&commit).unwrap_err(), error);
        assert_eq!(builder, before);
    }
    builder.append(&c("b", &[])).unwrap();
    assert_eq!(
        builder.into_layout(),
        layout(&[c("a", &["b"]), c("b", &[])]).unwrap()
    );
}

#[test]
#[ignore = "local performance probe; run in release mode with --nocapture"]
fn layout_cost_probe() {
    for (count, width) in [(128, 4), (512, 4), (2048, 4), (10000, 4), (2048, 128)] {
        let commits = (0..count)
            .map(|i| Commit {
                oid: format!("{i:040x}"),
                parents: if i + width < count {
                    vec![format!("{:040x}", i + width)]
                } else {
                    vec![]
                },
            })
            .collect::<Vec<_>>();
        let mut times = Vec::new();
        for _ in 0..7 {
            let start = std::time::Instant::now();
            let result = layout(std::hint::black_box(&commits)).unwrap();
            times.push(start.elapsed());
            std::hint::black_box(result);
        }
        times.sort();
        let result = layout(&commits).unwrap();
        let lane_entries: usize = result
            .rows
            .iter()
            .map(|r| r.incoming.len() + r.outgoing.len())
            .sum();
        let inline_bytes = result.rows.len() * std::mem::size_of::<tracr::git_layout::LayoutRow>()
            + lane_entries * std::mem::size_of::<tracr::git_layout::Lane>()
            + result.rows.iter().map(|r| r.parents.len()).sum::<usize>()
                * std::mem::size_of::<tracr::git_layout::ParentConnection>();
        println!(
            "commits={count} width={} median={:?} row_storage_bytes={inline_bytes} (excludes strings, map, spare capacity and builder)",
            result.width, times[3]
        );
    }
}
