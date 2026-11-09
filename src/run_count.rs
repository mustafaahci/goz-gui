//! The row menu's Set Run Count... dialog: the number of times the selected
//! items count as run, as Everything's ("Set to 0 to remove the run
//! count.").

use std::cell::{Cell, RefCell};

use windows_sys::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows_sys::Win32::System::SystemServices::SS_LEFT;
use windows_sys::Win32::UI::Controls::EM_SETSEL;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::EnableWindow;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    BS_DEFPUSHBUTTON, EN_CHANGE, ES_AUTOHSCROLL, ES_NUMBER, EndDialog, GWLP_USERDATA, GetDlgItem,
    IDCANCEL, IDOK, MB_ICONWARNING, MB_OK, SendMessageW, SetWindowLongPtrW, SetWindowTextW,
    WM_COMMAND, WM_INITDIALOG, WS_EX_CLIENTEDGE, WS_TABSTOP,
};

use crate::dialog::{Class, Look, Template, apply_theme, center_on_owner, common, state};
use crate::msgbox;
use crate::wide::{window_text, wz};

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
    t.run(owner, Some(proc), (&raw const state) as LPARAM);
    state.result.into_inner()
}

fn item(dlg: HWND, id: u16) -> HWND {
    // SAFETY: looks up a child of this thread's dialog.
    unsafe { GetDlgItem(dlg, i32::from(id)) }
}

unsafe extern "system" fn proc(dlg: HWND, msg: u32, w: WPARAM, l: LPARAM) -> isize {
    if msg == WM_INITDIALOG {
        // SAFETY: `l` is the `Ask` that `ask` keeps alive until the dialog
        // has ended.
        unsafe { SetWindowLongPtrW(dlg, GWLP_USERDATA, l) };
    }
    // SAFETY: GWLP_USERDATA is the `Ask` from `ask`, or null.
    let Some(s) = (unsafe { state::<Ask>(dlg) }) else {
        return 0;
    };
    if let Some(answer) = common(dlg, msg, w, l, &s.look) {
        return answer;
    }
    match msg {
        WM_INITDIALOG => {
            let edit = item(dlg, ID_COUNT);
            // SAFETY: a NUL-terminated string and a message for a control
            // of this thread's dialog.
            unsafe {
                SetWindowTextW(edit, wz(&s.initial.to_string()).as_ptr());
                SendMessageW(edit, EM_SETSEL, 0, -1);
            }
            apply_theme(dlg, s.look.get().dark);
            center_on_owner(dlg);
            // The keyboard in the count, its text selected.
            1
        }
        WM_COMMAND => {
            let (id, code) = ((w & 0xffff) as u16, ((w >> 16) & 0xffff) as u32);
            match id {
                OK => {
                    let Some(count) = parse(&window_text(item(dlg, ID_COUNT))) else {
                        msgbox::show(
                            dlg,
                            "Type a run count from 0 to 4294967295.",
                            "Set Run Count",
                            MB_OK | MB_ICONWARNING,
                        );
                        return 1;
                    };
                    *s.result.borrow_mut() = Some(count);
                    // SAFETY: ends this modal dialog.
                    unsafe { EndDialog(dlg, 1) };
                }
                // SAFETY: ends this modal dialog.
                CANCEL => unsafe {
                    EndDialog(dlg, 0);
                },
                ID_COUNT if code == EN_CHANGE => {
                    let typed = !window_text(item(dlg, ID_COUNT)).trim().is_empty();
                    // SAFETY: a control of this thread's dialog.
                    unsafe { EnableWindow(item(dlg, OK), typed.into()) };
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
