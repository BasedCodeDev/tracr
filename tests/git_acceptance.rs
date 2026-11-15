//! Real repositories exercise ownership projection rather than hand-built inventories.
use std::{
    path::{Path, PathBuf},
    process::Command,
    time::Duration,
};
use tracr::{
    browser::{Browser, FolderFilter, SourceSpec, Update},
    domain::SessionSummary,
    git_discovery::Controller,
};

fn git(path: &Path, args: &[&str]) {
    let output = Command::new("git")
        .arg("-C")
        .arg(path)
        .args([
            "-c",
            "protocol.file.allow=always",
            "-c",
            "user.name=Acceptance",
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
}
fn init(path: &Path) {
    std::fs::create_dir_all(path).unwrap();
    git(path, &["init", "-qb", "main"]);
    git(path, &["commit", "--allow-empty", "-qm", "Initial"]);
}
#[tokio::test]
async fn native_nested_submodule_clone_and_bare_worktree_scopes() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let project = root.join("project");
    init(&project);
    let src = project.join("src");
    std::fs::create_dir(&src).unwrap();
    let nested = project.join("nested");
    init(&nested);
    let origin = root.join("module-origin");
    init(&origin);
    let module = project.join("module");
    git(
        &project,
        &["submodule", "add", "-q", origin.to_str().unwrap(), "module"],
    );
    let clone = root.join("independent");
    git(
        root,
        &[
            "clone",
            "-q",
            project.to_str().unwrap(),
            clone.to_str().unwrap(),
        ],
    );
    let linked = root.join("external feature");
    git(
        &project,
        &[
            "worktree",
            "add",
            "-qb",
            "feature",
            linked.to_str().unwrap(),
        ],
    );
    let bare = root.join("bare.git");
    git(
        root,
        &[
            "clone",
            "--bare",
            "-q",
            project.to_str().unwrap(),
            bare.to_str().unwrap(),
        ],
    );
    let bare_linked = root.join("bare-linked");
    git(
        &bare,
        &[
            "worktree",
            "add",
            "-qb",
            "bare-feature",
            bare_linked.to_str().unwrap(),
        ],
    );
    let paths = [
        &project,
        &src,
        &nested,
        &module,
        &clone,
        &linked,
        &bare_linked,
    ];
    let template: serde_json::Value =
        serde_json::from_str(include_str!("fixtures/sessions/catalog.json")).unwrap();
    let records: Vec<SessionSummary> = paths
        .iter()
        .enumerate()
        .map(|(index, path)| {
            let mut row = template["sessions"][0].clone();
            row["key"]["native_id"] = index.to_string().into();
            row["cwd"] = path.to_str().unwrap().into();
            row["archived"] = false.into();
            serde_json::from_value(row).unwrap()
        })
        .collect();
    let mut browser = Browser::new(vec![SourceSpec::Fixture(PathBuf::from("unused"))]);
    browser.begin_refresh();
    browser.apply(Update::Page {
        generation: browser.generation,
        source: 0,
        sessions: records,
        warnings: vec![],
    });
    browser.apply(Update::Finished {
        generation: browser.generation,
        source: 0,
        error: None,
    });
    let mut controller = Controller::default();
    controller.generation(browser.git_generation);
    controller.set_candidates(
        paths
            .iter()
            .map(|path| (*path).clone())
            .chain([bare.clone()])
            .collect(),
    );
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let updates = controller.poll().await;
            controller.apply_batch(&mut browser, updates);
            if !controller.pending() && !controller.needs_poll() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(browser.repositories.len(), 5); // project, nested, submodule, clone, bare
    browser.open_folder(project.clone());
    assert_eq!(browser.visible().len(), 2);
    browser.open_folder(src.clone());
    assert_eq!(browser.visible().len(), 1);
    for path in [&nested, &module, &clone, &linked, &bare_linked] {
        browser.open_folder(path.clone());
        let visible = browser.visible();
        assert_eq!(visible.len(), 1, "{}", path.display());
        assert_eq!(visible[0].cwd.as_ref(), Some(path));
    }
    browser.open_folder(project.clone());
    browser.expanded.insert(bare.clone());
    let rows = browser.folder_rows();
    let row = |path: &Path| {
        rows.iter()
            .find(|row| row.filter == FolderFilter::Path(path.to_owned()))
            .unwrap()
    };
    assert_eq!(row(&project).count, 5); // includes nested projects and logical linked worktree once
    assert_eq!(row(&linked).count, 1);
    assert_eq!(row(&linked).depth, row(&project).depth + 1);
    assert_eq!(row(&bare_linked).depth, row(&bare).depth + 1);
    assert_eq!(
        rows.iter()
            .filter(|row| row.filter == FolderFilter::Path(linked.clone()))
            .count(),
        1
    );
    assert!(row(&linked).label.contains("feature"));
    // Selection preserves actual path identity, including the external and bare checkouts.
    for path in [&linked, &bare_linked] {
        browser.open_folder(path.clone());
        assert_eq!(browser.folder, FolderFilter::Path(path.clone()));
        assert_eq!(browser.visible()[0].cwd.as_ref(), Some(path));
    }
    controller.shutdown().await;
    tracr::git::drain_reads().await;
}
