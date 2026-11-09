//! What the user searched: the search history the search box's dropdown
//! lists, and the navigation history History > Back and Forward walk.
//!
//! Pure logic, no Win32. The search history's entries are the rows of
//! Everything's `Search History.csv` (`Search,Search Count,Last Search
//! Date`), which `crate::history_file` reads and writes. The rules that keep
//! two goz-gui windows from losing each other's searches when both write
//! that file ([`merge`], [`Forgotten`]) and the days kept ([`prune`]) are
//! here.

use std::cmp::Reverse;
use std::collections::HashMap;
use std::time::{SystemTime, UNIX_EPOCH};

/// One search the user made.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Searched {
    pub search: String,
    /// How many times it was searched.
    pub count: u32,
    /// When it was last searched, as a FILETIME (100 ns ticks since 1601,
    /// UTC), which is how the CSV writes it.
    pub last: u64,
}

/// The searches made, newest first, each once (the same text in another
/// case is another search, as in Everything).
#[derive(Debug, Default)]
pub struct SearchHistory {
    entries: Vec<Searched>,
}

impl SearchHistory {
    /// How far searches recorded here grow the history: past it, each new
    /// one pushes the oldest out of this window's list (a script searching
    /// in a loop, far past what a person types). Everything keeps every
    /// search, and so does the file: a history read or merged from it is
    /// never cut, and the next write merges any search pushed out here back
    /// from the file.
    pub const MAX: usize = 50_000;

    /// A history of `entries` in any order: newest first, each search once
    /// (its highest count and latest date), the empty search left out.
    pub fn from_entries(entries: Vec<Searched>) -> SearchHistory {
        SearchHistory {
            entries: tidy(entries),
        }
    }

    pub fn entries(&self) -> &[Searched] {
        &self.entries
    }

    /// Notes that `search` was searched at `now`: it moves to the front with
    /// its count raised. An empty search is not kept; false then.
    pub fn record(&mut self, search: &str, now: u64) -> bool {
        if search.is_empty() {
            return false;
        }
        // Never shorter than it was: a longer history read from the file
        // stays whole.
        let room = self.entries.len().max(Self::MAX);
        let entry = match self.entries.iter().position(|e| e.search == search) {
            Some(i) => {
                let mut e = self.entries.remove(i);
                e.count = e.count.saturating_add(1);
                e.last = e.last.max(now);
                e
            }
            None => Searched {
                search: search.to_owned(),
                count: 1,
                last: now,
            },
        };
        self.entries.insert(0, entry);
        self.entries.truncate(room);
        true
    }

    /// Forgets the entry at `index` (newest first).
    pub fn remove(&mut self, index: usize) -> Option<Searched> {
        (index < self.entries.len()).then(|| self.entries.remove(index))
    }

    /// Forgets `search` (exactly that text).
    pub fn remove_search(&mut self, search: &str) -> Option<Searched> {
        let i = self.entries.iter().position(|e| e.search == search)?;
        self.remove(i)
    }

    /// Takes `entries` (in any order) as the history.
    pub fn replace(&mut self, entries: Vec<Searched>) {
        self.entries = tidy(entries);
    }

    /// The searches that contain `text`, ignoring case, for the search box
    /// to suggest as it is typed: those that start with it first, then the
    /// others, each group newest first; at most `max`. The search that is
    /// `text` exactly is not suggested (it is in the box already).
    pub fn suggestions(&self, text: &str, max: usize) -> Vec<String> {
        let needle = text.to_lowercase();
        if needle.is_empty() {
            return Vec::new();
        }
        let mut starts: Vec<&Searched> = Vec::new();
        let mut contains: Vec<&Searched> = Vec::new();
        for e in &self.entries {
            if e.search == text {
                continue;
            }
            let hay = e.search.to_lowercase();
            if hay.starts_with(&needle) {
                starts.push(e);
            } else if hay.contains(&needle) {
                contains.push(e);
            }
        }
        // Newest first already; the stable sorts only make sure of it.
        starts.sort_by_key(|e| Reverse(e.last));
        contains.sort_by_key(|e| Reverse(e.last));
        starts
            .into_iter()
            .chain(contains)
            .take(max)
            .map(|e| e.search.clone())
            .collect()
    }
}

