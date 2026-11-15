//! Windows as handles: their place, text, state, focus and timers, and
//! goz-gui's own messages to them. Re-exported at [`super`]'s root, so
//! these read `win::set_focus(edit)`.

use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows_sys::Win32::Graphics::Dwm::DwmSetWindowAttribute;
use windows_sys::Win32::Graphics::Gdi::{
    ClientToScreen, InvalidateRect, MapWindowPoints, REDRAW_WINDOW_FLAGS, RedrawWindow,
    ScreenToClient,
};
use windows_sys::Win32::System::DataExchange::COPYDATASTRUCT;
use windows_sys::Win32::UI::HiDpi::AdjustWindowRectExForDpi;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    EnableWindow, GetActiveWindow, GetCapture, GetFocus, IsWindowEnabled, ReleaseCapture,
    SetCapture, SetFocus, TME_LEAVE, TRACKMOUSEEVENT, TrackMouseEvent,
};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{RegisterHotKey, UnregisterHotKey};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    BeginDeferWindowPos, DeferWindowPos, EndDeferWindowPos, RegisterWindowMessageW,
    SMTO_ABORTIFHUNG, SendMessageTimeoutW, SetParent, WINDOWPOS, WM_COPYDATA, WM_SETCURSOR,
    WM_WINDOWPOSCHANGED,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    DestroyWindow, GET_WINDOW_CMD, GWL_EXSTYLE, GWL_STYLE, GetClientRect, GetDlgCtrlID,
    GetForegroundWindow, GetParent, GetWindow, GetWindowLongPtrW, GetWindowPlacement,
    GetWindowRect, GetWindowTextLengthW, GetWindowTextW, GetWindowThreadProcessId, IsChild,
    IsIconic, IsWindow, IsWindowVisible, IsZoomed, KillTimer, MoveWindow, PostMessageW,
    SET_WINDOW_POS_FLAGS, SHOW_WINDOW_CMD, SetForegroundWindow, SetTimer, SetWindowLongPtrW,
    SetWindowPlacement, SetWindowPos, SetWindowTextW, ShowWindow, WINDOWPLACEMENT, WM_APP,
    WM_CLOSE, WM_GETFONT, WM_SETFONT, WM_SETREDRAW, WindowFromPoint,
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

/// A window and where it goes: x, y, width, height.
pub type WindowMove = (HWND, (i32, i32, i32, i32));

/// Moves each window of `moves` to its place together
/// (BeginDeferWindowPos, DeferWindowPos, EndDeferWindowPos), with SWP_*
/// `flags`, so they repaint once. False when the system could not (out of
/// resources): then none, or not all, moved.
pub fn defer_pos(moves: &[WindowMove], flags: SET_WINDOW_POS_FLAGS) -> bool {
    let n = super::len_i32(moves.len());
    // SAFETY: the structure the system makes for the moves stays in here:
    // each DeferWindowPos hands back the one to go on with (null when it
    // failed, and then the structure is gone and not used again), and the
    // last is ended once. The windows are handles the system checks.
    unsafe {
        let mut hdwp = BeginDeferWindowPos(n);
        for &(hwnd, (x, y, cx, cy)) in moves {
            if hdwp.is_null() {
                break;
            }
            hdwp = DeferWindowPos(hdwp, hwnd, core::ptr::null_mut(), x, y, cx, cy, flags);
        }
        !hdwp.is_null() && EndDeferWindowPos(hdwp) != 0
    }
}

