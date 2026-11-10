//! Options > Results: how the results list acts, with Everything's labels,
//! ids and keys (`hide_empty_search_results`, `clear_selection_on_search`,
//! `close_on_execute`, `double_click_path2`, `double_quote_copy_as_path`,
//! `rename_select_filepart_only`, `date_descending_first`,
//! `size_descending_first`, `full_row_select`, `single_click_open`), and
//! goz-gui's live updates (`live_update_interval_ms`). Everything's
//! "Automatically scroll view" and "Underline icon titles" are the list
//! control's own doing, which goz-gui leaves as Windows has it: not here.

use windows_sys::Win32::Foundation::HWND;
use windows_sys::Win32::UI::WindowsAndMessaging::KillTimer;

use crate::options::{
    Invalid, Page, Rows, checked, combo_value, enable, fill_combo, order, select_combo, set_check,
    set_text, text,
};
use crate::settings::{LIVE_UPDATE_MIN, LIVE_UPDATE_MS};
use crate::ui_options::{ClickOpen, UiOptions};
use crate::window::App;

/// Everything's control ids (the live updates' are goz-gui's).
const HIDE_EMPTY: u16 = 1128;
const CLEAR_SELECTION: u16 = 1129;
const CLOSE_ON_EXECUTE: u16 = 1133;
const DOUBLE_CLICK_PATH: u16 = 1134;
const QUOTE_COPY: u16 = 1136;
const RENAME_STEM: u16 = 1137;
const DATE_DESCENDING: u16 = 1130;
const SIZE_DESCENDING: u16 = 1131;
const FULL_ROW: u16 = 1079;
const CLICK_OPEN: u16 = 1076;
const LIVE: u16 = 1200;
const LIVE_MS: u16 = 1201;

/// The longest live update interval taken: an hour.
const LIVE_MAX: u32 = 3_600_000;

/// Everything's list, in its order of `single_click_open`.
const CLICK_LABELS: [(ClickOpen, &str); 3] = [
    (ClickOpen::System, "Use system settings"),
    (ClickOpen::Single, "Single click"),
    (ClickOpen::Double, "Double click"),
];

pub(super) struct Results;

fn show(page: HWND, ui: &UiOptions, live_ms: u32) {
    set_check(page, HIDE_EMPTY, ui.hide_empty);
    set_check(page, CLEAR_SELECTION, ui.clear_selection);
    set_check(page, CLOSE_ON_EXECUTE, ui.close_on_execute);
    set_check(page, DOUBLE_CLICK_PATH, ui.double_click_path);
    set_check(page, QUOTE_COPY, ui.quote_copy_path);
    set_check(page, RENAME_STEM, ui.rename_stem.on());
    set_check(page, DATE_DESCENDING, ui.date_descending.on());
    set_check(page, SIZE_DESCENDING, ui.size_descending.on());
    set_check(page, FULL_ROW, ui.full_row_select);
    select_combo(page, CLICK_OPEN, &CLICK_LABELS, ui.click_open);
    set_check(page, LIVE, live_ms > 0);
    let shown = if live_ms > 0 { live_ms } else { LIVE_UPDATE_MS };
    set_text(page, LIVE_MS, &shown.to_string());
}

/// A live update interval as typed, in milliseconds, if it is one.
fn interval(text: &str) -> Option<u32> {
    text.trim()
        .parse::<u32>()
        .ok()
        .filter(|ms| (LIVE_UPDATE_MIN..=LIVE_MAX).contains(ms))
}

