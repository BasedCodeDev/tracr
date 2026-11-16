use std::path::{Path, PathBuf};
use tracr::{
    browser::{Browser, FolderFilter, SourceSpec, Update},
    config::{SourceConfig, source_id},
    domain::*,
    providers::{SessionProvider, codex::CodexProvider, fixture::FixtureProvider},
};

fn catalog() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/sessions/catalog.json")
}
async fn loaded() -> Browser {
    let provider = FixtureProvider::open(&catalog()).unwrap();
    let inventory = tracr::app::collect(&provider).await;
    let mut browser = Browser::new(vec![SourceSpec::Fixture(catalog())]);
    browser.begin_refresh();
    browser.apply(Update::Page {
        generation: browser.generation,
        source: 0,
        sessions: inventory.sessions,
        warnings: vec![],
    });
    browser.apply(Update::Finished {
        generation: browser.generation,
        source: 0,
        error: None,
    });
    browser
}

#[tokio::test]
async fn search_is_global_and_archive_aware_and_clear_restores_folder() {
    let mut browser = loaded().await;
    browser.folder = FolderFilter::Path(PathBuf::from("/unrelated"));
    assert!(browser.visible().is_empty());
    browser.query = "CAFÉ codex".into();
    assert_eq!(browser.visible().len(), 1);
    browser.query = "archived".into();
    assert!(browser.visible().is_empty());
    browser.include_archived = true;
    assert_eq!(browser.visible().len(), 1);
    browser.query.clear();
    assert!(browser.visible().is_empty());
    assert_eq!(
        browser.folder,
        FolderFilter::Path(PathBuf::from("/unrelated"))
    );
}
#[tokio::test]
async fn refresh_preserves_selection_and_ignores_old_generations() {
    let mut browser = loaded().await;
    let original = browser.all();
    let key = original[0].key.clone();
    browser.selected = Some(key.clone());
    let old = browser.generation;
    browser.begin_refresh();
    browser.apply(Update::Finished {
        generation: old,
        source: 0,
        error: None,
    });
    assert!(browser.loading());
    browser.apply(Update::Finished {
        generation: browser.generation,
        source: 0,
        error: Some("disconnected".into()),
    });
    assert_eq!(browser.all(), original);
    assert_eq!(browser.selected, Some(key));
    assert!(!browser.diagnostics()[0].complete);
    browser.begin_refresh();
    browser.apply(Update::Finished {
        generation: browser.generation,
        source: 0,
        error: None,
    });
    assert!(browser.all().is_empty());
    assert!(browser.selected.is_none());
}
#[tokio::test]
async fn folder_matching_is_exact_and_unlinked_excludes_permission_errors() {
    let mut browser = loaded().await;
    let mut records = browser.all();
    records.truncate(2);
    records[0].cwd = Some("/work/app/src".into());
    records[1].cwd = Some("/work/apple".into());
    browser.begin_refresh();
    browser.apply(Update::Page {
        generation: browser.generation,
        source: 0,
        sessions: records.clone(),
        warnings: vec![],
    });
    browser.apply(Update::Finished {
        generation: browser.generation,
        source: 0,
        error: None,
    });
    browser.folder = FolderFilter::Path("/work/app".into());
    assert!(browser.visible().is_empty());
    browser.open_folder("/work/app/src".into());
    assert_eq!(browser.visible().len(), 1);
    browser
        .folders
        .insert("/work/app/src".into(), FolderStatus::Unlinked);
    browser.folders.insert(
        "/work/apple".into(),
        FolderStatus::Inaccessible {
            reason: "permission denied".into(),
        },
    );
    browser.folder = FolderFilter::Unlinked;
    assert_eq!(browser.visible().len(), 1);
}
#[tokio::test]
async fn layouts_render_normal_narrow_and_tiny_and_search_keys_are_text() {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use ratatui::{Terminal, backend::TestBackend};
    let mut browser = loaded().await;
    let mut view = tracr::tui::View::default();
    for (width, height) in [(140, 40), (60, 20), (8, 2), (1, 1)] {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal
            .draw(|f| tracr::tui::draw(f, &browser, &view))
            .unwrap();
        if width > 12 {
            assert!(
                terminal
                    .backend()
                    .buffer()
                    .content
                    .iter()
                    .any(|cell| cell.symbol() == "t")
            );
        }
    }
    tracr::tui::handle_key(
        &mut browser,
        &mut view,
        KeyEvent::new(KeyCode::Char('/'), KeyModifiers::NONE),
    );
    assert_eq!(
        tracr::tui::handle_key(
            &mut browser,
            &mut view,
            KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE)
        ),
        tracr::tui::Action::Continue
    );
    assert_eq!(view.search_query, "q");
    assert!(browser.query.is_empty());
    tracr::tui::handle_key(
        &mut browser,
        &mut view,
        KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE),
    );
    assert!(!view.editing);
    assert!(browser.query.is_empty());
    assert!(tracr::browser::safe_text("\x1b[31m").starts_with("\\u{1b}"));
}

fn fake(settings: serde_json::Value) -> (tempfile::TempDir, PathBuf, SourceConfig) {
    let temp = tempfile::tempdir().unwrap();
    // Shared immutable executable avoids ETXTBSY when parallel tests fork while
    // another test is copying a new executable. Settings stay in each unique home.
    let binary = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/app-server/fake.py");
    std::fs::write(temp.path().join("fake.json"), settings.to_string()).unwrap();
    let source = SourceConfig {
        id: source_id(&ProviderId("codex".into()), temp.path()),
        home: temp.path().to_owned(),
    };
    (temp, binary, source)
}
#[tokio::test]
async fn codex_pages_archive_partitions_and_reads_metadata_without_resuming() {
    let (temp, binary, source) = fake(serde_json::json!({}));
    let provider = CodexProvider::connect(&binary, source).await.unwrap();
    let result = tracr::app::collect(&provider).await;
    assert!(result.complete);
    assert_eq!(result.sessions.len(), 3);
    assert_eq!(result.sessions.iter().filter(|s| s.archived).count(), 1);
    let original = &result.sessions[0];
    let read = provider.read(&original.key).await.unwrap().unwrap();
    assert_ne!(read.cwd, original.cwd);
    provider.close().await;
    let requests = std::fs::read_to_string(temp.path().join("requests.jsonl")).unwrap();
    assert!(!requests.contains("thread/resume"));
    assert!(!requests.contains("turn/start"));
}
#[tokio::test]
async fn malformed_records_preserve_valid_sessions_and_mark_incomplete() {
    let (_temp, binary, source) = fake(serde_json::json!({"malformed":true}));
    let provider = CodexProvider::connect(&binary, source).await.unwrap();
    let result = tracr::app::collect(&provider).await;
    assert!(!result.complete);
    assert_eq!(result.sessions.len(), 3);
    assert!(result.error.is_some());
    provider.close().await;
}
#[tokio::test]
async fn process_exit_and_bad_executable_are_provider_errors() {
    let (_temp, binary, source) = fake(serde_json::json!({"exit":true}));
    assert!(
        CodexProvider::connect(Path::new("/no-such-codex"), source.clone())
            .await
            .is_err()
    );
    let provider = CodexProvider::connect(&binary, source).await.unwrap();
    assert!(provider.list(None).await.is_err());
    provider.close().await;
}
#[tokio::test]
async fn multi_source_discovery_preserves_other_homes_when_one_fails() {
    let (_one, bin1, source1) = fake(serde_json::json!({}));
    let (_two, bin2, source2) = fake(serde_json::json!({}));
    let specs = vec![
        SourceSpec::Codex {
            binary: bin1,
            source: source1.clone(),
        },
        SourceSpec::Codex {
            binary: bin2,
            source: source2,
        },
        SourceSpec::Codex {
            binary: "/missing".into(),
            source: source1,
        },
    ];
    let mut browser = Browser::new(specs);
    let mut discovery = tracr::browser::Discovery::new();
    discovery.refresh(&mut browser);
    while browser.loading() {
        browser.apply(discovery.rx.recv().await.unwrap());
    }
    assert_eq!(browser.all().len(), 6);
    assert!(!browser.diagnostics()[2].complete);
    discovery.close(&mut browser).await;
}
#[test]
fn tui_rejects_non_terminal_before_launching_provider() {
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_tracr"))
        .args(["browse", "--fixture"])
        .arg(catalog())
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("requires a terminal"));
}

