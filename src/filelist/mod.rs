//! File lists (wave 4): File > Open File List... (Ctrl+O) opens an
//! Everything File List (`.efu`), a CSV with a `Filename` column or a plain
//! list of paths, and searches it instead of gozd's index until File >
//! Close File List.
//!
//! The list is read on a thread of its own as it streams in, in chunks of
//! [`CHUNK_ROWS`] rows that never change once published, so a search can
//! run over what is read so far while the rest comes (the window searches
//! again each second while the list is incomplete, as it does while gozd
//! indexes). A search (`answer`) runs on the search worker's thread, or
//! on Export's and the find bar's: it parses the text with goz-core's own
//! parser and tests each row with [`matcher::Matcher`], then sorts the
//! matches by the search's sort, as gozd orders them. Once the whole list
//! is read, the name and path order of every row is worked out once
//! (`Ranks`), so sorting millions of matches by name or path compares two
//! numbers each. The last answer is kept, so the blocks of one search after
//! the first cost a copy of their rows.
//!
//! A row costs its path (UTF-8), a size, two dates and a flag: about 130
//! bytes for a typical path, so a list of a million rows holds about 140 MB
//! while it is open. Closing the list lets all of it go.
//!
//! Wave 5: the list's `Date Created` column is read too. While a list with
//! any date created is open the window shows a Date Created column, and a
//! search can be sorted by it here ([`Order::Created`]), which gozd's four
//! sort keys cannot say.

pub mod create;
pub mod matcher;
pub mod parse;
pub mod settings;
pub mod slots;
pub mod wild;

use std::collections::HashMap;
use std::fs::File;
use std::io::{self, BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};

use goz_core::fold::fold_into;
use goz_core::query::{LocalClock, mtime_key, parse_query_at, size_key};
use goz_core::types::{SortDir, SortKey, SortSpec};
use goz_winfs::TimeZone;

use crate::client::Row;
use matcher::{Candidate, Matcher, name_of};
use slots::{ListMatcher, Slots};

/// Rows in a chunk: the row's number in the list is its chunk's number
/// times this, plus its place in the chunk.
pub const CHUNK_ROWS: usize = 1 << 16;
const CHUNK_SHIFT: u32 = 16;
/// How many rows a scan tests between asking whether it is still wanted.
const STALE_EVERY: usize = 1 << 14;

const NO_SIZE: u64 = u64::MAX;
const NO_DATE: i64 = i64::MIN;

/// Rows read from the list: their paths end to end, and each row's end in
/// them, size, date modified, date created and whether it is a folder.
struct Chunk {
    text: Vec<u8>,
    ends: Vec<u32>,
    size: Vec<u64>,
    mtime: Vec<i64>,
    ctime: Vec<i64>,
    dir: Vec<bool>,
}

impl Chunk {
    fn new() -> Chunk {
        Chunk {
            text: Vec::new(),
            ends: Vec::with_capacity(CHUNK_ROWS),
            size: Vec::with_capacity(CHUNK_ROWS),
            mtime: Vec::with_capacity(CHUNK_ROWS),
            ctime: Vec::with_capacity(CHUNK_ROWS),
            dir: Vec::with_capacity(CHUNK_ROWS),
        }
    }

    fn len(&self) -> usize {
        self.ends.len()
    }

    fn path(&self, i: usize) -> &[u8] {
        let start = if i == 0 { 0 } else { self.ends[i - 1] as usize };
        &self.text[start..self.ends[i] as usize]
    }

    fn size(&self, i: usize) -> Option<u64> {
        Some(self.size[i]).filter(|&s| s != NO_SIZE)
    }

    fn mtime(&self, i: usize) -> Option<i64> {
        Some(self.mtime[i]).filter(|&t| t != NO_DATE)
    }

    fn ctime(&self, i: usize) -> Option<i64> {
        Some(self.ctime[i]).filter(|&t| t != NO_DATE)
    }

    /// Adds a row; false when its path does not fit (a chunk's text stays
    /// under 4 GB, far beyond any real list).
    fn push(&mut self, path: &str, size: Option<u64>, dates: Dates, dir: bool) -> bool {
        let Ok(end) = u32::try_from(self.text.len() + path.len()) else {
            return false;
        };
        self.text.extend_from_slice(path.as_bytes());
        self.ends.push(end);
        self.size.push(size.unwrap_or(NO_SIZE));
        self.mtime.push(dates.modified.unwrap_or(NO_DATE));
        self.ctime.push(dates.created.unwrap_or(NO_DATE));
        self.dir.push(dir);
        true
    }

