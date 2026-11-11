//! Wave 9: `content:` searches, read by goz-gui itself (gozd never reads a
//! file's content).
//!
//! A search with content terms is planned (`plan`): gozd is asked the
//! search without them (W9 B2: with a gozd that reads the new query
//! language, the 2^n searches of goz-core's exact helper, each term taken
//! as true or false), and a [`Run`] reads the files of its answer on a
//! thread of its own, under the user's own token, with Everything's
//! encodings and modifiers (`scan`, or goz-core's own reading of each term
//! for the exact plan, as the goz CLI reads). The files whose content
//! matches are
//! the search's results, in gozd's order, as they are found: the run
//! answers the search's blocks (`client::fetch_block`) from what it has
//! found so far, says it is still reading (the blocks' `incomplete`), and
//! tells the window as it finds more, which refreshes the list and shows
//! the run's progress in the status bar. Esc or another search stops it.
//!
//! Each search's candidates are asked of gozd in one answer and held (at
//! most 1,000,000, else the search is refused with the reason) before a
//! file is read, so gozd is never kept waiting on a pipe while files are
//! read and a change to the index meanwhile neither repeats nor skips a
//! candidate. The candidates of several searches are put in the search's
//! order before they are read.

mod exact;
pub mod plan;
pub mod scan;

/// How a run reads its files: the older plan's terms in one pass
/// (`scan`), or goz-core's exact terms one at a time.
enum Reader {
    Plain(Scanner),
    Exact(Arc<goz_core::query::ContentQuery>, exact::Env),
}

impl Reader {
    fn of(plan: &Plan) -> Reader {
        match &plan.exact {
            Some((split, _, _)) => Reader::Exact(Arc::clone(split), exact::Env::new()),
            None => Reader::Plain(Scanner::new(plan.terms.clone())),
        }
    }
}

use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use windows_sys::Win32::Foundation::HWND;
use windows_sys::Win32::UI::WindowsAndMessaging::PostMessageW;

use crate::client::{
    BLOCK_ROWS, Failure, Reply, Row, RowRange, Search, Stop, Streamed, stream_range,
};
use plan::{Combine, Plan};
use scan::{Scanner, Unread};

/// Progress is told the window at most this often.
const TELL_EVERY_MS: u64 = 100;
/// The most candidates a run gathers before it reads (each is held, with
/// the searches that found it): a content term with nothing else
/// (`content:x`, `!content:x`) names every file of every volume, and is
/// refused past this with the reason rather than held.
const MANY_MAX: usize = 1_000_000;

/// What a run answers: the same key, the same run (a refresh of the search
/// keeps reading where it is).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Key {
    /// gozd's searches, as asked (the filter and folder in them).
    pub queries: Vec<String>,
    pub sort: goz_core::types::SortSpec,
    /// A file list's Date Created sort, instead of `sort`.
    pub created_sort: Option<goz_core::types::SortDir>,
    pub match_case: bool,
    /// The file list searched instead of the index, by address.
    pub list: usize,
    pub terms: Vec<scan::Term>,
    /// The exact plan's search text and case (`plan::plan_exact`).
    pub exact: Option<(String, bool)>,
}

/// A file whose content matched, or a folder the exact plan lists.
struct Found {
    path: Box<[u16]>,
    size: Option<u64>,
    mtime: Option<i64>,
    ctime: Option<i64>,
    is_dir: bool,
}

impl Found {
    fn of(row: Row) -> Found {
        Found {
            path: row.path,
            size: row.size,
            mtime: row.mtime,
            ctime: row.ctime,
            is_dir: row.is_dir,
        }
    }
}

/// Why a search gathering more than [`MANY_MAX`] candidates is refused.
fn too_many() -> String {
    format!(
        "The content search would read more than {} files: narrow it with a name, a folder or \
         an extension",
        crate::format::grouped(MANY_MAX as u64)
    )
}

