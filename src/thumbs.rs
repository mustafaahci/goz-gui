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
//! draw from the system image lists); a shortcut shows its target's picture
//! where Explorer does (a shortcut to a picture, with no icon of its own);
//! every other type shows its type's
//! icon, which the view draws from the system image list. The factory
//! would give those icons too, but it keeps each file's own icon in the
//! process's jumbo image list for good (about 50 KB per program seen).
//!
//! The cache is let go of when the window hides and trimmed to the items in
//! view once the view has rested ([`Thumbs::release`], [`Thumbs::trim`]).

use std::collections::{HashMap, HashSet};
use std::ops::Range;
use std::sync::{Arc, Mutex};

use windows::Win32::UI::Shell::{SIIGBF_ICONONLY, SIIGBF_THUMBNAILONLY};
use windows_sys::Win32::Foundation::HWND;
use windows_sys::Win32::Graphics::Gdi::HBITMAP;
use windows_sys::Win32::Storage::FileSystem::FILE_ATTRIBUTE_DIRECTORY;
use windows_sys::Win32::System::Registry::HKEY_CLASSES_ROOT;
use windows_sys::Win32::System::Threading::INFINITE;

use crate::win::com::Apartment;
use crate::win::ext::b7::{self, ImageFactory};
use crate::win::sync::Event;
use crate::win::{gdi, parcel};

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

    /// A new `width` by `height` bitmap, all zero; `None` if it could not
    /// be made.
    fn new(width: i32, height: i32) -> Option<Bitmap> {
        let made = b7::create_dib32(width, height);
        (!made.is_null()).then_some(Bitmap {
            handle: made as usize,
            width,
            height,
        })
    }

    /// A `width` by `height` bitmap whose pixels (top-down, premultiplied
    /// BGRA) `fill` writes; `None` if it could not be made or `fill` said
    /// it failed. `fill` writes into a buffer of goz-gui's, copied into the
    /// bitmap after.
    pub fn with_pixels(
        width: i32,
        height: i32,
        fill: impl FnOnce(&mut [[u8; 4]]) -> bool,
    ) -> Option<Bitmap> {
        let out = Bitmap::new(width, height)?;
        // A new DIB section is all zero, as this is.
        let mut pixels = vec![[0u8; 4]; width as usize * height as usize];
        (fill(&mut pixels) && b7::dib_write(out.handle(), &pixels)).then_some(out)
    }

    /// A copy of its pixels (top-down, premultiplied BGRA).
    #[cfg_attr(not(test), expect(dead_code, reason = "the tests read pixels back"))]
    pub fn pixels(&self) -> Option<Vec<[u8; 4]>> {
        b7::dib_pixels(self.handle()).map(|(_, _, px)| px)
    }

    fn cost(&self) -> usize {
        (self.width.max(0) as usize * self.height.max(0) as usize * 4).max(MIN_COST)
    }
}

