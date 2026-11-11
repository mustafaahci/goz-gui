//! Wave 4's data and file tools from the window's side (G1; the work is
//! `crate::filelist` and `crate::datatools`): File > Open File List...
//! (Ctrl+O), Recent File Lists > and Close File List; the open list's
//! name in the status bar and the title; Tools > Import and Export
//! Settings...; Help > Troubleshooting Information. (Edit > Advanced >
//! Advanced Copy/Move to Folder... run through the file tools,
//! `file_tools.rs`.)
//!
//! An open file list is the shown tab's (wave 5; Everything's `index` and
//! `index_file_list` per tab in `Session.json`): each search the window
//! makes carries it (`client::Search::filelist`), and the search worker,
//! Export and the find bar answer from its rows instead of gozd's. The
//! lists other open tabs search are held while those tabs are, so going
//! back to one costs nothing; a list no open tab names any more is let go,
//! and read again when a tab that names it (a reopened tab, a restored
//! session) shows. A new tab searches the index, as Everything's Home
//! (`home_index`) says by default. Dropping `.efu` files on the window
//! opens the first as a list (`drop_file_list`).
//!
//! While the shown tab's list has dates created, the results have a Date
//! Created column after the layout's, which sorts the list's matches by
//! that date (`created_sort`), a sort only a list can make: any other sort
//! ends it, and so does another list or tab.
//!
//! No `RefCell` of the window is borrowed while a dialog runs.

use std::cell::{Cell, RefCell};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use goz_core::types::SortDir;

use windows_sys::Win32::Foundation::{LPARAM, WPARAM};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    DestroyWindow, HMENU, IDYES, MB_ICONINFORMATION, MB_ICONQUESTION, MB_ICONWARNING, MB_OK,
    MB_YESNO,
};

use super::App;
use crate::datatools::settings_io::{self, Action};
use crate::datatools::troubleshoot::{self, Facts};
use crate::datatools::{MSG_FIRST, cmd};
use crate::dialog::Quiet;
use crate::filelist::settings::MENU_MAX;
use crate::filelist::{FileList, Notify};
use crate::menu::{self, Item};
use crate::msgbox;
use crate::store::location;

/// A file list finished reading: wParam 1 when every row was read, 0 when
/// reading stopped on an error; lParam the list's id (the shown tab's list
/// or one held for another tab).
pub(super) const WM_FILELIST: u32 = MSG_FIRST;
/// The other windows asked to end for Import or Reset Settings have ended
/// (or the wait for them is over): the files can be replaced.
pub(super) const WM_OTHERS_ENDED: u32 = MSG_FIRST + 1;

/// How long Import and Reset wait for the other windows to end, as
/// `-wait-pid` waits for this one.
const OTHERS_WAIT_MS: u32 = 10_000;

/// What Import or Reset Settings does once no other window of this
/// goz-gui is left to write over it.
enum Replace {
    Import { data: PathBuf, folder: PathBuf },
    Reset { data: PathBuf },
}

/// What the command line asks to be done with no window, before any
/// (`-create-file-list`, `-install-efu-association`, ...): the exit code
/// when the process is to end then. `-wait-pid` waits here for the
/// goz-gui that restarts itself to end (at most 10 s).
pub(super) fn before_window(l: &crate::cmdline::Launch) -> Option<i32> {
    if let Some(pid) = l.wait_pid {
        wait_for_process(pid, 10_000);
    }
    if let Some((file, folders)) = &l.create_file_list {
        return Some(
            match crate::filelist::create::create(Path::new(file), folders, &l.create_options) {
                Ok(_) => 0,
                Err(_) => 1,
            },
        );
    }
    if let Some(on) = l.efu_association {
        return Some(if crate::integration::set_efu_association(on) {
            0
        } else {
            1
        });
    }
    None
}

/// Waits for process `pid` to end, at most `ms`.
fn wait_for_process(pid: u32, ms: u32) {
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::System::Threading::{
        OpenProcess, PROCESS_SYNCHRONIZE, WaitForSingleObject,
    };
    // SAFETY: opens the process for waiting only, waits, and closes the
    // handle; a process gone already gives no handle.
    unsafe {
        let h = OpenProcess(PROCESS_SYNCHRONIZE, 0, pid);
        if !h.is_null() {
            WaitForSingleObject(h, ms);
            CloseHandle(h);
        }
    }
}