    fn candidate(&self, i: usize) -> Candidate<'_> {
        Candidate {
            path: self.path(i),
            is_dir: self.dir[i],
            size: self.size(i),
            mtime: self.mtime(i),
        }
    }
}

/// A row's dates, as FILETIMEs: modified and created.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Dates {
    pub modified: Option<i64>,
    pub created: Option<i64>,
}

/// How a list's matches are ordered: as gozd orders its own, or (wave 5)
/// by the list's Date Created, which gozd cannot sort by.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Order {
    Gozd(SortSpec),
    Created(SortDir),
}

/// Row `g` of the list: its chunk and its place there.
fn split(g: u32) -> (usize, usize) {
    ((g >> CHUNK_SHIFT) as usize, (g as usize) & (CHUNK_ROWS - 1))
}

/// Each row's place in name order and in path order, once the list is read.
/// The tables are by slot (`chunks.len() << CHUNK_SHIFT`), so `rows` says
/// how many rows they rank.
struct Ranks {
    rows: usize,
    name: Vec<u32>,
    path: Vec<u32>,
}

/// A search's matches, sorted, as [`FileList::answer`] gives them.
pub struct Matched {
    query: String,
    sort: Order,
    match_case: bool,
    /// The tab's file-list slots the search's `filelistN:` terms read (W6).
    slots: Arc<Slots>,
    /// The rows read when it was made, and whether that was all of them.
    rows: usize,
    pub complete: bool,
    /// The matching rows, in order.
    pub order: Vec<u32>,
    /// The summed size of the matching files.
    pub total_size: u64,
}

/// An open file list.
pub struct FileList {
    /// Its own number in the process (W5R-5): its reader's message says
    /// it, so a window knows which of its lists the message is about.
    id: u64,
    path: PathBuf,
    name: String,
    chunks: Mutex<Vec<Arc<Chunk>>>,
    /// Every row is read (or reading stopped on an error).
    done: AtomicBool,
    /// The list was closed: reading stops.
    stop: AtomicBool,
    /// Why reading stopped short.
    error: Mutex<Option<String>>,
    /// Rows without a path, or whose path did not fit, left out.
    skipped: Mutex<u64>,
    /// Some row read has a date created (the window then shows the Date
    /// Created column).
    created: AtomicBool,
    ranks: OnceLock<Ranks>,
    last: Mutex<Option<Arc<Matched>>>,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// Where to tell that a list is read: a window and its message (wParam 1
/// when every row was read, 0 when reading stopped on an error; lParam
/// the list's [`id`](FileList::id)).
#[derive(Clone, Copy)]
pub struct Notify {
    pub hwnd: usize,
    pub msg: u32,
}

impl FileList {
    /// Opens the list at `path` and starts reading it on a thread of its
    /// own; an error when the file cannot be opened.
    pub fn open(path: &Path, notify: Option<Notify>) -> io::Result<Arc<FileList>> {
        let file = File::open(path)?;
        if file.metadata()?.is_dir() {
            return Err(io::Error::other("it is a folder"));
        }
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| path.display().to_string());
        let list = Arc::new(FileList {
            id: next_id(),
            path: path.to_path_buf(),
            name,
            chunks: Mutex::new(Vec::new()),
            done: AtomicBool::new(false),
            stop: AtomicBool::new(false),
            error: Mutex::new(None),
            skipped: Mutex::new(0),
            created: AtomicBool::new(false),
            ranks: OnceLock::new(),
            last: Mutex::new(None),
        });
        let base = path
            .parent()
            .map(|p| p.display().to_string())
            .unwrap_or_default();
        let reading = Arc::clone(&list);
        std::thread::Builder::new()
            .name("goz-filelist".into())
            .spawn(move || {
                let outcome = read_list(&reading, file, &base);
                if let Err(why) = &outcome {
                    *lock(&reading.error) = Some(why.clone());
                }
                reading.done.store(true, Ordering::Release);
                if let Some(n) = notify {
                    post(n, outcome.is_ok(), reading.id);
                }
                if outcome.is_ok() && !reading.stop.load(Ordering::Relaxed) {
                    let chunks = lock(&reading.chunks).clone();
                    if let Some(ranks) = compute_ranks(&chunks, &reading.stop) {
                        let _ = reading.ranks.set(ranks);
                    }
                }
            })?;
        Ok(list)
    }

