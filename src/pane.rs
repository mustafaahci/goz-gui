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
//! the focus, ...) goes to the content first; the mouse's moves and leaving
//! (and a capture lost) go to both, so a content can light a button it
//! paints.
//!
//! Re-entrancy: the content is an `Rc<dyn PaneContent>` whose methods take
//! `&self`. The framework holds no borrow of its own state while it calls
//! one, and a content must not hold a `RefCell` borrow of its own across a
//! call that sends messages (creating, moving or destroying its windows,
//! SendMessage to its controls), since they come back here.

use std::any::Any;
use std::cell::{Cell, RefCell};
use std::rc::{Rc, Weak};

use windows_sys::Win32::Foundation::{HWND, LRESULT, POINT, RECT};
use windows_sys::Win32::Graphics::Gdi::{
    DC_BRUSH, DT_END_ELLIPSIS, DT_NOPREFIX, DT_SINGLELINE, DT_VCENTER, HDC, SRCCOPY, TRANSPARENT,
};
use windows_sys::Win32::UI::Controls::WM_MOUSELEAVE;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    IDC_ARROW, MSG, WM_CAPTURECHANGED, WM_CREATE, WM_CTLCOLORBTN, WM_CTLCOLOREDIT,
    WM_CTLCOLORLISTBOX, WM_CTLCOLORSTATIC, WM_ERASEBKGND, WM_LBUTTONDBLCLK, WM_LBUTTONDOWN,
    WM_LBUTTONUP, WM_MOUSEMOVE, WM_NCCREATE, WM_NCDESTROY, WM_PAINT, WM_SETFOCUS, WM_SIZE,
    WS_CHILD, WS_CLIPCHILDREN, WS_CLIPSIBLINGS,
};

use crate::colors::{Item, State};
use crate::dock::{PaneId, Side};
use crate::paint::fill;
use crate::searchbox;
use crate::theme::Ink;
use crate::win::class::{self, Class, Create};
use crate::win::gdi::{self, MemDc, Paint, Selected};
use crate::win::{self, Msg, sys};
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
    /// content took it, having done what the key means itself; it then
    /// goes no further (it is neither translated nor dispatched). For the
    /// keys a control must keep (Enter in a box, F2 in a tree, Ctrl+A in an
    /// edit box...).
    fn key(&self, _app: &App, _msg: &MSG) -> bool {
        false
    }

    /// A message to the pane window that the framework leaves to the
    /// content: the content's controls' WM_COMMAND, WM_NOTIFY,
    /// WM_CTLCOLOR*, WM_DRAWITEM; WM_TIMER; WM_CONTEXTMENU; the keyboard and
    /// WM_SETFOCUS / WM_KILLFOCUS while the pane window itself has the
    /// focus; the left button below the title row; the content's own
    /// WM_APP+n (a value sent with one comes as a `win::parcel`). `Some`
    /// is the answer; `None` lets the framework answer (the theme's
    /// colours for WM_CTLCOLOR*, else DefWindowProc). The framework also
    /// passes on WM_MOUSEMOVE, WM_MOUSELEAVE (it tracks the leaving for its
    /// close button: a content asks for it too) and a WM_CAPTURECHANGED
    /// not its own, and answers those itself.
    ///
    /// `m` is the message as the pane window was given it: a notification's
    /// structure is read with `m.notify()`. By default the content takes
    /// none.
    fn handle(&self, _app: &App, _pane: HWND, _m: &Msg) -> Option<LRESULT> {
        None
    }

    /// Paints the client area where the content has no window, `rc` in
    /// the pane window's coordinates, into `hdc` (a buffer the size of
    /// what is being repainted). By default the list's background.
    fn paint(&self, app: &App, hdc: HDC, rc: &RECT) {
        fill(hdc, rc, self.colours(app).bg);
    }

    /// The content's background and its controls' text and background
    /// (WM_CTLCOLOR*): the window's own by default; a sidebar's or the
    /// preview's from Options > Fonts and Colors.
    fn colours(&self, app: &App) -> Ink {
        let pal = app.pal.get();
        Ink {
            fg: pal.fg,
            bg: pal.bg,
        }
    }

    /// The theme changed (`dark` or light); the pane repaints. Controls
    /// the content made need their colours or visual style set again.
    fn theme(&self, _app: &App, _dark: bool) {}

    /// The window's DPI, or a font of Options > Fonts and Colors, changed:
    /// fonts and sizes are remade (the content's controls given the fonts
    /// now: the fonts they held are deleted after this), then `place`
    /// follows.
    fn dpi(&self, _app: &App, _dpi: u32) {}

    /// The least height of the content (below the title row) in pixels at
    /// the fonts now, besides its `PaneSpec::min_high`: a divider stops
    /// where the larger of the two is reached.
    fn least_high(&self, _app: &App) -> i32 {
        0
    }

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

