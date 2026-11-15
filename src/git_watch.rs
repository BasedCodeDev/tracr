//! Native metadata notifications. No polling backend and no Git work in callbacks.
use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher, event::ModifyKind};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Change {
    References,
    Topology,
}

#[derive(Default)]
struct Pending {
    change: Option<Change>,
    first: Option<Instant>,
    last: Option<Instant>,
    error: Option<String>,
}
impl Pending {
    fn push(&mut self, change: Change, now: Instant) {
        self.change = Some(self.change.map_or(change, |old| old.max(change)));
        self.first.get_or_insert(now);
        self.last = Some(now);
    }
    fn deadline(&self) -> Option<Instant> {
        Some((self.first? + Duration::from_secs(1)).min(self.last? + Duration::from_millis(200)))
    }
    fn take(&mut self, now: Instant, busy: bool) -> Option<Change> {
        if busy || self.deadline().is_none_or(|deadline| now < deadline) {
            return None;
        }
        self.first = None;
        self.last = None;
        self.change.take()
    }
}

// Worktree metadata is in common/worktrees/<id>, not in its source directory.
fn path_change(common: &Path, path: &Path) -> Option<Change> {
    let relative = path.strip_prefix(common).ok()?;
    let parts: Vec<_> = relative.iter().collect();
    if parts.iter().any(|p| p.to_string_lossy().ends_with(".lock")) {
        return None;
    }
    if parts.is_empty() {
        return Some(Change::Topology);
    }
    let mut names = parts.as_slice();
    if names[0] == "worktrees" {
        if names.len() <= 2 {
            return Some(Change::Topology);
        }
        names = &names[2..];
        if matches!(names[0].to_str(), Some("gitdir" | "commondir" | "locked")) {
            return Some(Change::Topology);
        }
    }
    match names[0].to_str() {
        Some("HEAD" | "packed-refs" | "refs" | "reftable" | "shallow") => Some(Change::References),
        Some("config" | "config.worktree") => Some(Change::Topology),
        Some("info") if names.len() == 1 || names.get(1).is_some_and(|name| *name == "grafts") => {
            Some(Change::References)
        }
        _ => None,
    }
}
fn classify(common: &Path, event: &Event) -> Option<Change> {
    if event.need_rescan() {
        return Some(Change::Topology);
    }
    if matches!(
        event.kind,
        EventKind::Access(_) | EventKind::Modify(ModifyKind::Metadata(_))
    ) {
        return None;
    }
    if event.paths.is_empty() {
        return Some(Change::Topology);
    }
    event
        .paths
        .iter()
        .filter_map(|path| path_change(common, path))
        .max()
}

fn subscriptions(common: &Path) -> std::io::Result<BTreeMap<PathBuf, RecursiveMode>> {
    let mut paths = BTreeMap::new();
    // Parent detects replacement/removal of the common directory; callback filters siblings.
    if let Some(parent) = common.parent() {
        paths.insert(parent.to_owned(), RecursiveMode::NonRecursive);
    }
    paths.insert(common.to_owned(), RecursiveMode::NonRecursive);
    let mut admins = vec![common.to_owned()];
    let registrations = common.join("worktrees");
    if registrations.is_dir() {
        paths.insert(registrations.clone(), RecursiveMode::NonRecursive);
        for entry in std::fs::read_dir(registrations)? {
            let entry = entry?;
            if entry.file_type()?.is_dir() {
                admins.push(entry.path());
            }
        }
    }
    for admin in admins {
        paths.insert(admin.clone(), RecursiveMode::NonRecursive);
        for (name, mode) in [
            ("refs", RecursiveMode::Recursive),
            ("reftable", RecursiveMode::NonRecursive),
            ("info", RecursiveMode::NonRecursive),
        ] {
            let path = admin.join(name);
            if path.is_dir() {
                paths.insert(path, mode);
            }
        }
    }
    Ok(paths)
}

// Notify's Linux watcher shuts down its worker asynchronously on drop. The
// callback's lifetime acknowledgement lets terminal handoff wait for its exit.
struct CallbackLifetime(std::sync::mpsc::Sender<()>);
impl Drop for CallbackLifetime {
    fn drop(&mut self) {
        let _ = self.0.send(());
    }
}

