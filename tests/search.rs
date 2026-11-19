use std::{os::unix::fs::PermissionsExt, path::Path};
use tracr::{
    config::SourceConfig,
    domain::{ProviderId, SessionKey, SourceId},
    providers::{SessionProvider, codex::CodexProvider},
    search::ContentKind,
};

fn key() -> SessionKey {
    SessionKey {
        provider: ProviderId("codex".into()),
        source_id: SourceId("test".into()),
        native_id: "thread".into(),
    }
}
fn fake(root: &Path, mode: &str) -> std::path::PathBuf {
    std::fs::write(root.join("mode"), mode).unwrap();
    let binary = root.join("fake");
    std::fs::write(&binary, r#"#!/usr/bin/env python3
import json, os, sys
from pathlib import Path
root=Path(os.environ['CODEX_HOME'])
mode=(root/'mode').read_text()
turn={'id':'t', 'status':'completed', 'items':[{'type':'userMessage','id':'u','content':[{'type':'text','text':'Forward AltTab'}]}]}
for line in sys.stdin:
 r=json.loads(line)
 with (root/'requests').open('a') as out: out.write(json.dumps(r)+'\n')
 if 'id' not in r: continue
 method=r['method']; p=r.get('params',{})
 if method=='initialize': result={}
 elif method=='thread/turns/list':
  assert p['itemsView']=='full'
  if mode=='fallback':
   print(json.dumps({'id':r['id'],'error':{'code':-32601,'message':'Method not found'}}),flush=True); continue
  if mode=='failure':
   print(json.dumps({'id':r['id'],'error':{'code':-32000,'message':'Unreadable history'}}),flush=True); continue
  if mode=='oversize':
   print(json.dumps({'id':r['id'],'result':{'data':'x'*(8*1024*1024)}}),flush=True); continue
  if p.get('cursor'):
   result={'data':[{'id':'t2','status':'completed','items':[{'type':'agentMessage','phase':'final_answer','id':'a','text':'Ninja One'}]}], 'nextCursor': 'next' if mode=='repeat' else None}
  else: result={'data':[turn],'nextCursor':'next'}
 elif method=='thread/read':
  assert p['includeTurns'] is True
  result={'thread':{'id':'thread','turns':[turn]}}
 else: raise AssertionError(method)
 print(json.dumps({'id':r['id'],'result':result}),flush=True)
"#).unwrap();
    std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o755)).unwrap();
    binary
}
#[tokio::test]
async fn paginated_history_and_unsupported_method_fallback_are_read_only() {
    for mode in ["pages", "fallback", "repeat", "failure", "oversize"] {
        let root = tempfile::tempdir().unwrap();
        let binary = fake(root.path(), mode);
        let provider = CodexProvider::connect(
            &binary,
            SourceConfig {
                id: key().source_id,
                home: root.path().into(),
            },
        )
        .await
        .unwrap();
        let content = provider.search_content(&key()).await;
        if mode == "pages" {
            let content = content.unwrap();
            assert_eq!(content.len(), 2);
            assert_eq!(content[0].kind, ContentKind::User);
            assert_eq!(content[1].text, "Ninja One");
        } else if mode == "fallback" {
            assert_eq!(content.unwrap().len(), 1);
        } else {
            assert!(content.is_err(), "{mode}");
        }
        provider.close().await;
        let requests = std::fs::read_to_string(root.path().join("requests")).unwrap();
        assert!(!requests.contains("thread/resume"));
        assert!(!requests.contains("turn/start"));
        assert_eq!(requests.contains("thread/read"), mode == "fallback");
    }
}

#[tokio::test]
#[ignore = "Requires a disposable synthetic Codex fixture, never a user home"]
async fn real_codex_search() {
    let root = std::path::PathBuf::from(
        std::env::var_os("TRACR_SEARCH_FIXTURE").expect("synthetic fixture"),
    );
    let binary = std::path::PathBuf::from(std::env::var_os("TRACR_SEARCH_CODEX").expect("binary"));
    let id = std::fs::read_to_string(root.join("search-id")).unwrap();
    let provider = CodexProvider::connect(
        &binary,
        SourceConfig {
            id: key().source_id,
            home: root.join("codex"),
        },
    )
    .await
    .unwrap();
    let content = provider
        .search_content(&SessionKey {
            native_id: id,
            ..key()
        })
        .await
        .unwrap();
    assert!(
        content
            .iter()
            .any(|c| c.kind == ContentKind::User && c.text.contains("synthetic project")),
        "{content:?}"
    );
    assert!(
        content
            .iter()
            .any(|c| c.kind == ContentKind::Final && c.text.contains("synthetic project")),
        "{content:?}"
    );
    provider.close().await;
}

