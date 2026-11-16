//! Debounced, bounded status reads kept independent of history refreshes.
use notify::{EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use tokio::task::JoinHandle;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Counts {
    pub staged: usize,
    pub modified: usize,
    pub untracked: usize,
}
impl Counts {
    pub fn dirty(&self) -> bool {
        self.staged + self.modified + self.untracked > 0
    }
    pub fn description(&self) -> String {
        if self.dirty() {
            format!(
                "{} staged, {} modified, {} untracked",
                self.staged, self.modified, self.untracked
            )
        } else {
            "Clean — no uncommitted changes".into()
        }
    }
}
pub type Status = Result<Counts, String>;
fn parse(data: &[u8]) -> Status {
    let mut counts = Counts::default();
    let mut records = data.split(|b| *b == 0).filter(|r| !r.is_empty());
    while let Some(record) = records.next() {
        if record.len() < 4 || record[2] != b' ' {
            return Err("Invalid Git status output".into());
        }
        let (x, y) = (record[0], record[1]);
        if x == b'?' && y == b'?' {
            counts.untracked += 1;
            continue;
        }
        if x == b'!' && y == b'!' {
            continue;
        }
        if x != b' ' {
            counts.staged += 1;
        }
        if y != b' ' {
            counts.modified += 1;
        }
        if matches!(x, b'R' | b'C') || matches!(y, b'R' | b'C') {
            records.next().ok_or("Incomplete Git rename status")?;
        }
    }
    Ok(counts)
}
pub async fn read(path: &Path) -> Status {
    parse(
        &crate::git::read(
            path,
            &[
                "status",
                "--porcelain=v1",
                "-z",
                "--untracked-files=all",
                "--ignore-submodules=none",
            ],
        )
        .await?,
    )
}

#[derive(Default)]
pub struct Monitor {
    watcher: Option<RecommendedWatcher>,
    events: Arc<Mutex<BTreeSet<PathBuf>>>,
    roots: BTreeSet<PathBuf>,
    watched: BTreeSet<PathBuf>,
    fallback: Option<Instant>,
    pending: BTreeMap<PathBuf, Instant>,
    tasks: BTreeMap<PathBuf, JoinHandle<Status>>,
    generation: Option<u64>,
}
impl Monitor {
    pub fn sync(&mut self, browser: &mut crate::browser::Browser) {
        let roots: BTreeSet<_> = browser
            .repositories
            .values()
            .flat_map(|r| &r.worktrees)
            .filter(|w| !w.bare)
            .map(|w| w.path.clone())
            .collect();
        let now = Instant::now();
        if self.watcher.is_none() && !roots.is_empty() {
            let events = self.events.clone();
            self.watcher =
                notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
                    let mut pending = events.lock().unwrap();
                    match event {
                        Ok(event) if !matches!(event.kind, EventKind::Access(_)) => {
                            if event.need_rescan() {
                                pending.insert(PathBuf::new());
                            }
                            pending.extend(event.paths);
                            if pending.len() > 1024 {
                                pending.clear();
                                pending.insert(PathBuf::new());
                            }
                        }
                        Err(_) => {
                            pending.insert(PathBuf::new());
                        }

                        _ => {}
                    }
                })
                .ok();
        }
        for root in self.roots.difference(&roots) {
            if let Some(task) = self.tasks.remove(root) {
                task.abort();
            }
            self.pending.remove(root);
        }
        for root in roots.difference(&self.roots) {
            self.pending.insert(root.clone(), now);
        }
        let desired: BTreeSet<_> = roots
            .iter()
            .cloned()
            .chain(browser.repositories.keys().cloned())
            .collect();
        for path in self.watched.difference(&desired) {
            if let Some(w) = self.watcher.as_mut() {
                let _ = w.unwatch(path);
            }
        }
        self.watched.retain(|p| desired.contains(p));
        for path in desired
            .difference(&self.watched)
            .cloned()
            .collect::<Vec<_>>()
        {
            if self
                .watcher
                .as_mut()
                .is_some_and(|w| w.watch(&path, RecursiveMode::Recursive).is_ok())
            {
                self.watched.insert(path);
            } else {
                self.fallback.get_or_insert(now + Duration::from_secs(5));
            }
        }
        if self.fallback.is_some_and(|due| now >= due) {
            for root in &roots {
                self.pending.entry(root.clone()).or_insert(now);
            }
            self.fallback = Some(now + Duration::from_secs(5));
        }
        if self.generation != Some(browser.git_generation) {
            self.generation = Some(browser.git_generation);
            for root in &roots {
                self.pending.insert(root.clone(), now);
            }
        }
        self.roots = roots;
        browser
            .worktree_status
            .retain(|path, _| self.roots.contains(path));
    }
    pub async fn tick(&mut self, browser: &mut crate::browser::Browser) -> bool {
        self.sync(browser);
        let now = Instant::now();
        let events = std::mem::take(&mut *self.events.lock().unwrap());
        for event in events {
            let metadata = browser
                .repositories
                .keys()
                .find(|common| event.starts_with(common));
            if let Some(common) = metadata {
                let relative = event.strip_prefix(common).unwrap();
                if relative
                    .components()
                    .any(|c| matches!(c.as_os_str().to_str(), Some("objects" | "logs")))
                    || event.extension().is_some_and(|e| e == "lock")
                {
                    continue;
                }
            }
            for root in &self.roots {
                let same_repo = metadata.is_some_and(|id| {
                    browser.repositories[id]
                        .worktrees
                        .iter()
                        .any(|w| &w.path == root)
                });
                if event.as_os_str().is_empty() || event.starts_with(root) || same_repo {
                    self.pending
                        .entry(root.clone())
                        .or_insert(now + Duration::from_millis(300));
                }
            }
        }
        let finished: Vec<_> = self
            .tasks
            .iter()
            .filter(|(_, t)| t.is_finished())
            .map(|(p, _)| p.clone())
            .collect();
        let mut changed = false;
        for path in finished {
            let result = self
                .tasks
                .remove(&path)
                .unwrap()
                .await
                .unwrap_or_else(|e| Err(e.to_string()));
            if !self.pending.contains_key(&path)
                && browser.worktree_status.get(&path) != Some(&result)
            {
                browser.worktree_status.insert(path, result);
                changed = true;
            }
        }
        let due: Vec<_> = self
            .pending
            .iter()
            .filter(|(p, due)| **due <= now && !self.tasks.contains_key(*p))
            .take(4usize.saturating_sub(self.tasks.len()))
            .map(|(p, _)| p.clone())
            .collect();
        for path in due {
            self.pending.remove(&path);
            let target = path.clone();
            self.tasks
                .insert(path, tokio::spawn(async move { read(&target).await }));
        }
        changed
    }
    pub async fn shutdown(&mut self) {
        self.watcher = None;
        self.roots.clear();
        self.watched.clear();
        self.events.lock().unwrap().clear();
        self.pending.clear();
        for (_, task) in std::mem::take(&mut self.tasks) {
            task.abort();
            let _ = task.await;
        }
    }
}
impl Drop for Monitor {
    fn drop(&mut self) {
        for task in self.tasks.values() {
            task.abort();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn status_counts_handle_renames_and_unusual_paths() {
        let counts = parse(
            b"M  staged\0 M unstaged\0MM both\0?? new\nfile\0R  new name\0old name\0!! ignored\0",
        )
        .unwrap();
        assert_eq!(
            counts,
            Counts {
                staged: 3,
                modified: 2,
                untracked: 1
            }
        );
        assert!(counts.dirty());
        assert!(!parse(b"").unwrap().dirty());
        assert!(parse(b"R  missing source\0").is_err());
    }
    #[tokio::test]
    async fn real_changes_refresh_both_checkouts_and_ignore_ignored_files() {
        let temp = tempfile::tempdir().unwrap();
        let main = temp.path().join("main");
        let linked = temp.path().join("linked");
        std::fs::create_dir(&main).unwrap();
        let git = |path: &Path, args: &[&str]| {
            let output = std::process::Command::new("git")
                .arg("-C")
                .arg(path)
                .args(args)
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
        };
        git(&main, &["init", "-qb", "main"]);
        std::fs::write(main.join("tracked"), "initial").unwrap();
        std::fs::write(main.join(".gitignore"), "ignored\n").unwrap();
        git(&main, &["add", "."]);
        git(
            &main,
            &[
                "-c",
                "user.name=Test",
                "-c",
                "user.email=t@example.invalid",
                "commit",
                "-qm",
                "initial",
            ],
        );
        git(
            &main,
            &[
                "worktree",
                "add",
                "-qb",
                "feature",
                linked.to_str().unwrap(),
            ],
        );
        let repository = crate::git::discover(&main).await.unwrap();
        let mut browser = crate::browser::Browser::new(vec![]);
        browser
            .repositories
            .insert(repository.common_dir.clone(), repository);
        let mut monitor = Monitor::default();
        async fn expect(
            monitor: &mut Monitor,
            browser: &mut crate::browser::Browser,
            path: &Path,
            expected: Counts,
        ) {
            tokio::time::timeout(Duration::from_secs(8), async {
                loop {
                    monitor.tick(browser).await;
                    if browser.worktree_status.get(path) == Some(&Ok(expected.clone())) {
                        break;
                    }
                    tokio::time::sleep(Duration::from_millis(30)).await;
                }
            })
            .await
            .expect("status did not refresh");
        }
        expect(&mut monitor, &mut browser, &linked, Counts::default()).await;
        std::fs::write(linked.join("tracked"), "edited").unwrap();
        expect(
            &mut monitor,
            &mut browser,
            &linked,
            Counts {
                modified: 1,
                ..Counts::default()
            },
        )
        .await;
        git(&linked, &["add", "tracked"]);
        expect(
            &mut monitor,
            &mut browser,
            &linked,
            Counts {
                staged: 1,
                ..Counts::default()
            },
        )
        .await;
        git(&linked, &["reset", "--hard", "HEAD"]);
        expect(&mut monitor, &mut browser, &linked, Counts::default()).await;
        std::fs::write(main.join("new\nfile"), "new").unwrap();
        expect(
            &mut monitor,
            &mut browser,
            &main,
            Counts {
                untracked: 1,
                ..Counts::default()
            },
        )
        .await;
        std::fs::remove_file(main.join("new\nfile")).unwrap();
        std::fs::write(main.join("ignored"), "ignored").unwrap();
        expect(&mut monitor, &mut browser, &main, Counts::default()).await;
        assert_eq!(read(&main).await.unwrap(), Counts::default());
        monitor.shutdown().await;
    }
}
