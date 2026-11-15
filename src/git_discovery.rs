//! Bounded, generation-scoped discovery independent of graph loading.
use crate::git;
use std::{
    collections::{BTreeMap, BTreeSet},
    path::PathBuf,
};
use tokio::task::JoinHandle;

#[derive(Clone, Debug)]
pub enum Outcome {
    Repository {
        repository: git::Directory,
        root: Option<PathBuf>,
    },
    NotRepository,
    Unavailable(String),
}
#[derive(Clone, Debug)]
pub struct Ownership {
    pub generation: u64,
    pub outcome: Option<Outcome>,
    pub previous: Option<Outcome>,
}
impl Ownership {
    pub fn effective(&self) -> Option<&Outcome> {
        match &self.outcome {
            Some(Outcome::Unavailable(_)) | None => self.previous.as_ref(),
            outcome => outcome.as_ref(),
        }
    }
    pub fn stale(&self) -> bool {
        self.outcome.is_none() || matches!(self.outcome, Some(Outcome::Unavailable(_)))
    }
}
async fn resolve(path: PathBuf) -> Outcome {
    if !path.is_dir() {
        return Outcome::Unavailable("Directory is unavailable".into());
    }
    match git::resolve_directory(&path).await {
        Ok(repository) => match git::checkout_path(&path).await {
            Ok(root) => Outcome::Repository { repository, root },
            Err(error) => Outcome::Unavailable(error),
        },
        Err(error) if error.starts_with("fatal: not a git repository") => Outcome::NotRepository,
        Err(error) => Outcome::Unavailable(error),
    }
}
pub type BackendFuture<T> = std::pin::Pin<Box<dyn std::future::Future<Output = T> + Send>>;
pub trait Backend: Send + Sync {
    fn directory(&self, path: PathBuf) -> BackendFuture<Outcome>;
    fn inventory(&self, path: PathBuf) -> BackendFuture<Result<git::Repository, String>>;
}
struct Native;
impl Backend for Native {
    fn directory(&self, path: PathBuf) -> BackendFuture<Outcome> {
        Box::pin(resolve(path))
    }
    fn inventory(&self, path: PathBuf) -> BackendFuture<Result<git::Repository, String>> {
        Box::pin(async move { git::inventory(&path).await })
    }
}
#[derive(Clone, Debug)]
pub enum Update {
    Directory {
        generation: u64,
        token: u64,
        path: PathBuf,
        outcome: Outcome,
    },
    Inventory {
        generation: u64,
        token: u64,
        id: PathBuf,
        result: Result<git::Repository, String>,
    },
}
pub struct Controller {
    backend: std::sync::Arc<dyn Backend>,
    next_token: u64,
    tokens: BTreeMap<(bool, PathBuf), u64>,
    generation: u64,
    initialized: bool,
    candidates: Vec<PathBuf>,
    schedule_dirty: bool,
    pub results: BTreeMap<PathBuf, Outcome>,
    tasks: BTreeMap<PathBuf, JoinHandle<Outcome>>,
    inventories: BTreeSet<PathBuf>,
    inventory_tasks: BTreeMap<PathBuf, JoinHandle<Result<git::Repository, String>>>,
}
impl Default for Controller {
    fn default() -> Self {
        Self::with_backend(std::sync::Arc::new(Native))
    }
}
impl Controller {
    pub fn with_backend(backend: std::sync::Arc<dyn Backend>) -> Self {
        Self {
            backend,
            next_token: 0,
            tokens: BTreeMap::new(),
            generation: 0,
            initialized: false,
            candidates: vec![],
            schedule_dirty: true,
            results: BTreeMap::new(),
            tasks: BTreeMap::new(),
            inventories: BTreeSet::new(),
            inventory_tasks: BTreeMap::new(),
        }
    }
    fn reserve(&mut self, inventory: bool, path: &std::path::Path) {
        self.next_token += 1;
        self.tokens
            .insert((inventory, path.to_owned()), self.next_token);
    }
    pub fn current(&self, update: &Update) -> bool {
        let (generation, token, key) = match update {
            Update::Directory {
                generation,
                token,
                path,
                ..
            } => (*generation, *token, (false, path.clone())),
            Update::Inventory {
                generation,
                token,
                id,
                ..
            } => (*generation, *token, (true, id.clone())),
        };
        generation == self.generation && self.tokens.get(&key) == Some(&token)
    }
    pub fn apply_batch(&mut self, browser: &mut crate::browser::Browser, updates: Vec<Update>) {
        if updates.is_empty() {
            return;
        }
        let previous_rows = browser.folder_rows();
        let ancestors = browser.navigation_ancestors();
        let selected = browser.selected.clone();
        for update in updates {
            if !self.current(&update) {
                continue;
            }
            match update {
                Update::Directory {
                    generation,
                    path,
                    outcome,
                    ..
                } => browser.apply_ownership(generation, path, outcome),
                Update::Inventory {
                    generation,
                    id,
                    result,
                    ..
                } => {
                    let invalid = browser.apply_inventory(generation, id, result);
                    self.forget(&invalid);
                }
            }
        }
        let pending = self.inventory_tasks.keys().cloned().collect();
        for id in browser.retire_repositories_except(&pending) {
            self.forget_repository(&id);
        }
        // Unrelated discovery must not move a deliberately selected missing folder.
        if previous_rows
            .iter()
            .map(|row| &row.filter)
            .ne(browser.folder_rows().iter().map(|row| &row.filter))
        {
            browser.recover_navigation(&ancestors, selected);
        }
    }

