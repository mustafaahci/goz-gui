//! What the user searched: the search history the search box's dropdown
//! lists, and the navigation history History > Back and Forward walk.
//!
//! Pure logic, no Win32. The search history's entries are the rows of
//! Everything's `Search History.csv` (`Search,Search Count,Last Search
//! Date`), which `crate::history_file` reads and writes. What it shares with
//! the run history (`crate::run_history`, the rows of `Run History.csv`) is
//! here too: an entry of either ([`Entry`]), a list of them each once and
//! newest first ([`tidy`]), the days kept ([`prune`]), a lookup that keeps
//! no copy of the keys ([`Index`]), and how goz-gui windows on one data
//! folder share a file ([`Op`], [`apply`], [`Journal`]).
//!
//! **Several windows.** A window never writes out the list it holds. It
//! notes each change it makes ([`Op`]: a search made or a file run, with
//! the entry as it then has it; a deletion; Delete All; an import), and a
//! write reads the file again and makes those changes, and only those, on
//! what the file holds then ([`apply`]). So an entry the window only read
//! takes the file's state (deleted in another window, it stays deleted),
//! and one it touched takes the higher count and the later date. Once
//! written, the window takes the file as written for its own list, with the
//! changes it made since ([`Journal::settle`]).

use std::cmp::Reverse;
use std::collections::HashMap;
use std::hash::{BuildHasherDefault, Hasher};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::csv::{self, Field};

/// An entry of either history: a search, or a file run.
pub trait Entry: Clone {
    /// What it is known by: the search, or the file's path.
    fn key(&self) -> &str;
    /// The hash of `key` as the history compares keys ([`Entry::same`]).
    fn hash_key(key: &str) -> u64;
    /// Whether keys `a` and `b` are one entry's.
    fn same(a: &str, b: &str) -> bool;
    /// When it was last made, as a FILETIME.
    fn last(&self) -> u64;
    /// Whether it is kept at all (an empty search or path is not).
    fn valid(&self) -> bool;
    /// `other`, the same entry, seen again: the higher count and the later
    /// date are kept. Whether that changed it.
    fn absorb(&mut self, other: &Self) -> bool;
}

/// FNV-1a (64 bits) of `bytes`: the hash an [`Index`] keeps of a key.
pub fn fnv1a(bytes: impl IntoIterator<Item = u8>) -> u64 {
    bytes.into_iter().fold(0xcbf2_9ce4_8422_2325, |h, b| {
        (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3)
    })
}

/// The [`Index`]'s map takes the hashes it holds as hashed already, only
/// spread over 64 bits.
#[derive(Default)]
pub struct Prehashed(u64);

impl Hasher for Prehashed {
    fn finish(&self) -> u64 {
        self.0
    }

    fn write(&mut self, bytes: &[u8]) {
        self.0 = fnv1a(bytes.iter().copied());
    }

    fn write_u32(&mut self, n: u32) {
        self.0 = u64::from(n).wrapping_mul(0x9e37_79b9_7f4a_7c15);
    }
}

/// Where each entry of a list is, by the hash of its key: two numbers per
/// entry (8 bytes) and no copy of the key. Two keys of one hash (rare) are
/// told apart by their text: the first is found at once, the other by
/// looking through the list.
#[derive(Debug, Default)]
pub struct Index {
    at: HashMap<u32, u32, BuildHasherDefault<Prehashed>>,
}

/// The 32 bits of `key`'s hash an [`Index`] keeps.
fn slot<E: Entry>(key: &str) -> u32 {
    let h = E::hash_key(key);
    (h ^ (h >> 32)) as u32
}

impl Index {
    /// The index of `entries`.
    pub fn of<E: Entry>(entries: &[E]) -> Index {
        let mut index = Index::default();
        index.rebuild(entries);
        index
    }

    /// Indexes `entries` afresh (its room is kept).
    pub fn rebuild<E: Entry>(&mut self, entries: &[E]) {
        self.at.clear();
        self.at.reserve(entries.len());
        for (i, e) in entries.iter().enumerate() {
            self.add::<E>(e.key(), i);
        }
    }

    /// Entry `i` of the list (one added at its end) is known by `key`.
    pub fn add<E: Entry>(&mut self, key: &str, i: usize) {
        self.at
            .entry(slot::<E>(key))
            .or_insert(u32::try_from(i).unwrap_or(u32::MAX));
    }

