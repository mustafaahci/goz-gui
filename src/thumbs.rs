//! Thumbnails for the thumbnail views.
//!
//! Making one can mean reading and decoding a whole file, so one background
//! thread makes them, the items in view only, the newest request first;
//! requests for items that scrolled away, or of a size or search no longer
//! shown, are dropped. Each answer is posted to the window, which keeps the
//! bitmaps in a cache bounded by their memory and redraws the item.
//!
//! What a thumbnail is made from depends on the file's type ([`Source`]):
//! the shell's image factory (`IShellItemImageFactory`) only for types with
//! a thumbnail handler (pictures, videos, documents that have one) and for
//! folders; a program's, shortcut's or icon file's own icon is extracted
//! at the size by [`crate::bigicon`] (or, where it is an icon a class of
//! files shares, such as a shortcut to a help file, named for the view to
//! draw from the system image lists); every other type shows its type's
//! icon, which the view draws from the system image list. The factory
//! would give those icons too, but it keeps each file's own icon in the
//! process's jumbo image list for good (about 50 KB per program seen).
//!
//! The cache is let go of when the window hides and trimmed to the items in
//! view once the view has rested ([`Thumbs::release`], [`Thumbs::trim`]).

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
use windows_sys::Win32::Storage::FileSystem::{
    FILE_ATTRIBUTE_DIRECTORY, GetFileAttributesW, INVALID_FILE_ATTRIBUTES,
};
use windows_sys::Win32::System::Com::{
    COINIT_APARTMENTTHREADED, COINIT_DISABLE_OLE1DDE, CoInitializeEx,
};
use windows_sys::Win32::System::Registry::{
    HKEY, HKEY_CLASSES_ROOT, KEY_READ, RRF_RT_REG_SZ, RegCloseKey, RegGetValueW, RegOpenKeyExW,
};
use windows_sys::Win32::System::Threading::{
    CreateEventW, INFINITE, ResetEvent, SetEvent, WaitForSingleObject,
};
use windows_sys::Win32::UI::Shell::{ASSOCF_NONE, ASSOCSTR_SHELLEXTENSION, AssocQueryStringW};
use windows_sys::Win32::UI::WindowsAndMessaging::PostMessageW;

use crate::wide::{nul, wz};

/// The shell extension interfaces of a thumbnail handler: `IThumbnailProvider`
/// and the older `IExtractImage`.
const THUMBNAIL_PROVIDER: &str = "{E357FCCD-A995-4576-B01F-234630154E96}";
const EXTRACT_IMAGE: &str = "{BB2E617C-0920-11d1-9A0B-00C04FC2D6C1}";

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

/// What a thumbnail cell shows of a file.
pub enum Picture {
    /// A bitmap made for it: the shell's picture of its contents, or its
    /// own icon extracted at the size.
    Image(Bitmap),
    /// An icon of the system image lists that a class of files shares, by
    /// its index (a shortcut to a help file shows the .chm type's icon):
    /// drawn from the lists as a type's icon is.
    Shared(i32),
}

impl Picture {
    fn cost(&self) -> usize {
        match self {
            Picture::Image(bitmap) => bitmap.cost(),
            Picture::Shared(_) => MIN_COST,
        }
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

    /// Keeps only the values `keep` says to; the others are dropped. How
    /// many went.
    pub fn retain(&mut self, mut keep: impl FnMut(&V) -> bool) -> usize {
        let before = self.map.len();
        let mut freed = 0;
        self.map.retain(|_, (v, cost, _)| {
            let kept = keep(v);
            if !kept {
                freed += *cost;
            }
            kept
        });
        self.bytes -= freed;
        before - self.map.len()
    }
}

/// What a file's thumbnail is made from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Source {
    /// A picture of its contents, from the shell's image factory: types
    /// with a thumbnail handler, and folders.
    Shell,
    /// Its own icon (a program's, a shortcut's, an icon file's), extracted
    /// at the size.
    OwnIcon,
    /// Its type's icon, which the view draws itself: nothing to make.
    TypeIcon,
}

/// The source of a thumbnail for a file (not a folder: those are the
/// shell's) of extension `ext` (lower case, no dot); `handled` says whether
/// the type has a thumbnail handler, and is asked only when that decides
/// it.
fn source_for(ext: &[u16], handled: impl FnOnce() -> bool) -> Source {
    if crate::icons::wants_own(ext) {
        // Shortcuts have a thumbnail handler (the target's picture), but
        // are shown by their icon, as programs and icon files are.
        Source::OwnIcon
    } else if !ext.is_empty() && handled() {
        Source::Shell
    } else {
        Source::TypeIcon
    }
}

