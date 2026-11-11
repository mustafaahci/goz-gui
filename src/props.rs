//! Wave 9: the values of the Date Created, Date Accessed, Attributes and
//! Date Recently Changed columns, and where they come from.
//!
//! A row gets them from its source when the source has them: a file list's
//! own columns (its `Date Created`, its `Attributes`), or gozd's answer
//! once gozd lists them (the "columns" feature; stream B2 fills them in
//! [`crate::client::Row`]'s `from_item`, and asks for them with the
//! [`Want`] a search carries). A row of the index's results without them
//! has them read from the disk, under the user's own token, for the rows
//! drawn only: the first draw of such a cell asks this module's thread,
//! which reads the file's attributes and dates with one
//! `GetFileAttributesExW` (no handle is opened, nothing is read of the
//! file, its date accessed stays as it was) and tells the window, which
//! draws the row again. The values are kept in the row ([`More`]) while it
//! is held; a refresh re-reads them, showing the old ones meanwhile, as
//! the icons do. Date Recently Changed is no property of the file: only an
//! index that records changes has it (Everything's "Index recent changes"),
//! so without gozd's it stays blank, as in Everything with the option off.
//!
//! The thread starts with the first read asked for, and asks are dropped
//! when the results change (`reset`), as the icon thread's are.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use windows_sys::Win32::Foundation::{CloseHandle, FILETIME, HANDLE, HWND};
use windows_sys::Win32::Storage::FileSystem::{
    GetFileAttributesExW, GetFileExInfoStandard, WIN32_FILE_ATTRIBUTE_DATA,
};
use windows_sys::Win32::System::Threading::{
    CreateEventW, INFINITE, ResetEvent, SetEvent, WaitForSingleObject,
};
use windows_sys::Win32::UI::WindowsAndMessaging::PostMessageW;

use crate::client::Row;
use crate::columns::{COL_ACCESSED, COL_ATTRIB, COL_CREATED, COL_RECENT};
use crate::wide::nul;

/// Reads waiting at most; older ones have scrolled away.
const MAX_QUEUED: usize = 256;

/// The columns a search's rows are drawn with, of those a source may or
/// may not have: what stream B2 asks gozd for (`want_ctime`, ...) when
/// gozd lists the "columns" feature.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Want {
    pub ctime: bool,
    pub atime: bool,
    pub attrib: bool,
    pub rtime: bool,
}

impl Want {
    /// What the shown columns `ids` want.
    pub fn of_columns(ids: &[usize]) -> Want {
        Want {
            ctime: ids.contains(&COL_CREATED),
            atime: ids.contains(&COL_ACCESSED),
            attrib: ids.contains(&COL_ATTRIB),
            rtime: ids.contains(&COL_RECENT),
        }
    }
}

const ATTRIB: u8 = 1;
const CTIME: u8 = 2;
const ATIME: u8 = 4;
const RTIME: u8 = 8;
/// Its values came with the row (a file list's, gozd's): the disk is not
/// asked.
const GIVEN: u8 = 16;
/// Asked of the thread, no answer yet.
const ASKED: u8 = 32;
/// Read from the disk for this row (or the read failed).
const READ: u8 = 64;

/// A row's values of the wave 9 columns, and where they stand.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct More {
    flags: u8,
    attrib: u32,
    ctime: i64,
    atime: i64,
    rtime: i64,
}

impl More {
    /// Values that came with the row: none of them is read from the disk.
    pub fn given(
        attrib: Option<u32>,
        ctime: Option<i64>,
        atime: Option<i64>,
        rtime: Option<i64>,
    ) -> More {
        let mut m = More {
            flags: GIVEN,
            ..More::default()
        };
        m.set(attrib, ctime, atime);
        if let Some(t) = rtime {
            m.rtime = t;
            m.flags |= RTIME;
        }
        m
    }

    fn set(&mut self, attrib: Option<u32>, ctime: Option<i64>, atime: Option<i64>) {
        self.flags &= !(ATTRIB | CTIME | ATIME);
        if let Some(a) = attrib {
            self.attrib = a;
            self.flags |= ATTRIB;
        }
        if let Some(t) = ctime {
            self.ctime = t;
            self.flags |= CTIME;
        }
        if let Some(t) = atime {
            self.atime = t;
            self.flags |= ATIME;
        }
    }

    pub fn attrib(&self) -> Option<u32> {
        (self.flags & ATTRIB != 0).then_some(self.attrib)
    }

