//! Options > View: how the results look, with Everything's labels, ids and
//! keys (`alternate_row_color`, `show_mouseover`,
//! `show_highlighted_search_terms`, `show_selected_item_in_statusbar`,
//! `show_size_in_statusbar`, `size_format`) and its `grid_lines`; then
//! what the View menu sets too: the theme, On Top, the status bar and the
//! Back and Forward buttons (`theme`, `ontop`, `status_bar_visible`,
//! `toolbar_nav_bar_visible`). Everything's "Size standard" would change
//! how gozd reads `size:` as well: not here.

use windows_sys::Win32::Foundation::HWND;

use crate::options::{
    Page, Rows, checked, combo_value, fill_combo, order, select_combo, set_check,
};
use crate::settings::{OnTop, Settings};
use crate::theme::ThemeMode;
use crate::ui_options::{SizeFormat, UiOptions};
use crate::window::App;

/// Everything's control ids (the rest are goz-gui's).
const ALTERNATE: u16 = 1082;
const MOUSEOVER: u16 = 1081;
const HIGHLIGHT: u16 = 1084;
const SELECTED_ITEM: u16 = 1085;
const TOTAL_SIZE: u16 = 1087;
const SIZE_FORMAT: u16 = 1088;
const GRID_LINES: u16 = 104;
const THEME: u16 = 100;
const ON_TOP: u16 = 101;
const STATUS_BAR: u16 = 102;
const NAV_BAR: u16 = 103;

/// Everything's list, in its order of `size_format`.
const SIZE_FORMATS: [(SizeFormat, &str); 4] = [
    (SizeFormat::Auto, "Auto"),
    (SizeFormat::Bytes, "bytes"),
    (SizeFormat::Kb, "KB"),
    (SizeFormat::Mb, "MB"),
];

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

fn show(page: HWND, ui: &UiOptions) {
    set_check(page, ALTERNATE, ui.alternate_rows);
    set_check(page, MOUSEOVER, ui.mouseover);
    set_check(page, HIGHLIGHT, ui.highlight);
    set_check(page, SELECTED_ITEM, ui.selected_in_status);
    set_check(page, TOTAL_SIZE, ui.size_in_status);
    set_check(page, GRID_LINES, ui.grid_lines);
    select_combo(page, SIZE_FORMAT, &SIZE_FORMATS, ui.size_format);
}

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
        rows.check(ALTERNATE, "Alternate row &color")
            .check(MOUSEOVER, "Show row &mouseover")
            .check(HIGHLIGHT, "Show &highlighted search terms")
            .check(SELECTED_ITEM, "Show selected item in status &bar")
            .check(TOTAL_SIZE, "Show total si&ze in status bar")
            .check(GRID_LINES, "Show &grid lines")
            .skip(2)
            .combo("Size &format:", SIZE_FORMAT, 50, 60)
            .skip(6)
            .combo("&Theme:", THEME, 50, 100)
            .combo("On to&p:", ON_TOP, 50, 100)
            .skip(2)
            .check(STATUS_BAR, "Show &status bar")
            .check(NAV_BAR, "Show Back and Forwar&d buttons");
    }

    fn fill(&self, app: &App, page: HWND) {
        let ui = app.ui();
        fill_combo(page, SIZE_FORMAT, &SIZE_FORMATS, ui.size_format);
        show(page, &ui);
        fill_combo(page, THEME, &THEMES, app.theme_mode());
        fill_combo(page, ON_TOP, &ON_TOPS, app.on_top.get());
        set_check(page, STATUS_BAR, app.status_bar_visible());
        set_check(page, NAV_BAR, app.nav_bar_visible());
    }

    fn apply(&self, app: &App, page: HWND, _problems: &mut Vec<String>) {
        app.set_ui(UiOptions {
            alternate_rows: checked(page, ALTERNATE),
            mouseover: checked(page, MOUSEOVER),
            highlight: checked(page, HIGHLIGHT),
            selected_in_status: checked(page, SELECTED_ITEM),
            size_in_status: checked(page, TOTAL_SIZE),
            grid_lines: checked(page, GRID_LINES),
            size_format: combo_value(page, SIZE_FORMAT, &SIZE_FORMATS),
            ..app.ui()
        });
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
        show(page, &UiOptions::default());
        let d = Settings::default();
        select_combo(page, THEME, &THEMES, d.theme);
        select_combo(page, ON_TOP, &ON_TOPS, d.on_top);
        set_check(page, STATUS_BAR, d.status_bar);
        set_check(page, NAV_BAR, d.nav_bar);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn size_formats_are_everythings_list() {
        let numbers: Vec<u8> = SIZE_FORMATS.iter().map(|(f, _)| f.number()).collect();
        assert_eq!(numbers, [0, 1, 2, 3]);
        assert!(
            SIZE_FORMATS
                .iter()
                .any(|(f, label)| *f == SizeFormat::default() && *label == "KB")
        );
    }
}
