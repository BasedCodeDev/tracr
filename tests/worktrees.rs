use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use std::{
    path::{Path, PathBuf},
    process::Command,
    time::Duration,
};
use tracr::{path_picker::Picker, worktrees};
fn git(root: &Path, args: &[&str]) {
    let output = Command::new("git")
        .arg("-C")
        .arg(root)
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
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}
fn repo() -> (tempfile::TempDir, PathBuf, PathBuf, PathBuf) {
    let tmp = tempfile::tempdir().unwrap();
    let main = tmp.path().join("main project");
    std::fs::create_dir(&main).unwrap();
    git(&main, &["init", "-b", "main"]);
    git(&main, &["commit", "--allow-empty", "-m", "fixture"]);
    let nested = main.join("trees/feature 空");
    git(
        &main,
        &["worktree", "add", "-b", "feature", nested.to_str().unwrap()],
    );
    let external = tmp.path().join("external detached");
    git(
        &main,
        &["worktree", "add", "--detach", external.to_str().unwrap()],
    );
    (tmp, main, nested, external)
}
async fn settle(picker: &mut Picker, input: &mut String) {
    for _ in 0..300 {
        picker.tick(input);
        if !picker.loading {
            return;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("picker never completed");
}
fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}
#[tokio::test]
async fn discovers_registered_worktrees_from_subdirectories_and_linked_checkouts() {
    let (_tmp, main, nested, external) = repo();
    let child = nested.join("child");
    std::fs::create_dir(&child).unwrap();
    for origin in [&main, &nested, &child] {
        let rows = worktrees::discover(origin).await.unwrap();
        assert_eq!(rows.len(), 3);
        assert!(
            rows.iter()
                .any(|r| r.path == main && r.main && r.branch.as_deref() == Some("main"))
        );
        assert!(
            rows.iter()
                .any(|r| r.path == nested && r.branch.as_deref() == Some("feature"))
        );
        assert!(
            rows.iter()
                .any(|r| r.path == external && r.branch.is_none())
        );
    }
    worktrees::validate(&main, &external).await.unwrap();
    std::fs::remove_dir_all(&external).unwrap();
    assert!(
        worktrees::discover(&main)
            .await
            .unwrap()
            .iter()
            .find(|r| r.path == external)
            .unwrap()
            .unavailable
            .is_some()
    );
    assert!(worktrees::validate(&main, &external).await.is_err());
    assert!(worktrees::discover(&external).await.is_err());
    assert!(worktrees::discover(Path::new("")).await.is_err());
    assert!(worktrees::discover(main.parent().unwrap()).await.is_err());
}
#[tokio::test]
async fn tabs_restore_independent_paths_and_enter_only_fills() {
    let (_tmp, main, nested, _external) = repo();
    let mut picker = Picker::for_relink(main.clone(), Some(main.clone()));
    let mut input = nested.to_str().unwrap().to_owned();
    settle(&mut picker, &mut input).await;
    let folder = input.clone();
    let selected = picker.selected.clone();
    let rows: Vec<_> = picker.rows.iter().map(|r| r.path.clone()).collect();
    assert!(!picker.key(&mut input, key(KeyCode::Right)));
    settle(&mut picker, &mut input).await;
    assert_eq!(input, folder); // First visit does not choose a destination.
    for _ in 0..3 {
        assert!(!picker.key(&mut input, key(KeyCode::Right)));
        assert!(picker.worktrees_active());
        assert!(!picker.worktrees.as_ref().unwrap().loading);
        assert_eq!(input, folder);
    }

    assert!(!picker.key(&mut input, key(KeyCode::Down)));
    assert_eq!(input, main.to_str().unwrap());
    assert!(!picker.key(&mut input, key(KeyCode::Enter)));
    assert!(picker.worktree_validation(&input).is_some());
    for _ in 0..3 {
        picker.key(&mut input, key(KeyCode::Right));
        assert_eq!(input, main.to_str().unwrap());
        assert!(picker.worktree_validation(&input).is_some());
        assert!(!picker.worktrees.as_ref().unwrap().loading);
    }
    picker.key(&mut input, key(KeyCode::Left));
    settle(&mut picker, &mut input).await;
    assert_eq!(input, folder);
    assert_eq!(picker.selected, selected);
    for _ in 0..3 {
        picker.key(&mut input, key(KeyCode::Left));
        assert!(!picker.worktrees_active());
        assert_eq!(input, folder);
        assert_eq!(picker.selected, selected);
    }

    assert_eq!(
        picker
            .rows
            .iter()
            .map(|r| r.path.clone())
            .collect::<Vec<_>>(),
        rows
    );
    picker.key(&mut input, key(KeyCode::Right));
    settle(&mut picker, &mut input).await;
    assert_eq!(input, main.to_str().unwrap());
    picker.focus = 0;
    let caret = picker.caret;
    picker.key(&mut input, key(KeyCode::Left));
    assert!(picker.worktrees_active());
    assert!(picker.caret < caret);
    picker.key(&mut input, key(KeyCode::Char('x')));
    assert!(picker.worktree_validation(&input).is_none());
    let edited = input.clone();
    picker.focus = 1;
    picker.key(&mut input, key(KeyCode::Left));
    assert_eq!(input, folder);
    picker.key(&mut input, key(KeyCode::Right));
    settle(&mut picker, &mut input).await;
    assert_eq!(input, edited);
    picker.focus = 2;
    assert!(picker.key(&mut input, key(KeyCode::Enter)) == picker.valid);
}

#[tokio::test]
async fn removed_selection_and_abandoned_discovery_do_not_replace_folder_draft() {
    let (_tmp, main, nested, external) = repo();
    let mut picker = Picker::for_relink(main.clone(), Some(main.clone()));
    let mut input = nested.to_str().unwrap().to_owned();
    settle(&mut picker, &mut input).await;
    let original = input.clone();
    picker.key(&mut input, key(KeyCode::Right));
    picker.key(&mut input, key(KeyCode::Left)); // Abort before receiving discovery.
    settle(&mut picker, &mut input).await;
    assert_eq!(input, original);
    picker.key(&mut input, key(KeyCode::Right));
    settle(&mut picker, &mut input).await;
    let index = picker
        .worktrees
        .as_ref()
        .unwrap()
        .entries
        .iter()
        .position(|r| r.path == external)
        .unwrap();
    for _ in 0..=index {
        picker.key(&mut input, key(KeyCode::Down));
    }
    assert_eq!(input, external.to_str().unwrap());
    picker.key(&mut input, key(KeyCode::Left));
    assert_eq!(input, original);
    git(&main, &["worktree", "remove", external.to_str().unwrap()]);
    picker.key(&mut input, key(KeyCode::Right));
    settle(&mut picker, &mut input).await;
    assert!(input.is_empty());
    assert!(!picker.valid);
    assert!(
        picker
            .error
            .as_ref()
            .unwrap()
            .contains("no longer available")
    );
    picker.key(&mut input, key(KeyCode::Left));
    assert_eq!(input, original);
}
