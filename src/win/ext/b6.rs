//! Batch 6 of the move onto `win`: what the file tools (Advanced Rename,
//! Set File Time, Undo History, the file operations), the File List Editor
//! and the Edit File List Slot dialog call. Text and times (normalization,
//! ordinal comparison, code pages, local time), files (attributes, long
//! names, finds, a handle owned for its times), menus, the dark menu bar's
//! undocumented messages, files dropped on a window, and the shell's COM
//! objects: file operations with a progress sink, shell items, file open
//! dialogs and shortcuts.

use core::cell::RefCell;
use core::mem::ManuallyDrop;

use windows::Win32::Foundation::HWND as WHWND;
use windows::Win32::System::Com::{
    CLSCTX_ALL, CLSCTX_INPROC_SERVER, CoCreateInstance, CoTaskMemFree, IPersistFile,
};
use windows::Win32::UI::Shell::{
    FILEOPENDIALOGOPTIONS, FILEOPERATION_FLAGS, FileOpenDialog, FileOperation as FILE_OPERATION,
    IFileOpenDialog, IFileOperation, IFileOperationProgressSink, IFileOperationProgressSink_Impl,
    IShellItem, IShellItemArray, IShellLinkW, SHCreateItemFromParsingName, SIGDN_FILESYSPATH,
    ShellLink,
};
use windows::core::{HRESULT, Interface, PCWSTR, implement};
use windows_sys::Win32::Foundation::{
    CloseHandle, FILETIME, HANDLE, HWND, INVALID_HANDLE_VALUE, SYSTEMTIME,
};
use windows_sys::Win32::Globalization::{
    CompareStringOrdinal, LCMAP_UPPERCASE, LCMapStringEx, LOCALE_NAME_INVARIANT, NORM_FORM,
    NormalizeString, WideCharToMultiByte,
};
use windows_sys::Win32::Graphics::Gdi::HDC;
use windows_sys::Win32::Storage::FileSystem::{
    CreateFileW, DeleteFileW, FILE_CREATION_DISPOSITION, FILE_FLAGS_AND_ATTRIBUTES,
    FILE_SHARE_MODE, FIND_FIRST_EX_LARGE_FETCH, FindClose, FindExInfoBasic, FindExSearchNameMatch,
    FindFirstFileExW, FindNextFileW, GetFileAttributesExW, GetFileAttributesW,
    GetFileExInfoStandard, GetFileTime, GetLongPathNameW, INVALID_FILE_ATTRIBUTES, SetFileTime,
    WIN32_FILE_ATTRIBUTE_DATA, WIN32_FIND_DATAW,
};
use windows_sys::Win32::System::Time::{
    FileTimeToSystemTime, SystemTimeToFileTime, SystemTimeToTzSpecificLocalTime,
    TzSpecificLocalTimeToSystemTime,
};
use windows_sys::Win32::UI::Controls::DRAWITEMSTRUCT;
use windows_sys::Win32::UI::Shell::{DragAcceptFiles, DragFinish, DragQueryFileW, HDROP};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CreateMenu, DestroyMenu, DrawMenuBar, EnableMenuItem, HMENU, MENU_ITEM_FLAGS,
    MF_POPUP, MF_STRING, SetMenu, WM_APP, WM_DROPFILES,
};

use crate::win::msg::APP_END;
use crate::win::{Msg, parcel, subclass};

/// A length for a call that takes an `i32` count: a longer slice is cut
/// (the call then reads or writes less of it, never more).
fn len_i32(n: usize) -> i32 {
    i32::try_from(n).unwrap_or(i32::MAX)
}

/// `s` NUL-terminated (as it is if it is already).
fn nul_ended(s: &[u16]) -> Vec<u16> {
    if s.last() == Some(&0) {
        s.to_vec()
    } else {
        crate::win::wz_units(s)
    }
}

// Text.

/// NormalizeString: `src` in Unicode normalization form `form`
/// (NormalizationD...) into `out`; the units written (or, for an empty
/// `out`, needed), 0 or less on failure.
pub fn normalize(form: NORM_FORM, src: &[u16], out: &mut [u16]) -> i32 {
    // SAFETY: the source and its length, read; the buffer and its length,
    // written (a null buffer of length 0 asks for the length).
    unsafe {
        NormalizeString(
            form,
            src.as_ptr(),
            len_i32(src.len()),
            if out.is_empty() {
                core::ptr::null_mut()
            } else {
                out.as_mut_ptr()
            },
            len_i32(out.len()),
        )
    }
}

/// CompareStringOrdinal of `a` and `b` (`ignore_case`: as Windows folds
/// case for file names): CSTR_LESS_THAN, CSTR_EQUAL or CSTR_GREATER_THAN, 0
/// on failure.
pub fn compare_ordinal(a: &[u16], b: &[u16], ignore_case: bool) -> i32 {
    // SAFETY: two buffers, each read for its own length.
    unsafe {
        CompareStringOrdinal(
            a.as_ptr(),
            len_i32(a.len()),
            b.as_ptr(),
            len_i32(b.len()),
            ignore_case.into(),
        )
    }
}

/// LCMapStringEx of `src` to upper case as the invariant locale has it,
/// into `dst`; the units written, 0 on failure.
pub fn upper_invariant(src: &[u16], dst: &mut [u16]) -> i32 {
    // SAFETY: the source read and the destination written, each for its
    // own length (LCMAP_UPPERCASE writes units, not bytes); no version
    // information, no sort handle.
    unsafe {
        LCMapStringEx(
            LOCALE_NAME_INVARIANT,
            LCMAP_UPPERCASE,
            src.as_ptr(),
            len_i32(src.len()),
            if dst.is_empty() {
                core::ptr::null_mut()
            } else {
                dst.as_mut_ptr()
            },
            len_i32(dst.len()),
            core::ptr::null(),
            core::ptr::null(),
            0,
        )
    }
}

