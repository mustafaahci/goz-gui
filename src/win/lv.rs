//! List-view messages (SysListView32). Each checks the window's class
//! first: LVM_* share their numbers with other classes' messages.
//!
//! The item and column structures go in by value: the pointers in them
//! (the text, column arrays) are set here, never taken from the caller.

use std::cell::RefCell;
use std::collections::HashMap;

use windows_sys::Win32::Foundation::{COLORREF, HWND, POINT, RECT};
use windows_sys::Win32::Graphics::Gdi::HDC;
use windows_sys::Win32::UI::Controls::{
    HIMAGELIST, IMAGE_LIST_DRAW_STYLE, ImageList_Draw, LVCF_TEXT, LVCOLUMNW, LVHITTESTINFO,
    LVIF_COLFMT, LVIF_COLUMNS, LVIF_NORECOMPUTE, LVIF_TEXT, LVITEMW, LVM_ARRANGE,
    LVM_CANCELEDITLABEL, LVM_DELETEALLITEMS, LVM_DELETECOLUMN, LVM_EDITLABELW, LVM_ENSUREVISIBLE,
    LVM_GETCOLUMNORDERARRAY, LVM_GETCOLUMNW, LVM_GETCOLUMNWIDTH, LVM_GETCOUNTPERPAGE,
    LVM_GETEDITCONTROL, LVM_GETHEADER, LVM_GETHOTITEM, LVM_GETIMAGELIST, LVM_GETITEMCOUNT,
    LVM_GETITEMRECT, LVM_GETITEMSPACING, LVM_GETITEMSTATE, LVM_GETITEMTEXTW, LVM_GETITEMW,
    LVM_GETNEXTITEM, LVM_GETSELECTEDCOUNT, LVM_GETSELECTIONMARK, LVM_GETSTRINGWIDTHW,
    LVM_GETSUBITEMRECT, LVM_GETTOPINDEX, LVM_HITTEST, LVM_INSERTCOLUMNW, LVM_INSERTITEMW,
    LVM_REDRAWITEMS, LVM_SCROLL, LVM_SETBKCOLOR, LVM_SETCOLUMNORDERARRAY, LVM_SETCOLUMNWIDTH,
    LVM_SETEXTENDEDLISTVIEWSTYLE, LVM_SETICONSPACING, LVM_SETIMAGELIST, LVM_SETITEMCOUNT,
    LVM_SETITEMSTATE, LVM_SETITEMTEXTW, LVM_SETSELECTIONMARK, LVM_SETTEXTBKCOLOR, LVM_SETTEXTCOLOR,
    LVM_SUBITEMHITTEST,
};

use super::imagelist::{ImageList, SystemList};

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

/// How many items the list has.
pub fn item_count(lv: HWND) -> usize {
    usize::try_from(send_int(lv, LVM_GETITEMCOUNT, 0, 0)).unwrap_or(0)
}

/// LVM_SETITEMCOUNT of a virtual list, with LVSICF_* flags.
pub fn set_item_count(lv: HWND, count: usize, flags: u32) {
    send_int(lv, LVM_SETITEMCOUNT, count, flags as isize);
}

/// How many items are selected.
pub fn selected_count(lv: HWND) -> usize {
    usize::try_from(send_int(lv, LVM_GETSELECTEDCOUNT, 0, 0)).unwrap_or(0)
}

/// LVM_GETNEXTITEM: the next item after `start` (-1 for the first) with
/// LVNI_* `flags`; -1 for none.
pub fn next_item(lv: HWND, start: i32, flags: u32) -> i32 {
    send_int(lv, LVM_GETNEXTITEM, start as isize as usize, flags as isize) as i32
}

/// Scrolls `item` into view (`partial_ok`: part of it is enough).
pub fn ensure_visible(lv: HWND, item: i32, partial_ok: bool) {
    send_int(
        lv,
        LVM_ENSUREVISIBLE,
        item as isize as usize,
        isize::from(partial_ok),
    );
}

/// The selection mark: where a Shift+click selection starts (-1: none).
pub fn selection_mark(lv: HWND) -> i32 {
    send_int(lv, LVM_GETSELECTIONMARK, 0, 0) as i32
}

/// Sets the selection mark; the one before.
pub fn set_selection_mark(lv: HWND, item: i32) -> i32 {
    send_int(lv, LVM_SETSELECTIONMARK, 0, item as isize) as i32
}

