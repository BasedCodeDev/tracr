use std::path::Path;
use tracr::{
    config::SourceConfig,
    domain::*,
    providers::{SessionProvider, codex::CodexProvider},
    relocation,
};

#[test]
fn destination_requires_a_directory_and_never_runs_expansion() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("space $literal");
    std::fs::create_dir(&path).unwrap();
    assert_eq!(
        relocation::destination("space $literal", root.path()).unwrap(),
        path
    );
    assert!(relocation::destination("$(touch unexpected)", root.path()).is_err());
    assert!(relocation::destination("", root.path()).is_err());
    let file = root.path().join("file");
    std::fs::write(&file, "data").unwrap();
    assert!(relocation::destination("file", root.path()).is_err());
    assert!(!root.path().join("unexpected").exists());
}

#[tokio::test]
#[ignore = "Disposable real-Codex acceptance; invoked by scripts/check_codex_relink.py"]
async fn real_native_relink() {
    let root = std::path::PathBuf::from(std::env::var_os("TRACR_RELINK_FIXTURE").expect("fixture"));
    let binary = std::path::PathBuf::from(std::env::var_os("TRACR_RELINK_CODEX").expect("binary"));
    let record: serde_json::Value =
        serde_json::from_slice(&std::fs::read(root.join("fixture.json")).unwrap()).unwrap();
    let source = SourceConfig {
        id: SourceId("test-source".into()),
        home: root.join("codex"),
    };
    let provider = CodexProvider::connect(&binary, source.clone())
        .await
        .unwrap();
    let key = SessionKey {
        provider: ProviderId("codex".into()),
        source_id: source.id.clone(),
        native_id: record["id"].as_str().unwrap().into(),
    };
    let mut session = provider.read(&key).await.unwrap().unwrap();
    session.archived = record["archived"].as_bool().unwrap();
    provider.close().await;
    let target = record["target"]
        .as_str()
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| root.join("new folder"));
    let preview = relocation::preview(&binary, &source, &session, target.clone())
        .await
        .unwrap();
    let state = root.join("tracr-state");
    let scenario = record["scenario"].as_str().unwrap_or("normal");
    if scenario == "stale" || scenario == "builtin-stale" {
        std::fs::remove_dir_all(&target).unwrap();
    }
    let outcome = relocation::execute(&binary, &source, &state, preview.clone(), false).await;
    if scenario == "stale" || scenario == "builtin-stale" {
        assert!(
            matches!(outcome, relocation::Outcome::Rejected(_)),
            "{outcome:?}"
        );
        return;
    }
    if scenario == "locked"
        || scenario == "builtin-locked"
        || scenario.starts_with("shared")
        || scenario == "custom"
    {
        assert!(
            !matches!(outcome, relocation::Outcome::Verified(_)),
            "{outcome:?}"
        );
        return;
    }
    assert!(
        matches!(outcome, relocation::Outcome::Verified(_)),
        "{outcome:?}"
    );
    assert!(relocation::pending(&state).unwrap().is_empty());
    // New provider instance must group this session at its native current directory.
    let provider = CodexProvider::connect(&binary, source.clone())
        .await
        .unwrap();
    let mut cursor = None;
    let mut matches = vec![];
    loop {
        let page = provider.list(cursor).await.unwrap();
        assert!(page.warnings.is_empty(), "{:?}", page.warnings);
        matches.extend(page.sessions.into_iter().filter(|s| s.key == key));
        cursor = page.next;
        if cursor.is_none() {
            break;
        }
    }
    assert_eq!(matches.len(), 1);
    assert_eq!(matches[0].cwd.as_deref(), Some(target.as_path()));
    assert_eq!(matches[0].archived, session.archived);
    provider.close().await;
    // Simulate a crash after applying the directory but before marking the journal complete.
    let journal = std::fs::read_dir(state.join("relocations"))
        .unwrap()
        .map(|e| e.unwrap().path())
        .find(|p| p.extension() == Some(std::ffi::OsStr::new("json")))
        .unwrap();
    let mut value: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&journal).unwrap()).unwrap();
    value["phase"] = "verifying".into();
    std::fs::write(&journal, serde_json::to_vec(&value).unwrap()).unwrap();
    let recovered = relocation::execute(&binary, &source, &state, preview, true).await;
    assert!(
        matches!(recovered, relocation::Outcome::Verified(_)),
        "{recovered:?}"
    );
    assert!(relocation::pending(&state).unwrap().is_empty());
    use std::os::unix::fs::PermissionsExt;
    assert_eq!(
        std::fs::metadata(journal).unwrap().permissions().mode() & 0o777,
        0o600
    );
    if scenario.starts_with("existing-") {
        assert_eq!(
            std::fs::read_to_string(root.join("old/sentinel")).unwrap(),
            "unchanged"
        );
    } else {
        assert!(Path::new(&root.join("old")).try_exists().is_ok_and(|e| !e));
    }
}

