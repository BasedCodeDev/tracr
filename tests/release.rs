use serde_json::json;
use std::path::{Path, PathBuf};
use tracr::{
    config::SourceConfig,
    domain::*,
    providers::{
        SessionProvider,
        codex::CodexProvider,
        codex_release::{ReleaseOutcome, release},
    },
    relocation,
};

async fn fixture(
    extra: serde_json::Value,
) -> (
    tempfile::TempDir,
    PathBuf,
    SourceConfig,
    relocation::Preview,
) {
    let root = tempfile::tempdir().unwrap();
    let old = root.path().join("old");
    let target = root.path().join("new");
    std::fs::create_dir(&old).unwrap();
    std::fs::create_dir(&target).unwrap();
    let source = SourceConfig {
        id: SourceId("release-test".into()),
        home: root.path().into(),
    };
    let mut state = json!({"cwd":old,"loaded":true});
    for (k, v) in extra.as_object().unwrap() {
        state[k] = v.clone();
    }
    std::fs::write(root.path().join("native.json"), state.to_string()).unwrap();
    let binary =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/app-server/release_fake.py");
    let provider = CodexProvider::connect(&binary, source.clone())
        .await
        .unwrap();
    let key = SessionKey {
        provider: ProviderId("codex".into()),
        source_id: source.id.clone(),
        native_id: "session".into(),
    };
    let session = provider.read(&key).await.unwrap().unwrap();
    provider.close().await;
    let preview = relocation::preview(&binary, &source, &session, target)
        .await
        .unwrap();
    (root, binary, source, preview)
}

#[tokio::test]
async fn release_distinguishes_detached_attached_busy_and_unavailable() {
    for (extra, expected) in [
        (json!({}), ReleaseOutcome::Released),
        (json!({"loaded":false}), ReleaseOutcome::AlreadyUnloaded),
        (json!({"attached":true}), ReleaseOutcome::StillLoaded),
        (
            json!({"status":{"type":"active","activeFlags":[]}}),
            ReleaseOutcome::Busy,
        ),
        (
            json!({"status":{"type":"active","activeFlags":["waitingOnApproval"]}}),
            ReleaseOutcome::Busy,
        ),
        (json!({"unavailable":true}), ReleaseOutcome::Unavailable),
    ] {
        let (root, binary, source, p) = fixture(extra).await;
        assert_eq!(
            release(&binary, &source, &p, false).await.unwrap(),
            expected
        );
        let log =
            std::fs::read_to_string(root.path().join("live_requests.jsonl")).unwrap_or_default();
        let mut methods = vec![];
        for line in log.lines() {
            let v: serde_json::Value = serde_json::from_str(line).unwrap();
            methods.push(v["method"].as_str().unwrap().to_owned());
        }
        assert!(
            !methods.iter().any(|m| m == "turn/start"
                || m == "thread/archive"
                || m == "thread/settings/update")
        );
        assert_eq!(
            methods.iter().any(|m| m == "thread/resume"),
            matches!(
                expected,
                ReleaseOutcome::Released | ReleaseOutcome::StillLoaded
            )
        );
    }
}
#[tokio::test]
async fn unsafe_release_is_blocked_before_resume() {
    for extra in [json!({"terminals":true}), json!({"archived":true})] {
        // Build preview unarchived, then change archive state to exercise fresh validation.
        let (root, binary, source, p) = fixture(json!({})).await;
        let file = root.path().join("native.json");
        let mut state: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&file).unwrap()).unwrap();
        for (k, v) in extra.as_object().unwrap() {
            state[k] = v.clone();
        }
        std::fs::write(file, state.to_string()).unwrap();
        assert!(release(&binary, &source, &p, false).await.is_err());
        assert!(
            !std::fs::read_to_string(root.path().join("live_requests.jsonl"))
                .unwrap()
                .contains("thread/resume")
        );
    }
}
#[tokio::test]
async fn confirmed_relocation_releases_idle_before_writer_acquisition() {
    let (root, binary, source, p) = fixture(json!({})).await;
    let outcome = relocation::execute(
        &binary,
        &source,
        &root.path().join("state"),
        p.clone(),
        false,
    )
    .await;
    assert!(
        matches!(outcome, relocation::Outcome::Verified(_)),
        "{outcome:?}"
    );
    let state: serde_json::Value =
        serde_json::from_slice(&std::fs::read(root.path().join("native.json")).unwrap()).unwrap();
    assert_eq!(state["cwd"].as_str(), p.target.to_str());
    assert_eq!(state["loaded"], false);
}

#[tokio::test]
#[ignore = "Disposable native acceptance invoked by scripts/check_codex_release.py"]
async fn real_cached_idle_release() {
    let root = PathBuf::from(std::env::var_os("TRACR_RELEASE_FIXTURE").unwrap());
    let binary = root.join("codex-proxy");
    let source = SourceConfig {
        id: SourceId("release-native".into()),
        home: root.join("codex"),
    };
    let record: serde_json::Value =
        serde_json::from_slice(&std::fs::read(root.join("release.json")).unwrap()).unwrap();
    let provider = CodexProvider::connect(&binary, source.clone())
        .await
        .unwrap();
    let key = SessionKey {
        provider: ProviderId("codex".into()),
        source_id: source.id.clone(),
        native_id: record["id"].as_str().unwrap().into(),
    };
    let session = provider.read(&key).await.unwrap().unwrap();
    provider.close().await;
    let p = relocation::preview(&binary, &source, &session, root.join("new folder"))
        .await
        .unwrap();
    if record["attached"] == true {
        assert_eq!(
            release(&binary, &source, &p, false).await.unwrap(),
            ReleaseOutcome::StillLoaded
        );
        return;
    }
    if record["folder"] == true {
        std::fs::remove_dir(root.join("new folder")).unwrap();
        let sources = vec![tracr::folder_move::Source {
            binary: binary.clone(),
            home: source.home.clone(),
            id: source.id.clone(),
        }];
        let state = root.join("tracr-state");
        let plan =
            tracr::folder_move::preview(sources.clone(), p.old.clone(), p.target.clone(), &state)
                .await
                .unwrap();
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let result = tracr::folder_move::execute(plan, state, sources, false, tx).await;
        assert!(
            matches!(result, tracr::folder_move::Outcome::Complete(_)),
            "{result:?}"
        );
    } else {
        let outcome =
            relocation::execute(&binary, &source, &root.join("tracr-state"), p, false).await;
        assert!(
            matches!(outcome, relocation::Outcome::Verified(_)),
            "{outcome:?}"
        );
    }
}

