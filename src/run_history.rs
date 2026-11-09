//! What the user opened: the run history, a run count and the date of the
//! last run for each file or folder opened from goz-gui.
//!
//! Pure logic, no Win32. The entries are the rows of Everything's `Run
//! History.csv` (`Filename,Run Count,Last Run Date`), which
//! `crate::run_history_file` reads and writes. A file is one entry whatever
//! the case it is spelt in, as Windows names files. The rules that keep two
//! goz-gui windows from losing each other's runs when both write the file
//! ([`merge`], [`Forgotten`]: the higher count, the later date, a removal
//! made here stays removed) and the days kept ([`prune`]) are the search
//! history's (`crate::history`). [`best_row`] is what Enter in the search
//! box picks among the results held: the one run most (or last).

use std::cmp::Reverse;
use std::collections::HashMap;

use crate::history::Keep;

/// One file or folder opened, as `Run History.csv` keeps it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Ran {
    /// Its full path.
    pub filename: String,
    /// How many times it was opened.
    pub count: u32,
    /// When it was last opened, as a FILETIME (100 ns ticks since 1601,
    /// UTC), which is how the CSV writes it.
    pub last: u64,
}

/// The key a file is known by: its path, any case.
pub fn fold(filename: &str) -> String {
    filename.to_lowercase()
}

/// The runs, newest first, each file once.
#[derive(Debug, Default)]
pub struct RunHistory {
    entries: Vec<Ran>,
    /// Each entry's place in `entries`, by its [`fold`]ed path.
    index: HashMap<String, usize>,
}

impl RunHistory {
    /// How far files run here grow the history: past it, each new one
    /// pushes the oldest out of this window's list (a history gone wild).
    /// Everything keeps every run, and so does the file: a history read or
    /// merged from it is never cut, and the next write merges any file
    /// pushed out here back from the file.
    pub const MAX: usize = 50_000;

    /// A history of `entries` in any order (see [`tidy`]).
    pub fn from_entries(entries: Vec<Ran>) -> RunHistory {
        let mut h = RunHistory::default();
        h.replace(entries);
        h
    }

    pub fn entries(&self) -> &[Ran] {
        &self.entries
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// The entry of `filename` (any case).
    #[cfg(test)]
    pub fn get(&self, filename: &str) -> Option<&Ran> {
        self.find_folded(&fold(filename))
    }

    /// The entry whose [`fold`]ed path is `key`.
    fn find_folded(&self, key: &str) -> Option<&Ran> {
        self.index.get(key).map(|&i| &self.entries[i])
    }

    /// Notes that each of `files` was opened at `now`: one more run each,
    /// dated `now` (never earlier than the date it had), first in the
    /// history. An empty path is not kept. False when nothing was noted.
    pub fn record<'a>(&mut self, files: impl IntoIterator<Item = &'a str>, now: u64) -> bool {
        // Never shorter than it was: a longer history read from the file
        // stays whole.
        let room = self.entries.len().max(Self::MAX);
        let mut any = false;
        for file in files {
            if file.is_empty() {
                continue;
            }
            any = true;
            let key = fold(file);
            match self.index.get(&key) {
                Some(&i) => {
                    let e = &mut self.entries[i];
                    e.count = e.count.saturating_add(1);
                    e.last = e.last.max(now);
                    // Spelt as it was opened last.
                    file.clone_into(&mut e.filename);
                }
                None => {
                    self.index.insert(key, self.entries.len());
                    self.entries.push(Ran {
                        filename: file.to_owned(),
                        count: 1,
                        last: now,
                    });
                }
            }
        }
        if any {
            let mut entries = tidy(core::mem::take(&mut self.entries));
            entries.truncate(room);
            self.entries = entries;
            self.reindex();
        }
        any
    }

    /// The run count of `filename` (any case); 0 when it was never run.
    pub fn count(&self, filename: &str) -> u32 {
        self.find_folded(&fold(filename)).map_or(0, |e| e.count)
    }

    /// Sets the run count of `filename` (any case) to `count`, more than 0
    /// (Set Run Count...): its date stays, or is `now` for a file never
    /// run. Returns the date of the entry it changed, if there was one.
    pub fn set_count(&mut self, filename: &str, count: u32, now: u64) -> Option<u64> {
        if filename.is_empty() || count == 0 {
            return None;
        }
        let key = fold(filename);
        if let Some(&i) = self.index.get(&key) {
            let e = &mut self.entries[i];
            e.count = count;
            return Some(e.last);
        }
        self.entries.push(Ran {
            filename: filename.to_owned(),
            count,
            last: now,
        });
        let entries = core::mem::take(&mut self.entries);
        self.replace(entries);
        None
    }

