//! Ephemeral, read-only Git selection previews. Never changes browser selection or PTY ownership.
use crate::{
    browser::SourceSpec,
    domain::SessionKey,
    providers::{codex::Rpc, codex_live::PreviewClient},
};
use serde_json::{Value, json};
use std::{
    path::PathBuf,
    time::{Duration, Instant},
};
use tokio::{
    sync::{oneshot, watch},
    task::JoinHandle,
};

const MAX_ITEMS: usize = 160;
const MAX_TEXT: usize = 16 * 1024;

#[derive(Clone, Debug, Default, PartialEq)]
pub enum Mode {
    #[default]
    Loading,
    Live,
    History,
    List,
}
#[derive(Clone, Debug, Default, PartialEq)]
struct Item {
    turn: String,
    id: String,
    text: String,
    partial: bool,
    complete: bool,
}
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Snapshot {
    pub mode: Mode,
    pub notice: String,
    items: Vec<Item>,
}
fn bounded(text: &str) -> String {
    text.chars().take(MAX_TEXT).collect()
}
fn item_text(item: &Value) -> String {
    match item["type"].as_str().unwrap_or("") {
        "agentMessage" => format!("Assistant: {}", item["text"].as_str().unwrap_or("")),
        "userMessage" => format!(
            "You: {}",
            item["content"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|v| v["text"].as_str())
                .collect::<Vec<_>>()
                .join("\n")
        ),
        "commandExecution" => format!(
            "Command: {}\n{}",
            item["command"].as_str().unwrap_or(""),
            item["aggregatedOutput"].as_str().unwrap_or("")
        ),
        "fileChange" => format!(
            "File changes: {}",
            item["changes"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|v| v["path"].as_str())
                .collect::<Vec<_>>()
                .join(", ")
        ),
        "mcpToolCall" => format!(
            "Tool: {}/{} · {}",
            item["server"].as_str().unwrap_or(""),
            item["tool"].as_str().unwrap_or(""),
            item["status"].as_str().unwrap_or("")
        ),
        kind => format!("{} · {}", kind, item["status"].as_str().unwrap_or("")),
    }
}
impl Snapshot {
    fn put(&mut self, turn: &str, id: &str, text: String, partial: bool) {
        if id.is_empty() {
            return;
        }
        let item = Item {
            turn: turn.into(),
            id: id.into(),
            text: bounded(&text),
            partial,
            complete: false,
        };
        if let Some(old) = self.items.iter_mut().find(|v| v.turn == turn && v.id == id) {
            *old = item;
        } else {
            self.items.push(item);
        }
        if self.items.len() > MAX_ITEMS {
            self.items.drain(..self.items.len() - MAX_ITEMS);
        }
    }
    fn history(&mut self, turns: &[Value]) {
        for turn in turns {
            for item in turn["items"].as_array().into_iter().flatten() {
                self.put(
                    turn["id"].as_str().unwrap_or(""),
                    item["id"].as_str().unwrap_or(""),
                    item_text(item),
                    false,
                );
                if turn["status"].as_str() == Some("completed") {
                    if let Some(saved) = self.items.iter_mut().find(|v| {
                        v.turn == turn["id"].as_str().unwrap_or("")
                            && v.id == item["id"].as_str().unwrap_or("")
                    }) {
                        saved.complete = true;
                    }
                }
            }
        }
    }
    fn event(&mut self, thread: &str, event: &Value) {
        let p = &event["params"];
        if p["threadId"].as_str() != Some(thread) {
            return;
        }
        let method = event["method"].as_str().unwrap_or("");
        let turn = p["turnId"].as_str().unwrap_or("");
        match method {
            "item/started" | "item/completed" => {
                let item = &p["item"];
                let id = item["id"].as_str().unwrap_or("");
                if method == "item/started"
                    && self
                        .items
                        .iter()
                        .any(|v| v.turn == turn && v.id == id && v.complete)
                {
                    return;
                }
                self.put(
                    turn,
                    item["id"].as_str().unwrap_or(""),
                    item_text(item),
                    false,
                );
                if method == "item/completed" {
                    if let Some(saved) =
                        self.items.iter_mut().find(|v| v.turn == turn && v.id == id)
                    {
                        saved.complete = true;
                    }
                }
            }
            "item/agentMessage/delta" | "item/commandExecution/outputDelta" => {
                let id = p["itemId"].as_str().unwrap_or("");
                let delta = p["delta"].as_str().unwrap_or("");
                if let Some(item) = self.items.iter_mut().find(|v| v.turn == turn && v.id == id) {
                    if !item.complete && item.text.len() < MAX_TEXT {
                        item.text = bounded(&(item.text.clone() + delta));
                    }
                } else {
                    self.put(turn, id, delta.into(), true);
                }
            }
            "turn/started" => self.notice = "Running · read-only".into(),
            "turn/completed" => self.notice = "Turn completed · read-only".into(),
            "thread/status/changed" => {
                self.notice = format!(
                    "{} · read-only",
                    p["status"]["type"].as_str().unwrap_or("Connected")
                )
            }
            "thread/closed" => {
                self.mode = Mode::History;
                self.notice = "Session closed · saved history".into();
            }
            _ if event.get("id").is_some() => {
                self.notice = "Waiting for a response in the interactive client · read-only".into()
            }
            _ => (),
        }
    }
    pub fn text(&self) -> String {
        self.items
            .iter()
            .map(|i| {
                format!(
                    "{}{}",
                    if i.partial {
                        "[Joined mid-response; earlier text pending]\n"
                    } else {
                        ""
                    },
                    i.text
                )
            })
            .collect::<Vec<_>>()
            .join("\n\n")
    }
}

