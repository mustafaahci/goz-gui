//! Options > Keyboard: the global hotkeys that show the window or toggle it
//! (`show_window_key`, `toggle_window_key`), with Everything's labels.

use windows_sys::Win32::Foundation::HWND;
use windows_sys::Win32::UI::Controls::{HKM_GETHOTKEY, HKM_SETHOTKEY};

use crate::options::{Page, Rows, order, send};
use crate::window::App;

/// Everything's control ids.
const SHOW_KEY: u16 = 1165;
const TOGGLE_KEY: u16 = 1166;

pub(super) struct Keyboard;

fn hotkey(page: HWND, id: u16) -> u16 {
    send(page, id, HKM_GETHOTKEY, 0, 0) as u16
}

impl Page for Keyboard {
    fn name(&self) -> &'static str {
        "Keyboard"
    }

    fn parent(&self) -> Option<&'static str> {
        Some("General")
    }

    fn order(&self) -> u32 {
        order::KEYBOARD
    }

    fn build(&self, rows: &mut Rows<'_>) {
        rows.hotkey("&Show window Hotkey:", SHOW_KEY, 94, 163)
            .hotkey("&Toggle window Hotkey:", TOGGLE_KEY, 94, 163);
    }

    fn fill(&self, app: &App, page: HWND) {
        let b = app.background();
        send(
            page,
            SHOW_KEY,
            HKM_SETHOTKEY,
            usize::from(b.show_window_key),
            0,
        );
        send(
            page,
            TOGGLE_KEY,
            HKM_SETHOTKEY,
            usize::from(b.toggle_window_key),
            0,
        );
    }

    /// A hotkey box tells its parent of a change as EN_CHANGE.
    fn changed(&self, _page: HWND, id: u16, _code: u32) -> bool {
        id == SHOW_KEY || id == TOGGLE_KEY
    }

    fn apply(&self, app: &App, page: HWND, problems: &mut Vec<String>) {
        app.change_background(
            |b| {
                b.show_window_key = hotkey(page, SHOW_KEY);
                b.toggle_window_key = hotkey(page, TOGGLE_KEY);
            },
            problems,
        );
    }

    /// Everything's: none.
    fn defaults(&self, page: HWND) {
        send(page, SHOW_KEY, HKM_SETHOTKEY, 0, 0);
        send(page, TOGGLE_KEY, HKM_SETHOTKEY, 0, 0);
    }
}
