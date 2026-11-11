//! Wave 9, stream B2, from the window's side: what a gozd that speaks the
//! new protocol adds, each gated on its `Hello.features` (the worker's
//! connection learns them, `Reply::Features`), so a gozd that lists none
//! (v0.6.0) is used exactly as before.
//!
//! - **Push** (`"watch"`): one [`Watcher`] connection says which volumes
//!   changed; the results are then refreshed (coalesced, as `live.rs`
//!   says) instead of every 2 s. Any failure of it (gozd gone, its caps, a
//!   gozd without the feature) and polling takes over at once; a gozd that
//!   still lists "watch" is watched again a minute later.
//! - **Sorts**: a header click on Extension, Date Created, Date Accessed,
//!   Attributes or Date Recently Changed sorts by gozd's key when gozd
//!   lists it (`sort:extension` and so on); an open file list sorts by them
//!   itself.
//! - **Columns** (`"columns"`): the wave 9 columns come with gozd's rows
//!   (`client::Row::from_item`); a column shown after the search asks
//!   gozd again, once.

use std::cell::{Cell, RefCell};
use std::time::Instant;

use goz_core::proto::feature;
use goz_core::types::SortKey;
use windows_sys::Win32::UI::WindowsAndMessaging::{KillTimer, PostMessageW, SetTimer, WM_APP};

use super::App;
use crate::client::{self, WATCH_CHANGED, WATCH_OFF, WATCH_ON, Watcher};
use crate::columns::{self, COL_ACCESSED, COL_ATTRIB, COL_CREATED, COL_RECENT};
use crate::props::Want;

/// The watch's news (`wParam`: `client::WATCH_*`).
pub(super) const WM_WATCH: u32 = WM_APP + 424;
/// A wave 9 column was shown that gozd did not give with the rows: the
/// search runs again, once, to have it.
pub(super) const WM_REFETCH: u32 = WM_APP + 425;
/// The watch is tried again (a gozd that lists "watch" after it failed).
pub(super) const TIMER_WATCH_RETRY: usize = 421;
const WATCH_RETRY_MS: u32 = 60_000;
/// The least time between two `Changed` frames asked of gozd (ms): changes
/// show within a quarter of a second, and a burst of them costs four
/// searches a second at most (fewer for a slow search, `live::live_delay`).
pub(super) const WATCH_MIN_MS: u32 = 250;

/// The window's push state.
#[derive(Default)]
pub(crate) struct Push {
    watcher: RefCell<Option<Watcher>>,
    /// gozd confirmed the watch: the 2 s polling is off.
    on: Cell<bool>,
    /// When gozd last said something changed that no search has shown yet.
    changed_at: Cell<Option<Instant>>,
    /// A refetch for a newly shown column is posted.
    refetch_posted: Cell<bool>,
}

impl Push {
    /// Whether gozd's push replaces the polling now.
    pub fn on(&self) -> bool {
        self.on.get()
    }
}

/// What a watch's state says the live update does next.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum LiveStep {
    /// Polling: the search runs again every interval (no watch, or it
    /// failed).
    Poll,
    /// Watching, and gozd said something changed that the results do not
    /// show yet: a refresh.
    Refresh,
    /// Watching and nothing changed: no timer, no search.
    Wait,
}

/// The live update's next step: polling unless gozd's watch is on; with it,
/// a refresh only for a change no search since has shown (`last_search`
/// started after it).
pub(super) fn live_step(
    watching: bool,
    changed_at: Option<Instant>,
    last_search: Option<Instant>,
) -> LiveStep {
    if !watching {
        return LiveStep::Poll;
    }
    match (changed_at, last_search) {
        (Some(c), Some(s)) if s > c => LiveStep::Wait,
        (Some(_), _) => LiveStep::Refresh,
        (None, _) => LiveStep::Wait,
    }
}

/// The sort key of column `col` that gozd sorts by only when it lists the
/// key (`client::sort_feature`).
pub(super) fn new_sort_key(col: usize) -> Option<SortKey> {
    Some(match col {
        columns::COL_EXT => SortKey::Extension,
        COL_CREATED => SortKey::DateCreated,
        COL_ACCESSED => SortKey::DateAccessed,
        COL_ATTRIB => SortKey::Attributes,
        COL_RECENT => SortKey::DateRecentlyChanged,
        _ => return None,
    })
}