#[derive(Default)]
pub struct Monitor {
    common: Option<PathBuf>,
    checkpoint: u64,
    visible: bool,
    watcher: Option<RecommendedWatcher>,
    stopped: Option<std::sync::mpsc::Receiver<()>>,
    paths: BTreeMap<PathBuf, RecursiveMode>,
    pending: Arc<Mutex<Pending>>,
    pub error: Option<String>,
}
impl Drop for Monitor {
    fn drop(&mut self) {
        self.release();
    }
}
impl Monitor {
    fn release(&mut self) {
        if let Some(watcher) = self.watcher.as_mut() {
            for path in self.paths.keys() {
                let _ = watcher.unwatch(path);
            }
        }
        self.watcher = None;
        if let Some(stopped) = self.stopped.take() {
            let _ = stopped.recv();
        }
    }
    /// A fresh mailbox per subscription generation makes late callbacks harmless.
    pub fn stop(&mut self) {
        self.release();
        self.paths.clear();
        self.pending = Arc::new(Mutex::new(Pending::default()));
        self.common = None;
        self.error = None;
    }
    pub fn context(&mut self, common: Option<PathBuf>, checkpoint: u64, visible: bool) -> bool {
        let changed = self.common != common;
        let retry = self.checkpoint != checkpoint || (!self.visible && visible);
        self.checkpoint = checkpoint;
        self.visible = visible;
        if !changed && !retry {
            return false;
        }
        if changed || self.error.is_some() {
            self.stop();
            self.common = common;
            if let Some(common) = self.common.clone() {
                let pending = self.pending.clone();
                let (done, stopped) = std::sync::mpsc::channel();
                let lifetime = CallbackLifetime(done);
                match notify::recommended_watcher(move |result: notify::Result<Event>| {
                    let _keep_alive = &lifetime;
                    let mut pending = pending.lock().unwrap();
                    match result {
                        Ok(event) => {
                            if let Some(change) = classify(&common, &event) {
                                pending.push(change, Instant::now());
                            }
                        }
                        Err(error) => {
                            pending.error = Some(error.to_string());
                        }
                    }
                }) {
                    Ok(watcher) => {
                        self.watcher = Some(watcher);
                        self.stopped = Some(stopped);
                        self.reconcile();
                    }
                    Err(error) => self.error = Some(error.to_string()),
                }
            }
        }
        // Reconcile after attaching to close the discovery/subscription race.
        if self.common.is_some() {
            self.pending
                .lock()
                .unwrap()
                .push(Change::Topology, Instant::now());
        }
        true
    }
    pub fn reconcile(&mut self) {
        if self.error.is_some() {
            return;
        }
        let Some(common) = self.common.as_ref() else {
            return;
        };
        let result = (|| -> Result<(), String> {
            let wanted = subscriptions(common).map_err(|e| e.to_string())?;
            let Some(watcher) = self.watcher.as_mut() else {
                return Ok(());
            };
            for path in self.paths.keys().filter(|path| !wanted.contains_key(*path)) {
                let _ = watcher.unwatch(path); // Removed directories may already be detached by the kernel.
            }
            // Re-register paths even when they retain their names: atomic directory
            // replacement can invalidate the old inode subscription.
            for (path, mode) in &wanted {
                watcher.watch(path, *mode).map_err(|e| e.to_string())?;
            }
            self.paths = wanted;
            Ok(())
        })();
        if let Err(error) = result {
            self.error = Some(error);
            self.release();
        }
    }
    pub fn poll(&mut self, now: Instant, busy: bool) -> Option<Change> {
        let mut pending = self.pending.lock().unwrap();
        if let Some(error) = pending.error.take() {
            self.error = Some(error);
        }
        let change = pending.take(now, busy);
        drop(pending);
        if self.error.is_some() {
            self.release();
        }
        change
    }
    pub fn deadline(&self) -> Option<Instant> {
        self.pending.lock().unwrap().deadline()
    }
    pub fn repository(&self) -> Option<&Path> {
        self.common.as_deref()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use notify::event::{AccessKind, DataChange, MetadataKind};
    #[test]
    fn bursts_are_bounded_and_busy_retains_one_followup() {
        let start = Instant::now();
        let mut pending = Pending::default();
        for i in 0..10_000 {
            pending.push(Change::References, start + Duration::from_micros(i * 100));
        }
        assert_eq!(pending.deadline(), Some(start + Duration::from_secs(1)));
        assert_eq!(pending.take(start + Duration::from_secs(2), true), None);
        pending.push(Change::Topology, start + Duration::from_secs(2));
        assert_eq!(
            pending.take(start + Duration::from_secs(2), false),
            Some(Change::Topology)
        );
        assert_eq!(pending.deadline(), None);
        pending.push(Change::References, start);
        assert_eq!(
            pending.take(start + Duration::from_millis(199), false),
            None
        );
        assert_eq!(
            pending.take(start + Duration::from_millis(200), false),
            Some(Change::References)
        );
    }
    #[test]
    fn only_relevant_metadata_mutations_trigger_reads() {
        let root = Path::new("/project/.git");
        for name in [
            "HEAD",
            "refs/heads/main",
            "packed-refs",
            "worktrees/branch/HEAD",
            "worktrees/branch/reftable/tables.list",
        ] {
            let event = Event::new(EventKind::Modify(ModifyKind::Data(DataChange::Any)))
                .add_path(root.join(name));
            assert!(classify(root, &event).is_some(), "{name}");
        }
        for name in [
            "index",
            "HEAD.lock",
            "objects/ab/cd",
            "logs/HEAD",
            "worktrees/branch/index",
            "worktrees/branch/logs/HEAD",
            "refs/heads/main.lock",
            "../source.rs",
        ] {
            let event = Event::new(EventKind::Create(notify::event::CreateKind::Any))
                .add_path(root.join(name));
            assert_eq!(classify(root, &event), None, "{name}");
        }
        for kind in [
            EventKind::Access(AccessKind::Any),
            EventKind::Modify(ModifyKind::Metadata(MetadataKind::WriteTime)),
        ] {
            assert_eq!(
                classify(
                    root,
                    &Event::new(kind).add_path(root.join("worktrees/branch/gitdir"))
                ),
                None
            );
        }
    }
    #[test]
    fn failures_require_checkpoint_and_old_mailboxes_are_discarded() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("missing");
        let mut monitor = Monitor::default();
        monitor.context(Some(root.clone()), 0, true);
        assert!(monitor.error.is_some());
        let old = monitor.pending.clone();
        std::fs::create_dir(&root).unwrap();
        assert!(!monitor.context(Some(root.clone()), 0, true));
        assert!(monitor.error.is_some());
        monitor.context(Some(root), 1, true);
        assert!(monitor.error.is_none());
        monitor.poll(Instant::now() + Duration::from_secs(2), false);
        old.lock().unwrap().push(Change::Topology, Instant::now());
        assert!(monitor.deadline().is_none());
        monitor.stop();
        assert!(monitor.repository().is_none());
        assert!(monitor.watcher.is_none());
    }
    fn git(root: &Path, args: &[&str]) -> String {
        let output = std::process::Command::new("git")
            .arg("-C")
            .arg(root)
            .args([
                "-c",
                "user.name=Watch test",
                "-c",
                "user.email=watch@example.invalid",
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
        String::from_utf8(output.stdout).unwrap().trim().to_owned()
    }
    fn notification(monitor: &mut Monitor) -> Change {
        let end = Instant::now() + Duration::from_secs(5);
        loop {
            assert!(monitor.error.is_none(), "{:?}", monitor.error);
            if let Some(change) = monitor.poll(Instant::now(), false) {
                monitor.reconcile();
                return change;
            }
            assert!(Instant::now() < end, "missing native notification");
            std::thread::sleep(Duration::from_millis(10));
        }
    }
    #[test]
    fn native_files_and_reftables_in_main_and_bare_repositories() {
        for bare in [false, true] {
            for format in ["files", "reftable"] {
                let tmp = tempfile::tempdir().unwrap();
                let root = tmp.path().join("repo");
                std::fs::create_dir(&root).unwrap();
                let mut args = vec!["init", "-q", "-b", "main"];
                let flag = format!("--ref-format={format}");
                if format == "reftable" {
                    args.push(&flag);
                }
                if bare {
                    args.push("--bare");
                }
                let init = std::process::Command::new("git")
                    .arg("-C")
                    .arg(&root)
                    .args(&args)
                    .env("GIT_CONFIG_GLOBAL", "/dev/null")
                    .env("GIT_CONFIG_NOSYSTEM", "1")
                    .output()
                    .unwrap();
                if !init.status.success() && format == "reftable" {
                    eprintln!(
                        "reftable fixture unavailable: {}",
                        String::from_utf8_lossy(&init.stderr)
                    );
                    continue;
                }
                assert!(
                    init.status.success(),
                    "{}",
                    String::from_utf8_lossy(&init.stderr)
                );
                // mktree receives EOF from Command::output and writes an empty tree.
                let tree = git(&root, &["mktree"]);
                let commit = git(&root, &["commit-tree", &tree, "-m", "Initial"]);
                git(&root, &["update-ref", "refs/heads/main", &commit]);
                let common = if bare {
                    root.clone()
                } else {
                    root.join(".git")
                };
                let mut monitor = Monitor::default();
                monitor.context(Some(common.clone()), 0, true);
                notification(&mut monitor);
                git(&root, &["update-ref", "refs/heads/new/nested", &commit]);
                notification(&mut monitor);
                if format == "files" {
                    git(&root, &["pack-refs", "--all"]);
                    notification(&mut monitor);
                }
                let linked = tmp.path().join("linked");
                git(
                    &root,
                    &["worktree", "add", "-qb", "linked", linked.to_str().unwrap()],
                );
                assert_eq!(notification(&mut monitor), Change::Topology);
                git(&linked, &["checkout", "--detach", "-q"]);
                notification(&mut monitor);
                git(&root, &["worktree", "remove", linked.to_str().unwrap()]);
                assert_eq!(notification(&mut monitor), Change::Topology);
                monitor.stop();
                assert!(monitor.stopped.is_none());
            }
        }
    }
    #[test]
    fn atomic_replacement_and_new_metadata_directories_remain_watched() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("git");
        std::fs::create_dir(&root).unwrap();
        let mut monitor = Monitor::default();
        monitor.context(Some(root.clone()), 0, true);
        notification(&mut monitor);
        std::fs::write(root.join("HEAD.lock"), "ref: refs/heads/main").unwrap();
        std::fs::rename(root.join("HEAD.lock"), root.join("HEAD")).unwrap();
        assert_eq!(notification(&mut monitor), Change::References);
        std::fs::create_dir_all(root.join("refs/heads")).unwrap();
        notification(&mut monitor);
        std::fs::write(root.join("refs/heads/new"), "object").unwrap();
        notification(&mut monitor);
        std::fs::rename(root.join("refs"), root.join("retired")).unwrap();
        notification(&mut monitor);
        std::fs::create_dir_all(root.join("refs/heads")).unwrap();
        notification(&mut monitor);
        std::fs::write(root.join("refs/heads/new"), "replacement").unwrap();
        notification(&mut monitor);
    }
    #[test]
    fn runtime_errors_and_overflow_are_bounded() {
        let tmp = tempfile::tempdir().unwrap();
        let mut monitor = Monitor::default();
        monitor.context(Some(tmp.path().to_owned()), 0, true);
        notification(&mut monitor);
        monitor.pending.lock().unwrap().error = Some("watch queue failed".into());
        assert_eq!(monitor.poll(Instant::now(), false), None);
        assert_eq!(monitor.error.as_deref(), Some("watch queue failed"));
        assert!(monitor.watcher.is_none());
        let mut event = Event::new(EventKind::Other);
        event.attrs.set_flag(notify::event::Flag::Rescan);
        assert_eq!(classify(tmp.path(), &event), Some(Change::Topology));
    }
    #[test]
    fn subscriptions_exclude_objects_logs_and_sources() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        for name in [
            "refs/heads",
            "objects",
            "logs",
            "worktrees/a/refs",
            "worktrees/a/logs",
            "worktrees/a/reftable",
        ] {
            std::fs::create_dir_all(root.join(name)).unwrap();
        }
        let paths = subscriptions(root).unwrap();
        assert!(!paths.contains_key(&root.join("objects")));
        assert!(!paths.contains_key(&root.join("logs")));
        assert!(!paths.contains_key(&root.join("worktrees/a/logs")));
        assert_eq!(paths[&root.join("refs")], RecursiveMode::Recursive);
        assert_eq!(
            paths[&root.join("worktrees/a")],
            RecursiveMode::NonRecursive
        );
    }
}
