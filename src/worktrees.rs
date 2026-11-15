//! Read-only Git worktree discovery. The original session directory defines the project.
use std::{
    os::unix::ffi::OsStringExt,
    path::{Path, PathBuf},
    time::Duration,
};
use tokio::{process::Command, time::timeout};

#[derive(Clone, Debug)]
pub struct Worktree {
    pub path: PathBuf,
    pub branch: Option<String>,
    pub main: bool,
    pub unavailable: Option<String>,
}
async fn git(program: &std::ffi::OsStr, origin: &Path, args: &[&str]) -> Result<Vec<u8>, String> {
    let output = Command::new(program)
        .arg("-C")
        .arg(origin)
        .args(args)
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_COMMON_DIR")
        .env("GIT_OPTIONAL_LOCKS", "0")
        .kill_on_drop(true)
        .output()
        .await
        .map_err(|e| format!("Could not run Git: {e}"))?;
    if !output.status.success() {
        return Err(format!(
            "Original project unavailable: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(output.stdout)
}
pub async fn discover(origin: &Path) -> Result<Vec<Worktree>, String> {
    discover_with(std::ffi::OsStr::new("git"), origin).await
}
async fn discover_with(program: &std::ffi::OsStr, origin: &Path) -> Result<Vec<Worktree>, String> {
    if origin.as_os_str().is_empty() {
        return Err(
            "Original project unavailable: the session has no recorded folder. Use Folders.".into(),
        );
    }
    timeout(Duration::from_secs(5), async {
        if git(program, origin, &["rev-parse", "--is-inside-work-tree"]).await? != b"true\n" {
            return Err("The original folder is not in a Git working tree.".into());
        }
        let data = git(program, origin, &["worktree", "list", "--porcelain", "-z"]).await?;
        parse(&data)
    })
    .await
    .map_err(|_| "Git worktree discovery timed out. Use Folders or try again.".to_owned())?
}
fn parse(data: &[u8]) -> Result<Vec<Worktree>, String> {
    let mut entries = vec![];
    let mut current: Option<Worktree> = None;
    for field in data.split(|b| *b == 0) {
        if field.is_empty() {
            if let Some(mut row) = current.take() {
                if row.path.to_str().is_none() {
                    row.unavailable = Some("Path is not valid UTF-8".into());
                } else if !row.path.is_dir() || std::fs::read_dir(&row.path).is_err() {
                    row.unavailable = Some("Folder missing or inaccessible".into());
                }
                entries.push(row);
            }
        } else if let Some(path) = field.strip_prefix(b"worktree ") {
            if current.is_some() {
                return Err("Invalid Git worktree listing.".into());
            }
            current = Some(Worktree {
                path: PathBuf::from(std::ffi::OsString::from_vec(path.to_vec())),
                branch: None,
                main: entries.is_empty(),
                unavailable: None,
            });
        } else if let Some(row) = current.as_mut() {
            if let Some(branch) = field.strip_prefix(b"branch refs/heads/") {
                row.branch = Some(String::from_utf8_lossy(branch).into());
            }
            if field == b"bare" {
                row.unavailable = Some("Bare repository".into());
            }
        }
    }
    if current.is_some() {
        return Err("Incomplete Git worktree listing.".into());
    }
    Ok(entries)
}
pub async fn validate(origin: &Path, target: &Path) -> Result<(), String> {
    let entries = discover(origin).await?;
    let row = entries
        .iter()
        .find(|row| row.path == target)
        .ok_or("This worktree is no longer registered. Choose a destination again.")?;
    if let Some(error) = &row.unavailable {
        return Err(error.clone());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn unavailable_git_and_timeout_are_actionable() {
        use std::os::unix::fs::PermissionsExt;
        let tmp = tempfile::tempdir().unwrap();
        let missing = tmp.path().join("missing-git");
        assert!(
            discover_with(missing.as_os_str(), tmp.path())
                .await
                .unwrap_err()
                .contains("Could not run Git")
        );
        let fake = tmp.path().join("slow-git");
        std::fs::write(&fake, "#!/bin/sh\nexec sleep 15\n").unwrap();
        std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert!(
            discover_with(fake.as_os_str(), tmp.path())
                .await
                .unwrap_err()
                .contains("timed out")
        );
    }
    #[test]
    fn nul_records_preserve_newlines_and_reject_lossy_paths() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("line\nbreak");
        std::fs::create_dir(&path).unwrap();
        let bytes = format!(
            "worktree {}\0HEAD abc\0branch refs/heads/topic\0\0",
            path.display()
        );
        let rows = parse(bytes.as_bytes()).unwrap();
        assert_eq!(rows[0].path, path);
        assert!(rows[0].unavailable.is_none());
        let rows = parse(b"worktree /invalid\xff\0HEAD abc\0detached\0\0").unwrap();
        assert!(rows[0].unavailable.as_ref().unwrap().contains("UTF-8"));
        assert!(parse(b"worktree /incomplete").is_err());
    }
}
