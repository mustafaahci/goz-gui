//! Batch 3 of the move onto `win`.

use std::os::windows::ffi::OsStrExt;
use std::path::Path;

use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, HWND, POINT, SIZE, WAIT_OBJECT_0};
use windows_sys::Win32::Graphics::Gdi::{
    BI_RGB, BITMAPINFO, BITMAPINFOHEADER, CreateDIBSection, DIB_RGB_COLORS, GetTextExtentExPointW,
    HBITMAP, HDC, HGDIOBJ,
};
use windows_sys::Win32::Storage::FileSystem::{GetFileAttributesW, INVALID_FILE_ATTRIBUTES};
use windows_sys::Win32::System::Com::CoTaskMemFree;
use windows_sys::Win32::System::Memory::{HeapOptimizeResources, HeapSetInformation};
use windows_sys::Win32::System::Threading::{
    CreateProcessW, OpenProcess, PROCESS_CREATION_FLAGS, PROCESS_INFORMATION, PROCESS_SYNCHRONIZE,
    STARTF_USESHOWWINDOW, STARTUPINFOW, WaitForSingleObject,
};
use windows_sys::Win32::UI::Controls::{
    EM_CHARFROMPOS, EM_REPLACESEL, EM_SETSEL, HIMAGELIST, IMAGELIST_CREATION_FLAGS,
    IMAGELISTDRAWPARAMS, ImageList_Create, ImageList_Destroy, ImageList_DrawIndirect,
    LVHITTESTINFO, LVM_HITTEST, LVM_SETICONSPACING, LVM_SUBITEMHITTEST,
};
use windows_sys::Win32::UI::Shell::{
    SHELLFLAGSTATE, SHGetImageList, SHGetKnownFolderPath, SHGetSettings, SSF_MASK,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    AllowSetForegroundWindow, AppendMenuW, CheckMenuItem, CreatePopupMenu, DeleteMenu, DestroyMenu,
    EnableMenuItem, GetCaretPos, GetMenuDefaultItem, GetMenuItemCount, GetMenuItemID,
    GetMenuItemInfoW, GetMenuState, GetMenuStringW, HMENU, MENU_ITEM_FLAGS, MENU_ITEM_MASK,
    MENUITEMINFOW, MIIM_FTYPE, MIIM_STRING, MIIM_TYPE, PostMessageW, SetMenu, SetMenuItemInfoW,
};
use windows_sys::core::GUID;

/// Asks Windows to decommit what the process's heaps hold free
/// (HeapSetInformation with HeapOptimizeResources); any thread may.
pub fn heap_optimize_resources() {
    /// `HEAP_OPTIMIZE_RESOURCES_INFORMATION`.
    #[repr(C)]
    struct Optimize {
        version: u32,
        flags: u32,
    }
    let info = Optimize {
        version: 1,
        flags: 0,
    };
    // SAFETY: no heap handle (all of the process's); `info` is the
    // documented structure, version 1, with its size, read during the call.
    unsafe {
        HeapSetInformation(
            core::ptr::null_mut(),
            HeapOptimizeResources,
            (&raw const info).cast(),
            size_of::<Optimize>(),
        )
    };
}

/// Starts the program at `exe` (by its exact path) with the command line
/// `line`, its first window shown as `show` (a SW_*), with CREATE_* `flags`
/// and no handles inherited, in this process's environment and folder.
/// With `front` it may take the foreground. The process's handles are
/// closed here; the error is the system's.
pub fn create_process(
    exe: &Path,
    line: &str,
    show: u16,
    flags: PROCESS_CREATION_FLAGS,
    front: bool,
) -> std::io::Result<()> {
    let app: Vec<u16> = exe.as_os_str().encode_wide().chain(Some(0)).collect();
    // CreateProcessW may write to the command line: a buffer of its own.
    let mut line = crate::win::wz(line);
    let si = STARTUPINFOW {
        cb: size_of::<STARTUPINFOW>() as u32,
        dwFlags: STARTF_USESHOWWINDOW,
        wShowWindow: show,
        ..Default::default()
    };
    let mut pi = PROCESS_INFORMATION::default();
    // SAFETY: NUL-terminated strings (the command line in a buffer of its
    // own the call may write to), no security attributes, no handles
    // inherited, this process's environment and folder; an in-struct with
    // its size and an out-struct.
    let ok = unsafe {
        CreateProcessW(
            app.as_ptr(),
            line.as_mut_ptr(),
            core::ptr::null(),
            core::ptr::null(),
            0,
            flags,
            core::ptr::null(),
            core::ptr::null(),
            &si,
            &mut pi,
        )
    };
    if ok == 0 {
        return Err(std::io::Error::last_os_error());
    }
    if front {
        // SAFETY: no pointers.
        unsafe { AllowSetForegroundWindow(pi.dwProcessId) };
    }
    // SAFETY: the handles CreateProcessW gave back, owned here and closed
    // once.
    unsafe {
        CloseHandle(pi.hThread);
        CloseHandle(pi.hProcess);
    }
    Ok(())
}