/// The data tools' state in the window.
pub(super) struct DataTools {
    /// The shown tab's file list.
    list: RefCell<Option<Arc<FileList>>>,
    /// The lists the other open tabs search (W5).
    held: RefCell<Vec<Arc<FileList>>>,
    /// Held lists whose reading ended while held (their ids): how it ended
    /// is said when their tab shows (W5R-5).
    unsaid: RefCell<Vec<u64>>,
    /// The shown list's matches are sorted by its Date Created (W5).
    created_sort: Cell<Option<SortDir>>,
    /// The Date Created column's width at 96 DPI, while the window lasts.
    created_width: Cell<i32>,
    /// One of their dialogs is open.
    dialog: Cell<bool>,
    /// Import or Reset Settings waits for the other windows to end.
    replacing: RefCell<Option<Replace>>,
    /// W6: the shown tab's file-list slots (`lists.rs`).
    pub(super) slots: RefCell<Arc<crate::filelist::slots::Slots>>,
    /// W6: goz-gui was started for the File List Editor alone (`-edit`,
    /// `-filelist-editor`): it ends with the editor while its window was
    /// never shown.
    pub(super) editor_alone: Cell<bool>,
    /// W7: a `save-filelistN:` search's results being read (`lists.rs`).
    pub(super) saving: RefCell<Option<crate::client::Stop>>,
    /// W7: the search shown was made, and its save-filelistN: terms save
    /// its results once they are all there.
    pub(super) save_due: Cell<bool>,
}

/// Whose a list whose reading ended is (its WM_FILELIST).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Reader {
    /// The shown tab's.
    Shown,
    /// Another tab's, held: kept to say when that tab shows.
    Held,
    /// No tab's any more.
    Gone,
}

impl DataTools {
    /// The reading of the list `id` ended: whose it is (a held one's end
    /// is kept, to be said when its tab shows).
    fn read_ended(&self, id: u64) -> Reader {
        if self.list.borrow().as_ref().is_some_and(|l| l.id() == id) {
            Reader::Shown
        } else if self.held.borrow().iter().any(|l| l.id() == id) {
            self.unsaid.borrow_mut().push(id);
            Reader::Held
        } else {
            Reader::Gone
        }
    }

    /// The held `list` is to show: whether how its reading ended is still
    /// to be said (it is not kept any more).
    fn take_unsaid(&self, list: &FileList) -> bool {
        let mut unsaid = self.unsaid.borrow_mut();
        let at = unsaid.iter().position(|&id| id == list.id());
        at.map(|i| unsaid.swap_remove(i)).is_some()
    }

    /// A navigation entry or a tab is shown (Back and Forward, a bookmark,
    /// a tab switch; W5R-3): its sort is the one in force, so a Date
    /// Created sort, which no entry or tab keeps, ends.
    fn state_shown(&self) {
        self.created_sort.set(None);
    }

    /// Only the lists still shown or held are kept to say.
    fn forget_unsaid_of_gone(&self) {
        let list = self.list.borrow();
        let held = self.held.borrow();
        self.unsaid
            .borrow_mut()
            .retain(|&id| list.iter().chain(held.iter()).any(|l| l.id() == id));
    }

    pub(super) fn new() -> DataTools {
        DataTools {
            list: RefCell::new(None),
            held: RefCell::new(Vec::new()),
            unsaid: RefCell::new(Vec::new()),
            created_sort: Cell::new(None),
            created_width: Cell::new(crate::columns::DEFAULT_WIDTHS[crate::columns::COL_DATE]),
            dialog: Cell::new(false),
            replacing: RefCell::new(None),
            slots: RefCell::new(Arc::default()),
            editor_alone: Cell::new(false),
            saving: RefCell::new(None),
            save_due: Cell::new(false),
        }
    }
}

/// Whether `list` is the file at `path` (Windows' paths: any case).
fn is_list(list: &FileList, path: &str) -> bool {
    let full = std::path::absolute(Path::new(path)).unwrap_or_else(|_| PathBuf::from(path));
    list.path()
        .as_os_str()
        .to_string_lossy()
        .eq_ignore_ascii_case(&full.as_os_str().to_string_lossy())
}

/// The file list a drop opens: the first of `paths` when every one is an
/// `.efu` file (`is_dir` says which are folders); `None` when any is not,
/// and the drop searches for the paths as before.
pub(super) fn efu_drop(paths: &[Vec<u16>], is_dir: impl Fn(&[u16]) -> bool) -> Option<String> {
    let efu = |p: &Vec<u16>| {
        String::from_utf16_lossy(p)
            .to_ascii_lowercase()
            .ends_with(".efu")
            && !is_dir(p)
    };
    (!paths.is_empty() && paths.iter().all(efu)).then(|| String::from_utf16_lossy(&paths[0]))
}

/// Open File List...'s file types: Everything's.
const LIST_KINDS: &[crate::filedlg::Kind] = &[
    ("Everything File List (*.efu)", "*.efu"),
    ("Text Files (*.txt)", "*.txt"),
    ("CSV Files (*.csv)", "*.csv"),
    ("All Files (*.*)", "*.*"),
];

impl App {
    /// The open file list, if one is.
    pub(super) fn filelist(&self) -> Option<Arc<FileList>> {
        self.data_tools.list.borrow().clone()
    }

    /// What the status bar shows for the open file list: its name.
    pub fn filelist_indicator(&self) -> Option<String> {
        self.filelist().map(|l| l.name().to_owned())
    }