    /// Where the entry known by `key` is in `entries`, the list indexed.
    pub fn find<E: Entry>(&self, entries: &[E], key: &str) -> Option<usize> {
        let i = *self.at.get(&slot::<E>(key))? as usize;
        if entries.get(i).is_some_and(|e| E::same(e.key(), key)) {
            return Some(i);
        }
        // Another key has the same hash.
        entries.iter().position(|e| E::same(e.key(), key))
    }
}

/// `text` in a block of its own size: a value read from a file is built a
/// character at a time, in a block twice the size it may need, among the
/// blocks of the parse that are freed at once. An entry kept for the
/// process's life is better in a block that fits it.
pub fn exact(text: String) -> String {
    if text.capacity() == text.len() {
        text
    } else {
        text.as_str().to_owned()
    }
}

/// About how long the CSV text of `entries` is (each key, quoted, with a
/// count and a FILETIME): what to make room for at once.
pub fn csv_size<E: Entry>(entries: &[E]) -> usize {
    64 + entries.iter().map(|e| e.key().len() + 32).sum::<usize>()
}

/// `entries` newest first, each once (its highest count and latest date),
/// none that is not [`Entry::valid`]. None is left out for being many:
/// this is what a file is read with.
pub fn tidy<E: Entry>(mut entries: Vec<E>) -> Vec<E> {
    entries.retain(E::valid);
    // In place: a list read from a file is a few hundred kilobytes, and
    // most often tidy already.
    let mut index = Index::default();
    index.at.reserve(entries.len());
    let mut again = vec![false; entries.len()];
    for (j, again) in again.iter_mut().enumerate() {
        let (before, rest) = entries.split_at_mut(j);
        let e = &rest[0];
        // Only the first of each key is indexed, so it is the one found.
        match index.find(before, e.key()) {
            Some(i) => {
                before[i].absorb(e);
                *again = true;
            }
            None => index.add::<E>(e.key(), j),
        }
    }
    if again.contains(&true) {
        let mut seen = again.into_iter();
        entries.retain(|_| !seen.next().unwrap_or(false));
    }
    newest_first(&mut entries);
    // Kept as long as the process runs: no room to spare.
    entries.shrink_to_fit();
    entries
}

/// Makes room in `entries` for one more, a little at a time: a history
/// read from a file has no room to spare, and doubling it for one search
/// would keep room for thousands.
pub fn grow<E>(entries: &mut Vec<E>) {
    if entries.len() == entries.capacity() {
        entries.reserve_exact(64);
    }
}

/// Sorts `entries` newest first, unless they are. Stable: entries made at
/// the same moment keep their order.
fn newest_first<E: Entry>(entries: &mut [E]) {
    if !entries.is_sorted_by_key(|e| Reverse(e.last())) {
        entries.sort_by_key(|e| Reverse(e.last()));
    }
}

/// Appends a history file's row to `out`, as Everything writes it: the
/// key quoted, the count and the date bare, then the values of the columns
/// it does not know (`extra`, when the file has them), CRLF.
pub fn write_row(out: &mut String, key: &str, count: u32, last: u64, extra: Option<&[Field]>) {
    use std::fmt::Write;
    out.push('"');
    for (i, part) in key.split('"').enumerate() {
        if i > 0 {
            out.push_str("\"\"");
        }
        out.push_str(part);
    }
    let _ = write!(out, "\",{count},{last}");
    match extra {
        Some(values) if !values.is_empty() => {
            out.push(',');
            csv::write_fields(values.iter(), out);
        }
        _ => out.push_str("\r\n"),
    }
}

/// One change made to a history in this process, which a write makes again
/// on the file as it is then.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Op<E> {
    /// A search made or a file run (the entry as this process then had
    /// it), or an entry imported: the file's entry takes the higher count
    /// and the later date; one the file lacks is added.
    Put(E),
    /// The entry known by `key` was deleted at `at` (see [`deleted_at`]):
    /// gone, unless it was made again after that (in another goz-gui).
    Delete { key: String, at: u64 },
    /// Delete All at `at`: every entry last made by then is gone.
    Clear { at: u64 },
}