/// WideCharToMultiByte of `wide` in code page `code_page` (CP_ACP...), no
/// flags, the code page's own default character, into `out`; the bytes
/// written (or, for an empty `out`, needed), 0 on failure.
pub fn wide_to_multi_byte(code_page: u32, wide: &[u16], out: &mut [u8]) -> i32 {
    // SAFETY: the source read for its length; the buffer written for its
    // length (null and 0 ask for the length); no default character and no
    // flag to set, which every code page takes.
    unsafe {
        WideCharToMultiByte(
            code_page,
            0,
            wide.as_ptr(),
            len_i32(wide.len()),
            if out.is_empty() {
                core::ptr::null_mut()
            } else {
                out.as_mut_ptr()
            },
            len_i32(out.len()),
            core::ptr::null(),
            core::ptr::null_mut(),
        )
    }
}

// Times.

/// A local time (the current time zone's, by its rules for that date) as
/// UTC; `None` if Windows will not convert it.
pub fn local_to_utc(local: &SYSTEMTIME) -> Option<SYSTEMTIME> {
    let mut utc = SYSTEMTIME::default();
    // SAFETY: an in-struct and an out-struct; a null time zone is the
    // current one.
    (unsafe { TzSpecificLocalTimeToSystemTime(core::ptr::null(), local, &mut utc) } != 0)
        .then_some(utc)
}

/// A UTC time in the current time zone; `None` if Windows will not convert
/// it.
pub fn utc_to_local(utc: &SYSTEMTIME) -> Option<SYSTEMTIME> {
    let mut local = SYSTEMTIME::default();
    // SAFETY: an in-struct and an out-struct; a null time zone is the
    // current one.
    (unsafe { SystemTimeToTzSpecificLocalTime(core::ptr::null(), utc, &mut local) } != 0)
        .then_some(local)
}

pub fn system_time_to_file_time(st: &SYSTEMTIME) -> Option<FILETIME> {
    let mut ft = FILETIME::default();
    // SAFETY: an in-struct and an out-struct.
    (unsafe { SystemTimeToFileTime(st, &mut ft) } != 0).then_some(ft)
}

pub fn file_time_to_system_time(ft: &FILETIME) -> Option<SYSTEMTIME> {
    let mut st = SYSTEMTIME::default();
    // SAFETY: an in-struct and an out-struct.
    (unsafe { FileTimeToSystemTime(ft, &mut st) } != 0).then_some(st)
}

// Files.

/// GetFileAttributesW of `path`; `None` when there is nothing there.
pub fn file_attributes(path: &[u16]) -> Option<u32> {
    let z = nul_ended(path);
    // SAFETY: a NUL-terminated path, read during the call.
    let a = unsafe { GetFileAttributesW(z.as_ptr()) };
    (a != INVALID_FILE_ATTRIBUTES).then_some(a)
}

/// GetFileAttributesExW of `path` (its attributes, size and times);
/// `None` when it cannot be read.
pub fn file_attribute_data(path: &[u16]) -> Option<WIN32_FILE_ATTRIBUTE_DATA> {
    let z = nul_ended(path);
    let mut data = WIN32_FILE_ATTRIBUTE_DATA::default();
    // SAFETY: a NUL-terminated path; GetFileExInfoStandard fills a
    // WIN32_FILE_ATTRIBUTE_DATA, which `data` is.
    let ok =
        unsafe { GetFileAttributesExW(z.as_ptr(), GetFileExInfoStandard, (&raw mut data).cast()) };
    (ok != 0).then_some(data)
}

/// Deletes the file at `path`; false if it could not (the reason is the
/// thread's last error).
pub fn delete_file(path: &[u16]) -> bool {
    let z = nul_ended(path);
    // SAFETY: a NUL-terminated path.
    unsafe { DeleteFileW(z.as_ptr()) != 0 }
}

/// GetLongPathNameW: `path` with its 8.3 short names expanded, into `buf`;
/// the units written (without the NUL), or the units needed (with it) when
/// `buf` is too short, 0 on failure.
pub fn long_path_name(path: &[u16], buf: &mut [u16]) -> u32 {
    let z = nul_ended(path);
    let cap = u32::try_from(buf.len()).unwrap_or(u32::MAX);
    // SAFETY: a NUL-terminated path, read; a separate buffer of the length
    // given, written.
    unsafe {
        GetLongPathNameW(
            z.as_ptr(),
            if buf.is_empty() {
                core::ptr::null_mut()
            } else {
                buf.as_mut_ptr()
            },
            cap,
        )
    }
}

/// A file or folder opened with CreateFileW, closed when this drops; only
/// made here, so the handle is always one this owns.
pub struct FileHandle(HANDLE);

impl FileHandle {
    /// CreateFileW of `path` for `access` (FILE_READ_ATTRIBUTES...), shared
    /// as `share` says, opened or made as `disposition` says (OPEN_EXISTING...)
    /// with `flags` (FILE_FLAG_BACKUP_SEMANTICS for a folder...); no
    /// security attributes or template. The error is the system's.
    pub fn open(
        path: &[u16],
        access: u32,
        share: FILE_SHARE_MODE,
        disposition: FILE_CREATION_DISPOSITION,
        flags: FILE_FLAGS_AND_ATTRIBUTES,
    ) -> std::io::Result<FileHandle> {
        let z = nul_ended(path);
        // SAFETY: a NUL-terminated path; no security attributes, no
        // template; the handle is owned by what this returns.
        let h = unsafe {
            CreateFileW(
                z.as_ptr(),
                access,
                share,
                core::ptr::null(),
                disposition,
                flags,
                core::ptr::null_mut(),
            )
        };
        if h == INVALID_HANDLE_VALUE || h.is_null() {
            return Err(std::io::Error::last_os_error());
        }
        Ok(FileHandle(h))
    }

