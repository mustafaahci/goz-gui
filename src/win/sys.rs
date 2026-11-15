//! The system: the keyboard and mouse state, metrics and DPI, monitors,
//! sounds, the last error.

use windows_sys::Win32::Foundation::{GetLastError, POINT, RECT};
use windows_sys::Win32::Graphics::Gdi::{
    GetMonitorInfoW, HMONITOR, MONITOR_FROM_FLAGS, MONITORINFO, MonitorFromPoint, MonitorFromRect,
    MonitorFromWindow,
};
use windows_sys::Win32::System::Diagnostics::Debug::MessageBeep;
use windows_sys::Win32::System::SystemInformation::GetTickCount64;
use windows_sys::Win32::System::Threading::{GetCurrentProcessId, GetCurrentThreadId};
use windows_sys::Win32::UI::HiDpi::{
    GetDpiForSystem, GetDpiForWindow, GetSystemMetricsForDpi, SystemParametersInfoForDpi,
};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, GetDoubleClickTime, GetKeyState,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    GetCursorPos, GetMessagePos, GetSystemMetrics, HCURSOR, LoadCursorW, MESSAGEBOX_STYLE,
    NONCLIENTMETRICSW, SPI_GETNONCLIENTMETRICS, SYSTEM_METRICS_INDEX, SetCursor,
};

use windows_sys::Win32::Foundation::HWND;

/// Whether key `vk` is down, as this thread's messages so far have it.
pub fn key_down(vk: u16) -> bool {
    // SAFETY: no pointers.
    unsafe { GetKeyState(i32::from(vk)) < 0 }
}

/// Whether toggle key `vk` (Caps Lock…) is on.
pub fn key_toggled(vk: u16) -> bool {
    // SAFETY: no pointers.
    unsafe { GetKeyState(i32::from(vk)) & 1 != 0 }
}

/// Whether key `vk` is down now, whatever the messages say.
pub fn key_down_now(vk: u16) -> bool {
    // SAFETY: no pointers.
    unsafe { GetAsyncKeyState(i32::from(vk)) < 0 }
}

/// A key's name as the keyboard layout gives it ("Ctrl", "F5"), by its
/// virtual key; `extended` for the keys of the extended set.
pub fn key_name(vk: u16, extended: bool) -> String {
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
        GetKeyNameTextW, MAPVK_VK_TO_VSC, MapVirtualKeyW,
    };
    // SAFETY: no pointers.
    let scan = unsafe { MapVirtualKeyW(u32::from(vk), MAPVK_VK_TO_VSC) };
    let l = (scan << 16) | if extended { 1 << 24 } else { 0 };
    let mut buf = [0u16; 64];
    // SAFETY: the buffer and its length.
    let n = unsafe { GetKeyNameTextW(l as i32, buf.as_mut_ptr(), buf.len() as i32) };
    String::from_utf16_lossy(&buf[..usize::try_from(n).unwrap_or(0).min(buf.len())])
}

/// Moves this thread's caret.
pub fn set_caret_pos(x: i32, y: i32) {
    // SAFETY: no pointers.
    unsafe { windows_sys::Win32::UI::WindowsAndMessaging::SetCaretPos(x, y) };
}

/// Plays the sound of a message box of `kind` (MB_OK, MB_ICONWARNING…).
pub fn beep(kind: MESSAGEBOX_STYLE) {
    // SAFETY: no pointers.
    unsafe { MessageBeep(kind) };
}

/// The mouse pointer on the screen.
pub fn cursor_pos() -> POINT {
    let mut pt = POINT::default();
    // SAFETY: an out-struct.
    unsafe { GetCursorPos(&mut pt) };
    pt
}

/// Where the pointer was at the current message.
pub fn message_pos() -> POINT {
    // SAFETY: no pointers.
    let v = unsafe { GetMessagePos() } as usize;
    POINT {
        x: super::loword(v) as i16 as i32,
        y: super::hiword(v) as i16 as i32,
    }
}

/// One of the system's cursors (IDC_ARROW…); null for anything else.
pub fn system_cursor(id: windows_sys::core::PCWSTR) -> HCURSOR {
    // A system cursor's id travels as the pointer's address
    // (MAKEINTRESOURCE), below 0x10000; anything else would be read as a
    // name.
    if id as usize >= 0x10000 {
        return core::ptr::null_mut();
    }
    // SAFETY: an id, not a string (above), of the system's cursors.
    unsafe { LoadCursorW(core::ptr::null_mut(), id) }
}

