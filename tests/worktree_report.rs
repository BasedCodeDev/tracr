use serde_json::{Value, json};
use std::{
    path::{Path, PathBuf},
    process::{Command, Output},
};
fn git(path: &Path, args: &[&str]) {
    let out = Command::new("git")
        .arg("-C")
        .arg(path)
        .args(args)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
}
fn init(path: &Path) {
    std::fs::create_dir_all(path).unwrap();
    git(path, &["init", "-qb", "main"]);
}
fn home(root: &Path, name: &str, settings: Value) -> PathBuf {
    let path = root.join(name);
    std::fs::create_dir(&path).unwrap();
    std::fs::write(path.join("fake.json"), settings.to_string()).unwrap();
    path
}
fn run(path: &Path, homes: &[&Path], extra: &[&str]) -> Output {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_tracr"));
    cmd.current_dir(path)
        .env("XDG_CONFIG_HOME", path.join("no-config"))
        .arg("--codex-bin")
        .arg(Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/app-server/fake.py"));
    for home in homes {
        cmd.arg("--codex-home").arg(home);
    }
    cmd.arg("worktrees").args(extra).output().unwrap()
}
fn report(out: Output, code: i32) -> Value {
    assert_eq!(
        out.status.code(),
        Some(code),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_slice(&out.stdout).unwrap()
}
#[test]
fn inventory_matches_real_ownership_and_preserves_live_states() {
    let temp = tempfile::tempdir().unwrap();
    let main = temp.path().join("main");
    init(&main);
    git(
        &main,
        &[
            "-c",
            "user.name=Test",
            "-c",
            "user.email=t@example.invalid",
            "commit",
            "--allow-empty",
            "-qm",
            "initial",
        ],
    );
    let linked = temp.path().join("linked");
    git(
        &main,
        &[
            "worktree",
            "add",
            "-qb",
            "feature",
            linked.to_str().unwrap(),
        ],
    );
    let detached = temp.path().join("detached");
    git(
        &main,
        &["worktree", "add", "--detach", detached.to_str().unwrap()],
    );
    let sub = linked.join("sub");
    std::fs::create_dir(&sub).unwrap();
    let nested = main.join("nested");
    init(&nested);
    let alias = temp.path().join("alias");
    std::os::unix::fs::symlink(&sub, &alias).unwrap();
    let h = home(
        temp.path(),
        "home",
        json!({"live_threads": {
            "running":{"type":"active","activeFlags":[]}, "waiting":{"type":"active","activeFlags":["waitingOnApproval","waitingOnUserInput"]},
            "idle":{"type":"idle"}, "error":{"type":"systemError"}, "unknown":{"type":"future"}, "gone":{"type":"notLoaded"}, "nested":{"type":"idle"}
        }, "live_metadata": {
            "running":{"cwd":sub, "name":"Work\u{001b}[31m"}, "waiting":{"cwd":alias}, "idle":{"cwd":main}, "error":{"cwd":linked}, "unknown":{"cwd":linked}, "gone":{"cwd":linked}, "nested":{"cwd":nested}
        }}),
    );
    let h2 = home(
        temp.path(),
        "home2",
        json!({"live_threads":{"idle":{"type":"idle"}}, "live_metadata":{"idle":{"cwd":main}}}),
    );
    let result = report(run(&main, &[&h, &h2], &[]), 0);
    assert_eq!(result["complete"], true);
    assert_eq!(result["schema_version"], 1);
    assert_eq!(result["worktrees"].as_array().unwrap().len(), 3);
    let trees = result["worktrees"].as_array().unwrap();
    let feature = trees
        .iter()
        .find(|t| t["branch"] == "refs/heads/feature")
        .unwrap();
    let agents = feature["agents"].as_array().unwrap();
    assert_eq!(agents.len(), 4);
    assert_eq!(
        agents
            .iter()
            .find(|a| a["key"]["native_id"] == "waiting")
            .unwrap()["status"],
        json!({"status":"waiting","approval":true,"input":true})
    );
    assert_eq!(
        trees
            .iter()
            .find(|t| t["branch"] == "refs/heads/main")
            .unwrap()["agents"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    assert!(
        trees
            .iter()
            .any(|t| t["branch"].is_null() && t["agents"] == json!([]))
    );
    for cwd in [&linked, &sub, &alias] {
        assert_eq!(report(run(cwd, &[&h, &h2], &[]), 0), result);
    }
    let text = run(&main, &[&h], &["--text"]);
    assert!(text.status.success());
    assert!(!text.stdout.contains(&27));
    let log = std::fs::read_to_string(h.join("live_requests.jsonl")).unwrap();
    assert!(!log.contains("thread/resume"));
    assert!(!log.contains("turn/start"));
    assert!(!log.contains("thread/list"));
}
#[test]
fn failures_preserve_inventory_and_successful_sources() {
    let temp = tempfile::tempdir().unwrap();
    let repo = temp.path().join("repo");
    init(&repo);
    let good = home(
        temp.path(),
        "good",
        json!({"live_threads":{"a":{"type":"idle"}},"live_metadata":{"a":{"cwd":repo}}}),
    );
    let bad = home(
        temp.path(),
        "bad",
        json!({"live_enabled":true,"live_disconnect":true}),
    );
    let out = report(run(&repo, &[&good, &bad], &[]), 1);
    assert_eq!(out["complete"], false);
    assert!(out["worktrees"][0]["head"].is_null());
    assert_eq!(out["worktrees"][0]["agents"].as_array().unwrap().len(), 1);
    assert_eq!(out["sources"][1]["complete"], false);
    std::fs::write(good.join("fake.json"), json!({"live_threads":{"a":{"type":"idle"}},"live_metadata":{"a":{"cwd":repo.join("missing")}}}).to_string()).unwrap();
    assert_eq!(report(run(&repo, &[&good], &[]), 1)["complete"], false);
    let outside = report(run(temp.path(), &[&good], &[]), 1);
    assert!(outside["repository"].is_null());
    assert!(outside["error"].is_string());
    assert_eq!(run(&repo, &[&good], &["--invalid"]).status.code(), Some(2));
}
#[test]
fn bare_and_prunable_entries_are_retained() {
    let temp = tempfile::tempdir().unwrap();
    let repo = temp.path().join("bare");
    std::fs::create_dir(&repo).unwrap();
    git(&repo, &["init", "--bare", "-q"]);
    let h = home(temp.path(), "home", json!({"live_enabled":true}));
    assert_eq!(
        report(run(&repo, &[&h], &[]), 0)["worktrees"][0]["bare"],
        true
    );
    let main = temp.path().join("main");
    init(&main);
    git(
        &main,
        &[
            "-c",
            "user.name=T",
            "-c",
            "user.email=t@x",
            "commit",
            "--allow-empty",
            "-qm",
            "initial",
        ],
    );
    let linked = temp.path().join("linked");
    git(
        &main,
        &[
            "worktree",
            "add",
            "-qb",
            "feature",
            linked.to_str().unwrap(),
        ],
    );
    std::fs::remove_dir_all(&linked).unwrap();
    let out = report(run(&main, &[&h], &[]), 0);
    assert!(
        out["worktrees"]
            .as_array()
            .unwrap()
            .iter()
            .any(|w| w["prunable"] == true)
    );
}

#[test]
fn connection_timeout_and_configuration_errors_are_structured() {
    let temp = tempfile::tempdir().unwrap();
    let repo = temp.path().join("repo");
    init(&repo);
    let h = home(
        temp.path(),
        "home",
        json!({"live_enabled":true,"live_delay":6}),
    );
    let out = report(run(&repo, &[&h], &[]), 1);
    assert!(
        out["sources"][0]["errors"][0]
            .as_str()
            .unwrap()
            .contains("timed out")
    );
    let out = report(
        run(
            &repo,
            &[&h],
            &["--config", "/nonexistent/tracr-config.toml"],
        ),
        1,
    );
    assert!(out["error"].is_string());
    std::fs::write(
        h.join("fake.json"),
        json!({"live_threads":{"a":{"type":"idle"}}}).to_string(),
    )
    .unwrap();
    let out = report(run(&repo, &[&h], &[]), 1);
    assert!(
        out["sources"][0]["errors"][0]
            .as_str()
            .unwrap()
            .contains("missing session directory")
    );
}
