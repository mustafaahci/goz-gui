//! List-view messages (SysListView32). Each checks the window's class
//! first: LVM_* share their numbers with other classes' messages.
//!
//! The item and column structures go in by value: the pointers in them
//! (the text, column arrays) are set here, never taken from the caller.

use windows_sys::Win32::Foundation::{COLORREF, HWND, POINT, RECT};
use windows_sys::Win32::UI::Controls::{
    HIMAGELIST, LVCF_TEXT, LVCOLUMNW, LVHITTESTINFO, LVIF_COLFMT, LVIF_COLUMNS, LVIF_TEXT, LVITEMW,
    LVM_ARRANGE, LVM_CANCELEDITLABEL, LVM_DELETEALLITEMS, LVM_DELETECOLUMN, LVM_DELETEITEM,
    LVM_EDITLABELW, LVM_ENSUREVISIBLE, LVM_GETCOLUMNORDERARRAY, LVM_GETCOLUMNW, LVM_GETCOLUMNWIDTH,
    LVM_GETCOUNTPERPAGE, LVM_GETEDITCONTROL, LVM_GETEXTENDEDLISTVIEWSTYLE, LVM_GETHEADER,
    LVM_GETIMAGELIST, LVM_GETITEMCOUNT, LVM_GETITEMRECT, LVM_GETITEMSPACING, LVM_GETITEMSTATE,
    LVM_GETITEMW, LVM_GETNEXTITEM, LVM_GETSELECTEDCOUNT, LVM_GETSELECTIONMARK, LVM_GETSTRINGWIDTHW,
    LVM_GETSUBITEMRECT, LVM_GETTOOLTIPS, LVM_GETTOPINDEX, LVM_GETVIEW, LVM_HITTEST,
    LVM_INSERTCOLUMNW, LVM_INSERTITEMW, LVM_REDRAWITEMS, LVM_SCROLL, LVM_SETBKCOLOR,
    LVM_SETCOLUMNORDERARRAY, LVM_SETCOLUMNW, LVM_SETCOLUMNWIDTH, LVM_SETEXTENDEDLISTVIEWSTYLE,
    LVM_SETIMAGELIST, LVM_SETITEMCOUNT, LVM_SETITEMSTATE, LVM_SETITEMTEXTW, LVM_SETITEMW,
    LVM_SETSELECTIONMARK, LVM_SETTEXTBKCOLOR, LVM_SETTEXTCOLOR, LVM_SETVIEW, LVM_SUBITEMHITTEST,
    LVM_UPDATE,
};

/// The class these messages are for.
pub const CLASS: &str = "SysListView32";

/// Sends `msg` to `lv` if it is a list view.
///
/// # Safety
/// `w` and `l` are what `msg` defines for a list view.
unsafe fn send(lv: HWND, msg: u32, w: usize, l: isize) -> isize {
    // SAFETY: as the caller promises, to a list view (or to null, which
    // does nothing).
    unsafe { super::send_raw(super::of_class(lv, CLASS), msg, w, l) }
}

/// Sends a message whose parameters are integers.
fn send_int(lv: HWND, msg: u32, w: usize, l: isize) -> isize {
    // SAFETY: the callers' messages take integers.
    unsafe { send(lv, msg, w, l) }
}

pub fn item_count(lv: HWND) -> usize {
    usize::try_from(send_int(lv, LVM_GETITEMCOUNT, 0, 0)).unwrap_or(0)
}

/// LVM_SETITEMCOUNT of a virtual list, with LVSICF_* flags.
pub fn set_item_count(lv: HWND, count: usize, flags: u32) {
    send_int(lv, LVM_SETITEMCOUNT, count, flags as isize);
}

pub fn selected_count(lv: HWND) -> usize {
    usize::try_from(send_int(lv, LVM_GETSELECTEDCOUNT, 0, 0)).unwrap_or(0)
}

/// LVM_GETNEXTITEM: the next item after `start` (-1 for the first) with
/// LVNI_* `flags`; -1 for none.
pub fn next_item(lv: HWND, start: i32, flags: u32) -> i32 {
    send_int(lv, LVM_GETNEXTITEM, start as isize as usize, flags as isize) as i32
}

