//! Rows goz-gui selects or deselects one by one, a slice at a time.
//!
//! The list is virtual (owner data): it takes a selection one row at a
//! time, or all rows at once, and each row costs it about 10 µs (its
//! ranges, its notifications, accessibility events). Invert Selection
//! with a few thousand rows on its smaller side, or a refresh selecting
//! its few thousand held files again, held the window for 28 to 83 ms
//! under load. So those rows are set here in slices of at most
//! [`SLICE`]: the first at once (a small job is done before the command
//! returns, as before), the rest each from a message posted after the
//! last, or from a timer when the user's input is waiting, so the input
//! comes first.
//!
//! The rows end selected exactly as if they had been set at once: before
//! anything that reads or changes the selection (a command, a menu, the
//! keyboard or a mouse button in the list, a refresh noting the
//! selection, a tab or the session noting it) the rest is set at once
//! ([`App::finish_selecting`]), and a selection replaced (the rows
//! cleared for others) drops what was still to be set.

use std::collections::VecDeque;
use std::ops::Range;
use std::time::{Duration, Instant};

use windows_sys::Win32::Graphics::Gdi::InvalidateRect;
use windows_sys::Win32::UI::Controls::LVIS_SELECTED;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    GetInputState, KillTimer, PostMessageW, SetTimer, WM_APP,
};

use super::App;
use crate::pane::PaneEvent;

/// Posted to the main window: the next slice of rows to set.
pub(super) const WM_SELECTING: u32 = WM_APP + 370;
/// The main window's timer for the next slice when input was waiting.
pub(super) const TIMER_SELECTING: usize = 370;

/// The longest a slice runs: the window answers within a frame.
const SLICE: Duration = Duration::from_millis(8);

/// Rows set between two looks at the clock.
const CHUNK: usize = 32;

/// One step of a job: every row, or a range of rows, selected or not.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Step {
    All(bool),
    Rows(Range<usize>, bool),
}

/// The steps still to take, in order.
#[derive(Debug, Default)]
pub(super) struct Selecting {
    steps: VecDeque<Step>,
    /// The next slice is on its way (a posted message or the timer).
    queued: bool,
}

impl Selecting {
    /// Adds `step` at the end. Rows next to the last step's, set the same
    /// way, join it.
    pub(super) fn push(&mut self, step: Step) {
        if let Step::Rows(rows, on) = &step {
            if rows.is_empty() {
                return;
            }
            if let Some(Step::Rows(last, last_on)) = self.steps.back_mut()
                && last_on == on
                && last.end == rows.start
            {
                last.end = rows.end;
                return;
            }
        }
        self.steps.push_back(step);
    }

    pub(super) fn is_empty(&self) -> bool {
        self.steps.is_empty()
    }

    /// Takes the next steps, up to `rows` rows' worth (every row at once
    /// counts as one), splitting a range if need be.
    pub(super) fn take(&mut self, rows: usize) -> Vec<Step> {
        let mut out = Vec::new();
        let mut left = rows;
        while left > 0 {
            let Some(step) = self.steps.pop_front() else {
                break;
            };
            match step {
                Step::All(on) => {
                    out.push(Step::All(on));
                    left -= 1;
                }
                Step::Rows(r, on) if r.len() <= left => {
                    left -= r.len();
                    out.push(Step::Rows(r, on));
                }
                Step::Rows(r, on) => {
                    let split = r.start + left;
                    out.push(Step::Rows(r.start..split, on));
                    self.steps.push_front(Step::Rows(split..r.end, on));
                    left = 0;
                }
            }
        }
        out
    }

    /// Drops every step still to take.
    fn clear(&mut self) {
        self.steps.clear();
    }
}

impl App {
    /// Sets the rows of `steps`, in order, as goz-gui's own change: as
    /// many as fit in a slice now, the rest in later slices. The status bar
    /// and the panes hear of it once all is done.
    pub(super) fn select_rows(&self, steps: impl IntoIterator<Item = Step>) {
        {
            let mut job = self.selecting.borrow_mut();
            for step in steps {
                job.push(step);
            }
        }
        if !self.selecting.borrow().queued {
            self.selecting_slice(Some(SLICE));
        }
    }

    /// The next slice's message or timer came.
    pub(super) fn selecting_due(&self) {
        // SAFETY: this window's timer (none set is fine).
        unsafe { KillTimer(self.hwnd, TIMER_SELECTING) };
        self.selecting.borrow_mut().queued = false;
        self.selecting_slice(Some(SLICE));
    }

    /// Sets the rest at once: something is about to read or change the
    /// selection, and must find it as it is to be.
    pub(super) fn finish_selecting(&self) {
        if !self.selecting.borrow().is_empty() {
            self.selecting_slice(None);
        }
    }

