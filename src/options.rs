//! Tools > Options: the settings goz-gui has, on one page — starting with
//! Windows and running in the background, the tray icon and hotkeys, the
//! search box, the histories, and the view. OK and Apply hand the values to
//! the window, which applies them and writes the settings file.

use std::cell::{Cell, RefCell};

use windows_sys::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows_sys::Win32::System::SystemServices::SS_LEFT;
use windows_sys::Win32::UI::Controls::{HKM_GETHOTKEY, HKM_SETHOTKEY};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::EnableWindow;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    BM_GETCHECK, BM_SETCHECK, BN_CLICKED, BS_AUTOCHECKBOX, BS_DEFPUSHBUTTON, BS_GROUPBOX,
    CB_ADDSTRING, CB_GETCURSEL, CB_SETCURSEL, CBN_SELCHANGE, CBS_DROPDOWNLIST, EN_CHANGE,
    ES_AUTOHSCROLL, ES_NUMBER, EndDialog, GWLP_USERDATA, GetDlgItem, IDCANCEL, IDOK, SendMessageW,
    SetWindowLongPtrW, SetWindowTextW, WM_COMMAND, WM_INITDIALOG, WS_EX_CLIENTEDGE, WS_GROUP,
    WS_TABSTOP, WS_VSCROLL,
};

use crate::dialog::{Class, Look, Template, apply_theme, center_on_owner, common, state};
use crate::settings::{Background, EnterAction, OnTop};
use crate::theme::ThemeMode;
use crate::wide::{window_text, wz};

/// Everything the page shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Values {
    pub start_with_windows: bool,
    pub folder_item: bool,
    pub background: Background,
    pub enter: EnterAction,
    pub suggest: bool,
    pub search_history: bool,
    pub search_limit: bool,
    pub search_days: u32,
    pub run_history: bool,
    pub run_limit: bool,
    pub run_days: u32,
    pub theme: ThemeMode,
    pub on_top: OnTop,
    pub status_bar: bool,
    pub nav_bar: bool,
}

const STATIC: u16 = 0xffff;
const OK: u16 = IDOK as u16;
const CANCEL: u16 = IDCANCEL as u16;
const APPLY: u16 = 0x3021;

const STARTUP: u16 = 100;
const BACKGROUND: u16 = 101;
const TRAY: u16 = 102;
const SINGLE_CLICK: u16 = 103;
const MINIMIZE: u16 = 104;
const FOLDER_ITEM: u16 = 105;
const TOGGLE_KEY: u16 = 106;
const SHOW_KEY: u16 = 107;
const ENTER: u16 = 110;
const SUGGEST: u16 = 111;
const SEARCH_HISTORY: u16 = 120;
const SEARCH_LIMIT: u16 = 121;
const SEARCH_DAYS: u16 = 122;
const RUN_HISTORY: u16 = 123;
const RUN_LIMIT: u16 = 124;
const RUN_DAYS: u16 = 125;
const THEME: u16 = 130;
const ON_TOP: u16 = 131;
const STATUS_BAR: u16 = 132;
const NAV_BAR: u16 = 133;

const ENTER_LABELS: [(EnterAction, &str); 7] = [
    (EnterAction::MostRun, "Select the most run result"),
    (EnterAction::LastRun, "Select the last run result"),
    (EnterAction::Result, "Select the focused result"),
    (EnterAction::Top, "Select the top result"),
    (EnterAction::Bottom, "Select the bottom result"),
    (EnterAction::SelectSearch, "Select the search text"),
    (EnterAction::None, "Do nothing"),
];
const THEMES: [(ThemeMode, &str); 3] = [
    (ThemeMode::UserDefault, "Follow Windows"),
    (ThemeMode::Standard, "Light"),
    (ThemeMode::Dark, "Dark"),
];
const ON_TOPS: [(OnTop, &str); 3] = [
    (OnTop::Never, "Never"),
    (OnTop::Always, "Always"),
    (OnTop::WhileSearching, "While searching"),
];

