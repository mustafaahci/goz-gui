//! Edit > Find... (Ctrl+F): Everything's find bar, under the results. Its
//! box finds the next result whose name holds the text (`crate::find`) as
//! it is typed, and again with Enter, F3, Next and Previous (Shift+Enter,
//! Shift+F3); Highlight All lights the matches in the rows on screen; Match
//! Case; the × (or Esc) closes it.
//!
//! The rows the window holds are looked through first, here. The rest is
//! fetched from gozd on a thread of the find's own, over a connection of
//! its own, a stream of names only (`client::stream_range`): the window
//! never waits for it, it stops as soon as a match comes (or a new find,
//! a new search, closing the bar), and the bar shows how far it got.
//! Wrapping round the end and finding nothing say so in the status bar, in
//! Everything's words.

use std::any::Any;
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows_sys::Win32::Graphics::Gdi::{
    DC_BRUSH, DT_END_ELLIPSIS, DT_NOPREFIX, DT_SINGLELINE, DT_VCENTER, DrawTextW, GetDC,
    GetStockObject, HDC, InvalidateRect, ReleaseDC, SelectObject, SetBkColor, SetBkMode,
    SetDCBrushColor, SetTextColor, TRANSPARENT,
};
use windows_sys::Win32::System::Diagnostics::Debug::MessageBeep;
use windows_sys::Win32::UI::Controls::{
    BST_CHECKED, BST_UNCHECKED, EM_SETCUEBANNER, EM_SETSEL, LVIS_FOCUSED, LVIS_SELECTED,
    LVM_ENSUREVISIBLE, LVM_GETITEMCOUNT, LVM_GETNEXTITEM, LVM_SETSELECTIONMARK, LVNI_FOCUSED,
    WM_MOUSELEAVE,
};
use windows_sys::Win32::UI::HiDpi::GetSystemMetricsForDpi;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    GetActiveWindow, GetCapture, GetFocus, ReleaseCapture, SetCapture, SetFocus, TME_LEAVE,
    TRACKMOUSEEVENT, TrackMouseEvent, VK_BACK, VK_CONTROL, VK_DELETE, VK_ESCAPE, VK_RETURN,
    VK_SHIFT,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    BM_GETCHECK, BM_SETCHECK, BN_CLICKED, BS_AUTOCHECKBOX, CreateWindowExW, EN_CHANGE,
    ES_AUTOHSCROLL, GetWindowTextLengthW, HMENU, IsWindowVisible, KillTimer, MB_OK, MSG,
    PostMessageW, SM_CXMENUCHECK, SWP_HIDEWINDOW, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE,
    SWP_NOZORDER, SWP_SHOWWINDOW, SetTimer, SetWindowPos, WM_CAPTURECHANGED, WM_CHAR, WM_COMMAND,
    WM_CTLCOLOREDIT, WM_CTLCOLORSTATIC, WM_KEYDOWN, WM_LBUTTONDBLCLK, WM_LBUTTONDOWN, WM_LBUTTONUP,
    WM_MOUSEMOVE, WM_RBUTTONDOWN, WM_RBUTTONUP, WM_SETFONT, WM_SYSCHAR, WM_SYSKEYDOWN, WS_CHILD,
    WS_TABSTOP,
};

use super::{App, Fonts, key_down, send, slots};
use crate::client::{self, RowRange, Search, Stop, Streamed};
use crate::find::{self, Gate, Needle, Part, Side, Walked};
use crate::highlight::Span;
use crate::paint::fill;
use crate::strip::{Strip, StripSlot};
use crate::theme::{self, rgb};
use crate::wide::{window_text, wz};
use crate::{hover, searchbox};

/// The find bar's controls, children of the main window (F4's ids).
const ID_EDIT: usize = 0x5200;
const ID_HIGHLIGHT: usize = 0x5201;
const ID_CASE: usize = 0x5202;

/// A find's thread reports here: a `Box<Reply>` in lParam.
pub(super) const WM_FIND: u32 = slots::F4_MSG;
/// The text has stopped changing for a moment: find it.
pub(super) const TIMER_TYPED: usize = slots::F4_TIMER;
const TYPED_MS: u32 = 150;
/// How often a find's thread says how far it got, at most.
const PROGRESS_EVERY: Duration = Duration::from_millis(120);

/// A button the bar draws itself.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Button {
    Previous,
    Next,
    Close,
}

/// Where the bar's parts went in the last layout (client coordinates).
#[derive(Clone, Copy, Default)]
struct Geometry {
    frame: RECT,
    previous: RECT,
    next: RECT,
    close: RECT,
    /// Where the bar says how far a find got.
    status: RECT,
}

fn inside(rc: &RECT, pt: POINT) -> bool {
    pt.x >= rc.left && pt.x < rc.right && pt.y >= rc.top && pt.y < rc.bottom
}

fn empty(rc: &RECT) -> bool {
    rc.right <= rc.left || rc.bottom <= rc.top
}

/// The find bar: its controls (made the first time it shows), the buttons
/// it draws, and what it says.
pub(super) struct FindBar {
    edit: Cell<HWND>,
    highlight: Cell<HWND>,
    case: Cell<HWND>,
    visible: Cell<bool>,
    geo: Cell<Geometry>,
    hot: Cell<Option<Button>>,
    pressed: Cell<Option<Button>>,
    /// How far a find's thread got, while one runs.
    progress: RefCell<Option<String>>,
    /// The last find found nothing: the box goes red, as Everything's.
    not_found: Cell<bool>,
}

impl FindBar {
    pub(super) fn new() -> FindBar {
        FindBar {
            edit: Cell::new(core::ptr::null_mut()),
            highlight: Cell::new(core::ptr::null_mut()),
            case: Cell::new(core::ptr::null_mut()),
            visible: Cell::new(false),
            geo: Cell::new(Geometry::default()),
            hot: Cell::new(None),
            pressed: Cell::new(None),
            progress: RefCell::new(None),
            not_found: Cell::new(false),
        }
    }

    pub(super) fn visible(&self) -> bool {
        self.visible.get()
    }

    fn controls(&self) -> [HWND; 3] {
        [self.edit.get(), self.highlight.get(), self.case.get()]
    }

    /// The text to find.
    pub(super) fn text(&self) -> String {
        let edit = self.edit.get();
        if edit.is_null() {
            String::new()
        } else {
            window_text(edit)
        }
    }

    fn has_text(&self) -> bool {
        let edit = self.edit.get();
        // SAFETY: reads the length of this thread's box.
        !edit.is_null() && unsafe { GetWindowTextLengthW(edit) } > 0
    }

    fn checked(&self, hwnd: HWND) -> bool {
        !hwnd.is_null() && send(hwnd, BM_GETCHECK, 0, 0) == BST_CHECKED as isize
    }