/// Puts the candidates of several searches (and which searches found each,
/// `found_by`, alongside) in the search's order, as gozd or the file list
/// orders one search's rows: each search's come sorted, but one search's
/// after another's (W9R-4). The keys are goz-core's `sort_items`, the goz
/// CLI's order for the same search: the sort's value (unknown last), then
/// the folded name, then the folded path; a path sort by folder, part by
/// part, then name; Extension by gozd's groups (folders, files without
/// one, then by extension); a file list's Date Created sort (`created`) by
/// its date; the whole order reversed for a descending sort. The rows
/// carry the sort's column (`Search::want`, `content_run`).
fn sort_candidates(
    rows: &mut Vec<Row>,
    found_by: &mut Vec<u32>,
    sort: goz_core::types::SortSpec,
    created: Option<goz_core::types::SortDir>,
) {
    use goz_core::fold::fold_into;
    use goz_core::query::{extension_key, mtime_key, size_key, sort_extension};
    use goz_core::types::{SortDir, SortKey};
    let (key, dir) = match created {
        Some(dir) => (SortKey::DateCreated, dir),
        None => (sort.key, sort.dir),
    };
    /// The sort's value, then three byte strings, as `sort_items` keys.
    type SortKey4 = (u64, Vec<u8>, Vec<u8>, Vec<u8>);
    let mut keyed: Vec<(SortKey4, usize)> = rows
        .iter()
        .enumerate()
        .map(|(i, row)| {
            let path = String::from_utf16_lossy(&row.path).into_bytes();
            let mut name = Vec::new();
            fold_into(crate::filelist::matcher::name_of(&path), &mut name);
            let more = row.more.get();
            let k = match key {
                SortKey::Name => (0, Vec::new(), name, Vec::new()),
                SortKey::Path => (0, crate::filelist::path_key(&path), Vec::new(), Vec::new()),
                SortKey::Size => (size_key(row.size, dir), Vec::new(), name, Vec::new()),
                SortKey::DateModified => (mtime_key(row.mtime, dir), Vec::new(), name, Vec::new()),
                SortKey::DateCreated => {
                    (mtime_key(row.created(), dir), Vec::new(), name, Vec::new())
                }
                SortKey::DateAccessed => {
                    (mtime_key(more.atime(), dir), Vec::new(), name, Vec::new())
                }
                SortKey::DateRecentlyChanged => {
                    (mtime_key(more.rtime(), dir), Vec::new(), name, Vec::new())
                }
                SortKey::Attributes => {
                    let known = more.attrib().map(goz_core::output::attribute_sort_key);
                    (size_key(known, dir), Vec::new(), name, Vec::new())
                }
                SortKey::Extension => {
                    let ext = sort_extension(&name, row.is_dir).to_vec();
                    (extension_key(&name, row.is_dir), ext, name, Vec::new())
                }
            };
            // Ties on the name: the folded path, as `sort_items`.
            let k = match key {
                SortKey::Path => k,
                _ => {
                    let mut folded = Vec::new();
                    fold_into(&path, &mut folded);
                    (k.0, k.1, k.2, folded)
                }
            };
            (k, i)
        })
        .collect();
    keyed.sort();
    if dir == SortDir::Desc {
        keyed.reverse();
    }
    let mut taken: Vec<Option<Row>> = std::mem::take(rows).into_iter().map(Some).collect();
    let bits = std::mem::take(found_by);
    for (_, i) in keyed {
        if let Some(row) = taken[i].take() {
            rows.push(row);
            found_by.push(bits[i]);
        }
    }
}

const RUNNING: u8 = 0;
const DONE: u8 = 1;
const STOPPED: u8 = 2;
const FAILED: u8 = 3;

/// One content search, read on its own thread.
pub struct Run {
    pub key: Key,
    stop: Stop,
    found: Mutex<Vec<Found>>,
    /// Candidates in gozd's answer (the first search's), seen so far,
    /// files read, files that could not be read, files left unread as they
    /// are offline or in the cloud.
    total: AtomicU64,
    seen: AtomicU64,
    unread: AtomicU64,
    offline: AtomicU64,
    state: AtomicU8,
    failure: Mutex<Option<Failure>>,
    started: Instant,
    ended: Mutex<Option<Duration>>,
    /// Set while the window has a message about news not yet taken.
    told: AtomicBool,
    /// When the window was last told (ms from `started`).
    last_told: AtomicU64,
    hwnd: usize,
    msg: u32,
    /// Requests made of the searches' source, for tests.
    #[cfg(test)]
    asks: AtomicU64,
}

