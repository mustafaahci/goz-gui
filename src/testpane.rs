//! `GOZ_GUI_TEST_PANES`: panes and strips with nothing of a feature in
//! them, for testing the framework (the docks, dividers, title rows, F6,
//! Shift+Esc, the strips' rows) on its own. The variable's letters each add
//! one: `L` a pane in the left dock, `R` one in the right dock (up to four
//! panes, shown at start, kept in no settings), `T` a tab strip and `F` a
//! find bar placeholder, each a labelled row. `GOZ_GUI_TEST_PANES=LLRTF`
//! has two left panes, one right, and both rows.

use std::any::Any;
use std::cell::Cell;
use std::rc::Rc;

use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows_sys::Win32::Graphics::Gdi::{
    DT_CENTER, DT_NOPREFIX, DT_SINGLELINE, DT_VCENTER, DrawFocusRect, DrawTextW, HDC,
    InvalidateRect, SelectObject, SetBkMode, SetTextColor, TRANSPARENT,
};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    GetCapture, GetFocus, ReleaseCapture, SetCapture,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    WM_CAPTURECHANGED, WM_KILLFOCUS, WM_LBUTTONDOWN, WM_LBUTTONUP, WM_RBUTTONUP, WM_SETFOCUS,
};

use crate::dock::{PaneId, Side};
use crate::paint::fill;
use crate::pane::{PaneContent, PaneSpec};
use crate::strip::{Strip, StripSlot};
use crate::window::App;

/// What the variable asks for.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Wanted {
    pub panes: Vec<Side>,
    pub tabs: bool,
    pub find: bool,
}

/// The variable's letters (any case; others are ignored).
pub fn parse(text: &str) -> Wanted {
    let mut w = Wanted::default();
    for c in text.chars().map(|c| c.to_ascii_uppercase()) {
        match c {
            'L' if w.panes.len() < 4 => w.panes.push(Side::Left),
            'R' if w.panes.len() < 4 => w.panes.push(Side::Right),
            'T' => w.tabs = true,
            'F' => w.find = true,
            _ => {}
        }
    }
    w
}

/// What `GOZ_GUI_TEST_PANES` asks for; nothing without it.
pub fn wanted() -> Wanted {
    std::env::var("GOZ_GUI_TEST_PANES")
        .map(|v| parse(&v))
        .unwrap_or_default()
}

/// The test panes' specs, in order.
pub fn specs(w: &Wanted) -> Vec<PaneSpec> {
    w.panes
        .iter()
        .enumerate()
        .map(|(i, &side)| {
            let id = PaneId::Test(i as u8);
            PaneSpec {
                side,
                order: i as i32,
                ..PaneSpec::new(
                    id,
                    Box::new(move || Rc::new(TestPane { id }) as Rc<dyn PaneContent>),
                )
            }
        })
        .collect()
}

/// A pane with nothing in it: its name, dimmed, in the middle, and a focus
/// rectangle while it has the keyboard.
struct TestPane {
    id: PaneId,
}

fn centred(app: &App, hdc: HDC, rc: &RECT, text: &str, colour: u32) {
    let mut t: Vec<u16> = text.encode_utf16().collect();
    let mut r = *rc;
    // SAFETY: draws into the buffer handed over; the old font is put back.
    unsafe {
        let old = SelectObject(hdc, app.ui_font());
        SetBkMode(hdc, TRANSPARENT as i32);
        SetTextColor(hdc, colour);
        DrawTextW(
            hdc,
            t.as_mut_ptr(),
            t.len() as i32,
            &mut r,
            DT_CENTER | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX,
        );
        SelectObject(hdc, old);
    }
}

impl PaneContent for TestPane {
    fn create(&self, _app: &App, _pane: HWND) {}

    fn place(&self, _app: &App, _rc: RECT) {}

    fn message(&self, app: &App, pane: HWND, msg: u32, _w: WPARAM, _l: LPARAM) -> Option<LRESULT> {
        if matches!(msg, WM_SETFOCUS | WM_KILLFOCUS)
            && let Some(rc) = app.pane_client_rect(self.id)
        {
            // SAFETY: repaints this thread's pane.
            unsafe { InvalidateRect(pane, &rc, 0) };
        }
        None
    }