    /// Makes the controls, hidden, the first time the bar shows.
    fn create(&self, app: &App, highlight_all: bool, match_case: bool) {
        if !self.edit.get().is_null() {
            return;
        }
        // SAFETY: creates child controls of this thread's main window; the
        // strings live across the calls.
        unsafe {
            let make = |class: &str, text: &str, style: u32, id: usize| {
                CreateWindowExW(
                    0,
                    wz(class).as_ptr(),
                    wz(text).as_ptr(),
                    WS_CHILD | WS_TABSTOP | style,
                    0,
                    0,
                    0,
                    0,
                    app.hwnd,
                    id as HMENU,
                    core::ptr::null_mut(),
                    core::ptr::null(),
                )
            };
            self.edit
                .set(make("EDIT", "", ES_AUTOHSCROLL as u32, ID_EDIT));
            self.highlight.set(make(
                "BUTTON",
                "Highlight &All",
                BS_AUTOCHECKBOX as u32,
                ID_HIGHLIGHT,
            ));
            self.case.set(make(
                "BUTTON",
                "Match &Case",
                BS_AUTOCHECKBOX as u32,
                ID_CASE,
            ));
        }
        let cue = wz("Find in results");
        send(self.edit.get(), EM_SETCUEBANNER, 1, cue.as_ptr() as isize);
        for (hwnd, on) in [
            (self.highlight.get(), highlight_all),
            (self.case.get(), match_case),
        ] {
            let state = if on { BST_CHECKED } else { BST_UNCHECKED };
            send(hwnd, BM_SETCHECK, state as usize, 0);
        }
        self.set_fonts(app);
        self.theme(app, app.pal.get().dark);
    }

    fn set_fonts(&self, app: &App) {
        let font = app.ui_font();
        for hwnd in self.controls() {
            if !hwnd.is_null() {
                send(hwnd, WM_SETFONT, font as usize, 1);
            }
        }
    }

    /// A check box's width for its label in the bar's font.
    fn check_width(app: &App, hwnd: HWND) -> i32 {
        let text: Vec<u16> = window_text(hwnd).replace('&', "").encode_utf16().collect();
        // SAFETY: a DC of this thread's window, measured in and given back
        // with its old font.
        let w = unsafe {
            let dc = GetDC(app.hwnd);
            let old = SelectObject(dc, app.ui_font());
            let w = crate::paint::width(dc, &text);
            SelectObject(dc, old);
            ReleaseDC(app.hwnd, dc);
            w
        };
        // SAFETY: reads a metric.
        let check = unsafe { GetSystemMetricsForDpi(SM_CXMENUCHECK, app.dpi.get()) };
        check + app.scale(6) + w
    }

    fn invalidate(&self, app: &App, rc: &RECT) {
        if !empty(rc) {
            // SAFETY: repaints part of this thread's window.
            unsafe { InvalidateRect(app.hwnd, rc, 0) };
        }
    }

    fn button_at(&self, pt: POINT) -> Option<Button> {
        let g = self.geo.get();
        [
            (Button::Previous, g.previous),
            (Button::Next, g.next),
            (Button::Close, g.close),
        ]
        .into_iter()
        .find(|(_, rc)| inside(rc, pt))
        .map(|(b, _)| b)
    }

    fn button_rect(&self, b: Button) -> RECT {
        let g = self.geo.get();
        match b {
            Button::Previous => g.previous,
            Button::Next => g.next,
            Button::Close => g.close,
        }
    }

    fn set_hot(&self, app: &App, hot: Option<Button>) {
        let old = self.hot.replace(hot);
        if old != hot {
            for b in [old, hot].into_iter().flatten() {
                self.invalidate(app, &self.button_rect(b));
            }
        }
    }

    /// The box's background: red when the last find found nothing.
    fn field_colour(&self, app: &App) -> u32 {
        let pal = app.pal.get();
        if self.not_found.get() && self.has_text() {
            if pal.dark {
                rgb(0x40, 0x00, 0x00)
            } else {
                rgb(0xff, 0xbf, 0xbf)
            }
        } else {
            pal.bg
        }
    }

    pub(super) fn set_not_found(&self, app: &App, on: bool) {
        if self.not_found.replace(on) != on {
            self.invalidate(app, &self.geo.get().frame);
            let edit = self.edit.get();
            if !edit.is_null() {
                // SAFETY: repaints this thread's box.
                unsafe { InvalidateRect(edit, core::ptr::null(), 1) };
            }
        }
    }

    pub(super) fn set_progress(&self, app: &App, text: Option<String>) {
        *self.progress.borrow_mut() = text;
        self.invalidate(app, &self.geo.get().status);
    }

    /// Shows or hides the bar's controls (a layout places them).
    fn show_controls(&self, show: bool) {
        for hwnd in self.controls() {
            if !hwnd.is_null() {
                let flag = if show { SWP_SHOWWINDOW } else { SWP_HIDEWINDOW };
                // SAFETY: shows or hides a control of this thread's window.
                unsafe {
                    SetWindowPos(
                        hwnd,
                        core::ptr::null_mut(),
                        0,
                        0,
                        0,
                        0,
                        flag | SWP_NOZORDER | SWP_NOACTIVATE | SWP_NOSIZE | SWP_NOMOVE,
                    )
                };
            }
        }
    }
}

/// Moves `hwnd` to `rc`, shown, or hides it when `rc` is empty.
fn put(hwnd: HWND, rc: RECT) {
    if hwnd.is_null() {
        return;
    }
    let show = if empty(&rc) {
        SWP_HIDEWINDOW
    } else {
        SWP_SHOWWINDOW
    };
    // SAFETY: moves a control of this thread's window.
    unsafe {
        SetWindowPos(
            hwnd,
            core::ptr::null_mut(),
            rc.left,
            rc.top,
            (rc.right - rc.left).max(0),
            (rc.bottom - rc.top).max(0),
            show | SWP_NOZORDER | SWP_NOACTIVATE,
        );
    }
}

/// Sizes along the bar, in pixels at the window's DPI.
struct Sizes {
    /// ▴ and ▾, each square.
    button: i32,
    /// Between the box and ▴, between the check boxes, and before the
    /// progress text.
    gap: i32,
    /// Before the first check box.
    wide_gap: i32,
    /// Under this the box would show a clipped bit of its cue.
    min_box: i32,
    /// Everything's 256 px.
    max_box: i32,
}

/// Where the bar's parts go, from its left: each a span (left, right) or
/// left out.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Arranged {
    frame: Option<(i32, i32)>,
    /// The left of ▴ (▾ follows it).
    arrows: Option<i32>,
    checks: [Option<(i32, i32)>; 2],
    /// Where the progress text may start (it ends at the limit).
    rest: i32,
}

