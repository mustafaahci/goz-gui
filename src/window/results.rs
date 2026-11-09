//! The result actions: what they act on (the selected rows that are held),
//! opening (the shell work runs off this thread, in `actions`), Properties,
//! the copy commands and the file clipboard, and the double-click. The row
//! context menu is `context.rs`.
//!
//! Only rows that are held can be acted on: the list holds a few screens of
//! a result, never all of a large one. So an action on a selection whose
//! rows are not all held first fetches them ([`Fetching`]): their blocks
//! are asked of gozd and kept while it waits (the window stays usable,
//! with the busy cursor and a note), and the action runs once they are
//! all here; changing the search or the selection meanwhile cancels it. A
//! selection of more than [`FETCH_MAX`] rows, a block gozd refuses, and a
//! drag (which must start at once) act on the rows held instead, and the
//! status bar says so for a while, so nothing is silently left out.

use std::collections::BTreeSet;
use std::sync::Arc;

use windows_sys::Win32::Foundation::{HWND, POINT};
use windows_sys::Win32::System::Diagnostics::Debug::MessageBeep;
use windows_sys::Win32::UI::Controls::{
    LVHITTESTINFO, LVIF_STATE, LVIS_SELECTED, LVM_GETNEXTITEM, LVM_GETSELECTEDCOUNT,
    LVM_SUBITEMHITTEST, LVN_ITEMCHANGED, LVN_ODSTATECHANGED, LVNI_SELECTED, NMHDR, NMLISTVIEW,
    NMLVODSTATECHANGE,
};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::VK_CONTROL;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    GetCursorPos, GetForegroundWindow, IDC_APPSTARTING, IDC_WAIT, IDYES, IsChild, KillTimer,
    LoadCursorW, MB_ICONQUESTION, MB_ICONWARNING, MB_YESNO, PostMessageW, SendMessageW,
    SetCursor, SetTimer, WM_APP, WM_MOUSEMOVE, WM_NCHITTEST, WM_SETCURSOR, WindowFromPoint,
};

use super::{App, COL_PATH, TIMER_NOTE, key_down, send};
use crate::actions::{self, Target};
use crate::client::{BLOCK_ROWS, Job};
use crate::clipboard::{self, Effect, Files};
use crate::copytext::{self, Field};
use crate::store::location;
use crate::{format, msgbox};

/// A selection of more rows than this is not fetched: the action takes the
/// rows held, as the status bar then says. About 20 blocks, some 5 MB held
/// while it runs; more would make an accidental Ctrl+A and Enter a long
/// wait for something nobody meant.
pub(super) const FETCH_MAX: usize = 20_000;

/// Posted when the rows an action waits for are all here: the action runs
/// from it, after the reply that brought the last of them is done with.
/// Far from the window's own `WM_APP` messages.
pub(super) const WM_FETCHED: u32 = WM_APP + 0x40;

/// The cursor while goz-gui is busy for the user.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Busy {
    Idle,
    /// The shell is making the Explorer menu: the window does not answer
    /// until it is done, the wait cursor says so.
    Wait,
    /// An action waits for its rows; the window is usable meanwhile (the
    /// working-in-the-background cursor).
    Background,
}

/// An action on the selection that waits for its rows ([`Fetching`]).
#[derive(Clone, Copy)]
pub(super) enum Then {
    Open {
        reveal: bool,
    },
    RunAs,
    Properties,
    Copy(Field),
    Files(Effect),
    Delete {
        permanent: bool,
    },
    RunCount,
    /// The row menu: at the mouse (screen), or under the focused row; its
    /// bold item, and whether Shift added the extended verbs.
    RowMenu {
        at: Option<POINT>,
        default: u16,
        extended: bool,
    },
}

/// The blocks holding the selected rows, in order. `next(after)` is the
/// first selected row after row `after` (from the top for `None`), as
/// LVM_GETNEXTITEM gives it; it is asked once per block, as the walk skips
/// to the end of each block it finds a row in, so a huge selection costs no
/// more than its blocks.
pub(super) fn selection_blocks(next: impl Fn(Option<usize>) -> Option<usize>) -> Vec<u32> {
    let rows = BLOCK_ROWS as usize;
    let mut out = Vec::new();
    let mut after = None;
    while let Some(row) = next(after) {
        let block = row / rows;
        out.push(block as u32);
        after = Some((block + 1) * rows - 1);
    }
    out
}