struct Page<'a> {
    initial: Values,
    look: Cell<Look>,
    apply: &'a dyn Fn(&Values),
    /// What was last applied, so OK after Apply does not apply twice.
    applied: RefCell<Values>,
}

/// Shows the page; `apply` runs for Apply and for OK with changes.
pub fn show(owner: HWND, look: Look, initial: Values, apply: &dyn Fn(&Values)) {
    let page = Page {
        initial,
        look: Cell::new(look),
        apply,
        applied: RefCell::new(initial),
    };
    let check = WS_TABSTOP | BS_AUTOCHECKBOX as u32;
    let combo = WS_TABSTOP | WS_VSCROLL | CBS_DROPDOWNLIST as u32;
    let number = WS_TABSTOP | (ES_AUTOHSCROLL | ES_NUMBER) as u32;
    let group = BS_GROUPBOX as u32;
    let mut t = Template::new("Options", 0, 380, 246);
    t.control(Class::Button, "General", STATIC, group, 0, (7, 5, 178, 132))
        .control(
            Class::Button,
            "&Start goz-gui when Windows starts",
            STARTUP,
            check | WS_GROUP,
            0,
            (14, 18, 166, 10),
        )
        .control(
            Class::Button,
            "Run in &background (closing keeps goz-gui running)",
            BACKGROUND,
            check,
            0,
            (14, 31, 166, 10),
        )
        .control(
            Class::Button,
            "Show &tray icon",
            TRAY,
            check,
            0,
            (14, 44, 166, 10),
        )
        .control(
            Class::Button,
            "Single &click on the tray icon",
            SINGLE_CLICK,
            check,
            0,
            (24, 57, 156, 10),
        )
        .control(
            Class::Button,
            "&Minimize to the tray",
            MINIMIZE,
            check,
            0,
            (24, 70, 156, 10),
        )
        .control(
            Class::Button,
            "\"Search goz...\" on &folders in Explorer",
            FOLDER_ITEM,
            check,
            0,
            (14, 83, 166, 10),
        )
        .control(
            Class::Static,
            "To&ggle window hotkey:",
            STATIC,
            SS_LEFT,
            0,
            (14, 100, 80, 8),
        )
        .control(
            Class::Named("msctls_hotkey32"),
            "",
            TOGGLE_KEY,
            WS_TABSTOP,
            WS_EX_CLIENTEDGE,
            (96, 98, 84, 12),
        )
        .control(
            Class::Static,
            "Sho&w window hotkey:",
            STATIC,
            SS_LEFT,
            0,
            (14, 117, 80, 8),
        )
        .control(
            Class::Named("msctls_hotkey32"),
            "",
            SHOW_KEY,
            WS_TABSTOP,
            WS_EX_CLIENTEDGE,
            (96, 115, 84, 12),
        )
        .control(Class::Button, "View", STATIC, group, 0, (7, 142, 178, 76))
        .control(
            Class::Static,
            "T&heme:",
            STATIC,
            SS_LEFT,
            0,
            (14, 156, 40, 8),
        )
        .control(
            Class::Named("COMBOBOX"),
            "",
            THEME,
            combo | WS_GROUP,
            0,
            (60, 154, 120, 60),
        )
        .control(
            Class::Static,
            "On to&p:",
            STATIC,
            SS_LEFT,
            0,
            (14, 172, 40, 8),
        )
        .control(
            Class::Named("COMBOBOX"),
            "",
            ON_TOP,
            combo,
            0,
            (60, 170, 120, 60),
        )
        .control(
            Class::Button,
            "Show status &bar",
            STATUS_BAR,
            check,
            0,
            (14, 188, 166, 10),
        )
        .control(
            Class::Button,
            "Show Back and &Forward buttons",
            NAV_BAR,
            check,
            0,
            (14, 201, 166, 10),
        )
        .control(Class::Button, "Search", STATIC, group, 0, (192, 5, 181, 56))
        .control(
            Class::Static,
            "&Enter in the search box:",
            STATIC,
            SS_LEFT,
            0,
            (199, 17, 167, 8),
        )
        .control(
            Class::Named("COMBOBOX"),
            "",
            ENTER,
            combo | WS_GROUP,
            0,
            (199, 27, 167, 100),
        )
        .control(
            Class::Button,
            "Always show search s&uggestions",
            SUGGEST,
            check,
            0,
            (199, 45, 167, 10),
        )
        .control(
            Class::Button,
            "History",
            STATIC,
            group,
            0,
            (192, 66, 181, 112),
        )
        .control(
            Class::Button,
            "Keep a search h&istory",
            SEARCH_HISTORY,
            check | WS_GROUP,
            0,
            (199, 79, 167, 10),
        )
        .control(
            Class::Button,
            "Forget searches after",
            SEARCH_LIMIT,
            check,
            0,
            (209, 93, 88, 10),
        )
        .control(
            Class::Edit,
            "",
            SEARCH_DAYS,
            number,
            WS_EX_CLIENTEDGE,
            (298, 91, 30, 12),
        )
        .control(Class::Static, "days", STATIC, SS_LEFT, 0, (332, 93, 30, 8))
        .control(
            Class::Button,
            "Keep a &run history",
            RUN_HISTORY,
            check,
            0,
            (199, 110, 167, 10),
        )
        .control(
            Class::Button,
            "Forget runs after",
            RUN_LIMIT,
            check,
            0,
            (209, 124, 88, 10),
        )
        .control(
            Class::Edit,
            "",
            RUN_DAYS,
            number,
            WS_EX_CLIENTEDGE,
            (298, 122, 30, 12),
        )
        .control(Class::Static, "days", STATIC, SS_LEFT, 0, (332, 124, 30, 8))
        .control(
            Class::Static,
            "History settings take effect the next time goz-gui starts.",
            STATIC,
            SS_LEFT,
            0,
            (199, 145, 167, 20),
        )
        .control(
            Class::Button,
            "OK",
            OK,
            WS_TABSTOP | WS_GROUP | BS_DEFPUSHBUTTON as u32,
            0,
            (213, 225, 50, 14),
        )
        .control(
            Class::Button,
            "Cancel",
            CANCEL,
            WS_TABSTOP,
            0,
            (267, 225, 50, 14),
        )
        .control(
            Class::Button,
            "&Apply",
            APPLY,
            WS_TABSTOP,
            0,
            (321, 225, 52, 14),
        );
    t.run(owner, Some(proc), (&raw const page) as LPARAM);
}