impl Drop for Bitmap {
    fn drop(&mut self) {
        // The bitmap is owned here and selected into no DC once its owner
        // lets go of it (drawing borrows it).
        gdi::delete_object(self.handle());
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
    /// Its own icon (a program's, an Internet shortcut's, an icon
    /// file's), extracted at the size.
    OwnIcon,
    /// A shortcut (`.lnk`): its target's picture where the shell has one
    /// for it, as Explorer shows it (a shortcut to a picture or a video,
    /// with no icon of its own); otherwise its own icon. The shell's
    /// thumbnail of a shortcut is only ever a picture, never an icon (none
    /// for one to a program, a folder or a missing file, or one with an
    /// icon set), so this costs no entry of the system image lists.
    Shortcut,
    /// Its type's icon, which the view draws itself: nothing to make.
    TypeIcon,
}

/// The source of a thumbnail for a file (not a folder: those are the
/// shell's) of extension `ext` (lower case, no dot); `handled` says whether
/// the type has a thumbnail handler, and is asked only when that decides
/// it.
fn source_for(ext: &[u16], handled: impl FnOnce() -> bool) -> Source {
    if ext == "lnk".encode_utf16().collect::<Vec<u16>>() {
        Source::Shortcut
    } else if crate::icons::wants_own(ext) {
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
    let assoc = |iid: &str| b7::assoc_shell_extension(&dotted, iid).is_some();
    if assoc(THUMBNAIL_PROVIDER) || assoc(EXTRACT_IMAGE) {
        return true;
    }
    // The perceived type's own registration, which the association does not
    // always reach (a type some program took over).
    let Some(perceived) = perceived_type(&dotted) else {
        return false;
    };
    [THUMBNAIL_PROVIDER, EXTRACT_IMAGE].iter().any(|iid| {
        let mut key: Vec<u16> = "SystemFileAssociations\\".encode_utf16().collect();
        key.extend_from_slice(&perceived);
        key.extend("\\ShellEx\\".encode_utf16());
        key.extend(iid.encode_utf16());
        key_exists(&key)
    })
}

/// The `PerceivedType` of extension key `dotted` (".png") under
/// HKEY_CLASSES_ROOT.
fn perceived_type(dotted: &[u16]) -> Option<Vec<u16>> {
    b7::reg_string(HKEY_CLASSES_ROOT, dotted, "PerceivedType", 64).filter(|t| !t.is_empty())
}

/// Whether key `path` exists under HKEY_CLASSES_ROOT.
fn key_exists(path: &[u16]) -> bool {
    b7::reg_key_exists(HKEY_CLASSES_ROOT, path)
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
    /// Set while requests wait (reset by hand).
    work: Event,
    hwnd: usize,
    msg: u32,
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
    /// each with a [`ThumbReply`] parcel (`win::parcel::take`).
    pub fn new(hwnd: HWND, msg: u32) -> Thumbs {
        let work = Event::new(true, false).expect("an event for the thumbnail thread");
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
        self.shared.work.set();
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
    // The shell's image factories want COM, apartment-threaded, on the
    // thread that uses them, for as long as it runs.
    if let Some(apartment) = Apartment::sta() {
        apartment.keep();
    }
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
                        shared.work.reset();
                        break None;
                    }
                }
            }
        };
        let Some((generation, side, request)) = next else {
            shared.work.wait(INFINITE);
            continue;
        };
        let picture = make(&request.path, side, &mut sources);
        let reply = ThumbReply {
            generation,
            index: request.index,
            path: request.path,
            picture,
        };
        // The window takes the reply with the message; if it cannot be
        // posted, the reply (and its bitmap) is dropped.
        parcel::post(shared.hwnd as HWND, shared.msg, 0, reply);
    }
}

/// The thumbnail of `path` at most `side` pixels square: the picture of
/// its contents where its type has one, its own icon where it carries one;
/// `None` for the view to draw its type's icon. `sources` keeps what each
/// extension's thumbnails are made from.
fn make(path: &[u16], side: i32, sources: &mut HashMap<Vec<u16>, Source>) -> Option<Picture> {
    let is_dir = b7::file_attributes(path).is_ok_and(|a| a & FILE_ATTRIBUTE_DIRECTORY != 0);
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
        Source::Shortcut => shell_thumbnail(path, side, false)
            .map(Picture::Image)
            .or_else(|| crate::bigicon::own_icon(path, side)),
        Source::Shell => shell_thumbnail(path, side, is_dir).map(Picture::Image),
    }
}

/// The shell's picture of `path`'s contents, at most `side` pixels square;
/// `None` when it has none (then its type's icon shows). An empty folder's
/// is its icon, which may be its own (a `desktop.ini`'s: Downloads,
/// Music).
fn shell_thumbnail(path: &[u16], side: i32, is_dir: bool) -> Option<Bitmap> {
    // COM is initialized on this thread.
    let factory = ImageFactory::of_path(path)?;
    let shell = factory
        .image(side, SIIGBF_THUMBNAILONLY)
        .or_else(|e| {
            if is_dir {
                factory.image(side, SIIGBF_ICONONLY)
            } else {
                Err(e)
            }
        })
        .ok()?;
    let out = premultiplied(shell.get());
    // The shell's bitmap, copied above and no longer used, is deleted.
    drop(shell);
    out
}

