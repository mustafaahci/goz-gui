//! List-box messages (LB_* are below WM_USER: the same for every window).

use windows_sys::Win32::Foundation::HWND;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CallWindowProcW, LB_ADDSTRING, LB_DELETESTRING, LB_GETCURSEL, LB_GETTEXT, LB_GETTEXTLEN,
    LB_INSERTSTRING, LB_RESETCONTENT, LB_SETCURSEL, LB_SETITEMHEIGHT,
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

/// The selected item of a single-selection list (LB_ERR -1 for none).
pub fn cur_sel(lb: HWND) -> i32 {
    send_int(lb, LB_GETCURSEL, 0, 0) as i32
}

/// Selects item `i` of a single-selection list (-1: none).
pub fn set_cur_sel(lb: HWND, i: i32) -> i32 {
    send_int(lb, LB_SETCURSEL, i as isize as usize, 0) as i32
}

/// The text of item `i` of list box `lb`, a ListBox of this thread (empty
/// if there is none, or `lb` is not one).
///
/// LB_GETTEXT takes a buffer but not its length, which LB_GETTEXTLEN is
/// asked for first: a subclass of the box could answer that with less
/// than the text. So both go to the ListBox class's own procedure,
/// called directly, with no subclass in between.
pub fn text(lb: HWND, i: i32) -> String {
    match super::system_class_proc(lb, "ListBox") {
        Some(proc) => text_through(lb, proc, i),
        None => String::new(),
    }
}

/// The text of item `i` of the list `lb`, asked of `proc`, the list's
/// class's own procedure (one handling LB_GETTEXTLEN and LB_GETTEXT, the
/// ListBox's or the ComboLBox's, from [`super::system_class_proc`]).
pub(crate) fn text_through(lb: HWND, proc: super::WndProc, i: i32) -> String {
    let w = i as isize as usize;
    // SAFETY: LB_GETTEXTLEN has an index and no pointer; to the list's own
    // class procedure, for a window of that class of this thread
    // (`system_class_proc`).
    let len = unsafe { CallWindowProcW(Some(proc), lb, LB_GETTEXTLEN, w, 0) };
    let Ok(len) = usize::try_from(len) else {
        return String::new();
    };
    // Room for the text and its NUL, or the item's data, which a list
    // without strings copies instead.
    let mut buf = vec![0u16; (len + 1).max(8)];
    // SAFETY: a buffer with room for what the same procedure measured
    // above and a NUL (or the item data), nothing having run in between.
    let n = unsafe { CallWindowProcW(Some(proc), lb, LB_GETTEXT, w, buf.as_mut_ptr() as isize) };
    let n = usize::try_from(n).unwrap_or(0).min(len);
    String::from_utf16_lossy(&buf[..n])
}

/// Sets the height of item `i` (of every item, unless LBS_OWNERDRAWVARIABLE).
pub fn set_item_height(lb: HWND, i: i32, height: i32) {
    send_int(lb, LB_SETITEMHEIGHT, i as isize as usize, height as isize);
}
