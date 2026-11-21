use crate::browser::{Browser, Discovery, FolderFilter, safe_text};
use crate::scrollbar;
use crossterm::{
    event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers},
    execute,
    terminal::{self, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{
    Frame, Terminal,
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, Clear, List, ListItem, ListState, Paragraph, Wrap},
};
use std::{
    io::{self, IsTerminal},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

pub struct RelinkModal {
    pub session: crate::domain::SessionSummary,
    pub input: String,
    pub picker: Option<crate::path_picker::Picker>,
    pub preview: Option<crate::relocation::Preview>,
    pub recovery: bool,
    pub busy: bool,
    pub executing: bool,
    pub error: Option<String>,
}
pub fn relink_error(
    session: &str,
    old: &str,
    target: &str,
    failure: &crate::relocation::Failure,
    needs_recovery: bool,
) -> crate::error_dialog::ErrorDialog {
    let title = if needs_recovery {
        "Relink needs recovery"
    } else {
        "Couldn't relink session"
    };
    let explanation = if failure.kind == crate::relocation::FailureKind::PermissionProfile {
        "This session uses a permission profile that tracr does not yet support for relinking. Permission profiles control what a session can access."
    } else {
        &failure.detail
    };
    let outcome = if needs_recovery {
        "The folder may already have changed. Close this dialog and press u to inspect/recover the operation."
    } else if failure.kind == crate::relocation::FailureKind::PermissionProfile {
        "Its folder was not changed. Your session remains available, but tracr cannot move it with its current permission settings."
    } else {
        "tracr did not change the session's folder during this attempt."
    };
    crate::error_dialog::ErrorDialog::new(
        title,
        "! Relink failed",
        format!(
            "{explanation}\n\n{outcome}\n\nSession: {session}\nOriginal folder: {old}\nRequested destination: {target}\n\nTechnical details: {}",
            failure.detail
        ),
    )
}
fn show_relink_failure(view: &mut View, failure: crate::relocation::Failure, needs_recovery: bool) {
    view.terminal_recovery = None;
    if !needs_recovery {
        if let Some(modal) = view
            .relink
            .as_mut()
            .filter(|modal| !modal.recovery && modal.preview.is_some())
        {
            let old = modal
                .session
                .cwd
                .as_ref()
                .map(|p| p.display().to_string())
                .unwrap_or_default();
            let dialog = if failure.kind == crate::relocation::FailureKind::StillLoaded {
                Some(crate::terminal_recovery::Dialog::still_loaded(
                    &modal.session.title,
                    &old,
                    &modal.input,
                ))
            } else {
                failure.terminals.clone().map(|blocker| {
                    crate::terminal_recovery::Dialog::new(
                        blocker,
                        &modal.session.title,
                        &old,
                        &modal.input,
                    )
                })
            };
            if let Some(dialog) = dialog {
                modal.busy = false;
                modal.executing = false;
                view.terminal_recovery = Some(dialog);
                return;
            }
        }
    }

    if let Some(modal) = view.relink.take() {
        view.error_dialog = Some(relink_error(
            &modal.session.title,
            &modal
                .session
                .cwd
                .map(|p| p.display().to_string())
                .unwrap_or_default(),
            &modal.input,
            &failure,
            needs_recovery,
        ));
    }
}
fn show_folder_failure(view: &mut View, message: String, recovery: bool) {
    if let Some(modal) = view.folder_move.take() {
        let target = modal
            .plan
            .as_ref()
            .map(|p| p.target.clone())
            .unwrap_or_else(|| {
                modal
                    .target()
                    .unwrap_or_else(|_| std::path::PathBuf::from(&modal.input))
            });
        let count = modal
            .plan
            .as_ref()
            .map(|p| p.entries.len().to_string())
            .unwrap_or_else(|| "Not yet determined".into());
        let title = if recovery {
            "Move needs recovery"
        } else {
            "Folder was not moved"
        };
        let outcome = if recovery {
            "Files may already be at the destination. Close this dialog and press u to inspect and finish the move."
        } else {
            "tracr did not move the folder during this attempt."
        };
        view.error_dialog = Some(crate::error_dialog::ErrorDialog::new(
            title,
            "! Folder move failed",
            format!(
                "{outcome}\n\nOriginal folder: {}\nRequested destination: {}\nAffected sessions: {count}\n\nTechnical details:\n{message}",
                modal.old.display(),
                target.display()
            ),
        ));
    }
}
pub struct WorktreePicker {
    repository: std::path::PathBuf,
    oid: String,
    matches: Vec<crate::git::Worktree>,
    selected: usize,
    page_size: std::cell::Cell<usize>,
}

fn matching_worktrees(
    browser: &Browser,
    repository: &std::path::Path,
    oid: &str,
) -> Result<Vec<crate::git::Worktree>, String> {
    if browser.repository_errors.contains_key(repository) {
        return Err("Worktree information is stale or refreshing. Try again after refresh.".into());
    }
    let repo = browser
        .repositories
        .get(repository)
        .ok_or("Worktree information is unavailable. Refresh and try again.")?;
    let mut matches: Vec<_> = repo
        .worktrees
        .iter()
        .filter(|w| !w.bare && w.head.as_deref() == Some(oid))
        .cloned()
        .collect();
    matches.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(matches)
}

fn select_git_worktree(browser: &mut Browser, view: &mut View, picker: &WorktreePicker) -> Action {
    let Some(target) = picker.matches.get(picker.selected) else {
        return Action::Continue;
    };
    match matching_worktrees(browser, &picker.repository, &picker.oid) {
        Ok(matches) if matches.iter().any(|w| w.path == target.path) => {
            // Linked worktrees are displayed beneath the main checkout, even when
            // their filesystem paths live elsewhere.
            if let Some(main) = browser
                .repositories
                .get(&picker.repository)
                .and_then(|repo| repo.worktrees.first())
            {
                browser
                    .expanded
                    .extend(main.path.ancestors().map(std::path::PathBuf::from));
            }
            browser.open_folder(target.path.clone());
            view.git_focus = false;
            view.focus = 0;
            view.folders_hidden = false;
            view.message = None;
            return enter_folder(browser, view);
        }
        Ok(_) => {
            view.message = Some("Worktree changed or was removed. Select the commit again.".into())
        }
        Err(message) => view.message = Some(message),
    }
    Action::Continue
}

#[derive(Default)]
pub struct FolderNavigation {
    deadline: Option<std::time::Instant>,
    key: Option<KeyCode>,
}
impl FolderNavigation {
    fn observe(&mut self, key: KeyEvent, folder_focused: bool, now: std::time::Instant) {
        let navigation = key.modifiers == KeyModifiers::NONE
            && matches!(
                key.code,
                KeyCode::Up | KeyCode::Down | KeyCode::Char('j' | 'k')
            );
        if folder_focused && navigation && key.kind != KeyEventKind::Release {
            self.key = Some(key.code);
            self.deadline = Some(now + Duration::from_millis(75));
        } else if self.deadline.is_some()
            && (key.kind != KeyEventKind::Release || self.key == Some(key.code))
        {
            self.deadline = Some(now);
        }
    }

    fn pending(&self) -> bool {
        self.deadline.is_some()
    }

    fn ready(&self, now: std::time::Instant, input_waiting: bool) -> bool {
        self.deadline.is_some_and(|deadline| now >= deadline) && !input_waiting
    }

    fn finish(&mut self) {
        *self = Self::default();
    }
}

/// Dialogs, drafts, and feedback preserved for the inactive application area.
#[derive(Default)]
pub struct AreaDialogs {
    pub terminal_recovery: Option<crate::terminal_recovery::Dialog>,
    pub editing: bool,
    pub search_query: String,
    pub search_selected: Option<crate::domain::SessionKey>,
    pub help: bool,
    pub help_selected: usize,
    pub help_entries: Option<Vec<MenuEntry>>,
    pub help_control: usize,
    pub help_query: String,
    pub help_searching: bool,
    pub search_control: usize,
    pub source_control: usize,
    pub relink_back: bool,
    pub relink_scroll: u16,
    pub relink_scroll_limit: std::cell::Cell<u16>,
    pub info: Option<InfoModal>,
    pub commit_info: Option<crate::commit_info::Dialog>,
    pub message: Option<String>,
    pub preparing: bool,
    pub source_picker: Option<usize>,
    pub creation_folder: Option<std::path::PathBuf>,
    pub worktree_picker: Option<WorktreePicker>,
    pub relink: Option<RelinkModal>,
    pub folder_move: Option<crate::folder_move_ui::Modal>,
    pub worktree_create: Option<crate::worktree_create_ui::Modal>,
    pub error_dialog: Option<crate::error_dialog::ErrorDialog>,
    terminal_overlay: crate::inline_terminal::OverlayState,
}

#[derive(Clone, Copy, Default)]
pub struct DrawerReminder {
    learned: bool,
    started: bool,
    deadline: Option<std::time::Instant>,
}
impl DrawerReminder {
    fn expire(&mut self, now: std::time::Instant) {
        if self.deadline.is_some_and(|deadline| now >= deadline) {
            self.deadline = None;
        }
    }
    fn visible(&mut self, now: std::time::Instant, eligible: bool) -> bool {
        if self.learned || !eligible {
            return false;
        }
        if !self.started {
            self.started = true;
            self.deadline = Some(now + Duration::from_secs(2));
        }
        self.deadline.is_some_and(|deadline| now < deadline)
    }
    fn learn(&mut self) {
        self.learned = true;
        self.deadline = None;
    }
}

/// Geometry of a list with variable-height entries from its last draw.
#[derive(Default)]
pub struct ListPage {
    height: usize,
    starts: Vec<usize>,
}

impl ListPage {
    fn clear(&mut self) {
        self.height = 0;
        self.starts.clear();
    }

    pub(crate) fn set(&mut self, height: usize, starts: Vec<usize>) {
        *self = Self { height, starts };
    }

    pub(crate) fn select(&self, code: KeyCode, index: usize, len: usize) -> usize {
        if self.starts.len() != len || len == 0 || self.height == 0 {
            return list_index(code, index, len, 1);
        }
        let index = index.min(len - 1);
        match code {
            KeyCode::PageUp => {
                let target = self.starts[index].saturating_sub(self.height);
                self.starts
                    .partition_point(|&start| start <= target)
                    .saturating_sub(1)
                    .min(index.saturating_sub(1))
            }
            KeyCode::PageDown => {
                let target = self.starts[index].saturating_add(self.height);
                self.starts
                    .partition_point(|&start| start < target)
                    .min(len - 1)
            }
            _ => list_index(code, index, len, 1),
        }
    }

    pub(crate) fn set_items(&mut self, height: usize, items: &[ListItem<'_>]) {
        let mut start = 0;
        self.set(
            height,
            items
                .iter()
                .map(|item| {
                    let offset = start;
                    start += item.height();
                    offset
                })
                .collect(),
        );
    }
}

pub(crate) fn list_index(code: KeyCode, index: usize, len: usize, page: usize) -> usize {
    let last = len.saturating_sub(1);
    let index = index.min(last);
    match code {
        KeyCode::Home => 0,
        KeyCode::End => last,
        KeyCode::Up => index.saturating_sub(1),
        KeyCode::Down => index.saturating_add(1).min(last),
        KeyCode::PageUp => index.saturating_sub(page.max(1)),
        KeyCode::PageDown => index.saturating_add(page.max(1)).min(last),
        _ => index,
    }
}

pub struct View {
    pub folder_navigation: FolderNavigation,
    /// Scoped to the displayed frame and its next input; cleared before updates.
    pub folder_rows_snapshot: Option<std::rc::Rc<Vec<crate::browser::FolderRow>>>,
    pub drawer_reminder: std::cell::Cell<DrawerReminder>,
    // Keyboard focus / explicit opening; wide session browsing also pins visibility.
    pub drawer_open: bool,
    pub drawer_git: bool,
    /// Area whose dialog fields are currently loaded: false = main, true = drawer.
    /// Background completion polling can temporarily select the hidden area.
    pub dialog_area: bool,
    pub hidden_dialogs: AreaDialogs,
    pub layout_mode: std::cell::Cell<LayoutMode>,
    pub terminals: std::cell::RefCell<crate::inline_terminal::Manager>,
    pub session_preview: std::cell::RefCell<crate::session_preview::Controller>,
    pub terminal_recovery: Option<crate::terminal_recovery::Dialog>,
    pub history: crate::git_history::History,
    pub git_sessions: std::cell::RefCell<crate::git_sessions::Presentation>,
    pub git_preview: std::cell::RefCell<Option<ratatui::buffer::Buffer>>,
    pub git_navigation_loading: bool,
    pub git_navigation_error: bool,
    pub git_hidden: bool,
    pub git_zoomed: bool,
    pub git_details: Option<crate::commit_info::Dialog>,
    pub git_details_key: Option<(std::path::PathBuf, String)>,
    pub git_details_focused: bool,
    pub folders_hidden: bool,
    pub git_size_limited: bool,
    pub git_watch_error: Option<String>,
    pub git_focus: bool,
    pub focus: usize,
    pub editing: bool,
    pub search_query: String,
    pub search_selected: Option<crate::domain::SessionKey>,
    pub help: bool,
    pub help_selected: usize,
    pub help_entries: Option<Vec<MenuEntry>>,
    pub help_control: usize,
    pub help_query: String,
    pub help_searching: bool,
    pub search_control: usize,
    pub source_control: usize,
    pub relink_back: bool,
    pub relink_scroll: u16,
    pub relink_scroll_limit: std::cell::Cell<u16>,
    pub folder_page_size: std::cell::Cell<usize>,
    pub session_page_size: std::cell::Cell<usize>,
    pub source_page_size: std::cell::Cell<usize>,
    pub help_page: std::cell::RefCell<ListPage>,
    pub search_page: std::cell::RefCell<ListPage>,
    pub info: Option<InfoModal>,
    pub commit_info: Option<crate::commit_info::Dialog>,
    pub message: Option<String>,
    pub preparing: bool,
    pub source_picker: Option<usize>,
    pub creation_folder: Option<std::path::PathBuf>,
    pub worktree_picker: Option<WorktreePicker>,
    pub relink: Option<RelinkModal>,
    pub folder_move: Option<crate::folder_move_ui::Modal>,
    pub worktree_create: Option<crate::worktree_create_ui::Modal>,
    #[doc(hidden)]
    pub discard: Option<crate::worktree_discard_ui::Modal>,
    #[doc(hidden)]
    pub discard_recovery_available: bool,
    pub recovery_index: usize,
    pub recovery_available: bool,
    pub error_dialog: Option<crate::error_dialog::ErrorDialog>,
}
impl Default for View {
    fn default() -> Self {
        Self {
            folder_navigation: Default::default(),
            folder_rows_snapshot: Default::default(),
            drawer_reminder: Default::default(),
            drawer_open: Default::default(),
            drawer_git: Default::default(),
            dialog_area: Default::default(),
            hidden_dialogs: Default::default(),
            layout_mode: Default::default(),
            terminals: Default::default(),
            session_preview: Default::default(),
            terminal_recovery: Default::default(),
            history: Default::default(),
            git_sessions: Default::default(),
            git_preview: Default::default(),
            git_navigation_loading: false,
            git_navigation_error: false,
            git_hidden: Default::default(),
            git_zoomed: false,
            git_details: None,
            git_details_key: None,
            git_details_focused: false,
            folders_hidden: Default::default(),
            git_size_limited: Default::default(),
            git_watch_error: Default::default(),
            git_focus: Default::default(),
            focus: 1,
            editing: Default::default(),
            search_query: Default::default(),
            search_selected: Default::default(),
            help: Default::default(),
            help_selected: Default::default(),
            help_entries: Default::default(),
            help_control: Default::default(),
            help_query: Default::default(),
            help_searching: Default::default(),
            search_control: Default::default(),
            source_control: Default::default(),
            relink_back: Default::default(),
            relink_scroll: Default::default(),
            relink_scroll_limit: Default::default(),
            folder_page_size: Default::default(),
            session_page_size: Default::default(),
            source_page_size: Default::default(),
            help_page: Default::default(),
            search_page: Default::default(),
            info: Default::default(),
            commit_info: Default::default(),
            message: Default::default(),
            preparing: Default::default(),
            source_picker: Default::default(),
            creation_folder: Default::default(),
            worktree_picker: Default::default(),
            relink: Default::default(),
            folder_move: Default::default(),
            worktree_create: Default::default(),
            discard: None,
            discard_recovery_available: false,
            recovery_index: Default::default(),
            recovery_available: Default::default(),
            error_dialog: Default::default(),
        }
    }
}

impl View {
    fn folder_rows(&self, browser: &Browser) -> std::rc::Rc<Vec<crate::browser::FolderRow>> {
        self.folder_rows_snapshot
            .clone()
            .unwrap_or_else(|| std::rc::Rc::new(browser.folder_rows()))
    }
    fn coalesce_navigation(&self) -> bool {
        !self.modal_open()
            && !self.terminals.borrow().has_overlay()
            && !(self.terminal_focused() && self.terminals.borrow().active.is_some())
    }

    // Swapping dialog ownership never changes the visible drawer or PTY selection.
    fn select_dialog_area(&mut self, drawer: bool) {
        if self.dialog_area == drawer {
            return;
        }
        std::mem::swap(
            &mut self.terminal_recovery,
            &mut self.hidden_dialogs.terminal_recovery,
        );
        std::mem::swap(&mut self.editing, &mut self.hidden_dialogs.editing);
        std::mem::swap(
            &mut self.search_query,
            &mut self.hidden_dialogs.search_query,
        );
        std::mem::swap(
            &mut self.search_selected,
            &mut self.hidden_dialogs.search_selected,
        );
        std::mem::swap(&mut self.help, &mut self.hidden_dialogs.help);
        std::mem::swap(&mut self.help_query, &mut self.hidden_dialogs.help_query);
        std::mem::swap(
            &mut self.help_searching,
            &mut self.hidden_dialogs.help_searching,
        );
        std::mem::swap(
            &mut self.help_selected,
            &mut self.hidden_dialogs.help_selected,
        );
        std::mem::swap(
            &mut self.help_entries,
            &mut self.hidden_dialogs.help_entries,
        );
        std::mem::swap(
            &mut self.help_control,
            &mut self.hidden_dialogs.help_control,
        );
        std::mem::swap(
            &mut self.search_control,
            &mut self.hidden_dialogs.search_control,
        );
        std::mem::swap(
            &mut self.source_control,
            &mut self.hidden_dialogs.source_control,
        );
        std::mem::swap(&mut self.relink_back, &mut self.hidden_dialogs.relink_back);
        std::mem::swap(
            &mut self.relink_scroll,
            &mut self.hidden_dialogs.relink_scroll,
        );
        std::mem::swap(
            &mut self.relink_scroll_limit,
            &mut self.hidden_dialogs.relink_scroll_limit,
        );
        std::mem::swap(&mut self.info, &mut self.hidden_dialogs.info);
        std::mem::swap(&mut self.commit_info, &mut self.hidden_dialogs.commit_info);
        std::mem::swap(&mut self.message, &mut self.hidden_dialogs.message);
        std::mem::swap(&mut self.preparing, &mut self.hidden_dialogs.preparing);
        std::mem::swap(
            &mut self.source_picker,
            &mut self.hidden_dialogs.source_picker,
        );
        std::mem::swap(
            &mut self.creation_folder,
            &mut self.hidden_dialogs.creation_folder,
        );
        std::mem::swap(
            &mut self.worktree_picker,
            &mut self.hidden_dialogs.worktree_picker,
        );
        std::mem::swap(&mut self.relink, &mut self.hidden_dialogs.relink);
        std::mem::swap(&mut self.folder_move, &mut self.hidden_dialogs.folder_move);
        std::mem::swap(
            &mut self.worktree_create,
            &mut self.hidden_dialogs.worktree_create,
        );
        std::mem::swap(
            &mut self.error_dialog,
            &mut self.hidden_dialogs.error_dialog,
        );
        self.terminals
            .borrow_mut()
            .swap_overlay(&mut self.hidden_dialogs.terminal_overlay);
        self.dialog_area = drawer;
    }

    fn normalize_focus(&mut self) {
        if !self.history.present() {
            self.drawer_git = false;
        }
        self.git_focus = self.drawer_open && self.drawer_git;
        if !self.git_focus {
            self.git_zoomed = false;
        }
        if !self.git_zoomed {
            self.git_details = None;
            self.git_details_key = None;
            self.git_details_focused = false;
        }
        self.focus = if self.drawer_open { 0 } else { 1 };
        self.folders_hidden = false;
        self.git_hidden = false;
    }

    fn browser_drawer_pinned(&self) -> bool {
        self.layout_mode.get() == LayoutMode::Columns && self.terminals.borrow().active.is_none()
    }

    fn drawer_visible(&self) -> bool {
        self.drawer_open || self.browser_drawer_pinned()
    }

    fn toggle_drawer(&mut self) {
        self.drawer_open = !self.drawer_open;
        self.select_dialog_area(self.drawer_open);
        self.normalize_focus();
    }

    fn modal_open(&self) -> bool {
        self.terminal_recovery.is_some()
            || self.error_dialog.is_some()
            || self.worktree_create.is_some()
            || self.folder_move.is_some()
            || self.discard.is_some()
            || self.relink.is_some()
            || self.info.is_some()
            || self.commit_info.is_some()
            || self.source_picker.is_some()
            || self.worktree_picker.is_some()
            || self.help
            || self.editing
            || self.preparing
    }

    fn terminal_focused(&self) -> bool {
        !self.drawer_open && self.context() == 1 && !self.modal_open()
    }

    fn context(&self) -> usize {
        if self.git_focus { 2 } else { self.focus }
    }
}

fn refresh_recovery(state: Option<&std::path::Path>, view: &mut View) -> Option<(usize, bool)> {
    view.recovery_available = false;
    let state = state?;
    let result = (|| {
        let relocations = crate::relocation::pending(state)
            .map_err(|e| format!("Cannot inspect relocation journals: {e}"))?;
        let folder = crate::folder_move::pending(state)
            .map_err(|e| format!("Cannot inspect folder-move journal: {e}"))?;
        Ok::<_, String>((relocations.len(), folder.is_some()))
    })();
    match result {
        Ok((count, folder)) => {
            view.recovery_available = count > 0 || folder;
            Some((count, folder))
        }
        Err(error) => {
            view.message = Some(error);
            None
        }
    }
}

fn selection_style(focused: bool) -> Style {
    Style::default().add_modifier(if focused {
        Modifier::BOLD
    } else {
        Modifier::DIM
    })
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum LayoutMode {
    Columns,
    Stacked,
    #[default]
    Single,
}

impl LayoutMode {
    fn for_size(width: u16, height: u16) -> Self {
        if width >= 120 && height >= 12 {
            Self::Columns
        } else {
            Self::Single
        }
    }
}

#[derive(Debug)]
struct PanelLayout {
    mode: LayoutMode,
    folders: Option<Rect>,
    sessions: Option<Rect>,
    git: Option<Rect>,
    details: Option<Rect>,
    graph_visible: bool,
}

// Only confirmed ownership may reserve Git panel space. Discovery stays off the
// render path, and effective ownership preserves the panel during refreshes.
fn selected_git_context(browser: &Browser) -> Option<&crate::git_discovery::Outcome> {
    let FolderFilter::Path(path) = &browser.folder else {
        return None;
    };
    browser
        .ownership
        .get(path)
        .and_then(|record| record.effective())
}

fn selected_git_repository(browser: &Browser) -> Option<&std::path::PathBuf> {
    match selected_git_context(browser) {
        Some(crate::git_discovery::Outcome::Repository { repository, .. }) => {
            Some(&repository.common_dir)
        }
        _ => None,
    }
}

fn git_preview_pending(browser: &Browser, view: &View) -> bool {
    view.folder_navigation.pending()
        || view.git_navigation_loading
        || (selected_git_repository(browser).is_some()
            && view.history.repository().as_ref() != selected_git_repository(browser))
}

fn git_preview_blocked(browser: &Browser, view: &View) -> bool {
    git_preview_pending(browser, view) || view.git_navigation_error
}

fn finish_git_navigation(view: &mut View) {
    if view.git_navigation_loading && view.history.navigation_ready() {
        view.git_navigation_loading = false;
        view.git_navigation_error = view.history.error.is_some();
    } else if view.git_navigation_error
        && view.history.error.is_none()
        && view.history.navigation_ready()
    {
        view.git_navigation_error = false;
    }
}

fn cache_git_preview(frame: &mut Frame<'_>, area: Rect, view: &View) {
    let inner = pane_block("Git Worktrees", false).inner(area);
    let mut snapshot = ratatui::buffer::Buffer::empty(Rect::new(0, 0, inner.width, inner.height));
    for y in 0..inner.height {
        for x in 0..inner.width {
            snapshot[(x, y)] = frame.buffer_mut()[(inner.x + x, inner.y + y)].clone();
        }
    }
    *view.git_preview.borrow_mut() = Some(snapshot);
}

fn draw_git_preview(frame: &mut Frame<'_>, area: Rect, view: &View) {
    let block = pane_block("Git Worktrees", view.git_focus && !view.git_details_focused);
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if let Some(snapshot) = view.git_preview.borrow().as_ref() {
        for y in 0..inner.height.min(snapshot.area.height) {
            for x in 0..inner.width.min(snapshot.area.width) {
                let cell = &snapshot[(x, y)];
                // Never copy half a wide glyph at the new right edge.
                if x as usize + unicode_width::UnicodeWidthStr::width(cell.symbol())
                    > inner.width as usize
                {
                    continue;
                }
                let target = &mut frame.buffer_mut()[(inner.x + x, inner.y + y)];
                *target = cell.clone();
            }
        }
    } else {
        frame.render_widget(Paragraph::new("Loading Git preview…"), inner);
    }
}

#[cfg(test)]
pub(crate) fn select_git_fixture(browser: &mut Browser, repository: &str) {
    let path = std::path::PathBuf::from(repository);
    browser.folder = FolderFilter::Path(path.clone());
    browser.apply_ownership(
        browser.git_generation,
        path.clone(),
        crate::git_discovery::Outcome::Repository {
            repository: crate::git::Directory {
                common_dir: path.clone(),
                git_dir: path,
            },
            root: None,
        },
    );
}

// Shared by drawing, input navigation, Git scheduling, and terminal sizing.
// Total dimensions choose the mode so footer messages cannot change it.
fn panel_layout(area: Rect, terminal_height: u16, view: &View, present: bool) -> PanelLayout {
    let mode = LayoutMode::for_size(area.width, terminal_height);
    view.layout_mode.set(mode);
    let mut result = PanelLayout {
        mode,
        folders: None,
        sessions: None,
        git: None,
        details: None,
        graph_visible: false,
    };
    if view.git_zoomed && view.drawer_open && view.drawer_git && present {
        let halves = Layout::horizontal([Constraint::Ratio(1, 2); 2]).split(area);
        result.git = Some(halves[0]);
        result.details = Some(halves[1]);
        result.graph_visible = halves[0].height >= 4 && halves[0].width >= 12;
        return result;
    }
    if !view.drawer_visible() {
        result.sessions = Some(area);
        return result;
    }
    let drawer = if mode == LayoutMode::Columns {
        let width = (area.width * 2 / 5).min(60);
        result.sessions = Some(Rect::new(
            area.x + width,
            area.y,
            area.width - width,
            area.height,
        ));
        Rect::new(area.x, area.y, width, area.height)
    } else {
        area
    };
    if !present {
        result.folders = Some(drawer);
    } else if drawer.height < 12 {
        if view.drawer_git {
            result.git = Some(drawer);
        } else {
            result.folders = Some(drawer);
        }
    } else {
        let regions = Layout::vertical([Constraint::Ratio(1, 3), Constraint::Min(0)]).split(drawer);
        result.folders = Some(regions[0]);
        result.git = Some(regions[1]);
    }
    result.graph_visible = result.git.is_some_and(|r| r.height >= 4 && r.width >= 12);
    result
}

fn draw_rail(frame: &mut Frame<'_>, area: Rect, mode: LayoutMode, title: &str) {
    let mut rail = Block::default()
        .borders(if mode == LayoutMode::Columns {
            Borders::LEFT
        } else {
            Borders::TOP
        })
        .style(Style::default().fg(Color::DarkGray));
    if mode != LayoutMode::Columns {
        rail = rail.title(title);
    }
    frame.render_widget(rail, area);
}

static ENHANCED_INPUT: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
fn reset_input() {
    if ENHANCED_INPUT.swap(false, std::sync::atomic::Ordering::Relaxed) {
        let _ = execute!(io::stdout(), crossterm::style::Print("\x1b[<u"));
    }
}
fn restore() {
    reset_input();
    let _ = terminal::disable_raw_mode();
    let _ = execute!(
        io::stdout(),
        event::DisableMouseCapture,
        event::DisableBracketedPaste,
        event::DisableFocusChange,
        LeaveAlternateScreen,
        crossterm::cursor::Show
    );
}
// A direct flag is observed by the UI loop even if the async signal driver stalls.
struct ShutdownSignals {
    terminate: std::sync::Arc<std::sync::atomic::AtomicBool>,
    interrupt: std::sync::Arc<std::sync::atomic::AtomicBool>,
    resize: std::sync::Arc<std::sync::atomic::AtomicBool>,
    registrations: Vec<signal_hook::SigId>,
}
impl ShutdownSignals {
    fn new() -> io::Result<Self> {
        let mut guard = Self {
            terminate: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
            interrupt: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
            resize: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
            registrations: Vec::new(),
        };
        for (signal, flag) in [
            (signal_hook::consts::SIGTERM, guard.terminate.clone()),
            (signal_hook::consts::SIGINT, guard.interrupt.clone()),
            (signal_hook::consts::SIGWINCH, guard.resize.clone()),
        ] {
            guard
                .registrations
                .push(signal_hook::flag::register(signal, flag)?);
        }
        Ok(guard)
    }
}
impl Drop for ShutdownSignals {
    fn drop(&mut self) {
        for registration in self.registrations.drain(..) {
            signal_hook::low_level::unregister(registration);
        }
    }
}

struct TerminalGuard {
    active: bool,
    mouse: bool,
}
impl TerminalGuard {
    fn enhanced(&mut self, supported: bool) -> io::Result<()> {
        if supported && !ENHANCED_INPUT.swap(true, std::sync::atomic::Ordering::Relaxed) {
            execute!(io::stdout(), crossterm::style::Print("\x1b[>31u"))?;
        }
        Ok(())
    }
    fn mouse_capture(&mut self, enabled: bool) -> io::Result<()> {
        if enabled != self.mouse {
            if enabled {
                execute!(io::stdout(), event::EnableMouseCapture)?;
            } else {
                execute!(io::stdout(), event::DisableMouseCapture)?;
            }
            self.mouse = enabled;
        }
        Ok(())
    }
    fn activate(&mut self) -> io::Result<()> {
        terminal::enable_raw_mode()?;
        self.active = true;
        execute!(
            io::stdout(),
            EnterAlternateScreen,
            event::EnableBracketedPaste,
            event::EnableFocusChange,
            crossterm::style::Print("\x1b[?u\x1b[c")
        )
    }
    fn suspend(&mut self) -> io::Result<()> {
        reset_input();
        terminal::disable_raw_mode()?;
        execute!(
            io::stdout(),
            event::DisableMouseCapture,
            event::DisableBracketedPaste,
            event::DisableFocusChange,
            LeaveAlternateScreen,
            crossterm::cursor::Show
        )?;
        self.active = false;
        self.mouse = false;
        Ok(())
    }
}
impl Drop for TerminalGuard {
    fn drop(&mut self) {
        if self.active {
            restore();
        }
    }
}
fn block(title: &str, focused: bool) -> Block<'_> {
    Block::default()
        .borders(Borders::ALL)
        .title(title)
        .border_type(if focused {
            BorderType::Thick
        } else {
            BorderType::Plain
        })
        .border_style(Style::default().fg(if focused {
            Color::Cyan
        } else {
            Color::DarkGray
        }))
}

/// Keep the counter inside the corners and reserve its columns before truncating the title.
fn git_history_block(
    area: Rect,
    focused: bool,
    count: Option<usize>,
    more_above: bool,
) -> Block<'static> {
    let counter = match (count, more_above) {
        (Some(count), true) => Some(format!(" {count} ↑ ")),
        (Some(count), false) => Some(format!(" {count} ")),
        (None, true) => Some(" ↑ ".into()),
        (None, false) => None,
    };
    let available = usize::from(area.width.saturating_sub(2));
    let counter =
        counter.filter(|text| unicode_width::UnicodeWidthStr::width(text.as_str()) <= available);
    let title_width = available.saturating_sub(counter.as_ref().map_or(0, |text| {
        unicode_width::UnicodeWidthStr::width(text.as_str())
    }));
    let title = &"Git Worktrees"[..title_width.min("Git Worktrees".len())];
    let mut block = pane_block(title, focused);
    if let Some(counter) = counter {
        block = block.title(
            Line::from(counter).right_aligned().style(
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ),
        );
    }
    block
}

fn pane_block(title: &str, focused: bool) -> Block<'_> {
    block(title, focused).padding(ratatui::widgets::Padding::new(0, 0, 1, 0))
}

#[cfg(test)]
mod git_counter_tests {
    use super::*;
    use ratatui::{buffer::Buffer, widgets::Widget};

    #[test]
    fn narrow_title_reserves_counter_and_corners() {
        let area = Rect::new(0, 0, 12, 5);
        for focused in [false, true] {
            for (count, above, expected) in [
                (Some(12345), true, " 12345 ↑ "),
                (Some(1), true, " 1 ↑ "),
                (Some(0), false, " 0 "),
                (None, true, " ↑ "),
            ] {
                let mut buffer = Buffer::empty(area);
                git_history_block(area, focused, count, above).render(area, &mut buffer);
                let start = area.right() - 1 - expected.chars().count() as u16;
                let text: String = (start..area.right() - 1)
                    .map(|x| buffer[(x, 0)].symbol())
                    .collect();
                assert_eq!(text, expected);
                assert_eq!(buffer[(0, 0)].symbol(), if focused { "┏" } else { "┌" });
                assert_eq!(buffer[(11, 0)].symbol(), if focused { "┓" } else { "┐" });
            }
        }
    }
}

#[derive(Default)]
struct IdleGate {
    dirty: bool,
    deadline: Option<std::time::Instant>,
}
impl IdleGate {
    fn wake(&mut self) {
        self.dirty = true;
    }
    fn ready(&mut self, now: std::time::Instant, activity: bool) -> bool {
        let ready = self.dirty || activity || self.deadline.is_some_and(|deadline| now >= deadline);
        if ready {
            self.dirty = false;
            self.deadline = None;
        }
        ready
    }
}
fn age_delay(updated: i64, now: i64) -> u64 {
    let elapsed = now.saturating_sub(updated).max(0);
    if elapsed < 60 {
        return updated
            .saturating_add(60)
            .saturating_sub(now)
            .clamp(1, 86_400) as u64;
    }
    let units = [31_536_000, 2_592_000, 604_800, 86_400, 3_600, 60];
    let unit = units.into_iter().find(|unit| elapsed >= *unit).unwrap();
    let next_bucket = units
        .into_iter()
        .filter(|unit| *unit > elapsed)
        .map(|unit| unit - elapsed)
        .min()
        .unwrap_or(i64::MAX);
    (unit - elapsed % unit).min(next_bucket) as u64
}
fn next_age_update(browser: &Browser, view: &View, height: u16) -> Option<std::time::Instant> {
    if view.editing
        || view.help
        || view.error_dialog.is_some()
        || view.relink.is_some()
        || view.folder_move.is_some()
    {
        return None;
    }
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
        .min(i64::MAX as u64) as i64;
    let sessions = browser.visible();
    let selected = sessions
        .iter()
        .position(|session| Some(&session.key) == browser.selected.as_ref())
        .unwrap_or(0);
    let count = usize::from(height / 3).max(1);
    let start = selected.saturating_sub(count.saturating_sub(1));
    sessions
        .iter()
        .skip(start)
        .take(count)
        .map(|session| age_delay(session.updated_at, now))
        .min()
        .map(|seconds| std::time::Instant::now() + Duration::from_secs(seconds))
}

fn relative_time(updated_at: i64, now: i64) -> String {
    let elapsed = now.saturating_sub(updated_at).max(0);
    if elapsed < 60 {
        return "just now".into();
    }
    for (seconds, unit) in [
        (31_536_000, "year"),
        (2_592_000, "month"),
        (604_800, "week"),
        (86_400, "day"),
        (3_600, "hour"),
        (60, "minute"),
    ] {
        if elapsed >= seconds {
            let count = elapsed / seconds;
            return format!("{count} {unit}{} ago", if count == 1 { "" } else { "s" });
        }
    }
    unreachable!("elapsed is at least one minute")
}

// Hints occupy existing chrome, never a content row. Git leaves its scroll marker free.
fn draw_action_hint(frame: &mut Frame<'_>, area: Rect, terminal: bool, git: bool) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    let reserve = if git && area.width > 5 { 4 } else { 0 };
    let available = area.width.saturating_sub(2 + reserve);
    let full = if terminal {
        " Ctrl+B Browser "
    } else {
        " ? Actions "
    };
    let short = if terminal { "Ctrl+B" } else { "?" };
    let hint = if available >= full.len() as u16 {
        full
    } else {
        short
    };
    let width = (hint.len() as u16).min(area.width.saturating_sub(reserve));
    let x = if available >= hint.len() as u16 {
        area.right() - 1 - reserve - width
    } else {
        area.x
    };
    frame.render_widget(
        Paragraph::new(hint).style(Style::default().fg(Color::Cyan)),
        Rect::new(x, area.bottom() - 1, width, 1),
    );
}

fn is_unlinked(browser: &Browser, session: &crate::domain::SessionSummary) -> bool {
    session.cwd.as_ref().is_some_and(|path| {
        browser.folders.get(path) == Some(&crate::domain::FolderStatus::Unlinked)
    })
}
fn session_metadata(
    browser: &Browser,
    session: &crate::domain::SessionSummary,
    now: i64,
) -> Line<'static> {
    let mut spans = Vec::new();
    if browser
        .details
        .get(&session.key)
        .is_some_and(|detail| matches!(detail, Ok(Some(detail)) if detail.cwd != session.cwd))
    {
        spans.push(Span::styled(
            "Folder changed · i Info · ",
            Style::default().fg(Color::Rgb(255, 165, 0)),
        ));
    }
    if is_unlinked(browser, session) {
        spans.push(Span::styled(
            "Unlinked",
            Style::default()
                .fg(Color::Rgb(255, 165, 0))
                .add_modifier(Modifier::BOLD),
        ));
        spans.push(Span::raw(" · "));
    }
    spans.push(Span::styled(
        format!(
            "{} · {}",
            safe_text(&session.key.provider.0),
            relative_time(session.updated_at, now)
        ),
        Style::default().add_modifier(Modifier::DIM),
    ));
    Line::from(spans)
}

fn browser_footer(_area: Rect, browser: &Browser, view: &View) -> (String, u16) {
    let mut issues: Vec<_> = browser
        .diagnostics()
        .into_iter()
        .filter(|s| !s.complete)
        .map(|s| {
            format!(
                "{}: {}",
                safe_text(&s.source),
                safe_text(&s.error.unwrap_or_else(|| if s.warnings.is_empty() {
                    "refreshing".into()
                } else {
                    format!(
                        "{} ({} warnings; inventory incomplete)",
                        s.warnings[0],
                        s.warnings.len()
                    )
                }))
            )
        })
        .collect();
    if view.history.present()
        && !view.git_hidden
        && !(git_preview_pending(browser, view) && view.git_preview.borrow().is_some())
    {
        if let Some(notice) = view.history.notice() {
            issues.insert(0, safe_text(&notice));
        }
    }
    if let Some(error) = &view.git_watch_error {
        issues.insert(
            0,
            format!(
                "Git auto-update unavailable — r to refresh: {}",
                safe_text(error)
            ),
        );
    }
    for (source, spec) in browser.sources.iter().zip(&browser.specs) {
        if source.live_error.is_some() {
            issues.push(format!(
                "{}: live status unavailable",
                safe_text(&spec.label())
            ));
        }
    }
    let message = if view.preparing {
        "Preparing resume… Esc cancels".into()
    } else {
        view.message
            .as_ref()
            .map(|m| safe_text(m))
            .unwrap_or_else(|| issues.join(" | "))
    };
    let message =
        if view.git_watch_error.is_some() && !message.contains("Git auto-update unavailable") {
            format!("{} | {message}", issues[0])
        } else {
            message
        };
    let height = u16::from(!message.is_empty());
    (message, height)
}

fn selected_git_session(browser: &Browser, view: &View) -> Option<crate::git_sessions::Annotation> {
    if git_preview_blocked(browser, view) {
        return None;
    }
    let mut presentation = view.git_sessions.borrow_mut();
    presentation.sync(browser, &view.history);
    presentation.session().cloned()
}

fn preview_session(browser: &Browser, view: &View) -> Option<crate::domain::SessionSummary> {
    (view.drawer_open && view.git_focus && !view.git_hidden && !view.git_size_limited)
        .then(|| selected_git_session(browser, view))
        .flatten()
        .map(|a| a.session)
}

fn sync_terminal_view(browser: &Browser, view: &View) {
    let preview = preview_session(browser, view);
    let mut terminals = view.terminals.borrow_mut();
    if let Some(session) = preview {
        // A missing match must reveal the preview list/transcript, not an unrelated terminal.
        terminals.sync_preview(None, Some(&session.key));
    } else {
        terminals.sync_folder(match &browser.folder {
            FolderFilter::Path(path) => Some(path.as_path()),
            _ => None,
        });
    }
}

fn tick_session_preview(browser: &Browser, view: &View) -> bool {
    let target = preview_session(browser, view)
        .and_then(|session| (!view.terminals.borrow().owns(&session.key)).then_some(session.key));
    view.session_preview.borrow_mut().tick(target, browser)
}

fn git_session_status(status: crate::domain::RuntimeStatus) -> &'static str {
    use crate::domain::RuntimeStatus;
    match status {
        RuntimeStatus::Idle => "Idle",
        RuntimeStatus::Running => "Running",
        RuntimeStatus::Error => "Error",
        RuntimeStatus::Waiting {
            approval: true,
            input: true,
        } => "Waiting",
        RuntimeStatus::Waiting { approval: true, .. } => "Approval",
        RuntimeStatus::Waiting { input: true, .. } => "Input",
        _ => "",
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct GitSessionWidths {
    status: usize,
    title: usize,
}

fn git_session_widths(presentation: &crate::git_sessions::Presentation) -> GitSessionWidths {
    use unicode_width::UnicodeWidthStr;
    let mut widths = GitSessionWidths::default();
    for row in &presentation.rows {
        if let crate::git_sessions::Row::Session { annotation, .. } = row {
            let annotation = &presentation.annotations[*annotation];
            widths.status = widths
                .status
                .max(git_session_status(annotation.status).width());
            widths.title = widths
                .title
                .max(safe_text(&annotation.session.title).width());
        }
    }
    widths
}

fn git_session_spans(
    view: &View,
    anchor: usize,
    connector: Option<usize>,
    annotation: &crate::git_sessions::Annotation,
    last: bool,
    grouped: bool,
    widths: GitSessionWidths,
) -> Vec<Span<'static>> {
    let mut spans = view.history.session_spans(anchor, connector);
    let status = git_session_status(annotation.status);
    let connector = if last { " └─ " } else { " ├─ " };
    let connector_style = Style::default().fg(Color::White);
    if grouped {
        spans.push(Span::styled(connector.trim_start(), connector_style));
    }
    spans.push(Span::raw(
        " ".repeat(
            widths
                .status
                .saturating_sub(unicode_width::UnicodeWidthStr::width(status)),
        ),
    ));
    spans.push(Span::styled(
        status.to_owned(),
        activity_color(annotation.status)
            .map_or(Style::default(), |color| Style::default().fg(color)),
    ));
    if grouped {
        spans.push(Span::raw(" "));
    } else {
        spans.push(Span::styled(connector, connector_style));
    }
    let title = safe_text(&annotation.session.title);
    let padding = widths
        .title
        .saturating_sub(unicode_width::UnicodeWidthStr::width(title.as_str()));
    spans.push(Span::styled(
        title,
        Style::default().add_modifier(Modifier::BOLD),
    ));
    spans.push(Span::raw(" ".repeat(padding)));
    spans.push(Span::raw(format!(
        " · {}",
        safe_text(&annotation.session.key.provider.0)
    )));
    if let Some(label) = annotation.label.as_ref().filter(|_| !grouped) {
        spans.push(Span::styled(
            format!(" · {}", safe_text(label)),
            Style::default().fg(Color::DarkGray),
        ));
    }
    spans
}

pub fn draw(frame: &mut Frame<'_>, browser: &Browser, view: &View) {
    let mut reminder = view.drawer_reminder.get();
    reminder.expire(std::time::Instant::now());
    view.drawer_reminder.set(reminder);
    view.terminals.borrow_mut().visible = false;
    view.session_preview.borrow_mut().terminals.visible = false;
    if let Some(dialog) = &view.terminal_recovery {
        dialog.draw(frame);
        return;
    }

    if let Some(dialog) = &view.error_dialog {
        dialog.draw(frame);
        return;
    }
    if let Some(modal) = &view.discard {
        modal.draw(frame);
        return;
    }
    if let Some(modal) = &view.worktree_create {
        modal.draw(frame);
        return;
    }
    if let Some(modal) = &view.folder_move {
        modal.draw(frame, browser);
        return;
    }
    if let Some(info) = &view.commit_info {
        info.draw(frame);
        return;
    }
    if let Some(info) = &view.info {
        info.dialog.draw(frame);
        return;
    }
    let area = frame.area();
    view.layout_mode
        .set(LayoutMode::for_size(area.width, area.height));
    if !view.drawer_open && view.terminals.borrow().active.is_some() && !view.modal_open() {
        let label =
            view.terminals.borrow().active_folder().and_then(|folder| {
                crate::inline_terminal::WorktreeLabel::for_folder(browser, &folder)
            });
        view.terminals
            .borrow_mut()
            .draw_with_worktree(frame, area, true, false, label.as_ref());
        let mut reminder = view.drawer_reminder.get();
        let visible = reminder.visible(
            std::time::Instant::now(),
            area.width >= 9 && area.height >= 3 && !view.terminals.borrow().has_overlay(),
        );
        view.drawer_reminder.set(reminder);
        if visible {
            frame.render_widget(
                Paragraph::new("Alt+~")
                    .style(Style::default().fg(Color::DarkGray).bg(Color::Black)),
                Rect::new(area.x + 4, area.y + 2, 5, 1),
            );
        }
        return;
    }
    if area.width < 12 || area.height < 4 {
        frame.render_widget(Paragraph::new("tracr: enlarge terminal"), area);
        return;
    }
    let (message, footer_height) = browser_footer(area, browser, view);
    let layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(1), Constraint::Length(footer_height)])
        .split(area);
    let panels = panel_layout(
        layout[0],
        area.height,
        view,
        selected_git_repository(browser).is_some(),
    );
    if let Some(area) = panels.details {
        let block = pane_block("Commit information", view.git_details_focused);
        let inner = block.inner(area);
        frame.render_widget(block, area);
        if let Some(info) = &view.git_details {
            info.draw_panel(frame, inner);
        } else {
            frame.render_widget(
                Paragraph::new("Select a commit to view details").wrap(Wrap { trim: false }),
                inner,
            );
        }
    }
    let git_area = panels.git;
    let expanded = panels.graph_visible;
    if let Some(git_area) = git_area {
        if expanded && git_preview_pending(browser, view) {
            draw_git_preview(frame, git_area, view);
        } else if expanded && view.git_navigation_error {
            frame.render_widget(
                Paragraph::new(
                    view.history
                        .notice()
                        .unwrap_or_else(|| "Loading Git history…".into()),
                )
                .block(pane_block(
                    "Git Worktrees",
                    view.git_focus && !view.git_details_focused,
                )),
                git_area,
            );
        } else if expanded {
            use crate::git_sessions::Row;
            let inner = pane_block("Git Worktrees", view.git_focus && !view.git_details_focused)
                .inner(git_area);
            let height = usize::from(inner.height);
            view.history.page.set(height.max(1));
            let mut presentation = view.git_sessions.borrow_mut();
            let mut width = usize::from(git_area.width.saturating_sub(4));
            presentation.width = Some(width);
            presentation.sync(browser, &view.history);
            let mut range = presentation.range(height);
            let scrolling = presentation.rows.len() > range.len();
            if scrolling && width > 0 {
                width -= 1;
                presentation.width = Some(width);
                presentation.sync(browser, &view.history);
                range = presentation.range(height);
            }
            let commits_above = presentation.commits_above(range.start);
            let selected = presentation.cursor().saturating_sub(range.start);
            // Use all loaded rows, so scrolling does not shift the columns.
            let session_widths = git_session_widths(&presentation);
            // Commit subjects, session annotations, and labels in very narrow panes can pan.
            let requested = view.history.horizontal.get();
            view.history.clamp_horizontal(width);
            let annotation_max = presentation
                .rows
                .iter()
                .filter_map(|row| match row {
                    Row::References { line, .. } => Some(
                        view.history
                            .reference_line_width(line)
                            .saturating_sub(width),
                    ),
                    Row::Session {
                        anchor,
                        annotation,
                        connector,
                        heading,
                        last,
                    } => Some(
                        git_session_spans(
                            view,
                            *anchor,
                            *connector,
                            &presentation.annotations[*annotation],
                            *last,
                            heading.is_some(),
                            session_widths,
                        )
                        .iter()
                        .map(|s| unicode_width::UnicodeWidthStr::width(s.content.as_ref()))
                        .sum::<usize>()
                        .saturating_sub(width),
                    ),
                    _ => None,
                })
                .max()
                .unwrap_or(0);
            view.history.horizontal.set(
                view.history
                    .horizontal
                    .get()
                    .max(requested.min(annotation_max)),
            );
            let items: Vec<_> = presentation.rows[range.clone()]
                .iter()
                .map(|row| {
                    let spans = match row {
                        Row::Graph(i) => view.history.row_spans(&view.history.rows[*i]),
                        Row::References { anchor, line } => {
                            let mut spans = view.history.continuation_spans(*anchor);
                            spans.extend(line.spans.iter().cloned());
                            spans
                        }
                        Row::Session {
                            anchor,
                            annotation,
                            connector,
                            heading,
                            last,
                        } => git_session_spans(
                            view,
                            *anchor,
                            *connector,
                            &presentation.annotations[*annotation],
                            *last,
                            heading.is_some(),
                            session_widths,
                        ),
                    };
                    let item =
                        ListItem::new(graph_viewport(spans, view.history.horizontal.get(), width));
                    let background = match row {
                        Row::Graph(i) => view.history.row_background(&view.history.rows[*i]),
                        _ => None,
                    };
                    if let Some(background) = background {
                        item.style(Style::default().bg(background))
                    } else {
                        item
                    }
                })
                .collect();
            frame.render_stateful_widget(
                List::new(items)
                    .block(
                        git_history_block(
                            git_area,
                            view.git_focus && !view.git_details_focused,
                            commits_above,
                            range.start > 0,
                        )
                        .padding(ratatui::widgets::Padding::new(
                            0,
                            u16::from(scrolling),
                            1,
                            0,
                        )),
                    )
                    .highlight_symbol("> ")
                    .highlight_style(selection_style(
                        view.context() == 2 && !view.git_details_focused,
                    )),
                git_area,
                &mut ListState::default().with_selected(Some(selected)),
            );
            if !view.history.loading() && view.history.error.is_none() {
                cache_git_preview(frame, git_area, view);
            }
            scrollbar::draw(
                frame,
                scrollbar::inside_border_track(git_area, inner),
                presentation.rows.len(),
                range.len(),
                range.start,
            );
            if range.end < presentation.rows.len()
                || (!view.history.exhausted && !presentation.rows.is_empty())
            {
                frame.render_widget(
                    Paragraph::new(" ↓ ").style(
                        Style::default()
                            .fg(Color::Cyan)
                            .add_modifier(Modifier::BOLD),
                    ),
                    Rect::new(git_area.right() - 4, git_area.bottom() - 1, 3, 1),
                );
            }
        } else {
            draw_rail(frame, git_area, panels.mode, " Git Worktrees ");
        }
    }
    if let Some(area) = panels.folders {
        if view.folders_hidden {
            let title = folder_title(browser, area.width.saturating_sub(2));
            draw_rail(frame, area, panels.mode, &format!(" {title} "));
        } else {
            draw_pane(frame, area, 0, browser, view);
        }
    }
    if let Some(area) = panels.sessions {
        draw_pane(frame, area, 1, browser, view);
    }
    let focused_area = match view.context() {
        0 => panels.folders,
        2 if view.git_details_focused => panels.details,
        2 => panels.git,
        _ => panels.sessions,
    };
    if let Some(hint_area) = focused_area {
        draw_action_hint(
            frame,
            hint_area,
            view.terminal_focused() && view.terminals.borrow().active.is_some(),
            view.context() == 2 && expanded,
        );
    }
    if view.git_zoomed && !view.git_details_focused {
        if let Some(area) = panels
            .git
            .filter(|area| area.width >= 24 && area.height > 0)
        {
            frame.render_widget(
                Paragraph::new(" i Details ").style(Style::default().fg(Color::Cyan)),
                Rect::new(area.x + 1, area.bottom() - 1, 11, 1),
            );
        }
    }
    if footer_height > 0 {
        frame.render_widget(Paragraph::new(message), layout[1]);
    }
    if let Some(picker) = &view.worktree_picker {
        let popup = Rect::new(
            area.x + 1,
            area.y + 1,
            area.width.saturating_sub(2),
            area.height.saturating_sub(2).min(16),
        );
        picker
            .page_size
            .set(usize::from(popup.height.saturating_sub(2)) / 2);
        let items: Vec<_> = picker
            .matches
            .iter()
            .map(|w| {
                let branch = w
                    .branch
                    .as_deref()
                    .map(|b| b.trim_start_matches("refs/heads/"))
                    .unwrap_or("detached");
                ListItem::new(vec![
                    Line::from(safe_text(branch)),
                    Line::from(safe_text(&w.path.display().to_string())),
                ])
            })
            .collect();
        frame.render_widget(Clear, popup);
        let lines = scrollbar::ListLines::new(&items);
        let mut state = ListState::default().with_selected(Some(picker.selected));
        frame.render_stateful_widget(
            List::new(items)
                .block(
                    block("Choose worktree — ↑/↓ · Enter selects · Esc cancels", true)
                        .padding(ratatui::widgets::Padding::right(1)),
                )
                .highlight_symbol("> ")
                .highlight_style(Style::default().add_modifier(Modifier::BOLD)),
            popup,
            &mut state,
        );
        let body = block("", true).inner(popup);
        scrollbar::draw(
            frame,
            scrollbar::inside_border_track(popup, body),
            lines.total(),
            usize::from(body.height),
            lines.top(state.offset()),
        );
    }
    if view.help {
        draw_help(frame, browser, view);
    }
    if let Some(selected) = view.source_picker {
        let inner = crate::modal::shell(frame, "Choose harness", 76, 18);
        let regions = crate::modal::regions(inner);
        modal_actions(
            frame,
            regions.actions,
            "Cancel",
            "Create session",
            view.source_control,
            !creation_sources(browser).is_empty(),
            true,
        );
        frame.render_widget(
            Paragraph::new("↑/↓ select · Enter creates · Esc cancels"),
            regions.hints,
        );
        view.source_page_size.set(usize::from(regions.body.height));
        let items: Vec<_> = creation_sources(browser)
            .iter()
            .map(|&i| ListItem::new(safe_text(&browser.specs[i].label())))
            .collect();
        let total = items.len();
        let body = scrollbar::content_area(regions.body, total);
        let mut state = ListState::default().with_selected(Some(selected));
        frame.render_stateful_widget(
            List::new(items)
                .highlight_symbol("> ")
                .highlight_style(selection_style(view.source_control == 0)),
            body,
            &mut state,
        );
        if body.width < regions.body.width {
            scrollbar::draw(
                frame,
                scrollbar::border_track(regions.body, body),
                total,
                usize::from(body.height),
                state.offset(),
            );
        }
    }
    if view.editing {
        draw_search(frame, browser, view);
    }
    if let Some(modal) = &view.relink {
        if modal.preview.is_none() && modal.picker.is_some() {
            draw_path_picker(frame, modal);
            return;
        }
        let mut text = format!(
            "{}\nSource: {}\nFrom: {}\nArchived: {}\n\nDestination: {}\n\n",
            safe_text(&modal.session.title),
            safe_text(&browser.specs.iter().find(|spec|matches!(spec,crate::browser::SourceSpec::Codex{source,..} if source.id==modal.session.key.source_id)).map(|spec|spec.label()).unwrap_or_else(||modal.session.key.source_id.0.clone())),
            safe_text(
                &modal
                    .session
                    .cwd
                    .as_ref()
                    .map(|p| p.display().to_string())
                    .unwrap_or_default()
            ),
            modal.session.archived,
            safe_text(&modal.input)
        );
        if let Some(preview) = &modal.preview {
            text.push_str(&format!(
                "Permissions: {}\nOther session settings are preserved.\nOnly this session changes folders. Project files stay where they are.\nDetached idle sessions are released automatically after confirmation.\nIdle describes agent activity; background commands may still block moving.\n\n",
                preview.profile.label()
            ));
        }
        let controls = if modal.busy {
            modal
                .error
                .as_deref()
                .unwrap_or("Checking native state… Please wait.")
        } else if modal.preview.is_some() {
            "Enter confirms • Esc cancels"
        } else {
            "Type an existing folder • Enter previews • Esc cancels"
        };
        if let Some(error) = modal.error.as_ref().filter(|_| !modal.busy) {
            text.push_str(&format!("\n\n{}", safe_text(error)));
        }
        let inner = crate::modal::shell(
            frame,
            if modal.recovery {
                "Inspect/recover relocation"
            } else {
                "Move session"
            },
            84,
            28,
        );
        let regions = crate::modal::regions(inner);
        view.relink_scroll_limit.set(crate::modal::body(
            frame,
            regions.body,
            text,
            view.relink_scroll,
        ));
        frame.render_widget(
            Paragraph::new(controls).wrap(Wrap { trim: false }),
            regions.status,
        );
        modal_actions(
            frame,
            regions.actions,
            "Cancel",
            if modal.recovery {
                "Continue recovery"
            } else {
                "Move session"
            },
            if view.relink_back { 1 } else { 2 },
            !modal.busy,
            !modal.executing,
        );
    }
}

fn modal_actions(
    frame: &mut Frame<'_>,
    area: Rect,
    secondary: &'static str,
    primary: &'static str,
    focus: usize,
    enabled: bool,
    cancellable: bool,
) {
    use crate::modal::Button;
    crate::modal::buttons(
        frame,
        area,
        &[
            Button {
                action: 0,
                label: secondary,
                enabled: cancellable,
                primary: false,
            },
            Button {
                action: 1,
                label: primary,
                enabled,
                primary: true,
            },
        ],
        focus.checked_sub(1),
    );
}

fn draw_path_picker(frame: &mut Frame<'_>, modal: &RelinkModal) {
    render_picker(
        frame,
        modal.picker.as_ref().expect("path picker"),
        &modal.input,
        PickerText {
            title: "Move session",
            heading: &modal.session.title,
            old: &modal
                .session
                .cwd
                .as_ref()
                .map(|p| p.display().to_string())
                .unwrap_or_default(),
            field: "New folder",
            busy: modal.busy,
            error: modal.error.as_deref(),
            sidebar: None,
        },
    );
}
pub(crate) struct PickerText<'a> {
    pub title: &'a str,
    pub heading: &'a str,
    pub old: &'a str,
    pub field: &'a str,
    pub busy: bool,
    pub error: Option<&'a str>,
    pub sidebar: Option<(&'a [Line<'static>], u16, &'a std::cell::Cell<u16>)>,
}
pub(crate) fn render_picker(
    frame: &mut Frame<'_>,
    picker: &crate::path_picker::Picker,
    input: &str,
    text: PickerText<'_>,
) {
    let inner = crate::modal::shell(
        frame,
        text.title,
        if text.sidebar.is_some() { 120 } else { 84 },
        30,
    );
    if inner.height < 10 {
        picker.page.borrow_mut().clear();
        if let Some(worktrees) = &picker.worktrees {
            worktrees.page.borrow_mut().clear();
        }
        frame.render_widget(
            Paragraph::new("Enlarge terminal to choose a folder. Esc cancels."),
            inner,
        );
        return;
    }
    let heading = if picker.is_folder_move() {
        format!(
            "Previous folder: {}\n{}",
            safe_text(text.old),
            safe_text(text.heading)
        )
    } else {
        format!(
            "{}\nPrevious folder: {}",
            safe_text(text.heading),
            safe_text(text.old)
        )
    };
    let heading = Paragraph::new(heading).wrap(Wrap { trim: false });
    let heading_height = heading
        .line_count(inner.width)
        .min(usize::from(inner.height.saturating_sub(8)))
        .max(2) as u16;
    let parts = Layout::vertical([
        Constraint::Length(heading_height),
        Constraint::Length(3),
        Constraint::Min(1),
        Constraint::Length(2),
        Constraint::Length(1),
        Constraint::Length(1),
    ])
    .split(inner);
    frame.render_widget(heading, parts[0]);
    crate::modal::text_field(
        frame,
        parts[1],
        text.field,
        input,
        picker.caret,
        picker.focus == 0,
        !text.busy,
    );
    let worktrees = picker.worktrees.as_ref().filter(|w| w.active);
    let tree_area = if let Some((lines, scroll, limit)) = text.sidebar {
        let regions = if parts[2].width >= 90 {
            Layout::horizontal([Constraint::Percentage(55), Constraint::Percentage(45)])
                .split(parts[2])
        } else {
            Layout::vertical([Constraint::Percentage(55), Constraint::Percentage(45)])
                .split(parts[2])
        };
        let panel = block("Sessions · PgUp/PgDn", false);
        let body = panel.inner(regions[1]);
        frame.render_widget(panel, regions[1]);
        let sections = Layout::vertical([Constraint::Min(0), Constraint::Length(2)]).split(body);
        let content = lines
            .get(..lines.len().saturating_sub(2))
            .unwrap_or_default();
        let paragraph = Paragraph::new(content.to_vec()).wrap(Wrap { trim: false });
        limit.set(scrollbar::paragraph(frame, sections[0], paragraph, scroll));
        if let Some(message) = lines.last() {
            frame.render_widget(
                Paragraph::new(message.clone()).wrap(Wrap { trim: false }),
                sections[1],
            );
        }
        regions[0]
    } else {
        parts[2]
    };
    let (items, selected): (Vec<ListItem>, Option<usize>) = if let Some(w) = worktrees {
        let items = w
            .entries
            .iter()
            .map(|row| {
                let name = row
                    .path
                    .file_name()
                    .unwrap_or(row.path.as_os_str())
                    .to_string_lossy();
                let label = format!(
                    "{}{} · {}",
                    name,
                    if row.main { " (main)" } else { "" },
                    row.branch.as_deref().unwrap_or("Detached HEAD")
                );
                let width = usize::from(tree_area.width.saturating_sub(5));
                let mut lines: Vec<_> = crate::info_content::wrap(&label, width)
                    .into_iter()
                    .map(Line::from)
                    .collect();
                lines.extend(
                    crate::info_content::wrap(&row.path.display().to_string(), width)
                        .into_iter()
                        .map(Line::from),
                );
                if let Some(error) = &row.unavailable {
                    lines.extend(
                        crate::info_content::wrap(error, width)
                            .into_iter()
                            .map(Line::from),
                    );
                }
                ListItem::new(lines).style(if row.unavailable.is_some() {
                    Style::default().fg(Color::DarkGray)
                } else {
                    Style::default()
                })
            })
            .collect();
        (
            items,
            w.entries
                .iter()
                .position(|r| Some(&r.path) == w.selected.as_ref()),
        )
    } else {
        let items = picker
            .rows
            .iter()
            .map(|row| {
                let mut label = crate::path_picker::tree_label(&row.path, row.depth, row.expanded);
                if let Some(error) = &row.error {
                    label.push_str(&format!(" [{error}]"));
                }
                ListItem::new(safe_text(&label))
            })
            .collect();
        (
            items,
            picker
                .rows
                .iter()
                .position(|r| Some(&r.path) == picker.selected.as_ref()),
        )
    };
    let title = if picker.worktrees.is_some() {
        if worktrees.is_some() {
            "Folders   [Worktrees]"
        } else {
            "[Folders]   Worktrees"
        }
    } else {
        "Folders"
    };
    let page = worktrees.map_or(&picker.page, |w| &w.page);
    page.borrow_mut()
        .set_items(usize::from(tree_area.height.saturating_sub(2)), &items);
    let state = worktrees.map_or(&picker.list_state, |w| &w.list_state);
    let mut state = state.borrow_mut();
    state.select(selected);
    let lines = scrollbar::ListLines::new(&items);
    frame.render_stateful_widget(
        List::new(items)
            .block(block(title, picker.focus == 1).padding(ratatui::widgets::Padding::right(1)))
            .highlight_symbol("> ")
            .highlight_style(selection_style(picker.focus == 1)),
        tree_area,
        &mut state,
    );
    let body = block("", false).inner(tree_area);
    scrollbar::draw(
        frame,
        scrollbar::inside_border_track(tree_area, body),
        lines.total(),
        usize::from(body.height),
        lines.top(state.offset()),
    );
    let ready = picker.valid && !picker.loading && !text.busy;
    modal_actions(
        frame,
        parts[4],
        "Cancel",
        "Use this folder",
        match picker.focus {
            3 => 1,
            2 => 2,
            _ => 0,
        },
        ready,
        true,
    );
    let hint = if picker.focus == 1 && worktrees.is_some() {
        "←/→ views · ↑/↓ select · Enter fills path"
    } else if picker.focus == 1 && picker.worktrees.is_some() {
        "←/→ views · ↑/↓ select · Enter expand"
    } else if picker.focus == 1 {
        "↑/↓ select · Enter expand/collapse"
    } else {
        "Enter focuses Use this folder · Tab controls"
    };
    let status = if text.busy {
        "Checking native state… Esc cancels".into()
    } else if let Some(error) = text.error.or(picker.error.as_deref()) {
        safe_text(error)
    } else if picker.loading {
        if worktrees.is_some() {
            "Loading worktrees…".into()
        } else {
            "Loading folders…".into()
        }
    } else {
        if picker.is_folder_move() {
            "End with / to keep the current folder name.".into()
        } else {
            "Tab switch controls · Esc cancel".into()
        }
    };
    frame.render_widget(Paragraph::new(status).wrap(Wrap { trim: false }), parts[3]);
    frame.render_widget(Paragraph::new(hint), parts[5]);
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Command {
    ToggleDrawer,
    OpenTerminals,
    CloseTerminal,
    CreateWorktree,
    DiscardWorktree,
    RecoverDiscard,
    GitToggle,
    GitZoom,
    FolderToggle,
    GitReturn,
    GitUp,
    GitDown,
    GitPageUp,
    GitPageDown,
    GitLeft,
    GitRight,
    GitHome,
    GitEnd,
    GitFollow,
    GitSelectWorktree,
    Help,
    PreviousPane,
    EnterFolder,
    CyclePane,
    CyclePaneBack,
    PaneUp,
    PaneDown,
    PaneLeft,
    PaneRight,
    PreviousItem,
    NextItem,
    PageUp,
    PageDown,
    FirstItem,
    LastItem,
    FolderFocus,
    FolderLeft,
    FolderRight,
    Resume,
    BackgroundTerminal,
    Info,
    CommitInfo,
    CommitDetailsKey(KeyCode),
    Relink,
    MoveFolder,
    Recover,
    Create,
    Archives,
    Search,
    Refresh,
    Quit,
}
fn available_when_folders_hidden(command: Command) -> bool {
    matches!(
        command,
        Command::FolderToggle
            | Command::OpenTerminals
            | Command::BackgroundTerminal
            | Command::Help
            | Command::Quit
            | Command::Refresh
            | Command::CyclePane
            | Command::CyclePaneBack
            | Command::PaneUp
            | Command::PaneDown
            | Command::PaneLeft
            | Command::PaneRight
            | Command::EnterFolder
            | Command::PreviousPane
    )
}

#[derive(Clone)]
pub struct MenuEntry {
    command: Command,
    section: &'static str,
    group: &'static str,
    shortcut: &'static str,
    pub label: &'static str,
}
fn open_help(browser: &Browser, view: &mut View) {
    view.help = false;
    let entries = menu_entries(browser, view);
    view.help_selected = entries
        .iter()
        .position(|entry| unavailable(entry.command, browser, view).is_none())
        .unwrap_or(0);
    view.help_entries = Some(entries);
    view.help = true;
    view.help_control = 0;
    view.help_query.clear();
    view.help_searching = false;
    view.help_page.get_mut().clear();
}
fn menu_entries(browser: &Browser, view: &View) -> Vec<MenuEntry> {
    // Keep action identities stable while a menu is open. Availability is still
    // checked on every render and activation, so a refresh cannot redirect Enter.
    if view.help {
        if let Some(entries) = &view.help_entries {
            return entries.clone();
        }
    }
    use Command::*;
    let mut entries = vec![
        (Help, "General", "?", "Help / actions"),
        (Quit, "General", "q / Ctrl-C", "Quit tracr"),
        (
            Search,
            "General",
            "/",
            if browser.focus_root.is_some() {
                "Search within focused folder"
            } else {
                "Search all sessions"
            },
        ),
        (Refresh, "General", "r", "Refresh sessions"),
        (
            Archives,
            "General",
            "a",
            if browser.include_archived {
                "Hide archived sessions"
            } else {
                "Show archived sessions"
            },
        ),
        (Recover, "General", "u", "Inspect/recover relocation"),
        (CyclePane, "Navigation", "Tab", "Next drawer panel"),
        (
            CyclePaneBack,
            "Navigation",
            "Shift-Tab",
            "Previous drawer panel",
        ),
        (
            PreviousPane,
            "Navigation",
            "← / Esc / Backspace",
            "Select Folders",
        ),
        (
            EnterFolder,
            "Folders",
            "Enter",
            "Open location / session (drawer stays open)",
        ),
        (
            PreviousItem,
            "Navigation",
            "↑ / k",
            "Previous item / scroll up",
        ),
        (NextItem, "Navigation", "↓ / j", "Next item / scroll down"),
        (PageUp, "Navigation", "PageUp", "Previous page"),
        (PageDown, "Navigation", "PageDown", "Next page"),
        (FirstItem, "Navigation", "Home", "First item"),
        (LastItem, "Navigation", "End", "Last item"),
    ];
    entries.retain(|entry| {
        (entry.0 != Recover || view.recovery_available)
            && (entry.0 != EnterFolder || view.context() == 0)
            && (entry.0 != PreviousPane || view.context() == 1)
            && (!matches!(entry.0, PageUp | PageDown | FirstItem | LastItem) || view.context() != 2)
    });
    if view.context() == 0 {
        entries.push((
            FolderToggle,
            "Folders",
            "b",
            if view.folders_hidden {
                "Expand Folders pane"
            } else {
                "Collapse Folders pane"
            },
        ));
        entries.extend([
            (Create, "Folders", "n", "Create new session"),
            (
                FolderLeft,
                "Folders",
                "← / h",
                "Collapse folder / select parent",
            ),
            (FolderRight, "Folders", "l", "Expand folder"),
            (MoveFolder, "Folders", "m", "Move folder"),
        ]);
    } else {
        entries.push(if can_create(browser) && browser.visible().is_empty() {
            (Create, "Sessions", "Enter / n", "Create new session")
        } else {
            (Resume, "Sessions", "Enter", "Open session")
        });
        if !browser.visible().is_empty() || !can_create(browser) {
            entries.push((Create, "Sessions", "n", "Create new session"));
        }
        entries.extend([
            (Info, "Sessions", "i", "Session information"),
            (Relink, "Sessions", "m", "Move session"),
        ]);
    }
    if view.context() == 0 && selected_checkout(browser).is_some() {
        entries.push((Info, "Folders", "i", "Worktree information"));
    }
    if view.context() == 0 {
        entries.push((CreateWorktree, "Folders", "w", "Create worktree"));
        if crate::worktree_discard_ui::selected(browser).is_some()
            && !view.discard_recovery_available
            && crate::worktree_discard_ui::unavailable(browser).is_none()
        {
            entries.push((DiscardWorktree, "Folders", "Delete", "Discard worktree…"));
        }
    }
    if view.discard_recovery_available && view.context() != 2 {
        entries.push((RecoverDiscard, "Folders", "D", "Review unfinished discard"));
    }
    if view.context() == 2 {
        entries
            .retain(|entry| matches!(entry.0, Help | Quit | Refresh | CyclePane | CyclePaneBack));
        entries.push((
            GitToggle,
            "Git",
            "b",
            if view.git_hidden {
                "Expand Git history"
            } else {
                "Collapse Git history"
            },
        ));
        if !view.git_hidden && !view.git_size_limited {
            entries.push((
                CommitInfo,
                "Git",
                "i",
                if view.git_details_focused {
                    "Focus Git history"
                } else if view.git_zoomed {
                    "Focus commit details"
                } else {
                    "Commit information"
                },
            ));
            entries.push((
                GitSelectWorktree,
                "Git",
                "Enter",
                if selected_git_session(browser, view).is_some() {
                    "Resume session"
                } else if view
                    .history
                    .rows
                    .get(view.history.selected)
                    .is_some_and(|row| row.stash.is_some())
                {
                    "Describe saved stash"
                } else {
                    "Select worktree folder"
                },
            ));
            entries.push((
                CreateWorktree,
                "Git",
                "w",
                "Create worktree from selected commit",
            ));
            entries.extend([
                (GitUp, "Git", "↑ / k", "Previous commit/session"),
                (GitDown, "Git", "↓ / j", "Next commit/session"),
                (GitLeft, "Git", "← / h", "Scroll left"),
                (GitRight, "Git", "l", "Scroll right"),
                (GitPageUp, "Git", "PageUp", "Previous page"),
                (GitPageDown, "Git", "PageDown", "Next page"),
                (GitHome, "Git", "Home", "First loaded item"),
                (GitEnd, "Git", "End", "Last loaded item"),
            ]);
        }
        entries.push((
            GitZoom,
            "Git",
            "Alt+Z",
            if view.git_zoomed {
                "Restore Git panel"
            } else {
                "Full-screen Git"
            },
        ));
        entries.push((
            GitReturn,
            "Navigation",
            if view.git_zoomed {
                "Esc / Backspace"
            } else {
                "Esc"
            },
            if view.git_details_focused {
                "Focus Git history"
            } else if view.git_zoomed {
                "Restore Git panel"
            } else {
                "Close drawer"
            },
        ));
    }
    if view.git_details_focused {
        entries.retain(|entry| {
            !matches!(
                entry.0,
                GitUp | GitDown | GitLeft | GitRight | GitPageUp | GitPageDown | GitHome | GitEnd
            )
        });
        for (code, shortcut, label) in [
            (KeyCode::Left, "←", "Previous details tab"),
            (KeyCode::Right, "→", "Next details tab"),
            (KeyCode::Up, "↑ / k", "Scroll details up"),
            (KeyCode::Down, "↓ / j", "Scroll details down"),
            (KeyCode::PageUp, "PageUp", "Scroll details page up"),
            (KeyCode::PageDown, "PageDown", "Scroll details page down"),
            (KeyCode::Home, "Home", "Start of details"),
            (KeyCode::End, "End", "End of details"),
            (KeyCode::Char('c'), "c", "Copy commit details"),
            (KeyCode::Char('y'), "y", "Copy commit hash"),
        ] {
            entries.push((CommitDetailsKey(code), "Commit details", shortcut, label));
        }
    }
    entries.push((
        ToggleDrawer,
        "Navigation",
        "Alt+` / Alt+~",
        "Toggle Tracr drawer",
    ));
    if view.browser_drawer_pinned() {
        for entry in &mut entries {
            if entry.0 == ToggleDrawer || (entry.0 == GitReturn && !view.git_zoomed) {
                entry.3 = if view.drawer_open {
                    "Focus Sessions"
                } else {
                    "Focus drawer"
                };
            }
        }
    }
    entries.retain(|entry| !matches!(entry.0, PreviousPane | FolderToggle | GitToggle));
    if view.drawer_open && view.terminals.borrow().active.is_some() {
        entries.insert(
            0,
            (
                BackgroundTerminal,
                "Terminals",
                "Ctrl+B",
                "Background session",
            ),
        );
    }
    if view.drawer_open {
        entries.push((
            ToggleDrawer,
            "Navigation",
            "→",
            if view.browser_drawer_pinned() {
                "Focus Sessions"
            } else {
                "Focus session and close drawer"
            },
        ));
    }
    if !view.drawer_open && view.terminals.borrow().active.is_some() {
        entries.retain(|entry| !matches!(entry.0, CyclePane | CyclePaneBack));
    } else if !view.drawer_open {
        for entry in &mut entries {
            if entry.0 == CyclePane {
                entry.2 = "Tab / ←";
                entry.3 = "Focus drawer";
            }
        }
    } else if view.drawer_open && view.history.present() {
        entries.push((PaneUp, "Navigation", "Alt+Up / Alt+Left", "Focus Folders"));
        entries.push((
            PaneDown,
            "Navigation",
            "Alt+Down / Alt+Right",
            "Focus Git Worktrees",
        ));
    }
    entries
        .retain(|entry| entry.0 != CreateWorktree || worktree_unavailable(browser, view).is_none());
    if browser
        .specs
        .iter()
        .any(|spec| matches!(spec, crate::browser::SourceSpec::Codex { .. }))
        && (view.context() == 1 || !view.terminals.borrow().is_empty())
    {
        entries.push((OpenTerminals, "Terminals", "g", "Open terminals"));
        entries.push((CloseTerminal, "Terminals", "x", "Close terminal"));
    }
    if view.context() == 0 {
        let index = entries
            .iter()
            .position(|e| e.0 == CreateWorktree)
            .unwrap_or(entries.len());
        entries.insert(
            index,
            (
                FolderFocus,
                "Folders",
                "Alt+Z",
                if browser.focus_root.is_some() {
                    "Zoom out of folder"
                } else {
                    "Zoom into selected folder"
                },
            ),
        );
    }
    if view.context() == 0 && view.folders_hidden {
        entries.retain(|entry| available_when_folders_hidden(entry.0));
    }
    let tab_section = match view.context() {
        0 => "Folders",
        1 => "Sessions",
        _ => "Git",
    };
    let mut entries: Vec<_> = entries
        .into_iter()
        .map(|(command, section, shortcut, label)| MenuEntry {
            command,
            section,
            group: if section == tab_section
                || matches!(
                    command,
                    PreviousItem | NextItem | PageUp | PageDown | FirstItem | LastItem
                ) {
                tab_section
            } else {
                "Global"
            },
            shortcut,
            label,
        })
        .collect();
    // Capture a stable order on opening; live availability must not move selection.
    entries.sort_by_key(|entry| {
        if entry.group == tab_section {
            (
                0,
                unavailable(entry.command, browser, view).is_some(),
                usize::from(entry.section == "Navigation"),
            )
        } else {
            let rank = match entry.section {
                "Navigation" => 1,
                "Terminals" => 2,
                _ => 3,
            };
            (1, false, rank)
        }
    });
    entries
}

fn sync_git_details(browser: &Browser, view: &mut View) {
    let selected = view
        .git_zoomed
        .then(|| selected_commit(browser, view))
        .flatten();
    if selected != view.git_details_key {
        view.git_details = selected.as_ref().map(|(repository, oid)| {
            crate::commit_info::Dialog::new(repository.clone(), oid.clone())
        });
        view.git_details_key = selected;
    }
    if view.git_details.is_none() {
        view.git_details_focused = false;
    }
}

fn selected_commit(browser: &Browser, view: &View) -> Option<(std::path::PathBuf, String)> {
    if git_preview_blocked(browser, view)
        || !view.git_focus
        || view.git_hidden
        || view.git_size_limited
        || selected_git_session(browser, view).is_some()
    {
        return None;
    }
    Some((
        view.history.repository()?,
        view.history.rows.get(view.history.selected)?.oid.clone()?,
    ))
}

fn worktree_unavailable(browser: &Browser, view: &View) -> Option<String> {
    if view.git_focus && git_preview_blocked(browser, view) {
        return Some("Wait for the selected Git history to load.".into());
    }
    if view.git_focus
        && view
            .history
            .rows
            .get(view.history.selected)
            .is_some_and(|row| row.stash.is_some())
    {
        return Some("Select a commit rather than a stash to create a worktree.".into());
    }
    if view.git_focus && selected_git_session(browser, view).is_some() {
        return Some("Select a commit to create a worktree.".into());
    }
    if creation_sources(browser).is_empty() {
        return Some("Fixture data is browse-only.".into());
    }
    let FolderFilter::Path(path) = &browser.folder else {
        return Some("Select a Git checkout first.".into());
    };
    let Some(owner) = browser.ownership.get(path) else {
        return Some("Wait for Git discovery to finish.".into());
    };
    if owner.generation != browser.git_generation {
        return Some("Wait for Git discovery to finish.".into());
    }
    match &owner.outcome {
        Some(crate::git_discovery::Outcome::Repository { repository, .. }) => {
            if view.git_focus && view.history.repository().as_ref() != Some(&repository.common_dir)
            {
                return Some("Wait for history of the selected repository to load.".into());
            }
        }
        Some(crate::git_discovery::Outcome::NotRepository) => {
            return Some("The selected folder is not a Git checkout.".into());
        }
        _ => return Some("Git repository information is unavailable or still loading.".into()),
    }
    if view.git_focus
        && (view.git_hidden
            || view.git_size_limited
            || view
                .history
                .rows
                .get(view.history.selected)
                .and_then(|row| row.oid.as_ref())
                .is_none())
    {
        return Some("Select a visible Git commit first.".into());
    }
    None
}

fn unavailable(command: Command, browser: &Browser, view: &View) -> Option<String> {
    match command {
        Command::FolderFocus
            if browser.focus_root.is_none() && !matches!(browser.folder, FolderFilter::Path(_)) =>
        {
            Some("Select a concrete folder first.".into())
        }
        Command::CreateWorktree => worktree_unavailable(browser, view),
        Command::CommitInfo if selected_commit(browser, view).is_none() => {
            Some("Select a visible Git commit first.".into())
        }
        Command::FolderLeft | Command::FolderRight
            if !matches!(browser.folder, FolderFilter::Path(_)) =>
        {
            Some("Select a concrete folder first.".into())
        }
        Command::Info if view.focus == 0 && selected_checkout(browser).is_none() => {
            Some("Select a checkout folder first.".into())
        }
        Command::Info if view.focus == 1 && selected_session(browser).is_none() => {
            Some("Select a session first.".into())
        }
        Command::Relink => relink_unavailable(browser),
        Command::MoveFolder => crate::folder_move_ui::unavailable(browser),
        Command::Create => {
            if browser.loading() {
                Some("Wait for discovery to finish.".into())
            } else if creation_sources(browser).is_empty() {
                Some("Fixture data is browse-only.".into())
            } else if !matches!(browser.folder, FolderFilter::Path(_)) {
                Some("Select a concrete folder first.".into())
            } else {
                None
            }
        }
        Command::Resume => {
            let session = browser
                .visible()
                .into_iter()
                .find(|s| Some(&s.key) == browser.selected.as_ref());
            let Some(session) = session else {
                return Some("Select a session first.".into());
            };
            if creation_sources(browser).is_empty() {
                return Some("Fixture data is browse-only.".into());
            }
            if let Some(reason) = crate::domain::resume_blocker(&session) {
                return Some(reason.into());
            }
            match browser.details.get(&session.key) {
                Some(Ok(Some(detail))) => crate::domain::resume_blocker(detail).map(str::to_owned),
                Some(Ok(None)) => Some("Session no longer available; refresh the list.".into()),
                Some(Err(error)) => Some(error.clone()),
                None => None,
            }
        }
        _ => None,
    }
}
fn filtered_menu_entries(browser: &Browser, view: &View) -> Vec<MenuEntry> {
    let query = view.help_query.to_lowercase();
    menu_entries(browser, view)
        .into_iter()
        .filter(|entry| {
            let text = format!(
                "{} {} {} {}",
                entry.group, entry.section, entry.label, entry.shortcut
            )
            .to_lowercase();
            query.split_whitespace().all(|term| text.contains(term))
        })
        .collect()
}

fn draw_help(frame: &mut Frame<'_>, browser: &Browser, view: &View) {
    let inner = crate::modal::shell(frame, "Actions & shortcuts", 80, 28);
    if inner.width < 26 || inner.height < 7 {
        view.help_page.borrow_mut().clear();
        frame.render_widget(
            Paragraph::new("Enlarge terminal. Esc closes.").wrap(Wrap { trim: false }),
            inner,
        );
        return;
    }
    let search_area = Rect::new(inner.x, inner.y, inner.width, 1);
    let query = format!(
        "{}{}",
        if view.help_searching {
            "Search: "
        } else {
            "/ Filter: "
        },
        view.help_query
    );
    // Keep the end of a long query visible while editing.
    let overflow =
        unicode_width::UnicodeWidthStr::width(query.as_str()).saturating_sub(inner.width as usize);
    frame.render_widget(
        Paragraph::new(query)
            .scroll((0, overflow.min(u16::MAX as usize) as u16))
            .style(Style::default().fg(if view.help_searching {
                Color::Cyan
            } else {
                Color::DarkGray
            })),
        search_area,
    );
    let regions = crate::modal::regions(Rect::new(
        inner.x,
        inner.y + 1,
        inner.width,
        inner.height - 1,
    ));
    let sections = [regions.body, regions.status, regions.hints];
    let entries = filtered_menu_entries(browser, view);
    let selected = view.help_selected.min(entries.len().saturating_sub(1));
    modal_actions(
        frame,
        regions.actions,
        "Close",
        "Run action",
        view.help_control,
        entries
            .get(selected)
            .is_some_and(|entry| unavailable(entry.command, browser, view).is_none()),
        true,
    );
    let mut items = Vec::new();
    if entries.is_empty() {
        items.push(ListItem::new("No matching actions"));
    }
    let mut last_section = "";
    let mut selected_row = 0;
    let mut entry_rows = Vec::with_capacity(entries.len());
    for (i, entry) in entries.iter().enumerate() {
        if entry.group != last_section {
            if !last_section.is_empty() {
                items.push(ListItem::new(""));
                items.push(ListItem::new(Line::styled(
                    "─".repeat(usize::from(sections[0].width.saturating_sub(3))),
                    Style::default().fg(Color::DarkGray),
                )));
            }
            items.push(ListItem::new(Line::styled(
                entry.group,
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            )));
            last_section = entry.group;
        }
        entry_rows.push(items.len());
        if i == selected {
            selected_row = items.len();
        }
        let disabled = unavailable(entry.command, browser, view).is_some();
        let style = if disabled {
            Style::default().fg(Color::DarkGray)
        } else {
            Style::default()
        };
        let lines = if sections[0].width >= 68 {
            vec![Line::from(vec![
                Span::styled(
                    format!("{:<22}", entry.shortcut),
                    style.add_modifier(Modifier::BOLD),
                ),
                Span::styled(entry.label, style),
            ])]
        } else {
            let mut lines = vec![Line::styled(
                entry.shortcut,
                style.add_modifier(Modifier::BOLD),
            )];
            let mut line = String::new();
            for word in entry.label.split_whitespace() {
                if !line.is_empty()
                    && line.len() + 1 + word.len()
                        > usize::from(sections[0].width.saturating_sub(3))
                {
                    lines.push(Line::styled(std::mem::take(&mut line), style));
                }
                if !line.is_empty() {
                    line.push(' ');
                }
                line.push_str(word);
            }
            lines.push(Line::styled(line, style));
            lines
        };
        items.push(ListItem::new(lines));
    }
    let lines = scrollbar::ListLines::new(&items);
    let body = scrollbar::content_area(sections[0], lines.total());
    view.help_page.borrow_mut().set(
        usize::from(body.height),
        entry_rows.into_iter().map(|row| lines.top(row)).collect(),
    );
    let mut state =
        ListState::default().with_selected((!entries.is_empty()).then_some(selected_row));
    frame.render_stateful_widget(
        List::new(items)
            .highlight_symbol("> ")
            .highlight_style(selection_style(view.help_control == 0)),
        body,
        &mut state,
    );
    if body.width < sections[0].width {
        scrollbar::draw(
            frame,
            scrollbar::border_track(sections[0], body),
            lines.total(),
            usize::from(body.height),
            lines.top(state.offset()),
        );
    }
    if let Some(reason) = entries
        .get(selected)
        .and_then(|entry| unavailable(entry.command, browser, view))
    {
        frame.render_widget(
            Paragraph::new(reason).wrap(Wrap { trim: false }),
            sections[1],
        );
    }
    frame.render_widget(
        Paragraph::new(if inner.width >= 43 {
            if view.help_searching {
                "↑/↓ select · Enter run · Esc stop search"
            } else {
                "/ filter · ↑/↓ select · Enter run · Esc close"
            }
        } else if view.help_searching {
            "Esc stop search · ↑↓ · Enter run"
        } else if inner.width >= 32 {
            "/ filter · Esc close · Enter run"
        } else {
            "/ filter · Esc close"
        }),
        sections[2],
    );
}

pub(crate) fn activity_color(status: crate::domain::RuntimeStatus) -> Option<Color> {
    match status {
        crate::domain::RuntimeStatus::Running => Some(Color::Green),
        crate::domain::RuntimeStatus::Idle => Some(Color::Rgb(255, 255, 170)),
        crate::domain::RuntimeStatus::Waiting { .. } => Some(Color::Rgb(255, 165, 0)),
        _ => None,
    }
}
fn session_title(
    session: &crate::domain::SessionSummary,
    status: crate::domain::RuntimeStatus,
    width: u16,
) -> Line<'static> {
    use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};
    let title = format!(
        "{}{}",
        if session.archived { "[archived] " } else { "" },
        safe_text(&session.title)
    );
    let Some(label) = status.label() else {
        return Line::styled(title, Style::default().add_modifier(Modifier::BOLD));
    };
    let label = if usize::from(width) < label.width() + 12 {
        match status {
            crate::domain::RuntimeStatus::Running => "Running",
            crate::domain::RuntimeStatus::Waiting { .. } => "Waiting",
            _ => label,
        }
    } else {
        label
    };
    let available = usize::from(width).saturating_sub(label.width() + 1);
    let mut clipped = String::new();
    let mut used = 0;
    if title.width() > available {
        for c in title.chars() {
            let size = c.width().unwrap_or(0);
            if used + size > available.saturating_sub(1) {
                break;
            }
            clipped.push(c);
            used += size;
        }
        if available > 0 {
            clipped.push('…');
        }
    } else {
        clipped = title;
    }
    let padding = usize::from(width).saturating_sub(clipped.width() + label.width());
    Line::from(vec![
        Span::styled(clipped, Style::default().add_modifier(Modifier::BOLD)),
        Span::raw(" ".repeat(padding)),
        Span::styled(
            label,
            Style::default()
                .fg(activity_color(status).unwrap_or(Color::Reset))
                .add_modifier(Modifier::BOLD),
        ),
    ])
}

fn draw_search(frame: &mut Frame<'_>, browser: &Browser, view: &View) {
    let area = frame.area();
    let inner = crate::modal::shell(
        frame,
        if browser.focus_root.is_some() {
            "Search within folder"
        } else {
            "Search sessions"
        },
        area.width
            .saturating_sub(if area.width >= 28 { 4 } else { 0 }),
        area.height
            .saturating_sub(if area.height >= 12 { 2 } else { 0 }),
    );
    let compact = inner.height < 18;
    let short = inner.height < 12;
    let show_progress = browser.deep_search && !view.search_query.trim().is_empty();
    let mode_height = if compact { 1 } else { 2 };
    let sections = Layout::vertical([
        Constraint::Length(u16::from(browser.focus_root.is_some())),
        Constraint::Length(if short { 1 } else { 3 }),
        Constraint::Length(mode_height),
        Constraint::Min(1),
        Constraint::Length(u16::from(show_progress)),
        Constraint::Length(1),
        Constraint::Length(1),
    ])
    .split(inner);
    if let Some(root) = &browser.focus_root {
        let scope = safe_text(&format!("Within: {}", root.display()));
        let clipped =
            unicode_width::UnicodeWidthStr::width(scope.as_str()) > usize::from(sections[0].width);
        let mut line = graph_viewport(
            vec![Span::raw(scope)],
            0,
            usize::from(sections[0].width.saturating_sub(u16::from(clipped))),
        );
        if clipped && sections[0].width > 0 {
            line.spans.push(Span::raw("…"));
        }
        frame.render_widget(Paragraph::new(line), sections[0]);
    }
    let mode_style = if view.search_control == 3 {
        Style::default()
            .fg(Color::Black)
            .bg(Color::Cyan)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default()
            .fg(if browser.deep_search {
                Color::Green
            } else {
                Color::Yellow
            })
            .add_modifier(Modifier::BOLD)
    };
    let instructions = if inner.width >= 70 {
        "   Tab selects · Space toggles · Ctrl+D shortcut"
    } else {
        " · Ctrl+D"
    };
    let mut mode_lines = vec![Line::from(vec![
        Span::styled(
            if browser.deep_search {
                "[x] Deep search: ON"
            } else {
                "[ ] Deep search: OFF"
            },
            mode_style,
        ),
        Span::raw(instructions),
    ])];
    if !compact {
        mode_lines.push(Line::from(if inner.width < 70 {
            "Tab selects · Space toggles"
        } else if browser.deep_search {
            "Searches messages, plans, final replies, and metadata."
        } else {
            "Metadata only. Enable deep search to find conversation content."
        }));
    }
    if short {
        let text = safe_text(&format!("Search: {}", view.search_query));
        let width = usize::from(sections[1].width);
        let text_width = unicode_width::UnicodeWidthStr::width(text.as_str());
        let offset = text_width.saturating_sub(width.saturating_sub(1));
        frame.render_widget(
            Paragraph::new(graph_viewport(vec![Span::raw(text)], offset, width)),
            sections[1],
        );
        if view.search_control == 0 && sections[1].width > 0 && sections[1].height > 0 {
            frame.set_cursor_position((
                sections[1].x + text_width.min(width - 1) as u16,
                sections[1].y,
            ));
        }
    } else {
        crate::modal::text_field(
            frame,
            sections[1],
            "Search",
            &view.search_query,
            view.search_query.len(),
            view.search_control == 0,
            true,
        );
    }
    frame.render_widget(Paragraph::new(mode_lines), sections[2]);
    if show_progress {
        frame.render_widget(
            Paragraph::new(crate::search::progress(browser)),
            sections[4],
        );
    }
    modal_actions(
        frame,
        sections[5],
        "Cancel",
        "Open",
        view.search_control,
        !browser.search_results(&view.search_query).is_empty(),
        true,
    );
    frame.render_widget(
        Paragraph::new("↑/↓ results · Enter opens · Esc cancels"),
        sections[6],
    );
    let result_block = if sections[3].height >= 4 {
        block("Results — ↑/↓ select · Enter opens folder", false)
    } else {
        Block::default()
    }
    .padding(ratatui::widgets::Padding::horizontal(1));
    let results = browser.search_results(&view.search_query);
    if results.is_empty() {
        view.search_page.borrow_mut().clear();
        frame.render_widget(
            Paragraph::new(
                if browser.loading()
                    || (browser.deep_search
                        && !view.search_query.trim().is_empty()
                        && crate::search::pending(browser))
                {
                    "Searching…"
                } else {
                    "No matching sessions"
                },
            )
            .block(result_block),
            sections[3],
        );
        return;
    }
    let index = results
        .iter()
        .position(|s| Some(&s.key) == view.search_selected.as_ref())
        .unwrap_or(0);
    let result_height = usize::from(result_block.inner(sections[3]).height);
    let items: Vec<_> = results
        .iter()
        .map(|s| {
            let mut lines = vec![
                session_title(
                    s,
                    browser.runtime_status(&s.key),
                    sections[3].width.saturating_sub(6),
                ),
                Line::from(safe_text(
                    &s.cwd
                        .as_ref()
                        .map(|p| p.display().to_string())
                        .unwrap_or_else(|| "Unknown folder".into()),
                )),
            ];
            if let Some(excerpt) = crate::search::excerpt(browser, s, &view.search_query) {
                lines.push(Line::from(excerpt));
            }
            if !compact {
                lines.push(Line::default());
            }
            lines.truncate(result_height.max(1));
            ListItem::new(lines)
        })
        .collect();
    let body = result_block.inner(sections[3]);
    let lines = scrollbar::ListLines::new(&items);
    let mut state = ListState::default().with_selected(Some(index));
    view.search_page
        .borrow_mut()
        .set_items(result_height, &items);
    frame.render_stateful_widget(
        List::new(items)
            .block(result_block)
            .highlight_style(selection_style(view.help_control == 0))
            .highlight_symbol("> "),
        sections[3],
        &mut state,
    );
    scrollbar::draw(
        frame,
        if sections[3].height >= 4 {
            scrollbar::inside_border_track(sections[3], body)
        } else {
            scrollbar::border_track(sections[3], body)
        },
        lines.total(),
        usize::from(body.height),
        lines.top(state.offset()),
    );
}

/// Slice terminal columns without splitting graphemes or wide glyphs.
fn graph_viewport(spans: Vec<Span<'static>>, offset: usize, width: usize) -> Line<'static> {
    use unicode_segmentation::UnicodeSegmentation;
    use unicode_width::UnicodeWidthStr;
    let end = offset.saturating_add(width);
    let mut column = 0;
    let mut clipped = Vec::new();
    for span in spans {
        for grapheme in span.content.graphemes(true) {
            let next = column + grapheme.width();
            if column >= end {
                return Line::from(clipped);
            }
            if next > offset {
                let text = if column >= offset && next <= end {
                    grapheme.to_owned()
                } else {
                    " ".repeat(next.min(end).saturating_sub(column.max(offset)))
                };
                clipped.push(Span::styled(text, span.style));
            }
            column = next;
        }
    }
    Line::from(clipped)
}

/// Draw connectors from the complete displayed hierarchy, including projected
/// worktrees. Filesystem parents and viewport boundaries are not tree endings.
fn folder_tree_labels(rows: &[crate::browser::FolderRow]) -> Vec<String> {
    let mut later_sibling = vec![false; rows.len()];
    let mut depths = Vec::new();
    for (index, row) in rows.iter().enumerate().rev() {
        while depths.last().is_some_and(|depth| *depth > row.depth) {
            depths.pop();
        }
        later_sibling[index] = depths.last() == Some(&row.depth);
        if !later_sibling[index] {
            depths.push(row.depth);
        }
    }
    let mut ancestors = Vec::new();
    rows.iter()
        .enumerate()
        .map(|(index, row)| {
            ancestors.resize(row.depth + 1, false);
            ancestors[row.depth] = later_sibling[index];
            let mut label = String::new();
            if row.depth > 0 {
                for &continues in ancestors.iter().take(row.depth.min(20)).skip(1) {
                    label.push_str(if continues { "│  " } else { "   " });
                }
                label.push_str(if later_sibling[index] {
                    "├─ "
                } else {
                    "└─ "
                });
            }
            label.push_str(&safe_text(&row.label));
            label.push_str(&format!(" ({})", row.count));
            label
        })
        .collect()
}

fn folder_tree_line(
    row: &crate::browser::FolderRow,
    label: &str,
    status: crate::domain::RuntimeStatus,
    offset: usize,
    width: usize,
) -> Line<'static> {
    let text = format!("{} ({})", safe_text(&row.label), row.count);
    let prefix = &label[..label.len() - text.len()];
    let text_style =
        activity_color(status).map_or(Style::default(), |color| Style::default().fg(color));
    let mut spans = vec![Span::styled(
        prefix.to_owned(),
        Style::default().fg(Color::White),
    )];
    let safe_label = safe_text(&row.label);
    let (name, dirty) = safe_label
        .strip_suffix(" *")
        .map_or((safe_label.as_str(), false), |s| (s, true));
    if let Some(name) = name.strip_prefix(crate::git::WORKTREE_MARKER) {
        spans.push(Span::styled(
            "◇",
            Style::default().fg(crate::git_render::WORKTREE_COLOR),
        ));
        spans.push(Span::styled(format!(" {name}"), text_style));
    } else {
        spans.push(Span::styled(name.to_owned(), text_style));
    }
    if dirty {
        spans.push(Span::styled(" ", text_style));
        spans.push(Span::styled(
            "*",
            Style::default().fg(crate::git_render::DIRTY_COLOR),
        ));
    }
    spans.push(Span::styled(format!(" ({})", row.count), text_style));
    graph_viewport(spans, offset, width)
}

fn folder_title(browser: &Browser, width: u16) -> String {
    use unicode_width::UnicodeWidthStr;
    let prefix = "Folders · ";
    let available = usize::from(width).saturating_sub(prefix.width());
    let Some(root) = browser.focus_root.as_ref().filter(|_| available > 0) else {
        return "Folders".into();
    };
    let path = safe_text(&root.display().to_string());
    let path_width = path.width();
    if path_width <= available {
        return format!("{prefix}{path}");
    }
    let remaining = available - 1;
    let suffix = graph_viewport(vec![Span::raw(path)], path_width - remaining, remaining);
    format!("{prefix}…{suffix}")
}

fn draw_pane(frame: &mut Frame<'_>, area: Rect, pane: usize, browser: &Browser, view: &View) {
    match pane {
        0 => {
            let folders = view.folder_rows(browser);
            let selected = folders.iter().position(|row| row.filter == browser.folder);
            let labels = folder_tree_labels(&folders);
            // Reserve an interior scrollbar column when the folder list overflows.
            let scrolling = folders.len() > usize::from(pane_block("", false).inner(area).height);
            let width = usize::from(area.width.saturating_sub(4 + u16::from(scrolling)));
            let offset = selected
                .map(|index| {
                    unicode_width::UnicodeWidthStr::width(labels[index].as_str())
                        .saturating_sub(width)
                })
                .unwrap_or(0);
            let items: Vec<_> = folders
                .iter()
                .zip(&labels)
                .map(|(row, label)| {
                    ListItem::new(folder_tree_line(
                        row,
                        label,
                        browser.folder_activity(&row.filter),
                        offset,
                        width,
                    ))
                })
                .collect();
            // Folder rows occupy one line each. Keep the selection centered,
            // clamping at the ends so the viewport stays filled.
            let title = folder_title(browser, area.width.saturating_sub(2));
            let block = pane_block(&title, view.context() == 0).padding(
                ratatui::widgets::Padding::new(0, u16::from(scrolling), 1, 0),
            );
            let visible_rows = usize::from(block.inner(area).height);
            view.folder_page_size.set(visible_rows);
            let top = selected
                .filter(|_| visible_rows > 0)
                .map(|index| {
                    index
                        .saturating_sub(visible_rows / 2)
                        .min(folders.len().saturating_sub(visible_rows))
                })
                .unwrap_or(0);
            let mut state = ListState::default()
                .with_offset(top)
                .with_selected(selected);
            frame.render_stateful_widget(
                List::new(items)
                    .block(block)
                    .highlight_style(selection_style(view.context() == pane))
                    .highlight_symbol("> "),
                area,
                &mut state,
            );
            scrollbar::draw(
                frame,
                scrollbar::inside_border_track(area, pane_block("", false).inner(area)),
                folders.len(),
                visible_rows,
                state.offset(),
            );
        }
        1 => {
            let drawer_visible = view.drawer_visible();
            let label = view.terminals.borrow().active_folder().and_then(|folder| {
                crate::inline_terminal::WorktreeLabel::for_folder(browser, &folder)
            });
            if view.terminals.borrow_mut().draw_with_worktree(
                frame,
                area,
                view.terminal_focused(),
                drawer_visible,
                label.as_ref(),
            ) {
                return;
            }
            let preview = preview_session(browser, view);
            let mut preview_state = view.session_preview.borrow_mut();
            if let Some(session) = &preview {
                if preview_state.key.as_ref() == Some(&session.key) {
                    let border = pane_block("Codex preview · Enter to interact", false);
                    let inner = border.inner(area);
                    preview_state.terminals.area = inner;
                    if preview_state.terminals.draw(frame, inner, false, false) {
                        frame.render_widget(border, area);
                        return;
                    }
                }
            }
            let sessions = if let Some(session) = &preview {
                let mut sessions: Vec<_> = browser
                    .all()
                    .into_iter()
                    .filter(|s| {
                        s.cwd == session.cwd
                            && (!s.archived || browser.include_archived || s.key == session.key)
                    })
                    .collect();
                if !sessions.iter().any(|s| s.key == session.key) {
                    sessions.push(session.clone());
                }
                sessions
            } else {
                browser.visible()
            };
            let selected = preview
                .as_ref()
                .map(|s| &s.key)
                .or(browser.selected.as_ref());
            let preview_notice = safe_text(&preview_state.notice);
            let scope = preview
                .as_ref()
                .map(|session| {
                    format!(
                        "Sessions · {} · {}",
                        session
                            .cwd
                            .as_ref()
                            .map(|p| safe_text(&p.display().to_string()))
                            .unwrap_or_else(|| "Unknown folder".into()),
                        if preview_state.key.as_ref() == Some(&session.key) {
                            if preview_state.notice.is_empty() {
                                "Loading preview…"
                            } else {
                                &preview_notice
                            }
                        } else {
                            "Preview"
                        }
                    )
                })
                .unwrap_or_else(|| browser.scope_label());
            // Preserve eight columns for essential status labels in tiny panes.
            let right_padding = area.width.saturating_sub(12).min(2).max(u16::from(
                sessions.len() * 3 > usize::from(pane_block("", false).inner(area).height),
            ));
            let now = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs()
                .min(i64::MAX as u64) as i64;
            if sessions.is_empty() {
                draw_empty_sessions(frame, area, browser, view.context() == 1);
                return;
            }
            let items: Vec<_> = sessions
                .iter()
                .map(|s| {
                    ListItem::new(vec![
                        session_title(
                            s,
                            browser.runtime_status(&s.key),
                            area.width.saturating_sub(4 + right_padding),
                        ),
                        if view.terminals.borrow().owns(&s.key) {
                            Line::from("Open here · Enter Open · x Close terminal")
                        } else {
                            session_metadata(browser, s, now)
                        },
                        Line::default(),
                    ])
                })
                .collect();
            let mut state = ListState::default()
                .with_selected(sessions.iter().position(|s| Some(&s.key) == selected));
            let block = pane_block(&scope, view.context() == 1)
                .padding(ratatui::widgets::Padding::new(0, right_padding, 1, 0));
            view.session_page_size
                .set(usize::from(block.inner(area).height) / 3);
            frame.render_stateful_widget(
                List::new(items)
                    .block(block)
                    .highlight_style(selection_style(view.context() == pane))
                    .highlight_symbol("> "),
                area,
                &mut state,
            );
            scrollbar::draw(
                frame,
                scrollbar::inside_border_track(area, pane_block("", false).inner(area)),
                sessions.len() * 3,
                usize::from(pane_block("", false).inner(area).height),
                state.offset() * 3,
            );
        }
        _ => {}
    }
}

fn draw_empty_sessions(frame: &mut Frame, area: Rect, browser: &Browser, focused: bool) {
    let title = browser.scope_label();
    let margin = area.width.saturating_sub(10).min(4) / 2;
    let border =
        pane_block(&title, focused).padding(ratatui::widgets::Padding::new(margin, margin, 1, 0));
    let inner = border.inner(area);
    frame.render_widget(border, area);
    if inner.width == 0 || inner.height == 0 {
        return;
    }
    let status = if browser.loading() {
        "Loading sessions…"
    } else if browser.diagnostics().iter().any(|source| !source.complete) {
        "Session inventory incomplete.\nSee source errors below."
    } else {
        "No sessions currently shown"
    };
    let primary_style = Style::default()
        .add_modifier(Modifier::BOLD)
        .fg(if focused { Color::Cyan } else { Color::Reset });
    let muted = Style::default().fg(Color::DarkGray);
    let width = inner.width.min(40);
    let create = can_create(browser);
    let mut gaps = true;
    let mut hints = true;
    let mut compact = false;
    let paragraph = loop {
        let mut lines: Vec<Line<'_>> = status.lines().map(Line::from).collect();
        if create {
            if gaps {
                lines.push(Line::default());
            }
            if compact {
                lines.push(Line::styled("Enter: new session", primary_style));
            } else {
                lines.push(Line::styled("Create new session", primary_style));
                lines.push(Line::styled("Press Enter", primary_style));
            }
        }
        if hints {
            if gaps {
                lines.push(Line::default());
            }
            if !browser.include_archived {
                lines.push(Line::styled("a  Show archived sessions", muted));
            }
            lines.push(Line::styled(
                if browser.focus_root.is_some() {
                    "/  Search within focused folder"
                } else {
                    "/  Search all sessions"
                },
                muted,
            ));
        }
        let paragraph = Paragraph::new(lines)
            .alignment(ratatui::layout::Alignment::Center)
            .wrap(Wrap { trim: true });
        if paragraph.line_count(width) <= usize::from(inner.height) {
            break paragraph;
        }
        if gaps {
            gaps = false;
        } else if hints {
            hints = false;
        } else if create && !compact {
            compact = true;
        } else {
            break paragraph;
        }
    };
    let height = paragraph.line_count(width).min(usize::from(inner.height)) as u16;
    let spare_rows = inner.height - height;
    // The pane already supplies one blank row; add three more when space permits.
    let top_offset = spare_rows.min(3);
    let content = Rect::new(
        inner.x + (inner.width - width) / 2,
        inner.y + top_offset,
        width,
        height,
    );
    frame.render_widget(paragraph, content);
}

fn selected_checkout(browser: &Browser) -> Option<&std::path::Path> {
    let FolderFilter::Path(path) = &browser.folder else {
        return None;
    };
    browser
        .repositories
        .values()
        .flat_map(|r| &r.worktrees)
        .find(|w| !w.bare && &w.path == path)
        .map(|w| w.path.as_path())
}

fn worktree_status_description(browser: &Browser, path: &std::path::Path) -> String {
    match browser
        .worktree_status
        .iter()
        .filter(|(root, _)| path.starts_with(root))
        .max_by_key(|(root, _)| root.components().count())
        .map(|(_, s)| s)
    {
        Some(Ok(counts)) => counts.description(),
        Some(Err(error)) => format!("Unavailable: {error}"),
        None => "Not yet available".into(),
    }
}

fn session_information(
    browser: &Browser,
    s: &crate::domain::SessionSummary,
) -> crate::info_content::Information {
    use crate::domain::{FolderStatus, RuntimeStatus};
    use crate::info_content::{Information, Row, Section, local_date};
    let value = |value: Option<&str>| {
        value
            .filter(|s| !s.is_empty())
            .unwrap_or("Not recorded")
            .to_owned()
    };
    let owner = browser
        .specs
        .iter()
        .find_map(|spec| match spec {
            crate::browser::SourceSpec::Codex { source, .. } if source.id == s.key.source_id => {
                Some(source.home.display().to_string())
            }
            _ => None,
        })
        .unwrap_or_else(|| "Not recorded".into());
    let path = s
        .cwd
        .as_ref()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|| "Not recorded".into());
    let folder_state = match s.cwd.as_ref().and_then(|p| browser.folders.get(p)) {
        Some(FolderStatus::Available) => "Available".into(),
        Some(FolderStatus::Unlinked) => "Missing".into(),
        Some(FolderStatus::Inaccessible { reason }) => format!("Inaccessible: {reason}"),
        _ => "Unknown".into(),
    };
    let mut notices = vec![];
    let mut location = vec![Row::new("Folder", path.clone())];
    match browser.details.get(&s.key) {
        Some(Ok(Some(detail))) if detail.cwd != s.cwd => {
            notices.push("Folder differs from Codex's current resume destination.".into());
            location = vec![
                Row::new("Listed folder", path),
                Row::new(
                    "Resume folder",
                    detail
                        .cwd
                        .as_ref()
                        .map(|p| p.display().to_string())
                        .unwrap_or_else(|| "Not recorded".into()),
                ),
            ];
        }
        Some(Err(error)) => notices.push(format!("Could not load session information: {error}")),
        Some(Ok(None)) => notices.push("Session no longer available".into()),
        None => notices.push("Fetching metadata…".into()),
        _ => (),
    }
    if is_unlinked(browser, s) || crate::domain::resume_blocker(s).is_some() {
        notices.push(resume_hint(browser, s));
    }
    let runtime = browser.runtime_status(&s.key);
    let mut status = Row::new(
        "Status",
        if runtime == RuntimeStatus::Unknown {
            "Unknown"
        } else {
            runtime.label().unwrap_or("Unknown")
        },
    );
    status.color = activity_color(runtime);
    status.note = Some(
        match runtime {
            RuntimeStatus::Idle => "Cached by Codex; release is attempted when moving.",
            RuntimeStatus::Unloaded => "Released at the last check; moving still checks ownership.",
            RuntimeStatus::Running => "Working in Codex; close the session before moving.",
            RuntimeStatus::Waiting { .. } => "Waiting in Codex; close the session before moving.",
            RuntimeStatus::Unknown => "Live state unavailable; moving checks native ownership.",
            RuntimeStatus::Error => "Codex reported an error; moving checks native ownership.",
        }
        .into(),
    );
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64);
    let activity = if s.updated_at == 0 {
        "Not recorded".into()
    } else {
        format!(
            "{}\n{}",
            relative_time(s.updated_at, now),
            local_date(s.updated_at)
        )
    };
    if let Some(path) = s.cwd.as_ref() {
        location.push(Row::new(
            "Uncommitted changes",
            worktree_status_description(browser, path),
        ));
    }
    location.extend([
        Row::new("Folder state", folder_state),
        Row::new(
            "Git branch",
            value(s.git.as_ref().and_then(|g| g.branch.as_deref())),
        ),
        Row::new(
            "Repository",
            value(s.git.as_ref().and_then(|g| g.origin_url.as_deref())),
        ),
    ]);
    Information {
        title: s.title.clone(),
        notices,
        sections: vec![
            Section {
                title: "Session",
                rows: vec![
                    status,
                    Row::new("Last activity", activity),
                    Row::new("Created", local_date(s.created_at)),
                    Row::new(
                        "Agent",
                        if s.key.provider.0 == "codex" {
                            "Codex".into()
                        } else {
                            s.key.provider.0.clone()
                        },
                    ),
                    Row::new("Archived", if s.archived { "Yes" } else { "No" }),
                ],
            },
            Section {
                title: "Location",
                rows: location,
            },
            Section {
                title: "Technical details",
                rows: vec![
                    Row::new("Session ID", s.key.native_id.clone()),
                    Row::new("Codex home", owner),
                    Row::new(
                        "Session source",
                        if s.source_kind.as_deref() == Some("cli") {
                            "CLI".into()
                        } else {
                            value(s.source_kind.as_deref())
                        },
                    ),
                    Row::new(
                        "Git commit",
                        value(s.git.as_ref().and_then(|g| g.commit.as_deref())),
                    ),
                ],
            },
        ],
    }
}

pub struct InfoModal {
    pub session: crate::domain::SessionSummary,
    pub dialog: crate::error_dialog::ErrorDialog,
    warning: Option<String>,
}
impl InfoModal {
    pub fn new(browser: &Browser, session: crate::domain::SessionSummary) -> Self {
        let mut dialog = crate::error_dialog::ErrorDialog::information("Session information", "");
        dialog.set_information(session_information(browser, &session));
        Self {
            session,
            dialog,
            warning: None,
        }
    }
    pub fn refresh(&mut self, browser: &Browser) {
        let current = browser
            .all()
            .into_iter()
            .find(|s| s.key == self.session.key);
        let missing = current.is_none() && !browser.loading();
        if let Some(current) = current {
            self.session = current;
        }
        let mut body = session_information(browser, &self.session);
        if missing {
            body.notices.insert(
                0,
                "Session no longer available in the current inventory.".into(),
            );
        }
        if let Some(warning) = &self.warning {
            body.notices.insert(0, warning.clone());
        }
        self.dialog.set_information(body);
    }
}

#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum Action {
    CopyErrorReport,
    CopyInformation,
    CloseInformation,
    Continue,
    OpenRelink,
    OpenFolderMove,
    OpenDiscard,
    RecoverDiscard,
    ApplyDiscard,
    RetryDiscard,
    ReleaseDiscardIdle,
    ReviewDiscardTerminals,
    ConfirmDiscardTerminals,
    CloseDiscard,
    PreviewFolderMove,
    ApplyFolderMove,
    CloseFolderMove,
    CancelRelink,
    RefreshTerminals,
    StopTerminals,
    PreviewRelink,
    ApplyRelink,
    RecoverRelink,
    Quit,
    Refresh,
    ResumeSelected,
    CancelResume,
    CreateNew(usize),
}

fn selected_session(browser: &Browser) -> Option<crate::domain::SessionSummary> {
    browser
        .visible()
        .into_iter()
        .find(|s| Some(&s.key) == browser.selected.as_ref())
}
fn relink_unavailable(browser: &Browser) -> Option<String> {
    let session = match selected_session(browser) {
        Some(s) => s,
        None => return Some("Select a session first.".into()),
    };
    if browser.loading() {
        return Some("Wait for discovery to finish.".into());
    }
    let index=browser.specs.iter().position(|s|matches!(s,crate::browser::SourceSpec::Codex{source,..} if source.id==session.key.source_id));
    let Some(index) = index else {
        return Some("Fixture data is browse-only.".into());
    };
    if browser.sources[index].error.is_some() || !browser.sources[index].warnings.is_empty() {
        return Some("Resolve source discovery warnings before relinking.".into());
    }
    if browser.sources[index].live.contains_key(&session.key)
        && browser.runtime_status(&session.key) != crate::domain::RuntimeStatus::Idle
    {
        use crate::domain::RuntimeStatus;
        return Some(match browser.runtime_status(&session.key) {
            RuntimeStatus::Running => "This session is running in Codex. Close it and wait for Codex to release it before relinking.",
            RuntimeStatus::Waiting { .. } => "This session is waiting in Codex. Close it and wait for Codex to release it before relinking.",
            _ => "Codex still reports this session as loaded. Wait for it to be released before relinking.",
        }.into());
    }
    if !matches!(
        crate::domain::classify_folder(session.cwd.as_deref()),
        crate::domain::FolderStatus::Available | crate::domain::FolderStatus::Unlinked
    ) {
        return Some("Relinking requires a known original folder that is not inaccessible.".into());
    }
    if browser.state_path.is_none() {
        return Some("Persistent state is required for relocation recovery.".into());
    }
    None
}
enum FolderMoveResult {
    Preview(Result<crate::folder_move::Plan, String>),
    Applied(crate::folder_move::Outcome),
}
enum RelinkResult {
    Terminals(Result<Vec<crate::providers::codex_terminals::Terminal>, crate::relocation::Failure>),
    Preview(Result<crate::relocation::Preview, crate::relocation::Failure>),
    Applied(crate::relocation::Outcome),
}
fn creation_sources(browser: &Browser) -> Vec<usize> {
    browser
        .specs
        .iter()
        .enumerate()
        .filter_map(|(i, spec)| spec.creation_source_id().map(|_| i))
        .collect()
}
fn enter_folder(browser: &mut Browser, view: &mut View) -> Action {
    if !matches!(browser.folder, FolderFilter::Path(_)) {
        return Action::Continue;
    }
    if view.drawer_open {
        view.toggle_drawer();
    }
    view.focus = 1;
    view.git_focus = false;
    browser.query.clear();
    browser.reconcile(0);
    if browser.loading() || browser.diagnostics().iter().any(|source| !source.complete) {
        return Action::Continue;
    }
    let sessions = browser.visible();
    match sessions.as_slice() {
        [] => execute_command(Command::Create, browser, view),
        [session] => {
            browser.selected = Some(session.key.clone());
            execute_command(Command::Resume, browser, view)
        }
        _ => Action::Continue,
    }
}

fn can_create(browser: &Browser) -> bool {
    !browser.loading()
        && matches!(browser.folder, FolderFilter::Path(_))
        && !creation_sources(browser).is_empty()
}

fn resume_hint(browser: &Browser, session: &crate::domain::SessionSummary) -> String {
    if browser
        .specs
        .iter()
        .all(|s| matches!(s, crate::browser::SourceSpec::Fixture(_)))
    {
        return "Fixture data is browse-only.".into();
    }
    if let Some(reason) = crate::domain::resume_blocker(session) {
        return reason.into();
    }
    if let Some(Ok(Some(detail))) = browser.details.get(&session.key) {
        if let Some(reason) = crate::domain::resume_blocker(detail) {
            return reason.into();
        }
    }
    if is_unlinked(browser, session) {
        return relink_unavailable(browser)
            .unwrap_or_else(|| "Folder missing — Enter or m to move session.".into());
    }
    "Enter opens the session; the destination is checked again before launch.".into()
}

fn drawer_shortcut(key: KeyEvent) -> bool {
    matches!(key.code, KeyCode::Char('`' | '~'))
        && key.modifiers.contains(KeyModifiers::ALT)
        && !key
            .modifiers
            .intersects(KeyModifiers::CONTROL | KeyModifiers::SUPER)
}

pub fn handle_key(browser: &mut Browser, view: &mut View, key: KeyEvent) -> Action {
    if let Some(discard) = &mut view.discard {
        return discard.key(key);
    }
    if drawer_shortcut(key) {
        if key.kind == KeyEventKind::Press {
            let mut reminder = view.drawer_reminder.get();
            reminder.learn();
            view.drawer_reminder.set(reminder);
            view.toggle_drawer();
        }
        return Action::Continue;
    }
    view.normalize_focus();
    let action = handle_area_key(browser, view, key);
    view.normalize_focus();
    action
}

fn handle_area_key(browser: &mut Browser, view: &mut View, key: KeyEvent) -> Action {
    if key.kind == KeyEventKind::Release {
        return Action::Continue;
    }
    if key.code == KeyCode::Char('b')
        && key.modifiers == KeyModifiers::CONTROL
        && !view.modal_open()
        && !view.terminals.borrow().has_overlay()
    {
        if key.kind == KeyEventKind::Press {
            return execute_command(Command::BackgroundTerminal, browser, view);
        }
        return Action::Continue;
    }
    if view.drawer_open && key.modifiers == KeyModifiers::ALT {
        let command = match key.code {
            KeyCode::Up => Some(Command::PaneUp),
            KeyCode::Down => Some(Command::PaneDown),
            KeyCode::Left => Some(Command::PaneLeft),
            KeyCode::Right => Some(Command::PaneRight),
            _ => None,
        };
        if let Some(command) = command {
            // These keys belong to pane navigation, never to a dialog's arrows.
            if view.modal_open() {
                return Action::Continue;
            }
            return execute_command(command, browser, view);
        }
    }
    if let Some(dialog) = view.terminal_recovery.as_mut() {
        use crate::terminal_recovery::Event;
        return match dialog.key(key) {
            Event::None => Action::Continue,
            Event::Cancel => {
                view.terminal_recovery = None;
                view.relink = None;
                Action::Continue
            }
            Event::Refresh => Action::RefreshTerminals,
            Event::Stop => Action::StopTerminals,
            Event::Retry => {
                view.terminal_recovery = None;
                Action::ApplyRelink
            }
        };
    }
    if let Some(dialog) = view.error_dialog.as_mut() {
        match dialog.key(key) {
            crate::error_dialog::Event::Copy => return Action::CopyErrorReport,
            crate::error_dialog::Event::Close => view.error_dialog = None,
            crate::error_dialog::Event::None => {}
        }
        return Action::Continue;
    }
    if let Some(mut picker) = view.worktree_picker.take() {
        match key.code {
            KeyCode::Esc => return Action::Continue,
            KeyCode::PageUp | KeyCode::PageDown | KeyCode::Home | KeyCode::End => {
                picker.selected = list_index(
                    key.code,
                    picker.selected,
                    picker.matches.len(),
                    picker.page_size.get(),
                );
            }
            KeyCode::Up | KeyCode::Char('k') => picker.selected = picker.selected.saturating_sub(1),
            KeyCode::Down | KeyCode::Char('j') => {
                picker.selected = (picker.selected + 1).min(picker.matches.len().saturating_sub(1))
            }
            KeyCode::Enter if key.kind != KeyEventKind::Repeat => {
                return select_git_worktree(browser, view, &picker);
            }
            _ => {}
        }
        view.worktree_picker = Some(picker);
        return Action::Continue;
    }
    if let Some(modal) = view.worktree_create.as_mut() {
        if modal.key(key) {
            view.worktree_create = None;
        }
        return Action::Continue;
    }
    if let Some(info) = view.commit_info.as_mut() {
        match info.key(key) {
            crate::error_dialog::Event::Copy => info.copy_report(),
            crate::error_dialog::Event::Close => view.commit_info = None,
            crate::error_dialog::Event::None => {}
        }
        return Action::Continue;
    }
    if let Some(info) = view.info.as_mut() {
        match info.dialog.key(key) {
            crate::error_dialog::Event::Copy => return Action::CopyInformation,
            crate::error_dialog::Event::Close => {
                view.info = None;
                return Action::CloseInformation;
            }
            crate::error_dialog::Event::None => return Action::Continue,
        }
    }
    if let Some(modal) = view.folder_move.as_mut() {
        return modal.key(key);
    }
    if let Some(modal) = view.relink.as_mut() {
        if modal.busy {
            return if !modal.executing && key.code == KeyCode::Esc {
                Action::CancelRelink
            } else {
                Action::Continue
            };
        }
        if modal.preview.is_none()
            && modal.picker.as_ref().is_some_and(|p| p.focus == 3)
            && matches!(key.code, KeyCode::Enter | KeyCode::Char(' '))
        {
            return Action::CancelRelink;
        }
        if modal.preview.is_some() {
            match key.code {
                KeyCode::Up | KeyCode::PageUp => {
                    view.relink_scroll = view.relink_scroll.saturating_sub(1);
                    return Action::Continue;
                }
                KeyCode::Down | KeyCode::PageDown => {
                    view.relink_scroll = view
                        .relink_scroll
                        .saturating_add(1)
                        .min(view.relink_scroll_limit.get());
                    return Action::Continue;
                }
                KeyCode::Home => {
                    view.relink_scroll = 0;
                    return Action::Continue;
                }
                KeyCode::End => {
                    view.relink_scroll = view.relink_scroll_limit.get();
                    return Action::Continue;
                }
                _ => {}
            }
        }
        if modal.preview.is_some() && matches!(key.code, KeyCode::Tab | KeyCode::BackTab) {
            view.relink_back = !view.relink_back;
            return Action::Continue;
        }
        if modal.preview.is_some()
            && view.relink_back
            && matches!(key.code, KeyCode::Enter | KeyCode::Char(' '))
        {
            return Action::CancelRelink;
        }
        if modal.preview.is_none() && key.code != KeyCode::Esc {
            if let Some(picker) = modal.picker.as_mut() {
                modal.error = None;
                return if picker.key(&mut modal.input, key) {
                    Action::PreviewRelink
                } else {
                    Action::Continue
                };
            }
        }
        match key.code {
            KeyCode::Esc => view.relink = None,
            KeyCode::Char('u') if modal.recovery => return Action::RecoverRelink,
            KeyCode::Enter | KeyCode::Char(' ')
                if key.code == KeyCode::Enter || modal.preview.is_some() =>
            {
                return if modal.preview.is_some() {
                    Action::ApplyRelink
                } else {
                    Action::PreviewRelink
                };
            }
            KeyCode::Backspace if modal.preview.is_none() => {
                modal.input.pop();
            }
            KeyCode::Char(c)
                if modal.preview.is_none() && !key.modifiers.contains(KeyModifiers::CONTROL) =>
            {
                modal.input.push(c)
            }
            _ => {}
        }
        return Action::Continue;
    }
    if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
        return Action::Quit;
    }
    if view.preparing {
        return match key.code {
            KeyCode::Esc => Action::CancelResume,
            KeyCode::Char('q') => Action::Quit,
            _ => Action::Continue,
        };
    }
    if let Some(selected) = view.source_picker.as_mut() {
        if key.kind == KeyEventKind::Repeat && key.code == KeyCode::Enter {
            return Action::Continue;
        }
        let sources = creation_sources(browser);
        match key.code {
            KeyCode::Tab | KeyCode::BackTab => crate::modal::cycle(
                &mut view.source_control,
                &[true, true, !sources.is_empty()],
                key.code == KeyCode::BackTab,
            ),
            KeyCode::Enter | KeyCode::Char(' ') if view.source_control == 1 => {
                view.source_picker = None
            }
            KeyCode::Char(' ') if view.source_control == 2 => {
                let source = sources.get(*selected).copied();
                view.source_picker = None;
                if let Some(source) = source {
                    return Action::CreateNew(source);
                }
            }
            KeyCode::Esc => view.source_picker = None,
            KeyCode::Up => *selected = selected.saturating_sub(1),
            KeyCode::Down => *selected = (*selected + 1).min(sources.len().saturating_sub(1)),
            KeyCode::PageUp | KeyCode::PageDown | KeyCode::Home | KeyCode::End => {
                *selected = list_index(
                    key.code,
                    *selected,
                    sources.len(),
                    view.source_page_size.get(),
                );
            }
            KeyCode::Enter => {
                let source = sources.get(*selected).copied();
                view.source_picker = None;
                if let Some(source) = source {
                    return Action::CreateNew(source);
                }
            }
            _ => {}
        }
        return Action::Continue;
    }
    if view.help {
        let entries = filtered_menu_entries(browser, view);
        view.help_selected = view.help_selected.min(entries.len().saturating_sub(1));
        let selected_command = entries.get(view.help_selected).map(|entry| entry.command);
        let action_enabled =
            selected_command.is_some_and(|command| unavailable(command, browser, view).is_none());
        if view.help_searching {
            let edited = match key.code {
                KeyCode::Esc => {
                    view.help_searching = false;
                    return Action::Continue;
                }
                KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    view.help_query.clear();
                    true
                }
                KeyCode::Backspace => {
                    view.help_query.pop();
                    true
                }
                KeyCode::Char(c)
                    if !key.modifiers.intersects(
                        KeyModifiers::CONTROL | KeyModifiers::ALT | KeyModifiers::SUPER,
                    ) =>
                {
                    view.help_query.push(c);
                    true
                }
                _ => false,
            };
            if edited {
                view.help_selected = 0;
                view.help_control = 0;
                return Action::Continue;
            }
        }
        match key.code {
            KeyCode::Char('/') if !view.help_searching => {
                view.help_searching = true;
                view.help_control = 0;
            }
            KeyCode::Tab | KeyCode::BackTab => crate::modal::cycle(
                &mut view.help_control,
                &[true, true, action_enabled],
                key.code == KeyCode::BackTab,
            ),
            KeyCode::Enter | KeyCode::Char(' ') if view.help_control == 1 => view.help = false,
            KeyCode::Esc | KeyCode::Char('?') => view.help = false,
            KeyCode::Up | KeyCode::Char('k') => {
                view.help_selected = view.help_selected.saturating_sub(1)
            }
            KeyCode::Down | KeyCode::Char('j') => {
                view.help_selected = (view.help_selected + 1).min(entries.len().saturating_sub(1))
            }
            KeyCode::PageUp | KeyCode::PageDown | KeyCode::Home | KeyCode::End => {
                view.help_selected =
                    view.help_page
                        .borrow()
                        .select(key.code, view.help_selected, entries.len());
            }
            KeyCode::Enter | KeyCode::Char(' ') if key.kind != KeyEventKind::Repeat => {
                if let Some(command) = selected_command.filter(|_| action_enabled) {
                    view.help = false;
                    return execute_command(command, browser, view);
                }
            }
            _ => {}
        }
        return Action::Continue;
    }
    if view.editing {
        let results = browser.search_results(&view.search_query);
        let index = results
            .iter()
            .position(|s| Some(&s.key) == view.search_selected.as_ref())
            .unwrap_or(0);
        match key.code {
            KeyCode::Tab | KeyCode::BackTab => {
                // Follow the visual order: query, deep search, Cancel, Open.
                let controls = [0, 3, 1, 2];
                let mut index = controls
                    .iter()
                    .position(|c| *c == view.search_control)
                    .unwrap_or(0);
                crate::modal::cycle(
                    &mut index,
                    &[true, true, true, !results.is_empty()],
                    key.code == KeyCode::BackTab || key.modifiers.contains(KeyModifiers::SHIFT),
                );
                view.search_control = controls[index];
            }
            KeyCode::Char('d') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                browser.deep_search = !browser.deep_search;
                view.search_selected = None;
            }
            KeyCode::Enter | KeyCode::Char(' ') if view.search_control == 3 => {
                browser.deep_search = !browser.deep_search;
                view.search_selected = None;
            }
            KeyCode::Enter | KeyCode::Char(' ') if view.search_control == 1 => view.editing = false,
            KeyCode::Esc => view.editing = false,
            KeyCode::Up
            | KeyCode::Down
            | KeyCode::PageUp
            | KeyCode::PageDown
            | KeyCode::Home
            | KeyCode::End => {
                let next = view
                    .search_page
                    .borrow()
                    .select(key.code, index, results.len());
                view.search_selected = results.get(next).map(|s| s.key.clone());
            }
            KeyCode::Enter | KeyCode::Char(' ')
                if key.code == KeyCode::Enter || view.search_control == 2 =>
            {
                if let Some(session) = results.get(index) {
                    browser.query.clear();
                    if let Some(path) = session.cwd.as_ref().filter(|path| path.is_absolute()) {
                        browser.open_folder(path.clone());
                    } else {
                        browser.select_folder(FolderFilter::Unknown);
                    }
                    browser.selected = Some(session.key.clone());
                    browser.reconcile(0);
                    view.focus = 1;

                    view.message = None;
                    view.editing = false;
                }
            }
            KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                view.search_query.clear();
                view.search_selected = None;
            }
            KeyCode::Backspace => {
                view.search_query.pop();
                view.search_selected = None;
            }
            KeyCode::Char(c)
                if view.search_control == 0
                    && !key
                        .modifiers
                        .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
            {
                view.search_query.push(c);
                view.search_selected = None;
            }
            _ => {}
        }
        return Action::Continue;
    }
    if view.git_zoomed && view.git_details_focused && key.modifiers == KeyModifiers::NONE {
        if matches!(
            key.code,
            KeyCode::Esc | KeyCode::Backspace | KeyCode::Char('i')
        ) {
            if key.kind == KeyEventKind::Press {
                view.git_details_focused = false;
            }
            return Action::Continue;
        }
        if matches!(
            key.code,
            KeyCode::Left
                | KeyCode::Right
                | KeyCode::Up
                | KeyCode::Down
                | KeyCode::PageUp
                | KeyCode::PageDown
                | KeyCode::Home
                | KeyCode::End
                | KeyCode::Char('1' | '2' | '3' | 'j' | 'k' | 'c' | 'y')
        ) {
            if let Some(info) = view.git_details.as_mut() {
                if matches!(info.key(key), crate::error_dialog::Event::Copy) {
                    info.copy_report();
                }
            }
            return Action::Continue;
        }
    }
    if view.git_zoomed
        && key.modifiers == KeyModifiers::NONE
        && matches!(key.code, KeyCode::Esc | KeyCode::Backspace)
    {
        if key.kind == KeyEventKind::Press {
            return execute_command(Command::GitReturn, browser, view);
        }
        return Action::Continue;
    }
    if view.drawer_open && key.code == KeyCode::Right && key.modifiers == KeyModifiers::NONE {
        if key.kind == KeyEventKind::Press {
            view.toggle_drawer();
        }
        return Action::Continue;
    }
    if view.drawer_open && key.code == KeyCode::Esc {
        view.toggle_drawer();
        return Action::Continue;
    }
    if matches!(key.code, KeyCode::Tab | KeyCode::BackTab) {
        let command = if key.code == KeyCode::BackTab || key.modifiers.contains(KeyModifiers::SHIFT)
        {
            Command::CyclePaneBack
        } else {
            Command::CyclePane
        };
        return execute_command(command, browser, view);
    }
    if key.code == KeyCode::Char('z')
        && key.modifiers == KeyModifiers::ALT
        && matches!(view.context(), 0 | 2)
    {
        if key.kind == KeyEventKind::Press {
            let command = if view.git_focus {
                Command::GitZoom
            } else {
                Command::FolderFocus
            };
            return execute_command(command, browser, view);
        }
        return Action::Continue;
    }
    if key.modifiers.contains(KeyModifiers::ALT) {
        return Action::Continue;
    }
    if view.git_focus {
        let command = match key.code {
            KeyCode::Char('b') => Command::GitToggle,
            KeyCode::Esc => Command::GitReturn,
            KeyCode::Char('g') => Command::OpenTerminals,
            KeyCode::Char('x') if view.focus == 1 => Command::CloseTerminal,
            KeyCode::Char('q') => Command::Quit,
            KeyCode::Char('r') => Command::Refresh,
            KeyCode::Char('?') => Command::Help,
            _ if view.git_hidden || view.git_size_limited || view.folder_navigation.pending() => {
                return Action::Continue;
            }
            KeyCode::Enter if key.kind != KeyEventKind::Repeat => Command::GitSelectWorktree,
            KeyCode::Char('w') => Command::CreateWorktree,
            KeyCode::Char('i') => Command::CommitInfo,
            KeyCode::Up | KeyCode::Char('k') => Command::GitUp,
            KeyCode::Down | KeyCode::Char('j') => Command::GitDown,
            KeyCode::Left | KeyCode::Char('h') => Command::GitLeft,
            KeyCode::Char('l') => Command::GitRight,
            KeyCode::PageUp => Command::GitPageUp,
            KeyCode::PageDown => Command::GitPageDown,
            KeyCode::Home => Command::GitHome,
            KeyCode::End => Command::GitEnd,
            KeyCode::Char('f') => Command::GitFollow,
            _ => return Action::Continue,
        };
        return execute_command(command, browser, view);
    }
    let command = match key.code {
        KeyCode::Char('?') => {
            open_help(browser, view);
            return Action::Continue;
        }
        KeyCode::Char('g') => Command::OpenTerminals,
        KeyCode::Char('x') if view.focus == 1 => Command::CloseTerminal,
        KeyCode::Char('q') => Command::Quit,
        KeyCode::Char('/') => Command::Search,
        KeyCode::Char('r') => Command::Refresh,
        KeyCode::Char('i') if !view.git_focus => Command::Info,
        KeyCode::Char('n') if view.focus == 1 => Command::Create,
        KeyCode::Char('n') if view.focus == 0 => Command::Create,
        KeyCode::Char('m') if view.focus == 1 => Command::Relink,
        KeyCode::Char('b') if view.focus == 0 => Command::FolderToggle,
        KeyCode::Char('m') if view.focus == 0 => Command::MoveFolder,
        KeyCode::Char('w') if view.focus == 0 => Command::CreateWorktree,
        KeyCode::Delete
            if view.focus == 0 && key.kind == KeyEventKind::Press && key.modifiers.is_empty() =>
        {
            Command::DiscardWorktree
        }
        KeyCode::Char('D') if view.discard_recovery_available => Command::RecoverDiscard,
        KeyCode::Char('u') => Command::Recover,
        KeyCode::Char('a') => Command::Archives,
        KeyCode::Char('l') if view.focus == 0 => Command::FolderRight,
        KeyCode::Left | KeyCode::Char('h') if view.focus == 0 => Command::FolderLeft,
        KeyCode::Left if view.focus == 1 => Command::CyclePane,
        KeyCode::Backspace if view.focus == 1 => Command::PreviousPane,
        KeyCode::Esc => Command::PreviousPane,
        KeyCode::Up | KeyCode::Char('k') => Command::PreviousItem,
        KeyCode::Down | KeyCode::Char('j') => Command::NextItem,
        KeyCode::PageUp => Command::PageUp,
        KeyCode::PageDown => Command::PageDown,
        KeyCode::Home => Command::FirstItem,
        KeyCode::End => Command::LastItem,
        KeyCode::Enter if key.kind == KeyEventKind::Repeat => return Action::Continue,
        KeyCode::Enter if view.focus == 0 => Command::EnterFolder,
        KeyCode::Enter
            if view.focus == 1 && can_create(browser) && browser.visible().is_empty() =>
        {
            Command::Create
        }
        KeyCode::Enter => Command::Resume,
        _ => return Action::Continue,
    };
    execute_command(command, browser, view)
}

fn execute_command(command: Command, browser: &mut Browser, view: &mut View) -> Action {
    if git_preview_blocked(browser, view)
        && matches!(
            command,
            Command::GitSelectWorktree
                | Command::CommitInfo
                | Command::GitUp
                | Command::GitDown
                | Command::GitPageUp
                | Command::GitPageDown
                | Command::GitHome
                | Command::GitEnd
                | Command::GitFollow
                | Command::GitLeft
                | Command::GitRight
        )
    {
        return Action::Continue;
    }
    if git_preview_blocked(browser, view) && view.git_focus && command == Command::CreateWorktree {
        return Action::Continue;
    }
    if view.context() == 0 && view.folders_hidden && !available_when_folders_hidden(command) {
        return Action::Continue;
    }
    let previous_context = view.context();
    match command {
        Command::ToggleDrawer => view.toggle_drawer(),
        Command::BackgroundTerminal => view.terminals.borrow_mut().background(),
        Command::GitSelectWorktree => {
            if view.git_hidden || view.git_size_limited {
                return Action::Continue;
            }
            if let Some(annotation) = selected_git_session(browser, view) {
                let session = annotation.session;
                view.session_preview
                    .borrow_mut()
                    .promote(&mut view.terminals.borrow_mut(), &session.key);
                if let Some(cwd) = session.cwd {
                    if let Some(main) = view
                        .history
                        .repository()
                        .as_ref()
                        .and_then(|id| browser.repositories.get(id))
                        .and_then(|r| r.worktrees.first())
                    {
                        browser
                            .expanded
                            .extend(main.path.ancestors().map(std::path::PathBuf::from));
                    }
                    browser.query.clear();
                    browser.open_folder(cwd);
                    browser.selected = Some(session.key);
                    browser.reconcile(0);
                    view.git_focus = false;
                    view.focus = 1;
                    view.message = None;
                    return execute_command(Command::Resume, browser, view);
                }
                return Action::Continue;
            }
            if let Some(stash) = view
                .history
                .rows
                .get(view.history.selected)
                .and_then(|row| row.stash.as_ref())
            {
                view.message = Some(format!(
                    "Saved stash {} · based on {} · shown read-only",
                    stash.selectors.join(", "),
                    &stash.base[..stash.base.len().min(7)]
                ));
                return Action::Continue;
            }
            let Some(repository) = view.history.repository() else {
                return Action::Continue;
            };
            let Some(oid) = view
                .history
                .rows
                .get(view.history.selected)
                .and_then(|row| row.oid.clone())
            else {
                return Action::Continue;
            };
            match matching_worktrees(browser, &repository, &oid) {
                Ok(matches) if matches.is_empty() => {
                    view.message = Some("No worktree is checked out at this commit.".into())
                }
                Ok(matches) => {
                    let picker = WorktreePicker {
                        repository,
                        oid,
                        matches,
                        selected: 0,
                        page_size: Default::default(),
                    };
                    if picker.matches.len() == 1 {
                        return select_git_worktree(browser, view, &picker);
                    } else {
                        view.message = None;
                        view.worktree_picker = Some(picker);
                    }
                }
                Err(message) => view.message = Some(message),
            }
        }
        Command::DiscardWorktree => {
            if crate::worktree_discard_ui::selected(browser).is_some() {
                return Action::OpenDiscard;
            }
        }
        Command::RecoverDiscard => return Action::RecoverDiscard,
        Command::CreateWorktree => {
            if worktree_unavailable(browser, view).is_some() {
                return Action::Continue;
            }
            if let FolderFilter::Path(path) = &browser.folder {
                let commit = if view.git_focus {
                    view.history
                        .rows
                        .get(view.history.selected)
                        .and_then(|row| row.oid.clone())
                } else {
                    None
                };
                view.worktree_create =
                    Some(crate::worktree_create_ui::Modal::new(path.clone(), commit));
            }
        }

        Command::FolderToggle | Command::GitToggle => {}
        Command::GitZoom => {
            if view.git_focus && view.history.present() {
                view.git_zoomed = !view.git_zoomed;
            }
        }
        Command::GitReturn => {
            if view.git_details_focused {
                view.git_details_focused = false;
            } else if view.git_zoomed {
                view.git_zoomed = false;
            } else if view.drawer_open {
                view.toggle_drawer();
            }
        }
        Command::GitLeft => view.history.scroll_horizontal(false),
        Command::GitRight => view.history.scroll_horizontal(true),
        Command::GitFollow => {
            view.git_sessions.borrow_mut().follow();
            view.history.follow();
        }
        Command::GitUp
        | Command::GitDown
        | Command::GitPageUp
        | Command::GitPageDown
        | Command::GitHome
        | Command::GitEnd => {
            use crate::git_history::Movement;
            let mut presentation = view.git_sessions.borrow_mut();
            presentation.sync(browser, &view.history);
            presentation.navigate(
                match command {
                    Command::GitUp => Movement::Up,
                    Command::GitDown => Movement::Down,
                    Command::GitPageUp => Movement::PageUp,
                    Command::GitPageDown => Movement::PageDown,
                    Command::GitHome => Movement::Home,
                    _ => Movement::End,
                },
                &mut view.history,
            );
        }

        Command::Help => {
            open_help(browser, view);
        }
        Command::CommitDetailsKey(code) => {
            if let Some(info) = view.git_details.as_mut() {
                if matches!(
                    info.key(KeyEvent::new(code, KeyModifiers::NONE)),
                    crate::error_dialog::Event::Copy
                ) {
                    info.copy_report();
                }
            }
        }
        Command::CommitInfo => {
            if let Some((repository, oid)) = selected_commit(browser, view) {
                if view.git_zoomed {
                    sync_git_details(browser, view);
                    view.git_details_focused = !view.git_details_focused;
                } else {
                    view.commit_info = Some(crate::commit_info::Dialog::new(repository, oid));
                }
            } else {
                view.message = Some("Select a visible Git commit first.".into());
            }
        }
        Command::Info => {
            if view.focus == 0 {
                if let Some(path) = selected_checkout(browser) {
                    view.error_dialog = Some(crate::error_dialog::ErrorDialog::information(
                        "Worktree information",
                        format!(
                            "{}\n\nUncommitted changes: {}\n\n* marks staged, unstaged, or untracked changes. Ignored files are excluded.",
                            path.display(),
                            worktree_status_description(browser, path)
                        ),
                    ));
                }
            } else if let Some(session) = selected_session(browser) {
                view.info = Some(InfoModal::new(browser, session));
            }
        }
        Command::OpenTerminals => {
            view.git_zoomed = false;
            view.terminals.borrow_mut().picker = true;
        }
        Command::CloseTerminal => view
            .terminals
            .borrow_mut()
            .request_close(browser.selected.as_ref()),
        Command::Quit => return Action::Quit,
        Command::Relink => return Action::OpenRelink,
        Command::MoveFolder => return Action::OpenFolderMove,
        Command::Recover => {
            if view.recovery_available {
                return Action::RecoverRelink;
            }
        }
        Command::Search => {
            view.search_page.get_mut().clear();
            view.editing = true;
            view.search_control = 0;
            view.search_query.clear();
            view.search_selected = None;
        }
        Command::Refresh => {
            view.git_details_key = None;
            view.git_details = None;
            return Action::Refresh;
        }
        Command::Archives => browser.include_archived = !browser.include_archived,
        Command::CyclePane | Command::CyclePaneBack => {
            if view.drawer_open {
                if view.history.present() {
                    view.drawer_git = !view.drawer_git;
                    view.normalize_focus();
                }
            } else if view.terminals.borrow().active.is_none() {
                view.toggle_drawer();
            }
        }
        Command::PaneUp | Command::PaneDown | Command::PaneLeft | Command::PaneRight => {
            if view.drawer_open && view.history.present() {
                view.drawer_git = match command {
                    Command::PaneUp | Command::PaneLeft => false,
                    Command::PaneDown | Command::PaneRight => true,
                    _ => !view.drawer_git,
                };
                view.normalize_focus();
            }
        }
        Command::EnterFolder => return enter_folder(browser, view),
        Command::PreviousPane => {
            view.message = None;
        }
        Command::PreviousItem | Command::NextItem | Command::PageUp | Command::PageDown => {
            let direction = if matches!(command, Command::PreviousItem | Command::PageUp) {
                -1
            } else {
                1
            };
            let page = match command {
                Command::PageUp | Command::PageDown if view.focus == 0 => {
                    view.folder_page_size.get()
                }
                Command::PageUp | Command::PageDown => view.session_page_size.get(),
                _ => 1,
            };
            let delta = direction * page.max(1) as isize;
            match view.focus {
                0 => {
                    let rows = view.folder_rows(browser);
                    let i = rows
                        .iter()
                        .position(|r| r.filter == browser.folder)
                        .unwrap_or(0);
                    if let Some(row) = rows.get(
                        i.saturating_add_signed(delta)
                            .min(rows.len().saturating_sub(1)),
                    ) {
                        browser.select_folder(row.filter.clone());
                    }
                }
                1 => browser.move_selection(delta),
                _ => {}
            }
        }
        Command::FirstItem | Command::LastItem => {
            let first = command == Command::FirstItem;
            match view.focus {
                0 => {
                    let rows = view.folder_rows(browser);
                    if let Some(row) = if first { rows.first() } else { rows.last() } {
                        browser.select_folder(row.filter.clone());
                    }
                }
                1 => {
                    let sessions = browser.visible();
                    browser.selected = (if first {
                        sessions.first()
                    } else {
                        sessions.last()
                    })
                    .map(|session| session.key.clone());
                }
                _ => {}
            }
        }
        Command::FolderFocus => browser.toggle_folder_focus(),
        Command::FolderLeft | Command::FolderRight => {
            let FolderFilter::Path(path) = browser.folder.clone() else {
                return Action::Continue;
            };
            let rows = browser.folder_rows();
            let Some(index) = rows.iter().position(|row| row.filter == browser.folder) else {
                return Action::Continue;
            };
            let depth = rows[index].depth;
            if command == Command::FolderRight {
                browser.expanded.insert(path);
            } else if !browser.expanded.remove(&path) {
                if let Some(parent) = rows[..index]
                    .iter()
                    .rev()
                    .find(|row| row.depth < depth && matches!(row.filter, FolderFilter::Path(_)))
                {
                    browser.select_folder(parent.filter.clone());
                }
            }
        }
        Command::Create => {
            if let Some(reason) = unavailable(Command::Create, browser, view) {
                view.message = Some(reason);
                return Action::Continue;
            }
            if view.drawer_open {
                view.toggle_drawer();
            }
            view.focus = 1;
            view.creation_folder = match &browser.folder {
                FolderFilter::Path(path) => Some(path.clone()),
                _ => None,
            };
            let sources = creation_sources(browser);
            if sources.len() == 1 {
                return Action::CreateNew(sources[0]);
            }
            view.source_picker = Some(0);
            view.source_page_size.set(0);
            view.source_control = 0;
        }
        Command::Resume => {
            if view.drawer_open {
                view.toggle_drawer();
            }
            if selected_session(browser).is_some_and(|s| {
                crate::domain::classify_folder(s.cwd.as_deref())
                    == crate::domain::FolderStatus::Unlinked
            }) {
                return Action::OpenRelink;
            }
            return Action::ResumeSelected;
        }
    }
    if previous_context != 0 && view.context() == 0 {
        view.folders_hidden = false;
    }
    browser.reconcile(0);
    Action::Continue
}

struct PendingResume {
    origin: FolderFilter,
    task: tokio::task::JoinHandle<Result<crate::domain::ResumeCommand, String>>,
    observed_cwd: Option<std::path::PathBuf>,
}
impl Drop for PendingResume {
    fn drop(&mut self) {
        self.task.abort();
    }
}

fn save_folder(browser: &mut Browser, folder: Option<&std::path::Path>, view: &mut View) {
    if let (Some(file), Some(folder)) = (&browser.state_path, folder) {
        if let Err(error) = crate::state::save_context(
            file,
            folder,
            browser
                .launch_folder
                .as_deref()
                .map(|launch| (launch, browser.focus_root.as_deref())),
        ) {
            let warning = format!("Could not save browser state: {error}");
            view.message = Some(warning.clone());
            browser.state_warning = Some(warning);
        }
    }
}

pub async fn run(mut browser: Browser) -> Result<(), Box<dyn std::error::Error>> {
    if !io::stdin().is_terminal() || !io::stdout().is_terminal() {
        return Err("interactive browsing requires a terminal; use 'tracr list'".into());
    }
    let mut guard = TerminalGuard {
        active: false,
        mouse: false,
    };
    guard.activate()?;
    let mut input_reader = crate::terminal_input::Reader::default();
    let mut outer_focused = true;
    let mut terminal_copy: Option<tokio::task::JoinHandle<std::result::Result<(), String>>> = None;
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        restore();
        previous(info);
    }));
    let mut terminal = Terminal::new(CrosstermBackend::new(io::stdout()))?;
    let mut content_search = crate::search::Search::default();
    let mut discovery = Discovery::new();
    discovery.enable_live();
    browser.invalidate_git();
    discovery.refresh(&mut browser);
    let mut view = View {
        drawer_open: true,
        dialog_area: true,
        focus: 0,
        message: browser.state_warning.take(),
        ..View::default()
    };
    if let Some(path) = browser.state_path.as_deref() {
        match crate::state::drawer_shortcut_learned(path) {
            Ok(true) => {
                let mut reminder = view.drawer_reminder.get();
                reminder.learn();
                view.drawer_reminder.set(reminder);
            }
            Ok(false) => {}
            Err(error) => {
                let warning = format!("Could not read drawer shortcut preference: {error}");
                view.message = Some(warning.clone());
                browser.state_warning = Some(warning);
            }
        }
    }
    let signals = ShutdownSignals::new()?;
    let mut dev_reload = crate::dev_reload::DevReload::new()?;
    let mut reloading = false;
    // Task slots use the same main/drawer index as dialog_area.
    let mut pending: [Option<PendingResume>; 2] = [None, None];
    let mut relink_task: [Option<tokio::task::JoinHandle<RelinkResult>>; 2] = [None, None];
    let mut folder_task: [Option<tokio::task::JoinHandle<FolderMoveResult>>; 2] = [None, None];
    let (relink_tx, mut relink_rx): (Vec<_>, Vec<_>) = (0..2)
        .map(|_| tokio::sync::mpsc::unbounded_channel::<String>())
        .unzip();
    let (folder_tx, mut folder_rx): (Vec<_>, Vec<_>) = (0..2)
        .map(|_| tokio::sync::mpsc::unbounded_channel::<String>())
        .unzip();
    let mut folder_relocated: Option<(std::path::PathBuf, Option<crate::domain::SessionKey>)> =
        None;
    let launch_dir = std::env::current_dir()?;
    let relocation_state = browser
        .state_path
        .as_ref()
        .and_then(|p| p.parent())
        .map(|p| p.to_path_buf());
    if let Some((count, folder)) = refresh_recovery(relocation_state.as_deref(), &mut view) {
        if folder {
            view.message = Some("Unfinished folder move. Press u to inspect/recover.".into());
        } else if count > 0 {
            view.message = Some(format!(
                "{count} unfinished relocation(s). Press u to inspect/recover."
            ));
        }
    }
    if let Some(state) = &relocation_state {
        match crate::worktree_discard::pending(state) {
            Ok(Some(journal)) => {
                view.discard_recovery_available = true;
                view.message = Some(format!(
                    "Unfinished discard. Press D to review. {}",
                    journal.progress()
                ));
            }
            Ok(None) => {}
            Err(error) => view.message = Some(format!("Cannot inspect discard journal: {error}")),
        }
    }
    let mut relocated: Option<crate::relocation::Preview> = None;
    let mut last_folder = browser.initial_folder.clone();
    let mut git_discovery = crate::git_discovery::Controller::default();
    let mut status_monitor = crate::worktree_status::Monitor::default();
    let mut git_watch = crate::git_watch::Monitor::default();
    let mut terminal_git_watch = crate::git_watch::Monitor::default();
    let mut terminal_folder = None;
    let mut history_folder = None;
    let mut candidates_dirty = true;
    let mut candidate_context = None;
    let mut idle = IdleGate::default();
    idle.wake();
    let mut modal_deadline = std::time::Instant::now();
    let mut folder_checks_dirty = true;
    let outcome: Result<(), Box<dyn std::error::Error>> = async {
        loop {
            // A snapshot must never survive discovery, scope, or tree changes.
            view.folder_rows_snapshot = None;
            if let Some(notice) = browser.focus_notice.take() { view.message = Some(notice); idle.wake(); }
            // Terminal output and copy results belong to the coding surface even
            // when the drawer currently owns input.
            view.select_dialog_area(false);
            if terminal_copy.as_ref().is_some_and(|task|task.is_finished()) {
                view.message=Some(match terminal_copy.take().unwrap().await {Ok(Ok(()))=>"Terminal selection copied".into(),Ok(Err(error))=>error,Err(error)=>format!("Copy failed: {error}")});
                idle.wake();
            }
            if terminal_copy.is_none() {
                if let Some(text)=view.terminals.borrow_mut().copy_request.take() {terminal_copy=Some(tokio::spawn(crate::clipboard::copy(text)));}
            }
            if tick_session_preview(&browser, &view) { idle.wake(); }
            let (terminal_changed, terminal_exits) = view.terminals.borrow_mut().poll();
            let unexpected_terminal_failure = terminal_exits.iter().find(|notice| {
                notice.failed && notice.was_active
                    && !view.discard.as_ref().is_some_and(|modal| modal.tracks_terminal(notice.id))
            });
            if let Some(modal) = view.discard.as_mut() {
                if modal.poll_terminals(&view.terminals.borrow(), &terminal_exits, std::time::Instant::now()) {
                    idle.wake();
                }
            }
            if terminal_changed { idle.wake(); }
            if let Some(error)=view.terminals.borrow_mut().input_error.take() {view.message=Some(error);}
            if !terminal_exits.is_empty() {
                view.select_dialog_area(false);
                view.message = Some(terminal_exits.iter().map(|notice| notice.message.as_str()).collect::<Vec<_>>().join("; "));
                if let Some(notice) = unexpected_terminal_failure {
                    view.error_dialog = Some(crate::error_dialog::ErrorDialog::new("Terminal exited", "The session remains available to resume.", notice.message.clone()));
                }
                browser.invalidate_git();
                discovery.refresh(&mut browser);
            }
            view.select_dialog_area(view.drawer_open);
            for _ in 0..100 {
                match discovery.rx.try_recv() {
                    Ok(update) => {
                        if browser.update_changes(&update) {
                            let paths_changed = matches!(&update, crate::browser::Update::Page { .. } | crate::browser::Update::Finished { .. });
                            candidates_dirty |= paths_changed; folder_checks_dirty |= paths_changed;
                            browser.apply(update); idle.wake();
                        }
                    }
                    Err(_) => break,
                }
            }
            let search_active = view.editing && browser.deep_search && !view.search_query.trim().is_empty();
            if content_search.tick(&mut browser, search_active) {
                if view.editing && view.search_selected.is_none() {
                    view.search_selected = browser.search_results(&view.search_query).first().map(|s| s.key.clone());
                }
                idle.wake();
            }
            if status_monitor.tick(&mut browser).await { idle.wake(); }
            let now = std::time::Instant::now();
            let active_folder = view.terminals.borrow().active_folder();
            if terminal_folder != active_folder {
                terminal_folder = active_folder;
                candidates_dirty = true;
                idle.wake();
            }
            let terminal_repository = terminal_folder.as_ref().and_then(|folder| browser.ownership.get(folder)).and_then(|record| record.effective()).and_then(|outcome| match outcome {
                crate::git_discovery::Outcome::Repository { repository, .. } => Some(repository.common_dir.clone()),
                _ => None,
            });
            terminal_git_watch.context(terminal_repository.filter(|id| git_watch.repository() != Some(id)), browser.git_generation, terminal_folder.is_some());
            for watcher in [&mut git_watch, &mut terminal_git_watch] {
                if let Some(change) = watcher.poll(now, git_discovery.pending() || view.history.loading()) {
                    watcher.reconcile();
                    if let Some(id) = watcher.repository() {
                        git_discovery.refresh_repository(id, change == crate::git_watch::Change::Topology);
                        let retry_paths = browser.invalidate_git_repository(id, change == crate::git_watch::Change::Topology);
                        git_discovery.forget(&retry_paths);
                        view.history.request_refresh();
                        idle.wake();
                    }
                }
            }
            if view.git_watch_error != git_watch.error { view.git_watch_error = git_watch.error.clone(); idle.wake(); }
            let copy_ready = view.error_dialog.as_ref().is_some_and(|dialog| dialog.copy_task.as_ref().is_some_and(|task| task.is_finished())) || view.info.as_ref().is_some_and(|info| info.dialog.copy_task.as_ref().is_some_and(|task| task.is_finished()));
            let picker_ready = view.relink.as_ref().is_some_and(|modal| modal.picker.as_ref().is_some_and(|picker| picker.needs_tick()));
            let modal_timer = view.folder_move.is_some() && now >= modal_deadline;
            if modal_timer { modal_deadline = now + Duration::from_millis(100); }
            let resized = signals.resize.swap(false, std::sync::atomic::Ordering::Relaxed);
            let reload_requested = dev_reload.requested();
            let input_waiting = view.folder_navigation.pending() && view.focus == 0 && !view.git_focus && input_reader.poll(Duration::ZERO)?;
            let defer_git = view.folder_navigation.pending() && !view.folder_navigation.ready(now, input_waiting);
            let hidden_busy = view.hidden_dialogs.worktree_create.is_some() || view.hidden_dialogs.folder_move.is_some()
                || view.hidden_dialogs.relink.is_some() || view.hidden_dialogs.commit_info.is_some()
                || view.hidden_dialogs.preparing || view.hidden_dialogs.error_dialog.as_ref().is_some_and(|d| d.copy_task.is_some())
                || view.hidden_dialogs.info.as_ref().is_some_and(|i| i.dialog.copy_task.is_some());
            let hidden_tick = hidden_busy && now >= modal_deadline;
            if hidden_tick { modal_deadline = now + Duration::from_millis(100); }
            let activity = view.discard.as_ref().is_some_and(|modal| modal.ready()) || view.folder_navigation.ready(now, input_waiting) || hidden_tick || view.commit_info.as_ref().is_some_and(|info| info.ready()) || view.git_details.as_ref().is_some_and(|info| info.ready()) || reload_requested || view.worktree_create.as_ref().is_some_and(|modal| modal.ready()) || resized || (!defer_git && (candidates_dirty || git_discovery.needs_poll() || view.history.needs_poll())) || copy_ready || picker_ready || modal_timer
                || pending[usize::from(view.dialog_area)].as_ref().is_some_and(|p| p.task.is_finished())
                || relink_task[usize::from(view.dialog_area)].as_ref().is_some_and(|task| task.is_finished()) || folder_task[usize::from(view.dialog_area)].as_ref().is_some_and(|task| task.is_finished())
                || !relink_rx[usize::from(view.dialog_area)].is_empty() || !folder_rx[usize::from(view.dialog_area)].is_empty()
                || signals.terminate.load(std::sync::atomic::Ordering::Relaxed) || signals.interrupt.load(std::sync::atomic::Ordering::Relaxed);
            if idle.ready(now, activity) {
            if !defer_git {
            git_discovery.generation(browser.git_generation);
            let context_changed = candidate_context.as_ref().is_none_or(|(generation, folder, expanded, archived)| *generation != browser.git_generation || folder != &browser.folder || expanded != &browser.expanded || *archived != browser.include_archived);
            if candidates_dirty || context_changed {
                let mut candidates = Vec::new();
                candidates.extend(terminal_folder.iter().cloned());
                if let FolderFilter::Path(path) = &browser.folder { candidates.push(path.clone()); }
                candidates.extend(browser.folder_rows().into_iter().filter_map(|r| match r.filter { FolderFilter::Path(p) => Some(p), _ => None }));
                candidates.extend(browser.all().into_iter().filter_map(|s| s.cwd));
                candidates.extend(browser.repositories.values().flat_map(|r| r.worktrees.iter().map(|w| w.path.clone())));
                git_discovery.set_candidates(candidates);
                candidate_context = Some((browser.git_generation, browser.folder.clone(), browser.expanded.clone(), browser.include_archived));
                candidates_dirty = false;
            }
            let updates = git_discovery.poll().await;
            candidates_dirty |= !updates.is_empty();
            git_discovery.apply_batch(&mut browser, updates);
            let context = selected_git_context(&browser);
            let pending_context = match &browser.folder {
                FolderFilter::Path(path) => browser.ownership.get(path).is_none_or(|record| record.outcome.is_none() && record.previous.is_none()),
                _ => false,
            };
            let target = selected_git_repository(&browser).cloned();
            let checkout = match context { Some(crate::git_discovery::Outcome::Repository { root, .. }) => root.clone(), _ => None };
            let selected_folder = match &browser.folder { FolderFilter::Path(path) => Some(path.clone()), _ => None };
            let root_selected = selected_folder != history_folder && checkout.is_some() && selected_folder == checkout;
            if !pending_context {
                if selected_folder != history_folder || view.folder_navigation.pending() {
                    view.git_navigation_loading = target.is_some();
                    view.git_navigation_error = false;
                }
                history_folder = selected_folder;
                view.history.checkout(checkout, root_selected).await;
            }
            let size = terminal.size()?;
            let area = Rect::new(0, 0, size.width, size.height);
            view.layout_mode.set(LayoutMode::for_size(size.width, size.height));
            let (_, footer_height) = browser_footer(area, &browser, &view);
            let content_height = size.height.saturating_sub(footer_height);
            let panels = panel_layout(Rect::new(0, 0, size.width, content_height), size.height, &view, target.is_some());
            view.git_size_limited = panels.git.is_none_or(|area| area.height < 4 || area.width < 12);
            let graph_visible = panels.graph_visible;
            let page = usize::from(panels.git.map_or(0, |area| area.height.saturating_sub(3))).max(1);
            git_watch.context(target.clone(), browser.git_generation, graph_visible);
            if target.is_none() {
                view.git_navigation_loading = false;
                view.git_navigation_error = false;
            }
            view.history.tick(target, browser.git_generation, graph_visible, page).await;
            finish_git_navigation(&mut view);
            if let Some(repository) = view.history.repository().filter(|_| !view.history.rows.is_empty()) {
                browser.order_worktrees(repository, view.history.revision(), view.history.rows.iter().filter_map(|row| row.oid.as_deref()));
            }
            let displayed_repository = view.history.repository();
            view.history.set_worktrees(displayed_repository.as_ref().and_then(|id| browser.repositories.get(id)));
            view.history.set_status(displayed_repository.as_ref().and_then(|id| browser.repositories.get(id)), &browser.worktree_status);
            if !view.history.present() { view.drawer_git = false; }
            view.normalize_focus();
            {
                let mut presentation = view.git_sessions.borrow_mut();
                presentation.sync(&browser, &view.history);
                if presentation.session().is_some() {
                    if let Some(anchor) = presentation.anchor() { view.history.selected = anchor; }
                }
            }
            view.folder_navigation.finish();
            }
            // Finish the bounded operation and archive restoration before exiting.
            if (signals.terminate.load(std::sync::atomic::Ordering::Relaxed)
                || signals.interrupt.load(std::sync::atomic::Ordering::Relaxed)) && relink_task.iter().all(Option::is_none) && folder_task.iter().all(Option::is_none) && !view.discard.as_ref().is_some_and(|modal| modal.executing()) && !view.worktree_create.as_ref().is_some_and(|modal| modal.executing) && !view.hidden_dialogs.worktree_create.as_ref().is_some_and(|modal| modal.executing) {
                break;
            }
            // Reload is distinct from termination: never interrupt interactive Codex.
            // Background terminals and mutations must also settle first.
            if reload_requested && view.terminals.borrow().is_empty() && relink_task.iter().all(Option::is_none) && folder_task.iter().all(Option::is_none)
                && !view.discard.as_ref().is_some_and(|modal| modal.executing())
                && !view.worktree_create.as_ref().is_some_and(|modal| modal.executing) && !view.hidden_dialogs.worktree_create.as_ref().is_some_and(|modal| modal.executing) {
                reloading = true;
                break;
            }
            if let Some(modal) = view.discard.as_mut().filter(|modal| modal.ready()) {
                while input_reader.poll(Duration::ZERO)? { let _ = input_reader.read()?; }
                if let Some(result) = modal.tick().await {
                    view.discard = None;
                    let pending = relocation_state.as_deref().map(crate::worktree_discard::pending).transpose();
                    let progress = match pending {
                        Ok(Some(Some(journal))) => {
                            view.discard_recovery_available = true;
                            format!("{} Press D to review remaining cleanup.", journal.progress())
                        }
                        Ok(_) => {
                            view.discard_recovery_available = false;
                            String::new()
                        }
                        Err(error) => format!("Cannot read discard progress: {error}"),
                    };
                    match result {
                        Ok(main) => {
                            if browser.focus_root.is_some() { browser.focus_root = Some(main.clone()); }
                            browser.open_folder(main);
                            view.focus = 0;
                            view.git_focus = false;
                            view.message = Some("Worktree, local branch, and conversations discarded.".into());
                        }
                        Err(error) => {
                            view.error_dialog = Some(crate::error_dialog::ErrorDialog::new(
                                "Discard stopped", "Review the reported state before retrying.", format!("{error}\n{progress}"),
                            ));
                        }
                    }
                    browser.invalidate_git();
                    view.history.request_refresh();
                    candidates_dirty = true;
                    discovery.refresh(&mut browser);
                }
            }
            let visible_area = view.drawer_open;
            // Hidden forms continue polling; their results stay with their owner.
            for owner in [false, true] {
            view.select_dialog_area(owner);
            let worktree_ready = view.worktree_create.as_ref().is_some_and(|modal| modal.ready());
            let worktree_result = if let Some(modal) = view.worktree_create.as_mut() { modal.tick().await } else { None };
            if worktree_ready {
                // Discard queued input after worktree checks or creation complete.
                while view.dialog_area == view.drawer_open && input_reader.poll(Duration::ZERO)? { let _ = input_reader.read()?; }
            }
            if let Some(result) = worktree_result {
                browser.invalidate_git();
                candidates_dirty = true;
                match result {
                    Ok(path) => {
                        browser.open_folder(path);
                        view.worktree_create = None;
                        view.git_focus = false;
                        view.focus = 0;
                        view.folders_hidden = false;
                        view.message = Some("Worktree created. Alt+` closes the drawer to start a session.".into());
                        discovery.refresh(&mut browser);
                    }
                    Err(error) => view.error_dialog = Some(crate::error_dialog::ErrorDialog::new("Couldn't create worktree", "Inspect creation state before retrying", error)),
                }
            }
            while let Ok(message)=relink_rx[usize::from(view.dialog_area)].try_recv() {
                if let Some(dialog)=view.terminal_recovery.as_mut(){dialog.progress=message.clone();}
                if let Some(modal)=view.relink.as_mut(){modal.error=Some(message);}
            }
            while let Ok(message)=folder_rx[usize::from(view.dialog_area)].try_recv() {
                if let Some(modal)=view.folder_move.as_mut(){modal.message=Some(message);}
            }
            if let Some(modal)=view.folder_move.as_mut(){modal.tick();}
            if folder_task[usize::from(view.dialog_area)].as_ref().is_some_and(|t|t.is_finished()) {
                let result=folder_task[usize::from(view.dialog_area)].take().expect("finished folder move").await;
                while view.dialog_area == view.drawer_open && input_reader.poll(Duration::ZERO)?{let _=input_reader.read()?;}
                match result {
                    Ok(FolderMoveResult::Preview(Ok(plan)))=>{
                        if let Some(modal)=view.folder_move.as_mut(){modal.busy=false;modal.message=None;modal.plan=Some(plan);}
                    },
                    Ok(FolderMoveResult::Applied(crate::folder_move::Outcome::Complete(plan)))=>{
                        folder_relocated=Some((plan.target.clone(),browser.selected.clone()));
                        browser.open_folder(plan.target);view.folder_move=None;view.focus=0;
                        view.message=Some("Folder and sessions moved and verified.".into());browser.invalidate_git(); discovery.refresh(&mut browser);
                    },
                    other=>{
                        let (message,recovery)=match other {
                            Ok(FolderMoveResult::Preview(Err(e)))=>(e,false),
                            Ok(FolderMoveResult::Applied(crate::folder_move::Outcome::Failed{message,recovery}))=>(message,recovery),
                            Err(e)=>(format!("Folder move stopped: {e}"),true),
                            _=>unreachable!(),
                        };
                        show_folder_failure(&mut view,message,recovery);
                        browser.invalidate_git(); discovery.refresh(&mut browser);
                    }
                }
                refresh_recovery(relocation_state.as_deref(), &mut view);
            }
            if let Some(info)=view.commit_info.as_mut(){info.tick().await;}
            if let Some(dialog)=view.error_dialog.as_mut(){dialog.tick().await;}
            if let Some(info)=view.info.as_mut(){info.refresh(&browser);info.dialog.tick().await;}
            if relink_task[usize::from(view.dialog_area)].as_ref().is_some_and(|t|t.is_finished()) {
                let task=relink_task[usize::from(view.dialog_area)].take().expect("finished relocation");
                while view.dialog_area == view.drawer_open && input_reader.poll(Duration::ZERO)?{let _=input_reader.read()?;}
                match task.await {
                    Ok(RelinkResult::Terminals(result)) => {
                        let failure = match result {
                            Ok(commands) => crate::providers::codex_terminals::Blocker::Commands(commands).failure(),
                            Err(error) => error,
                        };
                        show_relink_failure(&mut view,failure,false);
                    }
                    Ok(RelinkResult::Preview(result))=>match result {
                        Ok(p)=>if let Some(modal)=view.relink.as_mut(){modal.busy=false;modal.input=p.target.display().to_string();modal.preview=Some(p);view.relink_back=false;view.relink_scroll=0;},
                        Err(e)=>show_relink_failure(&mut view,e,false),
                    },
                    Ok(RelinkResult::Applied(crate::relocation::Outcome::Verified(p)))=>{
                        view.terminal_recovery=None;view.relink=None;view.focus=1;if p.archived {browser.include_archived=true;}browser.open_folder(p.target.clone());
                        browser.selected=Some(p.key.clone());relocated=Some(p);
                        view.message=Some("Session relinked and verified.".into());browser.invalidate_git(); discovery.refresh(&mut browser);
                    },
                    Ok(RelinkResult::Applied(crate::relocation::Outcome::Rejected(e)))=>{
                        show_relink_failure(&mut view,e,false);browser.invalidate_git(); discovery.refresh(&mut browser);
                    },
                    Ok(RelinkResult::Applied(crate::relocation::Outcome::Unresolved(e)))=>{
                        show_relink_failure(&mut view,e,true);browser.invalidate_git(); discovery.refresh(&mut browser);
                    },
                    Err(e)=>{show_relink_failure(&mut view,format!("Relocation stopped: {e}").into(),true);}
                }
                refresh_recovery(relocation_state.as_deref(), &mut view);
            }
            if let FolderFilter::Path(path) = &browser.folder {
                last_folder = Some(path.clone());
            }
            if !browser.loading() {
                if let Some((path,selected))=folder_relocated.take(){
                    browser.open_folder(path);
                    if selected.as_ref().is_some_and(|k|browser.visible().iter().any(|s|&s.key==k)){browser.selected=selected;}
                }
                if let Some(p)=relocated.take(){browser.open_folder(p.target);browser.selected=Some(p.key);}

            }
            if let Some(modal)=view.relink.as_mut(){
                if !modal.busy && modal.preview.is_none(){if let Some(picker)=modal.picker.as_mut(){picker.tick(&mut modal.input);}}
            }
            if let Some(modal)=view.folder_move.as_mut(){ modal.monitor(&browser); }
            if folder_checks_dirty || browser.loading() { discovery.check_folders(&browser); folder_checks_dirty = false; }
            if pending.iter().all(Option::is_none) && relink_task.iter().all(Option::is_none) {
                discovery.detail(&browser);
            }
            if pending[usize::from(view.dialog_area)].as_ref().is_some_and(|p| p.task.is_finished()) {
                let mut ready = pending[usize::from(view.dialog_area)].take().expect("finished preparation");
                view.preparing = false;
                // An Enter queued during preparation must not approve a changed folder.
                while view.dialog_area == view.drawer_open && input_reader.poll(Duration::ZERO)? {
                    let _ = input_reader.read()?;
                }
                match (&mut ready.task).await {
                    Ok(Ok(command)) => {
                        browser.details.insert(
                            command.session.key.clone(),
                            Ok(Some(command.session.clone())),
                        );
                        if ready.observed_cwd.as_ref() != Some(&command.cwd) {
                            view.focus = 1;
                            let mut info = InfoModal::new(&browser, command.session.clone());
                            info.dialog.title = "Session folder changed".into();
                            info.warning = Some(format!("Previously displayed folder: {}\nCurrent resume folder: {}\n\nResume was stopped. Close this dialog, then press Enter in Sessions to request resume in the updated folder. The destination will be checked again.", ready.observed_cwd.as_ref().map(|p|p.display().to_string()).unwrap_or_else(||"Unknown".into()), command.cwd.display()));
                            info.refresh(&browser);
                            view.info = Some(info);
                            view.message = Some(format!(
                                "Resume folder changed to {}. Review it, then press Enter again.",
                                command.cwd.display()
                            ));
                        } else {
                            save_folder(&mut browser, last_folder.as_deref(), &mut view);
                            let result = view.terminals.borrow_mut().launch(&command.launch, Some(command.session.key.clone()), command.session.title.clone());
                            if result.is_ok() && browser.folder == ready.origin {
                                browser.open_folder(command.cwd.clone());
                                view.terminals.borrow_mut().sync_folder(Some(&command.cwd));
                                view.focus = 1;
                                view.git_focus = false;
                            }
                            if let Err(error) = result { view.message = Some(format!("Could not resume Codex: {error}")); }
                        }
                    }
                    Ok(Err(error)) => view.message = Some(error),
                    Err(error) => {
                        view.message = Some(format!("Resume preparation failed: {error}"))
                    }
                }
            }
            }
            view.select_dialog_area(visible_area);
            view.normalize_focus();
            sync_terminal_view(&browser, &view);
            sync_git_details(&browser, &mut view);
            if let Some(info) = view.git_details.as_mut() { info.tick().await; }
            let size = terminal.size()?;
            view.folder_rows_snapshot = Some(std::rc::Rc::new(browser.folder_rows()));
            terminal.draw(|frame| { draw(frame, &browser, &view); view.terminals.borrow().draw_overlay(frame, &browser); })?;
            guard.mouse_capture(view.terminals.borrow().active.is_some() || view.session_preview.borrow().terminals.visible)?;
            idle.deadline = next_age_update(&browser, &view, size.height)
                .into_iter().chain(view.drawer_reminder.get().deadline).min();
            }
            let busy = view.session_preview.borrow().active() || terminal_copy.is_some() || !view.terminals.borrow().is_empty() || browser.loading() || git_discovery.pending() || view.history.loading() || pending.iter().any(Option::is_some) || relink_task.iter().any(Option::is_some) || folder_task.iter().any(Option::is_some)
                || view.relink.is_some() || view.folder_move.is_some() || view.worktree_create.is_some() || view.discard.is_some()
                || view.error_dialog.as_ref().is_some_and(|dialog| dialog.copy_task.is_some())
                || view.info.as_ref().is_some_and(|info| info.dialog.copy_task.is_some());
            let mut wait = Duration::from_millis(if busy { 40 } else { 250 });
            if let Some(deadline) = idle.deadline { wait = wait.min(deadline.saturating_duration_since(std::time::Instant::now())); }
            if let Some(deadline) = view.folder_navigation.deadline { wait = wait.min(deadline.saturating_duration_since(std::time::Instant::now())); }
            if !git_discovery.pending() && !view.history.loading() {
                for watcher in [&git_watch, &terminal_git_watch] {
                    if let Some(deadline) = watcher.deadline() { wait = wait.min(deadline.saturating_duration_since(std::time::Instant::now())); }
                }
            }
            guard.enhanced(input_reader.enhanced())?;
            if let Err(error) = view.terminals.borrow_mut().sync_focus(outer_focused && view.terminal_focused()) { view.message = Some(error.to_string()); }
            if view.terminals.borrow().input_backlogged() {tokio::time::sleep(wait).await;}
            else if input_reader.poll(wait)? {
                idle.wake();
                let packet=if view.coalesce_navigation() { input_reader.read_navigation()? } else { input_reader.read()? };
                let incoming=packet.event;
                if matches!(incoming, Event::FocusGained | Event::FocusLost) {
                    outer_focused = matches!(incoming, Event::FocusGained);
                    if view.folder_navigation.pending() { view.folder_navigation.deadline = Some(std::time::Instant::now()); }
                    continue;
                }
                if let Event::Key(key) = &incoming {
                    if drawer_shortcut(*key) {
                        let learned = view.drawer_reminder.get().learned;
                        handle_key(&mut browser, &mut view, *key);
                        if !learned && view.drawer_reminder.get().learned {
                            if let Some(path) = browser.state_path.as_deref() {
                                if let Err(error) = crate::state::mark_drawer_shortcut_learned(path) {
                                    let warning = format!("Could not save drawer shortcut preference: {error}");
                                    view.message = Some(warning.clone());
                                    browser.state_warning = Some(warning);
                                }
                            }
                        }
                        continue;
                    }
                }
                if let Event::Mouse(mouse) = &incoming {
                    let preview_clicked = matches!(mouse.kind,crossterm::event::MouseEventKind::Down(crossterm::event::MouseButton::Left))
                        && !view.modal_open() && view.session_preview.borrow().terminals.visible
                        && view.session_preview.borrow().terminals.area.contains((mouse.column,mouse.row).into());
                    if preview_clicked {
                        let _ = execute_command(Command::GitSelectWorktree, &mut browser, &mut view);
                        continue;
                    }
                    if view.drawer_open { continue; }
                    if !view.modal_open() {
                        if matches!(mouse.kind,crossterm::event::MouseEventKind::Down(_)) && view.terminals.borrow().visible && view.terminals.borrow().area.contains((mouse.column,mouse.row).into()) {view.focus=1;view.git_focus=false;sync_terminal_view(&browser, &view);}
                        if let Err(error) = view.terminals.borrow_mut().mouse(*mouse) {
                            view.message = Some(error.to_string());
                        }
                    }
                    continue;
                }
                let mutation_running = relink_task.iter().any(Option::is_some) || folder_task.iter().any(Option::is_some)
                    || view.discard.as_ref().is_some_and(|m| m.executing())
                    || view.worktree_create.as_ref().is_some_and(|m| m.executing)
                    || view.hidden_dialogs.worktree_create.as_ref().is_some_and(|m| m.executing);
                let previous_terminal = view.terminals.borrow().active;
                let terminal_overlay = view.terminals.borrow().has_overlay();
                let terminal_input = if terminal_overlay || view.terminal_focused() {
                    view.terminals.borrow_mut().input_rich(&incoming, packet.key.as_ref())
                } else { Ok(crate::inline_terminal::Input::Pass) };
                match terminal_input {
                    Ok(crate::inline_terminal::Input::Quit) => {
                        if !mutation_running { break; }
                        view.message = Some("Wait for the running operation to finish before quitting.".into());
                        continue;
                    },
                    Ok(crate::inline_terminal::Input::Consumed) => {
                        let active = view.terminals.borrow().active;
                        if previous_terminal != active || (terminal_overlay && active.is_some() && !view.terminals.borrow().has_overlay() && matches!(&incoming, Event::Key(key) if key.kind == KeyEventKind::Press && key.code == KeyCode::Enter)) {
                            let folder = view.terminals.borrow().active_folder();
                            if let Some(folder) = folder {
                                browser.open_folder(folder.clone());
                                let mut terminals = view.terminals.borrow_mut();
                                terminals.sync_folder(Some(&folder));
                            }
                            if active.is_some() && view.drawer_open { view.toggle_drawer(); }
                            view.focus = 1; view.git_focus = false;
                        }
                        continue;
                    },
                    Ok(crate::inline_terminal::Input::Pass) => (),
                    Err(error) => { view.message = Some(error.to_string()); continue; }
                }
                if let Event::Paste(text)=&incoming {
                    if let Some(modal)=view.discard.as_mut(){modal.paste(text);}
                    if let Some(modal)=view.worktree_create.as_mut(){modal.paste(text);}
                    if let Some(modal)=view.folder_move.as_mut(){modal.paste(text);}
                    if let Some(modal)=view.relink.as_mut(){if !modal.busy && modal.preview.is_none(){if let Some(picker)=modal.picker.as_mut(){picker.paste(&mut modal.input,text);}}}
                }
                if let Event::Key(key) = incoming {
                    let was_help = view.help;
                    let folder_focused = view.focus == 0 && !view.git_focus && view.coalesce_navigation();
                    let now = std::time::Instant::now();
                    view.folder_navigation.observe(key, folder_focused, now);
                    if let Some(code) = input_reader.take_navigation_release() {
                        view.folder_navigation.observe(KeyEvent::new_with_kind(code, KeyModifiers::NONE, KeyEventKind::Release), folder_focused, now);
                    }
                    let action = handle_key(&mut browser, &mut view, key);
                    if was_help && !view.help && key.code == KeyCode::Enter {
                        // An activation must not also accept a newly opened modal or
                        // resume the underlying session through queued Enter repeats.
                        while view.dialog_area == view.drawer_open && input_reader.poll(Duration::ZERO)? {
                            let _ = input_reader.read()?;
                        }
                    }
                    match action {
                        Action::OpenDiscard | Action::RecoverDiscard => {
                            if let Some(reason) = crate::worktree_discard_ui::unavailable(&browser) {
                                view.message = Some(reason.into());
                                continue;
                            }
                            let Some(state) = relocation_state.clone() else { continue };
                            let journal = match crate::worktree_discard::pending(&state) {
                                Ok(journal) => journal,
                                Err(error) => { view.message = Some(error); continue; }
                            };
                            view.discard_recovery_available = journal.is_some();
                            let selected = crate::worktree_discard_ui::selected(&browser);
                            if matches!(action, Action::OpenDiscard) && journal.is_some() {
                                view.message = Some("Press D to review the unfinished discard first.".into());
                                continue;
                            }
                            let Some(root) = journal.as_ref().map(|journal| journal.root()).or(selected) else { continue };
                            let local = view.terminals.borrow().worktree_terminals(root);
                            let sources = crate::folder_move_ui::sources(&browser);
                            view.discard = Some(crate::worktree_discard_ui::Modal::new(state, sources, selected.map(ToOwned::to_owned), journal, local));
                        }
                        Action::CloseDiscard => view.discard = None,
                        Action::ReviewDiscardTerminals => {
                            if let Some(modal) = view.discard.as_mut() { modal.review_terminals(); }
                        }
                        Action::ConfirmDiscardTerminals => {
                            if let Some(modal) = view.discard.as_mut() {
                                modal.confirm_terminals(&mut view.terminals.borrow_mut());
                            }
                        }
                        Action::RetryDiscard | Action::ReleaseDiscardIdle | Action::ApplyDiscard => {
                            if let Some(modal) = view.discard.as_mut() {
                                let local = modal.root().map(|root| view.terminals.borrow().worktree_terminals(root)).unwrap_or_default();
                                if matches!(action, Action::ApplyDiscard) && local.is_empty() {
                                    modal.start();
                                } else {
                                    modal.refresh(local, matches!(action, Action::ReleaseDiscardIdle));
                                }
                            }
                        }
                        Action::OpenFolderMove=>{
                            if let Some(reason)=crate::folder_move_ui::unavailable(&browser){view.message=Some(reason);continue;}
                            if let FolderFilter::Path(path)=&browser.folder {
                                view.folder_move=Some(crate::folder_move_ui::Modal::new(path.clone(),launch_dir.clone()));
                            }
                        },
                        Action::CloseFolderMove=>{
                            if let Some(modal)=view.folder_move.as_mut(){modal.stop_release().await;}
                            if let Some(task)=folder_task[usize::from(view.dialog_area)].take(){task.abort();}
                            view.folder_move=None;
                        },
                        Action::PreviewFolderMove=>{
                            let Some(modal)=view.folder_move.as_mut() else {continue};
                            let Some(state)=relocation_state.clone() else {continue};
                            modal.stop_release().await;
                            let target=match modal.target() {
                                Ok(p)=>p,Err(e)=>{modal.message=Some(e);continue;}
                            };
                            let sources=crate::folder_move_ui::sources(&browser);
                            let old=modal.old.clone();modal.busy=true;
                            modal.message=Some("Checking all configured Codex sources…".into());
                            folder_task[usize::from(view.dialog_area)]=Some(tokio::spawn(async move{
                                FolderMoveResult::Preview(crate::folder_move::preview(sources,old,target,&state).await)
                            }));
                        },
                        Action::ApplyFolderMove=>{
                            let Some(modal)=view.folder_move.as_mut() else {continue};
                            modal.stop_release().await;
                            let Some(plan)=modal.plan.clone() else {continue};
                            let Some(state)=relocation_state.clone() else {continue};
                            let sources=crate::folder_move_ui::sources(&browser);
                            let recovery=modal.recovery;modal.busy=true;modal.executing=true;
                            let tx=folder_tx[usize::from(view.dialog_area)].clone();
                            folder_task[usize::from(view.dialog_area)]=Some(tokio::spawn(async move{
                                FolderMoveResult::Applied(crate::folder_move::execute(plan,state,sources,recovery,tx).await)
                            }));
                        },
                        Action::CloseInformation => {
                            while view.dialog_area == view.drawer_open && input_reader.poll(Duration::ZERO)? { let _ = input_reader.read()?; }
                        },
                        Action::CopyInformation => {
                            if let Some(info)=view.info.as_mut(){ info.dialog.start_copy(); }
                        },
                        Action::CopyErrorReport => {
                            if let Some(dialog)=view.error_dialog.as_mut(){dialog.start_copy();}
                        }
                        Action::CancelRelink => {
                            if let Some(task)=relink_task[usize::from(view.dialog_area)].take(){task.abort();}
                            view.relink=None;
                        }
                        Action::OpenRelink => {
                            if let Some(reason)=relink_unavailable(&browser){view.message=Some(reason);continue;}
                            if let Some(session)=selected_session(&browser){let input=session.cwd.as_ref().map(|p|p.display().to_string()).unwrap_or_default();let picker=crate::path_picker::Picker::for_relink(launch_dir.clone(),session.cwd.clone());view.relink=Some(RelinkModal{session,input,picker:Some(picker),preview:None,recovery:false,busy:false,executing:false,error:None});}
                        }
                        Action::RecoverRelink => {
                            refresh_recovery(relocation_state.as_deref(), &mut view);
                            if !view.recovery_available { continue; }
                            let Some(state)=&relocation_state else{view.message=Some("No persistent state directory.".into());continue};
                            let folder=match crate::folder_move::pending(state){Ok(j)=>j,Err(e)=>{view.message=Some(e);continue;}};
                            let count=match crate::relocation::pending(state){Ok(j)=>j.len(),Err(e)=>{view.message=Some(e);continue;}};
                            let total=count+usize::from(folder.is_some());
                            let index=view.recovery_index%total.max(1);
                            view.recovery_index=(index+1)%total.max(1);
                            view.folder_move=None;view.relink=None;
                            if index==count {
                                if let Some(j)=folder{view.folder_move=Some(crate::folder_move_ui::Modal::recover(j));continue;}
                            }
                            match crate::relocation::pending(state){
                                Ok(journals)=>if let Some(j)=journals.get(index){
                                    let p=j.preview.clone();
                                    let session=crate::domain::SessionSummary{key:p.key.clone(),title:"Unfinished relocation".into(),cwd:Some(p.old.clone()),created_at:p.created_at,updated_at:0,archived:p.archived,can_accept_direct_input:None,source_kind:None,git:None};
                                    view.relink=Some(RelinkModal{session,input:p.target.display().to_string(),picker:None,preview:Some(p),recovery:true,busy:false,executing:false,error:Some(format!("Recorded phase: {}. Enter attempts guarded completion. u cycles through {} unfinished operation(s).",j.phase,journals.len()))});
                                }else{view.message=Some("No unfinished relocations.".into());},
                                Err(e)=>view.message=Some(e),
                            }
                        }
                        Action::RefreshTerminals | Action::StopTerminals => {
                            let Some(modal)=view.relink.as_mut() else {continue};
                            let Some(p)=modal.preview.clone() else {continue};
                            if modal.recovery {continue;}
                            let Some(dialog)=view.terminal_recovery.as_mut() else {continue};
                            let reviewed=dialog.reviewed();
                            let Some((binary,source))=browser.specs.iter().find_map(|s|match s{crate::browser::SourceSpec::Codex{binary,source} if source.id==p.key.source_id=>Some((binary.clone(),source.clone())),_=>None}) else {continue};
                            let Some(state)=relocation_state.clone() else {continue};
                            if action==Action::StopTerminals && reviewed.is_none() {continue;}
                            dialog.busy=true;modal.busy=true;modal.executing=true;
                            let tx=relink_tx[usize::from(view.dialog_area)].clone();
                            relink_task[usize::from(view.dialog_area)]=Some(tokio::spawn(async move {
                                if action==Action::RefreshTerminals {
                                    RelinkResult::Terminals(crate::providers::codex_terminals::inspect(&binary,&source,&p).await)
                                } else {
                                    match crate::providers::codex_terminals::stop_reviewed(&binary,&source,&p,&reviewed.unwrap()).await {
                                        Ok(()) => RelinkResult::Applied(crate::relocation::execute_with_progress(&binary,&source,&state,p,false,Some(tx)).await),
                                        Err(error) => RelinkResult::Terminals(Err(error)),
                                    }
                                }
                            }));
                        }
                        Action::PreviewRelink | Action::ApplyRelink => {
                            let Some(modal)=view.relink.as_mut() else{continue};
                            let Some((binary,source))=browser.specs.iter().find_map(|s|match s{crate::browser::SourceSpec::Codex{binary,source} if source.id==modal.session.key.source_id=>Some((binary.clone(),source.clone())),_=>None}) else {modal.error=Some("Session source is no longer configured.".into());continue};
                            if action==Action::PreviewRelink {
                                let input=modal.input.clone();let launch=launch_dir.clone();
                                let worktree=modal.picker.as_ref().and_then(|p|p.worktree_validation(&input));
                                let session=modal.session.clone();modal.busy=true;modal.error=None;
                                relink_task[usize::from(view.dialog_area)]=Some(tokio::spawn(async move {
                                    if let Some((origin,target))=worktree { if let Err(error)=crate::worktrees::validate(&origin,&target).await { return RelinkResult::Preview(Err(error.into())); } }
                                    let destination=tokio::task::spawn_blocking(move||crate::relocation::destination(&input,&launch)).await;
                                    let result=match destination {
                                        Ok(Ok(target))=>crate::relocation::preview(&binary,&source,&session,target).await,
                                        Ok(Err(error))=>Err(error.into()),Err(error)=>Err(error.to_string().into()),
                                    };
                                    RelinkResult::Preview(result)
                                }));
                            } else {
                                let Some(p)=modal.preview.clone() else{continue};
                                let Some(state)=relocation_state.clone() else{continue};
                                let recovery=modal.recovery;modal.busy=true;modal.executing=true;modal.error=None;
                                let tx=relink_tx[usize::from(view.dialog_area)].clone();
                                relink_task[usize::from(view.dialog_area)]=Some(tokio::spawn(async move {RelinkResult::Applied(crate::relocation::execute_with_progress(&binary,&source,&state,p,recovery,Some(tx)).await)}));
                            }
                        }
                        Action::Quit => {
                            if mutation_running { view.message = Some("Wait for the running operation to finish before quitting.".into()); }
                            else if view.terminals.borrow_mut().request_quit() { break; }
                        },
                        Action::Refresh => {
                            view.message = None;
                            refresh_recovery(relocation_state.as_deref(), &mut view);
                            browser.invalidate_git(); discovery.refresh(&mut browser);
                        }
                        Action::CancelResume => {
                            pending[usize::from(view.dialog_area)].take();
                            view.preparing = false;
                            view.message = Some("Resume cancelled.".into());
                            // Cancellation may interrupt an RPC frame. Reconnect rather than reuse it.
                            browser.invalidate_git(); discovery.refresh(&mut browser);
                        }
                        Action::CreateNew(index) => {
                            let Some(path) = view.creation_folder.take() else { continue; };
                            let spec = &browser.specs[index];
                            let command = match spec.prepare_new(&path) {
                                Ok(command) => command,
                                Err(error) => {
                                    view.message = Some(error);
                                    continue;
                                }
                            };
                            let title = format!("New session · {}", path.display());
                            save_folder(&mut browser, last_folder.as_deref(), &mut view);
                            let result = view.terminals.borrow_mut().launch(&command, None, title);
                            view.focus = 1;
                            view.git_focus = false;
                            match result {
                                Ok(()) => (),
                                Err(error) => view.message = Some(format!("Could not start session: {error}")),
                            }
                        }
                        Action::ResumeSelected => {
                            if browser.selected.as_ref().is_some_and(|key| view.terminals.borrow_mut().attach(key)) {
                                if let Some(folder) = view.terminals.borrow().active_folder() { browser.open_folder(folder); }
                                let mut terminals = view.terminals.borrow_mut();
                                let folder = terminals.active_folder();
                                terminals.sync_folder(folder.as_deref());
                                view.focus = 1; view.git_focus = false; continue;
                            }
                            if let Some(session) = browser
                                .visible()
                                .into_iter()
                                .find(|s| Some(&s.key) == browser.selected.as_ref())
                            {
                                if let Some(reason) = crate::domain::resume_blocker(&session) {
                                    view.message = Some(reason.into());
                                    continue;
                                }
                                match browser.resume_provider(&session.key) {
                                    Ok(provider) => {
                                        let observed_cwd = browser
                                            .details
                                            .get(&session.key)
                                            .and_then(|r| r.as_ref().ok())
                                            .and_then(|s| s.as_ref())
                                            .map(|s| s.cwd.clone())
                                            .unwrap_or_else(|| session.cwd.clone());
                                        view.preparing = true;
                                        view.message = None;
                                        pending[usize::from(view.dialog_area)] = Some(PendingResume {
                                            origin: browser.folder.clone(),
                                            observed_cwd,
                                            task: tokio::spawn(async move {
                                                provider.prepare_resume(&session).await
                                            }),
                                        });
                                    }
                                    Err(error) => view.message = Some(error),
                                }
                            }
                        }
                        Action::Continue => {}
                    }
                }
            }
            // Let the runtime service I/O and timers between blocking terminal polls.
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
        Ok(())
    }
    .await;
    view.session_preview.get_mut().shutdown().await;
    view.terminals.get_mut().shutdown().await;
    git_watch.stop();
    terminal_git_watch.stop();
    view.history.shutdown().await;
    status_monitor.shutdown().await;
    git_discovery.shutdown().await;
    view.commit_info = None;
    view.hidden_dialogs.commit_info = None;
    crate::git::drain_reads().await;
    drop(pending);
    discovery.close(&mut browser).await;
    save_folder(&mut browser, last_folder.as_deref(), &mut view);
    guard.suspend()?;
    if let Some(warning) = browser.state_warning.as_ref() {
        eprintln!("tracr: {}", safe_text(warning));
    }
    if reloading && outcome.is_ok() {
        dev_reload.acknowledge()?;
    }
    outcome
}

#[cfg(test)]
mod cleanup_tests {
    use super::*;

    #[test]
    fn browser_paging_uses_the_rendered_pane_height() {
        let mut browser = entry_fixture(20, 1);
        let mut view = folder_view();
        let path = (0..20).fold(std::path::PathBuf::from("/"), |path, i| {
            path.join(format!("node{i:02}"))
        });
        browser.open_folder(path);
        let rows = browser.folder_rows();
        let mut terminal = Terminal::new(ratatui::backend::TestBackend::new(100, 9)).unwrap();
        terminal
            .draw(|frame| draw_pane(frame, frame.area(), 0, &browser, &view))
            .unwrap();
        press(&mut browser, &mut view, KeyCode::Home);
        assert_eq!(browser.folder, rows[0].filter);
        press(&mut browser, &mut view, KeyCode::PageDown);
        assert_eq!(browser.folder, rows[6].filter);
        press(&mut browser, &mut view, KeyCode::End);
        assert_eq!(browser.folder, rows.last().unwrap().filter);
        press(&mut browser, &mut view, KeyCode::PageUp);
        assert_eq!(browser.folder, rows[rows.len() - 7].filter);

        browser.open_folder("/tmp".into());
        view.toggle_drawer();
        let sessions: Vec<_> = browser
            .visible()
            .iter()
            .map(|session| session.key.clone())
            .collect();
        for (height, step) in [(9, 2), (15, 4)] {
            terminal.backend_mut().resize(100, height);
            terminal.resize(Rect::new(0, 0, 100, height)).unwrap();
            terminal
                .draw(|frame| draw_pane(frame, frame.area(), 1, &browser, &view))
                .unwrap();
            press(&mut browser, &mut view, KeyCode::Home);
            press(&mut browser, &mut view, KeyCode::PageDown);
            assert_eq!(browser.selected.as_ref(), Some(&sessions[step]));
            press(&mut browser, &mut view, KeyCode::PageDown);
            assert_eq!(browser.selected.as_ref(), Some(&sessions[step * 2]));
            press(&mut browser, &mut view, KeyCode::PageUp);
            assert_eq!(browser.selected.as_ref(), Some(&sessions[step]));
        }
        for _ in 0..sessions.len() {
            press(&mut browser, &mut view, KeyCode::PageDown);
        }
        assert_eq!(browser.selected.as_ref(), sessions.last());
        for _ in 0..sessions.len() {
            press(&mut browser, &mut view, KeyCode::PageUp);
        }
        assert_eq!(browser.selected.as_ref(), sessions.first());

        browser.open_folder("/empty".into());
        press(&mut browser, &mut view, KeyCode::PageUp);
        assert!(browser.selected.is_none());
    }

    #[test]
    fn search_paging_and_endpoints_track_result_selection() {
        let mut browser = entry_fixture(20, 1);
        let mut view = View {
            editing: true,
            ..View::default()
        };
        let mut terminal = Terminal::new(ratatui::backend::TestBackend::new(100, 30)).unwrap();
        terminal
            .draw(|frame| draw_search(frame, &browser, &view))
            .unwrap();
        let results: Vec<_> = browser
            .search_results("")
            .iter()
            .map(|session| session.key.clone())
            .collect();
        press(&mut browser, &mut view, KeyCode::PageDown);
        let selected = results
            .iter()
            .position(|key| Some(key) == view.search_selected.as_ref())
            .unwrap();
        assert!(selected > 1 && selected < results.len() - 1);
        press(&mut browser, &mut view, KeyCode::PageUp);
        assert_eq!(view.search_selected.as_ref(), results.first());
        press(&mut browser, &mut view, KeyCode::End);
        assert_eq!(view.search_selected.as_ref(), results.last());
        press(&mut browser, &mut view, KeyCode::Home);
        assert_eq!(view.search_selected.as_ref(), results.first());

        view.search_query = "no matching session".into();
        press(&mut browser, &mut view, KeyCode::PageDown);
        assert!(view.search_selected.is_none());
        let mut browser = entry_fixture(1, 1);
        view.search_query.clear();
        let only = browser.search_results("")[0].key.clone();
        for code in [
            KeyCode::PageDown,
            KeyCode::PageUp,
            KeyCode::End,
            KeyCode::Home,
        ] {
            press(&mut browser, &mut view, code);
            assert_eq!(view.search_selected.as_ref(), Some(&only));
        }
    }

    #[tokio::test]
    async fn git_page_keys_use_the_rendered_height() {
        use crate::git_sessions::tests::{add_session, fixture};
        let (mut browser, history) = fixture().await;
        select_git_fixture(&mut browser, "/repo/.git");
        for i in 0..12 {
            add_session(
                &mut browser,
                &format!("session-{i}"),
                "/worktree",
                "/worktree",
                "/repo/.git",
                crate::domain::RuntimeStatus::Idle,
                i,
            );
        }
        let mut view = View {
            history,
            git_focus: true,
            drawer_open: true,
            drawer_git: true,
            dialog_area: true,
            ..View::default()
        };
        let mut terminal = Terminal::new(ratatui::backend::TestBackend::new(100, 10)).unwrap();
        // The compact drawer gives Git the full height minus its borders and top padding.
        for (height, step) in [(10, 7), (8, 5)] {
            terminal.backend_mut().resize(100, height);
            terminal.resize(Rect::new(0, 0, 100, height)).unwrap();
            terminal.draw(|frame| draw(frame, &browser, &view)).unwrap();
            press(&mut browser, &mut view, KeyCode::PageDown);
            assert_eq!(view.git_sessions.borrow().selected, step);
            assert!(view.git_sessions.borrow().session().is_some());
            press(&mut browser, &mut view, KeyCode::PageUp);
            assert_eq!(view.history.selected, 0);
            assert_eq!(view.git_sessions.borrow().selected, 0);
        }
        press(&mut browser, &mut view, KeyCode::End);
        assert_eq!(view.history.selected, 1);
        assert_eq!(
            view.git_sessions.borrow().selected,
            view.git_sessions.borrow().rows.len() - 1
        );
        press(&mut browser, &mut view, KeyCode::Home);
        assert_eq!(view.history.selected, 0);
        assert_eq!(view.git_sessions.borrow().selected, 0);
    }

    #[test]
    fn folder_preview_waits_for_release_or_quiet_and_prioritizes_queued_input() {
        let start = std::time::Instant::now();
        let mut navigation = FolderNavigation::default();
        let press = KeyEvent::new(KeyCode::Down, KeyModifiers::NONE);
        navigation.observe(press, true, start);
        assert!(!navigation.ready(start + Duration::from_millis(74), false));
        assert!(navigation.ready(start + Duration::from_millis(75), false));
        // Simulate slow work: the timer expired, but queued keys must be read first.
        assert!(!navigation.ready(start + Duration::from_secs(1), true));
        let repeat =
            KeyEvent::new_with_kind(KeyCode::Down, KeyModifiers::NONE, KeyEventKind::Repeat);
        for millis in (30..=300).step_by(30) {
            let now = start + Duration::from_millis(millis);
            navigation.observe(repeat, true, now);
            assert!(!navigation.ready(now, false));
        }
        let released = start + Duration::from_millis(310);
        navigation.observe(
            KeyEvent::new_with_kind(KeyCode::Down, KeyModifiers::NONE, KeyEventKind::Release),
            true,
            released,
        );
        assert!(navigation.ready(released, false));
        navigation.finish();
        assert!(!navigation.pending());
        assert!(!navigation.ready(released + Duration::from_secs(1), false));
    }

    #[test]
    fn folder_preview_handles_reversal_actions_and_non_folder_navigation() {
        let now = std::time::Instant::now();
        let mut navigation = FolderNavigation::default();
        let down = KeyEvent::new(KeyCode::Down, KeyModifiers::NONE);
        navigation.observe(down, false, now);
        assert!(!navigation.pending());
        navigation.observe(down, true, now);
        navigation.observe(KeyEvent::new(KeyCode::Up, KeyModifiers::NONE), true, now);
        navigation.observe(
            KeyEvent::new_with_kind(KeyCode::Down, KeyModifiers::NONE, KeyEventKind::Release),
            true,
            now,
        );
        assert!(!navigation.ready(now, false));
        for code in [KeyCode::Enter, KeyCode::Tab, KeyCode::Right] {
            navigation.observe(down, true, now);
            navigation.observe(KeyEvent::new(code, KeyModifiers::NONE), true, now);
            assert!(navigation.ready(now, false));
        }
    }

    #[tokio::test]
    async fn deferred_folder_preview_hides_and_blocks_old_git_selection() {
        let (mut browser, mut view) = navigation_fixture(&["selected-oid"]).await;
        view.folder_navigation.observe(
            KeyEvent::new(KeyCode::Down, KeyModifiers::NONE),
            true,
            std::time::Instant::now(),
        );
        let original = browser.folder.clone();
        for command in [
            Command::GitSelectWorktree,
            Command::CommitInfo,
            Command::CreateWorktree,
        ] {
            assert_eq!(
                execute_command(command, &mut browser, &mut view),
                Action::Continue
            );
        }
        assert_eq!(browser.folder, original);
        assert!(
            view.commit_info.is_none()
                && view.worktree_create.is_none()
                && view.worktree_picker.is_none()
        );
        let mut terminal = Terminal::new(ratatui::backend::TestBackend::new(180, 40)).unwrap();
        terminal.draw(|frame| draw(frame, &browser, &view)).unwrap();
        let text: String = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect();
        assert!(!text.contains("Loading Git preview"));
        assert!(!text.contains("selected-oid"));
    }

    #[tokio::test]
    async fn git_panel_waits_for_selected_folder_ownership() {
        use crate::git_discovery::{Outcome, Ownership};
        for (width, height) in [(180, 40), (80, 24), (80, 10)] {
            let (mut browser, mut view) = navigation_fixture(&["selected-oid"]).await;
            let folder = std::path::PathBuf::from("/plain");
            browser.folder = FolderFilter::Path(folder.clone());
            let mut terminal =
                Terminal::new(ratatui::backend::TestBackend::new(width, height)).unwrap();
            let mut render = |browser: &Browser, view: &View| {
                terminal.draw(|frame| draw(frame, browser, view)).unwrap();
                terminal
                    .backend()
                    .buffer()
                    .content
                    .iter()
                    .map(|cell| cell.symbol())
                    .collect::<String>()
            };
            for pending in [true, false] {
                if pending {
                    view.folder_navigation.observe(
                        KeyEvent::new(KeyCode::Down, KeyModifiers::NONE),
                        true,
                        std::time::Instant::now(),
                    );
                } else {
                    view.folder_navigation.finish();
                }
                // Old history must not reserve space for an unknown or non-Git folder.
                for outcome in [
                    None,
                    Some(Outcome::NotRepository),
                    Some(Outcome::Unavailable("offline".into())),
                ] {
                    browser.ownership.insert(
                        folder.clone(),
                        Ownership {
                            generation: browser.git_generation,
                            outcome,
                            previous: None,
                        },
                    );
                    assert!(selected_git_repository(&browser).is_none());
                    let screen = render(&browser, &view);
                    assert!(!screen.contains("Loading Git preview"));
                    assert!(!screen.contains("Selected commit"));
                    let panels = panel_layout(
                        Rect::new(0, 0, width, height),
                        height,
                        &view,
                        selected_git_repository(&browser).is_some(),
                    );
                    assert!(panels.git.is_none());
                }
            }
            // Confirmation shows the preview, never commits from the previous repository.
            let repository = Outcome::Repository {
                repository: crate::git::Directory {
                    common_dir: "/another/.git".into(),
                    git_dir: "/another/.git".into(),
                },
                root: Some(folder.clone()),
            };
            browser.apply_ownership(browser.git_generation, folder.clone(), repository.clone());
            let screen = render(&browser, &view);
            assert!(screen.contains("Loading Git preview"));
            assert!(!screen.contains("Selected commit"));
            // Refresh retains effective ownership, including transient discovery failures.
            browser.invalidate_git();
            assert!(render(&browser, &view).contains("Loading Git preview"));
            browser.apply_ownership(
                browser.git_generation,
                folder.clone(),
                Outcome::Unavailable("offline".into()),
            );
            assert!(render(&browser, &view).contains("Loading Git preview"));
            // Returning to a confirmed matching repository exposes its history.
            browser.apply_ownership(
                browser.git_generation,
                folder.clone(),
                Outcome::Repository {
                    repository: crate::git::Directory {
                        common_dir: "/project/.git".into(),
                        git_dir: "/project/.git".into(),
                    },
                    root: Some(folder.clone()),
                },
            );
            assert!(render(&browser, &view).contains("Selected commit"));
            view.folder_navigation.observe(
                KeyEvent::new(KeyCode::Up, KeyModifiers::NONE),
                true,
                std::time::Instant::now(),
            );
            let screen = render(&browser, &view);
            assert!(screen.contains("Selected commit"));
            assert!(!screen.contains("Loading Git preview"));
            browser.apply_ownership(
                browser.git_generation,
                folder.clone(),
                Outcome::NotRepository,
            );
            assert!(!render(&browser, &view).contains("Loading Git preview"));
            browser.folder = FolderFilter::Path("/unseen".into());
            assert!(!render(&browser, &view).contains("Loading Git preview"));
        }
    }

    #[tokio::test]
    async fn git_navigation_retains_original_snapshot_until_ready() {
        let (mut browser, mut view) = navigation_fixture(&["selected-oid"]).await;
        select_git_fixture(&mut browser, "/project/.git");
        let mut terminal = Terminal::new(ratatui::backend::TestBackend::new(180, 40)).unwrap();
        terminal.draw(|frame| draw(frame, &browser, &view)).unwrap();
        let original = view.git_preview.borrow().clone().unwrap();
        view.folder_navigation.observe(
            KeyEvent::new(KeyCode::Down, KeyModifiers::NONE),
            true,
            std::time::Instant::now(),
        );
        terminal.draw(|frame| draw(frame, &browser, &view)).unwrap();
        assert_eq!(*view.git_preview.borrow(), Some(original.clone()));
        // Loading continues after debounce, including within the same repository.
        view.folder_navigation.finish();
        view.git_navigation_loading = true;
        view.history.rows[0].subject = "Destination commit".into();
        for (width, height) in [(180, 40), (80, 24), (80, 10), (200, 50)] {
            terminal.backend_mut().resize(width, height);
            terminal.resize(Rect::new(0, 0, width, height)).unwrap();
            terminal.draw(|frame| draw(frame, &browser, &view)).unwrap();
            let buffer = terminal.backend().buffer();
            let text: String = buffer.content.iter().map(|cell| cell.symbol()).collect();
            assert!(text.contains("Selected commit"));
            assert!(!text.contains("Destination commit"));
            assert!(!text.contains("Loading Git"));
            let (_, footer) = browser_footer(Rect::new(0, 0, width, height), &browser, &view);
            let panels = panel_layout(Rect::new(0, 0, width, height - footer), height, &view, true);
            let inner = pane_block("Git Worktrees", view.git_focus && !view.git_details_focused)
                .inner(panels.git.unwrap());
            for y in 0..inner.height.min(original.area.height) {
                for x in 0..inner.width.min(original.area.width) {
                    // Preserve symbols and all styling, including colored graph
                    // spans, backgrounds, and the previous selection highlight.
                    assert_eq!(buffer[(inner.x + x, inner.y + y)], original[(x, y)]);
                }
            }
            assert_eq!(*view.git_preview.borrow(), Some(original.clone()));
        }
        for command in [
            Command::GitSelectWorktree,
            Command::CommitInfo,
            Command::CreateWorktree,
            Command::GitDown,
        ] {
            assert_eq!(
                execute_command(command, &mut browser, &mut view),
                Action::Continue
            );
        }
        assert!(
            view.commit_info.is_none()
                && view.worktree_create.is_none()
                && view.worktree_picker.is_none()
        );
        assert!(selected_commit(&browser, &view).is_none());
        assert!(selected_git_session(&browser, &view).is_none());
        // A fast move to another repository and back must not recapture intermediate rows.
        select_git_fixture(&mut browser, "/other/.git");
        terminal.draw(|frame| draw(frame, &browser, &view)).unwrap();
        assert_eq!(*view.git_preview.borrow(), Some(original.clone()));
        select_git_fixture(&mut browser, "/project/.git");
        view.git_navigation_loading = false;
        terminal.draw(|frame| draw(frame, &browser, &view)).unwrap();
        let text: String = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect();
        assert!(text.contains("Destination commit"));
        assert!(!text.contains("Selected commit"));
        // Failure replaces the stale preview and cannot enable actions on old rows.
        view.git_navigation_loading = true;
        view.history.error = Some("destination unavailable".into());
        finish_git_navigation(&mut view);
        assert!(!view.git_navigation_loading && view.git_navigation_error);
        terminal.draw(|frame| draw(frame, &browser, &view)).unwrap();
        let text: String = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect();
        assert!(text.contains("destination unavailable"));
        assert!(!text.contains("Destination commit"));
        assert!(selected_commit(&browser, &view).is_none());
    }

    #[tokio::test]
    async fn cached_preview_preserves_sessions_and_crops_wide_characters() {
        let (mut browser, history) = crate::git_sessions::tests::fixture().await;
        crate::git_sessions::tests::add_session(
            &mut browser,
            "Previous session 界",
            "/worktree",
            "/worktree",
            "/repo/.git",
            crate::domain::RuntimeStatus::Idle,
            1,
        );
        select_git_fixture(&mut browser, "/repo/.git");
        let mut view = View {
            history,
            ..folder_view()
        };
        let mut terminal = Terminal::new(ratatui::backend::TestBackend::new(180, 40)).unwrap();
        terminal.draw(|frame| draw(frame, &browser, &view)).unwrap();
        let snapshot = view.git_preview.borrow().clone().unwrap();
        assert!(
            snapshot
                .content
                .iter()
                .map(|c| c.symbol())
                .collect::<String>()
                .contains("Previous session 界")
        );
        view.git_navigation_loading = true;
        view.history.rows.clear();
        terminal.draw(|frame| draw(frame, &browser, &view)).unwrap();
        assert!(
            terminal
                .backend()
                .buffer()
                .content
                .iter()
                .map(|c| c.symbol())
                .collect::<String>()
                .contains("Previous session 界")
        );
        // Clip a wide character at the right edge of the cached body.
        let mut narrow = ratatui::buffer::Buffer::empty(Rect::new(0, 0, 4, 1));
        narrow[(2, 0)].set_symbol("界");
        *view.git_preview.borrow_mut() = Some(narrow);
        terminal
            .draw(|frame| draw_git_preview(frame, Rect::new(0, 0, 5, 3), &view))
            .unwrap();
        assert!(
            !terminal
                .backend()
                .buffer()
                .content
                .iter()
                .any(|c| c.symbol() == "界")
        );
    }

    #[test]
    fn folder_navigation_reuses_displayed_rows_and_release_keeps_selection() {
        let mut browser = entry_fixture(3, 1);
        let mut view = folder_view();
        let rows = std::rc::Rc::new(browser.folder_rows());
        browser.folder = rows[0].filter.clone();
        view.folder_rows_snapshot = Some(rows.clone());
        assert!(std::rc::Rc::ptr_eq(&rows, &view.folder_rows(&browser)));
        press(&mut browser, &mut view, KeyCode::Down);
        assert_eq!(browser.folder, rows[1].filter);
        let selected = browser.folder.clone();
        handle_key(
            &mut browser,
            &mut view,
            KeyEvent::new_with_kind(KeyCode::Down, KeyModifiers::NONE, KeyEventKind::Release),
        );
        assert_eq!(browser.folder, selected);
        // The next frame discards the snapshot before any model updates.
        view.folder_rows_snapshot = None;
        browser.open_folder("/new/tree/leaf".into());
        browser.include_archived = true;
        let next = view.folder_rows(&browser);
        assert!(!std::rc::Rc::ptr_eq(&rows, &next));
        assert!(
            next.iter()
                .any(|row| row.filter == FolderFilter::Path("/new/tree/leaf".into()))
        );
    }

    #[test]
    fn navigation_coalescing_is_only_for_main_lists() {
        let mut view = View::default();
        for (focus, git_focus) in [(0, false), (1, false), (1, true)] {
            view.focus = focus;
            view.git_focus = git_focus;
            assert!(view.coalesce_navigation());
            view.editing = true;
            assert!(!view.coalesce_navigation());
            view.editing = false;
            view.help = true;
            assert!(!view.coalesce_navigation());
            view.help = false;
        }
        view.git_focus = false;
        view.terminals.borrow_mut().active = Some(1);
        assert!(!view.coalesce_navigation());
        view.focus = 0;
        assert!(view.coalesce_navigation());
    }

    fn folder_view() -> View {
        let mut view = View::default();
        view.toggle_drawer();
        view
    }

    fn press(browser: &mut Browser, view: &mut View, code: KeyCode) {
        handle_key(browser, view, KeyEvent::new(code, KeyModifiers::NONE));
    }

    fn entry_fixture(count: usize, sources: usize) -> Browser {
        use crate::browser::{SourceSpec, Update};
        let mut browser = Browser::new(
            (0..sources)
                .map(|i| SourceSpec::Codex {
                    binary: "codex".into(),
                    source: crate::config::SourceConfig {
                        id: crate::domain::SourceId(format!("source-{i}")),
                        home: "/tmp".into(),
                    },
                })
                .collect(),
        );
        browser.begin_refresh();
        for source in 0..sources {
            let sessions = if source == 0 {
                (0..count).map(|i| serde_json::from_value(serde_json::json!({
                    "key": {"provider":"codex","source_id":"source-0","native_id":format!("session-{i}")},
                    "title":format!("Session {i}"), "cwd":"/tmp",
                    "created_at":1,"updated_at":2,"archived":false,"source_kind":null,"git":null
                })).unwrap()).collect()
            } else {
                vec![]
            };
            browser.apply(Update::Page {
                generation: 1,
                source,
                sessions,
                warnings: vec![],
            });
            browser.apply(Update::Finished {
                generation: 1,
                source,
                error: None,
            });
        }
        browser.open_folder("/tmp".into());
        browser
    }

    fn enter(browser: &mut Browser, view: &mut View) -> Action {
        handle_key(
            browser,
            view,
            KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
        )
    }

    #[test]
    fn folder_new_session_shortcut_and_menu_ignore_session_count() {
        for count in 0..=2 {
            let mut browser = entry_fixture(count, 1);
            let mut view = folder_view();
            let entry = menu_entries(&browser, &view)
                .into_iter()
                .find(|entry| entry.command == Command::Create)
                .unwrap();
            assert_eq!(entry.shortcut, "n");
            assert_eq!(entry.label, "Create new session");
            assert_eq!(
                handle_key(
                    &mut browser,
                    &mut view,
                    KeyEvent::new(KeyCode::Char('n'), KeyModifiers::NONE)
                ),
                Action::CreateNew(0)
            );
            assert_eq!(view.focus, 1);
            assert!(!view.drawer_open);
            assert!(!view.dialog_area);
            assert_eq!(
                handle_key(
                    &mut browser,
                    &mut view,
                    KeyEvent::new(KeyCode::Char('n'), KeyModifiers::NONE)
                ),
                Action::CreateNew(0)
            );

            view.toggle_drawer();
            open_help(&browser, &mut view);
            view.help_selected = menu_entries(&browser, &view)
                .iter()
                .position(|entry| entry.command == Command::Create)
                .unwrap();
            assert_eq!(enter(&mut browser, &mut view), Action::CreateNew(0));
            assert!(!view.drawer_open);
            assert_eq!(view.focus, 1);
        }
    }

    #[test]
    fn folder_new_session_selects_source_and_cancels() {
        let mut browser = entry_fixture(2, 2);
        let mut view = folder_view();
        press(&mut browser, &mut view, KeyCode::Char('n'));
        assert_eq!(view.source_picker, Some(0));
        assert!(!view.drawer_open);
        assert!(!view.dialog_area);
        assert_eq!(view.creation_folder, Some("/tmp".into()));
        press(&mut browser, &mut view, KeyCode::Down);
        assert_eq!(enter(&mut browser, &mut view), Action::CreateNew(1));

        view.toggle_drawer();
        press(&mut browser, &mut view, KeyCode::Char('n'));
        assert_eq!(
            handle_key(
                &mut browser,
                &mut view,
                KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)
            ),
            Action::Continue
        );
        assert!(view.source_picker.is_none());
        assert!(!view.drawer_open);
        assert_eq!(view.focus, 1);
    }

    #[test]
    fn folder_new_session_respects_unavailable_states() {
        for mode in 0..3 {
            let mut browser = entry_fixture(1, 1);
            match mode {
                0 => browser.sources[0].loading = true,
                1 => browser.specs.clear(),
                _ => browser.folder = FolderFilter::Unknown,
            }
            let mut view = folder_view();
            assert!(unavailable(Command::Create, &browser, &view).is_some());
            assert!(
                menu_entries(&browser, &view)
                    .iter()
                    .any(|entry| entry.command == Command::Create)
            );
            assert_eq!(
                handle_key(
                    &mut browser,
                    &mut view,
                    KeyEvent::new(KeyCode::Char('n'), KeyModifiers::NONE)
                ),
                Action::Continue
            );
            assert!(view.message.is_some());
            assert!(view.drawer_open);
            assert_eq!(view.focus, 0);
            assert!(view.source_picker.is_none());
        }
    }

    #[test]
    fn folder_entry_dispatches_by_count_and_clears_search() {
        for count in 0..=2 {
            let mut browser = entry_fixture(count, 1);
            browser.query = "does not match".into();
            let mut view = folder_view();
            let action = enter(&mut browser, &mut view);
            assert_eq!(
                action,
                match count {
                    0 => Action::CreateNew(0),
                    1 => Action::ResumeSelected,
                    _ => Action::Continue,
                }
            );
            assert_eq!(view.focus, 1);
            assert!(!view.drawer_open);
            assert!(browser.query.is_empty());
            if count == 1 {
                assert_eq!(browser.selected, Some(browser.visible()[0].key.clone()));
            }
        }
    }

    #[test]
    fn folder_entry_waits_for_complete_inventory() {
        for count in 0..=1 {
            for mode in 0..3 {
                let mut browser = entry_fixture(count, 1);
                match mode {
                    0 => browser.sources[0].loading = true,
                    1 => browser.sources[0].error = Some("offline".into()),
                    _ => browser.sources[0].warnings.push("incomplete".into()),
                }
                let mut view = folder_view();
                assert_eq!(enter(&mut browser, &mut view), Action::Continue);
                assert_eq!(view.focus, 1);
                assert!(!view.drawer_open);
                assert!(view.source_picker.is_none());
            }
        }
    }

    #[test]
    fn folder_entry_preserves_creation_intent_through_source_picker() {
        let mut browser = entry_fixture(0, 2);
        let mut view = folder_view();
        assert_eq!(enter(&mut browser, &mut view), Action::Continue);
        assert_eq!(view.source_picker, Some(0));
        press(&mut browser, &mut view, KeyCode::Down);
        assert_eq!(enter(&mut browser, &mut view), Action::CreateNew(1));

        view.focus = 0;
        enter(&mut browser, &mut view);
        press(&mut browser, &mut view, KeyCode::Esc);
        assert!(view.source_picker.is_none());
        assert_eq!(
            execute_command(Command::Create, &mut browser, &mut view),
            Action::Continue
        );
        assert_eq!(enter(&mut browser, &mut view), Action::CreateNew(0));
        let mut browser = entry_fixture(0, 1);
        assert_eq!(
            execute_command(Command::Create, &mut browser, &mut view),
            Action::CreateNew(0)
        );
    }

    #[test]
    fn folder_entry_respects_archives_aggregate_rows_and_repeat() {
        let mut browser = entry_fixture(1, 1);
        let mut session = browser.all().remove(0);
        session.archived = true;
        browser.begin_refresh();
        browser.apply(crate::browser::Update::Page {
            generation: 2,
            source: 0,
            sessions: vec![session],
            warnings: vec![],
        });
        browser.apply(crate::browser::Update::Finished {
            generation: 2,
            source: 0,
            error: None,
        });
        let mut view = folder_view();
        assert_eq!(enter(&mut browser, &mut view), Action::CreateNew(0));
        browser.include_archived = true;
        view = folder_view();
        assert_eq!(enter(&mut browser, &mut view), Action::ResumeSelected);
        browser.folder = FolderFilter::All;
        view = folder_view();
        assert_eq!(enter(&mut browser, &mut view), Action::Continue);
        browser.open_folder("/tmp".into());
        view = folder_view();
        assert_eq!(
            handle_key(
                &mut browser,
                &mut view,
                KeyEvent::new_with_kind(KeyCode::Enter, KeyModifiers::NONE, KeyEventKind::Repeat)
            ),
            Action::Continue
        );
        assert_eq!(view.focus, 0);
    }

    #[tokio::test]
    async fn git_worktree_entry_propagates_session_actions() {
        for picker in [false, true] {
            for count in 0..=2 {
                let (navigation, mut view) = navigation_fixture(if picker {
                    &["selected-oid", "selected-oid"]
                } else {
                    &["selected-oid"]
                })
                .await;
                let mut browser = entry_fixture(count, 1);
                for mut repository in navigation.repositories.into_values() {
                    for worktree in &mut repository.worktrees {
                        worktree.path = "/tmp".into();
                    }
                    browser.install_repository(repository);
                }
                let mut action = enter(&mut browser, &mut view);
                if picker {
                    assert_eq!(action, Action::Continue);
                    assert!(view.worktree_picker.is_some());
                    action = enter(&mut browser, &mut view);
                }
                assert_eq!(
                    action,
                    match count {
                        0 => Action::CreateNew(0),
                        1 => Action::ResumeSelected,
                        _ => Action::Continue,
                    }
                );
                assert_eq!(view.context(), 1);
                assert!(!view.drawer_open);
            }
        }
    }

    #[tokio::test]
    async fn git_preview_renders_selected_folder_list_and_restores_original_scope() {
        use crate::git_sessions::tests::{add_session, fixture};
        let (mut browser, history) = fixture().await;
        select_git_fixture(&mut browser, "/repo/.git");
        let key = add_session(
            &mut browser,
            "REMOTE_PREVIEW_TARGET",
            "/worktree/src",
            "/worktree",
            "/repo/.git",
            crate::domain::RuntimeStatus::Running,
            1,
        );
        browser.folder = FolderFilter::Path("/unrelated".into());
        browser.query = "does not match".into();
        browser.reconcile(0);
        let original = (
            browser.folder.clone(),
            browser.query.clone(),
            browser.selected.clone(),
        );
        let mut view = View {
            history,
            git_focus: true,
            drawer_git: true,
            ..folder_view()
        };
        press(&mut browser, &mut view, KeyCode::Down);
        sync_terminal_view(&browser, &view);
        assert_eq!(preview_session(&browser, &view).unwrap().key, key);
        let mut terminal = Terminal::new(ratatui::backend::TestBackend::new(90, 20)).unwrap();
        terminal
            .draw(|frame| draw_pane(frame, frame.area(), 1, &browser, &view))
            .unwrap();
        let text: String = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|c| c.symbol())
            .collect();
        assert!(text.contains("REMOTE_PREVIEW_TARGET"));
        assert!(text.contains("/worktree/src"));
        assert!(text.contains("> "));
        assert_eq!(
            (
                browser.folder.clone(),
                browser.query.clone(),
                browser.selected.clone()
            ),
            original
        );
        press(&mut browser, &mut view, KeyCode::Up);
        sync_terminal_view(&browser, &view);
        terminal
            .draw(|frame| draw_pane(frame, frame.area(), 1, &browser, &view))
            .unwrap();
        let text: String = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|c| c.symbol())
            .collect();
        assert!(!text.contains("REMOTE_PREVIEW_TARGET"));
        assert_eq!(
            (
                browser.folder.clone(),
                browser.query.clone(),
                browser.selected.clone()
            ),
            original
        );
    }

    #[tokio::test]
    async fn git_highlight_previews_and_restores_folder_terminal() {
        use crate::git_sessions::tests::{add_session, fixture};
        let (mut browser, history) = fixture().await;
        let temp = tempfile::tempdir().unwrap();
        let key = add_session(
            &mut browser,
            "Preview session",
            "/worktree/src",
            "/worktree",
            "/repo/.git",
            crate::domain::RuntimeStatus::Running,
            1,
        );
        let mut view = View {
            history,
            git_focus: true,
            drawer_git: true,
            ..folder_view()
        };
        let command = crate::domain::LaunchCommand {
            executable: "/usr/bin/python3".into(),
            args: vec!["-c".into(), "import time; time.sleep(60)".into()],
            cwd: temp.path().into(),
            env: vec![("CODEX_HOME".into(), temp.path().as_os_str().to_owned())],
        };
        view.terminals
            .borrow_mut()
            .launch(&command, Some(key.clone()), "Preview".into())
            .unwrap();
        let preview = view.terminals.borrow().active;
        view.terminals
            .borrow_mut()
            .launch(&command, None, "Original".into())
            .unwrap();
        let original = view.terminals.borrow().active;
        browser.folder = FolderFilter::Path(temp.path().into());
        browser.reconcile(0);
        let folder = browser.folder.clone();
        let selected = browser.selected.clone();
        press(&mut browser, &mut view, KeyCode::Down);
        assert_eq!(
            selected_git_session(&browser, &view).unwrap().session.key,
            key
        );
        sync_terminal_view(&browser, &view);
        assert_eq!(view.terminals.borrow().active, preview);
        assert!(!view.terminal_focused());
        assert_eq!(browser.folder, folder);
        assert_eq!(browser.selected, selected);
        press(&mut browser, &mut view, KeyCode::Up);
        sync_terminal_view(&browser, &view);
        assert_eq!(view.terminals.borrow().active, original);
        press(&mut browser, &mut view, KeyCode::Down);
        sync_terminal_view(&browser, &view);
        assert_eq!(view.terminals.borrow().active, preview);
        press(&mut browser, &mut view, KeyCode::Esc);
        sync_terminal_view(&browser, &view);
        assert_eq!(view.terminals.borrow().active, original);
        view.toggle_drawer();
        sync_terminal_view(&browser, &view);
        assert_eq!(view.terminals.borrow().active, preview);
        view.drawer_git = false;
        view.normalize_focus();
        sync_terminal_view(&browser, &view);
        assert_eq!(view.terminals.borrow().active, original);
        view.drawer_git = true;
        view.normalize_focus();
        view.git_sessions.borrow_mut().follow();
        view.history.follow();
        sync_terminal_view(&browser, &view);
        assert_eq!(view.terminals.borrow().active, original);
        view.terminals.get_mut().shutdown().await;
    }

    #[tokio::test]
    async fn git_session_rows_render_and_enter_relinks_missing_folder() {
        use crate::git_sessions::tests::{add_session, fixture};
        let (mut browser, history) = fixture().await;
        select_git_fixture(&mut browser, "/repo/.git");
        let key = add_session(
            &mut browser,
            "Implement login 界",
            "/worktree/src",
            "/worktree",
            "/repo/.git",
            crate::domain::RuntimeStatus::Waiting {
                approval: false,
                input: true,
            },
            1,
        );
        let mut view = View {
            history,
            git_focus: true,
            drawer_git: true,
            ..folder_view()
        };
        browser.query = "blocking query".into();
        browser.expanded.clear();
        press(&mut browser, &mut view, KeyCode::Down);
        assert!(selected_git_session(&browser, &view).is_some());
        assert!(
            !menu_entries(&browser, &view)
                .iter()
                .any(|e| e.command == Command::CreateWorktree)
        );
        assert!(
            menu_entries(&browser, &view)
                .iter()
                .any(|e| e.label == "Resume session")
        );
        let mut annotation = selected_git_session(&browser, &view).unwrap();
        let mut name_column = None;
        let widths = GitSessionWidths {
            status: 8,
            title: 0,
        };
        for status in [
            crate::domain::RuntimeStatus::Idle,
            crate::domain::RuntimeStatus::Running,
            crate::domain::RuntimeStatus::Error,
            crate::domain::RuntimeStatus::Waiting {
                approval: true,
                input: false,
            },
            crate::domain::RuntimeStatus::Waiting {
                approval: false,
                input: true,
            },
            crate::domain::RuntimeStatus::Waiting {
                approval: true,
                input: true,
            },
        ] {
            annotation.status = status;
            let spans = git_session_spans(&view, 0, None, &annotation, true, false, widths);
            assert_eq!(
                spans
                    .iter()
                    .find(|span| span.content.contains('└'))
                    .unwrap()
                    .style
                    .fg,
                Some(Color::White)
            );
            let line: String = spans.iter().map(|span| span.content.as_ref()).collect();
            let title_start = line.find(&annotation.session.title).unwrap();
            let column = unicode_width::UnicodeWidthStr::width(&line[..title_start]);
            let expected = match status {
                crate::domain::RuntimeStatus::Idle => "Idle",
                crate::domain::RuntimeStatus::Running => "Running",
                crate::domain::RuntimeStatus::Error => "Error",
                crate::domain::RuntimeStatus::Waiting {
                    approval: true,
                    input: true,
                } => "Waiting",
                crate::domain::RuntimeStatus::Waiting { approval: true, .. } => "Approval",
                _ => "Input",
            };
            assert!(line[..title_start].ends_with(&format!("{expected:>8} └─ ")));
            let continuing: String =
                git_session_spans(&view, 0, None, &annotation, false, false, widths)
                    .iter()
                    .map(|span| span.content.as_ref())
                    .collect();
            assert_eq!(continuing, line.replace(" └─ ", " ├─ "));
            let prefix: String = view
                .history
                .session_spans(0, None)
                .iter()
                .map(|span| span.content.as_ref())
                .collect();
            assert_eq!(
                column,
                unicode_width::UnicodeWidthStr::width(prefix.as_str()) + 12
            );
            assert_eq!(*name_column.get_or_insert(column), column);
        }
        for (width, height) in [(119, 40), (80, 24), (60, 20), (40, 16), (12, 9)] {
            let mut terminal =
                Terminal::new(ratatui::backend::TestBackend::new(width, height)).unwrap();
            terminal.draw(|frame| draw(frame, &browser, &view)).unwrap();
            if width == 119 {
                let text: String = terminal
                    .backend()
                    .buffer()
                    .content
                    .iter()
                    .map(|c| c.symbol())
                    .collect();
                assert!(text.contains("Implement login 界"));
                assert!(text.contains("codex · feature"));
                assert!(text.contains("Input └─ Implement login 界"));
            }
        }
        assert!(matches!(
            handle_key(
                &mut browser,
                &mut view,
                KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)
            ),
            Action::OpenRelink
        ));
        assert_eq!(view.context(), 1);
        assert!(!view.drawer_open);
        assert_eq!(browser.folder, FolderFilter::Path("/worktree/src".into()));
        assert_eq!(browser.selected, Some(key));
        assert!(browser.query.is_empty());
        assert!(browser.expanded.contains(std::path::Path::new("/repo")));
        assert!(
            browser
                .folder_rows()
                .iter()
                .any(|r| r.filter == browser.folder)
        );
    }

    #[tokio::test]
    async fn git_session_enter_resumes_selected_session_in_one_step() {
        use crate::git::{RefKind, RefLabel};
        use crate::git_sessions::tests::{add_session, fixture};
        let cwd = tempfile::tempdir().unwrap();
        for status in [
            crate::domain::RuntimeStatus::Idle,
            crate::domain::RuntimeStatus::Running,
            crate::domain::RuntimeStatus::Waiting {
                approval: false,
                input: true,
            },
        ] {
            let (mut browser, mut history) = fixture().await;
            history.set_references(std::collections::BTreeMap::from([(
                "head".into(),
                vec![RefLabel {
                    kind: RefKind::Local,
                    name: "feature".into(),
                }],
            )]));
            let key = add_session(
                &mut browser,
                "Direct resume",
                cwd.path().to_str().unwrap(),
                "/worktree",
                "/repo/.git",
                status,
                1,
            );
            let mut view = View {
                history,
                git_focus: true,
                drawer_git: true,
                ..folder_view()
            };
            press(&mut browser, &mut view, KeyCode::Down);
            assert!(matches!(
                handle_key(
                    &mut browser,
                    &mut view,
                    KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)
                ),
                Action::ResumeSelected
            ));
            assert_eq!(browser.selected, Some(key));
            assert_eq!(browser.folder, FolderFilter::Path(cwd.path().into()));
            assert_eq!(view.context(), 1);
            assert!(!view.drawer_open);
        }
    }

    #[tokio::test]
    async fn git_history_renders_sessions_under_their_own_branch() {
        use crate::domain::RuntimeStatus;
        use crate::git::{RefKind, RefLabel};
        use crate::git_sessions::tests::{add_session, fixture};
        let (mut browser, mut history) = fixture().await;
        select_git_fixture(&mut browser, "/repo/.git");
        let repo = browser
            .repositories
            .get_mut(std::path::Path::new("/repo/.git"))
            .unwrap();
        repo.worktrees.push(crate::git::Worktree {
            path: "/detached".into(),
            branch: None,
            ..repo.worktrees[1].clone()
        });
        for (title, cwd, root, status) in [
            ("Main session", "/repo", "/repo", RuntimeStatus::Running),
            (
                "Feature runner",
                "/worktree",
                "/worktree",
                RuntimeStatus::Running,
            ),
            (
                "Feature input",
                "/worktree/src",
                "/worktree",
                RuntimeStatus::Waiting {
                    approval: false,
                    input: true,
                },
            ),
            (
                "Detached session",
                "/detached",
                "/detached",
                RuntimeStatus::Idle,
            ),
        ] {
            add_session(&mut browser, title, cwd, root, "/repo/.git", status, 1);
        }
        history.set_references(std::collections::BTreeMap::from([(
            "head".into(),
            [
                (RefKind::Local, "feature"),
                (RefKind::Local, "docs"),
                (RefKind::Local, "release"),
                (RefKind::Local, "master"),
                (RefKind::Local, "main"),
                (RefKind::Tag, "v1"),
            ]
            .into_iter()
            .map(|(kind, name)| RefLabel {
                kind,
                name: name.into(),
            })
            .collect(),
        )]));
        let view = View {
            history,
            git_focus: true,
            drawer_git: true,
            ..folder_view()
        };
        let mut terminal = Terminal::new(ratatui::backend::TestBackend::new(119, 40)).unwrap();
        terminal.draw(|frame| draw(frame, &browser, &view)).unwrap();
        let lines: Vec<String> = terminal
            .backend()
            .buffer()
            .content
            .chunks(119)
            .map(|row| row.iter().map(|cell| cell.symbol()).collect())
            .collect();
        let row = |needle: &str| {
            assert_eq!(
                lines.iter().filter(|line| line.contains(needle)).count(),
                1,
                "{needle}"
            );
            lines.iter().position(|line| line.contains(needle)).unwrap()
        };
        let master = row("[master]");
        assert_eq!(row("[docs]"), master + 1);
        assert_eq!(row("[release]"), master + 2);
        assert_eq!(row("[tag: v1]"), master + 3);
        let main = row("[main]");
        assert_eq!(main, master + 4);
        assert_eq!(row("Main session"), main + 1);
        assert!(lines[main + 1].contains("└─"));
        let feature = row("[feature]");
        assert_eq!(feature, main + 2);
        assert_eq!(row("Feature input"), feature + 1);
        assert!(lines[feature + 1].contains("├─"));
        assert_eq!(row("Feature runner"), feature + 2);
        assert!(lines[feature + 2].contains("└─"));
        assert_eq!(row("Detached session"), feature + 3);
    }

    #[tokio::test]
    async fn git_session_columns_follow_live_widths_and_stay_aligned_across_scroll() {
        use crate::domain::RuntimeStatus;
        use crate::git::{RefKind, RefLabel};
        use crate::git_sessions::tests::{add_session, fixture};
        use crate::git_sessions::{Presentation, Row};
        use unicode_width::UnicodeWidthStr;
        let (mut browser, mut history) = fixture().await;
        let [_, changing, _] = [
            ("Short", "/repo"),
            ("Longer 界界 name", "/worktree"),
            ("e\u{301} task", "/repo"),
        ]
        .map(|(title, root)| {
            add_session(
                &mut browser,
                title,
                root,
                root,
                "/repo/.git",
                RuntimeStatus::Idle,
                1,
            )
        });
        history.set_references(std::collections::BTreeMap::from([(
            "head".into(),
            vec![RefLabel {
                kind: RefKind::Local,
                name: "main".into(),
            }],
        )]));
        browser
            .repositories
            .get_mut(std::path::Path::new("/repo/.git"))
            .unwrap()
            .worktrees[1]
            .head = Some("older".into());
        let view = View {
            history,
            ..folder_view()
        };
        let mut presentation = view.git_sessions.borrow_mut();
        presentation.sync(&browser, &view.history);
        let provider_column = |presentation: &Presentation, widths| {
            let mut aligned = None;
            for row in &presentation.rows {
                if let Row::Session {
                    anchor,
                    annotation,
                    connector,
                    heading,
                    last,
                } = row
                {
                    let spans = git_session_spans(
                        &view,
                        *anchor,
                        *connector,
                        &presentation.annotations[*annotation],
                        *last,
                        heading.is_some(),
                        widths,
                    );
                    let text: String = spans.iter().map(|s| s.content.as_ref()).collect();
                    let column = text[..text.find("codex").unwrap()].width();
                    assert_eq!(*aligned.get_or_insert(column), column);
                }
            }
            aligned.unwrap()
        };
        let idle = git_session_widths(&presentation);
        assert_eq!(idle.status, 4);
        assert_eq!(idle.title, "Longer 界界 name".width());
        let idle_column = provider_column(&presentation, idle);
        assert!(
            presentation
                .rows
                .iter()
                .any(|row| matches!(row, Row::Session { heading: None, .. }))
        );

        browser.sources[0]
            .live
            .insert(changing.clone(), RuntimeStatus::Running);
        presentation.sync(&browser, &view.history);
        let mixed = git_session_widths(&presentation);
        assert_eq!(mixed.status, 7);
        assert_eq!(provider_column(&presentation, mixed), idle_column + 3);

        presentation.selected = presentation
            .rows
            .iter()
            .position(|row| {
                matches!(
                    row,
                    Row::Session {
                        heading: Some(_),
                        ..
                    }
                )
            })
            .unwrap();
        let visible = presentation.range(1);
        let Row::Session { annotation, .. } = presentation.rows[visible.start] else {
            panic!("expected an idle branch session to be visible");
        };
        assert_eq!(
            presentation.annotations[annotation].status,
            RuntimeStatus::Idle
        );
        assert_eq!(git_session_widths(&presentation), mixed);

        browser.sources[0]
            .live
            .insert(changing, RuntimeStatus::Idle);
        presentation.sync(&browser, &view.history);
        assert_eq!(git_session_widths(&presentation), idle);
        assert_eq!(provider_column(&presentation, idle), idle_column);
    }

    #[tokio::test]
    async fn zoom_details_follow_selection_and_keep_focus_local() {
        let (mut browser, mut view) = navigation_fixture(&["selected-oid"]).await;
        view.git_zoomed = true;
        sync_git_details(&browser, &mut view);
        assert!(view.git_details.is_some());
        assert!(!view.modal_open());
        view.git_details.as_mut().unwrap().dialog.scroll = 5;
        sync_git_details(&browser, &mut view);
        assert_eq!(view.git_details.as_ref().unwrap().dialog.scroll, 5);
        press(&mut browser, &mut view, KeyCode::Char('i'));
        assert!(view.git_details_focused);
        assert!(view.commit_info.is_none());
        view.git_details
            .as_mut()
            .unwrap()
            .dialog
            .scroll_limit
            .set(100);
        press(&mut browser, &mut view, KeyCode::Down);
        assert_eq!(view.git_details.as_ref().unwrap().dialog.scroll, 6);
        assert_eq!(view.history.selected, 0);
        press(&mut browser, &mut view, KeyCode::Char('?'));
        assert!(view.help);
        press(&mut browser, &mut view, KeyCode::Esc);
        assert!(view.git_details_focused);
        for code in [KeyCode::Esc, KeyCode::Backspace, KeyCode::Char('i')] {
            press(&mut browser, &mut view, code);
            assert!(!view.git_details_focused);
            assert!(view.git_zoomed);
            press(&mut browser, &mut view, KeyCode::Char('i'));
        }
        view.history.rows[0].oid = Some("new-oid".into());
        sync_git_details(&browser, &mut view);
        assert_eq!(view.git_details_key.as_ref().unwrap().1, "new-oid");
        assert_eq!(view.git_details.as_ref().unwrap().dialog.scroll, 0);
        // Old asynchronous completions cannot replace a new selection's dialog.
        tokio::task::yield_now().await;
        view.git_details.as_mut().unwrap().tick().await;
        assert_eq!(view.git_details_key.as_ref().unwrap().1, "new-oid");
        handle_key(
            &mut browser,
            &mut view,
            KeyEvent::new(KeyCode::Char('z'), KeyModifiers::ALT),
        );
        assert!(!view.git_zoomed);
        assert!(view.git_details.is_none());
        assert!(!view.git_details_focused);
        view.git_zoomed = true;
        sync_git_details(&browser, &mut view);
        view.history.rows.clear();
        sync_git_details(&browser, &mut view);
        assert!(view.git_details.is_none());
        assert!(view.git_details_key.is_none());
    }

    #[tokio::test]
    async fn commit_info_shortcut_opens_modal_and_preserves_selection_on_close() {
        let (mut browser, mut view) = navigation_fixture(&["selected-oid"]).await;
        assert!(
            menu_entries(&browser, &view)
                .iter()
                .any(|entry| entry.command == Command::CommitInfo)
        );
        let key = |code| KeyEvent::new(code, KeyModifiers::NONE);
        handle_key(&mut browser, &mut view, key(KeyCode::Char('i')));
        assert!(view.commit_info.is_some());
        assert!(view.modal_open());
        handle_key(&mut browser, &mut view, key(KeyCode::Down));
        assert_eq!(view.history.selected, 0);
        handle_key(&mut browser, &mut view, key(KeyCode::Esc));
        assert!(view.commit_info.is_none());
        assert!(view.git_focus);
        handle_key(&mut browser, &mut view, key(KeyCode::Char('i')));
        handle_key(&mut browser, &mut view, key(KeyCode::Char('i')));
        assert!(view.commit_info.is_none());
        view.history.rows.clear();
        handle_key(&mut browser, &mut view, key(KeyCode::Char('i')));
        assert!(view.commit_info.is_none());
    }

    async fn navigation_fixture(heads: &[&str]) -> (Browser, View) {
        let mut browser = Browser::new(vec![]);
        let repository = std::path::PathBuf::from("/project/.git");
        browser.install_repository(crate::git::Repository {
            common_dir: repository.clone(),
            git_dir: repository.clone(),
            worktrees: heads
                .iter()
                .enumerate()
                .map(|(i, head)| crate::git::Worktree {
                    path: if i == 0 {
                        "/project".into()
                    } else {
                        format!("/elsewhere/worktree-{i}").into()
                    },
                    head: Some((*head).into()),
                    branch: (i == 0).then(|| "refs/heads/main".into()),
                    bare: false,
                    locked: false,
                    prunable: false,
                })
                .collect(),
        });
        browser.expanded.clear();
        let mut view = View {
            git_focus: true,
            drawer_git: true,
            focus: 0,
            ..folder_view()
        };
        view.history.tick(Some(repository), 0, false, 10).await;
        view.history.rows = vec![crate::git::GraphRow {
            stash: None,
            oid: Some("selected-oid".into()),
            graph: String::new(),
            graph_spans: vec![],
            parents: vec![],
            subject: "Selected commit".into(),
        }];
        (browser, view)
    }

    #[tokio::test]
    async fn git_history_wraps_refs_and_scrolls_through_the_last_commit() {
        use crate::git::{RefKind, RefLabel};
        let (mut browser, mut view) = navigation_fixture(&["selected-oid"]).await;
        select_git_fixture(&mut browser, "/project/.git");
        let branches: Vec<_> = (0..10)
            .map(|i| format!("feature/branch-number-{i}"))
            .collect();
        view.history
            .set_references(std::collections::BTreeMap::from([(
                "selected-oid".into(),
                branches
                    .iter()
                    .map(|name| RefLabel {
                        kind: RefKind::Local,
                        name: name.clone(),
                    })
                    .collect(),
            )]));
        let render = |terminal: &mut Terminal<ratatui::backend::TestBackend>,
                      browser: &Browser,
                      view: &View| {
            terminal.draw(|frame| draw(frame, browser, view)).unwrap();
            terminal
                .backend()
                .buffer()
                .content
                .iter()
                .map(|cell| cell.symbol())
                .collect::<String>()
        };
        let mut wide = Terminal::new(ratatui::backend::TestBackend::new(110, 35)).unwrap();
        let text = render(&mut wide, &browser, &view);
        let subject = text.find("Selected commit").unwrap();
        assert!(
            branches
                .iter()
                .all(|name| text.find(name).unwrap() > subject)
        );

        let mut narrow = Terminal::new(ratatui::backend::TestBackend::new(38, 11)).unwrap();
        let mut seen = render(&mut narrow, &browser, &view);
        assert!(seen.contains("Selected commit"));
        assert!(!seen.contains(&branches[9]));
        for _ in 0..branches.len() {
            press(&mut browser, &mut view, KeyCode::Down);
            seen.push_str(&render(&mut narrow, &browser, &view));
        }
        assert!(branches.iter().all(|name| seen.contains(name)));
        assert_eq!(view.history.selected, 0);
        assert_eq!(selected_commit(&browser, &view).unwrap().1, "selected-oid");
        press(&mut browser, &mut view, KeyCode::Up);
        assert!(render(&mut narrow, &browser, &view).contains("Selected commit"));
    }

    #[tokio::test]
    async fn stash_selection_is_read_only_and_cannot_create_a_worktree() {
        let (mut browser, mut view) = navigation_fixture(&["selected-oid"]).await;
        view.history.rows[0].stash = Some(crate::git::Stash {
            selectors: vec!["stash@{0}".into()],
            base: "a".repeat(40),
        });
        assert!(
            worktree_unavailable(&browser, &view)
                .unwrap()
                .contains("rather than a stash")
        );
        assert!(matches!(
            execute_command(Command::GitSelectWorktree, &mut browser, &mut view),
            Action::Continue
        ));
        assert!(
            view.message
                .as_ref()
                .unwrap()
                .contains("Saved stash stash@{0}")
        );
        assert!(view.git_focus);
        assert!(view.worktree_picker.is_none());
    }

    #[tokio::test]
    async fn git_enter_reveals_linked_folder_and_enters_sessions() {
        let (mut browser, mut view) = navigation_fixture(&["other", "selected-oid"]).await;
        press(&mut browser, &mut view, KeyCode::Enter);
        assert_eq!(
            browser.folder,
            FolderFilter::Path("/elsewhere/worktree-1".into())
        );
        assert!(
            browser
                .folder_rows()
                .iter()
                .any(|row| row.filter == browser.folder)
        );
        assert!(browser.expanded.contains(std::path::Path::new("/project")));
        assert_eq!(view.context(), 1);
        assert!(!view.drawer_open);
        assert!(browser.visible().is_empty());
        assert!(browser.selected.is_none());
        assert_eq!(
            view.history.rows[view.history.selected].oid.as_deref(),
            Some("selected-oid")
        );
    }

    #[tokio::test]
    async fn git_enter_chooser_confirms_cancels_and_renders() {
        let (mut browser, mut view) = navigation_fixture(&["selected-oid", "selected-oid"]).await;
        let original = browser.folder.clone();
        press(&mut browser, &mut view, KeyCode::Enter);
        assert_eq!(browser.folder, original);
        assert!(view.worktree_picker.is_some());
        for width in [40, 100] {
            let mut terminal =
                Terminal::new(ratatui::backend::TestBackend::new(width, 24)).unwrap();
            terminal.draw(|frame| draw(frame, &browser, &view)).unwrap();
            let rendered: String = terminal
                .backend()
                .buffer()
                .content
                .iter()
                .map(|cell| cell.symbol())
                .collect();
            assert!(rendered.contains("Choose worktree"));
            assert!(rendered.contains("/elsewhere/worktree-1"));
            assert!(rendered.contains("detached"));
            assert!(rendered.contains("/project"));
        }
        press(&mut browser, &mut view, KeyCode::Esc);
        assert!(view.worktree_picker.is_none());
        assert_eq!(browser.folder, original);
        assert!(view.git_focus);
        press(&mut browser, &mut view, KeyCode::Enter);
        press(&mut browser, &mut view, KeyCode::Char('j'));
        press(&mut browser, &mut view, KeyCode::Enter);
        assert_eq!(browser.folder, FolderFilter::Path("/project".into()));
        assert_eq!(view.context(), 1);
        assert!(!view.drawer_open);
    }

    #[tokio::test]
    async fn git_enter_revalidates_chooser_and_rejects_stale_inventory() {
        let (mut browser, mut view) = navigation_fixture(&["selected-oid", "selected-oid"]).await;
        let original = browser.folder.clone();
        press(&mut browser, &mut view, KeyCode::Enter);
        let repo = browser.repositories.values_mut().next().unwrap();
        repo.worktrees[1].head = Some("changed".into());
        press(&mut browser, &mut view, KeyCode::Enter);
        assert_eq!(browser.folder, original);
        assert!(view.git_focus);
        assert!(view.message.as_ref().unwrap().contains("changed"));
        browser
            .repository_errors
            .insert("/project/.git".into(), "refreshing".into());
        press(&mut browser, &mut view, KeyCode::Enter);
        assert_eq!(browser.folder, original);
        assert!(view.message.as_ref().unwrap().contains("stale"));
        browser.repository_errors.clear();
        browser.repositories.clear();
        press(&mut browser, &mut view, KeyCode::Enter);
        assert!(view.message.as_ref().unwrap().contains("unavailable"));
    }

    #[tokio::test]
    async fn git_enter_ignores_unmatched_bare_size_limited_and_repeat_events() {
        let (mut browser, mut view) = navigation_fixture(&["other"]).await;
        let original = browser.folder.clone();
        let mut unrelated = browser.repositories.values().next().unwrap().clone();
        unrelated.common_dir = "/unrelated/.git".into();
        unrelated.worktrees[0].head = Some("selected-oid".into());
        browser.install_repository(unrelated);
        press(&mut browser, &mut view, KeyCode::Enter);
        assert!(view.message.as_ref().unwrap().contains("No worktree"));
        let repo = browser
            .repositories
            .get_mut(std::path::Path::new("/project/.git"))
            .unwrap();
        repo.worktrees[0].head = Some("selected-oid".into());
        repo.worktrees[0].bare = true;
        press(&mut browser, &mut view, KeyCode::Enter);
        assert_eq!(browser.folder, original);
        browser
            .repositories
            .get_mut(std::path::Path::new("/project/.git"))
            .unwrap()
            .worktrees[0]
            .bare = false;
        view.message = None;
        view.git_size_limited = true;
        press(&mut browser, &mut view, KeyCode::Enter);
        view.git_size_limited = false;
        handle_key(
            &mut browser,
            &mut view,
            KeyEvent::new_with_kind(KeyCode::Enter, KeyModifiers::NONE, KeyEventKind::Repeat),
        );
        assert_eq!(browser.folder, original);
        assert!(view.git_focus);
        assert!(view.message.is_none());
        view.history.rows[0].oid = None;
        press(&mut browser, &mut view, KeyCode::Enter);
        assert_eq!(browser.folder, original);
    }

    #[tokio::test]
    async fn right_closes_either_drawer_panel_without_changing_selection() {
        let (mut browser, mut view) = navigation_fixture(&["selected-oid"]).await;
        let key = KeyEvent::new(KeyCode::Right, KeyModifiers::NONE);
        for git in [false, true] {
            if !view.drawer_open {
                view.toggle_drawer();
            }
            view.drawer_git = git;
            view.normalize_focus();
            let folder = browser.folder.clone();
            let selected = browser.selected.clone();
            let expanded = browser.expanded.clone();
            view.editing = true;
            handle_key(&mut browser, &mut view, key);
            assert!(view.drawer_open);
            view.editing = false;
            for kind in [KeyEventKind::Repeat, KeyEventKind::Release] {
                handle_key(&mut browser, &mut view, KeyEvent { kind, ..key });
                assert!(view.drawer_open);
            }
            handle_key(&mut browser, &mut view, key);
            assert!(!view.drawer_open);
            assert!(view.terminal_focused());
            assert_eq!(browser.folder, folder);
            assert_eq!(browser.selected, selected);
            assert_eq!(browser.expanded, expanded);
        }
    }

    #[test]
    fn l_expands_without_leaving_drawer_and_left_follows_displayed_parent() {
        for (left, right) in [
            (KeyCode::Left, KeyCode::Char('l')),
            (KeyCode::Char('h'), KeyCode::Char('l')),
        ] {
            let mut browser = Browser::new(vec![]);
            browser.open_folder("/project/child/leaf".into());
            browser.select_folder(FolderFilter::Path("/project".into()));
            let mut view = folder_view();
            let press = |browser: &mut Browser, view: &mut View, code| {
                handle_key(browser, view, KeyEvent::new(code, KeyModifiers::NONE))
            };
            press(&mut browser, &mut view, left);
            assert!(!browser.expanded.contains(std::path::Path::new("/project")));
            assert!(
                browser
                    .expanded
                    .contains(std::path::Path::new("/project/child"))
            );
            let selected = browser.selected.clone();
            press(&mut browser, &mut view, right);
            assert_eq!(browser.folder, FolderFilter::Path("/project".into()));
            assert!(browser.expanded.contains(std::path::Path::new("/project")));
            assert_eq!(view.focus, 0);
            press(&mut browser, &mut view, right);
            assert_eq!(browser.folder, FolderFilter::Path("/project".into()));
            assert_eq!(browser.selected, selected);
            assert_eq!(view.focus, 0);
            press(&mut browser, &mut view, KeyCode::Down);
            assert_eq!(browser.folder, FolderFilter::Path("/project/child".into()));
            press(&mut browser, &mut view, left);
            assert_eq!(browser.folder, FolderFilter::Path("/project/child".into()));
            press(&mut browser, &mut view, left);
            assert_eq!(browser.folder, FolderFilter::Path("/project".into()));
            browser.open_folder("/project/child/leaf".into());
            browser
                .expanded
                .remove(std::path::Path::new("/project/child/leaf"));
            press(&mut browser, &mut view, right);
            assert!(
                browser
                    .expanded
                    .contains(std::path::Path::new("/project/child/leaf"))
            );
            assert_eq!(view.focus, 0);
            assert_eq!(
                browser.folder,
                FolderFilter::Path("/project/child/leaf".into())
            );
            press(&mut browser, &mut view, right);
            assert_eq!(view.focus, 0);
            assert_eq!(
                browser.folder,
                FolderFilter::Path("/project/child/leaf".into())
            );
            press(&mut browser, &mut view, left);
            press(&mut browser, &mut view, left);
            assert_eq!(browser.folder, FolderFilter::Path("/project/child".into()));
            browser.select_folder(FolderFilter::Path("/".into()));
            press(&mut browser, &mut view, left);
            press(&mut browser, &mut view, left);
            assert_eq!(browser.folder, FolderFilter::Path("/".into()));
            for filter in [FolderFilter::Unlinked, FolderFilter::Unknown] {
                browser.select_folder(filter.clone());
                let expanded = browser.expanded.clone();
                press(&mut browser, &mut view, left);
                press(&mut browser, &mut view, right);
                assert_eq!(browser.folder, filter);
                assert_eq!(browser.expanded, expanded);
                press(&mut browser, &mut view, KeyCode::Enter);
                assert_eq!(view.focus, 0);
                assert_eq!(view.focus, 0);
            }
        }
    }

    #[test]
    fn worktree_parent_is_displayed_repository_and_enter_repeat_is_ignored() {
        let mut browser = Browser::new(vec![]);
        browser.open_folder("/project".into());
        let worktree = |path: &str| crate::git::Worktree {
            path: path.into(),
            head: None,
            branch: None,
            bare: false,
            locked: false,
            prunable: false,
        };
        browser.install_repository(crate::git::Repository {
            common_dir: "/project/.git".into(),
            git_dir: "/project/.git".into(),
            worktrees: vec![worktree("/project"), worktree("/elsewhere/feature")],
        });
        let mut view = folder_view();
        let key = |code| KeyEvent::new(code, KeyModifiers::NONE);
        handle_key(&mut browser, &mut view, key(KeyCode::Down));
        assert_eq!(
            browser.folder,
            FolderFilter::Path("/elsewhere/feature".into())
        );
        handle_key(&mut browser, &mut view, key(KeyCode::Left));
        assert_eq!(browser.folder, FolderFilter::Path("/project".into()));
        let expanded = browser.expanded.clone();
        handle_key(&mut browser, &mut view, key(KeyCode::Enter));
        assert_eq!(view.focus, 1);
        assert!(!view.drawer_open);
        assert_eq!(browser.expanded, expanded);
        assert_eq!(
            handle_key(
                &mut browser,
                &mut view,
                KeyEvent::new_with_kind(KeyCode::Enter, KeyModifiers::NONE, KeyEventKind::Repeat)
            ),
            Action::Continue
        );
        assert_eq!(
            handle_key(&mut browser, &mut view, key(KeyCode::Char('m'))),
            Action::OpenRelink
        );
        assert_eq!(
            handle_key(&mut browser, &mut view, key(KeyCode::Char('r'))),
            Action::Refresh
        );
        for code in [KeyCode::Right, KeyCode::Char('h'), KeyCode::Char('l')] {
            assert_eq!(
                handle_key(&mut browser, &mut view, key(code)),
                Action::Continue
            );
            assert_eq!(view.focus, 1);
        }
        for code in [KeyCode::Esc, KeyCode::Backspace] {
            view.focus = 1;
            handle_key(&mut browser, &mut view, key(code));
            assert_eq!(view.focus, 1);
            assert_eq!(browser.folder, FolderFilter::Path("/project".into()));
            assert_eq!(browser.expanded, expanded);
        }
    }

    #[test]
    fn worktree_action_tracks_selected_repository_availability() {
        use crate::git_discovery::{Outcome, Ownership};
        let path = std::path::PathBuf::from("/project");
        let mut browser = Browser::new(vec![crate::browser::SourceSpec::Codex {
            binary: "codex".into(),
            source: crate::config::SourceConfig {
                id: crate::domain::SourceId("test".into()),
                home: "/codex".into(),
            },
        }]);
        browser.folder = FolderFilter::Path(path.clone());
        let repository = Outcome::Repository {
            repository: crate::git::Directory {
                common_dir: path.join(".git"),
                git_dir: path.join(".git"),
            },
            root: Some(path.clone()),
        };
        let mut view = folder_view();
        for (outcome, stale, available) in [
            (None, false, false),
            (Some(Outcome::NotRepository), false, false),
            (Some(Outcome::Unavailable("offline".into())), false, false),
            (Some(repository.clone()), true, false),
            (Some(repository.clone()), false, true),
            (Some(Outcome::NotRepository), false, false),
        ] {
            browser.ownership.insert(
                path.clone(),
                Ownership {
                    generation: browser.git_generation + u64::from(stale),
                    outcome,
                    previous: Some(repository.clone()),
                },
            );
            assert_eq!(
                menu_entries(&browser, &view)
                    .iter()
                    .any(|entry| entry.command == Command::CreateWorktree),
                available
            );
            if !available {
                view.message = Some("unchanged".into());
                handle_key(
                    &mut browser,
                    &mut view,
                    KeyEvent::new(KeyCode::Char('w'), KeyModifiers::NONE),
                );
                assert!(view.worktree_create.is_none());
                assert_eq!(view.message.as_deref(), Some("unchanged"));
            }
        }
        browser.ownership.get_mut(&path).unwrap().outcome = Some(repository.clone());
        open_help(&browser, &mut view);
        view.help_selected = menu_entries(&browser, &view)
            .iter()
            .position(|entry| entry.command == Command::CreateWorktree)
            .unwrap();
        assert_eq!(
            menu_entries(&browser, &view)[view.help_selected].command,
            Command::CreateWorktree
        );
        browser.git_generation += 1;
        assert_eq!(
            menu_entries(&browser, &view)[view.help_selected].command,
            Command::CreateWorktree
        );
        press(&mut browser, &mut view, KeyCode::Enter);
        assert!(view.help);
        assert!(view.error_dialog.is_none());
        assert!(view.worktree_create.is_none());
        browser.ownership.get_mut(&path).unwrap().generation = browser.git_generation;
        assert!(worktree_unavailable(&browser, &view).is_none());
        press(&mut browser, &mut view, KeyCode::Esc);
        browser.folder = FolderFilter::All;
        assert!(worktree_unavailable(&browser, &view).is_some());
        browser.folder = FolderFilter::Path(path.clone());
        browser.ownership.get_mut(&path).unwrap().outcome = Some(repository);
        view.git_focus = true;
        assert!(
            !menu_entries(&browser, &view)
                .iter()
                .any(|entry| entry.command == Command::CreateWorktree)
        );
        view.git_focus = false;
        browser.specs.clear();
        assert!(
            !menu_entries(&browser, &view)
                .iter()
                .any(|entry| entry.command == Command::CreateWorktree)
        );
    }

    #[test]
    fn recovery_menu_and_shortcut_follow_availability() {
        let mut browser = Browser::new(vec![]);
        let mut view = folder_view();
        for available in [false, true, false] {
            view.recovery_available = available;
            for focus in [0, 1] {
                view.focus = focus;
                assert_eq!(
                    menu_entries(&browser, &view)
                        .iter()
                        .any(|e| e.command == Command::Recover),
                    available
                );
                assert_eq!(
                    matches!(
                        handle_key(
                            &mut browser,
                            &mut view,
                            KeyEvent::new(KeyCode::Char('u'), KeyModifiers::NONE)
                        ),
                        Action::RecoverRelink
                    ),
                    available
                );
            }
        }
    }

    #[test]
    fn recovery_refresh_disables_missing_or_unreadable_state() {
        let mut view = View {
            recovery_available: true,
            ..folder_view()
        };
        assert_eq!(refresh_recovery(None, &mut view), None);
        assert!(!view.recovery_available);
        let state = tempfile::tempdir().unwrap();
        assert_eq!(
            refresh_recovery(Some(state.path()), &mut view),
            Some((0, false))
        );
        assert!(!view.recovery_available);
        std::fs::create_dir(state.path().join("folder-moves")).unwrap();
        std::fs::write(state.path().join("folder-moves/current.json"), "invalid").unwrap();
        view.recovery_available = true;
        assert_eq!(refresh_recovery(Some(state.path()), &mut view), None);
        assert!(!view.recovery_available);
        assert!(
            view.message
                .as_deref()
                .unwrap()
                .contains("Cannot inspect folder-move journal")
        );
    }

    #[tokio::test]
    async fn session_opening_uses_enter_and_ignores_alt_f() {
        use crate::git_sessions::tests::{add_session, fixture};
        let cwd = tempfile::tempdir().unwrap();
        let (mut browser, _) = fixture().await;
        let session = add_session(
            &mut browser,
            "Open mode",
            cwd.path().to_str().unwrap(),
            "/worktree",
            "/repo/.git",
            crate::domain::RuntimeStatus::Idle,
            1,
        );
        browser.open_folder(cwd.path().into());
        browser.selected = Some(session);
        let mut view = View {
            focus: 1,
            drawer_open: false,
            dialog_area: false,
            ..folder_view()
        };
        assert_eq!(
            handle_key(
                &mut browser,
                &mut view,
                KeyEvent::new(KeyCode::Char('f'), KeyModifiers::ALT)
            ),
            Action::Continue
        );
        assert_eq!(enter(&mut browser, &mut view), Action::ResumeSelected);
        for kind in [KeyEventKind::Repeat, KeyEventKind::Release] {
            assert_eq!(
                handle_key(
                    &mut browser,
                    &mut view,
                    KeyEvent::new_with_kind(KeyCode::Enter, KeyModifiers::NONE, kind)
                ),
                Action::Continue
            );
        }
        view.preparing = true;
        assert_eq!(enter(&mut browser, &mut view), Action::Continue);
    }

    #[tokio::test]
    async fn drawer_background_preserves_focus_and_respects_dialogs() {
        let mut browser = Browser::new(vec![]);
        let mut view = View::default();
        view.history
            .tick(Some("/synthetic/repo".into()), 0, false, 1)
            .await;
        view.toggle_drawer();
        let key = KeyEvent::new(KeyCode::Char('b'), KeyModifiers::CONTROL);
        for git in [false, true] {
            view.drawer_git = git;
            view.normalize_focus();
            view.terminals.borrow_mut().active = Some(1);
            assert!(
                menu_entries(&browser, &view)
                    .iter()
                    .any(|e| e.command == Command::BackgroundTerminal)
            );
            view.editing = true;
            handle_key(&mut browser, &mut view, key);
            assert_eq!(view.terminals.borrow().active, Some(1));
            view.editing = false;
            handle_key(
                &mut browser,
                &mut view,
                KeyEvent {
                    kind: KeyEventKind::Repeat,
                    ..key
                },
            );
            assert_eq!(view.terminals.borrow().active, Some(1));
            handle_key(&mut browser, &mut view, key);
            assert!(view.terminals.borrow().active.is_none());
            assert!(view.drawer_open);
            assert_eq!(view.git_focus, git);
            assert!(
                !menu_entries(&browser, &view)
                    .iter()
                    .any(|e| e.command == Command::BackgroundTerminal)
            );
        }
    }

    #[test]
    fn deep_search_toggle_is_keyboard_accessible_and_renders_small_terminals() {
        let mut browser = Browser::new(vec![]);
        let mut view = View {
            editing: true,
            search_query: "AltTab".into(),
            ..View::default()
        };
        handle_key(
            &mut browser,
            &mut view,
            KeyEvent::new(KeyCode::Char('d'), KeyModifiers::CONTROL),
        );
        assert!(browser.deep_search);
        assert_eq!(view.search_query, "AltTab");
        handle_key(
            &mut browser,
            &mut view,
            KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE),
        );
        assert_eq!(view.search_control, 3);
        handle_key(
            &mut browser,
            &mut view,
            KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE),
        );
        assert!(!browser.deep_search);
        let mut terminal = Terminal::new(ratatui::backend::TestBackend::new(100, 30)).unwrap();
        terminal
            .draw(|frame| draw_search(frame, &browser, &view))
            .unwrap();
        let screen: String = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect();
        assert!(screen.contains("Deep search: OFF"));
        assert!(screen.contains("Enable deep search"));
        assert!(screen.contains("Space toggles"));
        handle_key(
            &mut browser,
            &mut view,
            KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
        );
        assert!(browser.deep_search);
        assert!(view.editing);
        for (width, height) in [(120, 35), (40, 10), (15, 5)] {
            let mut terminal =
                Terminal::new(ratatui::backend::TestBackend::new(width, height)).unwrap();
            terminal
                .draw(|frame| draw_search(frame, &browser, &view))
                .unwrap();
            if width >= 40 {
                let screen: String = terminal
                    .backend()
                    .buffer()
                    .content
                    .iter()
                    .map(|cell| cell.symbol())
                    .collect();
                assert!(screen.contains("Deep search: ON"));
            }
        }
        handle_key(
            &mut browser,
            &mut view,
            KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE),
        );
        assert!(!view.editing);
        assert!(browser.deep_search);
    }

    #[test]
    fn drawer_shortcut_is_global_and_only_toggles_on_press() {
        let mut browser = Browser::new(vec![]);
        let mut view = View::default();
        assert_eq!(view.context(), 1);
        for (code, modifiers) in [
            (KeyCode::Char('`'), KeyModifiers::ALT),
            (KeyCode::Char('~'), KeyModifiers::ALT | KeyModifiers::SHIFT),
        ] {
            let key = KeyEvent::new(code, modifiers);
            handle_key(&mut browser, &mut view, key);
            assert!(view.drawer_open);
            assert_eq!(view.context(), 0);
            for kind in [KeyEventKind::Repeat, KeyEventKind::Release] {
                handle_key(&mut browser, &mut view, KeyEvent { kind, ..key });
                assert!(view.drawer_open);
            }
            handle_key(&mut browser, &mut view, key);
            assert!(!view.drawer_open);
            assert!(view.terminal_focused());
        }
        assert!(!drawer_shortcut(KeyEvent::new(
            KeyCode::Char('`'),
            KeyModifiers::NONE
        )));
    }

    #[tokio::test]
    async fn drawer_navigation_stays_inside_and_restores_git_selection() {
        let mut browser = Browser::new(vec![]);
        let mut view = View::default();
        view.history
            .tick(Some("/synthetic/repo".into()), 0, false, 1)
            .await;
        view.history.selected = 7;
        view.history.horizontal.set(4);
        view.toggle_drawer();
        for key in [KeyCode::Tab, KeyCode::BackTab] {
            press(&mut browser, &mut view, key);
            assert_eq!(view.context(), 2);
            view.toggle_drawer();
            assert_eq!(view.context(), 1);
            view.toggle_drawer();
            assert_eq!(view.context(), 2);
            press(&mut browser, &mut view, key);
            assert_eq!(view.context(), 0);
        }
        for (key, expected) in [
            (KeyCode::Down, 2),
            (KeyCode::Right, 2),
            (KeyCode::Up, 0),
            (KeyCode::Left, 0),
        ] {
            handle_key(
                &mut browser,
                &mut view,
                KeyEvent::new(key, KeyModifiers::ALT),
            );
            assert_eq!(view.context(), expected);
        }
        assert_eq!(
            (view.history.selected, view.history.horizontal.get()),
            (7, 4)
        );
        press(&mut browser, &mut view, KeyCode::Esc);
        assert!(!view.drawer_open);
        press(&mut browser, &mut view, KeyCode::Tab);
        assert_eq!(view.context(), 0);
        view.history.tick(None, 1, false, 1).await;
        press(&mut browser, &mut view, KeyCode::Tab);
        assert_eq!(view.context(), 0);
    }

    #[tokio::test]
    async fn tab_and_left_return_inactive_sessions_to_drawer() {
        for count in [0, 2] {
            for (width, height) in [(160, 40), (80, 24)] {
                for present in [false, true] {
                    for remembered_git in [false, true] {
                        for key in [
                            KeyEvent::new(KeyCode::Left, KeyModifiers::NONE),
                            KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE),
                            KeyEvent::new(KeyCode::BackTab, KeyModifiers::NONE),
                            KeyEvent::new(KeyCode::Tab, KeyModifiers::SHIFT),
                        ] {
                            let mut browser = entry_fixture(count, 1);
                            let mut view = View::default();
                            if present {
                                view.history
                                    .tick(Some("/synthetic/repo".into()), 0, false, 1)
                                    .await;
                            }
                            view.drawer_git = remembered_git;
                            let mut terminal =
                                Terminal::new(ratatui::backend::TestBackend::new(width, height))
                                    .unwrap();
                            terminal.draw(|f| draw(f, &browser, &view)).unwrap();
                            assert!(menu_entries(&browser, &view).iter().any(
                                |entry| entry.command == Command::CyclePane
                                    && entry.shortcut == "Tab / ←"
                                    && entry.label == "Focus drawer"
                            ));
                            assert_eq!(handle_key(&mut browser, &mut view, key), Action::Continue);
                            let expected = if present && remembered_git { 2 } else { 0 };
                            assert_eq!(view.context(), expected);
                            assert!(view.drawer_open);
                            terminal.draw(|f| draw(f, &browser, &view)).unwrap();
                            for index in 0..4 {
                                let cycle_key = if key.code == KeyCode::Left {
                                    KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE)
                                } else {
                                    key
                                };
                                handle_key(&mut browser, &mut view, cycle_key);
                                assert_eq!(
                                    view.context(),
                                    if present && index % 2 == 0 {
                                        2 - expected
                                    } else {
                                        expected
                                    }
                                );
                                assert!(view.drawer_open);
                            }
                            press(&mut browser, &mut view, KeyCode::Right);
                            assert_eq!(view.context(), 1);
                            assert!(!view.drawer_open);
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn tab_keeps_active_sessions_and_search_focus() {
        let mut browser = Browser::new(vec![]);
        let mut view = View::default();
        view.terminals.borrow_mut().active = Some(1);
        for code in [KeyCode::Tab, KeyCode::BackTab, KeyCode::Left] {
            press(&mut browser, &mut view, code);
            assert!(!view.drawer_open);
            assert_eq!(view.context(), 1);
        }
        assert!(
            !menu_entries(&browser, &view)
                .iter()
                .any(|entry| entry.command == Command::CyclePane)
        );
        view.terminals.borrow_mut().active = None;
        view.editing = true;
        press(&mut browser, &mut view, KeyCode::Left);
        assert!(!view.drawer_open);
        assert!(view.editing);
        press(&mut browser, &mut view, KeyCode::Tab);
        assert!(!view.drawer_open);
        assert_ne!(view.search_control, 0);
        press(&mut browser, &mut view, KeyCode::BackTab);
        assert_eq!(view.search_control, 0);
    }

    #[tokio::test]
    async fn git_zoom_preserves_navigation_and_restores_context() {
        let mut browser = Browser::new(vec![]);
        let mut view = View::default();
        view.history
            .tick(Some("/synthetic/repo".into()), 0, false, 1)
            .await;
        view.toggle_drawer();
        press(&mut browser, &mut view, KeyCode::Tab);
        let zoom = KeyEvent::new(KeyCode::Char('z'), KeyModifiers::ALT);
        handle_key(&mut browser, &mut view, zoom);
        assert!(view.git_zoomed);
        assert_eq!(view.context(), 2);
        for kind in [KeyEventKind::Repeat, KeyEventKind::Release] {
            handle_key(&mut browser, &mut view, KeyEvent { kind, ..zoom });
            assert!(view.git_zoomed);
        }
        view.history.horizontal.set(10);
        press(&mut browser, &mut view, KeyCode::Left);
        assert!(view.git_zoomed);
        assert!(view.history.horizontal.get() < 10);
        press(&mut browser, &mut view, KeyCode::Char('l'));
        assert!(view.git_zoomed);
        assert!(matches!(
            handle_key(
                &mut browser,
                &mut view,
                KeyEvent::new(KeyCode::Char('r'), KeyModifiers::NONE)
            ),
            Action::Refresh
        ));
        assert!(view.git_zoomed);

        press(&mut browser, &mut view, KeyCode::Char('?'));
        assert!(view.help);
        press(&mut browser, &mut view, KeyCode::Esc);
        assert!(!view.help);
        assert!(view.git_zoomed);
        assert!(
            menu_entries(&browser, &view)
                .iter()
                .any(|entry| entry.command == Command::GitReturn
                    && entry.label == "Restore Git panel")
        );
        for code in [KeyCode::Esc, KeyCode::Backspace] {
            press(&mut browser, &mut view, code);
            assert!(!view.git_zoomed);
            assert!(view.drawer_open);
            assert_eq!(view.context(), 2);
            handle_key(&mut browser, &mut view, zoom);
        }
        handle_key(&mut browser, &mut view, zoom);
        assert!(!view.git_zoomed);

        for code in [KeyCode::Tab, KeyCode::BackTab] {
            handle_key(&mut browser, &mut view, zoom);
            press(&mut browser, &mut view, code);
            assert!(!view.git_zoomed);
            assert_eq!(view.context(), 0);
            press(&mut browser, &mut view, code);
        }
        handle_key(&mut browser, &mut view, zoom);
        handle_key(
            &mut browser,
            &mut view,
            KeyEvent::new(KeyCode::Down, KeyModifiers::ALT),
        );
        assert!(view.git_zoomed);
        handle_key(
            &mut browser,
            &mut view,
            KeyEvent::new(KeyCode::Up, KeyModifiers::ALT),
        );
        assert!(!view.git_zoomed);
        assert_eq!(view.context(), 0);
        press(&mut browser, &mut view, KeyCode::Tab);
        handle_key(&mut browser, &mut view, zoom);
        press(&mut browser, &mut view, KeyCode::Right);
        assert!(!view.git_zoomed);
        assert!(!view.drawer_open);
        view.toggle_drawer();
        assert!(!view.git_zoomed);
        handle_key(&mut browser, &mut view, zoom);
        view.history.tick(None, 1, false, 1).await;
        view.normalize_focus();
        assert!(!view.git_zoomed);
        assert_eq!(view.context(), 0);
    }

    #[tokio::test]
    async fn git_zoom_geometry_survives_resize_and_restores_panels() {
        let (mut browser, mut view) = navigation_fixture(&["selected-oid"]).await;
        select_git_fixture(&mut browser, "/project/.git");
        for (width, height) in [(240, 80), (119, 30), (40, 10), (12, 4), (8, 3)] {
            let area = Rect::new(0, 0, width, height - 2);
            view.git_zoomed = false;
            let normal = panel_layout(area, height, &view, true);
            view.git_zoomed = true;
            let zoomed = panel_layout(area, height, &view, true);
            let left = zoomed.git.unwrap();
            let right = zoomed.details.unwrap();
            assert_eq!(left.x, area.x);
            assert_eq!(left.right(), right.x);
            assert_eq!(right.right(), area.right());
            assert!(left.width.abs_diff(right.width) <= 1);
            assert!(zoomed.folders.is_none());
            assert!(zoomed.sessions.is_none());
            assert_eq!(zoomed.graph_visible, left.width >= 12 && area.height >= 4);
            let mut terminal =
                Terminal::new(ratatui::backend::TestBackend::new(width, height)).unwrap();
            terminal.draw(|frame| draw(frame, &browser, &view)).unwrap();
            if width >= 40 {
                let screen: String = terminal
                    .backend()
                    .buffer()
                    .content
                    .iter()
                    .map(|c| c.symbol())
                    .collect();
                assert!(!screen.contains("Folders"));
            }
            view.git_zoomed = false;
            let restored = panel_layout(area, height, &view, true);
            assert_eq!(restored.git, normal.git);
            assert_eq!(restored.folders, normal.folders);
            assert_eq!(restored.sessions, normal.sessions);
        }
    }

    #[test]
    fn dialogs_and_terminal_overlays_are_preserved_per_area() {
        let mut browser = Browser::new(vec![]);
        let mut view = View {
            editing: true,
            search_query: "main draft".into(),
            ..View::default()
        };
        view.terminals.borrow_mut().picker = true;
        view.toggle_drawer();
        assert!(!view.modal_open());
        assert!(!view.terminals.borrow().has_overlay());
        view.editing = true;
        view.search_query = "drawer draft".into();
        view.search_control = 1;
        view.toggle_drawer();
        assert_eq!(view.search_query, "main draft");
        assert!(view.terminals.borrow().picker);
        // Simulate a hidden operation delivering its error to its owner.
        view.select_dialog_area(true);
        view.error_dialog = Some(crate::error_dialog::ErrorDialog::information(
            "Drawer result",
            "Finished",
        ));
        view.select_dialog_area(false);
        assert!(view.error_dialog.is_none());
        assert_eq!(view.search_query, "main draft");
        view.toggle_drawer();
        assert_eq!(view.search_query, "drawer draft");
        assert_eq!(view.search_control, 1);
        assert!(view.error_dialog.is_some());
        press(&mut browser, &mut view, KeyCode::Esc);
        assert!(view.drawer_open);
        assert!(view.error_dialog.is_none());
        press(&mut browser, &mut view, KeyCode::Esc);
        assert!(view.drawer_open);
        assert!(!view.editing);
        press(&mut browser, &mut view, KeyCode::Esc);
        assert!(!view.drawer_open);
        assert_eq!(view.search_query, "main draft");
    }

    #[test]
    fn coding_hint_fits_without_reserving_terminal_space() {
        let browser = Browser::new(vec![]);
        for (width, height, shown) in [(80, 24, true), (9, 3, true), (8, 3, false), (9, 2, false)] {
            let view = View::default();
            view.terminals.borrow_mut().active = Some(1);
            let mut terminal =
                Terminal::new(ratatui::backend::TestBackend::new(width, height)).unwrap();
            terminal.draw(|frame| draw(frame, &browser, &view)).unwrap();
            assert_eq!(view.terminals.borrow().area, Rect::new(0, 0, width, height));
            let buffer = terminal.backend().buffer();
            let text: String = buffer.content.iter().map(|cell| cell.symbol()).collect();
            assert_eq!(text.contains("Alt+~"), shown);
            assert!(
                buffer.content[..usize::from(width * 2)]
                    .iter()
                    .all(|cell| cell.symbol() == " ")
            );
            if shown {
                let line: String = (4..9).map(|col| buffer[(col, 2)].symbol()).collect();
                assert_eq!(line, "Alt+~");
            }
        }
    }

    #[test]
    fn only_shortcut_presses_teach_the_drawer_reminder() {
        let mut browser = Browser::new(vec![]);
        for code in [KeyCode::Char('`'), KeyCode::Char('~')] {
            let mut view = View::default();
            view.toggle_drawer();
            assert!(!view.drawer_reminder.get().learned);
            for kind in [KeyEventKind::Repeat, KeyEventKind::Release] {
                handle_key(
                    &mut browser,
                    &mut view,
                    KeyEvent::new_with_kind(code, KeyModifiers::ALT, kind),
                );
                assert!(!view.drawer_reminder.get().learned);
            }
            handle_key(
                &mut browser,
                &mut view,
                KeyEvent::new(code, KeyModifiers::ALT),
            );
            assert!(view.drawer_reminder.get().learned);
        }
    }

    #[test]
    fn drawer_reminder_expires_once_and_learning_retires_it() {
        let now = std::time::Instant::now();
        let mut reminder = DrawerReminder::default();
        assert!(!reminder.visible(now, false));
        assert!(!reminder.started);
        assert!(reminder.visible(now, true));
        assert!(!reminder.visible(now + Duration::from_secs(1), false));
        assert!(reminder.visible(now + Duration::from_secs(1), true));
        assert!(!reminder.visible(now + Duration::from_secs(2), true));
        assert!(!reminder.visible(now + Duration::from_secs(20), true));
        let mut reminder = DrawerReminder::default();
        reminder.learn();
        assert!(!reminder.visible(now, true));
        assert!(reminder.deadline.is_none());
    }

    #[test]
    fn drawer_geometry_and_rendering_follow_terminal_size() {
        let browser = Browser::new(vec![]);
        for (width, height) in [
            (119, 30),
            (120, 30),
            (160, 40),
            (240, 60),
            (160, 11),
            (80, 8),
            (12, 4),
        ] {
            let mut view = View::default();
            let area = Rect::new(0, 0, width, height - 2);
            let closed = panel_layout(area, height, &view, true);
            let wide = width >= 120 && height >= 12;
            assert_eq!(closed.folders.is_some(), wide);
            assert_eq!(closed.sessions.unwrap().x > 0, wide);
            assert_eq!(view.drawer_visible(), wide);
            view.toggle_drawer();
            for present in [false, true] {
                for git in [false, true] {
                    view.git_focus = git;
                    view.drawer_git = git;
                    let panels = panel_layout(area, height, &view, present);
                    let wide = width >= 120 && height >= 12;
                    assert_eq!(panels.sessions.is_some(), wide);
                    let drawer_width = if wide { (width * 2 / 5).min(60) } else { width };
                    if let Some(main) = panels.sessions {
                        assert_eq!(main.x, drawer_width);
                        assert_eq!(main.right(), width);
                    }
                    for panel in [panels.folders, panels.git].into_iter().flatten() {
                        assert_eq!(panel.width, drawer_width);
                        assert!(panel.bottom() <= area.bottom());
                    }
                    if !present {
                        assert_eq!(panels.folders.unwrap().height, area.height);
                        assert!(panels.git.is_none());
                    } else if area.height < 12 {
                        assert_eq!(panels.git.is_some(), git);
                        assert_eq!(panels.folders.is_some(), !git);
                    } else {
                        assert_eq!(panels.folders.unwrap().bottom(), panels.git.unwrap().y);
                    }
                }
            }
            view.toggle_drawer();
            let mut terminal =
                Terminal::new(ratatui::backend::TestBackend::new(width, height)).unwrap();
            terminal.draw(|f| draw(f, &browser, &view)).unwrap();
            let text: String = terminal
                .backend()
                .buffer()
                .content
                .iter()
                .map(|c| c.symbol())
                .collect();
            if width >= 80 {
                assert!(text.contains("? Actions"));
            }
            assert!(!text.contains("Git Worktrees"));
        }
    }

    #[test]
    fn drawer_selection_focuses_sessions() {
        for count in 0..=2 {
            let mut browser = entry_fixture(count, 1);
            let mut view = View::default();
            view.toggle_drawer();
            let action = enter(&mut browser, &mut view);
            assert_eq!(
                action,
                match count {
                    0 => Action::CreateNew(0),
                    1 => Action::ResumeSelected,
                    _ => Action::Continue,
                }
            );
            assert!(!view.drawer_open);
            assert_eq!(view.context(), 1);
        }
    }

    #[test]
    fn session_scrollbar_tracks_three_line_entries_and_hides_after_resize() {
        let mut browser = entry_fixture(30, 1);
        let view = View::default();
        let keys: Vec<_> = browser.visible().iter().map(|s| s.key.clone()).collect();
        let mut terminal = Terminal::new(ratatui::backend::TestBackend::new(80, 12)).unwrap();
        for index in [0, 15, 29, 0] {
            browser.selected = Some(keys[index].clone());
            terminal
                .draw(|frame| draw_pane(frame, frame.area(), 1, &browser, &view))
                .unwrap();
            let buffer = terminal.backend().buffer();
            let thumb: Vec<_> = (2..11)
                .filter(|y| buffer[(78, *y)].fg == Color::Rgb(112, 112, 112))
                .collect();
            assert!(!thumb.is_empty());
            for y in 2..11 {
                assert_eq!(buffer[(78, y)].symbol(), "🮇");
            }
            if index == 0 {
                assert_eq!(thumb.first(), Some(&2));
            }
            if index == 29 {
                assert_eq!(thumb.last(), Some(&10));
            }
            let text: String = buffer.content.iter().map(|c| c.symbol()).collect();
            let selected = browser
                .visible()
                .into_iter()
                .find(|s| s.key == keys[index])
                .unwrap();
            assert!(text.contains(&selected.title));
        }
        terminal.backend_mut().resize(80, 100);
        terminal.resize(Rect::new(0, 0, 80, 100)).unwrap();
        terminal
            .draw(|frame| draw_pane(frame, frame.area(), 1, &browser, &view))
            .unwrap();
        assert!((2..99).all(|y| terminal.backend().buffer()[(79, y)].symbol() == "┃"));
    }

    #[test]
    fn folder_selection_stays_centered_in_both_directions() {
        let mut browser = Browser::new(vec![]);
        let path = format!(
            "/{}",
            (0..20)
                .map(|i| format!("node{i:02}"))
                .collect::<Vec<_>>()
                .join("/")
        );
        browser.open_folder(path.into());
        let view = View::default();
        let rows = browser.folder_rows();
        let last = rows.len() - 1;
        let mut terminal = Terminal::new(ratatui::backend::TestBackend::new(160, 9)).unwrap();
        // Six interior rows: the lower middle is screen row five. At the end,
        // moving up reaches the middle before the viewport starts scrolling.
        for (selected, top, cursor) in [
            (0, 0, 2),
            (3, 0, 5),
            (10, 7, 5),
            (11, 8, 5),
            (10, 7, 5),
            (last, last - 5, 7),
            (last - 1, last - 5, 6),
            (last - 2, last - 5, 5),
            (last - 3, last - 6, 5),
        ] {
            browser.folder = rows[selected].filter.clone();
            terminal
                .draw(|frame| draw_pane(frame, frame.area(), 0, &browser, &view))
                .unwrap();
            let buffer = terminal.backend().buffer();
            assert_eq!(buffer[(1, cursor)].symbol(), ">");
            let first: String = (0..160).map(|x| buffer[(x, 2)].symbol()).collect();
            assert!(first.contains(&rows[top].label), "{first}");
            let thumb: Vec<_> = (2..8)
                .filter(|y| buffer[(158, *y)].fg == Color::Rgb(112, 112, 112))
                .collect();
            assert!(!thumb.is_empty());
            if selected == 0 {
                assert_eq!(thumb.first(), Some(&2));
            }
            if selected == last {
                assert_eq!(thumb.last(), Some(&7));
            }
            assert_eq!(browser.folder, rows[selected].filter);
        }
        // Resize the same selection, including zero/one usable row.
        browser.folder = rows[10].filter.clone();
        for (height, cursor) in [
            (8, Some(4)),
            (13, Some(7)),
            (5, Some(3)),
            (4, Some(2)),
            (3, None),
            (9, Some(5)),
        ] {
            terminal.backend_mut().resize(160, height);
            terminal.resize(Rect::new(0, 0, 160, height)).unwrap();
            terminal
                .draw(|frame| draw_pane(frame, frame.area(), 0, &browser, &view))
                .unwrap();
            if let Some(y) = cursor {
                assert_eq!(terminal.backend().buffer()[(1, y)].symbol(), ">");
            }
            assert_eq!(browser.folder, rows[10].filter);
        }
        // Collapsing the selected ancestor shortens the tree; expanding it
        // restores centering without retaining an obsolete viewport offset.
        if let FolderFilter::Path(path) = &browser.folder {
            browser.expanded.remove(path);
        }
        terminal
            .draw(|frame| draw_pane(frame, frame.area(), 0, &browser, &view))
            .unwrap();
        assert_eq!(terminal.backend().buffer()[(1, 7)].symbol(), ">");
        if let FolderFilter::Path(path) = &browser.folder {
            browser.expanded.insert(path.clone());
        }
        terminal
            .draw(|frame| draw_pane(frame, frame.area(), 0, &browser, &view))
            .unwrap();
        assert_eq!(terminal.backend().buffer()[(1, 5)].symbol(), ">");
        // A short list fits without artificial empty space above its rows.
        browser.expanded.clear();
        browser.folder = FolderFilter::Unlinked;
        terminal
            .draw(|frame| draw_pane(frame, frame.area(), 0, &browser, &view))
            .unwrap();
        assert_eq!(terminal.backend().buffer()[(1, 2)].symbol(), ">");
    }

    #[test]
    fn folder_layout_stays_stable_across_expansion_selection_and_viewport() {
        let mut browser = Browser::new(vec![]);
        let path = std::path::PathBuf::from(format!("/parent/{}", "long-folder-".repeat(8)));
        browser.open_folder(path.clone());
        let mut view = View {
            focus: 0,
            ..folder_view()
        };
        let divider = |browser: &Browser, view: &View, width, height| {
            let mut terminal =
                Terminal::new(ratatui::backend::TestBackend::new(width, height)).unwrap();
            terminal.draw(|frame| draw(frame, browser, view)).unwrap();
            (0..width)
                .find(|&x| terminal.backend().buffer()[(x, 0)].symbol() == "┓")
                .unwrap()
                + 1
        };
        assert_eq!(divider(&browser, &view, 99, 24), 99);
        assert_eq!(divider(&browser, &view, 100, 24), 100);
        assert_eq!(divider(&browser, &view, 160, 24), 60);
        browser.folder = FolderFilter::Unlinked;
        assert_eq!(divider(&browser, &view, 160, 6), 160);
        browser.expanded.remove(std::path::Path::new("/parent"));
        let collapsed = divider(&browser, &view, 160, 24);
        assert_eq!(collapsed, 60);
        browser.expanded.insert("/parent".into());
        assert_eq!(divider(&browser, &view, 160, 24), 60);
        view.toggle_drawer();
        let mut terminal = Terminal::new(ratatui::backend::TestBackend::new(99, 20)).unwrap();
        terminal.draw(|frame| draw(frame, &browser, &view)).unwrap();
        assert_eq!(terminal.backend().buffer()[(98, 0)].symbol(), "┓");
    }

    #[test]
    fn availability_labels_and_relink_gate_follow_live_updates() {
        use crate::{
            browser::{SourceSpec, Update},
            config::SourceConfig,
            domain::*,
        };
        use std::collections::BTreeMap;
        let tmp = tempfile::tempdir().unwrap();
        let source = SourceConfig {
            id: SourceId("test".into()),
            home: tmp.path().to_owned(),
        };
        let mut browser = Browser::new(vec![SourceSpec::Codex {
            binary: "codex".into(),
            source,
        }]);
        browser.state_path = Some(tmp.path().join("state.json"));
        let session: SessionSummary = serde_json::from_value(serde_json::json!({
            "key": {"provider":"codex","source_id":"test","native_id":"one"},
            "title":"Example session", "cwd":tmp.path().join("missing"),
            "created_at":1,"updated_at":2,"archived":false,"source_kind":null,"git":null
        }))
        .unwrap();
        browser.begin_refresh();
        browser.apply(Update::Page {
            generation: 1,
            source: 0,
            sessions: vec![session.clone()],
            warnings: vec![],
        });
        browser.apply(Update::Finished {
            generation: 1,
            source: 0,
            error: None,
        });
        browser.selected = Some(session.key.clone());
        for (status, label) in [
            (RuntimeStatus::Running, "Running"),
            (RuntimeStatus::Idle, "Idle"),
            (RuntimeStatus::Unloaded, "Unloaded"),
            (RuntimeStatus::Unknown, "Status unknown"),
        ] {
            let statuses = if status == RuntimeStatus::Unloaded || status == RuntimeStatus::Unknown
            {
                BTreeMap::new()
            } else {
                BTreeMap::from([(session.key.clone(), status)])
            };
            browser.apply(Update::Live {
                generation: 1,
                source: 0,
                statuses,
                error: (status == RuntimeStatus::Unknown).then(|| "offline".into()),
            });
            assert_eq!(browser.runtime_status(&session.key), status);
            for width in [18, 80] {
                let line = session_title(&session, status, width);
                let text: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
                assert!(text.contains(label), "{text}");
                assert_eq!(
                    line.spans.last().unwrap().style.fg,
                    Some(activity_color(status).unwrap_or(Color::Reset))
                );
            }
            let reason = relink_unavailable(&browser);
            match status {
                RuntimeStatus::Idle => assert!(reason.is_none()),
                RuntimeStatus::Running => assert!(reason.unwrap().contains("running in Codex")),
                _ => assert!(reason.is_none(), "{reason:?}"),
            }
            if matches!(status, RuntimeStatus::Idle | RuntimeStatus::Unloaded) {
                assert_eq!(
                    activity_color(browser.folder_activity(&crate::browser::FolderFilter::All)),
                    if status == RuntimeStatus::Idle {
                        Some(Color::Rgb(255, 255, 170))
                    } else {
                        None
                    }
                );
            }
        }
    }

    // Invoked only by the parent test in a fresh controlling terminal.
    #[test]
    #[ignore]
    fn guard_child() {
        let result = std::panic::catch_unwind(|| -> io::Result<()> {
            let mut guard = TerminalGuard {
                active: false,
                mouse: false,
            };
            guard.activate()?;
            guard.enhanced(true)?;
            guard.suspend()?;
            guard.activate()?;
            guard.enhanced(true)?;
            if std::env::var("TRACR_TEST_CLEANUP").as_deref() == Ok("panic") {
                panic!("simulated failure after terminal activation");
            }
            Err(io::Error::other("simulated setup failure after activation"))
        });
        assert!(!matches!(result, Ok(Ok(()))));
    }

    #[test]
    fn setup_error_and_panic_restore_terminal() {
        let status = std::process::Command::new("python3")
            .arg(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/tests/fixtures/app-server/guard_check.py"
            ))
            .arg(std::env::current_exe().unwrap())
            .status()
            .unwrap();
        assert!(status.success());
    }
}

#[cfg(test)]
mod folder_viewport_tests {
    fn folder_viewport(text: &str, offset: usize, width: usize) -> String {
        super::graph_viewport(
            vec![ratatui::text::Span::raw(text.to_owned())],
            offset,
            width,
        )
        .to_string()
    }

    #[test]
    fn folder_connectors_stay_white_while_text_keeps_activity_color() {
        use crate::{
            browser::{FolderFilter, FolderRow},
            domain::RuntimeStatus,
        };
        use ratatui::style::Color;
        let row = FolderRow {
            filter: FolderFilter::Path("/project".into()),
            label: "◇ project *".into(),
            depth: 2,
            count: 1,
        };
        for status in [
            RuntimeStatus::Running,
            RuntimeStatus::Idle,
            RuntimeStatus::Waiting {
                approval: true,
                input: false,
            },
        ] {
            for offset in [0, 1, 3, 4, 6] {
                let line =
                    super::folder_tree_line(&row, "│  └─ ◇ project * (1)", status, offset, 40);
                for span in line.spans {
                    if span.content.contains(['│', '└', '─']) {
                        assert_eq!(span.style.fg, Some(Color::White));
                    } else if span.content == "*" {
                        assert_eq!(span.style.fg, Some(crate::git_render::DIRTY_COLOR));
                    } else if span.content == "◇" {
                        assert_eq!(span.style.fg, Some(crate::git_render::WORKTREE_COLOR));
                    } else if span.content.chars().any(char::is_alphabetic) {
                        assert_eq!(span.style.fg, super::activity_color(status));
                    }
                }
            }
        }
    }

    #[test]
    fn folder_branches_follow_displayed_hierarchy_and_collapsed_siblings() {
        use crate::browser::{FolderFilter, FolderRow};
        let row = |label: &str, depth| FolderRow {
            label: label.into(),
            depth,
            count: 0,
            filter: FolderFilter::Path(std::path::PathBuf::from(label)),
        };
        // The linked worktree's filesystem path is irrelevant to this hierarchy.
        let mut rows = vec![
            row("/", 0),
            row("project", 1),
            row("feature", 2),
            row("src", 3),
            row("other", 1),
            row("child", 2),
        ];
        let labels = super::folder_tree_labels(&rows);
        assert_eq!(
            labels,
            [
                "/ (0)",
                "├─ project (0)",
                "│  └─ feature (0)",
                "│     └─ src (0)",
                "└─ other (0)",
                "   └─ child (0)"
            ]
        );
        // Clipping before the later sibling must not turn project into a corner.
        assert_eq!(&labels[1..3], ["├─ project (0)", "│  └─ feature (0)"]);
        rows.drain(2..4);
        assert_eq!(
            super::folder_tree_labels(&rows),
            ["/ (0)", "├─ project (0)", "└─ other (0)", "   └─ child (0)"]
        );
    }

    #[test]
    fn clips_columns_without_splitting_graphemes() {
        assert_eq!(folder_viewport("ab界cd", 3, 3), " cd");
        assert_eq!(folder_viewport("ab界cd", 0, 3), "ab ");
        assert_eq!(folder_viewport("ae\u{301}z", 1, 1), "e\u{301}");
        assert_eq!(folder_viewport("a👩‍💻z", 2, 2), " z");
        assert_eq!(folder_viewport("short", 10, 4), "");
        assert_eq!(folder_viewport("short", 0, 0), "");
    }
}

#[cfg(test)]
mod graph_viewport_tests {
    use super::*;
    #[test]
    fn styled_unicode_clips_by_columns() {
        let line = graph_viewport(
            vec![
                Span::styled("ab界", Style::default().fg(Color::Cyan)),
                Span::styled("éz", Style::default().fg(Color::Yellow)),
            ],
            3,
            3,
        );
        assert_eq!(
            line.spans
                .iter()
                .map(|s| s.content.as_ref())
                .collect::<String>(),
            " éz"
        );
        assert_eq!(line.spans[0].style.fg, Some(Color::Cyan));
        assert_eq!(line.spans[1].style.fg, Some(Color::Yellow));
    }
    #[test]
    fn graph_styles_and_fixed_selection_survive_horizontal_clipping() {
        use ratatui::{Terminal, backend::TestBackend};
        let mut terminal = Terminal::new(TestBackend::new(40, 8)).unwrap();
        let spans = vec![
            Span::styled(
                "| * ",
                Style::default().fg(Color::Cyan).bg(Color::Rgb(40, 40, 40)),
            ),
            Span::raw("  "),
            Span::styled(
                "[main] ",
                Style::default().fg(crate::git_render::BRANCH_COLOR),
            ),
            Span::styled("[remote: origin/main] ", Style::default().fg(Color::Cyan)),
            Span::styled("[tag: v1] ", Style::default().fg(Color::Yellow)),
            Span::raw("long subject 界界"),
        ];
        for (focused, offset) in [true, false]
            .into_iter()
            .flat_map(|focused| [0, 2, 4, 20].map(|offset| (focused, offset)))
        {
            terminal
                .draw(|frame| {
                    frame.render_stateful_widget(
                        List::new(vec![ListItem::new(graph_viewport(
                            spans.clone(),
                            offset,
                            36,
                        ))])
                        .block(block("Git history", true))
                        .highlight_symbol("> ")
                        .highlight_style(selection_style(focused)),
                        frame.area(),
                        &mut ListState::default().with_selected(Some(0)),
                    )
                })
                .unwrap();
            let buffer = terminal.backend().buffer();
            assert_eq!(buffer[(1, 1)].symbol(), ">");
            assert_eq!(buffer[(1, 1)].bg, Color::Reset);
            for x in 3..39 {
                assert_eq!(
                    buffer[(x, 1)].bg,
                    if usize::from(x - 3) + offset < 4 {
                        Color::Rgb(40, 40, 40)
                    } else {
                        Color::Reset
                    }
                );
            }
            assert!(buffer[(3, 1)].modifier.contains(if focused {
                Modifier::BOLD
            } else {
                Modifier::DIM
            }));
            assert_eq!(buffer[(39, 1)].symbol(), "┃");
            assert!(buffer.content.iter().any(|cell| cell.fg == Color::Cyan));
            if offset == 0 {
                assert!(
                    buffer
                        .content
                        .iter()
                        .any(|cell| cell.symbol() == "m"
                            && cell.fg == crate::git_render::BRANCH_COLOR)
                );
            }
            if offset == 20 {
                assert!(
                    buffer
                        .content
                        .iter()
                        .any(|cell| cell.symbol() == "t" && cell.fg == Color::Yellow)
                );
            }
        }
    }
}

#[cfg(test)]
mod idle_tests {
    use super::*;
    #[test]
    fn idle_gate_does_no_repeated_work_and_wakes_for_changes_or_deadlines() {
        let now = std::time::Instant::now();
        let mut gate = IdleGate::default();
        gate.wake();
        assert!(gate.ready(now, false));
        for _ in 0..10_000 {
            assert!(!gate.ready(now, false));
        }
        assert!(gate.ready(now, true));
        assert!(!gate.ready(now, false));
        gate.deadline = Some(now + Duration::from_secs(60));
        assert!(!gate.ready(now + Duration::from_secs(59), false));
        assert!(gate.ready(now + Duration::from_secs(60), false));
        assert!(!gate.ready(now + Duration::from_secs(61), false));
        gate.wake();
        assert!(gate.ready(now, false));
    }
    #[test]
    fn watch_failure_remains_visible_alongside_transient_messages() {
        let browser = Browser::new(vec![]);
        let view = View {
            git_watch_error: Some("native notifications unavailable".into()),
            message: Some("Session returned".into()),
            ..View::default()
        };
        let (message, _) = browser_footer(Rect::new(0, 0, 100, 30), &browser, &view);
        assert!(message.contains("Git auto-update unavailable — r to refresh"));
        assert!(message.contains("native notifications unavailable"));
        assert!(message.contains("Session returned"));
    }
    #[test]
    fn age_deadlines_track_displayed_text_boundaries() {
        for elapsed in [
            0, 1, 59, 60, 61, 3599, 3600, 86399, 86400, 604_799, 604_800, 2_591_999, 2_592_000,
            31_104_000, 31_535_999, 31_536_000,
        ] {
            let delay = age_delay(100, 100 + elapsed);
            let current = relative_time(100, 100 + elapsed);
            assert_eq!(
                relative_time(100, 100 + elapsed + delay as i64 - 1),
                current
            );
            assert_ne!(relative_time(100, 100 + elapsed + delay as i64), current);
        }
        assert_eq!(age_delay(i64::MAX, 0), 86_400);
    }
}

#[cfg(test)]
mod empty_sessions_tests {
    use super::*;
    use crate::{browser::SourceSpec, config::SourceConfig, domain::SourceId};
    use ratatui::backend::TestBackend;

    #[test]
    fn empty_sessions_top_align_and_keep_status_when_space_is_limited() {
        let mut browser = Browser::new(vec![SourceSpec::Codex {
            binary: "codex".into(),
            source: SourceConfig {
                id: SourceId("empty-test".into()),
                home: "/unused".into(),
            },
        }]);
        browser.folder = FolderFilter::Path("/project".into());
        let render = |browser: &Browser, width, height| {
            let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
            terminal
                .draw(|frame| draw_empty_sessions(frame, frame.area(), browser, true))
                .unwrap();
            (0..height)
                .map(|y| {
                    (0..width)
                        .map(|x| terminal.backend().buffer()[(x, y)].symbol())
                        .collect::<String>()
                })
                .collect::<Vec<_>>()
        };
        let lines = render(&browser, 60, 20);
        assert!(lines[5].contains("No sessions currently shown"));
        assert!(lines[7].contains("Create new session"));
        assert!(lines[8].contains("Press Enter"));
        for height in [14, 30, 50] {
            let resized = render(&browser, 60, height);
            assert!(resized[5].contains("No sessions currently shown"));
            for row in &resized[1..5] {
                assert!(row.chars().skip(1).take(58).all(|c| c == ' '));
            }
        }
        let status = &lines[5];
        let left = status.chars().position(|c| c == 'N').unwrap();
        let right = 59 - (left + "No sessions currently shown".len());
        assert!(left.abs_diff(right + 1) <= 1);
        assert!(
            lines
                .iter()
                .any(|line| line.contains("Show archived sessions"))
        );
        browser.include_archived = true;
        assert!(
            !render(&browser, 60, 20)
                .join("\n")
                .contains("Show archived")
        );
        let short = render(&browser, 60, 6).join("\n");
        assert!(short.contains("No sessions currently shown"));
        assert!(short.contains("Press Enter"));
        assert!(!short.contains("Search all sessions"));
        browser.begin_refresh();
        let loading = render(&browser, 60, 20).join("\n");
        assert!(loading.contains("Loading sessions"));
        assert!(!loading.contains("Create new session"));
        for (width, height) in [(20, 12), (8, 4), (1, 1)] {
            render(&browser, width, height);
        }
    }
}

#[cfg(test)]
mod action_discovery_tests {
    use super::*;
    use ratatui::backend::TestBackend;

    fn press(browser: &mut Browser, view: &mut View, code: KeyCode) -> Action {
        handle_key(browser, view, KeyEvent::new(code, KeyModifiers::NONE))
    }
    fn type_query(browser: &mut Browser, view: &mut View, query: &str) {
        press(browser, view, KeyCode::Char('/'));
        for c in query.chars() {
            press(browser, view, KeyCode::Char(c));
        }
    }
    fn lines(terminal: &Terminal<TestBackend>) -> Vec<String> {
        let buffer = terminal.backend().buffer();
        (0..buffer.area.height)
            .map(|y| {
                (0..buffer.area.width)
                    .map(|x| buffer[(x, y)].symbol())
                    .collect()
            })
            .collect()
    }

    #[test]
    fn current_tab_actions_lead_shared_sections_and_select_first_available() {
        let browser = Browser::new(vec![]);
        for (context, section) in [(0, "Folders"), (1, "Sessions"), (2, "Git")] {
            let mut view = View {
                drawer_open: context != 1,
                drawer_git: context == 2,
                git_focus: context == 2,
                focus: if context == 1 { 1 } else { 0 },
                ..View::default()
            };
            open_help(&browser, &mut view);
            let entries = menu_entries(&browser, &view);
            assert_eq!(entries[0].group, section);
            let ranks: Vec<_> = entries
                .iter()
                .map(|entry| match entry.group {
                    name if name == section => {
                        usize::from(unavailable(entry.command, &browser, &view).is_some())
                    }
                    "Global" => 2,
                    other => panic!("Unexpected section: {other}"),
                })
                .collect();
            assert!(ranks.windows(2).all(|pair| pair[0] <= pair[1]));
            assert_eq!(
                view.help_selected,
                entries
                    .iter()
                    .position(|entry| unavailable(entry.command, &browser, &view).is_none())
                    .unwrap()
            );
            // Empty tabs still expose their local item navigation.
            if context == 1 {
                assert_eq!(entries[view.help_selected].command, Command::PreviousItem);
            }
            view.help_query = section.into();
            let expected: Vec<_> = entries
                .iter()
                .filter(|entry| {
                    format!(
                        "{} {} {} {}",
                        entry.group, entry.section, entry.label, entry.shortcut
                    )
                    .to_lowercase()
                    .contains(&section.to_lowercase())
                })
                .map(|entry| entry.command)
                .collect();
            assert_eq!(
                filtered_menu_entries(&browser, &view)
                    .iter()
                    .map(|entry| entry.command)
                    .collect::<Vec<_>>(),
                expected
            );
        }
    }

    #[test]
    fn two_sections_render_and_filter_without_orphaned_dividers() {
        let browser = Browser::new(vec![]);
        for (width, height) in [(100, 35), (36, 20)] {
            let mut view = View::default();
            open_help(&browser, &mut view);
            // Keep both groups visible together even in the narrow layout.
            view.help_entries
                .as_mut()
                .unwrap()
                .retain(|entry| matches!(entry.command, Command::Info | Command::Quit));
            view.help_selected = 0;
            let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
            terminal
                .draw(|frame| draw_help(frame, &browser, &view))
                .unwrap();
            let text = lines(&terminal).join("\n");
            assert!(text.contains("Sessions") && text.contains("Global"));
            assert!(
                text.lines()
                    .any(|line| line.contains(" ───") && !line.contains("Actions & shortcuts"))
            );
            assert!(!text.contains("General"));
            for (query, heading, absent) in [
                ("Global", "Global", "Sessions"),
                ("information", "Sessions", "Global"),
            ] {
                view.help_query = query.into();
                assert_eq!(filtered_menu_entries(&browser, &view).len(), 1);
                terminal
                    .draw(|frame| draw_help(frame, &browser, &view))
                    .unwrap();
                let text = lines(&terminal).join("\n");
                assert!(text.contains(heading));
                assert!(!text.contains(absent));
                assert!(
                    !text
                        .lines()
                        .any(|line| line.contains(" ───") && !line.contains("Actions & shortcuts"))
                );
            }
        }
    }

    #[test]
    fn availability_changes_do_not_reorder_open_menu() {
        let mut browser = Browser::new(vec![]);
        let mut view = View::default();
        view.toggle_drawer();
        open_help(&browser, &mut view);
        let entries = menu_entries(&browser, &view);
        view.help_selected = entries
            .iter()
            .position(|entry| entry.command == Command::FolderFocus)
            .unwrap();
        assert!(unavailable(Command::FolderFocus, &browser, &view).is_some());
        browser.focus_root = Some("/synthetic".into());
        assert!(unavailable(Command::FolderFocus, &browser, &view).is_none());
        let updated = menu_entries(&browser, &view);
        assert_eq!(
            updated
                .iter()
                .map(|entry| entry.command)
                .collect::<Vec<_>>(),
            entries
                .iter()
                .map(|entry| entry.command)
                .collect::<Vec<_>>()
        );
        assert_eq!(updated[view.help_selected].command, Command::FolderFocus);
        open_help(&browser, &mut view);
        let reopened = menu_entries(&browser, &view);
        let zoom = reopened
            .iter()
            .position(|entry| entry.command == Command::FolderFocus)
            .unwrap();
        assert_eq!(reopened[zoom].shortcut, "Alt+Z");
        assert_eq!(reopened[zoom].label, "Zoom out of folder");
        let create = reopened
            .iter()
            .position(|entry| entry.command == Command::Create)
            .unwrap();
        assert!(zoom < create);
    }

    #[test]
    fn searching_matches_terms_shortcuts_and_executes_original_action() {
        let mut browser = Browser::new(vec![]);
        let mut view = View::default();
        open_help(&browser, &mut view);
        type_query(&mut browser, &mut view, "GENERAL ARCHIVED");
        let entries = filtered_menu_entries(&browser, &view);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].command, Command::Archives);
        press(&mut browser, &mut view, KeyCode::Enter);
        assert!(!view.help);
        assert!(browser.include_archived);
        open_help(&browser, &mut view);
        assert!(view.help_query.is_empty());
        assert!(!view.help_searching);
        assert!(
            unavailable(
                menu_entries(&browser, &view)[view.help_selected].command,
                &browser,
                &view,
            )
            .is_none()
        );
        view.help = false;
        view.toggle_drawer();
        open_help(&browser, &mut view);
        type_query(&mut browser, &mut view, "Shift-Tab");
        assert_eq!(
            filtered_menu_entries(&browser, &view)[0].command,
            Command::CyclePaneBack
        );
        press(&mut browser, &mut view, KeyCode::Backspace);
        assert_eq!(view.help_query, "Shift-Ta");
        press(&mut browser, &mut view, KeyCode::Esc);
        assert!(view.help && !view.help_searching);
        assert_eq!(view.help_query, "Shift-Ta");
        press(&mut browser, &mut view, KeyCode::Esc);
        assert!(!view.help);
    }

    #[test]
    fn empty_results_and_disabled_actions_cannot_run() {
        let mut browser = Browser::new(vec![]);
        let mut view = View::default();
        open_help(&browser, &mut view);
        type_query(&mut browser, &mut view, "zzzzjk?");
        assert_eq!(view.help_query, "zzzzjk?");
        for code in [
            KeyCode::Down,
            KeyCode::End,
            KeyCode::PageDown,
            KeyCode::Enter,
        ] {
            assert_eq!(press(&mut browser, &mut view, code), Action::Continue);
            assert!(view.help);
        }
        let mut terminal = Terminal::new(TestBackend::new(80, 30)).unwrap();
        terminal
            .draw(|frame| draw_help(frame, &browser, &view))
            .unwrap();
        assert!(lines(&terminal).join("\n").contains("No matching actions"));
        handle_key(
            &mut browser,
            &mut view,
            KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL),
        );
        assert!(view.help_query.is_empty());
        for c in "create new".chars() {
            press(&mut browser, &mut view, KeyCode::Char(c));
        }
        let entries = filtered_menu_entries(&browser, &view);
        assert_eq!(entries.len(), 1);
        assert!(unavailable(entries[0].command, &browser, &view).is_some());
        press(&mut browser, &mut view, KeyCode::Enter);
        assert!(view.help);
        // Refreshing underlying state cannot change the identity of filtered entries.
        browser.include_archived = true;
        assert_eq!(
            filtered_menu_entries(&browser, &view)[0].command,
            entries[0].command
        );
    }

    #[tokio::test]
    async fn hints_follow_focus_without_footer_rows_and_status_is_conditional() {
        let mut browser = Browser::new(vec![]);
        select_git_fixture(&mut browser, "/synthetic/repo");
        let mut view = View::default();
        view.history
            .tick(Some("/synthetic/repo".into()), 0, false, 1)
            .await;
        for (width, height) in [(160, 40), (100, 60), (80, 24), (12, 4)] {
            for focus in 0..3 {
                view.drawer_open = focus != 1;
                view.drawer_git = focus == 2;
                view.normalize_focus();
                // History can supply its own notice; assert the clean state separately below.
                let (_, footer) = browser_footer(Rect::new(0, 0, width, height), &browser, &view);
                let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
                terminal.draw(|frame| draw(frame, &browser, &view)).unwrap();
                let panels =
                    panel_layout(Rect::new(0, 0, width, height - footer), height, &view, true);
                let area = match focus {
                    0 => panels.folders,
                    1 => panels.sessions,
                    _ => panels.git,
                }
                .unwrap();
                let rows = lines(&terminal);
                assert!(rows[usize::from(area.bottom() - 1)].contains('?'));
                assert_eq!(
                    rows.iter()
                        .map(|line| line.matches('?').count())
                        .sum::<usize>(),
                    1
                );
            }
        }
        let mut clean = View::default();
        let area = Rect::new(0, 0, 80, 24);
        assert_eq!(browser_footer(area, &browser, &clean).1, 0);
        browser.focus_root = Some("/synthetic/repo".into());
        assert_eq!(browser_footer(area, &browser, &clean), (String::new(), 0));
        clean.message = Some("Refreshed".into());
        assert_eq!(
            browser_footer(area, &browser, &clean),
            ("Refreshed".into(), 1)
        );
        clean.message = None;
        assert_eq!(browser_footer(area, &browser, &clean).1, 0);
    }

    #[test]
    fn drawer_switch_preserves_each_action_menu_filter() {
        let browser = Browser::new(vec![]);
        let mut view = View::default();
        open_help(&browser, &mut view);
        view.help_query = "session".into();
        view.help_searching = true;
        view.toggle_drawer();
        assert!(!view.help);
        open_help(&browser, &mut view);
        view.help_query = "folder".into();
        view.toggle_drawer();
        assert!(view.help && view.help_searching);
        assert_eq!(view.help_query, "session");
        view.toggle_drawer();
        assert!(view.help && !view.help_searching);
        assert_eq!(view.help_query, "folder");
    }

    #[test]
    fn border_hints_fit_rails_and_leave_git_markers_intact() {
        for (width, height) in [(1, 20), (40, 1), (8, 4), (30, 10)] {
            let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
            terminal
                .draw(|frame| draw_action_hint(frame, frame.area(), false, false))
                .unwrap();
            assert!(lines(&terminal).last().unwrap().contains('?'));
        }
        let mut terminal = Terminal::new(TestBackend::new(40, 10)).unwrap();
        terminal
            .draw(|frame| {
                frame.render_widget(Paragraph::new(" ↓ "), Rect::new(36, 9, 3, 1));
                draw_action_hint(frame, frame.area(), false, true);
            })
            .unwrap();
        let rows = lines(&terminal);
        assert!(rows[9].contains("? Actions"));
        assert_eq!(terminal.backend().buffer()[(37, 9)].symbol(), "↓");
        terminal
            .draw(|frame| draw_action_hint(frame, frame.area(), true, false))
            .unwrap();
        assert!(lines(&terminal)[9].contains("Ctrl+B Browser"));
    }
}
