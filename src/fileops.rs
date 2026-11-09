//! File operations on the results: Delete (to the Recycle Bin, or for good),
//! the copy or move of a Paste, and Rename. They go through the shell's
//! `IFileOperation`, as Explorer's do: its questions, progress, conflict
//! and error windows (owned by the main window), and its undo record.
//!
//! They run on a thread of their own, an STA made the first time one is
//! asked for, so the window keeps painting while a large copy runs. Each
//! result is posted back to the main window as a boxed [`Done`]. The jobs
//! run one after another, in the order asked.
//!
//! When goz-gui closes with a job still running, the process waits for it
//! ([`FileOps::finish`]): a copy or move is never cut off half way.

use std::cell::{Cell, OnceCell};
use std::collections::VecDeque;
use std::os::windows::io::AsRawHandle;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

use windows::Win32::Foundation::HWND as WHWND;
use windows::Win32::System::Com::{CLSCTX_ALL, CoCreateInstance};
use windows::Win32::UI::Shell::{
    FILEOPERATION_FLAGS, FileOperation, IFileOperation, IFileOperationProgressSink, IShellItem,
    SHCreateItemFromParsingName,
};
use windows::core::{HRESULT, PCWSTR};
use windows_sys::Win32::Foundation::HWND;
use windows_sys::Win32::Globalization::{CSTR_EQUAL, CompareStringOrdinal};
use windows_sys::Win32::System::Com::{
    COINIT_APARTMENTTHREADED, COINIT_DISABLE_OLE1DDE, CoInitializeEx, CoUninitialize,
};
use windows_sys::Win32::System::Threading::GetThreadId;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    DispatchMessageW, GetMessageW, MSG, PM_NOREMOVE, PeekMessageW, PostMessageW,
    PostThreadMessageW, TranslateMessage, WM_APP,
};

use crate::shell::{self, FileOp};
use crate::store::location;
use crate::wide::nul;

/// A file operation, with what the window needs to know once it is done.
#[derive(Debug)]
pub enum Job {
    /// Delete `paths`: to the Recycle Bin, or (`permanent`) for good.
    /// `next` is the file the selection moves to once they are gone.
    Delete {
        paths: Vec<Vec<u16>>,
        permanent: bool,
        next: Option<Vec<u16>>,
    },
    /// Copy (or, `moving`, move) `sources` into `folder`: a paste of the
    /// clipboard's files, whose sequence number was `clipboard` then.
    Paste {
        sources: Vec<Vec<u16>>,
        folder: Vec<u16>,
        moving: bool,
        clipboard: u32,
    },
    /// Rename the file at `path` to `name` (a name, not a path).
    Rename { path: Vec<u16>, name: Vec<u16> },
}

/// How a job ended.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// Every item was done.
    Done,
    /// The user said no or cancelled, or skipped some items: some may have
    /// been done.
    Cancelled,
    /// It failed, for this reason (the system's text).
    Failed(String),
}

/// A job and how it ended, posted to the main window.
#[derive(Debug)]
pub struct Done {
    pub job: Job,
    pub outcome: Outcome,
}

/// The message that tells the thread there is work (or that it is to end).
const WAKE: u32 = WM_APP;

/// What the window and the thread share.
struct Queue {
    jobs: Mutex<VecDeque<Job>>,
    /// The window is closing: end once the queue is empty.
    quit: AtomicBool,
    thread: u32,
}

/// The file operations thread, owned by the main window.
pub struct FileOps {
    /// The window that owns the shell's windows and gets the results.
    owner: HWND,
    /// The message the results come as; lParam is a `Box<Done>`.
    message: u32,
    /// The thread, once a job asked for it (`None`: it could not be made).
    worker: OnceCell<Option<Worker>>,
}

/// The queue the thread takes its jobs from, and the thread to wait for.
type Worker = (Arc<Queue>, Cell<Option<JoinHandle<()>>>);

impl FileOps {
    pub fn new(owner: HWND, message: u32) -> FileOps {
        FileOps {
            owner,
            message,
            worker: OnceCell::new(),
        }
    }

