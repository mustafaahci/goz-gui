//! Options > Search: what the first Enter in the search box does
//! (`search_edit_enter_action`).

use windows_sys::Win32::Foundation::HWND;

use crate::options::{Page, Rows, combo_value, fill_combo, order, select_combo};
use crate::settings::EnterAction;
use crate::window::App;

const ENTER: u16 = 100;

/// Everything's list, in its order of `search_edit_enter_action`.
const ENTER_LABELS: [(EnterAction, &str); 7] = [
    (EnterAction::MostRun, "Select the most run result"),
    (EnterAction::LastRun, "Select the last run result"),
    (EnterAction::Result, "Select the focused result"),
    (EnterAction::Top, "Select the top result"),
    (EnterAction::Bottom, "Select the bottom result"),
    (EnterAction::SelectSearch, "Select the search text"),
    (EnterAction::None, "Do nothing"),
];

pub(super) struct Search;

impl Page for Search {
    fn name(&self) -> &'static str {
        "Search"
    }

    fn parent(&self) -> Option<&'static str> {
        Some("General")
    }

    fn order(&self) -> u32 {
        order::SEARCH
    }

    fn build(&self, rows: &mut Rows<'_>) {
        rows.combo("&Enter in the search box:", ENTER, 92, 130);
    }

    fn fill(&self, app: &App, page: HWND) {
        fill_combo(page, ENTER, &ENTER_LABELS, app.enter_action());
    }

    fn apply(&self, app: &App, page: HWND, _problems: &mut Vec<String>) {
        let enter = combo_value(page, ENTER, &ENTER_LABELS);
        app.set_enter_action(enter);
        app.prefs.borrow_mut().base.enter_action = enter;
    }

    fn defaults(&self, page: HWND) {
        select_combo(page, ENTER, &ENTER_LABELS, EnterAction::default());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_choice_has_a_label() {
        assert_eq!(ENTER_LABELS.len(), 7);
        for a in [
            EnterAction::None,
            EnterAction::MostRun,
            EnterAction::LastRun,
            EnterAction::Result,
            EnterAction::Top,
            EnterAction::Bottom,
            EnterAction::SelectSearch,
        ] {
            assert!(ENTER_LABELS.iter().any(|(v, _)| *v == a));
        }
        // The default is first, as Everything lists it.
        assert_eq!(ENTER_LABELS[0].0, EnterAction::default());
    }
}