    /// The file's times: created, last accessed, last written (UTC).
    pub fn times(&self) -> Option<[FILETIME; 3]> {
        let mut t = [FILETIME::default(); 3];
        let [c, a, w] = &mut t;
        // SAFETY: the handle this owns; three out-structs.
        let ok = unsafe { GetFileTime(self.0, c, a, w) } != 0;
        ok.then_some(t)
    }

    /// Sets the file's times (`None`: leave it as it is); false if it
    /// could not (the reason is the thread's last error).
    pub fn set_times(
        &self,
        created: Option<&FILETIME>,
        accessed: Option<&FILETIME>,
        written: Option<&FILETIME>,
    ) -> bool {
        let ptr = |f: Option<&FILETIME>| f.map_or(core::ptr::null(), |f| f as *const FILETIME);
        // SAFETY: the handle this owns; each time a FILETIME read during the
        // call, or null for "leave it".
        unsafe { SetFileTime(self.0, ptr(created), ptr(accessed), ptr(written)) != 0 }
    }
}

impl Drop for FileHandle {
    fn drop(&mut self) {
        // SAFETY: the handle this owns, closed once.
        unsafe { CloseHandle(self.0) };
    }
}

/// A folder's listing (FindFirstFileExW, basic information), closed when
/// this drops.
pub struct Find(HANDLE);

impl Find {
    /// Starts listing what `pattern` names (`C:\folder\*`), with
    /// FIND_FIRST_EX_LARGE_FETCH when `large_fetch`: the listing and its
    /// first item; `None` when nothing could be listed.
    pub fn first(pattern: &[u16], large_fetch: bool) -> Option<(Find, WIN32_FIND_DATAW)> {
        let z = nul_ended(pattern);
        let mut data = WIN32_FIND_DATAW::default();
        // SAFETY: a NUL-terminated pattern; FindExInfoBasic fills a
        // WIN32_FIND_DATAW, which `data` is; a name search takes no filter.
        let h = unsafe {
            FindFirstFileExW(
                z.as_ptr(),
                FindExInfoBasic,
                (&raw mut data).cast(),
                FindExSearchNameMatch,
                core::ptr::null(),
                if large_fetch {
                    FIND_FIRST_EX_LARGE_FETCH
                } else {
                    0
                },
            )
        };
        if h.is_null() || h == INVALID_HANDLE_VALUE {
            return None;
        }
        Some((Find(h), data))
    }

    /// The listing's next item; `None` at its end (or on failure).
    pub fn next(&mut self) -> Option<WIN32_FIND_DATAW> {
        let mut data = WIN32_FIND_DATAW::default();
        // SAFETY: the find handle this owns; an out-struct.
        (unsafe { FindNextFileW(self.0, &mut data) } != 0).then_some(data)
    }
}

impl Drop for Find {
    fn drop(&mut self) {
        // SAFETY: the find handle this owns, closed once.
        unsafe { FindClose(self.0) };
    }
}

// Menus (user32 checks the menu handles it is given).

/// A new, empty menu bar (CreateMenu); null if it could not be made.
pub fn create_menu() -> HMENU {
    // SAFETY: no pointers.
    unsafe { CreateMenu() }
}

/// Appends the popup menu `popup` to `menu` as an item titled `title`.
pub fn append_popup(menu: HMENU, popup: HMENU, title: &str) -> bool {
    let z = crate::win::wz(title);
    // SAFETY: a string item (MF_STRING) whose text is NUL-terminated and
    // read during the call; MF_POPUP makes the id the popup's handle.
    unsafe { AppendMenuW(menu, MF_POPUP | MF_STRING, popup as usize, z.as_ptr()) != 0 }
}

/// Sets `hwnd`'s menu bar (which goes with the window).
pub fn set_menu(hwnd: HWND, menu: HMENU) -> bool {
    // SAFETY: no pointers.
    unsafe { SetMenu(hwnd, menu) != 0 }
}

/// EnableMenuItem of item `id` with MF_* `flags` (MF_BYCOMMAND |
/// MF_GRAYED...); its state before, -1 if there is no such item.
pub fn enable_menu_item(menu: HMENU, id: u32, flags: MENU_ITEM_FLAGS) -> i32 {
    // SAFETY: no pointers.
    unsafe { EnableMenuItem(menu, id, flags) }
}

/// Draws `hwnd`'s menu bar again.
pub fn draw_menu_bar(hwnd: HWND) -> bool {
    // SAFETY: no pointers.
    unsafe { DrawMenuBar(hwnd) != 0 }
}

/// Destroys a menu (and its submenus).
pub fn destroy_menu(menu: HMENU) -> bool {
    // SAFETY: no pointers.
    unsafe { DestroyMenu(menu) != 0 }
}

// The dark menu bar. The menu bar has no dark theme of its own; Windows
// draws it through these undocumented messages, which let a window paint
// the bar and its items. The layout of their structures has held since
// Windows 10 1809.

pub const WM_UAHDRAWMENU: u32 = 0x0091;
pub const WM_UAHDRAWMENUITEM: u32 = 0x0092;

/// WM_UAHDRAWMENU's structure (UAHMENU).
#[repr(C)]
#[derive(Clone, Copy)]
pub struct UahMenu {
    pub hmenu: HMENU,
    pub hdc: HDC,
    pub flags: u32,
}

