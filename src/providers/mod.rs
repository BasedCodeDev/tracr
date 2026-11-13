pub mod codex;
pub mod codex_live;
pub mod fixture;

use crate::domain::{ProviderCapabilities, ResumeCommand, SessionKey, SessionSummary};
use std::future::Future;
use thiserror::Error;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Cursor(pub String);

#[derive(Debug)]
pub struct SessionPage {
    pub warnings: Vec<String>,
    pub sessions: Vec<SessionSummary>,
    pub next: Option<Cursor>,
}

#[derive(Debug, Error)]
pub enum ProviderError {
    #[error("invalid provider data: {0}")]
    InvalidData(String),
    #[error("provider unavailable: {0}")]
    Unavailable(String),
    #[error("invalid or expired pagination cursor")]
    InvalidCursor,
    #[error("provider I/O error: {0}")]
    Io(#[from] std::io::Error),
}

pub trait SessionProvider: Send + Sync {
    fn prepare_resume(
        &self,
        _session: &SessionSummary,
    ) -> impl Future<Output = Result<ResumeCommand, ProviderError>> + Send {
        async {
            Err(ProviderError::Unavailable(
                "This provider cannot resume sessions; fixture data is browse-only.".into(),
            ))
        }
    }

    fn capabilities(&self) -> ProviderCapabilities;
    fn list(
        &self,
        cursor: Option<Cursor>,
    ) -> impl Future<Output = Result<SessionPage, ProviderError>> + Send;
    fn read(
        &self,
        key: &SessionKey,
    ) -> impl Future<Output = Result<Option<SessionSummary>, ProviderError>> + Send;
}