    /// Runs `job` on the thread, after any before it; false if the thread
    /// could not be made (the job is then not run).
    pub fn run(&self, job: Job) -> bool {
        let Some((queue, _)) = self
            .worker
            .get_or_init(|| spawn(self.owner as usize, self.message))
        else {
            return false;
        };
        queue
            .jobs
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push_back(job);
        // SAFETY: posts to a thread of this process. Before its queue
        // exists the post fails, and the thread finds the job anyway.
        unsafe { PostThreadMessageW(queue.thread, WAKE, 0, 0) };
        true
    }

    /// goz-gui is closing: waits for the jobs asked for to finish.
    pub fn finish(&self) {
        let Some(Some((queue, handle))) = self.worker.get() else {
            return;
        };
        queue.quit.store(true, Ordering::SeqCst);
        // SAFETY: as in `run`.
        unsafe { PostThreadMessageW(queue.thread, WAKE, 0, 0) };
        if let Some(handle) = handle.take() {
            let _ = handle.join();
        }
    }
}

fn spawn(owner: usize, message: u32) -> Option<Worker> {
    let (tx, rx) = std::sync::mpsc::channel::<Arc<Queue>>();
    let handle = std::thread::Builder::new()
        .name("goz-fileops".into())
        .spawn(move || {
            if let Ok(queue) = rx.recv() {
                thread(&queue, owner, message);
            }
        })
        .ok()?;
    // SAFETY: the handle of the thread just started, open while `handle`
    // lives.
    let thread = unsafe { GetThreadId(handle.as_raw_handle()) };
    let queue = Arc::new(Queue {
        jobs: Mutex::new(VecDeque::new()),
        quit: AtomicBool::new(false),
        thread,
    });
    tx.send(Arc::clone(&queue)).ok()?;
    Some((queue, Cell::new(Some(handle))))
}

