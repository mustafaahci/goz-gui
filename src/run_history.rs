//! What the user opened: the run history, a run count and the date of the
//! last run for each file or folder opened from goz-gui.
//!
//! Pure logic, no Win32. The entries are the rows of Everything's `Run
//! History.csv` (`Filename,Run Count,Last Run Date`), which
//! `crate::run_history_file` reads and writes. A file is one entry whatever
//! the case it is spelt in, as Windows names files; each path is held once
//! (the lookup in any case keeps a hash of it, [`Index`]). How two goz-gui
//! windows share the file (each write makes only this window's changes on
//! the file as it is then: a removal made elsewhere sticks, a file run here
//! takes the higher count and the later date) and the days kept are the
//! search history's (`crate::history`). [`best_row`] is what Enter in the
//! search box picks among the results held: the one run most (or last).

use std::cmp::Reverse;

use crate::history::{self, Entry, Index, Op, fnv1a, tidy};

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

/// The hash of `filename`'s [`fold`], with no folded copy made for a path
/// all ASCII (most are).
fn fold_hash(filename: &str) -> u64 {
    if filename.is_ascii() {
        fnv1a(filename.bytes().map(|b| b.to_ascii_lowercase()))
    } else {
        fnv1a(fold(filename).into_bytes())
    }
}

/// Whether `a` and `b` are one file's paths: the same once [`fold`]ed.
pub fn same_file(a: &str, b: &str) -> bool {
    if a.is_ascii() && b.is_ascii() {
        a.eq_ignore_ascii_case(b)
    } else {
        fold(a) == fold(b)
    }
}

/// A file is known by its path in any case; seen again, it takes the higher
/// count, the later date and the spelling of its later run.
impl Entry for Ran {
    fn key(&self) -> &str {
        &self.filename
    }

    fn hash_key(key: &str) -> u64 {
        fold_hash(key)
    }

    fn same(a: &str, b: &str) -> bool {
        same_file(a, b)
    }

    fn last(&self) -> u64 {
        self.last
    }

    /// An empty path is no file, and a run count of 0 is Everything's "no
    /// run count".
    fn valid(&self) -> bool {
        !self.filename.is_empty() && self.count > 0
    }

    fn absorb(&mut self, other: &Self) -> bool {
        let count = self.count;
        self.count = self.count.max(other.count);
        if other.last > self.last {
            self.last = other.last;
            self.filename.clone_from(&other.filename);
            return true;
        }
        self.count != count
    }
}

/// The runs, newest first, each file once.
#[derive(Debug, Default)]
pub struct RunHistory {
    entries: Vec<Ran>,
    /// Where each entry is in `entries`, by its path in any case.
    index: Index,
}

impl RunHistory {
    /// How far files run here grow the history: past it, each new one
    /// pushes the oldest out of this window's list (a history gone wild).
    /// Everything keeps every run, and so does the file: a history read
    /// from it is never cut, and a file pushed out here stays in the file
    /// (a write only makes this window's changes on it).
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
    pub fn get(&self, filename: &str) -> Option<&Ran> {
        let i = self.index.find(&self.entries, filename)?;
        self.entries.get(i)
    }

