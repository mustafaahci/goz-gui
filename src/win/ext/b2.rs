//! Batch 2 of the move onto `win`: what the sidebars' panes, the docks and
//! the find bar need besides the modules above. Windows' parents, popup
//! menus, a few list- and tree-view messages that take UTF-16 text, the
//! trees' own image lists, the shell's file and icon information, the file
//! system's directory listings and drives, and windows moved together.

use windows_sys::Win32::Foundation::{HANDLE, HWND, INVALID_HANDLE_VALUE};
use windows_sys::Win32::Storage::FileSystem::{
    FIND_FIRST_EX_LARGE_FETCH, FindClose, FindExInfoBasic, FindExSearchLimitToDirectories,
    FindFirstFileExW, FindNextFileW, GetDriveTypeW, GetLogicalDrives, GetVolumeInformationW,
    WIN32_FIND_DATAW,
};
use windows_sys::Win32::System::Diagnostics::Debug::{SetThreadErrorMode, THREAD_ERROR_MODE};
use windows_sys::Win32::UI::Controls::{
    HIMAGELIST, HTREEITEM, IMAGE_LIST_DRAW_STYLE, IMAGELIST_CREATION_FLAGS, ImageList_Create,
    ImageList_Destroy, ImageList_GetIcon, ImageList_ReplaceIcon, ImageList_SetImageCount,
    LVM_GETHOTITEM, TVIF_TEXT, TVINSERTSTRUCTW, TVITEMW, TVM_INSERTITEMW, TVM_SETITEMW,
};
use windows_sys::Win32::UI::Shell::{
    SHELLSTATEA, SHFILEINFOW, SHGFI_FLAGS, SHGFI_PIDL, SHGSI_FLAGS, SHGetFileInfoW, SHGetImageList,
    SHGetSetSettings, SHGetStockIconInfo, SHSTOCKICONID, SHSTOCKICONINFO, SSF_MASK, StrCmpLogicalW,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, BeginDeferWindowPos, CreatePopupMenu, DeferWindowPos, DeleteMenu, DestroyMenu,
    EndDeferWindowPos, GET_MENU_DEFAULT_ITEM_FLAGS, GetMenuDefaultItem, HICON, HMENU,
    MENU_ITEM_FLAGS, MF_BITMAP, MF_OWNERDRAW, MF_SEPARATOR, SET_WINDOW_POS_FLAGS, SetParent,
};
use windows_sys::core::GUID;

use crate::win::{lv, of_class, send_raw, tv, wz_units};

/// Makes `parent` the window's parent (HWND_MESSAGE: a message-only
/// window); the parent before, null if it failed.
pub fn set_parent(hwnd: HWND, parent: HWND) -> HWND {
    // SAFETY: no pointers; both handles are checked by the system.
    unsafe { SetParent(hwnd, parent) }
}

// ---- Popup menus ----

/// A new, empty popup menu (null if the system could not make one): its
/// maker destroys it with [`destroy_menu`] once it is done with.
pub fn create_popup_menu() -> HMENU {
    // SAFETY: no pointers.
    unsafe { CreatePopupMenu() }
}

/// Appends an item to `menu`: `text` with MF_* `flags` (MF_STRING,
/// MF_CHECKED...) and command `id`. The item is always text: MF_BITMAP and
/// MF_OWNERDRAW, which would take the text's address for a handle or the
/// item's data, are not taken. False if it could not.
pub fn append_menu(menu: HMENU, flags: MENU_ITEM_FLAGS, id: usize, text: &str) -> bool {
    let z = crate::win::wz(text);
    // SAFETY: a NUL-terminated string read during the call (the menu
    // copies it), as text (the flags that would make it something else are
    // masked off). MF_POPUP makes `id` a menu handle, which the system
    // checks.
    unsafe { AppendMenuW(menu, flags & !(MF_BITMAP | MF_OWNERDRAW), id, z.as_ptr()) != 0 }
}

