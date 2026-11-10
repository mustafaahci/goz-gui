//! What the sidebars' tree views (Folders and Bookmarks, SysTreeView32) have
//! in common: making one in a pane, items in and out, the theme, and an
//! image list of their own.
//!
//! The image list is the tree's own rather than the system's, so it can
//! hold goz-gui's icon (the Folders tree's top) and take its icons at the
//! window's DPI: each one is copied from whichever of the system's lists
//! has them at the size wanted or the next larger (16, 32, 48 px), and
//! scaled down to it, never up.

use windows_sys::Win32::Foundation::{HWND, POINT, RECT};
use windows_sys::Win32::Storage::FileSystem::{FILE_ATTRIBUTE_DIRECTORY, FILE_ATTRIBUTE_NORMAL};
use windows_sys::Win32::UI::Controls::{
    HIMAGELIST, HTREEITEM, ILC_COLOR32, ILC_MASK, ILD_NORMAL, ImageList_Create, ImageList_Destroy,
    ImageList_GetIcon, ImageList_GetIconSize, ImageList_ReplaceIcon, ImageList_SetImageCount,
    TVE_COLLAPSE, TVE_EXPAND, TVGN_CARET, TVGN_CHILD, TVGN_DROPHILITE, TVGN_NEXT, TVHITTESTINFO,
    TVI_FIRST, TVI_LAST, TVI_ROOT, TVIF_CHILDREN, TVIF_IMAGE, TVIF_PARAM, TVIF_SELECTEDIMAGE,
    TVIF_STATE, TVIF_TEXT, TVINSERTSTRUCTW, TVINSERTSTRUCTW_0, TVIS_EXPANDED, TVITEMW,
    TVM_DELETEITEM, TVM_ENSUREVISIBLE, TVM_EXPAND, TVM_GETITEMRECT, TVM_GETITEMW, TVM_GETNEXTITEM,
    TVM_GETTOOLTIPS, TVM_HITTEST, TVM_INSERTITEMW, TVM_SELECTITEM, TVM_SETBKCOLOR,
    TVM_SETIMAGELIST, TVM_SETITEMHEIGHT, TVM_SETITEMW, TVM_SETTEXTCOLOR, TVSIL_NORMAL,
};
use windows_sys::Win32::UI::Shell::{
    SHFILEINFOW, SHGFI_SMALLICON, SHGFI_SYSICONINDEX, SHGFI_USEFILEATTRIBUTES, SHGSI_SYSICONINDEX,
    SHGetFileInfoW, SHGetImageList, SHGetStockIconInfo, SHSTOCKICONINFO, SIID_FIND,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{DestroyIcon, HICON, SendMessageW, WM_SETFONT};
use windows_sys::core::GUID;

use crate::theme::{self, Palette};
use crate::wide::{nul, wz};

fn send(hwnd: HWND, msg: u32, w: usize, l: isize) -> isize {
    // SAFETY: a message to a window of this thread; any pointer in `l` is
    // the caller's and lives across the synchronous call.
    unsafe { SendMessageW(hwnd, msg, w, l) }
}

/// No item: the tree's root (as a parent), or nothing (as an answer).
pub const NONE: HTREEITEM = 0;

/// A new item: under `parent` (`NONE` for the top level), after `after`
/// (`NONE` for first, `last` for the end), with `text`, `image` and a
/// `param` of the caller's; `children` shows an expand button before any
/// child is in.
pub struct New<'a> {
    pub parent: HTREEITEM,
    pub after: Option<HTREEITEM>,
    pub text: &'a [u16],
    pub image: i32,
    pub children: bool,
    pub param: isize,
}

/// Puts `item` in `tree`; the new item, or `NONE` if it failed.
pub fn insert(tree: HWND, item: &New<'_>) -> HTREEITEM {
    let mut text = nul(item.text);
    let ins = TVINSERTSTRUCTW {
        hParent: item.parent,
        hInsertAfter: match item.after {
            None => TVI_LAST,
            Some(NONE) => TVI_FIRST,
            Some(h) => h,
        },
        Anonymous: TVINSERTSTRUCTW_0 {
            item: TVITEMW {
                mask: TVIF_TEXT | TVIF_PARAM | TVIF_IMAGE | TVIF_SELECTEDIMAGE | TVIF_CHILDREN,
                pszText: text.as_mut_ptr(),
                iImage: item.image,
                iSelectedImage: item.image,
                cChildren: i32::from(item.children),
                lParam: item.param,
                // SAFETY: the rest of TVITEMW is plain data.
                ..unsafe { core::mem::zeroed() }
            },
        },
    };
    send(tree, TVM_INSERTITEMW, 0, (&raw const ins) as isize) as HTREEITEM
}

