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
//!
//! A cell smaller than the list's icons (Large: 128 px from the 256 px
//! list) needs them shrunk. The list's own shrink (`ILD_SCALE`) takes one
//! pixel of each two and drops whatever is finer than that: the lines of a
//! text file's page vanished, a blank page. The same thread shrinks each
//! icon once per size with an area filter, as Explorer's are; the view
//! draws that, and the list's shrink only for the moment it is being made.
//! Those bitmaps (64 KB an icon at 128 px) are one per type shown, at most
//! `SHRUNK_BUDGET`, and go with the view's size or the window's showing.

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, HWND};
use windows_sys::Win32::Graphics::Gdi::{
    CreateBitmap, CreateCompatibleDC, DeleteDC, GdiFlush, HGDIOBJ, SelectObject,
};
use windows_sys::Win32::System::Com::{
    COINIT_APARTMENTTHREADED, COINIT_DISABLE_OLE1DDE, CoInitializeEx,
};
use windows_sys::Win32::System::Threading::{
    CreateEventW, INFINITE, ResetEvent, SetEvent, WaitForSingleObject,
};
use windows_sys::Win32::UI::Controls::{
    HIMAGELIST, ILD_ASYNC, ILD_TRANSPARENT, IMAGELISTDRAWPARAMS, ImageList_Draw,
    ImageList_DrawIndirect, ImageList_GetIconSize,
};
use windows_sys::Win32::UI::WindowsAndMessaging::PostMessageW;

use crate::thumbs::{Bitmap, dib};

/// Icons waiting at most; older ones have scrolled away.
const MAX_QUEUED: usize = 256;

/// While icons keep coming, the window hears of them at most this often.
const TELL_EVERY: Duration = Duration::from_millis(40);

/// The filtered shrinks kept at most, in bytes: 64 types' icons at 128 px
/// (Large at 100 %), 28 at 192 px (Large at 150 %).
const SHRUNK_BUDGET: usize = 4 << 20;

/// An icon of one of the system's image lists: the list (its handle) and
/// the icon's index in it.
pub type Key = (usize, i32);

/// An icon of a system list (as `Key`) shrunk to a cell's side, in pixels.
pub type Shrink = (usize, i32, i32);

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

/// The icons shrunk with a filter (`P`: a bitmap, or none if it could not
/// be made), and those asked of the thread, kept on the window's thread.
#[derive(Debug)]
struct Pictures<P> {
    made: HashMap<Shrink, Option<P>>,
    asked: HashSet<Shrink>,
    /// What `made` holds, in bytes.
    bytes: usize,
}

impl<P> Default for Pictures<P> {
    fn default() -> Self {
        Pictures {
            made: HashMap::new(),
            asked: HashSet::new(),
            bytes: 0,
        }
    }
}

impl<P> Pictures<P> {
    /// The thread made `key` (`cost` bytes): kept if it is still wanted (not
    /// forgotten since it was asked for). Over the budget, the ones kept so
    /// far go first (made again when drawn again). Whether it was kept.
    fn arrived(&mut self, key: Shrink, picture: Option<P>, cost: usize) -> bool {
        if !self.asked.remove(&key) {
            return false;
        }
        if self.bytes + cost > SHRUNK_BUDGET {
            self.made.clear();
            self.bytes = 0;
        }
        self.bytes += cost;
        self.made.insert(key, picture);
        true
    }
}

/// Puts `key` at the end of `queue` (the newest, loaded first), unless it
/// is there; the oldest goes when it is full, and is given back.
fn push<K: PartialEq>(queue: &mut Vec<K>, key: K) -> Option<K> {
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
    /// Icons to shrink with a filter (after every icon to load), and those
    /// shrunk (`None`: could not be).
    shrinks: Mutex<Vec<Shrink>>,
    shrunk: Mutex<Vec<(Shrink, Option<Bitmap>)>>,
    /// Set while the window has a message about loaded icons not yet taken.
    told: AtomicBool,
    work: usize,
    hwnd: usize,
    msg: u32,
}

