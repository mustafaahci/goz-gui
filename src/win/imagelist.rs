//! Image lists: owned ones ([`ImageList`], destroyed when the last holder
//! drops), the shell's system image lists ([`SystemList`], which live as
//! long as the process), and drawing their images.
//!
//! An image list is not a handle the system checks: since comctl32 6 an
//! HIMAGELIST is an object's address (an `IImageList*`, as SHGetImageList
//! documents), followed as a pointer by every call. So no safe function
//! here takes a raw HIMAGELIST or destroys one: a list is read through an
//! [`ImageList`] or a [`SystemList`], each alive while it is used, and a
//! control given one keeps it alive as long as it shows it
//! ([`super::lv::set_image_list`], [`super::tv::set_image_list`]).

use std::rc::Rc;
use std::sync::Mutex;

use windows_sys::Win32::Graphics::Gdi::HDC;
use windows_sys::Win32::UI::Controls::{
    HIMAGELIST, IMAGE_LIST_DRAW_STYLE, IMAGELIST_CREATION_FLAGS, IMAGELISTDRAWPARAMS,
    ImageList_Create, ImageList_Destroy, ImageList_Draw, ImageList_DrawIndirect, ImageList_GetIcon,
    ImageList_GetIconSize, ImageList_ReplaceIcon, ImageList_SetImageCount,
};
use windows_sys::Win32::UI::Shell::SHGetImageList;
use windows_sys::Win32::UI::WindowsAndMessaging::HICON;
use windows_sys::core::GUID;

use super::icon::Icon;

/// An image list goz-gui made (ImageList_Create). Its clones are the same
/// list, which is destroyed when the last of them drops; a tree view it is
/// given to holds one for as long as it shows it. Not `Send`: its clones
/// count on one thread.
#[derive(Clone)]
pub struct ImageList(Rc<Made>);

/// The list itself, destroyed once, when the last [`ImageList`] of it
/// drops.
struct Made(HIMAGELIST);

impl Drop for Made {
    fn drop(&mut self) {
        // SAFETY: the list ImageList_Create made, which nothing holds any
        // more (no clone of it is left, and no control shows it: a control
        // given it holds a clone, or took it whole), destroyed once.
        unsafe { ImageList_Destroy(self.0) };
    }
}

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
        (il != 0).then(|| ImageList(Rc::new(Made(il))))
    }

    /// ImageList_ReplaceIcon: `icon` copied in as image `i` (-1: added at
    /// the end); its index, -1 if it failed. The list keeps a copy: the
    /// caller still owns `icon`.
    pub fn replace_icon(&self, i: i32, icon: HICON) -> i32 {
        // SAFETY: a list alive while this is; the icon is a handle the
        // system checks.
        unsafe { ImageList_ReplaceIcon(self.0.0, i, icon) }
    }

    /// Makes the list `count` images long (new ones blank).
    pub fn set_image_count(&self, count: u32) -> bool {
        // SAFETY: a list alive while this is.
        unsafe { ImageList_SetImageCount(self.0.0, count) != 0 }
    }

    /// The list, given up whole when this is its only holder (to a list
    /// view, which destroys it with itself); itself back when another
    /// clone (or a tree showing it) holds it too.
    pub(crate) fn into_unique(self) -> Result<HIMAGELIST, ImageList> {
        match Rc::try_unwrap(self.0) {
            Ok(made) => {
                let il = made.0;
                core::mem::forget(made);
                Ok(il)
            }
            Err(rc) => Err(ImageList(rc)),
        }
    }

    /// Takes back `il`, a list [`ImageList::into_unique`] gave up which no
    /// control shows any more, to destroy when this drops.
    ///
    /// # Safety
    /// `il` came from `into_unique` and is held by nothing else.
    pub(crate) unsafe fn reclaim(il: HIMAGELIST) -> ImageList {
        ImageList(Rc::new(Made(il)))
    }
}

/// One of the shell's system image lists (SHGetImageList's SHIL_*): never
/// destroyed, as the reference to it this layer asked for is kept for the
/// process (one per list, [`system`]). Its images are the shell's icons,
/// numbered alike in every one of the lists; it takes draws from any
/// thread.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct SystemList {
    list: HIMAGELIST,
    which: i32,
}

impl SystemList {
    /// A new reference to the list (SHGetImageList again), for a list view
    /// that may destroy the one it is given (one without
    /// LVS_SHAREIMAGELISTS, as it is destroyed): what it destroys is that
    /// reference, released, never the one this layer keeps. Null if the
    /// shell would not give one.
    pub(crate) fn new_reference(self) -> HIMAGELIST {
        get(self.which).unwrap_or(0)
    }

    /// Which list it is (SHIL_*).
    pub fn which(self) -> i32 {
        self.which
    }
}

/// `IID_IImageList`, which `SHGetImageList` hands the system lists out as.
const IID_IMAGE_LIST: GUID = GUID::from_u128(0x46eb5926_582e_4017_9fdf_e8998daa0950);

/// SHIL_LARGE to SHIL_JUMBO: the system lists there are.
const SYSTEM_LISTS: usize = 5;

/// The references [`system`] keeps, one per list, never released.
static SYSTEM: Mutex<[HIMAGELIST; SYSTEM_LISTS]> = Mutex::new([0; SYSTEM_LISTS]);

