//! Options > History: the search and run histories, with Everything's
//! labels and keys (`search_history_enabled`, `_keep_forever`,
//! `_days_to_keep`, `search_history_always_suggest`, and the run history's).
//! They take effect the next time goz-gui starts, but for the suggestions.

use windows_sys::Win32::Foundation::HWND;

use crate::options::{
    Invalid, Page, Rows, checked, days, enable, order, set_check, set_text, text,
};
use crate::settings::HistoryOptions;
use crate::window::App;

/// Everything's control ids.
const SEARCH_HISTORY: u16 = 1172;
const SEARCH_LIMIT: u16 = 1173;
const SEARCH_DAYS: u16 = 1174;
const SUGGEST: u16 = 1177;
const RUN_HISTORY: u16 = 1181;
const RUN_LIMIT: u16 = 1182;
const RUN_DAYS: u16 = 1183;

/// The days a history can keep.
const MAX_DAYS: u32 = 36_500;

pub(super) struct History;

fn show(page: HWND, h: &HistoryOptions) {
    set_check(page, SEARCH_HISTORY, h.search_enabled);
    set_check(page, SEARCH_LIMIT, !h.search_keep_forever);
    set_text(page, SEARCH_DAYS, &h.search_days_to_keep.to_string());
    set_check(page, SUGGEST, h.search_always_suggest);
    set_check(page, RUN_HISTORY, h.run_enabled);
    set_check(page, RUN_LIMIT, !h.run_keep_forever);
    set_text(page, RUN_DAYS, &h.run_days_to_keep.to_string());
}

/// Whether `text` is a number of days a history can keep.
fn days_ok(text: &str) -> bool {
    text.trim()
        .parse::<u32>()
        .is_ok_and(|d| (1..=MAX_DAYS).contains(&d))
}

impl Page for History {
    fn name(&self) -> &'static str {
        "History"
    }

    fn order(&self) -> u32 {
        order::HISTORY
    }

    fn build(&self, rows: &mut Rows<'_>) {
        rows.group("Search History", |r| {
            r.check(SEARCH_HISTORY, "&Enable search history")
                .check_number(
                    SEARCH_LIMIT,
                    "&Keep search history for",
                    90,
                    SEARCH_DAYS,
                    "days",
                )
                .check(SUGGEST, "Always show search su&ggestions");
        })
        .group("Run History", |r| {
            r.check(RUN_HISTORY, "Enable run &history").check_number(
                RUN_LIMIT,
                "Keep run history &for",
                90,
                RUN_DAYS,
                "days",
            );
        })
        .text(
            "Turning a history on or off, and how long it keeps, take effect the \
             next time goz-gui starts.",
            2,
        );
    }

    fn fill(&self, app: &App, page: HWND) {
        show(page, &app.prefs.borrow().base.history);
    }

    /// The days need their limit, the limit and the suggestions their
    /// history.
    fn update_enabled(&self, page: HWND) {
        let search = checked(page, SEARCH_HISTORY);
        enable(page, SEARCH_LIMIT, search);
        enable(page, SEARCH_DAYS, search && checked(page, SEARCH_LIMIT));
        enable(page, SUGGEST, search);
        let run = checked(page, RUN_HISTORY);
        enable(page, RUN_LIMIT, run);
        enable(page, RUN_DAYS, run && checked(page, RUN_LIMIT));
    }

    fn validate(&self, page: HWND) -> Result<(), Invalid> {
        for (history, limit, days) in [
            (SEARCH_HISTORY, SEARCH_LIMIT, SEARCH_DAYS),
            (RUN_HISTORY, RUN_LIMIT, RUN_DAYS),
        ] {
            if checked(page, history) && checked(page, limit) && !days_ok(&text(page, days)) {
                return Err(Invalid {
                    control: days,
                    message: format!("Enter a number of days from 1 to {MAX_DAYS}."),
                });
            }
        }
        Ok(())
    }

    fn apply(&self, app: &App, page: HWND, _problems: &mut Vec<String>) {
        let suggest = checked(page, SUGGEST);
        {
            let mut prefs = app.prefs.borrow_mut();
            let h = &mut prefs.base.history;
            h.search_enabled = checked(page, SEARCH_HISTORY);
            h.search_keep_forever = !checked(page, SEARCH_LIMIT);
            h.search_days_to_keep = days(&text(page, SEARCH_DAYS), h.search_days_to_keep);
            h.search_always_suggest = suggest;
            h.run_enabled = checked(page, RUN_HISTORY);
            h.run_keep_forever = !checked(page, RUN_LIMIT);
            h.run_days_to_keep = days(&text(page, RUN_DAYS), h.run_days_to_keep);
        }
        app.set_suggest(suggest);
    }

    fn defaults(&self, page: HWND) {
        show(page, &HistoryOptions::default());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn days_must_be_a_number_a_history_can_keep() {
        assert!(days_ok("90") && days_ok(" 1 ") && days_ok("36500"));
        assert!(!days_ok("") && !days_ok("0") && !days_ok("36501") && !days_ok("x"));
    }
}
