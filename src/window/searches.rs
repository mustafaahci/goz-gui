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
//! - **Writing**: a change (a search recorded, deleted or imported) is
//!   written by the store thread about a second later, and at exit. The
//!   write reads the file again and merges (`history::merge`), so another
//!   goz-gui's searches are kept; what it wrote back is then merged into
//!   this window's history too.

use std::cell::{Cell, RefCell};
use std::sync::Arc;

use windows_sys::Win32::Foundation::HWND;
use windows_sys::Win32::System::Diagnostics::Debug::MessageBeep;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    KillTimer, MB_ICONWARNING, PostMessageW, SetTimer,
};

use super::{App, TIMER_HISTORY, WM_HISTORY, prefs};
use crate::dialog::Quiet;
use crate::history::{self, Forgotten, Keep, SearchHistory, Searched, filetime_now};
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
    suggest: bool,
    keep: Keep,
    /// `None` without a data folder, or with the history off.
    file: Option<Arc<HistoryFile>>,
    /// Changed since it was last handed to the store thread.
    dirty: Cell<bool>,
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
            suggest: h.search_enabled && h.search_always_suggest,
            keep,
            file,
            dirty: Cell::new(false),
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
    /// Whether searches are recorded and listed at all.
    pub fn search_history_enabled(&self) -> bool {
        self.searches.enabled
    }

    /// The search history changed here: written once it stops changing.
    pub(super) fn history_changed(&self) {
        if self.searches.file.is_none() {
            return;
        }
        self.searches.dirty.set(true);
        // SAFETY: a timer of this window; setting it again restarts it.
        unsafe { SetTimer(self.hwnd, TIMER_HISTORY, SAVE_DELAY_MS, None) };
    }

    /// Hands the history to the store thread if it changed since the last
    /// time.
    pub(super) fn save_history(&self) {
        // SAFETY: a timer of this window.
        unsafe { KillTimer(self.hwnd, TIMER_HISTORY) };
        let dirty = self.searches.dirty.replace(false);
        let Some(file) = self.searches.file.clone() else {
            return;
        };
        if dirty {
            file.stage(self.search_history.borrow().entries().to_vec());
        } else if !file.pending() {
            // Nothing new, and no failed write waiting to go out again.
            return;
        }
        let hwnd = self.hwnd as usize;
        let job = Arc::clone(&file);
        self.writer.submit(
            file.path(),
            Box::new(move || match job.write_now() {
                Ok(Some(written)) => post(hwnd, WM_HISTORY, written),
                Ok(None) => {}
                Err(why) => prefs::report(hwnd, why, true),
            }),
        );
    }

    /// The store thread wrote the history: the searches another goz-gui
    /// added are taken in (not while a dialog or the dropdown shows the
    /// history: they come with the next write).
    pub(super) fn history_written(&self, written: Written) {
        if let Some(note) = written.note {
            self.show_note(note);
        }
        let Some(file) = &self.searches.file else {
            return;
        };
        if self.searches.dialog.get() || self.dropdown.is_open() {
            return;
        }
        let forgotten = file.forgotten();
        let mut h = self.search_history.borrow_mut();
        let mut merged = history::merge(h.entries(), &written.entries, &forgotten);
        history::prune(&mut merged, self.searches.keep, filetime_now());
        if merged != h.entries() {
            h.replace(merged);
        }
    }

    /// Notes the search in the box as made (once per change of its text).
    pub(super) fn record_in_history(&self, text: &str) {
        if !self.searches.enabled {
            return;
        }
        let recorded = self
            .search_history
            .borrow_mut()
            .record(text, filetime_now());
        if recorded {
            self.history_changed();
        }
    }

    /// Deletes `searches` from the history; the file loses them at the
    /// next write.
    pub fn forget_searches(&self, searches: &[String]) {
        let now = filetime_now();
        // Each deletion is dated no earlier than the search it deleted, so
        // one dated in the future (a clock that was ahead) goes too.
        let dated: Vec<(&String, u64)> = {
            let mut h = self.search_history.borrow_mut();
            searches
                .iter()
                .map(|s| {
                    (
                        s,
                        history::deleted_at(h.remove_search(s).map(|e| e.last), now),
                    )
                })
                .collect()
        };
        if let Some(file) = &self.searches.file {
            for (s, at) in dated {
                file.forget(s, at);
            }
        }
        self.history_changed();
    }

    fn forget_all_searches(&self) {
        let now = filetime_now();
        let at = {
            let mut h = self.search_history.borrow_mut();
            let newest = h.entries().iter().map(|e| e.last).max();
            h.replace(Vec::new());
            history::deleted_at(newest, now)
        };
        if let Some(file) = &self.searches.file {
            file.forget_all(at);
        }
        self.history_changed();
    }

    fn import_searches(&self, entries: Vec<Searched>) {
        {
            let mut h = self.search_history.borrow_mut();
            // Imported on purpose: a search deleted earlier comes back.
            let mut merged = history::merge(h.entries(), &entries, &Forgotten::default());
            history::prune(&mut merged, self.searches.keep, filetime_now());
            h.replace(merged);
        }
        self.history_changed();
    }

    /// While a search is typed: the dropdown lists the searches containing
    /// it (`search_history_always_suggest`), without taking the keyboard;
    /// it closes when none does.
    pub(super) fn suggest(&self) {
        if !self.searches.suggest {
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
