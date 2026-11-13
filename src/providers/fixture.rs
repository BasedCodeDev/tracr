use super::{Cursor, ProviderError, SessionPage, SessionProvider};
use crate::{
    config::absolute_path,
    domain::{Capability, ProviderCapabilities, SessionKey, SessionSummary},
};
use serde::Deserialize;
use std::path::Path;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Fixture {
    #[serde(default = "default_page_size")]
    page_size: usize,
    /// Zero-based page at which to simulate a provider failure.
    fail_at_page: Option<usize>,
    sessions: Vec<SessionSummary>,
}
fn default_page_size() -> usize {
    100
}

pub struct FixtureProvider {
    sessions: Vec<SessionSummary>,
    page_size: usize,
    fail_at_page: Option<usize>,
}

impl FixtureProvider {
    pub fn open(path: &Path) -> Result<Self, ProviderError> {
        let path = absolute_path(path, &std::env::current_dir()?)
            .map_err(|e| ProviderError::InvalidData(e.to_string()))?;
        let mut fixture: Fixture = serde_json::from_str(&std::fs::read_to_string(&path)?)
            .map_err(|e| ProviderError::InvalidData(e.to_string()))?;
        if fixture.page_size == 0 {
            return Err(ProviderError::InvalidData(
                "page_size must be positive".into(),
            ));
        }
        for session in &mut fixture.sessions {
            if session.key.provider.0.is_empty()
                || session.key.source_id.0.is_empty()
                || session.key.native_id.is_empty()
            {
                return Err(ProviderError::InvalidData(
                    "session identity must not be empty".into(),
                ));
            }
            if let Some(cwd) = &session.cwd {
                session.cwd = Some(
                    absolute_path(cwd, path.parent().expect("absolute path has a parent"))
                        .map_err(|e| ProviderError::InvalidData(e.to_string()))?,
                );
            }
        }
        Ok(Self {
            sessions: fixture.sessions,
            page_size: fixture.page_size,
            fail_at_page: fixture.fail_at_page,
        })
    }
}

impl SessionProvider for FixtureProvider {
    fn capabilities(&self) -> ProviderCapabilities {
        ProviderCapabilities {
            list: Capability::Supported,
            read: Capability::Supported,
            resume: Capability::Unsupported {
                reason: "synthetic fixtures cannot be resumed".into(),
            },
            relocation: Capability::Unsupported {
                reason:
                    "Codex 0.167.2 listing consistency blocks relocation; fixtures are read-only"
                        .into(),
            },
        }
    }

    async fn list(&self, cursor: Option<Cursor>) -> Result<SessionPage, ProviderError> {
        let page = match cursor {
            None => 0,
            Some(Cursor(value)) => value
                .strip_prefix("fixture-page:")
                .and_then(|s| s.parse::<usize>().ok())
                .ok_or(ProviderError::InvalidCursor)?,
        };
        let start = page
            .checked_mul(self.page_size)
            .ok_or(ProviderError::InvalidCursor)?;
        if start > self.sessions.len() {
            return Err(ProviderError::InvalidCursor);
        }
        if self.fail_at_page == Some(page) {
            return Err(ProviderError::Unavailable(format!(
                "synthetic failure on page {page}"
            )));
        }
        let end = start
            .saturating_add(self.page_size)
            .min(self.sessions.len());
        Ok(SessionPage {
            warnings: Vec::new(),
            sessions: self.sessions[start..end].to_vec(),
            next: (end < self.sessions.len()).then(|| Cursor(format!("fixture-page:{}", page + 1))),
        })
    }

    async fn read(&self, key: &SessionKey) -> Result<Option<SessionSummary>, ProviderError> {
        Ok(self.sessions.iter().find(|s| &s.key == key).cloned())
    }
}
