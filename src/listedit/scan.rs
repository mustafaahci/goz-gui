//! The File List Editor's work off its window's thread: adding files and
//! folders (each folder listed with `FindFirstFileExW`'s large fetches, its
//! subfolders after it when asked), reading a list file, and saving one.
//!
//! A job hands its rows over in batches ([`Found`]): at most one message
//! is waiting for the window at a time (`posted`), and each one takes every
//! row found since, so a folder of a million items costs the window a few
//! dozen messages, not a million. Esc (or Edit > Stop Adding) sets the
//! job's stop flag; the walk checks it between items.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use windows_sys::Win32::Foundation::FILETIME;
use windows_sys::Win32::Storage::FileSystem::{
    FIND_FIRST_EX_LARGE_FETCH, FindClose, FindExInfoBasic, FindExSearchNameMatch, FindFirstFileExW,
    FindNextFileW, GetFileAttributesExW, GetFileExInfoStandard, WIN32_FILE_ATTRIBUTE_DATA,
    WIN32_FIND_DATAW,
};
use windows_sys::Win32::UI::WindowsAndMessaging::PostMessageW;

use super::Entry;
use crate::wide::wz;

const DIRECTORY: u32 = 0x10;
const REPARSE_POINT: u32 = 0x400;
/// How often a job hands its rows over, at most.
const HAND_OVER: Duration = Duration::from_millis(100);
/// Rows a job gathers before it hands them over anyway.
const BATCH: usize = 8192;

/// What a job adds.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Source {
    /// One item as it is: a file, or a folder alone.
    Item(String),
    /// A folder and what it holds; with `subfolders`, everything under it
    /// (links to other folders, junctions and symbolic links, are listed,
    /// not entered).
    Folder { path: String, subfolders: bool },
    /// The rows of a list file (File > Open...).
    List(PathBuf),
}

/// What a job has for the window.
#[derive(Debug, Default)]
pub struct Found {
    /// The rows found since the window last took them.
    pub entries: Vec<Entry>,
    /// Items found so far, in all.
    pub seen: u64,
    /// Folders that could not be listed (and items that could not be read).
    pub unreadable: u64,
    /// The job is over (done, or stopped).
    pub done: bool,
    /// It was stopped before it was through.
    pub stopped: bool,
    /// Why a list file could not be read.
    pub error: Option<String>,
    /// A message is on its way to the window.
    posted: bool,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// A job running on a thread of its own.
pub struct Job {
    stop: Arc<AtomicBool>,
    shared: Arc<Mutex<Found>>,
}

impl Job {
    /// Starts adding `sources`, telling window `hwnd` with message `msg`
    /// whenever it has rows or is over.
    pub fn start(sources: Vec<Source>, hwnd: usize, msg: u32) -> std::io::Result<Job> {
        let stop = Arc::new(AtomicBool::new(false));
        let shared = Arc::new(Mutex::new(Found::default()));
        let (s, f) = (Arc::clone(&stop), Arc::clone(&shared));
        std::thread::Builder::new()
            .name("goz-list-editor-scan".into())
            .spawn(move || {
                let mut out = Out::new(&f, hwnd, msg);
                for source in sources {
                    if s.load(Ordering::Relaxed) {
                        break;
                    }
                    match source {
                        Source::Item(path) => match item(&path) {
                            Some(e) => out.push(e),
                            None => out.unreadable += 1,
                        },
                        Source::Folder { path, subfolders } => {
                            walk(&mut out, &path, subfolders, &s);
                        }
                        Source::List(path) => {
                            if let Err(why) = read_list(&mut out, &path, &s) {
                                lock(&f).error = Some(why);
                            }
                        }
                    }
                }
                lock(&f).stopped = s.load(Ordering::Relaxed);
                out.hand_over(true);
            })?;
        Ok(Job { stop, shared })
    }

    /// Asks the job to stop; what it found so far still comes.
    pub fn stop(&self) {
        self.stop.store(true, Ordering::Relaxed);
    }

    /// Takes what the job has for the window: the rows found since last
    /// time, and where it is.
    pub fn take(&self) -> Found {
        let mut f = lock(&self.shared);
        f.posted = false;
        Found {
            entries: std::mem::take(&mut f.entries),
            seen: f.seen,
            unreadable: f.unreadable,
            done: f.done,
            stopped: f.stopped,
            error: f.error.clone(),
            posted: false,
        }
    }
}

/// A job's side of [`Found`]: the rows gathered, handed over in batches.
struct Out<'a> {
    shared: &'a Mutex<Found>,
    batch: Vec<Entry>,
    seen: u64,
    unreadable: u64,
    last: Instant,
    hwnd: usize,
    msg: u32,
}