    /// The window starts: Tools and Help get their items, in Everything's
    /// places (File's are put in as it opens, Edit's by the file tools).
    pub(super) fn data_tools_start(&self) {
        menu::add_items_before(
            self.menus.tools,
            menu::OPTIONS,
            // Everything's access key, S, is Start goz Service's here. W5:
            // the File List Editor first, in Everything's place.
            &[
                Item::Cmd(
                    crate::listedit::cmd::FILE_LIST_EDITOR,
                    "File List &Editor...",
                ),
                Item::Cmd(
                    cmd::IMPORT_EXPORT_SETTINGS,
                    "&Import and Export Settings...",
                ),
            ],
        );
        menu::add_items(
            self.menus.help,
            Some(menu::SEARCH_SYNTAX),
            &[
                Item::Sep,
                Item::Cmd(cmd::TROUBLESHOOTING, "&Troubleshooting Information"),
            ],
        );
    }

    /// The window is ending: a list still being read stops.
    pub(super) fn data_tools_exit(&self) {
        if let Some(list) = self.data_tools.list.borrow_mut().take() {
            list.close();
        }
        for list in self.data_tools.held.take() {
            list.close();
        }
        // W7: a save-filelistN: search's read stops; its thread posts
        // nothing then.
        if let Some(stop) = self.data_tools.saving.take() {
            stop.stop();
        }
    }

    // ---- W5: a file list per tab, and its Date Created ----

    /// The full path the shown tab's list is kept under in the session.
    pub(super) fn filelist_path(&self) -> Option<String> {
        self.filelist()
            .map(|l| l.path().as_os_str().to_string_lossy().into_owned())
    }

    /// The lists the open tabs other than the shown one search, by path
    /// (the shown tab's own entry is not up to date while it shows).
    fn other_tabs_lists(&self) -> Vec<String> {
        let t = self.tabs.borrow();
        t.tabs()
            .iter()
            .enumerate()
            .filter(|&(i, _)| i != t.current())
            .filter_map(|(_, v)| v.file_list.clone())
            .collect()
    }

    /// `old` is not the shown tab's list any more: held while another open
    /// tab searches it, else let go.
    fn release_list(&self, old: Arc<FileList>) {
        if self.other_tabs_lists().iter().any(|p| is_list(&old, p)) {
            self.data_tools.held.borrow_mut().push(old);
        } else {
            old.close();
        }
    }

    /// The tab about to show searches `want` (a session's tab, another
    /// tab's, a reopened one's), or the index for `None`: the window's list
    /// becomes that one, the one held for it or (not held) opened again
    /// from its file; a list that cannot be opened now is said in the
    /// status bar and the tab searches the index. Lists no open tab names
    /// any more are let go.
    pub(super) fn tab_file_list(&self, want: Option<&str>) {
        let current = self.filelist();
        let same = match (&current, want) {
            (Some(l), Some(p)) => is_list(l, p),
            (None, None) => true,
            _ => false,
        };
        if !same {
            let mut held = self.data_tools.held.take();
            if let Some(old) = self.data_tools.list.borrow_mut().take() {
                held.push(old);
            }
            // A held list whose reading ended while held: said now.
            let mut unsaid = None;
            let next = want.and_then(|p| {
                if let Some(i) = held.iter().position(|l| is_list(l, p)) {
                    let list = held.remove(i);
                    if self.data_tools.take_unsaid(&list) {
                        unsaid = Some(Arc::clone(&list));
                    }
                    return Some(list);
                }
                self.open_list(p, false)
            });
            *self.data_tools.list.borrow_mut() = next;
            *self.data_tools.held.borrow_mut() = held;
            self.list_changed();
            if let Some(list) = unsaid {
                self.say_how_read(&list);
            }
        }
        // The tabs' lists as they are now: the others are let go.
        let named: Vec<String> = self
            .tabs
            .borrow()
            .tabs()
            .iter()
            .filter_map(|v| v.file_list.clone())
            .collect();
        self.data_tools.held.borrow_mut().retain(|l| {
            let keep = named.iter().any(|p| is_list(l, p));
            if !keep {
                l.close();
            }
            keep
        });
        self.data_tools.forget_unsaid_of_gone();
    }

    /// Opens the list at `path` to read; said when it cannot be: in a
    /// message box when the user asked (`ask`), else in the status bar.
    fn open_list(&self, path: &str, ask: bool) -> Option<Arc<FileList>> {
        let full = std::path::absolute(Path::new(path)).unwrap_or_else(|_| PathBuf::from(path));
        let notify = Notify {
            hwnd: self.hwnd as usize,
            msg: WM_FILELIST,
        };
        match FileList::open(&full, Some(notify)) {
            Ok(list) => Some(list),
            Err(e) if ask => {
                msgbox::show(
                    self.hwnd,
                    &format!(
                        "The file list could not be opened:\n\n{}\n\n{e}",
                        full.display()
                    ),
                    "Open File List",
                    MB_OK | MB_ICONWARNING,
                );
                None
            }
            Err(e) => {
                self.show_note(format!(
                    "The file list {} could not be opened: {e}",
                    full.display()
                ));
                None
            }
        }
    }

