//! What the shell says about each file type, by extension: its icon in the
//! system image lists (the Name column's icon and a thumbnail cell's type
//! icon) and its name (the Type column), looked up on a thread of their
//! own.
//!
//! The shell answers both from the registry without touching any file,
//! but its first answer for an extension costs about 1 ms (up to 6), and
//! the first draw of each icon from the system image list costs as much
//! again (up to 16 ms: the list reads the icon from its file then), so a
//! page showing a few dozen types never seen before held the window for
//! 100 ms to most of a second as it first painted. A paint now draws what
//! is known, and for a type not known yet the document icon of a file of
//! no known type and an empty Type cell; the types it lacked are asked for
//! together once the paint is over (one posted message). The thread looks
//! each up with one call (icon and name at once) and draws its icon once
//! where nobody sees it, so the list has it loaded before the window
//! draws it; the list paints again as answers come, at most every 40 ms
//! while they keep coming. Each extension is asked once and its answer is
//! kept for the process's life (a type an entry: a few hundred at most).
//! The thread starts with the window and first loads the two icons every
//! list draws before any answer: the folder's and the document's.
//!
//! Text that must be right at once (Copy Property's Type, File > Export,
//! the preview pane's details) asks the shell for the name itself, on its
//! own thread (`filetype::name`); the answers the thread gives are kept
//! where that looks first, so those seldom need to.

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, HWND};
use windows_sys::Win32::Graphics::Gdi::{CreateBitmap, CreateCompatibleDC, SelectObject};
use windows_sys::Win32::System::Com::{
    COINIT_APARTMENTTHREADED, COINIT_DISABLE_OLE1DDE, CoInitializeEx,
};
use windows_sys::Win32::System::Threading::{
    CreateEventW, INFINITE, ResetEvent, SetEvent, WaitForSingleObject,
};
use windows_sys::Win32::UI::Controls::{HIMAGELIST, ILD_TRANSPARENT, ImageList_Draw};
use windows_sys::Win32::UI::WindowsAndMessaging::PostMessageW;

use crate::filetype::{self, Key};

/// While answers keep coming, the window hears of them at most this often.
const TELL_EVERY: Duration = Duration::from_millis(40);

/// The shell's answer for one type.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Answer {
    pub key: Key,
    /// Its icon's index in the system image lists, if the shell gave one.
    pub icon: Option<i32>,
    /// Its name; empty when the shell has none.
    pub name: Vec<u16>,
}

/// The types asked of the thread and those it answered, kept on the
/// window's thread.
#[derive(Debug, Default)]
struct Book {
    /// Every type asked for, answered or not: none is asked twice.
    asked: HashSet<Key>,
    /// Asked during the paint now going on, sent when it is over.
    batch: Vec<Key>,
    /// The answers' icons, by extension (`None`: the shell gave none).
    icons: HashMap<Vec<u16>, Option<i32>>,
}

impl Book {
    /// Notes `key` to be asked for, unless it was. True when it is the
    /// first of its batch: the batch is to be sent after the paint.
    fn ask(&mut self, key: &Key) -> bool {
        if self.asked.contains(key) {
            return false;
        }
        self.asked.insert(key.clone());
        self.batch.push(key.clone());
        self.batch.len() == 1
    }

    /// The batch, to send to the thread.
    fn take_batch(&mut self) -> Vec<Key> {
        std::mem::take(&mut self.batch)
    }

    /// Keeps an answer's icon (a file type's: folders have one icon).
    fn answered(&mut self, key: Key, icon: Option<i32>) {
        if let Some(ext) = key {
            self.icons.insert(ext, icon);
        }
    }
}

/// Puts a batch on the thread's queue, a stack: the newest batch is
/// looked up first (what is on screen now), each in the order its paint
/// asked (top to bottom).
fn queue_batch(queue: &mut Vec<Key>, batch: Vec<Key>) {
    queue.extend(batch.into_iter().rev());
}

struct Shared {
    queue: Mutex<Vec<Key>>,
    answers: Mutex<Vec<Answer>>,
    /// Set while the window has a message about answers not yet taken.
    told: AtomicBool,
    work: usize,
    hwnd: usize,
    msg: u32,
    /// The system image list of small icons, which the window draws from.
    list: usize,
}

pub struct TypeInfo {
    shared: Arc<Shared>,
    book: RefCell<Book>,
    /// Posted to the window when a paint asked for types: the batch goes
    /// to the thread when it comes, once the paint is over.
    ask_msg: u32,
    /// The thread runs. If it could not start, the types stay unknown
    /// (the generic icon, no name) and are asked again as they are drawn.
    started: Cell<bool>,
}