/// The shell's settings `mask` asks for (SHGetSettings, SSF_*).
pub fn shell_settings(mask: SSF_MASK) -> SHELLFLAGSTATE {
    let mut s = SHELLFLAGSTATE::default();
    // SAFETY: an out-struct of plain flags.
    unsafe { SHGetSettings(&mut s, mask) };
    s
}

// ---- Menus. user32 checks every menu handle it is given: a stale one is
// an error, never followed. ----

/// The number of items in `menu` (-1 if it is not a menu).
pub fn menu_item_count(menu: HMENU) -> i32 {
    // SAFETY: no pointers.
    unsafe { GetMenuItemCount(menu) }
}

/// The command id of the item at `pos` (u32::MAX for a submenu or none).
pub fn menu_item_id(menu: HMENU, pos: i32) -> u32 {
    // SAFETY: no pointers.
    unsafe { GetMenuItemID(menu, pos) }
}

/// The state of item `item` (MF_BYCOMMAND or MF_BYPOSITION in `flags`);
/// u32::MAX when the menu has no such item.
pub fn menu_state(menu: HMENU, item: u32, flags: MENU_ITEM_FLAGS) -> u32 {
    // SAFETY: no pointers.
    unsafe { GetMenuState(menu, item, flags) }
}

/// The menu's default item (GetMenuDefaultItem): its id, or its position
/// with `by_position`; u32::MAX for none.
pub fn menu_default_item(menu: HMENU, by_position: bool, flags: u32) -> u32 {
    // SAFETY: no pointers.
    unsafe { GetMenuDefaultItem(menu, by_position.into(), flags) }
}

/// Enables or greys item `item` (MF_BYCOMMAND or MF_BYPOSITION with
/// MF_ENABLED, MF_GRAYED or MF_DISABLED); its state before, -1 for none.
pub fn enable_menu_item(menu: HMENU, item: u32, flags: MENU_ITEM_FLAGS) -> i32 {
    // SAFETY: no pointers.
    unsafe { EnableMenuItem(menu, item, flags) }
}

/// Checks or unchecks item `item` (MF_BYCOMMAND or MF_BYPOSITION with
/// MF_CHECKED or MF_UNCHECKED); its check state before, u32::MAX for none.
pub fn check_menu_item(menu: HMENU, item: u32, flags: u32) -> u32 {
    // SAFETY: no pointers.
    unsafe { CheckMenuItem(menu, item, flags) }
}

/// Deletes item `item` (MF_BYCOMMAND or MF_BYPOSITION), destroying its
/// submenu if it has one; false if there is no such item.
pub fn delete_menu(menu: HMENU, item: u32, flags: MENU_ITEM_FLAGS) -> bool {
    // SAFETY: no pointers.
    unsafe { DeleteMenu(menu, item, flags) != 0 }
}

/// Destroys `menu` and its submenus; false if it is not a menu (or was
/// destroyed already).
pub fn destroy_menu(menu: HMENU) -> bool {
    // SAFETY: no pointers; a destroyed menu's handle is not followed again.
    unsafe { DestroyMenu(menu) != 0 }
}

/// A new, empty popup menu (null if none could be made).
pub fn create_popup_menu() -> HMENU {
    // SAFETY: no pointers.
    unsafe { CreatePopupMenu() }
}

