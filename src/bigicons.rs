//! The system's big icons (48 and 256 px) for the thumbnail views, drawn
//! without waiting for them to load.
//!
//! The shell's big image lists load an icon the first time it is drawn,
//! reading it from its file at that size: 1 to 9 ms for a program's own
//! icon, so a page of Large cells over a folder of programs held the window
//! for 100 to 180 ms while the cells drew those. The cells draw their
//! type's icon now (a program's own comes as its thumbnail, made off these
//! lists by `bigicon.rs`; a shortcut to a document shows its target type's
//! icon, also from these lists), but each type's big icon is still read
//! the first time it is drawn. Here an icon is drawn only if it is loaded
//! (`ILD_ASYNC`); one that is not is loaded on a thread of its own, by
//! drawing it there, and the window is told (a posted message) so that it
//! paints again.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
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
use windows_sys::Win32::UI::Controls::{
    HIMAGELIST, ILD_ASYNC, ILD_TRANSPARENT, IMAGELISTDRAWPARAMS, ImageList_Draw,
    ImageList_DrawIndirect,
};
use windows_sys::Win32::UI::WindowsAndMessaging::PostMessageW;

/// Icons waiting at most; older ones have scrolled away.
const MAX_QUEUED: usize = 256;

/// While icons keep coming, the window hears of them at most this often.
const TELL_EVERY: Duration = Duration::from_millis(40);

/// An icon of one of the system's image lists: the list (its handle) and
/// the icon's index in it.
pub type Key = (usize, i32);

/// What has been asked of the thread, kept on the window's thread.
#[derive(Debug, Default)]
struct Book {
    /// Each icon asked for, and whether the thread has loaded it.
    asked: HashMap<Key, bool>,
}

impl Book {
    /// Whether `key` is to be asked for now: not asked yet.
    fn wants(&self, key: Key) -> bool {
        !self.asked.contains_key(&key)
    }

    /// `key` was queued; `dropped` (pushed out of the full queue) is
    /// forgotten, so that it is asked for again when it is drawn again.
    fn queued(&mut self, key: Key, dropped: Option<Key>) {
        if let Some(d) = dropped {
            self.asked.remove(&d);
        }
        self.asked.insert(key, false);
    }

    /// The thread loaded these.
    fn loaded(&mut self, keys: impl IntoIterator<Item = Key>) -> bool {
        let mut any = false;
        for k in keys {
            if let Some(done) = self.asked.get_mut(&k) {
                *done = true;
                any = true;
            }
        }
        any
    }

    /// Whether the thread has loaded `key` (and it still does not draw).
    fn was_loaded(&self, key: Key) -> bool {
        self.asked.get(&key).copied().unwrap_or(false)
    }

    /// Whether `key` is asked for and not loaded yet.
    fn waiting(&self, key: Key) -> bool {
        self.asked.get(&key) == Some(&false)
    }

    /// The queue was emptied: what was in it is not asked for any more.
    fn unqueued(&mut self, keys: impl IntoIterator<Item = Key>) {
        for k in keys {
            if self.asked.get(&k) == Some(&false) {
                self.asked.remove(&k);
            }
        }
    }
}

/// Puts `key` at the end of `queue` (the newest, loaded first), unless it
/// is there; the oldest goes when it is full, and is given back.
fn push(queue: &mut Vec<Key>, key: Key) -> Option<Key> {
    if queue.contains(&key) {
        return None;
    }
    let dropped = (queue.len() >= MAX_QUEUED).then(|| queue.remove(0));
    queue.push(key);
    dropped
}

struct Shared {
    queue: Mutex<Vec<Key>>,
    done: Mutex<Vec<Key>>,
    /// Set while the window has a message about loaded icons not yet taken.
    told: AtomicBool,
    work: usize,
    hwnd: usize,
    msg: u32,
}

pub struct BigIcons {
    shared: Arc<Shared>,
    book: RefCell<Book>,
    started: Cell<bool>,
}

impl BigIcons {
    /// Icons loaded are told to `hwnd` as `msg` (no parameters). The thread
    /// starts with the first icon to load.
    pub fn new(hwnd: HWND, msg: u32) -> BigIcons {
        // SAFETY: an unnamed manual-reset event.
        let work = unsafe { CreateEventW(core::ptr::null(), 1, 0, core::ptr::null()) } as usize;
        BigIcons {
            shared: Arc::new(Shared {
                queue: Mutex::new(Vec::new()),
                done: Mutex::new(Vec::new()),
                told: AtomicBool::new(false),
                work,
                hwnd: hwnd as usize,
                msg,
            }),
            book: RefCell::new(Book::default()),
            started: Cell::new(false),
        }
    }

    /// Draws as `params` says if the icon is loaded; else it is loaded on
    /// the thread (when `ask`) and false is given. An icon the thread
    /// loaded that still does not draw this way is drawn as it comes.
    pub fn draw(&self, params: &IMAGELISTDRAWPARAMS, ask: bool) -> bool {
        let quick = IMAGELISTDRAWPARAMS {
            fStyle: params.fStyle | ILD_ASYNC,
            ..*params
        };
        // SAFETY: the caller's parameters, for a system image list.
        if unsafe { ImageList_DrawIndirect(&quick) } != 0 {
            return true;
        }
        let key = (params.himl as usize, params.i);
        if self.book.borrow().was_loaded(key) {
            // SAFETY: as above.
            return unsafe { ImageList_DrawIndirect(params) } != 0;
        }
        if ask {
            self.load(params.himl, params.i);
        }
        false
    }

