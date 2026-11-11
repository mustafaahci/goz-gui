//! Wave 4's data and file tools from the window's side (G1; the work is
//! `crate::filelist` and `crate::datatools`): File > Open File List...
//! (Ctrl+O), Recent File Lists > and Close File List; the open list's
//! name in the status bar and the title; Tools > Import and Export
//! Settings...; Help > Troubleshooting Information. (Edit > Advanced >
//! Advanced Copy/Move to Folder... run through the file tools,
//! `file_tools.rs`.)
//!
//! An open file list is the window's, for every tab: each search the
//! window makes carries it (`client::Search::filelist`), and the search
//! worker, Export and the find bar answer from its rows instead of
//! gozd's. No `RefCell` of the window is borrowed while a dialog runs.

use std::cell::{Cell, RefCell};
use std::path::{Path, PathBuf};
use std::sync::Arc;

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
/// reading stopped on an error.
pub(super) const WM_FILELIST: u32 = MSG_FIRST;

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
            match crate::filelist::create::create(Path::new(file), folders, l.create_options) {
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
    list: RefCell<Option<Arc<FileList>>>,
    /// One of their dialogs is open.
    dialog: Cell<bool>,
}

impl DataTools {
    pub(super) fn new() -> DataTools {
        DataTools {
            list: RefCell::new(None),
            dialog: Cell::new(false),
        }
    }
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
            // Everything's access key, S, is Start goz Service's here.
            &[Item::Cmd(
                cmd::IMPORT_EXPORT_SETTINGS,
                "&Import and Export Settings...",
            )],
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

    /// A data tool's WM_APP message.
    pub(super) fn data_tools_message(&self, msg: u32, w: WPARAM, _l: LPARAM) {
        if msg == WM_FILELIST {
            self.file_list_read(w != 0);
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

    /// Opens the list at `path` for the searches to come (and in Recent
    /// File Lists); false (said) when it cannot be opened.
    pub(super) fn set_file_list(&self, path: &str) -> bool {
        let full = std::path::absolute(Path::new(path)).unwrap_or_else(|_| PathBuf::from(path));
        let notify = Notify {
            hwnd: self.hwnd as usize,
            msg: WM_FILELIST,
        };
        let list = match FileList::open(&full, Some(notify)) {
            Ok(list) => list,
            Err(e) => {
                msgbox::show(
                    self.hwnd,
                    &format!(
                        "The file list could not be opened:\n\n{}\n\n{e}",
                        full.display()
                    ),
                    "Open File List",
                    MB_OK | MB_ICONWARNING,
                );
                return false;
            }
        };
        let old = self.data_tools.list.replace(Some(list));
        if let Some(old) = old {
            old.close();
        }
        self.prefs
            .borrow_mut()
            .base
            .file_lists
            .opened(&full.display().to_string());
        self.settings_changed();
        true
    }

    /// File > Close File List (or a double-click on its name in the status
    /// bar): gozd's index is searched again.
    pub(super) fn close_file_list(&self) {
        let old = self.data_tools.list.borrow_mut().take();
        let Some(old) = old else { return };
        old.close();
        self.start_search(false);
        self.invalidate_status();
    }

    /// The open list was read to its end (`ok`), or reading stopped on an
    /// error: the results show all of it now, and the status bar says why
    /// a list stopped short.
    fn file_list_read(&self, ok: bool) {
        let Some(list) = self.filelist() else { return };
        if !list.loaded() {
            // A list closed and opened again since: its own message comes.
            return;
        }
        if !ok && let Some(why) = list.error() {
            self.show_note(format!("{} could not be read whole: {why}", list.name()));
        } else if list.skipped() > 0 {
            self.show_note(format!(
                "{}: {} rows without a path were left out",
                list.name(),
                crate::format::grouped(list.skipped())
            ));
        }
        self.start_search(true);
        self.invalidate_status();
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
        if !self.data_dialog_free() {
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
        match settings_io::import(folder, data) {
            Ok(_) => self.restart(),
            Err(e) => {
                // The window keeps its own state then, written as before
                // would be lost: say so and restart all the same, from what
                // the folder holds now.
                msgbox::show(
                    self.hwnd,
                    &format!("The settings could not all be imported:\n\n{e}"),
                    "Import Settings",
                    MB_OK | MB_ICONWARNING,
                );
                self.restart();
            }
        }
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
        if let Err(e) = settings_io::reset(data) {
            msgbox::show(
                self.hwnd,
                &format!("The settings could not be reset:\n\n{e}"),
                "Reset Settings",
                MB_OK | MB_ICONWARNING,
            );
        }
        self.restart();
    }

    /// Starts goz-gui again (once this process has ended: `-wait-pid`) and
    /// closes every window of this one, which writes nothing more.
    fn restart(&self) {
        if let Ok(exe) = std::env::current_exe() {
            let mut command = std::process::Command::new(exe);
            command.args(["-wait-pid", &std::process::id().to_string()]);
            if let Some(name) = self.instance.borrow().as_deref() {
                command.args(["-instance", name]);
            }
            let _ = command.spawn();
        }
        self.exit_other_windows();
        // SAFETY: destroys this thread's window; the process ends with it.
        unsafe { DestroyWindow(self.hwnd) };
    }
}