    pub fn generation(&mut self, generation: u64) {
        if !self.initialized || generation != self.generation {
            self.cancel();
            self.results.clear();
            self.inventories.clear();
            self.generation = generation;
            self.initialized = true;
        }
    }
    pub fn cancel(&mut self) {
        self.schedule_dirty = true;
        self.tokens.clear();
        for (_, task) in std::mem::take(&mut self.tasks) {
            task.abort();
        }
        for (id, task) in std::mem::take(&mut self.inventory_tasks) {
            self.inventories.remove(&id);
            task.abort();
        }
    }
    /// Finish cancellation before handing the terminal to another process.
    pub async fn shutdown(&mut self) {
        self.schedule_dirty = true;
        self.tokens.clear();
        let directories = std::mem::take(&mut self.tasks);
        let inventories = std::mem::take(&mut self.inventory_tasks);
        for task in directories.values() {
            task.abort();
        }
        for (id, task) in &inventories {
            self.inventories.remove(id);
            task.abort();
        }
        for (_, task) in directories {
            let _ = task.await;
        }
        for (_, task) in inventories {
            let _ = task.await;
        }
    }
    pub fn pending(&self) -> bool {
        !self.tasks.is_empty() || !self.inventory_tasks.is_empty()
    }
    pub fn refresh_repository(&mut self, id: &std::path::Path, topology: bool) {
        self.schedule_dirty = true;
        self.inventories.remove(id);
        self.tokens.remove(&(true, id.to_owned()));
        if let Some(task) = self.inventory_tasks.remove(id) {
            task.abort();
        }
        if topology {
            let paths: Vec<_> = self.results.iter().filter_map(|(path, outcome)|
                matches!(outcome, Outcome::Repository { repository, .. } if repository.common_dir == id).then_some(path.clone())).collect();
            self.forget(&paths);
        }
    }
    pub fn forget_repository(&mut self, id: &std::path::Path) {
        self.schedule_dirty = true;
        self.tokens.remove(&(true, id.to_owned()));
        if let Some(task) = self.inventory_tasks.remove(id) {
            task.abort();
        }
        self.inventories.remove(id);
        self.results.retain(|_, result| !matches!(result, Outcome::Repository { repository, .. } if repository.common_dir == id));
    }
    pub fn forget(&mut self, paths: &[PathBuf]) {
        self.schedule_dirty |= !paths.is_empty();
        for path in paths {
            self.tokens.remove(&(false, path.clone()));
            self.results.remove(path);
            if let Some(task) = self.tasks.remove(path) {
                task.abort();
            }
        }
    }
    pub fn set_candidates(&mut self, candidates: Vec<PathBuf>) {
        let mut seen = BTreeSet::new();
        self.candidates = candidates
            .into_iter()
            .filter(|path| seen.insert(path.clone()))
            .collect();
        self.schedule_dirty = true;
    }
    pub fn needs_poll(&self) -> bool {
        self.schedule_dirty
            || self.tasks.values().any(|task| task.is_finished())
            || self.inventory_tasks.values().any(|task| task.is_finished())
    }
    pub async fn tick(&mut self, candidates: Vec<PathBuf>) -> Vec<Update> {
        self.set_candidates(candidates);
        self.poll().await
    }
    pub async fn poll(&mut self) -> Vec<Update> {
        if !self.needs_poll() {
            return vec![];
        }
        self.schedule_dirty = false;
        let candidates = self.candidates.clone();
        let mut updates = Vec::new();
        let done: Vec<_> = self
            .tasks
            .iter()
            .filter(|(_, t)| t.is_finished())
            .map(|(p, _)| p.clone())
            .collect();
        for path in done {
            let outcome = self
                .tasks
                .remove(&path)
                .unwrap()
                .await
                .unwrap_or_else(|e| Outcome::Unavailable(e.to_string()));
            self.results.insert(path.clone(), outcome.clone());
            updates.push(Update::Directory {
                token: self.tokens[&(false, path.clone())],
                generation: self.generation,
                path,
                outcome,
            });
        }
        let done: Vec<_> = self
            .inventory_tasks
            .iter()
            .filter(|(_, t)| t.is_finished())
            .map(|(p, _)| p.clone())
            .collect();
        for id in done {
            let result = self
                .inventory_tasks
                .remove(&id)
                .unwrap()
                .await
                .unwrap_or_else(|e| Err(e.to_string()));
            updates.push(Update::Inventory {
                token: self.tokens[&(true, id.clone())],
                generation: self.generation,
                id,
                result,
            });
        }
        let ids: BTreeSet<_> = self
            .results
            .values()
            .filter_map(|o| match o {
                Outcome::Repository { repository, .. } => Some(repository.common_dir.clone()),
                _ => None,
            })
            .collect();
        for id in ids {
            if self.tasks.len() + self.inventory_tasks.len() >= 2 {
                break;
            }
            if self.inventories.insert(id.clone()) {
                self.reserve(true, &id);
                let future = self.backend.inventory(id.clone());
                self.inventory_tasks
                    .insert(id.clone(), tokio::spawn(future));
            }
        }
        for path in candidates {
            if self.tasks.len() + self.inventory_tasks.len() >= 2 {
                break;
            }
            if !self.results.contains_key(&path) && !self.tasks.contains_key(&path) {
                self.reserve(false, &path);
                self.tasks
                    .insert(path.clone(), tokio::spawn(self.backend.directory(path)));
            }
        }
        updates
    }
}
impl Drop for Controller {
    fn drop(&mut self) {
        self.cancel();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn limits_jobs_and_caches_negative_results() {
        let dir = tempfile::tempdir().unwrap();
        let paths: Vec<_> = (0..5)
            .map(|i| {
                let p = dir.path().join(i.to_string());
                std::fs::create_dir(&p).unwrap();
                p
            })
            .collect();
        let mut controller = Controller::default();
        controller.generation(1);
        controller.tick(paths.clone()).await;
        assert_eq!(controller.tasks.len(), 2);
        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            while controller.results.len() < 5 {
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
                controller.tick(paths.clone()).await;
                assert!(controller.tasks.len() <= 2);
            }
        })
        .await
        .unwrap();
        assert!(
            controller
                .results
                .values()
                .all(|r| matches!(r, Outcome::NotRepository))
        );
        controller.tick(paths).await;
        assert!(controller.tasks.is_empty());
        controller.generation(2);
        assert!(controller.results.is_empty());
    }
    #[tokio::test]
    async fn unchanged_candidates_settle_without_rescheduling() {
        let dir = tempfile::tempdir().unwrap();
        let mut controller = Controller::default();
        controller.generation(1);
        controller.set_candidates(vec![dir.path().to_owned(), dir.path().to_owned()]);
        assert_eq!(controller.candidates.len(), 1);
        controller.poll().await;
        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            while controller.pending() {
                tokio::task::yield_now().await;
                controller.poll().await;
            }
        })
        .await
        .unwrap();
        for _ in 0..1000 {
            assert!(!controller.needs_poll());
            assert!(controller.poll().await.is_empty());
        }
        controller.forget(&[dir.path().to_owned()]);
        assert!(controller.needs_poll());
        controller.poll().await;
        assert!(controller.pending());
        controller.shutdown().await;
    }
}