/// `item`'s param; `None` for no item.
pub fn param(tree: HWND, item: HTREEITEM) -> Option<isize> {
    if item == NONE {
        return None;
    }
    let mut it = TVITEMW {
        mask: TVIF_PARAM,
        hItem: item,
        // SAFETY: the rest of TVITEMW is plain data.
        ..unsafe { core::mem::zeroed() }
    };
    (send(tree, TVM_GETITEMW, 0, (&raw mut it) as isize) != 0).then_some(it.lParam)
}

fn set(tree: HWND, mut it: TVITEMW) {
    send(tree, TVM_SETITEMW, 0, (&raw mut it) as isize);
}

/// Shows an expand button on `item`, or not.
pub fn set_children(tree: HWND, item: HTREEITEM, children: bool) {
    set(
        tree,
        TVITEMW {
            mask: TVIF_CHILDREN,
            hItem: item,
            cChildren: i32::from(children),
            // SAFETY: the rest of TVITEMW is plain data.
            ..unsafe { core::mem::zeroed() }
        },
    );
}

/// `item`'s text.
pub fn set_text(tree: HWND, item: HTREEITEM, text: &[u16]) {
    let mut text = nul(text);
    set(
        tree,
        TVITEMW {
            mask: TVIF_TEXT,
            hItem: item,
            pszText: text.as_mut_ptr(),
            // SAFETY: the rest of TVITEMW is plain data.
            ..unsafe { core::mem::zeroed() }
        },
    );
}

/// Whether `item` shows an expand button.
pub fn has_children(tree: HWND, item: HTREEITEM) -> bool {
    let mut it = TVITEMW {
        mask: TVIF_CHILDREN,
        hItem: item,
        // SAFETY: the rest of TVITEMW is plain data.
        ..unsafe { core::mem::zeroed() }
    };
    send(tree, TVM_GETITEMW, 0, (&raw mut it) as isize) != 0 && it.cChildren != 0
}

/// Whether `item` shows open.
pub fn is_expanded(tree: HWND, item: HTREEITEM) -> bool {
    let mut it = TVITEMW {
        mask: TVIF_STATE,
        hItem: item,
        stateMask: TVIS_EXPANDED,
        // SAFETY: the rest of TVITEMW is plain data.
        ..unsafe { core::mem::zeroed() }
    };
    send(tree, TVM_GETITEMW, 0, (&raw mut it) as isize) != 0 && it.state & TVIS_EXPANDED != 0
}

/// Opens (`open`) or closes `item`.
pub fn expand(tree: HWND, item: HTREEITEM, open: bool) {
    let code = if open { TVE_EXPAND } else { TVE_COLLAPSE };
    send(tree, TVM_EXPAND, code as usize, item);
}

/// Selects `item` (`NONE`: nothing) and scrolls it into view.
pub fn select(tree: HWND, item: HTREEITEM) {
    send(tree, TVM_SELECTITEM, TVGN_CARET as usize, item);
    if item != NONE {
        send(tree, TVM_ENSUREVISIBLE, 0, item);
    }
}

/// The selected item.
pub fn selected(tree: HWND) -> HTREEITEM {
    send(tree, TVM_GETNEXTITEM, TVGN_CARET as usize, 0) as HTREEITEM
}

/// Marks `item` as the one a context menu is for (`NONE`: none), as
/// Explorer's tree does while its menu is up.
pub fn drop_highlight(tree: HWND, item: HTREEITEM) {
    send(tree, TVM_SELECTITEM, TVGN_DROPHILITE as usize, item);
}

/// Takes `item` (and what is under it) out; `NONE` empties the tree.
pub fn delete(tree: HWND, item: HTREEITEM) {
    let item = if item == NONE { TVI_ROOT } else { item };
    send(tree, TVM_DELETEITEM, 0, item);
}

/// The items directly under `parent` (`NONE`: the top level), in order.
pub fn children(tree: HWND, parent: HTREEITEM) -> Vec<HTREEITEM> {
    let mut out = Vec::new();
    let mut at = if parent == NONE {
        send(tree, TVM_GETNEXTITEM, 0, 0)
    } else {
        send(tree, TVM_GETNEXTITEM, TVGN_CHILD as usize, parent)
    } as HTREEITEM;
    while at != NONE {
        out.push(at);
        at = send(tree, TVM_GETNEXTITEM, TVGN_NEXT as usize, at) as HTREEITEM;
    }
    out
}

