//! Windows as handles: their place, text, state, focus and timers, and
//! goz-gui's own messages to them. Re-exported at [`super`]'s root, so
//! these read `win::set_focus(edit)`.

use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows_sys::Win32::Graphics::Gdi::{
    ClientToScreen, InvalidateRect, MapWindowPoints, REDRAW_WINDOW_FLAGS, RedrawWindow,
    ScreenToClient, UpdateWindow,
};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    EnableWindow, GetActiveWindow, GetCapture, GetFocus, IsWindowEnabled, ReleaseCapture,
    SetActiveWindow, SetCapture, SetFocus, TME_LEAVE, TRACKMOUSEEVENT, TrackMouseEvent,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    BringWindowToTop, DestroyWindow, GA_ROOT, GET_WINDOW_CMD, GWL_EXSTYLE, GWL_STYLE, GWLP_ID,
    GetAncestor, GetClientRect, GetDlgCtrlID, GetForegroundWindow, GetParent, GetWindow,
    GetWindowLongPtrW, GetWindowPlacement, GetWindowRect, GetWindowTextLengthW, GetWindowTextW,
    GetWindowThreadProcessId, IsChild, IsIconic, IsWindow, IsWindowVisible, IsZoomed, KillTimer,
    MoveWindow, PostMessageW, SET_WINDOW_POS_FLAGS, SHOW_WINDOW_CMD, SetForegroundWindow, SetTimer,
    SetWindowLongPtrW, SetWindowPlacement, SetWindowPos, SetWindowTextW, ShowWindow,
    WINDOWPLACEMENT, WM_APP, WM_CLOSE, WM_GETFONT, WM_SETFONT, WM_SETREDRAW, WindowFromPoint,
};

use super::msg::APP_END;

/// The window's text, such as an edit box's.
pub fn text(hwnd: HWND) -> String {
    String::from_utf16_lossy(&text_wide(hwnd))
}

/// The window's text as UTF-16, without a NUL.
pub fn text_wide(hwnd: HWND) -> Vec<u16> {
    // SAFETY: no pointers.
    let len = unsafe { GetWindowTextLengthW(hwnd) };
    let Ok(len) = usize::try_from(len) else {
        return Vec::new();
    };
    if len == 0 {
        return Vec::new();
    }
    let mut buf = vec![0u16; len + 1];
    // SAFETY: the buffer and its length in units.
    let n = unsafe { GetWindowTextW(hwnd, buf.as_mut_ptr(), buf.len() as i32) };
    buf.truncate(usize::try_from(n).unwrap_or(0).min(len));
    buf
}

/// The length of the window's text in UTF-16 units.
pub fn text_len(hwnd: HWND) -> usize {
    // SAFETY: no pointers.
    usize::try_from(unsafe { GetWindowTextLengthW(hwnd) }).unwrap_or(0)
}

/// Sets the window's text; false if it could not.
pub fn set_text(hwnd: HWND, text: &str) -> bool {
    set_text_wide(hwnd, &super::wz(text))
}

/// Sets the window's text from UTF-16 (a NUL is added if missing).
pub fn set_text_wide(hwnd: HWND, text: &[u16]) -> bool {
    let owned;
    let z = if text.last() == Some(&0) {
        text
    } else {
        owned = super::wz_units(text);
        &owned
    };
    // SAFETY: a NUL-terminated string that outlives the call.
    unsafe { SetWindowTextW(hwnd, z.as_ptr()) != 0 }
}

/// The window's rectangle on the screen; empty if it is not there.
pub fn window_rect(hwnd: HWND) -> RECT {
    let mut rc = RECT::default();
    // SAFETY: an out-struct.
    unsafe { GetWindowRect(hwnd, &mut rc) };
    rc
}

/// The window's client area (left and top 0); empty if it is not there.
pub fn client_rect(hwnd: HWND) -> RECT {
    let mut rc = RECT::default();
    // SAFETY: an out-struct.
    unsafe { GetClientRect(hwnd, &mut rc) };
    rc
}

