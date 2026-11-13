use serde_json::json;
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};
use tracr::{
    browser::{Browser, FolderFilter, SourceSpec, Update},
    config::{SourceConfig, source_id},
    domain::{ProviderId, RuntimeStatus, SessionKey},
    providers::codex_live::LiveClient,
};
fn setup(settings: serde_json::Value) -> (tempfile::TempDir, PathBuf, SourceConfig) {
    let home = tempfile::tempdir().unwrap();
    std::fs::write(home.path().join("fake.json"), settings.to_string()).unwrap();
    let source = SourceConfig {
        id: source_id(&ProviderId("codex".into()), home.path()),
        home: home.path().to_owned(),
    };
    (
        home,
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/app-server/fake.py"),
        source,
    )
}
#[tokio::test]
async fn passive_snapshot_paginates_maps_status_and_rejects_failures() {
    let (home, binary, source) = setup(json!({"live_threads":{
        "a":{"type":"active","activeFlags":[]},
        "b":{"type":"active","activeFlags":["waitingOnApproval","waitingOnUserInput"]},
        "c":{"type":"idle"}, "d":{"type":"systemError"}, "e":{"type":"future"}
    }}));
    let mut client = LiveClient::connect(&binary, source.clone()).await.unwrap();
    let key = |id: &str| SessionKey {
        provider: ProviderId("codex".into()),
        source_id: source.id.clone(),
        native_id: id.into(),
    };
    let status = client.snapshot().await.unwrap();
    assert_eq!(status.len(), 5);
    assert_eq!(status[&key("a")], RuntimeStatus::Running);
    assert_eq!(
        status[&key("b")],
        RuntimeStatus::Waiting {
            approval: true,
            input: true
        }
    );
    assert_eq!(status[&key("c")], RuntimeStatus::Idle);
    assert_eq!(status[&key("d")], RuntimeStatus::Error);
    assert_eq!(status[&key("e")], RuntimeStatus::Unknown);
    let settings = home.path().join("fake.json");
    std::fs::write(&settings,json!({"live_enabled":true,"live_threads":{"a":{"type":"active","activeFlags":[]}},"live_source":{"subAgent":{"other":"guardian"}}}).to_string()).unwrap();
    assert!(client.snapshot().await.unwrap().is_empty());
    std::fs::write(
        &settings,
        json!({"live_enabled":true,"live_repeat_cursor":true}).to_string(),
    )
    .unwrap();
    assert!(client.snapshot().await.unwrap_err().contains("cursor"));
    std::fs::write(&settings, json!({"live_disconnect":true}).to_string()).unwrap();
    assert!(client.snapshot().await.is_err());
    let log = std::fs::read_to_string(home.path().join("live_requests.jsonl")).unwrap();
    assert!(!log.contains("thread/resume"));
    assert!(!log.contains("turn/start"));
}
#[test]
fn snapshots_are_source_and_generation_scoped() {
    let (_, binary, source) = setup(json!({}));
    let key = SessionKey {
        provider: ProviderId("codex".into()),
        source_id: source.id.clone(),
        native_id: "same".into(),
    };
    let mut browser = Browser::new(vec![SourceSpec::Codex { binary, source }]);
    browser.begin_refresh();
    browser.apply(Update::Live {
        generation: 0,
        source: 0,
        statuses: BTreeMap::from([(key.clone(), RuntimeStatus::Running)]),
        error: None,
    });
    assert_eq!(browser.runtime_status(&key), RuntimeStatus::Unknown);
    browser.apply(Update::Live {
        generation: 1,
        source: 0,
        statuses: BTreeMap::from([(key.clone(), RuntimeStatus::Running)]),
        error: None,
    });
    assert_eq!(browser.runtime_status(&key), RuntimeStatus::Running);
    browser.apply(Update::Live {
        generation: 1,
        source: 0,
        statuses: BTreeMap::new(),
        error: Some("offline".into()),
    });
    assert_eq!(browser.runtime_status(&key), RuntimeStatus::Unknown);
    assert_eq!(
        browser.folder_activity(&FolderFilter::Path("/".into())),
        RuntimeStatus::Unknown
    );
}