    /// Forgets `filename` (any case).
    pub fn remove(&mut self, filename: &str) -> Option<Ran> {
        let i = self.index.remove(&fold(filename))?;
        let gone = self.entries.remove(i);
        self.reindex();
        Some(gone)
    }

    /// Takes `entries` (in any order) as the history.
    pub fn replace(&mut self, entries: Vec<Ran>) {
        self.entries = tidy(entries);
        self.reindex();
    }

    fn reindex(&mut self) {
        self.index.clear();
        self.index.reserve(self.entries.len());
        for (i, e) in self.entries.iter().enumerate() {
            self.index.insert(fold(&e.filename), i);
        }
    }
}

/// `entries` newest first, each file once (any case: its highest count,
/// its latest date, and the spelling of its latest run), no empty path and
/// no run count of 0 (Everything's "no run count"). None is left out for
/// being many: this is what the file is read and merged with.
pub fn tidy(entries: Vec<Ran>) -> Vec<Ran> {
    let mut out: Vec<Ran> = Vec::with_capacity(entries.len());
    let mut at: HashMap<String, usize> = HashMap::with_capacity(entries.len());
    for e in entries {
        if e.filename.is_empty() || e.count == 0 {
            continue;
        }
        let key = fold(&e.filename);
        match at.get(&key) {
            Some(&i) => {
                let kept = &mut out[i];
                kept.count = kept.count.max(e.count);
                if e.last > kept.last {
                    kept.last = e.last;
                    kept.filename = e.filename;
                }
            }
            None => {
                at.insert(key, out.len());
                out.push(e);
            }
        }
    }
    // Stable: files run at the same moment keep their order.
    out.sort_by_key(|e| Reverse(e.last));
    out
}

/// The files removed from the run history in this process since the file
/// was last written, so that a write merging the file's entries in does
/// not bring them back (the Run History dialog's Remove and Remove All).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Forgotten {
    /// Each file removed (by its [`fold`]ed path), with when.
    files: HashMap<String, u64>,
    /// Remove All: every file last run up to then.
    all_until: Option<u64>,
}

impl Forgotten {
    pub fn forget(&mut self, filename: &str, now: u64) {
        self.files.insert(fold(filename), now);
    }

    pub fn forget_all(&mut self, now: u64) {
        self.files.clear();
        self.all_until = Some(now);
    }

    #[cfg(test)]
    pub fn is_empty(&self) -> bool {
        self.files.is_empty() && self.all_until.is_none()
    }

    /// Whether `e`, found in the file, is one removed here. A file run again
    /// after it was removed (in another goz-gui) is not.
    pub fn covers(&self, e: &Ran) -> bool {
        self.all_until.is_some_and(|t| e.last <= t)
            || self
                .files
                .get(&fold(&e.filename))
                .is_some_and(|&t| e.last <= t)
    }

    /// `written` went out to the file: those removals are done, unless the
    /// same file was removed again since.
    pub fn settle(&mut self, written: &Forgotten) {
        for (file, t) in &written.files {
            if self.files.get(file) == Some(t) {
                self.files.remove(file);
            }
        }
        if self.all_until == written.all_until {
            self.all_until = None;
        }
    }
}

/// The run history after merging `theirs` (the file, which another goz-gui
/// may have written) into `mine` (this process's): every file of either,
/// with the higher count and the later date, newest first. The entries of
/// theirs that this process removed stay removed; its own stay.
pub fn merge(mine: &[Ran], theirs: &[Ran], forgotten: &Forgotten) -> Vec<Ran> {
    let mut all: Vec<Ran> = mine.to_vec();
    all.extend(theirs.iter().filter(|e| !forgotten.covers(e)).cloned());
    tidy(all)
}

/// Leaves out the files last run more than `keep` days before `now`
/// (Everything's `run_history_keep_forever` and `run_history_days_to_keep`).
pub fn prune(entries: &mut Vec<Ran>, keep: Keep, now: u64) {
    /// A day in FILETIME ticks.
    const DAY: u64 = 864_000_000_000;
    if let Some(days) = keep {
        let since = now.saturating_sub(u64::from(days) * DAY);
        entries.retain(|e| e.last >= since);
    }
}

