//! The search history dropdown: the searches made, newest first, in a popup
//! under the search frame and exactly as wide, as Everything shows them.
//! While a search is typed with `search_history_always_suggest=1` it lists
//! only the searches containing the text instead (`suggest`).
//!
//! The popup is never activated (`WS_EX_NOACTIVATE`, `MA_NOACTIVATE`), so
//! the keyboard stays in the search box, whose subclass drives it: Up and
//! Down move the highlight, Enter uses the entry, Delete forgets it, Esc
//! closes. A click anywhere else, the window going inactive, a menu opening
//! or the box losing the focus close it too. The whole history is read from
//! `App::search_history` as it is drawn, so the popup holds no copy of it;
//! suggestions (at most a few hundred) are kept as texts.

use std::cell::{Cell, RefCell};

use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows_sys::Win32::Graphics::Gdi::{
    BeginPaint, BitBlt, ClientToScreen, CreateCompatibleBitmap, CreateCompatibleDC,
    DT_END_ELLIPSIS, DT_LEFT, DT_NOPREFIX, DT_SINGLELINE, DT_VCENTER, DeleteDC, DeleteObject,
    DrawTextW, EndPaint, GetMonitorInfoW, GetWindowDC, InvalidateRect, MONITOR_DEFAULTTONEAREST,
    MONITORINFO, MonitorFromRect, PAINTSTRUCT, RDW_FRAME, RDW_INVALIDATE, RedrawWindow, ReleaseDC,
    SRCCOPY, SelectObject, SetBkMode, SetTextColor, TRANSPARENT,
};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::Controls::SetScrollInfo;
use windows_sys::Win32::UI::WindowsAndMessaging::*;

use crate::paint::fill;
use crate::searchbox::{self, Button};
use crate::theme;
use crate::wide::wz;
use crate::window::App;

const CLASS: &str = "goz-gui-dropdown";
/// Rows shown before the list scrolls, as Everything's
/// `search_history_visible_count_max`.
const MAX_ROWS: usize = 12;

/// The popup and what it shows.
pub struct Dropdown {
    hwnd: Cell<HWND>,
    /// The highlighted entry: under the mouse, or chosen with the keys.
    hot: Cell<Option<usize>>,
    /// The first entry shown.
    top: Cell<usize>,
    /// The entry the left button went down on.
    pressed: Cell<Option<usize>>,
    /// The suggestions shown while a search is typed; `None` while the
    /// whole history shows.
    suggestions: RefCell<Option<Vec<String>>>,
}

impl Dropdown {
    pub const fn new() -> Dropdown {
        Dropdown {
            hwnd: Cell::new(core::ptr::null_mut()),
            hot: Cell::new(None),
            top: Cell::new(0),
            pressed: Cell::new(None),
            suggestions: RefCell::new(None),
        }
    }

    pub fn hwnd(&self) -> HWND {
        self.hwnd.get()
    }

    pub fn is_open(&self) -> bool {
        let hwnd = self.hwnd.get();
        // SAFETY: asks about a window of this thread, or null.
        !hwnd.is_null() && unsafe { IsWindowVisible(hwnd) } != 0
    }

    /// The highlighted entry.
    pub fn hot(&self) -> Option<usize> {
        self.hot.get()
    }
}

/// Makes the popup, hidden, owned by the main window.
pub fn create(app: &'static App) {
    let class = wz(CLASS);
    // SAFETY: registers the class once and creates the popup on this
    // thread; `app` is leaked, so the pointer kept in the window stays good.
    unsafe {
        let hinst = GetModuleHandleW(core::ptr::null());
        let wc = WNDCLASSEXW {
            cbSize: size_of::<WNDCLASSEXW>() as u32,
            lpfnWndProc: Some(wndproc),
            hInstance: hinst,
            hCursor: LoadCursorW(core::ptr::null_mut(), IDC_ARROW),
            lpszClassName: class.as_ptr(),
            ..core::mem::zeroed()
        };
        RegisterClassExW(&wc);
        let hwnd = CreateWindowExW(
            WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW,
            class.as_ptr(),
            core::ptr::null(),
            WS_POPUP | WS_BORDER | WS_VSCROLL | WS_CLIPSIBLINGS,
            0,
            0,
            0,
            0,
            app.hwnd,
            core::ptr::null_mut(),
            hinst,
            core::ptr::null(),
        );
        if hwnd.is_null() {
            return;
        }
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, app as *const App as isize);
        app.dropdown.hwnd.set(hwnd);
    }
    apply_theme(app);
}

