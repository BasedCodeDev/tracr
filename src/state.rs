//! Optional local browser preferences, separate from provider session storage.
use serde::{Deserialize, Serialize};
use std::{
    fs,
    io::{self, Write},
    path::{Path, PathBuf},
};

#[derive(Serialize, Deserialize)]
struct State {
    folder: PathBuf,
}

pub fn load(path: &Path) -> Result<Option<PathBuf>, String> {
    let data = match fs::read(path) {
        Ok(data) => data,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(format!("Could not read browser state: {error}")),
    };
    let state: State =
        serde_json::from_slice(&data).map_err(|e| format!("Invalid browser state: {e}"))?;
    if !state.folder.is_absolute() {
        return Err("Invalid browser state: folder must be absolute".into());
    }
    if fs::metadata(&state.folder).is_ok_and(|m| !m.is_dir()) {
        return Ok(None);
    }
    Ok(Some(state.folder))
}

pub fn save(path: &Path, folder: &Path) -> io::Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| io::Error::other("state path has no parent"))?;
    fs::create_dir_all(parent)?;
    let temporary = parent.join(format!(".browser-state-{}.tmp", std::process::id()));
    let result = (|| {
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)?;
        file.write_all(&serde_json::to_vec(&State {
            folder: folder.to_owned(),
        })?)?;
        file.sync_all()?;
        fs::rename(&temporary, path)
    })();
    let _ = fs::remove_file(temporary);
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn state_roundtrip_and_invalid_paths() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state/browser-state.json");
        assert_eq!(load(&path).unwrap(), None);
        let missing = dir.path().join("missing");
        save(&path, &missing).unwrap();
        assert_eq!(load(&path).unwrap(), Some(missing));
        save(&path, &path).unwrap();
        assert_eq!(load(&path).unwrap(), None);
        fs::write(&path, "broken").unwrap();
        assert!(load(&path).is_err());
        fs::write(&path, r#"{"folder":"relative"}"#).unwrap();
        assert!(load(&path).is_err());
    }
}