    /// The shown tab's list changed (another, none, or another tab's): a
    /// Date Created sort ends, and the column comes or goes.
    fn list_changed(&self) {
        if self.data_tools.created_sort.take().is_some() {
            self.show_sort();
        }
        self.refresh_created_column();
        self.invalidate_status();
    }

    /// Whether the results have the Date Created column now: the shown
    /// tab's list has dates created.
    pub(super) fn created_column_wanted(&self) -> bool {
        self.filelist().is_some_and(|l| l.has_created())
    }

    /// Puts the Date Created column on the list or takes it off, as the
    /// shown list says (its dates may show up as it is read); a sort by it
    /// ends with it.
    pub(super) fn refresh_created_column(&self) {
        let wanted = self.created_column_wanted();
        if wanted == self.list_cols.get().has_created() {
            return;
        }
        if !wanted && self.data_tools.created_sort.take().is_some() {
            self.show_sort();
        }
        // The user's last widths and order first.
        self.sync_columns();
        self.apply_columns(self.layout.get());
    }

    pub(super) fn created_width(&self) -> i32 {
        self.data_tools.created_width.get()
    }

    pub(super) fn set_created_width(&self, width: i32) {
        self.data_tools
            .created_width
            .set(width.clamp(crate::columns::MIN_WIDTH, 8192));
    }

    /// The Date Created sort in force: only while the shown list has the
    /// column.
    pub(super) fn created_sort(&self) -> Option<SortDir> {
        self.data_tools
            .created_sort
            .get()
            .filter(|_| self.created_column_wanted())
    }

    /// Sorts the list's matches by Date Created, `dir`.
    pub(super) fn set_created_sort(&self, dir: SortDir) {
        if !self.created_column_wanted() {
            return;
        }
        self.data_tools.created_sort.set(Some(dir));
        self.show_sort();
        self.start_search(false);
    }

    /// A click on Date Created's header: the other way round when it is
    /// the sort, else newest first (as Date Modified's first sort, Options
    /// > Results' "Sort date descending first").
    pub(super) fn sort_by_created(&self) {
        let dir = match self.created_sort() {
            Some(SortDir::Asc) => SortDir::Desc,
            Some(SortDir::Desc) => SortDir::Asc,
            None => self.first_sort(goz_core::types::SortKey::DateModified).dir,
        };
        self.set_created_sort(dir);
    }

    /// Another sort was chosen: the Date Created sort ends.
    pub(super) fn end_created_sort(&self) {
        self.data_tools.created_sort.set(None);
    }

    /// A navigation entry or a tab is shown: [`DataTools::state_shown`].
    pub(super) fn created_sort_state_shown(&self) {
        self.data_tools.state_shown();
    }

    /// Files dropped on the window: when they are all `.efu` files, the
    /// first opens as a file list (true); otherwise false, and the drop
    /// searches for them as before.
    pub(super) fn drop_file_list(&self, paths: &[Vec<u16>]) -> bool {
        let is_dir = |p: &[u16]| crate::filetools::attributes(p).is_some_and(|a| a & 0x10 != 0);
        match efu_drop(paths, is_dir) {
            Some(list) => {
                if paths.len() > 1 {
                    self.show_note("Only the first file list dropped was opened".into());
                }
                self.open_file_list(&list);
                true
            }
            None => false,
        }
    }

    /// The window starts with the previous session's shown tab, which
    /// searched the list at `path`: it searches it again (a list gone
    /// since is said in the status bar).
    pub(super) fn start_with_file_list(&self, path: &str) {
        *self.data_tools.list.borrow_mut() = self.open_list(path, false);
        self.list_changed();
    }

    /// A menu is about to open: File gets Open File List..., Recent File
    /// Lists > and Close File List after New Window, as Everything's.
    pub(super) fn data_tools_menu_popup(&self, popup: HMENU) {
        if popup != self.menus.file {
            return;
        }
        let s = self.prefs.borrow().base.file_lists.clone();
        let labels: Vec<String> = if s.history_enabled {
            s.history
                .iter()
                .take(MENU_MAX)
                .map(|p| menu::label(p))
                .collect()
        } else {
            Vec::new()
        };
        const RECENT: &str = "Recent File Lists";
        let recent = if labels.is_empty() {
            Item::Greyed(RECENT)
        } else {
            let mut items: Vec<Item<'_>> = labels
                .iter()
                .enumerate()
                .map(|(i, l)| Item::Cmd(cmd::RECENT_FIRST + i as u16, l))
                .collect();
            items.push(Item::Sep);
            items.push(Item::Cmd(
                cmd::CLEAR_RECENT_FILE_LISTS,
                "&Clear Recent File Lists",
            ));
            Item::Sub(RECENT, menu::submenu(&items))
        };
        let close = if self.filelist().is_some() {
            Item::Cmd(cmd::CLOSE_FILE_LIST, "Close File List")
        } else {
            Item::Greyed("Close File List")
        };
        let items = [
            Item::Cmd(cmd::OPEN_FILE_LIST, "&Open File List...\tCtrl+O"),
            recent,
            close,
        ];
        let new_window = crate::tabs::NEW_WINDOW;
        if menu::has_item(popup, new_window) {
            menu::add_items(popup, Some(new_window), &items);
        } else {
            menu::add_items(popup, None, &items);
        }
    }