    /// A list of `rows`, read at once: for tests of what uses lists.
    #[cfg(test)]
    pub fn from_rows(rows: &[(&str, Option<u64>, Option<i64>, bool)]) -> Arc<FileList> {
        let with: Vec<_> = rows
            .iter()
            .map(|&(p, s, t, d)| (p, s, t, None, d))
            .collect();
        FileList::from_dated_rows(&with)
    }

    /// [`from_rows`](Self::from_rows) with each row's date created.
    #[cfg(test)]
    #[allow(clippy::type_complexity)]
    pub fn from_dated_rows(
        rows: &[(&str, Option<u64>, Option<i64>, Option<i64>, bool)],
    ) -> Arc<FileList> {
        let mut chunk = Chunk::new();
        let mut created = false;
        for &(p, s, modified, c, d) in rows {
            created |= c.is_some();
            chunk.push(
                p,
                s,
                Dates {
                    modified,
                    created: c,
                },
                d,
            );
        }
        let list = Arc::new(FileList {
            id: next_id(),
            path: PathBuf::from("test.efu"),
            name: "test.efu".into(),
            chunks: Mutex::new(vec![Arc::new(chunk)]),
            done: AtomicBool::new(true),
            stop: AtomicBool::new(false),
            error: Mutex::new(None),
            skipped: Mutex::new(0),
            created: AtomicBool::new(created),
            ranks: OnceLock::new(),
            last: Mutex::new(None),
        });
        let chunks = lock(&list.chunks).clone();
        let _ = list
            .ranks
            .set(compute_ranks(&chunks, &list.stop).expect("ranks"));
        list
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Its own number in the process: no two lists opened have the same.
    pub fn id(&self) -> u64 {
        self.id
    }

    /// Its file name, as the status bar and the title show it.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Whether every row is read.
    pub fn loaded(&self) -> bool {
        self.done.load(Ordering::Acquire)
    }

    /// How many rows are read so far.
    pub fn rows(&self) -> usize {
        lock(&self.chunks).iter().map(|c| c.len()).sum()
    }

    /// Why reading stopped short, if it did.
    pub fn error(&self) -> Option<String> {
        lock(&self.error).clone()
    }

    /// Rows left out: no path in them.
    pub fn skipped(&self) -> u64 {
        *lock(&self.skipped)
    }

    /// Whether a row read so far has a date created: the list has
    /// Everything's Date Created column with values in it.
    pub fn has_created(&self) -> bool {
        self.created.load(Ordering::Relaxed)
    }

    /// The list is closed: reading stops, and what was read goes with the
    /// last reference.
    pub fn close(&self) {
        self.stop.store(true, Ordering::Relaxed);
    }

    /// The rows of the list that match `query` (goz's syntax, and W6's
    /// `filelist:` terms, read with the tab's `slots`), sorted by `sort`;
    /// `Ok(None)` when `stale` said it is not wanted any more before it
    /// was done, an error (goz's words) for a query goz refuses.
    pub fn answer(
        &self,
        query: &str,
        sort: Order,
        match_case: bool,
        slots: &Arc<Slots>,
        stale: &dyn Fn() -> bool,
    ) -> Result<Option<Arc<Matched>>, String> {
        // Whether the list was read whole, then its rows: a chunk published
        // after the first look is only more.
        let complete = self.loaded();
        let chunks = lock(&self.chunks).clone();
        let rows: usize = chunks.iter().map(|c| c.len()).sum();
        if let Some(last) = lock(&self.last).as_ref()
            && last.query == query
            && last.sort == sort
            && last.match_case == match_case
            && last.rows == rows
            && last.complete == complete
            && (Arc::ptr_eq(&last.slots, slots) || last.slots == *slots)
        {
            return Ok(Some(Arc::clone(last)));
        }
        // W6: the `filelist:` terms are goz-gui's; the rest is goz's.
        let (rest, lists) = slots::split_for_list(query, slots)?;
        let parsed =
            parse_query_at(&rest, match_case, &MachineClock::read()).map_err(|e| e.to_string())?;
        let lists: Vec<ListMatcher> = lists
            .iter()
            .map(|names| ListMatcher::new(names, parsed.match_case))
            .collect();
        let mut matcher = Matcher::new(parsed);
        let mut order = Vec::new();
        let mut total_size = 0u64;
        let mut tested = 0usize;
        for (c, chunk) in chunks.iter().enumerate() {
            for i in 0..chunk.len() {
                tested += 1;
                if tested.is_multiple_of(STALE_EVERY) && stale() {
                    return Ok(None);
                }
                let row = chunk.candidate(i);
                if matcher.matches(&row) && lists.iter().all(|l| l.matches(row.path)) {
                    order.push(((c << CHUNK_SHIFT) | i) as u32);
                    if !chunk.dir[i] {
                        total_size = total_size.saturating_add(chunk.size(i).unwrap_or(0));
                    }
                }
            }
        }
        if stale() {
            return Ok(None);
        }
        sort_rows(&chunks, self.ranks_for(rows), &mut order, sort);
        let matched = Arc::new(Matched {
            query: query.to_owned(),
            sort,
            match_case,
            slots: Arc::clone(slots),
            rows,
            complete,
            order,
            total_size,
        });
        *lock(&self.last) = Some(Arc::clone(&matched));
        Ok(Some(matched))
    }

    /// The ranks, when they rank the `rows` rows read (they are made once
    /// the whole list is read).
    fn ranks_for(&self, rows: usize) -> Option<&Ranks> {
        self.ranks.get().filter(|r| r.rows == rows)
    }

    /// The rows `range` of `matched`, as the results list shows them.
    pub fn rows_of(&self, matched: &Matched, range: std::ops::Range<usize>) -> Vec<Row> {
        let chunks = lock(&self.chunks).clone();
        let end = range.end.min(matched.order.len());
        let start = range.start.min(end);
        matched.order[start..end]
            .iter()
            .filter_map(|&g| {
                let (c, i) = split(g);
                let chunk = chunks.get(c)?;
                let path: Box<[u16]> = String::from_utf8_lossy(chunk.path(i))
                    .encode_utf16()
                    .collect();
                let mut row = Row::new(path, chunk.size(i), chunk.mtime(i), chunk.dir[i]);
                row.ctime = chunk.ctime(i);
                Some(row)
            })
            .collect()
    }
}

fn post(n: Notify, ok: bool, id: u64) {
    use windows_sys::Win32::UI::WindowsAndMessaging::PostMessageW;
    // SAFETY: a plain post to the window that asked; nothing is passed by
    // pointer, and a window gone since just drops it.
    unsafe { PostMessageW(n.hwnd as _, n.msg, usize::from(ok), id as isize) };
}

/// A new list's [`FileList::id`]: never 0.
fn next_id() -> u64 {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    NEXT.fetch_add(1, Ordering::Relaxed)
}

/// The clock and time zone a `dm:` date resolves against, read as each
/// search starts (gozd's own reads the machine's the same way).
pub struct MachineClock {
    now: i64,
    zone: Option<TimeZone>,
    week_start: u8,
}

impl MachineClock {
    pub fn read() -> MachineClock {
        MachineClock {
            now: crate::history::filetime_now() as i64,
            zone: TimeZone::current().ok(),
            week_start: goz_winfs::first_day_of_week(),
        }
    }
}

impl LocalClock for MachineClock {
    fn now(&self) -> i64 {
        self.now
    }
    fn to_local(&self, utc: i64) -> Option<i64> {
        self.zone?.local_from_utc(utc)
    }
    fn to_utc(&self, local: i64) -> Option<i64> {
        self.zone?.utc_from_local(local)
    }
    fn week_start(&self) -> u8 {
        self.week_start
    }
}

/// A reader of the list's text as UTF-8: as it is, or a UTF-16 list
/// (with its byte order mark) turned into UTF-8 first.
pub(crate) fn utf8_reader(file: File) -> io::Result<Box<dyn BufRead + Send>> {
    let mut reader = BufReader::with_capacity(1 << 20, file);
    let head = reader.fill_buf()?;
    if head.starts_with(&[0xFF, 0xFE]) {
        let mut bytes = Vec::new();
        reader.read_to_end(&mut bytes)?;
        let units: Vec<u16> = bytes[2..]
            .as_chunks::<2>()
            .0
            .iter()
            .map(|&p| u16::from_le_bytes(p))
            .collect();
        let text = String::from_utf16_lossy(&units);
        return Ok(Box::new(io::Cursor::new(text.into_bytes())));
    }
    Ok(Box::new(reader))
}

/// Reads the list's rows into `list`, publishing each full chunk.
fn read_list(list: &FileList, file: File, base: &str) -> Result<(), String> {
    let reader = utf8_reader(file).map_err(|e| e.to_string())?;
    let clock = MachineClock::read();
    let to_utc = |t: i64| clock.to_utc(t);
    let mut chunk = Chunk::new();
    let mut skipped = 0u64;
    let publish = |chunk: Chunk| {
        let mut chunk = chunk;
        chunk.text.shrink_to_fit();
        lock(&list.chunks).push(Arc::new(chunk));
    };
    parse::each_row(
        reader,
        base,
        &to_utc,
        &list.stop,
        |path, size, dates, _, dir| {
            if path.is_empty() || !chunk.push(path, size, dates, dir) {
                skipped += 1;
            } else if dates.created.is_some() && !list.created.load(Ordering::Relaxed) {
                list.created.store(true, Ordering::Relaxed);
            }
            if chunk.len() == CHUNK_ROWS {
                publish(std::mem::replace(&mut chunk, Chunk::new()));
            }
        },
    )
    .map_err(|e| e.to_string())?;
    if list.stop.load(Ordering::Relaxed) {
        return Ok(());
    }
    if chunk.len() > 0 {
        publish(chunk);
    }
    *lock(&list.skipped) = skipped;
    Ok(())
}

/// A row's name folded as goz folds it, the key a name sort orders by.
fn folded_name(path: &[u8], out: &mut Vec<u8>) {
    fold_into(name_of(path), out);
}

/// The folder part of a full path (empty for a drive's root).
fn parent_of(path: &[u8]) -> &[u8] {
    let name = name_of(path);
    if name.len() == path.len() {
        return &[];
    }
    &path[..path.len() - name.len() - 1]
}

/// A folder's sort key: folded, each `\` made a byte below every other,
/// so folders order part by part (`a\x` before `a b`).
fn folder_key(parent: &[u8], out: &mut Vec<u8>) {
    let from = out.len();
    fold_into(parent, out);
    for b in &mut out[from..] {
        if *b == b'\\' {
            *b = 1;
        }
    }
}

/// The key of a path sort without ranks: the folder's key, then a byte
/// below its separators, then the folded name.
fn path_key(path: &[u8]) -> Vec<u8> {
    let mut key = Vec::with_capacity(path.len());
    folder_key(parent_of(path), &mut key);
    key.push(0);
    folded_name(path, &mut key);
    key
}

/// Sorts `order` as gozd sorts its matches: by the sort's key (unknown
/// sizes and dates last), then the folded name, then the row; a
/// descending sort is that order reversed. Date Created sorts as Date
/// Modified does, by the other date.
fn sort_rows(chunks: &[Arc<Chunk>], ranks: Option<&Ranks>, order: &mut [u32], sort: Order) {
    let at = |g: u32| {
        let (c, i) = split(g);
        (&chunks[c], i)
    };
    let (key, dir, created) = match sort {
        Order::Gozd(s) => (s.key, s.dir, false),
        Order::Created(dir) => (SortKey::DateModified, dir, true),
    };
    let primary = |g: u32| -> u64 {
        let (chunk, i) = at(g);
        match key {
            SortKey::Size => size_key(chunk.size(i).filter(|_| !chunk.dir[i]), dir),
            SortKey::DateModified if created => mtime_key(chunk.ctime(i), dir),
            SortKey::DateModified => mtime_key(chunk.mtime(i), dir),
            SortKey::Name | SortKey::Path => 0,
        }
    };
    match (ranks, key) {
        (Some(r), SortKey::Name) => order.sort_unstable_by_key(|&g| r.name[g as usize]),
        (Some(r), SortKey::Path) => order.sort_unstable_by_key(|&g| r.path[g as usize]),
        (Some(r), _) => order.sort_unstable_by_key(|&g| (primary(g), r.name[g as usize])),
        (None, SortKey::Path) => order.sort_by_cached_key(|&g| {
            let (chunk, i) = at(g);
            (path_key(chunk.path(i)), g)
        }),
        (None, _) => order.sort_by_cached_key(|&g| {
            let (chunk, i) = at(g);
            let mut name = Vec::new();
            folded_name(chunk.path(i), &mut name);
            (primary(g), name, g)
        }),
    }
    if dir == SortDir::Desc {
        order.reverse();
    }
}

/// Every row's place in name order and in path order: names compared
/// folded (then by row), paths folder by folder and then by name.
fn compute_ranks(chunks: &[Arc<Chunk>], stop: &AtomicBool) -> Option<Ranks> {
    let n: usize = chunks.iter().map(|c| c.len()).sum();
    let rows = || {
        chunks
            .iter()
            .enumerate()
            .flat_map(|(c, chunk)| (0..chunk.len()).map(move |i| (c, i, chunk)))
    };
    let index = |c: usize, i: usize| (c << CHUNK_SHIFT) | i;
    // The folded names end to end, and where each ends, by row.
    let mut names = Vec::new();
    let mut ends = vec![0usize; chunks.len() << CHUNK_SHIFT];
    for (c, i, chunk) in rows() {
        folded_name(chunk.path(i), &mut names);
        ends[index(c, i)] = names.len();
    }
    let name = |g: usize| {
        let start = if g == 0 {
            0
        } else {
            // The previous row, in the previous chunk at a chunk's start.
            let (c, i) = (g >> CHUNK_SHIFT, g & (CHUNK_ROWS - 1));
            if i == 0 {
                ends[((c - 1) << CHUNK_SHIFT) | (chunks[c - 1].len() - 1)]
            } else {
                ends[g - 1]
            }
        };
        &names[start..ends[g]]
    };
    let mut all: Vec<u32> = rows().map(|(c, i, _)| index(c, i) as u32).collect();
    if stop.load(Ordering::Relaxed) {
        return None;
    }
    all.sort_unstable_by(|&a, &b| name(a as usize).cmp(name(b as usize)).then(a.cmp(&b)));
    let slots = chunks.len() << CHUNK_SHIFT;
    let mut name_rank = vec![0u32; slots];
    for (r, &g) in all.iter().enumerate() {
        name_rank[g as usize] = r as u32;
    }
    drop(names);
    drop(ends);
    if stop.load(Ordering::Relaxed) {
        return None;
    }
    // Folders: each distinct one once, ordered by its key.
    let mut folder_ids: HashMap<&[u8], u32> = HashMap::new();
    let mut folders: Vec<&[u8]> = Vec::new();
    let mut folder_of = vec![0u32; slots];
    for (c, i, chunk) in rows() {
        let parent = parent_of(chunk.path(i));
        let id = *folder_ids.entry(parent).or_insert_with(|| {
            folders.push(parent);
            (folders.len() - 1) as u32
        });
        folder_of[index(c, i)] = id;
    }
    drop(folder_ids);
    let keys: Vec<Vec<u8>> = folders
        .iter()
        .map(|f| {
            let mut k = Vec::with_capacity(f.len());
            folder_key(f, &mut k);
            k
        })
        .collect();
    let mut by_key: Vec<u32> = (0..folders.len() as u32).collect();
    by_key.sort_unstable_by(|&a, &b| keys[a as usize].cmp(&keys[b as usize]));
    let mut folder_rank = vec![0u32; folders.len()];
    for (r, &f) in by_key.iter().enumerate() {
        folder_rank[f as usize] = r as u32;
    }
    drop(keys);
    if stop.load(Ordering::Relaxed) {
        return None;
    }
    all.sort_unstable_by_key(|&g| {
        (
            folder_rank[folder_of[g as usize] as usize],
            name_rank[g as usize],
        )
    });
    let mut path_rank = vec![0u32; slots];
    for (r, &g) in all.iter().enumerate() {
        path_rank[g as usize] = r as u32;
    }
    debug_assert_eq!(all.len(), n);
    Some(Ranks {
        rows: n,
        name: name_rank,
        path: path_rank,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const ROWS: [(&str, Option<u64>, Option<i64>, bool); 7] = [
        (r"C:\b\zeta.txt", Some(10), Some(300), false),
        (r"C:\a b\x.txt", Some(30), Some(100), false),
        (r"C:\a\Alpha.TXT", Some(20), None, false),
        (r"C:\a\x\y.txt", None, Some(200), false),
        (r"C:\a", None, Some(50), true),
        (r"C:\", None, None, true),
        (r"C:\a\beta.txt", Some(20), Some(400), false),
    ];

    fn no_slots() -> Arc<Slots> {
        Arc::new(Slots::default())
    }

    fn names(list: &FileList, query: &str, key: SortKey, dir: SortDir) -> Vec<String> {
        ordered(list, query, Order::Gozd(SortSpec { key, dir }))
    }

    /// W6: `filelist:` and the slots, over a list.
    #[test]
    fn filelist_terms_and_slots_in_a_list() {
        let list = FileList::from_rows(&ROWS);
        let spec = Order::Gozd(SortSpec::default());
        let found = |q: &str, slots: &Arc<Slots>| -> Result<Vec<String>, String> {
            let m = list
                .answer(q, spec, false, slots, &|| false)?
                .expect("not stale");
            Ok(list
                .rows_of(&m, 0..m.order.len())
                .iter()
                .map(|r| String::from_utf16_lossy(&r.path))
                .collect())
        };
        let none = no_slots();
        assert_eq!(
            found(r#"filelist:"alpha.txt|C:\b\zeta.txt""#, &none).unwrap(),
            [r"C:\a\Alpha.TXT", r"C:\b\zeta.txt"]
        );
        // With the rest of the search, as AND; a wildcard; a folder.
        assert_eq!(
            found(r#"filelist:*.txt "a b\""#, &none).unwrap(),
            [r"C:\a b\x.txt"]
        );
        assert_eq!(found(r"filelist:C:\a\ folder:", &none).unwrap(), [r"C:\a"]);
        let mut s = Slots::default();
        s.set(2, vec![r"C:\a\beta.txt".into(), "Y.TXT".into()]);
        let s = Arc::new(s);
        assert_eq!(
            found("filelist2:", &s).unwrap(),
            [r"C:\a\beta.txt", r"C:\a\x\y.txt"]
        );
        // The same search, other slots: not the answer kept.
        assert!(found("filelist2:", &none).unwrap().is_empty());
        assert!(found("filelist2:x", &s).is_err());
        // `|` outside quotes is goz's, which v0.6.0 refuses.
        assert!(found("filelist:a|b", &none).is_err());
    }

    fn ordered(list: &FileList, query: &str, order: Order) -> Vec<String> {
        let m = list
            .answer(query, order, false, &no_slots(), &|| false)
            .expect("answers")
            .expect("not stale");
        list.rows_of(&m, 0..m.order.len())
            .iter()
            .map(|r| String::from_utf16_lossy(&r.path))
            .collect()
    }

    /// The same list without its ranks: the sort made while it is read.
    fn unranked() -> Arc<FileList> {
        let list = FileList::from_rows(&ROWS);
        let mut l = Arc::try_unwrap(list).ok().expect("one owner");
        l.ranks = OnceLock::new();
        Arc::new(l)
    }

    #[test]
    fn sorts_as_gozd_with_ranks_and_without() {
        // The ranks are used for a list of any length (they are by slot,
        // 65,536 a chunk), and only for the rows they rank.
        let ranked = FileList::from_rows(&ROWS);
        assert!(ranked.ranks_for(ROWS.len()).is_some());
        assert!(ranked.ranks_for(ROWS.len() + 1).is_none());
        assert!(unranked().ranks_for(ROWS.len()).is_none());
        for list in [ranked, unranked()] {
            assert_eq!(
                names(&list, "", SortKey::Name, SortDir::Asc),
                [
                    r"C:\a",
                    r"C:\a\Alpha.TXT",
                    r"C:\a\beta.txt",
                    r"C:\",
                    r"C:\a b\x.txt",
                    r"C:\a\x\y.txt",
                    r"C:\b\zeta.txt"
                ]
            );
            // Folder by folder: C:\ (the root's own), C:\a, C:\a\x, C:\a b, C:\b.
            assert_eq!(
                names(&list, ".txt", SortKey::Path, SortDir::Asc),
                [
                    r"C:\a\Alpha.TXT",
                    r"C:\a\beta.txt",
                    r"C:\a\x\y.txt",
                    r"C:\a b\x.txt",
                    r"C:\b\zeta.txt"
                ]
            );
            // Unknown sizes (and folders') last, either way; ties by name.
            assert_eq!(
                names(&list, ".txt", SortKey::Size, SortDir::Asc),
                [
                    r"C:\b\zeta.txt",
                    r"C:\a\Alpha.TXT",
                    r"C:\a\beta.txt",
                    r"C:\a b\x.txt",
                    r"C:\a\x\y.txt"
                ]
            );
            assert_eq!(
                names(&list, ".txt", SortKey::Size, SortDir::Desc),
                [
                    r"C:\a b\x.txt",
                    r"C:\a\beta.txt",
                    r"C:\a\Alpha.TXT",
                    r"C:\b\zeta.txt",
                    r"C:\a\x\y.txt"
                ]
            );
            assert_eq!(
                names(&list, ".txt", SortKey::DateModified, SortDir::Desc),
                [
                    r"C:\a\beta.txt",
                    r"C:\b\zeta.txt",
                    r"C:\a\x\y.txt",
                    r"C:\a b\x.txt",
                    r"C:\a\Alpha.TXT"
                ]
            );
        }
    }

    #[test]
    fn sorts_by_date_created_with_unknown_dates_last() {
        let list = FileList::from_dated_rows(&[
            (r"C:\a\old.txt", Some(1), Some(900), Some(100), false),
            (r"C:\a\none.txt", Some(1), Some(800), None, false),
            (r"C:\a\new.txt", Some(1), Some(700), Some(300), false),
            (r"C:\a\mid.txt", Some(1), Some(600), Some(200), false),
        ]);
        assert!(list.has_created());
        assert!(!FileList::from_rows(&ROWS).has_created());
        assert_eq!(
            ordered(&list, "", Order::Created(SortDir::Desc)),
            [
                r"C:\a\new.txt",
                r"C:\a\mid.txt",
                r"C:\a\old.txt",
                r"C:\a\none.txt"
            ]
        );
        assert_eq!(
            ordered(&list, "", Order::Created(SortDir::Asc)),
            [
                r"C:\a\old.txt",
                r"C:\a\mid.txt",
                r"C:\a\new.txt",
                r"C:\a\none.txt"
            ]
        );
        // Another order is another answer, not the one kept.
        let by_dm = ordered(
            &list,
            "",
            Order::Gozd(SortSpec {
                key: SortKey::DateModified,
                dir: SortDir::Desc,
            }),
        );
        assert_eq!(by_dm[0], r"C:\a\old.txt");
        let m = list
            .answer(
                "",
                Order::Created(SortDir::Desc),
                false,
                &no_slots(),
                &|| false,
            )
            .unwrap()
            .unwrap();
        let rows = list.rows_of(&m, 0..1);
        assert_eq!(rows[0].ctime, Some(300));
    }

    #[test]
    fn answers_are_kept_and_refusals_are_gozds() {
        let list = FileList::from_rows(&ROWS);
        let spec = Order::Gozd(SortSpec::default());
        let a = list
            .answer("txt", spec, false, &no_slots(), &|| false)
            .unwrap()
            .unwrap();
        let b = list
            .answer("txt", spec, false, &no_slots(), &|| false)
            .unwrap()
            .unwrap();
        assert!(Arc::ptr_eq(&a, &b));
        assert_eq!(a.order.len(), 5);
        assert_eq!(a.total_size, 80);
        assert!(
            list.answer("zeta", spec, false, &no_slots(), &|| true)
                .unwrap()
                .is_none()
        );
        assert!(
            list.answer("bogus:x", spec, false, &no_slots(), &|| false)
                .is_err()
        );
    }

    #[test]
    fn reads_efu_csv_and_plain_lists() {
        let dir = crate::store::file::tests::fresh_dir("goz-gui-filelist", "");
        std::fs::create_dir_all(&dir).unwrap();
        let efu = dir.join("a.efu");
        std::fs::write(
            &efu,
            "Filename,Size,Date Modified,Date Created,Attributes\r\n\
             \"C:\\x\\a.txt\",5,133000000000000000,132000000000000000,32\r\n\
             \"C:\\x\\sub\\\",,,,16\r\n\
             \"rel\\b.txt\",7,,,32\r\n\
             ,,,,\r\n",
        )
        .unwrap();
        let txt = dir.join("b.txt");
        std::fs::write(&txt, "C:\\one,two.txt\r\nC:\\folder\\\r\n").unwrap();
        for (path, want) in [(&efu, 3), (&txt, 2)] {
            let list = FileList::open(path, None).unwrap();
            while !list.loaded() {
                std::thread::yield_now();
            }
            assert_eq!(list.rows(), want, "{}", path.display());
            assert!(list.error().is_none());
        }
        let list = FileList::open(&efu, None).unwrap();
        while !list.loaded() {
            std::thread::yield_now();
        }
        assert_eq!(list.skipped(), 1);
        let m = list
            .answer(
                "",
                Order::Gozd(SortSpec::default()),
                false,
                &no_slots(),
                &|| false,
            )
            .unwrap()
            .unwrap();
        let rows = list.rows_of(&m, 0..10);
        let paths: Vec<String> = rows
            .iter()
            .map(|r| String::from_utf16_lossy(&r.path))
            .collect();
        let rel = format!(r"{}\rel\b.txt", dir.display());
        assert!(paths.contains(&rel), "{paths:?}");
        let sub = rows
            .iter()
            .find(|r| String::from_utf16_lossy(&r.path) == r"C:\x\sub")
            .expect("the folder");
        assert!(sub.is_dir);
        let a = rows.iter().find(|r| r.size == Some(5)).expect("a.txt");
        assert_eq!(a.mtime, Some(133_000_000_000_000_000));
        assert_eq!(a.ctime, Some(132_000_000_000_000_000));
        assert!(list.has_created());
        assert!(FileList::open(&dir.join("missing.efu"), None).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
