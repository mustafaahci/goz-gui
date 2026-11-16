//! Kernel events, owned: a handle made here is closed when its owner
//! drops, and no other code can close it.

use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, WAIT_OBJECT_0};
use windows_sys::Win32::System::Threading::{
    CreateEventW, ResetEvent, SetEvent, WaitForMultipleObjects, WaitForSingleObject,
};

/// A kernel event (CreateEventW), closed when it drops. Shared between
/// threads by reference (an `Arc` of it).
pub struct Event(HANDLE);

// SAFETY: a kernel event handle may be used and closed from any thread.
unsafe impl Send for Event {}
// SAFETY: as above; the methods take `&self` and the kernel synchronizes.
unsafe impl Sync for Event {}

impl Event {
    /// A new unnamed event, reset by hand (`manual`) or by the wait that
    /// it releases, `set` to begin with or not. `None` if the system could
    /// not make one.
    pub fn new(manual: bool, set: bool) -> Option<Event> {
        // SAFETY: no security attributes, no name.
        let h = unsafe {
            CreateEventW(
                core::ptr::null(),
                manual.into(),
                set.into(),
                core::ptr::null(),
            )
        };
        (!h.is_null()).then_some(Event(h))
    }

    /// Sets the event (signalled).
    pub fn set(&self) {
        // SAFETY: the handle this owns.
        unsafe { SetEvent(self.0) };
    }

    /// Resets the event (not signalled).
    pub fn reset(&self) {
        // SAFETY: the handle this owns.
        unsafe { ResetEvent(self.0) };
    }

    /// Waits up to `ms` (INFINITE: no limit) for the event; whether it was
    /// set.
    pub fn wait(&self, ms: u32) -> bool {
        // SAFETY: the handle this owns.
        unsafe { WaitForSingleObject(self.0, ms) == WAIT_OBJECT_0 }
    }

    /// The handle, for the waits here (it stays this event's).
    pub(crate) fn handle(&self) -> HANDLE {
        self.0
    }
}

impl Drop for Event {
    fn drop(&mut self) {
        // SAFETY: the handle this owns, closed once.
        unsafe { CloseHandle(self.0) };
    }
}

/// Waits up to `ms` for any of `events` (or all, with `all`); the index of
/// the one set (the first, with `all`), `None` on a timeout or failure.
pub fn wait_any(events: &[&Event], all: bool, ms: u32) -> Option<usize> {
    let handles: Vec<HANDLE> = events.iter().map(|e| e.0).collect();
    // SAFETY: the handles the events own, alive during the call.
    let r =
        unsafe { WaitForMultipleObjects(handles.len() as u32, handles.as_ptr(), all.into(), ms) };
    let i = r.wrapping_sub(WAIT_OBJECT_0) as usize;
    (i < handles.len()).then_some(i)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_event_is_set_and_reset() {
        let e = Event::new(true, false).unwrap();
        assert!(!e.wait(0));
        e.set();
        assert!(e.wait(0));
        assert_eq!(wait_any(&[&e], false, 0), Some(0));
        e.reset();
        assert!(!e.wait(0));
    }
}