/// The item part of WM_UAHDRAWMENUITEM's structure (UAHMENUITEM).
#[repr(C)]
#[derive(Clone, Copy)]
pub struct UahMenuItem {
    pub position: i32,
    pub metrics: [u32; 8],
    pub popup: [u32; 5],
}

/// WM_UAHDRAWMENUITEM's structure (UAHDRAWMENUITEM).
#[repr(C)]
#[derive(Clone, Copy)]
pub struct UahDrawMenuItem {
    pub dis: DRAWITEMSTRUCT,
    pub um: UahMenu,
    pub umi: UahMenuItem,
}

/// A delivered WM_UAHDRAWMENU's UAHMENU, copied out.
pub fn uah_draw_menu(m: &Msg) -> Option<UahMenu> {
    if m.id != WM_UAHDRAWMENU {
        return None;
    }
    // SAFETY: the system delivers WM_UAHDRAWMENU with lParam pointing at a
    // UAHMENU (a message made by goz-gui hands out nothing); copied out.
    unsafe { m.view_lparam::<UahMenu>() }.map(|v| *v)
}

/// A delivered WM_UAHDRAWMENUITEM's UAHDRAWMENUITEM, copied out.
pub fn uah_draw_menu_item(m: &Msg) -> Option<UahDrawMenuItem> {
    if m.id != WM_UAHDRAWMENUITEM {
        return None;
    }
    // SAFETY: the system delivers WM_UAHDRAWMENUITEM with lParam pointing
    // at a UAHDRAWMENUITEM; copied out.
    unsafe { m.view_lparam::<UahDrawMenuItem>() }.map(|v| *v)
}

// Files dropped on a window.

/// The subclass that takes a window's dropped files (see [`accept_files`]).
static DROP_FILES: subclass::Key = subclass::Key::new("files dropped on a window");

thread_local! {
    /// The drops (HDROPs) this thread's queue has handed to a window with
    /// [`DROP_FILES`] that are not read and freed yet. A WM_DROPFILES
    /// carries its drop once, from the queue; the same message sent on
    /// again (`Msg::forward`) carries one freed already, which is not here.
    static LIVE_DROPS: RefCell<Vec<usize>> = const { RefCell::new(Vec::new()) };
    /// Whether this thread watches its queue for drops ([`watch_drops`]).
    static WATCHING: core::cell::Cell<bool> = const { core::cell::Cell::new(false) };
}

/// Notes each WM_DROPFILES this thread takes from its queue for a window
/// with [`DROP_FILES`] in [`LIVE_DROPS`] (a hook for the thread's life);
/// false if the hook could not be set.
fn watch_drops() -> bool {
    if WATCHING.get() {
        return true;
    }
    let hook = crate::win::hook::set(crate::win::hook::Kind::GetMessage, |e| {
        if let crate::win::hook::Event::Got { msg, removed: true } = *e
            && msg.message == WM_DROPFILES
            && subclass::has(msg.hwnd, &DROP_FILES)
        {
            let _ = LIVE_DROPS.try_with(|live| {
                let mut live = live.borrow_mut();
                if !live.contains(&msg.wParam) {
                    live.push(msg.wParam);
                }
            });
        }
        None
    });
    let Some(hook) = hook else {
        return false;
    };
    // Kept until the thread ends (the system takes a thread's hooks out
    // with it).
    core::mem::forget(hook);
    WATCHING.set(true);
    true
}

/// Takes `drop` out of [`LIVE_DROPS`]; whether it was there (and so is a
/// drop the queue handed over, not freed yet).
fn take_live_drop(drop: usize) -> bool {
    LIVE_DROPS
        .try_with(|live| {
            let mut live = live.borrow_mut();
            let at = live.iter().position(|&d| d == drop);
            at.map(|i| live.swap_remove(i)).is_some()
        })
        .unwrap_or(false)
}

/// Lets files be dropped on `hwnd` (DragAcceptFiles). Each drop's paths
/// are sent to `hwnd` as goz-gui's private message `msg`, wParam 0, with a
/// [`parcel`] of the paths (`Vec<Vec<u16>>`, without NULs): the window's
/// handler takes them with `parcel::take`. The drop (its HDROP) is read
/// whole and freed here, as WM_DROPFILES comes, so no handler sees it; a
/// WM_DROPFILES that did not come from the queue (sent on by a handler)
/// is ignored. False if the window could not be subclassed or the queue
/// watched (then no files are accepted).
///
/// # Panics
/// If `msg` is not one of goz-gui's private messages.
pub fn accept_files(hwnd: HWND, msg: u32) -> bool {
    assert!(
        (WM_APP..APP_END).contains(&msg),
        "not one of goz-gui's private messages"
    );
    if !watch_drops() {
        return false;
    }
    let set = subclass::set(hwnd, &DROP_FILES, move |m| {
        if m.id != WM_DROPFILES || !m.is_delivered() {
            return m.def();
        }
        // Only a drop the queue handed over and not freed yet: a copy of
        // this message sent on carries one freed already.
        if !take_live_drop(m.w) {
            return 0;
        }
        let paths = dropped_paths(m.w as HDROP);
        // SAFETY: an HDROP the system handed over with a WM_DROPFILES from
        // the queue, not freed yet (it was live), and freed here once: it is
        // out of the live drops now, so no other call reads or frees it.
        unsafe { DragFinish(m.w as HDROP) };
        parcel::send(m.hwnd, msg, 0, paths);
        0
    });
    if set {
        // SAFETY: no pointers.
        unsafe { DragAcceptFiles(hwnd, 1) };
    }
    set
}