/// Lays the bar out between `left` and `limit` (the ×'s side), given each
/// check box's width (`None`: not made yet).
///
/// The box comes first: it is the only thing to type into. ▴ ▾ and the
/// check boxes only repeat Enter, F3, Shift+F3 and the bar's options, so as
/// the bar narrows the check boxes go first (the last first), then the box
/// shrinks from Everything's 256 px down to its minimum, then ▴ ▾ go and
/// the box takes their room, and only under its minimum does the box go
/// too (its text still finds with F3), with everything else but the ×.
fn arrange(left: i32, limit: i32, z: &Sizes, checks: [Option<i32>; 2]) -> Arranged {
    let room = limit - left;
    let arrows_wide = z.gap + 2 * z.button;
    let (frame, arrows) = if room >= z.min_box + arrows_wide {
        let wide = z.max_box.min(room - arrows_wide);
        (Some((left, left + wide)), Some(left + wide + z.gap))
    } else if room >= z.min_box {
        (Some((left, left + z.max_box.min(room))), None)
    } else {
        (None, None)
    };
    let mut x = match (frame, arrows) {
        (_, Some(a)) => a + 2 * z.button + z.wide_gap,
        (Some((_, right)), None) => right + z.wide_gap,
        // No box: nothing else either.
        (None, _) => limit,
    };
    let mut placed = [None; 2];
    for (slot, wide) in placed.iter_mut().zip(checks) {
        let Some(wide) = wide else { continue };
        if x + wide <= limit {
            *slot = Some((x, x + wide));
            x += wide + z.gap;
        } else {
            // A check box that does not fit takes the ones after it along.
            x = limit;
        }
    }
    Arranged {
        frame,
        arrows,
        checks: placed,
        rest: x + z.gap,
    }
}

impl Strip for FindBar {
    fn height(&self, app: &App) -> i32 {
        if !self.visible.get() {
            return 0;
        }
        // Everything's 30 px at 100 % with the usual 15 px font.
        app.ui_fonts.borrow().height + app.scale(15)
    }

    fn place(&self, app: &App, rc: RECT) {
        if !self.visible.get() || empty(&rc) {
            self.geo.set(Geometry::default());
            return;
        }
        let s = |v: i32| app.scale(v);
        let fh = app.ui_fonts.borrow().height;
        let h = rc.bottom - rc.top;
        let button = fh + s(4);
        let box_h = fh + s(8);
        let mid = |size: i32| rc.top + (h - size + 1) / 2;
        let square = |left: i32| RECT {
            left,
            top: mid(button),
            right: left + button,
            bottom: mid(button) + button,
        };
        let close = square(rc.right - s(3) - button);
        let left = rc.left + s(3);
        let limit = close.left - s(8);
        // The box first, then ▴ ▾, then the check boxes, while they fit
        // before the × (`arrange`).
        let checks = [self.highlight.get(), self.case.get()]
            .map(|hwnd| (!hwnd.is_null()).then(|| FindBar::check_width(app, hwnd)));
        let sizes = Sizes {
            button,
            gap: s(4),
            wide_gap: s(6),
            min_box: s(48),
            max_box: s(256),
        };
        let row = arrange(left, limit, &sizes, checks);
        let frame = row.frame.map_or(RECT::default(), |(l, r)| RECT {
            left: l,
            top: mid(box_h),
            right: r,
            bottom: mid(box_h) + box_h,
        });
        let (previous, next) = row.arrows.map_or((RECT::default(), RECT::default()), |a| {
            (square(a), square(a + button))
        });
        let checks = row.checks.map(|span| {
            span.map_or(RECT::default(), |(l, r)| RECT {
                left: l,
                top: mid(fh + s(2)),
                right: r,
                bottom: mid(fh + s(2)) + fh + s(2),
            })
        });
        let status = RECT {
            left: row.rest,
            top: rc.top,
            right: limit,
            bottom: rc.bottom,
        };
        let edit = if empty(&frame) {
            RECT::default()
        } else {
            RECT {
                left: frame.left + s(4),
                top: frame.top + (box_h - fh) / 2,
                right: (frame.right - s(4)).max(frame.left + s(4)),
                bottom: frame.top + (box_h - fh) / 2 + fh,
            }
        };
        self.geo.set(Geometry {
            frame,
            previous,
            next,
            close,
            status: if status.right > status.left {
                status
            } else {
                RECT::default()
            },
        });
        let placed = [
            (self.edit.get(), edit),
            (self.highlight.get(), checks[0]),
            (self.case.get(), checks[1]),
        ];
        for (hwnd, rc) in placed {
            put(hwnd, rc);
        }
        // A control that went keeps no keyboard: it goes to the results
        // (now, or when the window is active again).
        let gone = |h: HWND| !h.is_null() && placed.iter().any(|&(c, rc)| c == h && empty(&rc));
        // SAFETY: reads the focus of this thread; it is only moved while
        // it is in this window (so the window is the active one).
        unsafe {
            if gone(GetFocus()) {
                SetFocus(app.list);
            }
        }
        if gone(app.last_focus.get()) {
            app.last_focus.set(app.list);
        }
    }

    fn paint(&self, app: &App, hdc: HDC, rc: &RECT) {
        let pal = app.pal.get();
        fill(hdc, rc, pal.chrome);
        let g = self.geo.get();
        if !empty(&g.frame) {
            fill(hdc, &g.frame, pal.border);
            let inner = RECT {
                left: g.frame.left + 1,
                top: g.frame.top + 1,
                right: g.frame.right - 1,
                bottom: g.frame.bottom - 1,
            };
            fill(hdc, &inner, self.field_colour(app));
        }
        let scale = app.dpi.get() as f32 / 96.0;
        let enabled = self.has_text();
        for b in [Button::Previous, Button::Next, Button::Close] {
            let r = self.button_rect(b);
            if empty(&r) {
                continue;
            }
            let usable = enabled || b == Button::Close;
            let hot = usable && self.hot.get() == Some(b);
            let pressed = hot && self.pressed.get() == Some(b);
            let bg = match (hot, pressed) {
                (true, true) => pal.header_pressed,
                (true, false) => pal.header_hot,
                _ => pal.chrome,
            };
            let fg = if !usable {
                pal.border
            } else if hot {
                pal.glyph_hot
            } else {
                pal.glyph
            };
            fill(hdc, &r, bg);
            let (cx, cy) = ((r.left + r.right) / 2, (r.top + r.bottom) / 2);
            match b {
                Button::Close => searchbox::paint_cross(hdc, cx, cy, 4.0, scale, fg, bg),
                _ => searchbox::paint_chevron(hdc, cx, cy, b == Button::Previous, scale, fg, bg),
            }
        }
        if let Some(text) = self.progress.borrow().as_deref()
            && !empty(&g.status)
        {
            let mut t: Vec<u16> = text.encode_utf16().collect();
            let mut r = g.status;
            // SAFETY: draws into the paint buffer; the old font is put
            // back.
            unsafe {
                let old = SelectObject(hdc, app.ui_font());
                SetBkMode(hdc, TRANSPARENT as i32);
                SetTextColor(hdc, pal.dim);
                DrawTextW(
                    hdc,
                    t.as_mut_ptr(),
                    t.len() as i32,
                    &mut r,
                    DT_SINGLELINE | DT_VCENTER | DT_END_ELLIPSIS | DT_NOPREFIX,
                );
                SelectObject(hdc, old);
            }
        }
    }

