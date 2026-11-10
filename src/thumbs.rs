//! Thumbnails for the thumbnail views.
//!
//! The shell makes them (`IShellItemImageFactory`), which can mean reading
//! and decoding whole files, so one background thread asks for them, the
//! items in view only, the newest request first; requests for items that
//! scrolled away, or of a size or search no longer shown, are dropped.
//! Each answer is posted to the window, which keeps the bitmaps in a cache
//! bounded by their memory and redraws the item.

use std::collections::{HashMap, HashSet};
use std::ops::Range;
use std::sync::{Arc, Mutex};

use windows::Win32::Foundation::SIZE;
use windows::Win32::UI::Shell::{
    IShellItemImageFactory, SHCreateItemFromParsingName, SIIGBF_ICONONLY, SIIGBF_THUMBNAILONLY,
};
use windows::core::PCWSTR;
use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, HWND};
use windows_sys::Win32::Graphics::Gdi::{
    BI_RGB, BITMAP, BITMAPINFO, BITMAPINFOHEADER, CreateDIBSection, DIB_RGB_COLORS, DeleteObject,
    GetDC, GetDIBits, GetObjectW, HBITMAP, ReleaseDC,
};
use windows_sys::Win32::System::Com::{
    COINIT_APARTMENTTHREADED, COINIT_DISABLE_OLE1DDE, CoInitializeEx,
};
use windows_sys::Win32::System::Threading::{
    CreateEventW, INFINITE, ResetEvent, SetEvent, WaitForSingleObject,
};
use windows_sys::Win32::UI::WindowsAndMessaging::PostMessageW;

use crate::wide::nul;

/// The most memory the cached thumbnails take (32-bit pixels).
pub const BUDGET: usize = 24 << 20;
/// What an entry counts for at least: one that has no image still costs
/// its key and bookkeeping, and must be evicted some time.
const MIN_COST: usize = 1024;
// The budget holds dozens of extra large thumbnails at 150 % (384 px), a
// few screenfuls, and hundreds of large ones at 100 %.
const _: () = assert!(BUDGET / (384 * 384 * 4) >= 40 && BUDGET / (128 * 128 * 4) >= 300);

/// Requests waiting at most; older ones have scrolled away.
const MAX_QUEUED: usize = 512;

/// A 32-bit top-down DIB with premultiplied alpha, ready for `AlphaBlend`;
/// deleted when dropped.
pub struct Bitmap {
    handle: usize,
    pub width: i32,
    pub height: i32,
}

impl Bitmap {
    pub fn handle(&self) -> HBITMAP {
        self.handle as HBITMAP
    }

    /// A `width` by `height` bitmap whose pixels (top-down, premultiplied
    /// BGRA) `fill` writes; `None` if it could not be made or `fill` said
    /// it failed.
    pub fn with_pixels(
        width: i32,
        height: i32,
        fill: impl FnOnce(&mut [[u8; 4]]) -> bool,
    ) -> Option<Bitmap> {
        let (out, bits) = dib(width, height)?;
        // SAFETY: the DIB section's pixels, `width * height` of them, owned
        // by `out`, which lives across the borrow and is in no DC yet.
        let pixels = unsafe {
            core::slice::from_raw_parts_mut(bits.cast::<[u8; 4]>(), (width * height) as usize)
        };
        fill(pixels).then_some(out)
    }

    fn cost(&self) -> usize {
        (self.width.max(0) as usize * self.height.max(0) as usize * 4).max(MIN_COST)
    }
}

impl Drop for Bitmap {
    fn drop(&mut self) {
        // SAFETY: the bitmap is owned here and selected into no DC once its
        // owner lets go of it (drawing borrows it).
        unsafe { DeleteObject(self.handle as HBITMAP) };
    }
}

/// A least-recently-used cache whose entries each cost some bytes; adding
/// one past the budget evicts the least recently used until it fits.
pub struct Lru<V> {
    map: HashMap<Box<[u16]>, (V, usize, u64)>,
    bytes: usize,
    budget: usize,
    tick: u64,
}