/// Makes `ops`, in order, on `entries` (each entry once, newest first),
/// which stay so. Whether that changed them.
pub fn apply<E: Entry>(entries: &mut Vec<E>, ops: &[Op<E>]) -> bool {
    if ops.is_empty() {
        return false;
    }
    let mut index = Index::of(entries);
    let mut gone = vec![false; entries.len()];
    let mut changed = false;
    for op in ops {
        match op {
            Op::Put(e) if e.valid() => match index.find(entries, e.key()) {
                // Deleted, then made again: as made, without the old count.
                Some(i) if gone[i] => {
                    entries[i] = e.clone();
                    gone[i] = false;
                    changed = true;
                }
                Some(i) => changed |= entries[i].absorb(e),
                None => {
                    index.add::<E>(e.key(), entries.len());
                    entries.push(e.clone());
                    gone.push(false);
                    changed = true;
                }
            },
            Op::Put(_) => {}
            Op::Delete { key, at } => {
                if let Some(i) = index.find(entries, key)
                    && entries[i].last() <= *at
                {
                    gone[i] = true;
                }
            }
            Op::Clear { at } => {
                for (e, gone) in entries.iter().zip(gone.iter_mut()) {
                    *gone |= e.last() <= *at;
                }
            }
        }
    }
    if gone.contains(&true) {
        changed = true;
        let mut gone = gone.into_iter();
        entries.retain(|_| !gone.next().unwrap_or(false));
    }
    newest_first(entries);
    changed
}

/// The changes made to a history in this process that its file has not
/// taken in, or that the window has not yet seen written: what a write
/// makes on the file ([`unwritten`](Journal::unwritten)), and what the
/// window makes again on the file as written ([`settle`](Journal::settle)).
/// Each change has a number, counted from 0 at the start.
#[derive(Debug)]
pub struct Journal<E> {
    /// The changes not settled, oldest first; the first is number `first`.
    ops: Vec<Op<E>>,
    first: u64,
    /// Every change numbered below this is in the file.
    written: u64,
}

impl<E> Default for Journal<E> {
    fn default() -> Self {
        Journal {
            ops: Vec::new(),
            first: 0,
            written: 0,
        }
    }
}

impl<E: Clone> Journal<E> {
    /// Notes a change made here.
    pub fn log(&mut self, op: Op<E>) {
        self.ops.push(op);
    }

    /// The number the next change will have.
    fn next(&self) -> u64 {
        self.first + self.ops.len() as u64
    }

    /// Changes are waiting to go out to the file.
    pub fn pending(&self) -> bool {
        self.next() > self.written
    }

    /// The changes a write makes on the file, and the number after the last
    /// of them (for [`wrote`](Journal::wrote) and
    /// [`settle`](Journal::settle)).
    pub fn unwritten(&self) -> (Vec<Op<E>>, u64) {
        let from = usize::try_from(self.written.saturating_sub(self.first)).unwrap_or(usize::MAX);
        let ops = self.ops.get(from..).unwrap_or_default().to_vec();
        (ops, self.next())
    }

    /// The changes numbered below `through` are in the file.
    pub fn wrote(&mut self, through: u64) {
        self.written = self.written.max(through);
    }

    /// The window takes in the file as written with the changes numbered
    /// below `through`: those are done with. The later ones are returned,
    /// for the window to make again on what was written.
    pub fn settle(&mut self, through: u64) -> Vec<Op<E>> {
        let done = usize::try_from(through.saturating_sub(self.first))
            .unwrap_or(usize::MAX)
            .min(self.ops.len());
        self.ops.drain(..done);
        self.first += done as u64;
        self.ops.clone()
    }
}

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

/// A search is known by its text exactly: the same text in another case is
/// another search, as in Everything.
impl Entry for Searched {
    fn key(&self) -> &str {
        &self.search
    }

    fn hash_key(key: &str) -> u64 {
        fnv1a(key.bytes())
    }

    fn same(a: &str, b: &str) -> bool {
        a == b
    }

    fn last(&self) -> u64 {
        self.last
    }

    fn valid(&self) -> bool {
        !self.search.is_empty()
    }

