//! The Bookmarks menu and its dialogs: Add to Bookmarks... saves the search
//! on screen; Find Bookmark... picks one; Organize Bookmarks... edits them,
//! with folders; and the menu lists them, folders as submenus, each
//! bookmark bringing its search back as a navigation entry.
//!
//! - **The file** (`Bookmarks.csv`, `crate::bookmarks::file`) is read at
//!   startup, and again when another program changed it, as the menu or a
//!   dialog opens, unless a change made here waits to be written.
//! - **Writing**: every change is handed to the store thread at once. Its
//!   write reads the file again and makes this window's changes on it (the
//!   merge rule, `crate::bookmarks::ops`), so two goz-gui windows never
//!   write each other's bookmarks out.
//! - **New bookmarks** take the window's options as the
//!   `bookmark_remember_*` settings say (Everything's defaults: the match
//!   options and the search; the filter and sort "(No change)").

use std::cell::{Cell, RefCell};
use std::sync::Arc;

use windows_sys::Win32::Foundation::HWND;
use windows_sys::Win32::System::Diagnostics::Debug::MessageBeep;
use windows_sys::Win32::UI::WindowsAndMessaging::{HMENU, IsWindow, MB_ICONWARNING, PostMessageW};

use super::{App, Options, WM_BOOKMARKS, prefs};
use crate::bookmark_dialogs::{self, Chosen, Host, WM_REFRESH};
use crate::bookmarks::file::{BookmarkFile, FILE_NAME};
use crate::bookmarks::ops::Op;
use crate::bookmarks::tree::{Item, TOP, Tree};
use crate::bookmarks::{Bookmark, Bookmarks, Kind, Remember, Settings, one_line};
use crate::clipboard;
use crate::dialog::{Look, Quiet};
use crate::menu::{self, Mark};
use crate::settings;
use crate::store::location;
use crate::theme::ThemeMode;
use crate::wide::window_text;

/// The name a new bookmark of an empty search is offered, Everything's.
const NEW_BOOKMARK: &str = "New Bookmark";

/// The bookmarks, their file and settings.
pub(super) struct Marks {
    file: Arc<BookmarkFile>,
    remember: Remember,
    /// What each bookmark of the menu opens, as the menu was last filled.
    menu: RefCell<Vec<Chosen>>,
    /// The Find or Organize dialog open, told when the bookmarks change.
    dialog: Cell<HWND>,
    /// What to tell the user once the window is up: the file could not be
    /// read.
    pub note: RefCell<Option<String>>,
}

impl Marks {
    /// Reads the bookmarks (before the window exists: the file is small).
    pub(super) fn load(s: &settings::Settings) -> Marks {
        let (file, note) = BookmarkFile::load(location::data_file(FILE_NAME), s.csv_backup);
        Marks {
            file: Arc::new(file),
            remember: s.bookmark_remember,
            menu: RefCell::new(Vec::new()),
            dialog: Cell::new(core::ptr::null_mut()),
            note: RefCell::new(note),
        }
    }
}

/// The menu's entries for what `node` holds, and what each bookmark opens,
/// depth first (the menu's ids follow the same order).
fn marks(list: &Bookmarks, tree: &Tree, node: usize, opens: &mut Vec<Chosen>) -> Vec<Mark> {
    tree.nodes[node]
        .items
        .iter()
        .filter_map(|&item| match item {
            Item::Row(r) => {
                let row = &list.rows()[r];
                match row.kind() {
                    Kind::Separator => Some(Mark::Separator),
                    Kind::Bookmark => {
                        opens.push(Chosen {
                            search: row.search_set().then(|| row.search().to_owned()),
                            settings: row.settings(),
                        });
                        Some(Mark::Bookmark(row.name().to_owned()))
                    }
                    Kind::Folder | Kind::Other => None,
                }
            }
            Item::Folder(f) => Some(Mark::Folder(
                tree.nodes[f].name().to_owned(),
                marks(list, tree, f, opens),
            )),
        })
        .collect()
}

/// Posts `value` to the window as `msg`'s lParam; freed again if the window
/// is gone.
fn post<T>(hwnd: usize, msg: u32, value: T) {
    let raw = Box::into_raw(Box::new(value));
    // SAFETY: posts to the window; if that fails the box is taken back.
    unsafe {
        if PostMessageW(hwnd as HWND, msg, 0, raw as isize) == 0 {
            drop(Box::from_raw(raw));
        }
    }
}