    /// Whether icon `icon` of `list` was asked of the thread and is not
    /// loaded yet.
    pub fn waiting(&self, list: HIMAGELIST, icon: i32) -> bool {
        self.book.borrow().waiting((list as usize, icon))
    }

    /// Icon `icon` of `list` is to be loaded on the thread, unless it was
    /// asked for already.
    pub fn load(&self, list: HIMAGELIST, icon: i32) {
        let key = (list as usize, icon);
        if icon < 0 || list == 0 || !self.book.borrow().wants(key) {
            return;
        }
        let dropped = {
            let mut q = self.shared.queue.lock().unwrap_or_else(|e| e.into_inner());
            push(&mut q, key)
        };
        self.book.borrow_mut().queued(key, dropped);
        if !self.started.replace(true) {
            let shared = Arc::clone(&self.shared);
            let spawned = std::thread::Builder::new()
                .name("goz-big-icons".into())
                .spawn(move || load_thread(&shared));
            if spawned.is_err() {
                self.started.set(false);
                return;
            }
        }
        // SAFETY: a live event handle.
        unsafe { SetEvent(self.shared.work as HANDLE) };
    }

    /// The thread's message came: true if it loaded icons since.
    pub fn arrived(&self) -> bool {
        self.shared.told.store(false, Ordering::SeqCst);
        let done = std::mem::take(&mut *self.shared.done.lock().unwrap_or_else(|e| e.into_inner()));
        self.book.borrow_mut().loaded(done)
    }

    /// The icons waiting are not wanted any more (another size, or none).
    pub fn forget_queued(&self) {
        let queued =
            std::mem::take(&mut *self.shared.queue.lock().unwrap_or_else(|e| e.into_inner()));
        self.book.borrow_mut().unqueued(queued);
    }
}

fn load_thread(shared: &Shared) {
    // SAFETY: the shell wants COM on a thread that loads its icons.
    unsafe {
        CoInitializeEx(
            core::ptr::null(),
            (COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE) as u32,
        )
    };
    // SAFETY: a memory DC with a 1 × 1 bitmap, kept for the thread's life
    // (which ends with the process's, as the icon thread's does): drawing
    // an icon there is what makes the list load it.
    let dc = unsafe {
        let dc = CreateCompatibleDC(core::ptr::null_mut());
        let bitmap = CreateBitmap(1, 1, 1, 32, core::ptr::null());
        SelectObject(dc, bitmap);
        dc
    };
    let mut last_told = Instant::now();
    loop {
        let next = {
            let mut q = shared.queue.lock().unwrap_or_else(|e| e.into_inner());
            let next = q.pop();
            if next.is_none() {
                // SAFETY: a live event handle.
                unsafe { ResetEvent(shared.work as HANDLE) };
            }
            next
        };
        let Some((list, icon)) = next else {
            // SAFETY: a live event handle.
            unsafe { WaitForSingleObject(shared.work as HANDLE, INFINITE) };
            continue;
        };
        // SAFETY: the system image list lives as long as the process, and
        // takes draws from any thread.
        unsafe { ImageList_Draw(list as HIMAGELIST, icon, dc, 0, 0, ILD_TRANSPARENT) };
        shared
            .done
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push((list, icon));
        let idle = shared
            .queue
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .is_empty();
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

    #[test]
    fn an_icon_is_asked_for_once_until_it_is_dropped() {
        let mut book = Book::default();
        let k = (1, 7);
        assert!(book.wants(k));
        book.queued(k, None);
        assert!(!book.wants(k), "asked, not loaded yet: not asked again");
        assert!(!book.was_loaded(k));
        assert!(book.waiting(k));
        assert!(!book.waiting((1, 9)), "never asked: not waited for");
        // Pushed out of the queue by a newer one: asked again later.
        book.queued((1, 8), Some(k));
        assert!(book.wants(k));
        assert!(!book.wants((1, 8)));
    }

    #[test]
    fn loaded_icons_are_noted() {
        let mut book = Book::default();
        book.queued((1, 7), None);
        book.queued((2, 7), None);
        assert!(book.loaded([(1, 7)]));
        assert!(book.was_loaded((1, 7)));
        assert!(!book.waiting((1, 7)));
        assert!(!book.was_loaded((2, 7)), "the same icon of the other list");
        // One never asked for (forgotten meanwhile) is not news.
        assert!(!book.loaded([(3, 3)]));
        assert!(!book.wants((1, 7)));
    }

    #[test]
    fn icons_no_longer_queued_are_asked_for_again() {
        let mut book = Book::default();
        book.queued((1, 1), None);
        book.queued((1, 2), None);
        book.loaded([(1, 1)]);
        book.unqueued([(1, 1), (1, 2)]);
        // A loaded one stays loaded; the one still waiting is forgotten.
        assert!(book.was_loaded((1, 1)));
        assert!(book.wants((1, 2)));
    }

    #[test]
    fn the_queue_keeps_the_newest() {
        let mut q = Vec::new();
        for i in 0..MAX_QUEUED as i32 {
            assert_eq!(push(&mut q, (1, i)), None);
        }
        // Already there: not twice.
        assert_eq!(push(&mut q, (1, 5)), None);
        assert_eq!(q.len(), MAX_QUEUED);
        // Full: the oldest goes.
        assert_eq!(push(&mut q, (1, 1000)), Some((1, 0)));
        assert_eq!(q.len(), MAX_QUEUED);
        assert_eq!(q.last(), Some(&(1, 1000)));
    }
}
