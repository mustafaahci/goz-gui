//! UTF-16 helpers for the Win32 boundary.

use windows_sys::Win32::Foundation::HWND;
use windows_sys::Win32::UI::WindowsAndMessaging::{GetWindowTextLengthW, GetWindowTextW};

/// `s` as a NUL-terminated UTF-16 string.
pub fn wz(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}

/// `s` with a NUL appended, for a call that wants a C string.
pub fn nul(s: &[u16]) -> Vec<u16> {
    let mut v = Vec::with_capacity(s.len() + 1);
    v.extend_from_slice(s);
    v.push(0);
    v
}

/// The text of a window, such as the search edit.
pub fn window_text(hwnd: HWND) -> String {
    // SAFETY: `hwnd` is a window of this thread; the buffer holds `len + 1`
    // code units, the count passed in.
    unsafe {
        let len = GetWindowTextLengthW(hwnd);
        if len <= 0 {
            return String::new();
        }
        let mut buf = vec![0u16; len as usize + 1];
        let n = GetWindowTextW(hwnd, buf.as_mut_ptr(), buf.len() as i32);
        String::from_utf16_lossy(&buf[..n.max(0) as usize])
    }
}

/// Whether the NUL-terminated string at `p` is `s`, such as the setting a
/// `WM_SETTINGCHANGE` names.
///
/// # Safety
/// `p` points at a NUL-terminated UTF-16 string.
pub unsafe fn is_wide(p: *const u16, s: &str) -> bool {
    let mut i = 0;
    for unit in s.encode_utf16() {
        // SAFETY: stops at the first mismatch, which a NUL always is.
        if unsafe { *p.add(i) } != unit {
            return false;
        }
        i += 1;
    }
    // SAFETY: as above; every earlier unit matched and was not NUL.
    unsafe { *p.add(i) == 0 }
}