/// `pt`, in `hwnd`'s client coordinates, on the screen.
pub fn client_to_screen(hwnd: HWND, pt: POINT) -> POINT {
    let mut p = pt;
    // SAFETY: an in-out struct.
    unsafe { ClientToScreen(hwnd, &mut p) };
    p
}

/// `pt`, on the screen, in `hwnd`'s client coordinates.
pub fn screen_to_client(hwnd: HWND, pt: POINT) -> POINT {
    let mut p = pt;
    // SAFETY: an in-out struct.
    unsafe { ScreenToClient(hwnd, &mut p) };
    p
}

/// `rc`, in `from`'s client coordinates (the screen's for null), in `to`'s.
pub fn map_rect(from: HWND, to: HWND, rc: RECT) -> RECT {
    let mut pts = [
        POINT {
            x: rc.left,
            y: rc.top,
        },
        POINT {
            x: rc.right,
            y: rc.bottom,
        },
    ];
    // SAFETY: two points, the count passed.
    unsafe { MapWindowPoints(from, to, pts.as_mut_ptr(), 2) };
    RECT {
        left: pts[0].x,
        top: pts[0].y,
        right: pts[1].x,
        bottom: pts[1].y,
    }
}

/// `pt`, in `from`'s client coordinates (the screen's for null), in `to`'s.
pub fn map_point(from: HWND, to: HWND, pt: POINT) -> POINT {
    let mut p = pt;
    // SAFETY: one point, the count passed.
    unsafe { MapWindowPoints(from, to, &mut p, 1) };
    p
}

/// Marks `rc` of the window (all of it for `None`) to be painted.
pub fn invalidate(hwnd: HWND, rc: Option<&RECT>, erase: bool) {
    let rc = rc.map_or(core::ptr::null(), |r| r as *const RECT);
    // SAFETY: a rectangle or null, read during the call.
    unsafe { InvalidateRect(hwnd, rc, erase.into()) };
}

/// RedrawWindow for `rc` of the window (all of it for `None`), no region.
pub fn redraw(hwnd: HWND, rc: Option<&RECT>, flags: REDRAW_WINDOW_FLAGS) {
    let rc = rc.map_or(core::ptr::null(), |r| r as *const RECT);
    // SAFETY: a rectangle or null, read during the call.
    unsafe { RedrawWindow(hwnd, rc, core::ptr::null_mut(), flags) };
}

/// Paints what is waiting to be painted, now.
pub fn update(hwnd: HWND) {
    // SAFETY: no pointers.
    unsafe { UpdateWindow(hwnd) };
}

/// SetWindowPos; false if it failed.
pub fn set_pos(
    hwnd: HWND,
    after: HWND,
    (x, y, cx, cy): (i32, i32, i32, i32),
    flags: SET_WINDOW_POS_FLAGS,
) -> bool {
    // SAFETY: no pointers.
    unsafe { SetWindowPos(hwnd, after, x, y, cx, cy, flags) != 0 }
}

/// MoveWindow: `rc` in the parent's client coordinates (the screen's for a
/// top-level window).
pub fn move_to(hwnd: HWND, rc: &RECT, repaint: bool) {
    // SAFETY: no pointers.
    unsafe {
        MoveWindow(
            hwnd,
            rc.left,
            rc.top,
            rc.right - rc.left,
            rc.bottom - rc.top,
            repaint.into(),
        )
    };
}

/// MoveWindow by position and size.
pub fn move_window(hwnd: HWND, x: i32, y: i32, cx: i32, cy: i32, repaint: bool) {
    // SAFETY: no pointers.
    unsafe { MoveWindow(hwnd, x, y, cx, cy, repaint.into()) };
}

/// ShowWindow; whether the window was visible before.
pub fn show(hwnd: HWND, cmd: SHOW_WINDOW_CMD) -> bool {
    // SAFETY: no pointers.
    unsafe { ShowWindow(hwnd, cmd) != 0 }
}

