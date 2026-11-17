//! Stash projection uses real Git metadata rather than message heuristics.
use std::{path::Path, process::Command};
use tracr::git::{CommitRecord, history_at};

fn git(path: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .arg("-C")
        .arg(path)
        .args([
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@example.invalid",
            "-c",
            "protocol.file.allow=always",
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
    String::from_utf8(output.stdout).unwrap().trim().to_owned()
}
fn init(path: &Path) -> String {
    git(path, &["init", "-qb", "main"]);
    std::fs::write(path.join("tracked"), "base").unwrap();
    git(path, &["add", "tracked"]);
    git(path, &["commit", "-qm", "Base"]);
    git(path, &["rev-parse", "HEAD"])
}
fn stash(path: &Path, name: &str, untracked: bool) -> (String, Vec<String>) {
    std::fs::write(path.join("tracked"), name).unwrap();
    if untracked {
        std::fs::write(path.join("untracked"), "extra").unwrap();
    }
    if untracked {
        git(path, &["stash", "push", "-u", "-m", name]);
    } else {
        git(path, &["stash", "push", "-m", name]);
    }
    let id = git(path, &["rev-parse", "refs/stash"]);
    let parents = git(path, &["show", "-s", "--format=%P", &id])
        .split_whitespace()
        .map(str::to_owned)
        .collect();
    (id, parents)
}
fn assert_layout(records: &[CommitRecord]) {
    let mut builder = tracr::git_layout::Builder::new();
    for row in records {
        builder
            .append(&tracr::git_layout::Commit {
                oid: row.oid.clone(),
                parents: row.parents.clone(),
            })
            .unwrap();
    }
}

#[tokio::test]
async fn bare_repository_reflogs_do_not_require_a_checkout() {
    let dir = tempfile::tempdir().unwrap();
    let project = dir.path().join("project");
    std::fs::create_dir(&project).unwrap();
    init(&project);
    let bare = dir.path().join("bare.git");
    git(
        dir.path(),
        &[
            "clone",
            "--bare",
            project.to_str().unwrap(),
            bare.to_str().unwrap(),
        ],
    );
    assert_eq!(history_at(&bare, 100, None).await.unwrap().len(), 1);
    let (id, _) = stash(&project, "Bare stash", false);
    git(&bare, &["fetch", project.to_str().unwrap(), &id]);
    git(
        &bare,
        &[
            "update-ref",
            "--create-reflog",
            "-m",
            "Bare stash",
            "refs/stash",
            &id,
        ],
    );
    let rows = history_at(&bare, 100, None).await.unwrap();
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].oid, id);
    assert_eq!(rows[0].stash.as_ref().unwrap().selectors, ["stash@{0}"]);
}

#[tokio::test]
async fn all_stashes_are_single_leaves_next_to_base_with_stable_prefixes() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let base = init(root);
    let (old, old_parents) = stash(root, "old 界", false);
    let (new, new_parents) = stash(root, "new", true);
    assert_eq!(old_parents.len(), 2);
    assert_eq!(new_parents.len(), 3);
    git(root, &["stash", "store", "-m", "duplicate old", &old]);
    git(root, &["commit", "--allow-empty", "-qm", "Later commit"]);
    let tip = git(root, &["rev-parse", "HEAD"]);
    let rows = history_at(root, 100, Some(&tip)).await.unwrap();
    assert_eq!(
        history_at(&root.join(".git"), 100, Some(&tip))
            .await
            .unwrap(),
        rows
    );
    assert_eq!(
        rows.iter().map(|r| r.oid.as_str()).collect::<Vec<_>>(),
        vec![&tip, &old, &new, &base]
    );
    assert_eq!(
        rows[1].stash.as_ref().unwrap().selectors,
        ["stash@{0}", "stash@{2}"]
    );
    assert_eq!(rows[2].stash.as_ref().unwrap().selectors, ["stash@{1}"]);
    for row in &rows[1..3] {
        assert_eq!(row.parents, [base.as_str()]);
    }
    for helper in old_parents.iter().skip(1).chain(new_parents.iter().skip(1)) {
        assert!(!rows.iter().any(|r| &r.oid == helper));
    }
    assert_layout(&rows);
    for limit in 1..=rows.len() + 1 {
        let prefix = history_at(root, limit, Some(&tip)).await.unwrap();
        assert_eq!(prefix, rows[..limit.min(rows.len())]);
        assert_layout(&prefix);
    }
    // Removing a non-tip reflog entry changes labels without changing refs/stash.
    git(root, &["stash", "drop", "stash@{1}"]);
    assert_eq!(git(root, &["rev-parse", "refs/stash"]), old);
    let rows = history_at(root, 100, Some(&tip)).await.unwrap();
    assert_eq!(rows.iter().filter(|r| r.stash.is_some()).count(), 1);
    assert_eq!(
        rows[1].stash.as_ref().unwrap().selectors,
        ["stash@{0}", "stash@{1}"]
    );
    git(root, &["stash", "clear"]);
    assert!(
        history_at(root, 100, Some(&tip))
            .await
            .unwrap()
            .iter()
            .all(|r| r.stash.is_none())
    );
}

