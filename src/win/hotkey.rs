//! Hotkey-control messages (msctls_hotkey32), sent only to one.

use windows_sys::Win32::Foundation::HWND;
use windows_sys::Win32::UI::Controls::{HKM_GETHOTKEY, HKM_SETHOTKEY, HKM_SETRULES};

/// The class these messages are for.
pub const CLASS: &str = "msctls_hotkey32";

fn send_int(h: HWND, msg: u32, w: usize, l: isize) -> isize {
    // SAFETY: the hotkey control's messages here take integers; to a
    // hotkey control (or to null).
    unsafe { super::send_raw(super::of_class(h, CLASS), msg, w, l) }
}

/// The key the box holds: the virtual key low, HOTKEYF_* high.
pub fn get(h: HWND) -> u16 {
    send_int(h, HKM_GETHOTKEY, 0, 0) as u16
}

/// Shows key `key` (the virtual key in the low byte, HOTKEYF_* above).
pub fn set(h: HWND, key: u16) {
    send_int(h, HKM_SETHOTKEY, usize::from(key), 0);
}

/// The combinations the box refuses (HKCOMB_*), and the modifiers it uses
/// instead.
pub fn set_rules(h: HWND, invalid: u16, instead: u16) {
    send_int(h, HKM_SETRULES, usize::from(invalid), instead as isize);
}