    /// A data tool's command; false for an id that is not one.
    pub(super) fn data_tools_command(&self, id: u16) -> bool {
        match id {
            cmd::OPEN_FILE_LIST => self.open_file_list_dialog(),
            cmd::CLOSE_FILE_LIST => self.close_file_list(),
            cmd::CLEAR_RECENT_FILE_LISTS => {
                self.prefs.borrow_mut().base.file_lists.history.clear();
                self.settings_changed();
            }
            cmd::ADVANCED_COPY_TO | cmd::ADVANCED_MOVE_TO => {
                return self.file_tools_command(id);
            }
            cmd::IMPORT_EXPORT_SETTINGS => self.import_export_settings(),
            cmd::TROUBLESHOOTING => self.troubleshooting(),
            n if (cmd::RECENT_FIRST..cmd::RECENT_FIRST + MENU_MAX as u16).contains(&n) => {
                let path = self
                    .prefs
                    .borrow()
                    .base
                    .file_lists
                    .history
                    .get(usize::from(n - cmd::RECENT_FIRST))
                    .cloned();
                if let Some(path) = path {
                    self.open_file_list(&path);
                }
            }
            _ => return false,
        }
        true
    }

    /// W5: Tools > File List Editor... (its own window, on a thread of its
    /// own; one already open comes to the front); false for another id.
    pub(super) fn list_editor_command(&self, id: u16) -> bool {
        if id != crate::listedit::cmd::FILE_LIST_EDITOR {
            return false;
        }
        self.open_list_editor(None);
        true
    }

    /// A data tool's WM_APP message.
    pub(super) fn data_tools_message(&self, msg: u32, _w: WPARAM, l: LPARAM) {
        if msg == WM_FILELIST {
            // Only the shown list's end shows now (W5R-5); a held list's is
            // said when its tab shows.
            if self.data_tools.read_ended(l as u64) == Reader::Shown {
                self.file_list_read();
            }
        } else if msg == WM_OTHERS_ENDED {
            let replace = self.data_tools.replacing.take();
            if let Some(replace) = replace {
                self.replace_settings(replace);
            }
        }
    }

    /// Whether a dialog may open now: none of goz-gui's is up.
    fn data_dialog_free(&self) -> bool {
        !self.data_tools.dialog.get() && self.may_open_dialog()
    }

    /// File > Open File List... (Ctrl+O): asks for a list, then searches it.
    fn open_file_list_dialog(&self) {
        if !self.data_dialog_free() {
            return;
        }
        self.data_tools.dialog.set(true);
        let path = {
            let _quiet = Quiet::begin();
            crate::filedlg::open(self.hwnd, "Open File List", LIST_KINDS, "efu")
        };
        self.data_tools.dialog.set(false);
        if let Some(path) = path {
            self.open_file_list(&path.display().to_string());
        }
    }

    /// Opens the list at `path` (`-filelist`, a recent list, the dialog's
    /// pick) and searches it from now on, as a new search; false (said)
    /// when it cannot be opened.
    pub(super) fn open_file_list(&self, path: &str) -> bool {
        if !self.set_file_list(path) {
            return false;
        }
        self.start_search(false);
        self.invalidate_status();
        true
    }

    /// Opens the list at `path` for the shown tab's searches to come (and
    /// in Recent File Lists); false (said) when it cannot be opened.
    pub(super) fn set_file_list(&self, path: &str) -> bool {
        let Some(list) = self.open_list(path, true) else {
            return false;
        };
        let full = list.path().display().to_string();
        let old = self.data_tools.list.replace(Some(list));
        if let Some(old) = old {
            self.release_list(old);
        }
        self.list_changed();
        self.prefs.borrow_mut().base.file_lists.opened(&full);
        self.settings_changed();
        true
    }

    /// File > Close File List (or a double-click on its name in the status
    /// bar): the shown tab searches gozd's index again.
    pub(super) fn close_file_list(&self) {
        let old = self.data_tools.list.borrow_mut().take();
        let Some(old) = old else { return };
        self.release_list(old);
        self.list_changed();
        self.start_search(false);
    }

