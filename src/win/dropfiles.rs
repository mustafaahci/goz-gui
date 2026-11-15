//! Files dropped on a window (WM_DROPFILES): their paths, read only while
//! the drop is being delivered, and the drop freed exactly once after.
//!
//! The trampolines mark a WM_DROPFILES's HDROP as in flight for as long as
//! a procedure of goz-gui's handles it (a subclass passing it on to the
//! window's own procedure nests: the depth counts both). When the outermost
//! of them returns, the drop is finished (`DragFinish`). [`Msg::dropped_files`]
//! reads it only while it is in flight, so never after it is freed, and no
//! handler frees it itself. `Msg::forward` does not pass a WM_DROPFILES on,
//! so no window outside goz-gui's procedures frees it either (the system's
//! controls do not handle WM_DROPFILES).
//!
//! [`Msg::dropped_files`]: super::Msg::dropped_files

use std::cell::RefCell;

use windows_sys::Win32::Foundation::{POINT, WPARAM};
use windows_sys::Win32::UI::Shell::{DragFinish, DragQueryFileW, DragQueryPoint, HDROP};
use windows_sys::Win32::UI::WindowsAndMessaging::WM_DROPFILES;

thread_local! {
    /// The drops being delivered on this thread: the HDROP and how many
    /// procedures deep.
    static IN_FLIGHT: RefCell<Vec<(usize, u32)>> = const { RefCell::new(Vec::new()) };
}

/// A procedure's hold on the drop it is handling; the last to let go
/// finishes the drop.
pub(crate) struct Delivery(Option<usize>);

impl Delivery {
    /// Called by a trampoline as it hands message `msg` (wParam `w`) to its
    /// handler.
    pub(crate) fn enter(msg: u32, w: WPARAM) -> Delivery {
        if msg != WM_DROPFILES || w == 0 {
            return Delivery(None);
        }
        IN_FLIGHT.with_borrow_mut(|f| match f.iter_mut().find(|(h, _)| *h == w) {
            Some((_, depth)) => *depth += 1,
            None => f.push((w, 1)),
        });
        Delivery(Some(w))
    }
}

impl Drop for Delivery {
    fn drop(&mut self) {
        let Some(h) = self.0 else {
            return;
        };
        let last = IN_FLIGHT
            .try_with(|f| {
                let mut f = f.borrow_mut();
                let Some(i) = f.iter().position(|&(d, _)| d == h) else {
                    return false;
                };
                f[i].1 -= 1;
                if f[i].1 == 0 {
                    f.swap_remove(i);
                    true
                } else {
                    false
                }
            })
            .unwrap_or(false);
        if last {
            // SAFETY: the HDROP the system delivered with this WM_DROPFILES,
            // finished once: by the outermost procedure that handled it, as
            // it returns (no handler frees it, and none can read it after).
            unsafe { DragFinish(h as HDROP) };
        }
    }
}

fn in_flight(h: usize) -> bool {
    IN_FLIGHT.with_borrow(|f| f.iter().any(|&(d, _)| d == h))
}

/// The paths of the drop `h`, if it is being delivered on this thread.
pub(crate) fn paths(h: usize) -> Option<Vec<Vec<u16>>> {
    if !in_flight(h) {
        return None;
    }
    // SAFETY: a drop being delivered (above), not yet finished.
    Some(unsafe { read_paths(h as HDROP) })
}

/// The paths of the drop `drop` (a WM_DROPFILES's, or the clipboard's
/// CF_HDROP), copied out.
///
/// # Safety
/// `drop` is a valid HDROP, not freed, for as long as the call runs.
pub(crate) unsafe fn read_paths(drop: HDROP) -> Vec<Vec<u16>> {
    // SAFETY: as the caller promises; index 0xFFFFFFFF with no buffer asks
    // for the count.
    let count = unsafe { DragQueryFileW(drop, u32::MAX, core::ptr::null_mut(), 0) };
    let mut out = Vec::with_capacity(count as usize);
    for i in 0..count {
        // SAFETY: as above; no buffer asks for the length of path `i`.
        let len = unsafe { DragQueryFileW(drop, i, core::ptr::null_mut(), 0) } as usize;
        let mut buf = vec![0u16; len + 1];
        // SAFETY: as above; the buffer and its length in units (with the
        // NUL's room).
        let got = unsafe { DragQueryFileW(drop, i, buf.as_mut_ptr(), buf.len() as u32) } as usize;
        buf.truncate(got.min(len));
        out.push(buf);
    }
    out
}

/// Where the drop `h` landed (client coordinates), and whether in the client
/// area; `None` if it is not being delivered on this thread.
pub(crate) fn point(h: usize) -> Option<(POINT, bool)> {
    if !in_flight(h) {
        return None;
    }
    let mut pt = POINT::default();
    // SAFETY: a drop being delivered, not yet finished; an out-struct.
    let client = unsafe { DragQueryPoint(h as HDROP, &mut pt) } != 0;
    Some((pt, client))
}

/// Whether `hwnd` takes files dropped on it (WM_DROPFILES).
pub fn accept(hwnd: windows_sys::Win32::Foundation::HWND, on: bool) {
    // SAFETY: no pointers.
    unsafe { windows_sys::Win32::UI::Shell::DragAcceptFiles(hwnd, on.into()) };
}