fn item(dlg: HWND, id: u16) -> HWND {
    // SAFETY: looks up a child of this thread's dialog.
    unsafe { GetDlgItem(dlg, i32::from(id)) }
}

fn send(dlg: HWND, id: u16, msg: u32, w: usize, l: isize) -> isize {
    // SAFETY: a message to a control of this thread's dialog; any pointer
    // in `l` is the caller's and outlives the synchronous call.
    unsafe { SendMessageW(item(dlg, id), msg, w, l) }
}

fn set_check(dlg: HWND, id: u16, on: bool) {
    send(dlg, id, BM_SETCHECK, usize::from(on), 0);
}

fn checked(dlg: HWND, id: u16) -> bool {
    send(dlg, id, BM_GETCHECK, 0, 0) == 1
}

fn enable(dlg: HWND, id: u16, on: bool) {
    // SAFETY: a control of this thread's dialog.
    unsafe { EnableWindow(item(dlg, id), on.into()) };
}

fn fill_combo<T: Copy + PartialEq>(dlg: HWND, id: u16, items: &[(T, &str)], current: T) {
    for (_, label) in items {
        let text = wz(label);
        send(dlg, id, CB_ADDSTRING, 0, text.as_ptr() as isize);
    }
    let at = items.iter().position(|(v, _)| *v == current).unwrap_or(0);
    send(dlg, id, CB_SETCURSEL, at, 0);
}