/// Sets the pointer's shape; the one before.
pub fn set_cursor(c: HCURSOR) -> HCURSOR {
    // SAFETY: no pointers.
    unsafe { SetCursor(c) }
}

pub fn metric(index: SYSTEM_METRICS_INDEX) -> i32 {
    // SAFETY: no pointers.
    unsafe { GetSystemMetrics(index) }
}

pub fn metric_for_dpi(index: SYSTEM_METRICS_INDEX, dpi: u32) -> i32 {
    // SAFETY: no pointers.
    unsafe { GetSystemMetricsForDpi(index, dpi) }
}

/// The window's DPI (96 at 100 %); 96 for a window that is not there.
pub fn dpi_for_window(hwnd: HWND) -> u32 {
    // SAFETY: no pointers.
    match unsafe { GetDpiForWindow(hwnd) } {
        0 => 96,
        dpi => dpi,
    }
}

pub fn system_dpi() -> u32 {
    // SAFETY: no pointers.
    unsafe { GetDpiForSystem() }
}

pub fn double_click_ms() -> u32 {
    // SAFETY: no pointers.
    unsafe { GetDoubleClickTime() }
}

pub fn tick_ms() -> u64 {
    // SAFETY: no pointers.
    unsafe { GetTickCount64() }
}

pub fn last_error() -> u32 {
    // SAFETY: no pointers.
    unsafe { GetLastError() }
}

pub fn process_id() -> u32 {
    // SAFETY: no pointers.
    unsafe { GetCurrentProcessId() }
}

pub fn thread_id() -> u32 {
    // SAFETY: no pointers.
    unsafe { GetCurrentThreadId() }
}

/// The non-client metrics (fonts of captions, menus, messages) at `dpi`.
pub fn non_client_metrics(dpi: u32) -> Option<NONCLIENTMETRICSW> {
    let mut ncm = NONCLIENTMETRICSW {
        cbSize: size_of::<NONCLIENTMETRICSW>() as u32,
        ..Default::default()
    };
    // SAFETY: an out-struct and its size.
    let ok = unsafe {
        SystemParametersInfoForDpi(
            SPI_GETNONCLIENTMETRICS,
            ncm.cbSize,
            (&raw mut ncm).cast(),
            0,
            dpi,
        )
    };
    (ok != 0).then_some(ncm)
}

/// Registers the common control classes `icc` names (ICC_*).
pub fn init_common_controls(icc: u32) -> bool {
    let init = windows_sys::Win32::UI::Controls::INITCOMMONCONTROLSEX {
        dwSize: size_of::<windows_sys::Win32::UI::Controls::INITCOMMONCONTROLSEX>() as u32,
        dwICC: icc,
    };
    // SAFETY: an in-struct with its size.
    unsafe { windows_sys::Win32::UI::Controls::InitCommonControlsEx(&init) != 0 }
}

/// The monitor a rectangle is mostly on (as `flags` say for none).
pub fn monitor_from_rect(rc: &RECT, flags: MONITOR_FROM_FLAGS) -> HMONITOR {
    // SAFETY: an in-struct.
    unsafe { MonitorFromRect(rc, flags) }
}

pub fn monitor_from_point(pt: POINT, flags: MONITOR_FROM_FLAGS) -> HMONITOR {
    // SAFETY: no pointers.
    unsafe { MonitorFromPoint(pt, flags) }
}

pub fn monitor_from_window(hwnd: HWND, flags: MONITOR_FROM_FLAGS) -> HMONITOR {
    // SAFETY: no pointers.
    unsafe { MonitorFromWindow(hwnd, flags) }
}

/// A monitor's rectangle and work area.
pub fn monitor_info(mon: HMONITOR) -> Option<MONITORINFO> {
    let mut mi = MONITORINFO {
        cbSize: size_of::<MONITORINFO>() as u32,
        ..Default::default()
    };
    // SAFETY: an out-struct with its size.
    (unsafe { GetMonitorInfoW(mon, &mut mi) } != 0).then_some(mi)
}
