//! Edit > Go To... (Ctrl+G): Everything's "Go To Result" dialog, by result
//! number. Everything's also numbers the files alone or the folders alone
//! ("Go to what"); goz-gui's list has no such numbering (the n-th file is
//! not the n-th row), so it asks for the result number only.

use std::cell::{Cell, RefCell};

use windows_sys::Win32::Foundation::HWND;
use windows_sys::Win32::System::SystemServices::SS_LEFT;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    BS_DEFPUSHBUTTON, EN_CHANGE, ES_AUTOHSCROLL, IDCANCEL, IDOK, MB_ICONWARNING, MB_OK, WM_COMMAND,
    WM_INITDIALOG, WS_EX_CLIENTEDGE, WS_TABSTOP,
};

use crate::dialog::{Class, Look, Template, apply_theme, center_on_owner, common};
use crate::format;
use crate::msgbox;
use crate::searchtools::result_number;
use crate::win::{self, Msg, dlg, edit};

const STATIC: u16 = 0xffff;
const ID_NUMBER: u16 = 1000;
const OK: u16 = IDOK as u16;
const CANCEL: u16 = IDCANCEL as u16;

/// The number box's label: Everything's, with the range.
pub fn label(count: u64) -> String {
    format!("Result &number (1 - {}):", format::grouped(count))
}

struct Ask {
    count: u64,
    initial: u64,
    look: Cell<Look>,
    result: RefCell<Option<u64>>,
}

/// Asks for a result number from 1 to `count`, starting at `initial`;
/// `None` if cancelled.
pub fn ask(owner: HWND, count: u64, initial: u64, look: Look) -> Option<u64> {
    let state = Ask {
        count,
        initial: initial.clamp(1, count.max(1)),
        look: Cell::new(look),
        result: RefCell::new(None),
    };
    // Everything's dialog: 262 × 178 px at 100 %, the label over the box.
    let mut t = Template::new("Go To Result", 0, 166, 66);
    t.control(
        Class::Static,
        &label(count),
        STATIC,
        SS_LEFT,
        0,
        (7, 7, 152, 8),
    )
    .control(
        Class::Edit,
        "",
        ID_NUMBER,
        WS_TABSTOP | ES_AUTOHSCROLL as u32,
        WS_EX_CLIENTEDGE,
        (7, 18, 152, 14),
    )
    .control(
        Class::Button,
        "OK",
        OK,
        WS_TABSTOP | BS_DEFPUSHBUTTON as u32,
        0,
        (55, 45, 50, 14),
    )
    .control(
        Class::Button,
        "Cancel",
        CANCEL,
        WS_TABSTOP,
        0,
        (109, 45, 50, 14),
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
            let number = dlg::item(dlg, ID_NUMBER);
            win::set_text(number, &s.initial.to_string());
            edit::select_all(number);
            win::set_focus(number);
            apply_theme(dlg, s.look.get().dark);
            center_on_owner(dlg);
            // The keyboard is in the number, put there above.
            0
        }
        WM_COMMAND => {
            let (id, code) = (win::loword(m.w), u32::from(win::hiword(m.w)));
            match id {
                OK => {
                    let number = dlg::item(dlg, ID_NUMBER);
                    let Some(n) = result_number(&win::text(number), s.count) else {
                        msgbox::show(
                            dlg,
                            &format!(
                                "Type a result number from 1 to {}.",
                                format::grouped(s.count)
                            ),
                            "Go To Result",
                            MB_OK | MB_ICONWARNING,
                        );
                        win::set_focus(number);
                        edit::select_all(number);
                        return 1;
                    };
                    *s.result.borrow_mut() = Some(n);
                    dlg::end(dlg, 1);
                }
                CANCEL => dlg::end(dlg, 0),
                ID_NUMBER if code == EN_CHANGE => {
                    let typed = !dlg::item_text(dlg, ID_NUMBER).trim().is_empty();
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
    fn the_label_says_the_range() {
        // The count grouped as the user's locale groups it.
        assert_eq!(
            label(25462),
            format!("Result &number (1 - {}):", format::grouped(25462))
        );
        assert_eq!(label(1), "Result &number (1 - 1):");
    }
}
