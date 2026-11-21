use serde_json::{Value, json};
use std::{path::Path, process::Command};

fn run(root: &Path, homes: &[&Path], id: &str, extra: &[&str]) -> (i32, Value) {
    run_binary(root, homes, id, extra, "relink_fake.py")
}
fn run_binary(
    root: &Path,
    homes: &[&Path],
    id: &str,
    extra: &[&str],
    binary: &str,
) -> (i32, Value) {
    let mut command = Command::new(env!("CARGO_BIN_EXE_tracr"));
    command
        .current_dir(root)
        .env("XDG_CONFIG_HOME", root.join("config"))
        .env("XDG_STATE_HOME", root.join("state"))
        .arg("--codex-bin")
        .arg(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("tests/fixtures/app-server")
                .join(binary),
        );
    for home in homes {
        command.arg("--codex-home").arg(home);
    }
    if extra.contains(&"--live") && homes[0].ends_with("missing-agent") {
        command.env("TRACR_TEST_VERSION_QUERY_FAIL", "1");
    }
    let output = command
        .arg("move-session")
        .arg(id)
        .arg(root.join("new"))
        .args(extra)
        .output()
        .unwrap();
    let value = serde_json::from_slice(&output.stdout).unwrap_or_else(|_| {
        panic!(
            "{} {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        )
    });
    (output.status.code().unwrap(), value)
}
fn home(root: &Path, name: &str, extra: Value) -> std::path::PathBuf {
    let home = root.join(name);
    std::fs::create_dir(&home).unwrap();
    let mut state = json!({"cwd":root.join("old")});
    state
        .as_object_mut()
        .unwrap()
        .extend(extra.as_object().unwrap().clone());
    std::fs::write(home.join("native.json"), state.to_string()).unwrap();
    home
}
fn setup() -> tempfile::TempDir {
    let temp = tempfile::tempdir().unwrap();
    for dir in ["old", "new"] {
        std::fs::create_dir(temp.path().join(dir)).unwrap();
    }
    temp
}
#[test]
fn moves_archived_and_unarchived_sessions_and_is_repeatable() {
    for archived in [false, true] {
        let temp = setup();
        let root = temp.path();
        let home = home(root, "codex", json!({"archived":archived}));
        let (code, report) = run(root, &[&home], "session", &["--dry-run"]);
        assert_eq!(code, 0, "{report}");
        assert_eq!(report["status"], "preview");
        let initial: Value =
            serde_json::from_slice(&std::fs::read(home.join("native.json")).unwrap()).unwrap();
        assert_eq!(initial["cwd"], root.join("old").to_str().unwrap());
        let (code, report) = run(
            root,
            &[&home],
            "session",
            &["--from", root.join("old").to_str().unwrap()],
        );
        assert_eq!(code, 0, "{report}");
        assert_eq!(report["status"], "moved");
        let state: Value =
            serde_json::from_slice(&std::fs::read(home.join("native.json")).unwrap()).unwrap();
        assert_eq!(state["cwd"], root.join("new").to_str().unwrap());
        assert_eq!(state["archived"], archived);
        assert_eq!(
            run(root, &[&home], "session", &[]).1["status"],
            "already_at_destination"
        );
        assert!(root.join("old").is_dir());
    }
}
#[test]
fn rejects_ambiguous_missing_and_stale_selection_without_updates() {
    let temp = setup();
    let root = temp.path();
    let a = home(root, "a", json!({}));
    let b = home(root, "b", json!({}));
    for (homes, id, flags) in [
        (vec![a.as_path(), b.as_path()], "session", vec![]),
        (vec![a.as_path()], "missing", vec![]),
        (vec![a.as_path()], "session", vec!["--from", "/wrong"]),
    ] {
        let (code, report) = run(root, &homes, id, &flags);
        assert_eq!(code, 1, "{report}");
        assert_eq!(report["status"], "rejected");
    }
    assert!(
        !std::fs::read_to_string(a.join("requests.jsonl"))
            .unwrap()
            .contains("thread/settings/update")
    );
}
#[test]
fn live_without_a_shared_daemon_never_falls_back_to_offline_move() {
    let temp = setup();
    let root = temp.path();
    let home = home(root, "codex", json!({}));
    let (code, report) = run(root, &[&home], "session", &["--live"]);
    assert_eq!(code, 1);
    assert_eq!(report["status"], "rejected");
    assert!(report["error"].is_string());
    assert!(report["journal"].is_null());
    assert!(
        !std::fs::read_to_string(home.join("requests.jsonl"))
            .unwrap()
            .contains("thread/settings/update")
    );
}
#[test]
fn writer_conflict_is_reported_and_directory_is_unchanged() {
    let temp = setup();
    let root = temp.path();
    let home = home(root, "codex", json!({"loaded":true}));
    let (code, report) = run(root, &[&home], "session", &[]);
    assert_eq!(code, 1, "{report}");
    let state: Value =
        serde_json::from_slice(&std::fs::read(home.join("native.json")).unwrap()).unwrap();
    assert_eq!(state["cwd"], root.join("old").to_str().unwrap());
}

#[test]
fn live_closeout_moves_with_mismatched_or_unavailable_versions() {
    for scenario in ["normal", "missing-agent"] {
        let temp = setup();
        let root = temp.path();
        let home = home(root, scenario, json!({"scenario":scenario}));
        let from = root.join("old");
        let preview = ["--live", "--from", from.to_str().unwrap(), "--dry-run"];
        let run =
            |flags: &[&str]| run_binary(root, &[&home], "session", flags, "live_move_fake.py");
        let (code, report) = run(&preview);
        assert_eq!(code, 0, "{scenario}: {report}");
        assert_eq!(report["schema_version"], 1);
        assert_eq!(report["status"], "preview");
        assert!(report["journal"].is_null());
        assert!(
            !std::fs::read_to_string(home.join("live_requests.jsonl"))
                .unwrap()
                .contains("thread/settings/update")
        );

        let (code, report) = run(&preview[..3]);
        assert_eq!(code, 0, "{scenario}: {report}");
        assert_eq!(report["status"], "moved");
        assert_eq!(report["destination"], root.join("new").to_str().unwrap());
        assert!(report["error"].is_null());
        let journal: Value =
            serde_json::from_slice(&std::fs::read(report["journal"].as_str().unwrap()).unwrap())
                .unwrap();
        assert_eq!(journal["phase"], "complete");

        let (code, report) = run(&preview[..3]);
        assert_eq!(code, 0, "{scenario}: {report}");
        assert_eq!(report["status"], "already_at_destination");
    }
}

#[test]
fn live_faults_distinguish_preflight_rejection_from_uncertain_writes() {
    for (scenario, error, attempted, changed) in [
        (
            "missing-home",
            "did not report its Codex home",
            false,
            false,
        ),
        ("wrong-home", "different Codex home", false, false),
        ("unloaded", "already loaded", false, false),
        ("extra-root", "Multi-root", false, false),
        (
            "preflight-error",
            "injected missing resume method",
            false,
            false,
        ),
        (
            "update-error",
            "injected missing update method",
            true,
            false,
        ),
        ("lost-ack", "Live directory update failed", true, true),
        ("readback-error", "injected readback failure", true, true),
        ("settings-race", "Settings changed", true, true),
    ] {
        let temp = setup();
        let root = temp.path();
        let home = home(root, "codex", json!({"scenario":scenario}));
        let (code, report) =
            run_binary(root, &[&home], "session", &["--live"], "live_move_fake.py");
        let state: Value =
            serde_json::from_slice(&std::fs::read(home.join("native.json")).unwrap()).unwrap();
        assert_eq!(code, 1, "{scenario}: {report}");
        assert_eq!(report["schema_version"], 1);
        assert_eq!(
            report["status"],
            if attempted { "unresolved" } else { "rejected" },
            "{scenario}: {report}"
        );
        assert!(
            report["error"].as_str().unwrap().contains(error),
            "{scenario}: {report}"
        );
        assert_eq!(
            state["cwd"],
            root.join(if changed { "new" } else { "old" })
                .to_str()
                .unwrap(),
        );
        let requests = std::fs::read_to_string(home.join("live_requests.jsonl")).unwrap();
        assert_eq!(
            requests.contains("thread/settings/update"),
            attempted,
            "{scenario}"
        );
        if attempted {
            let journal: Value = serde_json::from_slice(
                &std::fs::read(report["journal"].as_str().unwrap()).unwrap(),
            )
            .unwrap();
            assert_eq!(journal["phase"], "updating");
        } else {
            assert!(report["journal"].is_null());
        }
    }
}
