//! List-box messages (LB_* are below WM_USER: the same for every window).

use windows_sys::Win32::Foundation::{HWND, RECT};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    LB_ADDSTRING, LB_DELETESTRING, LB_FINDSTRINGEXACT, LB_GETCOUNT, LB_GETCURSEL, LB_GETITEMDATA,
    LB_GETITEMHEIGHT, LB_GETITEMRECT, LB_GETSEL, LB_GETSELCOUNT, LB_GETSELITEMS, LB_GETTEXT,
    LB_GETTEXTLEN, LB_GETTOPINDEX, LB_INSERTSTRING, LB_ITEMFROMPOINT, LB_RESETCONTENT,
    LB_SETCURSEL, LB_SETITEMDATA, LB_SETITEMHEIGHT, LB_SETSEL, LB_SETTOPINDEX,
};

fn send_int(lb: HWND, msg: u32, w: usize, l: isize) -> isize {
    // SAFETY: the callers' messages take integers.
    unsafe { super::send_raw(lb, msg, w, l) }
}

/// Adds `text`; its index (LB_ERR -1 or LB_ERRSPACE -2 if not).
pub fn add(lb: HWND, text: &str) -> i32 {
    let z = super::wz(text);
    // SAFETY: a NUL-terminated string read (or, for a list without strings,
    // kept as a number) during the call.
    unsafe { super::send_raw(lb, LB_ADDSTRING, 0, z.as_ptr() as isize) as i32 }
}

/// Inserts `text` at `i` (-1: at the end); its index.
pub fn insert(lb: HWND, i: i32, text: &str) -> i32 {
    let z = super::wz(text);
    // SAFETY: as in `add`.
    unsafe {
        super::send_raw(
            lb,
            LB_INSERTSTRING,
            i as isize as usize,
            z.as_ptr() as isize,
        ) as i32
    }
}

/// Deletes item `i`; how many are left (LB_ERR if there was none).
pub fn delete(lb: HWND, i: i32) -> i32 {
    send_int(lb, LB_DELETESTRING, i as isize as usize, 0) as i32
}

/// Empties the list.
pub fn reset(lb: HWND) {
    send_int(lb, LB_RESETCONTENT, 0, 0);
}

/// How many items the list has.
pub fn count(lb: HWND) -> i32 {
    send_int(lb, LB_GETCOUNT, 0, 0) as i32
}

/// The selected item of a single-selection list (LB_ERR -1 for none).
pub fn cur_sel(lb: HWND) -> i32 {
    send_int(lb, LB_GETCURSEL, 0, 0) as i32
}

/// Selects item `i` of a single-selection list (-1: none).
pub fn set_cur_sel(lb: HWND, i: i32) -> i32 {
    send_int(lb, LB_SETCURSEL, i as isize as usize, 0) as i32
}

/// Selects or unselects item `i` (-1: all) of a multiple-selection list.
pub fn set_sel(lb: HWND, on: bool, i: i32) {
    send_int(lb, LB_SETSEL, usize::from(on), i as isize);
}

/// Whether item `i` is selected.
pub fn is_selected(lb: HWND, i: i32) -> bool {
    send_int(lb, LB_GETSEL, i as isize as usize, 0) > 0
}

/// The selected items of a multiple-selection list.
pub fn selected(lb: HWND) -> Vec<i32> {
    let n = send_int(lb, LB_GETSELCOUNT, 0, 0);
    let Ok(n) = usize::try_from(n) else {
        return Vec::new();
    };
    let mut v = vec![0i32; n];
    // SAFETY: LB_GETSELITEMS's lParam is an array of wParam ints.
    let got = unsafe { super::send_raw(lb, LB_GETSELITEMS, v.len(), v.as_mut_ptr() as isize) };
    v.truncate(usize::try_from(got).unwrap_or(0));
    v
}

/// The text of item `i` (empty if there is none).
pub fn text(lb: HWND, i: i32) -> String {
    let len = send_int(lb, LB_GETTEXTLEN, i as isize as usize, 0);
    let Ok(len) = usize::try_from(len) else {
        return String::new();
    };
    // Room for the text and its NUL, or the item's data, which a list
    // without strings copies instead.
    let mut buf = vec![0u16; (len + 1).max(8)];
    // SAFETY: a buffer with room for what LB_GETTEXTLEN measured and a NUL
    // (or the item data).
    let n = unsafe {
        super::send_raw(
            lb,
            LB_GETTEXT,
            i as isize as usize,
            buf.as_mut_ptr() as isize,
        )
    };
    let n = usize::try_from(n).unwrap_or(0).min(len);
    String::from_utf16_lossy(&buf[..n])
}

/// The index of the item whose text is `text` (ignoring case), from after `after` (-1: from the
/// start); -1 if none.
pub fn find_exact(lb: HWND, after: i32, text: &str) -> i32 {
    let z = super::wz(text);
    // SAFETY: a NUL-terminated string read during the call.
    unsafe {
        super::send_raw(
            lb,
            LB_FINDSTRINGEXACT,
            after as isize as usize,
            z.as_ptr() as isize,
        ) as i32
    }
}

/// The number item `i` keeps (LB_SETITEMDATA's).
pub fn item_data(lb: HWND, i: i32) -> isize {
    send_int(lb, LB_GETITEMDATA, i as isize as usize, 0)
}

/// Keeps a number with item `i`.
pub fn set_item_data(lb: HWND, i: i32, data: isize) {
    send_int(lb, LB_SETITEMDATA, i as isize as usize, data);
}

/// The height of item `i` (of every item, unless LBS_OWNERDRAWVARIABLE).
pub fn item_height(lb: HWND, i: i32) -> i32 {
    send_int(lb, LB_GETITEMHEIGHT, i as isize as usize, 0) as i32
}

/// Sets the height of item `i` (of every item, unless LBS_OWNERDRAWVARIABLE).
pub fn set_item_height(lb: HWND, i: i32, height: i32) {
    send_int(lb, LB_SETITEMHEIGHT, i as isize as usize, height as isize);
}

/// Item `i`'s rectangle, in the list's client coordinates.
pub fn item_rect(lb: HWND, i: i32) -> Option<RECT> {
    let mut rc = RECT::default();
    // SAFETY: LB_GETITEMRECT's lParam is a RECT out.
    let r = unsafe {
        super::send_raw(
            lb,
            LB_GETITEMRECT,
            i as isize as usize,
            (&raw mut rc) as isize,
        )
    };
    (r != -1).then_some(rc)
}

/// The first item shown.
pub fn top_index(lb: HWND) -> i32 {
    send_int(lb, LB_GETTOPINDEX, 0, 0) as i32
}

/// Scrolls item `i` to the top.
pub fn set_top_index(lb: HWND, i: i32) {
    send_int(lb, LB_SETTOPINDEX, i as isize as usize, 0);
}

/// LB_ITEMFROMPOINT: (the nearest item, whether the point is outside the
/// client area).
pub fn item_from_point(lb: HWND, x: i32, y: i32) -> (u16, bool) {
    let v = send_int(
        lb,
        LB_ITEMFROMPOINT,
        0,
        super::make_lparam(x as u16, y as u16),
    ) as usize;
    (super::loword(v), super::hiword(v) != 0)
}