#[tokio::test]
async fn stalled_rpc_times_out_and_closes_owned_process() {
    let (_temp, binary, source) = fake(serde_json::json!({"timeout":true}));
    let provider = CodexProvider::connect(&binary, source).await.unwrap();
    let error = tokio::time::timeout(std::time::Duration::from_secs(25), provider.list(None))
        .await
        .unwrap()
        .unwrap_err();
    assert!(error.to_string().contains("timed out"));
    provider.close().await;
}

#[test]
fn terminal_attributes_restore_after_quit_interrupt_and_termination() {
    let result = std::process::Command::new("python3")
        .arg(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("tests/fixtures/app-server/terminal_check.py"),
        )
        .arg(env!("CARGO_BIN_EXE_tracr"))
        .arg(catalog())
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
}

#[tokio::test]
async fn directory_mismatch_is_visible_without_regrouping() {
    let mut browser = loaded().await;
    let session = browser.visible()[0].clone();
    browser.selected = Some(session.key.clone());
    let mut detail = session.clone();
    detail.cwd = Some("/new-folder".into());
    browser.apply(Update::Detail {
        generation: browser.generation,
        key: session.key.clone(),
        value: Box::new(Ok(Some(detail))),
    });
    assert_eq!(
        browser
            .all()
            .into_iter()
            .find(|s| s.key == session.key)
            .unwrap()
            .cwd,
        session.cwd
    );
    let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(150, 45)).unwrap();
    let view = tracr::tui::View {
        focus: 1,
        info: Some(tracr::tui::InfoModal::new(&browser, session.clone())),
        ..Default::default()
    };
    terminal
        .draw(|frame| tracr::tui::draw(frame, &browser, &view))
        .unwrap();
    let text: String = terminal
        .backend()
        .buffer()
        .content
        .iter()
        .map(|c| c.symbol())
        .collect();
    assert!(text.contains("Folder differs from Codex"));
}

#[tokio::test]
async fn resume_preparation_uses_fresh_directory_and_rejects_unsafe_targets() {
    let (temp, binary, source) = fake(serde_json::json!({}));
    let provider = CodexProvider::connect(&binary, source.clone())
        .await
        .unwrap();
    let inventory = tracr::app::collect(&provider).await;
    let session = inventory.sessions.iter().find(|s| !s.archived).unwrap();
    assert!(provider.prepare_resume(session).await.is_err());
    let cwd = temp.path().join("resume");
    std::fs::create_dir(&cwd).unwrap();
    let command = provider.prepare_resume(session).await.unwrap();
    assert_eq!(command.cwd, cwd);
    assert_eq!(command.executable, binary);
    assert_eq!(
        command.args,
        vec![
            std::ffi::OsString::from("resume"),
            "--cd".into(),
            cwd.into_os_string(),
            "--".into(),
            session.key.native_id.clone().into()
        ]
    );
    assert_eq!(
        command.env,
        vec![("CODEX_HOME".into(), source.home.into_os_string())]
    );
    let mut blocked = session.clone();
    blocked.archived = true;
    assert!(provider.prepare_resume(&blocked).await.is_err());
    blocked.archived = false;
    blocked.can_accept_direct_input = Some(false);
    assert!(provider.prepare_resume(&blocked).await.is_err());
    blocked.can_accept_direct_input = None;
    blocked.key.source_id.0.push_str("wrong");
    assert!(provider.prepare_resume(&blocked).await.is_err());
    provider.close().await;
}

#[tokio::test]
async fn enter_respects_focus_search_and_pending_preparation() {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use tracr::tui::{Action, View, handle_key};
    let mut browser = loaded().await;
    let mut view = View::default();
    let enter = KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE);
    assert_eq!(handle_key(&mut browser, &mut view, enter), Action::Continue);
    view.focus = 1;
    assert_eq!(
        handle_key(&mut browser, &mut view, enter),
        Action::ResumeSelected
    );
    view.editing = true;
    assert_eq!(handle_key(&mut browser, &mut view, enter), Action::Continue);
    view.preparing = true;
    assert_eq!(handle_key(&mut browser, &mut view, enter), Action::Continue);
    let provider = FixtureProvider::open(&catalog()).unwrap();
    assert!(provider.prepare_resume(&browser.all()[0]).await.is_err());
}

#[tokio::test]
async fn initial_folder_shows_direct_sessions_and_survives_empty_refresh() {
    let mut browser = loaded().await;
    let mut sessions = browser.all();
    sessions.truncate(4);
    for (session, path) in
        sessions
            .iter_mut()
            .zip(["/work/app", "/work/app/src", "/work/app-other", "/work/app"])
    {
        session.cwd = Some(path.into());
        session.archived = false;
    }
    sessions[3].archived = true;
    browser.open_folder("/work/app".into());
    browser.begin_refresh();
    browser.apply(Update::Page {
        generation: browser.generation,
        source: 0,
        sessions,
        warnings: vec![],
    });
    browser.apply(Update::Finished {
        generation: browser.generation,
        source: 0,
        error: None,
    });
    assert_eq!(browser.visible().len(), 1);
    assert!(
        browser
            .folder_rows()
            .iter()
            .any(|row| row.filter == browser.folder)
    );
    assert!(browser.expanded.contains(std::path::Path::new("/work")));
    assert_eq!(
        browser.visible()[0].cwd.as_deref(),
        Some(std::path::Path::new("/work/app"))
    );
    assert_eq!(
        browser
            .folder_rows()
            .iter()
            .find(|row| row.filter == browser.folder)
            .unwrap()
            .count,
        2
    );
    browser.include_archived = true;
    assert_eq!(browser.visible().len(), 2);
    assert!(
        browser
            .visible()
            .iter()
            .all(|s| s.cwd.as_deref() == Some(std::path::Path::new("/work/app")))
    );
    assert_eq!(
        browser
            .folder_rows()
            .iter()
            .find(|row| row.filter == browser.folder)
            .unwrap()
            .count,
        3
    );
    browser.include_archived = false;
    browser.open_folder("/work/app/src".into());
    assert_eq!(browser.visible().len(), 1);
    assert_eq!(
        browser.visible()[0].cwd.as_deref(),
        Some(std::path::Path::new("/work/app/src"))
    );
    browser.open_folder("/work".into());
    assert!(browser.visible().is_empty());
    assert_eq!(
        browser
            .folder_rows()
            .iter()
            .find(|row| row.filter == browser.folder)
            .unwrap()
            .count,
        3
    );
    browser.open_folder("/work/app".into());
    browser.query = "app/src".into();
    assert_eq!(browser.visible().len(), 1);
    assert_eq!(
        browser.visible()[0].cwd.as_deref(),
        Some(std::path::Path::new("/work/app/src"))
    );
    browser.query = "app-other".into();
    assert_eq!(browser.visible().len(), 1);
    browser.query.clear();
    assert_eq!(browser.visible().len(), 1);
    browser.begin_refresh();
    browser.apply(Update::Finished {
        generation: browser.generation,
        source: 0,
        error: None,
    });
    assert!(browser.visible().is_empty());
    assert!(
        browser
            .folder_rows()
            .iter()
            .any(|row| row.filter == browser.folder && row.count == 0)
    );
}