impl Run {
    /// Starts reading for `plan`, whose searches are `searches` (the plan's
    /// queries as gozd is asked them, in order); news goes to `hwnd` as
    /// `msg` (no payload).
    pub fn start(key: Key, plan: &Plan, searches: Vec<Search>, hwnd: HWND, msg: u32) -> Arc<Run> {
        let run = Arc::new(Run {
            key,
            stop: Stop::new(),
            found: Mutex::new(Vec::new()),
            total: AtomicU64::new(0),
            seen: AtomicU64::new(0),
            unread: AtomicU64::new(0),
            offline: AtomicU64::new(0),
            state: AtomicU8::new(RUNNING),
            failure: Mutex::new(None),
            started: Instant::now(),
            ended: Mutex::new(None),
            told: AtomicBool::new(false),
            last_told: AtomicU64::new(0),
            hwnd: hwnd as usize,
            msg,
            #[cfg(test)]
            asks: AtomicU64::new(0),
        });
        let thread = Arc::clone(&run);
        let reader = Reader::of(plan);
        let combine = Arc::clone(&plan.combine);
        let spawned = std::thread::Builder::new()
            .name("goz-content".into())
            .spawn(move || {
                let state = thread.read_all(&searches, &reader, &combine);
                *thread.ended.lock().unwrap_or_else(|e| e.into_inner()) =
                    Some(thread.started.elapsed());
                thread.state.store(state, Ordering::Release);
                thread.tell(true);
            });
        if spawned.is_err() {
            *run.failure.lock().unwrap_or_else(|e| e.into_inner()) =
                Some(Failure::Other("The content search could not start".into()));
            run.state.store(FAILED, Ordering::Release);
        }
        run
    }

    /// Stops reading (Esc, another search). True when it was reading.
    pub fn stop(&self) -> bool {
        self.stop.stop();
        self.running()
    }

    pub fn running(&self) -> bool {
        self.state.load(Ordering::Acquire) == RUNNING
    }

    /// Whether it ended without reading (gozd could not be asked): a
    /// refresh starts it again.
    pub fn failed(&self) -> bool {
        self.state.load(Ordering::Acquire) == FAILED
    }

    /// How many files matched so far.
    pub fn matches(&self) -> usize {
        self.found.lock().unwrap_or_else(|e| e.into_inner()).len()
    }

    /// The window took the news: the next is told again.
    pub fn told(&self) {
        self.told.store(false, Ordering::Release);
    }

    /// Tells the window there is news: at once when `now` (a match, the
    /// end), else at most every 100 ms (progress).
    fn tell(&self, now: bool) {
        let at = u64::try_from(self.started.elapsed().as_millis()).unwrap_or(u64::MAX);
        if !now && at.saturating_sub(self.last_told.load(Ordering::Relaxed)) < TELL_EVERY_MS {
            return;
        }
        if !self.told.swap(true, Ordering::AcqRel) {
            self.last_told.store(at, Ordering::Relaxed);
            // SAFETY: a plain post (no payload) to the window that started
            // the run; a window gone since just drops it.
            unsafe { PostMessageW(self.hwnd as HWND, self.msg, 0, 0) };
        }
    }

    /// The status bar's words for it: reading, stopped, or what it could
    /// not read; `None` once it read everything.
    pub fn status(&self) -> Option<String> {
        let g = crate::format::grouped;
        let seen = self.seen.load(Ordering::Relaxed);
        let total = self.total.load(Ordering::Relaxed);
        let skipped = self.unread.load(Ordering::Relaxed) + self.offline.load(Ordering::Relaxed);
        match self.state.load(Ordering::Acquire) {
            RUNNING => Some(format!(
                "Searching content: {} of {} (Esc to cancel)",
                g(seen),
                g(total.max(seen))
            )),
            STOPPED => Some(format!(
                "Content search cancelled after {} of {}",
                g(seen),
                g(total.max(seen))
            )),
            DONE if skipped > 0 => Some(format!(
                "{} {} not read",
                g(skipped),
                if skipped == 1 {
                    "file was"
                } else {
                    "files were"
                }
            )),
            _ => None,
        }
    }

