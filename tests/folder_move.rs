use serde_json::{Value, json};
use std::{
    fs,
    path::{Path, PathBuf},
};
use tracr::{
    domain::SourceId,
    folder_move::{self, Outcome, Source},
};

fn source(root: &Path, index: usize, cwd: &Path, archived: bool) -> Source {
    let home = root.join(format!("codex-{index}"));
    fs::create_dir(&home).unwrap();
    fs::write(
        home.join("native.json"),
        json!({"cwd":cwd,"archived":archived,"profile":":workspace"}).to_string(),
    )
    .unwrap();
    Source {
        binary: Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/app-server/relink_fake.py"),
        home,
        id: SourceId(format!("source-{index}")),
    }
}
fn change(source: &Source, key: &str, value: Value) {
    let path = source.home.join("native.json");
    let mut data: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    data[key] = value;
    fs::write(path, data.to_string()).unwrap();
}
fn read(source: &Source) -> Value {
    serde_json::from_slice(&fs::read(source.home.join("native.json")).unwrap()).unwrap()
}
async fn execute(plan: folder_move::Plan, state: &Path, recovery: bool) -> Outcome {
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    folder_move::execute(plan.clone(), state.to_owned(), plan.sources, recovery, tx).await
}
#[tokio::test]
async fn moves_descendants_across_sources_preserves_files_archives_and_recovers() {
    let root = tempfile::tempdir().unwrap();
    let old = root.path().join("old");
    fs::create_dir_all(old.join("sub")).unwrap();
    fs::write(old.join(".hidden"), "content").unwrap();
    std::os::unix::fs::symlink("/external", old.join("link")).unwrap();
    let sources = vec![
        source(root.path(), 0, &old, false),
        source(root.path(), 1, &old.join("sub"), true),
    ];
    let target = root.path().join("new");
    let state = root.path().join("state");
    let plan = folder_move::preview(sources.clone(), old.clone(), target.clone(), &state)
        .await
        .unwrap();
    assert_eq!(plan.entries.len(), 2);
    let result = execute(plan, &state, false).await;
    assert!(matches!(result, Outcome::Complete(_)), "{result:?}");
    assert!(!old.exists());
    assert_eq!(
        fs::read_to_string(target.join(".hidden")).unwrap(),
        "content"
    );
    assert_eq!(
        fs::read_link(target.join("link")).unwrap(),
        Path::new("/external")
    );
    assert_eq!(read(&sources[0])["cwd"], target.to_str().unwrap());
    assert_eq!(
        read(&sources[1])["cwd"],
        target.join("sub").to_str().unwrap()
    );
    assert_eq!(read(&sources[1])["archived"], true);
    let path = state.join("folder-moves/current.json");
    let mut j: folder_move::Journal = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    j.phase = "verifying".into();
    fs::write(&path, serde_json::to_vec(&j).unwrap()).unwrap();
    let result = execute(j.plan, &state, true).await;
    assert!(matches!(result, Outcome::Complete(_)), "{result:?}");
    assert!(folder_move::pending(&state).unwrap().is_none());
}
#[tokio::test]
async fn partial_updates_resume_without_renaming_again() {
    let root = tempfile::tempdir().unwrap();
    let old = root.path().join("old");
    fs::create_dir(&old).unwrap();
    let sources = vec![
        source(root.path(), 0, &old, false),
        source(root.path(), 1, &old, true),
    ];
    change(&sources[1], "reject_update", json!(true));
    let target = root.path().join("new");
    let state = root.path().join("state");
    let plan = folder_move::preview(sources.clone(), old.clone(), target.clone(), &state)
        .await
        .unwrap();
    let failed = execute(plan, &state, false).await;
    assert!(matches!(failed, Outcome::Failed { recovery: true, .. }));
    assert!(target.is_dir());
    assert!(!old.exists());
    assert_eq!(read(&sources[0])["cwd"], target.to_str().unwrap());
    assert_eq!(read(&sources[1])["cwd"], old.to_str().unwrap());
    assert_eq!(read(&sources[1])["archived"], true);
    change(&sources[1], "reject_update", json!(false));
    let j = folder_move::pending(&state).unwrap().unwrap();
    let result = execute(j.plan, &state, true).await;
    assert!(matches!(result, Outcome::Complete(_)), "{result:?}");
    assert_eq!(read(&sources[1])["cwd"], target.to_str().unwrap());
}
#[tokio::test]
async fn unsupported_and_stale_sessions_block_before_files_move() {
    for scenario in ["profile", "stale", "locked"] {
        let root = tempfile::tempdir().unwrap();
        let old = root.path().join("old");
        fs::create_dir(&old).unwrap();
        let sources = vec![source(root.path(), 0, &old, false)];
        let state = root.path().join("state");
        let target = root.path().join("new");
        if scenario == "profile" {
            change(&sources[0], "profile", json!("custom"));
            assert!(
                folder_move::preview(sources, old.clone(), target.clone(), &state)
                    .await
                    .is_err()
            );
        } else {
            let plan = folder_move::preview(sources.clone(), old.clone(), target.clone(), &state)
                .await
                .unwrap();
            let _lock = if scenario == "locked" {
                use std::os::fd::AsRawFd;
                let f = fs::File::create(sources[0].home.join("writer.lock")).unwrap();
                assert_eq!(
                    unsafe { libc::flock(f.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) },
                    0
                );
                Some(f)
            } else {
                change(&sources[0], "cwd", json!(root.path().join("elsewhere")));
                None
            };
            assert!(matches!(
                execute(plan, &state, false).await,
                Outcome::Failed { .. }
            ));
        }
        assert!(old.exists());
        assert!(!target.exists());
    }
}
#[tokio::test]
async fn zero_sessions_collisions_symlinks_and_protected_storage() {
    let root = tempfile::tempdir().unwrap();
    let old = root.path().join("old");
    fs::create_dir(&old).unwrap();
    let target = root.path().join("new");
    let state = root.path().join("state");
    assert!(
        folder_move::preview(vec![], old.clone(), old.join("child"), &state)
            .await
            .is_err()
    );
    assert!(
        folder_move::preview(vec![], old.clone(), target.clone(), &old.join("state"))
            .await
            .is_err()
    );
    let alias = root.path().join("alias");
    std::os::unix::fs::symlink(&old, &alias).unwrap();
    assert!(
        folder_move::preview(vec![], alias, target.clone(), &state)
            .await
            .is_err()
    );
    let plan = folder_move::preview(vec![], old.clone(), target.clone(), &state)
        .await
        .unwrap();
    fs::create_dir(&target).unwrap();
    assert!(matches!(
        execute(plan, &state, false).await,
        Outcome::Failed {
            recovery: false,
            ..
        }
    ));
    fs::remove_dir(&target).unwrap();
    let plan = folder_move::preview(vec![], old.clone(), target.clone(), &state)
        .await
        .unwrap();
    let result = execute(plan, &state, false).await;
    assert!(matches!(result, Outcome::Complete(_)), "{result:?}");
    assert!(target.exists());
    assert!(!old.exists());
    assert!(folder_move::destination(root.path(), "../escape").is_err());
}
#[tokio::test]
#[ignore = "Disposable native fixture, invoked by scripts/check_codex_folder_move.py"]
async fn native_folder_move() {
    let root = PathBuf::from(std::env::var_os("TRACR_FOLDER_FIXTURE").unwrap());
    let source = Source {
        binary: std::env::var_os("TRACR_FOLDER_CODEX").unwrap().into(),
        home: root.join("codex"),
        id: SourceId("native".into()),
    };
    let state = root.join("state-tracr");
    let plan = folder_move::preview(vec![source], root.join("old"), root.join("moved"), &state)
        .await
        .unwrap();
    assert_eq!(plan.entries.len(), 2);
    let scenario = std::env::var("TRACR_FOLDER_SCENARIO").unwrap_or_default();
    if scenario == "locked" {
        let result = execute(plan, &state, false).await;
        assert!(matches!(result, Outcome::Failed { .. }), "{result:?}");
        assert!(root.join("old").is_dir());
        assert!(!root.join("moved").exists());
        return;
    }
    if scenario == "interrupted" {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let task = tokio::spawn(folder_move::execute(
            plan.clone(),
            state.clone(),
            plan.sources.clone(),
            false,
            tx,
        ));
        while let Some(message) = rx.recv().await {
            if message.starts_with("Updating session 1/") {
                task.abort();
                break;
            }
        }
        let _ = task.await;
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        assert!(root.join("moved").is_dir());
        let j = folder_move::pending(&state)
            .unwrap()
            .expect("unfinished move");
        let result = execute(j.plan, &state, true).await;
        assert!(matches!(result, Outcome::Complete(_)), "{result:?}");
    } else {
        let result = execute(plan, &state, false).await;
        assert!(matches!(result, Outcome::Complete(_)), "{result:?}");
    }
    let path = state.join("folder-moves/current.json");
    let mut j: folder_move::Journal = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    j.phase = "verifying".into();
    fs::write(&path, serde_json::to_vec(&j).unwrap()).unwrap();
    let result = execute(j.plan, &state, true).await;
    assert!(matches!(result, Outcome::Complete(_)), "{result:?}");
}

