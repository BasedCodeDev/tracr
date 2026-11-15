use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::{Terminal as Screen, backend::TestBackend, layout::Rect};
use tracr::{
    providers::codex_terminals::{Blocker, Terminal},
    terminal_recovery::{Dialog, Event},
};
fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}
fn commands() -> Blocker {
    Blocker::Commands(vec![Terminal {
        process_id: "p".into(),
        item_id: "i".into(),
        command: "sleep 300; 界 ".repeat(80),
        cwd: "/temporary/project".into(),
    }])
}
#[test]
fn confirmation_defaults_to_cancel_and_unknown_state_cannot_stop() {
    let mut d = Dialog::new(commands(), "Session", "/old", "/new");
    assert_eq!(d.key(key(KeyCode::Enter)), Event::Cancel);
    assert_eq!(d.key(key(KeyCode::Tab)), Event::None);
    assert_eq!(d.key(key(KeyCode::Enter)), Event::Refresh);
    d.key(key(KeyCode::Tab));
    assert_eq!(d.key(key(KeyCode::Enter)), Event::Stop);
    d.busy = true;
    assert_eq!(d.key(key(KeyCode::Enter)), Event::None);
    assert_eq!(d.key(key(KeyCode::Esc)), Event::None);
    let mut d = Dialog::new(
        Blocker::Unavailable("unsupported".into()),
        "Session",
        "/old",
        "/new",
    );
    for _ in 0..8 {
        d.key(key(KeyCode::Tab));
        assert_ne!(d.key(key(KeyCode::Enter)), Event::Stop);
    }
    let mut d = Dialog::new(Blocker::Commands(vec![]), "Session", "/old", "/new");
    d.key(key(KeyCode::Left));
    assert_eq!(d.key(key(KeyCode::Enter)), Event::Retry);
}
#[test]
fn commands_wrap_scroll_and_resize_without_entering_persistent_reports() {
    let mut d = Dialog::new(commands(), "Session", "/old", "/new");
    let mut terminal = Screen::new(TestBackend::new(100, 30)).unwrap();
    for (width, height) in [(100, 30), (50, 20), (30, 12)] {
        terminal.backend_mut().resize(width, height);
        terminal.resize(Rect::new(0, 0, width, height)).unwrap();
        terminal.draw(|f| d.draw(f)).unwrap();
        for _ in 0..12 {
            d.key(key(KeyCode::PageDown));
        }
        terminal.draw(|f| d.draw(f)).unwrap();
        d.key(key(KeyCode::PageUp));
        terminal.draw(|f| d.draw(f)).unwrap();
    }
    let failure = commands().failure();
    let json = serde_json::to_string(&failure).unwrap();
    assert!(!json.contains("sleep"));
    assert!(!json.contains("temporary/project"));
    assert!(
        serde_json::from_str::<tracr::relocation::Failure>(&json)
            .unwrap()
            .terminals
            .is_none()
    );
}

#[test]
fn still_loaded_dialog_offers_only_cancel_and_explicit_retry() {
    let mut dialog = Dialog::still_loaded("Session", "/old", "/new");
    assert!(dialog.reviewed().is_none());
    assert_eq!(dialog.key(key(KeyCode::Enter)), Event::Cancel);
    dialog.key(key(KeyCode::Tab));
    assert_eq!(dialog.key(key(KeyCode::Enter)), Event::Retry);
    dialog.busy = true;
    assert_eq!(dialog.key(key(KeyCode::Enter)), Event::None);
    let mut screen = Screen::new(TestBackend::new(100, 30)).unwrap();
    screen.draw(|frame| dialog.draw(frame)).unwrap();
    let text = screen
        .backend()
        .buffer()
        .content
        .iter()
        .map(|c| c.symbol())
        .collect::<String>();
    assert!(text.contains("Session is still loaded"));
    assert!(text.contains("If already closed"));
    assert!(!text.contains("Stop listed commands"));
    assert!(!text.contains("Refresh"));
}