impl TypeInfo {
    /// Answers are told to `hwnd` as `msg` (no parameters); `ask_msg` is
    /// posted to it to send what a paint asked for. `list` is the small
    /// system image list the window draws the answers' icons from; the
    /// thread starts now and loads `first` from it (the icons drawn before
    /// any answer comes).
    pub fn new(hwnd: HWND, msg: u32, ask_msg: u32, list: HIMAGELIST, first: Vec<i32>) -> TypeInfo {
        // SAFETY: an unnamed manual-reset event.
        let work = unsafe { CreateEventW(core::ptr::null(), 1, 0, core::ptr::null()) } as usize;
        let info = TypeInfo {
            shared: Arc::new(Shared {
                queue: Mutex::new(Vec::new()),
                answers: Mutex::new(Vec::new()),
                told: AtomicBool::new(false),
                work,
                hwnd: hwnd as usize,
                msg,
                list: list as usize,
            }),
            book: RefCell::new(Book::default()),
            ask_msg,
            started: Cell::new(false),
        };
        info.start(first);
        info
    }

    /// Starts the thread, which first loads `first`.
    fn start(&self, first: Vec<i32>) {
        let shared = Arc::clone(&self.shared);
        let spawned = std::thread::Builder::new()
            .name("goz-types".into())
            .spawn(move || lookup_thread(&shared, first));
        self.started.set(spawned.is_ok());
    }

    /// The icon of files of extension `ext` (case-folded), if the thread
    /// has answered with one; else `None`, and it is asked for if it was
    /// not.
    pub fn icon(&self, ext: &[u16]) -> Option<i32> {
        if let Some(&icon) = self.book.borrow().icons.get(ext) {
            return icon;
        }
        self.ask(&Some(ext.to_vec()));
        None
    }

    /// The name of type `key` (`filetype::key`) as the Type column shows
    /// it, if it is known; else it is asked for and `None` is given.
    pub fn name(&self, key: &Key) -> Option<Rc<[u16]>> {
        let known = filetype::known(key);
        if known.is_none() {
            self.ask(key);
        }
        known
    }

    fn ask(&self, key: &Key) {
        let first = self.book.borrow_mut().ask(key);
        if first {
            // SAFETY: posts to this thread's window; the batch goes once
            // the paint asking is over (`send`).
            unsafe { PostMessageW(self.shared.hwnd as HWND, self.ask_msg, 0, 0) };
        }
    }

    /// The window's `ask_msg`: what the paints since asked for goes to the
    /// thread.
    pub fn send(&self) {
        let batch = self.book.borrow_mut().take_batch();
        if batch.is_empty() {
            return;
        }
        if !self.started.get() {
            self.start(Vec::new());
        }
        if !self.started.get() {
            // No thread: asked again as they are drawn again.
            let mut book = self.book.borrow_mut();
            for key in &batch {
                book.asked.remove(key);
            }
            return;
        }
        queue_batch(
            &mut self.shared.queue.lock().unwrap_or_else(|e| e.into_inner()),
            batch,
        );
        // SAFETY: a live event handle.
        unsafe { SetEvent(self.shared.work as HANDLE) };
    }

    /// The thread's message came: its answers are kept (the names where
    /// `filetype` looks first). True if there were any: the list is to
    /// paint again.
    pub fn arrived(&self) -> bool {
        self.shared.told.store(false, Ordering::SeqCst);
        let answers = std::mem::take(
            &mut *self
                .shared
                .answers
                .lock()
                .unwrap_or_else(|e| e.into_inner()),
        );
        let any = !answers.is_empty();
        let mut book = self.book.borrow_mut();
        for a in answers {
            filetype::learn(a.key.clone(), a.name);
            book.answered(a.key, a.icon);
        }
        any
    }
}

