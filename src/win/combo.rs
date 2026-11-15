//! Combo-box messages (CB_* are below WM_USER: the same for every window).

use windows_sys::Win32::Foundation::HWND;
use windows_sys::Win32::UI::Controls::{CB_SETMINVISIBLE, COMBOBOXINFO, GetComboBoxInfo};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CB_ADDSTRING, CB_DELETESTRING, CB_FINDSTRINGEXACT, CB_GETCOUNT, CB_GETCURSEL,
    CB_GETDROPPEDSTATE, CB_GETEDITSEL, CB_GETITEMDATA, CB_GETLBTEXT, CB_GETLBTEXTLEN,
    CB_INSERTSTRING, CB_RESETCONTENT, CB_SELECTSTRING, CB_SETCURSEL, CB_SETDROPPEDWIDTH,
    CB_SETEDITSEL, CB_SETITEMDATA, CB_SHOWDROPDOWN,
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

/// Inserts `text` at `i` (-1: at the end); its index.
pub fn insert(c: HWND, i: i32, text: &str) -> i32 {
    let z = super::wz(text);
    // SAFETY: as in `add`.
    unsafe { super::send_raw(c, CB_INSERTSTRING, i as isize as usize, z.as_ptr() as isize) as i32 }
}

/// Deletes item `i`; how many are left (CB_ERR if there was none).
pub fn delete(c: HWND, i: i32) -> i32 {
    send_int(c, CB_DELETESTRING, i as isize as usize, 0) as i32
}

/// Empties the list.
pub fn reset(c: HWND) {
    send_int(c, CB_RESETCONTENT, 0, 0);
}

/// How many items the list has.
pub fn count(c: HWND) -> i32 {
    send_int(c, CB_GETCOUNT, 0, 0) as i32
}

/// The selected item (CB_ERR -1 for none).
pub fn cur_sel(c: HWND) -> i32 {
    send_int(c, CB_GETCURSEL, 0, 0) as i32
}

/// Selects item `i` (-1: none).
pub fn set_cur_sel(c: HWND, i: i32) -> i32 {
    send_int(c, CB_SETCURSEL, i as isize as usize, 0) as i32
}

/// The text of item `i` (empty if there is none).
pub fn text(c: HWND, i: i32) -> String {
    let len = send_int(c, CB_GETLBTEXTLEN, i as isize as usize, 0);
    let Ok(len) = usize::try_from(len) else {
        return String::new();
    };
    // Room for the text and its NUL, or for the item's data, which a list
    // without strings copies instead.
    let mut buf = vec![0u16; (len + 1).max(8)];
    // SAFETY: a buffer with room for the text CB_GETLBTEXTLEN measured and
    // its NUL (or the item data).
    let n = unsafe {
        super::send_raw(
            c,
            CB_GETLBTEXT,
            i as isize as usize,
            buf.as_mut_ptr() as isize,
        )
    };
    let n = usize::try_from(n).unwrap_or(0).min(len);
    String::from_utf16_lossy(&buf[..n])
}

/// The index of the item whose text is `text` (ignoring case), from after
/// `after` (-1: from the start); -1 if none.
pub fn find_exact(c: HWND, after: i32, text: &str) -> i32 {
    let z = super::wz(text);
    // SAFETY: a NUL-terminated string read during the call.
    unsafe {
        super::send_raw(
            c,
            CB_FINDSTRINGEXACT,
            after as isize as usize,
            z.as_ptr() as isize,
        ) as i32
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

/// The number item `i` keeps (CB_SETITEMDATA's).
pub fn item_data(c: HWND, i: i32) -> isize {
    send_int(c, CB_GETITEMDATA, i as isize as usize, 0)
}

/// Keeps a number with item `i`.
pub fn set_item_data(c: HWND, i: i32, data: isize) {
    send_int(c, CB_SETITEMDATA, i as isize as usize, data);
}

/// Drops the list down, or closes it.
pub fn show_dropdown(c: HWND, show: bool) {
    send_int(c, CB_SHOWDROPDOWN, usize::from(show), 0);
}

/// Whether the list is dropped down.
pub fn dropped(c: HWND) -> bool {
    send_int(c, CB_GETDROPPEDSTATE, 0, 0) != 0
}

/// The least width of the dropped-down list, in pixels.
pub fn set_dropped_width(c: HWND, width: i32) {
    send_int(c, CB_SETDROPPEDWIDTH, width as isize as usize, 0);
}

/// The selection in the box's edit: (start, end).
pub fn edit_sel(c: HWND) -> (u32, u32) {
    let (mut start, mut end) = (0u32, 0u32);
    // SAFETY: CB_GETEDITSEL's wParam and lParam are u32 outs.
    unsafe {
        super::send_raw(
            c,
            CB_GETEDITSEL,
            (&raw mut start) as usize,
            (&raw mut end) as isize,
        )
    };
    (start, end)
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

/// How many items the drop-down shows at most (CB_SETMINVISIBLE, the
/// class's own message).
pub fn set_min_visible(c: HWND, n: i32) {
    let c = super::of_class(c, "ComboBox");
    send_int(c, CB_SETMINVISIBLE, n as isize as usize, 0);
}