/// Follows the window's theme: dark or light scroll bar, and a repaint.
pub fn apply_theme(app: &App) {
    let hwnd = app.dropdown.hwnd();
    if hwnd.is_null() {
        return;
    }
    let dark = app.pal.get().dark;
    theme::allow(hwnd, dark);
    theme::set_class(
        hwnd,
        Some(if dark {
            "DarkMode_Explorer"
        } else {
            "Explorer"
        }),
    );
    // SAFETY: repaints this thread's popup, frame included.
    unsafe {
        RedrawWindow(
            hwnd,
            core::ptr::null(),
            core::ptr::null_mut(),
            RDW_INVALIDATE | RDW_FRAME,
        )
    };
}

/// How many entries the popup lists.
fn count(app: &App) -> usize {
    match &*app.dropdown.suggestions.borrow() {
        Some(list) => list.len(),
        None => app.search_history.borrow().entries().len(),
    }
}

/// The text of entry `i`.
fn text_at(app: &App, i: usize) -> Option<String> {
    match &*app.dropdown.suggestions.borrow() {
        Some(list) => list.get(i).cloned(),
        None => app
            .search_history
            .borrow()
            .entries()
            .get(i)
            .map(|e| e.search.clone()),
    }
}

fn row_height(app: &App) -> i32 {
    app.ui_fonts.borrow().height + app.scale(4)
}

fn visible(app: &App) -> usize {
    count(app).min(MAX_ROWS)
}

/// Opens the dropdown with the entry that is the search box's text, if one
/// is, highlighted. False, and nothing shows, while there is no history.
pub fn open(app: &App, current: &str) -> bool {
    let d = &app.dropdown;
    *d.suggestions.borrow_mut() = None;
    if d.hwnd().is_null() || count(app) == 0 {
        return false;
    }
    let hot = app
        .search_history
        .borrow()
        .entries()
        .iter()
        .position(|e| e.search == current);
    d.hot.set(hot);
    d.pressed.set(None);
    d.top.set(0);
    place(app, true);
    if let Some(i) = hot {
        ensure_visible(app, i);
    }
    searchbox::invalidate(app);
    true
}

/// Lists `suggestions` (searches containing the text being typed) with
/// none highlighted, opening the popup without taking the keyboard from the
/// box, or resizing it when open; closes it when there are none.
pub fn suggest(app: &App, suggestions: Vec<String>) {
    let d = &app.dropdown;
    if d.hwnd().is_null() || suggestions.is_empty() {
        close(app);
        return;
    }
    *d.suggestions.borrow_mut() = Some(suggestions);
    d.hot.set(None);
    d.pressed.set(None);
    d.top.set(0);
    place(app, true);
    searchbox::invalidate(app);
}

pub fn close(app: &App) {
    let d = &app.dropdown;
    if !d.is_open() {
        *d.suggestions.borrow_mut() = None;
        return;
    }
    // SAFETY: hides this thread's popup.
    unsafe { ShowWindow(d.hwnd(), SW_HIDE) };
    d.hot.set(None);
    d.pressed.set(None);
    *d.suggestions.borrow_mut() = None;
    searchbox::invalidate(app);
}

