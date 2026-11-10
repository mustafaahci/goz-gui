//! A pane: a child window of the main window in one of its docks, with a
//! title row (its name and a close button, Everything's `pane_title=1`) over
//! a client area that a feature fills. The feature hands the framework a
//! [`PaneSpec`] (`App::register_pane`); the framework makes the window when
//! the pane opens and gives the feature's [`PaneContent`] the window to fill,
//! places it as the docks change, and destroys it when the pane closes.
//!
//! The window paints its title row itself, double-buffered: the header's
//! colours and lines, the name, and the close button lit under the mouse and
//! darker while held (it acts on release over itself). Every message the
//! framework does not take (the content's controls' WM_COMMAND, WM_NOTIFY
//! and WM_CTLCOLOR*, timers, the keyboard while the pane window itself has
//! the focus, ...) goes to the content first.
//!
//! Re-entrancy: the content is an `Rc<dyn PaneContent>` whose methods take
//! `&self`. The framework holds no borrow of its own state while it calls
//! one, and a content must not hold a `RefCell` borrow of its own across a
//! call that sends messages (creating, moving or destroying its windows,
//! SendMessage to its controls), since they come back here.

use std::any::Any;
use std::cell::Cell;
use std::rc::Rc;

use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows_sys::Win32::Graphics::Gdi::{
    BeginPaint, BitBlt, CreateCompatibleBitmap, CreateCompatibleDC, DC_BRUSH, DT_END_ELLIPSIS,
    DT_NOPREFIX, DT_SINGLELINE, DT_VCENTER, DeleteDC, DeleteObject, DrawTextW, EndPaint,
    GetStockObject, HDC, InvalidateRect, PAINTSTRUCT, SRCCOPY, SelectObject, SetBkColor, SetBkMode,
    SetDCBrushColor, SetTextColor, SetViewportOrgEx, TRANSPARENT,
};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::Controls::WM_MOUSELEAVE;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    GetCapture, ReleaseCapture, SetCapture, SetFocus, TME_LEAVE, TRACKMOUSEEVENT, TrackMouseEvent,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CREATESTRUCTW, CreateWindowExW, DefWindowProcW, GWLP_USERDATA, GetClientRect, GetParent,
    GetWindowLongPtrW, HMENU, IDC_ARROW, LoadCursorW, MSG, PostMessageW, RegisterClassExW,
    SetWindowLongPtrW, WM_CAPTURECHANGED, WM_CREATE, WM_CTLCOLORBTN, WM_CTLCOLOREDIT,
    WM_CTLCOLORLISTBOX, WM_CTLCOLORSTATIC, WM_ERASEBKGND, WM_LBUTTONDBLCLK, WM_LBUTTONDOWN,
    WM_LBUTTONUP, WM_MOUSEMOVE, WM_NCCREATE, WM_NCDESTROY, WM_PAINT, WM_SETFOCUS, WM_SIZE,
    WNDCLASSEXW, WS_CHILD, WS_CLIPCHILDREN, WS_CLIPSIBLINGS,
};

use crate::dock::{PaneId, Side};
use crate::paint::fill;
use crate::searchbox;
use crate::wide::wz;
use crate::window::App;

/// What a feature puts in a pane. Every method runs on the UI thread with
/// nothing of the framework borrowed; see the module's note on
/// re-entrancy.
pub trait PaneContent: Any {
    /// Makes the content's windows as children of `pane` (the pane's
    /// window), as the pane opens. `place` follows at once.
    fn create(&self, app: &App, pane: HWND);

    /// Puts the content in `rc`: the pane window's client area below its
    /// title row, in the pane window's coordinates. After `create`, and
    /// whenever the pane's size or the title row's height changes.
    fn place(&self, app: &App, rc: RECT);

    /// The pane is closing (or the window is going away): frees what
    /// `create` made and anything held for the pane (handlers, threads,
    /// caches). The pane's window and its children are destroyed after.
    fn destroy(&self, _app: &App) {}

    /// The window that takes the keyboard when the pane does (F6, Tab, a
    /// click on the title row); `None` for the pane window itself.
    fn focus(&self) -> Option<HWND> {
        None
    }

    /// Tab (`forward`) or Shift+Tab was pressed in `from`, one of the
    /// pane's windows: true when the content moved the keyboard itself
    /// (between its own controls), false to go on to the next frame.
    fn tab(&self, _app: &App, _from: HWND, _forward: bool) -> bool {
        false
    }

