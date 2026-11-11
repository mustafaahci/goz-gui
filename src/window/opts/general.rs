//! Options > General: what goz-gui puts in Windows for the user (see
//! `crate::integration`): starting with Windows, "Search goz..." on folders
//! in Explorer, the Start menu and Desktop shortcuts and the `goz:` URL
//! protocol, and (wave 4) the `.efu` file association, with Everything's
//! labels ("Start Everything on system
//! startup", "Show Search Everything folder context menu item", "Start
//! menu shortcut", "Desktop shortcut", "es: URL protocol") and ids. Each
//! box shows what is there now, not a setting: another program, or a copy
//! of goz-gui elsewhere, may have changed it.

use windows_sys::Win32::Foundation::HWND;

use crate::integration::{self, Shortcut};
use crate::options::{Page, Rows, checked, order, set_check};
use crate::window::App;

/// Everything's control ids.
const STARTUP: u16 = 1059;
const FOLDER_ITEM: u16 = 1061;
const START_MENU: u16 = 1056;
const DESKTOP: u16 = 1058;
const URL: u16 = 1062;
const EFU: u16 = 1063;

pub(super) struct General;

/// Each box: whether the thing is there, how to put it there or take it
/// away, and what to say when that fails.
struct Item {
    id: u16,
    has: fn() -> bool,
    set: fn(bool) -> bool,
    failed: &'static str,
}

const ITEMS: [Item; 6] = [
    Item {
        id: STARTUP,
        has: integration::starts_with_windows,
        set: integration::set_starts_with_windows,
        failed: "Starting with Windows could not be changed",
    },
    Item {
        id: FOLDER_ITEM,
        has: integration::has_folder_item,
        set: integration::set_folder_item,
        failed: "The folder item in Explorer could not be changed",
    },
    Item {
        id: START_MENU,
        has: || integration::has_shortcut(Shortcut::StartMenu),
        set: |on| integration::set_shortcut(Shortcut::StartMenu, on),
        failed: "The Start menu shortcut could not be changed",
    },
    Item {
        id: DESKTOP,
        has: || integration::has_shortcut(Shortcut::Desktop),
        set: |on| integration::set_shortcut(Shortcut::Desktop, on),
        failed: "The Desktop shortcut could not be changed",
    },
    Item {
        id: URL,
        has: integration::has_url_protocol,
        set: integration::set_url_protocol,
        failed: "The goz: URL protocol could not be changed",
    },
    // W4 (G1).
    Item {
        id: EFU,
        has: integration::has_efu_association,
        set: integration::set_efu_association,
        failed: "The EFU file association could not be changed",
    },
];

impl Page for General {
    fn name(&self) -> &'static str {
        "General"
    }

    fn order(&self) -> u32 {
        order::GENERAL
    }

    fn build(&self, rows: &mut Rows<'_>) {
        rows.check(STARTUP, "&Start goz-gui on system startup")
            .check(
                FOLDER_ITEM,
                "Show \"Search goz...\" folder context &menu item",
            )
            .check(START_MENU, "S&tart menu shortcut")
            .check(DESKTOP, "D&esktop shortcut")
            .check(URL, "goz: URL &protocol")
            .check(EFU, "EFU &file association");
    }

    fn fill(&self, _app: &App, page: HWND) {
        for item in &ITEMS {
            set_check(page, item.id, (item.has)());
        }
    }

    /// Only what was changed is touched: a box left as it was leaves what
    /// is there alone.
    fn apply(&self, _app: &App, page: HWND, problems: &mut Vec<String>) {
        for item in &ITEMS {
            let on = checked(page, item.id);
            if on != (item.has)() && !(item.set)(on) {
                problems.push(item.failed.into());
            }
        }
    }

    /// Everything's: none of them.
    fn defaults(&self, page: HWND) {
        for item in &ITEMS {
            set_check(page, item.id, false);
        }
    }
}