/// Sizes and places the popup under the search frame, as wide as it, and
/// shows it when `show`; above the frame when there is no room below.
fn place(app: &App, show: bool) {
    let hwnd = app.dropdown.hwnd();
    let rows = visible(app);
    let rh = row_height(app);
    let search = app.search_rc.get();
    // 1px border round the rows (WS_BORDER).
    let height = rows as i32 * rh + 2;
    let mut origin = POINT {
        x: search.left,
        y: search.top,
    };
    // SAFETY: plain out-structs for this thread's windows.
    unsafe {
        ClientToScreen(app.hwnd, &mut origin);
        let frame = RECT {
            left: origin.x,
            top: origin.y,
            right: origin.x + (search.right - search.left),
            bottom: origin.y + (search.bottom - search.top),
        };
        let mut y = frame.bottom;
        let mut mi: MONITORINFO = core::mem::zeroed();
        mi.cbSize = size_of::<MONITORINFO>() as u32;
        if GetMonitorInfoW(MonitorFromRect(&frame, MONITOR_DEFAULTTONEAREST), &mut mi) != 0
            && y + height > mi.rcWork.bottom
            && frame.top - height >= mi.rcWork.top
        {
            y = frame.top - height;
        }
        let mut flags = SWP_NOACTIVATE;
        if show {
            flags |= SWP_SHOWWINDOW;
        }
        SetWindowPos(
            hwnd,
            HWND_TOP,
            frame.left,
            y,
            frame.right - frame.left,
            height,
            flags,
        );
    }
    update_scroll(app);
    // SAFETY: repaints this thread's popup.
    unsafe { InvalidateRect(hwnd, core::ptr::null(), 0) };
}

/// The scroll bar after the entries or the first one shown changed; it
/// shows only while there are more entries than rows.
fn update_scroll(app: &App) {
    let d = &app.dropdown;
    let n = count(app);
    let rows = visible(app);
    let top = d.top.get().min(n.saturating_sub(rows));
    d.top.set(top);
    let si = SCROLLINFO {
        cbSize: size_of::<SCROLLINFO>() as u32,
        fMask: SIF_ALL,
        nMin: 0,
        nMax: n.saturating_sub(1) as i32,
        nPage: rows as u32,
        nPos: top as i32,
        nTrackPos: 0,
    };
    // SAFETY: the struct lives across the call.
    unsafe { SetScrollInfo(d.hwnd(), SB_VERT, &si, 1) };
}

fn scroll_to(app: &App, top: usize) {
    app.dropdown.top.set(top);
    update_scroll(app);
    // SAFETY: repaints this thread's popup.
    unsafe { InvalidateRect(app.dropdown.hwnd(), core::ptr::null(), 0) };
}

fn ensure_visible(app: &App, index: usize) {
    let rows = visible(app).max(1);
    let top = app.dropdown.top.get();
    if index < top {
        scroll_to(app, index);
    } else if index >= top + rows {
        scroll_to(app, index + 1 - rows);
    }
}

fn set_hot(app: &App, hot: Option<usize>) {
    if app.dropdown.hot.replace(hot) != hot {
        // SAFETY: repaints this thread's popup.
        unsafe { InvalidateRect(app.dropdown.hwnd(), core::ptr::null(), 0) };
    }
}

/// Moves the highlight `delta` entries, stopping at either end; from no
/// highlight it starts at the newest entry.
pub fn move_hot(app: &App, delta: isize) {
    let n = count(app);
    if n == 0 {
        return;
    }
    let next = match app.dropdown.hot() {
        None => 0,
        Some(i) => i.saturating_add_signed(delta).min(n - 1),
    };
    set_hot(app, Some(next));
    ensure_visible(app, next);
}

/// A page of rows, for PgUp (`-1`) and PgDn (`1`).
pub fn page(app: &App, direction: isize) {
    let step = visible(app).saturating_sub(1).max(1) as isize;
    move_hot(app, step * direction);
}

/// The highlighted entry's text, for the search box to take.
pub fn hot_text(app: &App) -> Option<String> {
    text_at(app, app.dropdown.hot()?)
}

/// Forgets the highlighted entry; the next one takes the highlight. The
/// popup closes once the history is empty. False when nothing was
/// highlighted.
pub fn delete_hot(app: &App) -> bool {
    let Some(i) = app.dropdown.hot() else {
        return false;
    };
    let Some(text) = text_at(app, i) else {
        return false;
    };
    if let Some(list) = app.dropdown.suggestions.borrow_mut().as_mut() {
        list.remove(i);
    }
    // Out of the history and, at the next write, the file.
    app.forget_searches(&[text]);
    let n = count(app);
    if n == 0 {
        close(app);
        return true;
    }
    app.dropdown.hot.set(Some(i.min(n - 1)));
    place(app, false);
    ensure_visible(app, i.min(n - 1));
    true
}