#[test]
fn relink_modal_has_separate_confirmation_and_ignores_escape_while_busy() {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use tracr::{
        browser::Browser,
        tui::{self, Action, RelinkModal, View},
    };
    let mut browser = Browser::new(vec![]);
    let session = SessionSummary {
        key: SessionKey {
            provider: ProviderId("codex".into()),
            source_id: SourceId("source".into()),
            native_id: "id".into(),
        },
        title: "Session".into(),
        cwd: Some("/missing".into()),
        created_at: 1,
        updated_at: 1,
        archived: false,
        can_accept_direct_input: None,
        source_kind: None,
        git: None,
    };
    let mut view = View {
        relink: Some(RelinkModal {
            session: session.clone(),
            input: String::new(),
            picker: None,
            preview: None,
            recovery: false,
            busy: false,
            executing: false,
            error: None,
        }),
        ..View::default()
    };
    let key = |code| KeyEvent::new(code, KeyModifiers::NONE);
    for c in "/new folder".chars() {
        assert_eq!(
            tui::handle_key(&mut browser, &mut view, key(KeyCode::Char(c))),
            Action::Continue
        );
    }
    assert_eq!(
        tui::handle_key(&mut browser, &mut view, key(KeyCode::Enter)),
        Action::PreviewRelink
    );
    let modal = view.relink.as_mut().unwrap();
    assert_eq!(modal.input, "/new folder");
    modal.busy = true;
    assert_eq!(
        tui::handle_key(&mut browser, &mut view, key(KeyCode::Esc)),
        Action::CancelRelink
    );
    view.relink.as_mut().unwrap().executing = true;
    assert_eq!(
        tui::handle_key(&mut browser, &mut view, key(KeyCode::Esc)),
        Action::Continue
    );
    assert!(view.relink.is_some());
    let modal = view.relink.as_mut().unwrap();
    modal.busy = false;
    modal.preview = Some(relocation::Preview {
        key: session.key,
        old: "/missing".into(),
        target: "/new folder".into(),
        archived: false,
        created_at: 1,
        version: "codex-cli 0.169.0".into(),
        fingerprint: serde_json::json!({}),
        profile: tracr::relocation_profile::Profile::Legacy,
    });
    assert_eq!(
        tui::handle_key(&mut browser, &mut view, key(KeyCode::Enter)),
        Action::ApplyRelink
    );
    assert_eq!(
        tui::handle_key(&mut browser, &mut view, key(KeyCode::Esc)),
        Action::Continue
    );
    assert!(view.relink.is_none());
}

