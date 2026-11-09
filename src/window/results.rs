//! The result actions: what they act on (the selected rows that are held),
//! opening (the shell work runs off this thread, in `actions`), Properties,
//! the copy commands and the file clipboard, and the double-click. The row
//! context menu is `context.rs`.
//!
//! Only rows that are held can be acted on: the list holds a few screens of
//! a result, never all of a large one. When part of a selection is not held
//! the action takes the rest and the status bar says so for a while, so
//! nothing is silently left out.

use windows_sys::Win32::Foundation::{HWND, POINT};
use windows_sys::Win32::System::Diagnostics::Debug::MessageBeep;
use windows_sys::Win32::UI::Controls::{
    LVHITTESTINFO, LVM_GETNEXTITEM, LVM_GETSELECTEDCOUNT, LVM_SUBITEMHITTEST, LVNI_SELECTED,
};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::VK_CONTROL;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    IDYES, KillTimer, MB_ICONQUESTION, MB_ICONWARNING, MB_YESNO, SetTimer,
};

use super::{App, COL_PATH, TIMER_NOTE, key_down, send};
use crate::actions::{self, Target};
use crate::clipboard::{self, Effect, Files};
use crate::copytext::{self, Field};
use crate::{format, msgbox};

/// Opening more items than this asks first, as Everything does
/// (open_many_files_warning_threshold).
const MANY: usize = 32;

/// How long a note stays in the status bar.
const NOTE_MS: u32 = 6000;

/// A result action, for the note on a selection only partly held.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Act {
    Open,
    OpenPath,
    RunAs,
    Properties,
    Copy,
    Cut,
    Delete,
    /// Dragged out.
    Drag,
    /// Another command of the Explorer menu (Send to, an archiver...).
    Command,
}

/// What the status bar says when an action took `done` of the `selected`
/// rows, the others not being held; nothing when it took them all.
fn partial_note(act: Act, done: usize, selected: usize) -> Option<String> {
    if done >= selected {
        return None;
    }
    let of = format!(
        "{} of {} items",
        format::grouped(done as u64),
        format::grouped(selected as u64)
    );
    let what = match act {
        Act::Open => format!("Opened {of}"),
        Act::OpenPath => format!("Opened the path of {of}"),
        Act::RunAs => format!("Ran {of} as administrator"),
        Act::Properties => format!("Showing the properties of {of}"),
        Act::Copy => format!("Copied {of}"),
        Act::Cut => format!("Cut {of}"),
        Act::Delete => format!("Deleting {of}"),
        Act::Drag => format!("Dragged {of}"),
        Act::Command => format!("Ran the command on {of}"),
    };
    Some(format!("{what}; scroll to load the rest"))
}

/// The question asked before opening `n` things at once, if `n` is many.
fn many_question(n: usize) -> Option<String> {
    (n > MANY).then(|| {
        format!(
            "Are you sure you want to open {} items?",
            format::grouped(n as u64)
        )
    })
}

impl App {
    /// The selected rows that are held, in order. It looks only inside the
    /// held ranges, so a huge selection costs no more than the rows held.
    pub fn selected_held(&self) -> Vec<usize> {
        let ranges = self.model.borrow().loaded();
        let mut out = Vec::new();
        for range in ranges {
            // The row before the range: GETNEXTITEM looks after it, and -1
            // (usize::MAX) means from the top.
            let mut at = range.start.wrapping_sub(1);
            loop {
                let next = send(self.list, LVM_GETNEXTITEM, at, LVNI_SELECTED as isize);
                if next < 0 || next as usize >= range.end {
                    break;
                }
                out.push(next as usize);
                at = next as usize;
            }
        }
        out
    }

    /// What a result action acts on: the selected rows that are held, and
    /// how many rows are selected in all.
    fn picked(&self) -> (Vec<usize>, usize) {
        let selected = send(self.list, LVM_GETSELECTEDCOUNT, 0, 0).max(0) as usize;
        if selected == 0 {
            return (Vec::new(), 0);
        }
        (self.selected_held(), selected)
    }