pub fn ensure_visible(lv: HWND, item: i32, partial_ok: bool) {
    send_int(
        lv,
        LVM_ENSUREVISIBLE,
        item as isize as usize,
        isize::from(partial_ok),
    );
}

pub fn selection_mark(lv: HWND) -> i32 {
    send_int(lv, LVM_GETSELECTIONMARK, 0, 0) as i32
}

/// Sets the selection mark; the one before.
pub fn set_selection_mark(lv: HWND, item: i32) -> i32 {
    send_int(lv, LVM_SETSELECTIONMARK, 0, item as isize) as i32
}

pub fn top_index(lv: HWND) -> i32 {
    send_int(lv, LVM_GETTOPINDEX, 0, 0) as i32
}

pub fn count_per_page(lv: HWND) -> i32 {
    send_int(lv, LVM_GETCOUNTPERPAGE, 0, 0) as i32
}

pub fn extended_style(lv: HWND) -> u32 {
    send_int(lv, LVM_GETEXTENDEDLISTVIEWSTYLE, 0, 0) as u32
}

/// Sets the LVS_EX_* styles in `mask` to `style`.
pub fn set_extended_style(lv: HWND, mask: u32, style: u32) {
    send_int(
        lv,
        LVM_SETEXTENDEDLISTVIEWSTYLE,
        mask as usize,
        style as isize,
    );
}

pub fn header(lv: HWND) -> HWND {
    send_int(lv, LVM_GETHEADER, 0, 0) as HWND
}

pub fn tooltips(lv: HWND) -> HWND {
    send_int(lv, LVM_GETTOOLTIPS, 0, 0) as HWND
}

pub fn edit_control(lv: HWND) -> HWND {
    send_int(lv, LVM_GETEDITCONTROL, 0, 0) as HWND
}

/// Starts editing item `item`'s label; the edit box (null if it did not).
pub fn edit_label(lv: HWND, item: i32) -> HWND {
    send_int(lv, LVM_EDITLABELW, item as isize as usize, 0) as HWND
}

pub fn cancel_edit_label(lv: HWND) {
    send_int(lv, LVM_CANCELEDITLABEL, 0, 0);
}

/// Redraws items `first` through `last` (once the window updates).
pub fn redraw_items(lv: HWND, first: i32, last: i32) {
    send_int(lv, LVM_REDRAWITEMS, first as isize as usize, last as isize);
}

pub fn update(lv: HWND, item: i32) {
    send_int(lv, LVM_UPDATE, item as isize as usize, 0);
}

/// Scrolls by `dx` pixels and `dy` (pixels; in report view, rounded to
/// rows).
pub fn scroll(lv: HWND, dx: i32, dy: i32) {
    send_int(lv, LVM_SCROLL, dx as isize as usize, dy as isize);
}

pub fn set_text_color(lv: HWND, c: COLORREF) {
    send_int(lv, LVM_SETTEXTCOLOR, 0, c as isize);
}

pub fn set_text_bk_color(lv: HWND, c: COLORREF) {
    send_int(lv, LVM_SETTEXTBKCOLOR, 0, c as isize);
}

pub fn set_bk_color(lv: HWND, c: COLORREF) {
    send_int(lv, LVM_SETBKCOLOR, 0, c as isize);
}

pub fn view(lv: HWND) -> u32 {
    send_int(lv, LVM_GETVIEW, 0, 0) as u32
}

pub fn set_view(lv: HWND, view: u32) {
    send_int(lv, LVM_SETVIEW, view as usize, 0);
}

/// LVM_ARRANGE with LVA_* `how`.
pub fn arrange(lv: HWND, how: u32) {
    send_int(lv, LVM_ARRANGE, how as usize, 0);
}

/// The spacing of items in icon views (or small icon views): (cx, cy).
pub fn item_spacing(lv: HWND, small: bool) -> (i32, i32) {
    let v = send_int(lv, LVM_GETITEMSPACING, usize::from(small), 0) as u32;
    ((v & 0xffff) as i32, (v >> 16) as i32)
}

pub fn image_list(lv: HWND, which: u32) -> HIMAGELIST {
    send_int(lv, LVM_GETIMAGELIST, which as usize, 0)
}

/// Sets image list `which` (LVSIL_*); the one before.
pub fn set_image_list(lv: HWND, which: u32, il: HIMAGELIST) -> HIMAGELIST {
    send_int(lv, LVM_SETIMAGELIST, which as usize, il)
}

