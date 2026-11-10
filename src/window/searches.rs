//! The search history kept across runs (`Search History.csv`, see
//! `crate::history_file`), the settings it follows, and History > Find
//! Search History... and Organize Search History... (`crate::history_dialogs`).
//!
//! - **Settings** (Everything's keys, read at startup): with
//!   `search_history_enabled=0` nothing is recorded, the file is neither
//!   read nor written, and the dropdown button, the two History items and
//!   their shortcuts are gone. `search_history_keep_forever=0` keeps
//!   `search_history_days_to_keep` days: older searches are left out when
//!   the file is read and when it is written. `search_history_always_suggest=1`
//!   opens the dropdown with the searches that contain what is typed.
//! - **Writing**: each change (a search recorded, deleted, Delete All, an
//!   import) is noted (`HistoryFile::log`) and written by the store thread
//!   about a second later, and at exit. The write reads the file again and
//!   makes those changes on it (`history::apply`), so another goz-gui's
//!   searches are kept and its deletions stick; this window then takes the
//!   file as written for its own history.

use std::cell::{Cell, RefCell};
use std::sync::Arc;

use windows_sys::Win32::Foundation::HWND;
use windows_sys::Win32::System::Diagnostics::Debug::MessageBeep;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    KillTimer, MB_ICONWARNING, PostMessageW, SetTimer,
};

use super::runs::give_back_memory;
use super::{App, TIMER_HISTORY, WM_HISTORY, prefs};
use crate::dialog::Quiet;
use crate::history::{self, Keep, Op, SearchHistory, Searched, filetime_now};
use crate::history_dialogs::{self, Host, Mode};
use crate::history_file::{self, FILE_NAME, HistoryFile, Written};
use crate::settings::Settings;
use crate::store::location;
use crate::wide::window_text;
use crate::{clipboard, dropdown};

/// How long after the last change the history is written.
const SAVE_DELAY_MS: u32 = 1000;
/// The most suggestions the dropdown lists while a search is typed
/// (Everything's `search_history_suggestion_max`).
const SUGGESTIONS: usize = 256;

/// The search history's settings and file.
pub(super) struct Searches {
    /// `search_history_enabled`.
    enabled: bool,
    /// `search_history_always_suggest` (and enabled).
    suggest: Cell<bool>,
    keep: Keep,
    /// `None` without a data folder, or with the history off.
    file: Option<Arc<HistoryFile>>,
    /// Find or Organize is open.
    dialog: Cell<bool>,
    /// What to tell the user once the window is up: the file could not be
    /// read.
    pub note: RefCell<Option<String>>,
}

impl Searches {
    /// Reads the search history the settings ask for (before the window
    /// exists: the file is small).
    pub(super) fn load(settings: &Settings) -> (Searches, SearchHistory) {
        let h = settings.history;
        let keep = (!h.search_keep_forever).then_some(h.search_days_to_keep);
        let mut entries = Vec::new();
        let mut note = None;
        let file = if h.search_enabled {
            location::data_file(FILE_NAME).map(|path| {
                (entries, note) = history_file::load(&path, keep, filetime_now());
                Arc::new(HistoryFile::new(path, keep, settings.csv_backup))
            })
        } else {
            None
        };
        let searches = Searches {
            enabled: h.search_enabled,
            suggest: Cell::new(h.search_enabled && h.search_always_suggest),
            keep,
            file,
            dialog: Cell::new(false),
            note: RefCell::new(note),
        };
        (searches, SearchHistory::from_entries(entries))
    }
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
    /// `search_history_always_suggest`, from Tools > Options.
    pub(super) fn set_suggest(&self, on: bool) {
        self.searches.suggest.set(self.searches.enabled && on);
    }

    /// Whether searches are recorded and listed at all.
    pub fn search_history_enabled(&self) -> bool {
        self.searches.enabled
    }

    /// The search history changed here (`ops`, made on it already): the
    /// file gets the changes once they stop coming.
    fn history_changed(&self, ops: Vec<Op<Searched>>) {
        let Some(file) = &self.searches.file else {
            return;
        };
        for op in ops {
            file.log(op);
        }
        // SAFETY: a timer of this window; setting it again restarts it.
        unsafe { SetTimer(self.hwnd, TIMER_HISTORY, SAVE_DELAY_MS, None) };
    }

    /// Hands the changes made to the history to the store thread, if any
    /// are waiting (new ones, or those of a write that failed).
    pub(super) fn save_history(&self) {
        // SAFETY: a timer of this window.
        unsafe { KillTimer(self.hwnd, TIMER_HISTORY) };
        let Some(file) = self.searches.file.clone() else {
            return;
        };
        if !file.pending() {
            return;
        }
        let hwnd = self.hwnd as usize;
        let job = Arc::clone(&file);
        self.writer.submit(
            file.path(),
            Box::new(move || {
                match job.write_now() {
                    Ok(Some(written)) => post(hwnd, WM_HISTORY, written),
                    Ok(None) => {}
                    Err(why) => prefs::report(hwnd, why, true),
                }
                give_back_memory();
            }),
        );
    }

