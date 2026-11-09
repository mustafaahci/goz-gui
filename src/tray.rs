//! The notification-area (tray) icon: shown while `show_tray_icon` is on,
//! put back when Explorer restarts, and its click and menu.

use windows_sys::Win32::Foundation::HWND;
use windows_sys::Win32::UI::Shell::{
    NIF_ICON, NIF_MESSAGE, NIF_SHOWTIP, NIF_TIP, NIM_ADD, NIM_DELETE, NIM_SETVERSION,
    NOTIFYICON_VERSION_4, NOTIFYICONDATAW, Shell_NotifyIconW,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    HICON, IDI_APPLICATION, LoadIconW, RegisterWindowMessageW,
};

use crate::wide::wz;

/// The icon's id among goz-gui's (it has one).
const ID: u32 = 1;

/// The message Explorer broadcasts when it (re)starts: tray icons must be
/// added again.
pub fn taskbar_created() -> u32 {
    let name = wz("TaskbarCreated");
    // SAFETY: a NUL-terminated name.
    unsafe { RegisterWindowMessageW(name.as_ptr()) }
}

fn data(hwnd: HWND) -> NOTIFYICONDATAW {
    NOTIFYICONDATAW {
        cbSize: size_of::<NOTIFYICONDATAW>() as u32,
        hWnd: hwnd,
        uID: ID,
        // SAFETY: the rest of NOTIFYICONDATAW is plain data.
        ..unsafe { core::mem::zeroed() }
    }
}

/// Adds the icon for `hwnd`; its clicks come as `msg` (version 4: lParam's
/// low word is the event, wParam the anchor point). `icon` null takes
/// Windows' default application icon.
pub fn add(hwnd: HWND, msg: u32, icon: HICON, tip: &str) -> bool {
    let mut d = data(hwnd);
    d.uFlags = NIF_MESSAGE | NIF_ICON | NIF_TIP | NIF_SHOWTIP;
    d.uCallbackMessage = msg;
    d.hIcon = if icon.is_null() {
        // SAFETY: a system icon, shared, never destroyed.
        unsafe { LoadIconW(core::ptr::null_mut(), IDI_APPLICATION) }
    } else {
        icon
    };
    let tip: Vec<u16> = tip.encode_utf16().take(d.szTip.len() - 1).collect();
    d.szTip[..tip.len()].copy_from_slice(&tip);
    d.Anonymous.uVersion = NOTIFYICON_VERSION_4;
    // SAFETY: a filled NOTIFYICONDATAW for this process's window.
    unsafe { Shell_NotifyIconW(NIM_ADD, &d) != 0 && Shell_NotifyIconW(NIM_SETVERSION, &d) != 0 }
}

pub fn remove(hwnd: HWND) {
    let d = data(hwnd);
    // SAFETY: names the icon `add` made; deleting a missing one is harmless.
    unsafe { Shell_NotifyIconW(NIM_DELETE, &d) };
}