#[derive(Default)]
pub struct Controller {
    pub key: Option<SessionKey>,
    pub snapshot: Snapshot,
    selected_at: Option<Instant>,
    task: Option<JoinHandle<()>>,
    cancel: Option<oneshot::Sender<()>>,
    updates: Option<watch::Receiver<Snapshot>>,
    started: bool,
}
impl Drop for Controller {
    fn drop(&mut self) {
        if let Some(task) = self.task.take() {
            task.abort();
        }
    }
}
impl Controller {
    pub fn active(&self) -> bool {
        self.key.is_some()
    }
    pub fn tick(&mut self, target: Option<(SessionKey, SourceSpec)>) -> bool {
        let key = target.as_ref().map(|(key, _)| key.clone());
        let mut changed = false;
        if self.key != key {
            if let Some(cancel) = self.cancel.take() {
                let _ = cancel.send(());
            }
            self.updates = None; // Old generations can never publish into the new view.
            self.key = key;
            self.snapshot = Snapshot::default();
            self.selected_at = Some(Instant::now());
            self.started = false;
            changed = true;
        }
        if let Some(updates) = &mut self.updates {
            // The final snapshot remains valid after its worker closes the sender.
            let latest = updates.borrow_and_update();
            if self.snapshot != *latest {
                self.snapshot = latest.clone();
                changed = true;
            }
        }
        if self.task.as_ref().is_some_and(|task| task.is_finished()) {
            self.task = None;
            self.selected_at = Some(Instant::now());
        }
        if self.started
            && self.task.is_none()
            && self
                .selected_at
                .is_some_and(|at| at.elapsed() >= Duration::from_secs(5))
        {
            self.started = false;
        }
        if !self.started
            && self.task.is_none()
            && self
                .selected_at
                .is_some_and(|at| at.elapsed() >= Duration::from_millis(150))
        {
            if let Some((key, SourceSpec::Codex { binary, source })) = target {
                let (tx, rx) = watch::channel(Snapshot::default());
                let (cancel, stopped) = oneshot::channel();
                self.updates = Some(rx);
                self.cancel = Some(cancel);
                self.started = true;
                self.task = Some(tokio::spawn(run(binary, source, key, tx, stopped)));
            }
        }
        changed
    }
}