    /// The store thread wrote the history: this window's history becomes
    /// the file as written (with what another goz-gui added or deleted),
    /// and the changes made here since the write began (not while a dialog
    /// or the dropdown shows the history: that comes with the next write).
    pub(super) fn history_written(&self, written: Written) {
        if let Some(note) = written.note {
            self.show_note(note);
        }
        let Some(file) = &self.searches.file else {
            return;
        };
        let later = file.settle(written.through);
        if self.searches.dialog.get() || self.dropdown.is_open() {
            return;
        }
        let mut entries = written.entries;
        history::apply(&mut entries, &later);
        history::prune(&mut entries, self.searches.keep, filetime_now());
        {
            let mut h = self.search_history.borrow_mut();
            if entries != h.entries() {
                h.replace(entries);
            }
        }
        // The list it replaced, or the one written (the same), is freed.
        give_back_memory();
    }

    /// Notes the search in the box as made (once per change of its text).
    pub(super) fn record_in_history(&self, text: &str) {
        if !self.searches.enabled {
            return;
        }
        let made = self
            .search_history
            .borrow_mut()
            .record(text, filetime_now());
        if let Some(made) = made {
            self.history_changed(vec![Op::Put(made)]);
        }
    }

    /// Deletes `searches` from the history; the file loses them at the
    /// next write.
    pub fn forget_searches(&self, searches: &[String]) {
        let now = filetime_now();
        // Each deletion is dated no earlier than the search it deleted, so
        // one dated in the future (a clock that was ahead) goes too.
        let ops = {
            let mut h = self.search_history.borrow_mut();
            searches
                .iter()
                .map(|s| Op::Delete {
                    key: s.clone(),
                    at: history::deleted_at(h.remove_search(s).map(|e| e.last), now),
                })
                .collect()
        };
        self.history_changed(ops);
    }

    fn forget_all_searches(&self) {
        let now = filetime_now();
        let at = {
            let mut h = self.search_history.borrow_mut();
            let newest = h.entries().iter().map(|e| e.last).max();
            h.replace(Vec::new());
            history::deleted_at(newest, now)
        };
        self.history_changed(vec![Op::Clear { at }]);
    }

    /// Merges `entries` in: each search takes the higher count and the
    /// later date. Imported on purpose: a search deleted earlier comes back.
    fn import_searches(&self, entries: Vec<Searched>) {
        let mut entries = history::tidy(entries);
        history::prune(&mut entries, self.searches.keep, filetime_now());
        let ops: Vec<Op<Searched>> = entries.into_iter().map(Op::Put).collect();
        self.search_history.borrow_mut().apply(&ops);
        self.history_changed(ops);
    }

    /// While a search is typed: the dropdown lists the searches containing
    /// it (`search_history_always_suggest`, and always while search as you
    /// type is off, as in Everything: nothing else answers the typing),
    /// without taking the keyboard; it closes when none does.
    pub(super) fn suggest(&self) {
        let always = self.searches.enabled && !self.ui().search_as_you_type;
        if !self.searches.suggest.get() && !always {
            dropdown::close(self);
            return;
        }
        let text = window_text(self.edit);
        let list = self.search_history.borrow().suggestions(&text, SUGGESTIONS);
        dropdown::suggest(self, list);
    }

    /// Find or Organize Search History is open.
    pub(super) fn search_history_dialog_open(&self) -> bool {
        self.searches.dialog.get()
    }

    /// History > Find Search History... (Ctrl+H).
    pub(super) fn find_search_history(&self) {
        self.search_history_dialog(Mode::Find);
    }

    /// History > Organize Search History... (Ctrl+Shift+H).
    pub(super) fn organize_search_history(&self) {
        self.search_history_dialog(Mode::Organize);
    }

    fn search_history_dialog(&self, mode: Mode) {
        if !self.searches.enabled
            || self.bookmark_dialog.get()
            || self.run_history_dialog_open()
            || self.searches.dialog.replace(true)
        {
            return;
        }
        dropdown::close(self);
        // Opened by a command posted while goz-gui is in the background:
        // shown, but not brought to the front.
        let _quiet = Quiet::begin();
        let chosen = history_dialogs::run(self.hwnd, mode, self, self.dialog_look());
        self.searches.dialog.set(false);
        give_back_memory();
        if let Some(search) = chosen {
            self.set_search_text(&search);
        }
    }
}

impl Host for App {
    fn searches(&self) -> Vec<Searched> {
        self.search_history.borrow().entries().to_vec()
    }

    fn forget(&self, searches: &[String]) {
        self.forget_searches(searches);
    }

    fn forget_all(&self) {
        self.forget_all_searches();
    }

    fn import(&self, entries: Vec<Searched>) {
        self.import_searches(entries);
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
}
