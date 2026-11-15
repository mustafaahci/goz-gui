//! Data objects (IDataObject): what the clipboard and drag and drop carry
//! of the shell's items, read and written by copy, the OLE clipboard's,
//! and dragging one out.

use core::mem::ManuallyDrop;

use windows::Win32::Foundation::{HGLOBAL, HWND as WHWND};
use windows::Win32::System::Com::{
    DVASPECT_CONTENT, FORMATETC, IDataObject, STGMEDIUM, STGMEDIUM_0, TYMED_HGLOBAL,
};
use windows::Win32::System::Ole::{DROPEFFECT, IDropSource, OleGetClipboard, ReleaseStgMedium};
use windows::Win32::UI::Shell::SHDoDragDrop;
use windows::core::Interface;
use windows_sys::Win32::Foundation::{GlobalFree, HWND};
use windows_sys::Win32::System::Memory::{
    GMEM_MOVEABLE, GlobalAlloc, GlobalLock, GlobalSize, GlobalUnlock,
};
use windows_sys::Win32::System::Ole::OleFlushClipboard;
use windows_sys::Win32::UI::Shell::Common::ITEMIDLIST;
use windows_sys::Win32::UI::Shell::{ILFindChild, SHCreateDataObject, SHMultiFileProperties};

use super::Pidl;
use crate::win::len_u32;

/// A data object (IDataObject), released when this drops. It belongs to
/// the thread (the apartment) that got it: it is not `Send`.
pub struct DataObject(pub(super) IDataObject);

impl DataObject {
    /// A data object of `items`, each held by `folder`, as children of it
    /// (SHCreateDataObject, which copies what it keeps); `None` if one of
    /// them is not under `folder`, or the shell made none.
    pub fn of_children(folder: &Pidl, items: &[&Pidl]) -> Option<DataObject> {
        let mut children: Vec<*const ITEMIDLIST> = Vec::with_capacity(items.len());
        for item in items {
            // SAFETY: two valid lists; the child points into `item`'s,
            // which outlives its use below.
            let child = unsafe { ILFindChild(folder.ptr(), item.ptr()) };
            if child.is_null() {
                return None;
            }
            children.push(child.cast_const());
        }
        let mut data = core::ptr::null_mut();
        // SAFETY: a valid folder and `children.len()` children of it, alive
        // for the call; no inner object; the interface id and an
        // out-pointer for an IDataObject, whose reference is this one's.
        let hr = unsafe {
            SHCreateDataObject(
                folder.ptr(),
                len_u32(children.len()),
                children.as_ptr(),
                core::ptr::null_mut(),
                core::ptr::from_ref(&IDataObject::IID).cast(),
                &mut data,
            )
        };
        if hr < 0 || data.is_null() {
            return None;
        }
        // SAFETY: an IDataObject (the IID asked for), not null, whose
        // reference the call handed over.
        Some(DataObject(unsafe { IDataObject::from_raw(data) }))
    }

    /// The clipboard's data object (OleGetClipboard); `None` without OLE
    /// on this thread, or if it could not be had.
    pub fn clipboard() -> Option<DataObject> {
        // SAFETY: no pointers; the object comes back owned.
        unsafe { OleGetClipboard() }.ok().map(DataObject)
    }

    /// Shows the shell's Properties for its items (SHMultiFileProperties):
    /// one sheet for them all, which takes its own reference.
    pub fn show_properties(&self) -> bool {
        // SAFETY: a live IDataObject this holds a reference to.
        unsafe { SHMultiFileProperties(self.0.as_raw(), 0) >= 0 }
    }