impl<'a> Out<'a> {
    fn new(shared: &'a Mutex<Found>, hwnd: usize, msg: u32) -> Out<'a> {
        Out {
            shared,
            batch: Vec::new(),
            seen: 0,
            unreadable: 0,
            last: Instant::now(),
            hwnd,
            msg,
        }
    }

    fn push(&mut self, e: Entry) {
        self.batch.push(e);
        self.seen += 1;
        if self.batch.len() >= BATCH
            || (self.seen.is_multiple_of(256) && self.last.elapsed() >= HAND_OVER)
        {
            self.hand_over(false);
        }
    }

    /// Hands the rows gathered to the window (`done`: and says the job is
    /// over), with a message unless one is on its way already.
    fn hand_over(&mut self, done: bool) {
        let post = {
            let mut f = lock(self.shared);
            f.entries.append(&mut self.batch);
            f.seen = self.seen;
            f.unreadable = self.unreadable;
            f.done |= done;
            !std::mem::replace(&mut f.posted, true)
        };
        if post && self.hwnd != 0 {
            // SAFETY: a plain post to the editor's window; nothing is passed
            // by pointer, and a window gone since just drops it.
            unsafe { PostMessageW(self.hwnd as _, self.msg, 0, 0) };
        }
        self.last = Instant::now();
    }
}

/// A FILETIME as goz keeps one; `None` for zero (no date).
fn filetime(t: FILETIME) -> Option<i64> {
    let v = (u64::from(t.dwHighDateTime) << 32) | u64::from(t.dwLowDateTime);
    (v != 0).then_some(v as i64)
}

/// `folder` joined with `name`.
fn join(folder: &str, name: &str) -> String {
    let mut out = folder.trim_end_matches('\\').to_owned();
    out.push('\\');
    out.push_str(name);
    out
}

/// The entry for the item at `path`, as the disk has it now; `None` when
/// nothing is there (or it cannot be read).
pub fn item(path: &str) -> Option<Entry> {
    let mut p = path.trim().replace('/', "\\");
    // A folder's trailing backslash goes, but a drive's root keeps it.
    while p.len() > 3 && p.ends_with('\\') {
        p.pop();
    }
    if p.len() == 2 && p.ends_with(':') {
        p.push('\\');
    }
    let z = wz(&p);
    // SAFETY: a NUL-terminated path and a plain out-struct of the size the
    // call fills.
    let data = unsafe {
        let mut data: WIN32_FILE_ATTRIBUTE_DATA = core::mem::zeroed();
        (GetFileAttributesExW(z.as_ptr(), GetFileExInfoStandard, (&raw mut data).cast()) != 0)
            .then_some(data)
    }?;
    let dir = data.dwFileAttributes & DIRECTORY != 0;
    Some(Entry {
        path: p,
        is_dir: dir,
        size: (!dir)
            .then_some((u64::from(data.nFileSizeHigh) << 32) | u64::from(data.nFileSizeLow)),
        modified: filetime(data.ftLastWriteTime),
        created: filetime(data.ftCreationTime),
        attributes: Some(data.dwFileAttributes),
    })
}

/// Lists `folder`'s items, handing each to `each` with its attributes;
/// false when it could not be listed.
fn list(folder: &str, mut each: impl FnMut(Entry, u32)) -> bool {
    let pattern = wz(&join(folder, "*"));
    // SAFETY: a NUL-terminated pattern, a plain out-struct the calls fill,
    // and a find handle closed before returning.
    unsafe {
        let mut data: WIN32_FIND_DATAW = core::mem::zeroed();
        let h = FindFirstFileExW(
            pattern.as_ptr(),
            FindExInfoBasic,
            (&raw mut data).cast(),
            FindExSearchNameMatch,
            core::ptr::null(),
            FIND_FIRST_EX_LARGE_FETCH,
        );
        if h.is_null() || h as isize == -1 {
            return false;
        }
        loop {
            let len = data
                .cFileName
                .iter()
                .position(|&c| c == 0)
                .unwrap_or(data.cFileName.len());
            let name = String::from_utf16_lossy(&data.cFileName[..len]);
            if name != "." && name != ".." {
                let attributes = data.dwFileAttributes;
                let dir = attributes & DIRECTORY != 0;
                let e = Entry {
                    path: join(folder, &name),
                    is_dir: dir,
                    size: (!dir).then_some(
                        (u64::from(data.nFileSizeHigh) << 32) | u64::from(data.nFileSizeLow),
                    ),
                    modified: filetime(data.ftLastWriteTime),
                    created: filetime(data.ftCreationTime),
                    attributes: Some(attributes),
                };
                each(e, attributes);
            }
            if FindNextFileW(h, &mut data) == 0 {
                break;
            }
        }
        FindClose(h);
    }
    true
}

/// Adds the folder at `root` and what it holds (everything under it with
/// `subfolders`), each folder's items after it, its subfolders in the
/// order the disk lists them.
fn walk(out: &mut Out<'_>, root: &str, subfolders: bool, stop: &AtomicBool) {
    let Some(top) = item(root) else {
        out.unreadable += 1;
        return;
    };
    if !top.is_dir {
        // A file: added as it is.
        out.push(top);
        return;
    }
    let start = top.path.clone();
    out.push(top);
    let mut stack = vec![start];
    while let Some(folder) = stack.pop() {
        if stop.load(Ordering::Relaxed) {
            return;
        }
        let mut inner = Vec::new();
        let listed = list(&folder, |e, attributes| {
            if subfolders && e.is_dir && attributes & REPARSE_POINT == 0 {
                inner.push(e.path.clone());
            }
            out.push(e);
        });
        if !listed {
            out.unreadable += 1;
        }
        stack.extend(inner.into_iter().rev());
    }
}

/// Adds the rows of the list file at `path`, as goz-gui's file lists read
/// them (a relative path from the list's folder).
fn read_list(out: &mut Out<'_>, path: &Path, stop: &AtomicBool) -> Result<(), String> {
    let file = std::fs::File::open(path).map_err(|e| e.to_string())?;
    let reader = crate::filelist::utf8_reader(file).map_err(|e| e.to_string())?;
    let base = path
        .parent()
        .map(|p| p.display().to_string())
        .unwrap_or_default();
    let clock = crate::filelist::MachineClock::read();
    let to_utc = |t: i64| goz_core::query::LocalClock::to_utc(&clock, t);
    crate::filelist::parse::each_row(
        reader,
        &base,
        &to_utc,
        stop,
        |p, size, dates, attributes, dir| {
            if !p.is_empty() {
                out.push(Entry {
                    path: p.to_owned(),
                    is_dir: dir,
                    size: size.filter(|_| !dir),
                    modified: dates.modified,
                    created: dates.created,
                    attributes,
                });
            }
        },
    )
    .map_err(|e| e.to_string())
}

/// A save on a thread of its own: its outcome, once it has one.
pub struct Saving {
    pub path: PathBuf,
    /// The list's state when it was taken (the editor's change count).
    pub generation: u64,
    pub rows: usize,
    outcome: Arc<Mutex<Option<Result<(), String>>>>,
}

impl Saving {
    /// Writes `entries` to `path` as an Everything File List, in one step
    /// (a temporary file beside it, flushed, then renamed over it), then
    /// tells window `hwnd` with `msg`.
    pub fn start(
        entries: Vec<Entry>,
        path: PathBuf,
        generation: u64,
        hwnd: usize,
        msg: u32,
    ) -> std::io::Result<Saving> {
        let outcome = Arc::new(Mutex::new(None));
        let (o, to) = (Arc::clone(&outcome), path.clone());
        let rows = entries.len();
        std::thread::Builder::new()
            .name("goz-list-editor-save".into())
            .spawn(move || {
                let bytes = super::to_efu(&entries);
                drop(entries);
                let result = crate::store::file::write(&to, &bytes, None)
                    .map(|_| ())
                    .map_err(|e| e.to_string());
                *lock(&o) = Some(result);
                if hwnd != 0 {
                    // SAFETY: a plain post to the editor's window; a window
                    // gone since just drops it.
                    unsafe { PostMessageW(hwnd as _, msg, 0, 0) };
                }
            })?;
        Ok(Saving {
            path,
            generation,
            rows,
            outcome,
        })
    }

