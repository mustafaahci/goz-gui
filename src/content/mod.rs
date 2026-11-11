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
//! The candidates are asked of gozd a page at a time (4,096 rows), so a
//! search that leaves millions of candidates never holds them all, and
//! gozd is never kept waiting on a pipe while files are read.

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

/// Candidates asked of gozd at a time.
const BATCH: u32 = 4096;
/// Progress is told the window at most this often.
const TELL_EVERY_MS: u64 = 100;
/// The most candidates a plan of several searches gathers before it reads
/// (each is held with the searches that found it): a NOT of a content term
/// with nothing else (`!content:x`) names every file of every volume, and
/// is refused past this with the reason rather than held.
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

/// A file whose content matched.
struct Found {
    path: Box<[u16]>,
    size: Option<u64>,
    mtime: Option<i64>,
    ctime: Option<i64>,
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
    fn read_all(&self, searches: &[Search], scanner: &Reader, combine: &Combine) -> u8 {
        let stopped = || self.stop.stopped();
        match searches {
            [] => DONE,
            [one] => {
                let mut offset = 0u32;
                loop {
                    let mut rows = Vec::new();
                    let range = RowRange {
                        offset,
                        limit: Some(BATCH),
                        details: true,
                    };
                    let streamed = stream_range(&self.stop, one, range, |page, total| {
                        self.total.store(total, Ordering::Relaxed);
                        rows.extend(page);
                        Ok(true)
                    });
                    match streamed {
                        Streamed::Done(_) => {}
                        Streamed::Stopped => return STOPPED,
                        Streamed::Failed(f) => return self.fail(f),
                    }
                    let got = rows.len() as u32;
                    for row in rows {
                        if stopped() {
                            return STOPPED;
                        }
                        self.test(row, 1, scanner, combine);
                    }
                    if got < BATCH {
                        return DONE;
                    }
                    offset += got;
                }
            }
            many => {
                // Which of the searches found each candidate, in the order
                // the first found them (the others' after).
                let mut order: Vec<Row> = Vec::new();
                let mut at: std::collections::HashMap<Box<[u16]>, (usize, u32)> =
                    std::collections::HashMap::new();
                for (i, search) in many.iter().enumerate() {
                    let all = RowRange {
                        offset: 0,
                        limit: None,
                        details: true,
                    };
                    let streamed = stream_range(&self.stop, search, all, |page, total| {
                        if i == 0 {
                            self.total.store(total, Ordering::Relaxed);
                        }
                        for row in page {
                            match at.get_mut(&row.path) {
                                Some((_, found)) => *found |= 1 << i,
                                None => {
                                    at.insert(row.path.clone(), (order.len(), 1 << i));
                                    order.push(row);
                                }
                            }
                        }
                        if order.len() > MANY_MAX {
                            return Err(format!(
                                "The content search would read more than {} files: narrow it \
                                 with a name, a folder or an extension",
                                crate::format::grouped(MANY_MAX as u64)
                            ));
                        }
                        Ok(true)
                    });
                    match streamed {
                        Streamed::Done(_) => {}
                        Streamed::Stopped => return STOPPED,
                        // Refused, not failed: trying again would gather
                        // as many again.
                        Streamed::Failed(Failure::Other(why)) if order.len() > MANY_MAX => {
                            return self.fail(Failure::Refused(why));
                        }
                        Streamed::Failed(f) => return self.fail(f),
                    }
                }
                self.total.store(order.len() as u64, Ordering::Relaxed);
                for row in order {
                    if stopped() {
                        return STOPPED;
                    }
                    let found = at.get(&row.path).map_or(0, |&(_, f)| f);
                    self.test(row, found, scanner, combine);
                }
                DONE
            }
        }
    }

    fn fail(&self, f: Failure) -> u8 {
        *self.failure.lock().unwrap_or_else(|e| e.into_inner()) = Some(f);
        FAILED
    }

    /// Tests one candidate, found by the searches `found` says.
    fn test(&self, row: Row, found: u32, reader: &Reader, combine: &Combine) {
        self.seen.fetch_add(1, Ordering::Relaxed);
        // Content is a file's: a folder never matches.
        if row.is_dir {
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
                .push(Found {
                    path: row.path,
                    size: row.size,
                    mtime: row.mtime,
                    ctime: row.ctime,
                });
        }
        self.tell(verdict);
    }

    fn row_of(f: &Found) -> Row {
        let mut row = Row::new(f.path.clone(), f.size, f.mtime, false);
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
        let total_size = (block == 0).then(|| {
            found
                .iter()
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
        let run = Run {
            key: Key {
                queries: plan.queries.clone(),
                sort: Default::default(),
                created_sort: None,
                match_case: false,
                list: 0,
                terms: plan.terms.clone(),
                exact: None,
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
        };
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
