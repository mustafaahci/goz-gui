//! Window subclasses (comctl32's SetWindowSubclass), each a closure that
//! sees a window's messages before its procedure does.
//!
//! A subclass is named by a [`Key`], a `static`: one subclass per key on a
//! window, and the key is how it is removed. The closure is freed when the
//! subclass is removed or the window is destroyed; a call to it that is
//! running keeps it alive until it returns.

use std::rc::Rc;

use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows_sys::Win32::UI::Shell::{GetWindowSubclass, RemoveWindowSubclass, SetWindowSubclass};
use windows_sys::Win32::UI::WindowsAndMessaging::WM_NCDESTROY;

use super::msg::{Msg, Via};

/// A subclass's name. Declare one per kind of subclass, as a `static`:
/// its address is the subclass's id. The name also gives the key a size,
/// and so each `static` an address of its own (zero-sized ones may share
/// one).
pub struct Key {
    /// What the subclass is, for a reader (and a debugger).
    name: &'static str,
}

impl core::fmt::Debug for Key {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "subclass::Key({:?})", self.name)
    }
}

impl Key {
    /// A key named `name`.
    pub const fn new(name: &'static str) -> Key {
        Key { name }
    }

    fn id(&'static self) -> usize {
        core::ptr::from_ref(self) as usize
    }
}

type Handler = Rc<dyn Fn(&Msg) -> LRESULT>;

/// The subclass `key` of `hwnd`: its box, if it has one.
fn slot(hwnd: HWND, key: &'static Key) -> Option<*mut Handler> {
    let mut data = 0usize;
    // SAFETY: an out-value.
    let found = unsafe { GetWindowSubclass(hwnd, Some(subclass_proc), key.id(), &mut data) };
    (found != 0 && data != 0).then_some(data as *mut Handler)
}

/// Subclasses `hwnd` as `key`: `handler` sees its messages first, and
/// passes one on with [`Msg::def`]. Replaces the subclass `key` the window
/// had. False if the window could not be subclassed (it is not there, or is
/// another thread's: refused here, as the handler and its box are this
/// thread's), and then `handler` is dropped.
pub fn set(hwnd: HWND, key: &'static Key, handler: impl Fn(&Msg) -> LRESULT + 'static) -> bool {
    if !super::is_this_thread(hwnd) {
        return false;
    }
    let old = slot(hwnd, key);
    let new: *mut Handler = Box::into_raw(Box::new(Rc::new(handler)));
    // SAFETY: the trampoline below, with the box as its data; the box stays
    // the subclass's until `remove` or WM_NCDESTROY frees it.
    if unsafe { SetWindowSubclass(hwnd, Some(subclass_proc), key.id(), new as usize) } == 0 {
        // SAFETY: the window did not take the box.
        drop(unsafe { Box::from_raw(new) });
        return false;
    }
    if let Some(old) = old {
        // SAFETY: the box the subclass had, which it no longer refers to (a
        // call to it that is running has its own clone of the handler).
        drop(unsafe { Box::from_raw(old) });
    }
    true
}

/// Removes subclass `key` from `hwnd`; false if it had none, or is a
/// window of another thread (whose subclasses are its thread's).
pub fn remove(hwnd: HWND, key: &'static Key) -> bool {
    super::is_this_thread(hwnd) && remove_id(hwnd, key.id())
}

fn remove_id(hwnd: HWND, id: usize) -> bool {
    let mut data = 0usize;
    // SAFETY: an out-value; then the subclass is removed before its box is
    // freed, so the box is not reached again (a running call to it has its
    // own clone of the handler).
    unsafe {
        if GetWindowSubclass(hwnd, Some(subclass_proc), id, &mut data) == 0 {
            return false;
        }
        RemoveWindowSubclass(hwnd, Some(subclass_proc), id);
        if data != 0 {
            drop(Box::from_raw(data as *mut Handler));
        }
    }
    true
}

/// Whether `hwnd`, a window of this thread, has subclass `key` (false for
/// another thread's).
pub fn has(hwnd: HWND, key: &'static Key) -> bool {
    super::is_this_thread(hwnd) && slot(hwnd, key).is_some()
}

/// The trampoline of every subclass here: `data` is the subclass's
/// `Box<Handler>`.
unsafe extern "system" fn subclass_proc(
    hwnd: HWND,
    msg: u32,
    w: WPARAM,
    l: LPARAM,
    id: usize,
    data: usize,
) -> LRESULT {
    // SAFETY: the box `set` made, which is freed only after the subclass is
    // removed or replaced; the handler is cloned, so it lives through a
    // call that removes the subclass or destroys the window.
    let handler = Rc::clone(unsafe { &*(data as *const Handler) });
    let drop_hold = super::dropfiles::Delivery::enter(msg, w);
    // SAFETY: the parameters as delivered.
    let sending = unsafe { super::busy::hold_sender(hwnd, msg, w, l) };
    // SAFETY: the parameters the system delivered, for this call.
    let answer = handler(&unsafe { Msg::delivered(hwnd, msg, w, l, Via::Subclass) });
    drop(sending);
    drop(drop_hold);
    if msg == WM_NCDESTROY {
        remove_id(hwnd, id);
    }
    answer
}
