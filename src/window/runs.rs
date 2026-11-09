//! The run history kept across runs (`Run History.csv`, see
//! `crate::run_history_file`), what counts as a run, Tools > Run History...
//! (`crate::run_dialog`), and what the first Enter in the search box selects
//! (`search_edit_enter_action`).
//!
//! - **A run**: every Open of results (Enter, a double-click, File > Open,
//!   the row menu's Open, the Run History dialog's Open) counts one run of
//!   each item opened and dates it, when the open is handed to the shell (a
//!   test's dry run counts too). Open Path counts only with
//!   `run_history_increase_run_count_on_open_path=1`.
//! - **Settings** (Everything's keys, read at startup): with
//!   `run_history_enabled=0` nothing is counted, the file is neither read
//!   nor written, and Tools > Run History... and Ctrl+J are gone;
//!   `run_history_keep_forever=0` keeps `run_history_days_to_keep` days.
//! - **Writing**: each change (a run, a removal, Remove All, Set Run Count,
//!   an import) is noted (`RunFile::log`) and written by the store thread
//!   about a second later, and at exit. The write reads the file again and
//!   makes those changes on it (`history::apply`), so another goz-gui's
//!   runs are kept and its removals stick; this window then takes the file
//!   as written for its own history.
//! - **Enter**: the first Enter after the search changed selects and
//!   focuses the held result run most (Everything's default), or as
//!   `search_edit_enter_action` says; with none of them ever run, the top
//!   one, as before. The next Enter opens it.

use std::cell::{Cell, RefCell};
use std::sync::Arc;