pub fn delete_item(lv: HWND, item: i32) -> bool {
    send_int(lv, LVM_DELETEITEM, item as isize as usize, 0) != 0
}

pub fn delete_all_items(lv: HWND) -> bool {
    send_int(lv, LVM_DELETEALLITEMS, 0, 0) != 0
}

pub fn delete_column(lv: HWND, col: i32) -> bool {
    send_int(lv, LVM_DELETECOLUMN, col as isize as usize, 0) != 0
}

pub fn column_width(lv: HWND, col: i32) -> i32 {
    send_int(lv, LVM_GETCOLUMNWIDTH, col as isize as usize, 0) as i32
}

/// Sets column `col`'s width (or LVSCW_AUTOSIZE…).
pub fn set_column_width(lv: HWND, col: i32, width: i32) -> bool {
    send_int(
        lv,
        LVM_SETCOLUMNWIDTH,
        col as isize as usize,
        width as isize,
    ) != 0
}

/// The item's state bits in `mask` (LVIS_*).
pub fn item_state(lv: HWND, item: i32, mask: u32) -> u32 {
    send_int(lv, LVM_GETITEMSTATE, item as isize as usize, mask as isize) as u32
}

/// Sets the state bits in `mask` of item `item` (-1 for every item) to
/// `state`.
pub fn set_item_state(lv: HWND, item: i32, state: u32, mask: u32) -> bool {
    let it = LVITEMW {
        state,
        stateMask: mask,
        ..Default::default()
    };
    // SAFETY: LVM_SETITEMSTATE's lParam is an LVITEMW (only its state
    // fields read), alive for the call.
    unsafe {
        send(
            lv,
            LVM_SETITEMSTATE,
            item as isize as usize,
            (&raw const it) as isize,
        ) != 0
    }
}

/// LVM_GETITEMRECT: item `item`'s rectangle of part `code` (LVIR_*).
pub fn item_rect(lv: HWND, item: i32, code: i32) -> Option<RECT> {
    let mut rc = RECT {
        left: code,
        ..Default::default()
    };
    // SAFETY: LVM_GETITEMRECT's lParam is a RECT, in and out.
    let ok = unsafe {
        send(
            lv,
            LVM_GETITEMRECT,
            item as isize as usize,
            (&raw mut rc) as isize,
        )
    };
    (ok != 0).then_some(rc)
}

/// LVM_GETSUBITEMRECT: the rectangle of subitem `sub` of item `item`, of
/// part `code` (LVIR_*).
pub fn subitem_rect(lv: HWND, item: i32, sub: i32, code: i32) -> Option<RECT> {
    let mut rc = RECT {
        left: code,
        top: sub,
        ..Default::default()
    };
    // SAFETY: LVM_GETSUBITEMRECT's lParam is a RECT, in and out.
    let ok = unsafe {
        send(
            lv,
            LVM_GETSUBITEMRECT,
            item as isize as usize,
            (&raw mut rc) as isize,
        )
    };
    (ok != 0).then_some(rc)
}

/// LVM_HITTEST at `pt` (client coordinates): the filled LVHITTESTINFO.
pub fn hit_test(lv: HWND, pt: POINT) -> LVHITTESTINFO {
    let mut ht = LVHITTESTINFO {
        pt,
        ..Default::default()
    };
    // SAFETY: LVM_HITTEST's lParam is an LVHITTESTINFO, in and out.
    unsafe { send(lv, LVM_HITTEST, usize::MAX, (&raw mut ht) as isize) };
    ht
}

/// LVM_SUBITEMHITTEST at `pt`: the filled LVHITTESTINFO.
pub fn subitem_hit_test(lv: HWND, pt: POINT) -> LVHITTESTINFO {
    let mut ht = LVHITTESTINFO {
        pt,
        ..Default::default()
    };
    // SAFETY: LVM_SUBITEMHITTEST's lParam is an LVHITTESTINFO, in and out.
    unsafe { send(lv, LVM_SUBITEMHITTEST, usize::MAX, (&raw mut ht) as isize) };
    ht
}