#[tokio::test]
async fn enter_and_escape_navigate_panes_outside_search() {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use tracr::tui::{View, handle_key};
    let mut browser = loaded().await;
    let mut view = View::default();
    let key = |code| KeyEvent::new(code, KeyModifiers::NONE);
    {
        let editing = false;
        view.editing = editing;
        view.focus = 0;
        browser.folder = FolderFilter::Unlinked;
        browser.query = "codex".into();
        handle_key(&mut browser, &mut view, key(KeyCode::Down));
        assert_eq!(browser.folder, FolderFilter::Unknown);
        handle_key(&mut browser, &mut view, key(KeyCode::Up));
        assert_eq!(browser.folder, FolderFilter::Unlinked);
        let path = PathBuf::from("/work/project");
        browser.open_folder(path.clone());
        browser.expanded.remove(&path);
        handle_key(&mut browser, &mut view, key(KeyCode::Enter));
        assert_eq!(view.focus, 1);
        assert!(!browser.expanded.contains(&path));
        assert_eq!(view.editing, editing);
        for code in [KeyCode::Right, KeyCode::Char('h'), KeyCode::Char('l')] {
            handle_key(&mut browser, &mut view, key(code));
            assert_eq!(view.focus, 1);
            assert!(!browser.expanded.contains(&path));
        }
        for code in [KeyCode::Esc, KeyCode::Left, KeyCode::Backspace] {
            view.focus = 1;
            handle_key(&mut browser, &mut view, key(code));
            assert_eq!(view.focus, 0);
            assert_eq!(browser.folder, FolderFilter::Path(path.clone()));
            assert!(!browser.expanded.contains(&path));
        }
        handle_key(&mut browser, &mut view, key(KeyCode::Down));
        for expected in [1, 0, 1, 0] {
            handle_key(&mut browser, &mut view, key(KeyCode::Tab));
            assert_eq!(view.focus, expected);
        }
        for expected in [1, 0] {
            handle_key(&mut browser, &mut view, key(KeyCode::BackTab));
            assert_eq!(view.focus, expected);
        }
        assert_eq!(browser.query, "codex");
    }
}

#[tokio::test]
async fn guardian_metadata_is_hidden_without_dropping_other_sessions_or_pages() {
    let (temp, binary, source) = fake(serde_json::json!({"guardians":true}));
    let provider = CodexProvider::connect(&binary, source).await.unwrap();
    let result = tracr::app::collect(&provider).await;
    assert!(result.complete);
    assert_eq!(result.sessions.len(), 4);
    assert_eq!(result.sessions.iter().filter(|s| s.archived).count(), 2);
    assert!(result.sessions.iter().any(|s| s.title == "Guardian review"));
    assert!(
        result
            .sessions
            .iter()
            .any(|s| s.key.native_id == "other-second")
    );
    assert!(
        result
            .sessions
            .iter()
            .all(|s| !s.key.native_id.starts_with("guardian-")
                && !s.key.native_id.starts_with("legacy-"))
    );
    std::fs::write(temp.path().join("fake.json"), r#"{"guardian_read":true}"#).unwrap();
    let session = &result.sessions[0];
    assert!(provider.read(&session.key).await.unwrap().is_none());
    assert!(provider.prepare_resume(session).await.is_err());
    provider.close().await;
}

#[test]
fn escape_returns_to_folders_and_closes_help_and_cancels_resume() {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use tracr::tui::{Action, View, handle_key};
    let mut browser = Browser::new(vec![]);
    browser.query = "keep filter".into();
    {
        let editing = false;
        for focus in 0..2 {
            {
                let code = KeyCode::Esc;
                let mut view = View {
                    focus,
                    editing,
                    ..View::default()
                };
                assert_eq!(
                    handle_key(
                        &mut browser,
                        &mut view,
                        KeyEvent::new(code, KeyModifiers::NONE)
                    ),
                    Action::Continue
                );
                assert_eq!(view.focus, 0);
                assert_eq!(view.editing, editing);
                assert_eq!(browser.query, "keep filter");
            }
        }
    }
    let escape = KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE);
    let mut view = View {
        focus: 1,
        help: true,
        ..View::default()
    };
    assert_eq!(
        handle_key(&mut browser, &mut view, escape),
        Action::Continue
    );
    assert!(!view.help);
    assert_eq!(view.focus, 1);
    view.preparing = true;
    assert_eq!(
        handle_key(&mut browser, &mut view, escape),
        Action::CancelResume
    );
    assert_eq!(view.focus, 1);
}

#[tokio::test]
async fn search_modal_selects_a_session_then_navigates_without_resuming() {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use tracr::tui::{Action, View, handle_key};
    let mut browser = loaded().await;
    let mut view = View {
        focus: 1,
        ..View::default()
    };
    browser.open_folder("/unrelated".into());
    let original_folder = browser.folder.clone();
    let original_selection = browser.selected.clone();
    let key = |code| KeyEvent::new(code, KeyModifiers::NONE);
    handle_key(&mut browser, &mut view, key(KeyCode::Char('/')));
    for c in "café".chars() {
        handle_key(&mut browser, &mut view, key(KeyCode::Char(c)));
    }
    let results = browser.search_results(&view.search_query);
    assert_eq!(results.len(), 1);
    let target = results[0].clone();
    assert_eq!(browser.folder, original_folder);
    handle_key(&mut browser, &mut view, key(KeyCode::Esc));
    assert_eq!(browser.folder, original_folder);
    assert_eq!(browser.selected, original_selection);
    assert_eq!(view.focus, 1);
    handle_key(&mut browser, &mut view, key(KeyCode::Char('/')));
    assert!(view.search_query.is_empty());
    handle_key(&mut browser, &mut view, key(KeyCode::Down));
    assert_eq!(
        view.search_selected.as_ref(),
        Some(&browser.search_results("")[1].key)
    );
    handle_key(&mut browser, &mut view, key(KeyCode::Up));
    assert_eq!(
        view.search_selected.as_ref(),
        Some(&browser.search_results("")[0].key)
    );
    for c in "café".chars() {
        handle_key(&mut browser, &mut view, key(KeyCode::Char(c)));
    }
    assert_eq!(
        handle_key(&mut browser, &mut view, key(KeyCode::Enter)),
        Action::Continue
    );
    assert!(!view.editing);
    assert_eq!(browser.folder, FolderFilter::Path(target.cwd.unwrap()));
    assert_eq!(browser.selected, Some(target.key));
    assert_eq!(view.focus, 1);
    handle_key(&mut browser, &mut view, key(KeyCode::Char('/')));
    for c in "no-such-session-jkqar".chars() {
        handle_key(&mut browser, &mut view, key(KeyCode::Char(c)));
    }
    assert!(browser.search_results(&view.search_query).is_empty());
    handle_key(&mut browser, &mut view, key(KeyCode::Enter));
    assert!(view.editing);
    assert_eq!(view.search_query, "no-such-session-jkqar");
    // Render the modal at wide and narrow widths, including an empty result list.
    for (width, height) in [(140, 40), (60, 20), (12, 4)] {
        let mut terminal =
            ratatui::Terminal::new(ratatui::backend::TestBackend::new(width, height)).unwrap();
        terminal
            .draw(|frame| tracr::tui::draw(frame, &browser, &view))
            .unwrap();
    }
}

#[tokio::test]
async fn modal_search_handles_missing_unknown_and_archived_folders() {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use tracr::tui::{Action, View, handle_key};
    let mut browser = loaded().await;
    let mut view = View::default();
    let key = |code| KeyEvent::new(code, KeyModifiers::NONE);
    assert!(browser.search_results("Archived task").is_empty());
    browser.include_archived = true;
    for query in ["Missing folder", "No folder recorded", "Archived task"] {
        handle_key(&mut browser, &mut view, key(KeyCode::Char('/')));
        view.search_query = query.into();
        let target = browser.search_results(query)[0].clone();
        assert_eq!(
            handle_key(&mut browser, &mut view, key(KeyCode::Enter)),
            Action::Continue
        );
        assert_eq!(browser.selected, Some(target.key));
        assert_eq!(view.focus, 1);
        assert!(!view.editing);
        assert_eq!(
            browser.folder,
            target
                .cwd
                .map(FolderFilter::Path)
                .unwrap_or(FolderFilter::Unknown)
        );
    }
}