/// Whether files of extension `ext` (lower case, no dot) have a thumbnail
/// handler: registered for the extension, its file type or its perceived
/// type (`SystemFileAssociations\image` for pictures), as `IThumbnailProvider`
/// or `IExtractImage`. Reads the registry.
fn has_thumbnail_handler(ext: &[u16]) -> bool {
    let mut dotted = vec![u16::from(b'.')];
    dotted.extend_from_slice(ext);
    let dotted_z = nul(&dotted);
    let assoc = |iid: &str| {
        let iid = wz(iid);
        let mut buf = [0u16; 64];
        let mut len = buf.len() as u32;
        // SAFETY: NUL-terminated strings; the buffer with its length.
        unsafe {
            AssocQueryStringW(
                ASSOCF_NONE,
                ASSOCSTR_SHELLEXTENSION,
                dotted_z.as_ptr(),
                iid.as_ptr(),
                buf.as_mut_ptr(),
                &mut len,
            ) == 0
        }
    };
    if assoc(THUMBNAIL_PROVIDER) || assoc(EXTRACT_IMAGE) {
        return true;
    }
    // The perceived type's own registration, which the association does not
    // always reach (a type some program took over).
    let Some(perceived) = perceived_type(&dotted_z) else {
        return false;
    };
    [THUMBNAIL_PROVIDER, EXTRACT_IMAGE].iter().any(|iid| {
        let mut key: Vec<u16> = "SystemFileAssociations\\".encode_utf16().collect();
        key.extend_from_slice(&perceived);
        key.extend("\\ShellEx\\".encode_utf16());
        key.extend(iid.encode_utf16());
        key_exists(&nul(&key))
    })
}

/// The `PerceivedType` of extension key `dotted_z` (".png", NUL-terminated)
/// under HKEY_CLASSES_ROOT.
fn perceived_type(dotted_z: &[u16]) -> Option<Vec<u16>> {
    let name = wz("PerceivedType");
    let mut buf = [0u16; 64];
    let mut bytes = (buf.len() * 2) as u32;
    // SAFETY: NUL-terminated names; the buffer with its size in bytes.
    let status = unsafe {
        RegGetValueW(
            HKEY_CLASSES_ROOT,
            dotted_z.as_ptr(),
            name.as_ptr(),
            RRF_RT_REG_SZ,
            core::ptr::null_mut(),
            buf.as_mut_ptr().cast(),
            &mut bytes,
        )
    };
    if status != 0 {
        return None;
    }
    let len = buf.iter().position(|&c| c == 0).unwrap_or(0);
    (len > 0).then(|| buf[..len].to_vec())
}

/// Whether key `path_z` (NUL-terminated) exists under HKEY_CLASSES_ROOT.
fn key_exists(path_z: &[u16]) -> bool {
    let mut key: HKEY = core::ptr::null_mut();
    // SAFETY: a NUL-terminated path; the key opened is closed at once.
    unsafe {
        if RegOpenKeyExW(HKEY_CLASSES_ROOT, path_z.as_ptr(), 0, KEY_READ, &mut key) != 0 {
            return false;
        }
        RegCloseKey(key);
    }
    true
}

/// `path`'s extension, lower case, without its dot; empty with none.
fn extension(path: &[u16]) -> Vec<u16> {
    let name = path
        .iter()
        .rposition(|&c| c == u16::from(b'\\'))
        .map_or(path, |i| &path[i + 1..]);
    match name.iter().rposition(|&c| c == u16::from(b'.')) {
        Some(dot) => name[dot + 1..]
            .iter()
            .map(|&c| crate::highlight::fold(c))
            .collect(),
        None => Vec::new(),
    }
}

/// A thumbnail the thread made (or failed to: `None`), for the item
/// `index` of `generation` that showed `path`.
pub struct ThumbReply {
    pub generation: u64,
    pub index: usize,
    pub path: Box<[u16]>,
    picture: Option<Picture>,
}

/// A cached thumbnail, and the item it was made for (where it showed).
struct Entry {
    picture: Option<Picture>,
    index: usize,
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
    /// Thumbnails by path; no picture where there is none to show (the
    /// view draws the type's icon).
    cache: std::cell::RefCell<Lru<Entry>>,
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

    /// Lets go of every thumbnail and request, keeping the size: the
    /// window hid, and what it shows when it comes back is made again.
    pub fn release(&self) {
        let side = self.queue().side;
        self.reset(side);
    }

    /// Lets go of the thumbnails of the items not in view (the view has
    /// rested); those come back when they are scrolled to. How many went.
    pub fn trim(&self) -> usize {
        let visible = self.queue().visible.clone();
        self.cache
            .borrow_mut()
            .retain(|e| visible.contains(&e.index))
    }

    /// The items in view now; requests for others are dropped.
    pub fn set_visible(&self, visible: Range<usize>) {
        self.queue().visible = visible;
    }