impl<V> Lru<V> {
    pub fn new(budget: usize) -> Lru<V> {
        Lru {
            map: HashMap::new(),
            bytes: 0,
            budget,
            tick: 0,
        }
    }

    /// The value under `key`, now the most recently used.
    pub fn get(&mut self, key: &[u16]) -> Option<&V> {
        self.tick += 1;
        let tick = self.tick;
        self.map.get_mut(key).map(|e| {
            e.2 = tick;
            &e.0
        })
    }

    /// Adds `value` (costing `cost` bytes) under `key`, replacing what was
    /// there; returns what was evicted to make room, which the caller
    /// drops. The entry just added stays even when it alone is over the
    /// budget.
    pub fn insert(&mut self, key: Box<[u16]>, value: V, cost: usize) -> Vec<V> {
        self.tick += 1;
        let mut out = Vec::new();
        if let Some((old, old_cost, _)) = self.map.remove(&key) {
            self.bytes -= old_cost;
            out.push(old);
        }
        while self.bytes + cost > self.budget && !self.map.is_empty() {
            let oldest = self
                .map
                .iter()
                .min_by_key(|(_, e)| e.2)
                .map(|(k, _)| k.clone())
                .expect("not empty");
            let (v, c, _) = self.map.remove(&oldest).expect("just found");
            self.bytes -= c;
            out.push(v);
        }
        self.bytes += cost;
        self.map.insert(key, (value, cost, self.tick));
        out
    }

    #[cfg(test)]
    pub fn bytes(&self) -> usize {
        self.bytes
    }

    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.map.len()
    }

    /// Empties the cache, dropping every value.
    pub fn clear(&mut self) {
        self.map.clear();
        self.bytes = 0;
    }
}

/// A thumbnail the thread made (or failed to: `None`), for the item
/// `index` of `generation` that showed `path`.
pub struct ThumbReply {
    pub generation: u64,
    pub index: usize,
    pub path: Box<[u16]>,
    bitmap: Option<Bitmap>,
}

struct Request {
    index: usize,
    path: Box<[u16]>,
}

struct Queue {
    generation: u64,
    /// The side in pixels asked for.
    side: i32,
    items: Vec<Request>,
    /// Queued or being made: not to be asked for again.
    waiting: HashSet<Box<[u16]>>,
    /// The items in view; others are not made.
    visible: Range<usize>,
}

struct Shared {
    queue: Mutex<Queue>,
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

/// The window's side: the cache, and the thread's queue.
pub struct Thumbs {
    shared: Arc<Shared>,
    /// Thumbnails by path; `None` where the shell had no image.
    cache: std::cell::RefCell<Lru<Option<Bitmap>>>,
    generation: std::cell::Cell<u64>,
}

impl Thumbs {
    /// Starts the thumbnail thread; its answers arrive at `hwnd` as `msg`,
    /// with a `Box<ThumbReply>` in `lParam`.
    pub fn new(hwnd: HWND, msg: u32) -> Thumbs {
        // SAFETY: an unnamed manual-reset event.
        let work = unsafe { CreateEventW(core::ptr::null(), 1, 0, core::ptr::null()) } as usize;
        let shared = Arc::new(Shared {
            queue: Mutex::new(Queue {
                generation: 0,
                side: 0,
                items: Vec::new(),
                waiting: HashSet::new(),
                visible: 0..0,
            }),
            work,
            hwnd: hwnd as usize,
            msg,
        });
        let thread = Arc::clone(&shared);
        std::thread::Builder::new()
            .name("goz-thumbs".into())
            .spawn(move || thumb_thread(&thread))
            .expect("spawn thumbnail thread");
        Thumbs {
            shared,
            cache: std::cell::RefCell::new(Lru::new(BUDGET)),
            generation: std::cell::Cell::new(0),
        }
    }

