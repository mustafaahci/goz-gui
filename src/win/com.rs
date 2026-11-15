//! COM apartments. A thread's COM objects live while its apartment does,
//! so the apartment is a guard: uninitialized when it drops, after the
//! objects the code that holds it made (which are dropped first, being
//! made later, or are tied to the guard's scope).

use core::marker::PhantomData;

use windows_sys::Win32::System::Com::{
    COINIT_APARTMENTTHREADED, COINIT_DISABLE_OLE1DDE, COINIT_MULTITHREADED, CoInitializeEx,
    CoUninitialize,
};

/// This thread's COM apartment, uninitialized when it drops. Not `Send`: it
/// belongs to the thread that made it.
pub struct Apartment {
    _not_send: PhantomData<*mut ()>,
}

impl Apartment {
    /// A single-threaded apartment for this thread (the UI thread's kind:
    /// the shell's objects want one). `None` if this thread has one of the
    /// other kind already.
    pub fn sta() -> Option<Apartment> {
        init((COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE) as u32)
    }

    /// A multithreaded apartment for this thread (a worker's).
    pub fn mta() -> Option<Apartment> {
        init(COINIT_MULTITHREADED as u32)
    }

    /// Keeps the apartment until the thread ends (the UI thread's, whose
    /// objects live as long as it does).
    pub fn keep(self) {
        core::mem::forget(self);
    }
}

fn init(flags: u32) -> Option<Apartment> {
    // SAFETY: no pointers (the reserved one is null).
    let hr = unsafe { CoInitializeEx(core::ptr::null(), flags) };
    // S_OK, or S_FALSE (already initialized alike), each to be balanced.
    (hr >= 0).then_some(Apartment {
        _not_send: PhantomData,
    })
}

impl Drop for Apartment {
    fn drop(&mut self) {
        // SAFETY: balances the CoInitializeEx that made this guard, on its
        // thread (the guard is not Send).
        unsafe { CoUninitialize() };
    }
}