    /// Calls `f` with the cached thumbnail of `path`: `Some(Some(_))` a
    /// picture, `Some(None)` none to be had (draw the type's icon), `None`
    /// not made yet.
    pub fn with<R>(&self, path: &[u16], f: impl FnOnce(Option<Option<&Picture>>) -> R) -> R {
        let mut cache = self.cache.borrow_mut();
        f(cache.get(path).map(|e| e.picture.as_ref()))
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
        let cost = reply.picture.as_ref().map_or(MIN_COST, Picture::cost);
        let entry = Entry {
            picture: reply.picture,
            index: reply.index,
        };
        // The evicted bitmaps are deleted as they drop here.
        drop(self.cache.borrow_mut().insert(reply.path, entry, cost));
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
    // Each extension's source, worked out once (registry reads).
    let mut sources: HashMap<Vec<u16>, Source> = HashMap::new();
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
        let picture = make(&request.path, side, &mut sources);
        let reply = Box::into_raw(Box::new(ThumbReply {
            generation,
            index: request.index,
            path: request.path,
            picture,
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

/// The thumbnail of `path` at most `side` pixels square: the picture of
/// its contents where its type has one, its own icon where it carries one;
/// `None` for the view to draw its type's icon. `sources` keeps what each
/// extension's thumbnails are made from.
fn make(path: &[u16], side: i32, sources: &mut HashMap<Vec<u16>, Source>) -> Option<Picture> {
    let path_z = nul(path);
    // SAFETY: a NUL-terminated path.
    let attrs = unsafe { GetFileAttributesW(path_z.as_ptr()) };
    let is_dir = attrs != INVALID_FILE_ATTRIBUTES && attrs & FILE_ATTRIBUTE_DIRECTORY != 0;
    let source = if is_dir {
        // A folder's picture shows what it holds.
        Source::Shell
    } else {
        *sources
            .entry(extension(path))
            .or_insert_with_key(|ext| source_for(ext, || has_thumbnail_handler(ext)))
    };
    match source {
        Source::TypeIcon => None,
        Source::OwnIcon => crate::bigicon::own_icon(path, side),
        Source::Shell => shell_thumbnail(&path_z, side, is_dir).map(Picture::Image),
    }
}

/// The shell's picture of `path_z`'s contents (NUL-terminated), at most
/// `side` pixels square; `None` when it has none (then its type's icon
/// shows). An empty folder's is its icon, which may be its own (a
/// `desktop.ini`'s: Downloads, Music).
fn shell_thumbnail(path_z: &[u16], side: i32, is_dir: bool) -> Option<Bitmap> {
    // SAFETY: a NUL-terminated path; COM is initialized on this thread.
    let factory: IShellItemImageFactory =
        unsafe { SHCreateItemFromParsingName(PCWSTR(path_z.as_ptr()), None) }.ok()?;
    let size = SIZE { cx: side, cy: side };
    // SAFETY: a live factory; the bitmap returned is ours to delete.
    let shell = unsafe {
        factory.GetImage(size, SIIGBF_THUMBNAILONLY).or_else(|e| {
            if is_dir {
                factory.GetImage(size, SIIGBF_ICONONLY)
            } else {
                Err(e)
            }
        })
    }
    .ok()?;
    let shell = shell.0 as HBITMAP;
    let out = premultiplied(shell);
    // SAFETY: the shell's bitmap, copied above and no longer used.
    unsafe { DeleteObject(shell) };
    out
}

/// A new 32-bit top-down DIB section `w` by `h` (all zero), and its pixels.
pub fn dib(w: i32, h: i32) -> Option<(Bitmap, *mut core::ffi::c_void)> {
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
    fn retaining_drops_the_others_and_their_bytes() {
        let mut lru = Lru::new(1000);
        lru.insert(key("a"), 1, 100);
        lru.insert(key("b"), 2, 200);
        lru.insert(key("c"), 3, 300);
        assert_eq!(lru.retain(|&v| v != 2), 1);
        assert_eq!((lru.bytes(), lru.len()), (400, 2));
        assert_eq!(lru.get(&key("b")), None);
        assert_eq!(lru.retain(|_| false), 2);
        assert_eq!((lru.bytes(), lru.len()), (0, 0));
    }

    #[test]
    fn what_a_thumbnail_is_made_from() {
        let ext = |s: &str| s.encode_utf16().collect::<Vec<u16>>();
        // Programs, shortcuts and icon files: their own icon, whatever
        // handler their type has (a shortcut's shows its target).
        for own in ["exe", "lnk", "ico", "url", "cpl"] {
            assert_eq!(source_for(&ext(own), || true), Source::OwnIcon, "{own}");
        }
        // A type with a thumbnail handler: the shell's picture.
        assert_eq!(source_for(&ext("jpg"), || true), Source::Shell);
        // Without one, or with no extension: the type's icon, and the
        // registry is not asked about a file with no extension.
        assert_eq!(source_for(&ext("txt"), || false), Source::TypeIcon);
        assert_eq!(
            source_for(&[], || panic!("asked for no extension")),
            Source::TypeIcon
        );
        // An own-icon type never asks.
        assert_eq!(
            source_for(&ext("exe"), || panic!("asked for a program")),
            Source::OwnIcon
        );
    }

    #[test]
    fn extensions_are_the_names_lower_cased() {
        let w = |s: &str| s.encode_utf16().collect::<Vec<u16>>();
        assert_eq!(extension(&w(r"C:\Windows\NOTEPAD.EXE")), w("exe"));
        assert_eq!(extension(&w(r"C:\a.b\file")), w(""));
        assert_eq!(extension(&w(r"C:\x\archive.tar.GZ")), w("gz"));
        assert_eq!(extension(&w(r"C:\x\.gitignore")), w("gitignore"));
        assert_eq!(extension(&w("name.")), w(""));
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