/// `entries` newest first, each search once (its highest count and latest
/// date), no empty search. None is left out for being many: this is what
/// the file is read and merged with.
pub fn tidy(entries: Vec<Searched>) -> Vec<Searched> {
    let mut out: Vec<Searched> = Vec::with_capacity(entries.len());
    let mut at: HashMap<String, usize> = HashMap::with_capacity(entries.len());
    for e in entries {
        if e.search.is_empty() {
            continue;
        }
        match at.get(&e.search) {
            Some(&i) => {
                let kept = &mut out[i];
                kept.count = kept.count.max(e.count);
                kept.last = kept.last.max(e.last);
            }
            None => {
                at.insert(e.search.clone(), out.len());
                out.push(e);
            }
        }
    }
    // Stable: searches made at the same moment keep their order.
    out.sort_by_key(|e| Reverse(e.last));
    out
}

/// The searches deleted in this process since the file was last written,
/// so that a write merging the file's entries in does not bring them back
/// (the dropdown's Delete, Organize's Delete and Delete All).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Forgotten {
    /// Each search deleted, with when.
    searches: HashMap<String, u64>,
    /// Delete All: every search last made up to then.
    all_until: Option<u64>,
}

impl Forgotten {
    pub fn forget(&mut self, search: &str, now: u64) {
        self.searches.insert(search.to_owned(), now);
    }

    pub fn forget_all(&mut self, now: u64) {
        self.searches.clear();
        self.all_until = Some(now);
    }

    #[cfg(test)]
    pub fn is_empty(&self) -> bool {
        self.searches.is_empty() && self.all_until.is_none()
    }

    /// Whether `e`, found in the file, is one deleted here. A search made
    /// again after it was deleted (in another goz-gui) is not.
    pub fn covers(&self, e: &Searched) -> bool {
        self.all_until.is_some_and(|t| e.last <= t)
            || self.searches.get(&e.search).is_some_and(|&t| e.last <= t)
    }

    /// `written` went out to the file: those deletions are done, unless the
    /// same search was deleted again since.
    pub fn settle(&mut self, written: &Forgotten) {
        for (search, t) in &written.searches {
            if self.searches.get(search) == Some(t) {
                self.searches.remove(search);
            }
        }
        if self.all_until == written.all_until {
            self.all_until = None;
        }
    }
}

/// When a deletion made `now` is dated: never before the entry it deleted
/// (`last`, its date, or the newest date for Delete All), so an entry
/// dated in the future (a clock that was ahead, a file from such a
/// machine) is covered too. Shared with the run history.
pub fn deleted_at(last: Option<u64>, now: u64) -> u64 {
    last.map_or(now, |last| last.max(now))
}

/// The search history after merging `theirs` (the file, which another
/// goz-gui may have written) into `mine` (this process's): every search in
/// either, with the higher count and the later date, newest first. The
/// entries of theirs that this process deleted stay deleted; its own stay.
pub fn merge(mine: &[Searched], theirs: &[Searched], forgotten: &Forgotten) -> Vec<Searched> {
    let mut all: Vec<Searched> = mine.to_vec();
    all.extend(theirs.iter().filter(|e| !forgotten.covers(e)).cloned());
    tidy(all)
}

/// How long searches are kept: `None` forever, else that many days
/// (Everything's `search_history_keep_forever` and
/// `search_history_days_to_keep`).
pub type Keep = Option<u32>;

/// Leaves out the searches last made more than `keep` days before `now`.
pub fn prune(entries: &mut Vec<Searched>, keep: Keep, now: u64) {
    /// A day in FILETIME ticks.
    const DAY: u64 = 864_000_000_000;
    if let Some(days) = keep {
        let since = now.saturating_sub(u64::from(days) * DAY);
        entries.retain(|e| e.last >= since);
    }
}

/// Now, as a FILETIME.
pub fn filetime_now() -> u64 {
    /// 1601-01-01 to 1970-01-01 in 100 ns ticks.
    const UNIX_EPOCH_TICKS: u64 = 116_444_736_000_000_000;
    let since = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    UNIX_EPOCH_TICKS + (since.as_nanos() / 100) as u64
}

