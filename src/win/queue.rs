//! A thread's message queue: taking messages and dispatching them.
//!
//! A message taken from the queue is a [`Queued`], which only this module
//! makes: so what is dispatched to a window procedure (and so reaches a
//! trampoline as a delivered [`super::Msg`]) is what the system queued,
//! never a message made up in Rust.

use windows_sys::Win32::Foundation::{HANDLE, HWND, LRESULT};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    DispatchMessageW, GetMessageW, HACCEL, IsDialogMessageW, MSG,
    MSG_WAIT_FOR_MULTIPLE_OBJECTS_EX_FLAGS, MsgWaitForMultipleObjectsEx, PEEK_MESSAGE_REMOVE_TYPE,
    PeekMessageW, PostQuitMessage, PostThreadMessageW, QUEUE_STATUS_FLAGS, TranslateAcceleratorW,
    TranslateMessage,
};

use super::sync::Event;

/// A message the system queued for this thread.
pub struct Queued(MSG);

impl Queued {
    /// What the message is (a copy).
    pub fn msg(&self) -> MSG {
        self.0
    }
}

/// Waits for and takes the next message (of `filter`'s windows; all for
/// null); `None` at WM_QUIT (or if the queue failed).
pub fn get(filter: HWND) -> Option<Queued> {
    let mut msg = MSG::default();
    // SAFETY: an out-struct.
    let got = unsafe { GetMessageW(&mut msg, filter, 0, 0) };
    (got > 0).then_some(Queued(msg))
}

/// The next message in `min..=max` (0, 0: any) of `filter`'s windows, taken
/// off the queue as `remove` says (PM_REMOVE, PM_NOREMOVE…), without
/// waiting.
pub fn peek(
    filter: HWND,
    (min, max): (u32, u32),
    remove: PEEK_MESSAGE_REMOVE_TYPE,
) -> Option<Queued> {
    let mut msg = MSG::default();
    // SAFETY: an out-struct.
    let got = unsafe { PeekMessageW(&mut msg, filter, min, max, remove) };
    (got != 0).then_some(Queued(msg))
}

/// Makes sure this thread has a message queue (a peek makes one).
pub fn ensure() {
    let _ = peek(core::ptr::null_mut(), (0, 0), 0);
}

/// Turns a key message into its characters (TranslateMessage).
pub fn translate(q: &Queued) {
    // SAFETY: a message the queue gave.
    unsafe { TranslateMessage(&q.0) };
}

/// Hands the message to its window's procedure.
pub fn dispatch(q: &Queued) -> LRESULT {
    // SAFETY: a message the queue gave, dispatched as it came.
    unsafe { DispatchMessageW(&q.0) }
}

/// Lets dialog `dlg` handle the message if it is its keyboard's (Tab,
/// Enter, Esc, access keys); whether it did.
pub fn is_dialog_message(dlg: HWND, q: &Queued) -> bool {
    // SAFETY: a message the queue gave.
    unsafe { IsDialogMessageW(dlg, &q.0) != 0 }
}

/// Runs the accelerator table `table` for window `hwnd` on the message;
/// whether it was one of its keys.
pub fn translate_accelerator(hwnd: HWND, table: HACCEL, q: &Queued) -> bool {
    // SAFETY: a message the queue gave; the table is a handle the system
    // checks.
    unsafe { TranslateAcceleratorW(hwnd, table, &q.0) != 0 }
}

/// Ends this thread's message loop with `code` (posts WM_QUIT).
pub fn post_quit(code: i32) {
    // SAFETY: no pointers.
    unsafe { PostQuitMessage(code) };
}

/// Posts goz-gui's private message `msg` (WM_APP and up, see
/// [`super::post_app`]) to thread `thread`'s queue; false if it could not.
///
/// # Panics
/// If `msg` is not one of goz-gui's private messages.
pub fn post_thread(thread: u32, msg: u32, w: usize, l: isize) -> bool {
    assert!(
        (windows_sys::Win32::UI::WindowsAndMessaging::WM_APP..super::msg::APP_END).contains(&msg),
        "not one of goz-gui's private messages"
    );
    // SAFETY: goz-gui's own messages carry integers, never pointers.
    unsafe { PostThreadMessageW(thread, msg, w, l) != 0 }
}

/// What [`msg_wait`] woke for.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Woke {
    /// Event `i` of those waited on was set.
    Event(usize),
    /// A message came (one of `wake`'s kinds).
    Message,
    Timeout,
    Failed,
}

/// Waits for one of `events` to be set or for a message of `wake`'s kinds
/// (QS_ALLINPUT…), up to `ms` (INFINITE: no limit), MWMO_* `flags`.
pub fn msg_wait(
    events: &[&Event],
    ms: u32,
    wake: QUEUE_STATUS_FLAGS,
    flags: MSG_WAIT_FOR_MULTIPLE_OBJECTS_EX_FLAGS,
) -> Woke {
    let handles: Vec<HANDLE> = events.iter().map(|e| e.handle()).collect();
    let n = handles.len() as u32;
    let ptr = if handles.is_empty() {
        core::ptr::null()
    } else {
        handles.as_ptr()
    };
    // SAFETY: `n` handles the events own (alive during the call).
    let r = unsafe { MsgWaitForMultipleObjectsEx(n, ptr, ms, wake, flags) };
    match r {
        r if r < n => Woke::Event(r as usize),
        r if r == n => Woke::Message,
        0x102 => Woke::Timeout,
        _ => Woke::Failed,
    }
}