    /// A key message (`WM_KEYDOWN`, `WM_SYSKEYDOWN`, `WM_CHAR`,
    /// `WM_SYSCHAR`) for one of the pane's windows, before the window's
    /// shortcuts (the accelerator table) and Tab see it: true when the
    /// content took it, and then it goes no further (to hand it to its
    /// control past the shortcuts, `TranslateMessage` and
    /// `DispatchMessageW` it here). For the keys a control must keep
    /// (Enter in a box, F2 in a tree, Ctrl+A in an edit box...).
    fn key(&self, _app: &App, _msg: &MSG) -> bool {
        false
    }

    /// A message to the pane window that the framework leaves to the
    /// content: the content's controls' WM_COMMAND, WM_NOTIFY,
    /// WM_CTLCOLOR*, WM_DRAWITEM; WM_TIMER; WM_CONTEXTMENU; the keyboard and
    /// WM_SETFOCUS / WM_KILLFOCUS while the pane window itself has the
    /// focus; the content's own WM_APP+n. `Some` is the answer; `None`
    /// lets the framework answer (the theme's colours for WM_CTLCOLOR*,
    /// else DefWindowProc).
    fn message(
        &self,
        _app: &App,
        _pane: HWND,
        _msg: u32,
        _w: WPARAM,
        _l: LPARAM,
    ) -> Option<LRESULT> {
        None
    }

    /// Paints the client area where the content has no window, `rc` in
    /// the pane window's coordinates, into `hdc` (a buffer the size of
    /// what is being repainted). By default the list's background.
    fn paint(&self, app: &App, hdc: HDC, rc: &RECT) {
        fill(hdc, rc, app.pal.get().bg);
    }

    /// The theme changed (`dark` or light); the pane repaints. Controls
    /// the content made need their colours or visual style set again.
    fn theme(&self, _app: &App, _dark: bool) {}

    /// The window's DPI changed: fonts and sizes are remade, then `place`
    /// follows.
    fn dpi(&self, _app: &App, _dpi: u32) {}

    /// Something happened that the content may follow.
    fn event(&self, _app: &App, _event: PaneEvent) {}

    /// The content as `Any`, so its feature can reach it again
    /// (`App::pane_content(id)` then `as_any().downcast_ref::<T>()`).
    #[allow(
        dead_code,
        reason = "the features' way back to their panes (FRAMEWORK.md)"
    )]
    fn as_any(&self) -> &dyn Any;
}

/// What the window tells the open panes (`PaneContent::event`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PaneEvent {
    /// The results' selection or focused row changed. They come in bursts
    /// (a click deselects one row and selects another): follow them with
    /// a short timer, not at once.
    SelectionChanged,
    /// A new search started (its text, options, filter or sort changed).
    SearchChanged,
    /// Rows came for the results on screen: a new search's (`refresh`
    /// false) or a refresh's, a live update's included (true).
    ResultsChanged { refresh: bool },
    /// The window came on screen (true), or was hidden or minimized
    /// (false): a pane holding anything costly lets it go meanwhile.
    WindowShown(bool),
}

/// A pane as a feature registers it.
pub struct PaneSpec {
    pub id: PaneId,
    /// Its name in its title row.
    pub title: &'static str,
    /// Where it goes, until the settings say otherwise (for Everything's
    /// panes, their `_side` and `_order` keys).
    pub side: Side,
    pub order: i32,
    /// The least height of its client area, below the title row, at 96
    /// DPI.
    pub min_high: i32,
    /// Makes the content as the pane opens; it is dropped as it closes.
    pub make: Box<dyn Fn() -> Rc<dyn PaneContent>>,
}

impl PaneSpec {
    /// A pane of `id` with Everything's name, side and order for it.
    pub fn new(id: PaneId, make: Box<dyn Fn() -> Rc<dyn PaneContent>>) -> PaneSpec {
        let d = id.default_layout();
        PaneSpec {
            id,
            title: id.title(),
            side: d.side,
            order: d.order,
            min_high: crate::dock::MIN_HIGH,
            make,
        }
    }
}

/// The pane window's class.
const CLASS: &str = "goz-gui pane";
/// A pane window's child id: this plus the pane's code.
const CHILD_ID: usize = 0x5000;

/// The pane window's own state, in its GWLP_USERDATA from WM_NCCREATE to
/// WM_NCDESTROY.
struct PaneWindow {
    id: PaneId,
    title: &'static str,
    content: Rc<dyn PaneContent>,
    /// The content made its windows (WM_CREATE).
    created: Cell<bool>,
    /// The mouse is over the close button; it was pressed on it.
    hot: Cell<bool>,
    pressed: Cell<bool>,
}