/// Appends an item titled `text`, with MF_* `flags` and command `id`.
pub fn append_menu(menu: HMENU, flags: MENU_ITEM_FLAGS, id: usize, text: &str) -> bool {
    let z = crate::win::wz(text);
    // SAFETY: a NUL-terminated string the menu copies during the call (an
    // item that is not a string, a bitmap's or one drawn by its owner, keeps
    // the value as a handle or as data, never reading through it).
    unsafe { AppendMenuW(menu, flags, id, z.as_ptr()) != 0 }
}

/// Gives the window the menu bar `menu` (null: none); false if it could not.
pub fn set_menu(hwnd: HWND, menu: HMENU) -> bool {
    // SAFETY: no pointers.
    unsafe { SetMenu(hwnd, menu) != 0 }
}

/// The label of item `item` (MF_BYCOMMAND or MF_BYPOSITION), up to 255
/// units.
pub fn menu_string(menu: HMENU, item: u32, flags: MENU_ITEM_FLAGS) -> String {
    let mut buf = [0u16; 256];
    // SAFETY: the buffer and its length in units.
    let n = unsafe { GetMenuStringW(menu, item, buf.as_mut_ptr(), buf.len() as i32, flags) };
    String::from_utf16_lossy(&buf[..usize::try_from(n).unwrap_or(0).min(buf.len())])
}

/// MIIM_TYPE says to read `dwTypeData` as a string or a bitmap by `fType`:
/// the item functions here take MIIM_FTYPE and MIIM_STRING instead.
fn without_type(mask: MENU_ITEM_MASK) -> MENU_ITEM_MASK {
    if mask & MIIM_TYPE != 0 {
        (mask & !MIIM_TYPE) | MIIM_FTYPE
    } else {
        mask
    }
}

/// The fields `mask` asks for (MIIM_*) of item `item` (its position with
/// `by_position`, else its command), and its label when MIIM_STRING is
/// among them, up to `max` units. `None` if there is no such item.
pub fn menu_item_info(
    menu: HMENU,
    item: u32,
    by_position: bool,
    mask: MENU_ITEM_MASK,
    max: usize,
) -> Option<(MENUITEMINFOW, Option<String>)> {
    let mask = without_type(mask);
    let mut buf = vec![
        0u16;
        if mask & MIIM_STRING != 0 {
            max.max(1)
        } else {
            0
        }
    ];
    let mut mii = MENUITEMINFOW {
        cbSize: size_of::<MENUITEMINFOW>() as u32,
        fMask: mask,
        ..Default::default()
    };
    if mask & MIIM_STRING != 0 {
        mii.dwTypeData = buf.as_mut_ptr();
        mii.cch = u32::try_from(buf.len()).unwrap_or(u32::MAX);
    }
    // SAFETY: an in-out struct with its size whose only pointer is the
    // buffer of `cch` units (MIIM_TYPE is not asked for, above).
    let ok = unsafe { GetMenuItemInfoW(menu, item, by_position.into(), &mut mii) } != 0;
    if !ok {
        return None;
    }
    let text = (mask & MIIM_STRING != 0)
        .then(|| String::from_utf16_lossy(&buf[..(mii.cch as usize).min(buf.len())]));
    mii.dwTypeData = core::ptr::null_mut();
    mii.cch = 0;
    Some((mii, text))
}

/// Sets the fields `mii.fMask` names (MIIM_*) of item `item` (its position
/// with `by_position`, else its command), with `text` as its label when
/// given (MIIM_STRING then); `mii`'s own text pointer is not used. False
/// if there is no such item.
pub fn set_menu_item_info(
    menu: HMENU,
    item: u32,
    by_position: bool,
    mii: MENUITEMINFOW,
    text: Option<&str>,
) -> bool {
    let mut mii = mii;
    mii.cbSize = size_of::<MENUITEMINFOW>() as u32;
    mii.fMask = without_type(mii.fMask);
    let z = text.map(crate::win::wz);
    match &z {
        Some(z) => {
            mii.fMask |= MIIM_STRING;
            mii.dwTypeData = z.as_ptr().cast_mut();
        }
        None => {
            mii.fMask &= !MIIM_STRING;
            mii.dwTypeData = core::ptr::null_mut();
        }
    }
    mii.cch = 0;
    // SAFETY: an in-struct with its size whose only pointer is the
    // NUL-terminated label set above (which the menu copies), or null.
    unsafe { SetMenuItemInfoW(menu, item, by_position.into(), &mii) != 0 }
}

