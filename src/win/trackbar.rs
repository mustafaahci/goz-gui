//! Trackbar messages (msctls_trackbar32), sent only to one.

use windows_sys::Win32::Foundation::HWND;
use windows_sys::Win32::UI::WindowsAndMessaging::WM_USER;

pub const CLASS: &str = "msctls_trackbar32";

// The trackbar's messages (CommCtrl.h), which windows-sys lacks.
const TBM_GETPOS: u32 = WM_USER;
const TBM_SETPOS: u32 = WM_USER + 5;
const TBM_SETRANGEMIN: u32 = WM_USER + 7;
const TBM_SETRANGEMAX: u32 = WM_USER + 8;
const TBM_SETTICFREQ: u32 = WM_USER + 20;
const TBM_SETPAGESIZE: u32 = WM_USER + 21;
const TBM_SETLINESIZE: u32 = WM_USER + 23;

fn send_int(t: HWND, msg: u32, w: usize, l: isize) -> isize {
    // SAFETY: the trackbar's messages here take integers; to a trackbar
    // (or to null).
    unsafe { super::send_raw(super::of_class(t, CLASS), msg, w, l) }
}

pub fn pos(t: HWND) -> i32 {
    send_int(t, TBM_GETPOS, 0, 0) as i32
}

pub fn set_pos(t: HWND, pos: i32, redraw: bool) {
    send_int(t, TBM_SETPOS, usize::from(redraw), pos as isize);
}

pub fn set_range(t: HWND, min: i32, max: i32, redraw: bool) {
    send_int(t, TBM_SETRANGEMIN, 0, min as isize);
    send_int(t, TBM_SETRANGEMAX, usize::from(redraw), max as isize);
}

pub fn set_tic_freq(t: HWND, freq: i32) {
    send_int(t, TBM_SETTICFREQ, freq as isize as usize, 0);
}

pub fn set_page_size(t: HWND, size: i32) {
    send_int(t, TBM_SETPAGESIZE, 0, size as isize);
}

pub fn set_line_size(t: HWND, size: i32) {
    send_int(t, TBM_SETLINESIZE, 0, size as isize);
}
