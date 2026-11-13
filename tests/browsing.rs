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
        focus: 2,
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
    assert!(text.contains("DIRECTORY MISMATCH"));
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
async fn arrows_navigate_panes_and_folders_outside_search() {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use tracr::tui::{View, handle_key};
    let mut browser = loaded().await;
    let mut view = View::default();
    let key = |code| KeyEvent::new(code, KeyModifiers::NONE);
    {
        let editing = false;
        view.editing = editing;
        view.focus = 0;
        browser.folder = FolderFilter::All;
        browser.query = "codex".into();
        handle_key(&mut browser, &mut view, key(KeyCode::Down));
        assert_eq!(browser.folder, FolderFilter::Unlinked);
        handle_key(&mut browser, &mut view, key(KeyCode::Up));
        assert_eq!(browser.folder, FolderFilter::All);
        let path = PathBuf::from("/work/project");
        browser.open_folder(path.clone());
        browser.expanded.remove(&path);
        handle_key(&mut browser, &mut view, key(KeyCode::Enter));
        assert!(browser.expanded.contains(&path));
        assert_eq!(view.editing, editing);
        handle_key(&mut browser, &mut view, key(KeyCode::Enter));
        assert!(!browser.expanded.contains(&path));
        for expected in [1, 2, 0] {
            handle_key(&mut browser, &mut view, key(KeyCode::Right));
            assert_eq!(view.focus, expected);
            assert!(!browser.expanded.contains(&path));
        }
        handle_key(&mut browser, &mut view, key(KeyCode::Left));
        assert_eq!(view.focus, 2);
        handle_key(&mut browser, &mut view, key(KeyCode::Down));
        assert_eq!(view.details_scroll, 1);
        handle_key(&mut browser, &mut view, key(KeyCode::Tab));
        assert_eq!(view.focus, 0);
        handle_key(&mut browser, &mut view, key(KeyCode::BackTab));
        assert_eq!(view.focus, 2);
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
fn escape_matches_left_navigation_but_still_closes_help_and_cancels_resume() {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use tracr::tui::{Action, View, handle_key};
    let mut browser = Browser::new(vec![]);
    browser.query = "keep filter".into();
    {
        let editing = false;
        for focus in 0..3 {
            for code in [KeyCode::Esc, KeyCode::Left] {
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
                assert_eq!(view.focus, (focus + 2) % 3);
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
        focus: 2,
        details_scroll: 3,
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
    assert_eq!(view.focus, 2);
    assert_eq!(view.details_scroll, 3);
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
    assert_eq!(view.details_scroll, 0);
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