fn lookup_thread(shared: &Shared, first: Vec<i32>) {
    // SAFETY: the shell wants COM on a thread that asks it about types.
    unsafe {
        CoInitializeEx(
            core::ptr::null(),
            (COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE) as u32,
        )
    };
    // SAFETY: a memory DC with a 1 x 1 bitmap, kept for the thread's life
    // (which ends with the process's, as the icon threads' do): drawing an
    // icon there is what makes the list load it (as `bigicons.rs` does for
    // the big lists).
    let dc = unsafe {
        let dc = CreateCompatibleDC(core::ptr::null_mut());
        let bitmap = CreateBitmap(1, 1, 1, 32, core::ptr::null());
        SelectObject(dc, bitmap);
        dc
    };
    // The icons drawn (so loaded) here: many types share one.
    let mut loaded = HashSet::new();
    let mut load = |icon: i32| {
        if icon >= 0 && loaded.insert(icon) {
            // SAFETY: the system image list lives as long as the process,
            // and takes draws from any thread.
            unsafe { ImageList_Draw(shared.list as HIMAGELIST, icon, dc, 0, 0, ILD_TRANSPARENT) };
        }
    };
    for icon in first {
        load(icon);
    }
    let mut last_told = Instant::now();
    loop {
        let next = {
            let mut q = shared.queue.lock().unwrap_or_else(|e| e.into_inner());
            let next = q.pop();
            if next.is_none() {
                // Reset while the queue is held: a batch sent after this
                // sets it again.
                // SAFETY: a live event handle.
                unsafe { ResetEvent(shared.work as HANDLE) };
            }
            next
        };
        let Some(key) = next else {
            // SAFETY: a live event handle.
            unsafe { WaitForSingleObject(shared.work as HANDLE, INFINITE) };
            continue;
        };
        // Folders have one icon, known from the start: only their name.
        let (icon, name) = filetype::look_up(key.as_deref(), key.is_some());
        if let Some(icon) = icon {
            load(icon);
        }
        let idle = {
            shared
                .answers
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .push(Answer { key, icon, name });
            shared
                .queue
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .is_empty()
        };
        if (idle || last_told.elapsed() >= TELL_EVERY) && !shared.told.swap(true, Ordering::SeqCst)
        {
            last_told = Instant::now();
            // SAFETY: posts to the window; if it is gone, nothing is owed.
            if unsafe { PostMessageW(shared.hwnd as HWND, shared.msg, 0, 0) } == 0 {
                shared.told.store(false, Ordering::SeqCst);
            }
        }
    }
}

impl Drop for Shared {
    fn drop(&mut self) {
        // SAFETY: the event is owned by `Shared` and closed once.
        unsafe { CloseHandle(self.work as HANDLE) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn k(s: &str) -> Key {
        Some(s.encode_utf16().collect())
    }

    #[test]
    fn each_type_is_asked_once_and_a_paint_sends_one_batch() {
        let mut b = Book::default();
        // The first of a paint's types posts the message; the others join.
        assert!(b.ask(&k("txt")));
        assert!(!b.ask(&k("log")));
        assert!(!b.ask(&k("txt")), "asked already");
        assert!(!b.ask(&None));
        assert_eq!(b.take_batch(), [k("txt"), k("log"), None]);
        assert!(b.take_batch().is_empty());
        // Never again, answered or not: a later paint asks for nothing.
        assert!(!b.ask(&k("txt")));
        assert!(!b.ask(&None));
        assert!(b.batch.is_empty());
        // A new type starts a new batch.
        assert!(b.ask(&k("md")));
        assert_eq!(b.take_batch(), [k("md")]);
        b.answered(k("txt"), Some(70));
        b.answered(k("odd"), None);
        b.answered(None, None);
        let ext = |s: &str| s.encode_utf16().collect::<Vec<u16>>();
        assert_eq!(b.icons.get(&ext("txt")[..]), Some(&Some(70)));
        assert_eq!(b.icons.get(&ext("odd")[..]), Some(&None));
        assert_eq!(b.icons.len(), 2, "a folder's icon is not kept here");
    }

    #[test]
    fn the_newest_batch_is_looked_up_first_in_its_own_order() {
        let mut q = Vec::new();
        queue_batch(&mut q, vec![k("a"), k("b")]);
        queue_batch(&mut q, vec![k("c"), k("d"), k("e")]);
        let order: Vec<Key> = std::iter::from_fn(|| q.pop()).collect();
        assert_eq!(order, [k("c"), k("d"), k("e"), k("a"), k("b")]);
    }

    #[test]
    fn answers_are_kept_where_the_columns_look() {
        // On this thread, as the window's: a name the thread answered is
        // the one the Type column, Copy Property and the rest find.
        let key = k("goz-typeinfo-test");
        assert_eq!(filetype::known(&key), None);
        filetype::learn(key.clone(), "Goz Test File".encode_utf16().collect());
        let name = filetype::known(&key).expect("kept");
        assert_eq!(String::from_utf16_lossy(&name), "Goz Test File");
    }

    #[test]
    fn the_shell_answers_a_type_with_its_icon_and_name() {
        // The machine's words differ; that a common type has both does not.
        let (icon, name) = filetype::look_up(Some(&"txt".encode_utf16().collect::<Vec<_>>()), true);
        assert!(icon.is_some_and(|i| i >= 0));
        assert!(!name.is_empty());
        // Only the name: no icon is looked up.
        let (icon, name) = filetype::look_up(None, false);
        assert_eq!(icon, None);
        assert!(!name.is_empty(), "folders have a type name");
    }
}
