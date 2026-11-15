//! Tooltip messages (tooltips_class32), each sent only to a tooltip. A
//! tool's text goes in as a [`Text`], never as a pointer.

use windows_sys::Win32::Foundation::{COLORREF, HWND, RECT};
use windows_sys::Win32::UI::Controls::{
    TTM_ACTIVATE, TTM_ADDTOOLW, TTM_ADJUSTRECT, TTM_DELTOOLW, TTM_GETDELAYTIME, TTM_NEWTOOLRECTW,
    TTM_POP, TTM_POPUP, TTM_SETDELAYTIME, TTM_SETMARGIN, TTM_SETMAXTIPWIDTH, TTM_SETTIPBKCOLOR,
    TTM_SETTIPTEXTCOLOR, TTM_SETWINDOWTHEME, TTM_TRACKACTIVATE, TTM_TRACKPOSITION,
    TTM_UPDATETIPTEXTW, TTTOOLINFOW,
};

pub const CLASS: &str = "tooltips_class32";

/// A tool's text.
pub enum Text<'a> {
    None,
    /// The tooltip copies it.
    Str(&'a str),
    /// Asked for with TTN_GETDISPINFO when the tip shows.
    Callback,
}

/// # Safety
/// `w` and `l` are what `msg` defines for a tooltip.
unsafe fn send(tip: HWND, msg: u32, w: usize, l: isize) -> isize {
    // SAFETY: as the caller promises, to a tooltip (or to null).
    unsafe { super::send_raw(super::of_class(tip, CLASS), msg, w, l) }
}

/// Sends `msg` with `ti` (its text from `text`, its size and no reserved
/// pointer) as lParam.
fn with_tool(tip: HWND, msg: u32, w: usize, ti: &TTTOOLINFOW, text: Text) -> isize {
    let mut t = *ti;
    t.cbSize = size_of::<TTTOOLINFOW>() as u32;
    t.lpReserved = core::ptr::null_mut();
    t.hinst = core::ptr::null_mut();
    let z;
    t.lpszText = match text {
        Text::None => core::ptr::null_mut(),
        Text::Str(s) => {
            z = super::wz(s);
            z.as_ptr().cast_mut()
        }
        // LPSTR_TEXTCALLBACKW: a marker, not a pointer.
        Text::Callback => core::ptr::without_provenance_mut(usize::MAX),
    };
    // SAFETY: the tooltip messages that take a TTTOOLINFOW read it (and copy
    // its text) during the call; its pointers are set above.
    unsafe { send(tip, msg, w, (&raw const t) as isize) }
}

/// Adds tool `ti` (its hwnd, uId, uFlags, rect, lParam) with `text`.
pub fn add_tool(tip: HWND, ti: &TTTOOLINFOW, text: Text) -> bool {
    with_tool(tip, TTM_ADDTOOLW, 0, ti, text) != 0
}

/// Removes the tool `ti` names (its hwnd and uId).
pub fn del_tool(tip: HWND, ti: &TTTOOLINFOW) {
    with_tool(tip, TTM_DELTOOLW, 0, ti, Text::None);
}

/// Sets the text of the tool `ti` names.
pub fn update_text(tip: HWND, ti: &TTTOOLINFOW, text: Text) {
    with_tool(tip, TTM_UPDATETIPTEXTW, 0, ti, text);
}

/// Sets the rectangle of the tool `ti` names to `ti.rect`.
pub fn new_tool_rect(tip: HWND, ti: &TTTOOLINFOW) {
    with_tool(tip, TTM_NEWTOOLRECTW, 0, ti, Text::None);
}

/// Shows or hides the tracking tool `ti` names.
pub fn track_activate(tip: HWND, on: bool, ti: &TTTOOLINFOW) {
    with_tool(tip, TTM_TRACKACTIVATE, usize::from(on), ti, Text::None);
}

/// Where a tracking tip shows (screen coordinates).
pub fn track_position(tip: HWND, x: i32, y: i32) {
    let l = super::make_lparam(x as u16, y as u16);
    // SAFETY: TTM_TRACKPOSITION's lParam is a packed point.
    unsafe { send(tip, TTM_TRACKPOSITION, 0, l) };
}

pub fn set_max_width(tip: HWND, width: i32) {
    // SAFETY: an integer.
    unsafe { send(tip, TTM_SETMAXTIPWIDTH, 0, width as isize) };
}

/// TTM_GETDELAYTIME of TTDT_* `which`, in ms.
pub fn delay_time(tip: HWND, which: u32) -> i32 {
    // SAFETY: an integer.
    unsafe { send(tip, TTM_GETDELAYTIME, which as usize, 0) as i32 }
}

pub fn set_delay_time(tip: HWND, which: u32, ms: i32) {
    // SAFETY: integers.
    unsafe { send(tip, TTM_SETDELAYTIME, which as usize, ms as isize) };
}

/// TTM_ADJUSTRECT: the window rectangle for a text rectangle `rc` (or the
/// text rectangle for a window rectangle, when not `larger`).
pub fn adjust_rect(tip: HWND, larger: bool, rc: RECT) -> RECT {
    let mut r = rc;
    // SAFETY: TTM_ADJUSTRECT's lParam is a RECT in-out.
    unsafe {
        send(
            tip,
            TTM_ADJUSTRECT,
            usize::from(larger),
            (&raw mut r) as isize,
        )
    };
    r
}

pub fn set_margin(tip: HWND, rc: RECT) {
    // SAFETY: TTM_SETMARGIN's lParam is a RECT, read.
    unsafe { send(tip, TTM_SETMARGIN, 0, (&raw const rc) as isize) };
}

pub fn activate(tip: HWND, on: bool) {
    // SAFETY: a flag.
    unsafe { send(tip, TTM_ACTIVATE, usize::from(on), 0) };
}

pub fn pop(tip: HWND) {
    // SAFETY: no parameters.
    unsafe { send(tip, TTM_POP, 0, 0) };
}

pub fn popup(tip: HWND) {
    // SAFETY: no parameters.
    unsafe { send(tip, TTM_POPUP, 0, 0) };
}

pub fn set_colors(tip: HWND, text: COLORREF, back: COLORREF) {
    // SAFETY: colours.
    unsafe {
        send(tip, TTM_SETTIPTEXTCOLOR, text as usize, 0);
        send(tip, TTM_SETTIPBKCOLOR, back as usize, 0);
    }
}

/// TTM_SETWINDOWTHEME: the visual style the tip draws with
/// ("DarkMode_Explorer").
pub fn set_window_theme(tip: HWND, theme: &str) {
    let z = super::wz(theme);
    // SAFETY: a NUL-terminated string read during the call.
    unsafe { send(tip, TTM_SETWINDOWTHEME, 0, z.as_ptr() as isize) };
}
