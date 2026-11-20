use std::{path::Path, process::Command};
use tracr::git::mainline;

fn git(path: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .arg("-C")
        .arg(path)
        .args([
            "-c",
            "core.hooksPath=/dev/null",
            "-c",
            "user.name=Fixture",
            "-c",
            "user.email=fixture@example.invalid",
        ])
        .args(args)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_owned()
}

#[tokio::test]
async fn names_win_and_detection_is_independent_of_checkout_head() {
    let tmp = tempfile::tempdir().unwrap();
    let p = tmp.path();
    git(p, &["init", "-qb", "trunk"]);
    assert_eq!(mainline(p).await.unwrap(), None);
    git(p, &["commit", "--allow-empty", "-qm", "root"]);
    let root = git(p, &["rev-parse", "HEAD"]);
    git(p, &["update-ref", "refs/remotes/origin/trunk", &root]);
    git(
        p,
        &[
            "symbolic-ref",
            "refs/remotes/origin/HEAD",
            "refs/remotes/origin/trunk",
        ],
    );
    assert_eq!(
        mainline(p).await.unwrap().unwrap().reference,
        "refs/heads/trunk"
    );
    git(p, &["branch", "master"]);
    assert_eq!(
        mainline(p).await.unwrap().unwrap().reference,
        "refs/heads/master"
    );
    git(p, &["branch", "main"]);
    git(p, &["checkout", "--detach", "-q"]);
    let main = mainline(p).await.unwrap().unwrap();
    assert_eq!(main.reference, "refs/heads/main");
    assert_eq!(main.tip, root);
    assert_eq!(main.ancestry, [root].into_iter().collect());
    git(p, &["branch", "-D", "main", "master", "trunk"]);
    assert_eq!(
        mainline(p).await.unwrap().unwrap().reference,
        "refs/remotes/origin/trunk"
    );
}

#[tokio::test]
async fn ambiguous_missing_and_dangling_remote_defaults_do_not_guess() {
    let tmp = tempfile::tempdir().unwrap();
    let p = tmp.path();
    git(p, &["init", "-qb", "feature"]);
    git(p, &["commit", "--allow-empty", "-qm", "root"]);
    let root = git(p, &["rev-parse", "HEAD"]);
    assert_eq!(mainline(p).await.unwrap(), None);
    git(
        p,
        &[
            "symbolic-ref",
            "refs/remotes/origin/HEAD",
            "refs/remotes/origin/missing",
        ],
    );
    assert_eq!(mainline(p).await.unwrap(), None);
    git(p, &["update-ref", "refs/remotes/upstream/trunk", &root]);
    git(
        p,
        &[
            "symbolic-ref",
            "refs/remotes/upstream/HEAD",
            "refs/remotes/upstream/trunk",
        ],
    );
    assert_eq!(
        mainline(p).await.unwrap().unwrap().reference,
        "refs/remotes/upstream/trunk"
    );
    git(p, &["update-ref", "refs/remotes/other/develop", &root]);
    git(
        p,
        &[
            "symbolic-ref",
            "refs/remotes/other/HEAD",
            "refs/remotes/other/develop",
        ],
    );
    assert_eq!(mainline(p).await.unwrap(), None);
}

#[tokio::test]
async fn ancestry_tracks_only_first_parents_and_refreshes_after_moves() {
    let tmp = tempfile::tempdir().unwrap();
    let p = tmp.path();
    git(p, &["init", "-qb", "main"]);
    git(p, &["commit", "--allow-empty", "-qm", "root"]);
    let root = git(p, &["rev-parse", "HEAD"]);
    git(p, &["checkout", "-qb", "feature"]);
    git(p, &["commit", "--allow-empty", "-qm", "side"]);
    let side = git(p, &["rev-parse", "HEAD"]);
    git(p, &["checkout", "-q", "main"]);
    git(p, &["merge", "--no-ff", "-m", "merge", "feature"]);
    let merge = git(p, &["rev-parse", "HEAD"]);
    let main = mainline(p).await.unwrap().unwrap();
    assert_eq!(main.ancestry, [root.clone(), merge].into_iter().collect());
    assert!(!main.ancestry.contains(&side));
    git(p, &["update-ref", "refs/heads/main", &root]);
    assert_eq!(
        mainline(p).await.unwrap().unwrap().ancestry,
        [root].into_iter().collect()
    );
}
