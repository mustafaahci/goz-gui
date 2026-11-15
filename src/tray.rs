//! The notification-area (tray) icon: shown while `show_tray_icon` is on,
//! put back when Explorer restarts, and its click and menu.

use crate::win;
use windows_sys::Win32::Foundation::HWND;
use windows_sys::Win32::UI::Shell::{
    NIF_ICON, NIF_MESSAGE, NIF_SHOWTIP, NIF_TIP, NIM_ADD, NIM_DELETE, NIM_SETVERSION,
    NOTIFYICON_VERSION_4, NOTIFYICONDATAW,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{HICON, IDI_APPLICATION};

/// The icon's id among goz-gui's (it has one).
const ID: u32 = 1;

/// The message Explorer broadcasts when it (re)starts: tray icons must be
/// added again.
pub fn taskbar_created() -> u32 {
    win::register_message("TaskbarCreated")
}

fn data(hwnd: HWND) -> NOTIFYICONDATAW {
    NOTIFYICONDATAW {
        cbSize: size_of::<NOTIFYICONDATAW>() as u32,
        hWnd: hwnd,
        uID: ID,
        ..Default::default()
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
        // A system icon, shared, never destroyed.
        win::icon::shared(IDI_APPLICATION as usize)
    } else {
        icon
    };
    let tip: Vec<u16> = tip.encode_utf16().take(d.szTip.len() - 1).collect();
    d.szTip[..tip.len()].copy_from_slice(&tip);
    d.Anonymous.uVersion = NOTIFYICON_VERSION_4;
    win::shell::notify_icon(NIM_ADD, &d) && win::shell::notify_icon(NIM_SETVERSION, &d)
}

pub fn remove(hwnd: HWND) {
    // Names the icon `add` made; deleting a missing one is harmless.
    win::shell::notify_icon(NIM_DELETE, &data(hwnd));
}
