//! Wave 9, stream B1, from the window's side: the disk reader of the wave 9
//! columns (`crate::props`), the content search running (`crate::content`).
//! What stream B2 adds over gozd's new protocol (push, sort keys, columns
//! from gozd) is `w9b.rs`.
//!
//! Its WM_APP messages are `WM_APP + 420..430`, its timers `420..430`, its
//! commands `5150..5200` (the header menu's columns and Organize Columns).

use std::cell::{Cell, RefCell};
use std::sync::Arc;

use windows_sys::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows_sys::Win32::UI::Controls::LVM_REDRAWITEMS;
use windows_sys::Win32::UI::WindowsAndMessaging::{KillTimer, SetTimer, WM_APP};

use super::{App, send};
use crate::client::Search;
use crate::columns::{self, COL_ACCESSED, COL_ATTRIB, COL_CREATED, COL_RECENT};
use crate::content::plan::{Plan, Planned};
use crate::content::{Key, Run};
use crate::props;
use goz_core::types::SortKey;

/// The disk reader's answers are waiting (`props::Reader::take`).
pub(super) const WM_PROPS: u32 = WM_APP + 420;
/// The content search running found more, or ended.
pub(super) const WM_CONTENT: u32 = WM_APP + 421;
/// Options > Indexes' change was answered: `lParam` is a `Box<String>`
/// for the status bar.
pub const WM_INDEX_SET: u32 = WM_APP + 422;
pub(super) const MSG_FIRST: u32 = WM_APP + 420;
pub(super) const MSG_END: u32 = WM_APP + 430;

/// The content search's results are refreshed at most this often.
pub(super) const TIMER_CONTENT: usize = 420;
pub(super) const TIMER_FIRST: usize = 420;
pub(super) const TIMER_END: usize = 430;
const CONTENT_REFRESH_MS: u32 = 250;

/// The window's wave 9 state.
pub(crate) struct W9 {
    pub props: props::Reader,
    /// Stream B2: gozd's watch (`w9b.rs`).
    pub push: super::w9b::Push,
    /// The content search started last (stopped when another starts).
    content: RefCell<Option<Arc<Run>>>,
    /// TIMER_CONTENT is set.
    refresh_due: Cell<bool>,
}

impl W9 {
    pub fn new(hwnd: HWND) -> W9 {
        W9 {
            props: props::Reader::new(hwnd, WM_PROPS),
            push: Default::default(),
            content: RefCell::new(None),
            refresh_due: Cell::new(false),
        }
    }
}

/// Whether `msg` is one of wave 9's.
pub(super) fn is_message(msg: u32) -> bool {
    (MSG_FIRST..MSG_END).contains(&msg)
}

/// Whether `id` is one of wave 9's timers.
pub(super) fn is_timer(id: usize) -> bool {
    (TIMER_FIRST..TIMER_END).contains(&id)
}

/// The gozd feature that sorts by column `id`, for the columns gozd 0.6
/// cannot sort by (wave 9 spec, protocol item 1).
pub(super) fn sort_feature(id: usize) -> Option<&'static str> {
    Some(match id {
        columns::COL_EXT => "sort:extension",
        COL_CREATED => "sort:date_created",
        COL_ACCESSED => "sort:date_accessed",
        COL_ATTRIB => "sort:attributes",
        COL_RECENT => "sort:date_recently_changed",
        _ => return None,
    })
}

impl App {
    /// Wave 9's WM_APP messages.
    pub(super) fn w9_message(&self, msg: u32, w: WPARAM, l: LPARAM) {
        match msg {
            WM_PROPS => self.props_answered(),
            WM_CONTENT => self.content_news(),
            super::w9b::WM_WATCH => self.watch_news(w),
            super::w9b::WM_REFETCH => self.refetch(),
            WM_INDEX_SET => {
                // SAFETY: the box the Indexes page's thread posted; owned
                // here once the message came.
                let text = unsafe { Box::from_raw(l as *mut String) };
                self.show_note(*text);
            }
            _ => {}
        }
    }

    /// Wave 9's timers.
    pub(super) fn w9_timer(&self, id: usize) {
        if id == super::w9b::TIMER_WATCH_RETRY {
            self.watch_retry();
        }
        if id == TIMER_CONTENT {
            self.w9.refresh_due.set(false);
            if self.model.borrow().search.content.is_some() {
                self.start_search(true);
            }
        }
    }

    /// The window ends: a content search stops reading.
    pub(super) fn w9_exit(&self) {
        if let Some(run) = self.w9.content.take() {
            run.stop();
        }
        // Closing the watch's pipe ends it in gozd.
        self.end_watch();
    }

    // ---- The wave 9 columns' values ----

    /// The values a cell of column `col` of `row` (row `index` of the
    /// search shown) draws: asked of the disk the first time when the row's
    /// source has none (rows of gozd's index, a content search's; a file
    /// list's rows have their own).
    pub fn wanted_props(
        &self,
        m: &crate::model::Model,
        row: &crate::client::Row,
        col: usize,
        index: usize,
    ) {
        if !matches!(col, COL_CREATED | COL_ACCESSED | COL_ATTRIB)
            || m.search.filelist.is_some()
            || (col == COL_CREATED && row.ctime.is_some())
            || !row.more.get().wants_disk()
        {
            return;
        }
        if let Some(other) = self
            .w9
            .props
            .ask(row, m.shown, index)
            .and_then(|i| m.peek(i))
        {
            props::unask(other);
        }
    }

