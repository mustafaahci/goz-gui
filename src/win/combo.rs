//! Combo-box messages (CB_* are below WM_USER: the same for every window).

use windows_sys::Win32::Foundation::HWND;
use windows_sys::Win32::UI::Controls::{COMBOBOXINFO, GetComboBoxInfo};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CB_ADDSTRING, CB_DELETESTRING, CB_GETCURSEL, CB_RESETCONTENT, CB_SELECTSTRING, CB_SETCURSEL,
    CB_SETEDITSEL,
};

fn send_int(c: HWND, msg: u32, w: usize, l: isize) -> isize {
    // SAFETY: the callers' messages take integers.
    unsafe { super::send_raw(c, msg, w, l) }
}

/// Adds `text` to the list; its index (CB_ERR -1 or CB_ERRSPACE -2 if not).
pub fn add(c: HWND, text: &str) -> i32 {
    let z = super::wz(text);
    // SAFETY: a NUL-terminated string read (or, for a list without strings,
    // kept as a number) during the call.
    unsafe { super::send_raw(c, CB_ADDSTRING, 0, z.as_ptr() as isize) as i32 }
}

/// Deletes item `i`; how many are left (CB_ERR if there was none).
pub fn delete(c: HWND, i: i32) -> i32 {
    send_int(c, CB_DELETESTRING, i as isize as usize, 0) as i32
}

/// Empties the list.
pub fn reset(c: HWND) {
    send_int(c, CB_RESETCONTENT, 0, 0);
}

/// The selected item (CB_ERR -1 for none).
pub fn cur_sel(c: HWND) -> i32 {
    send_int(c, CB_GETCURSEL, 0, 0) as i32
}

/// Selects item `i` (-1: none).
pub fn set_cur_sel(c: HWND, i: i32) -> i32 {
    send_int(c, CB_SETCURSEL, i as isize as usize, 0) as i32
}

/// The text of item `i` of combo box `c`, one of this thread (empty if
/// there is none, or `c` is not one).
///
/// CB_GETLBTEXT takes a buffer but not its length, which CB_GETLBTEXTLEN
/// is asked for first, and the box asks both of its list with messages:
/// a subclass of the box or of its list could answer the length with less
/// than the text. So both are asked of the list (the box's ComboLBox,
/// from GetComboBoxInfo) through the ComboLBox class's own procedure,
/// called directly, with no subclass in between.
pub fn text(c: HWND, i: i32) -> String {
    let Some(list) = info(c).map(|ci| ci.hwndList) else {
        return String::new();
    };
    match super::system_class_proc(list, "ComboLBox") {
        Some(proc) => super::listbox::text_through(list, proc, i),
        None => String::new(),
    }
}

/// Selects the first item after `after` (-1: from the start) whose text
/// starts with `text` (ignoring case); its index, CB_ERR if none.
pub fn select_string(c: HWND, after: i32, text: &str) -> i32 {
    let z = super::wz(text);
    // SAFETY: a NUL-terminated string read during the call.
    unsafe {
        super::send_raw(
            c,
            CB_SELECTSTRING,
            after as isize as usize,
            z.as_ptr() as isize,
        ) as i32
    }
}

/// Selects characters `start` to `end` of the box's edit (-1: to the end).
pub fn set_edit_sel(c: HWND, start: i16, end: i16) {
    send_int(
        c,
        CB_SETEDITSEL,
        0,
        super::make_lparam(start as u16, end as u16),
    );
}

/// The box's parts (its button, item area, edit and list) as the system's
/// GetComboBoxInfo answers them; `None` for a window that is no combo box.
/// No message is sent, so no subclass of the box sees one
/// (`dialog::fill_later`'s takes any CB_ message for a use of the list).
pub fn info(c: HWND) -> Option<COMBOBOXINFO> {
    let mut ci = COMBOBOXINFO {
        cbSize: size_of::<COMBOBOXINFO>() as u32,
        ..Default::default()
    };
    // SAFETY: an out-struct, its size set.
    (unsafe { GetComboBoxInfo(c, &mut ci) } != 0).then_some(ci)
}