impl App {
    /// How the bookmark and search history dialogs look: as the window
    /// does.
    pub(super) fn dialog_look(&self) -> Look {
        Look {
            dark: self.pal.get().dark,
            follow_system: self.theme_mode() == ThemeMode::UserDefault,
        }
    }

    /// Reads the file again if another program changed it.
    pub(super) fn refresh_bookmarks(&self) {
        let (_, note) = self.bookmarks.file.refresh();
        if let Some(note) = note {
            self.show_note(note);
        }
        // F2: the Bookmarks pane shows what was read.
        self.bookmarks_changed();
    }

    /// Rebuilds the Bookmarks menu's list as it opens.
    pub(super) fn fill_bookmarks_menu(&self, popup: HMENU) {
        self.refresh_bookmarks();
        let list = self.bookmarks.file.current();
        let tree = Tree::build(&list);
        let mut opens = Vec::new();
        let entries = marks(&list, &tree, TOP, &mut opens);
        *self.bookmarks.menu.borrow_mut() = opens;
        menu::fill_bookmarks(popup, &entries);
    }

    /// The options a new bookmark takes from the window.
    fn remembered(&self) -> Settings {
        let o = self.opts.get();
        self.bookmarks
            .remember
            .settings(o.match_case, o.match_path, o.filter, o.sort)
    }

    /// The bookmarks as they are now (the jump list's, `custom.rs`).
    pub(super) fn bookmark_list(&self) -> Arc<crate::bookmarks::Bookmarks> {
        self.bookmarks.file.current()
    }

    /// Hands the changes made here to the store thread.
    pub(super) fn save_bookmarks(&self) {
        let file = &self.bookmarks.file;
        if !file.pending() {
            return;
        }
        // The taskbar's jump list shows the bookmarks.
        self.jump_list_later();
        let Some(path) = file.path().map(ToOwned::to_owned) else {
            self.show_note("The bookmarks cannot be saved: goz-gui has no data folder".to_owned());
            return;
        };
        let hwnd = self.hwnd as usize;
        let job = Arc::clone(file);
        self.writer.submit(
            &path,
            Box::new(move || match job.write_now() {
                Ok(Some(note)) => post(hwnd, WM_BOOKMARKS, note),
                Ok(None) => {}
                Err(why) => prefs::report(hwnd, why, true),
            }),
        );
    }

    /// The store thread wrote the bookmarks: a dialog showing them shows
    /// what another goz-gui added meanwhile.
    pub(super) fn bookmarks_written(&self, note: Option<String>) {
        if let Some(note) = note {
            self.show_note(note);
        }
        // F2: the write merged another goz-gui's changes in.
        self.bookmarks_changed();
        let dlg = self.bookmarks.dialog.get();
        // SAFETY: checks and posts to a window of this thread.
        unsafe {
            if !dlg.is_null() && IsWindow(dlg) != 0 {
                PostMessageW(dlg, WM_REFRESH, 0, 0);
            }
        }
    }

    /// Bookmarks > Add to Bookmarks... (Ctrl+D): the editor, named after
    /// the search ("New Bookmark" for none, as Everything's), then the
    /// bookmark at the end of the top folder.
    pub(super) fn add_bookmark(&self) {
        if self.run_history_dialog_open() || self.bookmark_dialog.replace(true) {
            return;
        }
        self.refresh_bookmarks();
        let text = window_text(self.edit);
        let name = if text.is_empty() { NEW_BOOKMARK } else { &text };
        let mut row = Bookmark::new(&[], name, &text, &self.remembered());
        if !self.bookmarks.remember.search {
            row.set_search(None);
        }
        let look = self.dialog_look();
        let made = {
            // Opened by a command posted while goz-gui is in the
            // background: shown, but not brought to the front.
            let _quiet = Quiet::begin();
            bookmark_dialogs::edit(self.hwnd, "New Bookmark", &row.fields(), look)
        };
        self.bookmark_dialog.set(false);
        if let Some(fields) = made {
            row.set_fields(&fields);
            self.change_bookmarks(Op::Add(row));
        }
    }

    /// Bookmarks > Find Bookmark... (Ctrl+B).
    pub(super) fn find_bookmark(&self) {
        if self.search_history_dialog_open()
            || self.run_history_dialog_open()
            || self.bookmark_dialog.replace(true)
        {
            return;
        }
        self.refresh_bookmarks();
        let chosen = {
            let _quiet = Quiet::begin();
            bookmark_dialogs::find(self.hwnd, self, self.dialog_look())
        };
        self.bookmark_dialog.set(false);
        if let Some(c) = chosen {
            self.open_chosen(&c);
        }
    }

