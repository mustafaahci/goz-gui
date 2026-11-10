//! The sidebars from the window's side (F2): the three panes registered,
//! the Folders pane's folder and the search it scopes, Up One Folder
//! (Alt+Up), the status bar's part of it, and the Bookmarks pane kept in
//! step with the bookmarks.
//!
//! **The folder.** The folder chosen in the Folders pane is kept apart from
//! the search box's text: `start_search` asks [`App::scoped_search`] for
//! what gozd gets, the folder's path term in front of the text
//! (`crate::sidebars::scoped`). It goes with the search into the navigation
//! history and the session (`nav.rs`, `session.rs`), so Back, Forward and
//! Restore Previous Session bring it back. As in Everything
//! (`folders_sidebar_match_only_when_shown=1`), it scopes the search only
//! while the Folders pane is open; closed, the pane keeps it for when it
//! opens again (`folders_sidebar_clear_selection_on_close=0`). The status
//! bar shows it at the right while it scopes the search: a double-click
//! there clears it, as does the status bar menu's Clear Selection or the
//! tree's top item.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use windows_sys::Win32::Foundation::LPARAM;
use windows_sys::Win32::Graphics::Gdi::ScreenToClient;
use windows_sys::Win32::UI::WindowsAndMessaging::DestroyMenu;

use super::bookmarks_pane::BookmarksPane;
use super::filters_pane::FiltersPane;
use super::folders_pane::FoldersPane;
use super::{App, in_rect, point};
use crate::dock::PaneId;
use crate::menu::{self, Item};
use crate::pane::{PaneContent, PaneSpec};
use crate::sidebars;
use crate::status::{self, Indicator};

// F2's commands (`menu::F2_FIRST`, 3100..=3199). The first two are
// Everything's "Folders | Clear Selection" and "Folders | Up One Dir",
// which reach the window as commands too (Alt+Up is taken before the
// shortcuts, as the search box keeps it for its history); the others are
// the panes' menus' items, which the menus return to the pane that showed
// them.
/// The status bar menu's Clear Selection: no folder.
pub(super) const CLEAR_SELECTION: u16 = menu::F2_FIRST;
/// Alt+Up: the folder above the Folders pane's.
pub(super) const UP_ONE_FOLDER: u16 = menu::F2_FIRST + 1;
/// The Folders pane's menu.
pub(super) const FOLDER_EXPAND: u16 = menu::F2_FIRST + 10;
pub(super) const FOLDER_OPEN: u16 = menu::F2_FIRST + 11;
pub(super) const FOLDER_OPEN_PATH: u16 = menu::F2_FIRST + 12;
pub(super) const FOLDER_COPY_PATH: u16 = menu::F2_FIRST + 13;
/// The Bookmarks pane's menu (Add to Bookmarks..., Find Bookmark... and
/// Organize Bookmarks... are the Bookmarks menu's own commands).
pub(super) const MARK_OPEN: u16 = menu::F2_FIRST + 20;
pub(super) const MARK_EDIT: u16 = menu::F2_FIRST + 21;
pub(super) const MARK_DELETE: u16 = menu::F2_FIRST + 22;
pub(super) const MARK_EXPAND: u16 = menu::F2_FIRST + 23;

/// The least heights of the panes' client areas at 96 DPI: a few rows
/// below the Folders pane's Subfolders bar and the Bookmarks pane's filter
/// box.
const FOLDERS_MIN_HIGH: i32 = 64;
const BOOKMARKS_MIN_HIGH: i32 = 64;

/// What the window keeps for the sidebars.
pub(super) struct Sidebars {
    /// The Folders pane's folder: `C:\Users\me`, `C:\` (kept while the
    /// pane is closed).
    folder: RefCell<Option<String>>,
    /// The folder the last search sent to gozd was kept inside.
    applied: RefCell<Option<String>>,
    /// The window is ending: closing the panes searches nothing again.
    ending: Cell<bool>,
}

impl Sidebars {
    pub(super) fn new() -> Sidebars {
        Sidebars {
            folder: RefCell::new(None),
            applied: RefCell::new(None),
            ending: Cell::new(false),
        }
    }
}