pub fn is_window(hwnd: HWND) -> bool {
    // SAFETY: no pointers.
    !hwnd.is_null() && unsafe { IsWindow(hwnd) != 0 }
}

pub fn is_visible(hwnd: HWND) -> bool {
    // SAFETY: no pointers.
    unsafe { IsWindowVisible(hwnd) != 0 }
}

pub fn is_enabled(hwnd: HWND) -> bool {
    // SAFETY: no pointers.
    unsafe { IsWindowEnabled(hwnd) != 0 }
}

pub fn is_iconic(hwnd: HWND) -> bool {
    // SAFETY: no pointers.
    unsafe { IsIconic(hwnd) != 0 }
}

pub fn is_zoomed(hwnd: HWND) -> bool {
    // SAFETY: no pointers.
    unsafe { IsZoomed(hwnd) != 0 }
}

/// Whether `child` is a child (or a descendant) of `parent`.
pub fn is_child(parent: HWND, child: HWND) -> bool {
    // SAFETY: no pointers.
    unsafe { IsChild(parent, child) != 0 }
}

/// Enables or disables the window; whether it was disabled before.
pub fn enable(hwnd: HWND, on: bool) -> bool {
    // SAFETY: no pointers.
    unsafe { EnableWindow(hwnd, on.into()) != 0 }
}

/// Destroys the window; false if it could not.
pub fn destroy(hwnd: HWND) -> bool {
    // SAFETY: no pointers. What goz-gui keeps for a window (its handler,
    // its subclasses) is freed at WM_NCDESTROY, by the trampolines.
    unsafe { DestroyWindow(hwnd) != 0 }
}

/// Gives the window the keyboard; the window that had it.
pub fn set_focus(hwnd: HWND) -> HWND {
    // SAFETY: no pointers.
    unsafe { SetFocus(hwnd) }
}

/// The window of this thread with the keyboard (null for none).
pub fn focus() -> HWND {
    // SAFETY: no pointers.
    unsafe { GetFocus() }
}

pub fn parent(hwnd: HWND) -> HWND {
    // SAFETY: no pointers.
    unsafe { GetParent(hwnd) }
}

/// The top-level window `hwnd` is in (GA_ROOT).
pub fn root(hwnd: HWND) -> HWND {
    // SAFETY: no pointers.
    unsafe { GetAncestor(hwnd, GA_ROOT) }
}

/// GetWindow: a window related to `hwnd` (GW_CHILD, GW_HWNDNEXT, GW_OWNER…).
pub fn related(hwnd: HWND, cmd: GET_WINDOW_CMD) -> HWND {
    // SAFETY: no pointers.
    unsafe { GetWindow(hwnd, cmd) }
}

/// The window's control id.
pub fn ctrl_id(hwnd: HWND) -> i32 {
    // SAFETY: no pointers.
    unsafe { GetDlgCtrlID(hwnd) }
}

/// The first child of `parent` (the desktop's top-level windows for null)
/// after `after` whose class is `class` and whose title is `title` (`None`:
/// any); null if none.
pub fn find_child(parent: HWND, after: HWND, class: Option<&str>, title: Option<&str>) -> HWND {
    use windows_sys::Win32::UI::WindowsAndMessaging::FindWindowExW;
    let class = class.map(super::wz);
    let title = title.map(super::wz);
    let ptr = |s: &Option<Vec<u16>>| s.as_ref().map_or(core::ptr::null(), |v| v.as_ptr());
    // SAFETY: NUL-terminated strings or null, read during the call.
    unsafe { FindWindowExW(parent, after, ptr(&class), ptr(&title)) }
}

/// The children of `hwnd`, first to last (not their children).
pub fn children(hwnd: HWND) -> Vec<HWND> {
    use windows_sys::Win32::UI::WindowsAndMessaging::{GW_CHILD, GW_HWNDNEXT};
    let mut out = Vec::new();
    let mut child = related(hwnd, GW_CHILD);
    while !child.is_null() {
        out.push(child);
        child = related(child, GW_HWNDNEXT);
    }
    out
}