    /// Bookmarks > Organize Bookmarks... (Ctrl+Shift+B, Ctrl+Shift+O).
    pub(super) fn organize_bookmarks(&self) {
        if self.search_history_dialog_open()
            || self.run_history_dialog_open()
            || self.bookmark_dialog.replace(true)
        {
            return;
        }
        self.refresh_bookmarks();
        {
            let _quiet = Quiet::begin();
            bookmark_dialogs::organize(self.hwnd, self, self.dialog_look());
        }
        self.bookmark_dialog.set(false);
    }

    /// Bookmarks menu entry `index`: what it opened when the menu was
    /// filled (or, for a command that comes without the menu, such as one
    /// posted by another program, what it opens now).
    pub(super) fn open_bookmark(&self, index: usize) {
        let mut chosen = self.bookmarks.menu.borrow().get(index).cloned();
        if chosen.is_none() {
            self.refresh_bookmarks();
            let list = self.bookmarks.file.current();
            let mut opens = Vec::new();
            marks(&list, &Tree::build(&list), TOP, &mut opens);
            chosen = opens.get(index).cloned();
        }
        if let Some(c) = chosen {
            self.open_chosen(&c);
        }
    }

    /// A bookmark's search with its options, as a new navigation entry. An
    /// option the bookmark does not set stays as it is; with no search
    /// ("(No change)") the search in the box stays.
    pub(super) fn open_chosen(&self, c: &Chosen) {
        let o = self.opts.get();
        let s = c.settings;
        let text = match &c.search {
            Some(search) => one_line(search),
            None => window_text(self.edit),
        };
        self.navigate(
            &text,
            Options {
                match_case: s.match_case.unwrap_or(o.match_case),
                match_path: s.match_path.unwrap_or(o.match_path),
                filter: s.filter.unwrap_or(o.filter),
                sort: s.sort.unwrap_or(o.sort),
            },
        );
    }

    fn change_bookmarks(&self, op: Op) -> bool {
        let changed = self.bookmarks.file.change(op);
        if changed {
            self.save_bookmarks();
            // F2: the Bookmarks pane shows the change.
            self.bookmarks_changed();
        }
        changed
    }
}

impl Host for App {
    fn bookmarks(&self) -> (Arc<Bookmarks>, u64) {
        let file = &self.bookmarks.file;
        (file.current(), file.generation())
    }

    fn change(&self, op: Op) -> bool {
        self.change_bookmarks(op)
    }

    fn copy(&self, text: &str) -> bool {
        let wide: Vec<u16> = text.encode_utf16().collect();
        let ok = clipboard::put_text(self.hwnd, &wide);
        if !ok {
            self.show_note(clipboard::BUSY.into());
            // SAFETY: plays a system sound.
            unsafe { MessageBeep(MB_ICONWARNING) };
        }
        ok
    }

    fn new_settings(&self) -> Settings {
        self.remembered()
    }

    fn showing(&self, dlg: HWND) {
        self.bookmarks.dialog.set(dlg);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bookmarks::tree::tests::list;

    #[test]
    fn the_menu_follows_the_folders() {
        let l = list(&[
            ("b", "", "one"),
            ("f", "", "Music"),
            ("b", "Music/Rock", "queen"),
            ("-", "", ""),
            ("b", "Music", "abba"),
            ("f", "", "Empty"),
        ]);
        let t = Tree::build(&l);
        let mut opens = Vec::new();
        let m = marks(&l, &t, TOP, &mut opens);
        assert_eq!(
            m,
            [
                Mark::Bookmark("one".into()),
                Mark::Folder(
                    "Music".into(),
                    vec![
                        Mark::Folder("Rock".into(), vec![Mark::Bookmark("queen".into())]),
                        Mark::Bookmark("abba".into()),
                    ]
                ),
                Mark::Separator,
                Mark::Folder("Empty".into(), Vec::new()),
            ]
        );
        // The ids' order: depth first.
        let searches: Vec<_> = opens
            .iter()
            .map(|c| c.search.clone().unwrap_or_default())
            .collect();
        assert_eq!(searches, ["one", "queen", "abba"]);
    }
}