#[test]
fn failure_dialog_is_explicit_scrollable_and_only_dismisses() {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use ratatui::{Terminal, backend::TestBackend};
    use tracr::{
        browser::Browser,
        tui::{self, Action, View},
    };
    let mut reports = Vec::new();
    for recovery in [false, true] {
        for (width, height) in [(100, 30), (36, 12), (24, 8)] {
            let mut browser = Browser::new(vec![]);
            let mut view = View {
                error_dialog: Some(tui::relink_error(
                    "Test this",
                    &format!("/missing/{}", "long path/".repeat(30)),
                    "/new folder",
                    &relocation::Failure {
                        kind: relocation::FailureKind::PermissionProfile,
                        terminals: None,
                        detail: "native profile".into(),
                    },
                    recovery,
                )),
                ..Default::default()
            };
            let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
            let render = |terminal: &mut Terminal<TestBackend>, view: &View| {
                terminal
                    .draw(|frame| tui::draw(frame, &browser, view))
                    .unwrap();
                terminal
                    .backend()
                    .buffer()
                    .content
                    .iter()
                    .map(|c| c.symbol())
                    .collect::<String>()
            };
            reports.push(view.error_dialog.as_ref().unwrap().report());
            let text = render(&mut terminal, &view);
            assert!(text.contains("! Relink failed"));
            if width >= 36 {
                assert!(text.contains("c Copy message"));
                assert!(text.contains("Esc close"));
            } else {
                assert!(text.contains("Esc close"));
                assert!(text.contains("Enlarge terminal"));
            }
            if width == 100 {
                assert!(text.contains(if recovery {
                    "may already have changed"
                } else {
                    "Its folder was not changed."
                }));
            }
            let key = |code| KeyEvent::new(code, KeyModifiers::NONE);
            // Even a recovery result requires explicit dismissal before R can act.
            assert_eq!(
                tui::handle_key(&mut browser, &mut view, key(KeyCode::Char('m'))),
                Action::Continue
            );
            for _ in 0..50 {
                tui::handle_key(&mut browser, &mut view, key(KeyCode::PageDown));
            }
            terminal
                .draw(|frame| tui::draw(frame, &browser, &view))
                .unwrap();
            let text: String = terminal
                .backend()
                .buffer()
                .content
                .iter()
                .map(|c| c.symbol())
                .collect();
            assert!(
                text.contains("native") && text.contains("profile"),
                "{width}x{height}: {text}"
            );
            assert!(text.contains("Esc close"));
            assert_eq!(
                view.error_dialog.as_ref().unwrap().report(),
                *reports.last().unwrap()
            );
            assert_eq!(
                tui::handle_key(&mut browser, &mut view, key(KeyCode::Char('c'))),
                Action::CopyErrorReport
            );
            let scroll = view.error_dialog.as_ref().unwrap().scroll;
            tui::handle_key(&mut browser, &mut view, key(KeyCode::Up));
            assert_eq!(
                view.error_dialog.as_ref().unwrap().scroll,
                scroll.saturating_sub(1)
            );
            assert_eq!(
                tui::handle_key(
                    &mut browser,
                    &mut view,
                    key(if recovery {
                        KeyCode::Esc
                    } else {
                        KeyCode::Enter
                    })
                ),
                Action::Continue
            );
            assert!(view.error_dialog.is_none());
        }
    }
    assert_eq!(reports[0], reports[1]);
    assert_eq!(reports[1], reports[2]);
    assert_eq!(reports[3], reports[4]);
    assert_eq!(reports[4], reports[5]);
}

#[test]
fn journals_read_legacy_records_and_exclude_rejections_from_recovery() {
    let root = tempfile::tempdir().unwrap();
    let dir = root.path().join("relocations");
    std::fs::create_dir(&dir).unwrap();
    let mut value = serde_json::json!({
        "format":1,"phase":"complete","settings":null,
        "preview":{"key":{"provider":"codex","source_id":"s","native_id":"n"},
        "old":"/missing","target":"/new","archived":false,"created_at":1,"version":"v","fingerprint":{}}
    });
    let path = dir.join("test.json");
    std::fs::write(&path, value.to_string()).unwrap();
    assert!(relocation::pending(root.path()).unwrap().is_empty());
    let legacy: relocation::Journal = serde_json::from_value(value.clone()).unwrap();
    assert!(legacy.failure.is_none());
    value["phase"] = "rejected".into();
    value["failure"] =
        serde_json::json!({"kind":"PermissionProfile","detail":"unsupported profile"});
    std::fs::write(&path, value.to_string()).unwrap();
    assert!(relocation::pending(root.path()).unwrap().is_empty());
    value["phase"] = "changing-directory".into();
    std::fs::write(&path, value.to_string()).unwrap();
    assert_eq!(relocation::pending(root.path()).unwrap().len(), 1);
}