/// A process opened to wait for its end (PROCESS_SYNCHRONIZE), its handle
/// closed when this drops.
pub struct ProcessWait(HANDLE);

// SAFETY: a process handle may be waited on and closed from any thread.
unsafe impl Send for ProcessWait {}
// SAFETY: as above; waiting takes `&self` and the kernel synchronizes.
unsafe impl Sync for ProcessWait {}

impl ProcessWait {
    /// Process `pid`, opened for waiting only; `None` for one gone already
    /// (or one this process may not wait for).
    pub fn open(pid: u32) -> Option<ProcessWait> {
        // SAFETY: no pointers.
        let h = unsafe { OpenProcess(PROCESS_SYNCHRONIZE, 0, pid) };
        (!h.is_null()).then_some(ProcessWait(h))
    }

    /// Waits for the process to end, at most `ms`; whether it did.
    pub fn wait(&self, ms: u32) -> bool {
        // SAFETY: the handle this owns.
        unsafe { WaitForSingleObject(self.0, ms) == WAIT_OBJECT_0 }
    }
}

impl Drop for ProcessWait {
    fn drop(&mut self) {
        // SAFETY: the handle this owns, closed once.
        unsafe { CloseHandle(self.0) };
    }
}

/// The path of the known folder `id` (FOLDERID_*), as UTF-16 without a NUL;
/// `None` if the shell has none.
pub fn known_folder_path(id: &GUID) -> Option<Vec<u16>> {
    let mut p: *mut u16 = core::ptr::null_mut();
    // SAFETY: the folder's id read during the call; no token; the shell
    // allocates the path into `p` (null on failure).
    let hr = unsafe { SHGetKnownFolderPath(id, 0, core::ptr::null_mut(), &mut p) };
    let path = if hr >= 0 {
        // SAFETY: the shell's NUL-terminated path (or null), copied out
        // before it is freed below.
        unsafe { crate::win::msg::read_nul(p) }
    } else {
        None
    };
    // SAFETY: freed with the allocator the shell used, once (null is
    // allowed).
    unsafe { CoTaskMemFree(p.cast()) };
    path
}

// ---- List views. ----

/// LVM_HITTEST (or LVM_SUBITEMHITTEST with `subitem`) of `info` (its
/// point; the fields the list fills come back), with wParam 0: the list's
/// answer (the item hit, -1 for none) and the filled structure.
pub fn lv_hit_test(lv: HWND, info: LVHITTESTINFO, subitem: bool) -> (isize, LVHITTESTINFO) {
    let mut ht = info;
    let msg = if subitem {
        LVM_SUBITEMHITTEST
    } else {
        LVM_HITTEST
    };
    // SAFETY: both messages' lParam is an LVHITTESTINFO, in and out, with
    // no pointers; sent only to a list view.
    let answer = unsafe {
        crate::win::send_raw(
            crate::win::of_class(lv, crate::win::lv::CLASS),
            msg,
            0,
            (&raw mut ht) as isize,
        )
    };
    (answer, ht)
}

/// LVM_SETICONSPACING: the spacing of icons in icon view, `cx` × `cy`
/// (-1 for the default); the spacing before, as (cx, cy).
pub fn lv_set_icon_spacing(lv: HWND, cx: i32, cy: i32) -> (i32, i32) {
    let l = (cx as u16 as isize) | ((cy as u16 as isize) << 16);
    // SAFETY: LVM_SETICONSPACING's lParam is a packed size; sent only to a
    // list view.
    let v = unsafe {
        crate::win::send_raw(
            crate::win::of_class(lv, crate::win::lv::CLASS),
            LVM_SETICONSPACING,
            0,
            l,
        )
    } as usize;
    (
        i32::from(crate::win::loword(v)),
        i32::from(crate::win::hiword(v)),
    )
}