/// Registers the pane window's class (once, as the window starts).
pub fn register_class() {
    let class = wz(CLASS);
    // SAFETY: registers a class of this module with its window procedure.
    unsafe {
        let wc = WNDCLASSEXW {
            cbSize: size_of::<WNDCLASSEXW>() as u32,
            lpfnWndProc: Some(pane_proc),
            hInstance: GetModuleHandleW(core::ptr::null()),
            hCursor: LoadCursorW(core::ptr::null_mut(), IDC_ARROW),
            lpszClassName: class.as_ptr(),
            ..core::mem::zeroed()
        };
        RegisterClassExW(&wc);
    }
}

/// Makes the (hidden) window of pane `id` in the main window `parent`; its
/// content's `create` is the caller's to call.
pub fn create_window(
    parent: HWND,
    id: PaneId,
    title: &'static str,
    content: Rc<dyn PaneContent>,
) -> HWND {
    let state = Box::into_raw(Box::new(PaneWindow {
        id,
        title,
        content,
        created: Cell::new(false),
        hot: Cell::new(false),
        pressed: Cell::new(false),
    }));
    let class = wz(CLASS);
    let name = wz(title);
    // SAFETY: a child of this thread's main window; the state box goes to
    // the window at WM_NCCREATE and is freed at WM_NCDESTROY, or here if
    // the window could not be made.
    unsafe {
        let hwnd = CreateWindowExW(
            0,
            class.as_ptr(),
            name.as_ptr(),
            WS_CHILD | WS_CLIPCHILDREN | WS_CLIPSIBLINGS,
            0,
            0,
            0,
            0,
            parent,
            (CHILD_ID + id.code()) as HMENU,
            GetModuleHandleW(core::ptr::null()),
            state.cast(),
        );
        if hwnd.is_null() {
            drop(Box::from_raw(state));
        }
        hwnd
    }
}

/// The title row's height (0 with `pane_title=0`): the header's at 100 %
/// (the UI font's height and 12 px), so a pane's title row lines up with
/// the results' header beside it.
pub fn title_height(app: &App) -> i32 {
    if app.pane_titles() {
        app.ui_fonts.borrow().height + app.scale(12)
    } else {
        0
    }
}

/// The close button in a pane `width` wide whose title row is `title`
/// high: 19 px square at 100 %, 3 px from the right, centred in the row.
pub fn close_rect(width: i32, title: i32, scale: impl Fn(i32) -> i32) -> RECT {
    if title <= 0 {
        return RECT::default();
    }
    let side = scale(19).min(title - 2).max(0);
    let top = (title - side) / 2;
    let right = width - scale(3);
    RECT {
        left: right - side,
        top,
        right,
        bottom: top + side,
    }
}

fn state<'a>(hwnd: HWND) -> Option<&'a PaneWindow> {
    // SAFETY: GWLP_USERDATA holds this window's `PaneWindow` from
    // WM_NCCREATE to WM_NCDESTROY, and null before.
    unsafe { (GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *const PaneWindow).as_ref() }
}

fn client(hwnd: HWND) -> RECT {
    let mut rc = RECT::default();
    // SAFETY: reads this thread's window's size.
    unsafe { GetClientRect(hwnd, &mut rc) };
    rc
}

/// The content's place in a pane window: below the title row.
pub fn content_rect(app: &App, hwnd: HWND) -> RECT {
    let rc = client(hwnd);
    RECT {
        top: title_height(app).min(rc.bottom),
        ..rc
    }
}

fn point(l: LPARAM) -> POINT {
    POINT {
        x: (l & 0xffff) as i16 as i32,
        y: ((l >> 16) & 0xffff) as i16 as i32,
    }
}

fn inside(rc: &RECT, pt: POINT) -> bool {
    pt.x >= rc.left && pt.x < rc.right && pt.y >= rc.top && pt.y < rc.bottom
}

fn close_of(app: &App, hwnd: HWND) -> RECT {
    close_rect(client(hwnd).right, title_height(app), |v| app.scale(v))
}

fn invalidate_close(app: &App, hwnd: HWND) {
    let rc = close_of(app, hwnd);
    // SAFETY: repaints part of this thread's window.
    unsafe { InvalidateRect(hwnd, &rc, 0) };
}