/// Appends a separator to `menu`.
pub fn append_separator(menu: HMENU) -> bool {
    // SAFETY: a separator has no text: no pointer is read.
    unsafe { AppendMenuW(menu, MF_SEPARATOR, 0, core::ptr::null()) != 0 }
}

/// Destroys a menu (and its submenus); false if it could not (a menu
/// handle that is stale is reported as an error, not followed).
pub fn destroy_menu(menu: HMENU) -> bool {
    // SAFETY: no pointers; user32 checks the handle.
    unsafe { DestroyMenu(menu) != 0 }
}

/// GetMenuDefaultItem: the default item's command (or position, with
/// `by_position`), u32::MAX (-1) for none.
pub fn menu_default_item(
    menu: HMENU,
    by_position: bool,
    flags: GET_MENU_DEFAULT_ITEM_FLAGS,
) -> u32 {
    // SAFETY: no pointers.
    unsafe { GetMenuDefaultItem(menu, u32::from(by_position), flags) }
}

/// DeleteMenu: takes item `item` (a command with MF_BYCOMMAND, a position
/// with MF_BYPOSITION) out of `menu`, destroying a submenu it opens.
pub fn delete_menu(menu: HMENU, item: u32, flags: MENU_ITEM_FLAGS) -> bool {
    // SAFETY: no pointers.
    unsafe { DeleteMenu(menu, item, flags) != 0 }
}

// ---- Windows moved together ----

/// A window and where it goes: x, y, width, height.
pub type Move = (HWND, (i32, i32, i32, i32));

/// Moves each window of `moves` to its place together
/// (BeginDeferWindowPos, DeferWindowPos, EndDeferWindowPos), with
/// SWP_* `flags`, so they repaint once. False when the system could not
/// (out of resources): then none, or not all, moved.
pub fn defer_window_pos(moves: &[Move], flags: SET_WINDOW_POS_FLAGS) -> bool {
    let n = i32::try_from(moves.len()).unwrap_or(i32::MAX);
    // SAFETY: the structure the system makes for the moves stays in here:
    // each DeferWindowPos hands back the one to go on with (null when it
    // failed, and then the structure is gone and not used again), and the
    // last is ended once. The windows are handles the system checks.
    unsafe {
        let mut hdwp = BeginDeferWindowPos(n);
        for &(hwnd, (x, y, cx, cy)) in moves {
            if hdwp.is_null() {
                break;
            }
            hdwp = DeferWindowPos(hdwp, hwnd, core::ptr::null_mut(), x, y, cx, cy, flags);
        }
        !hdwp.is_null() && EndDeferWindowPos(hdwp) != 0
    }
}

// ---- List and tree views ----

/// LVM_GETHOTITEM: the item under the mouse, -1 for none.
pub fn lv_hot_item(lv: HWND) -> i32 {
    // SAFETY: LVM_GETHOTITEM has no parameters; sent to a list view only.
    unsafe { send_raw(of_class(lv, lv::CLASS), LVM_GETHOTITEM, 0, 0) as i32 }
}

/// [`tv::insert`] with UTF-16 text (a file name need not be valid
/// Unicode): `item` (its TVIF_* fields, TVIF_TEXT added) under `parent`
/// after `after`, with `text`. The new item; 0 if not.
pub fn tv_insert_wide(
    tree: HWND,
    parent: HTREEITEM,
    after: HTREEITEM,
    item: TVITEMW,
    text: &[u16],
) -> HTREEITEM {
    let z = wz_units(crate::win::until_nul(text));
    let mut it = item;
    it.mask |= TVIF_TEXT;
    it.pszText = z.as_ptr().cast_mut();
    it.cchTextMax = 0;
    let mut ins = TVINSERTSTRUCTW {
        hParent: parent,
        hInsertAfter: after,
        ..Default::default()
    };
    ins.Anonymous.item = it;
    // SAFETY: TVM_INSERTITEMW's lParam is a TVINSERTSTRUCTW whose only
    // pointer is the text set above, read during the call; sent to a tree
    // view only.
    unsafe {
        send_raw(
            of_class(tree, tv::CLASS),
            TVM_INSERTITEMW,
            0,
            (&raw const ins) as isize,
        )
    }
}

