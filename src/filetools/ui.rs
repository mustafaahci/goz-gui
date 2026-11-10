//! Small control helpers the file tools' dialogs share.

use windows_sys::Win32::Foundation::{HWND, RECT};
use windows_sys::Win32::UI::Controls::{
    BST_CHECKED, LVCF_FMT, LVCF_TEXT, LVCF_WIDTH, LVCFMT_LEFT, LVCOLUMNW, LVITEMW,
    LVM_INSERTCOLUMNW, LVM_SETITEMSTATE,
};
use windows_sys::Win32::UI::HiDpi::GetDpiForWindow;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::EnableWindow;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    BM_GETCHECK, BM_SETCHECK, GetDlgItem, MapDialogRect, SendMessageW,
};

use crate::wide::wz;

pub fn item(dlg: HWND, id: u16) -> HWND {
    // SAFETY: looks up a child of this thread's dialog.
    unsafe { GetDlgItem(dlg, i32::from(id)) }
}

pub fn send(hwnd: HWND, msg: u32, w: usize, l: isize) -> isize {
    // SAFETY: a message to a window of this thread; any pointer in `l` is
    // the caller's and lives across the synchronous call.
    unsafe { SendMessageW(hwnd, msg, w, l) }
}

pub fn enable(dlg: HWND, id: u16, on: bool) {
    let h = item(dlg, id);
    if !h.is_null() {
        // SAFETY: a control of this thread's dialog.
        unsafe { EnableWindow(h, on.into()) };
    }
}

pub fn checked(dlg: HWND, id: u16) -> bool {
    send(item(dlg, id), BM_GETCHECK, 0, 0) == BST_CHECKED as isize
}

pub fn set_check(dlg: HWND, id: u16, on: bool) {
    send(item(dlg, id), BM_SETCHECK, usize::from(on), 0);
}

/// Sets a list's row state (`row` usize::MAX: every row).
pub fn set_state(list: HWND, row: usize, state: u32, mask: u32) {
    let it = LVITEMW {
        state,
        stateMask: mask,
        // SAFETY: the rest of LVITEMW is plain data.
        ..unsafe { core::mem::zeroed() }
    };
    send(list, LVM_SETITEMSTATE, row, (&raw const it) as isize);
}

/// Adds the columns `(title, width at 96 DPI)` to a list, at its DPI.
pub fn columns(list: HWND, cols: &[(&str, i32)]) {
    // SAFETY: reads this thread's list's DPI.
    let dpi = unsafe { GetDpiForWindow(list) }.max(96) as i32;
    for (i, &(title, width)) in cols.iter().enumerate() {
        let mut text = wz(title);
        let col = LVCOLUMNW {
            mask: LVCF_TEXT | LVCF_WIDTH | LVCF_FMT,
            fmt: LVCFMT_LEFT,
            cx: width * dpi / 96,
            pszText: text.as_mut_ptr(),
            // SAFETY: the rest of LVCOLUMNW is plain data.
            ..unsafe { core::mem::zeroed() }
        };
        send(list, LVM_INSERTCOLUMNW, i, (&raw const col) as isize);
    }
}

/// `x`, `y` dialog units in pixels for `dlg`.
pub fn units(dlg: HWND, x: i32, y: i32) -> (i32, i32) {
    let mut r = RECT {
        left: 0,
        top: 0,
        right: x,
        bottom: y,
    };
    // SAFETY: converts within this thread's dialog.
    unsafe { MapDialogRect(dlg, &mut r) };
    (r.right, r.bottom)
}

/// The colour of text that says something is wrong: Windows' error red,
/// lighter on the dark theme.
pub fn error_colour(dark: bool) -> u32 {
    if dark {
        crate::theme::rgb(0xff, 0x99, 0xa4)
    } else {
        crate::theme::rgb(0xc4, 0x2b, 0x1c)
    }
}

/// Copies `text` into a list's display buffer of `cap` units (NUL
/// included).
///
/// # Safety
/// `buf` points to `cap` writable code units (`cap` > 0).
pub unsafe fn copy_text(text: &[u16], buf: *mut u16, cap: i32) {
    if buf.is_null() || cap <= 0 {
        return;
    }
    let n = text.len().min(cap as usize - 1);
    // SAFETY: as the caller promises.
    unsafe {
        core::ptr::copy_nonoverlapping(text.as_ptr(), buf, n);
        *buf.add(n) = 0;
    }
}