#[test]
fn folder_colors_follow_visible_descendants_and_waiting_wins() {
    use ratatui::{Terminal, backend::TestBackend, style::Color};
    use tracr::domain::SessionSummary;
    let mut browser = Browser::new(vec![SourceSpec::Fixture("fixture".into())]);
    browser.open_folder("/work/app".into());
    browser.begin_refresh();
    let session = |id: &str, path: &str, archived: bool| -> SessionSummary {
        serde_json::from_value(json!({"key":{"provider":"codex","source_id":"fixture","native_id":id},"title":"A long session title with Unicode 界界界界 and more text", "cwd":path,"created_at":1,"updated_at":2,"archived":archived,"source_kind":null,"git":null})).unwrap()
    };
    let running = session("a", "/work/app", false);
    let waiting = session("b", "/work/app/child", false);
    let archived = session("c", "/work/other", true);
    browser.apply(Update::Page {
        generation: 1,
        source: 0,
        sessions: vec![running.clone(), waiting.clone(), archived.clone()],
        warnings: vec![],
    });
    browser.apply(Update::Finished {
        generation: 1,
        source: 0,
        error: None,
    });
    browser.apply(Update::Live {
        generation: 1,
        source: 0,
        statuses: BTreeMap::from([
            (running.key.clone(), RuntimeStatus::Running),
            (
                waiting.key.clone(),
                RuntimeStatus::Waiting {
                    approval: true,
                    input: false,
                },
            ),
            (archived.key.clone(), RuntimeStatus::Running),
        ]),
        error: None,
    });
    let folder = |path: &str| FolderFilter::Path(path.into());
    assert!(matches!(
        browser.folder_activity(&folder("/work/app")),
        RuntimeStatus::Waiting { .. }
    ));
    assert!(matches!(
        browser.folder_activity(&folder("/work")),
        RuntimeStatus::Waiting { .. }
    ));
    assert_eq!(
        browser.folder_activity(&folder("/work/ap")),
        RuntimeStatus::Unknown
    );
    assert_eq!(
        browser.folder_activity(&folder("/work/other")),
        RuntimeStatus::Unknown
    );
    browser.include_archived = true;
    assert_eq!(
        browser.folder_activity(&folder("/work/other")),
        RuntimeStatus::Running
    );
    let view = tracr::tui::View {
        focus: 1,
        ..Default::default()
    };
    let mut terminal = Terminal::new(TestBackend::new(140, 40)).unwrap();
    terminal
        .draw(|f| tracr::tui::draw(f, &browser, &view))
        .unwrap();
    let buffer = terminal.backend().buffer();
    assert!(
        buffer
            .content
            .iter()
            .any(|c| c.symbol() == "R" && c.fg == Color::Green)
    );
    assert!(
        buffer
            .content
            .iter()
            .any(|c| c.fg == Color::Rgb(255, 165, 0))
    );
    browser.open_folder("/work/app/child".into());
    let mut terminal = Terminal::new(TestBackend::new(36, 15)).unwrap();
    terminal
        .draw(|f| tracr::tui::draw(f, &browser, &view))
        .unwrap();
    let text: String = terminal
        .backend()
        .buffer()
        .content
        .iter()
        .map(|c| c.symbol())
        .collect();
    assert!(text.contains("Waiting"));
    browser.apply(Update::Live {
        generation: 1,
        source: 0,
        statuses: BTreeMap::new(),
        error: None,
    });
    assert_eq!(
        browser.folder_activity(&folder("/work")),
        RuntimeStatus::Unknown
    );
}

#[test]
fn identical_native_ids_in_different_homes_keep_independent_status() {
    let mut browser = Browser::new(vec![
        SourceSpec::Fixture("one".into()),
        SourceSpec::Fixture("two".into()),
    ]);
    browser.begin_refresh();
    let key = |source: &str| SessionKey {
        provider: ProviderId("codex".into()),
        source_id: tracr::domain::SourceId(source.into()),
        native_id: "same".into(),
    };
    browser.apply(Update::Live {
        generation: 1,
        source: 0,
        statuses: BTreeMap::from([(key("one"), RuntimeStatus::Running)]),
        error: None,
    });
    browser.apply(Update::Live {
        generation: 1,
        source: 1,
        statuses: BTreeMap::from([(
            key("two"),
            RuntimeStatus::Waiting {
                approval: false,
                input: true,
            },
        )]),
        error: None,
    });
    browser.apply(Update::Live {
        generation: 1,
        source: 0,
        statuses: BTreeMap::new(),
        error: Some("disconnected".into()),
    });
    assert_eq!(browser.runtime_status(&key("one")), RuntimeStatus::Unknown);
    assert_eq!(
        browser.runtime_status(&key("two")),
        RuntimeStatus::Waiting {
            approval: false,
            input: true
        }
    );
    browser.begin_refresh();
    assert_eq!(browser.runtime_status(&key("two")), RuntimeStatus::Unknown);
}
