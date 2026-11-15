//! Small control helpers the file tools' dialogs share.

use windows_sys::Win32::Foundation::{HWND, RECT};
use windows_sys::Win32::UI::Controls::{LVCF_FMT, LVCF_WIDTH, LVCFMT_LEFT, LVCOLUMNW};

use crate::win::{self, button, dlg, lv};

pub fn item(dlg: HWND, id: u16) -> HWND {
    dlg::item(dlg, id)
}

pub fn enable(dlg: HWND, id: u16, on: bool) {
    let h = item(dlg, id);
    if !h.is_null() {
        win::enable(h, on);
    }
}

pub fn checked(dlg: HWND, id: u16) -> bool {
    button::is_checked(item(dlg, id))
}

pub fn set_check(dlg: HWND, id: u16, on: bool) {
    button::set_checked(item(dlg, id), on);
}

/// Sets a list's row state (`row` usize::MAX: every row).
pub fn set_state(list: HWND, row: usize, state: u32, mask: u32) {
    lv::set_item_state(list, row as i32, state, mask);
}

/// Adds the columns `(title, width at 96 DPI)` to a list, at its DPI.
pub fn columns(list: HWND, cols: &[(&str, i32)]) {
    let dpi = crate::custom::window_dpi(list) as i32;
    for (i, &(title, width)) in cols.iter().enumerate() {
        let col = LVCOLUMNW {
            mask: LVCF_WIDTH | LVCF_FMT,
            fmt: LVCFMT_LEFT,
            cx: width * dpi / 96,
            ..Default::default()
        };
        // The title goes in as the column's text (LVCF_TEXT).
        lv::insert_column(list, i as i32, col, Some(title));
    }
}

/// `x`, `y` dialog units in pixels for `dlg`.
pub fn units(dlg: HWND, x: i32, y: i32) -> (i32, i32) {
    let r = dlg::map_rect(
        dlg,
        RECT {
            left: 0,
            top: 0,
            right: x,
            bottom: y,
        },
    );
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