    fn absorb(&mut self, other: &Self) -> bool {
        let was = (self.count, self.last);
        self.count = self.count.max(other.count);
        self.last = self.last.max(other.last);
        (self.count, self.last) != was
    }
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
    /// search, and so does the file: a history read from it is never cut,
    /// and a search pushed out here stays in the file (a write only makes
    /// this window's changes on it).
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
    /// its count raised. Returns it as it now is; `None` for an empty
    /// search, which is not kept.
    pub fn record(&mut self, search: &str, now: u64) -> Option<Searched> {
        if search.is_empty() {
            return None;
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
        let made = entry.clone();
        grow(&mut self.entries);
        self.entries.insert(0, entry);
        self.entries.truncate(room);
        Some(made)
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

    /// Makes `ops` on the history (an import).
    pub fn apply(&mut self, ops: &[Op<Searched>]) {
        apply(&mut self.entries, ops);
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

/// When a deletion made `now` is dated: never before the entry it deleted
/// (`last`, its date, or the newest date for Delete All), so an entry
/// dated in the future (a clock that was ahead, a file from such a
/// machine) is covered too. Shared with the run history.
pub fn deleted_at(last: Option<u64>, now: u64) -> u64 {
    last.map_or(now, |last| last.max(now))
}

/// How long entries are kept: `None` forever, else that many days
/// (Everything's `search_history_keep_forever` and
/// `search_history_days_to_keep`, and the run history's `run_history_*`).
pub type Keep = Option<u32>;

/// Leaves out the entries last made more than `keep` days before `now`.
pub fn prune<E: Entry>(entries: &mut Vec<E>, keep: Keep, now: u64) {
    /// A day in FILETIME ticks.
    const DAY: u64 = 864_000_000_000;
    if let Some(days) = keep {
        let since = now.saturating_sub(u64::from(days) * DAY);
        entries.retain(|e| e.last() >= since);
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
        assert_eq!(h.record("foo", 10), Some(s("foo", 1, 10)));
        assert!(h.record("bar", 20).is_some());
        assert_eq!(searches(&h), [("bar", 1, 20), ("foo", 1, 10)]);
        assert_eq!(h.record("foo", 30), Some(s("foo", 2, 30)));
        assert_eq!(searches(&h), [("foo", 2, 30), ("bar", 1, 20)]);
        // The same text in another case is another search.
        assert!(h.record("Foo", 40).is_some());
        assert_eq!(h.entries().len(), 3);
    }

    #[test]
    fn empty_searches_are_not_kept() {
        let mut h = SearchHistory::default();
        assert_eq!(h.record("", 1), None);
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
        // Read from a file longer than the limit: all of it.
        let mut h = SearchHistory::from_entries(many(n + 5));
        assert_eq!(h.entries().len(), n + 5);
        assert_eq!(h.entries().last().map(|e| e.search.as_str()), Some("0"));
        // Recording here never makes it shorter than it was...
        h.record("new", n as u64 + 10);
        assert_eq!(h.entries().len(), n + 5);
        assert_eq!(h.entries()[0].search, "new");
        // ...and grows a shorter one only to the limit.
        let mut h = SearchHistory::from_entries(many(n));
        let made = h.record("new", n as u64 + 10).expect("recorded");
        assert_eq!(h.entries().len(), n);
        assert_eq!(h.entries().last().map(|e| e.search.as_str()), Some("1"));
        // The search pushed out stays in the file: a write only adds "new".
        let mut file = many(n);
        apply(&mut file, &[Op::Put(made)]);
        assert_eq!(file.len(), n + 1);
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

    /// A goz-gui process on a search history file (`file`, as the disk
    /// holds it): the list it shows and the changes it made.
    struct Window {
        view: SearchHistory,
        journal: Journal<Searched>,
    }

    impl Window {
        fn open(file: &[Searched]) -> Window {
            Window {
                view: SearchHistory::from_entries(file.to_vec()),
                journal: Journal::default(),
            }
        }

        fn search(&mut self, search: &str, now: u64) {
            let made = self.view.record(search, now).expect("recorded");
            self.journal.log(Op::Put(made));
        }

        fn delete(&mut self, search: &str, now: u64) {
            let gone = self.view.remove_search(search).map(|e| e.last);
            self.journal.log(Op::Delete {
                key: search.into(),
                at: deleted_at(gone, now),
            });
        }

        fn delete_all(&mut self, now: u64) {
            let newest = self.view.entries().iter().map(|e| e.last).max();
            self.view.replace(Vec::new());
            self.journal.log(Op::Clear {
                at: deleted_at(newest, now),
            });
        }

        fn import(&mut self, entries: Vec<Searched>) {
            let ops: Vec<Op<Searched>> = tidy(entries).into_iter().map(Op::Put).collect();
            self.view.apply(&ops);
            for op in ops {
                self.journal.log(op);
            }
        }

        /// A write: the file read again, this process's changes made on it,
        /// and the window takes in what was written.
        fn save(&mut self, file: &mut Vec<Searched>) {
            if !self.journal.pending() {
                return;
            }
            let (ops, through) = self.journal.unwritten();
            apply(file, &ops);
            self.journal.wrote(through);
            let later = self.journal.settle(through);
            let mut written = file.clone();
            apply(&mut written, &later);
            self.view.replace(written);
        }
    }

    #[test]
    fn a_search_deleted_in_one_window_stays_deleted_when_another_writes() {
        for a_first in [true, false] {
            let mut file = vec![s("foo", 3, 10), s("bar", 1, 20)];
            let mut a = Window::open(&file);
            let mut b = Window::open(&file);
            a.delete("foo", 100);
            b.search("baz", 110);
            if a_first {
                a.save(&mut file);
                b.save(&mut file);
            } else {
                b.save(&mut file);
                a.save(&mut file);
            }
            assert_eq!(file, [s("baz", 1, 110), s("bar", 1, 20)], "{a_first}");
            // B, which still showed it, shows the file once it wrote.
            if a_first {
                assert_eq!(b.view.entries(), file);
            }
        }
        // A window that changed nothing writes nothing.
        let mut file = vec![s("foo", 3, 10)];
        let mut idle = Window::open(&file);
        let mut a = Window::open(&file);
        a.delete("foo", 100);
        a.save(&mut file);
        idle.save(&mut file);
        assert!(file.is_empty());
    }

    #[test]
    fn two_windows_counting_one_search_keep_the_higher_count_and_later_date() {
        let mut file = vec![s("foo", 3, 10), s("bar", 1, 5)];
        let mut a = Window::open(&file);
        let mut b = Window::open(&file);
        a.search("foo", 100);
        b.search("foo", 120);
        b.search("bar", 130);
        a.save(&mut file);
        b.save(&mut file);
        assert_eq!(file, [s("bar", 2, 130), s("foo", 4, 120)]);
        // A, which saw only its own, counts on from what it wrote.
        a.search("foo", 140);
        a.save(&mut file);
        assert_eq!(file, [s("foo", 5, 140), s("bar", 2, 130)]);
        assert_eq!(a.view.entries(), file);
    }

    #[test]
    fn a_search_deleted_here_and_made_again_comes_back_without_its_old_count() {
        let mut file = vec![s("gone", 9, 90), s("kept", 1, 80)];
        let mut a = Window::open(&file);
        a.delete("gone", 100);
        a.search("gone", 130);
        a.save(&mut file);
        assert_eq!(file, [s("gone", 1, 130), s("kept", 1, 80)]);
        // Searched again in another window after the deletion: it stays.
        let mut file = vec![s("gone", 10, 120)];
        let mut a = Window::open(&[s("gone", 9, 90)]);
        a.delete("gone", 100);
        a.save(&mut file);
        assert_eq!(file, [s("gone", 10, 120)]);
    }

    #[test]
    fn delete_all_keeps_what_another_window_searches_after_it() {
        for a_first in [true, false] {
            let mut file = vec![s("a", 1, 10), s("b", 2, 20)];
            let mut a = Window::open(&file);
            let mut b = Window::open(&file);
            a.delete_all(100);
            b.search("new", 150);
            if a_first {
                a.save(&mut file);
                b.save(&mut file);
            } else {
                b.save(&mut file);
                a.save(&mut file);
            }
            assert_eq!(file, [s("new", 1, 150)], "{a_first}");
        }
        // A window that searched after its own Delete All keeps that.
        let mut file = vec![s("a", 1, 10)];
        let mut a = Window::open(&file);
        a.delete_all(100);
        a.search("mine", 150);
        a.save(&mut file);
        assert_eq!(file, [s("mine", 1, 150)]);
    }

    #[test]
    fn an_import_merges_and_brings_back_what_was_deleted() {
        let mut file = vec![s("foo", 3, 10), s("bar", 2, 50)];
        let mut a = Window::open(&file);
        let mut b = Window::open(&file);
        a.delete("foo", 100);
        a.save(&mut file);
        b.import(vec![s("foo", 9, 5), s("bar", 7, 10), s("new", 1, 60)]);
        assert_eq!(
            searches(&b.view),
            [("new", 1, 60), ("bar", 7, 50), ("foo", 9, 10)]
        );
        b.save(&mut file);
        // The higher count and the later date; imported on purpose, a
        // search deleted earlier comes back.
        assert_eq!(file, [s("new", 1, 60), s("bar", 7, 50), s("foo", 9, 5)]);
    }

    #[test]
    fn a_search_dated_in_the_future_can_be_deleted() {
        let now = 1000;
        let theirs = [s("ahead", 1, now + 500), s("normal", 1, now - 10)];
        // Dated by the entry deleted, not by the clock.
        assert_eq!(deleted_at(Some(now + 500), now), now + 500);
        assert_eq!(deleted_at(Some(now - 10), now), now);
        assert_eq!(deleted_at(None, now), now);
        let mut file = theirs.to_vec();
        let mut a = Window::open(&file);
        a.delete("ahead", now);
        a.delete("normal", now);
        a.save(&mut file);
        assert!(file.is_empty());
        // Delete All dated by the newest search.
        let mut file = theirs.to_vec();
        let mut a = Window::open(&file);
        a.delete_all(now);
        a.save(&mut file);
        assert!(file.is_empty());
    }

    #[test]
    fn the_journal_numbers_changes_until_written_and_settled() {
        let mut j: Journal<Searched> = Journal::default();
        assert!(!j.pending());
        assert_eq!(j.unwritten(), (vec![], 0));
        j.log(Op::Put(s("a", 1, 1)));
        j.log(Op::Clear { at: 2 });
        let (ops, through) = j.unwritten();
        assert_eq!((ops.len(), through), (2, 2));
        // A write that failed: still pending, the same changes again.
        assert!(j.pending());
        // Written; a change made while it was written is not in it.
        j.log(Op::Put(s("b", 1, 3)));
        j.wrote(through);
        assert!(j.pending());
        assert_eq!(j.unwritten(), (vec![Op::Put(s("b", 1, 3))], 3));
        j.wrote(3);
        assert!(!j.pending());
        // The window takes in the first write: the third change is not in
        // it. Then the second write: nothing is left.
        assert_eq!(j.settle(2), [Op::Put(s("b", 1, 3))]);
        assert_eq!(j.settle(3), []);
        assert_eq!(j.settle(1), [], "an older write settles nothing more");
        j.log(Op::Clear { at: 4 });
        assert_eq!(j.unwritten(), (vec![Op::Clear { at: 4 }], 4));
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

    /// An entry whose keys all have one hash.
    #[derive(Clone, Debug, PartialEq)]
    struct Clash(String, u64);

    impl Entry for Clash {
        fn key(&self) -> &str {
            &self.0
        }
        fn hash_key(_: &str) -> u64 {
            7
        }
        fn same(a: &str, b: &str) -> bool {
            a == b
        }
        fn last(&self) -> u64 {
            self.1
        }
        fn valid(&self) -> bool {
            true
        }
        fn absorb(&mut self, other: &Self) -> bool {
            let was = self.1;
            self.1 = self.1.max(other.1);
            self.1 != was
        }
    }

    #[test]
    fn keys_of_one_hash_are_told_apart_by_their_text() {
        let c = |k: &str, t| Clash(k.into(), t);
        let list = tidy(vec![c("a", 1), c("b", 2), c("a", 3), c("c", 0)]);
        assert_eq!(list, [c("a", 3), c("b", 2), c("c", 0)]);
        let index = Index::of(&list);
        assert_eq!(index.find(&list, "a"), Some(0));
        assert_eq!(index.find(&list, "b"), Some(1));
        assert_eq!(index.find(&list, "c"), Some(2));
        assert_eq!(index.find(&list, "d"), None);
        let mut list = list;
        apply(
            &mut list,
            &[
                Op::Put(c("d", 9)),
                Op::Delete {
                    key: "b".into(),
                    at: 5,
                },
                Op::Put(c("c", 4)),
            ],
        );
        assert_eq!(list, [c("d", 9), c("c", 4), c("a", 3)]);
        assert_eq!(fnv1a(*b"a"), 0xaf63_dc4c_8601_ec8c);
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