    fn queue(&self) -> std::sync::MutexGuard<'_, Queue> {
        self.shared.queue.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Forgets every thumbnail and request: other results, or another
    /// size (`side` pixels, 0 when no thumbnails show). Answers on their
    /// way are of an older generation and are dropped as they come.
    pub fn reset(&self, side: i32) {
        let generation = self.generation.get() + 1;
        self.generation.set(generation);
        {
            let mut q = self.queue();
            q.generation = generation;
            q.side = side;
            q.items.clear();
            q.waiting.clear();
            q.visible = 0..0;
        }
        self.cache.borrow_mut().clear();
    }

    /// The items in view now; requests for others are dropped.
    pub fn set_visible(&self, visible: Range<usize>) {
        self.queue().visible = visible;
    }

    /// Calls `f` with the cached thumbnail of `path`: `Some(Some(_))` an
    /// image, `Some(None)` none to be had (draw the icon), `None` not
    /// made yet.
    pub fn with<R>(&self, path: &[u16], f: impl FnOnce(Option<Option<&Bitmap>>) -> R) -> R {
        let mut cache = self.cache.borrow_mut();
        f(cache.get(path).map(Option::as_ref))
    }

    /// Asks for the thumbnail of item `index`, `path`, unless it is already
    /// asked for.
    pub fn request(&self, index: usize, path: &[u16]) {
        let mut q = self.queue();
        if q.side <= 0 || q.waiting.contains(path) {
            return;
        }
        if q.items.len() >= MAX_QUEUED {
            let old = q.items.remove(0);
            q.waiting.remove(&old.path);
        }
        let path: Box<[u16]> = path.into();
        q.waiting.insert(path.clone());
        q.items.push(Request { index, path });
        // SAFETY: a live event handle.
        unsafe { SetEvent(self.shared.work as HANDLE) };
    }

