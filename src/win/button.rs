//! Button messages (BM_* are below WM_USER: the same for every window).

use windows_sys::Win32::Foundation::{HWND, SIZE};
use windows_sys::Win32::UI::Controls::{BCM_GETIDEALSIZE, BCM_SETSHIELD};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    BM_CLICK, BM_GETCHECK, BM_GETSTATE, BM_SETCHECK, BM_SETSTATE, BM_SETSTYLE,
};

fn send_int(b: HWND, msg: u32, w: usize, l: isize) -> isize {
    // SAFETY: the callers' messages take integers.
    unsafe { super::send_raw(b, msg, w, l) }
}

/// BST_CHECKED, BST_UNCHECKED or BST_INDETERMINATE.
pub fn check(b: HWND) -> u32 {
    send_int(b, BM_GETCHECK, 0, 0) as u32
}

/// Whether the check box is checked (BST_CHECKED).
pub fn is_checked(b: HWND) -> bool {
    check(b) == 1
}

/// Sets the check state (BST_*).
pub fn set_check(b: HWND, state: u32) {
    send_int(b, BM_SETCHECK, state as usize, 0);
}

/// Checks or unchecks the check box.
pub fn set_checked(b: HWND, on: bool) {
    set_check(b, u32::from(on));
}

/// The button's BST_* state (pushed, focused, checked).
pub fn state(b: HWND) -> u32 {
    send_int(b, BM_GETSTATE, 0, 0) as u32
}

/// Shows the button pushed or not.
pub fn set_state(b: HWND, pushed: bool) {
    send_int(b, BM_SETSTATE, usize::from(pushed), 0);
}

/// Sets the button's BS_* style, redrawing it if `redraw`.
pub fn set_style(b: HWND, style: u32, redraw: bool) {
    send_int(b, BM_SETSTYLE, style as usize, isize::from(redraw));
}

/// Clicks the button, as the user would.
pub fn click(b: HWND) {
    send_int(b, BM_CLICK, 0, 0);
}

/// BCM_GETIDEALSIZE: the size that fits the button's text (a button's own
/// message).
pub fn ideal_size(b: HWND) -> SIZE {
    let mut size = SIZE::default();
    // SAFETY: BCM_GETIDEALSIZE's lParam is a SIZE in-out, to a button.
    unsafe {
        super::send_raw(
            super::of_class(b, "Button"),
            BCM_GETIDEALSIZE,
            0,
            (&raw mut size) as isize,
        )
    };
    size
}

/// Shows the UAC shield on the button (a button's own message).
pub fn set_shield(b: HWND, on: bool) {
    // SAFETY: a flag, to a button.
    unsafe {
        super::send_raw(
            super::of_class(b, "Button"),
            BCM_SETSHIELD,
            0,
            isize::from(on),
        )
    };
}
