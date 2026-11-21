//! Temporary native Codex clients, isolated from owned terminals and browser selection.
use crate::{
    browser::{Browser, SourceSpec},
    domain::{ResumeCommand, SessionKey},
    inline_terminal::Manager,
    providers::{SessionProvider, codex::CodexProvider, codex_live::OperationalClient},
};
use serde_json::json;
use std::time::{Duration, Instant};
use tokio::{sync::oneshot, task::JoinHandle};

#[derive(Default)]
pub struct Controller {
    pub key: Option<SessionKey>,
    pub notice: String,
    pub terminals: Manager,
    selected_at: Option<Instant>,
    task: Option<JoinHandle<()>>,
    ready: Option<oneshot::Receiver<Result<ResumeCommand, String>>>,
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
        self.key.is_some() || !self.terminals.is_empty()
    }

    pub fn promote(&mut self, owned: &mut Manager, key: &SessionKey) -> bool {
        self.key.as_ref() == Some(key) && self.terminals.promote_into(owned, key)
    }

    pub fn tick(&mut self, key: Option<SessionKey>, browser: &Browser) -> bool {
        let mut changed = false;
        if self.key != key {
            if let Some(task) = self.task.take() {
                task.abort();
            }
            self.ready = None; // Late preparation results cannot launch an old selection.
            self.terminals.stop_preview();
            self.key = key;
            self.notice = "Loading Codex preview…".into();
            self.selected_at = Some(Instant::now());
            self.started = false;
            changed = true;
        }
        let (output, exits) = self.terminals.poll();
        changed |= output;
        if !exits.is_empty() && self.started {
            self.notice = "Codex preview closed · Enter to open session".into();
            changed = true;
        }
        // Only protocol replies are written to this manager while browsing.
        let _ = self.terminals.sync_focus(false);
        let result = self.ready.as_mut().and_then(|rx| match rx.try_recv() {
            Ok(result) => Some(result),
            Err(oneshot::error::TryRecvError::Closed) => {
                Some(Err("Preview preparation stopped".into()))
            }
            Err(oneshot::error::TryRecvError::Empty) => None,
        });
        if let Some(result) = result {
            self.ready = None;
            self.task = None;
            match result {
                Ok(command) if self.key.as_ref() == Some(&command.session.key) => {
                    self.notice = match self.terminals.launch_preview(
                        &command.launch,
                        command.session.key,
                        command.session.title,
                    ) {
                        Ok(()) => "Codex preview · Enter to interact".into(),
                        Err(error) => format!("Preview unavailable: {error} · Enter to open"),
                    };
                }
                Ok(_) => self.notice = "Session changed · Enter to open".into(),
                Err(error) => self.notice = format!("Preview unavailable: {error} · Enter to open"),
            }
            changed = true;
        }
        if !self.started
            && self.terminals.is_empty()
            && self
                .selected_at
                .is_some_and(|at| at.elapsed() >= Duration::from_millis(150))
            && let Some(key) = self.key.clone()
        {
            self.started = true;
            let session = browser.all().into_iter().find(|s| s.key == key);
            let source = browser.specs.iter().find_map(|spec| match spec {
                SourceSpec::Codex { binary, source }
                    if source.id == key.source_id && key.provider.0 == "codex" =>
                {
                    Some((binary.clone(), source.clone()))
                }
                _ => None,
            });
            match (session, source, browser.resume_provider(&key)) {
                (Some(session), Some((binary, source)), Ok(_)) => {
                    let (tx, rx) = oneshot::channel();
                    self.ready = Some(rx);
                    self.task = Some(tokio::spawn(async move {
                        let work = async {
                            // Preparation is cancellable: never borrow the inventory RPC,
                            // whose in-flight response would be left unread on navigation.
                            let provider = CodexProvider::connect(&binary, source.clone())
                                .await
                                .map_err(|e| e.to_string())?;
                            let command = provider
                                .prepare_resume(&session)
                                .await
                                .map_err(|e| e.to_string());
                            provider.close().await;
                            let command = command?;
                            if command.session.key != key
                                || command.cwd != session.cwd.clone().unwrap_or_default()
                            {
                                return Err("Session identity or folder changed".into());
                            }
                            let mut client =
                                OperationalClient::connect(&binary, source.clone()).await?;
                            client.validate_server_home(&source.home)?;
                            let result = client
                                .call(
                                    "thread/read",
                                    json!({"threadId":key.native_id,"includeTurns":false}),
                                )
                                .await?;
                            if result["thread"]["id"].as_str() != Some(&key.native_id)
                                || !matches!(
                                    result["thread"]["status"]["type"].as_str(),
                                    Some("idle" | "active" | "systemError")
                                )
                            {
                                return Err("Session is not loaded in the shared runtime".into());
                            }
                            Ok(command)
                        };
                        let result = tokio::time::timeout(Duration::from_secs(10), work)
                            .await
                            .unwrap_or_else(|_| Err("Preview preparation timed out".into()));
                        let _ = tx.send(result);
                    }));
                }
                _ => {
                    self.notice = "Preview unavailable · Enter to open session".into();
                    changed = true;
                }
            }
        }
        changed
    }

    pub async fn shutdown(&mut self) {
        if let Some(task) = self.task.take() {
            task.abort();
        }
        self.ready = None;
        self.terminals.shutdown().await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn key(id: &str) -> SessionKey {
        SessionKey {
            provider: crate::domain::ProviderId("codex".into()),
            source_id: crate::domain::SourceId("test".into()),
            native_id: id.into(),
        }
    }
    #[tokio::test]
    async fn selection_change_discards_pending_preparation() {
        let browser = Browser::new(vec![]);
        let mut controller = Controller::default();
        controller.tick(Some(key("first")), &browser);
        let (tx, rx) = oneshot::channel();
        controller.ready = Some(rx);
        controller.started = true;
        controller.tick(Some(key("second")), &browser);
        assert!(tx.send(Err("STALE".into())).is_err());
        assert!(!controller.started);
        assert_eq!(controller.key, Some(key("second")));
        controller.tick(None, &browser);
        assert!(!controller.active());
        assert!(controller.terminals.is_empty());
    }
    #[test]
    fn failure_does_not_relaunch_until_selection_changes() {
        let browser = Browser::new(vec![]);
        let mut controller = Controller::default();
        controller.tick(Some(key("missing")), &browser);
        controller.selected_at = Some(Instant::now() - Duration::from_secs(1));
        assert!(controller.tick(Some(key("missing")), &browser));
        assert!(controller.started);
        let notice = controller.notice.clone();
        assert!(!controller.tick(Some(key("missing")), &browser));
        assert_eq!(controller.notice, notice);
        controller.tick(None, &browser);
        controller.tick(Some(key("missing")), &browser);
        assert!(!controller.started);
    }
}