/// Where the user has been, oldest first, as a browser keeps it: a new state
/// goes after the current one and drops any that were forward of it; Back
/// and Forward move along the list without changing it.
#[derive(Debug)]
pub struct NavHistory<T> {
    entries: Vec<T>,
    /// The entry shown; meaningless while `entries` is empty.
    current: usize,
}

impl<T> Default for NavHistory<T> {
    fn default() -> Self {
        NavHistory {
            entries: Vec::new(),
            current: 0,
        }
    }
}

impl<T: PartialEq> NavHistory<T> {
    /// The most entries kept; the oldest go first.
    pub const MAX: usize = 256;

    /// A history of `entries` (oldest first) with entry `current` shown, as
    /// a saved session holds it: past [`MAX`](Self::MAX) the oldest go, and
    /// `current` past the end is the last entry.
    pub fn from_parts(mut entries: Vec<T>, current: usize) -> NavHistory<T> {
        let cut = entries.len().saturating_sub(Self::MAX);
        entries.drain(..cut);
        let current = current
            .saturating_sub(cut)
            .min(entries.len().saturating_sub(1));
        NavHistory { entries, current }
    }

    /// Every entry, oldest first.
    pub fn entries(&self) -> &[T] {
        &self.entries
    }

    /// Where the current entry is in [`entries`](Self::entries); `None`
    /// while there is none.
    pub fn current_index(&self) -> Option<usize> {
        (!self.entries.is_empty()).then_some(self.current)
    }

    pub fn current(&self) -> Option<&T> {
        self.entries.get(self.current)
    }

    /// Makes `state` the current entry, after the one current now. False,
    /// and nothing changes, when `state` is the current entry already.
    pub fn commit(&mut self, state: T) -> bool {
        if self.current() == Some(&state) {
            return false;
        }
        if !self.entries.is_empty() {
            self.entries.truncate(self.current + 1);
        }
        self.entries.push(state);
        if self.entries.len() > Self::MAX {
            self.entries.remove(0);
        }
        self.current = self.entries.len() - 1;
        true
    }

    pub fn can_back(&self) -> bool {
        !self.entries.is_empty() && self.current > 0
    }

    pub fn can_forward(&self) -> bool {
        self.current + 1 < self.entries.len()
    }

    pub fn back(&mut self) -> Option<&T> {
        if !self.can_back() {
            return None;
        }
        self.current -= 1;
        self.current()
    }

    pub fn forward(&mut self) -> Option<&T> {
        if !self.can_forward() {
            return None;
        }
        self.current += 1;
        self.current()
    }

    /// Up to `max` entries, newest first, and where the current one is
    /// among them.
    pub fn newest_first(&self, max: usize) -> (Vec<&T>, Option<usize>) {
        let shown: Vec<&T> = self.entries.iter().rev().take(max).collect();
        let current = (!self.entries.is_empty())
            .then(|| self.entries.len() - 1 - self.current)
            .filter(|&i| i < shown.len());
        (shown, current)
    }