    /// Takes an answer of the thread into the cache; false (and the bitmap
    /// deleted) when it is of results or a size no longer shown.
    pub fn accept(&self, reply: ThumbReply) -> bool {
        if reply.generation != self.generation.get() {
            return false;
        }
        self.queue().waiting.remove(&reply.path);
        let cost = reply.bitmap.as_ref().map_or(MIN_COST, Bitmap::cost);
        // The evicted bitmaps are deleted as they drop here.
        drop(
            self.cache
                .borrow_mut()
                .insert(reply.path, reply.bitmap, cost),
        );
        true
    }
}

fn thumb_thread(shared: &Shared) {
    // SAFETY: the shell's image factories want COM, apartment-threaded, on
    // the thread that uses them.
    unsafe {
        CoInitializeEx(
            core::ptr::null(),
            (COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE) as u32,
        )
    };
    loop {
        let next = {
            let mut q = shared.queue.lock().unwrap_or_else(|e| e.into_inner());
            loop {
                match q.items.pop() {
                    // Scrolled away: forgotten, so it is asked for again
                    // when it comes back into view.
                    Some(r) if !q.visible.contains(&r.index) => {
                        q.waiting.remove(&r.path);
                    }
                    Some(r) => break Some((q.generation, q.side, r)),
                    None => {
                        // SAFETY: a live event handle.
                        unsafe { ResetEvent(shared.work as HANDLE) };
                        break None;
                    }
                }
            }
        };
        let Some((generation, side, request)) = next else {
            // SAFETY: a live event handle.
            unsafe { WaitForSingleObject(shared.work as HANDLE, INFINITE) };
            continue;
        };
        let bitmap = make(&request.path, side);
        let reply = Box::into_raw(Box::new(ThumbReply {
            generation,
            index: request.index,
            path: request.path,
            bitmap,
        }));
        // SAFETY: the window owns the box once posted; reclaimed (and its
        // bitmap deleted) if not.
        unsafe {
            if PostMessageW(shared.hwnd as HWND, shared.msg, 0, reply as isize) == 0 {
                drop(Box::from_raw(reply));
            }
        }
    }
}

/// The thumbnail of `path` at most `side` pixels square: the file's own
/// picture when the shell has one, else its icon at that size.
fn make(path: &[u16], side: i32) -> Option<Bitmap> {
    let path = nul(path);
    // SAFETY: a NUL-terminated path; COM is initialized on this thread.
    let factory: IShellItemImageFactory =
        unsafe { SHCreateItemFromParsingName(PCWSTR(path.as_ptr()), None) }.ok()?;
    let size = SIZE { cx: side, cy: side };
    // SAFETY: a live factory; the bitmap returned is ours to delete.
    let shell = unsafe {
        factory
            .GetImage(size, SIIGBF_THUMBNAILONLY)
            .or_else(|_| factory.GetImage(size, SIIGBF_ICONONLY))
    }
    .ok()?;
    let shell = shell.0 as HBITMAP;
    let out = premultiplied(shell);
    // SAFETY: the shell's bitmap, copied above and no longer used.
    unsafe { DeleteObject(shell) };
    out
}

/// A new 32-bit top-down DIB section `w` by `h`, and its pixels.
fn dib(w: i32, h: i32) -> Option<(Bitmap, *mut core::ffi::c_void)> {
    if w <= 0 || h <= 0 {
        return None;
    }
    // SAFETY: a plain header for a DIB section that the returned `Bitmap`
    // owns; its pixels stay valid until the bitmap is deleted.
    unsafe {
        let mut bi: BITMAPINFO = core::mem::zeroed();
        bi.bmiHeader = BITMAPINFOHEADER {
            biSize: size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: w,
            // Top-down.
            biHeight: -h,
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB,
            ..core::mem::zeroed()
        };
        let mut bits: *mut core::ffi::c_void = core::ptr::null_mut();
        let made = CreateDIBSection(
            core::ptr::null_mut(),
            &bi,
            DIB_RGB_COLORS,
            &mut bits,
            core::ptr::null_mut(),
            0,
        );
        if made.is_null() {
            return None;
        }
        let out = Bitmap {
            handle: made as usize,
            width: w,
            height: h,
        };
        (!bits.is_null()).then_some((out, bits))
    }
}

/// A copy of `src` as a 32-bit top-down DIB with premultiplied alpha. The
/// shell's bitmaps come with straight alpha, premultiplied alpha, or (some
/// photos) no alpha at all, all zero: each is made right for `AlphaBlend`.
pub fn premultiplied(src: HBITMAP) -> Option<Bitmap> {
    // SAFETY: reads the bitmap's header into a plain out-struct, then copies
    // its pixels into a DIB made for them; the screen DC is released.
    unsafe {
        let mut bm: BITMAP = core::mem::zeroed();
        if GetObjectW(src, size_of::<BITMAP>() as i32, (&raw mut bm).cast()) == 0 {
            return None;
        }
        let (w, h) = (bm.bmWidth, bm.bmHeight.abs());
        if w <= 0 || h <= 0 {
            return None;
        }
        let mut bi: BITMAPINFO = core::mem::zeroed();
        bi.bmiHeader = BITMAPINFOHEADER {
            biSize: size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: w,
            // Top-down.
            biHeight: -h,
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB,
            ..core::mem::zeroed()
        };
        let mut bits: *mut core::ffi::c_void = core::ptr::null_mut();
        let dib = CreateDIBSection(
            core::ptr::null_mut(),
            &bi,
            DIB_RGB_COLORS,
            &mut bits,
            core::ptr::null_mut(),
            0,
        );
        if dib.is_null() || bits.is_null() {
            return None;
        }
        let out = Bitmap {
            handle: dib as usize,
            width: w,
            height: h,
        };
        let dc = GetDC(core::ptr::null_mut());
        let lines = GetDIBits(dc, src, 0, h as u32, bits, &mut bi, DIB_RGB_COLORS);
        ReleaseDC(core::ptr::null_mut(), dc);
        if lines == 0 {
            return None;
        }
        let pixels = core::slice::from_raw_parts_mut(bits.cast::<[u8; 4]>(), (w * h) as usize);
        fix_alpha(pixels);
        Some(out)
    }
}

/// Makes BGRA pixels premultiplied: all-zero alpha is opaque; a colour
/// brighter than its alpha (impossible once premultiplied) means straight
/// alpha, which is multiplied in.
pub fn fix_alpha(pixels: &mut [[u8; 4]]) {
    if pixels.iter().all(|p| p[3] == 0) {
        for p in pixels.iter_mut() {
            p[3] = 255;
        }
        return;
    }
    let straight = pixels
        .iter()
        .any(|p| p[0] > p[3] || p[1] > p[3] || p[2] > p[3]);
    if straight {
        for p in pixels.iter_mut() {
            let a = u16::from(p[3]);
            for c in &mut p[..3] {
                *c = ((u16::from(*c) * a + 127) / 255) as u8;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(s: &str) -> Box<[u16]> {
        s.encode_utf16().collect()
    }

    #[test]
    fn the_least_recently_used_goes_first() {
        let mut lru = Lru::new(300);
        assert!(lru.insert(key("a"), 1, 100).is_empty());
        assert!(lru.insert(key("b"), 2, 100).is_empty());
        assert!(lru.insert(key("c"), 3, 100).is_empty());
        assert_eq!(lru.bytes(), 300);
        // `a` used: `b` is the oldest now.
        assert_eq!(lru.get(&key("a")), Some(&1));
        assert_eq!(lru.insert(key("d"), 4, 100), vec![2]);
        assert_eq!(lru.get(&key("b")), None);
        assert_eq!((lru.bytes(), lru.len()), (300, 3));
        // A big one pushes out as many as it needs.
        let mut out = lru.insert(key("e"), 5, 250);
        out.sort_unstable();
        assert_eq!(out, vec![1, 3, 4]);
        assert_eq!((lru.bytes(), lru.len()), (250, 1));
    }

    #[test]
    fn replacing_and_oversized_entries() {
        let mut lru = Lru::new(100);
        lru.insert(key("a"), 1, 60);
        // The same key again: the old value comes back, the bytes are the
        // new one's.
        assert_eq!(lru.insert(key("a"), 2, 40), vec![1]);
        assert_eq!(lru.bytes(), 40);
        // Bigger than the whole budget: everything else goes, it stays.
        assert_eq!(lru.insert(key("b"), 3, 500), vec![2]);
        assert_eq!((lru.bytes(), lru.len()), (500, 1));
        // And the next one evicts it.
        assert_eq!(lru.insert(key("c"), 4, 10), vec![3]);
        assert_eq!(lru.bytes(), 10);
        lru.clear();
        assert_eq!((lru.bytes(), lru.len()), (0, 0));
    }

    #[test]
    fn alpha_is_made_premultiplied() {
        // No alpha at all (a photo): opaque, colours untouched.
        let mut px = [[10, 20, 30, 0], [200, 100, 50, 0]];
        fix_alpha(&mut px);
        assert_eq!(px, [[10, 20, 30, 255], [200, 100, 50, 255]]);
        // Straight alpha: multiplied in.
        let mut px = [[255, 255, 255, 128], [0, 0, 0, 0], [100, 50, 0, 255]];
        fix_alpha(&mut px);
        assert_eq!(px, [[128, 128, 128, 128], [0, 0, 0, 0], [100, 50, 0, 255]]);
        // Already premultiplied: left alone.
        let mut px = [[64, 32, 0, 128], [0, 0, 0, 0]];
        fix_alpha(&mut px);
        assert_eq!(px, [[64, 32, 0, 128], [0, 0, 0, 0]]);
    }
}
