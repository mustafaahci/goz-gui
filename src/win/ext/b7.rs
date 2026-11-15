//! Batch 7 of the move onto `win`: the shell's COM side (item ID lists,
//! folders, context menus, data objects, shortcuts, item images), preview
//! handlers and the stream and frame goz-gui hands them, WIC, 32-bit DIB
//! sections, accessibility for what goz-gui paints itself, and the few
//! system calls those files make besides.
//!
//! COM interfaces are `!Send` (their wrappers hold a `NonNull`), so the
//! objects here stay on the thread, and in the apartment, that made them.
//! The objects goz-gui implements itself (the preview handler's stream and
//! frame, the accessible object) are private types: the functions here
//! hand out only their interfaces, whose methods are `unsafe` to call, and
//! configure them with Rust values and closures.

use core::mem::ManuallyDrop;
use core::ptr::{NonNull, null, null_mut};

use windows::Win32::Foundation::{
    COLORREF, E_FAIL, E_INVALIDARG, E_NOTIMPL, E_POINTER, FILETIME as WFILETIME, GENERIC_READ,
    HGLOBAL, HWND as WHWND, LPARAM as WLPARAM, LRESULT as WLRESULT, POINT as WPOINT, RECT as WRECT,
    S_FALSE, S_OK, SIZE, STG_E_ACCESSDENIED, STG_E_INVALIDFUNCTION, STG_E_INVALIDPOINTER,
    WPARAM as WWPARAM,
};
use windows::Win32::Graphics::Gdi::{
    FONT_CHARSET, FONT_CLIP_PRECISION, FONT_OUTPUT_PRECISION, FONT_QUALITY, LOGFONTW as WLOGFONTW,
};
use windows::Win32::Graphics::Imaging::{
    CLSID_WICImagingFactory, GUID_WICPixelFormat32bppPBGRA, IWICBitmapCodecInfo, IWICBitmapDecoder,
    IWICBitmapFrameDecode, IWICBitmapScaler, IWICBitmapSource, IWICFormatConverter,
    IWICImagingFactory, IWICPalette, WICBitmapDitherTypeNone, WICBitmapInterpolationModeFant,
    WICBitmapPaletteTypeCustom, WICComponentEnumerateDefault, WICDecodeMetadataCacheOnDemand,
    WICDecoder,
};
use windows::Win32::System::Com::StructuredStorage::{
    PROPVARIANT, PropVariantClear, PropVariantToUInt16,
};
use windows::Win32::System::Com::{
    CLSCTX, CLSCTX_INPROC_SERVER, CoCreateInstance, CoTaskMemAlloc, CoTaskMemFree, DATADIR_GET,
    DISPATCH_FLAGS, DISPPARAMS, DVASPECT_CONTENT, EXCEPINFO, FORMATETC, IDataObject, IDispatch,
    IDispatch_Impl, IPersistFile, ISequentialStream_Impl, IStream, IStream_Impl, ITypeInfo,
    LOCKTYPE, STATFLAG, STATFLAG_NONAME, STATSTG, STGC, STGM_READ, STGMEDIUM, STGMEDIUM_0,
    STGTY_STREAM, STREAM_SEEK, TYMED_HGLOBAL,
};
use windows::Win32::System::Ole::{
    DROPEFFECT, IDropSource, IObjectWithSite, OleGetClipboard, ReleaseStgMedium,
};
use windows::Win32::System::Variant::{VARIANT, VT_EMPTY, VT_I4};
use windows::Win32::UI::Accessibility::{
    CreateStdAccessibleObject, IAccessible, IAccessible_Impl, LresultFromObject,
};
use windows::Win32::UI::Shell::Common::ITEMIDLIST;
use windows::Win32::UI::Shell::PropertiesSystem::{IInitializeWithFile, IInitializeWithStream};
use windows::Win32::UI::Shell::{
    CMINVOKECOMMANDINFO, CMINVOKECOMMANDINFOEX, GCS_VERBW, IContextMenu, IContextMenu2,
    IContextMenu3, IInitializeWithItem, ILClone, ILCombine, ILCreateFromPathW, ILFindLastID,
    ILFree, ILIsParent, ILRemoveLastID, IPreviewHandler, IPreviewHandlerFrame,
    IPreviewHandlerFrame_Impl, IPreviewHandlerVisuals, IShellFolder, IShellItem,
    IShellItemImageFactory, IShellLinkW, PREVIEWHANDLERFRAMEINFO, SHBindToParent,
    SHCreateItemFromParsingName, SHDoDragDrop, SHGetDesktopFolder, SIIGBF,
};
use windows::Win32::UI::WindowsAndMessaging::{HACCEL as WHACCEL, HMENU as WHMENU, MSG as WMSG};
use windows::core::{BSTR, GUID, HRESULT, Interface, PCSTR, PCWSTR, PSTR, PWSTR, Ref, implement};
use windows_sys::Win32::Foundation::{
    CloseHandle, GENERIC_READ as SYS_GENERIC_READ, GlobalFree, HANDLE, HWND, INVALID_HANDLE_VALUE,
    POINT, RECT,
};
use windows_sys::Win32::Globalization::{CP_ACP, MultiByteToWideChar};
use windows_sys::Win32::Graphics::Gdi::{
    BI_RGB, BITMAPINFO, BITMAPINFOHEADER, CreateDIBSection, DIB_RGB_COLORS, DIBSECTION, GetDIBits,
    GetObjectW, HBITMAP, HDC, LOGFONTW, STRETCH_BLT_MODE, SetBrushOrgEx, SetStretchBltMode,
};
use windows_sys::Win32::Storage::FileSystem::{
    BY_HANDLE_FILE_INFORMATION, CreateFileW, FILE_FLAG_OVERLAPPED, FILE_FLAGS_AND_ATTRIBUTES,
    FILE_SHARE_MODE, GetFileAttributesW, GetFileInformationByHandle, INVALID_FILE_ATTRIBUTES,
    OPEN_EXISTING, ReadFile, SetFilePointerEx,
};
use windows_sys::Win32::System::Com::CoFreeUnusedLibrariesEx;
use windows_sys::Win32::System::DataExchange::{
    GetClipboardFormatNameW, GetClipboardSequenceNumber, RegisterClipboardFormatW,
};
use windows_sys::Win32::System::Diagnostics::Debug::{
    FORMAT_MESSAGE_FROM_SYSTEM, FORMAT_MESSAGE_IGNORE_INSERTS, FormatMessageW,
};
use windows_sys::Win32::System::Memory::{
    GMEM_MOVEABLE, GlobalAlloc, GlobalLock, GlobalSize, GlobalUnlock,
};
use windows_sys::Win32::System::Ole::{OleFlushClipboard, OleInitialize};
use windows_sys::Win32::System::Registry::{
    HKEY, KEY_READ, RRF_RT_REG_DWORD, RRF_RT_REG_SZ, RegCloseKey, RegGetValueW, RegOpenKeyExW,
};
use windows_sys::Win32::UI::Shell::{
    ASSOCF_NONE, ASSOCSTR_SHELLEXTENSION, AssocQueryStringW, SHGetPathFromIDListEx,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    ACCEL, CopyAcceleratorTableW, CreateAcceleratorTableW, CreatePopupMenu,
    DestroyAcceleratorTable, DestroyMenu, GetMenuItemCount, GetMessageTime, GetSubMenu, HACCEL,
    HMENU, SW_SHOWNORMAL, SetParent, WM_DRAWITEM, WM_GETOBJECT, WM_INITMENUPOPUP, WM_MEASUREITEM,
    WM_MENUCHAR,
};

use crate::win::gdi;
use crate::win::msg::{Msg, read_nul};
use crate::win::{until_nul, wz, wz_units};

// Windows, the message queue, files, the registry, associations.

/// Makes `parent` the window's parent (HWND_MESSAGE: none, off every
/// window); the parent before, null if it could not.
pub fn set_parent(hwnd: HWND, parent: HWND) -> HWND {
    // SAFETY: no pointers.
    unsafe { SetParent(hwnd, parent) }
}

/// When the message this thread took last was posted (GetMessageTime), in
/// ms since the system started (it wraps).
pub fn message_time() -> u32 {
    // SAFETY: no pointers.
    unsafe { GetMessageTime() as u32 }
}

/// How the DC stretches bitmaps (HALFTONE, COLORONCOLOR…); the mode before.
pub fn set_stretch_blt_mode(hdc: HDC, mode: STRETCH_BLT_MODE) -> i32 {
    // SAFETY: no pointers.
    unsafe { SetStretchBltMode(hdc, mode) }
}

/// Moves the DC's brush origin (the one before is not asked for).
pub fn set_brush_org(hdc: HDC, x: i32, y: i32) {
    // SAFETY: no out-point asked for.
    unsafe { SetBrushOrgEx(hdc, x, y, null_mut()) };
}

/// A file's or folder's attributes (GetFileAttributesW, FILE_ATTRIBUTE_*);
/// the error code when it has none to give (gone, unreachable).
pub fn file_attributes(path: &[u16]) -> Result<u32, u32> {
    let z = wz_units(path);
    // SAFETY: a NUL-terminated path read during the call.
    let attrs = unsafe { GetFileAttributesW(z.as_ptr()) };
    if attrs == INVALID_FILE_ATTRIBUTES {
        Err(crate::win::sys::last_error())
    } else {
        Ok(attrs)
    }
}

/// The system's text for error `code` (FormatMessageW from the system, no
/// inserts) as it gives it, at most 511 units; empty when it has none.
pub fn error_message(code: u32) -> String {
    let mut buf = [0u16; 512];
    // SAFETY: the buffer and its length in units; no inserts are read.
    let n = unsafe {
        FormatMessageW(
            FORMAT_MESSAGE_FROM_SYSTEM | FORMAT_MESSAGE_IGNORE_INSERTS,
            null(),
            code,
            0,
            buf.as_mut_ptr(),
            buf.len() as u32,
            null(),
        )
    };
    String::from_utf16_lossy(&buf[..(n as usize).min(buf.len())])
}