fn combo_value<T: Copy>(dlg: HWND, id: u16, items: &[(T, &str)]) -> T {
    let at = send(dlg, id, CB_GETCURSEL, 0, 0).max(0) as usize;
    items[at.min(items.len() - 1)].0
}

fn set_text(dlg: HWND, id: u16, text: &str) {
    let text = wz(text);
    // SAFETY: a NUL-terminated string for a control of this thread's dialog.
    unsafe { SetWindowTextW(item(dlg, id), text.as_ptr()) };
}

/// Days as typed, within 1..=36500; `fallback` if not a number.
fn days(text: &str, fallback: u32) -> u32 {
    text.trim()
        .parse::<u32>()
        .map_or(fallback, |d| d.clamp(1, 36_500))
}

fn fill(dlg: HWND, v: &Values) {
    set_check(dlg, STARTUP, v.start_with_windows);
    set_check(dlg, BACKGROUND, v.background.run_in_background);
    set_check(dlg, TRAY, v.background.show_tray_icon);
    set_check(dlg, SINGLE_CLICK, v.background.single_click_tray);
    set_check(dlg, MINIMIZE, v.background.minimize_to_tray);
    set_check(dlg, FOLDER_ITEM, v.folder_item);
    send(
        dlg,
        TOGGLE_KEY,
        HKM_SETHOTKEY,
        usize::from(v.background.toggle_window_key),
        0,
    );
    send(
        dlg,
        SHOW_KEY,
        HKM_SETHOTKEY,
        usize::from(v.background.show_window_key),
        0,
    );
    fill_combo(dlg, ENTER, &ENTER_LABELS, v.enter);
    set_check(dlg, SUGGEST, v.suggest);
    set_check(dlg, SEARCH_HISTORY, v.search_history);
    set_check(dlg, SEARCH_LIMIT, v.search_limit);
    set_text(dlg, SEARCH_DAYS, &v.search_days.to_string());
    set_check(dlg, RUN_HISTORY, v.run_history);
    set_check(dlg, RUN_LIMIT, v.run_limit);
    set_text(dlg, RUN_DAYS, &v.run_days.to_string());
    fill_combo(dlg, THEME, &THEMES, v.theme);
    fill_combo(dlg, ON_TOP, &ON_TOPS, v.on_top);
    set_check(dlg, STATUS_BAR, v.status_bar);
    set_check(dlg, NAV_BAR, v.nav_bar);
}

/// Greys what does not apply: the tray's options without a tray icon, the
/// days without a limit, a history's limit while it is off.
fn update_enabled(dlg: HWND) {
    let tray = checked(dlg, TRAY);
    enable(dlg, SINGLE_CLICK, tray);
    enable(dlg, MINIMIZE, tray);
    let sh = checked(dlg, SEARCH_HISTORY);
    enable(dlg, SEARCH_LIMIT, sh);
    enable(dlg, SEARCH_DAYS, sh && checked(dlg, SEARCH_LIMIT));
    enable(dlg, SUGGEST, sh);
    let rh = checked(dlg, RUN_HISTORY);
    enable(dlg, RUN_LIMIT, rh);
    enable(dlg, RUN_DAYS, rh && checked(dlg, RUN_LIMIT));
}