#[tokio::test]
async fn independent_snapshot_refs_and_real_merges_are_preserved() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    init(root);
    let (_, parents) = stash(root, "snapshot", true);
    git(root, &["tag", "keep-index", &parents[1]]);
    git(root, &["branch", "side"]);
    git(root, &["commit", "--allow-empty", "-qm", "main"]);
    git(root, &["checkout", "-q", "side"]);
    git(root, &["commit", "--allow-empty", "-qm", "side"]);
    git(root, &["checkout", "-q", "main"]);
    git(root, &["merge", "--no-ff", "-qm", "Real merge", "side"]);
    let rows = history_at(root, 100, None).await.unwrap();
    assert!(
        rows.iter()
            .any(|r| r.oid == parents[1] && r.stash.is_none())
    );
    assert!(!rows.iter().any(|r| r.oid == parents[2]));
    assert_eq!(
        rows.iter()
            .find(|r| r.subject == "Real merge")
            .unwrap()
            .parents
            .len(),
        2
    );
    assert_layout(&rows);
    // An independently checked-out helper remains ordinary history too.
    let rows = history_at(root, 100, Some(&parents[2])).await.unwrap();
    assert!(
        rows.iter()
            .any(|r| r.oid == parents[2] && r.stash.is_none())
    );
}

#[tokio::test]
async fn abandoned_bases_linked_worktrees_and_shallow_boundaries() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let base = init(root);
    git(root, &["checkout", "--detach", "-q"]);
    git(root, &["commit", "--allow-empty", "-qm", "Abandoned base"]);
    let abandoned = git(root, &["rev-parse", "HEAD"]);
    let (id, _) = stash(root, "abandoned", false);
    git(root, &["checkout", "-q", "main"]);
    let linked = root.join("linked");
    git(
        root,
        &["worktree", "add", "--detach", linked.to_str().unwrap()],
    );
    let rows = history_at(&linked, 100, Some(&base)).await.unwrap();
    let i = rows.iter().position(|r| r.oid == id).unwrap();
    assert_eq!(rows[i + 1].oid, abandoned);
    assert_layout(&rows);
    // Shallow history retains its existing dangling-parent representation.
    git(root, &["commit", "--allow-empty", "-qm", "Newest"]);
    let shallow = root.join("shallow");
    git(
        root,
        &[
            "clone",
            "--depth=1",
            &format!("file://{}", root.display()),
            shallow.to_str().unwrap(),
        ],
    );
    let shallow_base = git(&shallow, &["rev-parse", "HEAD"]);
    let (id, _) = stash(&shallow, "shallow", false);
    let rows = history_at(&shallow, 100, None).await.unwrap();
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].oid, id);
    assert_eq!(rows[0].parents, [shallow_base]);
    assert!(!rows[1].parents.is_empty());
    assert_layout(&rows);
}

#[tokio::test]
async fn recently_saved_stash_on_old_commit_stays_beside_that_base() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let old_base = init(root);
    let (older, _) = stash(root, "First old-base stash", false);
    git(root, &["commit", "--allow-empty", "-qm", "Recent base"]);
    let recent_base = git(root, &["rev-parse", "HEAD"]);
    let (recent, _) = stash(root, "Recent-base stash", false);
    git(root, &["checkout", "--detach", "-q", &old_base]);
    let (new_on_old, _) = stash(root, "Newest stash on old base", false);
    let rows = history_at(root, 100, Some(&old_base)).await.unwrap();
    assert_eq!(
        rows.iter().map(|r| &r.oid).collect::<Vec<_>>(),
        [&recent, &recent_base, &new_on_old, &older, &old_base]
    );
    assert_eq!(rows[0].stash.as_ref().unwrap().selectors, ["stash@{1}"]);
    assert_eq!(rows[2].stash.as_ref().unwrap().selectors, ["stash@{0}"]);
    assert_layout(&rows);
}