/// Paints the title row and what of the client area shows, through a
/// buffer the size of what is being repainted.
fn paint(app: &App, hwnd: HWND, p: &PaneWindow) {
    // SAFETY: a standard double-buffered WM_PAINT; every GDI object made
    // here is released before returning. The buffer's origin is moved to
    // the invalid rectangle's corner, so drawing stays in the window's
    // coordinates.
    unsafe {
        let mut ps: PAINTSTRUCT = core::mem::zeroed();
        let hdc = BeginPaint(hwnd, &mut ps);
        let dirty = ps.rcPaint;
        let (w, h) = (dirty.right - dirty.left, dirty.bottom - dirty.top);
        if w <= 0 || h <= 0 {
            EndPaint(hwnd, &ps);
            return;
        }
        let mem = CreateCompatibleDC(hdc);
        let bmp = CreateCompatibleBitmap(hdc, w, h);
        let old_bmp = SelectObject(mem, bmp);
        SetViewportOrgEx(mem, -dirty.left, -dirty.top, core::ptr::null_mut());

        let rc = client(hwnd);
        let title_h = title_height(app);
        let title = RECT {
            bottom: title_h.min(rc.bottom),
            ..rc
        };
        if title_h > 0 {
            paint_title(app, hwnd, p, mem, &title);
        }
        let body = RECT {
            top: title.bottom,
            ..rc
        };
        if body.bottom > body.top {
            p.content.paint(app, mem, &body);
        }

        BitBlt(
            hdc, dirty.left, dirty.top, w, h, mem, dirty.left, dirty.top, SRCCOPY,
        );
        SelectObject(mem, old_bmp);
        DeleteObject(bmp);
        DeleteDC(mem);
        EndPaint(hwnd, &ps);
    }
}

/// The title row `rc`: the header's background and lines, the name, and
/// the close button.
fn paint_title(app: &App, hwnd: HWND, p: &PaneWindow, hdc: HDC, rc: &RECT) {
    let pal = app.pal.get();
    fill(hdc, rc, pal.header);
    let line = |top: i32| {
        fill(
            hdc,
            &RECT {
                top,
                bottom: top + 1,
                ..*rc
            },
            pal.line,
        );
    };
    line(rc.top);
    line(rc.bottom - 1);
    let close = close_of(app, hwnd);
    let pad = app.scale(4);
    let mut text_rc = RECT {
        left: rc.left + pad,
        right: close.left - pad,
        ..*rc
    };
    let mut text: Vec<u16> = p.title.encode_utf16().collect();
    // SAFETY: draws into the paint buffer; the old font is put back.
    unsafe {
        let old = SelectObject(hdc, app.ui_font());
        SetBkMode(hdc, TRANSPARENT as i32);
        SetTextColor(hdc, pal.fg);
        DrawTextW(
            hdc,
            text.as_mut_ptr(),
            text.len() as i32,
            &mut text_rc,
            DT_SINGLELINE | DT_VCENTER | DT_END_ELLIPSIS | DT_NOPREFIX,
        );
        SelectObject(hdc, old);
    }
    if close.right <= close.left {
        return;
    }
    let (hot, pressed) = (p.hot.get(), p.pressed.get());
    let bg = match (hot, pressed) {
        (true, true) => pal.header_pressed,
        (true, false) => pal.header_hot,
        _ => pal.header,
    };
    fill(hdc, &close, bg);
    let fg = if hot { pal.glyph_hot } else { pal.glyph };
    let cx = (close.left + close.right) / 2;
    let cy = (close.top + close.bottom) / 2;
    searchbox::paint_cross(hdc, cx, cy, 4.0, app.dpi.get() as f32 / 96.0, fg, bg);
}

/// The mouse moved to `pt`: the close button lights while it is over it
/// (while held, only if it went down there).
fn mouse_move(app: &App, hwnd: HWND, p: &PaneWindow, pt: POINT) {
    let over = inside(&close_of(app, hwnd), pt);
    if over && !p.pressed.get() {
        let mut tme = TRACKMOUSEEVENT {
            cbSize: size_of::<TRACKMOUSEEVENT>() as u32,
            dwFlags: TME_LEAVE,
            hwndTrack: hwnd,
            dwHoverTime: 0,
        };
        // SAFETY: asks for WM_MOUSELEAVE on this thread's window.
        unsafe { TrackMouseEvent(&mut tme) };
    }
    if p.hot.replace(over) != over {
        invalidate_close(app, hwnd);
    }
}