/// The window at `pt` on the screen.
pub fn from_point(pt: POINT) -> HWND {
    // SAFETY: no pointers.
    unsafe { WindowFromPoint(pt) }
}

pub fn foreground() -> HWND {
    // SAFETY: no pointers.
    unsafe { GetForegroundWindow() }
}

/// Brings the window forward; false if the system would not.
pub fn set_foreground(hwnd: HWND) -> bool {
    // SAFETY: no pointers.
    unsafe { SetForegroundWindow(hwnd) != 0 }
}

pub fn bring_to_top(hwnd: HWND) {
    // SAFETY: no pointers.
    unsafe { BringWindowToTop(hwnd) };
}

/// This thread's active window.
pub fn active() -> HWND {
    // SAFETY: no pointers.
    unsafe { GetActiveWindow() }
}

pub fn set_active(hwnd: HWND) -> HWND {
    // SAFETY: no pointers.
    unsafe { SetActiveWindow(hwnd) }
}

/// The process the window belongs to (0 if it is not there).
pub fn process_of(hwnd: HWND) -> u32 {
    let mut pid = 0u32;
    // SAFETY: an out-value.
    unsafe { GetWindowThreadProcessId(hwnd, &mut pid) };
    pid
}

/// Captures the mouse for the window; the window that had it.
pub fn set_capture(hwnd: HWND) -> HWND {
    // SAFETY: no pointers.
    unsafe { SetCapture(hwnd) }
}

pub fn capture() -> HWND {
    // SAFETY: no pointers.
    unsafe { GetCapture() }
}

pub fn release_capture() {
    // SAFETY: no pointers.
    unsafe { ReleaseCapture() };
}

/// Asks for WM_MOUSELEAVE once the mouse leaves the window.
pub fn track_leave(hwnd: HWND) {
    track_mouse(hwnd, TME_LEAVE, 0);
}

/// TrackMouseEvent for `hwnd` with `flags` (TME_LEAVE, TME_HOVER…) and a
/// hover time in ms (0 for the system's).
pub fn track_mouse(hwnd: HWND, flags: u32, hover_ms: u32) -> bool {
    let mut tme = TRACKMOUSEEVENT {
        cbSize: size_of::<TRACKMOUSEEVENT>() as u32,
        dwFlags: flags,
        hwndTrack: hwnd,
        dwHoverTime: if hover_ms == 0 { u32::MAX } else { hover_ms },
    };
    // SAFETY: an in-struct with its size.
    unsafe { TrackMouseEvent(&mut tme) != 0 }
}

/// The window's style (GWL_STYLE).
pub fn style(hwnd: HWND) -> u32 {
    // SAFETY: no pointers.
    unsafe { GetWindowLongPtrW(hwnd, GWL_STYLE) as u32 }
}

/// Sets the window's style; the old one.
pub fn set_style(hwnd: HWND, style: u32) -> u32 {
    // SAFETY: a style is a set of flags, no pointer.
    unsafe { SetWindowLongPtrW(hwnd, GWL_STYLE, style as isize) as u32 }
}

pub fn ex_style(hwnd: HWND) -> u32 {
    // SAFETY: no pointers.
    unsafe { GetWindowLongPtrW(hwnd, GWL_EXSTYLE) as u32 }
}

/// Sets the window's extended style; the old one.
pub fn set_ex_style(hwnd: HWND, style: u32) -> u32 {
    // SAFETY: a style is a set of flags, no pointer.
    unsafe { SetWindowLongPtrW(hwnd, GWL_EXSTYLE, style as isize) as u32 }
}

/// Sets a child window's control id (GWLP_ID).
pub fn set_ctrl_id(hwnd: HWND, id: i32) {
    // SAFETY: an id, no pointer.
    unsafe { SetWindowLongPtrW(hwnd, GWLP_ID, id as isize) };
}