    /// The selection is being replaced (the rows cleared for others): what
    /// was still to be set is not wanted.
    pub(super) fn drop_selecting(&self) {
        self.selecting.borrow_mut().clear();
    }

    /// Sets rows for up to `budget` (all of them with `None`), then queues
    /// the next slice if any are left.
    fn selecting_slice(&self, budget: Option<Duration>) {
        let start = Instant::now();
        let was = self.bulk.replace(true);
        self.quietly(|| {
            loop {
                // Not borrowed while the list is told: it notifies this
                // window of each row.
                let steps = self.selecting.borrow_mut().take(CHUNK);
                if steps.is_empty() {
                    break;
                }
                for step in steps {
                    match step {
                        Step::All(on) => self.set_state(
                            usize::MAX,
                            if on { LVIS_SELECTED } else { 0 },
                            LVIS_SELECTED,
                        ),
                        Step::Rows(rows, on) => {
                            let state = if on { LVIS_SELECTED } else { 0 };
                            for i in rows {
                                self.set_state(i, state, LVIS_SELECTED);
                            }
                        }
                    }
                }
                if budget.is_some_and(|b| start.elapsed() >= b) {
                    break;
                }
            }
        });
        self.bulk.set(was);
        let done = self.selecting.borrow().is_empty();
        if !done && !self.selecting.borrow().queued {
            self.selecting.borrow_mut().queued = true;
            // The user's keys and clicks first, if some are waiting (a
            // posted message would come before them): the timer comes
            // after them.
            // SAFETY: a plain query, and a timer or a post of this
            // thread's window.
            unsafe {
                if GetInputState() != 0 {
                    SetTimer(self.hwnd, TIMER_SELECTING, 1, None);
                } else {
                    PostMessageW(self.hwnd, WM_SELECTING, 0, 0);
                }
            }
        }
        if was {
            // Inside another run of rows (`in_bulk`): it reports.
            return;
        }
        // SAFETY: repaints this thread's list.
        unsafe { InvalidateRect(self.list, core::ptr::null(), 0) };
        if done && self.bulk_changed.take() {
            self.invalidate_status();
            self.pane_event(PaneEvent::SelectionChanged);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn steps(job: &mut Selecting, rows: usize) -> Vec<Step> {
        job.take(rows)
    }

    #[test]
    fn steps_come_in_order_a_slice_at_a_time() {
        let mut job = Selecting::default();
        job.push(Step::All(true));
        job.push(Step::Rows(0..10, false));
        // Next to the last, the same way: one range.
        job.push(Step::Rows(10..20, false));
        // Empty: nothing.
        job.push(Step::Rows(5..5, true));
        job.push(Step::Rows(30..31, true));
        assert_eq!(
            steps(&mut job, 8),
            [Step::All(true), Step::Rows(0..7, false)]
        );
        assert_eq!(steps(&mut job, 13), [Step::Rows(7..20, false)]);
        assert!(!job.is_empty());
        assert_eq!(steps(&mut job, 100), [Step::Rows(30..31, true)]);
        assert!(job.is_empty());
        assert!(steps(&mut job, 100).is_empty());
        // Selected after deselected, or apart: not joined.
        job.push(Step::Rows(0..2, false));
        job.push(Step::Rows(2..4, true));
        job.push(Step::Rows(5..6, true));
        assert_eq!(
            steps(&mut job, 100),
            [
                Step::Rows(0..2, false),
                Step::Rows(2..4, true),
                Step::Rows(5..6, true)
            ]
        );
        job.push(Step::Rows(1..3, true));
        job.clear();
        assert!(job.is_empty());
    }

    #[test]
    fn a_job_ends_as_if_set_at_once() {
        // Inverting rows 0..4096 of 10,000 selected, taken in slices of
        // any size, ends with the same rows selected.
        for slice in [1, 7, 32, 1000, 5000] {
            let mut job = Selecting::default();
            job.push(Step::All(true));
            job.push(Step::Rows(0..4096, false));
            let mut sel = crate::selection::Selection::default();
            sel.set_count(10_000);
            sel.set(0..4096, true);
            loop {
                let next = job.take(slice);
                if next.is_empty() {
                    break;
                }
                for step in next {
                    match step {
                        Step::All(on) => sel.set_all(on),
                        Step::Rows(r, on) => {
                            for i in r {
                                sel.set(i..i + 1, on);
                            }
                        }
                    }
                }
            }
            assert_eq!(sel.len(), 10_000 - 4096, "slice {slice}");
            assert_eq!(
                sel.ranges().map(|r| (r.start, r.end)).collect::<Vec<_>>(),
                [(4096, 10_000)]
            );
        }
    }
}