    pub fn ctime(&self) -> Option<i64> {
        (self.flags & CTIME != 0).then_some(self.ctime)
    }

    pub fn atime(&self) -> Option<i64> {
        (self.flags & ATIME != 0).then_some(self.atime)
    }

    pub fn rtime(&self) -> Option<i64> {
        (self.flags & RTIME != 0).then_some(self.rtime)
    }

    /// Whether the disk is to be asked: its values did not come with the
    /// row, and were neither read nor asked for yet.
    pub fn wants_disk(&self) -> bool {
        self.flags & (GIVEN | ASKED | READ) == 0
    }

    fn asked(self) -> More {
        More {
            flags: self.flags | ASKED,
            ..self
        }
    }

    fn unasked(self) -> More {
        More {
            flags: self.flags & !ASKED,
            ..self
        }
    }

    /// What the disk said (nothing: the file could not be read).
    pub fn read(self, disk: Option<Disk>) -> More {
        let mut m = self;
        m.flags = (m.flags & !ASKED) | READ;
        match disk {
            Some(d) => m.set(Some(d.attrib), Some(d.ctime), Some(d.atime)),
            None => m.set(None, None, None),
        }
        m
    }

    /// The same file's values on a refreshed row: shown until they are
    /// read again (values that came with the row are not carried over).
    pub fn carried(self) -> More {
        if self.flags & GIVEN != 0 {
            return More::default();
        }
        More {
            flags: self.flags & !(ASKED | READ),
            ..self
        }
    }
}

/// What the disk says of a file.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Disk {
    pub attrib: u32,
    pub ctime: i64,
    pub atime: i64,
}

fn filetime(ft: FILETIME) -> i64 {
    ((u64::from(ft.dwHighDateTime) << 32) | u64::from(ft.dwLowDateTime)) as i64
}

/// The attributes and dates of the file at `path` (no NUL), read with the
/// process's own token; `None` when it cannot be (gone, no access).
pub fn read_disk(path: &[u16]) -> Option<Disk> {
    let z = nul(path);
    // SAFETY: WIN32_FILE_ATTRIBUTE_DATA is plain data.
    let mut data: WIN32_FILE_ATTRIBUTE_DATA = unsafe { core::mem::zeroed() };
    // SAFETY: a NUL-terminated path and an out-struct of the level's type.
    let ok =
        unsafe { GetFileAttributesExW(z.as_ptr(), GetFileExInfoStandard, (&raw mut data).cast()) }
            != 0;
    ok.then(|| Disk {
        attrib: data.dwFileAttributes,
        ctime: filetime(data.ftCreationTime),
        atime: filetime(data.ftLastAccessTime),
    })
}

/// A read's answer, for row `index` of search `epoch`.
#[derive(Debug)]
pub struct Answer {
    pub epoch: u64,
    pub index: usize,
    pub disk: Option<Disk>,
}

struct Queue {
    epoch: u64,
    items: Vec<(usize, Box<[u16]>)>,
}

struct Shared {
    queue: Mutex<Queue>,
    answers: Mutex<Vec<Answer>>,
    /// Set while the window has a message about answers not yet taken.
    told: AtomicBool,
    work: usize,
    hwnd: usize,
    msg: u32,
}

impl Drop for Shared {
    fn drop(&mut self) {
        // SAFETY: the event is owned by `Shared` and closed once.
        unsafe { CloseHandle(self.work as HANDLE) };
    }
}

/// The window's disk reader.
pub struct Reader {
    hwnd: usize,
    msg: u32,
    /// The search whose rows are on screen, as `reset` last said (kept
    /// here too: the thread may start after it).
    epoch: AtomicU64,
    shared: OnceLock<Arc<Shared>>,
}

impl Reader {
    /// A reader whose answers are told to `hwnd` as `msg` (no payload:
    /// [`Reader::take`] has them). Its thread starts with the first ask.
    pub fn new(hwnd: HWND, msg: u32) -> Reader {
        Reader {
            hwnd: hwnd as usize,
            msg,
            epoch: AtomicU64::new(0),
            shared: OnceLock::new(),
        }
    }

