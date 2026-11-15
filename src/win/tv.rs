//! Tree-view messages (SysTreeView32), each sent only to a tree view. The
//! item structures go in by value, their text pointers set here.

use windows_sys::Win32::Foundation::{COLORREF, HWND, POINT, RECT};
use windows_sys::Win32::UI::Controls::{
    HIMAGELIST, HTREEITEM, TVHITTESTINFO, TVI_LAST, TVI_ROOT, TVIF_TEXT, TVINSERTSTRUCTW,
    TVITEMEXW, TVITEMW, TVM_DELETEITEM, TVM_ENSUREVISIBLE, TVM_EXPAND, TVM_GETITEMRECT,
    TVM_GETITEMW, TVM_GETNEXTITEM, TVM_GETTOOLTIPS, TVM_HITTEST, TVM_INSERTITEMW, TVM_SELECTITEM,
    TVM_SETBKCOLOR, TVM_SETEXTENDEDSTYLE, TVM_SETIMAGELIST, TVM_SETITEMHEIGHT, TVM_SETITEMW,
    TVM_SETLINECOLOR, TVM_SETTEXTCOLOR,
};

/// The class these messages are for.
pub const CLASS: &str = "SysTreeView32";

/// # Safety
/// `w` and `l` are what `msg` defines for a tree view.
unsafe fn send(tv: HWND, msg: u32, w: usize, l: isize) -> isize {
    // SAFETY: as the caller promises, to a tree view (or to null).
    unsafe { super::send_raw(super::of_class(tv, CLASS), msg, w, l) }
}

fn send_int(tv: HWND, msg: u32, w: usize, l: isize) -> isize {
    // SAFETY: the callers' messages take integers and item handles.
    unsafe { send(tv, msg, w, l) }
}

/// The root of a tree, for `insert`'s parent.
pub const ROOT: HTREEITEM = TVI_ROOT;
/// After the last child, for `insert`'s place.
pub const LAST: HTREEITEM = TVI_LAST;

/// TVM_GETNEXTITEM: the item related to `item` as `code` (TVGN_*); 0 for
/// none.
pub fn next_item(tv: HWND, item: HTREEITEM, code: u32) -> HTREEITEM {
    send_int(tv, TVM_GETNEXTITEM, code as usize, item)
}

/// TVM_SELECTITEM: selects `item` as `code` (TVGN_CARET, TVGN_DROPHILITE…).
pub fn select_item(tv: HWND, code: u32, item: HTREEITEM) -> bool {
    send_int(tv, TVM_SELECTITEM, code as usize, item) != 0
}

/// TVM_EXPAND with TVE_* `code`.
pub fn expand(tv: HWND, item: HTREEITEM, code: u32) -> bool {
    send_int(tv, TVM_EXPAND, code as usize, item) != 0
}

/// Expands and scrolls the tree so `item` shows.
pub fn ensure_visible(tv: HWND, item: HTREEITEM) {
    send_int(tv, TVM_ENSUREVISIBLE, 0, item);
}

/// Deletes `item` and its children ([`ROOT`]: every item).
pub fn delete_item(tv: HWND, item: HTREEITEM) -> bool {
    send_int(tv, TVM_DELETEITEM, 0, item) != 0
}

/// Sets the items' height, in pixels (-1: the default).
pub fn set_item_height(tv: HWND, height: i32) {
    send_int(tv, TVM_SETITEMHEIGHT, height as isize as usize, 0);
}

/// The items' text colour.
pub fn set_text_color(tv: HWND, c: COLORREF) {
    send_int(tv, TVM_SETTEXTCOLOR, 0, c as isize);
}

/// The tree's background colour.
pub fn set_bk_color(tv: HWND, c: COLORREF) {
    send_int(tv, TVM_SETBKCOLOR, 0, c as isize);
}