/// A copy of `src` as a 32-bit top-down DIB with premultiplied alpha. The
/// shell's bitmaps come with straight alpha, premultiplied alpha, or (some
/// photos) no alpha at all, all zero: each is made right for `AlphaBlend`.
pub fn premultiplied(src: HBITMAP) -> Option<Bitmap> {
    let bm = gdi::bitmap_info(src)?;
    let (w, h) = (bm.bmWidth, bm.bmHeight.abs());
    if w <= 0 || h <= 0 {
        return None;
    }
    let out = Bitmap::new(w, h)?;
    // Its pixels through the screen's DC, made right, then into the DIB.
    let mut pixels = b7::bitmap_pixels(src, w, h)?;
    fix_alpha(&mut pixels);
    b7::dib_write(out.handle(), &pixels).then_some(out)
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
        // Programs, Internet shortcuts and icon files: their own icon,
        // whatever handler their type has.
        for own in ["exe", "ico", "url", "cpl"] {
            assert_eq!(source_for(&ext(own), || true), Source::OwnIcon, "{own}");
        }
        // A shortcut: its target's picture where there is one, as in
        // Explorer, else its own icon; never the registry asked.
        assert_eq!(
            source_for(&ext("lnk"), || panic!("asked for a shortcut")),
            Source::Shortcut
        );
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

    /// A `w` by `h` 24-bit .bmp file of one colour (blue, green, red).
    fn bmp(w: u32, h: u32, bgr: [u8; 3]) -> Vec<u8> {
        let row = (w * 3).div_ceil(4) * 4;
        let size = 54 + row * h;
        let mut out = b"BM".to_vec();
        for v in [size, 0, 54, 40] {
            out.extend(v.to_le_bytes());
        }
        out.extend((w as i32).to_le_bytes());
        out.extend((h as i32).to_le_bytes());
        out.extend(1u16.to_le_bytes());
        out.extend(24u16.to_le_bytes());
        for v in [0u32, row * h, 2835, 2835, 0, 0] {
            out.extend(v.to_le_bytes());
        }
        for _ in 0..h {
            let mut line: Vec<u8> = (0..w).flat_map(|_| bgr).collect();
            line.resize(row as usize, 0);
            out.extend(line);
        }
        out
    }

    /// The pixel (premultiplied BGRA) at the centre of `b`.
    fn centre(b: &Bitmap) -> [u8; 4] {
        // Its pixels are `width * height` top-down BGRA.
        let px = b.pixels().expect("a DIB section of ours");
        px[(b.height / 2 * b.width + b.width / 2) as usize]
    }

    /// Explorer's rule: a shortcut to a picture shows the picture; one to
    /// a program shows the program's icon.
    #[test]
    fn a_shortcut_to_a_picture_shows_the_picture() {
        assert!(crate::shell::ole_ready());
        let dir = crate::store::file::tests::fresh_dir("goz-gui-thumbs", "lnk");
        let picture = dir.join("red.bmp");
        std::fs::write(&picture, bmp(64, 48, [0, 0, 255])).expect("picture");
        let w = |s: &str| s.encode_utf16().collect::<Vec<u16>>();
        let link = |name: &str, to: &std::path::Path| {
            let lnk = w(&dir.join(name).to_string_lossy());
            // COM is ready (OLE).
            let link = b7::ShellLink::new().expect("link");
            assert!(link.set_path(&w(&to.to_string_lossy())), "set path");
            assert!(link.save(&lnk), "save");
            lnk
        };
        let to_picture = link("to picture.lnk", &picture);
        let to_program = link(
            "to program.lnk",
            std::path::Path::new(r"C:\Windows\System32\notepad.exe"),
        );
        let mut sources = HashMap::new();
        match make(&to_picture, 96, &mut sources) {
            Some(Picture::Image(b)) => {
                assert_eq!((b.width, b.height), (64, 48));
                assert_eq!(centre(&b), [0, 0, 255, 255]);
            }
            _ => panic!("no picture for a shortcut to one"),
        }
        match make(&to_program, 96, &mut sources) {
            Some(Picture::Image(b)) => assert_ne!(centre(&b), [0, 0, 255, 255]),
            Some(Picture::Shared(_)) => {}
            None => panic!("no icon for a shortcut to a program"),
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}