    fn shared(&self) -> &Arc<Shared> {
        self.shared.get_or_init(|| {
            // SAFETY: an unnamed manual-reset event.
            let work = unsafe { CreateEventW(core::ptr::null(), 1, 0, core::ptr::null()) } as usize;
            let shared = Arc::new(Shared {
                queue: Mutex::new(Queue {
                    epoch: self.epoch.load(Ordering::Relaxed),
                    items: Vec::new(),
                }),
                answers: Mutex::new(Vec::new()),
                told: AtomicBool::new(false),
                work,
                hwnd: self.hwnd,
                msg: self.msg,
            });
            let thread = Arc::clone(&shared);
            std::thread::Builder::new()
                .name("goz-props".into())
                .spawn(move || run(&thread))
                .expect("spawn properties thread");
            shared
        })
    }

    /// Drops the reads asked for: rows of search `epoch` are on screen now.
    pub fn reset(&self, epoch: u64) {
        self.epoch.store(epoch, Ordering::Relaxed);
        if let Some(shared) = self.shared.get() {
            let mut q = shared.queue.lock().unwrap_or_else(|e| e.into_inner());
            q.epoch = epoch;
            q.items.clear();
        }
    }

    /// Asks for `row`'s values (row `index` of search `epoch`) when the disk
    /// is to be asked for them; the row of that search whose read was
    /// dropped to make room, if any, is to be marked unasked again
    /// ([`unask`]).
    pub fn ask(&self, row: &Row, epoch: u64, index: usize) -> Option<usize> {
        if !row.more.get().wants_disk() {
            return None;
        }
        let shared = self.shared();
        let mut q = shared.queue.lock().unwrap_or_else(|e| e.into_inner());
        if q.epoch != epoch {
            // Rows of another search: `reset` has not caught up; asked
            // again at their next draw.
            return None;
        }
        let mut dropped = None;
        if q.items.len() >= MAX_QUEUED {
            dropped = Some(q.items.remove(0).0);
        }
        q.items.push((index, row.path.clone()));
        row.more.set(row.more.get().asked());
        // SAFETY: a live event handle.
        unsafe { SetEvent(shared.work as HANDLE) };
        dropped
    }

    /// The answers come since the last call.
    pub fn take(&self) -> Vec<Answer> {
        let Some(shared) = self.shared.get() else {
            return Vec::new();
        };
        shared.told.store(false, Ordering::Release);
        std::mem::take(&mut *shared.answers.lock().unwrap_or_else(|e| e.into_inner()))
    }
}

/// `row`'s read was dropped from the queue: asked again at its next draw.
pub fn unask(row: &Row) {
    row.more.set(row.more.get().unasked());
}

fn run(shared: &Shared) {
    loop {
        let next = {
            let mut q = shared.queue.lock().unwrap_or_else(|e| e.into_inner());
            // The newest ask first: what is on screen now.
            match q.items.pop() {
                Some((index, path)) => Some((q.epoch, index, path)),
                None => {
                    // SAFETY: a live event handle.
                    unsafe { ResetEvent(shared.work as HANDLE) };
                    None
                }
            }
        };
        let Some((epoch, index, path)) = next else {
            // SAFETY: a live event handle.
            unsafe { WaitForSingleObject(shared.work as HANDLE, INFINITE) };
            continue;
        };
        let disk = read_disk(&path);
        shared
            .answers
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(Answer { epoch, index, disk });
        if !shared.told.swap(true, Ordering::AcqRel) {
            // SAFETY: a plain post (no payload) to the window that asked.
            unsafe { PostMessageW(shared.hwnd as HWND, shared.msg, 0, 0) };
        }
    }
}

// ---- Everything's text for the values ----

/// Everything's attribute letters, in the order its Attributes column
/// shows them, each with its `FILE_ATTRIBUTE_*` bit.
pub const LETTERS: [(char, u32); 13] = [
    ('R', 0x1),    // READONLY
    ('H', 0x2),    // HIDDEN
    ('S', 0x4),    // SYSTEM
    ('D', 0x10),   // DIRECTORY
    ('A', 0x20),   // ARCHIVE
    ('I', 0x2000), // NOT_CONTENT_INDEXED
    ('L', 0x400),  // REPARSE_POINT
    ('C', 0x800),  // COMPRESSED
    ('E', 0x4000), // ENCRYPTED
    ('O', 0x1000), // OFFLINE
    ('P', 0x200),  // SPARSE_FILE
    ('T', 0x100),  // TEMPORARY
    ('N', 0x80),   // NORMAL
];

