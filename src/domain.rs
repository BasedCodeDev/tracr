use serde::{Deserialize, Serialize};
use std::{
    io,
    path::{Path, PathBuf},
};

macro_rules! identifier {
    ($name:ident) => {
        #[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq, PartialOrd, Ord, Hash)]
        #[serde(transparent)]
        pub struct $name(pub String);
    };
}
identifier!(ProviderId);
identifier!(SourceId);

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SessionKey {
    pub provider: ProviderId,
    pub source_id: SourceId,
    pub native_id: String,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
pub struct GitMetadata {
    pub branch: Option<String>,
    pub origin_url: Option<String>,
    pub commit: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct SessionSummary {
    pub key: SessionKey,
    pub title: String,
    pub cwd: Option<PathBuf>,
    /// Unix seconds, matching the provider's recorded timestamps.
    pub created_at: i64,
    pub updated_at: i64,
    #[serde(default)]
    pub archived: bool,
    #[serde(default)]
    pub can_accept_direct_input: Option<bool>,
    pub source_kind: Option<String>,
    pub git: Option<GitMetadata>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum FolderStatus {
    Available,
    Unlinked,
    Inaccessible { reason: String },
    Unknown,
}

/// Do not infer deletion from permission failures or an unusable path.
pub fn classify_folder(path: Option<&Path>) -> FolderStatus {
    let Some(path) = path.filter(|p| p.is_absolute()) else {
        return FolderStatus::Unknown;
    };
    match std::fs::read_dir(path) {
        Ok(_) => FolderStatus::Available,
        Err(error) => classify_folder_error(error),
    }
}

pub fn classify_folder_error(error: io::Error) -> FolderStatus {
    if error.kind() == io::ErrorKind::NotFound {
        FolderStatus::Unlinked
    } else {
        FolderStatus::Inaccessible {
            reason: error.to_string(),
        }
    }
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum Capability {
    Supported,
    Unsupported { reason: String },
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct ProviderCapabilities {
    pub list: Capability,
    pub read: Capability,
    pub resume: Capability,
    pub relocation: Capability,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn folder_classification_distinguishes_missing_and_permission_errors() {
        let temp = tempfile::tempdir().unwrap();
        assert_eq!(classify_folder(Some(temp.path())), FolderStatus::Available);
        assert_eq!(
            classify_folder(Some(&temp.path().join("absent"))),
            FolderStatus::Unlinked
        );
        assert_eq!(classify_folder(None), FolderStatus::Unknown);
        assert_eq!(
            classify_folder(Some(Path::new("relative"))),
            FolderStatus::Unknown
        );
        assert!(matches!(
            classify_folder_error(io::Error::from(io::ErrorKind::PermissionDenied)),
            FolderStatus::Inaccessible { .. }
        ));
        let file = temp.path().join("file");
        std::fs::write(&file, "text").unwrap();
        assert!(matches!(
            classify_folder(Some(&file)),
            FolderStatus::Inaccessible { .. }
        ));
    }
}

/// A prepared launch. Paths and arguments are passed directly, never through a shell.
#[derive(Clone, Debug)]
pub struct LaunchCommand {
    pub executable: PathBuf,
    pub args: Vec<std::ffi::OsString>,
    pub cwd: PathBuf,
    pub env: Vec<(std::ffi::OsString, std::ffi::OsString)>,
}

#[derive(Clone, Debug)]
pub struct ResumeCommand {
    pub launch: LaunchCommand,
    pub session: SessionSummary,
}
impl std::ops::Deref for ResumeCommand {
    type Target = LaunchCommand;
    fn deref(&self) -> &Self::Target {
        &self.launch
    }
}

pub fn resume_blocker(session: &SessionSummary) -> Option<&'static str> {
    if session.archived {
        Some("Unarchive this session in Codex before resuming.")
    } else if session.can_accept_direct_input == Some(false) {
        Some("Codex reports that this session cannot accept direct input.")
    } else {
        None
    }
}