/// Before a message is dispatched: a press anywhere but the popup (or the
/// ▾ button, which toggles it itself) closes the dropdown.
pub fn click_elsewhere(app: &App, msg: &MSG) {
    if !is_press(msg.message) || !app.dropdown.is_open() || msg.hwnd == app.dropdown.hwnd() {
        return;
    }
    if msg.hwnd == app.hwnd && matches!(msg.message, WM_LBUTTONDOWN | WM_LBUTTONDBLCLK) {
        let pt = point(msg.lParam);
        if searchbox::hit(app, pt) == Some(Button::History) {
            return;
        }
    }
    close(app);
}

fn is_press(msg: u32) -> bool {
    matches!(
        msg,
        WM_LBUTTONDOWN
            | WM_LBUTTONDBLCLK
            | WM_RBUTTONDOWN
            | WM_RBUTTONDBLCLK
            | WM_MBUTTONDOWN
            | WM_MBUTTONDBLCLK
            | WM_XBUTTONDOWN
            | WM_XBUTTONDBLCLK
            | WM_NCLBUTTONDOWN
            | WM_NCLBUTTONDBLCLK
            | WM_NCRBUTTONDOWN
            | WM_NCRBUTTONDBLCLK
            | WM_NCMBUTTONDOWN
            | WM_NCMBUTTONDBLCLK
            | WM_NCXBUTTONDOWN
            | WM_NCXBUTTONDBLCLK
    )
}

fn point(l: LPARAM) -> POINT {
    POINT {
        x: (l & 0xffff) as i16 as i32,
        y: ((l >> 16) & 0xffff) as i16 as i32,
    }
}

/// The entry at `y` in the popup's client area.
fn row_at(app: &App, y: i32) -> Option<usize> {
    if y < 0 {
        return None;
    }
    let i = app.dropdown.top.get() + (y / row_height(app).max(1)) as usize;
    (i < count(app)).then_some(i)
}

/// Scrolls `lines` entries, up for negative.
fn scroll_by(app: &App, lines: isize) {
    let top = app.dropdown.top.get().saturating_add_signed(lines);
    scroll_to(app, top);
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    // SAFETY: GWLP_USERDATA holds the leaked `App` once `create` set it, and
    // null before; each arm reads `w`/`l` as its message defines them.
    unsafe {
        let app = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *const App;
        if app.is_null() {
            return DefWindowProcW(hwnd, msg, w, l);
        }
        let app = &*app;
        match msg {
            // A click must not take the focus from the search box.
            WM_MOUSEACTIVATE => MA_NOACTIVATE as LRESULT,
            // Minimizing the window hides the popup with it; it stays closed
            // when the window comes back.
            WM_SHOWWINDOW if l == SW_PARENTOPENING as LPARAM => 0,
            WM_ERASEBKGND => 1,
            WM_PAINT => {
                paint(app, hwnd);
                0
            }
            WM_NCPAINT => {
                let r = DefWindowProcW(hwnd, msg, w, l);
                border(app, hwnd);
                r
            }
            WM_MOUSEMOVE => {
                if let Some(i) = row_at(app, point(l).y) {
                    set_hot(app, Some(i));
                }
                0
            }
            WM_LBUTTONDOWN => {
                app.dropdown.pressed.set(row_at(app, point(l).y));
                0
            }
            WM_LBUTTONUP => {
                let row = row_at(app, point(l).y);
                if row.is_some() && app.dropdown.pressed.take() == row {
                    set_hot(app, row);
                    app.use_history_entry();
                }
                0
            }
            WM_MOUSEWHEEL => {
                let delta = i32::from(((w >> 16) & 0xffff) as u16 as i16);
                let mut lines: u32 = 3;
                SystemParametersInfoW(SPI_GETWHEELSCROLLLINES, 0, (&raw mut lines).cast(), 0);
                let lines = (lines.clamp(1, MAX_ROWS as u32) as i32 * -delta / 120) as isize;
                scroll_by(app, lines);
                0
            }
            WM_VSCROLL => {
                let rows = visible(app) as isize;
                let top = app.dropdown.top.get() as isize;
                let target = match (w & 0xffff) as i32 {
                    SB_LINEUP => top - 1,
                    SB_LINEDOWN => top + 1,
                    SB_PAGEUP => top - rows,
                    SB_PAGEDOWN => top + rows,
                    SB_TOP => 0,
                    SB_BOTTOM => count(app) as isize,
                    SB_THUMBTRACK | SB_THUMBPOSITION => {
                        let mut si = SCROLLINFO {
                            cbSize: size_of::<SCROLLINFO>() as u32,
                            fMask: SIF_TRACKPOS,
                            ..Default::default()
                        };
                        GetScrollInfo(hwnd, SB_VERT, &mut si);
                        si.nTrackPos as isize
                    }
                    _ => top,
                };
                scroll_to(app, target.max(0) as usize);
                0
            }
            _ => DefWindowProcW(hwnd, msg, w, l),
        }
    }
}