/// The window's placement (normal position, state).
pub fn placement(hwnd: HWND) -> Option<WINDOWPLACEMENT> {
    let mut wp = WINDOWPLACEMENT {
        length: size_of::<WINDOWPLACEMENT>() as u32,
        ..Default::default()
    };
    // SAFETY: an out-struct with its size.
    (unsafe { GetWindowPlacement(hwnd, &mut wp) } != 0).then_some(wp)
}

pub fn set_placement(hwnd: HWND, wp: &WINDOWPLACEMENT) -> bool {
    let mut wp = *wp;
    wp.length = size_of::<WINDOWPLACEMENT>() as u32;
    // SAFETY: an in-struct with its size.
    unsafe { SetWindowPlacement(hwnd, &wp) != 0 }
}

/// Starts (or restarts) the window's timer `id`, due every `ms`; its
/// WM_TIMER goes to the window's procedure. False if it could not.
pub fn set_timer(hwnd: HWND, id: usize, ms: u32) -> bool {
    // SAFETY: no timer procedure: WM_TIMER goes to the window.
    unsafe { SetTimer(hwnd, id, ms, None) != 0 }
}

pub fn kill_timer(hwnd: HWND, id: usize) {
    // SAFETY: no pointers.
    unsafe { KillTimer(hwnd, id) };
}

/// The font the window draws its text in (WM_GETFONT); null for the
/// system's.
pub fn font(hwnd: HWND) -> windows_sys::Win32::Graphics::Gdi::HFONT {
    // SAFETY: WM_GETFONT has no parameters.
    unsafe { super::send_raw(hwnd, WM_GETFONT, 0, 0) as _ }
}

/// Sets the font the window draws its text in (WM_SETFONT). The window does
/// not own the font: it must outlive its use (GDI checks the handle).
pub fn set_font(hwnd: HWND, font: windows_sys::Win32::Graphics::Gdi::HFONT, redraw: bool) {
    // SAFETY: WM_SETFONT's parameters are a handle and a flag.
    unsafe { super::send_raw(hwnd, WM_SETFONT, font as usize, isize::from(redraw)) };
}

/// Turns the window's drawing on or off (WM_SETREDRAW).
pub fn set_redraw(hwnd: HWND, on: bool) {
    // SAFETY: WM_SETREDRAW's parameter is a flag.
    unsafe { super::send_raw(hwnd, WM_SETREDRAW, usize::from(on), 0) };
}

/// Asks the window to close (posts WM_CLOSE).
pub fn post_close(hwnd: HWND) -> bool {
    // SAFETY: WM_CLOSE has no parameters.
    unsafe { PostMessageW(hwnd, WM_CLOSE, 0, 0) != 0 }
}

/// Sends WM_CLOSE now.
pub fn close(hwnd: HWND) -> LRESULT {
    // SAFETY: WM_CLOSE has no parameters.
    unsafe { super::send_raw(hwnd, WM_CLOSE, 0, 0) }
}

/// Whether `msg` is one of goz-gui's private messages, which carry no
/// pointers (see the trust model in [`super`]).
fn is_app(msg: u32) -> bool {
    (WM_APP..APP_END).contains(&msg)
}

/// Posts goz-gui's private message `msg` (`WM_APP` up to the registered
/// messages) to one of its windows; false if it could not (the window has
/// gone, the queue is full). A value that has to go with it goes as a
/// [`super::parcel`].
///
/// # Panics
/// If `msg` is not in that range.
pub fn post_app(hwnd: HWND, msg: u32, w: WPARAM, l: LPARAM) -> bool {
    assert!(is_app(msg), "not one of goz-gui's private messages");
    // SAFETY: goz-gui's own messages carry integers, never pointers.
    unsafe { PostMessageW(hwnd, msg, w, l) != 0 }
}

/// Sends goz-gui's private message `msg` to one of its windows and waits for
/// the answer.
///
/// # Panics
/// If `msg` is not one of goz-gui's private messages.
pub fn send_app(hwnd: HWND, msg: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    assert!(is_app(msg), "not one of goz-gui's private messages");
    // SAFETY: as in `post_app`.
    unsafe { super::send_raw(hwnd, msg, w, l) }
}

