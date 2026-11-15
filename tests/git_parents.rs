//! Ordered parent metadata is independent of graph glyphs and prefix boundaries.
use std::{collections::HashMap, path::Path, process::Command};
use tracr::git::{history_at, parse_commits};
#[path = "support/git_graph_oracle.rs"]
mod graph_oracle;

#[test]
fn parses_ordered_parents_roots_and_both_hash_lengths() {
    for width in [40, 64] {
        let oid = "a".repeat(width);
        let parents: Vec<_> = ['d', 'b', 'c']
            .iter()
            .map(|c| c.to_string().repeat(width))
            .collect();
        let raw = format!("\0{oid}\0{}\0merge 界\0\n", parents.join(" "));
        let rows = parse_commits(raw.as_bytes()).unwrap();
        assert_eq!(rows[0].parents, parents);
        assert_eq!(rows[0].subject, "merge 界");
        let root = parse_commits(format!("\0{oid}\0\0root\0\n").as_bytes()).unwrap();
        assert!(root[0].parents.is_empty());
        assert_eq!(root[0].oid, oid);
        let single =
            parse_commits(format!("\0{oid}\0{}\0child\0\n", parents[0]).as_bytes()).unwrap();
        assert_eq!(single[0].parents, vec![parents[0].clone()]);
    }
}

#[test]
fn rejects_invalid_parent_tokens_and_old_or_missing_fields() {
    let oid = "a".repeat(40);
    let valid = "b".repeat(40);
    for parents in [
        "b".repeat(39),
        "b".repeat(64),
        "z".repeat(40),
        format!(" {valid}"),
        format!("{valid} "),
        format!("{valid}  {valid}"),
        format!("{valid}\t{valid}"),
    ] {
        assert!(
            parse_commits(format!("\0{oid}\0{parents}\0bad\0\n").as_bytes()).is_err(),
            "{parents:?}"
        );
    }
    for raw in [
        format!("\0{oid}\0legacy\0\n"),
        format!("\0{oid}\0\0missing terminator\n"),
        format!("\0{oid}\0\0subject\0extra\0\n"),
    ] {
        assert!(parse_commits(raw.as_bytes()).is_err());
    }
}

fn git(path: &Path, args: &[&str]) -> String {
    let mut command = Command::new("git");
    // Avoid routing test repositories through ambient Git environment overrides.
    for (key, _) in std::env::vars_os() {
        if key.to_string_lossy().starts_with("GIT_") {
            command.env_remove(key);
        }
    }
    let output = command
        .arg("-C")
        .arg(path)
        .args([
            "-c",
            "user.name=Parent test",
            "-c",
            "user.email=test@example.invalid",
        ])
        .args(args)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout)
        .unwrap()
        .trim_end()
        .to_owned()
}
fn commit(path: &Path, tree: &str, parents: &[&str], message: &str) -> String {
    let mut args = vec!["commit-tree", tree, "-m", message];
    for parent in parents {
        args.extend(["-p", parent]);
    }
    git(path, &args)
}