/// The first item shown (list and details views).
pub fn top_index(lv: HWND) -> i32 {
    send_int(lv, LVM_GETTOPINDEX, 0, 0) as i32
}

/// How many items fit in the list's view at once.
pub fn count_per_page(lv: HWND) -> i32 {
    send_int(lv, LVM_GETCOUNTPERPAGE, 0, 0) as i32
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

/// The list's header control.
pub fn header(lv: HWND) -> HWND {
    send_int(lv, LVM_GETHEADER, 0, 0) as HWND
}

/// The edit box of a label being edited (null: none).
pub fn edit_control(lv: HWND) -> HWND {
    send_int(lv, LVM_GETEDITCONTROL, 0, 0) as HWND
}

/// Starts editing item `item`'s label; the edit box (null if it did not).
pub fn edit_label(lv: HWND, item: i32) -> HWND {
    send_int(lv, LVM_EDITLABELW, item as isize as usize, 0) as HWND
}

/// Ends a label's editing without a change.
pub fn cancel_edit_label(lv: HWND) {
    send_int(lv, LVM_CANCELEDITLABEL, 0, 0);
}

/// Redraws items `first` through `last` (once the window updates).
pub fn redraw_items(lv: HWND, first: i32, last: i32) {
    send_int(lv, LVM_REDRAWITEMS, first as isize as usize, last as isize);
}

/// Scrolls by `dx` pixels and `dy` (pixels; in report view, rounded to
/// rows).
pub fn scroll(lv: HWND, dx: i32, dy: i32) {
    send_int(lv, LVM_SCROLL, dx as isize as usize, dy as isize);
}

/// The items' text colour.
pub fn set_text_color(lv: HWND, c: COLORREF) {
    send_int(lv, LVM_SETTEXTCOLOR, 0, c as isize);
}

/// The colour behind the items' text (CLR_NONE: none).
pub fn set_text_bk_color(lv: HWND, c: COLORREF) {
    send_int(lv, LVM_SETTEXTBKCOLOR, 0, c as isize);
}

/// The list's background colour.
pub fn set_bk_color(lv: HWND, c: COLORREF) {
    send_int(lv, LVM_SETBKCOLOR, 0, c as isize);
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

/// LVM_SETICONSPACING: the spacing of icons in icon view, `cx` × `cy`
/// (-1 for the default); the spacing before, as (cx, cy).
pub fn set_icon_spacing(lv: HWND, cx: i32, cy: i32) -> (i32, i32) {
    let l = super::make_lparam(cx as u16, cy as u16);
    let v = send_int(lv, LVM_SETICONSPACING, 0, l) as usize;
    (i32::from(super::loword(v)), i32::from(super::hiword(v)))
}

/// LVM_GETHOTITEM: the item under the mouse, -1 for none.
pub fn hot_item(lv: HWND) -> i32 {
    send_int(lv, LVM_GETHOTITEM, 0, 0) as i32
}

/// Draws image `i` of the list's image list `which` (LVSIL_*) on `hdc` at
/// `x, y`; false if the list has none (or the image did not draw).
pub fn draw_image(
    lv: HWND,
    which: u32,
    i: i32,
    hdc: HDC,
    at: (i32, i32),
    style: IMAGE_LIST_DRAW_STYLE,
) -> bool {
    let il = send_int(lv, LVM_GETIMAGELIST, which as usize, 0);
    if il == 0 {
        return false;
    }
    // SAFETY: the list's own image list, which it keeps alive while it
    // shows it: one it was given here (`set_image_list`: one it destroys
    // only with itself, or a system list) or by the system. Nothing here
    // can change it during the call (drawing sends the list no message).
    unsafe { ImageList_Draw(il, i, hdc, at.0, at.1, style) != 0 }
}

/// What a list view was given as one of its image lists by
/// [`set_image_list`] or [`set_system_image_list`], while it shows it.
#[derive(Clone, Copy)]
enum Given {
    /// A list given up whole ([`ImageList::into_unique`]): the list view's
    /// own, destroyed with it (or, given back, goz-gui's again).
    Own(HIMAGELIST),
    /// A reference of its own to a system list
    /// ([`SystemList::new_reference`]): released with it (or, given back,
    /// by goz-gui).
    System(HIMAGELIST),
}

thread_local! {
    /// What each list view of this thread was given, by its window and
    /// image list kind (LVSIL_*), until it gives it back or is destroyed.
    static GIVEN: RefCell<HashMap<(usize, u32), Given>> = RefCell::new(HashMap::new());
}

static GIVEN_KEY: super::subclass::Key = super::subclass::Key::new("list view image lists");

/// Gives the list view `lv` (one of this thread) image list `which`
/// (LVSIL_*): `list`, which it takes whole (a list view destroys its image
/// lists with itself, unless it has LVS_SHAREIMAGELISTS: then the list is
/// kept for the process), or none. The list it had, if it was given it
/// here, is goz-gui's again and destroyed (a system list's reference,
/// released). False, and `list` dropped, if it is not a list view of this
/// thread, or `list` is held elsewhere too (a clone, or a tree showing
/// it), which the list view could then destroy under its other holders.
pub fn set_image_list(lv: HWND, which: u32, list: Option<ImageList>) -> bool {
    let il = match list.map(ImageList::into_unique) {
        None => 0,
        Some(Ok(il)) => il,
        Some(Err(_shared)) => return false,
    };
    let given = (il != 0).then_some(Given::Own(il));
    let ok = give(lv, which, il, given);
    if !ok && il != 0 {
        // SAFETY: the list given up above, which the list view did not take.
        drop(unsafe { ImageList::reclaim(il) });
    }
    ok
}

/// Gives the list view `lv` (one of this thread) the shell's system image
/// list `list` as its image list `which` (LVSIL_*), with a reference of its
/// own: a list view without LVS_SHAREIMAGELISTS destroys its lists with
/// itself, which for a system list releases that reference. The list it
/// had is let go as in [`set_image_list`]. False if `lv` is not a list
/// view of this thread.
pub fn set_system_image_list(lv: HWND, which: u32, list: SystemList) -> bool {
    let il = list.new_reference();
    if il == 0 {
        return false;
    }
    let ok = give(lv, which, il, Some(Given::System(il)));
    if !ok {
        release_system(il);
    }
    ok
}

/// Releases a reference to a system list (an `IImageList*` of its own,
/// from SHGetImageList).
fn release_system(il: HIMAGELIST) {
    // SAFETY: a reference SHGetImageList gave (`SystemList::new_reference`)
    // which nothing holds any more, released once; the list itself lives
    // on (`imagelist::system` keeps one).
    drop(unsafe {
        <windows::core::IUnknown as windows::core::Interface>::from_raw(
            il as *mut core::ffi::c_void,
        )
    });
}

/// Sends LVM_SETIMAGELIST of `il` to `lv`, notes `given` for it, and lets
/// go of the list it gives back if it was one given here.
fn give(lv: HWND, which: u32, il: HIMAGELIST, given: Option<Given>) -> bool {
    if !super::class_is(lv, CLASS) || !super::is_this_thread(lv) {
        return false;
    }
    // Its entries go with it: its own lists are then its to destroy.
    if !super::subclass::has(lv, &GIVEN_KEY)
        && !super::subclass::set(lv, &GIVEN_KEY, |m| {
            if m.id == windows_sys::Win32::UI::WindowsAndMessaging::WM_NCDESTROY {
                let key = m.hwnd as usize;
                GIVEN.with_borrow_mut(|g| g.retain(|&(h, _), _| h != key));
            }
            m.def()
        })
    {
        return false;
    }
    let key = (lv as usize, which);
    let before = GIVEN.with_borrow_mut(|g| match given {
        Some(given) => g.insert(key, given),
        None => g.remove(&key),
    });
    let old = send_int(lv, LVM_SETIMAGELIST, which as usize, il);
    match before {
        // SAFETY: the list given up whole here before, which the list view
        // no longer shows (it gave it back): goz-gui's again.
        Some(Given::Own(had)) if had == old && old != 0 => drop(unsafe { ImageList::reclaim(old) }),
        Some(Given::System(had)) if had == old && old != 0 => release_system(old),
        _ => {}
    }
    true
}

/// Deletes every item; false if it could not.
pub fn delete_all_items(lv: HWND) -> bool {
    send_int(lv, LVM_DELETEALLITEMS, 0, 0) != 0
}

/// Deletes column `col`; false if it could not.
pub fn delete_column(lv: HWND, col: i32) -> bool {
    send_int(lv, LVM_DELETECOLUMN, col as isize as usize, 0) != 0
}

/// Column `col`'s width, in pixels.
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

/// LVM_HITTEST at `pt` (client coordinates): the filled LVHITTESTINFO,
/// its `iItem` the item hit (-1 for none). WParam 0, as goz-gui's lists
/// have always asked: the item and where on it, not its group.
pub fn hit_test(lv: HWND, pt: POINT) -> LVHITTESTINFO {
    let mut ht = LVHITTESTINFO {
        pt,
        iItem: -1,
        ..Default::default()
    };
    // SAFETY: LVM_HITTEST's lParam is an LVHITTESTINFO, in and out.
    unsafe { send(lv, LVM_HITTEST, 0, (&raw mut ht) as isize) };
    ht
}

/// LVM_SUBITEMHITTEST at `pt`: the filled LVHITTESTINFO, its `iItem` and
/// `iSubItem` the cell hit (-1 for none). WParam 0, as in [`hit_test`].
pub fn subitem_hit_test(lv: HWND, pt: POINT) -> LVHITTESTINFO {
    let mut ht = LVHITTESTINFO {
        pt,
        iItem: -1,
        iSubItem: -1,
        ..Default::default()
    };
    // SAFETY: LVM_SUBITEMHITTEST's lParam is an LVHITTESTINFO, in and out.
    unsafe { send(lv, LVM_SUBITEMHITTEST, 0, (&raw mut ht) as isize) };
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
/// `item.mask` has LVIF_TEXT, up to `max` units. LVIF_NORECOMPUTE is
/// dropped: with it a callback item's text comes back as the
/// LPSTR_TEXTCALLBACKW marker, not a pointer.
pub fn get_item(lv: HWND, item: LVITEMW, max: usize) -> Option<(LVITEMW, Option<String>)> {
    let mut it = item;
    it.mask &= !(LVIF_COLUMNS | LVIF_COLFMT | LVIF_NORECOMPUTE);
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
/// list) where the list pointed instead; none for the callback marker
/// (LPSTR_TEXTCALLBACKW, -1), which is no pointer.
fn text_of(it: &LVITEMW, buf: &[u16]) -> String {
    if it.pszText as isize == -1 {
        String::new()
    } else if it.pszText == buf.as_ptr().cast_mut() || it.pszText.is_null() {
        String::from_utf16_lossy(super::until_nul(buf))
    } else {
        // SAFETY: a list may point pszText at its own NUL-terminated copy,
        // alive until the next message to it.
        unsafe { super::msg::read_nul(it.pszText) }
            .map(|u| String::from_utf16_lossy(&u))
            .unwrap_or_default()
    }
}

/// The text of subitem `sub` of item `item`, up to `max` units
/// (LVM_GETITEMTEXTW, which copies it into the buffer given whatever the
/// item's text is: the list's, or a callback's).
pub fn item_text(lv: HWND, item: i32, sub: i32, max: usize) -> String {
    let mut buf = vec![0u16; max.max(1)];
    let it = LVITEMW {
        iSubItem: sub,
        pszText: buf.as_mut_ptr(),
        cchTextMax: super::len_i32(buf.len()),
        ..Default::default()
    };
    // SAFETY: LVM_GETITEMTEXTW's lParam is an LVITEMW whose subitem and
    // text buffer of cchTextMax units it reads, the buffer written (the
    // text copied in, cut to fit, NUL-terminated).
    let n = unsafe {
        send(
            lv,
            LVM_GETITEMTEXTW,
            item as isize as usize,
            (&raw const it) as isize,
        )
    };
    let n = usize::try_from(n).unwrap_or(0).min(buf.len());
    String::from_utf16_lossy(super::until_nul(&buf[..n]))
}

/// Inserts `item` (at `item.iItem`), with `text`; its index, -1 if not.
pub fn insert_item(lv: HWND, item: LVITEMW, text: Option<&str>) -> i32 {
    with_item(item, text, |it| {
        // SAFETY: LVM_INSERTITEMW's lParam is an LVITEMW, read during the
        // call, whose pointers `with_item` set: its text (alive with `it`)
        // or none, and no column arrays.
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
    if count == 0 {
        return Vec::new();
    }
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
    if order.is_empty() {
        return false;
    }
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