/// The item at `pt` (the tree's client coordinates) and where on it
/// (`TVHT_*`).
pub fn hit(tree: HWND, pt: POINT) -> (HTREEITEM, u32) {
    let mut info = TVHITTESTINFO {
        pt,
        flags: 0,
        hItem: NONE,
    };
    send(tree, TVM_HITTEST, 0, (&raw mut info) as isize);
    (info.hItem, info.flags)
}

/// `item`'s row, or its text only (`text`), in the tree's coordinates.
pub fn item_rect(tree: HWND, item: HTREEITEM, text: bool) -> RECT {
    // TVM_GETITEMRECT takes the item in the rectangle it fills.
    let mut rc = RECT::default();
    // SAFETY: an HTREEITEM fits in the RECT's first bytes, as the message
    // wants it there.
    unsafe { core::ptr::write_unaligned((&raw mut rc).cast::<HTREEITEM>(), item) };
    if send(
        tree,
        TVM_GETITEMRECT,
        usize::from(text),
        (&raw mut rc) as isize,
    ) == 0
    {
        return RECT::default();
    }
    rc
}

/// The tree in the window's colours: the list's background and text, the
/// Explorer look (its hover and selection, dark ones in the dark theme),
/// and its tooltips to match.
pub fn apply_theme(tree: HWND, pal: &Palette) {
    let class = if pal.dark {
        "DarkMode_Explorer"
    } else {
        "Explorer"
    };
    theme::allow(tree, pal.dark);
    theme::set_class(tree, Some(class));
    send(tree, TVM_SETBKCOLOR, 0, pal.bg as isize);
    send(tree, TVM_SETTEXTCOLOR, 0, pal.fg as isize);
    let tip = send(tree, TVM_GETTOOLTIPS, 0, 0) as HWND;
    if !tip.is_null() {
        theme::allow(tip, pal.dark);
        theme::set_class(tip, Some(class));
    }
}

/// The tree's font and the height of its rows: the text with 2 px above
/// and below at 100 % (19 px with the usual 15 px font, as Everything's),
/// never less than the icons and a pixel above and below them.
pub fn set_font(tree: HWND, font: isize, font_height: i32, icon: i32, scale: impl Fn(i32) -> i32) {
    send(tree, WM_SETFONT, font as usize, 1);
    let high = (font_height + scale(4)).max(icon + scale(2));
    send(tree, TVM_SETITEMHEIGHT, high.max(1) as usize, 0);
}

/// `IID_IImageList`, which `SHGetImageList` hands the system lists out as.
const IID_IMAGE_LIST: GUID = GUID::from_u128(0x46eb5926_582e_4017_9fdf_e8998daa0950);

/// Where an icon of a tree's image list comes from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    /// goz-gui's own icon (the shell's search icon in a build without one).
    App,
    /// The system image list's icon of this index.
    System(i32),
}

/// The system image list's index of a plain folder's icon and of a plain
/// file's (a bookmark's, as in the bookmark dialogs), from the shell's
/// registry alone.
pub fn plain_icons() -> (i32, i32) {
    let lookup = |name: &str, attributes: u32| {
        // SAFETY: an attribute-only lookup into a plain out-struct.
        unsafe {
            let mut info: SHFILEINFOW = core::mem::zeroed();
            SHGetFileInfoW(
                wz(name).as_ptr(),
                attributes,
                &mut info,
                size_of::<SHFILEINFOW>() as u32,
                SHGFI_SYSICONINDEX | SHGFI_SMALLICON | SHGFI_USEFILEATTRIBUTES,
            );
            info.iIcon
        }
    };
    (
        lookup("folder", FILE_ATTRIBUTE_DIRECTORY),
        lookup("bookmark", FILE_ATTRIBUTE_NORMAL),
    )
}

/// The system image list's index of the shell's search icon (a
/// magnifier); -1 if the shell has none.
fn stock_search_icon() -> i32 {
    let mut info = SHSTOCKICONINFO {
        cbSize: size_of::<SHSTOCKICONINFO>() as u32,
        // SAFETY: the rest of SHSTOCKICONINFO is plain data.
        ..unsafe { core::mem::zeroed() }
    };
    // SAFETY: fills the struct whose size it is given.
    let hr = unsafe { SHGetStockIconInfo(SIID_FIND, SHGSI_SYSICONINDEX, &mut info) };
    if hr >= 0 { info.iSysImageIndex } else { -1 }
}

