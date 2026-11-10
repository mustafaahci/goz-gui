//! Options > Search: how the search box searches, with Everything's labels,
//! ids and keys (`search_as_you_type`, `search_delay`,
//! `select_search_on_mouse_click`, `focus_search_on_activate`), and what
//! the first Enter in it does (`search_edit_enter_action`, goz-gui's place
//! for it). Everything's other Search options change how a search is
//! understood, which is gozd's: they are not here.

use windows_sys::Win32::Foundation::HWND;
use windows_sys::Win32::System::SystemServices::SS_LEFT;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    ES_AUTOHSCROLL, ES_NUMBER, WS_EX_CLIENTEDGE, WS_TABSTOP,
};

use crate::dialog::Class;
use crate::options::{
    Invalid, LABEL, Page, Rows, checked, combo_value, enable, fill_combo, order, select_combo,
    set_check, set_text, text,
};
use crate::settings::EnterAction;
use crate::ui_options::{SEARCH_DELAY_MAX, UiOptions};
use crate::window::App;

/// Everything's control ids (Enter's is goz-gui's).
const AS_YOU_TYPE: u16 = 1072;
const DELAY: u16 = 1549;
const SELECT_ON_CLICK: u16 = 1073;
const FOCUS_ON_ACTIVATE: u16 = 1074;
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

fn show(page: HWND, ui: &UiOptions) {
    set_check(page, AS_YOU_TYPE, ui.search_as_you_type);
    set_text(page, DELAY, &ui.search_delay.to_string());
    set_check(page, SELECT_ON_CLICK, ui.select_on_click);
    set_check(page, FOCUS_ON_ACTIVATE, ui.focus_on_activate);
}

/// A search delay as typed, in milliseconds, if it is one.
fn delay(text: &str) -> Option<u32> {
    text.trim()
        .parse::<u32>()
        .ok()
        .filter(|&d| d <= SEARCH_DELAY_MAX)
}

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
        rows.check(AS_YOU_TYPE, "&Search as you type");
        // "Search delay: [0] milliseconds", as Everything lays it out.
        rows.control(
            Class::Static,
            "Search &delay:",
            LABEL,
            SS_LEFT,
            0,
            (0, 2, 54, 9),
        )
        .control(
            Class::Edit,
            "",
            DELAY,
            WS_TABSTOP | (ES_AUTOHSCROLL | ES_NUMBER) as u32,
            WS_EX_CLIENTEDGE,
            (54, 0, 40, 13),
        )
        .control(
            Class::Static,
            "milliseconds",
            LABEL,
            SS_LEFT,
            0,
            (98, 2, 80, 9),
        )
        .skip(17)
        .check(SELECT_ON_CLICK, "Select search on &mouse click")
        .check(FOCUS_ON_ACTIVATE, "Focus search on acti&vate")
        .skip(4)
        .combo("&Enter in the search box:", ENTER, 92, 130);
    }

    fn fill(&self, app: &App, page: HWND) {
        show(page, &app.ui());
        fill_combo(page, ENTER, &ENTER_LABELS, app.enter_action());
    }

    /// The delay is for typing that searches.
    fn update_enabled(&self, page: HWND) {
        enable(page, DELAY, checked(page, AS_YOU_TYPE));
    }

    fn validate(&self, page: HWND) -> Result<(), Invalid> {
        if checked(page, AS_YOU_TYPE) && delay(&text(page, DELAY)).is_none() {
            return Err(Invalid {
                control: DELAY,
                message: format!("Enter a search delay from 0 to {SEARCH_DELAY_MAX} milliseconds."),
            });
        }
        Ok(())
    }

    fn apply(&self, app: &App, page: HWND, _problems: &mut Vec<String>) {
        let ui = app.ui();
        app.set_ui(UiOptions {
            search_as_you_type: checked(page, AS_YOU_TYPE),
            search_delay: delay(&text(page, DELAY)).unwrap_or(ui.search_delay),
            select_on_click: checked(page, SELECT_ON_CLICK),
            focus_on_activate: checked(page, FOCUS_ON_ACTIVATE),
            ..ui
        });
        let enter = combo_value(page, ENTER, &ENTER_LABELS);
        app.set_enter_action(enter);
        app.prefs.borrow_mut().base.enter_action = enter;
    }

    fn defaults(&self, page: HWND) {
        show(page, &UiOptions::default());
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

    #[test]
    fn delays_are_milliseconds_up_to_a_minute() {
        assert_eq!(delay("0"), Some(0));
        assert_eq!(delay(" 250 "), Some(250));
        assert_eq!(delay("60000"), Some(60_000));
        assert_eq!(delay("60001"), None);
        assert_eq!(delay(""), None);
        assert_eq!(delay("-1"), None);
    }
}