async fn history(
    client: &mut PreviewClient,
    id: &str,
    events: &mut Vec<Value>,
) -> Result<Snapshot, String> {
    let mut snapshot = Snapshot::default();
    match client
        .call(
            "thread/turns/list",
            json!({"threadId":id,"limit":20,"sortDirection":"desc","itemsView":"full"}),
            events,
        )
        .await
    {
        Ok(page) => {
            let mut turns = page["data"]
                .as_array()
                .ok_or("Invalid history page")?
                .clone();
            turns.reverse();
            snapshot.history(&turns);
        }
        Err(_) => {
            let result = client
                .call(
                    "thread/read",
                    json!({"threadId":id,"includeTurns":true}),
                    events,
                )
                .await?;
            let turns = result["thread"]["turns"]
                .as_array()
                .ok_or("Missing history")?;
            snapshot.history(turns);
        }
    }
    Ok(snapshot)
}
async fn saved(
    binary: &std::path::Path,
    home: &std::path::Path,
    id: &str,
) -> Result<Snapshot, String> {
    let mut rpc = Rpc::start(binary, home).await.map_err(|e| e.to_string())?;
    let mut snapshot = Snapshot {
        mode: Mode::History,
        notice: "Live connection unavailable · saved history (not streaming)".into(),
        ..Snapshot::default()
    };
    match rpc
        .call(
            "thread/turns/list",
            json!({"threadId":id,"limit":20,"sortDirection":"desc","itemsView":"full"}),
        )
        .await
    {
        Ok(page) => {
            let mut turns = page["data"]
                .as_array()
                .ok_or("Invalid history page")?
                .clone();
            turns.reverse();
            snapshot.history(&turns);
        }
        Err(_) => {
            rpc.close().await;
            rpc = Rpc::start(binary, home).await.map_err(|e| e.to_string())?;
            let result = rpc
                .call("thread/read", json!({"threadId":id,"includeTurns":true}))
                .await
                .map_err(|e| e.to_string())?;
            snapshot.history(
                result["thread"]["turns"]
                    .as_array()
                    .ok_or("Missing history")?,
            );
        }
    }
    rpc.close().await;
    Ok(snapshot)
}
async fn run(
    binary: PathBuf,
    source: crate::config::SourceConfig,
    key: SessionKey,
    tx: watch::Sender<Snapshot>,
    mut cancel: oneshot::Receiver<()>,
) {
    // A watch channel stores only the latest complete bounded snapshot, never an event backlog.
    let id = key.native_id;
    let mut client = None;
    let work = async {
        let mut last = Snapshot::default();
        for attempt in 0..3 {
            let result: Result<(), String> = async {
                client = Some(PreviewClient::connect(&binary, source.clone()).await?);
                let c = client.as_mut().unwrap();
                let mut events = Vec::new();
                let metadata = c
                    .call(
                        "thread/read",
                        json!({"threadId":id,"includeTurns":false}),
                        &mut events,
                    )
                    .await?;
                if metadata["thread"]["id"].as_str() != Some(&id) {
                    return Err("Preview session identity mismatch".into());
                }
                if metadata["thread"]["status"]["type"].as_str() == Some("notLoaded") {
                    tx.send_replace(Snapshot {
                        mode: Mode::List,
                        notice: "Session is not running · Enter to open".into(),
                        ..Snapshot::default()
                    });
                    return Ok(());
                }
                if !matches!(
                    metadata["thread"]["status"]["type"].as_str(),
                    Some("idle" | "active" | "systemError")
                ) {
                    return Err("Unknown runtime status".into());
                }
                c.call(
                    "thread/resume",
                    json!({"threadId":id,"excludeTurns":true}),
                    &mut events,
                )
                .await?;
                last = history(c, &id, &mut events).await?;
                last.mode = Mode::Live;
                last.notice = if attempt == 0 {
                    "Live · read-only"
                } else {
                    "Reconnected · history refreshed; live interval may be incomplete"
                }
                .into();
                for event in events {
                    last.event(&id, &event);
                }
                tx.send_replace(last.clone());
                loop {
                    let event = c.next().await?;
                    last.event(&id, &event);
                    if event["method"].as_str() == Some("thread/closed") {
                        return Err("Session closed".into());
                    }
                    tx.send_if_modified(|snapshot| {
                        if *snapshot == last {
                            false
                        } else {
                            *snapshot = last.clone();
                            true
                        }
                    });
                }
            }
            .await;
            if result.is_ok() {
                return;
            }
            client = None; // Disconnect only our proxy, never the shared runtime.
            last.mode = if last.items.is_empty() {
                Mode::Loading
            } else {
                Mode::History
            };
            last.notice =
                "Live preview disconnected · reconnecting; displayed content may be stale".into();
            tx.send_replace(last.clone());
            if attempt < 2 {
                tokio::time::sleep(Duration::from_millis(500)).await;
            }
        }
        let snapshot = saved(&binary, &source.home, &id)
            .await
            .unwrap_or_else(|_| Snapshot {
                mode: Mode::List,
                notice: "Preview unavailable · Enter to open session".into(),
                ..Snapshot::default()
            });
        tx.send_replace(snapshot);
    };
    tokio::select! { _ = &mut cancel => (), _ = work => () }
    if let Some(c) = &mut client {
        let _ = tokio::time::timeout(
            Duration::from_millis(500),
            c.call(
                "thread/unsubscribe",
                json!({"threadId":id}),
                &mut Vec::new(),
            ),
        )
        .await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn completed_items_replace_partial_deltas_and_ignore_other_sessions() {
        let mut snapshot = Snapshot::default();
        snapshot.event("wanted", &json!({"method":"item/agentMessage/delta","params":{"threadId":"other","turnId":"turn","itemId":"message","delta":"WRONG"}}));
        assert!(snapshot.items.is_empty());
        snapshot.event("wanted", &json!({"method":"item/agentMessage/delta","params":{"threadId":"wanted","turnId":"turn","itemId":"message","delta":"tail"}}));
        assert!(snapshot.text().contains("Joined mid-response"));
        snapshot.event("wanted", &json!({"method":"item/completed","params":{"threadId":"wanted","turnId":"turn","item":{"id":"message","type":"agentMessage","text":"full tail"}}}));
        assert_eq!(snapshot.items.len(), 1);
        assert_eq!(snapshot.text(), "Assistant: full tail");
        snapshot.event("wanted", &json!({"id":42,"method":"item/commandExecution/requestApproval","params":{"threadId":"wanted"}}));
        assert!(snapshot.notice.contains("interactive client"));
        snapshot.event(
            "wanted",
            &json!({"method":"turn/completed","params":{"threadId":"wanted","turn":{"id":"turn"}}}),
        );
        assert!(snapshot.notice.contains("completed"));
    }

    #[test]
    fn bootstrap_events_cannot_duplicate_completed_history() {
        let mut snapshot = Snapshot::default();
        snapshot.history(&[json!({"id":"turn","status":"completed","items":[{"id":"item","type":"agentMessage","text":"Full reply"}]})]);
        snapshot.event("thread", &json!({"method":"item/started","params":{"threadId":"thread","turnId":"turn","item":{"id":"item","type":"agentMessage","text":""}}}));
        snapshot.event("thread", &json!({"method":"item/agentMessage/delta","params":{"threadId":"thread","turnId":"turn","itemId":"item","delta":"reply"}}));
        assert_eq!(snapshot.text(), "Assistant: Full reply");
    }

    #[test]
    fn finished_worker_final_snapshot_is_visible() {
        let mut controller = Controller::default();
        let (tx, rx) = watch::channel(Snapshot::default());
        controller.updates = Some(rx);
        tx.send_replace(Snapshot {
            mode: Mode::List,
            notice: "Finished".into(),
            ..Snapshot::default()
        });
        drop(tx);
        assert!(controller.tick(None));
        assert_eq!(controller.snapshot.notice, "Finished");
    }

    #[test]
    fn transcript_is_bounded_and_distinguishes_turns() {
        let mut snapshot = Snapshot::default();
        for i in 0..MAX_ITEMS + 5 {
            snapshot.put(&i.to_string(), "same", "界".repeat(MAX_TEXT + 1), false);
        }
        assert_eq!(snapshot.items.len(), MAX_ITEMS);
        assert_eq!(snapshot.items[0].turn, "5");
        assert_eq!(snapshot.items[0].text.chars().count(), MAX_TEXT);
    }

    #[tokio::test]
    async fn selection_change_discards_old_generation_without_waiting_for_it() {
        let key = |name: &str| SessionKey {
            provider: crate::domain::ProviderId("codex".into()),
            source_id: crate::domain::SourceId("test".into()),
            native_id: name.into(),
        };
        let spec = SourceSpec::Fixture("unused".into());
        let mut controller = Controller::default();
        assert!(controller.tick(Some((key("first"), spec.clone()))));
        let (tx, rx) = watch::channel(Snapshot::default());
        controller.updates = Some(rx);
        assert!(controller.tick(Some((key("second"), spec.clone()))));
        tx.send_replace(Snapshot {
            mode: Mode::Live,
            notice: "STALE".into(),
            ..Snapshot::default()
        });
        controller.tick(Some((key("second"), spec)));
        assert_ne!(controller.snapshot.notice, "STALE");
        assert!(controller.tick(None));
        assert!(!controller.active());
    }
}
