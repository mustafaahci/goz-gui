//! Data objects (IDataObject): what the clipboard and drag and drop carry
//! of the shell's items, read and written by copy, the OLE clipboard's,
//! and dragging one out.

use core::mem::ManuallyDrop;

use windows::Win32::Foundation::{HGLOBAL, HWND as WHWND};
use windows::Win32::System::Com::{
    CoTaskMemFree, DATADIR_GET, DVASPECT_CONTENT, FORMATETC, IDataObject, STGMEDIUM, STGMEDIUM_0,
    TYMED_HGLOBAL,
};
use windows::Win32::System::Ole::{DROPEFFECT, IDropSource, OleGetClipboard, ReleaseStgMedium};
use windows::Win32::UI::Shell::SHDoDragDrop;
use windows::core::Interface;
use windows_sys::Win32::Foundation::{GlobalFree, HWND};
use windows_sys::Win32::System::Memory::{
    GMEM_MOVEABLE, GlobalAlloc, GlobalLock, GlobalSize, GlobalUnlock,
};
use windows_sys::Win32::System::Ole::{CF_HDROP, OleFlushClipboard};
use windows_sys::Win32::UI::Shell::Common::ITEMIDLIST;
use windows_sys::Win32::UI::Shell::{ILFindChild, SHCreateDataObject, SHMultiFileProperties};

use super::Pidl;
use crate::win::len_u32;
use crate::win::locale::ansi_to_wide;

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

    /// The files its CF_HDROP names, in order; `None` if it has none.
    pub fn files(&self) -> Option<Vec<Vec<u16>>> {
        drop_files(&self.get(CF_HDROP)?)
    }

    /// The clipboard formats it offers to be read (EnumFormatEtc,
    /// DATADIR_GET), in its order.
    pub fn formats(&self) -> Vec<u16> {
        let mut out = Vec::new();
        // SAFETY: a call on a data object of this thread.
        let Ok(formats) = (unsafe { self.0.EnumFormatEtc(DATADIR_GET.0 as u32) }) else {
            return out;
        };
        loop {
            let mut one = [FORMATETC::default()];
            let mut got = 0u32;
            // SAFETY: one FORMATETC into a local array of one, and the
            // count.
            if unsafe { formats.Next(&mut one, Some(&mut got)) }.is_err() || got == 0 {
                break;
            }
            out.push(one[0].cfFormat);
            if !one[0].ptd.is_null() {
                // SAFETY: a target device the enumerator allocated for its
                // caller, who frees it (CoTaskMemFree) and does not keep it.
                unsafe { CoTaskMemFree(Some(one[0].ptd.cast())) };
            }
        }
        out
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

/// The paths a DROPFILES block lists, read within the block: its header
/// (`pFiles`, `pt`, `fNC`, `fWide`), then NUL-separated names that end
/// with an empty one, UTF-16 when `fWide`, else in the ANSI code page.
fn drop_files(block: &[u8]) -> Option<Vec<Vec<u16>>> {
    let field = |at: usize| -> Option<u32> {
        let b = block.get(at..at + 4)?;
        Some(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    };
    let (start, wide) = (field(0)? as usize, field(16)? != 0);
    let list = block.get(start..)?;
    let mut out = Vec::new();
    if wide {
        let units: Vec<u16> = list
            .as_chunks::<2>()
            .0
            .iter()
            .map(|&c| u16::from_le_bytes(c))
            .collect();
        for name in units.split(|&c| c == 0) {
            if name.is_empty() {
                break;
            }
            out.push(name.to_vec());
        }
    } else {
        for name in list.split(|&c| c == 0) {
            if name.is_empty() {
                break;
            }
            out.push(ansi_to_wide(name));
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A DROPFILES block of `names`, wide or not.
    fn dropfiles(names: &[&str], wide: bool) -> Vec<u8> {
        let mut out = Vec::new();
        for v in [20u32, 0, 0, 0, u32::from(wide)] {
            out.extend(v.to_le_bytes());
        }
        for name in names {
            if wide {
                out.extend(
                    name.encode_utf16()
                        .chain(Some(0))
                        .flat_map(u16::to_le_bytes),
                );
            } else {
                out.extend(name.bytes().chain(Some(0)));
            }
        }
        out.extend(if wide {
            [0u8, 0].as_slice()
        } else {
            [0u8].as_slice()
        });
        out
    }

    #[test]
    fn a_drop_lists_its_files_within_the_block() {
        let w = |s: &str| s.encode_utf16().collect::<Vec<u16>>();
        let names = [r"C:\a.txt", r"C:\dir\b c.png"];
        for wide in [true, false] {
            let got = drop_files(&dropfiles(&names, wide)).expect("a list");
            assert_eq!(got, vec![w(names[0]), w(names[1])], "wide {wide}");
        }
        // Cut short, or pointing past the block: no more than is there.
        let mut short = dropfiles(&names, true);
        short.truncate(26);
        assert_eq!(drop_files(&short), Some(vec![w(r"C:\")]));
        assert_eq!(drop_files(&[0u8; 12]), None);
        let mut past = dropfiles(&names, true);
        past[0] = 200;
        assert_eq!(drop_files(&past), None);
    }
}