    /// The picked rows' paths and kinds.
    fn picked_targets(&self) -> (Vec<Target>, usize) {
        let (held, selected) = self.picked();
        let m = self.model.borrow();
        let targets = held
            .iter()
            .filter_map(|&i| m.row(i))
            .map(|row| Target {
                path: row.path.to_vec(),
                is_dir: row.is_dir,
            })
            .collect();
        (targets, selected)
    }

    pub(super) fn picked_paths(&self) -> (Vec<Vec<u16>>, usize) {
        let (targets, selected) = self.picked_targets();
        (targets.into_iter().map(|t| t.path).collect(), selected)
    }

    /// Says in the status bar, for a while, that `act` took only `done` of
    /// the `selected` rows; an action that took them all takes away a note
    /// about an earlier one.
    pub(super) fn note_partial(&self, act: Act, done: usize, selected: usize) {
        match partial_note(act, done, selected) {
            Some(text) => self.show_note(text),
            None => self.clear_note(),
        }
    }

    /// Shows `text` in the status bar for a while, instead of the counts.
    pub(super) fn show_note(&self, text: String) {
        *self.note.borrow_mut() = Some(text);
        // SAFETY: a timer of this window; setting it again restarts it.
        unsafe { SetTimer(self.hwnd, TIMER_NOTE, NOTE_MS, None) };
        self.invalidate_status();
    }

    /// A copy found the clipboard taken: says so, as nothing changed on it
    /// and a paste would bring back whatever was copied before.
    fn copy_failed(&self) {
        self.show_note(clipboard::BUSY.into());
        // SAFETY: plays a system sound.
        unsafe { MessageBeep(MB_ICONWARNING) };
    }

    /// The status bar's note, while it shows one.
    pub fn status_note(&self) -> Option<String> {
        self.note.borrow().clone()
    }

    /// Takes the note away (its time is up, or the search changed).
    pub(super) fn clear_note(&self) {
        if self.note.borrow_mut().take().is_some() {
            // SAFETY: a timer of this window.
            unsafe { KillTimer(self.hwnd, TIMER_NOTE) };
            self.invalidate_status();
        }
    }

    /// Asks before opening `n` things at once, when that is many; true to
    /// go ahead.
    fn confirm_open(&self, n: usize) -> bool {
        self.confirm_open_over(self.hwnd, n)
    }

    /// [`confirm_open`](Self::confirm_open) with the question owned by
    /// `owner` (a dialog that opens things).
    pub(super) fn confirm_open_over(&self, owner: HWND, n: usize) -> bool {
        let Some(question) = many_question(n) else {
            return true;
        };
        // Nothing of `App` is borrowed while the box runs its loop.
        msgbox::show(owner, &question, "goz", MB_YESNO | MB_ICONQUESTION) == IDYES
    }

    /// Opens the selection, or (`reveal`) the folders holding it with the
    /// items selected, one Explorer window per folder. Each open counts as
    /// a run of the item in the run history (Open Path only as
    /// `run_history_increase_run_count_on_open_path` says), once it is
    /// handed to the shell.
    pub(super) fn open_selected(&self, reveal: bool) {
        if reveal {
            let (paths, selected) = self.picked_paths();
            // Many items in one folder are one window: what is counted is
            // the windows.
            if !self.confirm_open(actions::reveal_windows(&paths)) {
                return;
            }
            let done = paths.len();
            self.record_runs(paths.iter().map(|p| &p[..]), true);
            actions::reveal(paths);
            self.note_partial(Act::OpenPath, done, selected);
        } else {
            let (targets, selected) = self.picked_targets();
            if !self.confirm_open(targets.len()) {
                return;
            }
            let done = targets.len();
            self.record_runs(targets.iter().map(|t| &t.path[..]), false);
            actions::open(self.hwnd, targets);
            self.note_partial(Act::Open, done, selected);
        }
    }

    /// Ctrl+Shift+Enter: runs the selection as administrator.
    pub(super) fn run_as_selected(&self) {
        let (paths, selected) = self.picked_paths();
        if !self.confirm_open(paths.len()) {
            return;
        }
        let done = paths.len();
        actions::run_as(self.hwnd, paths);
        self.note_partial(Act::RunAs, done, selected);
    }