/// The width of `text` in the list's font.
pub fn string_width(lv: HWND, text: &str) -> i32 {
    let z = super::wz(text);
    // SAFETY: a NUL-terminated string, alive for the call.
    unsafe { send(lv, LVM_GETSTRINGWIDTHW, 0, z.as_ptr() as isize) as i32 }
}

/// An item's text, and the text-free fields `item` asks for (LVIF_*): the
/// item `item.iItem`, subitem `item.iSubItem`. The text is the item's when
/// `item.mask` has LVIF_TEXT, up to `max` units.
pub fn get_item(lv: HWND, item: LVITEMW, max: usize) -> Option<(LVITEMW, Option<String>)> {
    let mut it = item;
    it.mask &= !(LVIF_COLUMNS | LVIF_COLFMT);
    it.puColumns = core::ptr::null_mut();
    it.piColFmt = core::ptr::null_mut();
    let mut buf = vec![
        0u16;
        if it.mask & LVIF_TEXT != 0 {
            max.max(1)
        } else {
            0
        }
    ];
    it.pszText = buf.as_mut_ptr();
    it.cchTextMax = buf.len().min(i32::MAX as usize) as i32;
    // SAFETY: LVM_GETITEMW's lParam is an LVITEMW whose only pointer is the
    // text buffer of cchTextMax units (the column arrays are cleared).
    let ok = unsafe { send(lv, LVM_GETITEMW, 0, (&raw mut it) as isize) } != 0;
    if !ok {
        return None;
    }
    let text = (it.mask & LVIF_TEXT != 0).then(|| text_of(&it, &buf));
    it.pszText = core::ptr::null_mut();
    it.cchTextMax = 0;
    Some((it, text))
}

/// The text an LVITEMW came back with: in the buffer, or (for a callback
/// list) where the list pointed instead.
fn text_of(it: &LVITEMW, buf: &[u16]) -> String {
    if it.pszText == buf.as_ptr().cast_mut() || it.pszText.is_null() {
        String::from_utf16_lossy(super::until_nul(buf))
    } else {
        // SAFETY: a list may point pszText at its own NUL-terminated copy,
        // alive until the next message to it.
        unsafe { super::msg::read_nul(it.pszText) }
            .map(|u| String::from_utf16_lossy(&u))
            .unwrap_or_default()
    }
}

/// The text of subitem `sub` of item `item`, up to `max` units.
pub fn item_text(lv: HWND, item: i32, sub: i32, max: usize) -> String {
    let it = LVITEMW {
        mask: LVIF_TEXT,
        iItem: item,
        iSubItem: sub,
        ..Default::default()
    };
    get_item(lv, it, max)
        .and_then(|(_, t)| t)
        .unwrap_or_default()
}

/// Sets the fields `item` names (LVIF_*), with `text` as the item's text
/// when given (LVIF_TEXT then). False if the list refused.
pub fn set_item(lv: HWND, item: LVITEMW, text: Option<&str>) -> bool {
    with_item(item, text, |it| {
        // SAFETY: LVM_SETITEMW's lParam is an LVITEMW whose pointers are set
        // by `with_item`.
        unsafe { send(lv, LVM_SETITEMW, 0, (&raw const *it) as isize) != 0 }
    })
}

/// Inserts `item` (at `item.iItem`), with `text`; its index, -1 if not.
pub fn insert_item(lv: HWND, item: LVITEMW, text: Option<&str>) -> i32 {
    with_item(item, text, |it| {
        // SAFETY: as in `set_item`.
        unsafe { send(lv, LVM_INSERTITEMW, 0, (&raw const *it) as isize) as i32 }
    })
}

/// Sets the text of subitem `sub` of item `item`.
pub fn set_item_text(lv: HWND, item: i32, sub: i32, text: &str) -> bool {
    let z = super::wz(text);
    let it = LVITEMW {
        iSubItem: sub,
        pszText: z.as_ptr().cast_mut(),
        ..Default::default()
    };
    // SAFETY: LVM_SETITEMTEXTW's lParam is an LVITEMW whose text is read
    // during the call.
    unsafe {
        send(
            lv,
            LVM_SETITEMTEXTW,
            item as isize as usize,
            (&raw const it) as isize,
        ) != 0
    }
}