#[test]
fn empty_folder_creation_picker_and_validation() {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use tracr::tui::{Action, View, handle_key};
    let temp = tempfile::tempdir().unwrap();
    let spec = |name: &str| SourceSpec::Codex {
        binary: "codex".into(),
        source: SourceConfig {
            id: source_id(&ProviderId("codex".into()), &temp.path().join(name)),
            home: temp.path().join(name),
        },
    };
    let mut browser = Browser::new(vec![spec("one")]);
    browser.open_folder(temp.path().to_owned());
    assert!(
        browser
            .folder_rows()
            .iter()
            .all(|r| r.filter != FolderFilter::All)
    );
    let mut view = View {
        focus: 1,
        ..View::default()
    };
    let key = |code| KeyEvent::new(code, KeyModifiers::NONE);
    assert_eq!(
        handle_key(&mut browser, &mut view, key(KeyCode::Enter)),
        Action::CreateNew(0)
    );
    let command = browser.specs[0].prepare_new(temp.path()).unwrap();
    assert_eq!(command.cwd, temp.path());
    assert_eq!(
        command.args,
        vec![
            std::ffi::OsString::from("--cd"),
            temp.path().as_os_str().to_owned()
        ]
    );
    assert_eq!(command.env[0].1, temp.path().join("one").as_os_str());
    assert!(
        browser.specs[0]
            .prepare_new(&temp.path().join("missing"))
            .is_err()
    );
    assert!(browser.specs[0].prepare_new(Path::new("relative")).is_err());
    let file = temp.path().join("file");
    std::fs::write(&file, "").unwrap();
    assert!(browser.specs[0].prepare_new(&file).is_err());
    browser.specs.push(spec("two"));
    assert_eq!(
        handle_key(&mut browser, &mut view, key(KeyCode::Enter)),
        Action::Continue
    );
    assert_eq!(view.source_picker, Some(0));
    handle_key(&mut browser, &mut view, key(KeyCode::Down));
    assert_eq!(
        handle_key(&mut browser, &mut view, key(KeyCode::Enter)),
        Action::CreateNew(1)
    );
    handle_key(&mut browser, &mut view, key(KeyCode::Enter));
    handle_key(&mut browser, &mut view, key(KeyCode::Esc));
    assert_eq!(view.source_picker, None);
    browser.begin_refresh();
    assert_eq!(
        handle_key(&mut browser, &mut view, key(KeyCode::Enter)),
        Action::ResumeSelected
    );
}

#[tokio::test]
async fn action_menu_navigates_cancels_and_dispatches_without_changing_context() {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use tracr::tui::{Action, View, handle_key};
    let mut browser = loaded().await;
    let mut view = View {
        focus: 1,
        help_page_size: 4,
        ..View::default()
    };
    let key = |code| KeyEvent::new(code, KeyModifiers::NONE);
    let selected = browser.selected.clone();
    handle_key(&mut browser, &mut view, key(KeyCode::Char('?')));
    handle_key(&mut browser, &mut view, key(KeyCode::PageDown));
    assert_eq!(view.help_selected, 4);
    handle_key(&mut browser, &mut view, key(KeyCode::Down));
    assert_eq!(view.help_selected, 5);
    view.help_selected = 11; // Create is disabled for fixtures.
    assert_eq!(
        handle_key(&mut browser, &mut view, key(KeyCode::Enter)),
        Action::Continue
    );
    assert!(view.help);
    assert_eq!(browser.selected, selected);
    assert_eq!(view.focus, 1);
    handle_key(&mut browser, &mut view, key(KeyCode::Esc));
    assert!(!view.help);
    assert_eq!(view.focus, 1);
    handle_key(&mut browser, &mut view, key(KeyCode::Char('?')));
    assert_eq!(view.help_selected, 0);
    view.help_selected = 7;
    handle_key(&mut browser, &mut view, key(KeyCode::Down));
    handle_key(&mut browser, &mut view, key(KeyCode::Enter));
    assert!(!view.help);
    assert_eq!(view.focus, 1); // Previous item keeps focus on Sessions.
    handle_key(&mut browser, &mut view, key(KeyCode::Char('?')));
    view.help_selected = 3;
    assert_eq!(
        handle_key(&mut browser, &mut view, key(KeyCode::Enter)),
        Action::Refresh
    );
    assert!(!view.help);
    handle_key(&mut browser, &mut view, key(KeyCode::Char('?')));
    view.help_selected = 2;
    handle_key(&mut browser, &mut view, key(KeyCode::Enter));
    assert!(view.editing);
    assert!(!view.help);
    assert_eq!(browser.selected, selected);
}

#[tokio::test]
async fn action_menu_scrolls_selection_into_view_and_handles_small_layouts() {
    use ratatui::{Terminal, backend::TestBackend};
    use tracr::tui::{View, draw};
    let browser = loaded().await;
    for (width, height) in [(140, 40), (60, 20), (36, 15), (12, 4)] {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        let mut view = View {
            help: true,
            help_selected: 14,
            ..View::default()
        };
        terminal.draw(|frame| draw(frame, &browser, &view)).unwrap();
        if width >= 36 {
            let text: String = terminal
                .backend()
                .buffer()
                .content
                .iter()
                .map(|cell| cell.symbol())
                .collect();
            assert!(text.contains("Move folder"), "{width}x{height}");
            assert!(text.contains("Esc close"));
            view.help_selected = 0;
            terminal.draw(|frame| draw(frame, &browser, &view)).unwrap();
            let text: String = terminal
                .backend()
                .buffer()
                .content
                .iter()
                .map(|cell| cell.symbol())
                .collect();
            assert!(text.contains("Help / actions"));
        }
    }
}

#[tokio::test]
async fn native_current_directory_wins_and_unindexed_sessions_are_reconciled() {
    for settings in [
        serde_json::json!({"db_cwd":"/native/current"}),
        serde_json::json!({"db_empty":true,"read_cwd":"/native/current"}),
        serde_json::json!({"db_unavailable":true,"read_cwd":"/native/current"}),
    ] {
        let unavailable = settings.get("db_unavailable").is_some();
        let (_temp, binary, source) = fake(settings);
        let provider = CodexProvider::connect(&binary, source).await.unwrap();
        let inventory = tracr::app::collect(&provider).await;
        assert_eq!(inventory.complete, !unavailable);
        assert_eq!(inventory.sessions.len(), 3);
        assert!(
            inventory
                .sessions
                .iter()
                .all(|s| s.cwd.as_deref() == Some(Path::new("/native/current")))
        );
        assert_eq!(inventory.sessions.iter().filter(|s| s.archived).count(), 1);
        provider.close().await;
    }
}

#[tokio::test]
async fn unlinked_rows_use_explicit_undimmed_labels_and_footer_preserves_primary_controls() {
    use ratatui::{
        Terminal,
        backend::TestBackend,
        style::{Color, Modifier},
    };
    use tracr::tui::{View, draw};
    let mut browser = loaded().await;
    let session = browser
        .visible()
        .into_iter()
        .find(|s| s.cwd.is_some())
        .unwrap();
    let path = session.cwd.clone().unwrap();
    browser.sources[0]
        .live
        .insert(session.key.clone(), RuntimeStatus::Running);
    for status in [
        None,
        Some(FolderStatus::Available),
        Some(FolderStatus::Unknown),
        Some(FolderStatus::Inaccessible {
            reason: "Permission denied".into(),
        }),
        Some(FolderStatus::Unlinked),
    ] {
        browser.folders.clear();
        if let Some(status) = status.clone() {
            browser.folders.insert(path.clone(), status);
        }
        let unlinked = status == Some(FolderStatus::Unlinked);
        for selected in [false, true] {
            browser.selected = selected.then(|| session.key.clone());
            for width in [12, 24, 36, 80, 140] {
                let mut terminal = Terminal::new(TestBackend::new(width, 24)).unwrap();
                let view = View {
                    focus: 1,
                    message: Some("Test message".into()),
                    ..View::default()
                };
                terminal.draw(|frame| draw(frame, &browser, &view)).unwrap();
                let buffer = terminal.backend().buffer();
                let line = |row: u16| {
                    (0..width)
                        .map(|col| buffer[(col, row)].symbol())
                        .collect::<String>()
                };
                let footer = (21..24).map(line).collect::<Vec<_>>().join("\n");
                let upper = (21..24)
                    .find(|&row| line(row).contains(if width < 15 { "? · q" } else { "? Help" }))
                    .unwrap();
                assert!(
                    line(upper + 1).contains("Enter Resume"),
                    "{width}: {footer}"
                );
                assert!(line(upper + 1).ends_with(if width >= 80 {
                    "R Relink"
                } else if width >= 36 {
                    "i Info"
                } else if width >= 24 {
                    "n New"
                } else {
                    "Enter Resume"
                }));
                assert!(footer.contains("Test message"));
                let mut labels = 0;
                for row in 1..21 {
                    for col in 0..width.saturating_sub(7) {
                        let word = (col..col + 8)
                            .map(|x| buffer[(x, row)].symbol())
                            .collect::<String>();
                        if word == "Unlinked" && buffer[(col, row)].fg == Color::Rgb(255, 165, 0) {
                            labels += 1;
                            assert!(!buffer[(col, row)].modifier.contains(Modifier::DIM));
                        }
                    }
                }
                assert_eq!(
                    labels > 0,
                    unlinked,
                    "width={width}, selected={selected}, status={status:?}"
                );
                if width >= 80 {
                    let text = (0..21).map(line).collect::<Vec<_>>().join("\n");
                    assert!(text.contains("Running"));
                }
            }
        }
    }
}

