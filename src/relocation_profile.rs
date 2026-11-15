//! Version-pinned profile provenance from native rollout metadata only.
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    fs,
    io::{BufRead, BufReader, Read},
    os::unix::fs::OpenOptionsExt,
    path::Path,
};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Profile {
    #[default]
    Unknown,
    Legacy,
    Workspace,
    ReadOnly,
}
impl Profile {
    pub fn label(self) -> &'static str {
        match self {
            Self::Workspace => "Workspace",
            Self::ReadOnly => "Read only",
            Self::Legacy => "Legacy permissions",
            Self::Unknown => "Unverified",
        }
    }
    pub fn from_native(value: &Value) -> Result<Self, String> {
        if value.is_null() {
            return Ok(Self::Legacy);
        }
        let object = value
            .as_object()
            .ok_or("Cannot verify the recorded permission profile.")?;
        if object.keys().any(|k| k != "id" && k != "extends") || !value["extends"].is_null() {
            return Err("This permission profile has unsupported inheritance or fields. Only the built-in Workspace and Read only profiles are supported.".into());
        }
        match value["id"].as_str() {
            Some(":workspace") => Ok(Self::Workspace),
            Some(":read-only") => Ok(Self::ReadOnly),
            Some(id) => Err(format!(
                "Permission profile {id:?} is not supported for relinking. Built-in Workspace and Read only profiles are supported; custom and full-access profiles are not yet supported."
            )),
            None => Err("The recorded permission profile has no valid identity.".into()),
        }
    }
}

#[derive(Deserialize, Default)]
struct Settings {
    #[serde(default)]
    active_permission_profile: Value,
}
#[derive(Deserialize)]
struct Identity {
    id: String,
}
#[derive(Deserialize)]
struct Record<'a> {
    #[serde(rename = "type")]
    kind: String,
    #[serde(borrow)]
    payload: &'a serde_json::value::RawValue,
}
#[derive(Deserialize)]
struct EventKind {
    #[serde(rename = "type")]
    kind: String,
}
#[derive(Deserialize)]
struct AppliedSettings {
    thread_id: Option<String>,
    thread_settings: Settings,
}
fn decode<T: serde::de::DeserializeOwned>(
    value: &serde_json::value::RawValue,
) -> Result<T, String> {
    serde_json::from_str(value.get())
        .map_err(|_| "Session permission metadata is malformed.".into())
}

fn parse(reader: impl BufRead, id: &str) -> Result<Profile, String> {
    let mut reader = reader;
    let mut owned = None;
    let mut owner_matches = false;
    let mut fallback = None;
    let mut identity = false;
    let mut total = 0usize;
    loop {
        // Unknown payload fields (including messages) are discarded by serde.
        let mut line = Vec::new();
        let size = reader
            .by_ref()
            .take(8 * 1024 * 1024 + 1)
            .read_until(b'\n', &mut line)
            .map_err(|e| e.to_string())?;
        if size == 0 {
            break;
        }
        total += size;
        if size > 8 * 1024 * 1024 || total > 256 * 1024 * 1024 {
            return Err("Session metadata exceeds the supported verification size.".into());
        }
        let record: Record = serde_json::from_slice(&line).map_err(|_| {
            "Session metadata is malformed or still being written; refresh and retry.".to_owned()
        })?;
        match record.kind.as_str() {
            "session_meta" => {
                let meta: Identity = decode(record.payload)?;
                if meta.id != id {
                    return Err(
                        "Session metadata identity does not match the selected session.".into(),
                    );
                }
                identity = true;
            }
            "turn_context" => {
                fallback = Some(decode::<Settings>(record.payload)?.active_permission_profile)
            }
            "event_msg"
                if decode::<EventKind>(record.payload)?.kind == "thread_settings_applied" =>
            {
                let AppliedSettings {
                    thread_id,
                    thread_settings: settings,
                } = decode(record.payload)?;
                match thread_id.as_deref() {
                    Some(owner) if owner == id => {
                        owned = Some(settings.active_permission_profile.clone());
                        owner_matches = true;
                    }
                    Some(_) => owner_matches = false,
                    None if owner_matches => {
                        owned = Some(settings.active_permission_profile.clone())
                    }
                    None => {}
                }
                fallback = if thread_id.as_deref().is_none_or(|owner| owner == id) {
                    Some(settings.active_permission_profile)
                } else {
                    None
                };
            }
            _ => {}
        }
    }
    if !identity {
        return Err("Session identity metadata is missing.".into());
    }
    Profile::from_native(
        &owned
            .or(fallback)
            .ok_or("Cannot verify the session's recorded permission settings.")?,
    )
}

pub fn read(home: &Path, path: &Path, id: &str) -> Result<Profile, String> {
    let home = fs::canonicalize(home).map_err(|e| e.to_string())?;
    let path = fs::canonicalize(path)
        .map_err(|e| format!("Cannot read session permission metadata: {e}"))?;
    if !path.starts_with(home.join("sessions")) && !path.starts_with(home.join("archived_sessions"))
    {
        return Err(
            "Session permission metadata is outside the configured Codex history directories."
                .into(),
        );
    }
    let file = fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)
        .map_err(|e| e.to_string())?;
    if !file.metadata().map_err(|e| e.to_string())?.is_file() {
        return Err("Session metadata is not a regular file.".into());
    }
    parse(BufReader::new(file), id)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn record(kind: &str, payload: Value) -> String {
        format!("{}\n", serde_json::json!({"type":kind,"payload":payload}))
    }
    fn header() -> String {
        record("session_meta", serde_json::json!({"id":"mine"}))
    }
    fn context(id: &str) -> String {
        record(
            "turn_context",
            serde_json::json!({"active_permission_profile":{"id":id}}),
        )
    }
    fn settings(owner: Value, profile: &str) -> String {
        record(
            "event_msg",
            serde_json::json!({"type":"thread_settings_applied","thread_id":owner,"thread_settings":{"active_permission_profile":{"id":profile}}}),
        )
    }
    #[test]
    fn owned_settings_precedence_matches_native_and_does_not_accept_fallback() {
        let input = header()
            + &context(":read-only")
            + &settings("mine".into(), ":workspace")
            + &context("custom")
            + &settings(Value::Null, ":read-only")
            + &settings("foreign".into(), "custom")
            + &settings(Value::Null, "custom");
        assert_eq!(parse(input.as_bytes(), "mine").unwrap(), Profile::ReadOnly);
        assert!(parse((header() + &context("custom")).as_bytes(), "mine").is_err());
        assert!(parse((header() + &context(":workspace")).as_bytes(), "other").is_err());
    }
    #[test]
    fn message_payloads_are_ignored_without_reading_their_fields() {
        let input = header()
            + &context(":workspace")
            + &record(
                "response_item",
                serde_json::json!({"id":17,"thread_id":false,"thread_settings":"not metadata","text":"not collected"}),
            )
            + &record(
                "event_msg",
                serde_json::json!({"type":"other_event","id":false}),
            );
        assert_eq!(parse(input.as_bytes(), "mine").unwrap(), Profile::Workspace);
    }
    #[test]
    fn rejects_unknown_inherited_and_malformed_profiles() {
        for v in [
            serde_json::json!({"id":":danger-full-access"}),
            serde_json::json!({"id":":workspace","extends":"custom"}),
            serde_json::json!({"id":":workspace","extra":true}),
            serde_json::json!("bad"),
        ] {
            assert!(Profile::from_native(&v).is_err());
        }
        assert!(parse((header() + "{").as_bytes(), "mine").is_err());
        assert!(parse(header().as_bytes(), "mine").is_err());
    }
}