#[tokio::test]
async fn attached_or_racing_owner_prevents_directory_change() {
    for extra in [json!({"attached":true}), json!({"reattach":true})] {
        let (root, binary, source, p) = fixture(extra).await;
        let result = relocation::execute(
            &binary,
            &source,
            &root.path().join("state"),
            p.clone(),
            false,
        )
        .await;
        assert!(
            matches!(result, relocation::Outcome::Rejected(_)),
            "{result:?}"
        );
        let state: serde_json::Value =
            serde_json::from_slice(&std::fs::read(root.path().join("native.json")).unwrap())
                .unwrap();
        assert_eq!(state["cwd"].as_str(), p.old.to_str());
        let log = std::fs::read_to_string(root.path().join("requests.jsonl")).unwrap();
        assert!(!log.contains("thread/settings/update"));
    }
}

#[tokio::test]
async fn cancellation_and_timeout_close_only_the_operational_proxy() {
    for cancel in [true, false] {
        let (root, binary, source, p) = fixture(json!({"stall":true})).await;
        let task = tokio::spawn(async move { release(&binary, &source, &p, false).await });
        let pid_file = root.path().join("proxy.pid");
        for _ in 0..100 {
            if std::fs::read_to_string(root.path().join("live_requests.jsonl"))
                .unwrap_or_default()
                .contains("thread/resume")
            {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        let pid: i32 = std::fs::read_to_string(pid_file).unwrap().parse().unwrap();
        if cancel {
            task.abort();
            assert!(task.await.unwrap_err().is_cancelled());
        } else {
            assert!(task.await.unwrap().unwrap_err().contains("Timed out"));
        }
        for _ in 0..100 {
            if unsafe { libc::kill(pid, 0) } != 0 {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        assert_ne!(
            unsafe { libc::kill(pid, 0) },
            0,
            "proxy survived cancellation/timeout: {:?}",
            std::fs::read_to_string(format!("/proc/{pid}/status"))
        );
        let log = std::fs::read_to_string(root.path().join("requests.jsonl")).unwrap();
        assert!(!log.contains("thread/settings/update"));
    }
}

#[tokio::test]
async fn recovery_releases_idle_session_at_the_recorded_destination() {
    let (root, binary, source, p) = fixture(json!({})).await;
    let state = root.path().join("state");
    assert!(matches!(
        relocation::execute(&binary, &source, &state, p.clone(), false).await,
        relocation::Outcome::Verified(_)
    ));
    let journals = std::fs::read_dir(state.join("relocations")).unwrap();
    let path = journals
        .map(|e| e.unwrap().path())
        .find(|p| p.extension().is_some_and(|e| e == "json"))
        .unwrap();
    let mut journal: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    journal["phase"] = json!("verifying");
    std::fs::write(path, journal.to_string()).unwrap();
    let file = root.path().join("native.json");
    let mut native: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&file).unwrap()).unwrap();
    native["loaded"] = json!(true);
    std::fs::write(file, native.to_string()).unwrap();
    let outcome = relocation::execute(&binary, &source, &state, p, true).await;
    assert!(
        matches!(outcome, relocation::Outcome::Verified(_)),
        "{outcome:?}"
    );
    assert!(relocation::pending(&state).unwrap().is_empty());
}

#[tokio::test]
async fn attached_session_blocks_folder_rename() {
    let (root, binary, source, p) = fixture(json!({"attached":true})).await;
    std::fs::remove_dir(&p.target).unwrap();
    let sources = vec![tracr::folder_move::Source {
        binary,
        home: source.home,
        id: source.id,
    }];
    let state = root.path().join("state");
    let plan =
        tracr::folder_move::preview(sources.clone(), p.old.clone(), p.target.clone(), &state)
            .await
            .unwrap();
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let outcome = tracr::folder_move::execute(plan, state, sources, false, tx).await;
    assert!(
        matches!(
            outcome,
            tracr::folder_move::Outcome::Failed {
                recovery: false,
                ..
            }
        ),
        "{outcome:?}"
    );
    assert!(p.old.is_dir());
    assert!(!p.target.exists());
}

#[tokio::test]
async fn stale_metadata_blocks_release_before_runtime_replacement() {
    let (root, binary, source, mut preview) = fixture(json!({})).await;
    preview.fingerprint["updatedAt"] = json!(0);
    assert!(
        release(&binary, &source, &preview, false)
            .await
            .unwrap_err()
            .contains("changed before release")
    );
    assert!(
        !std::fs::read_to_string(root.path().join("live_requests.jsonl"))
            .unwrap()
            .contains("thread/resume")
    );
}