#[tokio::test]
async fn information_modal_is_requested_pinned_scrollable_and_blocks_browser_actions() {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use tracr::tui::{Action, View, draw, handle_key};
    let mut browser = loaded().await;
    let session = browser.visible()[0].clone();
    browser.selected = Some(session.key.clone());
    let key = |code| KeyEvent::new(code, KeyModifiers::NONE);
    let mut view = View::default();
    handle_key(&mut browser, &mut view, key(KeyCode::Char('i')));
    assert!(view.info.is_none());
    view.focus = 1;
    handle_key(&mut browser, &mut view, key(KeyCode::Char('i')));
    assert!(view.info.is_some());
    let folder = browser.folder.clone();
    let selected = browser.selected.clone();
    for (width, height) in [(140, 40), (60, 20), (24, 10), (12, 4), (8, 3)] {
        let mut terminal =
            ratatui::Terminal::new(ratatui::backend::TestBackend::new(width, height)).unwrap();
        terminal.draw(|f| draw(f, &browser, &view)).unwrap();
        handle_key(&mut browser, &mut view, key(KeyCode::End));
        let info = view.info.as_ref().unwrap();
        assert_eq!(info.dialog.scroll, info.dialog.scroll_limit.get());
        handle_key(&mut browser, &mut view, key(KeyCode::Home));
        assert_eq!(view.info.as_ref().unwrap().dialog.scroll, 0);
    }
    for code in [
        KeyCode::Right,
        KeyCode::Tab,
        KeyCode::Down,
        KeyCode::Char('l'),
        KeyCode::Char('/'),
    ] {
        assert_eq!(
            handle_key(&mut browser, &mut view, key(code)),
            Action::Continue
        );
    }
    assert_eq!(browser.folder, folder);
    assert_eq!(browser.selected, selected);
    assert_eq!(view.focus, 1);
    assert_eq!(
        handle_key(&mut browser, &mut view, key(KeyCode::Char('c'))),
        Action::CopyInformation
    );
    browser.selected = None;
    browser
        .details
        .insert(session.key.clone(), Err("Metadata unavailable".into()));
    let info = view.info.as_mut().unwrap();
    info.refresh(&browser);
    assert!(info.dialog.report().contains("Metadata unavailable"));
    assert!(info.dialog.report().contains(&session.key.native_id));
    // Tab moved focus to Copy; Enter activates that button, not Close.
    assert_eq!(
        handle_key(&mut browser, &mut view, key(KeyCode::Enter)),
        Action::CopyInformation
    );
    handle_key(&mut browser, &mut view, key(KeyCode::Tab));
    assert_eq!(
        handle_key(&mut browser, &mut view, key(KeyCode::Enter)),
        Action::CloseInformation
    );
    assert!(view.info.is_none());
    assert_eq!(view.focus, 1);
    handle_key(&mut browser, &mut view, key(KeyCode::Char('i')));
    assert!(view.info.is_none());
    browser.selected = selected;
    for close in [KeyCode::Esc, KeyCode::Char('i')] {
        handle_key(&mut browser, &mut view, key(KeyCode::Char('i')));
        assert_eq!(
            handle_key(&mut browser, &mut view, key(close)),
            Action::CloseInformation
        );
    }
    view.help = true;
    view.help_selected = 12; // Session information; Recover is hidden.
    handle_key(&mut browser, &mut view, key(KeyCode::Enter));
    assert!(view.info.is_some());
    assert!(!view.help);
}

#[tokio::test]
async fn footer_and_menu_follow_focus_with_stable_shared_controls() {
    use ratatui::{Terminal, backend::TestBackend};
    use tracr::tui::{View, draw};
    let browser = loaded().await;
    for width in [12, 24, 36, 80, 140, 240] {
        let mut shared = None;
        for focus in [0, 1] {
            let mut terminal = Terminal::new(TestBackend::new(width, 30)).unwrap();
            let mut view = View {
                focus,
                ..View::default()
            };
            terminal.draw(|frame| draw(frame, &browser, &view)).unwrap();
            let line = |row| {
                (0..width)
                    .map(|col| terminal.backend().buffer()[(col, row)].symbol())
                    .collect::<String>()
            };
            let global = line(28);
            if let Some(previous) = &shared {
                assert_eq!(previous, &global);
            }
            shared = Some(global);
            let context = line(29);
            if focus == 0 {
                assert!(
                    !context.contains("Relink")
                        && !context.contains("Info")
                        && !context.contains("Resume")
                );
                if width >= 80 {
                    assert!(context.contains("m Move"));
                }
            } else {
                assert!(!context.contains("Move") && !context.contains("Toggle"));
                if width >= 80 {
                    assert!(
                        context.contains("i Info")
                            && context.contains("R Relink")
                            && context.contains("n New")
                    );
                }
            }
            if width >= 80 {
                view.help = true;
                view.help_selected = 10;
                terminal.draw(|frame| draw(frame, &browser, &view)).unwrap();
                let text = terminal
                    .backend()
                    .buffer()
                    .content
                    .iter()
                    .map(|cell| cell.symbol())
                    .collect::<String>();
                if focus == 0 {
                    assert!(text.contains("Move folder"));
                    assert!(
                        !text.contains("Relink session")
                            && !text.contains("Resume selected session")
                            && !text.contains("Session information")
                    );
                } else {
                    assert!(text.contains("Relink session"));
                    assert!(
                        !text.contains("Move folder")
                            && !text.contains("Expand / collapse folder")
                            && text.contains("Create new session")
                    );
                }
            }
        }
    }
}

