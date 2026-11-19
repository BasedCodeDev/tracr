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