    fn mouse(&self, app: &App, msg: u32, _w: WPARAM, pt: POINT) -> bool {
        if !self.visible.get() {
            return false;
        }
        let row = app.strip_rect(StripSlot::FindBar);
        match msg {
            WM_MOUSEMOVE => {
                let over = inside(&row, pt).then(|| self.button_at(pt)).flatten();
                // While a button is held, it is lit only with the mouse on it.
                let hot = match self.pressed.get() {
                    Some(p) => (over == Some(p)).then_some(p),
                    None => over,
                };
                if hot.is_some() {
                    let mut tme = TRACKMOUSEEVENT {
                        cbSize: size_of::<TRACKMOUSEEVENT>() as u32,
                        dwFlags: TME_LEAVE,
                        hwndTrack: app.hwnd,
                        dwHoverTime: 0,
                    };
                    // SAFETY: asks for WM_MOUSELEAVE on this thread's window.
                    unsafe { TrackMouseEvent(&mut tme) };
                }
                self.set_hot(app, hot);
                false
            }
            WM_MOUSELEAVE => {
                if self.pressed.get().is_none() {
                    self.set_hot(app, None);
                }
                false
            }
            WM_CAPTURECHANGED => {
                if self.pressed.take().is_some() {
                    app.capture_strip(None);
                    self.set_hot(app, None);
                }
                false
            }
            WM_LBUTTONDOWN | WM_LBUTTONDBLCLK => {
                match self.button_at(pt) {
                    Some(b) if b == Button::Close || self.has_text() => {
                        self.pressed.set(Some(b));
                        self.set_hot(app, Some(b));
                        // SAFETY: takes the mouse for this thread's window.
                        unsafe { SetCapture(app.hwnd) };
                        app.capture_strip(Some(StripSlot::FindBar));
                        self.invalidate(app, &self.button_rect(b));
                    }
                    Some(_) => {}
                    None if inside(&self.geo.get().frame, pt) => {
                        // The frame round the text is the box too.
                        // SAFETY: focuses a control of this thread's window.
                        unsafe { SetFocus(self.edit.get()) };
                    }
                    None => {}
                }
                true
            }
            WM_LBUTTONUP => {
                let Some(b) = self.pressed.take() else {
                    return inside(&row, pt);
                };
                app.capture_strip(None);
                // SAFETY: lets go of the mouse this bar took.
                unsafe {
                    if GetCapture() == app.hwnd {
                        ReleaseCapture();
                    }
                }
                let over = self.button_at(pt) == Some(b);
                self.set_hot(app, over.then_some(b));
                self.invalidate(app, &self.button_rect(b));
                if over {
                    match b {
                        Button::Previous => app.find_next(false),
                        Button::Next => app.find_next(true),
                        Button::Close => app.close_find(),
                    }
                }
                true
            }
            WM_RBUTTONDOWN | WM_RBUTTONUP => inside(&row, pt),
            _ => false,
        }
    }

    fn focus(&self) -> Option<HWND> {
        let edit = self.edit.get();
        // Not while the bar is too narrow for its box (hidden).
        (self.visible.get() && !edit.is_null() && !empty(&self.geo.get().frame)).then_some(edit)
    }

    fn contains(&self, hwnd: HWND) -> bool {
        !hwnd.is_null() && self.controls().contains(&hwnd)
    }

    fn tab(&self, _app: &App, from: HWND, forward: bool) -> bool {
        // The box, then the check boxes that show.
        let stops: Vec<HWND> = self
            .controls()
            .into_iter()
            .filter(|&h| {
                // SAFETY: asks about this thread's controls.
                !h.is_null() && unsafe { IsWindowVisible(h) } != 0
            })
            .collect();
        let Some(at) = stops.iter().position(|&h| h == from) else {
            return false;
        };
        let next = if forward {
            stops.get(at + 1)
        } else {
            at.checked_sub(1).and_then(|i| stops.get(i))
        };
        match next {
            Some(&h) => {
                // SAFETY: focuses a control of this thread's window.
                unsafe { SetFocus(h) };
                true
            }
            None => false,
        }
    }

    fn message(&self, app: &App, msg: u32, w: WPARAM, _l: LPARAM) -> Option<LRESULT> {
        let pal = app.pal.get();
        match msg {
            WM_COMMAND => {
                let (id, code) = (w & 0xffff, ((w >> 16) & 0xffff) as u32);
                match (id, code) {
                    (ID_EDIT, EN_CHANGE) => app.find_text_changed(),
                    (ID_HIGHLIGHT | ID_CASE, BN_CLICKED) => app.find_options_changed(),
                    _ => {}
                }
                Some(0)
            }
            WM_CTLCOLOREDIT => {
                let colour = self.field_colour(app);
                // SAFETY: the DC the box is about to draw with; the DC
                // brush takes the colour set on that DC.
                unsafe {
                    SetTextColor(w as HDC, pal.fg);
                    SetBkColor(w as HDC, colour);
                    SetDCBrushColor(w as HDC, colour);
                    Some(GetStockObject(DC_BRUSH) as LRESULT)
                }
            }
            WM_CTLCOLORSTATIC => {
                // SAFETY: as above, for a check box on the bar's chrome.
                unsafe {
                    SetTextColor(w as HDC, pal.fg);
                    SetBkColor(w as HDC, pal.chrome);
                    SetDCBrushColor(w as HDC, pal.chrome);
                    Some(GetStockObject(DC_BRUSH) as LRESULT)
                }
            }
            _ => None,
        }
    }

    fn key(&self, app: &App, msg: &MSG) -> bool {
        let vk = msg.wParam as u16;
        let on_edit = msg.hwnd == self.edit.get();
        match msg.message {
            WM_KEYDOWN => match vk {
                VK_RETURN if !key_down(VK_CONTROL) => {
                    app.find_next(!key_down(VK_SHIFT));
                    true
                }
                VK_ESCAPE if !key_down(VK_SHIFT) => {
                    app.close_find();
                    true
                }
                VK_BACK if on_edit && key_down(VK_CONTROL) => {
                    super::edit::delete_word(msg.hwnd, true);
                    true
                }
                VK_DELETE if on_edit && key_down(VK_CONTROL) && !key_down(VK_SHIFT) => {
                    super::edit::delete_word(msg.hwnd, false);
                    true
                }
                _ => false,
            },
            // The characters of the keys above would beep.
            WM_CHAR => matches!(msg.wParam, 0x0a | 0x0d | 0x1b | 0x7f),
            // Alt+A and Alt+C, the check boxes' access keys (the window has
            // no dialog manager to find them).
            WM_SYSKEYDOWN => {
                let target = match vk {
                    0x41 => self.highlight.get(),
                    0x43 => self.case.get(),
                    _ => return false,
                };
                if target.is_null() {
                    return false;
                }
                let state = if self.checked(target) {
                    BST_UNCHECKED
                } else {
                    BST_CHECKED
                };
                send(target, BM_SETCHECK, state as usize, 0);
                app.find_options_changed();
                true
            }
            WM_SYSCHAR => matches!(msg.wParam, 0x61 | 0x41 | 0x63 | 0x43),
            _ => false,
        }
    }