impl App {
    /// F2's part of the window's start (`slots.rs`): the three panes, in
    /// Everything's order, opened as the settings left them; and the
    /// folder of the session a window that restores one starts with.
    pub(super) fn start_sidebars(&self) {
        if let Some(s) = self.sessions.at_start() {
            *self.sidebars.folder.borrow_mut() = s.view().search.folder.clone();
        }
        self.register_pane(PaneSpec {
            min_high: FOLDERS_MIN_HIGH,
            ..PaneSpec::new(
                PaneId::Folders,
                Box::new(|| Rc::new(FoldersPane::new()) as Rc<dyn PaneContent>),
            )
        });
        self.register_pane(PaneSpec::new(
            PaneId::Filters,
            Box::new(|| Rc::new(FiltersPane::new()) as Rc<dyn PaneContent>),
        ));
        self.register_pane(PaneSpec {
            min_high: BOOKMARKS_MIN_HIGH,
            ..PaneSpec::new(
                PaneId::Bookmarks,
                Box::new(|| Rc::new(BookmarksPane::new()) as Rc<dyn PaneContent>),
            )
        });
    }

    /// The window is ending (`slots.rs`): the panes close without a search.
    pub(super) fn exit_sidebars(&self) {
        self.sidebars.ending.set(true);
    }

    /// F2's commands that reach the window (`slots.rs`): true when one ran.
    pub(super) fn sidebar_command(&self, id: u16) -> bool {
        match id {
            CLEAR_SELECTION => self.clear_folder(),
            UP_ONE_FOLDER => {
                self.up_one_folder();
            }
            _ => return false,
        }
        true
    }

    /// The sidebars' settings.
    pub(super) fn sidebar_settings(&self) -> sidebars::Settings {
        self.prefs.borrow().base.sidebars.clone()
    }

    /// The Folders pane's folder, scoping the search or not.
    pub(super) fn folder_selection(&self) -> Option<String> {
        self.sidebars.folder.borrow().clone()
    }

    /// The folder the search is kept inside now: the Folders pane's, while
    /// the pane is open (or always, with
    /// `folders_sidebar_match_only_when_shown=0`).
    pub(super) fn folder_scope(&self) -> Option<String> {
        let folder = self.folder_selection()?;
        let only_shown = self.prefs.borrow().base.sidebars.match_only_when_shown;
        (!only_shown || self.pane_visible(PaneId::Folders)).then_some(folder)
    }

    /// The folder the status bar shows, Everything's place for the Folders
    /// sidebar's selection: while it scopes the search.
    pub fn folder_indicator(&self) -> Option<String> {
        self.folder_scope()
    }

    /// What gozd gets for the search box's `text` (`start_search`): the
    /// folder's path term in front of it while a folder scopes the search.
    pub(super) fn scoped_search(&self, text: &str) -> String {
        let scope = self.folder_scope();
        let query = sidebars::scoped(scope.as_deref(), text);
        *self.sidebars.applied.borrow_mut() = scope;
        query
    }

    /// The Folders pane chose `folder` (`None`: its top item, no folder):
    /// the search runs again inside it, as a new search.
    pub(super) fn choose_folder(&self, folder: Option<String>) {
        let folder = folder.map(|f| sidebars::normalize(&f));
        if *self.sidebars.folder.borrow() == folder {
            return;
        }
        *self.sidebars.folder.borrow_mut() = folder;
        self.invalidate_status();
        self.start_search(false);
    }

    /// Back, Forward, a history entry or a session brings back `folder`:
    /// kept, and shown in the tree; the search that brings it back follows
    /// (the caller's).
    pub(super) fn restore_folder(&self, folder: Option<String>) {
        let folder = folder.map(|f| sidebars::normalize(&f));
        if *self.sidebars.folder.borrow() == folder {
            return;
        }
        *self.sidebars.folder.borrow_mut() = folder;
        self.invalidate_status();
        self.folders_follow();
    }

    /// `folder` from outside the tree (Up One Folder, Clear Selection): the
    /// search runs inside it, and the tree shows it.
    fn move_to_folder(&self, folder: Option<String>) {
        self.choose_folder(folder);
        self.folders_follow();
    }

    /// The Folders pane, if open, shows the folder kept.
    fn folders_follow(&self) {
        if let Some(content) = self.pane_content(PaneId::Folders)
            && let Some(pane) = content.as_any().downcast_ref::<FoldersPane>()
        {
            pane.follow(self);
        }
    }

