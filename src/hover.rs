//! The result row under the mouse, which the list draws highlighted.
//!
//! The list's subclass reports mouse moves here; `TrackMouseEvent` reports
//! when the mouse leaves. Only the rows that gain or lose the highlight are
//! redrawn. The cell under the mouse goes on to `celltip`, which shows a
//! cell's cut-off text.

use windows_sys::Win32::Foundation::POINT;
use windows_sys::Win32::UI::Controls::{
    LVHT_ONITEMICON, LVHT_ONITEMLABEL, LVHT_ONITEMSTATEICON, LVIR_BOUNDS,
};

use crate::celltip;
use crate::win::ext::b1;
use crate::win::{self, lv};
use crate::window::App;

/// The mouse moved to `pt`, in the list's client coordinates.
pub fn moved(app: &App, pt: POINT) {
    // Asks for WM_MOUSELEAVE on this thread's list. Asking again while
    // tracking is harmless, and keeps tracking alive should the list cancel
    // its own.
    win::track_leave(app.list);
    let row = row_at(app, pt);
    set(app, row);
    celltip::moved(app, row, pt);
}

pub fn left(app: &App) {
    set(app, None);
    celltip::left(app);
}

/// The list scrolled under a mouse that did not move (or a key was
/// pressed, which may have scrolled it).
pub fn scrolled(app: &App) {
    // The cursor position, mapped into the list.
    let over = b1::cursor_pos()
        .filter(|&pt| win::from_point(pt) == app.list)
        .map(|pt| win::screen_to_client(app.list, pt));
    let row = over.and_then(|pt| row_at(app, pt));
    set(app, row);
    celltip::scrolled(app, row, over);
}

/// The row at `pt`, inside the columns' width only, as the selection is.
fn row_at(app: &App, pt: POINT) -> Option<usize> {
    let hit = lv::hit_test(app.list, pt);
    let index = hit.iItem;
    let on_item = LVHT_ONITEMICON | LVHT_ONITEMLABEL | LVHT_ONITEMSTATEICON;
    if index < 0 || hit.flags & on_item == 0 {
        return None;
    }
    let bounds = lv::item_rect(app.list, index, LVIR_BOUNDS as i32).unwrap_or_default();
    (pt.x < bounds.right).then_some(index as usize)
}

fn set(app: &App, row: Option<usize>) {
    let old = app.hot_row.replace(row);
    // Without Options > View's "Show row mouseover" nothing shows it.
    if old == row || !app.ui().mouseover {
        return;
    }
    for i in [old, row].into_iter().flatten() {
        lv::redraw_items(app.list, i as i32, i as i32);
    }
}