/// The paths of a drop, before it is freed.
fn dropped_paths(drop: HDROP) -> Vec<Vec<u16>> {
    // SAFETY: an HDROP the system handed over with a WM_DROPFILES from the
    // queue, not freed yet (only `accept_files`'s subclass calls this, for
    // a live drop); null buffers ask for counts and lengths, and each
    // buffer is sized by the length DragQueryFileW reports, its NUL
    // included.
    unsafe {
        let n = DragQueryFileW(drop, u32::MAX, core::ptr::null_mut(), 0);
        (0..n)
            .filter_map(|i| {
                let len = DragQueryFileW(drop, i, core::ptr::null_mut(), 0) as usize;
                if len == 0 {
                    return None;
                }
                let mut buf = vec![0u16; len + 1];
                let got = DragQueryFileW(drop, i, buf.as_mut_ptr(), buf.len() as u32) as usize;
                buf.truncate(got.min(len));
                (got > 0).then_some(buf)
            })
            .collect()
    }
}

// The shell's COM objects. Each wrapper holds a reference-counted
// interface of the `windows` crate (not `Send`: it stays on the thread that
// made it); every string a call takes is NUL-terminated here and outlives
// the call, and every string the shell gives is copied and freed here.

/// The shell item at `path` (SHCreateItemFromParsingName); `None` when it
/// is not there.
pub fn shell_item(path: &[u16]) -> Option<IShellItem> {
    let z = nul_ended(path);
    // SAFETY: a NUL-terminated path that outlives the call; no bind context.
    unsafe { SHCreateItemFromParsingName(PCWSTR(z.as_ptr()), None).ok() }
}

/// The file system path of a shell item (SIGDN_FILESYSPATH); `None` for an
/// item that has none.
pub fn shell_item_path(item: &IShellItem) -> Option<Vec<u16>> {
    // SAFETY: the item is a live COM object; the name it gives is a
    // NUL-terminated string from the COM allocator, copied, then freed
    // (once, and not read after).
    unsafe {
        let name = item.GetDisplayName(SIGDN_FILESYSPATH).ok()?;
        if name.is_null() {
            return None;
        }
        let path = name.as_wide().to_vec();
        CoTaskMemFree(Some(name.0.cast()));
        Some(path)
    }
}

/// Saves a shortcut (a ShellLink) to `target` at `path`, starting in
/// `work_dir` when given.
pub fn save_shortcut(
    target: &[u16],
    work_dir: Option<&[u16]>,
    path: &[u16],
) -> windows::core::Result<()> {
    let target = nul_ended(target);
    let work = work_dir.map(nul_ended);
    let path = nul_ended(path);
    // SAFETY: COM objects of this thread, released when this returns; every
    // string is NUL-terminated and outlives the call that takes it.
    unsafe {
        let link: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER)?;
        link.SetPath(PCWSTR(target.as_ptr()))?;
        if let Some(w) = &work {
            link.SetWorkingDirectory(PCWSTR(w.as_ptr()))?;
        }
        link.cast::<IPersistFile>()?
            .Save(PCWSTR(path.as_ptr()), true)?;
        Ok(())
    }
}

/// The shell's file operation (IFileOperation): items queued, then done at
/// once, with the shell's progress, questions and undo record. The names
/// queued are kept here until it drops (after the operation is released),
/// so the operation may read them whenever it does its work.
pub struct FileOperation {
    op: IFileOperation,
    names: RefCell<Vec<Vec<u16>>>,
}

impl FileOperation {
    /// A new operation (CoCreateInstance, any context); this thread needs
    /// COM.
    pub fn new() -> windows::core::Result<FileOperation> {
        // SAFETY: no pointers but the class id, a constant.
        let op: IFileOperation = unsafe { CoCreateInstance(&FILE_OPERATION, None, CLSCTX_ALL) }?;
        Ok(FileOperation {
            op,
            names: RefCell::new(Vec::new()),
        })
    }

    /// A name to pass to the operation: kept with it, or null for `None`.
    fn name(&self, name: Option<&[u16]>) -> PCWSTR {
        let Some(name) = name else {
            return PCWSTR::null();
        };
        let mut names = self.names.borrow_mut();
        names.push(nul_ended(name));
        // The buffer stays where it is when the list grows: only the list's
        // own vector moves.
        PCWSTR(names.last().map_or(core::ptr::null(), |n| n.as_ptr()))
    }

    /// The window that owns the operation's progress and questions.
    pub fn set_owner_window(&self, owner: HWND) -> windows::core::Result<()> {
        // SAFETY: a window handle, which the shell checks.
        unsafe { self.op.SetOwnerWindow(WHWND(owner)) }
    }

    /// FOF_* and FOFX_* flags.
    pub fn set_operation_flags(&self, flags: u32) -> windows::core::Result<()> {
        // SAFETY: flags, no pointers.
        unsafe { self.op.SetOperationFlags(FILEOPERATION_FLAGS(flags)) }
    }

    /// Tells `sink` of each item as it is done; the cookie that
    /// [`FileOperation::unadvise`] takes.
    pub fn advise(&self, sink: &IFileOperationProgressSink) -> windows::core::Result<u32> {
        // SAFETY: a live sink, which the operation keeps a reference to.
        unsafe { self.op.Advise(sink) }
    }

    pub fn unadvise(&self, cookie: u32) -> windows::core::Result<()> {
        // SAFETY: a cookie, no pointers.
        unsafe { self.op.Unadvise(cookie) }
    }

    /// Queues a rename of `item` to `name`.
    pub fn rename_item(&self, item: &IShellItem, name: &[u16]) -> windows::core::Result<()> {
        let n = self.name(Some(name));
        // SAFETY: a live item; a NUL-terminated name kept with the
        // operation; no sink of the item's own.
        unsafe {
            self.op
                .RenameItem(item, n, None::<&IFileOperationProgressSink>)
        }
    }