    /// The rows of the search shown are drawn again: the disk's answers.
    fn props_answered(&self) {
        let answers = self.w9.props.take();
        let (mut low, mut high) = (usize::MAX, 0usize);
        {
            let m = self.model.borrow();
            for a in answers {
                if a.epoch != m.shown {
                    continue;
                }
                if let Some(row) = m.peek(a.index) {
                    row.more.set(row.more.get().read(a.disk));
                    low = low.min(a.index);
                    high = high.max(a.index);
                }
            }
        }
        if low <= high {
            send(self.list, LVM_REDRAWITEMS, low, high as isize);
        }
    }

    // ---- content: ----

    /// The search's content terms planned (`content::plan`): none, the
    /// plan, or why it cannot be searched. W9 B2: goz-core's exact helper
    /// (any boolean shape, `plan_exact`) for a gozd that reads the new
    /// query language, which the always-true and always-false searches it
    /// makes need; an older gozd, B1's top-level AND as before.
    pub(super) fn plan_content(&self, text: &str, match_case: bool) -> Planned {
        if crate::client::boolean_syntax() {
            crate::content::plan::plan_exact(text, match_case)
        } else {
            crate::content::plan::plan(text, match_case)
        }
    }

    /// The content run for `plan`, whose searches gozd is asked as
    /// `to_gozd` makes them and as `base` (its sort, case, list, slots)
    /// says: the run reading now when it answers the same (a refresh,
    /// `keep_place`), else a new one; the old one stops.
    pub(super) fn content_run(
        &self,
        plan: &Plan,
        base: &Search,
        keep_place: bool,
        to_gozd: &dyn Fn(&str) -> String,
    ) -> Arc<Run> {
        let queries: Vec<String> = plan.queries.iter().map(|q| to_gozd(q)).collect();
        let key = Key {
            queries: queries.clone(),
            sort: base.sort,
            created_sort: base.created_sort,
            match_case: base.match_case,
            list: base
                .filelist
                .as_ref()
                .map_or(0, |l| Arc::as_ptr(l) as usize),
            terms: plan.terms.clone(),
            exact: plan.exact.as_ref().map(|e| (e.1.clone(), e.2)),
        };
        if keep_place
            && let Some(run) = self.w9.content.borrow().as_ref()
            && run.key == key
            && !run.failed()
        {
            return Arc::clone(run);
        }
        if let Some(old) = self.w9.content.take() {
            old.stop();
        }
        // Several searches' candidates are put in the sort's order by the
        // run (W9R-4), so their rows carry the sort's column.
        let mut want = base.want;
        if queries.len() > 1 {
            match base.sort.key {
                SortKey::DateCreated => want.ctime = true,
                SortKey::DateAccessed => want.atime = true,
                SortKey::Attributes => want.attrib = true,
                SortKey::DateRecentlyChanged => want.rtime = true,
                _ => {}
            }
        }
        let searches = queries
            .into_iter()
            .map(|query| Search {
                query,
                sort: base.sort,
                match_case: base.match_case,
                filelist: base.filelist.clone(),
                created_sort: base.created_sort,
                slots: Arc::clone(&base.slots),
                want,
                content: None,
            })
            .collect();
        let run = Run::start(key, plan, searches, self.hwnd, WM_CONTENT);
        *self.w9.content.borrow_mut() = Some(Arc::clone(&run));
        run
    }

    /// A search without content terms starts: the content search reading
    /// stops.
    pub(super) fn end_content(&self) {
        if let Some(run) = self.w9.content.take() {
            run.stop();
        }
        // SAFETY: a timer of this window.
        unsafe { KillTimer(self.hwnd, TIMER_CONTENT) };
        self.w9.refresh_due.set(false);
    }

    /// Esc: the content search reading stops (true), with what it found so
    /// far; false when none is reading.
    pub(super) fn cancel_content(&self) -> bool {
        let stopped = self
            .w9
            .content
            .borrow()
            .as_ref()
            .is_some_and(|run| run.stop());
        if stopped {
            self.invalidate_status();
        }
        stopped
    }

    /// The content search found more or ended: the status bar says how far
    /// it is, and the results show what it found, at most every 250 ms.
    fn content_news(&self) {
        let shown = self.model.borrow().search.content.clone();
        let Some(run) = shown else {
            // An older run's news: nothing shows it.
            if let Some(run) = self.w9.content.borrow().as_ref() {
                run.told();
            }
            return;
        };
        run.told();
        self.invalidate_status();
        let total = self.model.borrow().total;
        let changed = run.matches() as u64 != total || !run.running();
        if changed && !self.w9.refresh_due.replace(true) {
            // SAFETY: a timer of this window.
            unsafe { SetTimer(self.hwnd, TIMER_CONTENT, CONTENT_REFRESH_MS, None) };
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_stay_in_wave_9s_ranges() {
        assert!(is_message(WM_PROPS) && is_message(WM_CONTENT) && is_message(WM_INDEX_SET));
        assert!(!is_message(MSG_END) && MSG_END == WM_APP + 430);
        assert!(is_timer(TIMER_CONTENT) && !is_timer(TIMER_END));
        // Clear of the framework's and the features' ranges.
        assert!(!super::super::slots::is_feature_message(WM_PROPS));
        assert!(!super::super::slots::is_feature_timer(TIMER_CONTENT));
        assert!(!super::super::custom::is_message(WM_PROPS));
        assert!(!super::super::custom::is_timer(TIMER_CONTENT));
    }

    #[test]
    fn the_new_sorts_name_gozds_features() {
        assert_eq!(sort_feature(COL_CREATED), Some("sort:date_created"));
        assert_eq!(sort_feature(COL_RECENT), Some("sort:date_recently_changed"));
        assert_eq!(sort_feature(COL_ATTRIB), Some("sort:attributes"));
        assert_eq!(sort_feature(columns::COL_NAME), None);
    }
}