    /// Makes entry `index` of [`newest_first`](Self::newest_first) current.
    pub fn go_newest_first(&mut self, index: usize) -> Option<&T> {
        let at = self.entries.len().checked_sub(index + 1)?;
        self.current = at;
        self.current()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn searches(h: &SearchHistory) -> Vec<(&str, u32, u64)> {
        h.entries()
            .iter()
            .map(|e| (e.search.as_str(), e.count, e.last))
            .collect()
    }

    #[test]
    fn a_search_moves_to_the_front_and_counts() {
        let mut h = SearchHistory::default();
        assert!(h.entries().is_empty());
        assert!(h.record("foo", 10));
        assert!(h.record("bar", 20));
        assert_eq!(searches(&h), [("bar", 1, 20), ("foo", 1, 10)]);
        assert!(h.record("foo", 30));
        assert_eq!(searches(&h), [("foo", 2, 30), ("bar", 1, 20)]);
        // The same text in another case is another search.
        assert!(h.record("Foo", 40));
        assert_eq!(h.entries().len(), 3);
    }

    #[test]
    fn empty_searches_are_not_kept() {
        let mut h = SearchHistory::default();
        assert!(!h.record("", 1));
        assert!(h.entries().is_empty());
    }

    #[test]
    fn entries_can_be_removed() {
        let mut h = SearchHistory::default();
        h.record("a", 1);
        h.record("b", 2);
        h.record("c", 3);
        assert_eq!(h.remove(1).map(|e| e.search), Some("b".into()));
        assert_eq!(searches(&h), [("c", 1, 3), ("a", 1, 1)]);
        assert_eq!(h.remove(2), None);
    }

    #[test]
    fn a_history_read_is_never_cut_and_recording_grows_it_to_the_limit() {
        let n = SearchHistory::MAX;
        let many = |count: usize| -> Vec<Searched> {
            (0..count)
                .map(|i| Searched {
                    search: i.to_string(),
                    count: 1,
                    last: i as u64,
                })
                .collect()
        };
        // Read (or merged) from a file longer than the limit: all of it.
        let mut h = SearchHistory::from_entries(many(n + 5));
        assert_eq!(h.entries().len(), n + 5);
        assert_eq!(h.entries().last().map(|e| e.search.as_str()), Some("0"));
        assert_eq!(
            merge(h.entries(), &many(n + 5), &Forgotten::default()).len(),
            n + 5
        );
        // Recording here never makes it shorter than it was...
        h.record("new", n as u64 + 10);
        assert_eq!(h.entries().len(), n + 5);
        assert_eq!(h.entries()[0].search, "new");
        // ...and grows a shorter one only to the limit.
        let mut h = SearchHistory::from_entries(many(n));
        h.record("new", n as u64 + 10);
        assert_eq!(h.entries().len(), n);
        assert_eq!(h.entries().last().map(|e| e.search.as_str()), Some("1"));
        // The search pushed out is merged back from the file at the write.
        assert_eq!(
            merge(h.entries(), &many(n), &Forgotten::default()).len(),
            n + 1
        );
    }

    fn s(search: &str, count: u32, last: u64) -> Searched {
        Searched {
            search: search.into(),
            count,
            last,
        }
    }

    #[test]
    fn a_history_from_a_file_is_newest_first_and_each_once() {
        let mut h = SearchHistory::from_entries(vec![
            s("old", 4, 10),
            s("new", 1, 30),
            s("", 9, 99),
            s("old", 2, 20),
            s("Old", 1, 5),
        ]);
        assert_eq!(
            searches(&h),
            [("new", 1, 30), ("old", 4, 20), ("Old", 1, 5)]
        );
        // Recording one keeps its count and moves it up.
        h.record("old", 40);
        assert_eq!(searches(&h)[0], ("old", 5, 40));
        // A clock set back never makes the date older.
        h.record("old", 1);
        assert_eq!(searches(&h)[0], ("old", 6, 40));
        assert_eq!(h.remove_search("new").map(|e| e.count), Some(1));
        assert_eq!(h.remove_search("new"), None);
        h.replace(vec![s("x", 1, 1), s("y", 1, 2)]);
        assert_eq!(searches(&h), [("y", 1, 2), ("x", 1, 1)]);
    }

    #[test]
    fn suggestions_start_with_the_text_first_then_contain_it() {
        let h = SearchHistory::from_entries(vec![
            s("report.docx", 1, 10),
            s("*.exe", 1, 20),
            s("old report", 1, 30),
            s("REPORT 2024", 1, 40),
            s("notes", 1, 50),
            s("rep", 1, 60),
        ]);
        // Case ignored, the text itself left out, newest first in each group.
        assert_eq!(
            h.suggestions("rep", 10),
            ["REPORT 2024", "report.docx", "old report"]
        );
        // "rep" is another search than "Rep".
        assert_eq!(h.suggestions("Rep", 10).len(), 4);
        assert_eq!(h.suggestions("rep", 2), ["REPORT 2024", "report.docx"]);
        assert!(h.suggestions("", 10).is_empty());
        assert!(h.suggestions("zzz", 10).is_empty());
        assert_eq!(h.suggestions(".", 10), ["*.exe", "report.docx"]);
    }

    #[test]
    fn merging_keeps_both_sides_with_the_higher_count_and_later_date() {
        let mine = [s("a", 3, 50), s("b", 1, 40)];
        let theirs = [s("c", 7, 45), s("a", 5, 20), s("B", 1, 10)];
        let merged = merge(&mine, &theirs, &Forgotten::default());
        assert_eq!(
            merged,
            [s("a", 5, 50), s("c", 7, 45), s("b", 1, 40), s("B", 1, 10)]
        );
        // Merging again changes nothing.
        assert_eq!(merge(&merged, &theirs, &Forgotten::default()), merged);
        assert_eq!(merge(&merged, &merged, &Forgotten::default()), merged);
    }

    #[test]
    fn deleted_searches_stay_deleted_until_written() {
        let mut f = Forgotten::default();
        assert!(f.is_empty());
        f.forget("gone", 100);
        let theirs = [s("gone", 9, 90), s("kept", 1, 80)];
        assert_eq!(merge(&[], &theirs, &f), [s("kept", 1, 80)]);
        // Searched again after the delete (in another window): it stays.
        let again = [s("gone", 10, 120)];
        assert_eq!(merge(&[], &again, &f), [s("gone", 10, 120)]);
        // Searched again here: this process's entry stays, and the old
        // count from the file does not come back with it.
        assert_eq!(
            merge(&[s("gone", 1, 130)], &theirs, &f)[0],
            s("gone", 1, 130)
        );
        // Written: settled. Deleted again meanwhile: still pending.
        let written = f.clone();
        f.forget("gone", 140);
        f.settle(&written);
        assert!(!f.is_empty());
        f.settle(&f.clone());
        assert!(f.is_empty());
    }

    #[test]
    fn delete_all_covers_everything_searched_before_it() {
        let mut f = Forgotten::default();
        f.forget("x", 5);
        f.forget_all(100);
        let theirs = [s("a", 1, 10), s("b", 2, 100), s("c", 1, 101)];
        assert_eq!(
            merge(&[s("mine", 1, 150)], &theirs, &f),
            [s("mine", 1, 150), s("c", 1, 101)]
        );
        let written = f.clone();
        f.settle(&written);
        assert!(f.is_empty());
    }

    #[test]
    fn a_search_dated_in_the_future_can_be_deleted() {
        let now = 1000;
        let theirs = [s("ahead", 1, now + 500), s("normal", 1, now - 10)];
        // Dated by the entry deleted, not by the clock.
        assert_eq!(deleted_at(Some(now + 500), now), now + 500);
        assert_eq!(deleted_at(Some(now - 10), now), now);
        assert_eq!(deleted_at(None, now), now);
        let mut f = Forgotten::default();
        f.forget("ahead", deleted_at(Some(now + 500), now));
        f.forget("normal", deleted_at(Some(now - 10), now));
        assert_eq!(merge(&[], &theirs, &f), []);
        // Delete All dated by the newest search.
        let mut f = Forgotten::default();
        f.forget_all(deleted_at(theirs.iter().map(|e| e.last).max(), now));
        assert_eq!(merge(&[], &theirs, &f), []);
        // Searched again after that (later still): it stays.
        assert_eq!(
            merge(&[], &[s("ahead", 2, now + 600)], &f),
            [s("ahead", 2, now + 600)]
        );
    }

    #[test]
    fn days_to_keep() {
        const DAY: u64 = 864_000_000_000;
        let now = 1000 * DAY;
        let mut e = vec![
            s("today", 1, now - 1),
            s("edge", 1, now - 90 * DAY),
            s("old", 1, now - 91 * DAY),
        ];
        prune(&mut e, None, now);
        assert_eq!(e.len(), 3, "kept forever");
        prune(&mut e, Some(90), now);
        assert_eq!(
            e.iter().map(|e| e.search.as_str()).collect::<Vec<_>>(),
            ["today", "edge"]
        );
        prune(&mut e, Some(0), now);
        assert!(e.is_empty());
    }

    #[test]
    fn filetime_is_after_2020() {
        // 2020-01-01 as a FILETIME.
        assert!(filetime_now() > 132_223_104_000_000_000);
    }

    fn nav(states: &[&'static str]) -> NavHistory<&'static str> {
        let mut n = NavHistory::default();
        for s in states {
            n.commit(*s);
        }
        n
    }

    #[test]
    fn back_and_forward_walk_the_entries() {
        let mut n = nav(&["", "a", "b"]);
        assert_eq!(n.current(), Some(&"b"));
        assert!(n.can_back() && !n.can_forward());
        assert_eq!(n.back(), Some(&"a"));
        assert_eq!(n.back(), Some(&""));
        assert_eq!(n.back(), None);
        assert_eq!(n.current(), Some(&""));
        assert!(!n.can_back() && n.can_forward());
        assert_eq!(n.forward(), Some(&"a"));
        assert_eq!(n.forward(), Some(&"b"));
        assert_eq!(n.forward(), None);
    }

    #[test]
    fn the_current_entry_is_never_repeated() {
        let mut n = nav(&["a"]);
        assert!(!n.commit("a"));
        assert!(n.commit("b"));
        assert!(n.commit("a"));
        assert_eq!(n.newest_first(20).0, [&"a", &"b", &"a"]);
    }

    #[test]
    fn a_new_entry_drops_the_ones_forward_of_the_current() {
        let mut n = nav(&["a", "b", "c"]);
        n.back();
        n.back();
        // Back at "a"; committing what is shown changes nothing.
        assert!(!n.commit("a"));
        assert!(n.can_forward());
        assert!(n.commit("d"));
        assert!(!n.can_forward());
        assert_eq!(n.newest_first(20).0, [&"d", &"a"]);
    }

    #[test]
    fn the_menu_lists_newest_first_with_the_current_one() {
        let mut n = nav(&["", "a", "b", "c"]);
        assert_eq!(n.newest_first(20), (vec![&"c", &"b", &"a", &""], Some(0)));
        n.back();
        assert_eq!(n.newest_first(20).1, Some(1));
        assert_eq!(n.newest_first(2), (vec![&"c", &"b"], Some(1)));
        n.back();
        // The current entry is older than the ones listed.
        assert_eq!(n.newest_first(2).1, None);
        assert_eq!(n.go_newest_first(3), Some(&""));
        assert_eq!(n.newest_first(20).1, Some(3));
        assert_eq!(n.go_newest_first(0), Some(&"c"));
        assert_eq!(n.go_newest_first(4), None);
        assert_eq!(n.current(), Some(&"c"));
        let mut empty: NavHistory<&str> = NavHistory::default();
        assert_eq!(empty.newest_first(20), (vec![], None));
        assert_eq!(empty.go_newest_first(0), None);
        assert!(!empty.can_back() && !empty.can_forward());
    }

    #[test]
    fn a_saved_history_comes_back_where_it_was() {
        let mut n = nav(&["", "a", "b", "c"]);
        n.back();
        assert_eq!(n.entries(), ["", "a", "b", "c"]);
        assert_eq!(n.current_index(), Some(2));
        let mut back = NavHistory::from_parts(n.entries().to_vec(), 2);
        assert_eq!(back.current(), Some(&"b"));
        assert_eq!(back.forward(), Some(&"c"));
        assert_eq!(back.back(), Some(&"b"));
        assert_eq!(back.back(), Some(&"a"));
        // A current index past the end is the last entry; none is none.
        assert_eq!(
            NavHistory::from_parts(vec!["x", "y"], 9).current(),
            Some(&"y")
        );
        let empty: NavHistory<&str> = NavHistory::from_parts(Vec::new(), 3);
        assert_eq!((empty.current(), empty.current_index()), (None, None));
        // Too many: the oldest go, and the current one stays the same entry.
        let max = NavHistory::<usize>::MAX;
        let long = NavHistory::from_parts((0..max + 4).collect(), max + 1);
        assert_eq!(long.entries().len(), max);
        assert_eq!((long.entries()[0], long.current()), (4, Some(&(max + 1))));
        // Current among the dropped: the oldest kept.
        let long = NavHistory::from_parts((0..max + 4).collect(), 1);
        assert_eq!(long.current(), Some(&4));
    }

    #[test]
    fn the_oldest_entries_go_past_the_limit() {
        let mut n = NavHistory::default();
        for i in 0..NavHistory::<usize>::MAX + 3 {
            n.commit(i);
        }
        let (all, current) = n.newest_first(usize::MAX);
        assert_eq!(all.len(), NavHistory::<usize>::MAX);
        assert_eq!(current, Some(0));
        assert_eq!(**all.last().expect("entries"), 3);
        assert_eq!(n.back(), Some(&(NavHistory::<usize>::MAX + 1)));
    }
}