/// The Attributes column's text for `attrib`: Everything's letters of the
/// bits set, in its order.
pub fn attribute_letters(attrib: u32, out: &mut Vec<u16>) {
    for (letter, bit) in LETTERS {
        if attrib & bit != 0 {
            out.push(letter as u16);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn letters(a: u32) -> String {
        let mut v = Vec::new();
        attribute_letters(a, &mut v);
        String::from_utf16_lossy(&v)
    }

    #[test]
    fn attributes_read_as_everythings_letters() {
        assert_eq!(letters(0x20), "A");
        assert_eq!(letters(0x10), "D");
        assert_eq!(letters(0x16), "HSD");
        assert_eq!(letters(0x27), "RHSA");
        assert_eq!(letters(0x80), "N");
        assert_eq!(letters(0x2000 | 0x400 | 0x10), "DIL");
        assert_eq!(letters(0x1000 | 0x200 | 0x100 | 0x800 | 0x4000), "CEOPT");
        assert_eq!(letters(0), "");
    }

    #[test]
    fn values_and_where_they_stand() {
        let m = More::default();
        assert!(m.wants_disk());
        assert_eq!(
            (m.attrib(), m.ctime(), m.atime(), m.rtime()),
            (None, None, None, None)
        );
        // Asked: not again until it is answered, or dropped.
        let asked = m.asked();
        assert!(!asked.wants_disk());
        assert!(asked.unasked().wants_disk());
        let disk = Disk {
            attrib: 0x20,
            ctime: 5,
            atime: 7,
        };
        let read = asked.read(Some(disk));
        assert!(!read.wants_disk());
        assert_eq!(
            (read.attrib(), read.ctime(), read.atime()),
            (Some(0x20), Some(5), Some(7))
        );
        assert_eq!(read.rtime(), None, "never from the disk");
        // A failed read is answered too: blank, and not asked again.
        let failed = m.asked().read(None);
        assert!(!failed.wants_disk() && failed.attrib().is_none());
        // A refresh shows the old values until they are read again.
        let carried = read.carried();
        assert!(carried.wants_disk());
        assert_eq!(carried.ctime(), Some(5));
        // Given values (gozd's, a list's) never ask the disk, nor carry.
        let given = More::given(Some(0x10), None, Some(9), Some(11));
        assert!(!given.wants_disk());
        assert_eq!(
            (given.attrib(), given.ctime(), given.atime(), given.rtime()),
            (Some(0x10), None, Some(9), Some(11))
        );
        assert_eq!(given.carried(), More::default());
    }

    #[test]
    fn want_follows_the_shown_columns() {
        use crate::columns::{COL_NAME, COL_SIZE};
        assert_eq!(Want::of_columns(&[COL_NAME, COL_SIZE]), Want::default());
        let w = Want::of_columns(&[COL_NAME, COL_ATTRIB, COL_RECENT]);
        assert!(w.attrib && w.rtime && !w.ctime && !w.atime);
    }

    #[test]
    fn a_file_on_the_disk_is_read() {
        let exe = std::env::current_exe().unwrap();
        let path: Vec<u16> = exe.as_os_str().to_string_lossy().encode_utf16().collect();
        let d = read_disk(&path).expect("the test's own exe");
        assert_eq!(d.attrib & 0x10, 0, "a file");
        assert!(d.ctime > 0 && d.atime > 0);
        let gone: Vec<u16> = "C:\\goz-gui-w9-no-such-file\\x".encode_utf16().collect();
        assert_eq!(read_disk(&gone), None);
    }

    /// The thread answers the rows of the search on screen, though it
    /// started after the window said which that is (its first ask); those
    /// of another search are not read.
    #[test]
    fn the_reader_answers_the_search_on_screen() {
        let exe = std::env::current_exe().unwrap();
        let path: Box<[u16]> = exe.as_os_str().to_string_lossy().encode_utf16().collect();
        let row = Row::new(path.clone(), None, None, false);
        let other = Row::new(path, None, None, false);
        // No window: the posts go nowhere that matters.
        let reader = Reader::new(core::ptr::null_mut(), 0);
        reader.reset(7);
        assert_eq!(reader.ask(&other, 6, 0), None);
        assert!(other.more.get().wants_disk(), "another search's: not asked");
        assert_eq!(reader.ask(&row, 7, 3), None);
        assert!(!row.more.get().wants_disk(), "asked once");
        let mut got = Vec::new();
        for _ in 0..200 {
            got.extend(reader.take());
            if !got.is_empty() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert_eq!(got.len(), 1);
        assert_eq!((got[0].epoch, got[0].index), (7, 3));
        assert!(got[0].disk.is_some_and(|d| d.attrib & 0x10 == 0));
    }
}
