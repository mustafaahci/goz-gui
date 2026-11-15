//! Edit-control messages. EM_* below WM_USER mean the same to every
//! window; the cue banner (ECM_FIRST and up) is sent only to an edit box.

use windows_sys::Win32::Foundation::{HWND, POINT, RECT};
use windows_sys::Win32::UI::Controls::{
    EM_CANUNDO, EM_CHARFROMPOS, EM_EMPTYUNDOBUFFER, EM_GETFIRSTVISIBLELINE, EM_GETLINECOUNT,
    EM_GETMARGINS, EM_GETRECT, EM_GETSEL, EM_LINEINDEX, EM_LINESCROLL, EM_POSFROMCHAR,
    EM_REPLACESEL, EM_SCROLLCARET, EM_SETCUEBANNER, EM_SETLIMITTEXT, EM_SETMARGINS, EM_SETREADONLY,
    EM_SETRECTNP, EM_SETSEL, EM_SETTABSTOPS, EM_UNDO,
};

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

pub fn undo(e: HWND) -> bool {
    send_int(e, EM_UNDO, 0, 0) != 0
}

pub fn can_undo(e: HWND) -> bool {
    send_int(e, EM_CANUNDO, 0, 0) != 0
}

pub fn empty_undo(e: HWND) {
    send_int(e, EM_EMPTYUNDOBUFFER, 0, 0);
}

pub fn set_limit(e: HWND, units: usize) {
    send_int(e, EM_SETLIMITTEXT, units, 0);
}

pub fn set_read_only(e: HWND, on: bool) {
    send_int(e, EM_SETREADONLY, usize::from(on), 0);
}

pub fn scroll_caret(e: HWND) {
    send_int(e, EM_SCROLLCARET, 0, 0);
}

pub fn line_count(e: HWND) -> i32 {
    send_int(e, EM_GETLINECOUNT, 0, 0) as i32
}

pub fn first_visible_line(e: HWND) -> i32 {
    send_int(e, EM_GETFIRSTVISIBLELINE, 0, 0) as i32
}

/// The character index where line `line` starts (-1: the caret's line).
pub fn line_index(e: HWND, line: i32) -> i32 {
    send_int(e, EM_LINEINDEX, line as isize as usize, 0) as i32
}

pub fn line_scroll(e: HWND, cols: i32, lines: i32) {
    send_int(e, EM_LINESCROLL, cols as isize as usize, lines as isize);
}

/// EM_CHARFROMPOS: (character index, line) nearest `pt` (client
/// coordinates).
pub fn char_from_pos(e: HWND, pt: POINT) -> (u16, u16) {
    let v = send_int(
        e,
        EM_CHARFROMPOS,
        0,
        super::make_lparam(pt.x as u16, pt.y as u16),
    ) as usize;
    (super::loword(v), super::hiword(v))
}

/// EM_POSFROMCHAR: the client point of character `i` ((-1, -1) past the
/// end).
pub fn pos_from_char(e: HWND, i: usize) -> POINT {
    let v = send_int(e, EM_POSFROMCHAR, i, 0) as usize;
    POINT {
        x: super::loword(v) as i16 as i32,
        y: super::hiword(v) as i16 as i32,
    }
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

/// The formatting rectangle.
pub fn rect(e: HWND) -> RECT {
    let mut rc = RECT::default();
    // SAFETY: EM_GETRECT's lParam is a RECT out.
    unsafe { super::send_raw(e, EM_GETRECT, 0, (&raw mut rc) as isize) };
    rc
}

/// Sets the formatting rectangle without redrawing (EM_SETRECTNP).
pub fn set_rect_np(e: HWND, rc: &RECT) {
    // SAFETY: EM_SETRECTNP's lParam is a RECT, read.
    unsafe { super::send_raw(e, EM_SETRECTNP, 0, core::ptr::from_ref(rc) as isize) };
}

/// The tab stops of a multiline box, in dialog template units.
pub fn set_tab_stops(e: HWND, stops: &[u32]) {
    // SAFETY: EM_SETTABSTOPS's lParam is an array of wParam u32s.
    unsafe { super::send_raw(e, EM_SETTABSTOPS, stops.len(), stops.as_ptr() as isize) };
}