use windows_sys::Win32::Foundation::HWND;
use windows_sys::Win32::System::Memory::{HeapOptimizeResources, HeapSetInformation};
use windows_sys::Win32::UI::Controls::{
    EM_SETSEL, LVIS_FOCUSED, LVIS_SELECTED, LVM_ENSUREVISIBLE, LVM_GETITEMCOUNT, LVM_GETNEXTITEM,
    LVM_SETSELECTIONMARK, LVNI_FOCUSED,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{KillTimer, PostMessageW, SetTimer};

use super::{App, TIMER_RUNS, WM_RUNS, prefs, send};
use crate::actions::{self, Target};
use crate::dialog::Quiet;
use crate::history::{self, Keep, Op, deleted_at, filetime_now};
use crate::run_dialog::{self, Host};
use crate::run_history::{self, Pick, Ran, RunHistory};
use crate::run_history_file::{self, FILE_NAME, RunFile, Written};
use crate::settings::{EnterAction, Settings};
use crate::store::location;

/// How long after the last change the run history is written.
const SAVE_DELAY_MS: u32 = 1000;

/// The run history, its settings and file.
pub(super) struct Runs {
    /// `run_history_enabled`.
    enabled: bool,
    /// `run_history_increase_run_count_on_open_path`.
    open_path: bool,
    /// `search_edit_enter_action`.
    enter: Cell<EnterAction>,
    keep: Keep,
    /// The runs, the last first.
    pub history: RefCell<RunHistory>,
    /// `None` without a data folder, or with the run history off.
    file: Option<Arc<RunFile>>,
    /// The Run History dialog is open.
    dialog: Cell<bool>,
    /// What to tell the user once the window is up: the file could not be
    /// read.
    pub note: RefCell<Option<String>>,
}

impl Runs {
    /// Reads the run history the settings ask for (before the window
    /// exists: the file is small).
    pub(super) fn load(settings: &Settings) -> Runs {
        let h = settings.history;
        let keep = (!h.run_keep_forever).then_some(h.run_days_to_keep);
        let mut entries = Vec::new();
        let mut note = None;
        let file = if h.run_enabled {
            location::data_file(FILE_NAME).map(|path| {
                (entries, note) = run_history_file::load(&path, keep, filetime_now());
                Arc::new(RunFile::new(path, keep, settings.csv_backup))
            })
        } else {
            None
        };
        // The last file read at startup: what reading them took is given back.
        give_back_memory();
        Runs {
            enabled: h.run_enabled,
            open_path: h.run_on_open_path,
            enter: Cell::new(settings.enter_action),
            keep,
            history: RefCell::new(RunHistory::from_entries(entries)),
            file,
            dialog: Cell::new(false),
            note: RefCell::new(note),
        }
    }
}

/// Gives the memory a burst of work freed (a history file read, parsed and
/// written; a dialog listing every entry) back to Windows. The process heap
/// keeps freed blocks committed: an idle goz-gui would otherwise keep the
/// largest burst it ever had (a write reads, parses and writes the whole
/// file, several times the history's own size).
pub(super) fn give_back_memory() {
    /// `HEAP_OPTIMIZE_RESOURCES_INFORMATION`.
    #[repr(C)]
    struct Optimize {
        version: u32,
        flags: u32,
    }
    let info = Optimize {
        version: 1,
        flags: 0,
    };
    // SAFETY: asks Windows to decommit what the process's heaps hold free
    // (any thread may; each heap is locked meanwhile). `info` is the
    // documented structure, version 1.
    unsafe {
        HeapSetInformation(
            core::ptr::null_mut(),
            HeapOptimizeResources,
            (&raw const info).cast(),
            size_of::<Optimize>(),
        )
    };
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
    /// The Run History dialog is open.
    pub(super) fn run_history_dialog_open(&self) -> bool {
        self.runs.dialog.get()
    }

    /// Counts one run of each of `paths` (full paths, as the list holds
    /// them), now. `reveal`: they were opened by Open Path, which counts
    /// only when `run_history_increase_run_count_on_open_path` says so.
    pub(super) fn record_runs<'a>(&self, paths: impl IntoIterator<Item = &'a [u16]>, reveal: bool) {
        if !self.runs.enabled || (reveal && !self.runs.open_path) {
            return;
        }
        // A path that is not valid UTF-16 cannot be written to the file
        // (UTF-8) and matched again: it is not counted.
        let files: Vec<String> = paths
            .into_iter()
            .filter_map(|p| String::from_utf16(p).ok())
            .collect();
        let made = self
            .runs
            .history
            .borrow_mut()
            .record(files.iter().map(String::as_str), filetime_now());
        if !made.is_empty() {
            self.runs_changed(made.into_iter().map(Op::Put).collect());
        }
    }

    /// The run history changed here (`ops`, made on it already): the file
    /// gets the changes once they stop coming.
    fn runs_changed(&self, ops: Vec<Op<Ran>>) {
        let Some(file) = &self.runs.file else {
            return;
        };
        for op in ops {
            file.log(op);
        }
        // SAFETY: a timer of this window; setting it again restarts it.
        unsafe { SetTimer(self.hwnd, TIMER_RUNS, SAVE_DELAY_MS, None) };
    }

    /// Hands the changes made to the run history to the store thread, if
    /// any are waiting (new ones, or those of a write that failed).
    pub(super) fn save_runs(&self) {
        // SAFETY: a timer of this window.
        unsafe { KillTimer(self.hwnd, TIMER_RUNS) };
        let Some(file) = self.runs.file.clone() else {
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
                    Ok(Some(written)) => post(hwnd, WM_RUNS, written),
                    Ok(None) => {}
                    Err(why) => prefs::report(hwnd, why, true),
                }
                give_back_memory();
            }),
        );
    }

    /// The store thread wrote the run history: this window's run history
    /// becomes the file as written (with what another goz-gui added or
    /// removed), and the changes made here since the write began (not
    /// while the dialog shows the history: that comes with the next write).
    pub(super) fn runs_written(&self, written: Written) {
        if let Some(note) = written.note {
            self.show_note(note);
        }
        let Some(file) = &self.runs.file else {
            return;
        };
        let later = file.settle(written.through);
        if self.runs.dialog.get() {
            return;
        }
        let mut entries = written.entries;
        history::apply(&mut entries, &later);
        history::prune(&mut entries, self.runs.keep, filetime_now());
        {
            let mut h = self.runs.history.borrow_mut();
            if entries != h.entries() {
                h.replace(entries);
            }
        }
        // The list it replaced, or the one written (the same), is freed.
        give_back_memory();
    }

    /// Tools > Run History... (Ctrl+J).
    pub(super) fn run_history_dialog(&self) {
        if !self.runs.enabled
            || self.bookmark_dialog.get()
            || self.search_history_dialog_open()
            || self.runs.dialog.replace(true)
        {
            return;
        }
        crate::dropdown::close(self);
        {
            // Opened by a command posted while goz-gui is in the
            // background: shown, but not brought to the front.
            let _quiet = Quiet::begin();
            run_dialog::run(self.hwnd, self, self.dialog_look());
        }
        self.runs.dialog.set(false);
        give_back_memory();
    }

    /// Enter in the search box, the first after the search changed: selects
    /// what `search_edit_enter_action` says, the keyboard staying in the
    /// box.
    pub(super) fn enter_select(&self) {
        let pick = match self.runs.enter.get() {
            EnterAction::None => return,
            EnterAction::SelectSearch => {
                send(self.edit, EM_SETSEL, 0, -1);
                return;
            }
            EnterAction::Top => return self.select_row(0),
            EnterAction::Result => {
                let focused = send(
                    self.list,
                    LVM_GETNEXTITEM,
                    usize::MAX,
                    LVNI_FOCUSED as isize,
                );
                return self.select_row(usize::try_from(focused).unwrap_or(0));
            }
            EnterAction::Bottom => {
                let count = send(self.list, LVM_GETITEMCOUNT, 0, 0);
                return self.select_row(usize::try_from(count - 1).unwrap_or(0));
            }
            EnterAction::MostRun => Pick::MostRun,
            EnterAction::LastRun => Pick::LastRun,
        };
        let row = self.held_row_run(pick).unwrap_or(0);
        self.select_row(row);
    }

    /// The held result `pick` chooses by the run history, if any of them
    /// was ever run.
    fn held_row_run(&self, pick: Pick) -> Option<usize> {
        if !self.runs.enabled {
            return None;
        }
        let history = self.runs.history.borrow();
        if history.is_empty() {
            return None;
        }
        let m = self.model.borrow();
        let ranges = m.loaded();
        let rows = ranges
            .into_iter()
            .flatten()
            .filter_map(|i| m.peek(i).map(|row| (i, &row.path[..])));
        run_history::best_row(&history, rows, pick)
    }

    /// Selects and focuses result `row` alone, in view (the keyboard stays
    /// where it is).
    pub(super) fn select_row(&self, row: usize) {
        let count = send(self.list, LVM_GETITEMCOUNT, 0, 0);
        if count <= 0 || row >= count as usize {
            return;
        }
        self.clear_selection();
        self.set_state(
            row,
            LVIS_SELECTED | LVIS_FOCUSED,
            LVIS_SELECTED | LVIS_FOCUSED,
        );
        send(self.list, LVM_SETSELECTIONMARK, 0, row as isize);
        send(self.list, LVM_ENSUREVISIBLE, row, 0);
    }

    fn forget_runs(&self, files: &[String]) {
        let now = filetime_now();
        // Each removal is dated no earlier than the run it removed, so one
        // dated in the future (a clock that was ahead) goes too.
        let ops = {
            let mut h = self.runs.history.borrow_mut();
            files
                .iter()
                .map(|f| Op::Delete {
                    key: f.clone(),
                    at: deleted_at(h.remove(f).map(|e| e.last), now),
                })
                .collect()
        };
        self.runs_changed(ops);
    }

    /// `search_edit_enter_action`, from Tools > Options.
    pub(super) fn set_enter_action(&self, action: EnterAction) {
        self.runs.enter.set(action);
    }

    pub(super) fn enter_action(&self) -> EnterAction {
        self.runs.enter.get()
    }

    /// Whether the run history is on (`run_history_enabled`).
    pub(super) fn run_history_enabled(&self) -> bool {
        self.runs.enabled
    }

    /// The run count of `file` (a full path); 0 if never run.
    pub(super) fn run_count(&self, file: &str) -> u32 {
        self.runs.history.borrow().count(file)
    }

    /// Set Run Count...: each of `files` (full paths) counts as run `count`
    /// times; 0 removes them from the run history, as in Everything. A
    /// lower count than the file has must win the write, which keeps the
    /// higher count: the file's entry is removed first, then put back with
    /// the new count.
    pub(super) fn set_run_counts(&self, files: &[String], count: u32) {
        if !self.runs.enabled || files.is_empty() {
            return;
        }
        if count == 0 {
            self.forget_runs(files);
            return;
        }
        let now = filetime_now();
        let mut ops = Vec::new();
        {
            let mut h = self.runs.history.borrow_mut();
            for f in files {
                if let Some((set, had)) = h.set_count(f, count, now) {
                    ops.push(Op::Delete {
                        key: f.clone(),
                        at: deleted_at(had, now),
                    });
                    ops.push(Op::Put(set));
                }
            }
        }
        self.runs_changed(ops);
    }

    fn forget_all_runs(&self) {
        let now = filetime_now();
        let at = {
            let mut h = self.runs.history.borrow_mut();
            let newest = h.entries().iter().map(|e| e.last).max();
            h.replace(Vec::new());
            deleted_at(newest, now)
        };
        self.runs_changed(vec![Op::Clear { at }]);
    }

    /// Merges `entries` in: each file takes the higher count and the later
    /// date. Imported on purpose: a file removed earlier comes back.
    fn take_in_runs(&self, entries: Vec<Ran>) {
        let mut entries = history::tidy(entries);
        history::prune(&mut entries, self.runs.keep, filetime_now());
        let ops: Vec<Op<Ran>> = entries.into_iter().map(Op::Put).collect();
        self.runs.history.borrow_mut().apply(&ops);
        self.runs_changed(ops);
    }
}

impl Host for App {
    fn runs(&self) -> Vec<Ran> {
        self.runs.history.borrow().entries().to_vec()
    }

    fn open_runs(&self, owner: HWND, files: &[String], reveal: bool) {
        let paths: Vec<Vec<u16>> = files.iter().map(|f| f.encode_utf16().collect()).collect();
        let many = if reveal {
            actions::reveal_windows(&paths)
        } else {
            paths.len()
        };
        if !self.confirm_open_over(owner, many) {
            return;
        }
        self.record_runs(paths.iter().map(|p| &p[..]), reveal);
        if reveal {
            actions::reveal(paths);
        } else {
            // Whether each is a folder is not known without asking the disk
            // here; the shell opens either the same way.
            let targets = paths
                .into_iter()
                .map(|path| Target {
                    path,
                    is_dir: false,
                })
                .collect();
            actions::open(self.hwnd, targets);
        }
    }

    fn remove_runs(&self, files: &[String]) {
        self.forget_runs(files);
    }

    fn remove_all_runs(&self) {
        self.forget_all_runs();
    }

    fn import_runs(&self, entries: Vec<Ran>) {
        self.take_in_runs(entries);
    }
}