unsafe extern "system" fn pane_proc(hwnd: HWND, msg: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    // SAFETY: each arm reads `w`/`l` as the message defines them;
    // GWLP_USERDATA is this window's `PaneWindow` (see `state`).
    unsafe {
        if msg == WM_NCCREATE {
            let cs = &*(l as *const CREATESTRUCTW);
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, cs.lpCreateParams as isize);
            return DefWindowProcW(hwnd, msg, w, l);
        }
        if msg == WM_NCDESTROY {
            let raw = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut PaneWindow;
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
            if !raw.is_null() {
                drop(Box::from_raw(raw));
            }
            return DefWindowProcW(hwnd, msg, w, l);
        }
        let (Some(p), Some(app)) = (state(hwnd), crate::window::app_of(GetParent(hwnd))) else {
            return DefWindowProcW(hwnd, msg, w, l);
        };
        match msg {
            // The content makes its windows as the pane's is made, before
            // any WM_SIZE places them.
            WM_CREATE => {
                p.content.create(app, hwnd);
                p.created.set(true);
                return 0;
            }
            WM_PAINT => {
                paint(app, hwnd, p);
                return 0;
            }
            WM_ERASEBKGND => return 1,
            WM_SIZE => {
                if p.created.get() {
                    p.content.place(app, content_rect(app, hwnd));
                }
                // The close button keeps to the right edge.
                let title = RECT {
                    bottom: title_height(app),
                    ..client(hwnd)
                };
                InvalidateRect(hwnd, &title, 0);
                return 0;
            }
            WM_MOUSEMOVE => {
                mouse_move(app, hwnd, p, point(l));
                return 0;
            }
            WM_MOUSELEAVE => {
                if !p.pressed.get() && p.hot.replace(false) {
                    invalidate_close(app, hwnd);
                }
                return 0;
            }
            WM_LBUTTONDOWN | WM_LBUTTONDBLCLK => {
                let pt = point(l);
                if inside(&close_of(app, hwnd), pt) {
                    p.pressed.set(true);
                    p.hot.set(true);
                    SetCapture(hwnd);
                    invalidate_close(app, hwnd);
                    return 0;
                }
                // The title row, or the client area where the content has
                // no window and does not want the click: the pane takes
                // the keyboard.
                if pt.y < title_height(app) {
                    app.focus_pane(p.id);
                    return 0;
                }
                if let Some(answer) = p.content.message(app, hwnd, msg, w, l) {
                    return answer;
                }
                app.focus_pane(p.id);
                return 0;
            }
            WM_LBUTTONUP if p.pressed.get() => {
                p.pressed.set(false);
                if GetCapture() == hwnd {
                    ReleaseCapture();
                }
                let over = inside(&close_of(app, hwnd), point(l));
                p.hot.set(over);
                invalidate_close(app, hwnd);
                if over {
                    // Closed from the window's own queue: this window
                    // goes, which it must not do inside its own message.
                    PostMessageW(app.hwnd, crate::window::WM_PANE_CLOSE, p.id.code(), 0);
                }
                return 0;
            }
            WM_CAPTURECHANGED => {
                if p.pressed.replace(false) {
                    p.hot.set(false);
                    invalidate_close(app, hwnd);
                }
                return 0;
            }
            WM_SETFOCUS => {
                if let Some(target) = p.content.focus().filter(|&t| t != hwnd) {
                    SetFocus(target);
                    return 0;
                }
            }
            _ => {}
        }
        if let Some(answer) = p.content.message(app, hwnd, msg, w, l) {
            return answer;
        }
        match msg {
            // The content's controls in the list's colours, unless it said
            // otherwise.
            WM_CTLCOLOREDIT | WM_CTLCOLORLISTBOX | WM_CTLCOLORSTATIC | WM_CTLCOLORBTN => {
                let pal = app.pal.get();
                SetTextColor(w as HDC, pal.fg);
                SetBkColor(w as HDC, pal.bg);
                SetDCBrushColor(w as HDC, pal.bg);
                GetStockObject(DC_BRUSH) as LRESULT
            }
            _ => DefWindowProcW(hwnd, msg, w, l),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_close_button_sits_at_the_title_rows_right() {
        let id = |v: i32| v;
        // 256 px wide, a 27 px title row at 100 %: Everything's 19 px
        // button 3 px from the right edge, centred.
        let rc = close_rect(256, 27, id);
        assert_eq!((rc.left, rc.top, rc.right, rc.bottom), (234, 4, 253, 23));
        // 150 %.
        let rc = close_rect(384, 41, |v| v * 3 / 2);
        assert_eq!((rc.right - rc.left, rc.bottom - rc.top), (28, 28));
        assert_eq!((rc.right, rc.top), (380, 6));
        // No title row: no button.
        let rc = close_rect(256, 0, id);
        assert!(rc.right <= rc.left);
        // A title row too short for the button: it fits inside.
        let rc = close_rect(256, 12, id);
        assert_eq!(rc.bottom - rc.top, 10);
    }
}
