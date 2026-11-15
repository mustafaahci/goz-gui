//! The clipboard: opened by a guard, its data copied out and in. What it
//! holds stays the clipboard's: a block is read only while the clipboard is
//! open, and copied before it closes.

use core::marker::PhantomData;

use windows_sys::Win32::Foundation::{GlobalFree, HWND};
use windows_sys::Win32::System::DataExchange::{
    CloseClipboard, EmptyClipboard, GetClipboardData, GetClipboardSequenceNumber,
    IsClipboardFormatAvailable, OpenClipboard, RegisterClipboardFormatW, SetClipboardData,
};
use windows_sys::Win32::System::Memory::{
    GMEM_MOVEABLE, GlobalAlloc, GlobalLock, GlobalSize, GlobalUnlock,
};
use windows_sys::Win32::System::Ole::{CF_HDROP, CF_TEXT, CF_UNICODETEXT};
use windows_sys::Win32::UI::Shell::HDROP;

use super::wz;

/// The clipboard, opened by this thread (OpenClipboard): closed when this
/// drops, on this thread (it is not `Send`).
pub struct Clipboard {
    _thread: PhantomData<*mut ()>,
}

/// Whether the clipboard's data in `format` is a global memory block,
/// which is what [`Clipboard::data`] reads and [`Clipboard::set`] gives:
/// text, file drops, and the registered formats (others, such as bitmaps,
/// are handles of other kinds).
fn is_global_format(format: u32) -> bool {
    [CF_TEXT, CF_UNICODETEXT, CF_HDROP]
        .iter()
        .any(|&f| u32::from(f) == format)
        || (0xc000..=0xffff).contains(&format)
}

impl Clipboard {
    /// Opens the clipboard for `hwnd` (null: this thread); `None` when
    /// another window has it open.
    pub fn open(hwnd: HWND) -> Option<Clipboard> {
        // SAFETY: no pointers; it is closed when the guard drops.
        (unsafe { OpenClipboard(hwnd) } != 0).then_some(Clipboard {
            _thread: PhantomData,
        })
    }

    /// Empties it, making the window that opened it its owner.
    pub fn empty(&self) -> bool {
        // SAFETY: no pointers; the clipboard is open.
        unsafe { EmptyClipboard() != 0 }
    }

    /// A copy of its data in `format` (text, CF_HDROP or a registered
    /// format; see [`is_global_format`]); `None` when it holds none.
    pub fn data(&self, format: u32) -> Option<Vec<u8>> {
        if !is_global_format(format) {
            return None;
        }
        // SAFETY: the clipboard is open; the handle stays its own while it
        // is.
        let mem = unsafe { GetClipboardData(format) };
        if mem.is_null() {
            return None;
        }
        // SAFETY: a global memory block (the format's kind), locked while
        // it is copied out, no further than its size, and unlocked.
        unsafe {
            let p = GlobalLock(mem) as *const u8;
            if p.is_null() {
                return None;
            }
            let bytes = core::slice::from_raw_parts(p, GlobalSize(mem)).to_vec();
            GlobalUnlock(mem);
            Some(bytes)
        }
    }

    /// The paths of the files on it (CF_HDROP); empty when it holds none.
    pub fn files(&self) -> Vec<Vec<u16>> {
        // SAFETY: the clipboard is open; the handle stays its own while it
        // is.
        let drop = unsafe { GetClipboardData(u32::from(CF_HDROP)) } as HDROP;
        if drop.is_null() {
            return Vec::new();
        }
        // SAFETY: the clipboard's CF_HDROP, valid while it is open (for
        // the call), and not freed here (it stays the clipboard's).
        unsafe { super::dropfiles::read_paths(drop) }
    }

    /// Puts `data` on it as `format` (text, CF_HDROP or a registered
    /// format); false if it could not.
    pub fn set(&self, format: u32, data: &[u8]) -> bool {
        if !is_global_format(format) {
            return false;
        }
        // SAFETY: the block is sized for `data` and filled while locked; the
        // clipboard owns it once SetClipboardData succeeds, and it is freed
        // here otherwise.
        unsafe {
            let mem = GlobalAlloc(GMEM_MOVEABLE, data.len().max(1));
            if mem.is_null() {
                return false;
            }
            let dst = GlobalLock(mem) as *mut u8;
            let mut ok = false;
            if !dst.is_null() {
                core::ptr::copy_nonoverlapping(data.as_ptr(), dst, data.len());
                GlobalUnlock(mem);
                ok = !SetClipboardData(format, mem).is_null();
            }
            if !ok {
                GlobalFree(mem);
            }
            ok
        }
    }
}

impl Drop for Clipboard {
    fn drop(&mut self) {
        // SAFETY: closes the clipboard this guard opened, on its thread.
        unsafe { CloseClipboard() };
    }
}

/// The clipboard's sequence number: it changes with anything put on it.
pub fn sequence() -> u32 {
    // SAFETY: no pointers.
    unsafe { GetClipboardSequenceNumber() }
}

/// Whether the clipboard holds `format` (it need not be open).
pub fn has(format: u32) -> bool {
    // SAFETY: no pointers.
    unsafe { IsClipboardFormatAvailable(format) != 0 }
}

/// The clipboard format registered as `name` (made if new); 0 if it could
/// not be.
pub fn register_format(name: &str) -> u32 {
    let name = wz(name);
    // SAFETY: a NUL-terminated name.
    unsafe { RegisterClipboardFormatW(name.as_ptr()) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_global_memory_formats_are_read_or_put() {
        assert!(is_global_format(u32::from(CF_UNICODETEXT)));
        assert!(is_global_format(0xc0ff));
        // CF_BITMAP's data is a bitmap, not a memory block.
        assert!(!is_global_format(2));
    }
}