/// The system image list to copy `size` px icons from: the one whose icons
/// are that size or the least larger, else the largest.
fn system_list(size: i32) -> HIMAGELIST {
    // SHIL_LARGE (32), SHIL_SMALL (16), SHIL_EXTRALARGE (48).
    let mut best: Option<(i32, HIMAGELIST)> = None;
    let mut largest: Option<(i32, HIMAGELIST)> = None;
    for which in [1, 0, 2] {
        let mut ppv: *mut core::ffi::c_void = core::ptr::null_mut();
        // SAFETY: the shell hands out its own image list, which lives as long
        // as the process; it is never released.
        if unsafe { SHGetImageList(which, &IID_IMAGE_LIST, &mut ppv) } < 0 || ppv.is_null() {
            continue;
        }
        let list = ppv as HIMAGELIST;
        let (mut cx, mut cy) = (0, 0);
        // SAFETY: reads the size of a system image list's icons.
        unsafe { ImageList_GetIconSize(list, &mut cx, &mut cy) };
        if cx >= size && best.is_none_or(|(b, _)| cx < b) {
            best = Some((cx, list));
        }
        if largest.is_none_or(|(l, _)| cx > l) {
            largest = Some((cx, list));
        }
    }
    best.or(largest).map_or(0, |(_, list)| list)
}

/// A tree's own image list: the icons it shows, at one size.
pub struct Images {
    list: HIMAGELIST,
    size: i32,
    /// What each image is, in order.
    sources: Vec<Source>,
}

impl Images {
    /// An empty list of `size` px icons.
    pub fn new(size: i32) -> Images {
        let size = size.max(1);
        // SAFETY: a new image list, destroyed in Drop.
        let list = unsafe { ImageList_Create(size, size, ILC_COLOR32 | ILC_MASK, 8, 8) };
        Images {
            list,
            size,
            sources: Vec::new(),
        }
    }

    pub fn handle(&self) -> HIMAGELIST {
        self.list
    }

    pub fn size(&self) -> i32 {
        self.size
    }

    /// The image of `source`, put in first if it is not there yet; -1 if it
    /// could not be.
    pub fn get(&mut self, source: Source) -> i32 {
        if let Some(i) = self.sources.iter().position(|&s| s == source) {
            return i as i32;
        }
        if self.add(source) {
            self.sources.push(source);
            self.sources.len() as i32 - 1
        } else {
            -1
        }
    }

    fn add(&self, source: Source) -> bool {
        let system = |index: i32| -> HICON {
            let list = system_list(self.size);
            if list == 0 || index < 0 {
                return core::ptr::null_mut();
            }
            // SAFETY: a copy of one of the system list's icons, ours to
            // destroy.
            unsafe { ImageList_GetIcon(list, index, ILD_NORMAL) }
        };
        let icon: HICON = match source {
            // A build without an icon of its own (no assets/goz.ico) shows
            // the shell's search icon: the magnifier, as Everything's top
            // item has its own.
            Source::App => {
                let own = crate::window::app_icon(self.size);
                if own.is_null() {
                    system(stock_search_icon())
                } else {
                    own
                }
            }
            Source::System(index) => system(index),
        };
        if icon.is_null() {
            return false;
        }
        // SAFETY: the list copies the icon (drawn at its own size), after
        // which the copy made above is destroyed.
        unsafe {
            let at = ImageList_ReplaceIcon(self.list, -1, icon);
            DestroyIcon(icon);
            at >= 0
        }
    }

    /// The same images at `size` px: a new list, in the same order, so the
    /// items keep their image numbers. The caller hands the tree the new
    /// list before this one is dropped.
    pub fn resized(&self, size: i32) -> Images {
        let mut new = Images::new(size);
        for &source in &self.sources {
            // A source that fails now still takes its place, so the numbers
            // after it hold: a blank image.
            if !new.add(source) {
                // SAFETY: grows this list by one blank image.
                unsafe { ImageList_SetImageCount(new.list, new.sources.len() as u32 + 1) };
            }
            new.sources.push(source);
        }
        new
    }
}

impl Drop for Images {
    fn drop(&mut self) {
        if self.list != 0 {
            // SAFETY: the list this made; no tree shows it any more (see
            // `resized`, and the panes' destroy).
            unsafe { ImageList_Destroy(self.list) };
        }
    }
}

/// Hands `tree` the image list `images` (`None`: none).
pub fn set_images(tree: HWND, images: Option<&Images>) {
    send(
        tree,
        TVM_SETIMAGELIST,
        TVSIL_NORMAL as usize,
        images.map_or(0, Images::handle),
    );
}