#[tokio::test]
async fn dismissing_copy_aborts_pending_job_and_repeats_do_nothing() {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use tracr::{
        browser::Browser,
        tui::{self, Action, View},
    };
    let task = tokio::spawn(async { std::future::pending::<Result<(), String>>().await });
    let abort = task.abort_handle();
    let mut dialog = tui::relink_error("test", "/old", "/new", &"test".into(), false);
    dialog.copy_status = Some("Copying message…".into());
    dialog.copy_task = Some(task);
    let mut view = View {
        error_dialog: Some(dialog),
        ..Default::default()
    };
    let mut browser = Browser::new(vec![]);
    let key = |code| KeyEvent::new(code, KeyModifiers::NONE);
    assert_eq!(
        tui::handle_key(&mut browser, &mut view, key(KeyCode::Char('c'))),
        Action::Continue
    );
    assert_eq!(
        tui::handle_key(&mut browser, &mut view, key(KeyCode::Esc)),
        Action::Continue
    );
    tokio::task::yield_now().await;
    assert!(abort.is_finished());
    assert!(view.error_dialog.is_none());
}

async fn fake_profile_fixture(
    profile: Option<&str>,
) -> (
    tempfile::TempDir,
    std::path::PathBuf,
    SourceConfig,
    SessionSummary,
) {
    let root = tempfile::tempdir().unwrap();
    let home = root.path().join("codex");
    std::fs::create_dir(&home).unwrap();
    std::fs::create_dir(root.path().join("new")).unwrap();
    std::fs::write(
        home.join("native.json"),
        serde_json::json!({"cwd":root.path().join("missing"),"archived":false,"profile":profile})
            .to_string(),
    )
    .unwrap();
    let source = SourceConfig {
        id: SourceId("fixture".into()),
        home,
    };
    let binary =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/app-server/relink_fake.py");
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
    (root, binary, source, session)
}

#[tokio::test]
async fn builtins_verify_and_old_builtin_journals_cannot_authorize_recovery() {
    for profile in [":workspace", ":read-only"] {
        let (root, binary, source, session) = fake_profile_fixture(Some(profile)).await;
        let preview = relocation::preview(&binary, &source, &session, root.path().join("new"))
            .await
            .unwrap();
        let state = root.path().join("state");
        let outcome = relocation::execute(&binary, &source, &state, preview.clone(), false).await;
        assert!(
            matches!(outcome, relocation::Outcome::Verified(_)),
            "{outcome:?}"
        );
        let journal = std::fs::read_dir(state.join("relocations"))
            .unwrap()
            .map(|e| e.unwrap().path())
            .find(|p| p.extension().is_some_and(|e| e == "json"))
            .unwrap();
        let mut value: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&journal).unwrap()).unwrap();
        assert_eq!(value["permission_validation"]["version"], 1);
        value["phase"] = "verifying".into();
        value
            .as_object_mut()
            .unwrap()
            .remove("permission_validation");
        value["preview"].as_object_mut().unwrap().remove("profile");
        let old_preview = serde_json::from_value(value["preview"].clone()).unwrap();
        std::fs::write(&journal, value.to_string()).unwrap();
        assert!(matches!(
            relocation::execute(&binary, &source, &state, old_preview, true).await,
            relocation::Outcome::Unresolved(_)
        ));
    }
}

#[tokio::test]
async fn unsupported_profiles_are_rejected_before_confirmation_or_resume() {
    for profile in ["project", ":danger-full-access"] {
        let (root, binary, source, session) = fake_profile_fixture(Some(profile)).await;
        assert!(
            relocation::preview(&binary, &source, &session, root.path().join("new"))
                .await
                .is_err()
        );
        let requests = std::fs::read_to_string(source.home.join("requests.jsonl")).unwrap();
        assert!(!requests.contains("thread/resume"));
        assert!(!requests.contains("thread/settings/update"));
    }
}

