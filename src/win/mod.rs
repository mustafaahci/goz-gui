//! The Win32 boundary: every `unsafe` in goz-gui is in this module.
//!
//! The rest of the crate is `#![deny(unsafe_code)]` (see `main.rs`) and
//! reaches Windows through the safe functions here. Each one is sound for
//! any arguments safe code can give it, given the trust model below; where
//! that would not hold (a message whose parameters are pointers, a raw
//! `lParam`), the API is typed instead.
//!
//! # Trust model
//!
//! - **The system behaves as documented.** user32, gdi32, comctl32, shell32
//!   and ole32 check the handles they are given: a stale or wrong `HWND`,
//!   `HDC`, `HMENU` or GDI object is reported as a failure, never followed
//!   as a pointer. Handles are therefore plain values here, `Copy`, and any
//!   value is a valid argument.
//! - **A message's parameters mean what its documentation says.** A message
//!   below `WM_USER` means the same to every window class, so a wrapper for
//!   one may be sent to any window. A message from `WM_USER` up is defined
//!   by its control class (`LVM_FIRST` and `DTM_FIRST` are both `0x1000`), so
//!   the wrappers for those check the window's class first and do nothing
//!   for a window of another class.
//! - **What the system delivers is what it documents.** A procedure's
//!   `lParam` for `WM_NOTIFY`, `WM_DRAWITEM` and the like points at the
//!   structure the message defines. Only a [`Msg`] the system delivered
//!   hands those out; one made by goz-gui's own code does not.
//! - **goz-gui's private messages (`WM_APP` and up) carry no pointers.** A
//!   value that has to travel with one goes as a [`parcel`].
//! - **In-process COM servers keep their interfaces' contracts.** The calls
//!   here load third-party code into the process: shell extensions
//!   (context menu, icon and thumbnail handlers), WIC's codecs, namespace
//!   extensions. Like the system, they write no further than the buffers
//!   they are given, and a server's DllCanUnloadNow says it can go only
//!   when nothing of it is in use or running. Code in the process could
//!   break any promise of Rust's anyway; this is the narrow part of that
//!   trust the layer relies on, stated because it is about code that is not
//!   the system's. The second half is relied on by one call alone,
//!   [`com::Apartment::end_unloading_unused`], which goz-gui makes only as
//!   the preview's thread ends, its objects released; never on the UI
//!   thread, where a context menu handler is known to leave a thread
//!   behind (see `crate::shell::ContextMenu`).
//!
//! The procedures goz-gui gives Windows (window, dialog and subclass
//! procedures) are the trampolines here; they hand each message to a safe
//! Rust closure as a [`Msg`]. The closure is kept alive for as long as a
//! call to it runs, so a window that is destroyed by its own handler does
//! not free what that handler is using.

#![allow(unsafe_code)]
#![warn(clippy::undocumented_unsafe_blocks)]

pub mod a11y;
pub mod button;
pub mod class;
pub mod clipboard;
pub mod com;
pub mod combo;
pub mod datetime;
pub mod dib;
pub mod dlg;
pub mod dropfiles;
pub mod edit;
pub mod file;
pub mod gdi;
pub mod header;
pub mod hook;
pub mod hotkey;
pub mod icon;
pub mod imagelist;
pub mod listbox;
pub mod locale;
pub mod lv;
pub mod menu;
pub mod menubar;
pub mod msg;
pub mod parcel;
pub mod preview;
pub mod process;
pub mod queue;
pub mod reg;
pub mod resource;
pub mod scroll;
pub mod shell;
pub mod subclass;
pub mod sync;
pub mod sys;
pub mod tab;
pub mod time;
pub mod tip;
pub mod trackbar;
pub mod tv;
pub mod uxtheme;
pub mod wic;
pub mod wnd;

#[cfg(test)]
mod trampolines_tests;

pub use msg::{Msg, Notify, View};
pub use wnd::*;

use std::borrow::Cow;

use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows_sys::Win32::UI::WindowsAndMessaging::{GetClassNameW, SendMessageW};

/// Sends any message. Only the wrappers in this module call it, each for a
/// message whose parameters it has typed.
///
/// # Safety
/// `w` and `l` are what message `msg` defines for `hwnd`'s class: any
/// pointer among them is valid for the access the message makes, for the
/// length of the call.
pub(crate) unsafe fn send_raw(hwnd: HWND, msg: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    // SAFETY: as the caller promises.
    unsafe { SendMessageW(hwnd, msg, w, l) }
}

/// Whether `hwnd`'s window class is `name`, which window classes compare
/// as: ignoring ASCII case. False for a window that is not there.
pub fn class_is(hwnd: HWND, name: &str) -> bool {
    let mut buf = [0u16; 64];
    // SAFETY: the buffer and its length in code units.
    let n = unsafe { GetClassNameW(hwnd, buf.as_mut_ptr(), buf.len() as i32) };
    let Ok(n) = usize::try_from(n) else {
        return false;
    };
    let got = &buf[..n.min(buf.len())];
    let mut want = name.encode_utf16();
    for &unit in got {
        match want.next() {
            Some(w) if fold(w) == fold(unit) => {}
            _ => return false,
        }
    }
    want.next().is_none()
}

/// Whether `hwnd` is a window of the calling thread (false for one that is
/// not there).
pub(crate) fn is_this_thread(hwnd: HWND) -> bool {
    use windows_sys::Win32::UI::WindowsAndMessaging::GetWindowThreadProcessId;
    // SAFETY: no pointers (no process id asked for).
    let thread = unsafe { GetWindowThreadProcessId(hwnd, core::ptr::null_mut()) };
    thread != 0 && thread == sys::thread_id()
}

