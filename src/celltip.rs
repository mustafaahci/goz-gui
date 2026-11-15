//! The full text of a result cell that its column cuts off, in a tooltip
//! laid over the cell (Explorer's in-place tips), so the text sits where
//! the cell's own text is.
//!
//! The list's mouse tracking (`hover`) reports the cell under the mouse
//! here. A tip shows once the mouse has rested on a cell for the tooltip's
//! usual delay, and only when that cell's text does not fit; it goes when
//! the mouse leaves the cell or the list, the list scrolls, or a key or
//! button is pressed. It is a tracking tooltip that lets the mouse through
//! (TTF_TRANSPARENT), so the list goes on getting its moves and clicks.

use std::cell::Cell;

use windows_sys::Win32::Foundation::{HWND, POINT, RECT};
use windows_sys::Win32::Graphics::Gdi::MONITOR_DEFAULTTONEAREST;
use windows_sys::Win32::UI::Controls::{
    LVIR_BOUNDS, TTDT_INITIAL, TTF_ABSOLUTE, TTF_TRACK, TTF_TRANSPARENT, TTS_ALWAYSTIP,
    TTS_NOPREFIX, TTTOOLINFOW,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{CW_USEDEFAULT, WS_EX_TOPMOST, WS_POPUP};

use crate::paint;
use crate::theme;
use crate::win::class::{self, Create};
use crate::win::ext::b1;
use crate::win::{self, lv, sys, tip};
use crate::window::App;

/// The main window's timer that shows the tip once the mouse has rested.
pub const TIMER: usize = 21;

/// The tip's one tool.
const TOOL: usize = 1;

/// The tooltip, and the cell under the mouse.
pub struct CellTip {
    hwnd: Cell<HWND>,
    /// The row and column id under the mouse.
    cell: Cell<Option<(usize, usize)>>,
    shown: Cell<bool>,
}

impl Default for CellTip {
    fn default() -> CellTip {
        CellTip {
            hwnd: Cell::new(core::ptr::null_mut()),
            cell: Cell::new(None),
            shown: Cell::new(false),
        }
    }
}

/// The tip's one tool (its text goes in by the `tip` functions).
fn tool(app: &App) -> TTTOOLINFOW {
    TTTOOLINFOW {
        cbSize: size_of::<TTTOOLINFOW>() as u32,
        uFlags: TTF_TRACK | TTF_ABSOLUTE | TTF_TRANSPARENT,
        hwnd: app.hwnd,
        uId: TOOL,
        ..Default::default()
    }
}

/// Makes the tooltip, owned by the main window, with its one tool.
pub fn create(app: &App) {
    // A tooltip control owned by this thread's window.
    let tip = class::create_plain(&Create {
        ex_style: WS_EX_TOPMOST,
        class: tip::CLASS,
        title: "",
        style: WS_POPUP | TTS_ALWAYSTIP | TTS_NOPREFIX,
        rect: (CW_USEDEFAULT, CW_USEDEFAULT, CW_USEDEFAULT, CW_USEDEFAULT),
        parent: app.hwnd,
        menu: core::ptr::null_mut(),
    });
    if tip.is_null() {
        return;
    }
    let info = tool(app);
    tip::add_tool(tip, &info, tip::Text::Str(""));
    app.cell_tip.hwnd.set(tip);
}

/// The tip in the list's font.
pub fn set_font(app: &App) {
    let tip = app.cell_tip.hwnd.get();
    if !tip.is_null() {
        let font = app.fonts.borrow().normal;
        win::set_font(tip, font, false);
    }
}

/// Dark or light with the window, as the search box's tooltips.
pub fn set_theme(app: &App, dark: bool) {
    let tip = app.cell_tip.hwnd.get();
    if tip.is_null() {
        return;
    }
    theme::allow(tip, dark);
    theme::set_class(
        tip,
        Some(if dark {
            "DarkMode_Explorer"
        } else {
            "Explorer"
        }),
    );
}

/// The cell at `pt` (the list's client coordinates) on `row`: its column
/// id and rectangle.
fn cell_at(app: &App, row: usize, pt: POINT) -> Option<(usize, RECT, RECT)> {
    let bounds = lv::item_rect(app.list, row as i32, LVIR_BOUNDS as i32).unwrap_or_default();
    let cells = app.column_rects(&bounds);
    cells
        .iter()
        .find(|(_, rc)| pt.x >= rc.left && pt.x < rc.right)
        .map(|&(id, rc)| (id, rc, bounds))
}

/// The mouse is at `pt` over `row` (none: not over a row): a new cell
/// hides the tip, and shows its own once the mouse rests there.
pub fn moved(app: &App, row: Option<usize>, pt: POINT) {
    // Thumbnails have no columns: their names wrap under the picture.
    let row = row.filter(|_| !app.view().thumbnails);
    let cell = row.and_then(|r| cell_at(app, r, pt).map(|(id, _, _)| (r, id)));
    if app.cell_tip.cell.replace(cell) == cell {
        return;
    }
    hide(app);
    let tip = app.cell_tip.hwnd.get();
    if cell.is_none() || tip.is_null() {
        return;
    }
    let delay = tip::delay_time(tip, TTDT_INITIAL).clamp(1, 5000);
    // A timer of this thread's window; setting it again restarts it.
    win::set_timer(app.hwnd, TIMER, delay as u32);
}

/// The rows moved under the mouse (a scroll, a jump): the tip goes, and
/// the cell now under the mouse waits for the mouse to move on.
pub fn scrolled(app: &App, row: Option<usize>, pt: Option<POINT>) {
    hide(app);
    let cell = row
        .zip(pt)
        .and_then(|(r, pt)| cell_at(app, r, pt).map(|(id, _, _)| (r, id)));
    app.cell_tip.cell.set(cell);
}

/// The mouse left the list.
pub fn left(app: &App) {
    hide(app);
    app.cell_tip.cell.set(None);
}

/// Takes the tip away (a key, a click), and stops one about to show. It
/// shows again only on another cell.
pub fn hide(app: &App) {
    // A timer of this thread's window.
    win::kill_timer(app.hwnd, TIMER);
    if app.cell_tip.shown.replace(false) {
        let info = tool(app);
        tip::track_activate(app.cell_tip.hwnd.get(), false, &info);
    }
}

/// The mouse has rested on its cell: the tip shows there if the cell's
/// text is cut off.
pub fn timer(app: &App) {
    let tip = app.cell_tip.hwnd.get();
    let Some((row, id)) = app.cell_tip.cell.get() else {
        return;
    };
    if tip.is_null() || app.hot_row.get() != Some(row) {
        return;
    }
    // The cell's place, from the column wherever it was dragged to.
    let bounds = lv::item_rect(app.list, row as i32, LVIR_BOUNDS as i32).unwrap_or_default();
    let Some(&(_, cell)) = app.column_rects(&bounds).iter().find(|(c, _)| *c == id) else {
        return;
    };
    // The text measured with the model borrowed, and nothing sent while it
    // is.
    let measured = {
        let m = app.model.borrow();
        m.peek(row)
            .map(|r| paint::cell_text(app, &m, r, id, &bounds, cell.left))
    };
    let Some(ct) = measured else {
        return;
    };
    if !needs_tip(ct.need, cell.right - cell.left) {
        return;
    }
    let height = app.fonts.borrow().height;
    // The text's two corners, from the list's client area to the screen.
    let text_rc = RECT {
        left: ct.x,
        top: ct.y,
        right: ct.x + ct.width,
        bottom: ct.y + height,
    };
    let window = win::map_rect(app.list, core::ptr::null_mut(), text_rc);
    // The tip's window round that text: its border and margins outside.
    let window = tip::adjust_rect(tip, true, window);
    let work = work_area(POINT {
        x: window.left,
        y: window.top,
    });
    // As wide as the screen at most (a longer text wraps), and on it.
    tip::set_max_width(tip, (work.right - work.left).max(1));
    let (x, y) = clamp(&window, &work);
    let info = tool(app);
    b1::tip_update_text_wide(tip, &info, &ct.text);
    tip::track_position(tip, x, y);
    tip::track_activate(tip, true, &info);
    app.cell_tip.shown.set(true);
}

/// A cell needing `need` pixels for its text shows it cut off in `width`.
fn needs_tip(need: i32, width: i32) -> bool {
    need > 0 && need > width
}

/// Where a tip of `window` goes so that it stays on `work`: moved left (or
/// up) as far as it must, but never past the left (or top) edge.
fn clamp(window: &RECT, work: &RECT) -> (i32, i32) {
    let w = window.right - window.left;
    let h = window.bottom - window.top;
    let x = window.left.min(work.right - w).max(work.left);
    let y = window.top.min(work.bottom - h).max(work.top);
    (x, y)
}

/// The work area of the monitor nearest `pt` (screen).
fn work_area(pt: POINT) -> RECT {
    sys::monitor_info(sys::monitor_from_point(pt, MONITOR_DEFAULTTONEAREST))
        .map(|info| info.rcWork)
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_cut_off_text_gets_a_tip() {
        assert!(needs_tip(120, 100));
        assert!(!needs_tip(100, 100));
        assert!(!needs_tip(80, 100));
        // An empty cell needs nothing, however narrow.
        assert!(!needs_tip(0, 0));
    }

    #[test]
    fn a_tip_stays_on_the_screen() {
        let work = RECT {
            left: 0,
            top: 0,
            right: 1000,
            bottom: 800,
        };
        let at = |left, top, w, h| RECT {
            left,
            top,
            right: left + w,
            bottom: top + h,
        };
        assert_eq!(clamp(&at(100, 50, 200, 20), &work), (100, 50));
        assert_eq!(clamp(&at(900, 790, 200, 20), &work), (800, 780));
        // Wider than the screen: at its left edge.
        assert_eq!(clamp(&at(300, 10, 1500, 20), &work), (0, 10));
        assert_eq!(clamp(&at(-50, -5, 100, 20), &work), (0, 0));
    }
}
