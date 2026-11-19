//! On-demand, memory-only conversation search. Provider histories never enter metadata.
use crate::{browser::Browser, domain::SessionSummary, providers::ProviderError};
use serde_json::Value;
use std::{collections::BTreeSet, sync::Arc};
use tokio::{
    sync::{Semaphore, mpsc},
    task::JoinHandle,
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ContentKind {
    User,
    Plan,
    Final,
}
impl ContentKind {
    pub fn label(&self) -> &'static str {
        match self {
            Self::User => "You",
            Self::Plan => "Plan",
            Self::Final => "Final reply",
        }
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Content {
    pub turn_id: String,
    pub item_id: String,
    pub kind: ContentKind,
    pub text: String,
}
pub struct CachedContent {
    pub session: SessionSummary,
    pub content: Result<Vec<Content>, String>,
    pub(crate) searchable: String,
}
impl CachedContent {
    pub fn new(session: SessionSummary, content: Result<Vec<Content>, String>) -> Self {
        let searchable = content
            .as_ref()
            .map(|items| {
                items
                    .iter()
                    .map(|item| item.text.to_lowercase())
                    .collect::<Vec<_>>()
                    .join("\n")
            })
            .unwrap_or_default();
        Self {
            session,
            content,
            searchable,
        }
    }
}

pub(crate) fn extract_turns(value: &Value) -> Result<Vec<Content>, ProviderError> {
    let turns = value
        .as_array()
        .ok_or_else(|| ProviderError::InvalidData("Missing history turns".into()))?;
    let mut result = Vec::new();
    for turn in turns {
        let items = turn["items"]
            .as_array()
            .ok_or_else(|| ProviderError::InvalidData("Missing turn items".into()))?;
        let has_final = items
            .iter()
            .any(|i| i["type"] == "agentMessage" && i["phase"] == "final_answer");
        let legacy_final = if !has_final && turn["status"] == "completed" {
            items
                .iter()
                .rposition(|i| i["type"] == "agentMessage" && i["phase"].is_null())
        } else {
            None
        };
        let mut plans = BTreeSet::new();
        // Explicit plans are authoritative and take precedence over legacy markup.
        for item in items.iter().filter(|i| i["type"] == "plan") {
            if let Some(text) = item["text"].as_str().filter(|s| !s.trim().is_empty()) {
                if plans.insert(text.trim().to_owned()) {
                    result.push(record(turn, item, ContentKind::Plan, text.trim().into()));
                }
            }
        }
        for (index, item) in items.iter().enumerate() {
            if item["type"] == "userMessage" {
                let text = item["content"]
                    .as_array()
                    .map(|parts| {
                        parts
                            .iter()
                            .filter(|p| p["type"] == "text")
                            .filter_map(|p| p["text"].as_str())
                            .collect::<Vec<_>>()
                            .join("\n")
                    })
                    .unwrap_or_default();
                if !text.is_empty() {
                    result.push(record(turn, item, ContentKind::User, text));
                }
            } else if item["type"] == "agentMessage" {
                let text = item["text"].as_str().unwrap_or_default();
                let mut remainder = text.to_owned();
                while let Some(start) = remainder.find("<proposed_plan>") {
                    let after = start + "<proposed_plan>".len();
                    let Some(length) = remainder[after..].find("</proposed_plan>") else {
                        break;
                    };
                    let plan = remainder[after..after + length].trim();
                    if !plan.is_empty() && plans.insert(plan.to_owned()) {
                        result.push(record(turn, item, ContentKind::Plan, plan.into()));
                    }
                    remainder.replace_range(start..after + length + "</proposed_plan>".len(), "");
                }
                if (item["phase"] == "final_answer" || legacy_final == Some(index))
                    && !remainder.trim().is_empty()
                {
                    // Some versions repeat an explicit plan as the entire final answer.
                    if !plans.contains(remainder.trim()) {
                        result.push(record(turn, item, ContentKind::Final, remainder));
                    }
                }
            }
        }
    }
    Ok(result)
}
fn record(turn: &Value, item: &Value, kind: ContentKind, text: String) -> Content {
    Content {
        turn_id: turn["id"].as_str().unwrap_or_default().into(),
        item_id: item["id"].as_str().unwrap_or_default().into(),
        kind,
        text,
    }
}

pub fn excerpt(browser: &Browser, session: &SessionSummary, query: &str) -> Option<String> {
    if !browser.deep_search {
        return None;
    }
    let cached = browser
        .search_cache
        .get(&session.key)
        .filter(|c| c.session == *session)?;
    let terms: Vec<_> = query.split_whitespace().map(str::to_lowercase).collect();
    for item in cached.content.as_ref().ok()? {
        let chars: Vec<char> = item.text.chars().collect();
        // Map a lowercase byte match back to original character coordinates.
        let lowercase = item.text.to_lowercase();
        let byte_index = terms.iter().filter_map(|term| lowercase.find(term)).min();
        let mut offset = 0;
        let index = byte_index.and_then(|target| {
            chars.iter().enumerate().find_map(|(i, c)| {
                offset += c.to_lowercase().map(char::len_utf8).sum::<usize>();
                (offset > target).then_some(i)
            })
        });
        if let Some(index) = index {
            let start = index.saturating_sub(35);
            let text: String = chars
                .iter()
                .skip(start)
                .take(150)
                .map(|c| if c.is_whitespace() { ' ' } else { *c })
                .collect();
            return Some(format!(
                "{}: {}{}{}",
                item.kind.label(),
                if start > 0 { "…" } else { "" },
                crate::browser::safe_text(&text),
                if chars.len() > start + 150 { "…" } else { "" }
            ));
        }
    }
    None
}

struct Completion {
    epoch: u64,
    cached: CachedContent,
}
pub struct Search {
    generation: u64,
    epoch: u64,
    candidates: Vec<SessionSummary>,
    tasks: Vec<JoinHandle<()>>,
    tx: mpsc::Sender<Completion>,
    rx: mpsc::Receiver<Completion>,
}
impl Default for Search {
    fn default() -> Self {
        let (tx, rx) = mpsc::channel(16);
        Self {
            generation: 0,
            epoch: 0,
            candidates: Vec::new(),
            tasks: Vec::new(),
            tx,
            rx,
        }
    }
}
impl Search {
    pub fn tick(&mut self, browser: &mut Browser, active: bool) -> bool {
        let candidates = if active && !browser.loading() {
            browser.search_results("")
        } else {
            Vec::new()
        };
        let mut changed = false;
        if self.generation != browser.generation || candidates != self.candidates {
            self.cancel();
            self.generation = browser.generation;
            self.candidates = candidates;
            let limit = Arc::new(Semaphore::new(4));
            for (source, spec) in browser.sources.iter().zip(&browser.specs) {
                // Inventory must be ready before deciding which source owns a session.
                if source.loading {
                    continue;
                }
                let wanted: Vec<_> = self
                    .candidates
                    .iter()
                    .filter(|s| source.contains(&s.key))
                    .filter(|s| {
                        browser
                            .search_cache
                            .get(&s.key)
                            .is_none_or(|c| c.session != **s)
                    })
                    .cloned()
                    .collect();
                if wanted.is_empty() {
                    continue;
                }
                let spec = spec.clone();
                let tx = self.tx.clone();
                let epoch = self.epoch;
                let limit = limit.clone();
                self.tasks.push(tokio::spawn(async move {
                    let Ok(_permit) = limit.acquire_owned().await else {
                        return;
                    };
                    let mut provider = spec.open().await;
                    for session in wanted {
                        let content = match &provider {
                            Ok(p) => p.search_content(&session.key).await,
                            Err(e) => Err(e.clone()),
                        };
                        let failed = content.is_err();
                        if tx
                            .send(Completion {
                                epoch,
                                cached: CachedContent::new(session, content),
                            })
                            .await
                            .is_err()
                        {
                            break;
                        }
                        if failed {
                            // A failed RPC retires its connection; later sessions may still be readable.
                            if let Ok(p) = &provider {
                                p.close().await;
                            }
                            provider = spec.open().await;
                        }
                    }
                    if let Ok(p) = provider {
                        p.close().await;
                    }
                }));
            }
            changed = true;
        }
        while let Ok(update) = self.rx.try_recv() {
            if update.epoch == self.epoch {
                browser
                    .search_cache
                    .insert(update.cached.session.key.clone(), update.cached);
                changed = true;
            }
        }
        changed
    }
    fn cancel(&mut self) {
        self.epoch += 1;
        for task in self.tasks.drain(..) {
            task.abort();
        }
    }
}
impl Drop for Search {
    fn drop(&mut self) {
        self.cancel();
    }
}

pub fn pending(browser: &Browser) -> bool {
    browser.search_results("").iter().any(|s| {
        browser
            .search_cache
            .get(&s.key)
            .is_none_or(|c| c.session != *s)
    })
}

pub fn progress(browser: &Browser) -> String {
    let sessions = browser.search_results("");
    let mut done = 0;
    let mut errors = 0;
    let mut first_error = None;
    for session in &sessions {
        if let Some(cached) = browser
            .search_cache
            .get(&session.key)
            .filter(|c| c.session == *session)
        {
            done += 1;
            if let Err(error) = &cached.content {
                errors += 1;
                first_error.get_or_insert(error);
            }
        }
    }
    let suffix = first_error
        .map(|e| {
            format!(
                " · {errors} unavailable: {}",
                crate::browser::safe_text(e)
                    .chars()
                    .take(100)
                    .collect::<String>()
            )
        })
        .unwrap_or_default();
    format!("Content: {done}/{} checked{suffix}", sessions.len())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        browser::{SourceSpec, Update},
        domain::{ProviderId, SessionKey, SourceId},
    };
    use serde_json::json;

    fn session(id: &str, path: &str) -> SessionSummary {
        SessionSummary {
            key: SessionKey {
                provider: ProviderId("codex".into()),
                source_id: SourceId("test".into()),
                native_id: id.into(),
            },
            title: "Unrelated title".into(),
            cwd: Some(path.into()),
            created_at: 1,
            updated_at: 2,
            archived: false,
            can_accept_direct_input: None,
            source_kind: None,
            git: None,
        }
    }
    fn browser(sessions: Vec<SessionSummary>) -> Browser {
        let mut browser = Browser::new(vec![SourceSpec::Fixture("unused".into())]);
        browser.apply(Update::Page {
            generation: 0,
            source: 0,
            sessions,
            warnings: vec![],
        });
        browser
    }
    fn cache(browser: &mut Browser, session: &SessionSummary, text: &str) {
        browser.search_cache.insert(
            session.key.clone(),
            CachedContent {
                searchable: text.to_lowercase(),
                session: session.clone(),
                content: Ok(vec![Content {
                    turn_id: "turn".into(),
                    item_id: "item".into(),
                    kind: ContentKind::User,
                    text: text.into(),
                }]),
            },
        );
    }
    #[test]
    fn extracts_only_requested_content_and_deduplicates_plans() {
        let content = extract_turns(&json!([{"id":"t", "status":"completed", "items":[
            {"id":"u", "type":"userMessage", "content":[{"type":"text","text":"AltTab"},{"type":"image","url":"excluded"}]},
            {"type":"reasoning","text":"excluded"},
            {"type":"commandExecution","aggregatedOutput":"excluded"},
            {"type":"agentMessage","phase":"commentary","text":"excluded"},
            {"type":"agentMessage","phase":"partial_answer","text":"excluded"},
            {"id":"p","type":"plan","text":"Forward keys"},
            {"id":"a","type":"agentMessage","phase":"final_answer","text":"<proposed_plan>Forward keys</proposed_plan>Use Ninja One"}
        ]}])).unwrap();
        assert_eq!(content.len(), 3);
        assert_eq!(
            content
                .iter()
                .filter(|c| c.kind == ContentKind::Plan)
                .count(),
            1
        );
        assert!(content.iter().all(|c| !c.text.contains("excluded")));
        assert!(
            content
                .iter()
                .any(|c| c.item_id == "a" && c.text == "Use Ninja One")
        );
        assert!(extract_turns(&Value::Null).is_err());
    }
    #[test]
    fn legacy_final_requires_completed_turn_and_missing_phase() {
        let items = json!([
            {"type":"agentMessage","text":"preamble"},
            {"type":"agentMessage","text":"outcome"},
            {"type":"agentMessage","phase":"commentary","text":"progress"}
        ]);
        let content = extract_turns(&json!([{"status":"completed","items":items}])).unwrap();
        assert_eq!(content.len(), 1);
        assert_eq!(content[0].text, "outcome");
        assert!(
            extract_turns(&json!([{"status":"inProgress","items":items}]))
                .unwrap()
                .is_empty()
        );
    }
    #[test]
    fn deep_search_respects_scope_archives_identity_and_freshness() {
        let a = session("same", "/sysadmin");
        let mut b = session("same", "/elsewhere");
        b.key.source_id = SourceId("other".into());
        let mut archived = session("archived", "/sysadmin/child");
        archived.archived = true;
        let mut browser = browser(vec![a.clone(), b.clone(), archived.clone()]);
        for s in [&a, &b, &archived] {
            cache(&mut browser, s, "Forward AltTab into Ninja One");
        }
        assert!(browser.search_results("alttab ninja").is_empty());
        browser.deep_search = true;
        assert_eq!(browser.search_results("alttab ninja").len(), 2);
        browser.focus_root = Some("/sysadmin".into());
        assert_eq!(browser.search_results("alttab ninja").len(), 1);
        browser.include_archived = true;
        assert_eq!(browser.search_results("alttab ninja").len(), 2);
        browser
            .search_cache
            .get_mut(&a.key)
            .unwrap()
            .session
            .updated_at = 1;
        assert_eq!(browser.search_results("alttab ninja").len(), 1);
        browser.begin_refresh();
        assert!(browser.search_cache.is_empty());
    }
    #[test]
    fn terms_can_span_messages_and_metadata_and_excerpt_is_safe() {
        let a = session("id", "/sysadmin");
        let mut browser = browser(vec![a.clone()]);
        cache(&mut browser, &a, "İ Unicode \u{1b} AltTab");
        browser
            .search_cache
            .get_mut(&a.key)
            .unwrap()
            .content
            .as_mut()
            .unwrap()
            .push(Content {
                turn_id: "t".into(),
                item_id: "a".into(),
                kind: ContentKind::Final,
                text: "Ninja One".into(),
            });
        browser.deep_search = true;
        browser
            .search_cache
            .get_mut(&a.key)
            .unwrap()
            .searchable
            .push_str("\nninja one");
        assert_eq!(browser.search_results("sysadmin AltTab ninja").len(), 1);
        let snippet = excerpt(&browser, &a, "alttab").unwrap();
        assert!(snippet.starts_with("You:"));
        assert!(!snippet.contains('\u{1b}'));
        assert!(snippet.contains("AltTab"));
    }
    #[tokio::test]
    async fn cancelled_updates_cannot_repopulate_cache() {
        let a = session("id", "/sysadmin");
        let mut browser = browser(vec![a.clone()]);
        let mut search = Search::default();
        search
            .tx
            .send(Completion {
                epoch: 0,
                cached: CachedContent {
                    searchable: String::new(),
                    session: a,
                    content: Ok(vec![]),
                },
            })
            .await
            .unwrap();
        search.cancel();
        search.tick(&mut browser, false);
        assert!(browser.search_cache.is_empty());
    }
}