#[tokio::test]
async fn background_search_reuses_cache_and_refresh_invalidates_it() {
    use tracr::{
        browser::{Browser, SourceSpec, Update},
        domain::SessionSummary,
        search::Search,
    };
    let root = tempfile::tempdir().unwrap();
    let binary = fake(root.path(), "pages");
    let mut browser = Browser::new(vec![SourceSpec::Codex {
        binary,
        source: SourceConfig {
            id: key().source_id,
            home: root.path().into(),
        },
    }]);
    let session = SessionSummary {
        key: key(),
        title: "Unrelated".into(),
        cwd: Some(root.path().into()),
        created_at: 1,
        updated_at: 2,
        archived: false,
        can_accept_direct_input: None,
        source_kind: None,
        git: None,
    };
    browser.apply(Update::Page {
        generation: 0,
        source: 0,
        sessions: vec![session.clone()],
        warnings: vec![],
    });
    browser.deep_search = true;
    let mut search = Search::default();
    search.tick(&mut browser, false);
    assert!(!root.path().join("requests").exists());
    search.tick(&mut browser, true);
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        while browser.search_cache.is_empty() {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            search.tick(&mut browser, true);
        }
    })
    .await
    .unwrap();
    assert_eq!(browser.search_results("alttab ninja").len(), 1);
    let mut view = tracr::tui::View {
        editing: true,
        search_query: "alttab ninja".into(),
        ..Default::default()
    };
    let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(120, 35)).unwrap();
    terminal
        .draw(|frame| tracr::tui::draw(frame, &browser, &view))
        .unwrap();
    let screen: String = terminal
        .backend()
        .buffer()
        .content
        .iter()
        .map(|cell| cell.symbol())
        .collect();
    assert!(screen.contains("You: Forward AltTab"), "{screen}");
    tracr::tui::handle_key(
        &mut browser,
        &mut view,
        crossterm::event::KeyEvent::new(
            crossterm::event::KeyCode::Enter,
            crossterm::event::KeyModifiers::NONE,
        ),
    );
    assert!(!view.editing);
    assert_eq!(browser.selected.as_ref(), Some(&key()));

    let requests = std::fs::read(root.path().join("requests")).unwrap();
    search.tick(&mut browser, false);
    search.tick(&mut browser, true);
    tokio::time::sleep(std::time::Duration::from_millis(40)).await;
    assert_eq!(
        std::fs::read(root.path().join("requests")).unwrap(),
        requests
    );
    let mut updated = session.clone();
    updated.updated_at += 1;
    browser.apply(Update::Page {
        generation: 0,
        source: 0,
        sessions: vec![updated],
        warnings: vec![],
    });
    assert!(browser.search_results("alttab").is_empty());
    search.tick(&mut browser, true);
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        while browser.search_results("alttab").is_empty() {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            search.tick(&mut browser, true);
        }
    })
    .await
    .unwrap();
    browser.begin_refresh();
    search.tick(&mut browser, false);
    assert!(browser.search_cache.is_empty());
}

#[test]
fn search_layout_expands_spaces_results_and_keeps_selection_visible() {
    use ratatui::{Terminal, backend::TestBackend};
    use tracr::{
        browser::{Browser, SourceSpec, Update},
        domain::SessionSummary,
        search::{CachedContent, Content},
        tui::View,
    };
    let mut browser = Browser::new(vec![SourceSpec::Fixture("unused".into())]);
    let sessions: Vec<SessionSummary> = (0..30)
        .map(|i| {
            serde_json::from_value(serde_json::json!({
        "key":{"provider":"codex","source_id":"test","native_id":format!("{i:02}")},
        "title":format!("Result {i:02}"), "cwd":"/sysadmin", "created_at":1, "updated_at":30-i,
        "git":null, "source_kind":null
    })).unwrap()
        })
        .collect();
    browser.apply(Update::Page {
        generation: 0,
        source: 0,
        sessions: sessions.clone(),
        warnings: vec![],
    });
    browser.focus_root = Some("/sysadmin".into());
    for session in &sessions {
        browser.search_cache.insert(
            session.key.clone(),
            CachedContent::new(
                session.clone(),
                Ok(vec![Content {
                    turn_id: "t".into(),
                    item_id: "u".into(),
                    kind: ContentKind::User,
                    text: "Result excerpt".into(),
                }]),
            ),
        );
    }
    let mut view = View {
        editing: true,
        search_query: "Result".into(),
        ..View::default()
    };
    let render = |browser: &Browser, view: &View, width, height| {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal
            .draw(|frame| tracr::tui::draw(frame, browser, view))
            .unwrap();
        let buffer = terminal.backend().buffer();
        assert_eq!(buffer[(2, 1)].symbol(), "╭");
        assert_eq!(buffer[(width - 3, height - 2)].symbol(), "╯");
        (0..height)
            .map(|y| {
                (0..width)
                    .map(|x| buffer[(x, y)].symbol())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
    };
    for deep in [false, true] {
        browser.deep_search = deep;
        let normal = render(&browser, &view, 100, 30);
        let large = render(&browser, &view, 160, 50);
        let count = |rows: &[String]| {
            rows.iter()
                .filter(|row| row.contains("Result ") && !row.contains("excerpt"))
                .count()
        };
        assert!(count(&large) > count(&normal));
        let first = large
            .iter()
            .position(|row| row.contains("Result 00"))
            .unwrap();
        let second = large
            .iter()
            .position(|row| row.contains("Result 01"))
            .unwrap();
        assert_eq!(second - first, if deep { 4 } else { 3 });
        assert!(!large[second - 1].chars().any(char::is_alphanumeric));
        assert_eq!(large.iter().any(|row| row.contains("Content:")), deep);
        view.search_selected = Some(sessions[29].key.clone());
        for (width, height) in [(160, 50), (80, 24), (40, 18), (60, 12)] {
            let rows = render(&browser, &view, width, height);
            assert!(
                rows.iter().any(|row| row.contains("Result 29")),
                "{width}x{height}: {rows:?}"
            );
            assert!(rows.iter().any(|row| row.contains("Deep search:")));
        }
        view.search_selected = None;
    }
}
