//! Optional local browser preferences, separate from provider session storage.
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    fs,
    io::{self, Write},
    path::{Path, PathBuf},
};

#[derive(Serialize, Deserialize)]
struct State {
    folder: PathBuf,
    #[serde(default)]
    focus_by_launch_folder: BTreeMap<PathBuf, Option<PathBuf>>,
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

/// None means this launch folder has no saved preference; Some(None) means unfocused.
pub fn load_focus(path: &Path, launch_folder: &Path) -> Result<Option<Option<PathBuf>>, String> {
    let data = match fs::read(path) {
        Ok(data) => data,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(format!("Could not read browser state: {error}")),
    };
    let state: State =
        serde_json::from_slice(&data).map_err(|error| format!("Invalid browser state: {error}"))?;
    let focus = state.focus_by_launch_folder.get(launch_folder).cloned();
    if focus
        .as_ref()
        .and_then(|value| value.as_ref())
        .is_some_and(|root| !root.is_absolute())
    {
        return Err("Invalid browser state: focus folder must be absolute".into());
    }
    Ok(focus)
}

pub fn save(path: &Path, folder: &Path) -> io::Result<()> {
    save_context(path, folder, None)
}

pub fn save_context(
    path: &Path,
    folder: &Path,
    context: Option<(&Path, Option<&Path>)>,
) -> io::Result<()> {
    let fresh = || State {
        folder: folder.to_owned(),
        focus_by_launch_folder: BTreeMap::new(),
    };
    let mut state = match fs::read(path) {
        // Loading already reports corrupt state; saving repairs it on normal exit.
        Ok(data) => serde_json::from_slice::<State>(&data).unwrap_or_else(|_| fresh()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => fresh(),
        Err(error) => return Err(error),
    };
    state.folder = folder.to_owned();
    if let Some((launch_folder, focus)) = context {
        state
            .focus_by_launch_folder
            .insert(launch_folder.to_owned(), focus.map(Path::to_owned));
    }
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
        file.write_all(&serde_json::to_vec(&state)?)?;
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
    fn focus_is_independent_per_launch_folder_and_preserves_legacy_state() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.json");
        let a = dir.path().join("a");
        let b = dir.path().join("b");
        let nested = a.join("nested");
        fs::write(&path, serde_json::json!({"folder": a}).to_string()).unwrap();
        assert_eq!(load_focus(&path, &a).unwrap(), None);
        save_context(&path, &nested, Some((&a, Some(&nested)))).unwrap();
        save_context(&path, &b, Some((&b, None))).unwrap();
        assert_eq!(load_focus(&path, &a).unwrap(), Some(Some(nested.clone())));
        assert_eq!(load_focus(&path, &b).unwrap(), Some(None));
        assert_eq!(load(&path).unwrap(), Some(b.clone()));
        save(&path, &a).unwrap();
        assert_eq!(load_focus(&path, &a).unwrap(), Some(Some(nested)));
        save_context(&path, &a, Some((&a, None))).unwrap();
        assert_eq!(load_focus(&path, &a).unwrap(), Some(None));
        assert_eq!(load_focus(&path, &b).unwrap(), Some(None));
    }

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
        save_context(&path, dir.path(), Some((dir.path(), None))).unwrap();
        assert_eq!(load(&path).unwrap(), Some(dir.path().to_owned()));
        assert_eq!(load_focus(&path, dir.path()).unwrap(), Some(None));
        fs::write(&path, r#"{"folder":"relative"}"#).unwrap();
        assert!(load(&path).is_err());
    }
}