/// Sets tree item `item`'s text from UTF-16.
pub fn tv_set_text_wide(tree: HWND, item: HTREEITEM, text: &[u16]) -> bool {
    let z = wz_units(crate::win::until_nul(text));
    let it = TVITEMW {
        mask: TVIF_TEXT,
        hItem: item,
        pszText: z.as_ptr().cast_mut(),
        ..Default::default()
    };
    // SAFETY: TVM_SETITEMW's lParam is a TVITEMW whose only pointer is the
    // text set above, read during the call; sent to a tree view only.
    unsafe {
        send_raw(
            of_class(tree, tv::CLASS),
            TVM_SETITEMW,
            0,
            (&raw const it) as isize,
        ) != 0
    }
}

// ---- Image lists ----

/// An image list this made (ImageList_Create), destroyed when it drops:
/// no other code destroys it, so a window it is handed to must be given
/// another (or none) before this drops.
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
        // SAFETY: the list this made, destroyed once.
        unsafe { ImageList_Destroy(self.0) };
    }
}

/// A copy of image `i` of `il` as an icon (ImageList_GetIcon), which the
/// caller destroys (`gdi::destroy_icon`); null if there is none.
pub fn imagelist_icon(il: HIMAGELIST, i: i32, flags: IMAGE_LIST_DRAW_STYLE) -> HICON {
    // SAFETY: no pointers; comctl32 checks the list's handle.
    unsafe { ImageList_GetIcon(il, i, flags) }
}

/// `IID_IImageList`, which `SHGetImageList` hands the system lists out as.
const IID_IMAGE_LIST: GUID = GUID::from_u128(0x46eb5926_582e_4017_9fdf_e8998daa0950);

/// One of the system's image lists (SHGetImageList: SHIL_SMALL 1,
/// SHIL_LARGE 0, SHIL_EXTRALARGE 2...), 0 if the shell has none. It is the
/// shell's, alive for as long as the process: it is never destroyed.
pub fn system_image_list(which: i32) -> HIMAGELIST {
    let mut ppv: *mut core::ffi::c_void = core::ptr::null_mut();
    // SAFETY: an out-pointer for the interface asked for; what comes back
    // is never followed here, only handed on as the list's handle (which
    // comctl32 takes an IImageList as). Its reference is kept, so the list
    // stays.
    let hr = unsafe { SHGetImageList(which, &IID_IMAGE_LIST, &mut ppv) };
    if hr < 0 || ppv.is_null() {
        0
    } else {
        ppv as HIMAGELIST
    }
}

// ---- The shell ----

/// SHGetFileInfoW of the file at `path` (no NUL needed) with
/// `attributes` and SHGFI_* `flags`: what it answered (an image list's
/// handle with SHGFI_SYSICONINDEX, nonzero for success otherwise) and the
/// structure it filled. SHGFI_PIDL is not taken: `path` is a path.
/// With SHGFI_ICON the icon in the structure is the caller's to destroy.
pub fn file_info(path: &[u16], attributes: u32, flags: SHGFI_FLAGS) -> (usize, SHFILEINFOW) {
    let p = wz_units(crate::win::until_nul(path));
    let mut info = SHFILEINFOW::default();
    // SAFETY: a NUL-terminated path (SHGFI_PIDL masked off, so it is read
    // as one) and an out-struct of the size given.
    let answer = unsafe {
        SHGetFileInfoW(
            p.as_ptr(),
            attributes,
            &mut info,
            size_of::<SHFILEINFOW>() as u32,
            flags & !SHGFI_PIDL,
        )
    };
    (answer, info)
}

