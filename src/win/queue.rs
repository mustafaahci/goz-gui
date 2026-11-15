//! A thread's message queue: taking messages and dispatching them.
//!
//! A message taken from the queue is a [`Queued`], which only this module
//! makes: so what is dispatched to a window procedure (and so reaches a
//! trampoline as a delivered [`super::Msg`]) is what the system queued,
//! never a message made up in Rust. A `Queued` is taken off the queue
//! once and dispatched at most once ([`dispatch`] takes it by value, and
//! [`translate_accelerator`] gives it back only when it did not use it),
//! as the system hands its messages out: a WM_DROPFILES's drop, say, is
//! freed after the one delivery ([`super::dropfiles`]). A message looked
//! at and left in the queue ([`peek`]) is a plain copy.

use windows_sys::Win32::Foundation::{HANDLE, HWND, LRESULT};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    ACCEL, CopyAcceleratorTableW, CreateAcceleratorTableW, DestroyAcceleratorTable,
    DispatchMessageW, GetInputState, GetMessageW, HACCEL, MSG,
    MSG_WAIT_FOR_MULTIPLE_OBJECTS_EX_FLAGS, MsgWaitForMultipleObjectsEx, PEEK_MESSAGE_REMOVE_TYPE,
    PM_REMOVE, PeekMessageW, PostQuitMessage, PostThreadMessageW, QUEUE_STATUS_FLAGS,
    TranslateAcceleratorW, TranslateMessage,
};

use super::sync::Event;

/// A message the system queued for this thread, taken off the queue: not
/// `Clone`, so it is dispatched at most once.
pub struct Queued(MSG);

impl Queued {
    /// What the message is (a copy).
    pub fn msg(&self) -> MSG {
        self.0
    }
}

/// What [`get`] took from the queue.
pub enum Got {
    /// A message, to translate and dispatch.
    Message(Queued),
    /// WM_QUIT: the message loop is to end, with the code
    /// [`post_quit`] gave (its wParam).
    Quit(i32),
    /// The queue failed (`filter` is no window, say).
    Failed,
}

/// Waits for and takes the next message (of `filter`'s windows; all for
/// null).
pub fn get(filter: HWND) -> Got {
    let mut msg = MSG::default();
    // SAFETY: an out-struct.
    match unsafe { GetMessageW(&mut msg, filter, 0, 0) } {
        0 => Got::Quit(msg.wParam as i32),
        -1 => Got::Failed,
        _ => Got::Message(Queued(msg)),
    }
}

/// Takes the next message in `min..=max` (0, 0: any) of `filter`'s windows
/// off the queue (PeekMessageW with PM_REMOVE), without waiting.
pub fn take(filter: HWND, (min, max): (u32, u32)) -> Option<Queued> {
    let mut msg = MSG::default();
    // SAFETY: an out-struct.
    let got = unsafe { PeekMessageW(&mut msg, filter, min, max, PM_REMOVE) };
    (got != 0).then_some(Queued(msg))
}

/// A copy of the next message in `min..=max` (0, 0: any) of `filter`'s
/// windows, left in the queue (PeekMessageW with PM_* `flags`, PM_REMOVE
/// taken off: PM_NOYIELD, PM_QS_SENDMESSAGE…), without waiting. Sent
/// messages that wait are delivered meanwhile, as by any peek. The copy
/// is no [`Queued`]: what stays in the queue is dispatched once, after
/// it is taken.
pub fn peek(filter: HWND, (min, max): (u32, u32), flags: PEEK_MESSAGE_REMOVE_TYPE) -> Option<MSG> {
    let mut msg = MSG::default();
    // SAFETY: an out-struct.
    let got = unsafe { PeekMessageW(&mut msg, filter, min, max, flags & !PM_REMOVE) };
    (got != 0).then_some(msg)
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

/// Hands the message to its window's procedure, once: it is used up.
pub fn dispatch(q: Queued) -> LRESULT {
    // SAFETY: a message the queue gave, taken off it, dispatched as it
    // came, and only this once (`Queued` is moved in).
    unsafe { DispatchMessageW(&q.0) }
}

/// Runs the accelerator table `table` for window `hwnd` on the message:
/// `None` when it was one of the table's keys, whose command went to
/// `hwnd` (the message is used up, as TranslateAcceleratorW has it: it is
/// not to be translated or dispatched); the message back otherwise.
pub fn translate_accelerator(hwnd: HWND, table: HACCEL, q: Queued) -> Option<Queued> {
    // SAFETY: a message the queue gave; the table is a handle the system
    // checks.
    let used = unsafe { TranslateAcceleratorW(hwnd, table, &q.0) != 0 };
    (!used).then_some(q)
}

/// Whether mouse or keyboard input waits in this thread's queue.
pub fn input_waiting() -> bool {
    // SAFETY: no pointers.
    unsafe { GetInputState() != 0 }
}

/// An accelerator table (CreateAcceleratorTableW), destroyed when this
/// drops.
pub struct AccelTable(HACCEL);

impl AccelTable {
    /// A table of `entries` (Windows copies them); `None` for an empty one,
    /// which cannot be made, or when Windows would not make it.
    pub fn new(entries: &[ACCEL]) -> Option<AccelTable> {
        if entries.is_empty() {
            return None;
        }
        // SAFETY: the entries and their count (at least one).
        let h = unsafe { CreateAcceleratorTableW(entries.as_ptr(), super::len_i32(entries.len())) };
        (!h.is_null()).then_some(AccelTable(h))
    }

    /// A copy of its entries.
    pub fn entries(&self) -> Vec<ACCEL> {
        // SAFETY: no buffer: asks for the count.
        let n = unsafe { CopyAcceleratorTableW(self.0, core::ptr::null_mut(), 0) }.max(0);
        let mut entries = vec![ACCEL::default(); n as usize];
        // SAFETY: a buffer of `n` entries, the count passed.
        let got = unsafe { CopyAcceleratorTableW(self.0, entries.as_mut_ptr(), n) }.max(0);
        entries.truncate(got as usize);
        entries
    }

    /// The table's handle, for [`translate_accelerator`] (it stays this
    /// table's).
    pub fn handle(&self) -> HACCEL {
        self.0
    }
}

impl Drop for AccelTable {
    fn drop(&mut self) {
        // SAFETY: the table this owns, destroyed once.
        unsafe { DestroyAcceleratorTable(self.0) };
    }
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
