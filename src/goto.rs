//! Edit > Go To... (Ctrl+G): Everything's "Go To Result" dialog, by result
//! number. Everything's also numbers the files alone or the folders alone
//! ("Go to what"); goz-gui's list has no such numbering (the n-th file is
//! not the n-th row), so it asks for the result number only.

use std::cell::{Cell, RefCell};

use windows_sys::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows_sys::Win32::System::SystemServices::SS_LEFT;
use windows_sys::Win32::UI::Controls::EM_SETSEL;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{EnableWindow, SetFocus};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    BS_DEFPUSHBUTTON, EN_CHANGE, ES_AUTOHSCROLL, EndDialog, GWLP_USERDATA, GetDlgItem, IDCANCEL,
    IDOK, MB_ICONWARNING, MB_OK, SendMessageW, SetWindowLongPtrW, SetWindowTextW, WM_COMMAND,
    WM_INITDIALOG, WS_EX_CLIENTEDGE, WS_TABSTOP,
};

use crate::dialog::{Class, Look, Template, apply_theme, center_on_owner, common, state};
use crate::format;
use crate::msgbox;
use crate::searchtools::result_number;
use crate::wide::{window_text, wz};

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
            let edit = item(dlg, ID_NUMBER);
            // SAFETY: a NUL-terminated string and messages for a control
            // of this thread's dialog.
            unsafe {
                SetWindowTextW(edit, wz(&s.initial.to_string()).as_ptr());
                SendMessageW(edit, EM_SETSEL, 0, -1);
                SetFocus(edit);
            }
            apply_theme(dlg, s.look.get().dark);
            center_on_owner(dlg);
            // The keyboard is in the number, put there above.
            0
        }
        WM_COMMAND => {
            let (id, code) = ((w & 0xffff) as u16, ((w >> 16) & 0xffff) as u32);
            match id {
                OK => {
                    let edit = item(dlg, ID_NUMBER);
                    let Some(n) = result_number(&window_text(edit), s.count) else {
                        msgbox::show(
                            dlg,
                            &format!(
                                "Type a result number from 1 to {}.",
                                format::grouped(s.count)
                            ),
                            "Go To Result",
                            MB_OK | MB_ICONWARNING,
                        );
                        // SAFETY: a control of this thread's dialog.
                        unsafe {
                            SetFocus(edit);
                            SendMessageW(edit, EM_SETSEL, 0, -1);
                        }
                        return 1;
                    };
                    *s.result.borrow_mut() = Some(n);
                    // SAFETY: ends this modal dialog.
                    unsafe { EndDialog(dlg, 1) };
                }
                // SAFETY: ends this modal dialog.
                CANCEL => unsafe {
                    EndDialog(dlg, 0);
                },
                ID_NUMBER if code == EN_CHANGE => {
                    let typed = !window_text(item(dlg, ID_NUMBER)).trim().is_empty();
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
    fn the_label_says_the_range() {
        // The count grouped as the user's locale groups it.
        assert_eq!(
            label(25462),
            format!("Result &number (1 - {}):", format::grouped(25462))
        );
        assert_eq!(label(1), "Result &number (1 - 1):");
    }
}