fn read(dlg: HWND, before: &Values) -> Values {
    Values {
        start_with_windows: checked(dlg, STARTUP),
        folder_item: checked(dlg, FOLDER_ITEM),
        background: Background {
            run_in_background: checked(dlg, BACKGROUND),
            show_tray_icon: checked(dlg, TRAY),
            single_click_tray: checked(dlg, SINGLE_CLICK),
            minimize_to_tray: checked(dlg, MINIMIZE),
            toggle_window_key: send(dlg, TOGGLE_KEY, HKM_GETHOTKEY, 0, 0) as u16,
            show_window_key: send(dlg, SHOW_KEY, HKM_GETHOTKEY, 0, 0) as u16,
        },
        enter: combo_value(dlg, ENTER, &ENTER_LABELS),
        suggest: checked(dlg, SUGGEST),
        search_history: checked(dlg, SEARCH_HISTORY),
        search_limit: checked(dlg, SEARCH_LIMIT),
        search_days: days(&window_text(item(dlg, SEARCH_DAYS)), before.search_days),
        run_history: checked(dlg, RUN_HISTORY),
        run_limit: checked(dlg, RUN_LIMIT),
        run_days: days(&window_text(item(dlg, RUN_DAYS)), before.run_days),
        theme: combo_value(dlg, THEME, &THEMES),
        on_top: combo_value(dlg, ON_TOP, &ON_TOPS),
        status_bar: checked(dlg, STATUS_BAR),
        nav_bar: checked(dlg, NAV_BAR),
    }
}

/// Applies what changed since the last apply; true if anything did.
fn apply(dlg: HWND, p: &Page<'_>) -> bool {
    let before = *p.applied.borrow();
    let now = read(dlg, &before);
    if now == before {
        return false;
    }
    *p.applied.borrow_mut() = now;
    (p.apply)(&now);
    // The theme may have changed: the page follows the window's.
    let look = crate::dialog::app_look();
    if look != p.look.get() {
        p.look.set(look);
        apply_theme(dlg, look.dark);
    }
    true
}

unsafe extern "system" fn proc(dlg: HWND, msg: u32, w: WPARAM, l: LPARAM) -> isize {
    if msg == WM_INITDIALOG {
        // SAFETY: `l` is the `Page` that `show` keeps alive until the dialog
        // has ended.
        unsafe { SetWindowLongPtrW(dlg, GWLP_USERDATA, l) };
    }
    // SAFETY: GWLP_USERDATA is the `Page` from `show`, or null.
    let Some(p) = (unsafe { state::<Page<'_>>(dlg) }) else {
        return 0;
    };
    if let Some(answer) = common(dlg, msg, w, l, &p.look) {
        return answer;
    }
    match msg {
        WM_INITDIALOG => {
            fill(dlg, &p.initial);
            update_enabled(dlg);
            enable(dlg, APPLY, false);
            apply_theme(dlg, p.look.get().dark);
            center_on_owner(dlg);
            1
        }
        WM_COMMAND => {
            let (id, code) = ((w & 0xffff) as u16, ((w >> 16) & 0xffff) as u32);
            match id {
                OK => {
                    apply(dlg, p);
                    // SAFETY: ends this modal dialog.
                    unsafe { EndDialog(dlg, 1) };
                }
                // SAFETY: ends this modal dialog.
                CANCEL => unsafe {
                    EndDialog(dlg, 0);
                },
                APPLY => {
                    apply(dlg, p);
                    enable(dlg, APPLY, false);
                }
                _ => {
                    let changed = matches!(code, BN_CLICKED | CBN_SELCHANGE | EN_CHANGE)
                        // A hotkey control reports a change as EN_CHANGE.
                        || id == TOGGLE_KEY
                        || id == SHOW_KEY;
                    if !changed {
                        return 0;
                    }
                    update_enabled(dlg);
                    enable(dlg, APPLY, true);
                }
            }
            1
        }
        _ => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn days_are_clamped_numbers() {
        assert_eq!(days("90", 7), 90);
        assert_eq!(days(" 0 ", 7), 1);
        assert_eq!(days("999999", 7), 36_500);
        assert_eq!(days("x", 7), 7);
    }

    #[test]
    fn every_choice_has_a_label() {
        assert_eq!(ENTER_LABELS.len(), 7);
        for a in [
            EnterAction::None,
            EnterAction::MostRun,
            EnterAction::SelectSearch,
        ] {
            assert!(ENTER_LABELS.iter().any(|(v, _)| *v == a));
        }
    }
}
