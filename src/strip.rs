//! The two rows the framework keeps for features: the tab strip, between
//! the menu bar and the search row, and the find bar, under the results
//! (between the docks, as Everything 1.5's). A feature installs its
//! [`Strip`] in a slot (`App::set_strip`); the window gives the row the
//! height the strip asks for (none while it asks for 0), paints it through
//! the strip into its own double buffer, and hands it the mouse over it.
//! A strip with windows of its own (an edit box, buttons) places them in
//! `place`.

use std::any::Any;

use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows_sys::Win32::Graphics::Gdi::HDC;
use windows_sys::Win32::UI::WindowsAndMessaging::MSG;

use crate::paint::fill;
use crate::window::App;

/// Which row.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StripSlot {
    /// Above the search row, under the menu bar.
    Tabs,
    /// Under the results, between the docks.
    FindBar,
}

/// What a feature puts in a slot. Every method runs on the UI thread with
/// nothing of the framework borrowed (see `crate::pane` on re-entrancy).
pub trait Strip: Any {
    /// Its height at the window's DPI now; 0 takes the row away. Asked at
    /// every layout: call `App::relayout` when it changes.
    fn height(&self, app: &App) -> i32;

    /// It was laid out at `rc` (main window client coordinates): its own
    /// windows go there.
    fn place(&self, _app: &App, _rc: RECT) {}

    /// Paints the row `rc` into `hdc`, the main window's paint buffer
    /// (client coordinates; only the part being repainted reaches the
    /// screen). By default the window's chrome.
    fn paint(&self, app: &App, hdc: HDC, rc: &RECT) {
        fill(hdc, rc, app.pal.get().chrome);
    }

    /// A mouse message of the main window (`WM_MOUSEMOVE`, `WM_*BUTTON*`,
    /// `WM_MOUSELEAVE`, `WM_CAPTURECHANGED`) over the row, or anywhere
    /// while the strip holds the mouse (`App::capture_strip`); `pt` in
    /// client coordinates. True when it took a button message (the window
    /// does nothing else with it); moves and leaves go on to the rest of
    /// the window either way.
    fn mouse(&self, _app: &App, _msg: u32, _w: WPARAM, _pt: POINT) -> bool {
        false
    }

    /// The window F6 gives the keyboard to; `None`: not a frame.
    fn focus(&self) -> Option<HWND> {
        None
    }

    /// Whether `hwnd` is one of the strip's windows (it has the keyboard).
    fn contains(&self, _hwnd: HWND) -> bool {
        false
    }

    /// Tab or Shift+Tab in `from`, one of its windows: true when the strip
    /// moved the keyboard itself.
    fn tab(&self, _app: &App, _from: HWND, _forward: bool) -> bool {
        false
    }

    /// A message one of its own controls sent the main window (their
    /// parent): `WM_COMMAND`, `WM_NOTIFY`, `WM_CTLCOLOR*`, `WM_DRAWITEM`.
    /// `Some` is the answer; `None` leaves it to the window.
    fn message(&self, _app: &App, _msg: u32, _w: WPARAM, _l: LPARAM) -> Option<LRESULT> {
        None
    }

    /// A key message for one of its windows before the window's shortcuts
    /// and Tab: true when the strip took it (`PaneContent::key`).
    fn key(&self, _app: &App, _msg: &MSG) -> bool {
        false
    }

    /// The theme changed; the row repaints.
    fn theme(&self, _app: &App, _dark: bool) {}

    /// The window's DPI changed; a layout follows.
    fn dpi(&self, _app: &App, _dpi: u32) {}

    /// The strip as `Any`, so its feature can reach it again
    /// (`App::strip(slot)` then `as_any().downcast_ref::<T>()`).
    #[allow(
        dead_code,
        reason = "the features' way back to their strips (FRAMEWORK.md)"
    )]
    fn as_any(&self) -> &dyn Any;
}