    /// Notes that each of `files` was opened at `now`: one more run each,
    /// dated `now` (never earlier than the date it had), spelt as opened,
    /// first in the history in the order given. An empty path is not kept.
    /// Returns the entries noted, as they now are (none: nothing was).
    pub fn record<'a>(&mut self, files: impl IntoIterator<Item = &'a str>, now: u64) -> Vec<Ran> {
        // Never shorter than it was: a longer history read from the file
        // stays whole.
        let room = self.entries.len().max(Self::MAX);
        // The entries run, in the order given, each once.
        let mut order: Vec<usize> = Vec::new();
        let mut run: Vec<bool> = Vec::new();
        for file in files {
            if file.is_empty() {
                continue;
            }
            let i = match self.index.find(&self.entries, file) {
                Some(i) => {
                    let e = &mut self.entries[i];
                    e.count = e.count.saturating_add(1);
                    e.last = e.last.max(now);
                    // Spelt as it was opened last.
                    file.clone_into(&mut e.filename);
                    i
                }
                None => {
                    self.entries.push(Ran {
                        filename: file.to_owned(),
                        count: 1,
                        last: now,
                    });
                    let i = self.entries.len() - 1;
                    self.index.add::<Ran>(file, i);
                    i
                }
            };
            run.resize(self.entries.len(), false);
            if !run[i] {
                run[i] = true;
                order.push(i);
            }
        }
        if order.is_empty() {
            return Vec::new();
        }
        let mut slots: Vec<Option<Ran>> = core::mem::take(&mut self.entries)
            .into_iter()
            .map(Some)
            .collect();
        let mut entries = Vec::with_capacity(slots.len());
        entries.extend(order.iter().filter_map(|&i| slots[i].take()));
        entries.extend(slots.into_iter().flatten());
        entries.truncate(room);
        self.entries = entries;
        self.index.rebuild(&self.entries);
        self.entries[..order.len().min(self.entries.len())].to_vec()
    }

    /// The run count of `filename` (any case); 0 when it was never run.
    pub fn count(&self, filename: &str) -> u32 {
        self.get(filename).map_or(0, |e| e.count)
    }

    /// Sets the run count of `filename` (any case) to `count`, more than 0
    /// (Set Run Count...): its date stays, or is `now` for a file never
    /// run. Returns the entry as it now is, and the date it had if it was
    /// in the history.
    pub fn set_count(
        &mut self,
        filename: &str,
        count: u32,
        now: u64,
    ) -> Option<(Ran, Option<u64>)> {
        if filename.is_empty() || count == 0 {
            return None;
        }
        if let Some(i) = self.index.find(&self.entries, filename) {
            let e = &mut self.entries[i];
            e.count = count;
            return Some((e.clone(), Some(e.last)));
        }
        let e = Ran {
            filename: filename.to_owned(),
            count,
            last: now,
        };
        history::grow(&mut self.entries);
        self.entries.push(e.clone());
        // Stable: dated now, it goes before every older run.
        self.entries.sort_by_key(|e| Reverse(e.last));
        self.index.rebuild(&self.entries);
        Some((e, None))
    }

    /// Forgets `filename` (any case).
    pub fn remove(&mut self, filename: &str) -> Option<Ran> {
        let i = self.index.find(&self.entries, filename)?;
        let gone = self.entries.remove(i);
        self.index.rebuild(&self.entries);
        Some(gone)
    }

    /// Takes `entries` (in any order) as the history.
    pub fn replace(&mut self, entries: Vec<Ran>) {
        self.entries = tidy(entries);
        self.index.rebuild(&self.entries);
    }

    /// Makes `ops` on the history (an import).
    pub fn apply(&mut self, ops: &[Op<Ran>]) {
        history::apply(&mut self.entries, ops);
        self.index.rebuild(&self.entries);
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
        let Some(e) = history.get(&path) else {
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
    use crate::history::{Journal, deleted_at, prune};

    #[test]
    fn set_count_keeps_the_date_or_dates_a_new_file() {
        let mut h = RunHistory::from_entries(vec![Ran {
            filename: r"C:\a.txt".into(),
            count: 5,
            last: 100,
        }]);
        assert_eq!(
            h.set_count(r"c:\A.TXT", 2, 900),
            Some((r(r"C:\a.txt", 2, 100), Some(100)))
        );
        assert_eq!(h.count(r"C:\a.txt"), 2);
        assert_eq!(h.get(r"C:\a.txt").map(|e| e.last), Some(100));
        assert_eq!(
            h.set_count(r"C:\b.txt", 7, 900),
            Some((r(r"C:\b.txt", 7, 900), None))
        );
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
        assert_eq!(h.record([r"C:\a.exe"], 10), [r(r"C:\a.exe", 1, 10)]);
        assert_eq!(h.record([r"C:\b.txt"], 20).len(), 1);
        assert_eq!(files(&h), [(r"C:\b.txt", 1, 20), (r"C:\a.exe", 1, 10)]);
        // Another case is the same file, spelt as it was opened last.
        assert_eq!(h.record([r"c:\A.EXE"], 30), [r(r"c:\A.EXE", 2, 30)]);
        assert_eq!(files(&h), [(r"c:\A.EXE", 2, 30), (r"C:\b.txt", 1, 20)]);
        assert_eq!(h.get(r"C:\a.exe").map(|e| e.count), Some(2));
        // Several at once, as opening a selection does, in the order given.
        assert_eq!(
            h.record([r"C:\b.txt", r"C:\new.pdf"], 40),
            [r(r"C:\b.txt", 2, 40), r(r"C:\new.pdf", 1, 40)]
        );
        assert_eq!(
            files(&h),
            [
                (r"C:\b.txt", 2, 40),
                (r"C:\new.pdf", 1, 40),
                (r"c:\A.EXE", 2, 30)
            ]
        );
        // The same file twice counts twice and is listed once.
        assert_eq!(
            h.record([r"C:\NEW.pdf", r"C:\x", r"c:\new.PDF"], 45),
            [r(r"c:\new.PDF", 3, 45), r(r"C:\x", 1, 45)]
        );
        assert_eq!(h.entries().len(), 4);
        // A clock set back never makes a date older; an empty path is no run.
        assert_eq!(h.record([r"C:\b.txt"], 5), [r(r"C:\b.txt", 3, 40)]);
        assert_eq!(h.get(r"C:\B.TXT").map(|e| (e.count, e.last)), Some((3, 40)));
        assert!(h.record([""], 50).is_empty());
        assert!(h.record(std::iter::empty(), 50).is_empty());
    }

    #[test]
    fn a_path_is_found_in_any_case_without_a_folded_copy() {
        let h = RunHistory::from_entries(vec![
            r(r"C:\Ärger\ÖL.txt", 1, 3),
            r(r"C:\K.txt", 2, 2),
            r(r"C:\plain\File.TXT", 3, 1),
        ]);
        assert_eq!(h.count(r"c:\ärger\öl.TXT"), 1);
        assert_eq!(h.count(r"C:\PLAIN\file.txt"), 3);
        // The Kelvin sign folds to an ASCII k: one file, as `fold` has it.
        assert_eq!(fold("C:\\\u{212a}.txt"), fold(r"C:\K.txt"));
        assert_eq!(h.count("C:\\\u{212a}.txt"), 2);
        assert_eq!(h.count(r"C:\Ärger"), 0);
        assert!(same_file(r"C:\A", r"c:\a") && !same_file(r"C:\A", r"C:\B"));
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
        // Read from a file longer than the limit: all of it.
        let mut h = RunHistory::from_entries(many(n + 3));
        assert_eq!(h.entries().len(), n + 3);
        assert!(h.get(r"C:\0").is_some());
        // A run here never makes it shorter than it was...
        assert_eq!(h.record([r"C:\new"], n as u64 + 10).len(), 1);
        assert_eq!(h.entries().len(), n + 3);
        assert!(h.get(r"C:\new").is_some());
        // ...and grows a shorter one only to the limit, the oldest going.
        let mut h = RunHistory::from_entries(many(n));
        let made = h.record([r"C:\new"], n as u64 + 10);
        assert_eq!(h.entries().len(), n);
        assert_eq!(h.get(r"C:\0"), None);
        assert!(h.get(r"C:\1").is_some() && h.get(r"C:\new").is_some());
        // The file pushed out here stays in the file: a write adds "new".
        let mut file = many(n);
        history::apply(&mut file, &[Op::Put(made[0].clone())]);
        assert_eq!(file.len(), n + 1);
    }

    /// A goz-gui process on a run history file (`file`, as the disk holds
    /// it), as `crate::window::runs` drives one.
    struct Window {
        view: RunHistory,
        journal: Journal<Ran>,
    }

    impl Window {
        fn open(file: &[Ran]) -> Window {
            Window {
                view: RunHistory::from_entries(file.to_vec()),
                journal: Journal::default(),
            }
        }

        fn run(&mut self, files: &[&str], now: u64) {
            for e in self.view.record(files.iter().copied(), now) {
                self.journal.log(Op::Put(e));
            }
        }

        fn remove(&mut self, file: &str, now: u64) {
            let gone = self.view.remove(file).map(|e| e.last);
            self.journal.log(Op::Delete {
                key: file.into(),
                at: deleted_at(gone, now),
            });
        }

        fn set_count(&mut self, file: &str, count: u32, now: u64) {
            let (e, had) = self.view.set_count(file, count, now).expect("set");
            self.journal.log(Op::Delete {
                key: file.into(),
                at: deleted_at(had, now),
            });
            self.journal.log(Op::Put(e));
        }

        fn remove_all(&mut self, now: u64) {
            let newest = self.view.entries().iter().map(|e| e.last).max();
            self.view.replace(Vec::new());
            self.journal.log(Op::Clear {
                at: deleted_at(newest, now),
            });
        }

        fn import(&mut self, entries: Vec<Ran>) {
            let ops: Vec<Op<Ran>> = tidy(entries).into_iter().map(Op::Put).collect();
            self.view.apply(&ops);
            for op in ops {
                self.journal.log(op);
            }
        }

        fn save(&mut self, file: &mut Vec<Ran>) {
            let (ops, through) = self.journal.unwritten();
            history::apply(file, &ops);
            self.journal.wrote(through);
            let later = self.journal.settle(through);
            let mut written = file.clone();
            history::apply(&mut written, &later);
            self.view.replace(written);
        }
    }

    #[test]
    fn a_file_removed_in_one_window_stays_removed_when_another_writes() {
        for a_first in [true, false] {
            let mut file = vec![r(r"C:\gone.exe", 9, 90), r(r"C:\kept", 1, 80)];
            let mut a = Window::open(&file);
            let mut b = Window::open(&file);
            a.remove(r"c:\GONE.EXE", 100);
            b.run(&[r"C:\new"], 110);
            if a_first {
                a.save(&mut file);
                b.save(&mut file);
            } else {
                b.save(&mut file);
                a.save(&mut file);
            }
            assert_eq!(file, [r(r"C:\new", 1, 110), r(r"C:\kept", 1, 80)]);
            // B, which still showed it, shows the file once it wrote after
            // the removal.
            assert_eq!(b.view.get(r"C:\gone.exe").is_none(), a_first);
        }
    }

    #[test]
    fn two_windows_running_one_file_keep_the_higher_count_and_later_run() {
        let mut file = vec![r(r"C:\a", 3, 10)];
        let mut a = Window::open(&file);
        let mut b = Window::open(&file);
        a.run(&[r"C:\a"], 100);
        b.run(&[r"c:\A"], 120);
        b.save(&mut file);
        a.save(&mut file);
        assert_eq!(file, [r(r"c:\A", 4, 120)]);
        // Removed here and run again: back without the old count.
        a.remove(r"C:\a", 130);
        a.run(&[r"C:\a"], 140);
        a.save(&mut file);
        assert_eq!(file, [r(r"C:\a", 1, 140)]);
        // Run again in another window after the removal: it stays.
        let mut file = vec![r(r"C:\a", 10, 120)];
        let mut a = Window::open(&[r(r"C:\a", 9, 90)]);
        a.remove(r"C:\a", 100);
        a.save(&mut file);
        assert_eq!(file, [r(r"C:\a", 10, 120)]);
    }

    #[test]
    fn a_lower_run_count_set_here_wins_over_the_files() {
        let mut file = vec![r(r"C:\a", 9, 50)];
        let mut a = Window::open(&[r(r"C:\a", 5, 50)]);
        a.set_count(r"C:\A", 2, 100);
        a.save(&mut file);
        assert_eq!(file, [r(r"C:\a", 2, 50)]);
        // A file this window never ran: the count set, dated now.
        let mut a = Window::open(&[]);
        a.set_count(r"C:\a", 3, 200);
        a.save(&mut file);
        assert_eq!(file, [r(r"C:\a", 3, 200)]);
    }

    #[test]
    fn remove_all_keeps_what_another_window_runs_after_it() {
        for a_first in [true, false] {
            let mut file = vec![r("a", 1, 10), r("b", 2, 100)];
            let mut a = Window::open(&file);
            let mut b = Window::open(&file);
            a.remove_all(100);
            b.run(&["c"], 101);
            if a_first {
                a.save(&mut file);
                b.save(&mut file);
            } else {
                b.save(&mut file);
                a.save(&mut file);
            }
            assert_eq!(file, [r("c", 1, 101)], "{a_first}");
        }
    }

    #[test]
    fn an_import_merges_in_any_case_and_brings_back_what_was_removed() {
        let mut file = vec![r(r"C:\a", 3, 50), r(r"C:\gone", 1, 10)];
        let mut a = Window::open(&file);
        let mut b = Window::open(&file);
        a.remove(r"C:\gone", 100);
        a.save(&mut file);
        b.import(vec![r(r"c:\A", 7, 20), r(r"C:\GONE", 2, 5)]);
        b.save(&mut file);
        assert_eq!(file, [r(r"C:\a", 7, 50), r(r"C:\GONE", 2, 5)]);
        assert_eq!(b.view.entries(), file);
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
