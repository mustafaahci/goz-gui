//! Icons: owned ones (destroyed when they drop), the program's own and the
//! system's, the shell's icons of files and its stock icons, and the system
//! image lists' entries.

use windows_sys::Win32::Graphics::Gdi::DeleteObject;
use windows_sys::Win32::UI::Controls::{HIMAGELIST, LoadIconWithScaleDown};
use windows_sys::Win32::UI::Shell::{
    SHDefExtractIconW, SHGFI_FLAGS, SHGFI_ICON, SHGFI_SYSICONINDEX, SHGSI_ICONLOCATION,
    SHGSI_SYSICONINDEX, SHGetStockIconInfo, SHSTOCKICONID, SHSTOCKICONINFO,
    Shell_GetCachedImageIndexW,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    DestroyIcon, GetIconInfo, HICON, ICONINFO, IMAGE_ICON, LR_DEFAULTCOLOR, LoadIconW, LoadImageW,
};

use super::{nul_ended, until_nul};

/// A resource id (MAKEINTRESOURCE) as the name pointer a call takes: the
/// id travels as the pointer's address, below 0x10000, which the call reads
/// as an id, never as a string. `None` for a number that is no id.
fn int_resource(id: usize) -> Option<*const u16> {
    (id < 0x10000).then(|| core::ptr::without_provenance(id))
}

/// An icon goz-gui made (extracted, loaded), destroyed when this drops.
pub struct Icon(HICON);

impl Icon {
    /// Takes `icon`, one made for the caller (not null), to destroy when
    /// this drops.
    pub(super) fn own(icon: HICON) -> Option<Icon> {
        (!icon.is_null()).then_some(Icon(icon))
    }

    /// One of the system's icons (IDI_ERROR, IDI_WARNING... as a number)
    /// at `cx` × `cy`, scaled down from a larger one where it has one
    /// (LoadIconWithScaleDown); `None` if it could not be loaded.
    pub fn system(id: usize, cx: i32, cy: i32) -> Option<Icon> {
        let name = int_resource(id)?;
        let mut icon = core::ptr::null_mut();
        // SAFETY: an id, not a string (`int_resource`), of the system's
        // icons; an out-handle, the caller's to destroy.
        let hr = unsafe { LoadIconWithScaleDown(core::ptr::null_mut(), name, cx, cy, &mut icon) };
        if hr < 0 {
            return None;
        }
        Icon::own(icon)
    }

    /// The icon's handle, to draw with (it stays this icon's).
    pub fn handle(&self) -> HICON {
        self.0
    }

    /// The width of its image, in pixels.
    pub fn width(&self) -> Option<i32> {
        let mut info = ICONINFO::default();
        // SAFETY: an out-struct.
        if unsafe { GetIconInfo(self.0, &mut info) } == 0 {
            return None;
        }
        let shown = if info.hbmColor.is_null() {
            info.hbmMask
        } else {
            info.hbmColor
        };
        let bm = crate::win::gdi::bitmap_info(shown);
        for b in [info.hbmColor, info.hbmMask] {
            if !b.is_null() {
                // SAFETY: no pointers; the bitmaps GetIconInfo made for
                // the caller, deleted once.
                unsafe { DeleteObject(b) };
            }
        }
        bm.filter(|bm| bm.bmWidth > 0).map(|bm| bm.bmWidth)
    }
}

impl Drop for Icon {
    fn drop(&mut self) {
        // SAFETY: no pointers; the icon this owns, destroyed once.
        unsafe { DestroyIcon(self.0) };
    }
}

/// Icon `index` of `file` (a program, library or icon file; minus a
/// resource id), `size` pixels square, as the shell extracts it
/// (SHDefExtractIconW); `None` if it could not.
pub fn def_extract(file: &[u16], index: i32, size: u32) -> Option<Icon> {
    let file = nul_ended(file);
    let mut icon: HICON = core::ptr::null_mut();
    // SAFETY: a NUL-terminated path; an out-handle for the large icon,
    // none for the small one.
    unsafe {
        SHDefExtractIconW(
            file.as_ptr(),
            index,
            0,
            &mut icon,
            core::ptr::null_mut(),
            size,
        )
    };
    Icon::own(icon)
}