    fn theme(&self, app: &App, dark: bool) {
        let edit = self.edit.get();
        if edit.is_null() {
            return;
        }
        theme::allow(edit, dark);
        theme::set_class(edit, dark.then_some("DarkMode_CFD"));
        for hwnd in [self.highlight.get(), self.case.get()] {
            theme::allow(hwnd, dark);
            theme::set_class(hwnd, dark.then_some("DarkMode_Explorer"));
        }
        for hwnd in self.controls() {
            // SAFETY: repaints this thread's controls.
            unsafe { InvalidateRect(hwnd, core::ptr::null(), 1) };
        }
        app.invalidate_strip(StripSlot::FindBar);
    }

    fn dpi(&self, app: &App, _dpi: u32) {
        self.set_fonts(app);
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// What a find's thread reports.
pub(super) enum Reply {
    /// It looked at `done` of the `of` rows it has to.
    Progress {
        done: u64,
        of: u64,
    },
    Found {
        index: u64,
        path: Box<[u16]>,
        wrapped: bool,
    },
    NotFound,
    Failed(String),
}

/// A find's report, with the find it is of.
pub(super) struct Report {
    generation: u64,
    reply: Reply,
}

/// The find the window has under way.
#[derive(Default)]
pub(super) struct Finding {
    /// Counts the finds started: a report of an older one is dropped.
    generation: Cell<u64>,
    /// Stops the thread of the one under way.
    stop: RefCell<Option<Stop>>,
    /// What it looks for, and in which search, and which way.
    search: RefCell<Option<Arc<Search>>>,
    forward: Cell<bool>,
    /// The needle the rows on screen light up (Highlight All), while the
    /// bar shows with text in it.
    pub(super) lit: RefCell<Option<Rc<Needle>>>,
}

fn same_search(a: &Search, b: &Search) -> bool {
    a.query == b.query && a.sort == b.sort && a.match_case == b.match_case
}

impl App {
    fn find_bar(&self) -> &FindBar {
        &self.tools.find_bar
    }

    fn find_needle(&self) -> Option<Needle> {
        let bar = self.find_bar();
        Needle::new(&bar.text(), bar.checked(bar.case.get()))
    }

    /// Edit > Find... (Ctrl+F): shows the bar (made the first time), the
    /// keyboard in its box with its text selected.
    pub(super) fn show_find(&self) {
        let bar = self.find_bar();
        if !bar.visible() {
            let t = self.prefs.borrow().base.search_tools.clone();
            bar.create(self, t.findbar_highlight_all, t.findbar_match_case);
            bar.visible.set(true);
            bar.show_controls(true);
            self.relayout();
            self.set_find_visible_setting(true);
            self.update_lit();
        }
        // A bar too narrow for its box (hidden) leaves the keyboard where
        // it is; F3 still finds its text.
        if empty(&bar.geo.get().frame) {
            return;
        }
        let edit = bar.edit.get();
        // SAFETY: moves the keyboard within this thread's window; while
        // the window is not the active one the box is where it goes when
        // it is (focusing would activate it, which only the user should).
        unsafe {
            if GetActiveWindow() == self.hwnd {
                SetFocus(edit);
            } else {
                self.last_focus.set(edit);
            }
        }
        send(edit, EM_SETSEL, 0, -1);
    }

    /// The × or Esc: the bar goes, and the keyboard to the results if it
    /// was in the bar.
    pub(super) fn close_find(&self) {
        let bar = self.find_bar();
        if !bar.visible() {
            return;
        }
        self.cancel_find();
        // SAFETY: reads this thread's focus.
        let had_focus = bar.contains(unsafe { GetFocus() });
        bar.visible.set(false);
        bar.hot.set(None);
        bar.show_controls(false);
        self.relayout();
        self.set_find_visible_setting(false);
        self.update_lit();
        if had_focus {
            self.focus_list();
        }
        if bar.contains(self.last_focus.get()) {
            self.last_focus.set(self.list);
        }
    }

    fn set_find_visible_setting(&self, visible: bool) {
        self.prefs.borrow_mut().base.search_tools.findbar_visible = visible;
        self.settings_changed();
    }

    /// The bar was left open: it opens again as the window starts.
    pub(super) fn restore_find(&self) {
        if self.prefs.borrow().base.search_tools.findbar_visible {
            let bar = self.find_bar();
            let t = self.prefs.borrow().base.search_tools.clone();
            bar.create(self, t.findbar_highlight_all, t.findbar_match_case);
            bar.visible.set(true);
            bar.show_controls(true);
            self.relayout();
        }
    }

    /// The box's text changed: what lights up follows, and the text is
    /// found from the focused row once typing pauses.
    pub(super) fn find_text_changed(&self) {
        self.cancel_find();
        let bar = self.find_bar();
        bar.set_not_found(self, false);
        self.invalidate_strip(StripSlot::FindBar);
        self.update_lit();
        // SAFETY: a timer of this window; setting it again restarts it.
        unsafe { SetTimer(self.hwnd, TIMER_TYPED, TYPED_MS, None) };
    }

    /// The typing paused: the text is found from the focused row on.
    pub(super) fn find_typed(&self) {
        if self.find_bar().has_text() {
            self.start_find(true, true);
        }
    }

    /// Highlight All or Match Case was switched.
    pub(super) fn find_options_changed(&self) {
        let bar = self.find_bar();
        let (all, case) = (
            bar.checked(bar.highlight.get()),
            bar.checked(bar.case.get()),
        );
        {
            let mut prefs = self.prefs.borrow_mut();
            let t = &mut prefs.base.search_tools;
            t.findbar_highlight_all = all;
            t.findbar_match_case = case;
        }
        self.settings_changed();
        bar.set_not_found(self, false);
        self.update_lit();
    }

    /// What the rows on screen light up, after the text, Match Case,
    /// Highlight All or the bar's showing changed.
    fn update_lit(&self) {
        let bar = self.find_bar();
        let lit = (bar.visible() && bar.checked(bar.highlight.get()))
            .then(|| self.find_needle())
            .flatten()
            .map(Rc::new);
        let changed = {
            let mut now = self.tools.finding.lit.borrow_mut();
            let changed = *now != lit;
            *now = lit;
            changed
        };
        if changed {
            // SAFETY: repaints the list.
            unsafe { InvalidateRect(self.list, core::ptr::null(), 0) };
        }
    }

    /// Find Next (`forward`; F3, Enter, Next) or Find Previous: the bar
    /// shows if it was closed; with nothing to find, it only shows.
    pub(super) fn find_next(&self, forward: bool) {
        let bar = self.find_bar();
        if !bar.visible() || !bar.has_text() {
            let had = bar.has_text();
            self.show_find();
            if !had {
                return;
            }
        }
        // SAFETY: a timer of this window.
        unsafe { KillTimer(self.hwnd, TIMER_TYPED) };
        self.start_find(forward, false);
    }

    /// Stops the find under way, if any.
    pub(super) fn cancel_find(&self) {
        let f = &self.tools.finding;
        f.generation.set(f.generation.get() + 1);
        if let Some(stop) = f.stop.borrow_mut().take() {
            stop.stop();
        }
        f.search.borrow_mut().take();
        if self.find_bar().progress.borrow().is_some() {
            self.find_bar().set_progress(self, None);
        }
    }

    /// Finds the text from the focused row: the rows held first, then the
    /// rest on a thread.
    fn start_find(&self, forward: bool, inclusive: bool) {
        self.cancel_find();
        let Some(needle) = self.find_needle() else {
            return;
        };
        let count = send(self.list, LVM_GETITEMCOUNT, 0, 0).max(0) as u64;
        let focus = u64::try_from(send(
            self.list,
            LVM_GETNEXTITEM,
            usize::MAX,
            LVNI_FOCUSED as isize,
        ))
        .ok();
        let parts = find::plan(count, focus, forward, inclusive);
        let (walked, search, anchor) = {
            let m = self.model.borrow();
            let walked = find::walk(&parts, |i| {
                m.peek(i as usize).map(|row| needle.matches(row.name()))
            });
            // The focused row's file: where the rows read from gozd are
            // seen from, should they have moved since (`find::Gate`).
            let anchor = focus.and_then(|f| m.peek(f as usize).map(|row| row.path.clone()));
            (walked, Arc::clone(&m.search), anchor)
        };
        match walked {
            Walked::Found { index, wrapped } => self.found(index, wrapped, forward),
            Walked::Exhausted => self.not_found(),
            Walked::Rest(rest) => {
                let job = Job {
                    needle,
                    search,
                    parts: rest,
                    anchor,
                };
                self.find_rest(job, forward);
            }
        }
    }

    /// Looks through `job`'s rows on a thread of its own.
    fn find_rest(&self, job: Job, forward: bool) {
        let search = Arc::clone(&job.search);
        let f = &self.tools.finding;
        let generation = f.generation.get();
        let stop = Stop::new();
        *f.stop.borrow_mut() = Some(stop.clone());
        *f.search.borrow_mut() = Some(Arc::clone(&search));
        f.forward.set(forward);
        self.find_bar()
            .set_progress(self, Some("Searching...".to_owned()));
        let hwnd = self.hwnd as usize;
        let spawned = std::thread::Builder::new()
            .name("goz-find".into())
            .spawn(move || {
                let reply = scan(&stop, &job, &|done, of| {
                    post(hwnd, generation, Reply::Progress { done, of });
                });
                if let Some(reply) = reply {
                    post(hwnd, generation, reply);
                }
            });
        if spawned.is_err() {
            self.cancel_find();
            self.show_note("The find could not be started".to_owned());
        }
    }

    /// A find's thread reported (`WM_FIND`).
    pub(super) fn find_report(&self, report: Report) {
        let f = &self.tools.finding;
        if report.generation != f.generation.get() {
            return;
        }
        // Its rows are another search's now: it is not wanted.
        let current = Arc::clone(&self.model.borrow().search);
        let same = f
            .search
            .borrow()
            .as_ref()
            .is_some_and(|s| same_search(s, &current));
        if !same {
            self.cancel_find();
            return;
        }
        let forward = f.forward.get();
        match report.reply {
            Reply::Progress { done, of } => {
                let pct = (done.saturating_mul(100) / of.max(1)).min(99);
                self.find_bar()
                    .set_progress(self, Some(format!("Searching... {pct}%")));
            }
            Reply::Found {
                index,
                path,
                wrapped,
            } => {
                self.cancel_find();
                // Where the file is in the rows held, if it is (a file
                // that came or went above it since moved it); else it is
                // looked for again as the rows round `index` come.
                let held = self.model.borrow().find(&path);
                match held {
                    Some(i) => self.found(i as u64, wrapped, forward),
                    None => self.found_far(index, path, wrapped, forward),
                }
            }
            Reply::NotFound => {
                self.cancel_find();
                self.not_found();
            }
            Reply::Failed(why) => {
                self.cancel_find();
                self.show_note(why);
            }
        }
    }

    /// Selects the match at `index` alone, as a click on it would, and
    /// brings it into view; the keyboard stays where it is.
    fn found(&self, index: u64, wrapped: bool, forward: bool) {
        let count = send(self.list, LVM_GETITEMCOUNT, 0, 0).max(0) as u64;
        if index >= count {
            return;
        }
        let index = index as usize;
        self.find_bar().set_not_found(self, false);
        self.set_state(usize::MAX, 0, LVIS_SELECTED);
        self.set_state(
            index,
            LVIS_SELECTED | LVIS_FOCUSED,
            LVIS_SELECTED | LVIS_FOCUSED,
        );
        send(self.list, LVM_SETSELECTIONMARK, 0, index as isize);
        send(self.list, LVM_ENSUREVISIBLE, index, 0);
        hover::scrolled(self);
        self.wrap_note(wrapped, forward);
    }

    /// A match the find's thread read at `index`, in rows the window does
    /// not hold: the list goes there, and the file is selected by its path
    /// as its rows come (`reselect.rs`), so a file that came or went above
    /// it meanwhile does not put another one under the selection.
    fn found_far(&self, index: u64, path: Box<[u16]>, wrapped: bool, forward: bool) {
        let count = send(self.list, LVM_GETITEMCOUNT, 0, 0).max(0) as u64;
        if count == 0 {
            return;
        }
        let index = index.min(count - 1) as usize;
        self.find_bar().set_not_found(self, false);
        // The match is the selection now: an action still waiting for the
        // rows of the one before (Delete, a file tool...) is not wanted
        // any more, and must not run on the match when they come. (The
        // selection changes quietly here, which cancels nothing itself.)
        self.cancel_fetch();
        self.quietly(|| self.clear_selection());
        let path = path.into_vec();
        *self.reselect.borrow_mut() = Some(super::reselect::Reselect::sought_again(
            vec![path.clone()],
            Some(path),
        ));
        send(self.list, LVM_ENSUREVISIBLE, index, 0);
        // Asked for now, not when the list paints them: a window not on
        // screen paints nothing, and the file is selected as they come.
        self.request_rows(index.saturating_sub(64), index + 64);
        hover::scrolled(self);
        self.wrap_note(wrapped, forward);
    }

    fn wrap_note(&self, wrapped: bool, forward: bool) {
        if wrapped {
            self.show_note(
                if forward {
                    find::WRAPPED_TO_TOP
                } else {
                    find::WRAPPED_TO_BOTTOM
                }
                .to_owned(),
            );
        }
    }

    fn not_found(&self) {
        let bar = self.find_bar();
        bar.set_not_found(self, true);
        self.show_note(find::not_found(&bar.text()));
        // SAFETY: plays a system sound.
        unsafe { MessageBeep(MB_OK) };
    }

    /// Lights the find's matches in a name being drawn (Highlight All):
    /// behind the text of `name` drawn in `rc` with `bold` spans in bold.
    pub fn paint_find_matches(
        &self,
        hdc: HDC,
        rc: &RECT,
        name: &[u16],
        bold: &[Span],
        fonts: &Fonts,
    ) {
        let lit = self.tools.finding.lit.borrow();
        let Some(needle) = lit.as_ref() else {
            return;
        };
        let mut spans = Vec::new();
        needle.spans(name, &mut spans);
        if spans.is_empty() {
            return;
        }
        let colour = if self.pal.get().dark {
            rgb(0x7a, 0x5c, 0x00)
        } else {
            rgb(0xff, 0xd8, 0x5c)
        };
        let fh = fonts.height;
        let top = rc.top + (rc.bottom - rc.top - fh) / 2;
        for (s, e) in spans {
            let x0 = rc.left + prefix_width(hdc, name, bold, fonts, s);
            let x1 = rc.left + prefix_width(hdc, name, bold, fonts, e);
            let r = RECT {
                left: x0.max(rc.left),
                top,
                right: x1.min(rc.right),
                bottom: top + fh,
            };
            if r.right > r.left {
                fill(hdc, &r, colour);
            }
        }
    }
}

/// The width of `text[..end]` drawn with `bold` spans in bold.
fn prefix_width(hdc: HDC, text: &[u16], bold: &[Span], fonts: &Fonts, end: usize) -> i32 {
    let end = end.min(text.len());
    let mut at = 0;
    let mut w = 0;
    let mut measure = |s: usize, e: usize, font| {
        if e > s {
            // SAFETY: selects one of the row's fonts into the row's DC to
            // measure; the caller puts its own back as it draws.
            unsafe { SelectObject(hdc, font) };
            w += crate::paint::width(hdc, &text[s..e]);
        }
    };
    for &(s, e) in bold {
        let (s, e) = (s.min(end), e.min(end));
        if s > at {
            measure(at, s, fonts.normal);
        }
        if e > s.max(at) {
            measure(s.max(at), e, fonts.bold);
        }
        at = at.max(e);
    }
    if at < end {
        measure(at, end, fonts.normal);
    }
    // SAFETY: as above: the normal font back, which the row draws in.
    unsafe { SelectObject(hdc, fonts.normal) };
    w
}

/// Posts `reply` to the window; if the window is gone, it is freed here.
fn post(hwnd: usize, generation: u64, reply: Reply) {
    let raw = Box::into_raw(Box::new(Report { generation, reply }));
    // SAFETY: the window owns the box once posted; else it is taken back.
    unsafe {
        if PostMessageW(hwnd as HWND, WM_FIND, 0, raw as isize) == 0 {
            drop(Box::from_raw(raw));
        }
    }
}

/// What a find's thread looks through.
struct Job {
    needle: Needle,
    search: Arc<Search>,
    parts: Vec<Part>,
    /// The path of the row the find started from, if it had one held.
    anchor: Option<Box<[u16]>>,
}

/// One stream a find reads: its rows `lo..hi` (`None`: to the end), and
/// the gate that picks its match.
struct Stream {
    lo: u64,
    hi: Option<u64>,
    gate: Gate,
}

/// The streams `part` is read in. Forward, one: after the anchor from a
/// little before the plan's first row to the end, before it a little past
/// the plan's last. Backward, chunks from the end of the part, the one at
/// the anchor's side read a little past the plan (`find::SLACK`): rows may
/// have moved since the part was planned, and the anchor says where its
/// side is now.
fn streams(part: &Part, anchor: Option<&[u16]>) -> Vec<Stream> {
    let slack = if anchor.is_some() { find::SLACK } else { 0 };
    if part.forward {
        let (lo, hi) = match part.side() {
            Side::After => (part.start.saturating_sub(slack), None),
            Side::Before => (part.start, Some(part.end.saturating_add(slack))),
        };
        return vec![Stream {
            lo,
            hi,
            gate: Gate::new(part, anchor, true),
        }];
    }
    let chunks = find::back_chunks(part.start, part.end);
    let n = chunks.len();
    chunks
        .into_iter()
        .enumerate()
        .map(|(i, (lo, hi))| {
            // Before the anchor (Previous from it): the first chunk, at
            // the end, is the anchor's; after it (round the end), the last.
            let at_anchor = match part.side() {
                Side::Before => i == 0,
                Side::After => i + 1 == n,
            };
            if !at_anchor {
                return Stream {
                    lo,
                    hi: Some(hi),
                    gate: Gate::open(false),
                };
            }
            let (lo, hi) = match part.side() {
                Side::Before => (lo, hi.saturating_add(slack)),
                Side::After => (lo.saturating_sub(slack), hi),
            };
            Stream {
                lo,
                hi: Some(hi),
                gate: Gate::new(part, anchor, false),
            }
        })
        .collect()
}

/// Looks through `job`'s parts for its needle, reading names from gozd,
/// each part's streams in turn (`streams`); `progress` hears how far it
/// got now and then. `None` when stopped.
fn scan(stop: &Stop, job: &Job, progress: &dyn Fn(u64, u64)) -> Option<Reply> {
    let of: u64 = job.parts.iter().map(Part::len).sum();
    let mut done = 0u64;
    let mut last = Instant::now();
    for part in &job.parts {
        for stream in streams(part, job.anchor.as_deref()) {
            let mut gate = stream.gate;
            let mut index = stream.lo;
            let clamp = |v: u64| v.min(u64::from(u32::MAX)) as u32;
            let range = RowRange {
                offset: clamp(stream.lo),
                limit: stream.hi.map(|hi| clamp(hi.saturating_sub(stream.lo))),
                details: false,
            };
            let streamed = client::stream_range(stop, &job.search, range, |rows, _total| {
                for row in &rows {
                    let matched = job.needle.matches(row.name());
                    if gate.row(index, &row.path, matched) {
                        return Ok(false);
                    }
                    index += 1;
                }
                done += rows.len() as u64;
                if last.elapsed() >= PROGRESS_EVERY {
                    last = Instant::now();
                    progress(done, of);
                }
                Ok(true)
            });
            match streamed {
                Streamed::Stopped => return None,
                Streamed::Failed(f) => {
                    let why = match f {
                        client::Failure::NotRunning => client::not_running_text(),
                        client::Failure::Untrusted => "gozd could not be verified.".into(),
                        client::Failure::Refused(m) | client::Failure::Other(m) => m,
                    };
                    return Some(Reply::Failed(format!("Find: {why}")));
                }
                Streamed::Done(_) => {}
            }
            if let Some((index, path)) = gate.pick() {
                return Some(Reply::Found {
                    index,
                    path,
                    wrapped: part.wrapped,
                });
            }
            if stop.stopped() {
                return None;
            }
        }
    }
    Some(Reply::NotFound)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn streams_read_past_the_plan_on_the_anchors_side() {
        let a: Vec<u16> = "anchor".encode_utf16().collect();
        let part = |start, end, forward, wrapped| Part {
            start,
            end,
            forward,
            wrapped,
            edge: wrapped,
        };
        let spans = |p: &Part, anchor: Option<&[u16]>| -> Vec<(u64, Option<u64>)> {
            streams(p, anchor).iter().map(|s| (s.lo, s.hi)).collect()
        };
        // Next after the anchor: from the slack before it, to the end.
        let next = part(10_000, 50_000, true, false);
        assert_eq!(spans(&next, Some(&a)), [(10_000 - find::SLACK, None)]);
        assert_eq!(spans(&next, None), [(10_000, None)]);
        // Round the end: a slack past the plan.
        let round = part(0, 10_000, true, true);
        assert_eq!(spans(&round, Some(&a)), [(0, Some(10_000 + find::SLACK))]);
        assert_eq!(spans(&round, None), [(0, Some(10_000))]);
        // Previous: the first chunk (nearest the anchor) a slack longer.
        let prev = part(0, 10_000, false, false);
        let s = spans(&prev, Some(&a));
        assert_eq!(s[0], (10_000 - 4096, Some(10_000 + find::SLACK)));
        assert_eq!(s.last().map(|c| c.0), Some(0));
        // Previous round the end: the last chunk (nearest the anchor).
        let back = part(9000, 20_000, false, true);
        let s = spans(&back, Some(&a));
        assert_eq!(s[0], (20_000 - 4096, Some(20_000)));
        assert_eq!(s.last().map(|c| c.0), Some(9000 - find::SLACK));
    }

    /// The sizes at 100 % with the usual 15 px font: 19 px buttons.
    const AT_96: Sizes = Sizes {
        button: 19,
        gap: 4,
        wide_gap: 6,
        min_box: 48,
        max_box: 256,
    };

    #[test]
    fn a_wide_bar_holds_everything() {
        let row = arrange(3, 1000, &AT_96, [Some(90), Some(80)]);
        assert_eq!(row.frame, Some((3, 259)));
        assert_eq!(row.arrows, Some(263));
        // ▾ ends at 301; the first check box 6 px on, the next 4 px on.
        assert_eq!(row.checks, [Some((307, 397)), Some((401, 481))]);
        assert_eq!(row.rest, 489);
        // Check boxes not made yet: the progress text follows the arrows.
        let row = arrange(3, 1000, &AT_96, [None, None]);
        assert_eq!(row.checks, [None, None]);
        assert_eq!(row.rest, 311);
    }

    #[test]
    fn the_check_boxes_go_first_the_last_first() {
        // Room for the first only.
        let row = arrange(3, 450, &AT_96, [Some(90), Some(80)]);
        assert_eq!(row.frame, Some((3, 259)));
        assert_eq!(row.checks, [Some((307, 397)), None]);
        // Room for the second only: it goes with the first.
        let row = arrange(3, 390, &AT_96, [Some(90), Some(40)]);
        assert_eq!(row.checks, [None, None]);
        assert!(row.rest > 390, "no room left for the progress text");
        // Then the box shrinks, the arrows kept.
        let row = arrange(3, 200, &AT_96, [Some(90), Some(80)]);
        assert_eq!(row.frame, Some((3, 158)));
        assert_eq!(row.arrows, Some(162));
        assert_eq!(row.checks, [None, None]);
    }

    #[test]
    fn the_arrows_give_way_before_the_box() {
        // Exactly the box's minimum and the arrows: both.
        let row = arrange(3, 3 + 48 + 4 + 38, &AT_96, [Some(90), Some(80)]);
        assert_eq!(row.frame, Some((3, 51)));
        assert_eq!(row.arrows, Some(55));
        // A pixel less: the arrows go and the box takes their room.
        let row = arrange(3, 3 + 48 + 4 + 37, &AT_96, [Some(90), Some(80)]);
        assert_eq!(row.frame, Some((3, 92)));
        assert_eq!(row.arrows, None);
        assert_eq!(row.checks, [None, None]);
        // A Medium window with every pane open (the bar 100 px wide): the
        // box, 67 px, and the × (was ▴ ▾ and the × with no box).
        let row = arrange(3, 70, &AT_96, [Some(90), Some(80)]);
        assert_eq!(row.frame, Some((3, 70)));
        assert_eq!(row.arrows, None);
    }

    #[test]
    fn under_its_minimum_the_box_goes_with_everything_but_the_close() {
        let row = arrange(3, 3 + 48, &AT_96, [Some(10), Some(10)]);
        assert_eq!(row.frame, Some((3, 51)));
        let row = arrange(3, 3 + 47, &AT_96, [Some(10), Some(10)]);
        assert_eq!(row.frame, None);
        assert_eq!(row.arrows, None);
        assert_eq!(row.checks, [None, None]);
        assert!(row.rest > 50, "no progress text either");
        // No room at all.
        let row = arrange(3, -20, &AT_96, [Some(90), Some(80)]);
        assert_eq!((row.frame, row.arrows), (None, None));
    }

    #[test]
    fn at_150_percent_the_same_order() {
        let z = Sizes {
            button: 29,
            gap: 6,
            wide_gap: 9,
            min_box: 72,
            max_box: 384,
        };
        let row = arrange(5, 5 + 72 + 6 + 58, &z, [Some(130), Some(120)]);
        assert_eq!((row.frame, row.arrows), (Some((5, 77)), Some(83)));
        let row = arrange(5, 5 + 72 + 6 + 57, &z, [Some(130), Some(120)]);
        assert_eq!((row.frame, row.arrows), (Some((5, 140)), None));
    }

    #[test]
    fn a_find_is_of_one_search() {
        let s = |q: &str| Search {
            query: q.into(),
            sort: Default::default(),
            match_case: false,
        };
        assert!(same_search(&s("a"), &s("a")));
        assert!(!same_search(&s("a"), &s("b")));
        let mut c = s("a");
        c.match_case = true;
        assert!(!same_search(&s("a"), &c));
    }
}