/// `item` with its pointers set here: the text (or none), no column arrays.
fn with_item<R>(item: LVITEMW, text: Option<&str>, f: impl FnOnce(&LVITEMW) -> R) -> R {
    let mut it = item;
    it.mask &= !(LVIF_COLUMNS | LVIF_COLFMT);
    it.puColumns = core::ptr::null_mut();
    it.piColFmt = core::ptr::null_mut();
    let z = text.map(super::wz);
    match &z {
        Some(z) => {
            it.mask |= LVIF_TEXT;
            it.pszText = z.as_ptr().cast_mut();
        }
        None => {
            it.mask &= !LVIF_TEXT;
            it.pszText = core::ptr::null_mut();
        }
    }
    it.cchTextMax = 0;
    f(&it)
}

/// `col` with its text pointer set here.
fn with_column<R>(col: LVCOLUMNW, text: Option<&str>, f: impl FnOnce(&LVCOLUMNW) -> R) -> R {
    let mut c = col;
    let z = text.map(super::wz);
    match &z {
        Some(z) => {
            c.mask |= LVCF_TEXT;
            c.pszText = z.as_ptr().cast_mut();
        }
        None => {
            c.mask &= !LVCF_TEXT;
            c.pszText = core::ptr::null_mut();
        }
    }
    c.cchTextMax = 0;
    f(&c)
}

/// Inserts column `col` (LVCF_* fields), titled `text`; its index, -1 if
/// not.
pub fn insert_column(lv: HWND, at: i32, col: LVCOLUMNW, text: Option<&str>) -> i32 {
    with_column(col, text, |c| {
        // SAFETY: LVM_INSERTCOLUMNW's lParam is an LVCOLUMNW whose text is
        // set by `with_column`.
        unsafe {
            send(
                lv,
                LVM_INSERTCOLUMNW,
                at as isize as usize,
                (&raw const *c) as isize,
            ) as i32
        }
    })
}

/// Sets the fields `col` names of column `at`, titled `text` when given.
pub fn set_column(lv: HWND, at: i32, col: LVCOLUMNW, text: Option<&str>) -> bool {
    with_column(col, text, |c| {
        // SAFETY: as in `insert_column`.
        unsafe {
            send(
                lv,
                LVM_SETCOLUMNW,
                at as isize as usize,
                (&raw const *c) as isize,
            ) != 0
        }
    })
}

/// The fields `mask` asks for (LVCF_*) of column `at`, and its title when
/// LVCF_TEXT is among them (up to 260 units).
pub fn get_column(lv: HWND, at: i32, mask: u32) -> Option<(LVCOLUMNW, Option<String>)> {
    let mut buf = [0u16; 260];
    let mut c = LVCOLUMNW {
        mask,
        ..Default::default()
    };
    if mask & LVCF_TEXT != 0 {
        c.pszText = buf.as_mut_ptr();
        c.cchTextMax = buf.len() as i32;
    }
    // SAFETY: LVM_GETCOLUMNW's lParam is an LVCOLUMNW whose only pointer is
    // the text buffer of cchTextMax units.
    let ok = unsafe {
        send(
            lv,
            LVM_GETCOLUMNW,
            at as isize as usize,
            (&raw mut c) as isize,
        )
    } != 0;
    if !ok {
        return None;
    }
    let text = (mask & LVCF_TEXT != 0).then(|| String::from_utf16_lossy(super::until_nul(&buf)));
    c.pszText = core::ptr::null_mut();
    c.cchTextMax = 0;
    Some((c, text))
}

/// The columns' display order (`count` of them: the header's item count).
pub fn column_order(lv: HWND, count: usize) -> Vec<i32> {
    let mut order = vec![0i32; count];
    // SAFETY: LVM_GETCOLUMNORDERARRAY's lParam is an array of wParam ints.
    let ok = unsafe {
        send(
            lv,
            LVM_GETCOLUMNORDERARRAY,
            order.len(),
            order.as_mut_ptr() as isize,
        )
    } != 0;
    if ok { order } else { Vec::new() }
}

/// Sets the columns' display order.
pub fn set_column_order(lv: HWND, order: &[i32]) -> bool {
    // SAFETY: LVM_SETCOLUMNORDERARRAY's lParam is an array of wParam ints.
    unsafe {
        send(
            lv,
            LVM_SETCOLUMNORDERARRAY,
            order.len(),
            order.as_ptr() as isize,
        ) != 0
    }
}