/// SHGetStockIconInfo of stock icon `which` with SHGSI_* `flags`; `None`
/// if the shell has none. With SHGSI_ICON the icon in it is the caller's
/// to destroy.
pub fn stock_icon_info(which: SHSTOCKICONID, flags: SHGSI_FLAGS) -> Option<SHSTOCKICONINFO> {
    let mut info = SHSTOCKICONINFO {
        cbSize: size_of::<SHSTOCKICONINFO>() as u32,
        ..Default::default()
    };
    // SAFETY: an out-struct with its size.
    let hr = unsafe { SHGetStockIconInfo(which, flags, &mut info) };
    (hr >= 0).then_some(info)
}

/// The shell's settings `mask` names (SSF_*), read (SHGetSetSettings).
pub fn shell_state(mask: SSF_MASK) -> SHELLSTATEA {
    let mut state = SHELLSTATEA::default();
    // SAFETY: an out-struct (no pointers in it); only read, not set.
    unsafe { SHGetSetSettings(&mut state, mask, 0) };
    state
}

/// StrCmpLogicalW of `a` and `b` (no NULs needed): below 0, 0 or above 0
/// as Explorer sorts the names (`2` before `10`, case aside).
pub fn cmp_logical(a: &[u16], b: &[u16]) -> i32 {
    let (a, b) = (
        wz_units(crate::win::until_nul(a)),
        wz_units(crate::win::until_nul(b)),
    );
    // SAFETY: two NUL-terminated strings read during the call.
    unsafe { StrCmpLogicalW(a.as_ptr(), b.as_ptr()) }
}

// ---- The file system ----

/// A directory listing under way (FindFirstFileExW asking for directories,
/// basic information), closed when it drops.
pub struct FindDirs {
    handle: HANDLE,
    data: WIN32_FIND_DATAW,
}

impl FindDirs {
    /// Starts listing what matches `pattern` (`C:\x\*`, no NUL needed),
    /// directories asked for (a file system may give files too: the
    /// attributes say), in large fetches with `large_fetch`. The first
    /// entry is in [`FindDirs::data`]; `Err` with the system's error when
    /// nothing can be listed.
    pub fn first(pattern: &[u16], large_fetch: bool) -> Result<FindDirs, u32> {
        let p = wz_units(crate::win::until_nul(pattern));
        let mut data = WIN32_FIND_DATAW::default();
        let flags = if large_fetch {
            FIND_FIRST_EX_LARGE_FETCH
        } else {
            0
        };
        // SAFETY: a NUL-terminated pattern; FindExInfoBasic fills a
        // WIN32_FIND_DATAW, the out-struct given; no search filter.
        let handle = unsafe {
            FindFirstFileExW(
                p.as_ptr(),
                FindExInfoBasic,
                (&raw mut data).cast(),
                FindExSearchLimitToDirectories,
                core::ptr::null(),
                flags,
            )
        };
        if handle == INVALID_HANDLE_VALUE {
            return Err(crate::win::sys::last_error());
        }
        Ok(FindDirs { handle, data })
    }

    /// The entry the listing is at (a copy of what the system wrote).
    pub fn data(&self) -> &WIN32_FIND_DATAW {
        &self.data
    }

    /// Goes on to the next entry; false when there is none (or it failed).
    pub fn next(&mut self) -> bool {
        // SAFETY: the handle this owns, and its out-struct.
        unsafe { FindNextFileW(self.handle, &mut self.data) != 0 }
    }
}

impl Drop for FindDirs {
    fn drop(&mut self) {
        // SAFETY: the find handle this owns, closed once.
        unsafe { FindClose(self.handle) };
    }
}

/// The drives there are, one bit each from A (GetLogicalDrives).
pub fn logical_drives() -> u32 {
    // SAFETY: no arguments.
    unsafe { GetLogicalDrives() }
}