/// Whether `shown` wants a wave 9 column that `asked` (what gozd gave with
/// the rows) has not.
pub(super) fn wants_more(shown: Want, asked: Want) -> bool {
    (shown.ctime && !asked.ctime)
        || (shown.atime && !asked.atime)
        || (shown.attrib && !asked.attrib)
        || (shown.rtime && !asked.rtime)
}

impl App {
    // ---- Push ----

    /// What the worker's connection learned gozd lists now (the first
    /// greeting, and a gozd restarted as another build).
    pub(super) fn features_learned(&self, _features: Vec<String>) {
        if client::has_feature(feature::WATCH) {
            self.start_watch();
        } else {
            // A gozd without it: polling, as before.
            self.end_watch();
            self.arm_live();
        }
    }

    /// Starts gozd's watch, unless one runs, live updates are off, or gozd
    /// does not list "watch".
    pub(super) fn start_watch(&self) {
        if self.prefs.borrow().base.live_update_ms == 0
            || !client::has_feature(feature::WATCH)
            || self.w9.push.watcher.borrow().is_some()
        {
            return;
        }
        let watcher = Watcher::spawn(self.hwnd, WM_WATCH, WATCH_MIN_MS);
        *self.w9.push.watcher.borrow_mut() = watcher;
    }

    /// Stops gozd's watch (its pipe closed); polling takes over.
    pub(super) fn end_watch(&self) {
        let old = self.w9.push.watcher.borrow_mut().take();
        drop(old);
        self.w9.push.on.set(false);
    }

    /// The watch's news.
    pub(super) fn watch_news(&self, what: usize) {
        if self.w9.push.watcher.borrow().is_none() {
            // A watch stopped since: its last words.
            return;
        }
        match what {
            WATCH_ON => {
                self.w9.push.on.set(true);
                crate::trace::mark("watch on");
            }
            WATCH_CHANGED => {
                crate::trace::mark("watch changed");
                // Coalesced: the first change not shown yet sets the
                // refresh's timer; the ones after it wait for that refresh
                // (setting the timer again would put it off for as long as
                // changes keep coming). One a search since has shown is no
                // pending change. Hidden, nothing now: showing the window
                // arms it.
                if self.live_step() != LiveStep::Refresh {
                    self.w9.push.changed_at.set(Some(Instant::now()));
                    if self.shown() {
                        self.arm_live();
                    }
                }
            }
            WATCH_OFF => {
                crate::trace::mark("watch off: polling");
                self.end_watch();
                self.arm_live();
                if client::has_feature(feature::WATCH) {
                    // SAFETY: a timer of this window.
                    unsafe { SetTimer(self.hwnd, TIMER_WATCH_RETRY, WATCH_RETRY_MS, None) };
                }
            }
            _ => {}
        }
    }

    /// The watch's retry timer.
    pub(super) fn watch_retry(&self) {
        // SAFETY: a timer of this window.
        unsafe { KillTimer(self.hwnd, TIMER_WATCH_RETRY) };
        self.start_watch();
    }

    /// The live update's next step, as the watch says.
    pub(super) fn live_step(&self) -> LiveStep {
        live_step(
            self.w9.push.on.get(),
            self.w9.push.changed_at.get(),
            self.last_search.get(),
        )
    }

    /// A refresh for gozd's change ran (or a search since showed it).
    pub(super) fn change_shown(&self) {
        self.w9.push.changed_at.set(None);
    }

    // ---- Sorts ----