#[tokio::test]
async fn rename_before_first_update_recovers_and_refuses_replaced_directory() {
    let root = tempfile::tempdir().unwrap();
    let old = root.path().join("old");
    fs::create_dir(&old).unwrap();
    let sources = vec![source(root.path(), 0, &old, false)];
    let state = root.path().join("state");
    let target = root.path().join("new");
    change(&sources[0], "reject_update", json!(true));
    let plan = folder_move::preview(sources.clone(), old.clone(), target.clone(), &state)
        .await
        .unwrap();
    assert!(matches!(
        execute(plan, &state, false).await,
        Outcome::Failed { recovery: true, .. }
    ));
    assert_eq!(read(&sources[0])["cwd"], old.to_str().unwrap());
    let j = folder_move::pending(&state).unwrap().unwrap();
    let saved = root.path().join("saved");
    fs::rename(&target, &saved).unwrap();
    fs::create_dir(&target).unwrap();
    assert!(matches!(
        execute(j.plan.clone(), &state, true).await,
        Outcome::Failed { recovery: true, .. }
    ));
    assert_eq!(read(&sources[0])["cwd"], old.to_str().unwrap());
    fs::remove_dir(&target).unwrap();
    fs::rename(&saved, &target).unwrap();
    change(&sources[0], "reject_update", json!(false));
    let result = execute(j.plan, &state, true).await;
    assert!(matches!(result, Outcome::Complete(_)), "{result:?}");
}