#[tokio::test]
async fn idle_color_is_shared_by_sessions_search_info_and_folder_ancestors() {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use ratatui::{Terminal, backend::TestBackend, style::Color};
    use tracr::tui::{View, draw, handle_key};
    let mut browser = loaded().await;
    let mut idle = browser.all().into_iter().find(|s| !s.archived).unwrap();
    idle.cwd = Some(PathBuf::from("/project/child"));
    let mut other = idle.clone();
    other.key.native_id = "other".into();
    other.archived = true;
    browser.begin_refresh();
    browser.apply(Update::Page {
        generation: browser.generation,
        source: 0,
        sessions: vec![idle.clone(), other.clone()],
        warnings: vec![],
    });
    browser.apply(Update::Finished {
        generation: browser.generation,
        source: 0,
        error: None,
    });
    browser.sources[0]
        .live
        .insert(idle.key.clone(), RuntimeStatus::Idle);
    browser.sources[0]
        .live
        .insert(other.key.clone(), RuntimeStatus::Running);
    let parent = FolderFilter::Path(PathBuf::from("/project"));
    assert_eq!(browser.folder_activity(&parent), RuntimeStatus::Idle);
    browser.include_archived = true;
    assert_eq!(browser.folder_activity(&parent), RuntimeStatus::Running);
    browser.sources[0].live.insert(
        idle.key.clone(),
        RuntimeStatus::Waiting {
            approval: true,
            input: false,
        },
    );
    assert!(matches!(
        browser.folder_activity(&parent),
        RuntimeStatus::Waiting { .. }
    ));
    browser.include_archived = false;
    browser.sources[0]
        .live
        .insert(idle.key.clone(), RuntimeStatus::Idle);
    browser.open_folder(idle.cwd.clone().unwrap());
    browser.selected = Some(idle.key.clone());
    let mut view = View {
        focus: 1,
        ..View::default()
    };
    let mut terminal = Terminal::new(TestBackend::new(140, 40)).unwrap();
    let pale = Color::Rgb(255, 255, 170);
    for surface in 0..3 {
        if surface == 1 {
            view.editing = true;
        }
        if surface == 2 {
            view.editing = false;
            handle_key(
                &mut browser,
                &mut view,
                KeyEvent::new(KeyCode::Char('i'), KeyModifiers::NONE),
            );
        }
        terminal.draw(|frame| draw(frame, &browser, &view)).unwrap();
        let buffer = terminal.backend().buffer();
        assert!(
            buffer
                .content
                .iter()
                .any(|cell| cell.symbol() == "I" && cell.fg == pale),
            "surface {surface}"
        );
        if surface == 0 {
            assert!(
                buffer
                    .content
                    .iter()
                    .any(|cell| cell.symbol() == "p" && cell.fg == pale)
            );
        }
    }
    browser.sources[0]
        .live
        .insert(idle.key.clone(), RuntimeStatus::Unloaded);
    assert_eq!(browser.folder_activity(&parent), RuntimeStatus::Unknown);
}

#[tokio::test]
async fn footer_shares_feedback_only_when_display_width_leaves_a_gap() {
    use ratatui::{Terminal, backend::TestBackend};
    use tracr::tui::{View, draw};
    use unicode_width::UnicodeWidthStr;
    let browser = loaded().await;
    for width in [36, 140, 240] {
        for focus in [0, 1] {
            let mut terminal = Terminal::new(TestBackend::new(width, 30)).unwrap();
            let mut view = View {
                focus,
                ..View::default()
            };
            terminal.draw(|f| draw(f, &browser, &view)).unwrap();
            let line = |terminal: &Terminal<TestBackend>, row| {
                (0..width)
                    .map(|col| terminal.backend().buffer()[(col, row)].symbol())
                    .collect::<String>()
            };
            let shared = line(&terminal, 28).trim().to_owned();
            let context = line(&terminal, 29).trim().to_owned();
            let room = usize::from(width).saturating_sub(shared.width() + 2);
            let fitting = format!("{}{}", "界".repeat(room / 2), "x".repeat(room % 2));
            for (message, separate) in [
                (fitting.clone(), false),
                (format!("{fitting}x"), true),
                ("Long diagnostic ".repeat(30), true),
                (String::new(), false),
            ] {
                view.message = Some(message.clone());
                terminal.draw(|f| draw(f, &browser, &view)).unwrap();
                let top = if separate { 27 } else { 28 };
                let shared_start = width - shared.width() as u16;
                let buffer = terminal.backend().buffer();
                assert_eq!(buffer[(shared_start, top)].symbol(), "?");
                assert!(line(&terminal, top).ends_with(&shared));
                assert!(line(&terminal, top + 1).ends_with(&context));
                if !message.is_empty() {
                    let feedback_row = if separate { 29 } else { 28 };
                    assert_eq!(
                        buffer[(0, feedback_row)].symbol(),
                        &message.chars().next().unwrap().to_string()
                    );
                }
                if !separate && !message.is_empty() {
                    assert_eq!(buffer[(shared_start - 1, top)].symbol(), " ");
                    assert_eq!(buffer[(shared_start - 2, top)].symbol(), " ");
                }
            }
        }
    }
}

#[tokio::test]
async fn linked_checkout_is_projected_under_repository_and_aggregates_sessions() {
    let mut browser = loaded().await;
    browser.include_archived = true;
    let mut records = browser.all();
    records.truncate(2);
    records[0].cwd = Some("/scratch/feature/src".into());
    records[1].cwd = Some("/projects/app/src".into());
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
    let wt = |path: &str| tracr::git::Worktree {
        path: path.into(),
        head: None,
        branch: Some("refs/heads/main".into()),
        bare: false,
        locked: false,
        prunable: false,
    };
    browser.install_repository(tracr::git::Repository {
        common_dir: "/projects/app/.git".into(),
        git_dir: "/projects/app/.git".into(),
        worktrees: vec![wt("/projects/app"), wt("/scratch/feature")],
    });
    for (path, root) in [
        ("/scratch/feature", "/scratch/feature"),
        ("/scratch/feature/src", "/scratch/feature"),
        ("/projects/app", "/projects/app"),
        ("/projects/app/src", "/projects/app"),
    ] {
        let repository = browser.repositories.values().next().unwrap().clone();
        browser.apply_ownership(
            browser.git_generation,
            path.into(),
            tracr::git_discovery::Outcome::Repository {
                repository: tracr::git::Directory {
                    common_dir: repository.common_dir,
                    git_dir: repository.git_dir,
                },
                root: Some(root.into()),
            },
        );
    }
    browser.open_folder("/scratch/feature".into());
    assert_eq!(browser.visible().len(), 1);
    let rows = browser.folder_rows();
    let group = rows
        .iter()
        .position(|r| r.filter == FolderFilter::Path("/projects/app".into()))
        .unwrap();
    let feature = rows
        .iter()
        .position(|r| r.filter == FolderFilter::Path("/scratch/feature".into()))
        .unwrap();
    assert_eq!(feature, group + 1);
    assert!(!rows[feature].label.contains("↪"));
    assert!(!rows.iter().any(|r| r.label.starts_with("Worktrees")));
    assert_eq!(rows[feature].depth, rows[group].depth + 1);
    assert_eq!(rows[feature].count, 1);
    browser.expanded.insert("/scratch/feature".into());
    let expanded_rows = browser.folder_rows();
    let child = expanded_rows
        .iter()
        .find(|r| r.filter == FolderFilter::Path("/scratch/feature/src".into()))
        .unwrap();
    assert_eq!(child.depth, rows[feature].depth + 1);
    browser.expanded.remove(&PathBuf::from("/scratch/feature"));
    assert!(
        !browser
            .folder_rows()
            .iter()
            .any(|r| r.filter == FolderFilter::Path("/scratch/feature/src".into()))
    );
    browser.expanded.remove(&PathBuf::from("/projects/app"));
    assert!(
        !browser
            .folder_rows()
            .iter()
            .any(|r| r.filter == FolderFilter::Path("/scratch/feature".into()))
    );
    browser.expanded.insert("/projects/app".into());

    assert_eq!(
        rows.iter()
            .filter(|r| r.filter == FolderFilter::Path("/scratch/feature".into()))
            .count(),
        1
    );
    browser.open_folder("/projects/app".into());
    assert_eq!(browser.visible().len(), 1);
}

#[tokio::test]
async fn ownership_refresh_preserves_stale_membership_and_rejects_old_results() {
    let mut browser = loaded().await;
    let path: PathBuf = "/project".into();
    let repo = tracr::git::Repository {
        common_dir: "/project/.git".into(),
        git_dir: "/project/.git".into(),
        worktrees: vec![],
    };
    browser.apply_ownership(
        0,
        path.clone(),
        tracr::git_discovery::Outcome::Repository {
            repository: tracr::git::Directory {
                common_dir: repo.common_dir,
                git_dir: repo.git_dir,
            },
            root: Some(path.clone()),
        },
    );
    browser.invalidate_git();
    assert!(browser.ownership[&path].stale());
    assert!(browser.ownership[&path].effective().is_some());
    browser.apply_ownership(
        0,
        path.clone(),
        tracr::git_discovery::Outcome::NotRepository,
    );
    assert!(browser.ownership[&path].outcome.is_none());
    browser.apply_ownership(
        1,
        path.clone(),
        tracr::git_discovery::Outcome::Unavailable("unmounted".into()),
    );
    assert!(matches!(
        browser.ownership[&path].effective(),
        Some(tracr::git_discovery::Outcome::Repository { .. })
    ));
    browser.apply_ownership(
        1,
        path.clone(),
        tracr::git_discovery::Outcome::NotRepository,
    );
    assert!(matches!(
        browser.ownership[&path].effective(),
        Some(tracr::git_discovery::Outcome::NotRepository)
    ));
}