impl Page for Results {
    fn name(&self) -> &'static str {
        "Results"
    }

    fn parent(&self) -> Option<&'static str> {
        Some("General")
    }

    fn order(&self) -> u32 {
        order::RESULTS
    }

    fn build(&self, rows: &mut Rows<'_>) {
        rows.check(HIDE_EMPTY, "&Hide results when the search is empty")
            .check(CLEAR_SELECTION, "Clear selectio&n on search")
            .check(CLOSE_ON_EXECUTE, "&Close window on execute")
            .check(
                DOUBLE_CLICK_PATH,
                "Open &path with double click in path column",
            )
            .check(QUOTE_COPY, "Double &quote copy as path")
            .check(RENAME_STEM, "Do not select &extension when renaming")
            .check(DATE_DESCENDING, "Sort &date descending first")
            .check(SIZE_DESCENDING, "Sort &size descending first")
            .check(FULL_ROW, "&Full row select")
            .skip(2)
            .combo("Single click &open:", CLICK_OPEN, 72, 92)
            .check_number(
                LIVE,
                "Update the results &live every",
                102,
                LIVE_MS,
                "milliseconds",
            );
    }

    fn fill(&self, app: &App, page: HWND) {
        fill_combo(page, CLICK_OPEN, &CLICK_LABELS, ClickOpen::System);
        let live = app.prefs.borrow().base.live_update_ms;
        show(page, &app.ui(), live);
    }

    fn update_enabled(&self, page: HWND) {
        enable(page, LIVE_MS, checked(page, LIVE));
    }

    fn validate(&self, page: HWND) -> Result<(), Invalid> {
        if checked(page, LIVE) && interval(&text(page, LIVE_MS)).is_none() {
            return Err(Invalid {
                control: LIVE_MS,
                message: format!(
                    "Enter a live update interval from {LIVE_UPDATE_MIN} to {LIVE_MAX} milliseconds."
                ),
            });
        }
        Ok(())
    }

    fn apply(&self, app: &App, page: HWND, _problems: &mut Vec<String>) {
        let ui = app.ui();
        app.set_ui(UiOptions {
            hide_empty: checked(page, HIDE_EMPTY),
            clear_selection: checked(page, CLEAR_SELECTION),
            close_on_execute: checked(page, CLOSE_ON_EXECUTE),
            double_click_path: checked(page, DOUBLE_CLICK_PATH),
            quote_copy_path: checked(page, QUOTE_COPY),
            rename_stem: ui.rename_stem.with(checked(page, RENAME_STEM)),
            date_descending: ui.date_descending.with(checked(page, DATE_DESCENDING)),
            size_descending: ui.size_descending.with(checked(page, SIZE_DESCENDING)),
            full_row_select: checked(page, FULL_ROW),
            click_open: combo_value(page, CLICK_OPEN, &CLICK_LABELS),
            ..ui
        });
        let live = if checked(page, LIVE) {
            interval(&text(page, LIVE_MS)).unwrap_or(LIVE_UPDATE_MS)
        } else {
            0
        };
        app.set_live_interval(live);
        // A hidden empty search, or one no longer hidden, shows at once.
        if ui.hide_empty != app.ui().hide_empty && !app.search_is_dirty() {
            app.start_search(true);
        }
    }

    /// Everything's, and goz-gui's live updates every 2 s.
    fn defaults(&self, page: HWND) {
        show(page, &UiOptions::default(), LIVE_UPDATE_MS);
    }
}

impl App {
    /// Options > Results' live updates: every `ms` milliseconds, or never
    /// (0); the timer follows at once.
    fn set_live_interval(&self, ms: u32) {
        let old = {
            let mut prefs = self.prefs.borrow_mut();
            std::mem::replace(&mut prefs.base.live_update_ms, ms)
        };
        if old == ms {
            return;
        }
        if ms == 0 {
            // SAFETY: a timer of this window.
            unsafe { KillTimer(self.hwnd, super::super::TIMER_LIVE) };
        } else {
            self.arm_live();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn live_intervals_are_half_a_second_to_an_hour() {
        assert_eq!(interval("2000"), Some(2000));
        assert_eq!(interval(" 500 "), Some(500));
        assert_eq!(interval("499"), None);
        assert_eq!(interval("3600000"), Some(3_600_000));
        assert_eq!(interval("3600001"), None);
        assert_eq!(interval("x"), None);
    }

    #[test]
    fn every_click_choice_has_its_label() {
        for c in ClickOpen::ALL {
            assert!(CLICK_LABELS.iter().any(|(v, _)| *v == c));
        }
        assert_eq!(CLICK_LABELS[0].0, ClickOpen::default());
    }
}
