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
use windows_sys::Win32::Graphics::Gdi::{
    GetMonitorInfoW, MONITOR_DEFAULTTONEAREST, MONITORINFO, MapWindowPoints, MonitorFromPoint,
};
use windows_sys::Win32::UI::Controls::{
    LVIR_BOUNDS, LVM_GETITEMRECT, TOOLTIPS_CLASSW, TTDT_INITIAL, TTF_ABSOLUTE, TTF_TRACK,
    TTF_TRANSPARENT, TTM_ADDTOOLW, TTM_ADJUSTRECT, TTM_GETDELAYTIME, TTM_SETMAXTIPWIDTH,
    TTM_TRACKACTIVATE, TTM_TRACKPOSITION, TTM_UPDATETIPTEXTW, TTS_ALWAYSTIP, TTS_NOPREFIX,
    TTTOOLINFOW,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CW_USEDEFAULT, CreateWindowExW, KillTimer, SendMessageW, SetTimer, WM_SETFONT, WS_EX_TOPMOST,
    WS_POPUP,
};

use crate::paint;
use crate::theme;
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

fn tool(app: &App, text: *mut u16) -> TTTOOLINFOW {
    TTTOOLINFOW {
        cbSize: size_of::<TTTOOLINFOW>() as u32,
        uFlags: TTF_TRACK | TTF_ABSOLUTE | TTF_TRANSPARENT,
        hwnd: app.hwnd,
        uId: TOOL,
        lpszText: text,
        // SAFETY: the rest of TTTOOLINFOW is plain data.
        ..unsafe { core::mem::zeroed() }
    }
}

fn send(tip: HWND, msg: u32, w: usize, l: isize) -> isize {
    // SAFETY: a message to this thread's tooltip; any pointer in `l` is the
    // caller's and lives across the synchronous call.
    unsafe { SendMessageW(tip, msg, w, l) }
}

/// Makes the tooltip, owned by the main window, with its one tool.
pub fn create(app: &App) {
    // SAFETY: creates a tooltip control owned by this thread's window.
    let tip = unsafe {
        CreateWindowExW(
            WS_EX_TOPMOST,
            TOOLTIPS_CLASSW,
            core::ptr::null(),
            WS_POPUP | TTS_ALWAYSTIP | TTS_NOPREFIX,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            app.hwnd,
            core::ptr::null_mut(),
            core::ptr::null_mut(),
            core::ptr::null(),
        )
    };
    if tip.is_null() {
        return;
    }
    let mut empty = [0u16];
    let info = tool(app, empty.as_mut_ptr());
    send(tip, TTM_ADDTOOLW, 0, (&raw const info) as isize);
    app.cell_tip.hwnd.set(tip);
}

/// The tip in the list's font.
pub fn set_font(app: &App) {
    let tip = app.cell_tip.hwnd.get();
    if !tip.is_null() {
        let font = app.fonts.borrow().normal;
        send(tip, WM_SETFONT, font as usize, 0);
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
    let mut bounds = RECT {
        left: LVIR_BOUNDS as i32,
        ..RECT::default()
    };
    // SAFETY: the rectangle lives across the synchronous call.
    unsafe { SendMessageW(app.list, LVM_GETITEMRECT, row, (&raw mut bounds) as isize) };
    let cells = app.column_rects(&bounds);
    cells
        .iter()
        .find(|(_, rc)| pt.x >= rc.left && pt.x < rc.right)
        .map(|&(id, rc)| (id, rc, bounds))
}

/// The mouse is at `pt` over `row` (none: not over a row): a new cell
/// hides the tip, and shows its own once the mouse rests there.
pub fn moved(app: &App, row: Option<usize>, pt: POINT) {
    let cell = row.and_then(|r| cell_at(app, r, pt).map(|(id, _, _)| (r, id)));
    if app.cell_tip.cell.replace(cell) == cell {
        return;
    }
    hide(app);
    let tip = app.cell_tip.hwnd.get();
    if cell.is_none() || tip.is_null() {
        return;
    }
    let delay = send(tip, TTM_GETDELAYTIME, TTDT_INITIAL as usize, 0).clamp(1, 5000);
    // SAFETY: a timer of this thread's window; setting it again restarts it.
    unsafe { SetTimer(app.hwnd, TIMER, delay as u32, None) };
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
    // SAFETY: a timer of this thread's window.
    unsafe { KillTimer(app.hwnd, TIMER) };
    if app.cell_tip.shown.replace(false) {
        let info = tool(app, core::ptr::null_mut());
        send(
            app.cell_tip.hwnd.get(),
            TTM_TRACKACTIVATE,
            0,
            (&raw const info) as isize,
        );
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
    let mut bounds = RECT {
        left: LVIR_BOUNDS as i32,
        ..RECT::default()
    };
    // SAFETY: the rectangle lives across the synchronous call.
    unsafe { SendMessageW(app.list, LVM_GETITEMRECT, row, (&raw mut bounds) as isize) };
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
    let Some(mut ct) = measured else {
        return;
    };
    if !needs_tip(ct.need, cell.right - cell.left) {
        return;
    }
    let height = app.fonts.borrow().height;
    let mut rc = [
        POINT { x: ct.x, y: ct.y },
        POINT {
            x: ct.x + ct.width,
            y: ct.y + height,
        },
    ];
    // SAFETY: maps two points from the list's client area to the screen.
    unsafe { MapWindowPoints(app.list, core::ptr::null_mut(), rc.as_mut_ptr(), 2) };
    let mut window = RECT {
        left: rc[0].x,
        top: rc[0].y,
        right: rc[1].x,
        bottom: rc[1].y,
    };
    // The tip's window round that text: its border and margins outside.
    send(tip, TTM_ADJUSTRECT, 1, (&raw mut window) as isize);
    let work = work_area(POINT {
        x: window.left,
        y: window.top,
    });
    // As wide as the screen at most (a longer text wraps), and on it.
    send(
        tip,
        TTM_SETMAXTIPWIDTH,
        0,
        (work.right - work.left).max(1) as isize,
    );
    let (x, y) = clamp(&window, &work);
    ct.text.push(0);
    let info = tool(app, ct.text.as_mut_ptr());
    send(tip, TTM_UPDATETIPTEXTW, 0, (&raw const info) as isize);
    send(
        tip,
        TTM_TRACKPOSITION,
        0,
        ((x as u16 as u32) | ((y as u16 as u32) << 16)) as isize,
    );
    send(tip, TTM_TRACKACTIVATE, 1, (&raw const info) as isize);
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
    let mut info = MONITORINFO {
        cbSize: size_of::<MONITORINFO>() as u32,
        ..MONITORINFO::default()
    };
    // SAFETY: fills a plain out-struct for a monitor handle just found.
    unsafe { GetMonitorInfoW(MonitorFromPoint(pt, MONITOR_DEFAULTTONEAREST), &mut info) };
    info.rcWork
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
