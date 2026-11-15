//! Button messages (BM_* are below WM_USER: the same for every window).

use windows_sys::Win32::Foundation::HWND;
use windows_sys::Win32::UI::WindowsAndMessaging::{BM_GETCHECK, BM_SETCHECK};

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