    /// Reads every candidate of `searches`; the state it ended in.
    ///
    /// Each search's candidates are gathered whole (one answer, at most
    /// [`MANY_MAX`] held) before a file is read, so gozd is never kept
    /// waiting on a pipe while files are read, and a change to the index
    /// meanwhile neither repeats a candidate nor skips one, as answers
    /// asked a page at a time by offset did (W9R-6). The candidates of
    /// several searches are then put in the search's order (W9R-4).
    fn read_all(&self, searches: &[Search], scanner: &Reader, combine: &Combine) -> u8 {
        if searches.is_empty() {
            return DONE;
        }
        let several = searches.len() > 1;
        // The candidates, which of the searches found each (a bit each),
        // and, for several searches, where each path is in `order`.
        let mut order: Vec<Row> = Vec::new();
        let mut found_by: Vec<u32> = Vec::new();
        let mut at: std::collections::HashMap<Box<[u16]>, usize> = std::collections::HashMap::new();
        for (i, search) in searches.iter().enumerate() {
            let all = RowRange {
                offset: 0,
                limit: None,
                details: true,
            };
            #[cfg(test)]
            self.asks.fetch_add(1, Ordering::Relaxed);
            let streamed = stream_range(&self.stop, search, all, |page, total| {
                if i == 0 {
                    self.total.store(total, Ordering::Relaxed);
                }
                for row in page {
                    if !several {
                        order.push(row);
                        found_by.push(1);
                        continue;
                    }
                    match at.get(&row.path) {
                        Some(&k) => found_by[k] |= 1 << i,
                        None => {
                            at.insert(row.path.clone(), order.len());
                            order.push(row);
                            found_by.push(1 << i);
                        }
                    }
                }
                if order.len() > MANY_MAX {
                    return Err(too_many());
                }
                Ok(true)
            });
            match streamed {
                Streamed::Done(_) => {}
                Streamed::Stopped => return STOPPED,
                // Refused, not failed: trying again would gather as many
                // again.
                Streamed::Failed(Failure::Other(why)) if order.len() > MANY_MAX => {
                    return self.fail(Failure::Refused(why));
                }
                Streamed::Failed(f) => return self.fail(f),
            }
        }
        drop(at);
        if several {
            // gozd's sort when it was asked it, else the name order it
            // sorted by; a file list's own.
            let sort = if self.key.list == 0 {
                crate::client::wire_sort(self.key.sort, &crate::client::features())
            } else {
                self.key.sort
            };
            sort_candidates(&mut order, &mut found_by, sort, self.key.created_sort);
        }
        self.total.store(order.len() as u64, Ordering::Relaxed);
        for (row, found) in order.into_iter().zip(found_by) {
            if self.stop.stopped() {
                return STOPPED;
            }
            self.test(row, found, scanner, combine);
        }
        DONE
    }

    fn fail(&self, f: Failure) -> u8 {
        *self.failure.lock().unwrap_or_else(|e| e.into_inner()) = Some(f);
        FAILED
    }