/// The pane window's own state, held by its handler (the closure the
/// window was made with) until WM_NCDESTROY, and found by its window in
/// [`WINDOWS`] meanwhile.
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

thread_local! {
    /// The open pane windows' states, by window, from WM_NCCREATE to
    /// WM_NCDESTROY (the handler holds each state itself).
    static WINDOWS: RefCell<Vec<(HWND, Weak<PaneWindow>)>> = const { RefCell::new(Vec::new()) };
}

/// Registers the pane window's class (once, as the window starts).
pub fn register_class() {
    Class {
        cursor: sys::system_cursor(IDC_ARROW),
        ..Class::new(CLASS)
    }
    .register();
}

/// Makes the (hidden) window of pane `id` in the main window `parent`; its
/// content's `create` is the caller's to call.
pub fn create_window(
    parent: HWND,
    id: PaneId,
    title: &'static str,
    content: Rc<dyn PaneContent>,
) -> HWND {
    let state = Rc::new(PaneWindow {
        id,
        title,
        content,
        created: Cell::new(false),
        hot: Cell::new(false),
        pressed: Cell::new(false),
    });
    // The handler owns the state, and goes (with it) after WM_NCDESTROY,
    // or at once if the window could not be made.
    class::create(
        &Create {
            ex_style: 0,
            class: CLASS,
            title,
            style: WS_CHILD | WS_CLIPCHILDREN | WS_CLIPSIBLINGS,
            rect: (0, 0, 0, 0),
            parent,
            menu: class::id_menu(CHILD_ID + id.code()),
        },
        move |m| pane_proc(&state, m),
    )
}

