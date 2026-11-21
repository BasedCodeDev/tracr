//! Lazy filesystem tree shared visually with the session folder tree.
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
    sync::mpsc,
    time::{Duration, Instant},
};

pub fn tree_label(path: &Path, depth: usize, expanded: bool) -> String {
    format!(
        "{}{} {}",
        "  ".repeat(depth.min(20)),
        if expanded { "▾" } else { "▸" },
        path.file_name()
            .unwrap_or(path.as_os_str())
            .to_string_lossy()
    )
}
#[derive(Debug)]
pub struct Row {
    pub path: PathBuf,
    pub depth: usize,
    pub expanded: bool,
    pub error: Option<String>,
}
struct Snapshot {
    expanded: BTreeSet<PathBuf>,
    rows: Vec<Row>,
    selected: PathBuf,
    valid: bool,
    error: Option<String>,
    initial: Option<String>,
}
pub struct WorktreeView {
    pub active: bool,
    pub entries: Vec<crate::worktrees::Worktree>,
    pub selected: Option<PathBuf>,
    pub loading: bool,
    pub error: Option<String>,
    pub list_state: std::cell::RefCell<ratatui::widgets::ListState>,
    pub(crate) page: std::cell::RefCell<crate::tui::ListPage>,
    origin: PathBuf,
    folder_draft: String,
    folder_caret: usize,
    folder_valid: bool,
    folder_loading: bool,
    folder_error: Option<String>,
    draft: Option<String>,
    caret: usize,
    derived: Option<PathBuf>,
    task: Option<tokio::task::JoinHandle<()>>,
    rx: Option<mpsc::Receiver<Result<Vec<crate::worktrees::Worktree>, String>>>,
}
impl Drop for WorktreeView {
    fn drop(&mut self) {
        if let Some(task) = self.task.take() {
            task.abort();
        }
    }
}
impl WorktreeView {
    fn refresh(&mut self) {
        if let Some(task) = self.task.take() {
            task.abort();
        }
        let (tx, rx) = mpsc::channel();
        self.rx = Some(rx);
        let origin = self.origin.clone();
        self.loading = true;
        self.error = None;
        self.task = Some(tokio::spawn(async move {
            let _ = tx.send(crate::worktrees::discover(&origin).await);
        }));
    }
    fn fill(&mut self, input: &mut String) -> bool {
        let Some(row) = self
            .entries
            .iter()
            .find(|r| Some(&r.path) == self.selected.as_ref())
        else {
            return false;
        };
        if let Some(error) = &row.unavailable {
            self.error = Some(error.clone());
            return false;
        }
        *input = row.path.to_str().expect("validated path").into();
        self.derived = Some(row.path.clone());
        self.error = None;
        true
    }
}
pub struct Picker {
    pub focus: usize, // path, tree, button
    pub worktrees: Option<WorktreeView>,
    pub list_state: std::cell::RefCell<ratatui::widgets::ListState>,
    pub(crate) page: std::cell::RefCell<crate::tui::ListPage>,
    pub caret: usize, // UTF-8 byte boundary
    pub rows: Vec<Row>,
    pub selected: Option<PathBuf>,
    pub valid: bool,
    pub error: Option<String>,
    pub loading: bool,
    launch: PathBuf,
    move_source: Option<PathBuf>,
    home: Option<PathBuf>,
    expanded: BTreeSet<PathBuf>,
    generation: u64,
    requested: Option<u64>,
    active: usize,
    edited: Instant,
    tx: mpsc::Sender<(u64, Snapshot)>,
    rx: mpsc::Receiver<(u64, Snapshot)>,
}
impl Picker {
    pub fn new(launch: PathBuf) -> Self {
        let (tx, rx) = mpsc::channel();
        Self {
            focus: 1,
            worktrees: None,
            list_state: Default::default(),
            page: Default::default(),
            caret: 0,
            rows: vec![],
            selected: None,
            valid: false,
            error: None,
            loading: true,
            launch,
            move_source: None,
            home: dirs::home_dir(),
            expanded: BTreeSet::new(),
            generation: 0,
            requested: None,
            active: 0,
            edited: Instant::now() - Duration::from_secs(1),
            tx,
            rx,
        }
    }
    pub fn for_relink(launch: PathBuf, origin: Option<PathBuf>) -> Self {
        let mut picker = Self::new(launch);
        picker.worktrees = Some(WorktreeView {
            active: false,
            entries: vec![],
            selected: None,
            loading: false,
            error: None,
            list_state: Default::default(),
            page: Default::default(),
            origin: origin.unwrap_or_default(),
            folder_draft: String::new(),
            folder_caret: 0,
            folder_valid: false,
            folder_loading: false,
            folder_error: None,
            draft: None,
            caret: 0,
            derived: None,
            task: None,
            rx: None,
        });
        picker
    }
    pub fn worktrees_active(&self) -> bool {
        self.worktrees.as_ref().is_some_and(|w| w.active)
    }
    pub fn worktree_validation(&self, input: &str) -> Option<(PathBuf, PathBuf)> {
        self.worktrees.as_ref().filter(|w| w.active).and_then(|w| {
            w.derived
                .as_ref()
                .filter(|p| p.to_str() == Some(input))
                .map(|p| (w.origin.clone(), p.clone()))
        })
    }
    pub fn for_move(launch: PathBuf, source: PathBuf, input: &str) -> Self {
        let mut picker = Self::new(launch);
        picker.move_source = Some(source);
        picker.caret = input.len();
        picker
    }
    pub fn is_folder_move(&self) -> bool {
        self.move_source.is_some()
    }
    fn changed(&mut self) {
        if let Some(w) = self.worktrees.as_mut().filter(|w| w.active) {
            w.derived = None;
        }
        self.generation += 1;
        self.valid = false;
        self.loading = true;
        self.error = None;
        self.edited = Instant::now();
    }
    pub fn paste(&mut self, input: &mut String, text: &str) {
        self.focus = 0;
        let text: String = text.chars().filter(|c| !c.is_control()).collect();
        self.caret = self.caret.min(input.len());
        input.insert_str(self.caret, &text);
        self.caret += text.len();
        self.changed();
    }
    /// Returns true only when the user explicitly requests destination preview.
    pub fn key(&mut self, input: &mut String, key: KeyEvent) -> bool {
        if let Some(w) = self
            .worktrees
            .as_mut()
            .filter(|_| self.focus == 1 && matches!(key.code, KeyCode::Left | KeyCode::Right))
        {
            let want_worktrees = key.code == KeyCode::Right;
            if w.active == want_worktrees {
                return false;
            }
            if w.active {
                w.draft = Some(input.clone());
                w.caret = self.caret;
                w.active = false;
                if let Some(task) = w.task.take() {
                    task.abort();
                }
                w.rx = None;
                *input = w.folder_draft.clone();
                self.caret = w.folder_caret;
            } else {
                w.folder_draft = input.clone();
                w.folder_caret = self.caret;
                w.folder_valid = self.valid;
                w.folder_loading = self.loading;
                w.folder_error = self.error.clone();
                w.active = true;
                if let Some(draft) = &w.draft {
                    *input = draft.clone();
                    self.caret = w.caret;
                }
                w.refresh();
            }
            self.generation += 1;
            if w.active {
                self.requested = None;
                self.error = None;
                self.loading = true;
            } else {
                self.requested = if w.folder_loading {
                    None
                } else {
                    Some(self.generation)
                };
                self.error = w.folder_error.clone();
                self.loading = w.folder_loading;
                self.valid = w.folder_valid;
            }
            return false;
        }
        let list_key = matches!(
            key.code,
            KeyCode::Up | KeyCode::Down | KeyCode::PageUp | KeyCode::PageDown
        ) || (self.focus == 1 && matches!(key.code, KeyCode::Home | KeyCode::End));
        if self.worktrees_active() && list_key {
            let w = self.worktrees.as_mut().unwrap();
            if !w.loading && !w.entries.is_empty() {
                let current = w
                    .entries
                    .iter()
                    .position(|r| Some(&r.path) == w.selected.as_ref());
                let index = if current.is_none() && key.code == KeyCode::Down {
                    0
                } else {
                    w.page
                        .borrow()
                        .select(key.code, current.unwrap_or(0), w.entries.len())
                };
                w.selected = Some(w.entries[index].path.clone());
                if w.fill(input) {
                    self.caret = input.len();
                }
            }
            return false;
        }
        if self.worktrees_active() && self.focus == 1 && key.code == KeyCode::Enter {
            let w = self.worktrees.as_mut().unwrap();
            if !w.loading && w.fill(input) {
                self.caret = input.len();
            }
            return false;
        }
        match key.code {
            KeyCode::Tab => {
                self.focus = match self.focus {
                    0 => 1,
                    1 => 3,
                    3 => 2,
                    _ => 0,
                }
            }
            KeyCode::BackTab => {
                self.focus = match self.focus {
                    0 => 2,
                    2 => 3,
                    3 => 1,
                    _ => 0,
                }
            }
            _ if list_key => {
                if !self.rows.is_empty() {
                    let index = self
                        .rows
                        .iter()
                        .position(|r| Some(&r.path) == self.selected.as_ref())
                        .unwrap_or(0);
                    let next = self.page.borrow().select(key.code, index, self.rows.len());
                    let path = self.rows[next].path.clone();
                    self.selected = Some(path.clone());
                    self.changed();
                    if let Some(text) = path.to_str() {
                        *input = text.to_owned();
                        if self.move_source.is_some() && !input.ends_with('/') {
                            input.push('/');
                        }
                    } else {
                        input.clear();
                        self.error = Some("This folder name cannot be entered as UTF-8.".into());
                        self.loading = false;
                        self.requested = Some(self.generation);
                    }
                    self.caret = input.len();
                }
            }
            KeyCode::Enter if self.focus == 1 => {
                if let Some(path) = self.selected.clone().filter(|p| p.to_str().is_some()) {
                    if !self.expanded.remove(&path) {
                        self.expanded.insert(path);
                    }
                    self.changed();
                }
            }
            KeyCode::Enter if self.focus == 0 => self.focus = 2,
            KeyCode::Enter | KeyCode::Char(' ') if self.focus == 2 => {
                return self.valid && !self.loading;
            }
            KeyCode::Left if self.focus == 0 => self.caret = previous(input, self.caret),
            KeyCode::Right if self.focus == 0 => self.caret = next(input, self.caret),
            KeyCode::Home if self.focus == 0 => self.caret = 0,
            KeyCode::End if self.focus == 0 => self.caret = input.len(),
            KeyCode::Backspace if self.focus == 0 => {
                let pos = previous(input, self.caret);
                input.replace_range(pos..self.caret, "");
                self.caret = pos;
                self.changed();
            }
            KeyCode::Delete if self.focus == 0 => {
                let end = next(input, self.caret);
                input.replace_range(self.caret..end, "");
                self.changed();
            }
            KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                input.clear();
                self.caret = 0;
                self.focus = 0;
                self.changed();
            }
            KeyCode::Char(c)
                if !key
                    .modifiers
                    .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
            {
                self.focus = 0;
                self.caret = self.caret.min(input.len());
                input.insert(self.caret, c);
                self.caret += c.len_utf8();
                self.changed();
            }
            _ => {}
        }
        false
    }
    pub fn needs_tick(&self) -> bool {
        if let Some(worktrees) = self.worktrees.as_ref().filter(|view| view.active) {
            return worktrees.loading;
        }
        self.active > 0
            || (self.requested != Some(self.generation)
                && self.edited.elapsed() >= Duration::from_millis(100))
    }
    pub fn tick(&mut self, input: &mut String) {
        if let Some(w) = self.worktrees.as_mut().filter(|w| w.active) {
            if let Some(result) = w.rx.as_ref().and_then(|rx| rx.try_recv().ok()) {
                w.loading = false;
                w.task = None;
                w.rx = None;
                match result {
                    Ok(entries) => {
                        w.entries = entries;
                        if w.selected.as_ref().is_some_and(|p| {
                            !w.entries
                                .iter()
                                .any(|r| &r.path == p && r.unavailable.is_none())
                        }) {
                            w.selected = None;
                            w.error = Some("Selected worktree is no longer available.".into());
                            if w.derived.take().is_some() {
                                input.clear();
                                self.caret = 0;
                                w.draft = None;
                            }
                        }
                    }
                    Err(error) => {
                        w.entries.clear();
                        w.error = Some(error);
                    }
                }
            }
            self.loading = w.loading;
            self.valid =
                !input.is_empty() && resolve(input, &self.launch, self.home.as_deref()).is_dir();
            self.error = w.error.clone();
            return;
        }
        while let Ok((generation, snapshot)) = self.rx.try_recv() {
            self.active = self.active.saturating_sub(1);
            if generation != self.generation {
                continue;
            }
            if let Some(initial) = snapshot.initial {
                *input = initial;
                self.caret = input.len();
            }
            self.expanded = snapshot.expanded;
            self.selected = Some(snapshot.selected);
            self.rows = snapshot.rows;
            self.valid = snapshot.valid;
            self.error = snapshot.error;
            self.loading = false;
        }
        if self.requested == Some(self.generation)
            || self.active >= 2
            || self.edited.elapsed() < Duration::from_millis(100)
        {
            return;
        }
        self.requested = Some(self.generation);
        self.active += 1;
        let (tx, generation, input, launch, home, expanded) = (
            self.tx.clone(),
            self.generation,
            input.clone(),
            self.launch.clone(),
            self.home.clone(),
            self.expanded.clone(),
        );
        let move_source = self.move_source.clone();
        // Detached bounded workers cannot stall the terminal or Tokio shutdown on a slow mount.
        std::thread::spawn(move || {
            let mut snapshot = scan(&input, &launch, home.as_deref(), expanded, generation == 0);
            if let Some(old) = move_source {
                snapshot.initial = None;
                let result = crate::folder_move::resolve_destination(&input, &old, &launch);
                snapshot.valid = result.as_ref().is_ok_and(|path| matches!(std::fs::symlink_metadata(path), Err(e) if e.kind() == std::io::ErrorKind::NotFound));
                snapshot.error = result.err();
                if !snapshot.valid && snapshot.error.is_none() {
                    snapshot.error =
                        Some("Destination already exists; choose a new folder path.".into());
                }
            }
            let _ = tx.send((generation, snapshot));
        });
    }
}
fn previous(text: &str, index: usize) -> usize {
    text[..index]
        .char_indices()
        .next_back()
        .map(|(i, _)| i)
        .unwrap_or(0)
}
fn next(text: &str, index: usize) -> usize {
    text[index..]
        .chars()
        .next()
        .map(|c| index + c.len_utf8())
        .unwrap_or(index)
}
fn resolve(input: &str, launch: &Path, home: Option<&Path>) -> PathBuf {
    let raw = if input == "~" {
        home.unwrap_or(launch).to_owned()
    } else if let Some(rest) = input.strip_prefix("~/") {
        home.unwrap_or(launch).join(rest)
    } else {
        launch.join(input)
    };
    let mut path = PathBuf::new();
    for part in raw.components() {
        match part {
            std::path::Component::ParentDir => {
                path.pop();
            }
            std::path::Component::CurDir => {}
            _ => path.push(part.as_os_str()),
        }
    }
    path
}
fn accessible(path: &Path) -> bool {
    std::fs::read_dir(path).is_ok()
}
fn scan(
    input: &str,
    launch: &Path,
    home: Option<&Path>,
    mut expanded: BTreeSet<PathBuf>,
    initial: bool,
) -> Snapshot {
    let path = resolve(input, launch, home);
    let valid = !input.is_empty() && accessible(&path);
    let selected = if !initial && path.is_dir() {
        path.clone()
    } else {
        path.ancestors()
            .find(|p| accessible(p))
            .map(Path::to_owned)
            .or_else(|| home.filter(|p| accessible(p)).map(Path::to_owned))
            .unwrap_or_else(|| PathBuf::from("/"))
    };
    expanded.extend(selected.ancestors().skip(1).map(Path::to_owned));
    if !valid || initial {
        expanded.insert(selected.clone());
    }
    let mut rows = vec![];
    visit(Path::new("/"), 0, &expanded, &mut vec![], &mut rows);
    Snapshot {
        expanded,
        rows,
        initial: initial.then(|| selected.to_string_lossy().into()),
        selected,
        valid: valid || initial,
        error: if valid || initial {
            None
        } else {
            Some("Choose an existing accessible directory.".into())
        },
    }
}
fn visit(
    path: &Path,
    depth: usize,
    expanded: &BTreeSet<PathBuf>,
    ancestors: &mut Vec<PathBuf>,
    rows: &mut Vec<Row>,
) {
    let canonical = std::fs::canonicalize(path).ok();
    let cycle = canonical.as_ref().is_some_and(|p| ancestors.contains(p));
    let open = expanded.contains(path) && !cycle;
    let index = rows.len();
    rows.push(Row {
        path: path.to_owned(),
        depth,
        expanded: open,
        error: cycle.then(|| "Symlink cycle".into()),
    });
    if !open {
        return;
    }
    let children = match std::fs::read_dir(path) {
        Ok(c) => c,
        Err(_) => {
            rows[index].error = Some("Cannot open this folder".into());
            return;
        }
    };
    let mut dirs = vec![];
    for entry in children {
        match entry {
            Ok(entry) if entry.path().is_dir() => dirs.push(entry.path()),
            Ok(_) => {}
            Err(_) => rows[index].error = Some("Some folders could not be read".into()),
        }
    }
    dirs.sort_by(|a, b| a.file_name().cmp(&b.file_name()));
    let pushed = canonical.is_some();
    if let Some(real) = canonical {
        ancestors.push(real);
    }
    for child in dirs {
        visit(&child, depth + 1, expanded, ancestors, rows);
    }
    if pushed {
        ancestors.pop();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn tree_reveals_typed_paths_keeps_partial_input_and_handles_cycles() {
        let root = tempfile::tempdir().unwrap();
        let project = root.path().join("project space");
        std::fs::create_dir(&project).unwrap();
        std::fs::create_dir(project.join(".hidden")).unwrap();
        std::fs::create_dir(project.join("empty")).unwrap();
        std::fs::write(project.join("file"), "not a directory").unwrap();
        std::os::unix::fs::symlink(&project, project.join("loop")).unwrap();
        let expanded = BTreeSet::from([project.clone(), project.join("loop")]);
        let exact = scan(
            project.to_str().unwrap(),
            root.path(),
            None,
            expanded,
            false,
        );
        assert!(exact.valid);
        assert_eq!(exact.selected, project);
        assert!(exact.rows.iter().any(|r| r.path == project.join(".hidden")));
        assert!(!exact.rows.iter().any(|r| r.path == project.join("file")));
        assert!(
            exact
                .rows
                .iter()
                .any(|r| r.path == project.join("loop") && r.error.is_some())
        );
        let partial = scan(
            "project space/par",
            root.path(),
            None,
            BTreeSet::new(),
            false,
        );
        assert!(!partial.valid);
        assert!(partial.initial.is_none());
        assert_eq!(partial.selected, project);
        let initial = scan(
            "project space/missing/child",
            root.path(),
            None,
            BTreeSet::new(),
            true,
        );
        assert_eq!(initial.initial.as_deref(), project.to_str());
        let empty = scan(
            "project space/empty",
            root.path(),
            None,
            BTreeSet::new(),
            false,
        );
        assert!(empty.valid);
    }
    #[test]
    fn paging_selects_paths_and_endpoints_respect_input_focus() {
        let mut picker = Picker::new("/".into());
        picker.rows = (0..6)
            .map(|index| Row {
                path: PathBuf::from(format!("/folder{index}")),
                depth: 0,
                expanded: false,
                error: None,
            })
            .collect();
        picker.page.borrow_mut().set(2, (0..6).collect());
        let mut input = String::new();
        let key = |code| KeyEvent::new(code, KeyModifiers::NONE);
        picker.key(&mut input, key(KeyCode::PageDown));
        assert_eq!(input, "/folder2");
        picker.key(&mut input, key(KeyCode::End));
        assert_eq!(input, "/folder5");
        picker.key(&mut input, key(KeyCode::PageUp));
        assert_eq!(input, "/folder3");
        picker.key(&mut input, key(KeyCode::Home));
        assert_eq!(input, "/folder0");
        picker.focus = 0;
        picker.key(&mut input, key(KeyCode::Home));
        assert_eq!(picker.caret, 0);
        picker.key(&mut input, key(KeyCode::End));
        assert_eq!(picker.caret, input.len());
        assert_eq!(input, "/folder0");
    }

    #[test]
    fn unicode_editing_arrows_and_confirmation_are_separate() {
        let root = tempfile::tempdir().unwrap();
        let mut picker = Picker::new(root.path().into());
        let mut input = String::new();
        picker.paste(&mut input, "é/路径");
        let key = |code| KeyEvent::new(code, KeyModifiers::NONE);
        picker.key(&mut input, key(KeyCode::Left));
        picker.key(&mut input, key(KeyCode::Backspace));
        assert_eq!(input, "é/径");
        picker.key(&mut input, key(KeyCode::Home));
        picker.key(&mut input, key(KeyCode::Delete));
        assert_eq!(input, "/径");
        picker.rows = vec![
            Row {
                path: root.path().into(),
                depth: 0,
                expanded: false,
                error: None,
            },
            Row {
                path: root.path().join("empty"),
                depth: 1,
                expanded: false,
                error: None,
            },
        ];
        picker.selected = Some(root.path().into());
        picker.key(&mut input, key(KeyCode::Down));
        assert_eq!(input, root.path().join("empty").to_string_lossy());
        picker.focus = 1;
        assert!(!picker.key(&mut input, key(KeyCode::Enter)));
        assert!(picker.expanded.contains(&root.path().join("empty")));
        picker.valid = true;
        picker.loading = false;
        picker.key(&mut input, key(KeyCode::Tab));
        assert_eq!(picker.focus, 3);
        picker.key(&mut input, key(KeyCode::Tab));
        assert!(picker.key(&mut input, key(KeyCode::Enter)));
    }
    #[test]
    fn initially_open_folder_collapses_on_first_enter() {
        let root = tempfile::tempdir().unwrap();
        let mut picker = Picker::new(root.path().into());
        let mut input = root.path().join("missing").to_string_lossy().into_owned();
        picker.requested = Some(0);
        picker
            .tx
            .send((0, scan(&input, root.path(), None, BTreeSet::new(), true)))
            .unwrap();
        picker.tick(&mut input);
        assert!(picker.expanded.contains(root.path()));
        picker.key(
            &mut input,
            KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
        );
        assert!(!picker.expanded.contains(root.path()));
    }
    #[test]
    fn non_utf8_selection_cannot_confirm_a_lossy_destination() {
        use std::os::unix::ffi::OsStringExt;
        let mut picker = Picker::new("/".into());
        let path = PathBuf::from(std::ffi::OsString::from_vec(b"/bad\xff".to_vec()));
        picker.rows.push(Row {
            path,
            depth: 0,
            expanded: false,
            error: None,
        });
        let mut input = String::new();
        picker.key(&mut input, KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
        assert!(input.is_empty());
        assert!(picker.error.is_some());
        assert!(!picker.valid);
        picker.focus = 0;
        assert!(!picker.key(
            &mut input,
            KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)
        ));
    }
    #[test]
    fn move_picker_preserves_exact_input_and_tree_selection_adds_slash() {
        let root = tempfile::tempdir().unwrap();
        let old = root.path().join("old");
        std::fs::create_dir(&old).unwrap();
        let mut input = root.path().join("new name").display().to_string();
        let mut picker = Picker::for_move(root.path().into(), old, &input);
        let expected = input.clone();
        let deadline = Instant::now() + Duration::from_secs(3);
        while picker.loading {
            picker.tick(&mut input);
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(input, expected);
        assert!(picker.valid);
        picker.key(&mut input, KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
        assert!(input.ends_with('/'));
        let selected = picker.selected.clone().unwrap();
        assert_eq!(Path::new(&input), selected);
        let expected = input.clone();
        let deadline = Instant::now() + Duration::from_secs(3);
        while picker.loading {
            picker.tick(&mut input);
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(input, expected);
    }
    #[test]
    fn stale_directory_results_cannot_replace_newer_input() {
        let root = tempfile::tempdir().unwrap();
        let mut picker = Picker::new(root.path().into());
        let mut input = "new unfinished path".to_owned();
        picker.generation = 2;
        picker.requested = Some(2);
        picker
            .tx
            .send((1, scan("/", root.path(), None, BTreeSet::new(), true)))
            .unwrap();
        picker.tick(&mut input);
        assert_eq!(input, "new unfinished path");
        assert!(picker.rows.is_empty());
    }
    #[test]
    fn worktree_picker_ticks_only_while_loading() {
        let mut picker = Picker::for_relink(PathBuf::from("/tmp"), Some(PathBuf::from("/tmp")));
        picker.edited = Instant::now() - Duration::from_secs(1);
        picker.requested = None;
        picker.active = 1; // A directory read may still be pending in the other view.
        let worktrees = picker.worktrees.as_mut().unwrap();
        worktrees.active = true;
        worktrees.loading = true;
        assert!(picker.needs_tick());
        picker.worktrees.as_mut().unwrap().loading = false;
        assert!(!picker.needs_tick());
        picker.worktrees.as_mut().unwrap().active = false;
        assert!(picker.needs_tick());
    }
}
