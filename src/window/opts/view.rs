//! Options > View: the theme, On Top, the status bar and the Back and
//! Forward buttons (`theme`, `ontop`, `status_bar_visible`,
//! `toolbar_nav_bar_visible`), as the View menu sets them too.

use windows_sys::Win32::Foundation::HWND;

use crate::options::{
    Page, Rows, checked, combo_value, fill_combo, order, select_combo, set_check,
};
use crate::settings::{OnTop, Settings};
use crate::theme::ThemeMode;
use crate::window::App;

const THEME: u16 = 100;
const ON_TOP: u16 = 101;
const STATUS_BAR: u16 = 102;
const NAV_BAR: u16 = 103;

/// As View > Theme and View > On Top name them.
const THEMES: [(ThemeMode, &str); 3] = [
    (ThemeMode::UserDefault, "User Default"),
    (ThemeMode::Standard, "Standard"),
    (ThemeMode::Dark, "Dark"),
];
const ON_TOPS: [(OnTop, &str); 3] = [
    (OnTop::Never, "Never"),
    (OnTop::Always, "Always"),
    (OnTop::WhileSearching, "While Searching"),
];

pub(super) struct View;

impl Page for View {
    fn name(&self) -> &'static str {
        "View"
    }

    fn parent(&self) -> Option<&'static str> {
        Some("General")
    }

    fn order(&self) -> u32 {
        order::VIEW
    }

    fn build(&self, rows: &mut Rows<'_>) {
        rows.combo("&Theme:", THEME, 50, 100)
            .combo("On to&p:", ON_TOP, 50, 100)
            .skip(2)
            .check(STATUS_BAR, "Show status &bar")
            .check(NAV_BAR, "Show Back and &Forward buttons");
    }

    fn fill(&self, app: &App, page: HWND) {
        fill_combo(page, THEME, &THEMES, app.theme_mode());
        fill_combo(page, ON_TOP, &ON_TOPS, app.on_top.get());
        set_check(page, STATUS_BAR, app.status_bar_visible());
        set_check(page, NAV_BAR, app.nav_bar_visible());
    }

    fn apply(&self, app: &App, page: HWND, _problems: &mut Vec<String>) {
        let theme = combo_value(page, THEME, &THEMES);
        if theme != app.theme_mode() {
            app.set_theme_mode(theme);
        }
        let on_top = combo_value(page, ON_TOP, &ON_TOPS);
        if on_top != app.on_top.get() {
            app.set_on_top(on_top);
        }
        let status = checked(page, STATUS_BAR);
        if status != app.status_bar_visible() {
            app.set_status_bar_visible(status);
        }
        let nav = checked(page, NAV_BAR);
        if nav != app.nav_bar_visible() {
            app.set_nav_bar_visible(nav);
            app.prefs.borrow_mut().base.nav_bar = nav;
        }
    }

    fn defaults(&self, page: HWND) {
        let d = Settings::default();
        select_combo(page, THEME, &THEMES, d.theme);
        select_combo(page, ON_TOP, &ON_TOPS, d.on_top);
        set_check(page, STATUS_BAR, d.status_bar);
        set_check(page, NAV_BAR, d.nav_bar);
    }
}
