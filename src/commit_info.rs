//! On-demand, read-only details for a fixed commit selection.
use crate::error_dialog::ErrorDialog;
use std::path::PathBuf;
use tokio::task::JoinHandle;

pub struct Dialog {
    pub dialog: ErrorDialog,
    task: Option<JoinHandle<Result<String, String>>>,
}

impl Dialog {
    pub fn new(repository: PathBuf, oid: String) -> Self {
        Self {
            dialog: ErrorDialog::information(
                "Commit information",
                format!("Commit: {oid}\n\nLoading commit details…"),
            ),
            task: Some(tokio::spawn(
                async move { details(&repository, &oid).await },
            )),
        }
    }

    pub fn ready(&self) -> bool {
        self.task.as_ref().is_some_and(|task| task.is_finished())
            || self
                .dialog
                .copy_task
                .as_ref()
                .is_some_and(|task| task.is_finished())
    }

    pub async fn tick(&mut self) {
        if self.task.as_ref().is_some_and(|task| task.is_finished()) {
            let result = self.task.take().expect("finished commit details").await;
            self.dialog.set_body(match result {
                Ok(Ok(body)) => body,
                Ok(Err(error)) => format!("Could not load commit details:\n\n{error}"),
                Err(error) => format!("Could not load commit details:\n\n{error}"),
            });
        }
        self.dialog.tick().await;
    }
}

impl Drop for Dialog {
    fn drop(&mut self) {
        if let Some(task) = self.task.take() {
            task.abort();
        }
    }
}

async fn details(repository: &std::path::Path, oid: &str) -> Result<String, String> {
    if ![40, 64].contains(&oid.len()) || !oid.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err("Invalid commit object ID".into());
    }
    let bytes = crate::git::read(repository, &[
        "show", "--no-color", "--no-ext-diff", "--no-textconv", "--no-renames",
        "--stat", "--stat-width=72", "--root", "--diff-merges=first-parent",
        "--format=Commit: %H%nParents: %P%nRefs: %D%nAuthor: %an <%ae>%nAuthor date: %aI%nCommitter: %cn <%ce>%nCommit date: %cI%n%n%B",
        oid, "--",
    ]).await?;
    Ok(format!(
        "Repository: {}\n\n{}",
        repository.display(),
        String::from_utf8_lossy(&bytes)
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn git(path: &std::path::Path, args: &[&str]) -> String {
        let output = std::process::Command::new("git")
            .arg("-C")
            .arg(path)
            .args([
                "-c",
                "user.name=Commit Tester",
                "-c",
                "user.email=test@example.invalid",
                "-c",
                "commit.gpgsign=false",
            ])
            .args(args)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).unwrap().trim().into()
    }

    #[tokio::test]
    async fn details_read_selected_commit_instead_of_head_and_include_full_message() {
        let repo = tempfile::tempdir().unwrap();
        git(repo.path(), &["init", "-qb", "main"]);
        std::fs::write(repo.path().join("example.txt"), "one\n").unwrap();
        git(repo.path(), &["add", "example.txt"]);
        git(
            repo.path(),
            &[
                "commit",
                "-qm",
                "First subject\n\nFull body with Unicode: café\nAnother paragraph.",
            ],
        );
        let oid = git(repo.path(), &["rev-parse", "HEAD"]);
        git(
            repo.path(),
            &["commit", "--allow-empty", "-qm", "Later commit"],
        );
        let head = git(repo.path(), &["rev-parse", "HEAD"]);
        let body = details(&repo.path().join(".git"), &oid).await.unwrap();
        for expected in [
            &oid,
            "Parents:",
            "Author: Commit Tester <test@example.invalid>",
            "Author date:",
            "Committer:",
            "Commit date:",
            "First subject",
            "Full body with Unicode: café",
            "Another paragraph.",
            "example.txt",
            "1 insertion",
        ] {
            assert!(body.contains(expected), "missing {expected}: {body}");
        }
        assert!(!body.contains("Later commit"));
        assert!(
            details(repo.path(), &head)
                .await
                .unwrap()
                .contains(&format!("Parents: {oid}"))
        );
        assert!(details(repo.path(), "--all").await.is_err());
        assert!(details(repo.path(), &"0".repeat(40)).await.is_err());
    }
}