    /// The shell's Properties for the selection, in one sheet (off this
    /// thread: `actions::properties`).
    pub(super) fn properties_selected(&self) {
        let (paths, selected) = self.picked_paths();
        let done = paths.len();
        actions::properties(self.hwnd, paths);
        self.note_partial(Act::Properties, done, selected);
    }

    /// Copies `field` of each selected row, as the list shows it, one row
    /// per line.
    pub(super) fn copy(&self, field: Field) {
        let (held, selected) = self.picked();
        let text = {
            let m = self.model.borrow();
            copytext::lines(held.iter().filter_map(|&i| m.row(i)), field)
        };
        if !text.is_empty() && !clipboard::put_text(self.hwnd, &text) {
            self.copy_failed();
            return;
        }
        self.note_partial(Act::Copy, held.len(), selected);
    }

    /// Edit > Copy and Cut: the selected files on the clipboard, for
    /// Explorer to paste (moving them when cut).
    pub(super) fn copy_files(&self, effect: Effect) {
        let (held, selected) = self.picked();
        let files = {
            let m = self.model.borrow();
            Files::new(held.iter().filter_map(|&i| m.row(i)).map(|r| &r.path[..]))
        };
        if !held.is_empty() && !clipboard::put_files(self.hwnd, &files, effect) {
            self.copy_failed();
            return;
        }
        let act = match effect {
            Effect::Copy => Act::Copy,
            Effect::Move => Act::Cut,
        };
        self.note_partial(act, held.len(), selected);
    }

    /// The column under `pt` (the list's client coordinates) on a row, by
    /// its id, wherever it has been dragged to.
    fn column_at(&self, pt: POINT) -> Option<usize> {
        let mut hit = LVHITTESTINFO {
            pt,
            flags: 0,
            iItem: -1,
            iSubItem: -1,
            iGroup: 0,
        };
        let item = send(self.list, LVM_SUBITEMHITTEST, 0, (&raw mut hit) as isize);
        if item < 0 || hit.iSubItem < 0 {
            return None;
        }
        // The list gives the column's index; its id says which it is.
        self.column_id(hit.iSubItem as usize)
    }

    /// A double-click on a row at `pt`: opens the selection, or its path
    /// when the click was in the Path column or Ctrl was down.
    pub(super) fn double_click(&self, pt: POINT) {
        self.record_search();
        let reveal = key_down(VK_CONTROL) || self.column_at(pt) == Some(COL_PATH);
        self.open_selected(reveal);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_partly_held_selection_is_reported() {
        assert_eq!(
            partial_note(Act::Copy, 1024, 5000).as_deref(),
            Some("Copied 1,024 of 5,000 items; scroll to load the rest")
        );
        assert_eq!(
            partial_note(Act::Cut, 0, 3).as_deref(),
            Some("Cut 0 of 3 items; scroll to load the rest")
        );
        assert_eq!(
            partial_note(Act::Open, 10, 11).as_deref(),
            Some("Opened 10 of 11 items; scroll to load the rest")
        );
        assert_eq!(
            partial_note(Act::Drag, 1019, 43_345).as_deref(),
            Some("Dragged 1,019 of 43,345 items; scroll to load the rest")
        );
        for act in [
            Act::OpenPath,
            Act::RunAs,
            Act::Properties,
            Act::Delete,
            Act::Command,
        ] {
            assert!(partial_note(act, 1, 2).is_some());
        }
        // All of it, or nothing selected: no note.
        assert_eq!(partial_note(Act::Copy, 5, 5), None);
        assert_eq!(partial_note(Act::Open, 0, 0), None);
    }

    #[test]
    fn opening_many_asks_first() {
        assert_eq!(many_question(0), None);
        assert_eq!(many_question(32), None);
        assert_eq!(
            many_question(33).as_deref(),
            Some("Are you sure you want to open 33 items?")
        );
        assert_eq!(
            many_question(24_576).as_deref(),
            Some("Are you sure you want to open 24,576 items?")
        );
    }
}