    /// A copy of the bytes it holds as clipboard format `cf` in global
    /// memory (GetData, TYMED_HGLOBAL); `None` if it holds none.
    pub fn get(&self, cf: u16) -> Option<Vec<u8>> {
        let format = hglobal_format(cf);
        // SAFETY: the format, read during the call; the medium comes back
        // owned, released below.
        let mut medium = unsafe { self.0.GetData(&format) }.ok()?;
        let bytes = if medium.tymed == TYMED_HGLOBAL.0 as u32 {
            // SAFETY: a TYMED_HGLOBAL medium's union is its HGLOBAL.
            let mem = unsafe { medium.u.hGlobal };
            // SAFETY: the medium's own block, alive until it is released.
            unsafe { copy_global(mem) }
        } else {
            None
        };
        // SAFETY: the medium GetData handed over, released once.
        unsafe { ReleaseStgMedium(&mut medium) };
        bytes
    }

    /// Gives it `bytes` as clipboard format `cf` in global memory
    /// (SetData, which takes the memory); false if it would not take them.
    pub fn set(&self, cf: u16, bytes: &[u8]) -> bool {
        let format = hglobal_format(cf);
        // SAFETY: a block of `bytes.len()` bytes, filled while locked. With
        // fRelease the object owns it once SetData succeeds; it is freed
        // here otherwise.
        unsafe {
            let mem = GlobalAlloc(GMEM_MOVEABLE, bytes.len());
            if mem.is_null() {
                return false;
            }
            let p = GlobalLock(mem) as *mut u8;
            if p.is_null() {
                GlobalFree(mem);
                return false;
            }
            core::ptr::copy_nonoverlapping(bytes.as_ptr(), p, bytes.len());
            GlobalUnlock(mem);
            let medium = STGMEDIUM {
                tymed: TYMED_HGLOBAL.0 as u32,
                u: STGMEDIUM_0 {
                    hGlobal: HGLOBAL(mem),
                },
                pUnkForRelease: ManuallyDrop::new(None),
            };
            let ok = self.0.SetData(&format, &medium, true).is_ok();
            if !ok {
                GlobalFree(mem);
            }
            ok
        }
    }

    /// Drags it out of `owner` with the shell's drag image and cursors
    /// (SHDoDragDrop with its own drop source, which follows either mouse
    /// button), allowing the DROPEFFECT_* in `allowed`. Returns once it is
    /// dropped or cancelled, the drag's loop dispatching this thread's
    /// messages meanwhile, with the effect the target reported (0: none, or
    /// failure).
    pub fn drag(&self, owner: HWND, allowed: u32) -> u32 {
        // SAFETY: a data object of this thread; no drop source of goz-gui's
        // (the shell makes its own).
        unsafe {
            SHDoDragDrop(
                Some(WHWND(owner)),
                &self.0,
                None::<&IDropSource>,
                DROPEFFECT(allowed),
            )
        }
        .map(|e| e.0)
        .unwrap_or(0)
    }
}

/// Renders a data object of this thread's that is on the clipboard onto
/// it, so the clipboard keeps it once the thread is gone
/// (OleFlushClipboard); nothing without one, or without OLE.
pub fn flush_clipboard() {
    // SAFETY: no pointers.
    unsafe { OleFlushClipboard() };
}

/// A FORMATETC for the content of format `cf` in global memory.
fn hglobal_format(cf: u16) -> FORMATETC {
    FORMATETC {
        cfFormat: cf,
        ptd: core::ptr::null_mut(),
        dwAspect: DVASPECT_CONTENT.0,
        lindex: -1,
        tymed: TYMED_HGLOBAL.0 as u32,
    }
}

/// A copy of global memory block `mem`'s bytes, read while it is locked.
///
/// # Safety
/// `mem` is a global memory block that stays allocated during the call.
unsafe fn copy_global(mem: HGLOBAL) -> Option<Vec<u8>> {
    // SAFETY: as the caller promises; GlobalSize bytes of the locked block
    // are copied out, then it is unlocked.
    unsafe {
        let size = GlobalSize(mem.0);
        let p = GlobalLock(mem.0) as *const u8;
        if p.is_null() {
            return None;
        }
        let out = core::slice::from_raw_parts(p, size).to_vec();
        GlobalUnlock(mem.0);
        Some(out)
    }
}
