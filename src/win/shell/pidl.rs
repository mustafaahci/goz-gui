//! Item ID lists and the shell's folders.
//!
//! An ID list is a chain of IDs, each led by its size, ending with an empty
//! one. Only the shell makes the lists here (no other code makes a
//! [`Pidl`]), so each is well-formed. The folders read what an ID holds as
//! data they check (IDs come from anywhere: shortcuts, other processes'
//! drags), so a list given to the wrong folder is refused, never followed.

use core::ptr::NonNull;

use windows::Win32::Foundation::HWND as WHWND;
use windows::Win32::UI::Shell::Common::ITEMIDLIST as WITEMIDLIST;
use windows::Win32::UI::Shell::{IShellFolder, SHBindToParent, SHGetDesktopFolder};
use windows::core::{Interface, PCWSTR};
use windows_sys::Win32::Foundation::HWND;
use windows_sys::Win32::UI::Shell::Common::ITEMIDLIST;
use windows_sys::Win32::UI::Shell::{
    ILClone, ILCombine, ILCreateFromPathW, ILFindLastID, ILFree, ILIsEqual, ILIsParent,
    ILRemoveLastID, SHGetPathFromIDListEx, SHOpenFolderAndSelectItems,
};

use super::{DataObject, ShellMenu};
use crate::win::{len_u32, nul_ended};

/// An item ID list the shell made, freed (ILFree) when this drops.
/// Absolute (from the desktop) or relative to a folder, as the function
/// that made it says.
pub struct Pidl(NonNull<ITEMIDLIST>);

impl Pidl {
    /// The list at `p`, owned from now on (null: none).
    ///
    /// # Safety
    /// `p` is null, or a well-formed list the shell allocated for the
    /// caller that nothing else frees or keeps.
    pub(super) unsafe fn own(p: *mut ITEMIDLIST) -> Option<Pidl> {
        NonNull::new(p).map(Pidl)
    }

    pub(super) fn ptr(&self) -> *const ITEMIDLIST {
        self.0.as_ptr()
    }

    /// The same list, as the `windows` crate's interfaces take it.
    fn wptr(&self) -> *const WITEMIDLIST {
        self.ptr().cast()
    }

    /// The absolute ID list of the item at `path`, parsed whole from the
    /// desktop (ILCreateFromPathW, which may read the disk); `None` when
    /// the shell finds no such item.
    pub fn from_path(path: &[u16]) -> Option<Pidl> {
        let path = nul_ended(path);
        // SAFETY: a NUL-terminated path; the list made (or null) is the
        // caller's, owned by the new value.
        unsafe { Pidl::own(ILCreateFromPathW(path.as_ptr())) }
    }

    /// Whether it is the empty list (as an absolute one, the desktop).
    pub fn is_empty(&self) -> bool {
        // SAFETY: a list starts with its first ID's size (two bytes, at
        // least those of the terminator), read unaligned.
        unsafe { self.ptr().cast::<u16>().read_unaligned() == 0 }
    }

    /// `self` followed by `child` (relative to `self`'s item): a new list
    /// (ILCombine).
    pub fn join(&self, child: &Pidl) -> Option<Pidl> {
        // SAFETY: two well-formed lists, read during the call; the new one
        // (or null) is the caller's, owned by the new value.
        unsafe { Pidl::own(ILCombine(self.ptr(), child.ptr())) }
    }

    /// A copy (ILClone); `None` if there was no memory for it.
    pub fn try_clone(&self) -> Option<Pidl> {
        // SAFETY: a well-formed list, copied; the copy (or null) is the
        // caller's, owned by the new value.
        unsafe { Pidl::own(ILClone(self.ptr())) }
    }

    /// Takes its last ID off, making it its parent's; false at the
    /// desktop's (empty) list, which has none.
    pub fn remove_last_id(&mut self) -> bool {
        // SAFETY: a well-formed list of this one's; ILRemoveLastID only
        // writes the new terminator within it.
        unsafe { ILRemoveLastID(self.0.as_ptr()) != 0 }
    }

    /// A copy without its last ID: the list of its item's parent.
    pub fn parent(&self) -> Option<Pidl> {
        let mut copy = self.try_clone()?;
        copy.remove_last_id();
        Some(copy)
    }

    /// Whether `self` is `child`'s parent (`immediate`) or one of its
    /// ancestors (ILIsParent).
    pub fn is_parent_of(&self, child: &Pidl, immediate: bool) -> bool {
        // SAFETY: two well-formed lists, read during the call.
        unsafe { ILIsParent(self.ptr(), child.ptr(), immediate.into()) != 0 }
    }

    /// Whether it names the same item as `other` (ILIsEqual).
    pub fn is_equal(&self, other: &Pidl) -> bool {
        // SAFETY: two well-formed lists, read during the call.
        unsafe { ILIsEqual(self.ptr(), other.ptr()) != 0 }
    }