/// The system image lists' entry for icon `index` of `file`, as
/// Explorer asks for it (Shell_GetCachedImageIndexW); -1 for none.
pub fn cached_image_index(file: &[u16], index: i32) -> i32 {
    let file = nul_ended(file);
    // SAFETY: a NUL-terminated path.
    unsafe { Shell_GetCachedImageIndexW(file.as_ptr(), index, 0) }
}

/// The system image list (null if the shell could not say) and the
/// index in it of the icon of `path` (SHGetFileInfoW with
/// SHGFI_SYSICONINDEX and `flags`, such as SHGFI_SMALLICON, or
/// SHGFI_USEFILEATTRIBUTES with `attributes`). SHGFI_ICON is ignored: no
/// icon is made.
pub fn sys_icon(path: &[u16], attributes: u32, flags: SHGFI_FLAGS) -> (HIMAGELIST, i32) {
    let flags = (flags & !SHGFI_ICON) | SHGFI_SYSICONINDEX;
    let (list, info) = super::shell::file_info(path, attributes, flags);
    (list as HIMAGELIST, info.iIcon)
}

/// Icon resource `id` of this program (one build.rs embeds) at `size`
/// pixels; null if there is none. The caller owns it (`gdi::destroy_icon`).
pub fn from_module(id: u16, size: i32) -> HICON {
    let name = core::ptr::without_provenance(usize::from(id));
    // SAFETY: MAKEINTRESOURCE(id): an id below 0x10000 as the name, which
    // LoadImageW reads as an id, never as a string.
    unsafe {
        LoadImageW(
            super::class::module(),
            name,
            IMAGE_ICON,
            size,
            size,
            LR_DEFAULTCOLOR,
        )
    }
}

/// One of the system's icons (IDI_APPLICATION…, as a number), shared:
/// never to be destroyed. Null for any other number.
pub fn shared(id: usize) -> HICON {
    let Some(name) = int_resource(id) else {
        return core::ptr::null_mut();
    };
    // SAFETY: an id, not a string (`int_resource`), of the system's icons.
    unsafe { LoadIconW(core::ptr::null_mut(), name) }
}

/// Stock icon `id`'s index in the system image lists.
pub fn stock_index(id: SHSTOCKICONID) -> Option<i32> {
    let mut info = SHSTOCKICONINFO {
        cbSize: size_of::<SHSTOCKICONINFO>() as u32,
        ..Default::default()
    };
    // SAFETY: an out-struct with its size; no icon is made.
    let hr = unsafe { SHGetStockIconInfo(id, SHGSI_SYSICONINDEX, &mut info) };
    (hr >= 0).then_some(info.iSysImageIndex)
}

/// Where stock icon `id` is: its file and its index there.
pub fn stock_location(id: SHSTOCKICONID) -> Option<(Vec<u16>, i32)> {
    let mut info = SHSTOCKICONINFO {
        cbSize: size_of::<SHSTOCKICONINFO>() as u32,
        ..Default::default()
    };
    // SAFETY: an out-struct with its size; no icon is made.
    let hr = unsafe { SHGetStockIconInfo(id, SHGSI_ICONLOCATION, &mut info) };
    (hr == 0).then(|| (until_nul(&info.szPath).to_vec(), info.iIcon))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_icon_is_asked_for_by_number() {
        use windows_sys::Win32::UI::WindowsAndMessaging::IDI_APPLICATION;
        assert!(Icon::system(0x10000, 32, 32).is_none());
        assert!(shared(0x10000).is_null());
        let app = IDI_APPLICATION as usize;
        assert!(!shared(app).is_null());
        assert!(Icon::system(app, 32, 32).is_some_and(|i| i.width() == Some(32)));
    }
}