    /// Tests one candidate, found by the searches `found` says.
    fn test(&self, row: Row, found: u32, reader: &Reader, combine: &Combine) {
        self.seen.fetch_add(1, Ordering::Relaxed);
        // Content is a file's. The older plan ANDs its content terms with
        // the rest, so a folder never matches it; the exact plan answers a
        // folder as one whose every content answer is no, as the goz CLI
        // and Everything do (`report | content:x` lists a folder named
        // report, `!content:x` every folder), W9R-2.
        if row.is_dir {
            if let Reader::Exact(..) = reader
                && combine(found, &mut |_| false)
                && !self.stop.stopped()
            {
                self.found
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .push(Found::of(row));
                self.tell(true);
            }
            return;
        }
        let path = std::path::PathBuf::from(String::from_utf16_lossy(&row.path));
        let stop = || self.stop.stopped();
        // What the file could not give: counted once, every answer false.
        let mut unread: Option<Unread> = None;
        let mut note = |why: Unread| {
            if unread.is_none() {
                match why {
                    Unread::Offline => self.offline.fetch_add(1, Ordering::Relaxed),
                    Unread::Failed => self.unread.fetch_add(1, Ordering::Relaxed),
                    Unread::Stopped => 0,
                };
                unread = Some(why);
            }
        };
        let verdict = match reader {
            Reader::Plain(scanner) => {
                let mut answers: Option<u32> = None;
                combine(found, &mut |j| {
                    let a = *answers.get_or_insert_with(|| {
                        scanner.scan_file(&path, &stop).unwrap_or_else(|why| {
                            note(why);
                            0
                        })
                    });
                    a & (1 << j) != 0
                })
            }
            Reader::Exact(split, env) => {
                let name = String::from_utf16_lossy(row.name());
                let mut answers = [None::<bool>; 8];
                let mut failed = false;
                combine(found, &mut |j| {
                    if failed {
                        return false;
                    }
                    *answers[j].get_or_insert_with(|| {
                        match exact::search(&split.terms()[j], &path, &name, env, &stop) {
                            Ok(found) => found,
                            Err(why) => {
                                failed = true;
                                note(why);
                                false
                            }
                        }
                    })
                })
            }
        };
        if verdict && !self.stop.stopped() {
            self.found
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .push(Found::of(row));
        }
        self.tell(verdict);
    }

    fn row_of(f: &Found) -> Row {
        let mut row = Row::new(f.path.clone(), f.size, f.mtime, f.is_dir);
        row.ctime = f.ctime;
        row
    }

    /// Block `block` of search `epoch`: the matches found so far.
    pub fn block(&self, epoch: u64, block: u32) -> Reply {
        let state = self.state.load(Ordering::Acquire);
        if state == FAILED {
            let failure = self
                .failure
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .clone()
                .unwrap_or_else(|| Failure::Other("The content search failed".into()));
            return Reply::Failed {
                epoch,
                block,
                failure,
            };
        }
        let found = self.found.lock().unwrap_or_else(|e| e.into_inner());
        let from = (block as usize * BLOCK_ROWS as usize).min(found.len());
        let to = (from + BLOCK_ROWS as usize).min(found.len());
        let rows = found[from..to].iter().map(Run::row_of).collect();
        // Files' sizes, as the goz CLI totals them: a folder's is theirs.
        let total_size = (block == 0).then(|| {
            found
                .iter()
                .filter(|f| !f.is_dir)
                .filter_map(|f| f.size)
                .fold(0u64, u64::saturating_add)
        });
        let elapsed = self
            .ended
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .unwrap_or_else(|| self.started.elapsed());
        Reply::Block {
            epoch,
            block,
            rows,
            total: found.len() as u64,
            // Still reading: the window searches again as it hears more.
            incomplete: state == RUNNING,
            metadata_pending: false,
            total_size,
            elapsed,
        }
    }

