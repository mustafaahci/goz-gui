//! Values that go with one of goz-gui's private messages, such as a
//! background thread's result for a window.
//!
//! The message carries a ticket, never a pointer: the value waits here
//! until the window's handler claims it with [`take`], by ticket and by
//! type. A ticket that names nothing (a message posted by another process,
//! one claimed already, one of another type) claims nothing.

use std::any::Any;
use std::collections::HashMap;
use std::sync::Mutex;
use std::sync::atomic::{AtomicIsize, Ordering};

use windows_sys::Win32::Foundation::{HWND, LRESULT, WPARAM};

use super::msg::Msg;

struct Parcel {
    /// The window it was sent to, as an integer (an HWND is not Send).
    to: usize,
    value: Box<dyn Any + Send>,
}

static WAITING: Mutex<Option<HashMap<isize, Parcel>>> = Mutex::new(None);
static NEXT: AtomicIsize = AtomicIsize::new(1);

fn ticket() -> isize {
    // Never 0, so an lParam of 0 claims nothing.
    loop {
        let t = NEXT.fetch_add(1, Ordering::Relaxed);
        if t != 0 {
            return t;
        }
    }
}

fn put(t: isize, to: HWND, value: Box<dyn Any + Send>) {
    let mut w = WAITING.lock().unwrap_or_else(|e| e.into_inner());
    w.get_or_insert_with(HashMap::new).insert(
        t,
        Parcel {
            to: to as usize,
            value,
        },
    );
}

fn remove(t: isize) -> Option<Parcel> {
    let mut w = WAITING.lock().unwrap_or_else(|e| e.into_inner());
    w.as_mut()?.remove(&t)
}

/// Posts goz-gui's private message `msg` (see [`super::post_app`]) to
/// `hwnd` with `value`; false if it could not be posted, and then `value`
/// is dropped. From any thread.
pub fn post<T: Send + 'static>(hwnd: HWND, msg: u32, w: WPARAM, value: T) -> bool {
    let t = ticket();
    put(t, hwnd, Box::new(value));
    if super::post_app(hwnd, msg, w, t) {
        return true;
    }
    drop(remove(t));
    false
}

/// Sends goz-gui's private message `msg` to `hwnd` with `value`, and waits
/// for the answer; the value is dropped then if the handler did not take it.
pub fn send<T: Send + 'static>(hwnd: HWND, msg: u32, w: WPARAM, value: T) -> LRESULT {
    let t = ticket();
    put(t, hwnd, Box::new(value));
    let answer = super::send_app(hwnd, msg, w, t);
    drop(remove(t));
    answer
}

/// Takes the `T` that came with `m` (sent with [`post`] or [`send`]);
/// `None` if `m` brought none, or one of another type (which is left).
pub fn take<T: 'static>(m: &Msg) -> Option<T> {
    let mut w = WAITING.lock().unwrap_or_else(|e| e.into_inner());
    let waiting = w.as_mut()?;
    let parcel = waiting.get(&m.l)?;
    if parcel.to != m.hwnd as usize || !parcel.value.is::<T>() {
        return None;
    }
    let parcel = waiting.remove(&m.l)?;
    drop(w);
    parcel.value.downcast::<T>().ok().map(|b| *b)
}

/// Drops what still waits for `hwnd`, which is being destroyed: the
/// messages that would have claimed it will not come.
pub(crate) fn discard_for(hwnd: HWND) {
    let mut w = WAITING.lock().unwrap_or_else(|e| e.into_inner());
    let Some(waiting) = w.as_mut() else {
        return;
    };
    if waiting.is_empty() {
        return;
    }
    let to = hwnd as usize;
    let gone: Vec<Parcel> = waiting
        .extract_if(|_, p| p.to == to)
        .map(|(_, p)| p)
        .collect();
    // Dropped without the lock: a value's drop may post another parcel.
    drop(w);
    drop(gone);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_parcel_is_claimed_once_by_its_type_and_window() {
        let hwnd = core::ptr::without_provenance_mut(0x1234);
        let t = ticket();
        put(t, hwnd, Box::new(String::from("hi")));
        let m = Msg::new(hwnd, 0x8001, 0, t);
        assert_eq!(take::<u32>(&m), None);
        let other = Msg::new(core::ptr::without_provenance_mut(0x99), 0x8001, 0, t);
        assert_eq!(take::<String>(&other), None);
        assert_eq!(take::<String>(&m).as_deref(), Some("hi"));
        assert_eq!(take::<String>(&m), None);
    }

    #[test]
    fn what_waits_for_a_window_goes_with_it() {
        let hwnd = core::ptr::without_provenance_mut(0x5678);
        let t = ticket();
        put(t, hwnd, Box::new(7u8));
        discard_for(hwnd);
        assert_eq!(take::<u8>(&Msg::new(hwnd, 0x8001, 0, t)), None);
    }
}