#[tokio::test]
async fn changed_profile_or_fallback_is_rejected_before_directory_update() {
    for live in [None, Some(":read-only"), Some("custom")] {
        let (root, binary, source, session) = fake_profile_fixture(Some(":workspace")).await;
        let preview = relocation::preview(&binary, &source, &session, root.path().join("new"))
            .await
            .unwrap();
        let path = source.home.join("native.json");
        let mut value: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        value["recorded_profile"] = ":workspace".into();
        value["profile"] = serde_json::json!(live);
        std::fs::write(path, value.to_string()).unwrap();
        let outcome =
            relocation::execute(&binary, &source, &root.path().join("state"), preview, false).await;
        assert!(
            matches!(outcome, relocation::Outcome::Rejected(_)),
            "{outcome:?}"
        );
        assert!(
            !std::fs::read_to_string(source.home.join("requests.jsonl"))
                .unwrap()
                .contains("thread/settings/update")
        );
    }
}

#[tokio::test]
async fn legacy_journal_without_profile_fields_remains_recoverable() {
    let (root, binary, source, session) = fake_profile_fixture(None).await;
    let preview = relocation::preview(&binary, &source, &session, root.path().join("new"))
        .await
        .unwrap();
    let state = root.path().join("state");
    assert!(matches!(
        relocation::execute(&binary, &source, &state, preview, false).await,
        relocation::Outcome::Verified(_)
    ));
    let journal = std::fs::read_dir(state.join("relocations"))
        .unwrap()
        .map(|e| e.unwrap().path())
        .find(|p| p.extension().is_some_and(|e| e == "json"))
        .unwrap();
    let mut value: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&journal).unwrap()).unwrap();
    value["phase"] = "verifying".into();
    value
        .as_object_mut()
        .unwrap()
        .remove("permission_validation");
    value["preview"].as_object_mut().unwrap().remove("profile");
    let old_preview = serde_json::from_value(value["preview"].clone()).unwrap();
    std::fs::write(&journal, value.to_string()).unwrap();
    let result = relocation::execute(&binary, &source, &state, old_preview, true).await;
    assert!(
        matches!(result, relocation::Outcome::Verified(_)),
        "{result:?}"
    );
}

#[tokio::test]
async fn existing_folder_moves_preserve_files_and_reject_same_destination() {
    for kind in ["child", "parent", "other"] {
        let (root, binary, source, session) = fake_profile_fixture(Some(":workspace")).await;
        let old = session.cwd.as_ref().unwrap();
        std::fs::create_dir(old).unwrap();
        std::fs::write(old.join("sentinel"), "keep").unwrap();
        let alias = root.path().join("alias");
        std::os::unix::fs::symlink(old, &alias).unwrap();
        for target in [old.clone(), alias] {
            let error = relocation::preview(&binary, &source, &session, target)
                .await
                .unwrap_err();
            assert!(error.detail.contains("already in that folder"));
        }
        let target = match kind {
            "child" => {
                let p = old.join("child");
                std::fs::create_dir(&p).unwrap();
                p
            }
            "parent" => root.path().to_owned(),
            _ => root.path().join("new"),
        };
        let preview = relocation::preview(&binary, &source, &session, target)
            .await
            .unwrap();
        let result =
            relocation::execute(&binary, &source, &root.path().join("state"), preview, false).await;
        assert!(
            matches!(result, relocation::Outcome::Verified(_)),
            "{result:?}"
        );
        assert_eq!(
            std::fs::read_to_string(old.join("sentinel")).unwrap(),
            "keep"
        );
    }
}

#[tokio::test]
async fn original_reappearing_is_allowed_but_changed_session_is_rejected() {
    for changed in [false, true] {
        let (root, binary, source, session) = fake_profile_fixture(None).await;
        let preview = relocation::preview(&binary, &source, &session, root.path().join("new"))
            .await
            .unwrap();
        std::fs::create_dir(session.cwd.as_ref().unwrap()).unwrap();
        if changed {
            let path = source.home.join("native.json");
            let mut value: serde_json::Value =
                serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
            value["cwd"] = serde_json::json!(root.path().join("elsewhere"));
            std::fs::write(path, value.to_string()).unwrap();
        }
        let outcome =
            relocation::execute(&binary, &source, &root.path().join("state"), preview, false).await;
        if changed {
            assert!(
                matches!(outcome, relocation::Outcome::Rejected(_)),
                "{outcome:?}"
            );
        } else {
            assert!(
                matches!(outcome, relocation::Outcome::Verified(_)),
                "{outcome:?}"
            );
        }
    }
}
