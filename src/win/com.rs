//! COM apartments, and OLE. COM, once initialized on a thread, stays so
//! for the rest of the thread's life: it is never uninitialized here, as
//! CoUninitialize unloads the libraries the thread's objects run from,
//! and nothing ties those objects (wrappers of their interfaces, which
//! safe code may keep anywhere) to a guard. COM cleans up after a thread
//! as it ends.

use core::cell::Cell;
use core::marker::PhantomData;

use windows_sys::Win32::System::Com::{
    COINIT_APARTMENTTHREADED, COINIT_DISABLE_OLE1DDE, CoFreeUnusedLibrariesEx, CoInitializeEx,
};
use windows_sys::Win32::System::Ole::OleInitialize;

thread_local! {
    /// Whether this thread's libraries are never to be unloaded early
    /// ([`Apartment::end_unloading_unused`] refuses): its apartment was
    /// kept, or it made an Explorer context menu (whose handlers are known
    /// to answer DllCanUnloadNow wrongly, see `crate::shell::ContextMenu`).
    static KEPT: Cell<bool> = const { Cell::new(false) };
}

/// Marks this thread as one whose COM libraries are never unloaded early.
pub(crate) fn refuse_unloading() {
    KEPT.set(true);
}

/// This thread's COM apartment, which stays for the rest of the thread's
/// life (it is never uninitialized). Not `Send`: it belongs to the thread
/// that made it.
pub struct Apartment {
    _not_send: PhantomData<*mut ()>,
}

impl Apartment {
    /// A single-threaded apartment for this thread (the UI thread's kind:
    /// the shell's objects want one), for the rest of its life. `None` if
    /// this thread has one of the other kind already.
    pub fn sta() -> Option<Apartment> {
        init((COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE) as u32)
    }

    /// Keeps the apartment until the thread ends (the UI thread's, whose
    /// objects live as long as it does), and marks the thread: its
    /// libraries are never unloaded early ([`Self::end_unloading_unused`]
    /// refuses on it).
    pub fn keep(self) {
        refuse_unloading();
    }

    /// Unloads now the in-process COM servers loaded for this thread's
    /// apartment that say they can go (CoFreeUnusedLibrariesEx with no
    /// delay, each asked by its DllCanUnloadNow), rather than ten minutes
    /// after they last served; for a thread whose objects are all released,
    /// as it ends. A server that still has one says so, and stays. Nothing
    /// on a thread whose apartment was kept, or that made an Explorer
    /// context menu.
    pub fn end_unloading_unused(self) {
        if KEPT.get() {
            return;
        }
        // SAFETY: no pointers. A library is unloaded only once its
        // DllCanUnloadNow says nothing of it is in use, and the trust model
        // (`win`'s documentation) takes that answer as true for the servers
        // of a thread neither kept nor making context menus.
        unsafe { CoFreeUnusedLibrariesEx(0, 0) };
    }
}

/// Initialises OLE on this thread (OleInitialize, an STA's, never
/// uninitialized: it stays for the thread's life); whether it is.
pub fn ole_initialize() -> bool {
    // SAFETY: the reserved pointer is null.
    unsafe { OleInitialize(core::ptr::null()) >= 0 }
}

/// Initializes COM on this thread as a single-threaded apartment with OLE
/// 1.0's DDE left on (COINIT_APARTMENTTHREADED alone), for the rest of the
/// thread's life: it is never uninitialized, so nothing made in it can
/// outlive it. False if the thread has an apartment of another kind.
pub fn sta_with_ole1dde() -> bool {
    // SAFETY: no pointers (the reserved one is null); never balanced, which
    // only keeps COM loaded on the thread.
    unsafe { CoInitializeEx(core::ptr::null(), COINIT_APARTMENTTHREADED as u32) >= 0 }
}

fn init(flags: u32) -> Option<Apartment> {
    // SAFETY: no pointers (the reserved one is null); never balanced, which
    // only keeps COM on the thread for the rest of its life.
    let hr = unsafe { CoInitializeEx(core::ptr::null(), flags) };
    // S_OK, or S_FALSE (already initialized alike).
    (hr >= 0).then_some(Apartment {
        _not_send: PhantomData,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_kept_thread_unloads_nothing_early() {
        std::thread::spawn(|| {
            Apartment::sta().expect("an apartment").keep();
            assert!(KEPT.get());
            // Refused: nothing is unloaded.
            Apartment::sta().expect("the same").end_unloading_unused();
        })
        .join()
        .unwrap();
        std::thread::spawn(|| assert!(!KEPT.get())).join().unwrap();
    }
}