/// The colour of the lines between the items.
pub fn set_line_color(tv: HWND, c: COLORREF) {
    send_int(tv, TVM_SETLINECOLOR, 0, c as isize);
}

/// Sets the TVS_EX_* styles in `mask` to `style`.
pub fn set_extended_style(tv: HWND, mask: u32, style: u32) {
    send_int(tv, TVM_SETEXTENDEDSTYLE, mask as usize, style as isize);
}

/// Sets image list `which` (TVSIL_*); the one before.
pub fn set_image_list(tv: HWND, which: u32, il: HIMAGELIST) -> HIMAGELIST {
    send_int(tv, TVM_SETIMAGELIST, which as usize, il)
}

/// The tree's tooltip control.
pub fn tooltips(tv: HWND) -> HWND {
    send_int(tv, TVM_GETTOOLTIPS, 0, 0) as HWND
}

/// The item's rectangle (only its text's, when `text_only`).
pub fn item_rect(tv: HWND, item: HTREEITEM, text_only: bool) -> Option<RECT> {
    let mut rc = RECT::default();
    // TVM_GETITEMRECT takes the item in the rectangle's first bytes.
    // SAFETY: a RECT is 16 bytes, room for the handle; written by the
    // tree, read during the call.
    let ok = unsafe {
        (&raw mut rc).cast::<HTREEITEM>().write_unaligned(item);
        send(
            tv,
            TVM_GETITEMRECT,
            usize::from(text_only),
            (&raw mut rc) as isize,
        )
    } != 0;
    ok.then_some(rc)
}

/// TVM_HITTEST at `pt` (client coordinates).
pub fn hit_test(tv: HWND, pt: POINT) -> TVHITTESTINFO {
    let mut ht = TVHITTESTINFO {
        pt,
        ..Default::default()
    };
    // SAFETY: TVM_HITTEST's lParam is a TVHITTESTINFO, in and out.
    unsafe { send(tv, TVM_HITTEST, 0, (&raw mut ht) as isize) };
    ht
}

/// The fields `item.mask` asks for of `item.hItem`, and its text when
/// TVIF_TEXT is among them (up to `max` units).
pub fn get_item(tv: HWND, item: TVITEMW, max: usize) -> Option<(TVITEMW, Option<String>)> {
    let mut it = item;
    let mut buf = vec![
        0u16;
        if it.mask & TVIF_TEXT != 0 {
            max.max(1)
        } else {
            0
        }
    ];
    it.pszText = buf.as_mut_ptr();
    it.cchTextMax = buf.len().min(i32::MAX as usize) as i32;
    // SAFETY: TVM_GETITEMW's lParam is a TVITEMW whose only pointer is the
    // text buffer of cchTextMax units.
    let ok = unsafe { send(tv, TVM_GETITEMW, 0, (&raw mut it) as isize) } != 0;
    if !ok {
        return None;
    }
    let text = (it.mask & TVIF_TEXT != 0).then(|| {
        if it.pszText == buf.as_mut_ptr() || it.pszText.is_null() {
            String::from_utf16_lossy(super::until_nul(&buf))
        } else {
            // SAFETY: a tree may point pszText at its own NUL-terminated
            // copy instead, alive until the next message to it.
            unsafe { super::msg::read_nul(it.pszText) }
                .map(|u| String::from_utf16_lossy(&u))
                .unwrap_or_default()
        }
    });
    it.pszText = core::ptr::null_mut();
    it.cchTextMax = 0;
    Some((it, text))
}

/// The item's `lParam`.
pub fn item_param(tv: HWND, item: HTREEITEM) -> Option<isize> {
    let it = TVITEMW {
        mask: windows_sys::Win32::UI::Controls::TVIF_PARAM,
        hItem: item,
        ..Default::default()
    };
    get_item(tv, it, 0).map(|(it, _)| it.lParam)
}