#[tokio::test]
async fn retirement_requires_conclusive_results_and_rejects_late_inventory() {
    use tracr::git_discovery::Outcome;
    let mut browser = loaded().await;
    let root: PathBuf = "/work/project".into();
    let id: PathBuf = "/work/project/.git".into();
    let repo = tracr::git::Repository {
        common_dir: id.clone(),
        git_dir: id.clone(),
        worktrees: vec![],
    };
    browser.install_repository(repo.clone());
    browser.apply_ownership(
        0,
        root.clone(),
        Outcome::Repository {
            repository: tracr::git::Directory {
                common_dir: id.clone(),
                git_dir: id.clone(),
            },
            root: Some(root.clone()),
        },
    );
    browser.invalidate_git();
    assert!(browser.retire_repositories().is_empty());
    browser.apply_ownership(1, root.clone(), Outcome::Unavailable("offline".into()));
    assert!(browser.retire_repositories().is_empty());
    browser.apply_ownership(1, root, Outcome::NotRepository);
    assert_eq!(browser.retire_repositories(), vec![id.clone()]);
    browser.apply_inventory(1, id.clone(), Ok(repo));
    assert!(!browser.repositories.contains_key(&id));
}

#[tokio::test]
async fn inventory_failure_and_recovery_render_staleness_without_losing_worktrees() {
    use ratatui::{Terminal, backend::TestBackend};
    use tracr::tui::{View, draw};
    let mut browser = loaded().await;
    let id: PathBuf = "/projects/app/.git".into();
    let wt = |path: &str| tracr::git::Worktree {
        path: path.into(),
        head: None,
        branch: None,
        bare: false,
        locked: false,
        prunable: false,
    };
    let repo = tracr::git::Repository {
        common_dir: id.clone(),
        git_dir: id.clone(),
        worktrees: vec![wt("/projects/app"), wt("/scratch/feature")],
    };
    browser.install_repository(repo.clone());
    browser.open_folder("/projects/app".into());
    let mut terminal = Terminal::new(TestBackend::new(140, 40)).unwrap();
    for failed in [true, false] {
        let result = if failed {
            Err("inventory unavailable".into())
        } else {
            Ok(repo.clone())
        };
        browser.apply_inventory(browser.git_generation, id.clone(), result);
        terminal
            .draw(|frame| draw(frame, &browser, &View::default()))
            .unwrap();
        let text: String = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|c| c.symbol())
            .collect();
        assert_eq!(text.contains("[stale]"), failed);
        assert!(text.contains("─ detached HEAD"));
        assert_eq!(browser.repositories[&id].worktrees.len(), 2);
    }
}

#[tokio::test]
async fn removed_worktree_recovers_physical_session_or_project() {
    for missing in [false, true] {
        let mut browser = loaded().await;
        let mut session = browser.all().remove(0);
        session.archived = false;
        session.cwd = Some("/scratch/feature".into());
        let selected = session.key.clone();
        browser.begin_refresh();
        browser.apply(Update::Page {
            generation: browser.generation,
            source: 0,
            sessions: vec![session],
            warnings: vec![],
        });
        browser.apply(Update::Finished {
            generation: browser.generation,
            source: 0,
            error: None,
        });
        browser.folders.insert(
            "/scratch/feature".into(),
            tracr::domain::FolderStatus::Available,
        );
        let id: PathBuf = "/projects/app/.git".into();
        let wt = |path: &str| tracr::git::Worktree {
            path: path.into(),
            head: None,
            branch: None,
            bare: false,
            locked: false,
            prunable: false,
        };
        let mut repo = tracr::git::Repository {
            common_dir: id.clone(),
            git_dir: id.clone(),
            worktrees: vec![wt("/projects/app"), wt("/scratch/feature")],
        };
        browser.install_repository(repo.clone());
        browser.open_folder("/scratch/feature".into());
        browser.selected = Some(selected.clone());
        if missing {
            browser.initial_folder = None;
            browser.folders.insert(
                "/scratch/feature".into(),
                tracr::domain::FolderStatus::Unlinked,
            );
        }
        let ancestors = browser.navigation_ancestors();
        repo.worktrees.pop();
        browser.apply_inventory(browser.git_generation, id, Ok(repo));
        browser.recover_navigation(&ancestors, Some(selected.clone()));
        if missing {
            assert_eq!(browser.folder, FolderFilter::Path("/projects/app".into()));
        } else {
            assert_eq!(
                browser.folder,
                FolderFilter::Path("/scratch/feature".into())
            );
            assert_eq!(browser.selected, Some(selected));
        }
        assert!(
            browser
                .folder_rows()
                .iter()
                .any(|r| r.filter == browser.folder)
        );
    }
}

#[tokio::test]
async fn mixed_checkout_results_keep_repository_until_all_paths_are_conclusive() {
    use tracr::git_discovery::Outcome;
    for reverse in [false, true] {
        let mut browser = loaded().await;
        let id: PathBuf = "/project/.git".into();
        let paths: Vec<PathBuf> = vec!["/project".into(), "/linked".into()];
        let owned = |path: &PathBuf| Outcome::Repository {
            repository: tracr::git::Directory {
                common_dir: id.clone(),
                git_dir: id.clone(),
            },
            root: Some(path.clone()),
        };
        browser.install_repository(tracr::git::Repository {
            common_dir: id.clone(),
            git_dir: id.clone(),
            worktrees: vec![],
        });
        for path in &paths {
            browser.apply_ownership(0, path.clone(), owned(path));
        }
        browser.invalidate_git();
        let mut updates = vec![
            (paths[0].clone(), Outcome::Unavailable("offline".into())),
            (paths[1].clone(), owned(&paths[1])),
        ];
        if reverse {
            updates.reverse();
        }
        for (path, result) in updates {
            browser.apply_ownership(1, path, result);
            assert!(browser.retire_repositories().is_empty());
        }
        assert!(browser.ownership[&paths[0]].stale());
        assert!(matches!(
            browser.ownership[&paths[0]].effective(),
            Some(Outcome::Repository { .. })
        ));
        browser.apply_ownership(1, paths[1].clone(), Outcome::NotRepository);
        assert!(browser.retire_repositories().is_empty());
        browser.apply_ownership(1, paths[0].clone(), Outcome::NotRepository);
        assert_eq!(browser.retire_repositories(), vec![id]);
    }
}