    /// The matches `range` of the results so far, a page at a time, as a
    /// stream of gozd's answer gives them (File > Export, the selection's
    /// rows).
    pub fn stream(
        &self,
        stop: &Stop,
        range: RowRange,
        mut page: impl FnMut(Vec<Row>, u64) -> Result<bool, String>,
    ) -> Streamed {
        const PAGE: usize = 4096;
        let (total, end) = {
            let found = self.found.lock().unwrap_or_else(|e| e.into_inner());
            let total = found.len();
            let end = range
                .limit
                .map_or(total, |l| {
                    (range.offset as usize).saturating_add(l as usize)
                })
                .min(total);
            (total, end)
        };
        let mut at = (range.offset as usize).min(end);
        if at == end {
            return match page(Vec::new(), total as u64) {
                Err(why) => Streamed::Failed(Failure::Other(why)),
                Ok(_) => Streamed::Done(0),
            };
        }
        let mut done = 0u64;
        while at < end {
            if stop.stopped() {
                return Streamed::Stopped;
            }
            let next = (at + PAGE).min(end);
            let rows: Vec<Row> = {
                let found = self.found.lock().unwrap_or_else(|e| e.into_inner());
                found[at..next].iter().map(Run::row_of).collect()
            };
            done += rows.len() as u64;
            match page(rows, total as u64) {
                Err(why) => return Streamed::Failed(Failure::Other(why)),
                Ok(false) => return Streamed::Done(done),
                Ok(true) => {}
            }
            at = next;
        }
        Streamed::Done(done)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::filelist::FileList;
    use goz_core::types::{SortDir, SortKey, SortSpec};

    /// A run for `plan`, not started: its candidates are given or read
    /// (`read_all`) on the test's thread.
    fn run_of(plan: &Plan, sort: SortSpec, list: usize) -> Run {
        Run {
            key: Key {
                queries: plan.queries.clone(),
                sort,
                created_sort: None,
                match_case: false,
                list,
                terms: plan.terms.clone(),
                exact: plan.exact.as_ref().map(|e| (e.1.clone(), e.2)),
            },
            stop: Stop::new(),
            found: Mutex::new(Vec::new()),
            total: AtomicU64::new(0),
            seen: AtomicU64::new(0),
            unread: AtomicU64::new(0),
            offline: AtomicU64::new(0),
            state: AtomicU8::new(RUNNING),
            failure: Mutex::new(None),
            started: Instant::now(),
            ended: Mutex::new(None),
            told: AtomicBool::new(true),
            last_told: AtomicU64::new(0),
            hwnd: 0,
            msg: 0,
            asks: AtomicU64::new(0),
        }
    }

    /// `plan`'s searches over the file list `list`, as `content_run` makes
    /// them.
    fn searches_of(plan: &Plan, list: &Arc<FileList>, sort: SortSpec) -> Vec<Search> {
        plan.queries
            .iter()
            .map(|q| Search {
                query: q.clone(),
                sort,
                match_case: false,
                filelist: Some(Arc::clone(list)),
                created_sort: None,
                slots: Arc::new(crate::filelist::slots::Slots::default()),
                want: Default::default(),
                content: None,
            })
            .collect()
    }

    /// The paths of a run's matches, as its first block lists them, each
    /// with whether it is a folder; and the block's total size.
    fn listed(run: &Run) -> (Vec<(String, bool)>, Option<u64>) {
        let Reply::Block {
            rows, total_size, ..
        } = run.block(1, 0)
        else {
            panic!("a block")
        };
        let rows = rows
            .iter()
            .map(|r| (String::from_utf16_lossy(&r.path), r.is_dir))
            .collect();
        (rows, total_size)
    }

    /// W9R-2 and W9R-4: with the exact plan a folder is answered as one
    /// whose every content answer is no, as the goz CLI and Everything
    /// answer it (`report` is listed; the folder `notes.md`, which only the
    /// searches taking content as yes found, is not), and the matches of
    /// several searches come in the search's order (Date Modified, newest
    /// first), not one search's after another's. Read from files on the
    /// disk, listed by a file list.
    #[test]
    fn the_exact_plan_lists_folders_and_keeps_the_sort() {
        let dir = std::env::temp_dir().join(format!("goz-gui-w9r-exact-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("report")).unwrap();
        std::fs::create_dir_all(dir.join("notes.md")).unwrap();
        for (name, body) in [("x.txt", "foo"), ("y.md", "foo"), ("z.md", "bar")] {
            std::fs::write(dir.join(name), body).unwrap();
        }
        let path = |name: &str| dir.join(name).to_string_lossy().into_owned();
        let (x, y, z, report, other) = (
            path("x.txt"),
            path("y.md"),
            path("z.md"),
            path("report"),
            path("notes.md"),
        );
        // The dates the list gives, which the sort reads: y newest.
        let list = FileList::from_rows(&[
            (x.as_str(), Some(3), Some(100), false),
            (y.as_str(), Some(3), Some(300), false),
            (z.as_str(), Some(3), Some(200), false),
            (report.as_str(), Some(9), Some(50), true),
            (other.as_str(), Some(9), Some(400), true),
        ]);
        let newest = SortSpec {
            key: SortKey::DateModified,
            dir: SortDir::Desc,
        };
        let text = "<*.txt content:foo> | <*.md content:foo> | report";
        let Planned::Content(plan) = plan::plan_exact(text, false) else {
            panic!("the exact plan")
        };
        assert_eq!(plan.queries.len(), 4);
        let run = run_of(&plan, newest, Arc::as_ptr(&list) as usize);
        let searches = searches_of(&plan, &list, newest);
        let state = run.read_all(&searches, &Reader::of(&plan), &plan.combine);
        assert_eq!(state, DONE);
        assert_eq!(run.asks.load(Ordering::Relaxed), 4, "one answer a search");
        let (rows, total_size) = listed(&run);
        assert_eq!(rows, [(y, false), (x, false), (report, true)]);
        assert_eq!(total_size, Some(6), "files' sizes, as the CLI totals them");

        // The older plan ANDs content with the rest: no folder.
        let Planned::Content(plain) = plan::plan("content:foo", false) else {
            panic!("the older plan")
        };
        let run = run_of(&plain, newest, Arc::as_ptr(&list) as usize);
        let searches = searches_of(&plain, &list, newest);
        run.read_all(&searches, &Reader::of(&plain), &plain.combine);
        let (rows, _) = listed(&run);
        assert_eq!(rows.len(), 2);
        assert!(rows.iter().all(|(_, dir)| !dir));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// W9R-6: one search's candidates come in one answer, gathered before
    /// a file is read, not a page of 4,096 at a time by offset, which an
    /// index changing while files are read shifted, so candidates were
    /// tested twice or never. Each is tested once.
    #[test]
    fn one_search_is_gathered_in_one_answer() {
        let dir = std::env::temp_dir().join(format!("goz-gui-w9r-gather-{}", std::process::id()));
        let paths: Vec<String> = (0..5_000)
            .map(|i| {
                dir.join(format!("f{i:05}.txt"))
                    .to_string_lossy()
                    .into_owned()
            })
            .collect();
        let rows: Vec<(&str, Option<u64>, Option<i64>, bool)> = paths
            .iter()
            .map(|p| (p.as_str(), Some(1), Some(1), false))
            .collect();
        let list = FileList::from_rows(&rows);
        let Planned::Content(plan) = plan::plan("content:foo", false) else {
            panic!("the older plan")
        };
        assert_eq!(plan.queries.len(), 1);
        let sort = SortSpec::default();
        let run = run_of(&plan, sort, Arc::as_ptr(&list) as usize);
        let searches = searches_of(&plan, &list, sort);
        let state = run.read_all(&searches, &Reader::of(&plan), &plan.combine);
        assert_eq!(state, DONE);
        assert_eq!(run.asks.load(Ordering::Relaxed), 1);
        assert_eq!(run.seen.load(Ordering::Relaxed), 5_000);
        assert_eq!(run.total.load(Ordering::Relaxed), 5_000);
        assert_eq!(run.unread.load(Ordering::Relaxed), 5_000, "none exists");
    }

    /// W9R-4: the candidates of several searches, sorted as one search's
    /// rows, with which searches found each kept alongside.
    #[test]
    fn candidates_are_sorted_as_one_search() {
        let row = |p: &str, size: u64, mtime: i64, dir: bool| {
            Row::new(p.encode_utf16().collect(), Some(size), Some(mtime), dir)
        };
        let fresh = || {
            vec![
                row(r"C:\b\old.txt", 5, 100, false),
                row(r"C:\a\new.txt", 1, 400, false),
                row(r"C:\c\mid.md", 9, 200, false),
                row(r"C:\a\Dir", 0, 300, true),
            ]
        };
        let names = |rows: &[Row]| -> Vec<String> {
            rows.iter()
                .map(|r| String::from_utf16_lossy(r.name()))
                .collect()
        };
        let sorted = |key: SortKey, dir: SortDir| {
            let mut rows = fresh();
            let mut bits = vec![1, 2, 4, 8];
            sort_candidates(&mut rows, &mut bits, SortSpec { key, dir }, None);
            let n = names(&rows);
            // Each row keeps its own searches.
            let by_name: std::collections::HashMap<String, u32> =
                names(&fresh()).into_iter().zip([1, 2, 4, 8]).collect();
            for (name, b) in n.iter().zip(&bits) {
                assert_eq!(by_name[name], *b, "{name}");
            }
            n
        };
        assert_eq!(
            sorted(SortKey::DateModified, SortDir::Desc),
            ["new.txt", "Dir", "mid.md", "old.txt"]
        );
        assert_eq!(
            sorted(SortKey::Name, SortDir::Asc),
            ["Dir", "mid.md", "new.txt", "old.txt"]
        );
        assert_eq!(
            sorted(SortKey::Size, SortDir::Asc),
            ["Dir", "new.txt", "old.txt", "mid.md"]
        );
        assert_eq!(
            sorted(SortKey::Path, SortDir::Asc),
            ["Dir", "new.txt", "old.txt", "mid.md"]
        );
        assert_eq!(
            sorted(SortKey::Extension, SortDir::Asc),
            ["Dir", "mid.md", "new.txt", "old.txt"]
        );
        // A file list's Date Created sort: unknown dates last, then names.
        let mut rows = fresh();
        rows[2].ctime = Some(7);
        let mut bits = vec![0; 4];
        sort_candidates(
            &mut rows,
            &mut bits,
            SortSpec::default(),
            Some(SortDir::Asc),
        );
        assert_eq!(names(&rows), ["mid.md", "Dir", "new.txt", "old.txt"]);
    }

    /// A run over files on the disk, its candidates given (no gozd): the
    /// files read, matched in order, folders and unreadable files left out.
    #[test]
    fn a_run_tests_its_candidates_in_order() {
        let dir = std::env::temp_dir().join(format!("goz-gui-w9-content-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = |name: &str, body: &[u8]| {
            let p = dir.join(name);
            std::fs::write(&p, body).unwrap();
            p
        };
        let a = file("a.txt", b"chicken noodle soup");
        let b = file("b.txt", b"tomato soup");
        let c = file(
            "c.txt",
            "CHICKEN NOODLE"
                .encode_utf16()
                .flat_map(u16::to_le_bytes)
                .collect::<Vec<_>>()
                .as_slice(),
        );
        let row = |p: &std::path::Path, dir: bool| {
            Row::new(
                p.as_os_str().to_string_lossy().encode_utf16().collect(),
                Some(1),
                None,
                dir,
            )
        };
        let Planned::Content(plan) = plan::plan("content:\"chicken noodle\"", false) else {
            panic!()
        };
        let run = run_of(&plan, Default::default(), 0);
        let scanner = Reader::of(&plan);
        let missing = dir.join("gone.txt");
        for (r, found) in [
            (row(&a, false), 1),
            (row(&dir, true), 1),
            (row(&b, false), 1),
            (row(&missing, false), 1),
            (row(&c, false), 1),
            (row(&a, false), 0),
        ] {
            run.test(r, found, &scanner, &plan.combine);
        }
        assert_eq!(run.matches(), 2);
        assert_eq!(run.seen.load(Ordering::Relaxed), 6);
        assert_eq!(run.unread.load(Ordering::Relaxed), 1);
        let Reply::Block {
            rows,
            total,
            incomplete,
            ..
        } = run.block(1, 0)
        else {
            panic!()
        };
        assert_eq!(total, 2);
        assert!(incomplete, "still running");
        let names: Vec<String> = rows
            .iter()
            .map(|r| String::from_utf16_lossy(r.name()))
            .collect();
        assert_eq!(names, ["a.txt", "c.txt"]);
        run.state.store(STOPPED, Ordering::Release);
        assert!(
            run.status()
                .unwrap()
                .starts_with("Content search cancelled after 6 of")
        );
        let mut paged = Vec::new();
        let streamed = run.stream(
            &Stop::new(),
            RowRange {
                offset: 1,
                limit: None,
                details: true,
            },
            |page, total| {
                assert_eq!(total, 2);
                paged.extend(page);
                Ok(true)
            },
        );
        assert!(matches!(streamed, Streamed::Done(1)));
        assert_eq!(String::from_utf16_lossy(paged[0].name()), "c.txt");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    use plan::Planned;
}