/// The thread: an STA with a message loop (the shell's windows run their
/// own loops inside a job), running each job as it comes.
fn thread(queue: &Queue, owner: usize, message: u32) {
    // SAFETY: COM for the life of the thread, ended below; then a standard
    // message loop over a zeroed MSG.
    unsafe {
        CoInitializeEx(
            core::ptr::null(),
            (COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE) as u32,
        );
        let mut msg: MSG = core::mem::zeroed();
        // Makes the thread's message queue, so that a wake posted from now
        // on waits in it; jobs pushed before it are found below anyway.
        PeekMessageW(&mut msg, core::ptr::null_mut(), 0, 0, PM_NOREMOVE);
        loop {
            // After every message, not only after a wake: the loop of a
            // progress or question window inside a job may have taken it.
            loop {
                let job = queue
                    .jobs
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .pop_front();
                let Some(job) = job else { break };
                let outcome = perform(owner as HWND, &job, location::fileop_silent());
                let done = Box::into_raw(Box::new(Done { job, outcome }));
                if PostMessageW(owner as HWND, message, 0, done as isize) == 0 {
                    // The window is gone: nobody will take it.
                    drop(Box::from_raw(done));
                }
            }
            if queue.quit.load(Ordering::SeqCst) {
                break;
            }
            if GetMessageW(&mut msg, core::ptr::null_mut(), 0, 0) <= 0 {
                break;
            }
            if msg.hwnd.is_null() && msg.message == WAKE {
                continue;
            }
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
        CoUninitialize();
    }
}

/// HRESULT_FROM_WIN32(ERROR_CANCELLED), and the copy engine's own
/// "cancelled by the user".
const CANCELLED: [i32; 2] = [0x8007_04C7_u32 as i32, 0x8027_0000_u32 as i32];

const BACKSLASH: u16 = b'\\' as u16;

/// The folder that holds `path` (`C:\` for an item at the root of C:);
/// `None` for a path with no folder in it.
pub fn parent(path: &[u16]) -> Option<&[u16]> {
    let i = path.iter().rposition(|&c| c == BACKSLASH)?;
    let end = if i > 0 && path[i - 1] == u16::from(b':') {
        i + 1
    } else {
        i
    };
    (end > 0).then(|| &path[..end])
}

/// Whether `a` and `b` are the same folder as Windows compares paths: case
/// does not matter, nor does a trailing backslash.
fn same_folder(a: &[u16], b: &[u16]) -> bool {
    fn trim(mut p: &[u16]) -> &[u16] {
        while let [rest @ .., BACKSLASH] = p {
            p = rest;
        }
        p
    }
    let (a, b) = (trim(a), trim(b));
    // SAFETY: two buffers, each read for its own length.
    unsafe {
        CompareStringOrdinal(a.as_ptr(), a.len() as i32, b.as_ptr(), b.len() as i32, 1)
            == CSTR_EQUAL
    }
}

/// The shell items at `paths`; those gone are left out.
fn items(paths: &[Vec<u16>]) -> Vec<IShellItem> {
    paths.iter().filter_map(|p| item(p)).collect()
}

/// The shell item at `path`; `None` when it is gone.
fn item(path: &[u16]) -> Option<IShellItem> {
    let path = nul(path);
    // SAFETY: a NUL-terminated path that outlives the call.
    unsafe { SHCreateItemFromParsingName(PCWSTR(path.as_ptr()), None).ok() }
}

/// An operation of `kind`, owned by `owner` (its questions, progress and
/// errors; none with `silent`). COM must be initialised on this thread.
fn operation(owner: HWND, kind: FileOp, silent: bool) -> windows::core::Result<IFileOperation> {
    // SAFETY: plain COM calls; without COM on the thread they fail.
    unsafe {
        let op: IFileOperation = CoCreateInstance(&FileOperation, None, CLSCTX_ALL)?;
        if !owner.is_null() {
            op.SetOwnerWindow(WHWND(owner))?;
        }
        op.SetOperationFlags(FILEOPERATION_FLAGS(shell::fileop_flags(kind, silent)))?;
        Ok(op)
    }
}

/// Runs the `queued` items of `op`: Done, or Cancelled when any was
/// stopped or skipped. Nothing queued (every item gone) is a failure.
fn finish(op: &IFileOperation, queued: usize) -> windows::core::Result<Outcome> {
    if queued == 0 {
        return Err(HRESULT::from_win32(2).into()); // ERROR_FILE_NOT_FOUND
    }
    // SAFETY: an operation of this thread; its windows run their own loops.
    unsafe {
        let result = op.PerformOperations();
        let aborted = op
            .GetAnyOperationsAborted()
            .map(|b| b.as_bool())
            .unwrap_or(false);
        result?;
        Ok(if aborted {
            Outcome::Cancelled
        } else {
            Outcome::Done
        })
    }
}

/// Runs `job` with `IFileOperation`, owned by `owner` (its questions,
/// progress and errors; none with `silent`, `GOZ_GUI_FILEOP_SILENT`).
///
/// A paste treats what is in the target folder already as Explorer does:
/// a copy of it goes beside it under a name of its own ("name - Copy"),
/// in an operation of its own (that name is the operation's choice, made
/// for every collision in it), and a move of it is nothing to do.
pub fn perform(owner: HWND, job: &Job, silent: bool) -> Outcome {
    let none = None::<&IFileOperationProgressSink>;
    let run = || -> windows::core::Result<Outcome> {
        // SAFETY: COM is initialised on this thread (an STA); every item
        // and string lives across the calls that take it, and each
        // operation is released when its arm ends.
        unsafe {
            match job {
                Job::Delete {
                    paths, permanent, ..
                } => {
                    let kind = FileOp::Delete {
                        permanent: *permanent,
                    };
                    let op = operation(owner, kind, silent)?;
                    let items = items(paths);
                    for item in &items {
                        op.DeleteItem(item, none)?;
                    }
                    finish(&op, items.len())
                }
                Job::Paste {
                    sources,
                    folder,
                    moving,
                    ..
                } => {
                    let Some(target) = item(folder) else {
                        return Err(HRESULT::from_win32(3).into()); // ERROR_PATH_NOT_FOUND
                    };
                    let (here, elsewhere): (Vec<Vec<u16>>, Vec<Vec<u16>>) = sources
                        .iter()
                        .cloned()
                        .partition(|p| parent(p).is_some_and(|d| same_folder(d, folder)));
                    if *moving {
                        let items = items(&elsewhere);
                        if items.is_empty() && !here.is_empty() {
                            return Ok(Outcome::Done);
                        }
                        let op = operation(owner, FileOp::Transfer, silent)?;
                        for item in &items {
                            op.MoveItem(item, &target, PCWSTR::null(), none)?;
                        }
                        return finish(&op, items.len());
                    }
                    let mut outcome = Err(HRESULT::from_win32(2).into()); // ERROR_FILE_NOT_FOUND
                    for (batch, kind) in [(elsewhere, FileOp::Transfer), (here, FileOp::Duplicate)]
                    {
                        let items = items(&batch);
                        if items.is_empty() {
                            continue;
                        }
                        let op = operation(owner, kind, silent)?;
                        for item in &items {
                            op.CopyItem(item, &target, PCWSTR::null(), none)?;
                        }
                        let done = finish(&op, items.len())?;
                        // Stopped by the user: the rest is not started.
                        if done != Outcome::Done {
                            return Ok(done);
                        }
                        outcome = Ok(done);
                    }
                    outcome
                }
                Job::Rename { path, name } => {
                    let op = operation(owner, FileOp::Transfer, silent)?;
                    let items: Vec<IShellItem> = item(path).into_iter().collect();
                    let name = nul(name);
                    for item in &items {
                        op.RenameItem(item, PCWSTR(name.as_ptr()), none)?;
                    }
                    finish(&op, items.len())
                }
            }
        }
    };
    match run() {
        Ok(outcome) => outcome,
        Err(e) if CANCELLED.contains(&e.code().0) => Outcome::Cancelled,
        Err(e) => {
            let text = e.message();
            Outcome::Failed(if text.trim().is_empty() {
                format!("Error 0x{:08X}", e.code().0 as u32)
            } else {
                text.trim().to_owned()
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::{Path, PathBuf};
    use windows::Win32::System::Com::COINIT_APARTMENTTHREADED as WSTA;
    use windows::Win32::System::Com::CoInitializeEx as WCoInitializeEx;

    fn w(p: &Path) -> Vec<u16> {
        p.as_os_str().to_string_lossy().encode_utf16().collect()
    }

    /// A fresh folder of the test's own under the temporary folder.
    fn scratch(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("goz-gui-fileops-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// Copy, move, rename and a permanent delete, silently, on files the
    /// test made (the Recycle Bin is left alone).
    #[test]
    fn copy_move_rename_and_delete_for_good() {
        // SAFETY: COM for this test thread, which ends with the test.
        let _ = unsafe { WCoInitializeEx(None, WSTA) };
        let dir = scratch("ops");
        let (a, b) = (dir.join("a"), dir.join("b"));
        std::fs::create_dir_all(&a).unwrap();
        std::fs::create_dir_all(&b).unwrap();
        let one = a.join("one.txt");
        let two = a.join("two.txt");
        std::fs::write(&one, "1").unwrap();
        std::fs::write(&two, "2").unwrap();
        let none = core::ptr::null_mut();

        let copy = Job::Paste {
            sources: vec![w(&one)],
            folder: w(&b),
            moving: false,
            clipboard: 0,
        };
        assert_eq!(perform(none, &copy, true), Outcome::Done);
        assert!(one.exists() && b.join("one.txt").exists());

        let mv = Job::Paste {
            sources: vec![w(&two)],
            folder: w(&b),
            moving: true,
            clipboard: 0,
        };
        assert_eq!(perform(none, &mv, true), Outcome::Done);
        assert!(!two.exists() && b.join("two.txt").exists());

        let rename = Job::Rename {
            path: w(&b.join("two.txt")),
            name: "deux.txt".encode_utf16().collect(),
        };
        assert_eq!(perform(none, &rename, true), Outcome::Done);
        assert!(b.join("deux.txt").exists() && !b.join("two.txt").exists());

        let delete = Job::Delete {
            paths: vec![w(&b.join("deux.txt")), w(&b.join("one.txt"))],
            permanent: true,
            next: None,
        };
        assert_eq!(perform(none, &delete, true), Outcome::Done);
        assert!(!b.join("deux.txt").exists() && !b.join("one.txt").exists());

        // Nothing left to act on: a failure with the system's reason.
        let gone = Job::Delete {
            paths: vec![w(&b.join("deux.txt"))],
            permanent: true,
            next: None,
        };
        assert!(matches!(perform(none, &gone, true), Outcome::Failed(t) if !t.is_empty()));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// The names in `dir`, sorted.
    fn names(dir: &Path) -> Vec<String> {
        let mut out: Vec<String> = std::fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        out.sort();
        out
    }

    /// A paste into the folder an item is in, as Explorer: a copy goes
    /// beside it under a name of its own, a move does nothing; items from
    /// elsewhere in the same paste are copied as usual.
    #[test]
    fn a_paste_into_the_items_own_folder() {
        // SAFETY: COM for this test thread, which ends with the test.
        let _ = unsafe { WCoInitializeEx(None, WSTA) };
        let dir = scratch("same");
        let (a, b) = (dir.join("a"), dir.join("b"));
        std::fs::create_dir_all(&a).unwrap();
        std::fs::create_dir_all(&b).unwrap();
        std::fs::write(a.join("one.txt"), "1").unwrap();
        std::fs::write(b.join("two.txt"), "2").unwrap();
        let none = core::ptr::null_mut();

        // The folder spelt in another case, with a trailing backslash.
        let mut upper: Vec<u16> = String::from_utf16_lossy(&w(&a))
            .to_uppercase()
            .encode_utf16()
            .collect();
        upper.push(u16::from(b'\\'));
        let copy = Job::Paste {
            sources: vec![w(&a.join("one.txt"))],
            folder: upper,
            moving: false,
            clipboard: 0,
        };
        assert_eq!(perform(none, &copy, true), Outcome::Done);
        let got = names(&a);
        assert_eq!(got.len(), 2, "{got:?}");
        assert!(got.contains(&"one.txt".into()));
        let twin = got.iter().find(|n| *n != "one.txt").unwrap();
        assert!(twin.starts_with("one") && twin.ends_with(".txt"), "{twin}");

        let mv = Job::Paste {
            sources: vec![w(&a.join("one.txt"))],
            folder: w(&a),
            moving: true,
            clipboard: 0,
        };
        assert_eq!(perform(none, &mv, true), Outcome::Done, "nothing to do");
        assert_eq!(names(&a), got);

        // One from elsewhere and one from the folder itself.
        let both = Job::Paste {
            sources: vec![w(&a.join("one.txt")), w(&b.join("two.txt"))],
            folder: w(&b),
            moving: false,
            clipboard: 0,
        };
        assert_eq!(perform(none, &both, true), Outcome::Done);
        let in_b = names(&b);
        assert_eq!(in_b.len(), 3, "{in_b:?}");
        assert!(in_b.contains(&"one.txt".into()) && in_b.contains(&"two.txt".into()));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn folders_of_paths() {
        let s = |p: &str| parent(&w(Path::new(p))).map(String::from_utf16_lossy);
        assert_eq!(s(r"C:\a\f.txt").as_deref(), Some(r"C:\a"));
        assert_eq!(s(r"C:\f.txt").as_deref(), Some(r"C:\"));
        assert_eq!(s("f.txt"), None);
        let f = |a: &str, b: &str| same_folder(&w(Path::new(a)), &w(Path::new(b)));
        assert!(f(r"C:\Docs", r"c:\docs\"));
        assert!(f(r"C:\", r"c:"));
        assert!(!f(r"C:\docs", r"C:\docs2"));
        assert!(!f(r"C:\docs\x", r"C:\docs"));
    }
}