/// The entries in view, double-buffered.
fn paint(app: &App, hwnd: HWND) {
    let pal = app.pal.get();
    let rh = row_height(app);
    let pad = app.scale(4);
    // SAFETY: a standard double-buffered WM_PAINT; every GDI object made
    // here is released before returning.
    unsafe {
        let mut ps: PAINTSTRUCT = core::mem::zeroed();
        let hdc = BeginPaint(hwnd, &mut ps);
        let mut client = RECT::default();
        GetClientRect(hwnd, &mut client);
        let (w, h) = (client.right.max(1), client.bottom.max(1));
        let mem = CreateCompatibleDC(hdc);
        let bmp = CreateCompatibleBitmap(hdc, w, h);
        let old_bmp = SelectObject(mem, bmp);
        fill(mem, &client, pal.popup_bg);
        let old_font = SelectObject(mem, app.ui_font());
        SetBkMode(mem, TRANSPARENT as i32);
        SetTextColor(mem, pal.fg);
        let top = app.dropdown.top.get();
        let hot = app.dropdown.hot();
        let suggestions = app.dropdown.suggestions.borrow();
        let history = app.search_history.borrow();
        let texts: Box<dyn Iterator<Item = &str>> = match &*suggestions {
            Some(list) => Box::new(list.iter().map(String::as_str)),
            None => Box::new(history.entries().iter().map(|e| e.search.as_str())),
        };
        for (row, text) in texts.enumerate().skip(top) {
            let y = (row - top) as i32 * rh;
            if y >= h {
                break;
            }
            let rc = RECT {
                top: y,
                bottom: y + rh,
                ..client
            };
            if hot == Some(row) {
                fill(mem, &rc, pal.popup_hot);
            }
            let mut text: Vec<u16> = text.encode_utf16().collect();
            let mut text_rc = RECT {
                left: rc.left + pad,
                right: rc.right - pad,
                ..rc
            };
            DrawTextW(
                mem,
                text.as_mut_ptr(),
                text.len() as i32,
                &mut text_rc,
                DT_LEFT | DT_SINGLELINE | DT_VCENTER | DT_END_ELLIPSIS | DT_NOPREFIX,
            );
        }
        drop(history);
        drop(suggestions);
        SelectObject(mem, old_font);
        BitBlt(hdc, 0, 0, w, h, mem, 0, 0, SRCCOPY);
        SelectObject(mem, old_bmp);
        DeleteObject(bmp);
        DeleteDC(mem);
        EndPaint(hwnd, &ps);
    }
}

/// The 1px edge round the popup, in the theme's colour over the one
/// WS_BORDER draws.
fn border(app: &App, hwnd: HWND) {
    let colour = app.pal.get().popup_border;
    // SAFETY: paints this thread's popup's frame; the DC is released.
    unsafe {
        let mut wr = RECT::default();
        GetWindowRect(hwnd, &mut wr);
        let (w, h) = (wr.right - wr.left, wr.bottom - wr.top);
        let hdc = GetWindowDC(hwnd);
        for rc in [
            RECT {
                left: 0,
                top: 0,
                right: w,
                bottom: 1,
            },
            RECT {
                left: 0,
                top: h - 1,
                right: w,
                bottom: h,
            },
            RECT {
                left: 0,
                top: 0,
                right: 1,
                bottom: h,
            },
            RECT {
                left: w - 1,
                top: 0,
                right: w,
                bottom: h,
            },
        ] {
            fill(hdc, &rc, colour);
        }
        ReleaseDC(hwnd, hdc);
    }
}