    /// A click on the header of column `col`, which gozd 0.6 cannot sort
    /// by: sorted by gozd's key when gozd lists it (and by an open file
    /// list itself, which needs no gozd); otherwise the sort stays and the
    /// status bar says why for a moment.
    pub(super) fn sort_by_new_column(&self, col: usize) {
        if let Some(key) = new_sort_key(col)
            && (self.searches_a_list() || client::sort_feature(key).is_none_or(client::has_feature))
        {
            self.sort_by_key(key);
            return;
        }
        let title = columns::title(col);
        let why = match (super::w9a::sort_feature(col), client::server()) {
            (Some(feature), Some((server, _))) => {
                format!("goz can't sort by {title} yet: {server} does not list \"{feature}\"")
            }
            _ => format!("goz can't sort by {title} yet"),
        };
        self.show_note(why);
    }

    // ---- Columns ----

    /// The columns on the list changed: a wave 9 column gozd did not give
    /// with the rows shown is asked again (one search), when gozd gives
    /// them at all ("columns"); otherwise the disk is read for it, as for
    /// any row.
    pub(super) fn w9_columns_changed(&self) {
        if !client::has_feature(feature::COLUMNS) || self.w9.push.refetch_posted.get() {
            return;
        }
        let shown = Want::of_columns(self.layout.get().shown());
        let more = match self.model.try_borrow() {
            Ok(m) => {
                m.search.filelist.is_none()
                    && m.search.content.is_none()
                    && wants_more(shown, m.search.want)
            }
            Err(_) => false,
        };
        if more {
            self.w9.push.refetch_posted.set(true);
            // SAFETY: a plain post to this thread's window.
            unsafe { PostMessageW(self.hwnd, WM_REFETCH, 0, 0) };
        }
    }

    /// The search runs again for the columns shown since.
    pub(super) fn refetch(&self) {
        self.w9.push.refetch_posted.set(false);
        self.start_search(true);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn polling_stays_without_a_watch() {
        let now = Instant::now();
        assert_eq!(live_step(false, None, None), LiveStep::Poll);
        assert_eq!(live_step(false, Some(now), Some(now)), LiveStep::Poll);
    }

    #[test]
    fn a_watch_refreshes_only_for_a_change_not_shown() {
        let t0 = Instant::now();
        let t1 = t0 + Duration::from_millis(10);
        // Nothing changed: no search, no timer.
        assert_eq!(live_step(true, None, Some(t0)), LiveStep::Wait);
        // A change after the last search: refreshed.
        assert_eq!(live_step(true, Some(t1), Some(t0)), LiveStep::Refresh);
        assert_eq!(live_step(true, Some(t1), None), LiveStep::Refresh);
        // A search started after the change shows it already.
        assert_eq!(live_step(true, Some(t0), Some(t1)), LiveStep::Wait);
    }

    #[test]
    fn the_new_columns_sort_by_gozds_new_keys() {
        assert_eq!(new_sort_key(COL_CREATED), Some(SortKey::DateCreated));
        assert_eq!(new_sort_key(columns::COL_EXT), Some(SortKey::Extension));
        assert_eq!(new_sort_key(COL_RECENT), Some(SortKey::DateRecentlyChanged));
        assert_eq!(new_sort_key(columns::COL_TYPE), None);
        assert_eq!(new_sort_key(columns::COL_NAME), None);
        // Each key's feature is the one the header names when it is missing.
        for col in [
            columns::COL_EXT,
            COL_CREATED,
            COL_ACCESSED,
            COL_ATTRIB,
            COL_RECENT,
        ] {
            let key = new_sort_key(col).expect("a key");
            assert_eq!(
                client::sort_feature(key),
                super::super::w9a::sort_feature(col)
            );
        }
    }

    #[test]
    fn a_column_shown_since_is_asked_again() {
        let none = Want::default();
        let ctime = Want {
            ctime: true,
            ..none
        };
        let both = Want {
            attrib: true,
            ..ctime
        };
        assert!(wants_more(ctime, none));
        assert!(wants_more(both, ctime));
        assert!(!wants_more(ctime, both));
        assert!(!wants_more(none, none));
    }

    #[test]
    fn ids_stay_in_wave_9s_ranges() {
        use super::super::w9a;
        assert!(w9a::is_message(WM_WATCH) && w9a::is_message(WM_REFETCH));
        assert!(w9a::is_timer(TIMER_WATCH_RETRY));
        assert_ne!(WM_WATCH, w9a::WM_INDEX_SET);
    }
}