// ---- Edit boxes. ----

/// Posts EM_SETSEL to `e` (characters `start` up to `end`), to be done
/// once what runs now has returned. Its parameters are integers, and below
/// WM_USER it means the same to every window.
pub fn post_edit_sel(e: HWND, start: i32, end: i32) -> bool {
    // SAFETY: EM_SETSEL's parameters are integers.
    unsafe { PostMessageW(e, EM_SETSEL, start as isize as usize, end as isize) != 0 }
}

// ---- Files. ----

/// The file system's attributes of `path` (UTF-16, a NUL added if it has
/// none); `None` when it is not there.
pub fn file_attributes(path: &[u16]) -> Option<u32> {
    let z = if path.last() == Some(&0) {
        path.to_vec()
    } else {
        crate::win::wz_units(path)
    };
    // SAFETY: a NUL-terminated path read during the call.
    let attrs = unsafe { GetFileAttributesW(z.as_ptr()) };
    (attrs != INVALID_FILE_ATTRIBUTES).then_some(attrs)
}

// ---- Image lists (comctl32 checks the handles it is given). ----

/// ImageList_Create: an empty image list of `cx` × `cy` images (ILC_*
/// `flags`); null if none could be made.
pub fn imagelist_create(
    cx: i32,
    cy: i32,
    flags: IMAGELIST_CREATION_FLAGS,
    initial: i32,
    grow: i32,
) -> HIMAGELIST {
    // SAFETY: no pointers.
    unsafe { ImageList_Create(cx, cy, flags, initial, grow) }
}

/// ImageList_Destroy; false if `il` is not an image list.
pub fn imagelist_destroy(il: HIMAGELIST) -> bool {
    // SAFETY: no pointers.
    unsafe { ImageList_Destroy(il) != 0 }
}

/// `IID_IImageList`, which `SHGetImageList` hands the system lists out as.
const IID_IMAGE_LIST: GUID = GUID::from_u128(0x46eb5926_582e_4017_9fdf_e8998daa0950);

/// The shell's system image list `which` (SHIL_*), which lives as long as
/// the process: the reference handed out is kept, never released. `None`
/// if the shell has no such list.
pub fn system_image_list(which: i32) -> Option<HIMAGELIST> {
    let mut ppv: *mut core::ffi::c_void = core::ptr::null_mut();
    // SAFETY: an out-pointer for the interface the IID names; the list it
    // gives is an IImageList, which is an HIMAGELIST.
    let hr = unsafe { SHGetImageList(which, &IID_IMAGE_LIST, &mut ppv) };
    (hr >= 0 && !ppv.is_null()).then_some(ppv as HIMAGELIST)
}

// ---- Drawing. ----

/// GetTextExtentExPointW of `text` in the DC's font: how many of its units
/// fit in `max` pixels, and the size of all of it.
pub fn text_fit(hdc: HDC, text: &[u16], max: i32) -> (i32, SIZE) {
    let mut fit = 0;
    let mut size = SIZE::default();
    let len = i32::try_from(text.len()).unwrap_or(i32::MAX);
    // SAFETY: `len` units of `text`, read; two out-values; no array of
    // partial extents.
    unsafe {
        GetTextExtentExPointW(
            hdc,
            text.as_ptr(),
            len,
            max,
            &mut fit,
            core::ptr::null_mut(),
            &mut size,
        )
    };
    (fit, size)
}

/// A 32-bit top-down DIB section of `cx` × `cy` pixels in a memory DC of
/// its own, both deleted when this drops, to draw an image list's image
/// into and read back. Neither the DC nor the bitmap is handed out, so
/// nothing else can deselect or delete the bitmap whose pixels this reads.
pub struct DibCanvas {
    dc: HDC,
    dib: HBITMAP,
    old: HGDIOBJ,
    bits: *mut u32,
    cx: i32,
    count: usize,
}