/// Which held result Enter in the search box picks
/// (`search_edit_enter_action`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pick {
    /// The one run most; a tie goes to the one run last.
    MostRun,
    /// The one run last; a tie goes to the one run most.
    LastRun,
}

/// Among `rows` (each a row number and the result's full path, UTF-16 as
/// the list holds it), the one `pick` chooses by its run history; a tie
/// goes to the row nearest the top. `None` when no row was ever run.
pub fn best_row<'a>(
    history: &RunHistory,
    rows: impl IntoIterator<Item = (usize, &'a [u16])>,
    pick: Pick,
) -> Option<usize> {
    if history.is_empty() {
        return None;
    }
    let mut best: Option<(u64, u64, Reverse<usize>)> = None;
    for (row, path) in rows {
        // A path that is not valid UTF-16 was never written to the file.
        let Ok(path) = String::from_utf16(path) else {
            continue;
        };
        let Some(e) = history.find_folded(&fold(&path)) else {
            continue;
        };
        let (count, last) = (u64::from(e.count), e.last);
        let key = match pick {
            Pick::MostRun => (count, last, Reverse(row)),
            Pick::LastRun => (last, count, Reverse(row)),
        };
        if best.is_none_or(|b| key > b) {
            best = Some(key);
        }
    }
    best.map(|(_, _, Reverse(row))| row)
}

