//! Options > General: starting with Windows, and "Search goz..." on folders
//! in Explorer (Everything's "Start Everything on system startup" and
//! "Show Search Everything folder context menu item").

use windows_sys::Win32::Foundation::HWND;

use crate::integration;
use crate::options::{Page, Rows, checked, order, set_check};
use crate::window::App;

/// Everything's control ids.
const STARTUP: u16 = 1059;
const FOLDER_ITEM: u16 = 1061;

pub(super) struct General;

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
            );
    }

    fn fill(&self, _app: &App, page: HWND) {
        set_check(page, STARTUP, integration::starts_with_windows());
        set_check(page, FOLDER_ITEM, integration::has_folder_item());
    }

    fn apply(&self, _app: &App, page: HWND, problems: &mut Vec<String>) {
        let startup = checked(page, STARTUP);
        if startup != integration::starts_with_windows()
            && !integration::set_starts_with_windows(startup)
        {
            problems.push("Starting with Windows could not be changed".into());
        }
        let folder = checked(page, FOLDER_ITEM);
        if folder != integration::has_folder_item() && !integration::set_folder_item(folder) {
            problems.push("The folder item in Explorer could not be changed".into());
        }
    }

    /// Everything's: neither.
    fn defaults(&self, page: HWND) {
        set_check(page, STARTUP, false);
        set_check(page, FOLDER_ITEM, false);
    }
}