#[tokio::test]
async fn cross_filesystem_and_busy_coordinator_are_rejected() {
    use std::os::{fd::AsRawFd, unix::fs::MetadataExt};
    let root = tempfile::tempdir().unwrap();
    let old = root.path().join("old");
    fs::create_dir(&old).unwrap();
    let state = root.path().join("state");
    fs::create_dir(&state).unwrap();
    if let Ok(meta) = fs::metadata("/dev/shm") {
        if meta.dev() != fs::metadata(&old).unwrap().dev() {
            let target =
                Path::new("/dev/shm").join(format!("tracr-no-create-{}", std::process::id()));
            let error = folder_move::preview(vec![], old.clone(), target, &state)
                .await
                .unwrap_err();
            assert!(error.contains("filesystems"));
        }
    }
    let plan = folder_move::preview(vec![], old.clone(), root.path().join("new"), &state)
        .await
        .unwrap();
    let guard = fs::File::create(state.join("relocation.lock")).unwrap();
    assert_eq!(
        unsafe { libc::flock(guard.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) },
        0
    );
    assert!(matches!(
        execute(plan, &state, false).await,
        Outcome::Failed {
            recovery: false,
            ..
        }
    ));
    assert!(old.exists());
}

#[test]
fn destination_input_distinguishes_nesting_from_rename() {
    let root = tempfile::tempdir().unwrap();
    let old = root.path().join("项目 old");
    let parent = root.path().join("archive");
    std::fs::create_dir(&old).unwrap();
    std::fs::create_dir(&parent).unwrap();
    let resolve = |input: &str| folder_move::resolve_destination(input, &old, root.path());
    assert_eq!(resolve("archive/").unwrap(), parent.join("项目 old"));
    assert_eq!(
        resolve("archive/new name").unwrap(),
        parent.join("new name")
    );
    assert_eq!(resolve("renamed").unwrap(), root.path().join("renamed"));
    assert_eq!(
        resolve("/").unwrap(),
        std::path::Path::new("/").join("项目 old")
    );
    assert_eq!(
        resolve(&format!("{}/", parent.display())).unwrap(),
        parent.join("项目 old")
    );
    // Existing destinations remain exact; preview rejects them instead of nesting.
    assert_eq!(resolve("archive").unwrap(), parent);
    assert!(resolve("").is_err());
    assert!(resolve("missing/new").is_err());
    assert!(resolve("archive/..").is_err());
    assert!(resolve("archive/.").is_err());
    let home = dirs::home_dir().unwrap();
    assert_eq!(
        resolve("~/").unwrap(),
        std::fs::canonicalize(home).unwrap().join("项目 old")
    );
}