/// `bytes` in the system's ANSI code page (MultiByteToWideChar, CP_ACP).
pub fn ansi_to_wide(bytes: &[u8]) -> Vec<u16> {
    if bytes.is_empty() {
        return Vec::new();
    }
    let len = bytes.len().min(i32::MAX as usize) as i32;
    let mut out = vec![0u16; bytes.len()];
    // SAFETY: `len` bytes in, and room for `len` units out (`out` holds at
    // least that many): a code page character is never more UTF-16 units
    // than bytes, and the call writes no more than the room it is given.
    let n = unsafe { MultiByteToWideChar(CP_ACP, 0, bytes.as_ptr(), len, out.as_mut_ptr(), len) };
    out.truncate(n.max(0) as usize);
    out
}

/// A DWORD value `value` of key `subkey` under `root` (RegGetValueW);
/// `None` if there is none, or it is not a DWORD.
pub fn reg_dword(root: HKEY, subkey: &str, value: &str) -> Option<u32> {
    let (key, name) = (wz(subkey), wz(value));
    let mut data: u32 = 0;
    let mut size = size_of::<u32>() as u32;
    // SAFETY: NUL-terminated names; one DWORD out, its size passed
    // (RRF_RT_REG_DWORD writes four bytes or fails).
    let rc = unsafe {
        RegGetValueW(
            root,
            key.as_ptr(),
            name.as_ptr(),
            RRF_RT_REG_DWORD,
            null_mut(),
            (&raw mut data).cast(),
            &mut size,
        )
    };
    (rc == 0).then_some(data)
}

/// A string value `value` (REG_SZ) of key `subkey` under `root`, up to its
/// NUL; `None` if there is none, it is not a string, or it is longer than
/// `cap` units with its NUL.
pub fn reg_string(root: HKEY, subkey: &[u16], value: &str, cap: usize) -> Option<Vec<u16>> {
    let (key, name) = (wz_units(subkey), wz(value));
    let mut buf = vec![0u16; cap];
    let mut bytes = u32::try_from(cap.checked_mul(2)?).ok()?;
    // SAFETY: NUL-terminated names; the buffer and its size in bytes.
    let status = unsafe {
        RegGetValueW(
            root,
            key.as_ptr(),
            name.as_ptr(),
            RRF_RT_REG_SZ,
            null_mut(),
            buf.as_mut_ptr().cast(),
            &mut bytes,
        )
    };
    (status == 0).then(|| until_nul(&buf).to_vec())
}

/// Whether key `subkey` exists under `root` and can be opened for reading.
pub fn reg_key_exists(root: HKEY, subkey: &[u16]) -> bool {
    let path = wz_units(subkey);
    let mut key: HKEY = null_mut();
    // SAFETY: a NUL-terminated path and an out-handle, which is closed at
    // once.
    unsafe {
        if RegOpenKeyExW(root, path.as_ptr(), 0, KEY_READ, &mut key) != 0 {
            return false;
        }
        RegCloseKey(key);
    }
    true
}

/// The shell extension registered for `assoc` (an extension with its dot)
/// as interface `iid` (`{…}`), through its type and perceived type
/// (AssocQueryStringW, ASSOCSTR_SHELLEXTENSION): its class id as text, up
/// to 63 units; `None` when there is none.
pub fn assoc_shell_extension(assoc: &[u16], iid: &str) -> Option<Vec<u16>> {
    let (assoc, iid) = (wz_units(assoc), wz(iid));
    let mut buf = [0u16; 64];
    let mut len = buf.len() as u32;
    // SAFETY: NUL-terminated strings; the buffer with its length in units.
    let hr = unsafe {
        AssocQueryStringW(
            ASSOCF_NONE,
            ASSOCSTR_SHELLEXTENSION,
            assoc.as_ptr(),
            iid.as_ptr(),
            buf.as_mut_ptr(),
            &mut len,
        )
    };
    (hr == 0).then(|| until_nul(&buf).to_vec())
}

// Accelerator tables and menus (user32 checks their handles).

/// A copy of accelerator table `table`'s entries.
pub fn accel_entries(table: HACCEL) -> Vec<ACCEL> {
    // SAFETY: no buffer: asks for the count.
    let n = unsafe { CopyAcceleratorTableW(table, null_mut(), 0) }.max(0);
    let mut entries = vec![ACCEL::default(); n as usize];
    // SAFETY: a buffer of `n` entries, the count passed.
    let got = unsafe { CopyAcceleratorTableW(table, entries.as_mut_ptr(), n) }.max(0);
    entries.truncate(got as usize);
    entries
}

/// A new accelerator table of `entries` (Windows copies them); null if it
/// could not be made. Destroyed with [`destroy_accel`].
pub fn create_accel(entries: &[ACCEL]) -> HACCEL {
    let Ok(n) = i32::try_from(entries.len()) else {
        return null_mut();
    };
    // SAFETY: the entries and their count, read during the call.
    unsafe { CreateAcceleratorTableW(entries.as_ptr(), n) }
}

/// Destroys an accelerator table; false if it was not one.
pub fn destroy_accel(table: HACCEL) -> bool {
    // SAFETY: no pointers.
    unsafe { DestroyAcceleratorTable(table) != 0 }
}

/// A new empty popup menu; null if it could not be made.
pub fn create_popup_menu() -> HMENU {
    // SAFETY: no pointers.
    unsafe { CreatePopupMenu() }
}

pub fn destroy_menu(menu: HMENU) -> bool {
    // SAFETY: no pointers.
    unsafe { DestroyMenu(menu) != 0 }
}

/// How many items `menu` has (-1 for a menu that is not there).
pub fn menu_item_count(menu: HMENU) -> i32 {
    // SAFETY: no pointers.
    unsafe { GetMenuItemCount(menu) }
}

/// The submenu at position `pos` of `menu`; null if that item opens none.
pub fn sub_menu(menu: HMENU, pos: i32) -> HMENU {
    // SAFETY: no pointers.
    unsafe { GetSubMenu(menu, pos) }
}

// COM, OLE and the clipboard.

/// Unloads at once the in-process COM servers that say they can go
/// (CoFreeUnusedLibrariesEx, DllCanUnloadNow), rather than ten minutes
/// after they last served. A server's answer is its promise that nothing
/// of it is running.
pub fn co_free_unused_libraries() {
    // SAFETY: no pointers; each library goes only once its DllCanUnloadNow
    // says nothing of it is in use.
    unsafe { CoFreeUnusedLibrariesEx(0, 0) };
}

/// Initialises OLE on this thread (OleInitialize, an STA's); whether it is.
pub fn ole_initialize() -> bool {
    // SAFETY: the reserved pointer is null.
    unsafe { OleInitialize(null()) >= 0 }
}

/// Renders a data object of this thread's that is on the clipboard onto
/// it, so the clipboard keeps it once the thread is gone
/// (OleFlushClipboard); nothing without one, or without OLE.
pub fn ole_flush_clipboard() {
    // SAFETY: no pointers.
    unsafe { OleFlushClipboard() };
}

/// The clipboard's data object (OleGetClipboard); `None` without OLE on
/// this thread, or if it could not be had.
pub fn ole_get_clipboard() -> Option<IDataObject> {
    // SAFETY: no pointers; the object comes back owned.
    unsafe { OleGetClipboard() }.ok()
}

/// The clipboard's sequence number, which changes with whatever is put on
/// it.
pub fn clipboard_sequence() -> u32 {
    // SAFETY: reads a counter.
    unsafe { GetClipboardSequenceNumber() }
}

/// The registered clipboard format `name` (0 if it could not be).
pub fn register_clipboard_format(name: &str) -> u32 {
    let z = wz(name);
    // SAFETY: a NUL-terminated name read during the call.
    unsafe { RegisterClipboardFormatW(z.as_ptr()) }
}

/// A registered clipboard format's name; `None` for a standard format.
pub fn clipboard_format_name(cf: u32) -> Option<String> {
    let mut buf = [0u16; 128];
    // SAFETY: the buffer and its length in units.
    let n = unsafe { GetClipboardFormatNameW(cf, buf.as_mut_ptr(), buf.len() as i32) };
    (n > 0).then(|| String::from_utf16_lossy(&buf[..(n as usize).min(buf.len())]))
}

/// A FORMATETC for the content of format `cf` in global memory.
fn hglobal_format(cf: u16) -> FORMATETC {
    FORMATETC {
        cfFormat: cf,
        ptd: null_mut(),
        dwAspect: DVASPECT_CONTENT.0,
        lindex: -1,
        tymed: TYMED_HGLOBAL.0 as u32,
    }
}

/// A copy of the bytes a data object holds as clipboard format `cf` in
/// global memory (GetData, TYMED_HGLOBAL); `None` if it holds none.
pub fn data_get(data: &IDataObject, cf: u16) -> Option<Vec<u8>> {
    let format = hglobal_format(cf);
    // SAFETY: the format, read during the call; the medium comes back
    // owned, released below.
    let mut medium = unsafe { data.GetData(&format) }.ok()?;
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

/// Gives a data object `bytes` as clipboard format `cf` in global memory
/// (SetData, which takes the memory); false if it would not take them.
pub fn data_set(data: &IDataObject, cf: u16, bytes: &[u8]) -> bool {
    let format = hglobal_format(cf);
    // SAFETY: a block of `bytes.len()` bytes, filled while locked. With
    // fRelease the object owns it once SetData succeeds; it is freed here
    // otherwise.
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
        let ok = data.SetData(&format, &medium, true).is_ok();
        if !ok {
            GlobalFree(mem);
        }
        ok
    }
}

/// CF_HDROP.
const CF_HDROP: u16 = 15;