    /// Queues a move of `item` into `folder`, as `name` when given.
    pub fn move_item(
        &self,
        item: &IShellItem,
        folder: &IShellItem,
        name: Option<&[u16]>,
    ) -> windows::core::Result<()> {
        let n = self.name(name);
        // SAFETY: live items; a name kept with the operation, or null.
        unsafe {
            self.op
                .MoveItem(item, folder, n, None::<&IFileOperationProgressSink>)
        }
    }

    /// Queues a copy of `item` into `folder`, as `name` when given.
    pub fn copy_item(
        &self,
        item: &IShellItem,
        folder: &IShellItem,
        name: Option<&[u16]>,
    ) -> windows::core::Result<()> {
        let n = self.name(name);
        // SAFETY: live items; a name kept with the operation, or null.
        unsafe {
            self.op
                .CopyItem(item, folder, n, None::<&IFileOperationProgressSink>)
        }
    }

    /// Queues the deletion of `item` (to the Recycle Bin, as the flags say).
    pub fn delete_item(&self, item: &IShellItem) -> windows::core::Result<()> {
        // SAFETY: a live item.
        unsafe {
            self.op
                .DeleteItem(item, None::<&IFileOperationProgressSink>)
        }
    }

    /// Queues a new item `name` in `folder`, with FILE_ATTRIBUTE_*
    /// `attributes` (FILE_ATTRIBUTE_DIRECTORY: a folder), from no template.
    pub fn new_item(
        &self,
        folder: &IShellItem,
        attributes: u32,
        name: &[u16],
    ) -> windows::core::Result<()> {
        let n = self.name(Some(name));
        // SAFETY: a live folder; a name kept with the operation; no
        // template.
        unsafe {
            self.op.NewItem(
                folder,
                attributes,
                n,
                PCWSTR::null(),
                None::<&IFileOperationProgressSink>,
            )
        }
    }

    /// Does what is queued (its windows run their own message loops).
    pub fn perform_operations(&self) -> windows::core::Result<()> {
        // SAFETY: no pointers; the operation stays alive through the call
        // (`&self` holds it).
        unsafe { self.op.PerformOperations() }
    }

    /// Whether any of it was stopped or skipped.
    pub fn any_operations_aborted(&self) -> windows::core::Result<bool> {
        // SAFETY: no pointers.
        unsafe { self.op.GetAnyOperationsAborted() }.map(|b| b.as_bool())
    }
}

/// Which kind of item a file operation's progress sink reports done.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ItemOp {
    Rename,
    Move,
    Copy,
    Delete,
    New,
}

/// What a progress sink hands each item done to: the kind, the item (for a
/// new item, the folder it went in), the item it is now, and how it went.
type OnPost = Box<dyn Fn(ItemOp, Option<&IShellItem>, Option<&IShellItem>, HRESULT)>;

/// A value used only on the thread that made it. COM calls an object that
/// is agile (as `#[implement]` makes it) from whatever thread it is on, and
/// may release it from any; so each use checks the thread first, and the
/// value is dropped only on its own thread (left alone on another, where
/// its drop could touch what is not `Send`).
struct OnThread<T> {
    owner: u32,
    value: ManuallyDrop<T>,
}

impl<T> OnThread<T> {
    fn new(value: T) -> OnThread<T> {
        OnThread {
            owner: crate::win::sys::thread_id(),
            value: ManuallyDrop::new(value),
        }
    }

    /// The value, on its own thread (a thread id is reused only once its
    /// thread has ended, and with it everything else of that thread that
    /// the value could share).
    fn get(&self) -> Option<&T> {
        (crate::win::sys::thread_id() == self.owner).then_some(&*self.value)
    }
}

impl<T> Drop for OnThread<T> {
    fn drop(&mut self) {
        // A thread id is reused only once its thread has ended, and with it
        // everything else of that thread that the value could share.
        if crate::win::sys::thread_id() == self.owner {
            // SAFETY: dropped once, here, on the thread that made it.
            unsafe { ManuallyDrop::drop(&mut self.value) };
        }
    }
}

/// An IFileOperationProgressSink that hands each item done (PostRenameItem,
/// PostMoveItem...) to a closure.
#[implement(IFileOperationProgressSink)]
struct ProgressSink {
    on_post: OnThread<OnPost>,
}

impl ProgressSink {
    fn post(&self, op: ItemOp, item: Option<&IShellItem>, new: Option<&IShellItem>, hr: HRESULT) {
        if let Some(f) = self.on_post.get() {
            f(op, item, new, hr);
        }
    }
}

/// A progress sink for [`FileOperation::advise`] that calls `on_post` for
/// each item an operation has done: the kind, the item, the item it is now
/// (`None` when there is none), and how it went. `on_post` runs only on
/// this thread (the operation's), and is dropped here.
pub fn progress_sink(
    on_post: impl Fn(ItemOp, Option<&IShellItem>, Option<&IShellItem>, HRESULT) + 'static,
) -> IFileOperationProgressSink {
    ProgressSink {
        on_post: OnThread::new(Box::new(on_post)),
    }
    .into()
}

type R = windows::core::Result<()>;
type ItemRef<'a> = windows::core::Ref<'a, IShellItem>;