    fn paint(&self, app: &App, hdc: HDC, rc: &RECT) {
        let pal = app.pal.get();
        fill(hdc, rc, pal.bg);
        centred(app, hdc, rc, self.id.title(), pal.dim);
        let pane = app.pane_hwnd(self.id);
        // SAFETY: reads this thread's focus; draws into the buffer.
        unsafe {
            if pane.is_some_and(|p| GetFocus() == p) {
                let inner = RECT {
                    left: rc.left + 2,
                    top: rc.top + 2,
                    right: rc.right - 2,
                    bottom: rc.bottom - 2,
                };
                SetTextColor(hdc, pal.fg);
                DrawFocusRect(hdc, &inner);
            }
        }
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// A placeholder row: its slot's name on the chrome, lined at its edge
/// towards the results. It answers the mouse so the strips' hooks can be
/// tried: held down it lights (holding the mouse, `App::capture_strip`),
/// each click it counts, and a right-click makes it twice as tall and back
/// (`App::relayout`).
pub struct TestStrip {
    pub slot: StripSlot,
    pressed: Cell<bool>,
    clicks: Cell<u32>,
    tall: Cell<bool>,
}

impl TestStrip {
    pub fn new(slot: StripSlot) -> TestStrip {
        TestStrip {
            slot,
            pressed: Cell::new(false),
            clicks: Cell::new(0),
            tall: Cell::new(false),
        }
    }
}

impl Strip for TestStrip {
    fn height(&self, app: &App) -> i32 {
        // Everything's tab strip (29 px) and find bar (30 px) at 100 %.
        let h = match self.slot {
            StripSlot::Tabs => 29,
            StripSlot::FindBar => 30,
        };
        app.scale(if self.tall.get() { 2 * h } else { h })
    }

    fn mouse(&self, app: &App, msg: u32, _w: WPARAM, _pt: POINT) -> bool {
        // SAFETY: takes and lets go of the mouse for this thread's window.
        unsafe {
            match msg {
                WM_LBUTTONDOWN => {
                    self.pressed.set(true);
                    SetCapture(app.hwnd);
                    app.capture_strip(Some(self.slot));
                }
                WM_LBUTTONUP if self.pressed.get() => {
                    self.pressed.set(false);
                    self.clicks.set(self.clicks.get() + 1);
                    app.capture_strip(None);
                    if GetCapture() == app.hwnd {
                        ReleaseCapture();
                    }
                }
                WM_CAPTURECHANGED => self.pressed.set(false),
                WM_RBUTTONUP => {
                    self.tall.set(!self.tall.get());
                    app.relayout();
                }
                _ => return false,
            }
        }
        app.invalidate_strip(self.slot);
        msg != WM_CAPTURECHANGED
    }

    fn paint(&self, app: &App, hdc: HDC, rc: &RECT) {
        let pal = app.pal.get();
        fill(
            hdc,
            rc,
            if self.pressed.get() {
                pal.header_hot
            } else {
                pal.chrome
            },
        );
        let line = match self.slot {
            StripSlot::Tabs => RECT {
                top: rc.bottom - 1,
                ..*rc
            },
            StripSlot::FindBar => RECT {
                bottom: rc.top + 1,
                ..*rc
            },
        };
        fill(hdc, &line, pal.line);
        let name = match self.slot {
            StripSlot::Tabs => "Tab strip (test)",
            StripSlot::FindBar => "Find bar (test)",
        };
        let label = match self.clicks.get() {
            0 => name.to_owned(),
            n => format!("{name}: {n} clicks"),
        };
        centred(app, hdc, rc, &label, pal.dim);
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn letters_ask_for_panes_and_rows() {
        assert_eq!(parse(""), Wanted::default());
        let w = parse("llRtf");
        assert_eq!(w.panes, [Side::Left, Side::Left, Side::Right]);
        assert!(w.tabs && w.find);
        // At most four panes; other letters do nothing.
        let w = parse("LRLRLRx1");
        assert_eq!(w.panes.len(), 4);
        assert!(!w.tabs && !w.find);
        let specs = specs(&parse("RL"));
        assert_eq!(specs.len(), 2);
        assert_eq!((specs[0].id, specs[0].side), (PaneId::Test(0), Side::Right));
        assert_eq!((specs[1].id, specs[1].order), (PaneId::Test(1), 1));
        assert_eq!(specs[1].title, "Test Pane 2");
    }
}
