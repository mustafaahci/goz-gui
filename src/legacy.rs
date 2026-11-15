//! Bridges for the code not yet moved onto `crate::win`: the old raw-pointer
//! dialog API, kept only while some dialogs still use it. Deleted once the
//! move is done (nothing may call these then).

#![allow(unsafe_code)]

use std::cell::Cell;

use windows_sys::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CreateDialogIndirectParamW, DLGPROC, DialogBoxIndirectParamW, GWLP_USERDATA, GetWindowLongPtrW,
};

use crate::dialog::Look;
use crate::win::Msg;
use crate::win::dlg::Template;
use crate::win::msg::Via;

/// A dialog procedure's message as the new API takes it.
fn dialog_msg(dlg: HWND, msg: u32, w: WPARAM, l: LPARAM) -> Msg {
    // SAFETY: the old dialog procedures pass on what they were delivered.
    unsafe { Msg::delivered(dlg, msg, w, l, Via::Dialog) }
}

/// The old `dialog::common`, for a dialog procedure's raw message.
pub fn common(dlg: HWND, msg: u32, w: WPARAM, l: LPARAM, look: &Cell<Look>) -> Option<isize> {
    crate::dialog::common_msg(&dialog_msg(dlg, msg, w, l), look)
}

/// The old `Look::after_setting_change`, for WM_SETTINGCHANGE's lParam.
pub fn after_setting_change(look: Look, dlg: HWND, l: LPARAM) -> Option<Look> {
    use windows_sys::Win32::UI::WindowsAndMessaging::WM_SETTINGCHANGE;
    look.after_setting_change_msg(&dialog_msg(dlg, WM_SETTINGCHANGE, 0, l))
}

/// The state an old dialog procedure keeps in its GWLP_USERDATA.
///
/// # Safety
/// The dialog's GWLP_USERDATA is null or a `T` that outlives the dialog.
pub unsafe fn state<'a, T>(dlg: HWND) -> Option<&'a T> {
    // SAFETY: as the caller promises.
    unsafe { (GetWindowLongPtrW(dlg, GWLP_USERDATA) as *const T).as_ref() }
}

/// The old `Template::run`: a modal dialog with a raw procedure.
pub fn run(t: &Template, owner: HWND, proc: DLGPROC, param: LPARAM) -> isize {
    let aligned = t.aligned();
    crate::trace::mark("dialog run");
    // SAFETY: a complete template that outlives the modal call; the old
    // procedures read `param` as the state their caller keeps alive.
    let ended = unsafe {
        DialogBoxIndirectParamW(
            crate::win::class::module(),
            aligned.as_ptr().cast(),
            owner,
            proc,
            param,
        )
    };
    crate::trace::mark("dialog ended");
    ended
}

/// The old `Template::create_child`: a child dialog with a raw procedure.
pub fn create_child(t: &Template, parent: HWND, proc: DLGPROC, param: LPARAM) -> HWND {
    let aligned = t.aligned();
    // SAFETY: as in `run`; the system copies the template.
    unsafe {
        CreateDialogIndirectParamW(
            crate::win::class::module(),
            aligned.as_ptr().cast(),
            parent,
            proc,
            param,
        )
    }
}