/// A selection's rows on their way before an action runs on them.
pub(super) struct Fetching {
    /// The search whose rows they are.
    epoch: u64,
    /// Every block with a selected row, held or not: kept (pinned) while
    /// the action waits, so a block that comes cannot push out another.
    blocks: Vec<u32>,
    /// The blocks not here yet.
    waiting: BTreeSet<u32>,
    then: Then,
}

impl Fetching {
    /// What `then` waits for, of the `blocks` with selected rows of search
    /// `epoch`, `held` saying which are here; `None` when all are.
    pub(super) fn new(
        epoch: u64,
        blocks: Vec<u32>,
        held: impl Fn(u32) -> bool,
        then: Then,
    ) -> Option<Fetching> {
        let waiting: BTreeSet<u32> = blocks.iter().copied().filter(|&b| !held(b)).collect();
        (!waiting.is_empty()).then_some(Fetching {
            epoch,
            blocks,
            waiting,
            then,
        })
    }

    /// Block `block` of search `epoch` came, or will not come (gozd refused
    /// it): true when that was the last one waited for, once.
    pub(super) fn arrived(&mut self, epoch: u64, block: u32) -> bool {
        epoch == self.epoch && self.waiting.remove(&block) && self.waiting.is_empty()
    }
}

/// Whether a list notification (LVN_ITEMCHANGED, LVN_ODSTATECHANGED at
/// `l`) changed which rows are selected, not only the focus.
///
/// # Safety
/// `l` is the notification's lParam, for a code of the list's.
pub(super) unsafe fn selection_touched(l: isize) -> bool {
    // SAFETY: each structure starts with the NMHDR its code says it is.
    unsafe {
        let code = (*(l as *const NMHDR)).code;
        let (old, new) = match code {
            LVN_ITEMCHANGED => {
                let nm = &*(l as *const NMLISTVIEW);
                if nm.uChanged & LVIF_STATE == 0 {
                    return false;
                }
                (nm.uOldState, nm.uNewState)
            }
            LVN_ODSTATECHANGED => {
                let nm = &*(l as *const NMLVODSTATECHANGE);
                (nm.uOldState, nm.uNewState)
            }
            _ => return false,
        };
        (old ^ new) & LVIS_SELECTED != 0
    }
}

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
    /// Before an action on the selection: when some selected rows are not
    /// held (and not too many are selected), asks gozd for their blocks,
    /// keeps them, and runs `then` once all are here ([`fetch_done`]);
    /// true while it waits. False: act now, on the rows held.
    ///
    /// [`fetch_done`]: Self::fetch_done
    pub(super) fn fetch_first(&self, then: Then) -> bool {
        if self.fetch_bypass.get() {
            return false;
        }
        let selected = send(self.list, LVM_GETSELECTEDCOUNT, 0, 0).max(0) as usize;
        if selected == 0 || selected > FETCH_MAX {
            return false;
        }
        let blocks = selection_blocks(|after| {
            let at = after.unwrap_or(usize::MAX);
            usize::try_from(send(self.list, LVM_GETNEXTITEM, at, LVNI_SELECTED as isize)).ok()
        });
        let fetching = {
            let m = self.model.borrow();
            // A search on its way replaces these rows, and its own blocks
            // are the only ones gozd is asked for now. After gozd refused a
            // block (its time budget), it is not asked for again.
            if m.pending.is_some() || m.partial_failure.is_some() {
                return false;
            }
            Fetching::new(m.shown, blocks, |b| m.holds(b), then)
        };
        let Some(fetching) = fetching else {
            return false;
        };
        // It replaces an action still waiting.
        self.cancel_fetch();
        let (epoch, search, missing) = {
            let mut m = self.model.borrow_mut();
            m.set_pins(fetching.blocks.iter().copied());
            let missing = m.request(fetching.waiting.iter().copied());
            (m.shown, Arc::clone(&m.search), missing)
        };
        // The worker runs the newest job first: the first block last.
        for block in missing.into_iter().rev() {
            self.worker.push(Job::Block {
                epoch,
                block,
                search: Arc::clone(&search),
            });
        }
        *self.fetching.borrow_mut() = Some(fetching);
        // Until the action runs: no timer takes it away.
        // SAFETY: a timer of this window.
        unsafe { KillTimer(self.hwnd, TIMER_NOTE) };
        *self.note.borrow_mut() = Some(format!(
            "Loading {} selected items...",
            format::grouped(selected as u64)
        ));
        self.invalidate_status();
        self.set_busy(Busy::Background);
        true
    }

    /// An action waits for its rows.
    pub(super) fn fetch_waiting(&self) -> bool {
        self.fetching.borrow().is_some()
    }

    /// Block `block` of search `epoch` came, or will not (refused, or
    /// failed): once every block an action waits for has, it runs, from a
    /// posted message, on what is held.
    pub(super) fn fetch_arrived(&self, epoch: u64, block: u32) {
        let done = self
            .fetching
            .borrow_mut()
            .as_mut()
            .is_some_and(|f| f.arrived(epoch, block));
        if done {
            // SAFETY: posts to this thread's window.
            unsafe { PostMessageW(self.hwnd, WM_FETCHED, 0, 0) };
        }
    }

    /// WM_FETCHED: the rows are here; the action runs on them, then they
    /// are no longer kept.
    pub(super) fn fetch_done(&self) {
        let Some(fetching) = self.fetching.borrow_mut().take() else {
            // Cancelled since.
            return;
        };
        self.set_busy(Busy::Idle);
        self.clear_note();
        if self.model.borrow().shown == fetching.epoch {
            // Out of every cell: the action may pump messages (a menu, a
            // question), and must not fetch again.
            self.fetch_bypass.set(true);
            self.run_then(fetching.then);
            self.fetch_bypass.set(false);
        }
        // Kept no more, unless an action started meanwhile waits for them.
        let pins = self
            .fetching
            .borrow()
            .as_ref()
            .map(|f| f.blocks.clone())
            .unwrap_or_default();
        self.unpin(pins);
    }

    /// Keeps only `pins` (none: nothing), and lets the cache shrink back.
    fn unpin(&self, pins: Vec<u32>) {
        let on_screen = self.view_blocks();
        let mut m = self.model.borrow_mut();
        m.set_pins(pins);
        m.make_room(on_screen);
    }

    fn run_then(&self, then: Then) {
        match then {
            Then::Open { reveal } => self.open_selected(reveal),
            Then::RunAs => self.run_as_selected(),
            Then::Properties => self.properties_selected(),
            Then::Copy(field) => self.copy(field),
            Then::Files(effect) => self.copy_files(effect),
            Then::Delete { permanent } => self.delete_selected(permanent),
            Then::RunCount => self.set_run_count(),
            Then::RowMenu {
                at,
                default,
                extended,
            } => {
                // SAFETY: reads the foreground window.
                let front = unsafe { GetForegroundWindow() } == self.hwnd;
                // Not out of nowhere over a program the user went to
                // meanwhile (a test's dump shows nothing).
                if front || location::menu_dump() {
                    self.show_row_menu(at, default, extended);
                }
            }
        }
    }

    /// Forgets the action waiting for its rows: the search or the
    /// selection changed, or another action replaces it. Its blocks still
    /// queued are not wanted any more.
    pub(super) fn cancel_fetch(&self) {
        let Some(fetching) = self.fetching.borrow_mut().take() else {
            return;
        };
        self.unpin(Vec::new());
        self.set_busy(Busy::Idle);
        self.clear_note();
        self.trim_requests(fetching.epoch);
    }

    /// Shows `busy`'s cursor from now on: at once where the mouse is over
    /// the window, as the window under it would on a move.
    pub(super) fn set_busy(&self, busy: Busy) {
        if self.busy.replace(busy) == busy {
            return;
        }
        let mut pt = POINT { x: 0, y: 0 };
        // SAFETY: reads the cursor's place and asks a window of this
        // thread (checked) for its cursor there, as a mouse move would.
        unsafe {
            if GetCursorPos(&mut pt) == 0 {
                return;
            }
            let under = WindowFromPoint(pt);
            if under.is_null() || (under != self.hwnd && IsChild(self.hwnd, under) == 0) {
                return;
            }
            let at = (pt.x as u16 as u32 | ((pt.y as u16 as u32) << 16)) as isize;
            let hit = SendMessageW(under, WM_NCHITTEST, 0, at) as u32 & 0xffff;
            SendMessageW(
                under,
                WM_SETCURSOR,
                under as usize,
                (hit | (WM_MOUSEMOVE << 16)) as isize,
            );
        }
    }

    /// WM_SETCURSOR on the client area while busy: the busy cursor; false
    /// when not busy (the window's own).
    pub(super) fn busy_cursor(&self) -> bool {
        let id = match self.busy.get() {
            Busy::Idle => return false,
            Busy::Wait => IDC_WAIT,
            Busy::Background => IDC_APPSTARTING,
        };
        // SAFETY: a system cursor, shown for this thread's window.
        unsafe { SetCursor(LoadCursorW(core::ptr::null_mut(), id)) };
        true
    }

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
        if self.fetch_first(Then::Open { reveal }) {
            return;
        }
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
        if self.fetch_first(Then::RunAs) {
            return;
        }
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
        if self.fetch_first(Then::Properties) {
            return;
        }
        let (paths, selected) = self.picked_paths();
        let done = paths.len();
        actions::properties(self.hwnd, paths);
        self.note_partial(Act::Properties, done, selected);
    }

    /// Copies `field` of each selected row, as the list shows it, one row
    /// per line.
    pub(super) fn copy(&self, field: Field) {
        if self.fetch_first(Then::Copy(field)) {
            return;
        }
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
        if self.fetch_first(Then::Files(effect)) {
            return;
        }
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

    /// A selection as LVM_GETNEXTITEM walks it, counting the calls.
    fn walk(selected: &[usize]) -> (Vec<u32>, usize) {
        let calls = core::cell::Cell::new(0);
        let blocks = selection_blocks(|after| {
            calls.set(calls.get() + 1);
            selected
                .iter()
                .copied()
                .find(|&i| after.is_none_or(|a| i > a))
        });
        (blocks, calls.get())
    }

    #[test]
    fn the_blocks_of_a_selection_cost_a_call_each() {
        let rows = BLOCK_ROWS as usize;
        assert_eq!(walk(&[]), (vec![], 1));
        assert_eq!(walk(&[0]), (vec![0], 2));
        // Every row of blocks 0-2, and one of block 7: four blocks, five
        // calls, however many rows.
        let mut many: Vec<usize> = (0..3 * rows).collect();
        many.push(7 * rows + 5);
        assert_eq!(walk(&many), (vec![0, 1, 2, 7], 5));
        // The last row of a block and the first of the next.
        assert_eq!(walk(&[rows - 1, rows]), (vec![0, 1], 3));
    }

    #[test]
    fn an_action_waits_for_the_blocks_not_held() {
        let then = Then::Copy(Field::FullPath);
        // All held: nothing to wait for.
        assert!(Fetching::new(5, vec![0, 1], |_| true, then).is_none());
        let mut f = Fetching::new(5, vec![0, 2, 3, 9], |b| b == 0 || b == 3, then).unwrap();
        assert_eq!(f.blocks, [0, 2, 3, 9], "all kept, held or not");
        assert_eq!(f.waiting.iter().copied().collect::<Vec<_>>(), [2, 9]);
        // Blocks of another search, or not waited for, change nothing.
        assert!(!f.arrived(4, 2));
        assert!(!f.arrived(5, 3));
        assert!(!f.arrived(5, 2));
        assert!(!f.arrived(5, 2), "again");
        // The last one: done, once.
        assert!(f.arrived(5, 9));
        assert!(!f.arrived(5, 9));
        assert!(matches!(f.then, Then::Copy(Field::FullPath)));
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
