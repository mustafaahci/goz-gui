//! Options > UI: the UI scale (`dpi`), running in the background, the tray
//! icon and new windows, with Everything's labels, ids and keys
//! (`run_in_background`, `show_tray_icon`, `single_click_tray`,
//! `allow_multiple_windows_from_tray`, `allow_multiple_windows`), goz-gui's
//! place for `minimize_to_tray`, and Everything's `double_ctrl_action`.

use windows_sys::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows_sys::Win32::System::SystemServices::SS_LEFT;
use windows_sys::Win32::UI::Controls::{TBM_SETPOS, TBM_SETRANGE, TBS_AUTOTICKS};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    ES_AUTOHSCROLL, ES_NUMBER, WM_HSCROLL, WS_EX_CLIENTEDGE, WS_TABSTOP,
};

use crate::custom::{self, DoubleCtrl, SCALE_MAX, SCALE_MIN};
use crate::dialog::Class;
use crate::options::{
    Invalid, LABEL, Page, Rows, checked, combo_value, enable, fill_combo, item, order,
    select_combo, send, set_check, set_text, text,
};
use crate::settings::Background;
use crate::ui_options::UiOptions;
use crate::window::App;

/// Everything's control ids (Minimize to tray and Double Ctrl are
/// goz-gui's).
const SCALE_SLIDER: u16 = 1536;
const SCALE: u16 = 1537;
const BACKGROUND: u16 = 1066;
const TRAY: u16 = 1069;
const SINGLE_CLICK: u16 = 1071;
const NEW_FROM_TRAY: u16 = 1068;
const NEW_ON_START: u16 = 1067;
const MINIMIZE: u16 = 1100;
const DOUBLE_CTRL: u16 = 1101;

/// The scale's slider moves in Everything's 5 % steps.
const STEP: u32 = 5;
/// A trackbar's position (`TBM_GETPOS`, `WM_USER`), which windows-sys lacks.
const TBM_GETPOS: u32 = 0x400;
/// What the box takes (Everything's `dpi` from 50 % to 400 %).
const TYPED_MIN: u32 = 50;
const TYPED_MAX: u32 = 400;

const ACTIONS: [(DoubleCtrl, &str); 4] = [
    (DoubleCtrl::None, "None"),
    (DoubleCtrl::Show, "Show window"),
    (DoubleCtrl::NewWindow, "New window"),
    (DoubleCtrl::Toggle, "Toggle window"),
];

pub(super) struct Ui;

fn show(page: HWND, b: &Background, ui: &UiOptions) {
    set_check(page, BACKGROUND, b.run_in_background);
    set_check(page, TRAY, b.show_tray_icon);
    set_check(page, SINGLE_CLICK, b.single_click_tray);
    set_check(page, NEW_FROM_TRAY, ui.new_window_from_tray);
    set_check(page, MINIMIZE, b.minimize_to_tray);
    set_check(page, NEW_ON_START, ui.new_window_on_start);
}

fn show_scale(page: HWND, percent: u32) {
    set_text(page, SCALE, &percent.to_string());
    let pos = (percent.clamp(SCALE_MIN, SCALE_MAX) + STEP / 2) / STEP;
    send(page, SCALE_SLIDER, TBM_SETPOS, 1, pos as isize);
}

/// The percent typed, if one the scale takes.
fn typed_percent(t: &str) -> Option<u32> {
    t.trim()
        .parse::<u32>()
        .ok()
        .filter(|p| (TYPED_MIN..=TYPED_MAX).contains(p))
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
        rows.control(Class::Static, "&Scale:", LABEL, SS_LEFT, 0, (0, 3, 30, 9))
            .control(
                Class::Named("msctls_trackbar32"),
                "",
                SCALE_SLIDER,
                WS_TABSTOP | TBS_AUTOTICKS,
                0,
                (30, 0, 100, 15),
            )
            .control(
                Class::Edit,
                "",
                SCALE,
                WS_TABSTOP | (ES_AUTOHSCROLL | ES_NUMBER) as u32,
                WS_EX_CLIENTEDGE,
                (134, 1, 30, 13),
            )
            .control(Class::Static, "%", LABEL, SS_LEFT, 0, (168, 3, 12, 9))
            .skip(19);
        rows.check(BACKGROUND, "Run in &background")
            .check(TRAY, "Show tray &icon")
            .check(SINGLE_CLICK, "Single click &tray")
            .check(NEW_FROM_TRAY, "Create a new &window from the system tray")
            .check(MINIMIZE, "&Minimize to tray")
            .check(NEW_ON_START, "Create a &new window when running goz-gui")
            .skip(4)
            .combo("Double &Ctrl:", DOUBLE_CTRL, 60, 90);
    }

    fn fill(&self, app: &App, page: HWND) {
        show(page, &app.background(), &app.ui());
        let c = app.custom_settings();
        send(
            page,
            SCALE_SLIDER,
            TBM_SETRANGE,
            1,
            (((SCALE_MAX / STEP) << 16) | (SCALE_MIN / STEP)) as isize,
        );
        show_scale(page, custom::scale_percent(c.dpi));
        fill_combo(page, DOUBLE_CTRL, &ACTIONS, c.double_ctrl);
    }

    /// The tray's options need the tray icon; Ctrl pressed twice is the
    /// first window's, as the hotkeys are.
    fn update_enabled(&self, page: HWND) {
        let tray = checked(page, TRAY);
        enable(page, SINGLE_CLICK, tray);
        enable(page, NEW_FROM_TRAY, tray);
        enable(page, MINIMIZE, tray);
    }

    /// The slider moved: the box follows (and that is the change).
    fn message(&self, _app: &App, page: HWND, msg: u32, _w: WPARAM, l: LPARAM) -> Option<isize> {
        if msg == WM_HSCROLL && l as HWND == item(page, SCALE_SLIDER) {
            let pos = send(page, SCALE_SLIDER, TBM_GETPOS, 0, 0).max(0) as u32;
            set_text(page, SCALE, &(pos * STEP).to_string());
            return Some(0);
        }
        None
    }

    fn validate(&self, page: HWND) -> Result<(), Invalid> {
        match typed_percent(&text(page, SCALE)) {
            Some(_) => Ok(()),
            None => Err(Invalid {
                control: SCALE,
                message: format!("Type a scale from {TYPED_MIN} to {TYPED_MAX} %."),
            }),
        }
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
        if let Some(p) = typed_percent(&text(page, SCALE)) {
            let old = app.custom_settings().dpi;
            // 100 % stays as the file had it (96, or Everything's 0).
            if custom::scale_percent(old) != p {
                app.set_ui_scale(custom::dpi_of_percent(p));
            }
        }
        app.set_double_ctrl(combo_value(page, DOUBLE_CTRL, &ACTIONS));
    }

    /// Everything's: 100 %, all off, no double-Ctrl action.
    fn defaults(&self, page: HWND) {
        show(page, &Background::default(), &UiOptions::default());
        show_scale(page, 100);
        select_combo(page, DOUBLE_CTRL, &ACTIONS, DoubleCtrl::None);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_scale_box_takes_everythings_range() {
        assert_eq!(typed_percent("125"), Some(125));
        assert_eq!(typed_percent(" 50 "), Some(50));
        assert_eq!(typed_percent("49"), None);
        assert_eq!(typed_percent("401"), None);
        assert_eq!(typed_percent(""), None);
        // Everything's `double_ctrl_action` 0 to 3, in its order.
        for (i, (a, _)) in ACTIONS.iter().enumerate() {
            assert_eq!(usize::from(a.number()), i);
        }
    }
}