#[allow(non_snake_case, reason = "the interface's method names")]
impl IFileOperationProgressSink_Impl for ProgressSink_Impl {
    fn StartOperations(&self) -> R {
        Ok(())
    }
    fn FinishOperations(&self, _hr: HRESULT) -> R {
        Ok(())
    }
    fn PreRenameItem(&self, _f: u32, _item: ItemRef<'_>, _name: &PCWSTR) -> R {
        Ok(())
    }
    fn PostRenameItem(
        &self,
        _f: u32,
        item: ItemRef<'_>,
        _name: &PCWSTR,
        hr: HRESULT,
        new: ItemRef<'_>,
    ) -> R {
        self.post(ItemOp::Rename, item.as_ref(), new.as_ref(), hr);
        Ok(())
    }
    fn PreMoveItem(&self, _f: u32, _item: ItemRef<'_>, _to: ItemRef<'_>, _name: &PCWSTR) -> R {
        Ok(())
    }
    fn PostMoveItem(
        &self,
        _f: u32,
        item: ItemRef<'_>,
        _to: ItemRef<'_>,
        _name: &PCWSTR,
        hr: HRESULT,
        new: ItemRef<'_>,
    ) -> R {
        self.post(ItemOp::Move, item.as_ref(), new.as_ref(), hr);
        Ok(())
    }
    fn PreCopyItem(&self, _f: u32, _item: ItemRef<'_>, _to: ItemRef<'_>, _name: &PCWSTR) -> R {
        Ok(())
    }
    fn PostCopyItem(
        &self,
        _f: u32,
        item: ItemRef<'_>,
        _to: ItemRef<'_>,
        _name: &PCWSTR,
        hr: HRESULT,
        new: ItemRef<'_>,
    ) -> R {
        self.post(ItemOp::Copy, item.as_ref(), new.as_ref(), hr);
        Ok(())
    }
    fn PreDeleteItem(&self, _f: u32, _item: ItemRef<'_>) -> R {
        Ok(())
    }
    fn PostDeleteItem(&self, _f: u32, item: ItemRef<'_>, hr: HRESULT, new: ItemRef<'_>) -> R {
        self.post(ItemOp::Delete, item.as_ref(), new.as_ref(), hr);
        Ok(())
    }
    fn PreNewItem(&self, _f: u32, _folder: ItemRef<'_>, _name: &PCWSTR) -> R {
        Ok(())
    }
    fn PostNewItem(
        &self,
        _f: u32,
        folder: ItemRef<'_>,
        _name: &PCWSTR,
        _template: &PCWSTR,
        _attrs: u32,
        hr: HRESULT,
        new: ItemRef<'_>,
    ) -> R {
        self.post(ItemOp::New, folder.as_ref(), new.as_ref(), hr);
        Ok(())
    }
    fn UpdateProgress(&self, _total: u32, _done: u32) -> R {
        Ok(())
    }
    fn ResetTimer(&self) -> R {
        Ok(())
    }
    fn PauseTimer(&self) -> R {
        Ok(())
    }
    fn ResumeTimer(&self) -> R {
        Ok(())
    }
}

/// The common item dialog that opens files or picks folders
/// (IFileOpenDialog). The strings it is given are kept here until it drops.
pub struct OpenDialog {
    dialog: IFileOpenDialog,
    strings: RefCell<Vec<Vec<u16>>>,
}

impl OpenDialog {
    /// A new dialog (CoCreateInstance, in process); this thread needs COM.
    pub fn new() -> Option<OpenDialog> {
        // SAFETY: no pointers but the class id, a constant.
        let dialog: IFileOpenDialog =
            unsafe { CoCreateInstance(&FileOpenDialog, None, CLSCTX_INPROC_SERVER) }.ok()?;
        Some(OpenDialog {
            dialog,
            strings: RefCell::new(Vec::new()),
        })
    }

    /// A string to pass to the dialog, kept with it.
    fn keep(&self, s: &str) -> PCWSTR {
        let mut strings = self.strings.borrow_mut();
        strings.push(crate::win::wz(s));
        PCWSTR(strings.last().map_or(core::ptr::null(), |s| s.as_ptr()))
    }

    /// Its FOS_* options.
    pub fn options(&self) -> Option<FILEOPENDIALOGOPTIONS> {
        // SAFETY: no pointers.
        unsafe { self.dialog.GetOptions() }.ok()
    }

    pub fn set_options(&self, options: FILEOPENDIALOGOPTIONS) -> bool {
        // SAFETY: flags, no pointers.
        unsafe { self.dialog.SetOptions(options) }.is_ok()
    }

    pub fn set_title(&self, title: &str) -> bool {
        let t = self.keep(title);
        // SAFETY: a NUL-terminated string kept with the dialog.
        unsafe { self.dialog.SetTitle(t) }.is_ok()
    }

    /// The text of its OK button.
    pub fn set_ok_label(&self, label: &str) -> bool {
        let t = self.keep(label);
        // SAFETY: a NUL-terminated string kept with the dialog.
        unsafe { self.dialog.SetOkButtonLabel(t) }.is_ok()
    }

    /// The folder it opens in.
    pub fn set_folder(&self, folder: &IShellItem) -> bool {
        // SAFETY: a live item.
        unsafe { self.dialog.SetFolder(folder) }.is_ok()
    }

    /// Shows it, owned by `owner`, until it closes (its own message loop
    /// runs meanwhile); false when it was cancelled or failed.
    pub fn show(&self, owner: HWND) -> bool {
        // SAFETY: a window handle, which the shell checks; the dialog stays
        // alive through the call (`&self` holds it).
        unsafe { self.dialog.Show(Some(WHWND(owner))) }.is_ok()
    }

    /// The item chosen.
    pub fn result(&self) -> Option<IShellItem> {
        // SAFETY: no pointers.
        unsafe { self.dialog.GetResult() }.ok()
    }