/// The window rectangle whose client area is `rc`, for these styles at
/// `dpi` (AdjustWindowRectExForDpi).
pub fn adjust_rect_for_dpi(rc: RECT, style: u32, menu: bool, ex_style: u32, dpi: u32) -> RECT {
    let mut r = rc;
    // SAFETY: an in-out struct.
    unsafe { AdjustWindowRectExForDpi(&mut r, style, menu.into(), ex_style, dpi) };
    r
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

/// Whether `hwnd` is a window that is there.
pub fn is_window(hwnd: HWND) -> bool {
    // SAFETY: no pointers.
    !hwnd.is_null() && unsafe { IsWindow(hwnd) != 0 }
}

/// Whether the window and its ancestors have WS_VISIBLE.
pub fn is_visible(hwnd: HWND) -> bool {
    // SAFETY: no pointers.
    unsafe { IsWindowVisible(hwnd) != 0 }
}

/// Whether the window takes input.
pub fn is_enabled(hwnd: HWND) -> bool {
    // SAFETY: no pointers.
    unsafe { IsWindowEnabled(hwnd) != 0 }
}

/// Whether the window is minimized.
pub fn is_iconic(hwnd: HWND) -> bool {
    // SAFETY: no pointers.
    unsafe { IsIconic(hwnd) != 0 }
}

/// Whether the window is maximized.
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

/// Makes `parent` the window's parent (HWND_MESSAGE: a message-only
/// window); the parent before, null if it failed.
pub fn set_parent(hwnd: HWND, parent: HWND) -> HWND {
    // SAFETY: no pointers; both handles are checked by the system.
    unsafe { SetParent(hwnd, parent) }
}

/// The window's parent (or owner, for a top-level window); null for none.
pub fn parent(hwnd: HWND) -> HWND {
    // SAFETY: no pointers.
    unsafe { GetParent(hwnd) }
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

/// The window the user works in now.
pub fn foreground() -> HWND {
    // SAFETY: no pointers.
    unsafe { GetForegroundWindow() }
}

/// Brings the window forward; false if the system would not.
pub fn set_foreground(hwnd: HWND) -> bool {
    // SAFETY: no pointers.
    unsafe { SetForegroundWindow(hwnd) != 0 }
}

/// This thread's active window.
pub fn active() -> HWND {
    // SAFETY: no pointers.
    unsafe { GetActiveWindow() }
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

/// The window of this thread that has the mouse captured (null: none).
pub fn capture() -> HWND {
    // SAFETY: no pointers.
    unsafe { GetCapture() }
}

/// Lets the mouse go (the capture).
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

/// The window's WS_EX_* styles.
pub fn ex_style(hwnd: HWND) -> u32 {
    // SAFETY: no pointers.
    unsafe { GetWindowLongPtrW(hwnd, GWL_EXSTYLE) as u32 }
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

/// Puts the window where `wp` says (its size is set here).
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

/// Stops the window's timer `id`.
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

/// DwmSetWindowAttribute of a 4-byte value (a BOOL, a COLORREF, an enum):
/// DWMWA_USE_IMMERSIVE_DARK_MODE (20) and the like. False if DWM refused
/// it (a window that is not there, an attribute that takes another size).
pub fn set_dwm_attribute(hwnd: HWND, attribute: u32, value: i32) -> bool {
    // SAFETY: DWM reads `cbAttribute` bytes from the pointer, here the 4 of
    // `value`, during the call; no window attribute's value is a pointer.
    unsafe {
        DwmSetWindowAttribute(
            hwnd,
            attribute,
            (&raw const value).cast(),
            size_of::<i32>() as u32,
        ) >= 0
    }
}

/// Registers hotkey `id` of `hwnd` (WM_HOTKEY): MOD_* `mods` and key `vk`;
/// false if another program has it.
pub fn register_hotkey(hwnd: HWND, id: i32, mods: u32, vk: u32) -> bool {
    // SAFETY: no pointers.
    unsafe { RegisterHotKey(hwnd, id, mods, vk) != 0 }
}

/// Unregisters hotkey `id` of `hwnd` (one not registered is fine).
pub fn unregister_hotkey(hwnd: HWND, id: i32) {
    // SAFETY: no pointers.
    unsafe { UnregisterHotKey(hwnd, id) };
}

/// Sends WM_WINDOWPOSCHANGED to `hwnd` with a copy of `wp`, as if it had
/// moved there (an owner-drawn list then measures its items again).
pub fn send_window_pos_changed(hwnd: HWND, wp: &WINDOWPOS) -> LRESULT {
    let mut pos = *wp;
    // SAFETY: WM_WINDOWPOSCHANGED (below WM_USER, the same for every
    // window) takes a WINDOWPOS of handles and integers: a copy, alive for
    // the call.
    unsafe { super::send_raw(hwnd, WM_WINDOWPOSCHANGED, 0, (&raw mut pos) as LPARAM) }
}

/// Sends WM_SETCURSOR to `hwnd` as a mouse message `mouse` over hit-test
/// area `hit` of window `over` would; whether the cursor was set.
pub fn send_set_cursor(hwnd: HWND, over: HWND, hit: u16, mouse: u16) -> LRESULT {
    // SAFETY: WM_SETCURSOR (below WM_USER, the same for every window) takes
    // a window handle and two words, no pointer.
    unsafe {
        super::send_raw(
            hwnd,
            WM_SETCURSOR,
            over as usize,
            super::make_lparam(hit, mouse),
        )
    }
}

/// Sends `to` a WM_COPYDATA from `from` with `data` (its dwData) and a copy
/// of `bytes`, giving up after `timeout_ms` or at once if `to` hangs
/// (SMTO_ABORTIFHUNG): its answer, `None` if it gave none.
pub fn send_copy_data(
    to: HWND,
    from: HWND,
    data: usize,
    bytes: &[u8],
    timeout_ms: u32,
) -> Option<usize> {
    let cds = COPYDATASTRUCT {
        dwData: data,
        cbData: super::len_u32(bytes.len()),
        lpData: bytes.as_ptr() as *mut core::ffi::c_void,
    };
    let mut answer = 0usize;
    // SAFETY: WM_COPYDATA's lParam is a COPYDATASTRUCT, here of `bytes`
    // (no more than its length); both outlive the call, and the system
    // copies them for a receiver on another thread before it returns.
    let ok = unsafe {
        SendMessageTimeoutW(
            to,
            WM_COPYDATA,
            from as usize,
            (&raw const cds) as LPARAM,
            SMTO_ABORTIFHUNG,
            timeout_ms,
            &mut answer,
        )
    } != 0;
    ok.then_some(answer)
}

/// The message registered as `name` (RegisterWindowMessageW), the same in
/// every process; 0 if it could not be.
pub fn register_message(name: &str) -> u32 {
    let name = super::wz(name);
    // SAFETY: a NUL-terminated name.
    unsafe { RegisterWindowMessageW(name.as_ptr()) }
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
