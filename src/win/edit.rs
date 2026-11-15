//! Edit-control messages. EM_* are below WM_USER, and the ones here other
//! than EM_CHARFROMPOS mean the same to every window: to an edit box, and
//! to a Rich Edit, which redefines a few of them (EM_CHARFROMPOS takes a
//! POINTL's address there, EM_POSFROMCHAR one to write to), checked against
//! its documentation: EM_GETSEL, EM_SETSEL, EM_REPLACESEL, EM_UNDO,
//! EM_CANUNDO, EM_SETLIMITTEXT, EM_GETMARGINS, EM_SETMARGINS, EM_SETRECTNP
//! (its wParam 0: absolute coordinates) and EM_SETTABSTOPS take the same
//! parameters in both. EM_CHARFROMPOS and the cue banner (ECM_FIRST and
//! up) are sent only to an edit box.

use windows_sys::Win32::Foundation::{HWND, POINT, RECT};
use windows_sys::Win32::UI::Controls::{
    EM_CANUNDO, EM_CHARFROMPOS, EM_GETMARGINS, EM_GETSEL, EM_REPLACESEL, EM_SETCUEBANNER,
    EM_SETLIMITTEXT, EM_SETMARGINS, EM_SETRECTNP, EM_SETSEL, EM_SETTABSTOPS, EM_UNDO,
};
use windows_sys::Win32::UI::WindowsAndMessaging::PostMessageW;

/// The class the cue banner's and EM_CHARFROMPOS's messages are sent to.
pub const CLASS: &str = "Edit";

fn send_int(e: HWND, msg: u32, w: usize, l: isize) -> isize {
    // SAFETY: the callers' messages take integers.
    unsafe { super::send_raw(e, msg, w, l) }
}

/// Selects characters `start` up to `end` (-1: to the end; `start` -1:
/// nothing).
pub fn set_sel(e: HWND, start: i32, end: i32) {
    send_int(e, EM_SETSEL, start as isize as usize, end as isize);
}

/// Selects all of the text.
pub fn select_all(e: HWND) {
    set_sel(e, 0, -1);
}

/// The selection: (start, end).
pub fn sel(e: HWND) -> (u32, u32) {
    let (mut start, mut end) = (0u32, 0u32);
    // SAFETY: EM_GETSEL's wParam and lParam are u32 outs.
    unsafe {
        super::send_raw(
            e,
            EM_GETSEL,
            (&raw mut start) as usize,
            (&raw mut end) as isize,
        )
    };
    (start, end)
}

/// Replaces the selection with `text`.
pub fn replace_sel(e: HWND, text: &str, can_undo: bool) {
    let z = super::wz(text);
    // SAFETY: a NUL-terminated string read during the call.
    unsafe { super::send_raw(e, EM_REPLACESEL, usize::from(can_undo), z.as_ptr() as isize) };
}

/// Replaces the selection with `text` as UTF-16 (up to a NUL, if it has
/// one).
pub fn replace_sel_wide(e: HWND, text: &[u16], can_undo: bool) {
    let z = super::nul_ended(text);
    // SAFETY: a NUL-terminated string read during the call.
    unsafe { super::send_raw(e, EM_REPLACESEL, usize::from(can_undo), z.as_ptr() as isize) };
}

/// Posts EM_SETSEL to `e` (characters `start` up to `end`), to be done
/// once what runs now has returned; false if it could not be posted. Its
/// parameters are integers, and below WM_USER it means the same to every
/// window.
pub fn post_set_sel(e: HWND, start: i32, end: i32) -> bool {
    // SAFETY: EM_SETSEL's parameters are integers.
    unsafe { PostMessageW(e, EM_SETSEL, start as isize as usize, end as isize) != 0 }
}

/// The grey text shown while the box is empty.
pub fn set_cue_banner(e: HWND, text: &str, while_focused: bool) {
    let z = super::wz(text);
    // SAFETY: a NUL-terminated string read during the call, to an edit box
    // (EM_SETCUEBANNER is ECM_FIRST + 1, the class's own).
    unsafe {
        super::send_raw(
            super::of_class(e, CLASS),
            EM_SETCUEBANNER,
            usize::from(while_focused),
            z.as_ptr() as isize,
        )
    };
}

/// Undoes the last change; false if there was none.
pub fn undo(e: HWND) -> bool {
    send_int(e, EM_UNDO, 0, 0) != 0
}

/// Whether there is a change to undo.
pub fn can_undo(e: HWND) -> bool {
    send_int(e, EM_CANUNDO, 0, 0) != 0
}

/// The most UTF-16 units the box takes.
pub fn set_limit(e: HWND, units: usize) {
    send_int(e, EM_SETLIMITTEXT, units, 0);
}

/// EM_CHARFROMPOS: (character index, line) nearest `pt` (client
/// coordinates), of an edit box (0, 0 for another window). Sent only to an
/// Edit: a Rich Edit takes EM_CHARFROMPOS's lParam as a POINTL's address.
pub fn char_from_pos(e: HWND, pt: POINT) -> (u16, u16) {
    let v = send_int(
        super::of_class(e, CLASS),
        EM_CHARFROMPOS,
        0,
        super::make_lparam(pt.x as u16, pt.y as u16),
    ) as usize;
    (super::loword(v), super::hiword(v))
}

/// The margins: (left, right).
pub fn margins(e: HWND) -> (u16, u16) {
    let v = send_int(e, EM_GETMARGINS, 0, 0) as usize;
    (super::loword(v), super::hiword(v))
}

/// Sets the margins `which` names (EC_LEFTMARGIN | EC_RIGHTMARGIN).
pub fn set_margins(e: HWND, which: u32, left: u16, right: u16) {
    send_int(
        e,
        EM_SETMARGINS,
        which as usize,
        super::make_lparam(left, right),
    );
}

/// Sets the formatting rectangle without redrawing (EM_SETRECTNP).
pub fn set_rect_np(e: HWND, rc: &RECT) {
    // SAFETY: EM_SETRECTNP's lParam is a RECT, read.
    unsafe { super::send_raw(e, EM_SETRECTNP, 0, core::ptr::from_ref(rc) as isize) };
}

/// The tab stops of a multiline box, in dialog template units.
pub fn set_tab_stops(e: HWND, stops: &[u32]) {
    let ptr = if stops.is_empty() {
        0
    } else {
        stops.as_ptr() as isize
    };
    // SAFETY: EM_SETTABSTOPS's lParam is an array of wParam u32s (none for
    // none: the defaults).
    unsafe { super::send_raw(e, EM_SETTABSTOPS, stops.len(), ptr) };
}
