//! Image lists (comctl32 checks the handles it is given): owned ones,
//! the shell's system image lists, and drawing their images.

use windows_sys::Win32::Foundation::COLORREF;
use windows_sys::Win32::Graphics::Gdi::HDC;
use windows_sys::Win32::UI::Controls::{
    HIMAGELIST, IMAGE_LIST_DRAW_STYLE, IMAGELIST_CREATION_FLAGS, IMAGELISTDRAWPARAMS,
    ImageList_Create, ImageList_Destroy, ImageList_Draw, ImageList_DrawIndirect, ImageList_GetIcon,
    ImageList_GetIconSize, ImageList_GetImageCount, ImageList_ReplaceIcon, ImageList_SetBkColor,
    ImageList_SetImageCount,
};
use windows_sys::Win32::UI::Shell::SHGetImageList;
use windows_sys::Win32::UI::WindowsAndMessaging::HICON;
use windows_sys::core::GUID;

/// An image list this made (ImageList_Create), destroyed when it drops
/// unless it is given away ([`ImageList::into_raw`]): a window it is handed
/// to while this keeps it must be given another (or none) before it drops.
pub struct ImageList(HIMAGELIST);

impl ImageList {
    /// A new list of `cx` × `cy` images, ILC_* `flags`, room for `initial`
    /// growing by `grow`; `None` if comctl32 could not make one.
    pub fn create(
        cx: i32,
        cy: i32,
        flags: IMAGELIST_CREATION_FLAGS,
        initial: i32,
        grow: i32,
    ) -> Option<ImageList> {
        // SAFETY: no pointers.
        let il = unsafe { ImageList_Create(cx, cy, flags, initial, grow) };
        (il != 0).then_some(ImageList(il))
    }

    /// The list's handle, for the messages that hand it to a control.
    pub fn handle(&self) -> HIMAGELIST {
        self.0
    }

    /// Gives the list up without destroying it (to a control that destroys
    /// its image lists with itself, say).
    pub fn into_raw(self) -> HIMAGELIST {
        let il = self.0;
        core::mem::forget(self);
        il
    }

    /// ImageList_ReplaceIcon: `icon` copied in as image `i` (-1: added at
    /// the end); its index, -1 if it failed. The list keeps a copy: the
    /// caller still owns `icon`.
    pub fn replace_icon(&self, i: i32, icon: HICON) -> i32 {
        // SAFETY: a list this owns; the icon is a handle the system checks.
        unsafe { ImageList_ReplaceIcon(self.0, i, icon) }
    }

    /// Makes the list `count` images long (new ones blank).
    pub fn set_image_count(&self, count: u32) -> bool {
        // SAFETY: a list this owns.
        unsafe { ImageList_SetImageCount(self.0, count) != 0 }
    }
}

impl Drop for ImageList {
    fn drop(&mut self) {
        destroy(self.0);
    }
}

/// ImageList_Destroy; false if `il` is not an image list.
pub fn destroy(il: HIMAGELIST) -> bool {
    // SAFETY: no pointers.
    unsafe { ImageList_Destroy(il) != 0 }
}

/// `IID_IImageList`, which `SHGetImageList` hands the system lists out as.
const IID_IMAGE_LIST: GUID = GUID::from_u128(0x46eb5926_582e_4017_9fdf_e8998daa0950);

/// The shell's system image list `which` (SHIL_*), which lives as long as
/// the process: the reference handed out is kept, never released. `None`
/// if the shell has no such list.
pub fn system(which: i32) -> Option<HIMAGELIST> {
    let mut ppv: *mut core::ffi::c_void = core::ptr::null_mut();
    // SAFETY: an out-pointer for the interface the IID names; what comes
    // back is never followed here, only handed on as the list's handle
    // (which comctl32 takes an IImageList as).
    let hr = unsafe { SHGetImageList(which, &IID_IMAGE_LIST, &mut ppv) };
    (hr >= 0 && !ppv.is_null()).then_some(ppv as HIMAGELIST)
}

/// A copy of image `i` of `il` as an icon (ImageList_GetIcon), which the
/// caller destroys (`gdi::destroy_icon`); null if there is none.
pub fn icon(il: HIMAGELIST, i: i32, flags: IMAGE_LIST_DRAW_STYLE) -> HICON {
    // SAFETY: no pointers.
    unsafe { ImageList_GetIcon(il, i, flags) }
}

/// Draws as `params` says (its size is set here); false if it did not
/// (with ILD_ASYNC: the image is not loaded yet).
pub fn draw_indirect(params: &IMAGELISTDRAWPARAMS) -> bool {
    let p = IMAGELISTDRAWPARAMS {
        cbSize: size_of::<IMAGELISTDRAWPARAMS>() as u32,
        ..*params
    };
    // SAFETY: an in-struct with its size, of numbers and handles.
    unsafe { ImageList_DrawIndirect(&p) != 0 }
}

/// Draws image `i` of `il` on `hdc` at `x, y`.
pub fn draw(
    il: HIMAGELIST,
    i: i32,
    hdc: HDC,
    (x, y): (i32, i32),
    style: IMAGE_LIST_DRAW_STYLE,
) -> bool {
    // SAFETY: no pointers.
    unsafe { ImageList_Draw(il, i, hdc, x, y, style) != 0 }
}

/// The size of the list's images: (cx, cy).
pub fn icon_size(il: HIMAGELIST) -> Option<(i32, i32)> {
    let (mut cx, mut cy) = (0, 0);
    // SAFETY: two out-values.
    (unsafe { ImageList_GetIconSize(il, &mut cx, &mut cy) } != 0).then_some((cx, cy))
}

/// How many images the list has.
pub fn count(il: HIMAGELIST) -> i32 {
    // SAFETY: no pointers.
    unsafe { ImageList_GetImageCount(il) }
}

/// The colour drawn behind the images (CLR_NONE: none); the one before.
pub fn set_bk_color(il: HIMAGELIST, c: COLORREF) -> COLORREF {
    // SAFETY: no pointers.
    unsafe { ImageList_SetBkColor(il, c) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_image_list_is_made_and_grown() {
        let il = ImageList::create(16, 16, 0x20 | 0x1, 4, 4).expect("an image list");
        assert!(il.set_image_count(3));
        assert_eq!(count(il.handle()), 3);
        assert!(icon(il.handle(), 7, 0).is_null());
        let raw = il.into_raw();
        assert!(destroy(raw));
        assert!(!destroy(raw), "gone: reported, not followed");
    }
}