/// SHGetImageList of list `which`: an `IImageList*` with a reference of
/// its own (the caller's to release, or keep), or `None`.
fn get(which: i32) -> Option<HIMAGELIST> {
    let mut ppv: *mut core::ffi::c_void = core::ptr::null_mut();
    // SAFETY: an out-pointer for the interface the IID names; what comes
    // back is never followed here, only handed on as the list's handle
    // (which comctl32 takes an IImageList as).
    let hr = unsafe { SHGetImageList(which, &IID_IMAGE_LIST, &mut ppv) };
    (hr >= 0 && !ppv.is_null()).then_some(ppv as HIMAGELIST)
}

/// The shell's system image list `which` (SHIL_*), which lives as long as
/// the process: the first reference this asks for is kept, never
/// released. `None` if the shell has no such list.
pub fn system(which: i32) -> Option<SystemList> {
    let slot = usize::try_from(which).ok().filter(|&s| s < SYSTEM_LISTS)?;
    let mut lists = SYSTEM.lock().unwrap_or_else(|e| e.into_inner());
    if lists[slot] == 0 {
        lists[slot] = get(which)?;
    }
    Some(SystemList {
        list: lists[slot],
        which,
    })
}

mod sealed {
    use windows_sys::Win32::UI::Controls::HIMAGELIST;

    /// The lists that stay alive while they are borrowed.
    pub trait Sealed {
        /// The list's handle, alive while `self` is.
        fn raw(&self) -> HIMAGELIST;
    }
}

/// An image list to read or draw from: an [`ImageList`] or a
/// [`SystemList`] (sealed: a list alive for as long as it is borrowed).
pub trait List: sealed::Sealed {}

impl sealed::Sealed for ImageList {
    fn raw(&self) -> HIMAGELIST {
        self.0.0
    }
}
impl List for ImageList {}

impl sealed::Sealed for SystemList {
    fn raw(&self) -> HIMAGELIST {
        self.list
    }
}
impl List for SystemList {}

/// `list`'s handle, alive while `list` is borrowed, for the functions of
/// this layer that hand it to a call.
pub(crate) fn raw_of(list: &impl List) -> HIMAGELIST {
    list.raw()
}

/// A copy of image `i` of `list` as an icon (ImageList_GetIcon), destroyed
/// when it drops; `None` if there is none.
pub fn icon(list: &impl List, i: i32, flags: IMAGE_LIST_DRAW_STYLE) -> Option<Icon> {
    // SAFETY: a list alive while borrowed; the icon made is the caller's.
    Icon::own(unsafe { ImageList_GetIcon(list.raw(), i, flags) })
}

/// Draws an image of `list` as `params` says (its list and size are set
/// here); false if it did not (with ILD_ASYNC: the image is not loaded
/// yet).
pub fn draw_indirect(list: &impl List, params: &IMAGELISTDRAWPARAMS) -> bool {
    let p = IMAGELISTDRAWPARAMS {
        cbSize: size_of::<IMAGELISTDRAWPARAMS>() as u32,
        himl: list.raw(),
        ..*params
    };
    // SAFETY: an in-struct with its size, of numbers and handles, its list
    // alive while borrowed.
    unsafe { ImageList_DrawIndirect(&p) != 0 }
}

/// Draws image `i` of `list` on `hdc` at `x, y`.
pub fn draw(
    list: &impl List,
    i: i32,
    hdc: HDC,
    (x, y): (i32, i32),
    style: IMAGE_LIST_DRAW_STYLE,
) -> bool {
    // SAFETY: a list alive while borrowed; a DC GDI checks.
    unsafe { ImageList_Draw(list.raw(), i, hdc, x, y, style) != 0 }
}

/// The size of the list's images: (cx, cy).
pub fn icon_size(list: &impl List) -> Option<(i32, i32)> {
    let (mut cx, mut cy) = (0, 0);
    // SAFETY: a list alive while borrowed; two out-values.
    (unsafe { ImageList_GetIconSize(list.raw(), &mut cx, &mut cy) } != 0).then_some((cx, cy))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_image_list_is_made_grown_and_shared() {
        let il = ImageList::create(16, 16, 0x20 | 0x1, 4, 4).expect("an image list");
        assert!(il.set_image_count(3));
        assert!(icon(&il, 7, 0).is_none());
        assert_eq!(icon_size(&il), Some((16, 16)));
        // A clone is the same list: given up whole only alone.
        let other = il.clone();
        let Err(il) = il.into_unique() else {
            panic!("shared: not given up");
        };
        drop(other);
        let Ok(raw) = il.into_unique() else {
            panic!("alone, given up whole");
        };
        // SAFETY: the list just given up, held by nothing.
        let il = unsafe { ImageList::reclaim(raw) };
        assert_eq!(icon_size(&il), Some((16, 16)));
    }

    #[test]
    fn a_system_list_is_kept_once() {
        // SHIL_SMALL, twice: the same list, its reference asked for once.
        let a = system(1).expect("the small icons");
        let b = system(1).expect("again");
        assert_eq!(a, b);
        assert!(icon_size(&a).is_some());
        assert!(system(99).is_none());
    }
}