/// The files a data object's CF_HDROP names, in order; `None` if it has
/// none.
pub fn data_files(data: &IDataObject) -> Option<Vec<Vec<u16>>> {
    drop_files(&data_get(data, CF_HDROP)?)
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

/// The clipboard formats a data object offers to be read (EnumFormatEtc,
/// DATADIR_GET), in its order.
pub fn data_formats(data: &IDataObject) -> Vec<u16> {
    let mut out = Vec::new();
    // SAFETY: a call on a data object of this thread.
    let Ok(formats) = (unsafe { data.EnumFormatEtc(DATADIR_GET.0 as u32) }) else {
        return out;
    };
    loop {
        let mut one = [FORMATETC::default()];
        let mut got = 0u32;
        // SAFETY: one FORMATETC into a local array of one, and the count.
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

/// Drags `data` out of `owner` with the shell's drag image and cursors
/// (SHDoDragDrop with its own drop source, which follows either mouse
/// button), allowing the DROPEFFECT_* in `allowed`. Returns once it is
/// dropped or cancelled, the drag's loop dispatching this thread's messages
/// meanwhile, with the effect the target reported (0: none, or failure).
pub fn do_drag_drop(owner: HWND, data: &IDataObject, allowed: u32) -> u32 {
    // SAFETY: a data object of this thread; no drop source of goz-gui's
    // (the shell makes its own).
    unsafe {
        SHDoDragDrop(
            Some(WHWND(owner)),
            data,
            None::<&IDropSource>,
            DROPEFFECT(allowed),
        )
    }
    .map(|e| e.0)
    .unwrap_or(0)
}

// Item ID lists and folders.
//
// An ID list is a chain of IDs, each led by its size, ending with an empty
// one; only the shell makes the lists here, so each is well-formed. The
// folders read what an ID holds as data they check (IDs come from
// anywhere: shortcuts, other processes' drags), so a list given to the
// wrong folder is refused, never followed.

/// An item ID list the shell made, owned: freed (ILFree) when it drops.
/// Absolute (from the desktop) or relative to a folder, as the function
/// that made it says.
pub struct Pidl(NonNull<ITEMIDLIST>);

impl Pidl {
    fn own(p: *mut ITEMIDLIST) -> Option<Pidl> {
        NonNull::new(p).map(Pidl)
    }

    fn as_ptr(&self) -> *const ITEMIDLIST {
        self.0.as_ptr()
    }

    /// The absolute ID list of `path`, parsed whole from the desktop
    /// (ILCreateFromPathW); `None` when the shell finds no such item.
    pub fn of_path(path: &[u16]) -> Option<Pidl> {
        let z = wz_units(path);
        // SAFETY: a NUL-terminated path; the list comes back owned.
        Pidl::own(unsafe { ILCreateFromPathW(PCWSTR(z.as_ptr())) })
    }

    /// Whether it is the empty list (as an absolute one, the desktop).
    pub fn is_empty(&self) -> bool {
        // SAFETY: a list starts with its first ID's size (two bytes, at
        // least those of the terminator), read unaligned.
        unsafe { self.as_ptr().cast::<u16>().read_unaligned() == 0 }
    }

    /// `self` followed by `child` (relative to `self`'s item): a new list
    /// (ILCombine).
    pub fn join(&self, child: &Pidl) -> Option<Pidl> {
        // SAFETY: two well-formed lists, read during the call; the new one
        // comes back owned.
        Pidl::own(unsafe { ILCombine(Some(self.as_ptr()), Some(child.as_ptr())) })
    }

    /// A copy without its last ID: the list of its item's parent (ILClone,
    /// ILRemoveLastID).
    pub fn parent(&self) -> Option<Pidl> {
        // SAFETY: a well-formed list, copied; the copy is owned, and its
        // last ID is cut off in place.
        let copy = Pidl::own(unsafe { ILClone(self.as_ptr()) })?;
        // SAFETY: the copy this owns; ILRemoveLastID only writes the new
        // terminator within it.
        let _ = unsafe { ILRemoveLastID(Some(copy.0.as_ptr())) };
        Some(copy)
    }

    /// Whether `self` is `child`'s parent (`immediate`) or one of its
    /// ancestors (ILIsParent).
    pub fn is_parent_of(&self, child: &Pidl, immediate: bool) -> bool {
        // SAFETY: two well-formed lists, read during the call.
        unsafe { ILIsParent(self.as_ptr(), child.as_ptr(), immediate).as_bool() }
    }

    /// The path of the file or folder an absolute list names
    /// (SHGetPathFromIDListEx); `None` for an item that is neither.
    pub fn path(&self) -> Option<Vec<u16>> {
        let mut buf = vec![0u16; 32_768];
        // SAFETY: a well-formed list; the buffer with its length in units.
        let ok = unsafe {
            SHGetPathFromIDListEx(self.as_ptr().cast(), buf.as_mut_ptr(), buf.len() as u32, 0)
        } != 0;
        let len = buf.iter().position(|&c| c == 0).unwrap_or(0);
        (ok && len > 0).then(|| buf[..len].to_vec())
    }
}

impl Drop for Pidl {
    fn drop(&mut self) {
        // SAFETY: a list the shell allocated for this value alone, freed
        // once.
        unsafe { ILFree(Some(self.as_ptr())) };
    }
}

/// The desktop's folder, the root of the shell's namespace
/// (SHGetDesktopFolder). COM must be initialised on this thread.
pub fn desktop_folder() -> Option<IShellFolder> {
    // SAFETY: no pointers; the folder comes back owned.
    unsafe { SHGetDesktopFolder() }.ok()
}

/// The folder that list `id` (relative to `folder`) names in `folder`
/// (BindToObject).
pub fn bind_folder(folder: &IShellFolder, id: &Pidl) -> Option<IShellFolder> {
    // SAFETY: a well-formed list, read during the call; no bind context.
    unsafe { folder.BindToObject(id.as_ptr(), None) }.ok()
}

/// The list, relative to `folder`, of the item called `name` in it
/// (ParseDisplayName, no attributes asked for); `None` when it is not
/// there.
pub fn parse_name(folder: &IShellFolder, name: &[u16]) -> Option<Pidl> {
    let name = wz_units(name);
    let mut id = null_mut();
    let mut attributes = 0u32;
    // SAFETY: a NUL-terminated name and out-values that outlive the call;
    // the list, when it succeeds, comes back owned.
    let ok = unsafe {
        folder.ParseDisplayName(
            WHWND::default(),
            None,
            PCWSTR(name.as_ptr()),
            None,
            &mut id,
            &mut attributes,
        )
    }
    .is_ok();
    if ok { Pidl::own(id) } else { None }
}

/// The folder that holds the item absolute list `id` names
/// (SHBindToParent).
pub fn bind_to_parent(id: &Pidl) -> Option<IShellFolder> {
    // SAFETY: a well-formed list, read during the call; the last ID is not
    // asked for.
    unsafe { SHBindToParent(id.as_ptr(), None) }.ok()
}

/// One of `folder`'s UI objects (a context menu, a data object) for the
/// items `ids` name, owned by `owner` (GetUIObjectOf). With `last`, each
/// item is its list's last ID (the lists are absolute ones of items that
/// `folder` holds); else each list whole, relative to `folder`.
pub fn ui_object<T: Interface>(
    folder: &IShellFolder,
    owner: HWND,
    ids: &[Pidl],
    last: bool,
) -> Option<T> {
    let children: Vec<*const ITEMIDLIST> = ids
        .iter()
        .map(|id| {
            if last {
                // SAFETY: a well-formed list; the pointer is into it, which
                // `ids` keeps alive across the call below.
                unsafe { ILFindLastID(id.as_ptr()) }.cast_const()
            } else {
                id.as_ptr()
            }
        })
        .collect();
    // SAFETY: lists (or IDs within them) that `ids` keeps alive during the
    // call; the object comes back owned, as interface `T`.
    unsafe { folder.GetUIObjectOf::<T>(WHWND(owner), &children, None) }.ok()
}

// Shortcuts.

/// A shell link object: a shortcut (`.lnk`) read or made.
pub struct ShellLink(IShellLinkW);

impl ShellLink {
    /// A new, empty one (in process). COM must be initialised on this
    /// thread.
    pub fn new() -> Option<ShellLink> {
        // SAFETY: a class id and no outer object; the object comes back
        // owned.
        let link = unsafe {
            CoCreateInstance(
                &windows::Win32::UI::Shell::ShellLink,
                None,
                CLSCTX_INPROC_SERVER,
            )
        }
        .ok()?;
        Some(ShellLink(link))
    }

    /// Reads the shortcut file at `path` (IPersistFile::Load, read only);
    /// false if it could not.
    pub fn load(&self, path: &[u16]) -> bool {
        let z = wz_units(path);
        let Ok(file) = self.0.cast::<IPersistFile>() else {
            return false;
        };
        // SAFETY: a NUL-terminated path read during the call.
        unsafe { file.Load(PCWSTR(z.as_ptr()), STGM_READ) }.is_ok()
    }

    /// What it points to, as an absolute ID list (GetIDList).
    pub fn id_list(&self) -> Option<Pidl> {
        // SAFETY: no pointers in; the list comes back owned.
        Pidl::own(unsafe { self.0.GetIDList() }.ok()?)
    }

    /// Points it at `target` (SetPath).
    pub fn set_path(&self, target: &[u16]) -> bool {
        let z = wz_units(target);
        // SAFETY: a NUL-terminated path read during the call.
        unsafe { self.0.SetPath(PCWSTR(z.as_ptr())) }.is_ok()
    }

    /// Writes it to `path` (IPersistFile::Save, the object then known as
    /// that file's).
    pub fn save(&self, path: &[u16]) -> bool {
        let z = wz_units(path);
        let Ok(file) = self.0.cast::<IPersistFile>() else {
            return false;
        };
        // SAFETY: a NUL-terminated path read during the call.
        unsafe { file.Save(PCWSTR(z.as_ptr()), true) }.is_ok()
    }
}

// Context menus.

/// The Explorer context menu of some items (IContextMenu, and the versions
/// that draw their own items).
pub struct ShellMenu {
    menu: IContextMenu,
    menu2: Option<IContextMenu2>,
    menu3: Option<IContextMenu3>,
}

impl ShellMenu {
    pub fn new(menu: IContextMenu) -> ShellMenu {
        ShellMenu {
            menu2: menu.cast().ok(),
            menu3: menu.cast().ok(),
            menu,
        }
    }

    /// Has the handlers add their items to `menu` from position `at`, with
    /// ids from `first` to `last`, as CMF_* `flags` ask (QueryContextMenu);
    /// how many ids they used, `None` if it failed.
    pub fn query(&self, menu: HMENU, at: u32, first: u32, last: u32, flags: u32) -> Option<u32> {
        // SAFETY: a menu handle (user32 checks it, as the handlers add to
        // it); no pointers.
        let hr = unsafe {
            self.menu
                .QueryContextMenu(WHMENU(menu), at, first, last, flags)
        };
        // Success carries how many ids were used.
        hr.is_ok().then_some((hr.0 & 0xffff) as u32)
    }

    /// The canonical verb (`copy`, `delete`, `properties`…) of the item
    /// `offset` ids after the first, if its handler names one (GCS_VERBW).
    pub fn verb(&self, offset: usize) -> Option<String> {
        let mut buf = [0u16; 260];
        // SAFETY: a buffer of `len` code units for a Unicode verb; the
        // handler writes at most that many, NUL included.
        let ok = unsafe {
            self.menu
                .GetCommandString(
                    offset,
                    GCS_VERBW,
                    None,
                    PSTR(buf.as_mut_ptr().cast()),
                    buf.len() as u32,
                )
                .is_ok()
        };
        let len = buf.iter().position(|&c| c == 0).unwrap_or(0);
        (ok && len > 0).then(|| String::from_utf16_lossy(&buf[..len]))
    }

    /// Runs the item `offset` ids after the first for `owner`, as chosen at
    /// `pt` (screen), with CMIC_MASK_* `mask` (InvokeCommand with a
    /// CMINVOKECOMMANDINFOEX). False if the handler failed, or `offset` is
    /// not one an item can have (it travels as a resource id, under
    /// 0x10000).
    pub fn invoke(&self, owner: HWND, offset: u32, pt: POINT, mask: u32) -> bool {
        // MAKEINTRESOURCE: the offset travels as the pointer's address,
        // which only below 0x10000 is read as an id, not a string.
        if offset >= 0x1_0000 {
            return false;
        }
        let verb = offset as usize;
        let info = CMINVOKECOMMANDINFOEX {
            cbSize: size_of::<CMINVOKECOMMANDINFOEX>() as u32,
            fMask: mask,
            hwnd: WHWND(owner),
            lpVerb: PCSTR(core::ptr::without_provenance(verb)),
            lpVerbW: PCWSTR(core::ptr::without_provenance(verb)),
            nShow: SW_SHOWNORMAL,
            ptInvoke: WPOINT { x: pt.x, y: pt.y },
            ..Default::default()
        };
        // SAFETY: the EX structure (its cbSize says so), which outlives the
        // call: its verbs are resource ids (checked above), and its other
        // pointers (parameters, directory, title) null, which every mask
        // allows. The handler may dispatch messages meanwhile.
        unsafe {
            self.menu
                .InvokeCommand((&raw const info).cast::<CMINVOKECOMMANDINFO>())
                .is_ok()
        }
    }

    /// A message of the menu while it is up, for the handlers that fill
    /// their submenus as they open (WM_INITMENUPOPUP, WM_MENUCHAR) or draw
    /// their own items (WM_MEASUREITEM, WM_DRAWITEM: only as the system
    /// delivered them); `Some` with the answer when one took it, `None` for
    /// any other message.
    pub fn message(&self, m: &Msg) -> Option<isize> {
        match m.id {
            WM_MEASUREITEM => {
                let view = m.measure_item()?;
                let l = view.as_ptr() as isize;
                // SAFETY: the MEASUREITEMSTRUCT the system delivered, kept
                // from any other use by the view held across the call.
                let answer = unsafe { self.message_raw(m.id, m.w, l) };
                drop(view);
                answer
            }
            WM_DRAWITEM => {
                let view = m.draw_item()?;
                let l = view.as_ptr() as isize;
                // SAFETY: the DRAWITEMSTRUCT the system delivered, kept
                // from any other use by the view held across the call.
                let answer = unsafe { self.message_raw(m.id, m.w, l) };
                drop(view);
                answer
            }
            WM_INITMENUPOPUP | WM_MENUCHAR => {
                // SAFETY: their parameters are a menu handle and integers.
                unsafe { self.message_raw(m.id, m.w, m.l) }
            }
            _ => None,
        }
    }

    /// [`ShellMenu::message`] with the message's parameters as they came.
    ///
    /// # Safety
    /// `w` and `l` are what message `msg` carries as the system delivers
    /// it: for WM_MEASUREITEM and WM_DRAWITEM, `l` points at a live
    /// MEASUREITEMSTRUCT or DRAWITEMSTRUCT the handler may write to.
    pub(crate) unsafe fn message_raw(&self, msg: u32, w: usize, l: isize) -> Option<isize> {
        // SAFETY: as the caller promises, passed on unchanged while the menu
        // they are about is up; the answer comes into a local.
        unsafe {
            if let Some(m3) = &self.menu3 {
                let mut result = WLRESULT(0);
                m3.HandleMenuMsg2(msg, WWPARAM(w), WLPARAM(l), Some(&mut result))
                    .ok()?;
                return Some(result.0);
            }
            let m2 = self.menu2.as_ref()?;
            m2.HandleMenuMsg(msg, WWPARAM(w), WLPARAM(l)).ok()?;
            Some(0)
        }
    }
}

// The shell's images of items.

/// The shell's image maker for an item (IShellItemImageFactory).
pub struct ImageFactory(IShellItemImageFactory);

impl ImageFactory {
    /// The one of the item at `path` (SHCreateItemFromParsingName). COM
    /// must be initialised on this thread.
    pub fn of_path(path: &[u16]) -> Option<ImageFactory> {
        let z = wz_units(path);
        // SAFETY: a NUL-terminated path read during the call; no bind
        // context.
        let factory = unsafe { SHCreateItemFromParsingName(PCWSTR(z.as_ptr()), None) }.ok()?;
        Some(ImageFactory(factory))
    }

    /// Its image, at most `side` pixels square, as SIIGBF_* `flags` ask
    /// (GetImage): a bitmap now the caller's.
    pub fn image(&self, side: i32, flags: SIIGBF) -> windows::core::Result<gdi::Owned> {
        // SAFETY: no pointers in; the bitmap comes back the caller's.
        let made = unsafe { self.0.GetImage(SIZE { cx: side, cy: side }, flags) }?;
        Ok(gdi::Owned::new(made.0))
    }
}

// 32-bit DIB sections. Their pixels are GDI's memory, which GDI may draw
// into: they are copied in and out within a call (after GdiFlush), never
// lent as a Rust slice, and found again from the handle each time (a
// bitmap deleted since has none). They live until the bitmap is deleted:
// its owner does that, as with every GDI object here.

fn header32(w: i32, h: i32) -> BITMAPINFOHEADER {
    BITMAPINFOHEADER {
        biSize: size_of::<BITMAPINFOHEADER>() as u32,
        biWidth: w,
        // Top-down.
        biHeight: -h,
        biPlanes: 1,
        biBitCount: 32,
        biCompression: BI_RGB,
        ..Default::default()
    }
}

/// A new 32-bit top-down DIB section `w` by `h`, all zero
/// (CreateDIBSection); null if it could not be made. The caller deletes it.
pub fn create_dib32(w: i32, h: i32) -> HBITMAP {
    if w <= 0 || h <= 0 {
        return null_mut();
    }
    let bi = BITMAPINFO {
        bmiHeader: header32(w, h),
        ..Default::default()
    };
    let mut bits = null_mut();
    // SAFETY: a complete header for a section the system allocates; the
    // bits it hands back are not kept (`dib_write` and `dib_pixels` ask GDI
    // for them again).
    let made =
        unsafe { CreateDIBSection(null_mut(), &bi, DIB_RGB_COLORS, &mut bits, null_mut(), 0) };
    if !made.is_null() && bits.is_null() {
        gdi::delete_object(made);
        return null_mut();
    }
    made
}

/// The bits of `bmp`, how many pixels they hold, and its width and height,
/// if it is a 32-bit DIB section with rows of exactly its width. (Which
/// way its rows go GDI does not say here; the ones `create_dib32` makes go
/// down.)
fn dib32_bits(bmp: HBITMAP) -> Option<(NonNull<[u8; 4]>, usize, i32, i32)> {
    let mut ds = DIBSECTION::default();
    // SAFETY: an out-struct and its size.
    let n = unsafe { GetObjectW(bmp, size_of::<DIBSECTION>() as i32, (&raw mut ds).cast()) };
    // A bitmap that is not a DIB section fills only the BITMAP.
    if usize::try_from(n).ok()? != size_of::<DIBSECTION>() {
        return None;
    }
    let bm = ds.dsBm;
    let (w, h) = (bm.bmWidth, bm.bmHeight);
    if bm.bmBitsPixel != 32 || w <= 0 || h <= 0 {
        return None;
    }
    if i64::from(bm.bmWidthBytes) != i64::from(w) * 4 {
        return None;
    }
    let count = (w as usize).checked_mul(h as usize)?;
    let bits = NonNull::new(bm.bmBits.cast::<[u8; 4]>())?;
    Some((bits, count, w, h))
}

/// Copies `pixels` (BGRA, rows in the section's order: top-down for one
/// [`create_dib32`] made) into DIB section `bmp`, which is a 32-bit one of
/// exactly that many pixels; false, and nothing written, for any other
/// bitmap.
pub fn dib_write(bmp: HBITMAP, pixels: &[[u8; 4]]) -> bool {
    // What GDI has still to draw lands before, not after, the copy.
    gdi::flush();
    let Some((bits, count, _, _)) = dib32_bits(bmp) else {
        return false;
    };
    if count != pixels.len() {
        return false;
    }
    // SAFETY: the section's own bits, `count` pixels as GDI describes them
    // (32 bits, rows of its width), alive while the bitmap is; written by a
    // raw copy within this call, no Rust reference made into them. `pixels`
    // is Rust memory of its own.
    unsafe { core::ptr::copy_nonoverlapping(pixels.as_ptr(), bits.as_ptr(), count) };
    true
}

/// A copy of DIB section `bmp`'s pixels (BGRA, rows in its order: top-down
/// for one [`create_dib32`] made), with its width and height, if it is a
/// 32-bit one.
pub fn dib_pixels(bmp: HBITMAP) -> Option<(i32, i32, Vec<[u8; 4]>)> {
    // What GDI has still to draw is in the bits before they are read.
    gdi::flush();
    let (bits, count, w, h) = dib32_bits(bmp)?;
    let mut out = vec![[0u8; 4]; count];
    // SAFETY: as in `dib_write`, read into a buffer of `count` pixels.
    unsafe { core::ptr::copy_nonoverlapping(bits.as_ptr(), out.as_mut_ptr(), count) };
    Some((w, h, out))
}

/// The pixels of bitmap `src` (of any kind and depth) as `w` by `h` 32-bit
/// top-down BGRA, through the screen's DC (GetDIBits); `None` if GDI could
/// not give them.
pub fn bitmap_pixels(src: HBITMAP, w: i32, h: i32) -> Option<Vec<[u8; 4]>> {
    if w <= 0 || h <= 0 {
        return None;
    }
    let count = (w as usize).checked_mul(h as usize)?;
    let mut px = vec![[0u8; 4]; count];
    let mut bi = BITMAPINFO {
        bmiHeader: header32(w, h),
        ..Default::default()
    };
    let dc = gdi::WindowDc::get(null_mut());
    // SAFETY: a buffer of `w * h` 32-bit pixels: what the header asks for
    // (`h` rows of `w * 4` bytes; at 32 bits no colour table is written);
    // the header is in-out.
    let lines = unsafe {
        GetDIBits(
            dc.hdc(),
            src,
            0,
            h as u32,
            px.as_mut_ptr().cast(),
            &mut bi,
            DIB_RGB_COLORS,
        )
    };
    drop(dc);
    (lines != 0).then_some(px)
}

// Preview handlers.

/// A preview handler (IPreviewHandler): a COM object, usually in a
/// surrogate process, that draws a file into a window it is given.
pub struct PreviewHandler(IPreviewHandler);

impl PreviewHandler {
    /// A new one of class `clsid`, made as `context` says
    /// (CLSCTX_LOCAL_SERVER…). COM must be initialised on this thread.
    pub fn create(clsid: &GUID, context: CLSCTX) -> windows::core::Result<PreviewHandler> {
        // SAFETY: a class id read during the call, no outer object; the
        // object comes back owned.
        unsafe { CoCreateInstance(clsid, None, context) }.map(PreviewHandler)
    }

    /// Gives it its file as a stream, if it takes one
    /// (IInitializeWithStream): `open` makes the stream once that is known.
    /// False if it takes none, `open` made none, or it refused it.
    pub fn init_with_stream(&self, open: impl FnOnce() -> Option<IStream>) -> bool {
        let Ok(init) = self.0.cast::<IInitializeWithStream>() else {
            return false;
        };
        let Some(stream) = open() else {
            return false;
        };
        // SAFETY: a stream of this thread, which the handler keeps a
        // reference to (counted) if it wants.
        unsafe { init.Initialize(&stream, STGM_READ.0) }.is_ok()
    }

    /// Gives it its file by path, if it takes one (IInitializeWithFile);
    /// false if it does not or refused it.
    pub fn init_with_file(&self, path: &[u16]) -> bool {
        let z = wz_units(path);
        let Ok(init) = self.0.cast::<IInitializeWithFile>() else {
            return false;
        };
        // SAFETY: a NUL-terminated path read during the call.
        unsafe { init.Initialize(PCWSTR(z.as_ptr()), STGM_READ.0) }.is_ok()
    }

    /// Gives it its file as a shell item, if it takes one
    /// (IInitializeWithItem); false if it does not, the shell has no item
    /// there, or it refused it.
    pub fn init_with_item(&self, path: &[u16]) -> bool {
        let z = wz_units(path);
        let Ok(init) = self.0.cast::<IInitializeWithItem>() else {
            return false;
        };
        // SAFETY: a NUL-terminated path read during the call; no bind
        // context.
        let item =
            unsafe { SHCreateItemFromParsingName::<_, _, IShellItem>(PCWSTR(z.as_ptr()), None) };
        let Ok(item) = item else {
            return false;
        };
        // SAFETY: an item of this thread, kept (counted) if it wants.
        unsafe { init.Initialize(&item, STGM_READ.0) }.is_ok()
    }

    /// The colours and font it draws with, if it takes them
    /// (IPreviewHandlerVisuals); what it refuses is let be.
    pub fn set_visuals(&self, bg: u32, fg: u32, font: &LOGFONTW) {
        let Ok(visuals) = self.0.cast::<IPreviewHandlerVisuals>() else {
            return;
        };
        let lf = WLOGFONTW {
            lfHeight: font.lfHeight,
            lfWidth: font.lfWidth,
            lfEscapement: font.lfEscapement,
            lfOrientation: font.lfOrientation,
            lfWeight: font.lfWeight,
            lfItalic: font.lfItalic,
            lfUnderline: font.lfUnderline,
            lfStrikeOut: font.lfStrikeOut,
            lfCharSet: FONT_CHARSET(font.lfCharSet),
            lfOutPrecision: FONT_OUTPUT_PRECISION(font.lfOutPrecision),
            lfClipPrecision: FONT_CLIP_PRECISION(font.lfClipPrecision),
            lfQuality: FONT_QUALITY(font.lfQuality),
            lfPitchAndFamily: font.lfPitchAndFamily,
            lfFaceName: font.lfFaceName,
        };
        // SAFETY: values, and a font structure read during the call.
        unsafe {
            let _ = visuals.SetBackgroundColor(COLORREF(bg));
            let _ = visuals.SetTextColor(COLORREF(fg));
            let _ = visuals.SetFont(&lf);
        }
    }

    /// Its site (IObjectWithSite), if it takes one: the frame it passes keys
    /// to, or none.
    pub fn set_site(&self, frame: Option<&IPreviewHandlerFrame>) {
        let Ok(site) = self.0.cast::<IObjectWithSite>() else {
            return;
        };
        // SAFETY: a frame of this thread (kept, counted, if it wants), or
        // none.
        let _ = unsafe {
            match frame {
                Some(frame) => site.SetSite(frame),
                None => site.SetSite(None::<&windows::core::IUnknown>),
            }
        };
    }

    /// The window it draws in, and the size it takes up there (SetWindow).
    pub fn set_window(&self, parent: HWND, size: (i32, i32)) -> windows::core::Result<()> {
        let rc = rect_of(size);
        // SAFETY: a window handle (the system checks it) and a rectangle
        // read during the call.
        unsafe { self.0.SetWindow(WHWND(parent), &rc) }
    }

    /// The size it takes up in its window (SetRect).
    pub fn set_rect(&self, size: (i32, i32)) -> windows::core::Result<()> {
        let rc = rect_of(size);
        // SAFETY: a rectangle read during the call.
        unsafe { self.0.SetRect(&rc) }
    }

    /// Shows its file (DoPreview).
    pub fn do_preview(&self) -> windows::core::Result<()> {
        // SAFETY: no pointers.
        unsafe { self.0.DoPreview() }
    }

    /// Lets its file go (Unload).
    pub fn unload(&self) -> windows::core::Result<()> {
        // SAFETY: no pointers.
        unsafe { self.0.Unload() }
    }

    /// Gives its window the keyboard (SetFocus).
    pub fn set_focus(&self) -> windows::core::Result<()> {
        // SAFETY: no pointers.
        unsafe { self.0.SetFocus() }
    }
}

/// A rectangle at 0, 0 of `size`, at least a pixel each way.
fn rect_of(size: (i32, i32)) -> WRECT {
    WRECT {
        left: 0,
        top: 0,
        right: size.0.max(1),
        bottom: size.1.max(1),
    }
}

/// The frame a preview handler passes back keys it does not use.
/// Not agile: calls from elsewhere come through its thread's apartment,
/// one at a time, as its closures need.
#[implement(IPreviewHandlerFrame, Agile = false)]
struct Frame {
    context: Box<dyn Fn() -> (HACCEL, u32)>,
    translate: Box<dyn Fn(&windows_sys::Win32::UI::WindowsAndMessaging::MSG) -> bool>,
}

/// A preview handler's frame (IPreviewHandlerFrame): GetWindowContext
/// answers the accelerator table and its count `context` gives, and
/// TranslateAccelerator hands `translate` a copy of each message the
/// handler passes on (true: it was one of the window's keys, S_OK; false:
/// S_FALSE, the handler's to use).
pub fn preview_frame(
    context: impl Fn() -> (HACCEL, u32) + 'static,
    translate: impl Fn(&windows_sys::Win32::UI::WindowsAndMessaging::MSG) -> bool + 'static,
) -> IPreviewHandlerFrame {
    Frame {
        context: Box::new(context),
        translate: Box::new(translate),
    }
    .into()
}

impl IPreviewHandlerFrame_Impl for Frame_Impl {
    fn GetWindowContext(&self) -> windows::core::Result<PREVIEWHANDLERFRAMEINFO> {
        let (table, count) = (self.context)();
        Ok(PREVIEWHANDLERFRAMEINFO {
            haccel: WHACCEL(table),
            cAccelEntries: count,
        })
    }

    fn TranslateAccelerator(&self, pmsg: *const WMSG) -> windows::core::Result<()> {
        // SAFETY: the handler passes a message of its own, alive across the
        // call, or null; it is copied.
        let msg = unsafe { pmsg.as_ref() }.ok_or(windows::core::Error::from(E_POINTER))?;
        let msg = windows_sys::Win32::UI::WindowsAndMessaging::MSG {
            hwnd: msg.hwnd.0,
            message: msg.message,
            wParam: msg.wParam.0,
            lParam: msg.lParam.0,
            time: msg.time,
            pt: POINT {
                x: msg.pt.x,
                y: msg.pt.y,
            },
        };
        if (self.translate)(&msg) {
            Ok(())
        } else {
            Err(S_FALSE.into())
        }
    }
}

// A read-only stream over a file, for preview handlers that take one.

/// A read-only stream over an open file, closed when the last reference
/// goes.
#[implement(IStream)]
struct FileStream {
    file: usize,
    /// The file's path, NUL-terminated, for `Stat`.
    name: Vec<u16>,
}

/// A read-only stream (IStream) over the file at `path`, opened for reading
/// with FILE_SHARE_* `share` and FILE_FLAG_* `flags`; `None` if it cannot
/// be opened. Its `Stat` names it by `path`.
pub fn file_stream(
    path: &[u16],
    share: FILE_SHARE_MODE,
    flags: FILE_FLAGS_AND_ATTRIBUTES,
) -> Option<IStream> {
    let name = wz_units(path);
    // Never overlapped: `Read` reads synchronously into the caller's
    // buffer, which an overlapped read could still be filling after the
    // call returns.
    let flags = flags & !FILE_FLAG_OVERLAPPED;
    // SAFETY: a NUL-terminated path; no security attributes or template;
    // the handle is owned by the stream and closed as it drops.
    let file = unsafe {
        CreateFileW(
            name.as_ptr(),
            SYS_GENERIC_READ,
            share,
            null(),
            OPEN_EXISTING,
            flags,
            null_mut(),
        )
    };
    if file == INVALID_HANDLE_VALUE || file.is_null() {
        return None;
    }
    Some(
        FileStream {
            file: file as usize,
            name,
        }
        .into(),
    )
}

impl FileStream {
    fn handle(&self) -> HANDLE {
        self.file as HANDLE
    }
}

impl Drop for FileStream {
    fn drop(&mut self) {
        // SAFETY: the handle `file_stream` opened, closed once.
        unsafe { CloseHandle(self.handle()) };
    }
}

/// The last error as an HRESULT.
fn last_error() -> HRESULT {
    HRESULT::from_win32(crate::win::sys::last_error())
}

fn filetime(ft: windows_sys::Win32::Foundation::FILETIME) -> WFILETIME {
    WFILETIME {
        dwLowDateTime: ft.dwLowDateTime,
        dwHighDateTime: ft.dwHighDateTime,
    }
}

impl ISequentialStream_Impl for FileStream_Impl {
    fn Read(&self, pv: *mut core::ffi::c_void, cb: u32, pcbread: *mut u32) -> HRESULT {
        let mut got = 0u32;
        // SAFETY: the caller's buffer of `cb` bytes, as IStream::Read has it
        // (COM passes the caller's, or its own copy across processes);
        // `pcbread` is the caller's or null.
        unsafe {
            if ReadFile(self.handle(), pv.cast(), cb, &mut got, null_mut()) == 0 {
                return last_error();
            }
            if !pcbread.is_null() {
                *pcbread = got;
            }
        }
        // Fewer bytes than asked for: the end of the file.
        if got == cb { S_OK } else { S_FALSE }
    }

    fn Write(&self, _pv: *const core::ffi::c_void, _cb: u32, _pcbwritten: *mut u32) -> HRESULT {
        STG_E_ACCESSDENIED
    }
}

impl IStream_Impl for FileStream_Impl {
    fn Seek(
        &self,
        dlibmove: i64,
        dworigin: STREAM_SEEK,
        plibnewposition: *mut u64,
    ) -> windows::core::Result<()> {
        let mut at = 0i64;
        // SAFETY: STREAM_SEEK_SET, _CUR and _END are FILE_BEGIN, _CURRENT
        // and _END; the new position goes to the caller's pointer, if any.
        unsafe {
            if SetFilePointerEx(self.handle(), dlibmove, &mut at, dworigin.0) == 0 {
                return Err(last_error().into());
            }
            if !plibnewposition.is_null() {
                *plibnewposition = at as u64;
            }
        }
        Ok(())
    }

    fn SetSize(&self, _libnewsize: u64) -> windows::core::Result<()> {
        Err(STG_E_ACCESSDENIED.into())
    }

    fn CopyTo(
        &self,
        _pstm: Ref<IStream>,
        _cb: u64,
        _pcbread: *mut u64,
        _pcbwritten: *mut u64,
    ) -> windows::core::Result<()> {
        Err(E_NOTIMPL.into())
    }

    fn Commit(&self, _grfcommitflags: &STGC) -> windows::core::Result<()> {
        // Nothing is ever written.
        Ok(())
    }

    fn Revert(&self) -> windows::core::Result<()> {
        Ok(())
    }

    fn LockRegion(
        &self,
        _liboffset: u64,
        _cb: u64,
        _dwlocktype: &LOCKTYPE,
    ) -> windows::core::Result<()> {
        Err(STG_E_INVALIDFUNCTION.into())
    }

    fn UnlockRegion(
        &self,
        _liboffset: u64,
        _cb: u64,
        _dwlocktype: u32,
    ) -> windows::core::Result<()> {
        Err(STG_E_INVALIDFUNCTION.into())
    }

    fn Stat(&self, pstatstg: *mut STATSTG, grfstatflag: &STATFLAG) -> windows::core::Result<()> {
        if pstatstg.is_null() {
            return Err(STG_E_INVALIDPOINTER.into());
        }
        let mut info = BY_HANDLE_FILE_INFORMATION::default();
        // SAFETY: the file's information into a plain out-struct.
        if unsafe { GetFileInformationByHandle(self.handle(), &mut info) } == 0 {
            return Err(last_error().into());
        }
        let mut name = PWSTR::null();
        if *grfstatflag != STATFLAG_NONAME {
            let bytes = self.name.len() * 2;
            // SAFETY: memory the caller frees (CoTaskMemFree), filled with
            // the NUL-terminated name, `name.len()` units, as allocated.
            unsafe {
                let mem = CoTaskMemAlloc(bytes).cast::<u16>();
                if !mem.is_null() {
                    core::ptr::copy_nonoverlapping(self.name.as_ptr(), mem, self.name.len());
                    name = PWSTR(mem);
                }
            }
        }
        // SAFETY: the caller's STATSTG, not null (checked above), written
        // whole.
        unsafe {
            pstatstg.write(STATSTG {
                pwcsName: name,
                r#type: STGTY_STREAM.0 as u32,
                cbSize: (u64::from(info.nFileSizeHigh) << 32) | u64::from(info.nFileSizeLow),
                mtime: filetime(info.ftLastWriteTime),
                ctime: filetime(info.ftCreationTime),
                atime: filetime(info.ftLastAccessTime),
                grfMode: STGM_READ,
                grfLocksSupported: 0,
                clsid: GUID::zeroed(),
                grfStateBits: 0,
                reserved: 0,
            });
        }
        Ok(())
    }

    fn Clone(&self) -> windows::core::Result<IStream> {
        Err(E_NOTIMPL.into())
    }
}

/// Reads from `stream` into `buf` (ISequentialStream::Read): its answer,
/// and how many bytes came.
pub fn stream_read(stream: &IStream, buf: &mut [u8]) -> (HRESULT, u32) {
    let cb = u32::try_from(buf.len()).unwrap_or(u32::MAX);
    let mut got = 0u32;
    // SAFETY: a buffer of at least `cb` bytes and a count out, both alive
    // during the call.
    let hr = unsafe { stream.Read(buf.as_mut_ptr().cast(), cb, Some(&raw mut got)) };
    (hr, got.min(cb))
}

/// Writes `bytes` to `stream` (ISequentialStream::Write): its answer.
pub fn stream_write(stream: &IStream, bytes: &[u8]) -> HRESULT {
    let cb = u32::try_from(bytes.len()).unwrap_or(u32::MAX);
    // SAFETY: `cb` bytes read during the call; no count asked for.
    unsafe { stream.Write(bytes.as_ptr().cast(), cb, None) }
}

/// Moves `stream`'s position (Seek, STREAM_SEEK_*); the new one.
pub fn stream_seek(stream: &IStream, by: i64, from: STREAM_SEEK) -> windows::core::Result<u64> {
    let mut at = 0u64;
    // SAFETY: an out-value alive during the call.
    unsafe { stream.Seek(by, from, Some(&raw mut at)) }?;
    Ok(at)
}

/// `stream`'s size, and its name if `named` (Stat; the name it allocates
/// is copied and freed here).
pub fn stream_stat(stream: &IStream, named: bool) -> windows::core::Result<(u64, Option<String>)> {
    let mut st = STATSTG::default();
    let flag = if named { STATFLAG(0) } else { STATFLAG_NONAME };
    // SAFETY: an out-struct alive during the call.
    unsafe { stream.Stat(&mut st, flag) }?;
    let name = if st.pwcsName.is_null() {
        None
    } else {
        // SAFETY: the NUL-terminated name the stream allocated for its
        // caller, copied, then freed once.
        let units = unsafe { read_nul(st.pwcsName.0) };
        // SAFETY: as above.
        unsafe { CoTaskMemFree(Some(st.pwcsName.0.cast())) };
        units.map(|u| String::from_utf16_lossy(&u))
    };
    Ok((st.cbSize, name))
}

// WIC. COM must be initialised on the thread.

/// WIC's factory (in process).
pub fn wic_factory() -> Option<IWICImagingFactory> {
    // SAFETY: a class id and no outer object; the object comes back owned.
    unsafe { CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER) }.ok()
}

/// The file extensions each of WIC's decoders takes, as each lists them
/// (`.jpg,.jpeg,…`).
pub fn wic_decoder_extensions(factory: &IWICImagingFactory) -> Vec<Vec<u16>> {
    let mut out = Vec::new();
    // SAFETY: a call on a live factory of this thread.
    let list = unsafe {
        factory
            .CreateComponentEnumerator(WICDecoder.0 as u32, WICComponentEnumerateDefault.0 as u32)
    };
    let Ok(list) = list else {
        return out;
    };
    loop {
        let mut item = [None];
        let mut fetched = 0u32;
        // SAFETY: one object into a local array of one, and the count.
        if unsafe { list.Next(&mut item, Some(&mut fetched)) }.is_err() || fetched == 0 {
            break;
        }
        let Some(info) = item[0]
            .take()
            .and_then(|u| u.cast::<IWICBitmapCodecInfo>().ok())
        else {
            continue;
        };
        let mut buf = vec![0u16; 1024];
        let mut len = 0u32;
        // SAFETY: the buffer (its length passed with it) and a count out.
        if unsafe { info.GetFileExtensions(&mut buf, &mut len) }.is_err() {
            if len as usize <= buf.len() {
                continue;
            }
            buf = vec![0u16; len as usize];
            // SAFETY: as above, with a buffer of the length asked for.
            if unsafe { info.GetFileExtensions(&mut buf, &mut len) }.is_err() {
                continue;
            }
        }
        let text = &buf[..(len as usize).min(buf.len())];
        out.push(until_nul(text).to_vec());
    }
    out
}

/// A decoder for the file at `path`, reading it (CreateDecoderFromFilename,
/// metadata read when asked for).
pub fn wic_decoder(
    factory: &IWICImagingFactory,
    path: &[u16],
) -> windows::core::Result<IWICBitmapDecoder> {
    let z = wz_units(path);
    // SAFETY: a NUL-terminated path read during the call; no vendor.
    unsafe {
        factory.CreateDecoderFromFilename(
            PCWSTR(z.as_ptr()),
            None,
            GENERIC_READ,
            WICDecodeMetadataCacheOnDemand,
        )
    }
}

/// The decoder's container format (GUID_ContainerFormat*).
pub fn wic_container(decoder: &IWICBitmapDecoder) -> windows::core::Result<GUID> {
    // SAFETY: a call on a live decoder of this thread.
    unsafe { decoder.GetContainerFormat() }
}

pub fn wic_frame_count(decoder: &IWICBitmapDecoder) -> windows::core::Result<u32> {
    // SAFETY: a call on a live decoder of this thread.
    unsafe { decoder.GetFrameCount() }
}

/// The decoder's frame `i`.
pub fn wic_frame(
    decoder: &IWICBitmapDecoder,
    i: u32,
) -> windows::core::Result<IWICBitmapFrameDecode> {
    // SAFETY: a call on a live decoder of this thread.
    unsafe { decoder.GetFrame(i) }
}

/// A bitmap source's size: (width, height).
pub fn wic_size(source: &IWICBitmapSource) -> windows::core::Result<(u32, u32)> {
    let (mut w, mut h) = (0u32, 0u32);
    // SAFETY: two out-values alive during the call.
    unsafe { source.GetSize(&mut w, &mut h) }?;
    Ok((w, h))
}

/// The frame's metadata at `query` (`/app1/ifd/{ushort=274}`) as a u16;
/// `None` when it has none there, or one that is no number.
pub fn wic_metadata_u16(frame: &IWICBitmapFrameDecode, query: &str) -> Option<u16> {
    let query = wz(query);
    // SAFETY: calls on a live frame of this thread; the PROPVARIANT is
    // cleared after it is read.
    unsafe {
        let reader = frame.GetMetadataQueryReader().ok()?;
        let mut value = PROPVARIANT::default();
        reader
            .GetMetadataByName(PCWSTR(query.as_ptr()), &mut value)
            .ok()?;
        let n = PropVariantToUInt16(&value);
        let _ = PropVariantClear(&mut value);
        n.ok()
    }
}

/// `source` scaled to `w` by `h` (a bitmap scaler, Fant).
pub fn wic_scaler(
    factory: &IWICImagingFactory,
    source: &IWICBitmapSource,
    w: u32,
    h: u32,
) -> windows::core::Result<IWICBitmapScaler> {
    // SAFETY: calls on live objects of this thread.
    unsafe {
        let scaler = factory.CreateBitmapScaler()?;
        scaler.Initialize(source, w, h, WICBitmapInterpolationModeFant)?;
        Ok(scaler)
    }
}

/// `source` as 32-bit premultiplied BGRA (a format converter, no dither,
/// no palette).
pub fn wic_pbgra(
    factory: &IWICImagingFactory,
    source: &IWICBitmapSource,
) -> windows::core::Result<IWICFormatConverter> {
    // SAFETY: calls on live objects of this thread; the format's GUID is
    // read during the call.
    unsafe {
        let converter = factory.CreateFormatConverter()?;
        converter.Initialize(
            source,
            &GUID_WICPixelFormat32bppPBGRA,
            WICBitmapDitherTypeNone,
            None::<&IWICPalette>,
            0.0,
            WICBitmapPaletteTypeCustom,
        )?;
        Ok(converter)
    }
}

/// Copies all of `source`'s 32-bit pixels, rows of `width`, into `out`
/// (CopyPixels); false if it failed (the size or format did not fit).
pub fn wic_copy_pixels(source: &IWICBitmapSource, width: u32, out: &mut [[u8; 4]]) -> bool {
    let Some(stride) = width.checked_mul(4) else {
        return false;
    };
    // SAFETY: the whole rectangle, into a buffer whose length is passed
    // with it (WIC writes no more).
    unsafe { source.CopyPixels(null(), stride, out.as_flattened_mut()) }.is_ok()
}

// Accessibility (MSAA) for what a window paints itself.

const OBJID_CLIENT: i32 = -4;
const NAVDIR_NEXT: i32 = 5;
const NAVDIR_PREVIOUS: i32 = 6;
/// STATE_SYSTEM_UNAVAILABLE.
const STATE_UNAVAILABLE: u32 = 0x1;

/// One painted part of a window, as accessibility tools see it: a simple
/// element (no object of its own).
#[derive(Clone)]
pub struct Element {
    pub name: String,
    /// Its MSAA role (ROLE_SYSTEM_*).
    pub role: u32,
    /// Its MSAA state (STATE_SYSTEM_*).
    pub state: u32,
    /// In the window's client coordinates.
    pub rect: RECT,
    pub value: Option<String>,
    pub description: Option<String>,
    /// Its default action's name ("Press"), if it has one.
    pub action: Option<&'static str>,
}

/// What a window's accessible object asks of the window, each time it is
/// asked (it keeps nothing of the window between calls).
pub trait Elements {
    /// The painted parts now, in order.
    fn elements(&self) -> Vec<Element>;
    /// The window's own name and role, instead of the standard object's.
    fn own(&self) -> Option<(String, u32)>;
    /// Does the default action of part `index` (one that has an action
    /// and is not unavailable).
    fn act(&self, index: usize);
}

/// A VARIANT holding `v` (VT_I4).
pub fn variant_i4(v: i32) -> VARIANT {
    let mut out = VARIANT::default();
    // SAFETY: writes the VT_I4 member of a fresh VARIANT and the tag that
    // says it is the one there.
    unsafe {
        let inner = &mut *out.Anonymous.Anonymous;
        inner.vt = VT_I4;
        inner.Anonymous.lVal = v;
    }
    out
}

/// The child id a VARIANT holds (VT_I4; VT_EMPTY is 0, the object itself).
pub fn child_id(v: &VARIANT) -> Option<i32> {
    // SAFETY: reads the tag, then the member it says is there.
    unsafe {
        let inner = &v.Anonymous.Anonymous;
        if inner.vt == VT_I4 {
            Some(inner.Anonymous.lVal)
        } else if inner.vt == VT_EMPTY {
            Some(0)
        } else {
            None
        }
    }
}

/// Which child a VARIANT names.
enum Child {
    Me,
    Standard,
    Part(usize),
}

fn bstr(s: &str) -> BSTR {
    BSTR::from(s)
}

/// Windows' "not supported here" for a part without the property.
fn none<T>() -> windows::core::Result<T> {
    Err(windows::core::Error::from_hresult(S_FALSE))
}

/// A window's object: the system's standard one for the window, its
/// children after it, with the painted parts added as simple elements past
/// the standard children. Not agile: calls from other processes come
/// through the window's thread's apartment, where its closures live.
#[implement(IAccessible, Agile = false)]
struct Accessible {
    hwnd: HWND,
    standard: IAccessible,
    elements: Box<dyn Elements>,
}

impl Accessible {
    fn parts(&self) -> Vec<Element> {
        self.elements.elements()
    }

    fn standard_count(&self) -> i32 {
        // SAFETY: a call on the standard object of this thread's window.
        unsafe { self.standard.accChildCount() }.unwrap_or(0)
    }

    fn which(&self, v: &VARIANT) -> windows::core::Result<Child> {
        let id = child_id(v).ok_or_else(|| windows::core::Error::from_hresult(E_INVALIDARG))?;
        if id == 0 {
            return Ok(Child::Me);
        }
        let n = self.standard_count();
        if id <= n {
            return Ok(Child::Standard);
        }
        let i = (id - n - 1) as usize;
        if i < self.parts().len() {
            Ok(Child::Part(i))
        } else {
            Err(windows::core::Error::from_hresult(E_INVALIDARG))
        }
    }

    fn part(&self, i: usize) -> windows::core::Result<Element> {
        self.parts()
            .into_iter()
            .nth(i)
            .ok_or_else(|| windows::core::Error::from_hresult(E_INVALIDARG))
    }

    fn screen(&self, rc: RECT) -> RECT {
        let tl = crate::win::client_to_screen(
            self.hwnd,
            POINT {
                x: rc.left,
                y: rc.top,
            },
        );
        RECT {
            left: tl.x,
            top: tl.y,
            right: tl.x + (rc.right - rc.left),
            bottom: tl.y + (rc.bottom - rc.top),
        }
    }
}

impl IDispatch_Impl for Accessible_Impl {
    fn GetTypeInfoCount(&self) -> windows::core::Result<u32> {
        Ok(0)
    }

    fn GetTypeInfo(&self, _itinfo: u32, _lcid: u32) -> windows::core::Result<ITypeInfo> {
        Err(E_NOTIMPL.into())
    }

    fn GetIDsOfNames(
        &self,
        _riid: *const GUID,
        _names: *const PCWSTR,
        _count: u32,
        _lcid: u32,
        _ids: *mut i32,
    ) -> windows::core::Result<()> {
        Err(E_NOTIMPL.into())
    }

    fn Invoke(
        &self,
        _id: i32,
        _riid: *const GUID,
        _lcid: u32,
        _flags: DISPATCH_FLAGS,
        _params: *const DISPPARAMS,
        _result: *mut VARIANT,
        _excep: *mut EXCEPINFO,
        _arg: *mut u32,
    ) -> windows::core::Result<()> {
        Err(E_NOTIMPL.into())
    }
}

/// A call on the standard object of this thread's window, with the
/// VARIANTs and out-pointers the caller gave, passed on as they came.
macro_rules! standard {
    ($self:ident . $call:ident ( $($arg:expr),* )) => {{
        // SAFETY: a call on the standard object of this thread's window,
        // with what its caller gave, passed on as it came.
        unsafe { $self.standard.$call($($arg),*) }
    }};
}

impl IAccessible_Impl for Accessible_Impl {
    fn accParent(&self) -> windows::core::Result<IDispatch> {
        standard!(self.accParent())
    }

    fn accChildCount(&self) -> windows::core::Result<i32> {
        Ok(self.standard_count() + self.parts().len() as i32)
    }

    fn get_accChild(&self, v: &VARIANT) -> windows::core::Result<IDispatch> {
        match self.which(v)? {
            Child::Standard => standard!(self.get_accChild(v)),
            // A simple element: no object of its own.
            Child::Part(_) | Child::Me => none(),
        }
    }

    fn get_accName(&self, v: &VARIANT) -> windows::core::Result<BSTR> {
        match self.which(v)? {
            Child::Part(i) => Ok(bstr(&self.part(i)?.name)),
            Child::Me => match self.elements.own() {
                Some((name, _)) => Ok(bstr(&name)),
                None => standard!(self.get_accName(v)),
            },
            Child::Standard => standard!(self.get_accName(v)),
        }
    }

    fn get_accValue(&self, v: &VARIANT) -> windows::core::Result<BSTR> {
        match self.which(v)? {
            Child::Part(i) => match self.part(i)?.value {
                Some(value) => Ok(bstr(&value)),
                None => none(),
            },
            _ => standard!(self.get_accValue(v)),
        }
    }

    fn get_accDescription(&self, v: &VARIANT) -> windows::core::Result<BSTR> {
        match self.which(v)? {
            Child::Part(i) => match self.part(i)?.description {
                Some(d) => Ok(bstr(&d)),
                None => none(),
            },
            _ => standard!(self.get_accDescription(v)),
        }
    }

    fn get_accRole(&self, v: &VARIANT) -> windows::core::Result<VARIANT> {
        match self.which(v)? {
            Child::Part(i) => Ok(variant_i4(self.part(i)?.role as i32)),
            Child::Me => match self.elements.own() {
                Some((_, role)) => Ok(variant_i4(role as i32)),
                None => standard!(self.get_accRole(v)),
            },
            Child::Standard => standard!(self.get_accRole(v)),
        }
    }

    fn get_accState(&self, v: &VARIANT) -> windows::core::Result<VARIANT> {
        match self.which(v)? {
            Child::Part(i) => Ok(variant_i4(self.part(i)?.state as i32)),
            _ => standard!(self.get_accState(v)),
        }
    }

    fn get_accHelp(&self, v: &VARIANT) -> windows::core::Result<BSTR> {
        match self.which(v)? {
            Child::Part(_) => none(),
            _ => standard!(self.get_accHelp(v)),
        }
    }

    fn get_accHelpTopic(&self, file: *mut BSTR, v: &VARIANT) -> windows::core::Result<i32> {
        match self.which(v)? {
            Child::Part(_) => none(),
            _ => standard!(self.get_accHelpTopic(file, v)),
        }
    }

    fn get_accKeyboardShortcut(&self, v: &VARIANT) -> windows::core::Result<BSTR> {
        match self.which(v)? {
            Child::Part(_) => none(),
            _ => standard!(self.get_accKeyboardShortcut(v)),
        }
    }

    fn accFocus(&self) -> windows::core::Result<VARIANT> {
        standard!(self.accFocus())
    }

    fn accSelection(&self) -> windows::core::Result<VARIANT> {
        standard!(self.accSelection())
    }

    fn get_accDefaultAction(&self, v: &VARIANT) -> windows::core::Result<BSTR> {
        match self.which(v)? {
            Child::Part(i) => match self.part(i)?.action {
                Some(name) => Ok(bstr(name)),
                None => none(),
            },
            _ => standard!(self.get_accDefaultAction(v)),
        }
    }

    fn accSelect(&self, flags: i32, v: &VARIANT) -> windows::core::Result<()> {
        match self.which(v)? {
            Child::Part(_) => none(),
            _ => standard!(self.accSelect(flags, v)),
        }
    }

    fn accLocation(
        &self,
        x: *mut i32,
        y: *mut i32,
        cx: *mut i32,
        cy: *mut i32,
        v: &VARIANT,
    ) -> windows::core::Result<()> {
        match self.which(v)? {
            Child::Part(i) => {
                let rc = self.screen(self.part(i)?.rect);
                if x.is_null() || y.is_null() || cx.is_null() || cy.is_null() {
                    return Err(E_INVALIDARG.into());
                }
                // SAFETY: the caller's out-parameters, checked above.
                unsafe {
                    *x = rc.left;
                    *y = rc.top;
                    *cx = rc.right - rc.left;
                    *cy = rc.bottom - rc.top;
                }
                Ok(())
            }
            _ => standard!(self.accLocation(x, y, cx, cy, v)),
        }
    }

    fn accNavigate(&self, dir: i32, start: &VARIANT) -> windows::core::Result<VARIANT> {
        match self.which(start)? {
            Child::Part(i) => {
                let n = self.standard_count();
                let count = self.parts().len();
                let to = match dir {
                    NAVDIR_NEXT if i + 1 < count => i + 1,
                    NAVDIR_PREVIOUS if i > 0 => i - 1,
                    _ => return none(),
                };
                Ok(variant_i4(n + 1 + to as i32))
            }
            _ => standard!(self.accNavigate(dir, start)),
        }
    }

    fn accHitTest(&self, x: i32, y: i32) -> windows::core::Result<VARIANT> {
        let n = self.standard_count();
        for (i, p) in self.parts().iter().enumerate() {
            let rc = self.screen(p.rect);
            if x >= rc.left && x < rc.right && y >= rc.top && y < rc.bottom {
                return Ok(variant_i4(n + 1 + i as i32));
            }
        }
        standard!(self.accHitTest(x, y))
    }

    fn accDoDefaultAction(&self, v: &VARIANT) -> windows::core::Result<()> {
        match self.which(v)? {
            Child::Part(i) => {
                if self.part(i)?.action.is_none() {
                    return none();
                }
                if self.part(i)?.state & STATE_UNAVAILABLE != 0 {
                    return Err(E_FAIL.into());
                }
                self.elements.act(i);
                Ok(())
            }
            _ => standard!(self.accDoDefaultAction(v)),
        }
    }

    fn put_accName(&self, _v: &VARIANT, _name: &BSTR) -> windows::core::Result<()> {
        Err(E_NOTIMPL.into())
    }

    fn put_accValue(&self, _v: &VARIANT, _value: &BSTR) -> windows::core::Result<()> {
        Err(E_NOTIMPL.into())
    }
}

/// A window's accessible object for its client area: the standard one,
/// with the parts `elements` reports added.
pub struct AccessibleObject {
    object: IAccessible,
}

impl AccessibleObject {
    /// The object for window `hwnd` (a window of this thread, whose
    /// standard client object it wraps); `None` if the system would not
    /// make the standard one.
    pub fn new(hwnd: HWND, elements: impl Elements + 'static) -> Option<AccessibleObject> {
        let mut raw = null_mut();
        // SAFETY: asks for the standard client object of the window, as
        // IAccessible, into an out-pointer.
        unsafe {
            CreateStdAccessibleObject(WHWND(hwnd), OBJID_CLIENT, &IAccessible::IID, &mut raw)
        }
        .ok()?;
        if raw.is_null() {
            return None;
        }
        // SAFETY: an IAccessible (the IID asked for), not null, whose
        // reference the call handed over.
        let standard = unsafe { IAccessible::from_raw(raw) };
        let object: IAccessible = Accessible {
            hwnd,
            standard,
            elements: Box::new(elements),
        }
        .into();
        Some(AccessibleObject { object })
    }

    /// WM_GETOBJECT's answer for the client area, given its wParam
    /// (LresultFromObject: a reference to the object for the asker).
    pub fn answer_w(&self, w: usize) -> isize {
        // SAFETY: the interface's id and the object, read during the call;
        // wParam is WM_GETOBJECT's flags, an integer.
        let r: WLRESULT = unsafe { LresultFromObject(&IAccessible::IID, WWPARAM(w), &self.object) };
        r.0
    }

    /// The answer to `m` if it is WM_GETOBJECT for the client area.
    pub fn answer(&self, m: &Msg) -> Option<isize> {
        (m.id == WM_GETOBJECT && wants_client(m.l)).then(|| self.answer_w(m.w))
    }
}

/// Whether WM_GETOBJECT's `l` asks for the client area (OBJID_CLIENT).
pub fn wants_client(l: isize) -> bool {
    l as i32 == OBJID_CLIENT
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn child_ids_read_from_variants() {
        assert_eq!(child_id(&variant_i4(3)), Some(3));
        assert_eq!(child_id(&VARIANT::default()), Some(0));
        assert!(wants_client(-4) && !wants_client(-3));
    }

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

    #[test]
    fn pixels_go_into_a_dib_section_and_come_back() {
        let bmp = create_dib32(3, 2);
        assert!(!bmp.is_null());
        let owned = gdi::Owned::new(bmp);
        let px: Vec<[u8; 4]> = (0..6).map(|i| [i, 2 * i, 3 * i, 255]).collect();
        assert!(!dib_write(bmp, &px[..5]), "the wrong count is refused");
        assert_eq!(dib_pixels(bmp).map(|p| p.2), Some(vec![[0; 4]; 6]));
        assert!(dib_write(bmp, &px));
        assert_eq!(dib_pixels(bmp), Some((3, 2, px.clone())));
        // Read through GDI as from any bitmap.
        assert_eq!(bitmap_pixels(bmp, 3, 2), Some(px));
        // Not a DIB section: neither read nor written.
        let ddb = gdi::Owned::new(gdi::create_compatible_bitmap(
            gdi::WindowDc::get(null_mut()).hdc(),
            3,
            2,
        ));
        assert!(!dib_write(ddb.get(), &[[0; 4]; 6]));
        assert!(dib_pixels(ddb.get()).is_none());
        assert!(create_dib32(0, 5).is_null());
        drop(owned);
    }
}