pub struct BigIcons {
    shared: Arc<Shared>,
    book: RefCell<Book>,
    pictures: RefCell<Pictures<Bitmap>>,
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
                shrinks: Mutex::new(Vec::new()),
                shrunk: Mutex::new(Vec::new()),
                told: AtomicBool::new(false),
                work,
                hwnd: hwnd as usize,
                msg,
            }),
            book: RefCell::new(Book::default()),
            pictures: RefCell::new(Pictures::default()),
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
        self.wake();
    }

    /// Starts the thread if it is not running, and gives it work.
    fn wake(&self) {
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

    /// Draws (`draw`) icon `icon` of `list` shrunk with a filter to `side`
    /// pixels square, if it has been: true. Else it is asked of the thread
    /// (once), and false is given: the caller draws it shrunk by the list
    /// meanwhile, or for good if it could not be made.
    pub fn draw_shrunk(
        &self,
        list: HIMAGELIST,
        icon: i32,
        side: i32,
        draw: impl FnOnce(&Bitmap),
    ) -> bool {
        if icon < 0 || list == 0 || side <= 0 {
            return false;
        }
        let key = (list as usize, icon, side);
        {
            // Drawn while borrowed: GDI only, nothing that can call back.
            let pictures = self.pictures.borrow();
            match pictures.made.get(&key) {
                Some(Some(bitmap)) => {
                    draw(bitmap);
                    return true;
                }
                Some(None) => return false,
                None if pictures.asked.contains(&key) => return false,
                None => {}
            }
        }
        let dropped = {
            let mut q = self
                .shared
                .shrinks
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            push(&mut q, key)
        };
        {
            let mut pictures = self.pictures.borrow_mut();
            if let Some(d) = dropped {
                pictures.asked.remove(&d);
            }
            pictures.asked.insert(key);
        }
        self.wake();
        false
    }

    /// The filtered shrinks are not wanted any more (the view's size
    /// changed, or the window hid): made ones go, waiting ones are not
    /// made, and one being made is thrown away when it comes.
    pub fn forget_shrunk(&self) {
        self.shared
            .shrinks
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clear();
        // Dropped (their bitmaps deleted) outside the borrow.
        let gone = std::mem::take(&mut *self.pictures.borrow_mut());
        drop(gone);
    }

    /// The thread's message came: true if it loaded or shrank icons since.
    pub fn arrived(&self) -> bool {
        self.shared.told.store(false, Ordering::SeqCst);
        let done = std::mem::take(&mut *self.shared.done.lock().unwrap_or_else(|e| e.into_inner()));
        let shrunk =
            std::mem::take(&mut *self.shared.shrunk.lock().unwrap_or_else(|e| e.into_inner()));
        let loaded = self.book.borrow_mut().loaded(done);
        let mut kept = false;
        let mut gone = Vec::new();
        {
            let mut pictures = self.pictures.borrow_mut();
            for (key, picture) in shrunk {
                let cost = picture.as_ref().map_or(0, |b| {
                    b.width.max(0) as usize * b.height.max(0) as usize * 4
                });
                if pictures.asked.contains(&key) {
                    kept |= pictures.arrived(key, picture, cost);
                } else {
                    gone.push(picture);
                }
            }
        }
        // Thrown away (no longer wanted) outside the borrow.
        drop(gone);
        loaded || kept
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
        match next_job(shared) {
            None => {
                // SAFETY: a live event handle.
                unsafe { WaitForSingleObject(shared.work as HANDLE, INFINITE) };
                continue;
            }
            Some(Job::Load((list, icon))) => {
                // SAFETY: the system image list lives as long as the
                // process, and takes draws from any thread.
                unsafe { ImageList_Draw(list as HIMAGELIST, icon, dc, 0, 0, ILD_TRANSPARENT) };
                shared
                    .done
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .push((list, icon));
            }
            Some(Job::Shrink(key)) => {
                let (list, icon, side) = key;
                let picture = make_shrunk(list as HIMAGELIST, icon, side);
                shared
                    .shrunk
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .push((key, picture));
            }
        }
        let idle = shared
            .queue
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .is_empty()
            && shared
                .shrinks
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

/// What the thread does next.
enum Job {
    Load(Key),
    Shrink(Shrink),
}

/// The next icon to load (the newest asked), else the next to shrink. With
/// none, the event is reset while both queues are held, so that one asked
/// for after this sets it again.
fn next_job(shared: &Shared) -> Option<Job> {
    let mut q = shared.queue.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(key) = q.pop() {
        return Some(Job::Load(key));
    }
    let mut s = shared.shrinks.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(key) = s.pop() {
        return Some(Job::Shrink(key));
    }
    // SAFETY: a live event handle.
    unsafe { ResetEvent(shared.work as HANDLE) };
    None
}

/// Icon `icon` of `list`, shrunk with an area filter to `side` pixels
/// square, premultiplied for `AlphaBlend`: drawn at the list's size on
/// black and on white (which gives each pixel's coverage, as
/// `bigicon::render` does), then shrunk. `None` if the list's icons are
/// not bigger than `side`, or it could not be drawn. Loads the icon if the
/// list has not.
fn make_shrunk(list: HIMAGELIST, icon: i32, side: i32) -> Option<Bitmap> {
    let (mut cx, mut cy) = (0, 0);
    // SAFETY: reads the size of a system image list.
    unsafe { ImageList_GetIconSize(list, &mut cx, &mut cy) };
    if cx != cy || cx <= side || side <= 0 {
        return None;
    }
    let (scratch, bits) = dib(cx, cx)?;
    let count = (cx * cx) as usize;
    // SAFETY: a memory DC made and deleted here holds the scratch DIB; its
    // `count` pixels are touched only through slices made after GdiFlush,
    // between the draws, while `scratch` keeps them alive. The system
    // image list takes draws from any thread.
    let full = unsafe {
        let dc = CreateCompatibleDC(core::ptr::null_mut());
        if dc.is_null() {
            return None;
        }
        let old = SelectObject(dc, scratch.handle() as HGDIOBJ);
        let pixels = bits.cast::<[u8; 4]>();
        let mut black = Vec::new();
        let mut drawn = true;
        for back in [[0, 0, 0, 0], [255, 255, 255, 0]] {
            core::slice::from_raw_parts_mut(pixels, count).fill(back);
            if ImageList_Draw(list, icon, dc, 0, 0, ILD_TRANSPARENT) == 0 {
                drawn = false;
                break;
            }
            GdiFlush();
            if black.is_empty() {
                black = core::slice::from_raw_parts(pixels, count).to_vec();
            }
        }
        let full = drawn.then(|| {
            core::slice::from_raw_parts(pixels, count)
                .iter()
                .zip(&black)
                .map(|(white, black)| crate::bigicon::premultiplied(*black, *white))
                .collect::<Vec<_>>()
        });
        SelectObject(dc, old);
        DeleteDC(dc);
        full
    }?;
    let small = shrink(&full, cx as usize, side as usize);
    Bitmap::with_pixels(side, side, |px| {
        px.copy_from_slice(&small);
        true
    })
}

/// For each of `to` pixels that `from` pixels shrink into, the source
/// pixels its span covers and how much of the span each makes up (an area
/// filter: each one's weights add up to 1).
fn area_weights(from: usize, to: usize) -> Vec<Vec<(usize, f32)>> {
    let scale = from as f32 / to as f32;
    (0..to)
        .map(|d| {
            let (start, end) = (d as f32 * scale, (d + 1) as f32 * scale);
            let first = start.floor() as usize;
            let last = (end.ceil() as usize).min(from);
            (first..last)
                .filter_map(|s| {
                    let cover = end.min(s as f32 + 1.0) - start.max(s as f32);
                    (cover > 1e-6).then_some((s, cover / scale))
                })
                .collect()
        })
        .collect()
}

/// The square image `src` (`from` pixels a side, premultiplied BGRA rows)
/// shrunk to `to` pixels a side, each pixel the average of the area it
/// covers: a line finer than the step stays, fainter, where taking every
/// other pixel would drop it. Premultiplied stays premultiplied (no colour
/// over its alpha), as an average of such pixels is one.
fn shrink(src: &[[u8; 4]], from: usize, to: usize) -> Vec<[u8; 4]> {
    let weights = area_weights(from, to);
    // Across first: `from` rows of `to` pixels.
    let mut rows = vec![[0f32; 4]; from * to];
    for y in 0..from {
        for (x, w) in weights.iter().enumerate() {
            let mut acc = [0f32; 4];
            for &(s, f) in w {
                let p = src[y * from + s];
                for c in 0..4 {
                    acc[c] += f32::from(p[c]) * f;
                }
            }
            rows[y * to + x] = acc;
        }
    }
    let mut out = vec![[0u8; 4]; to * to];
    for (y, w) in weights.iter().enumerate() {
        for x in 0..to {
            let mut acc = [0f32; 4];
            for &(s, f) in w {
                let p = rows[s * to + x];
                for c in 0..4 {
                    acc[c] += p[c] * f;
                }
            }
            out[y * to + x] = acc.map(|v| (v + 0.5).clamp(0.0, 255.0) as u8);
        }
    }
    out
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
    fn a_shrunk_icon_is_kept_only_if_still_asked_for() {
        let mut p: Pictures<()> = Pictures::default();
        let (a, b) = ((1, 7, 128), (1, 8, 128));
        p.asked.insert(a);
        assert!(p.arrived(a, Some(()), 1000));
        assert!(p.made.contains_key(&a));
        assert!(!p.asked.contains(&a));
        // Forgotten since (another size): thrown away.
        assert!(!p.arrived(b, Some(()), 1000));
        assert!(!p.made.contains_key(&b));
        // One that could not be made is kept as such: not asked again.
        p.asked.insert(b);
        assert!(p.arrived(b, None, 0));
        assert_eq!(p.made.get(&b).map(Option::is_some), Some(false));
        assert_eq!(p.bytes, 1000);
    }

    #[test]
    fn shrunk_icons_over_the_budget_start_again() {
        let mut p: Pictures<()> = Pictures::default();
        let each = 128 * 128 * 4;
        let fit = SHRUNK_BUDGET / each;
        for i in 0..fit as i32 {
            p.asked.insert((1, i, 128));
            p.arrived((1, i, 128), Some(()), each);
        }
        assert_eq!(p.made.len(), fit);
        p.asked.insert((1, -5, 128));
        p.arrived((1, -5, 128), Some(()), each);
        assert_eq!(p.made.len(), 1, "the budget is never passed");
        assert_eq!(p.bytes, each);
    }

    #[test]
    fn area_weights_add_up_to_one() {
        for (from, to) in [
            (256, 128),
            (256, 160),
            (256, 192),
            (256, 100),
            (48, 32),
            (7, 3),
        ] {
            let w = area_weights(from, to);
            assert_eq!(w.len(), to);
            for (d, span) in w.iter().enumerate() {
                let sum: f32 = span.iter().map(|&(_, f)| f).sum();
                assert!((sum - 1.0).abs() < 1e-4, "{from}->{to} pixel {d}: {sum}");
                assert!(span.iter().all(|&(s, _)| s < from));
            }
            // Every source pixel counts somewhere.
            let covered: HashSet<usize> = w.iter().flatten().map(|&(s, _)| s).collect();
            assert_eq!(covered.len(), from);
        }
    }

    #[test]
    fn a_thin_line_survives_the_shrink() {
        // A white page with a one-pixel grey line on row 11 (odd), as a text
        // file's icon has: taking every other row would lose it.
        let (from, to) = (256, 128);
        let page = [250u8, 250, 250, 255];
        let line = [120u8, 120, 120, 255];
        let mut src = vec![page; from * from];
        for x in 0..from {
            src[11 * from + x] = line;
        }
        let out = shrink(&src, from, to);
        // Row 5 holds rows 10 and 11: half page, half line.
        assert_eq!(out[5 * to + 40], [185, 185, 185, 255]);
        assert_eq!(out[4 * to + 40], page);
        assert_eq!(out[6 * to + 40], page);
    }

    #[test]
    fn a_shrink_stays_premultiplied() {
        // Pixels of every coverage, colour never over alpha.
        let from = 64;
        let src: Vec<[u8; 4]> = (0..from * from)
            .map(|i| {
                let a = ((i * 37) % 256) as u8;
                let c = ((i * 11) % 256) as u8;
                [c.min(a), (c / 2).min(a), a / 3, a]
            })
            .collect();
        for to in [32, 40, 17] {
            for p in shrink(&src, from, to) {
                assert!(p[0] <= p[3] && p[1] <= p[3] && p[2] <= p[3], "{p:?}");
            }
        }
        // Clear stays clear, solid stays solid.
        assert!(
            shrink(&vec![[0; 4]; 256 * 256], 256, 128)
                .iter()
                .all(|p| *p == [0; 4])
        );
        let solid = [10, 20, 30, 255];
        assert!(
            shrink(&vec![solid; 256 * 256], 256, 160)
                .iter()
                .all(|p| *p == solid)
        );
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