    /// Up One Folder (Alt+Up, Everything's "Up One Dir"): the folder above
    /// the one scoping the search; from a drive's root, no folder. False
    /// while no folder scopes the search.
    pub(super) fn up_one_folder(&self) -> bool {
        let Some(folder) = self.folder_scope() else {
            return false;
        };
        self.move_to_folder(sidebars::parent_folder(&folder));
        true
    }

    /// Clear Selection: no folder.
    pub(super) fn clear_folder(&self) {
        self.move_to_folder(None);
    }

    /// The Folders pane opened (`shown`) or closed: its folder scopes the
    /// search from now on, or no longer (closed, it is forgotten with
    /// `folders_sidebar_clear_selection_on_close=1`); the search runs again
    /// when that changes what gozd gets.
    pub(super) fn folders_pane_shown(&self, shown: bool) {
        if self.sidebars.ending.get() {
            return;
        }
        if !shown && self.prefs.borrow().base.sidebars.clear_on_close {
            *self.sidebars.folder.borrow_mut() = None;
        }
        let scope = self.folder_scope();
        let changed = *self.sidebars.applied.borrow() != scope;
        if changed {
            self.invalidate_status();
            self.start_search(false);
        }
    }

    /// The Bookmarks pane's open folders changed (`bookmarks_sidebar_
    /// expanded`).
    pub(super) fn set_bookmarks_expanded(&self, folders: Vec<String>) {
        {
            let mut prefs = self.prefs.borrow_mut();
            if prefs.base.sidebars.bookmarks_expanded == folders {
                return;
            }
            prefs.base.sidebars.bookmarks_expanded = folders;
        }
        self.settings_changed();
    }

    /// The bookmarks may have changed (here, or in another goz-gui whose
    /// file was read again): the Bookmarks pane, if open, shows them as
    /// they are.
    pub(super) fn bookmarks_changed(&self) {
        if let Some(content) = self.pane_content(PaneId::Bookmarks)
            && let Some(pane) = content.as_any().downcast_ref::<BookmarksPane>()
        {
            pane.sync(self);
        }
    }

    /// WM_CONTEXTMENU at `l` (screen) over the status bar: Everything's menu
    /// there, the bars and the panes to show or hide, with Clear Selection
    /// first over the folder. False when `l` is not over the status bar.
    pub(super) fn status_menu(&self, l: LPARAM) -> bool {
        if l == -1 {
            return false;
        }
        let screen = point(l);
        let mut pt = screen;
        // SAFETY: converts a screen point to this thread's window's.
        unsafe { ScreenToClient(self.hwnd, &mut pt) };
        if !in_rect(&self.status_rc.get(), pt) {
            return false;
        }
        let on_folder = status::indicator_at(self, pt) == Some(Indicator::Folder);
        // Everything's labels, without the shortcut the View menu shows.
        let mut panes: Vec<(u32, u16, &str)> = PaneId::KNOWN
            .into_iter()
            .filter(|&p| self.pane_registered(p))
            .filter_map(|p| {
                p.menu_item().map(|(cmd, label)| {
                    let label = label.split('\t').next().unwrap_or(label);
                    (p.menu_rank(), cmd, label)
                })
            })
            .collect();
        panes.sort_by_key(|&(rank, ..)| rank);
        let mut items = Vec::new();
        if on_folder {
            items.extend([Item::Cmd(CLEAR_SELECTION, "Clear Selection"), Item::Sep]);
        }
        items.extend([
            Item::Cmd(menu::FILTER_BAR, "Filt&er Bar"),
            Item::Cmd(menu::STATUS_BAR, "Status &Bar"),
        ]);
        if !panes.is_empty() {
            items.push(Item::Sep);
            items.extend(panes.iter().map(|&(_, cmd, label)| Item::Cmd(cmd, label)));
        }
        let popup = menu::submenu(&items);
        // The window readies the checks as it opens (WM_INITMENUPOPUP).
        let id = self.track_with_shell(popup, screen, "status bar menu", None);
        // SAFETY: the menu made above.
        unsafe { DestroyMenu(popup) };
        match id as u16 {
            0 => {}
            CLEAR_SELECTION => self.clear_folder(),
            id => self.command(id),
        }
        true
    }
}