    /// The shown list was read to its end, or reading stopped on an error:
    /// the results show all of it now, and the status bar says why a list
    /// stopped short.
    fn file_list_read(&self) {
        let Some(list) = self.filelist() else { return };
        if !list.loaded() {
            return;
        }
        self.say_how_read(&list);
        self.refresh_created_column();
        self.start_search(true);
        self.invalidate_status();
    }

    /// Says in the status bar why `list` (read) stopped short, or how many
    /// rows it left out.
    fn say_how_read(&self, list: &FileList) {
        if let Some(why) = list.error() {
            self.show_note(format!("{} could not be read whole: {why}", list.name()));
        } else if list.skipped() > 0 {
            self.show_note(format!(
                "{}: {} rows without a path were left out",
                list.name(),
                crate::format::grouped(list.skipped())
            ));
        }
    }

    /// Help > Troubleshooting Information.
    fn troubleshooting(&self) {
        if !self.data_dialog_free() {
            return;
        }
        let config = {
            let now = self.prefs.borrow().base.to_pairs();
            let defaults = crate::settings::Settings::default().to_pairs();
            now.into_iter()
                .filter(|(k, v)| !defaults.iter().any(|(dk, dv)| dk == k && dv == v))
                .collect()
        };
        let file_list = self.filelist().map(|l| {
            let state = if l.loaded() { "" } else { ", still being read" };
            format!(
                "{}, {} rows{state}",
                l.path().display(),
                crate::format::grouped(l.rows() as u64)
            )
        });
        let look = self.dialog_look();
        let facts = Facts {
            gozd: crate::client::server(),
            boolean_syntax: crate::client::boolean_syntax(),
            command_line: std::env::args().skip(1).collect::<Vec<_>>().join(" "),
            exe: std::env::current_exe()
                .map(|p| p.display().to_string())
                .unwrap_or_default(),
            data_dir: location::data_dir().map(|d| d.display().to_string()),
            data_source: location::data_source(),
            instance_key: self.instance_key.borrow().clone(),
            instance: self.instance.borrow().clone(),
            dpi: self.dpi.get(),
            zoom: self.zoom.get(),
            dark: look.dark,
            theme_setting: match self.theme_mode() {
                crate::theme::ThemeMode::UserDefault => "User Default",
                crate::theme::ThemeMode::Standard => "Standard",
                crate::theme::ThemeMode::Dark => "Dark",
            },
            file_list,
            config,
        };
        let text = troubleshoot::text(&facts, &troubleshoot::os_version(), troubleshoot::memory());
        self.data_tools.dialog.set(true);
        {
            let _quiet = Quiet::begin();
            troubleshoot::show(self.hwnd, &text, look);
        }
        self.data_tools.dialog.set(false);
    }

    /// Asks a yes or no question (yes without asking under
    /// `GOZ_GUI_SETTINGS_ACTION`, a test's).
    fn confirm(&self, text: &str, title: &str, test: bool) -> bool {
        test || msgbox::show(self.hwnd, text, title, MB_YESNO | MB_ICONQUESTION) == IDYES
    }

    /// Tools > Import and Export Settings...: Everything's wizard (its
    /// first page; `GOZ_GUI_SETTINGS_ACTION` names the action instead),
    /// then the folder (`GOZ_GUI_PICK_FOLDER` for tests).
    fn import_export_settings(&self) {
        // Not while an Import or Reset waits for the other windows.
        if !self.data_dialog_free() || self.data_tools.replacing.borrow().is_some() {
            return;
        }
        let Some(data) = location::data_dir().map(Path::to_path_buf) else {
            return;
        };
        let test = Action::from_test();
        self.data_tools.dialog.set(true);
        let action = match test {
            Some(a) => Some(a),
            None => {
                let _quiet = Quiet::begin();
                settings_io::ask(self.hwnd, self.dialog_look())
            }
        };
        let folder = match action {
            Some(Action::Export | Action::Import) => {
                let (title, ok) = if action == Some(Action::Export) {
                    ("Export Settings To", "Export")
                } else {
                    ("Import Settings From", "Import")
                };
                let _quiet = Quiet::begin();
                crate::filetools::folder_pick::pick(self.hwnd, title, ok, None)
                    .map(|f| PathBuf::from(String::from_utf16_lossy(&f)))
            }
            _ => None,
        };
        self.data_tools.dialog.set(false);
        let test = test.is_some();
        match (action, folder) {
            (Some(Action::Export), Some(folder)) => self.export_settings(&data, &folder, test),
            (Some(Action::Import), Some(folder)) => self.import_settings(&data, &folder, test),
            (Some(Action::Reset), _) => self.reset_settings(&data, test),
            _ => {}
        }
    }

