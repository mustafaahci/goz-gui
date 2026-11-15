//! Tab-control messages (SysTabControl32), each sent only to a tab control.

use windows_sys::Win32::Foundation::{HWND, RECT};
use windows_sys::Win32::UI::Controls::{
    TCIF_TEXT, TCITEMW, TCM_ADJUSTRECT, TCM_DELETEALLITEMS, TCM_GETCURSEL, TCM_GETITEMCOUNT,
    TCM_GETITEMRECT, TCM_INSERTITEMW, TCM_SETCURSEL, TCM_SETITEMW,
};

pub const CLASS: &str = "SysTabControl32";

/// # Safety
/// `w` and `l` are what `msg` defines for a tab control.
unsafe fn send(t: HWND, msg: u32, w: usize, l: isize) -> isize {
    // SAFETY: as the caller promises, to a tab control (or to null).
    unsafe { super::send_raw(super::of_class(t, CLASS), msg, w, l) }
}

pub fn count(t: HWND) -> usize {
    // SAFETY: no parameters.
    usize::try_from(unsafe { send(t, TCM_GETITEMCOUNT, 0, 0) }).unwrap_or(0)
}

pub fn cur_sel(t: HWND) -> i32 {
    // SAFETY: no parameters.
    unsafe { send(t, TCM_GETCURSEL, 0, 0) as i32 }
}

pub fn set_cur_sel(t: HWND, i: i32) -> i32 {
    // SAFETY: an integer.
    unsafe { send(t, TCM_SETCURSEL, i as isize as usize, 0) as i32 }
}

pub fn delete_all(t: HWND) {
    // SAFETY: no parameters.
    unsafe { send(t, TCM_DELETEALLITEMS, 0, 0) };
}

pub fn item_rect(t: HWND, i: i32) -> Option<RECT> {
    let mut rc = RECT::default();
    // SAFETY: TCM_GETITEMRECT's lParam is a RECT out.
    let ok = unsafe {
        send(
            t,
            TCM_GETITEMRECT,
            i as isize as usize,
            (&raw mut rc) as isize,
        )
    } != 0;
    ok.then_some(rc)
}

/// TCM_ADJUSTRECT: the display area for a window rectangle `rc` (or the
/// window rectangle for a display area, when `larger`).
pub fn adjust_rect(t: HWND, larger: bool, rc: RECT) -> RECT {
    let mut r = rc;
    // SAFETY: TCM_ADJUSTRECT's lParam is a RECT in-out.
    unsafe {
        send(
            t,
            TCM_ADJUSTRECT,
            usize::from(larger),
            (&raw mut r) as isize,
        )
    };
    r
}

/// Inserts a tab titled `text` at `i` (with `item`'s other TCIF_* fields);
/// its index, -1 if not.
pub fn insert(t: HWND, i: i32, item: TCITEMW, text: &str) -> i32 {
    let z = super::wz(text);
    let mut it = item;
    it.mask |= TCIF_TEXT;
    it.pszText = z.as_ptr().cast_mut();
    it.cchTextMax = 0;
    // SAFETY: TCM_INSERTITEMW's lParam is a TCITEMW whose text is set above.
    unsafe {
        send(
            t,
            TCM_INSERTITEMW,
            i as isize as usize,
            (&raw const it) as isize,
        ) as i32
    }
}

/// Sets the title of tab `i`.
pub fn set_text(t: HWND, i: i32, text: &str) -> bool {
    let z = super::wz(text);
    let it = TCITEMW {
        mask: TCIF_TEXT,
        pszText: z.as_ptr().cast_mut(),
        ..Default::default()
    };
    // SAFETY: TCM_SETITEMW's lParam is a TCITEMW whose text is set above.
    unsafe {
        send(
            t,
            TCM_SETITEMW,
            i as isize as usize,
            (&raw const it) as isize,
        ) != 0
    }
}