#[cfg(test)]
mod inventory_tests {
    use super::*;
    #[tokio::test]
    async fn real_inventory_is_deduplicated_and_refreshes_removals() {
        let dir = tempfile::tempdir().unwrap();
        let main = dir.path().join("main");
        let linked = dir.path().join("linked");
        let run = |cwd: &std::path::Path, args: &[&str]| {
            let result = std::process::Command::new("git")
                .arg("-C")
                .arg(cwd)
                .args(args)
                .env("GIT_CONFIG_GLOBAL", "/dev/null")
                .env("GIT_CONFIG_NOSYSTEM", "1")
                .output()
                .unwrap();
            assert!(
                result.status.success(),
                "{}",
                String::from_utf8_lossy(&result.stderr)
            );
        };
        std::fs::create_dir(&main).unwrap();
        run(&main, &["init", "-q"]);
        run(
            &main,
            &[
                "-c",
                "user.name=Test",
                "-c",
                "user.email=test@example.invalid",
                "commit",
                "--allow-empty",
                "-qm",
                "root",
            ],
        );
        run(
            &main,
            &["worktree", "add", "-qb", "linked", linked.to_str().unwrap()],
        );
        std::fs::create_dir(main.join("src")).unwrap();
        let paths = vec![main.clone(), main.join("src"), linked.clone()];
        let mut controller = Controller::default();
        controller.generation(1);
        let mut snapshots = Vec::new();
        for _ in 0..500 {
            for update in controller.tick(paths.clone()).await {
                if let Update::Inventory { result, .. } = update {
                    snapshots.push(result.unwrap());
                }
            }
            if controller.results.len() == 3
                && controller.tasks.is_empty()
                && controller.inventory_tasks.is_empty()
            {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        assert_eq!(snapshots.len(), 1);
        assert_eq!(snapshots[0].worktrees.len(), 2);
        run(&main, &["worktree", "remove", linked.to_str().unwrap()]);
        controller.generation(2);
        let refreshed = tokio::time::timeout(std::time::Duration::from_secs(5), async {
            loop {
                for update in controller.tick(vec![main.clone()]).await {
                    if let Update::Inventory {
                        generation, result, ..
                    } = update
                    {
                        assert_eq!(generation, 2);
                        return result.unwrap();
                    }
                }
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        assert_eq!(refreshed.worktrees.len(), 1);
    }
}

#[cfg(test)]
mod lifecycle_tests {
    use super::*;
    use tokio::sync::{mpsc, oneshot};
    struct Controlled {
        requests: mpsc::UnboundedSender<oneshot::Sender<Outcome>>,
        inventories:
            Option<mpsc::UnboundedSender<oneshot::Sender<Result<git::Repository, String>>>>,
    }
    impl Backend for Controlled {
        fn directory(&self, _: PathBuf) -> BackendFuture<Outcome> {
            let (tx, rx) = oneshot::channel();
            self.requests.send(tx).unwrap();
            Box::pin(async move { rx.await.unwrap_or(Outcome::NotRepository) })
        }
        fn inventory(&self, _: PathBuf) -> BackendFuture<Result<git::Repository, String>> {
            if let Some(requests) = &self.inventories {
                let (tx, rx) = oneshot::channel();
                requests.send(tx).unwrap();
                Box::pin(async move { rx.await.unwrap_or_else(|_| Err("cancelled".into())) })
            } else {
                Box::pin(async { Err("controlled failure".into()) })
            }
        }
    }
    #[tokio::test]
    async fn cancelled_probe_can_restart_and_queued_results_are_invalidated() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let mut controller = Controller::with_backend(std::sync::Arc::new(Controlled {
            requests: tx,
            inventories: None,
        }));
        controller.generation(1);
        let path = PathBuf::from("/test");
        controller.tick(vec![path.clone()]).await;
        let mut first = rx.recv().await.unwrap();
        controller.shutdown().await;
        assert!(!controller.pending());
        assert!(first.is_closed());
        first.closed().await;
        controller.tick(vec![path.clone()]).await;
        rx.recv()
            .await
            .unwrap()
            .send(Outcome::NotRepository)
            .unwrap();
        let updates = tokio::time::timeout(std::time::Duration::from_secs(2), async {
            loop {
                tokio::task::yield_now().await;
                let updates = controller.tick(vec![]).await;
                if !updates.is_empty() {
                    break updates;
                }
            }
        })
        .await
        .unwrap();
        assert!(controller.current(&updates[0]));
        controller.forget(&[path]);
        assert!(!controller.current(&updates[0]));
    }
    #[tokio::test]
    async fn generation_change_invalidates_completed_but_unapplied_result() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let mut controller = Controller::with_backend(std::sync::Arc::new(Controlled {
            requests: tx,
            inventories: None,
        }));
        controller.generation(3);
        controller.tick(vec!["/test".into()]).await;
        rx.recv()
            .await
            .unwrap()
            .send(Outcome::NotRepository)
            .unwrap();
        let update = tokio::time::timeout(std::time::Duration::from_secs(2), async {
            loop {
                tokio::task::yield_now().await;
                if let Some(update) = controller.tick(vec![]).await.pop() {
                    break update;
                }
            }
        })
        .await
        .unwrap();
        controller.generation(4);
        assert!(!controller.current(&update));
    }
    #[tokio::test]
    async fn inventory_cancellation_releases_reservation_and_rejects_queued_result() {
        let (tx, _) = mpsc::unbounded_channel();
        let (inventory_tx, mut inventory_rx) = mpsc::unbounded_channel();
        let mut controller = Controller::with_backend(std::sync::Arc::new(Controlled {
            requests: tx,
            inventories: Some(inventory_tx),
        }));
        controller.generation(1);
        let id = PathBuf::from("/project/.git");
        controller.results.insert(
            "/project".into(),
            Outcome::Repository {
                repository: git::Directory {
                    common_dir: id.clone(),
                    git_dir: id.clone(),
                },
                root: Some("/project".into()),
            },
        );
        controller.tick(vec![]).await;
        let mut first = inventory_rx.recv().await.unwrap();
        controller.shutdown().await;
        assert!(!controller.pending());
        assert!(first.is_closed());
        first.closed().await;
        assert!(!controller.inventories.contains(&id));
        controller.tick(vec![]).await;
        inventory_rx
            .recv()
            .await
            .unwrap()
            .send(Err("failure".into()))
            .unwrap();
        let update = tokio::time::timeout(std::time::Duration::from_secs(2), async {
            loop {
                tokio::task::yield_now().await;
                if let Some(update) = controller.tick(vec![]).await.pop() {
                    break update;
                }
            }
        })
        .await
        .unwrap();
        assert!(controller.current(&update));
        controller.forget_repository(&id);
        assert!(!controller.current(&update));
        assert!(!controller.inventories.contains(&id));
    }

    #[tokio::test]
    async fn unrelated_probe_does_not_delay_conclusive_retirement() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let mut controller = Controller::with_backend(std::sync::Arc::new(Controlled {
            requests: tx,
            inventories: None,
        }));
        let mut browser = crate::browser::Browser::new(vec![]);
        let id = PathBuf::from("/project/.git");
        let path = PathBuf::from("/project");
        browser.install_repository(git::Repository {
            common_dir: id.clone(),
            git_dir: id.clone(),
            worktrees: vec![],
        });
        browser.apply_ownership(
            0,
            path.clone(),
            Outcome::Repository {
                repository: git::Directory {
                    common_dir: id.clone(),
                    git_dir: id.clone(),
                },
                root: Some(path.clone()),
            },
        );
        browser.invalidate_git();
        controller.generation(browser.git_generation);
        controller
            .tick(vec![path.clone(), "/unrelated".into()])
            .await;
        rx.recv()
            .await
            .unwrap()
            .send(Outcome::NotRepository)
            .unwrap();
        let _unrelated = rx.recv().await.unwrap();
        let updates = tokio::time::timeout(std::time::Duration::from_secs(2), async {
            loop {
                tokio::task::yield_now().await;
                let updates = controller.tick(vec![]).await;
                if !updates.is_empty() {
                    break updates;
                }
            }
        })
        .await
        .unwrap();
        assert!(controller.pending());
        controller.apply_batch(&mut browser, updates);
        assert!(!browser.repositories.contains_key(&id));
    }
    #[test]
    fn metadata_refresh_retries_stale_ownership_without_touching_other_projects() {
        let mut browser = crate::browser::Browser::new(vec![]);
        for name in ["/one", "/two"] {
            let path = PathBuf::from(name);
            let id = path.join(".git");
            browser.ownership.insert(
                path.clone(),
                Ownership {
                    generation: 0,
                    outcome: Some(Outcome::Unavailable("transient failure".into())),
                    previous: Some(Outcome::Repository {
                        repository: git::Directory {
                            common_dir: id.clone(),
                            git_dir: id,
                        },
                        root: Some(path),
                    }),
                },
            );
        }
        let retry = browser.invalidate_git_repository(std::path::Path::new("/one/.git"), false);
        assert_eq!(retry, vec![PathBuf::from("/one")]);
        assert!(
            browser.ownership[std::path::Path::new("/one")]
                .outcome
                .is_none()
        );
        assert!(matches!(
            browser.ownership[std::path::Path::new("/two")].outcome,
            Some(Outcome::Unavailable(_))
        ));
    }
    #[test]
    fn metadata_refresh_is_scoped_to_one_repository() {
        let mut controller = Controller::default();
        controller.generation(0);
        for name in ["/one", "/two"] {
            let path = PathBuf::from(name);
            let id = path.join(".git");
            controller.results.insert(
                path.clone(),
                Outcome::Repository {
                    repository: git::Directory {
                        common_dir: id.clone(),
                        git_dir: id.clone(),
                    },
                    root: Some(path),
                },
            );
            controller.inventories.insert(id);
        }
        controller.refresh_repository(std::path::Path::new("/one/.git"), false);
        assert!(
            controller
                .results
                .contains_key(std::path::Path::new("/one"))
        );
        assert!(
            !controller
                .inventories
                .contains(std::path::Path::new("/one/.git"))
        );
        controller.refresh_repository(std::path::Path::new("/one/.git"), true);
        assert!(
            !controller
                .results
                .contains_key(std::path::Path::new("/one"))
        );
        assert!(
            controller
                .results
                .contains_key(std::path::Path::new("/two"))
        );
        assert!(
            controller
                .inventories
                .contains(std::path::Path::new("/two/.git"))
        );
    }
    #[test]
    fn directory_and_inventory_batches_converge_in_either_order() {
        for inventory_first in [false, true] {
            let mut controller = Controller::default();
            let mut browser = crate::browser::Browser::new(vec![]);
            controller.generation(0);
            let path = PathBuf::from("/project");
            let id = path.join(".git");
            controller.reserve(false, &path);
            let directory = Update::Directory {
                generation: 0,
                token: controller.tokens[&(false, path.clone())],
                path: path.clone(),
                outcome: Outcome::Repository {
                    repository: git::Directory {
                        common_dir: id.clone(),
                        git_dir: id.clone(),
                    },
                    root: Some(path.clone()),
                },
            };
            controller.reserve(true, &id);
            let inventory = Update::Inventory {
                generation: 0,
                token: controller.tokens[&(true, id.clone())],
                id: id.clone(),
                result: Ok(git::Repository {
                    common_dir: id.clone(),
                    git_dir: id.clone(),
                    worktrees: vec![],
                }),
            };
            let updates = if inventory_first {
                vec![inventory, directory]
            } else {
                vec![directory, inventory]
            };
            for update in updates {
                controller.apply_batch(&mut browser, vec![update]);
            }
            assert!(browser.repositories.contains_key(&id));
            assert!(matches!(
                browser.ownership[&path].effective(),
                Some(Outcome::Repository { .. })
            ));
            assert!(browser.retire_repositories().is_empty());
        }
    }
}