/// The title row's height (0 with `pane_title=0`): the header's at 100 %
/// (its font's height (Fonts and Colors' Pane) and 12 px), so a pane's
/// title row lines up with the results' header beside it.
pub fn title_height(app: &App) -> i32 {
    if app.pane_titles() {
        app.part_height(Item::Pane) + app.scale(12)
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

/// Pane window `hwnd`'s state, while it is there.
fn state(hwnd: HWND) -> Option<Rc<PaneWindow>> {
    WINDOWS.with_borrow(|w| {
        w.iter()
            .find(|(h, _)| *h == hwnd)
            .and_then(|(_, p)| p.upgrade())
    })
}

fn client(hwnd: HWND) -> RECT {
    win::client_rect(hwnd)
}

/// The content's place in a pane window: below the title row.
pub fn content_rect(app: &App, hwnd: HWND) -> RECT {
    let rc = client(hwnd);
    RECT {
        top: title_height(app).min(rc.bottom),
        ..rc
    }
}

fn inside(rc: &RECT, pt: POINT) -> bool {
    pt.x >= rc.left && pt.x < rc.right && pt.y >= rc.top && pt.y < rc.bottom
}

fn close_of(app: &App, hwnd: HWND) -> RECT {
    close_rect(client(hwnd).right, title_height(app), |v| app.scale(v))
}

/// What accessibility tools see of a pane window besides its content's
/// windows (`crate::a11y`): the close button of its title row.
pub fn accessible_parts(app: &App, hwnd: HWND) -> Vec<crate::a11y::Part> {
    let rc = close_of(app, hwnd);
    if rc.right <= rc.left {
        return Vec::new();
    }
    let mut close = crate::a11y::Part::new("Close", crate::a11y::ROLE_PUSHBUTTON, rc);
    if state(hwnd).is_some_and(|p| p.hot.get()) {
        close.state = crate::a11y::STATE_HOTTRACKED;
    }
    close.action = Some(("Press", crate::a11y::Action::ClosePane));
    vec![close]
}

fn invalidate_close(app: &App, hwnd: HWND) {
    let rc = close_of(app, hwnd);
    win::invalidate(hwnd, Some(&rc), false);
}

/// Paints the title row and what of the client area shows, through a
/// buffer the size of what is being repainted.
fn paint(app: &App, hwnd: HWND, p: &PaneWindow) {
    // A standard double-buffered WM_PAINT: every GDI object made here is
    // released before returning (the guards drop in the reverse order of
    // their making: the bitmap selected out, deleted, the buffer's DC, then
    // EndPaint). The buffer's origin is moved to the invalid rectangle's
    // corner, so drawing stays in the window's coordinates.
    let ps = Paint::begin(hwnd);
    let hdc = ps.hdc();
    let dirty = ps.area();
    let (w, h) = (dirty.right - dirty.left, dirty.bottom - dirty.top);
    if w <= 0 || h <= 0 {
        return;
    }
    let mem = MemDc::compatible(hdc);
    let Some(bmp) = gdi::create_compatible_bitmap(hdc, w, h) else {
        return;
    };
    let _bmp_in = Selected::new(mem.hdc(), &bmp);
    gdi::set_viewport_org(mem.hdc(), -dirty.left, -dirty.top);

    let rc = client(hwnd);
    let title_h = title_height(app);
    let title = RECT {
        bottom: title_h.min(rc.bottom),
        ..rc
    };
    if title_h > 0 {
        paint_title(app, hwnd, p, mem.hdc(), &title);
    }
    let body = RECT {
        top: title.bottom,
        ..rc
    };
    if body.bottom > body.top {
        p.content.paint(app, mem.hdc(), &body);
    }

    gdi::bit_blt(
        hdc,
        (dirty.left, dirty.top, w, h),
        mem.hdc(),
        (dirty.left, dirty.top),
        SRCCOPY,
    );
}

/// What shows of the pane, in its colours (the title row's, the content's
/// background) as soon as it shows: Windows asks for this (WM_ERASEBKGND)
/// at once as the pane is shown or grows, while the paint (WM_PAINT) comes
/// later. Left untouched, that part showed white until then: the whole
/// pane as it opened, a band where it grew.
fn erase(app: &App, hwnd: HWND, hdc: HDC, content_bg: u32) {
    let pal = app.pal.get();
    let rc = client(hwnd);
    let title = title_height(app).min(rc.bottom);
    fill(
        hdc,
        &RECT {
            bottom: rc.top + title,
            ..rc
        },
        pal.ink(Item::Pane, State::Normal).bg,
    );
    fill(
        hdc,
        &RECT {
            top: rc.top + title,
            ..rc
        },
        content_bg,
    );
}

/// The title row `rc`: the header's lines, Fonts and Colors' Pane colours
/// and style, the name, and the close button.
fn paint_title(app: &App, hwnd: HWND, p: &PaneWindow, hdc: HDC, rc: &RECT) {
    let pal = app.pal.get();
    let ink = pal.ink(Item::Pane, State::Normal);
    fill(hdc, rc, ink.bg);
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
    let text: Vec<u16> = p.title.encode_utf16().collect();
    {
        // Drawn into the paint buffer; the old font goes back as this
        // drops.
        let font = app.part_font(Item::Pane, pal.is_bold(Item::Pane, State::Normal));
        let _font = Selected::of(hdc, font);
        gdi::set_bk_mode(hdc, TRANSPARENT as i32);
        gdi::set_text_color(hdc, ink.fg);
        gdi::draw_text(
            hdc,
            &text,
            &mut text_rc,
            DT_SINGLELINE | DT_VCENTER | DT_END_ELLIPSIS | DT_NOPREFIX,
        );
    }
    if close.right <= close.left {
        return;
    }
    let (hot, pressed) = (p.hot.get(), p.pressed.get());
    let bg = match (hot, pressed) {
        (true, true) => pal.header_pressed,
        (true, false) => pal.header_hot,
        _ => ink.bg,
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
        // WM_MOUSELEAVE, when the mouse leaves the window.
        win::track_leave(hwnd);
    }
    if p.hot.replace(over) != over {
        invalidate_close(app, hwnd);
    }
}

/// The pane window's handler, `p` its state (the closure's own).
fn pane_proc(p: &Rc<PaneWindow>, m: &Msg) -> LRESULT {
    let hwnd = m.hwnd;
    if m.id == WM_NCCREATE {
        let weak = Rc::downgrade(p);
        WINDOWS.with_borrow_mut(|w| w.push((hwnd, weak)));
        return m.def();
    }
    if m.id == WM_NCDESTROY {
        WINDOWS.with_borrow_mut(|w| w.retain(|(h, _)| *h != hwnd));
        return m.def();
    }
    let Some(app) = crate::window::app_of(win::parent(hwnd)) else {
        return m.def();
    };
    // A content's handler may close its own pane (a choice in a menu of
    // its own), which destroys this window while the handler runs. The
    // window's handler, this, is kept alive by the call that runs it until
    // it returns, so `p` and its content stay alive meanwhile; what is
    // done with the window after such a call does nothing, the window
    // being gone.
    let content = Rc::clone(&p.content);
    let id = p.id;
    let (msg, w, l) = (m.id, m.w, m.l);
    match msg {
        // The content makes its windows as the pane's is made, before
        // any WM_SIZE places them.
        WM_CREATE => {
            content.create(app, hwnd);
            p.created.set(true);
            return 0;
        }
        WM_PAINT => {
            paint(app, hwnd, p);
            return 0;
        }
        // Its title and close button for accessibility tools.
        m if m == windows_sys::Win32::UI::WindowsAndMessaging::WM_GETOBJECT
            && crate::a11y::wants_client(l) =>
        {
            if let Some(answer) = crate::window::pane_object(hwnd, id.code(), w) {
                return answer;
            }
        }
        WM_ERASEBKGND => {
            // wParam is the DC to erase with, a handle GDI checks.
            erase(app, hwnd, w as HDC, content.colours(app).bg);
            return 1;
        }
        WM_SIZE => {
            if p.created.get() {
                content.place(app, content_rect(app, hwnd));
            }
            // The close button keeps to the right edge.
            let title = RECT {
                bottom: title_height(app),
                ..client(hwnd)
            };
            win::invalidate(hwnd, Some(&title), false);
            return 0;
        }
        // The close button's, then the content's (a button it paints).
        WM_MOUSEMOVE => {
            mouse_move(app, hwnd, p, m.point());
            content.handle(app, hwnd, m);
            return 0;
        }
        WM_MOUSELEAVE => {
            if !p.pressed.get() && p.hot.replace(false) {
                invalidate_close(app, hwnd);
            }
            content.handle(app, hwnd, m);
            return 0;
        }
        WM_LBUTTONDOWN | WM_LBUTTONDBLCLK => {
            let pt = m.point();
            if inside(&close_of(app, hwnd), pt) {
                p.pressed.set(true);
                p.hot.set(true);
                win::set_capture(hwnd);
                invalidate_close(app, hwnd);
                return 0;
            }
            // The title row, or the client area where the content has
            // no window and does not want the click: the pane takes
            // the keyboard.
            if pt.y < title_height(app) {
                app.focus_pane(id);
                return 0;
            }
            if let Some(answer) = content.handle(app, hwnd, m) {
                return answer;
            }
            app.focus_pane(id);
            return 0;
        }
        WM_LBUTTONUP if p.pressed.get() => {
            p.pressed.set(false);
            if win::capture() == hwnd {
                win::release_capture();
            }
            let over = inside(&close_of(app, hwnd), m.point());
            p.hot.set(over);
            invalidate_close(app, hwnd);
            if over {
                // Closed from the window's own queue: this window
                // goes, which it must not do inside its own message.
                win::post_app(app.hwnd, crate::window::WM_PANE_CLOSE, p.id.code(), 0);
            }
            return 0;
        }
        WM_CAPTURECHANGED => {
            if p.pressed.replace(false) {
                p.hot.set(false);
                invalidate_close(app, hwnd);
            } else {
                // The content's own press, if it took the mouse.
                content.handle(app, hwnd, m);
            }
            return 0;
        }
        WM_SETFOCUS => {
            if let Some(target) = content.focus().filter(|&t| t != hwnd) {
                win::set_focus(target);
                return 0;
            }
        }
        _ => {}
    }
    if let Some(answer) = content.handle(app, hwnd, m) {
        return answer;
    }
    match msg {
        // The content's controls in its colours, unless it said
        // otherwise.
        WM_CTLCOLOREDIT | WM_CTLCOLORLISTBOX | WM_CTLCOLORSTATIC | WM_CTLCOLORBTN => {
            // wParam is the DC the control is about to draw with, a handle
            // GDI checks; the DC brush takes the colour set on it.
            let dc = w as HDC;
            let ink = content.colours(app);
            gdi::set_text_color(dc, ink.fg);
            gdi::set_bk_color(dc, ink.bg);
            gdi::set_dc_brush_color(dc, ink.bg);
            gdi::stock_object(DC_BRUSH) as LRESULT
        }
        _ => m.def(),
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
