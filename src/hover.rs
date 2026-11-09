//! The result row under the mouse, which the list draws highlighted.
//!
//! The list's subclass reports mouse moves here; `TrackMouseEvent` reports
//! when the mouse leaves. Only the rows that gain or lose the highlight are
//! redrawn.

use windows_sys::Win32::Foundation::{POINT, RECT};
use windows_sys::Win32::Graphics::Gdi::ScreenToClient;
use windows_sys::Win32::UI::Controls::{
    LVHITTESTINFO, LVHT_ONITEMICON, LVHT_ONITEMLABEL, LVHT_ONITEMSTATEICON, LVIR_BOUNDS,
    LVM_GETITEMRECT, LVM_HITTEST, LVM_REDRAWITEMS,
};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    TME_LEAVE, TRACKMOUSEEVENT, TrackMouseEvent,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{GetCursorPos, SendMessageW, WindowFromPoint};

use crate::window::App;

fn send(app: &App, msg: u32, w: usize, l: isize) -> isize {
    // SAFETY: a message to this thread's list; any pointer in `l` is the
    // caller's and lives across the synchronous call.
    unsafe { SendMessageW(app.list, msg, w, l) }
}

/// The mouse moved to `pt`, in the list's client coordinates.
pub fn moved(app: &App, pt: POINT) {
    let mut tme = TRACKMOUSEEVENT {
        cbSize: size_of::<TRACKMOUSEEVENT>() as u32,
        dwFlags: TME_LEAVE,
        hwndTrack: app.list,
        dwHoverTime: 0,
    };
    // SAFETY: asks for WM_MOUSELEAVE on this thread's list. Asking again
    // while tracking is harmless, and keeps tracking alive should the list
    // cancel its own.
    unsafe { TrackMouseEvent(&mut tme) };
    set(app, row_at(app, pt));
}

pub fn left(app: &App) {
    set(app, None);
}

/// The list scrolled under a mouse that did not move.
pub fn scrolled(app: &App) {
    let mut pt = POINT::default();
    // SAFETY: reads the cursor position and maps it into the list.
    let over = unsafe {
        GetCursorPos(&mut pt) != 0
            && WindowFromPoint(pt) == app.list
            && ScreenToClient(app.list, &mut pt) != 0
    };
    set(app, if over { row_at(app, pt) } else { None });
}

/// The row at `pt`, inside the columns' width only, as the selection is.
fn row_at(app: &App, pt: POINT) -> Option<usize> {
    let mut hit = LVHITTESTINFO {
        pt,
        flags: 0,
        iItem: -1,
        iSubItem: 0,
        iGroup: 0,
    };
    let index = send(app, LVM_HITTEST, 0, (&raw mut hit) as isize);
    let on_item = LVHT_ONITEMICON | LVHT_ONITEMLABEL | LVHT_ONITEMSTATEICON;
    if index < 0 || hit.flags & on_item == 0 {
        return None;
    }
    let mut bounds = RECT {
        left: LVIR_BOUNDS as i32,
        ..RECT::default()
    };
    send(
        app,
        LVM_GETITEMRECT,
        index as usize,
        (&raw mut bounds) as isize,
    );
    (pt.x < bounds.right).then_some(index as usize)
}

fn set(app: &App, row: Option<usize>) {
    let old = app.hot_row.replace(row);
    if old == row {
        return;
    }
    for i in [old, row].into_iter().flatten() {
        send(app, LVM_REDRAWITEMS, i, i as isize);
    }
}