/// A path's name (after the last `\`) and the folder holding it (before
/// it), as the Run History dialog's Name and Path columns show them. A
/// volume root (`C:\`) is all name.
pub fn split(filename: &str) -> (&str, &str) {
    match filename.rfind('\\') {
        Some(i) if i + 1 < filename.len() => (&filename[i + 1..], &filename[..i]),
        _ => (filename, ""),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn set_count_keeps_the_date_or_dates_a_new_file() {
        let mut h = RunHistory::from_entries(vec![Ran {
            filename: r"C:\a.txt".into(),
            count: 5,
            last: 100,
        }]);
        assert_eq!(h.set_count(r"c:\A.TXT", 2, 900), Some(100));
        assert_eq!(h.count(r"C:\a.txt"), 2);
        assert_eq!(h.get(r"C:\a.txt").map(|e| e.last), Some(100));
        assert_eq!(h.set_count(r"C:\b.txt", 7, 900), None);
        assert_eq!(
            h.get(r"C:\b.txt").map(|e| (e.count, e.last)),
            Some((7, 900))
        );
        assert_eq!(h.entries()[0].filename, r"C:\b.txt", "newest first");
        // 0 is a removal, which the caller does; an empty path is nothing.
        assert_eq!(h.set_count(r"C:\a.txt", 0, 900), None);
        assert_eq!(h.count(r"C:\a.txt"), 2);
        assert_eq!(h.set_count("", 3, 900), None);
        assert_eq!(h.entries().len(), 2);
        assert_eq!(h.count(r"C:\never.txt"), 0);
    }

    fn r(filename: &str, count: u32, last: u64) -> Ran {
        Ran {
            filename: filename.into(),
            count,
            last,
        }
    }

    fn files(h: &RunHistory) -> Vec<(&str, u32, u64)> {
        h.entries()
            .iter()
            .map(|e| (e.filename.as_str(), e.count, e.last))
            .collect()
    }

    #[test]
    fn a_run_counts_and_moves_to_the_front() {
        let mut h = RunHistory::default();
        assert!(h.is_empty());
        assert!(h.record([r"C:\a.exe"], 10));
        assert!(h.record([r"C:\b.txt"], 20));
        assert_eq!(files(&h), [(r"C:\b.txt", 1, 20), (r"C:\a.exe", 1, 10)]);
        // Another case is the same file, spelt as it was opened last.
        assert!(h.record([r"c:\A.EXE"], 30));
        assert_eq!(files(&h), [(r"c:\A.EXE", 2, 30), (r"C:\b.txt", 1, 20)]);
        assert_eq!(h.get(r"C:\a.exe").map(|e| e.count), Some(2));
        // Several at once, as opening a selection does.
        assert!(h.record([r"C:\b.txt", r"C:\new.pdf"], 40));
        assert_eq!(
            files(&h),
            [
                (r"C:\b.txt", 2, 40),
                (r"C:\new.pdf", 1, 40),
                (r"c:\A.EXE", 2, 30)
            ]
        );
        // A clock set back never makes a date older; an empty path is no run.
        assert!(h.record([r"C:\b.txt"], 5));
        assert_eq!(h.get(r"C:\B.TXT").map(|e| (e.count, e.last)), Some((3, 40)));
        assert!(!h.record([""], 50));
        assert!(!h.record(std::iter::empty(), 50));
    }

    #[test]
    fn entries_can_be_removed_and_replaced() {
        let mut h = RunHistory::from_entries(vec![r("x", 1, 1), r("y", 2, 2), r("z", 3, 3)]);
        assert_eq!(h.remove("Y").map(|e| e.count), Some(2));
        assert_eq!(h.remove("y"), None);
        assert_eq!(files(&h), [("z", 3, 3), ("x", 1, 1)]);
        // The index follows the entries that moved up.
        assert_eq!(h.get("x").map(|e| e.last), Some(1));
        h.replace(vec![r("q", 1, 9)]);
        assert_eq!(files(&h), [("q", 1, 9)]);
        assert_eq!(h.get("z"), None);
    }

    #[test]
    fn a_history_from_a_file_is_newest_first_and_each_file_once() {
        let h = RunHistory::from_entries(vec![
            r(r"C:\old.exe", 4, 10),
            r(r"C:\new.exe", 1, 30),
            r("", 9, 99),
            r(r"C:\zero.exe", 0, 50),
            r(r"c:\OLD.exe", 2, 20),
        ]);
        assert_eq!(
            files(&h),
            [(r"C:\new.exe", 1, 30), (r"c:\OLD.exe", 4, 20)],
            "the higher count, the later date and its spelling"
        );
    }

    #[test]
    fn a_history_read_is_never_cut_and_runs_grow_it_to_the_limit() {
        let n = RunHistory::MAX;
        let many = |count: usize| -> Vec<Ran> {
            (0..count)
                .map(|i| r(&format!(r"C:\{i}"), 1, i as u64))
                .collect()
        };
        // Read (or merged) from a file longer than the limit: all of it.
        let mut h = RunHistory::from_entries(many(n + 3));
        assert_eq!(h.entries().len(), n + 3);
        assert!(h.get(r"C:\0").is_some());
        assert_eq!(
            merge(h.entries(), &many(n + 3), &Forgotten::default()).len(),
            n + 3
        );
        // A run here never makes it shorter than it was...
        assert!(h.record([r"C:\new"], n as u64 + 10));
        assert_eq!(h.entries().len(), n + 3);
        assert!(h.get(r"C:\new").is_some());
        // ...and grows a shorter one only to the limit, the oldest going.
        let mut h = RunHistory::from_entries(many(n));
        h.record([r"C:\new"], n as u64 + 10);
        assert_eq!(h.entries().len(), n);
        assert_eq!(h.get(r"C:\0"), None);
        assert!(h.get(r"C:\1").is_some() && h.get(r"C:\new").is_some());
    }

    #[test]
    fn merging_keeps_both_sides_with_the_higher_count_and_later_date() {
        let mine = [r(r"C:\a", 3, 50), r(r"C:\b", 1, 40)];
        let theirs = [r(r"C:\c", 7, 45), r(r"c:\A", 5, 20)];
        let merged = merge(&mine, &theirs, &Forgotten::default());
        assert_eq!(
            merged,
            [r(r"C:\a", 5, 50), r(r"C:\c", 7, 45), r(r"C:\b", 1, 40)]
        );
        // Merging again changes nothing.
        assert_eq!(merge(&merged, &theirs, &Forgotten::default()), merged);
        assert_eq!(merge(&merged, &merged, &Forgotten::default()), merged);
    }

    #[test]
    fn removed_files_stay_removed_until_written() {
        let mut f = Forgotten::default();
        assert!(f.is_empty());
        f.forget(r"C:\Gone.exe", 100);
        let theirs = [r(r"c:\gone.exe", 9, 90), r(r"C:\kept", 1, 80)];
        assert_eq!(merge(&[], &theirs, &f), [r(r"C:\kept", 1, 80)]);
        // Run again after the removal (in another window): it stays.
        let again = [r(r"C:\gone.exe", 10, 120)];
        assert_eq!(merge(&[], &again, &f), again);
        // Run again here: this process's entry stays, and the old count
        // from the file does not come back with it.
        assert_eq!(
            merge(&[r(r"C:\gone.exe", 1, 130)], &theirs, &f)[0],
            r(r"C:\gone.exe", 1, 130)
        );
        // Written: settled. Removed again meanwhile: still pending.
        let written = f.clone();
        f.forget(r"C:\gone.exe", 140);
        f.settle(&written);
        assert!(!f.is_empty());
        f.settle(&f.clone());
        assert!(f.is_empty());
    }

    #[test]
    fn remove_all_covers_everything_run_before_it() {
        let mut f = Forgotten::default();
        f.forget("x", 5);
        f.forget_all(100);
        let theirs = [r("a", 1, 10), r("b", 2, 100), r("c", 1, 101)];
        assert_eq!(
            merge(&[r("mine", 1, 150)], &theirs, &f),
            [r("mine", 1, 150), r("c", 1, 101)]
        );
        let written = f.clone();
        f.settle(&written);
        assert!(f.is_empty());
    }

    #[test]
    fn days_to_keep() {
        const DAY: u64 = 864_000_000_000;
        let now = 1000 * DAY;
        let mut e = vec![
            r("today", 1, now - 1),
            r("edge", 1, now - 90 * DAY),
            r("old", 1, now - 91 * DAY),
        ];
        prune(&mut e, None, now);
        assert_eq!(e.len(), 3, "kept forever");
        prune(&mut e, Some(90), now);
        assert_eq!(
            e.iter().map(|e| e.filename.as_str()).collect::<Vec<_>>(),
            ["today", "edge"]
        );
    }

    fn w(s: &str) -> Vec<u16> {
        s.encode_utf16().collect()
    }

    #[test]
    fn enter_picks_the_result_run_most_then_last_then_nearest_the_top() {
        let h = RunHistory::from_entries(vec![
            r(r"C:\a.exe", 3, 10),
            r(r"C:\b.exe", 5, 20),
            r(r"C:\c.exe", 5, 30),
            r(r"C:\d.exe", 1, 90),
            r(r"C:\not-held.exe", 99, 99),
        ]);
        let rows: Vec<(usize, Vec<u16>)> = [
            (0, r"C:\x.txt"),
            (1, r"C:\a.exe"),
            (2, r"C:\B.EXE"),
            (3, r"C:\c.exe"),
            (4, r"C:\d.exe"),
        ]
        .into_iter()
        .map(|(i, p)| (i, w(p)))
        .collect();
        let held = || rows.iter().map(|(i, p)| (*i, &p[..]));
        // b and c both ran 5 times: c ran last.
        assert_eq!(best_row(&h, held(), Pick::MostRun), Some(3));
        assert_eq!(best_row(&h, held(), Pick::LastRun), Some(4));
        // The same count and date: the row nearer the top.
        let twins = RunHistory::from_entries(vec![r(r"C:\p", 2, 7), r(r"C:\q", 2, 7)]);
        let (p, q) = (w(r"C:\p"), w(r"C:\q"));
        let rows = [(8, &q[..]), (5, &p[..])];
        assert_eq!(best_row(&twins, rows, Pick::MostRun), Some(5));
        assert_eq!(best_row(&twins, rows, Pick::LastRun), Some(5));
        // Nothing held was ever run, or nothing was ever run: none.
        let other = [(0, &w(r"C:\x.txt")[..])];
        assert_eq!(best_row(&h, other, Pick::MostRun), None);
        assert_eq!(
            best_row(&RunHistory::default(), held(), Pick::MostRun),
            None
        );
        // A path that is not valid UTF-16 is skipped.
        let lone = [0xd800u16, 0x41];
        assert_eq!(best_row(&h, [(0, &lone[..])], Pick::MostRun), None);
    }

    #[test]
    fn names_and_folders() {
        assert_eq!(
            split(r"C:\Windows\notepad.exe"),
            ("notepad.exe", r"C:\Windows")
        );
        assert_eq!(split(r"C:\a.txt"), ("a.txt", "C:"));
        assert_eq!(split(r"C:\"), (r"C:\", ""));
        assert_eq!(split(r"\\srv\share\f"), ("f", r"\\srv\share"));
        assert_eq!(split("plain"), ("plain", ""));
    }
}