    /// The path of the file or folder an absolute list names
    /// (SHGetPathFromIDListEx); `None` for an item that is neither.
    pub fn path(&self) -> Option<Vec<u16>> {
        let mut buf = vec![0u16; 32_768];
        // SAFETY: a well-formed list; the buffer with its length in units.
        let ok =
            unsafe { SHGetPathFromIDListEx(self.ptr(), buf.as_mut_ptr(), len_u32(buf.len()), 0) }
                != 0;
        // A path without its NUL is not taken.
        let len = buf.iter().position(|&c| c == 0).unwrap_or(0);
        (ok && len > 0).then(|| buf[..len].to_vec())
    }
}

impl Drop for Pidl {
    fn drop(&mut self) {
        // SAFETY: the list this owns, freed once.
        unsafe { ILFree(self.ptr()) };
    }
}

/// Opens `folder` in Explorer with `items` (items in it) selected, by
/// their last IDs; with none, opens `folder`'s own folder with `folder`
/// selected (SHOpenFolderAndSelectItems).
pub fn open_folder_and_select(folder: &Pidl, items: &[&Pidl]) -> bool {
    // SAFETY: each item's last ID points into that item's list, which
    // outlives the call.
    let children: Vec<*const ITEMIDLIST> = items
        .iter()
        .map(|p| unsafe { ILFindLastID(p.ptr()) }.cast_const())
        .collect();
    let array = if children.is_empty() {
        core::ptr::null()
    } else {
        children.as_ptr()
    };
    // SAFETY: a valid list and `children.len()` relative IDs, all alive for
    // the call.
    unsafe { SHOpenFolderAndSelectItems(folder.ptr(), len_u32(children.len()), array, 0) >= 0 }
}

/// A folder of the shell's namespace (IShellFolder). COM must be
/// initialised on the thread, which it then belongs to.
#[derive(Clone)]
pub struct Folder(IShellFolder);

impl Folder {
    /// The desktop's folder, the root of the namespace
    /// (SHGetDesktopFolder).
    pub fn desktop() -> Option<Folder> {
        // SAFETY: no pointers; the folder comes back owned.
        unsafe { SHGetDesktopFolder() }.ok().map(Folder)
    }

    /// The folder that holds the item absolute list `id` names
    /// (SHBindToParent).
    pub fn parent_of(id: &Pidl) -> Option<Folder> {
        // SAFETY: a well-formed list, read during the call; the last ID is
        // not asked for.
        unsafe { SHBindToParent(id.wptr(), None) }.ok().map(Folder)
    }

    /// The folder that list `id` (relative to this one) names in it
    /// (BindToObject).
    pub fn bind(&self, id: &Pidl) -> Option<Folder> {
        // SAFETY: a well-formed list, read during the call; no bind context.
        unsafe { self.0.BindToObject(id.wptr(), None) }
            .ok()
            .map(Folder)
    }

    /// The list, relative to this folder, of the item called `name` in it
    /// (ParseDisplayName, no attributes asked for); `None` when it is not
    /// there.
    pub fn parse(&self, name: &[u16]) -> Option<Pidl> {
        let name = nul_ended(name);
        let mut id: *mut WITEMIDLIST = core::ptr::null_mut();
        let mut attributes = 0u32;
        // SAFETY: a NUL-terminated name and out-values that outlive the
        // call; the list, when it succeeds, comes back owned.
        let ok = unsafe {
            self.0.ParseDisplayName(
                WHWND::default(),
                None,
                PCWSTR(name.as_ptr()),
                None,
                &mut id,
                &mut attributes,
            )
        }
        .is_ok();
        // SAFETY: on success the list ParseDisplayName allocated for the
        // caller.
        if ok {
            unsafe { Pidl::own(id.cast()) }
        } else {
            None
        }
    }

    /// The Explorer context menu of the items `ids` name, owned by `owner`
    /// (GetUIObjectOf). With `last`, each item is its list's last ID (the
    /// lists are absolute ones of items this folder holds); else each list
    /// whole, relative to this folder.
    pub fn context_menu(&self, owner: HWND, ids: &[Pidl], last: bool) -> Option<ShellMenu> {
        self.ui_object(owner, ids, last).map(ShellMenu::new)
    }

    /// The data object of the items `ids` name (as for
    /// [`context_menu`](Self::context_menu)), as Explorer puts on the
    /// clipboard or drags: CF_HDROP, the Shell IDList Array and the rest.
    pub fn data_object(&self, owner: HWND, ids: &[Pidl], last: bool) -> Option<DataObject> {
        self.ui_object(owner, ids, last).map(DataObject)
    }

    /// One of the folder's UI objects for the items `ids` name, as
    /// interface `T`.
    fn ui_object<T: Interface>(&self, owner: HWND, ids: &[Pidl], last: bool) -> Option<T> {
        let children: Vec<*const WITEMIDLIST> = ids
            .iter()
            .map(|id| {
                if last {
                    // SAFETY: a well-formed list; the pointer is into it,
                    // which `ids` keeps alive across the call below.
                    unsafe { ILFindLastID(id.ptr()) }.cast_const().cast()
                } else {
                    id.wptr()
                }
            })
            .collect();
        // SAFETY: lists (or IDs within them) that `ids` keeps alive during
        // the call; the object comes back owned, as interface `T`.
        unsafe { self.0.GetUIObjectOf::<T>(WHWND(owner), &children, None) }.ok()
    }
}
