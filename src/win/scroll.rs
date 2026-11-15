//! Scroll bars, and how far the wheel scrolls.

use windows_sys::Win32::Foundation::HWND;
use windows_sys::Win32::UI::Controls::SetScrollInfo;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    GetScrollInfo, SCROLLBAR_CONSTANTS, SCROLLINFO, SCROLLINFO_MASK, SPI_GETWHEELSCROLLLINES,
    SystemParametersInfoW,
};

/// Sets the range, page or position of a window's scroll bar (`bar`:
/// SB_VERT, SB_HORZ, or SB_CTL for a scroll bar control) as `info`
/// says (its fMask; its size is set here), redrawing it if `redraw`;
/// the position now.
pub fn set_info(hwnd: HWND, bar: SCROLLBAR_CONSTANTS, info: &SCROLLINFO, redraw: bool) -> i32 {
    let si = SCROLLINFO {
        cbSize: size_of::<SCROLLINFO>() as u32,
        ..*info
    };
    // SAFETY: an in-struct with its size.
    unsafe { SetScrollInfo(hwnd, bar, &si, redraw.into()) }
}

/// What `mask` (SIF_*) asks of a window's scroll bar; `None` if it has
/// none.
pub fn info(hwnd: HWND, bar: SCROLLBAR_CONSTANTS, mask: SCROLLINFO_MASK) -> Option<SCROLLINFO> {
    let mut si = SCROLLINFO {
        cbSize: size_of::<SCROLLINFO>() as u32,
        fMask: mask,
        ..Default::default()
    };
    // SAFETY: an out-struct with its size.
    (unsafe { GetScrollInfo(hwnd, bar, &mut si) } != 0).then_some(si)
}

/// The lines a notch of the mouse wheel scrolls, as the user set it.
pub fn wheel_lines() -> Option<u32> {
    let mut lines = 0u32;
    // SAFETY: SPI_GETWHEELSCROLLLINES writes one u32, to `lines`.
    let ok =
        unsafe { SystemParametersInfoW(SPI_GETWHEELSCROLLLINES, 0, (&raw mut lines).cast(), 0) }
            != 0;
    ok.then_some(lines)
}