impl DibCanvas {
    /// `None` for an empty size, or if GDI could not make it.
    pub fn new(cx: i32, cy: i32) -> Option<DibCanvas> {
        if cx <= 0 || cy <= 0 {
            return None;
        }
        let count = usize::try_from(cx)
            .ok()?
            .checked_mul(usize::try_from(cy).ok()?)?;
        let bi = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: cx,
                // Top-down.
                biHeight: -cy,
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut bits: *mut core::ffi::c_void = core::ptr::null_mut();
        // SAFETY: a complete BITMAPINFO for a 32-bit DIB, read; an out
        // pointer for its pixels; no file mapping.
        let dib = unsafe {
            CreateDIBSection(
                core::ptr::null_mut(),
                &bi,
                DIB_RGB_COLORS,
                &mut bits,
                core::ptr::null_mut(),
                0,
            )
        };
        if dib.is_null() || bits.is_null() {
            if !dib.is_null() {
                crate::win::gdi::delete_object(dib);
            }
            return None;
        }
        let dc = crate::win::gdi::create_compatible_dc(core::ptr::null_mut());
        let old = crate::win::gdi::select(dc, dib);
        Some(DibCanvas {
            dc,
            dib,
            old,
            bits: bits.cast(),
            cx,
            count,
        })
    }

    /// Sets every pixel to `value` (0x00RRGGBB, alpha in the top byte).
    pub fn fill(&mut self, value: u32) {
        crate::win::gdi::flush();
        // SAFETY: the DIB section's `count` pixels, which this owns and
        // nothing else reaches (its DC and bitmap are not handed out); GDI's
        // drawing into them is done (flushed above).
        unsafe { core::slice::from_raw_parts_mut(self.bits, self.count) }.fill(value);
    }

    /// ImageList_DrawIndirect of `params` into the canvas (its `hdcDst` is
    /// the canvas's own, set here); false if the list did not draw.
    pub fn draw_image(&mut self, params: &IMAGELISTDRAWPARAMS) -> bool {
        let mut p = *params;
        p.cbSize = size_of::<IMAGELISTDRAWPARAMS>() as u32;
        p.hdcDst = self.dc;
        // SAFETY: an in-struct with its size and no pointers (handles and
        // values only).
        unsafe { ImageList_DrawIndirect(&p) != 0 }
    }

    /// The pixels, `cx` to a row from the top, lent to `f` once what was
    /// drawn is in them.
    pub fn with_pixels<R>(&self, f: impl FnOnce(&[u32]) -> R) -> R {
        crate::win::gdi::flush();
        // SAFETY: as in `fill`; the slice is borrowed from `self` for the
        // call, so the canvas outlives it, and nothing draws into the
        // canvas meanwhile (only `&mut self` methods do).
        f(unsafe { core::slice::from_raw_parts(self.bits, self.count) })
    }

    /// The width in pixels.
    pub fn width(&self) -> i32 {
        self.cx
    }
}

impl Drop for DibCanvas {
    fn drop(&mut self) {
        crate::win::gdi::select(self.dc, self.old);
        crate::win::gdi::delete_dc(self.dc);
        crate::win::gdi::delete_object(self.dib);
    }
}

/// EM_CHARFROMPOS at `pt` (client coordinates) as the box answers it: the
/// character index in the low word and the line in the high word, -1 for
/// a point outside it.
pub fn edit_char_from_pos(e: HWND, pt: POINT) -> isize {
    let packed = ((pt.y as u32 & 0xffff) << 16) | (pt.x as u32 & 0xffff);
    // SAFETY: EM_CHARFROMPOS's lParam is a packed point.
    unsafe { crate::win::send_raw(e, EM_CHARFROMPOS, 0, packed as isize) }
}

/// Replaces the selection with `text` (UTF-16; a NUL is added if missing).
pub fn edit_replace_sel_wide(e: HWND, text: &[u16], can_undo: bool) {
    let z = if text.last() == Some(&0) {
        text.to_vec()
    } else {
        crate::win::wz_units(text)
    };
    // SAFETY: a NUL-terminated string read during the call.
    unsafe { crate::win::send_raw(e, EM_REPLACESEL, usize::from(can_undo), z.as_ptr() as isize) };
}

/// Where this thread's caret is, in its window's client coordinates.
pub fn caret_pos() -> POINT {
    let mut p = POINT::default();
    // SAFETY: an out-struct.
    unsafe { GetCaretPos(&mut p) };
    p
}
