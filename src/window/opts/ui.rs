//! Options > UI: running in the background, the tray icon and new windows,
//! with Everything's labels, ids and keys (`run_in_background`,
//! `show_tray_icon`, `single_click_tray`,
//! `allow_multiple_windows_from_tray`, `allow_multiple_windows`), and
//! goz-gui's place for `minimize_to_tray`.

use windows_sys::Win32::Foundation::HWND;

use crate::options::{Page, Rows, checked, enable, order, set_check};
use crate::settings::Background;
use crate::ui_options::UiOptions;
use crate::window::App;

/// Everything's control ids (Minimize to tray is goz-gui's).
const BACKGROUND: u16 = 1066;
const TRAY: u16 = 1069;
const SINGLE_CLICK: u16 = 1071;
const NEW_FROM_TRAY: u16 = 1068;
const NEW_ON_START: u16 = 1067;
const MINIMIZE: u16 = 1100;

pub(super) struct Ui;

fn show(page: HWND, b: &Background, ui: &UiOptions) {
    set_check(page, BACKGROUND, b.run_in_background);
    set_check(page, TRAY, b.show_tray_icon);
    set_check(page, SINGLE_CLICK, b.single_click_tray);
    set_check(page, NEW_FROM_TRAY, ui.new_window_from_tray);
    set_check(page, MINIMIZE, b.minimize_to_tray);
    set_check(page, NEW_ON_START, ui.new_window_on_start);
}

impl Page for Ui {
    fn name(&self) -> &'static str {
        "UI"
    }

    fn parent(&self) -> Option<&'static str> {
        Some("General")
    }

    fn order(&self) -> u32 {
        order::UI
    }

    fn build(&self, rows: &mut Rows<'_>) {
        rows.check(BACKGROUND, "Run in &background")
            .check(TRAY, "Show tray &icon")
            .check(SINGLE_CLICK, "Single click &tray")
            .check(NEW_FROM_TRAY, "Create a new &window from the system tray")
            .check(MINIMIZE, "&Minimize to tray")
            .check(NEW_ON_START, "Create a &new window when running goz-gui");
    }

    fn fill(&self, app: &App, page: HWND) {
        show(page, &app.background(), &app.ui());
    }

    /// The tray's options need the tray icon.
    fn update_enabled(&self, page: HWND) {
        let tray = checked(page, TRAY);
        enable(page, SINGLE_CLICK, tray);
        enable(page, NEW_FROM_TRAY, tray);
        enable(page, MINIMIZE, tray);
    }

    fn apply(&self, app: &App, page: HWND, problems: &mut Vec<String>) {
        app.change_background(
            |b| {
                b.run_in_background = checked(page, BACKGROUND);
                b.show_tray_icon = checked(page, TRAY);
                b.single_click_tray = checked(page, SINGLE_CLICK);
                b.minimize_to_tray = checked(page, MINIMIZE);
            },
            problems,
        );
        app.set_ui(UiOptions {
            new_window_from_tray: checked(page, NEW_FROM_TRAY),
            new_window_on_start: checked(page, NEW_ON_START),
            ..app.ui()
        });
    }

    /// Everything's: all off.
    fn defaults(&self, page: HWND) {
        show(page, &Background::default(), &UiOptions::default());
    }
}