/// GetDriveTypeW of the drive whose root is `root` (`C:\`, no NUL needed).
pub fn drive_type(root: &[u16]) -> u32 {
    let p = wz_units(crate::win::until_nul(root));
    // SAFETY: a NUL-terminated root path.
    unsafe { GetDriveTypeW(p.as_ptr()) }
}

/// Whether the volume at `root` answers (GetVolumeInformationW with no
/// buffers): false for a removable drive with no disk in.
pub fn volume_ready(root: &[u16]) -> bool {
    let p = wz_units(crate::win::until_nul(root));
    // SAFETY: a NUL-terminated root path; no buffers asked for.
    unsafe {
        GetVolumeInformationW(
            p.as_ptr(),
            core::ptr::null_mut(),
            0,
            core::ptr::null_mut(),
            core::ptr::null_mut(),
            core::ptr::null_mut(),
            core::ptr::null_mut(),
            0,
        ) != 0
    }
}

/// Sets this thread's error mode (SEM_FAILCRITICALERRORS: no dialog for a
/// drive without a disk); false if it could not.
pub fn set_thread_error_mode(mode: THREAD_ERROR_MODE) -> bool {
    // SAFETY: no old mode asked for (null is allowed).
    unsafe { SetThreadErrorMode(mode, core::ptr::null_mut()) != 0 }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn w(s: &str) -> Vec<u16> {
        s.encode_utf16().collect()
    }

    #[test]
    fn names_compare_as_explorer_sorts_them() {
        assert!(cmp_logical(&w("b2"), &w("b10")) < 0);
        assert!(cmp_logical(&w("B10"), &w("b2")) > 0);
        assert_eq!(cmp_logical(&w("abc"), &w("ABC")), 0);
        // What follows a NUL is not compared.
        assert_eq!(cmp_logical(&[97, 0, 98], &w("a")), 0);
    }

    #[test]
    fn a_menu_is_made_filled_and_destroyed() {
        let menu = create_popup_menu();
        assert!(!menu.is_null());
        assert!(append_menu(menu, 0, 7, "&Seven"));
        assert!(append_separator(menu));
        assert_eq!(menu_default_item(menu, false, 0), u32::MAX);
        assert!(delete_menu(
            menu,
            7,
            windows_sys::Win32::UI::WindowsAndMessaging::MF_BYCOMMAND
        ));
        assert!(destroy_menu(menu));
        // Gone: the handle is reported stale, not followed.
        assert!(!destroy_menu(menu));
    }

    #[test]
    fn a_listing_goes_through_a_folder() {
        let base = std::env::temp_dir().join(format!("goz-gui-b2-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(base.join("inner")).expect("make dirs");
        let mut pattern: Vec<u16> = {
            use std::os::windows::ffi::OsStrExt;
            base.as_os_str().encode_wide().collect()
        };
        pattern.extend(w("\\*"));
        let mut find = FindDirs::first(&pattern, true).expect("lists");
        let mut names = Vec::new();
        loop {
            let name = crate::win::until_nul(&find.data().cFileName).to_vec();
            names.push(String::from_utf16_lossy(&name));
            if !find.next() {
                break;
            }
        }
        drop(find);
        assert!(names.iter().any(|n| n == "inner"));
        let _ = std::fs::remove_dir_all(&base);
        let mut missing = pattern.clone();
        missing.truncate(missing.len() - 1);
        missing.extend(w("nothing-here\\*"));
        assert!(FindDirs::first(&missing, false).is_err());
    }

    #[test]
    fn an_image_list_is_made_and_grown() {
        let il = ImageList::create(16, 16, 0x20 | 0x1, 4, 4).expect("an image list");
        assert!(il.set_image_count(3));
        assert_eq!(crate::win::imagelist::count(il.handle()), 3);
        assert!(imagelist_icon(il.handle(), 7, 0).is_null());
    }

    #[test]
    fn the_drives_are_there() {
        assert!(logical_drives() != 0);
        // DRIVE_NO_ROOT_DIR for a root that is none.
        assert_eq!(drive_type(&w("no such root")), 1);
    }
}
