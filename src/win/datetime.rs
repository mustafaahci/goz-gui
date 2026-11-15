//! Date and time picker messages (SysDateTimePick32). DTM_FIRST is
//! LVM_FIRST, so each is sent only to a picker.

use windows_sys::Win32::Foundation::{HWND, SYSTEMTIME};
use windows_sys::Win32::UI::Controls::{
    DTM_GETMONTHCAL, DTM_GETSYSTEMTIME, DTM_SETSYSTEMTIME, GDT_NONE, GDT_VALID,
};

use super::{of_class, send_raw};

/// The date and time picker's class.
pub const CLASS: &str = "SysDateTimePick32";

/// The picker's date and time; `None` while it has none (its check box
/// cleared, with DTS_SHOWNONE) and for a window that is no picker.
pub fn time(picker: HWND) -> Option<SYSTEMTIME> {
    // GDT_VALID is 0, which is also what a message to no window answers:
    // no picker, no time.
    let picker = of_class(picker, CLASS);
    if picker.is_null() {
        return None;
    }
    let mut st = SYSTEMTIME::default();
    // SAFETY: DTM_GETSYSTEMTIME's lParam is a SYSTEMTIME the picker fills,
    // alive for the call; sent only to a picker.
    let r = unsafe { send_raw(picker, DTM_GETSYSTEMTIME, 0, (&raw mut st) as isize) };
    (r == GDT_VALID as isize).then_some(st)
}

/// Sets the picker's date and time, or with `None` takes it away (clears
/// its check box, with DTS_SHOWNONE); false if the picker refused.
pub fn set_time(picker: HWND, time: Option<&SYSTEMTIME>) -> bool {
    let (flag, l) = match time {
        Some(st) => (GDT_VALID, core::ptr::from_ref(st) as isize),
        None => (GDT_NONE, 0),
    };
    // SAFETY: DTM_SETSYSTEMTIME's lParam is a SYSTEMTIME read during the
    // call (not read with GDT_NONE, and null then); sent only to a picker.
    unsafe { send_raw(of_class(picker, CLASS), DTM_SETSYSTEMTIME, flag as usize, l) != 0 }
}

/// The month calendar the picker has dropped down; null while it has none.
pub fn calendar(picker: HWND) -> HWND {
    // SAFETY: DTM_GETMONTHCAL has no parameters; sent only to a picker.
    unsafe { send_raw(of_class(picker, CLASS), DTM_GETMONTHCAL, 0, 0) as HWND }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn picker_messages_go_only_to_pickers() {
        // A window that is not there: no picker, nothing sent.
        assert!(time(core::ptr::null_mut()).is_none());
        assert!(calendar(core::ptr::null_mut()).is_null());
    }
}