/// Posts a system message that carries only integers and handles: the
/// commands and notifications goz-gui sends itself (WM_COMMAND, WM_SYSCOMMAND,
/// WM_CLOSE, WM_NULL, keys).
///
/// # Panics
/// If `msg` is not one of those (see [`plain`]).
pub fn post_plain(hwnd: HWND, msg: u32, w: WPARAM, l: LPARAM) -> bool {
    assert!(plain(msg), "message {msg:#x} may carry pointers");
    // SAFETY: the message's parameters are integers or handles.
    unsafe { PostMessageW(hwnd, msg, w, l) != 0 }
}

/// Sends a system message that carries only integers and handles.
///
/// # Panics
/// If `msg` is not one of those (see [`plain`]).
pub fn send_plain(hwnd: HWND, msg: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    assert!(plain(msg), "message {msg:#x} may carry pointers");
    // SAFETY: the message's parameters are integers or handles.
    unsafe { super::send_raw(hwnd, msg, w, l) }
}

/// Whether `msg` is a system message whose parameters are only integers and
/// handles for every window class, so any values are sound to send.
pub fn plain(msg: u32) -> bool {
    use windows_sys::Win32::UI::WindowsAndMessaging as wm;
    matches!(
        msg,
        wm::WM_NULL
            | wm::WM_CLOSE
            | wm::WM_COMMAND
            | wm::WM_SYSCOMMAND
            | wm::WM_KEYDOWN
            | wm::WM_KEYUP
            | wm::WM_CHAR
            | wm::WM_SYSKEYDOWN
            | wm::WM_SYSKEYUP
            | wm::WM_SETREDRAW
            | wm::WM_SETFONT
            | wm::WM_GETFONT
            | wm::WM_SETICON
            | wm::WM_CUT
            | wm::WM_COPY
            | wm::WM_PASTE
            | wm::WM_CLEAR
            | wm::WM_UNDO
            | wm::WM_NEXTDLGCTL
            | wm::WM_CHANGEUISTATE
            | wm::WM_UPDATEUISTATE
            | wm::WM_QUERYUISTATE
            | wm::WM_CANCELMODE
            | wm::WM_ENABLE
            | wm::WM_ACTIVATE
            | wm::WM_SIZE
            | wm::WM_MOVE
            | wm::WM_MOUSEMOVE
            | wm::WM_LBUTTONDOWN
            | wm::WM_LBUTTONUP
            | wm::WM_LBUTTONDBLCLK
            | wm::WM_RBUTTONDOWN
            | wm::WM_RBUTTONUP
            | wm::WM_MBUTTONDOWN
            | wm::WM_MBUTTONUP
            | wm::WM_MOUSEWHEEL
            | wm::WM_MOUSEHWHEEL
            | wm::WM_HSCROLL
            | wm::WM_VSCROLL
            | wm::WM_CONTEXTMENU
            | wm::WM_NCHITTEST
            | wm::WM_NCLBUTTONDOWN
            | wm::WM_SETFOCUS
            | wm::WM_KILLFOCUS
            | wm::WM_SHOWWINDOW
            | wm::WM_INITMENUPOPUP
            | wm::WM_DWMCOLORIZATIONCOLORCHANGED
            | wm::WM_THEMECHANGED
            | wm::WM_SYSCOLORCHANGE
            | wm::WM_DISPLAYCHANGE
            | wm::WM_INPUTLANGCHANGEREQUEST
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        WM_COMMAND, WM_GETTEXT, WM_NOTIFY, WM_SETTEXT, WM_TIMER,
    };

    #[test]
    fn messages_with_pointers_are_not_plain() {
        assert!(!plain(WM_NOTIFY));
        assert!(!plain(WM_SETTEXT));
        assert!(!plain(WM_GETTEXT));
        // Its lParam is a timer procedure that DispatchMessage would call.
        assert!(!plain(WM_TIMER));
        assert!(plain(WM_COMMAND));
        assert!(is_app(WM_APP + 5));
        assert!(!is_app(APP_END));
    }
}