    fn export_settings(&self, data: &Path, folder: &Path, test: bool) {
        let there = settings_io::present_in(folder);
        if !there.is_empty() {
            let ask = format!(
                "{} already holds goz-gui's files ({}).\n\nReplace them?",
                folder.display(),
                there.join(", ")
            );
            if !self.confirm(&ask, "Export Settings", test) {
                return;
            }
        }
        // What the window holds now goes to its files first.
        self.save_everything(false);
        self.writer.flush(crate::store::file::WRITE_WAIT);
        let (text, icon) = match settings_io::export(data, folder) {
            Ok(names) => (
                format!(
                    "{} files were exported to {}:\n\n{}",
                    names.len(),
                    folder.display(),
                    names.join("\n")
                ),
                MB_ICONINFORMATION,
            ),
            Err(e) => (
                format!(
                    "The settings could not be exported to {}:\n\n{e}",
                    folder.display()
                ),
                MB_ICONWARNING,
            ),
        };
        if !test {
            msgbox::show(self.hwnd, &text, "Export Settings", MB_OK | icon);
        } else {
            self.show_note(text.replace('\n', " "));
        }
    }

    fn import_settings(&self, data: &Path, folder: &Path, test: bool) {
        let there = settings_io::present_in(folder);
        if there.is_empty() {
            if !test {
                msgbox::show(
                    self.hwnd,
                    &format!("{} holds none of goz-gui's files.", folder.display()),
                    "Import Settings",
                    MB_OK | MB_ICONWARNING,
                );
            }
            return;
        }
        let ask = format!(
            "Import {} from {}?\n\nThe files they replace are kept beside them \
             (goz-gui.before-import.ini, ...), and goz-gui restarts to use them.",
            there.join(", "),
            folder.display()
        );
        if !self.confirm(&ask, "Import Settings", test) {
            return;
        }
        // Nothing this window holds may be written over the new files.
        self.writer.flush(crate::store::file::WRITE_WAIT);
        self.writer.close();
        self.after_other_windows(Replace::Import {
            data: data.to_path_buf(),
            folder: folder.to_path_buf(),
        });
    }

    fn reset_settings(&self, data: &Path, test: bool) {
        let ask = "Reset goz-gui's settings and session to their defaults?\n\n\
                   The search history, run history, filters and bookmarks are \
                   kept; the old settings are kept as goz-gui.before-reset.ini. \
                   goz-gui restarts.";
        if !self.confirm(ask, "Reset Settings", test) {
            return;
        }
        self.writer.flush(crate::store::file::WRITE_WAIT);
        self.writer.close();
        self.after_other_windows(Replace::Reset {
            data: data.to_path_buf(),
        });
    }

    /// Asks the other windows of this goz-gui to end writing nothing (each
    /// is a process of its own, which would write its settings and session
    /// as it ends), then does `replace` once they have ended: at once when
    /// there are none, else when a thread that waits for them (at most
    /// [`OTHERS_WAIT_MS`]) says so. The window answers meanwhile.
    fn after_other_windows(&self, replace: Replace) {
        use windows_sys::Win32::Foundation::{CloseHandle, HWND};
        use windows_sys::Win32::System::Threading::{
            OpenProcess, PROCESS_SYNCHRONIZE, WaitForSingleObject,
        };
        use windows_sys::Win32::UI::WindowsAndMessaging::PostMessageW;
        let pids = self.exit_other_windows_unsaved();
        // Opened now, so a process id used again later is not waited for.
        // SAFETY: opens each process for waiting only; a process gone
        // already gives no handle.
        let handles: Vec<usize> = pids
            .iter()
            .map(|&pid| unsafe { OpenProcess(PROCESS_SYNCHRONIZE, 0, pid) } as usize)
            .filter(|&h| h != 0)
            .collect();
        if handles.is_empty() {
            self.replace_settings(replace);
            return;
        }
        *self.data_tools.replacing.borrow_mut() = Some(replace);
        self.show_note("Waiting for the other goz-gui windows to close...".into());
        let hwnd = self.hwnd as usize;
        let wait = move || {
            let end = std::time::Instant::now()
                + std::time::Duration::from_millis(u64::from(OTHERS_WAIT_MS));
            for &h in &handles {
                let left = end.saturating_duration_since(std::time::Instant::now());
                // SAFETY: waits on and closes the handles opened above,
                // each once.
                unsafe {
                    WaitForSingleObject(h as _, left.as_millis() as u32);
                    CloseHandle(h as _);
                }
            }
            // SAFETY: posts a message with no pointers; a window gone
            // meanwhile is reported and ignored.
            unsafe { PostMessageW(hwnd as HWND, WM_OTHERS_ENDED, 0, 0) };
        };
        if let Err(e) = std::thread::Builder::new()
            .name("goz-wait-windows".into())
            .spawn(wait)
        {
            // No thread (the handles stay open until the restart ends
            // this process): replace now.
            self.show_note(format!("Could not wait for the other windows: {e}"));
            let replace = self.data_tools.replacing.take();
            if let Some(replace) = replace {
                self.replace_settings(replace);
            }
        }
    }

