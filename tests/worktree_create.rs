use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};
use tracr::worktree_create::{self as create};
fn git(root: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{:?}: {}",
        args,
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout)
        .unwrap()
        .trim_end_matches('\n')
        .into()
}
fn repository() -> (tempfile::TempDir, PathBuf) {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("project with spaces");
    fs::create_dir(&root).unwrap();
    git(&root, &["init", "-q"]);
    git(&root, &["config", "user.name", "Test"]);
    git(&root, &["config", "user.email", "test@example.invalid"]);
    fs::write(root.join("file"), "first").unwrap();
    git(&root, &["add", "file"]);
    git(&root, &["commit", "-qm", "First"]);
    (temp, root)
}
#[tokio::test]
async fn local_exclusions_relative_links_and_parent_move() {
    let (temp, root) = repository();
    let old_exclude = fs::read(root.join(".git/info/exclude")).unwrap();
    let context = create::context(&root, None).await.unwrap();
    let destination = create::suggested(&context, "feature/login");
    let plan = create::prepare(context, "feature/login".into(), destination.clone())
        .await
        .unwrap();
    assert_eq!(plan.exclusion.as_deref(), Some("/.worktrees/"));
    assert_eq!(
        fs::read(root.join(".git/info/exclude")).unwrap(),
        old_exclude
    );
    create::execute(plan).await.unwrap();
    assert!(git(&root, &["status", "--porcelain"]).is_empty());
    assert!(!root.join(".gitignore").exists());
    assert_eq!(
        git(&destination, &["branch", "--show-current"]),
        "feature/login"
    );
    let context = create::context(&destination, None).await.unwrap();
    let second = create::suggested(&context, "second");
    assert_eq!(second, root.join(".worktrees/second"));
    create::execute(
        create::prepare(context, "second".into(), second)
            .await
            .unwrap(),
    )
    .await
    .unwrap();
    let bytes = fs::read(root.join(".git/info/exclude")).unwrap();
    assert!(bytes.starts_with(&old_exclude));
    assert_eq!(
        String::from_utf8(bytes)
            .unwrap()
            .matches("/.worktrees/")
            .count(),
        1
    );
    let moved = temp.path().join("moved");
    fs::rename(&root, &moved).unwrap();
    assert!(
        git(
            &moved.join(".worktrees/feature/login"),
            &["status", "--porcelain"]
        )
        .is_empty()
    );
    let inventory = git(&moved, &["worktree", "list", "--porcelain"]);
    assert!(inventory.contains(moved.join(".worktrees/second").to_str().unwrap()));
    assert!(!inventory.contains("prunable"));
}
#[tokio::test]
async fn selected_commit_is_frozen_and_custom_destination_needs_no_exclude() {
    let (temp, root) = repository();
    let first = git(&root, &["rev-parse", "HEAD"]);
    fs::write(root.join("file"), "second").unwrap();
    git(&root, &["commit", "-qam", "Second"]);
    let context = create::context(&root, Some(&first)).await.unwrap();
    let path = temp.path().join("outside");
    let plan = create::prepare(context, "from-old".into(), path.clone())
        .await
        .unwrap();
    assert!(plan.outside);
    assert!(plan.exclusion.is_none());
    git(&root, &["commit", "--allow-empty", "-qm", "Third"]);
    create::execute(plan).await.unwrap();
    assert_eq!(git(&path, &["rev-parse", "HEAD"]), first);
    assert_eq!(fs::read_to_string(path.join("file")).unwrap(), "first");
}
#[tokio::test]
async fn collisions_symlinks_and_tracked_containers_are_rejected() {
    let (_temp, root) = repository();
    let context = create::context(&root, None).await.unwrap();
    for name in ["", "bad..branch", "-option", "@{-1}"] {
        assert!(
            create::prepare(context.clone(), name.into(), root.join(".worktrees/test"))
                .await
                .is_err()
        );
    }
    let current = git(&root, &["branch", "--show-current"]);
    assert!(
        create::prepare(context.clone(), current, root.join(".worktrees/test"))
            .await
            .unwrap_err()
            .contains("already exists")
    );
    fs::create_dir(root.join("exists")).unwrap();
    assert!(
        create::prepare(context.clone(), "new".into(), root.join("exists"))
            .await
            .is_err()
    );
    std::os::unix::fs::symlink("missing", root.join(".worktrees")).unwrap();
    assert!(
        create::prepare(context.clone(), "new".into(), root.join(".worktrees/new"))
            .await
            .unwrap_err()
            .contains("symlink")
    );
    fs::remove_file(root.join(".worktrees")).unwrap();
    fs::create_dir(root.join(".worktrees")).unwrap();
    fs::write(root.join(".worktrees/tracked"), "keep").unwrap();
    git(&root, &["add", ".worktrees/tracked"]);
    assert!(
        create::prepare(context, "new".into(), root.join(".worktrees/new"))
            .await
            .unwrap_err()
            .contains("tracked content")
    );
}
#[tokio::test]
async fn exclusion_conflicts_and_concurrent_changes_do_not_create_worktrees() {
    let (_temp, root) = repository();
    fs::write(root.join(".gitignore"), "!.worktrees/\n").unwrap();
    let context = create::context(&root, None).await.unwrap();
    let path = create::suggested(&context, "new");
    let plan = create::prepare(context.clone(), "new".into(), path.clone())
        .await
        .unwrap();
    assert!(
        create::execute(plan)
            .await
            .unwrap_err()
            .contains("higher-priority")
    );
    assert!(!path.exists());
    fs::remove_file(root.join(".gitignore")).unwrap();
    let plan = create::prepare(context.clone(), "new".into(), path.clone())
        .await
        .unwrap();
    fs::create_dir_all(&path).unwrap();
    fs::write(path.join("keep"), "keep").unwrap();
    assert!(create::execute(plan).await.is_err());
    assert!(path.join("keep").exists());
    let outside = root.join("custom");
    let plan = create::prepare(context, "other".into(), outside.clone())
        .await
        .unwrap();
    fs::write(root.join(".git/info/exclude"), "# concurrent change\n").unwrap();
    assert!(
        create::execute(plan)
            .await
            .unwrap_err()
            .contains("changed since review")
    );
    assert!(!outside.exists());
    assert_eq!(
        fs::read_to_string(root.join(".git/info/exclude")).unwrap(),
        "# concurrent change\n"
    );
}
#[tokio::test]
async fn custom_internal_exclusion_is_exact_and_unrelated_repo_is_rejected() {
    let (_temp, root) = repository();
    let context = create::context(&root, None).await.unwrap();
    let path = root.join("custom [space]");
    let plan = create::prepare(context.clone(), "custom".into(), path.clone())
        .await
        .unwrap();
    assert_eq!(plan.exclusion.as_deref(), Some("/custom\\ \\[space\\]/"));
    create::execute(plan).await.unwrap();
    assert!(git(&root, &["status", "--porcelain"]).is_empty());
    let (_other_temp, other) = repository();
    assert!(
        create::prepare(context, "unrelated".into(), other.join("nested"))
            .await
            .unwrap_err()
            .contains("unrelated")
    );
}
#[tokio::test]
async fn branch_race_and_hook_failure_preserve_partial_state() {
    let (_temp, root) = repository();
    let context = create::context(&root, None).await.unwrap();
    let path = create::suggested(&context, "new");
    let plan = create::prepare(context.clone(), "new".into(), path.clone())
        .await
        .unwrap();
    git(&root, &["branch", "new"]);
    assert!(
        create::execute(plan)
            .await
            .unwrap_err()
            .contains("already exists")
    );
    assert!(!path.exists());
    use std::os::unix::fs::PermissionsExt;
    let hook = root.join(".git/hooks/post-checkout");
    fs::write(&hook, "#!/bin/sh\nexit 1\n").unwrap();
    fs::set_permissions(&hook, fs::Permissions::from_mode(0o755)).unwrap();
    let path = create::suggested(&context, "hook");
    let plan = create::prepare(context, "hook".into(), path.clone())
        .await
        .unwrap();
    let error = create::execute(plan).await.unwrap_err();
    assert!(error.contains("Registered worktree: yes"), "{error}");
    assert!(path.exists());
    assert_eq!(git(&path, &["branch", "--show-current"]), "hook");
}
#[tokio::test]
async fn submodules_empty_and_external_git_dirs_are_rejected() {
    let (_temp, root) = repository();
    let oid = git(&root, &["rev-parse", "HEAD"]);
    git(
        &root,
        &[
            "update-index",
            "--add",
            "--cacheinfo",
            &format!("160000,{oid},sub"),
        ],
    );
    assert!(
        create::context(&root, None)
            .await
            .unwrap_err()
            .contains("submodules")
    );
    let temp = tempfile::tempdir().unwrap();
    let empty = temp.path().join("empty");
    fs::create_dir(&empty).unwrap();
    git(&empty, &["init", "-q"]);
    assert!(
        create::context(&empty, None)
            .await
            .unwrap_err()
            .contains("starting commit")
    );
    let (_temp2, root2) = repository();
    let external = root2.parent().unwrap().join("external-git");
    git(
        &root2,
        &["init", "--separate-git-dir", external.to_str().unwrap()],
    );
    assert!(
        create::context(&root2, None)
            .await
            .unwrap_err()
            .contains("external Git")
    );
}

#[tokio::test]
async fn exclude_lock_and_missing_registered_worktree_block_creation() {
    let (_temp, root) = repository();
    let context = create::context(&root, None).await.unwrap();
    let path = create::suggested(&context, "blocked");
    let plan = create::prepare(context.clone(), "blocked".into(), path.clone())
        .await
        .unwrap();
    let before = fs::read(root.join(".git/info/exclude")).unwrap();
    fs::write(root.join(".git/info/exclude.lock"), "other writer").unwrap();
    assert!(
        create::execute(plan)
            .await
            .unwrap_err()
            .contains("Cannot lock")
    );
    assert!(!path.exists());
    assert_eq!(fs::read(root.join(".git/info/exclude")).unwrap(), before);
    fs::remove_file(root.join(".git/info/exclude.lock")).unwrap();
    create::execute(
        create::prepare(context.clone(), "blocked".into(), path.clone())
            .await
            .unwrap(),
    )
    .await
    .unwrap();
    fs::remove_dir_all(&path).unwrap();
    assert!(
        create::prepare(context, "different".into(), path)
            .await
            .unwrap_err()
            .contains("already registered")
    );
}
