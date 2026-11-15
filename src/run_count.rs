//! The row menu's Set Run Count... dialog: the number of times the selected
//! items count as run, as Everything's ("Set to 0 to remove the run
//! count.").

use std::cell::{Cell, RefCell};

use windows_sys::Win32::Foundation::HWND;
use windows_sys::Win32::System::SystemServices::SS_LEFT;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    BS_DEFPUSHBUTTON, EN_CHANGE, ES_AUTOHSCROLL, ES_NUMBER, IDCANCEL, IDOK, MB_ICONWARNING, MB_OK,
    WM_COMMAND, WM_INITDIALOG, WS_EX_CLIENTEDGE, WS_TABSTOP,
};

use crate::dialog::{Class, Look, Template, apply_theme, center_on_owner, common};
use crate::msgbox;
use crate::win::{self, Msg, dlg, edit};

const STATIC: u16 = 0xffff;
const ID_COUNT: u16 = 100;
const OK: u16 = IDOK as u16;
const CANCEL: u16 = IDCANCEL as u16;

struct Ask {
    initial: u32,
    look: Cell<Look>,
    result: RefCell<Option<u32>>,
}

/// The run count typed, if it is one: digits only, at most `u32::MAX`.
fn parse(text: &str) -> Option<u32> {
    let text = text.trim();
    if text.is_empty() || !text.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    text.parse().ok()
}

/// Asks for a run count, starting from `initial`; `None` if cancelled.
pub fn ask(owner: HWND, initial: u32, look: Look) -> Option<u32> {
    let state = Ask {
        initial,
        look: Cell::new(look),
        result: RefCell::new(None),
    };
    let mut t = Template::new("Set Run Count", 0, 186, 62);
    t.control(
        Class::Static,
        "&Run count:",
        STATIC,
        SS_LEFT,
        0,
        (7, 9, 44, 8),
    )
    .control(
        Class::Edit,
        "",
        ID_COUNT,
        WS_TABSTOP | (ES_AUTOHSCROLL | ES_NUMBER) as u32,
        WS_EX_CLIENTEDGE,
        (54, 7, 125, 14),
    )
    .control(
        Class::Static,
        "Set to 0 to remove the run count.",
        STATIC,
        SS_LEFT,
        0,
        (7, 26, 172, 8),
    )
    .control(
        Class::Button,
        "OK",
        OK,
        WS_TABSTOP | BS_DEFPUSHBUTTON as u32,
        0,
        (75, 41, 50, 14),
    )
    .control(
        Class::Button,
        "Cancel",
        CANCEL,
        WS_TABSTOP,
        0,
        (129, 41, 50, 14),
    );
    t.show(owner, &|m| proc(&state, m));
    state.result.into_inner()
}

fn proc(s: &Ask, m: &Msg) -> isize {
    let dlg = m.hwnd;
    if let Some(answer) = common(m, &s.look) {
        return answer;
    }
    match m.id {
        WM_INITDIALOG => {
            let count = dlg::item(dlg, ID_COUNT);
            win::set_text(count, &s.initial.to_string());
            edit::select_all(count);
            apply_theme(dlg, s.look.get().dark);
            center_on_owner(dlg);
            // The keyboard in the count, its text selected.
            1
        }
        WM_COMMAND => {
            let (id, code) = (win::loword(m.w), u32::from(win::hiword(m.w)));
            match id {
                OK => {
                    let Some(count) = parse(&dlg::item_text(dlg, ID_COUNT)) else {
                        msgbox::show(
                            dlg,
                            "Type a run count from 0 to 4294967295.",
                            "Set Run Count",
                            MB_OK | MB_ICONWARNING,
                        );
                        return 1;
                    };
                    *s.result.borrow_mut() = Some(count);
                    dlg::end(dlg, 1);
                }
                CANCEL => dlg::end(dlg, 0),
                ID_COUNT if code == EN_CHANGE => {
                    let typed = !dlg::item_text(dlg, ID_COUNT).trim().is_empty();
                    win::enable(dlg::item(dlg, OK), typed);
                }
                _ => return 0,
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
    fn counts_are_whole_numbers() {
        assert_eq!(parse("0"), Some(0));
        assert_eq!(parse(" 12 "), Some(12));
        assert_eq!(parse("4294967295"), Some(u32::MAX));
        assert_eq!(parse("4294967296"), None);
        assert_eq!(parse(""), None);
        assert_eq!(parse("-1"), None);
        assert_eq!(parse("1e3"), None);
    }
}
