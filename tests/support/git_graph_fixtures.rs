use tracr::git_layout::Commit;

pub fn fixtures() -> Vec<(&'static str, Vec<Commit>)> {
    // Parent numbers match the earlier Python renderer/library probe corpus.
    type Shape = (&'static str, Vec<Vec<usize>>, Vec<usize>);
    let shapes: Vec<Shape> = vec![
        ("empty", vec![], vec![]),
        (
            "linear",
            vec![vec![], vec![1], vec![2], vec![3]],
            vec![4, 3, 2, 1],
        ),
        (
            "merge",
            vec![vec![], vec![1], vec![1], vec![2, 3]],
            vec![4, 3, 2, 1],
        ),
        (
            "four-lanes",
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
            vec![11, 10, 8, 9, 6, 7, 4, 5, 2, 3, 1],
        ),
        (
            "parallel-four",
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
            vec![9, 7, 5, 3, 8, 6, 4, 2, 1],
        ),
        (
            "octopus",
            vec![vec![], vec![1], vec![1], vec![1], vec![1], vec![2, 3, 4, 5]],
            vec![6, 5, 4, 3, 2, 1],
        ),
        (
            "crossing",
            vec![vec![], vec![1], vec![1], vec![2, 3], vec![3, 2], vec![4, 5]],
            vec![6, 5, 4, 3, 2, 1],
        ),
        (
            "disconnected",
            vec![vec![], vec![1], vec![], vec![3]],
            vec![4, 3, 2, 1],
        ),
        (
            "detached",
            vec![vec![], vec![1], vec![1], vec![3]],
            vec![4, 2, 3, 1],
        ),
    ];
    shapes
        .into_iter()
        .map(|(name, parents, order)| {
            (
                name,
                order
                    .into_iter()
                    .map(|id| Commit {
                        oid: format!("{id:040x}"),
                        parents: parents[id - 1]
                            .iter()
                            .map(|p| format!("{p:040x}"))
                            .collect(),
                    })
                    .collect(),
            )
        })
        .collect()
}

pub fn wide(count: usize, width: usize) -> Vec<Commit> {
    (0..count)
        .map(|i| Commit {
            oid: format!("{i:040x}"),
            parents: if i + width < count {
                vec![format!("{:040x}", i + width)]
            } else {
                vec![]
            },
        })
        .collect()
}
