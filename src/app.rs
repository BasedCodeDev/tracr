use crate::{
    domain::{SessionKey, SessionSummary},
    providers::{ProviderError, SessionProvider},
};
use std::collections::{BTreeMap, HashSet};

#[derive(Debug)]
pub struct Inventory {
    pub sessions: Vec<SessionSummary>,
    pub complete: bool,
    pub error: Option<ProviderError>,
}

pub async fn collect(provider: &impl SessionProvider) -> Inventory {
    let mut sessions: BTreeMap<SessionKey, SessionSummary> = BTreeMap::new();
    let mut cursor = None;
    let mut seen = HashSet::new();
    let mut warnings = Vec::new();
    let error = loop {
        match provider.list(cursor).await {
            Err(error) => break Some(error),
            Ok(page) => {
                warnings.extend(page.warnings);
                for summary in page.sessions {
                    // Prefer newer metadata; equal timestamps preserve the first observation.
                    match sessions.get(&summary.key) {
                        Some(old) if old.updated_at >= summary.updated_at => {}
                        _ => {
                            sessions.insert(summary.key.clone(), summary);
                        }
                    }
                }
                match page.next {
                    None => break None,
                    Some(next) => {
                        if !seen.insert(next.0.clone()) {
                            break Some(ProviderError::InvalidData(
                                "provider repeated pagination cursor".into(),
                            ));
                        }
                        cursor = Some(next);
                    }
                }
            }
        }
    };
    let error = error.or_else(|| {
        (!warnings.is_empty()).then(|| ProviderError::InvalidData(warnings.join("; ")))
    });
    Inventory {
        sessions: sessions.into_values().collect(),
        complete: error.is_none(),
        error,
    }
}

pub fn visible_sessions(sessions: &mut Vec<SessionSummary>, include_archived: bool) {
    sessions.retain(|s| include_archived || !s.archived);
    sessions.sort_by(|a, b| {
        b.updated_at
            .cmp(&a.updated_at)
            .then_with(|| a.key.cmp(&b.key))
    });
}