    /// Replaces the settings as Import or Reset says, then restarts.
    fn replace_settings(&self, replace: Replace) {
        match replace {
            Replace::Import { data, folder } => {
                if let Err(e) = settings_io::import(&folder, &data) {
                    // The window keeps its own state then, written as
                    // before would be lost: say so and restart all the
                    // same, from what the folder holds now.
                    msgbox::show(
                        self.hwnd,
                        &format!("The settings could not all be imported:\n\n{e}"),
                        "Import Settings",
                        MB_OK | MB_ICONWARNING,
                    );
                }
            }
            Replace::Reset { data } => {
                if let Err(e) = settings_io::reset(&data) {
                    msgbox::show(
                        self.hwnd,
                        &format!("The settings could not be reset:\n\n{e}"),
                        "Reset Settings",
                        MB_OK | MB_ICONWARNING,
                    );
                }
            }
        }
        self.restart();
    }

    /// Starts goz-gui again (once this process has ended: `-wait-pid`) and
    /// closes every window of this one, which writes nothing more (the
    /// others ended before the files were replaced; one started since ends
    /// unsaved too).
    fn restart(&self) {
        if let Ok(exe) = std::env::current_exe() {
            let mut command = std::process::Command::new(exe);
            command.args(["-wait-pid", &std::process::id().to_string()]);
            if let Some(name) = self.instance.borrow().as_deref() {
                command.args(["-instance", name]);
            }
            let _ = command.spawn();
        }
        self.exit_other_windows_unsaved();
        // SAFETY: destroys this thread's window; the process ends with it.
        unsafe { DestroyWindow(self.hwnd) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn w(s: &str) -> Vec<u16> {
        s.encode_utf16().collect()
    }

    #[test]
    fn a_drop_of_efu_files_only_opens_the_first() {
        let no_dirs = |_: &[u16]| false;
        assert_eq!(
            efu_drop(&[w(r"C:\a\x.EFU"), w(r"C:\b.efu")], no_dirs),
            Some(r"C:\a\x.EFU".into())
        );
        // Anything else in the drop: the paths are searched for, as before.
        assert_eq!(efu_drop(&[w(r"C:\a.efu"), w(r"C:\b.txt")], no_dirs), None);
        assert_eq!(efu_drop(&[w(r"C:\a.txt")], no_dirs), None);
        assert_eq!(efu_drop(&[], no_dirs), None);
        // A folder named so is no list.
        let dirs = |p: &[u16]| String::from_utf16_lossy(p).ends_with("dir.efu");
        assert_eq!(efu_drop(&[w(r"C:\dir.efu")], dirs), None);
    }

    #[test]
    fn a_held_lists_end_of_reading_waits_for_its_tab() {
        // W5R-5: the shown tab's list Y; another tab's X, held.
        let d = DataTools::new();
        let y = FileList::from_rows(&[(r"C:\a.txt", None, None, false)]);
        let x = FileList::from_rows(&[(r"C:\b.txt", None, None, false)]);
        let gone = FileList::from_rows(&[(r"C:\c.txt", None, None, false)]);
        assert!(x.id() != y.id() && gone.id() != x.id() && gone.id() != y.id());
        *d.list.borrow_mut() = Some(Arc::clone(&y));
        d.held.borrow_mut().push(Arc::clone(&x));
        // X's message is X's, not Y's: Y's note and refresh do not come
        // again; X's end is kept for its tab.
        assert_eq!(d.read_ended(x.id()), Reader::Held);
        assert_eq!(d.read_ended(y.id()), Reader::Shown);
        assert_eq!(d.read_ended(gone.id()), Reader::Gone);
        assert!(!d.take_unsaid(&y));
        // X's tab shows: said once.
        assert!(d.take_unsaid(&x));
        assert!(!d.take_unsaid(&x));
        // A list let go before its tab showed is not kept.
        assert_eq!(d.read_ended(x.id()), Reader::Held);
        d.held.borrow_mut().clear();
        d.forget_unsaid_of_gone();
        assert!(d.unsaid.borrow().is_empty());
    }

    #[test]
    fn a_shown_entry_or_tab_ends_the_date_created_sort() {
        // W5R-3: Back, a bookmark or a tab switch shows its own sort.
        let d = DataTools::new();
        d.created_sort.set(Some(SortDir::Desc));
        d.state_shown();
        assert_eq!(d.created_sort.get(), None);
    }

    #[test]
    fn a_list_is_known_by_its_path_in_any_case() {
        let dir = crate::store::file::tests::fresh_dir("goz-gui-islist", "");
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("List.efu");
        std::fs::write(&file, "Filename\r\nC:\\a.txt\r\n").unwrap();
        let list = FileList::open(&file, None).unwrap();
        let path = file.display().to_string();
        assert!(is_list(&list, &path));
        assert!(is_list(&list, &path.to_uppercase()));
        assert!(!is_list(
            &list,
            &dir.join("other.efu").display().to_string()
        ));
        list.close();
        while !list.loaded() {
            std::thread::yield_now();
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}