#[tokio::test]
async fn inventory_refresh_preserves_collapsed_projects_and_session_focus() {
    use ratatui::{Terminal, backend::TestBackend};
    use tracr::tui::{View, draw};
    let mut browser = loaded().await;
    let session = browser
        .all()
        .into_iter()
        .find(|s| !s.archived && s.cwd.is_some())
        .unwrap();
    browser.open_folder(session.cwd.clone().unwrap());
    browser.selected = Some(session.key.clone());
    let original_folder = browser.folder.clone();
    let wt = |path: &str| tracr::git::Worktree {
        path: path.into(),
        head: None,
        branch: None,
        bare: false,
        locked: false,
        prunable: false,
    };
    let repos: Vec<_> = ["/project", "/other"]
        .into_iter()
        .map(|path| tracr::git::Repository {
            common_dir: PathBuf::from(path).join(".git"),
            git_dir: PathBuf::from(path).join(".git"),
            worktrees: vec![wt(path), wt(&format!("{path}-linked"))],
        })
        .collect();
    for repo in &repos {
        browser.install_repository(repo.clone());
    }
    browser.expanded.remove(&PathBuf::from("/project"));
    browser.expanded.remove(&PathBuf::from("/other"));
    let expanded = browser.expanded.clone();
    let view = View {
        focus: 1,
        ..View::default()
    };
    let mut terminal = Terminal::new(TestBackend::new(140, 40)).unwrap();
    for repo in repos {
        let ancestors = browser.navigation_ancestors();
        let selected = browser.selected.clone();
        browser.apply_inventory(browser.git_generation, repo.common_dir.clone(), Ok(repo));
        browser.recover_navigation(&ancestors, selected);
        terminal.draw(|frame| draw(frame, &browser, &view)).unwrap();
        assert!(!browser.expanded.contains(&PathBuf::from("/project")));
        assert!(!browser.expanded.contains(&PathBuf::from("/other")));
        assert_eq!(browser.expanded, expanded);
        assert_eq!(browser.folder, original_folder);
        assert_eq!(browser.selected, Some(session.key.clone()));
        assert_eq!(view.focus, 1);
        let text: String = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect();
        assert!(!text.contains("project-linked"));
        assert!(!text.contains("other-linked"));
    }
}

#[tokio::test]
async fn worktree_labels_and_scope_titles_use_repository_identity() {
    use tracr::git_discovery::Outcome;
    let mut browser = loaded().await;
    let id: PathBuf = "/project/.git".into();
    let wt = |path: &str, branch: Option<&str>| tracr::git::Worktree {
        path: path.into(),
        head: Some("abcdef1234567890".into()),
        branch: branch.map(str::to_owned),
        bare: false,
        locked: false,
        prunable: false,
    };
    browser.install_repository(tracr::git::Repository {
        common_dir: id.clone(),
        git_dir: id.clone(),
        worktrees: vec![
            wt("/project", Some("refs/heads/main")),
            wt("/unimportant-name", Some("refs/heads/feature/login")),
            wt("/detached-name", None),
        ],
    });
    for path in ["/project", "/unimportant-name"] {
        browser.apply_ownership(
            browser.git_generation,
            path.into(),
            Outcome::Repository {
                repository: tracr::git::Directory {
                    common_dir: id.clone(),
                    git_dir: id.clone(),
                },
                root: Some(path.into()),
            },
        );
    }
    for path in [
        "/project",
        "/unimportant-name",
        "/unimportant-name/src",
        "/detached-name",
    ] {
        browser.folder = FolderFilter::Path(path.into());
        assert_eq!(browser.scope_label(), "Sessions");
        let mut terminal =
            ratatui::Terminal::new(ratatui::backend::TestBackend::new(140, 40)).unwrap();
        terminal
            .draw(|frame| tracr::tui::draw(frame, &browser, &tracr::tui::View::default()))
            .unwrap();
        let text: String = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect();
        assert!(text.contains("Sessions"));
    }
    let rows = browser.folder_rows();
    assert_eq!(
        rows.iter()
            .find(|r| r.filter == FolderFilter::Path("/unimportant-name".into()))
            .unwrap()
            .label,
        "feature/login"
    );
    assert_eq!(
        rows.iter()
            .find(|r| r.filter == FolderFilter::Path("/detached-name".into()))
            .unwrap()
            .label,
        "detached HEAD abcdef1"
    );
    browser.invalidate_git();
    browser.folder = FolderFilter::Path("/unimportant-name".into());
    assert_eq!(browser.scope_label(), "Sessions");
}

#[tokio::test]
async fn folder_pane_scrolls_whole_tree_and_resets_for_short_selection_and_resize() {
    use ratatui::{Terminal, backend::TestBackend};
    use tracr::tui::{View, draw};
    let mut browser = loaded().await;
    let path: PathBuf =
        "/parent/long-parent/another-parent/deep/feature-with-a-very-long-name-and-extra-overflow-界界".into();
    browser.open_folder(path.clone());
    let view = View {
        focus: 0,
        ..View::default()
    };
    for width in [100u16, 140, 240, 80] {
        browser.folder = FolderFilter::Path(path.clone());
        let mut terminal = Terminal::new(TestBackend::new(width, 40)).unwrap();
        terminal.draw(|frame| draw(frame, &browser, &view)).unwrap();
        let pane_width = if width >= 100 {
            width * 50 / 100
        } else {
            width
        };
        let lines = |terminal: &Terminal<TestBackend>| -> Vec<String> {
            (0..40)
                .map(|y| {
                    (0..pane_width)
                        .map(|x| terminal.backend().buffer()[(x, y)].symbol())
                        .collect()
                })
                .collect()
        };
        assert_eq!(
            terminal.backend().buffer()[(pane_width - 1, 0)].symbol(),
            "┓"
        );
        if width >= 100 {
            assert_eq!(terminal.backend().buffer()[(pane_width, 0)].symbol(), "┌");
        }
        let before = lines(&terminal);
        let selected = before
            .iter()
            .find(|line| line.starts_with("┃> ") || line.starts_with("│> "))
            .unwrap();
        assert!(selected.contains("界"), "{selected}");
        assert!(selected.contains("(0)"), "{selected}");
        assert!(before[0].contains("Folders"));
        if width == 100 {
            // The same offset clips short ancestor rows completely.
            assert!(!before.iter().any(|line| line.contains("long-parent")));
        }
        browser.folder = FolderFilter::Unlinked;
        terminal.draw(|frame| draw(frame, &browser, &view)).unwrap();
        let after = lines(&terminal);
        assert!(after.iter().any(|line| line.contains("Unlinked folders")));
        assert!(after.iter().any(|line| line.contains("long-parent")));
    }
}

#[tokio::test]
async fn sessions_have_two_columns_of_right_padding() {
    use ratatui::{Terminal, backend::TestBackend};
    use tracr::tui::{View, draw};
    let mut browser = loaded().await;
    let mut session = browser.all().into_iter().find(|s| !s.archived).unwrap();
    session.title = "Long session title ".repeat(50);
    browser.begin_refresh();
    browser.apply(Update::Page {
        generation: browser.generation,
        source: 0,
        sessions: vec![session],
        warnings: vec![],
    });
    browser.apply(Update::Finished {
        generation: browser.generation,
        source: 0,
        error: None,
    });
    for empty in [false, true] {
        if empty {
            browser.folder = FolderFilter::Path("/empty-padding-fixture".into());
        }
        for width in [80, 100, 140] {
            let mut terminal = Terminal::new(TestBackend::new(width, 40)).unwrap();
            terminal
                .draw(|frame| {
                    draw(
                        frame,
                        &browser,
                        &View {
                            focus: 1,
                            ..View::default()
                        },
                    )
                })
                .unwrap();
            let buffer = terminal.backend().buffer();
            for y in 1..37 {
                assert_eq!(buffer[(width - 2, y)].symbol(), " ");
                assert_eq!(buffer[(width - 3, y)].symbol(), " ");
            }
        }
    }
}

#[tokio::test]
async fn identical_provider_updates_do_not_invalidate_the_browser() {
    let mut browser = loaded().await;
    let update = || Update::Live {
        generation: 0,
        source: 0,
        statuses: std::collections::BTreeMap::new(),
        error: None,
    };
    let generation = browser.generation;
    let live = |statuses| Update::Live {
        generation,
        source: 0,
        statuses,
        error: None,
    };
    browser.apply(live(std::collections::BTreeMap::new()));
    assert!(!browser.update_changes(&live(std::collections::BTreeMap::new())));
    let key = browser.all()[0].key.clone();
    let statuses = std::collections::BTreeMap::from([(key, RuntimeStatus::Idle)]);
    assert!(browser.update_changes(&live(statuses.clone())));
    browser.apply(live(statuses.clone()));
    assert!(!browser.update_changes(&live(statuses)));
    if generation != 0 {
        assert!(!browser.update_changes(&update()));
    }
    let folder = || Update::Folder {
        generation,
        path: "/test".into(),
        status: tracr::domain::FolderStatus::Available,
    };
    browser.apply(folder());
    assert!(!browser.update_changes(&folder()));
}
