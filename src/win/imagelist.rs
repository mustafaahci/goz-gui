//! Image lists (comctl32 checks the handles it is given).

use windows_sys::Win32::Foundation::COLORREF;
use windows_sys::Win32::Graphics::Gdi::HDC;
use windows_sys::Win32::UI::Controls::{
    HIMAGELIST, IMAGE_LIST_DRAW_STYLE, ImageList_Draw, ImageList_GetIconSize,
    ImageList_GetImageCount, ImageList_SetBkColor,
};

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

pub fn count(il: HIMAGELIST) -> i32 {
    // SAFETY: no pointers.
    unsafe { ImageList_GetImageCount(il) }
}

pub fn set_bk_color(il: HIMAGELIST, c: COLORREF) -> COLORREF {
    // SAFETY: no pointers.
    unsafe { ImageList_SetBkColor(il, c) }
}