async fn compare(path: &Path, head: Option<&str>) {
    let oracle: HashMap<String, Vec<String>> =
        git(path, &["rev-list", "--all", "HEAD", "--parents"])
            .lines()
            .map(|line| {
                let mut ids = line.split_whitespace();
                (
                    ids.next().unwrap().to_owned(),
                    ids.map(str::to_owned).collect(),
                )
            })
            .collect();
    let full = history_at(path, 128, head).await.unwrap();
    let commits: Vec<_> = full
        .iter()
        .map(|row| tracr::git_layout::Commit {
            oid: row.oid.clone(),
            parents: row.parents.clone(),
        })
        .collect();
    let model = tracr::git_layout::layout(&commits).unwrap();
    let mut builder = tracr::git_layout::Builder::new();
    for (index, commit) in commits.iter().enumerate() {
        builder.append(commit).unwrap();
        graph_oracle::check(
            &commits[..=index],
            builder.snapshot(),
            &tracr::git_render::render(builder.snapshot()),
        )
        .unwrap();
        assert_eq!(
            builder.snapshot(),
            &tracr::git_layout::layout(&commits[..=index]).unwrap()
        );
        assert_eq!(builder.snapshot().rows, model.rows[..=index]);
        assert_eq!(
            model.rows[index]
                .parents
                .iter()
                .map(|p| p.parent.to_string())
                .collect::<Vec<_>>(),
            oracle[&commit.oid]
        );
    }
    assert!(model.unresolved.is_empty());
    for row in full.iter() {
        assert_eq!(&row.parents, &oracle[&row.oid]);
    }
    assert_eq!(full.len(), oracle.len());
    for limit in [1, 2, 3, 4] {
        let prefix = history_at(path, limit, head).await.unwrap();
        for row in &prefix {
            let original = full.iter().find(|r| r.oid == row.oid).unwrap();
            assert_eq!(row.parents, original.parents);
        }
    }
}

#[tokio::test]
async fn native_merges_disconnected_detached_and_shallow_keep_ordered_parents() {
    let temp = tempfile::tempdir().unwrap();
    let repo = temp.path().join("repo");
    std::fs::create_dir(&repo).unwrap();
    git(&repo, &["init", "-qb", "main"]);
    let tree = git(&repo, &["mktree"]);
    let root = commit(&repo, &tree, &[], "root");
    let left = commit(&repo, &tree, &[&root], "left");
    let right = commit(&repo, &tree, &[&root], "right");
    let side = commit(&repo, &tree, &[&root], "side");
    let merge = commit(&repo, &tree, &[&left, &right], "ordinary merge");
    let octopus = commit(&repo, &tree, &[&merge, &side, &right], "octopus merge");
    git(&repo, &["update-ref", "refs/heads/main", &octopus]);
    let first = history_at(&repo, 1, None).await.unwrap();
    let first = &first[0];
    assert_eq!(first.parents, vec![merge.clone(), side, right]);
    assert!(first.parents.iter().all(|p| &first.oid != p));
    compare(&repo, None).await;
    let disconnected = commit(&repo, &tree, &[], "independent root");
    git(
        &repo,
        &["update-ref", "refs/heads/independent", &disconnected],
    );
    compare(&repo, None).await;
    let detached = commit(&repo, &tree, &[&left], "unreferenced detached tip");
    git(&repo, &["update-ref", "--no-deref", "HEAD", &detached]);
    compare(&repo, Some(&detached)).await;
    let shallow = temp.path().join("shallow");
    git(
        temp.path(),
        &[
            "clone",
            "-q",
            "--depth=2",
            "--branch=main",
            &format!("file://{}", repo.display()),
            shallow.to_str().unwrap(),
        ],
    );
    assert_eq!(
        git(&shallow, &["rev-parse", "--is-shallow-repository"]),
        "true"
    );
    let records = history_at(&shallow, 128, None).await.unwrap();
    for record in &records {
        let raw = git(&shallow, &["cat-file", "-p", &record.oid]);
        let parents: Vec<_> = raw
            .lines()
            .take_while(|line| !line.is_empty())
            .filter_map(|line| line.strip_prefix("parent ").map(str::to_owned))
            .collect();
        assert_eq!(record.parents, parents);
    }
    assert!(records.iter().any(|record| {
        record
            .parents
            .iter()
            .any(|parent| !records.iter().any(|r| &r.oid == parent))
    }));
}

#[test]
fn commit_subjects_cannot_inject_terminal_controls() {
    let oid = "a".repeat(40);
    let records = parse_commits(format!("\0{oid}\0\0subject \x1b[31mred\0\n").as_bytes()).unwrap();
    assert!(!records[0].subject.contains('\x1b'));
    assert!(parse_commits(format!("* \0{oid}\0\0legacy graph\0\n").as_bytes()).is_err());
}