    /// The outcome, once the save is over.
    pub fn outcome(&self) -> Option<Result<(), String>> {
        lock(&self.outcome).clone()
    }

    /// Waits for the save to be over (at most `limit`): the window is
    /// closing.
    pub fn wait(&self, limit: Duration) -> Option<Result<(), String>> {
        let end = Instant::now() + limit;
        loop {
            if let Some(o) = self.outcome() {
                return Some(o);
            }
            if Instant::now() >= end {
                return None;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        crate::store::file::tests::fresh_dir("goz-gui-listedit", name)
    }

    /// Runs a job to its end and gathers what it found.
    fn run(sources: Vec<Source>) -> Found {
        let job = Job::start(sources, 0, 0).unwrap();
        let mut all = Vec::new();
        loop {
            let f = job.take();
            all.extend(f.entries);
            if f.done {
                return Found { entries: all, ..f };
            }
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    fn folders_with_and_without_subfolders_and_items() {
        let dir = scratch("walk");
        let tree = dir.join("tree");
        std::fs::create_dir_all(tree.join("sub").join("deeper")).unwrap();
        std::fs::write(tree.join("a.txt"), "hello").unwrap();
        std::fs::write(tree.join("sub").join("b.txt"), "x").unwrap();
        std::fs::write(tree.join("sub").join("deeper").join("c.txt"), "").unwrap();
        let root = tree.display().to_string();
        let names = |f: &Found| -> Vec<String> {
            f.entries
                .iter()
                .map(|e| e.path.strip_prefix(&root).unwrap_or(&e.path).to_owned())
                .collect()
        };
        let all = run(vec![Source::Folder {
            path: format!("{root}\\"),
            subfolders: true,
        }]);
        assert_eq!(
            names(&all),
            [
                "",
                r"\a.txt",
                r"\sub",
                r"\sub\b.txt",
                r"\sub\deeper",
                r"\sub\deeper\c.txt"
            ]
        );
        assert_eq!(all.seen, 6);
        assert_eq!(all.unreadable, 0);
        let a = &all.entries[1];
        assert_eq!((a.size, a.is_dir), (Some(5), false));
        assert!(a.modified.is_some() && a.created.is_some());
        assert!(all.entries[0].is_dir && all.entries[0].size.is_none());
        let top = run(vec![Source::Folder {
            path: root.clone(),
            subfolders: false,
        }]);
        assert_eq!(names(&top), ["", r"\a.txt", r"\sub"]);
        let items = run(vec![
            Source::Item(tree.join("a.txt").display().to_string()),
            Source::Item(tree.join("missing.txt").display().to_string()),
            Source::Item(root.clone()),
        ]);
        assert_eq!(names(&items), [r"\a.txt", ""]);
        assert_eq!(items.unreadable, 1);
        // A stopped job is over at once, with what it had.
        let job = Job::start(
            vec![Source::Folder {
                path: root.clone(),
                subfolders: true,
            }],
            0,
            0,
        )
        .unwrap();
        job.stop();
        loop {
            let f = job.take();
            if f.done {
                assert!(f.stopped);
                break;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(!all.stopped);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_list_is_saved_in_one_step_and_read_back() {
        let dir = scratch("save");
        let file = dir.join("out.efu");
        std::fs::write(&file, "old").unwrap();
        let entries = vec![
            Entry {
                path: r"C:\x".into(),
                is_dir: true,
                attributes: Some(16),
                ..Entry::default()
            },
            Entry {
                path: r"C:\x\a.txt".into(),
                size: Some(3),
                modified: Some(133_000_000_000_000_000),
                created: Some(132_000_000_000_000_000),
                attributes: Some(32),
                ..Entry::default()
            },
        ];
        let saving = Saving::start(entries.clone(), file.clone(), 7, 0, 0).unwrap();
        assert_eq!(saving.wait(Duration::from_secs(10)), Some(Ok(())));
        assert_eq!((saving.generation, saving.rows), (7, 2));
        // No temporary file is left beside it.
        let left: Vec<_> = std::fs::read_dir(&dir).unwrap().flatten().collect();
        assert_eq!(left.len(), 1);
        let back = run(vec![Source::List(file.clone())]);
        assert_eq!(back.entries, entries);
        assert!(back.error.is_none());
        let missing = run(vec![Source::List(dir.join("missing.efu"))]);
        assert!(missing.error.is_some());
        // A folder that is not there: the save fails, and says so.
        let bad = Saving::start(entries, dir.join("out.efu").join("x.efu"), 1, 0, 0).unwrap();
        assert!(matches!(bad.wait(Duration::from_secs(10)), Some(Err(_))));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