/// A pointer to the units of `s` for a call that takes a pointer and a
/// length: a real address even when `s` is empty. An empty slice's own
/// pointer is dangling (fine for Rust, as nothing is read), but parts of
/// Windows read the first unit whatever the length (DrawTextW does).
pub(crate) fn units_ptr(s: &[u16]) -> *const u16 {
    static EMPTY: [u16; 1] = [0];
    if s.is_empty() {
        EMPTY.as_ptr()
    } else {
        s.as_ptr()
    }
}

/// [`units_ptr`] for bytes.
pub(crate) fn bytes_ptr(s: &[u8]) -> *const u8 {
    static EMPTY: [u8; 1] = [0];
    if s.is_empty() {
        EMPTY.as_ptr()
    } else {
        s.as_ptr()
    }
}

/// `hwnd`'s window class's name (empty for a window that is not there).
pub fn class_name(hwnd: HWND) -> String {
    let mut buf = [0u16; 256];
    // SAFETY: the buffer and its length in code units.
    let n = unsafe { GetClassNameW(hwnd, buf.as_mut_ptr(), buf.len() as i32) };
    String::from_utf16_lossy(&buf[..usize::try_from(n).unwrap_or(0).min(buf.len())])
}

/// Whether `a` and `b` name the same window class: equal ignoring case
/// as the system's atoms compare names (its own upper-case table,
/// CompareStringOrdinal's), not only ASCII's.
pub(crate) fn same_class_name(a: &str, b: &str) -> bool {
    let a: Vec<u16> = a.encode_utf16().collect();
    let b: Vec<u16> = b.encode_utf16().collect();
    locale::compare_ordinal(&a, &b, true) == windows_sys::Win32::Globalization::CSTR_EQUAL
}

fn fold(unit: u16) -> u16 {
    if (u16::from(b'A')..=u16::from(b'Z')).contains(&unit) {
        unit + 32
    } else {
        unit
    }
}

/// `hwnd` if its class is `name`, else null: a message sent to null does
/// nothing and answers 0, as a message to a window that has gone does.
pub(crate) fn of_class(hwnd: HWND, name: &str) -> HWND {
    if class_is(hwnd, name) {
        hwnd
    } else {
        debug_assert!(
            hwnd.is_null() || !wnd::is_window(hwnd),
            "a {name} message for a window of another class"
        );
        core::ptr::null_mut()
    }
}

/// A NUL-terminated UTF-16 copy of `s`.
pub fn wz(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}

/// `s` with a NUL appended, for a call that wants a C string.
pub fn wz_units(s: &[u16]) -> Vec<u16> {
    let mut v = Vec::with_capacity(s.len() + 1);
    v.extend_from_slice(s);
    v.push(0);
    v
}

/// `units` NUL-terminated, for a call that wants a C string: as they are
/// when they end with a NUL, else a copy with one.
pub(crate) fn nul_ended(units: &[u16]) -> Cow<'_, [u16]> {
    if units.last() == Some(&0) {
        Cow::Borrowed(units)
    } else {
        Cow::Owned(wz_units(units))
    }
}

/// A slice's length as the `i32` count a call takes, cut to fit (the call
/// then reads or writes less of the slice, never more).
pub(crate) fn len_i32(len: usize) -> i32 {
    i32::try_from(len).unwrap_or(i32::MAX)
}

/// A slice's length as the `u32` count a call takes, cut to fit.
pub(crate) fn len_u32(len: usize) -> u32 {
    u32::try_from(len).unwrap_or(u32::MAX)
}

/// `units` as the bytes they are in memory (native order), for a byte
/// search over UTF-16 text.
pub fn units_as_bytes(units: &[u16]) -> &[u8] {
    // SAFETY: u16 has no padding and u8's alignment is 1; the bytes are
    // the slice's own, borrowed for as long as it is.
    unsafe { core::slice::from_raw_parts(units.as_ptr().cast::<u8>(), size_of_val(units)) }
}

/// The UTF-16 units before the first NUL of `buf` (all of it without one).
pub fn until_nul(buf: &[u16]) -> &[u16] {
    let end = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    &buf[..end]
}

/// The low word of a message parameter.
pub fn loword(v: usize) -> u16 {
    (v & 0xffff) as u16
}

/// The high word of a message parameter.
pub fn hiword(v: usize) -> u16 {
    ((v >> 16) & 0xffff) as u16
}

/// `lo` and `hi` as one `LPARAM`, as MAKELPARAM makes it.
pub fn make_lparam(lo: u16, hi: u16) -> LPARAM {
    (u32::from(lo) | (u32::from(hi) << 16)) as i32 as LPARAM
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn words_go_in_and_come_out() {
        assert_eq!(loword(make_lparam(3, 7) as usize), 3);
        assert_eq!(hiword(make_lparam(3, 7) as usize), 7);
        assert_eq!(make_lparam(0xffff, 0xffff), -1);
    }

    #[test]
    fn units_are_their_bytes() {
        assert_eq!(units_as_bytes(&[0x0102, 0x0304]), [2, 1, 4, 3]);
        assert!(units_as_bytes(&[]).is_empty());
        assert_eq!(&*nul_ended(&[104, 0]), &[104, 0]);
        assert_eq!(&*nul_ended(&[104]), &[104, 0]);
    }

    #[test]
    fn a_string_ends_at_its_nul() {
        assert_eq!(until_nul(&[104, 105, 0, 33]), &[104, 105]);
        assert_eq!(until_nul(&[104, 105]), &[104, 105]);
        assert_eq!(wz("hi"), vec![104, 105, 0]);
    }
}