    /// The items chosen (with FOS_ALLOWMULTISELECT).
    pub fn results(&self) -> Option<ShellItems> {
        // SAFETY: no pointers.
        unsafe { self.dialog.GetResults() }.ok().map(ShellItems)
    }
}

/// A list of shell items (IShellItemArray).
pub struct ShellItems(IShellItemArray);

impl ShellItems {
    /// How many there are; `None` if the list would not say.
    pub fn count(&self) -> Option<u32> {
        // SAFETY: no pointers.
        unsafe { self.0.GetCount() }.ok()
    }

    /// Item `i`.
    pub fn get(&self, i: u32) -> Option<IShellItem> {
        // SAFETY: an index, which the list checks.
        unsafe { self.0.GetItemAt(i) }.ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_is_compared_and_mapped_as_windows_does() {
        let w = |s: &str| s.encode_utf16().collect::<Vec<u16>>();
        assert_eq!(
            compare_ordinal(&w("Abc"), &w("aBC"), true),
            windows_sys::Win32::Globalization::CSTR_EQUAL
        );
        let mut out = [0u16; 3];
        assert_eq!(upper_invariant(&w("abc"), &mut out), 3);
        assert_eq!(out, *w("ABC").as_slice());
        let mut d = [0u16; 8];
        let n = normalize(
            windows_sys::Win32::Globalization::NormalizationD,
            &w("é"),
            &mut d,
        );
        assert_eq!(n, 2);
        assert_eq!(d[0], u16::from(b'e'));
        let need = wide_to_multi_byte(0, &w("hi"), &mut []);
        assert_eq!(need, 2);
    }

    /// An HDROP of `paths`, as the shell makes one.
    fn hdrop(paths: &[&str]) -> HDROP {
        use windows_sys::Win32::Foundation::POINT;
        use windows_sys::Win32::System::Memory::{
            GMEM_MOVEABLE, GMEM_ZEROINIT, GlobalAlloc, GlobalLock, GlobalUnlock,
        };
        use windows_sys::Win32::UI::Shell::DROPFILES;
        let mut units: Vec<u16> = Vec::new();
        for p in paths {
            units.extend(p.encode_utf16());
            units.push(0);
        }
        units.push(0);
        let head = size_of::<DROPFILES>();
        let bytes = head + units.len() * 2;
        // SAFETY: a new block of `bytes`, locked while a DROPFILES and then
        // the paths are written into it.
        unsafe {
            let mem = GlobalAlloc(GMEM_MOVEABLE | GMEM_ZEROINIT, bytes);
            assert!(!mem.is_null());
            let p = GlobalLock(mem).cast::<u8>();
            assert!(!p.is_null());
            p.cast::<DROPFILES>().write_unaligned(DROPFILES {
                pFiles: head as u32,
                pt: POINT::default(),
                fNC: 0,
                fWide: 1,
            });
            core::ptr::copy_nonoverlapping(
                units.as_ptr().cast::<u8>(),
                p.add(head),
                units.len() * 2,
            );
            GlobalUnlock(mem);
            mem
        }
    }

    static SENT_ON: subclass::Key = subclass::Key::new("test: drops sent on");

    #[test]
    fn a_drop_is_read_once_even_when_its_message_is_sent_on() {
        use crate::win::class::{self, Class, Create};
        use crate::win::queue;
        use std::cell::Cell;
        use std::rc::Rc;
        use windows_sys::Win32::UI::WindowsAndMessaging::{HWND_MESSAGE, PM_REMOVE, PostMessageW};
        const CLASS: &str = "goz-gui test drops";
        const DROPPED: u32 = WM_APP + 9;
        let _ = Class::new(CLASS).register();
        let got = Rc::new(RefCell::new(Vec::new()));
        let g = Rc::clone(&got);
        let hwnd = class::create(
            &Create {
                ex_style: 0,
                class: CLASS,
                title: "",
                style: 0,
                rect: (0, 0, 0, 0),
                parent: HWND_MESSAGE,
                menu: core::ptr::null_mut(),
            },
            move |m| {
                if m.id == DROPPED {
                    g.borrow_mut().push(parcel::take::<Vec<Vec<u16>>>(m));
                    return 0;
                }
                m.def()
            },
        );
        assert!(!hwnd.is_null());
        assert!(accept_files(hwnd, DROPPED));
        // A handler before it sends each drop on to the window again (once)
        // before passing it on: the drop is read and freed by the first.
        let sending = Rc::new(Cell::new(false));
        subclass::set(hwnd, &SENT_ON, move |m| {
            if m.id == WM_DROPFILES && !sending.replace(true) {
                m.forward(m.hwnd);
                sending.set(false);
            }
            m.def()
        });
        let drop = hdrop(&[r"C:\a.txt", r"C:\b"]);
        // SAFETY: WM_DROPFILES with a drop of this thread's, handed over.
        assert!(unsafe { PostMessageW(hwnd, WM_DROPFILES, drop as usize, 0) } != 0);
        while let Some(q) = queue::peek(hwnd, (0, 0), PM_REMOVE) {
            queue::dispatch(&q);
        }
        let w = |s: &str| s.encode_utf16().collect::<Vec<u16>>();
        assert_eq!(*got.borrow(), vec![Some(vec![w(r"C:\a.txt"), w(r"C:\b")])]);
        crate::win::destroy(hwnd);
    }

    #[test]
    fn a_value_on_its_thread_is_had_there_only() {
        let v = std::sync::Arc::new(OnThread::new(5));
        assert_eq!(v.get(), Some(&5));
        let other = std::sync::Arc::clone(&v);
        let seen = std::thread::spawn(move || other.get().copied())
            .join()
            .unwrap();
        assert_eq!(seen, None);
    }
}