/// Sets the fields `item.mask` names of `item.hItem`, with `text` as its
/// text when given (TVIF_TEXT then).
pub fn set_item(tv: HWND, item: TVITEMW, text: Option<&str>) -> bool {
    let mut it = item;
    let z = text.map(super::wz);
    match &z {
        Some(z) => {
            it.mask |= TVIF_TEXT;
            it.pszText = z.as_ptr().cast_mut();
        }
        None => {
            it.mask &= !TVIF_TEXT;
            it.pszText = core::ptr::null_mut();
        }
    }
    it.cchTextMax = 0;
    // SAFETY: TVM_SETITEMW's lParam is a TVITEMW whose text is set above.
    unsafe { send(tv, TVM_SETITEMW, 0, (&raw const it) as isize) != 0 }
}

/// Inserts `item` (its TVIF_* fields) under `parent` after `after` ([`ROOT`],
/// [`LAST`] or an item), with `text` when given (TVIF_TEXT then; for a
/// callback item, leave it out and answer TVN_GETDISPINFO). The new item; 0
/// if not.
pub fn insert(
    tv: HWND,
    parent: HTREEITEM,
    after: HTREEITEM,
    item: TVITEMEXW,
    text: Option<&str>,
) -> HTREEITEM {
    let mut it = item;
    let z = text.map(super::wz);
    match &z {
        Some(z) => {
            it.mask |= TVIF_TEXT;
            it.pszText = z.as_ptr().cast_mut();
        }
        None if it.pszText as isize == -1 => {
            // LPSTR_TEXTCALLBACKW: a marker the tree keeps, not a pointer.
        }
        None => {
            it.mask &= !TVIF_TEXT;
            it.pszText = core::ptr::null_mut();
        }
    }
    it.cchTextMax = 0;
    let mut ins = TVINSERTSTRUCTW {
        hParent: parent,
        hInsertAfter: after,
        ..Default::default()
    };
    ins.Anonymous.itemex = it;
    // SAFETY: TVM_INSERTITEMW's lParam is a TVINSERTSTRUCTW whose text is
    // set above (or the callback marker), read during the call.
    unsafe { send(tv, TVM_INSERTITEMW, 0, (&raw const ins) as isize) }
}

/// [`insert`] with UTF-16 text (a file name need not be valid Unicode, and
/// is read up to a NUL, if it has one): `item` (its TVIF_* fields,
/// TVIF_TEXT added) under `parent` after `after`, with `text`. The new
/// item; 0 if not.
pub fn insert_units(
    tv: HWND,
    parent: HTREEITEM,
    after: HTREEITEM,
    item: TVITEMW,
    text: &[u16],
) -> HTREEITEM {
    let z = super::wz_units(super::until_nul(text));
    let mut it = item;
    it.mask |= TVIF_TEXT;
    it.pszText = z.as_ptr().cast_mut();
    it.cchTextMax = 0;
    let mut ins = TVINSERTSTRUCTW {
        hParent: parent,
        hInsertAfter: after,
        ..Default::default()
    };
    ins.Anonymous.item = it;
    // SAFETY: TVM_INSERTITEMW's lParam is a TVINSERTSTRUCTW whose only
    // pointer is the text set above, read during the call.
    unsafe { send(tv, TVM_INSERTITEMW, 0, (&raw const ins) as isize) }
}

/// Sets tree item `item`'s text from UTF-16 (up to a NUL, if it has one).
pub fn set_text_units(tv: HWND, item: HTREEITEM, text: &[u16]) -> bool {
    let z = super::wz_units(super::until_nul(text));
    let it = TVITEMW {
        mask: TVIF_TEXT,
        hItem: item,
        pszText: z.as_ptr().cast_mut(),
        ..Default::default()
    };
    // SAFETY: TVM_SETITEMW's lParam is a TVITEMW whose only pointer is the
    // text set above, read during the call.
    unsafe { send(tv, TVM_SETITEMW, 0, (&raw const it) as isize) != 0 }
}
