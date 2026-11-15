//! The shell: its items and item ID lists, the common item dialogs, file
//! operations and their progress, shortcuts, property stores and the
//! taskbar's jump list, known folders, ShellExecute, and a few shell calls
//! that take or give memory of its own.
//!
//! Each COM object is owned by a wrapper that releases it when it drops,
//! and reached only through the wrapper's methods, which build every
//! pointer they pass and copy out (and free) every string the shell
//! allocates. They are made on a thread with COM (`win::com::Apartment`;
//! making one fails without), and stay on it: no wrapper is `Send`.

mod dialog;
mod fileop;
mod link;

pub use dialog::{Customize, FileDialog};
pub use fileop::{FileOperation, ItemOp, ProgressSink};
pub use link::{JumpList, ObjectArray, ObjectCollection, PropertyStore, ShellLink, save_shortcut};

use core::ptr::NonNull;
use std::os::windows::io::{FromRawHandle, OwnedHandle};

use windows::Win32::System::Com::CoTaskMemFree;
use windows::Win32::UI::Shell::{
    BHID_SFUIObject, IExtractIconW, IShellItem, IShellItemArray, SHCreateItemFromParsingName,
    SIGDN, SIGDN_FILESYSPATH, SetCurrentProcessExplicitAppUserModelID,
};
use windows::core::{PCWSTR, PWSTR, Result};
use windows_sys::Win32::Foundation::{HWND, LocalFree};
use windows_sys::Win32::UI::Shell::Common::ITEMIDLIST;
use windows_sys::Win32::UI::Shell::{
    CommandLineToArgvW, ILClone, ILCreateFromPathW, ILFindChild, ILFindLastID, ILFree, ILIsEqual,
    ILIsParent, ILRemoveLastID, SEE_MASK_NOASYNC, SEE_MASK_NOCLOSEPROCESS, SHARD_PATHW,
    SHAddToRecentDocs, SHCreateDataObject, SHELLEXECUTEINFOW, SHELLFLAGSTATE, SHELLSTATEA,
    SHFILEINFOW, SHGFI_FLAGS, SHGFI_PIDL, SHGetFileInfoW, SHGetSetSettings, SHGetSettings,
    SHMultiFileProperties, SHOP_FILEPATH, SHObjectProperties, SHOpenFolderAndSelectItems, SSF_MASK,
    ShellExecuteExW,
};
use windows_sys::Win32::UI::Shell::{
    NIF_MESSAGE, NOTIFY_ICON_MESSAGE, NOTIFYICONDATAW, Shell_NotifyIconW,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{SHOW_WINDOW_CMD, WM_APP};
use windows_sys::core::GUID;

use super::icon::Icon;
use super::msg::APP_END;
use super::process::Process;
use super::{len_u32, nul_ended, sys, until_nul, wz};

/// A copy of a string the shell allocated, which is freed.
fn take_string(s: PWSTR) -> Option<Vec<u16>> {
    if s.is_null() {
        return None;
    }
    // SAFETY: a NUL-terminated string from the COM allocator, read up to
    // its NUL, then freed once (nothing else holds it).
    unsafe {
        let units = s.as_wide().to_vec();
        CoTaskMemFree(Some(s.0 as *const core::ffi::c_void));
        Some(units)
    }
}

/// Gives the process the AppUserModelID `id` (before it shows a
/// window), under which the taskbar groups its windows.
pub fn set_app_user_model_id(id: &str) -> Result<()> {
    let id = wz(id);
    // SAFETY: a NUL-terminated string read during the call.
    unsafe { SetCurrentProcessExplicitAppUserModelID(PCWSTR(id.as_ptr())) }
}

/// A known folder's path (`FOLDERID_*`), without a NUL; `None` if the
/// shell does not know it.
pub fn known_folder(id: &GUID) -> Option<Vec<u16>> {
    use windows_sys::Win32::UI::Shell::SHGetKnownFolderPath;
    let mut raw: *mut u16 = core::ptr::null_mut();
    // SAFETY: the folder's id, no token; the shell allocates the
    // string, which is read up to its NUL and freed with CoTaskMemFree
    // as documented (on failure too).
    unsafe {
        let hr = SHGetKnownFolderPath(id, 0, core::ptr::null_mut(), &mut raw);
        let path = (hr >= 0 && !raw.is_null()).then(|| PWSTR(raw).as_wide().to_vec());
        windows_sys::Win32::System::Com::CoTaskMemFree(raw.cast());
        path
    }
}

/// Tells the shell that a file association changed (SHCNE_ASSOCCHANGED).
pub fn notify_associations_changed() {
    use windows_sys::Win32::UI::Shell::{SHCNE_ASSOCCHANGED, SHCNF_IDLIST, SHChangeNotify};
    // SAFETY: a notification with no items (both null).
    unsafe {
        SHChangeNotify(
            SHCNE_ASSOCCHANGED as i32,
            SHCNF_IDLIST,
            core::ptr::null(),
            core::ptr::null(),
        )
    };
}

/// A shell item: a file or folder as the shell sees it.
pub struct ShellItem(IShellItem);

impl ShellItem {
    /// The item at `path` (a path, or another name the shell parses);
    /// `None` when there is none.
    pub fn from_parsing_name(path: &[u16]) -> Option<ShellItem> {
        let path = nul_ended(path);
        // SAFETY: a NUL-terminated name read during the call; no bind
        // context.
        unsafe { SHCreateItemFromParsingName(PCWSTR(path.as_ptr()), None) }
            .ok()
            .map(ShellItem)
    }

    /// Its name of kind `kind` (SIGDN_*).
    pub fn display_name(&self, kind: SIGDN) -> Option<Vec<u16>> {
        // SAFETY: the item this holds; the name it gives is the
        // caller's, taken (copied and freed) here.
        let name = unsafe { self.0.GetDisplayName(kind) }.ok()?;
        take_string(name)
    }

    /// Its path in the file system; `None` for an item that has none.
    pub fn file_system_path(&self) -> Option<Vec<u16>> {
        self.display_name(SIGDN_FILESYSPATH)
    }

    /// Its icon handler (as Explorer binds it, `BHID_SFUIObject`).
    pub fn extract_icon(&self) -> Option<ExtractIcon> {
        // SAFETY: the item this holds; no bind context; the handler's
        // id, a static GUID.
        unsafe {
            self.0
                .BindToHandler::<_, IExtractIconW>(None, &BHID_SFUIObject)
        }
        .ok()
        .map(ExtractIcon)
    }
}

/// A list of shell items (IShellItemArray), as an Open dialog that takes
/// several gives them.
pub struct ShellItems(IShellItemArray);

impl ShellItems {
    /// How many there are; `None` if the list would not say.
    pub fn count(&self) -> Option<u32> {
        // SAFETY: the list this holds.
        unsafe { self.0.GetCount() }.ok()
    }

    /// Item `i`.
    pub fn get(&self, i: u32) -> Option<ShellItem> {
        // SAFETY: the list this holds; an index, which the list checks.
        unsafe { self.0.GetItemAt(i) }.ok().map(ShellItem)
    }
}

/// A file's icon handler (IExtractIconW).
pub struct ExtractIcon(IExtractIconW);

impl ExtractIcon {
    /// Where the icon is, asked with GIL_* `flags`: the file (`"*"` for
    /// an entry of the system image lists), its index there, and GIL_*
    /// flags about it.
    pub fn location(&self, flags: u32) -> Option<(Vec<u16>, i32, u32)> {
        let mut location = [0u16; 1024];
        let (mut index, mut out_flags) = (0i32, 0u32);
        // SAFETY: the handler this holds; a buffer with its length (the
        // slice) and two out-values.
        unsafe {
            self.0
                .GetIconLocation(flags, &mut location, &mut index, &mut out_flags)
        }
        .ok()?;
        Some((until_nul(&location).to_vec(), index, out_flags))
    }

    /// The icon its location named (`file` and `index`), at `size`
    /// pixels (the low word; the high word is the small icon's, not
    /// made); `None` if the handler made none.
    pub fn extract(&self, file: &[u16], index: u32, size: u32) -> Option<Icon> {
        let file = nul_ended(file);
        let mut icon = windows::Win32::UI::WindowsAndMessaging::HICON::default();
        // SAFETY: the handler this holds; a NUL-terminated file name and
        // an out-handle for the large icon, none for the small one.
        let _ = unsafe {
            self.0
                .Extract(PCWSTR(file.as_ptr()), index, Some(&mut icon), None, size)
        };
        Icon::own(icon.0)
    }
}

/// SHGetFileInfoW of the file at `path` with `attributes` and SHGFI_*
/// `flags`: what it answered (the system image list with
/// SHGFI_SYSICONINDEX, nonzero for success otherwise) and the structure it
/// filled. SHGFI_PIDL is not taken: `path` is a path. With SHGFI_ICON the
/// icon in the structure is the caller's to destroy.
pub fn file_info(path: &[u16], attributes: u32, flags: SHGFI_FLAGS) -> (usize, SHFILEINFOW) {
    let path = nul_ended(path);
    let mut info = SHFILEINFOW::default();
    // SAFETY: a NUL-terminated path (SHGFI_PIDL masked off, so it is read
    // as one) and an out-struct of the size given.
    let answer = unsafe {
        SHGetFileInfoW(
            path.as_ptr(),
            attributes,
            &mut info,
            size_of::<SHFILEINFOW>() as u32,
            flags & !SHGFI_PIDL,
        )
    };
    (answer, info)
}

/// The shell's settings `mask` asks for (SHGetSettings, SSF_*).
pub fn settings(mask: SSF_MASK) -> SHELLFLAGSTATE {
    let mut s = SHELLFLAGSTATE::default();
    // SAFETY: an out-struct of plain flags.
    unsafe { SHGetSettings(&mut s, mask) };
    s
}

/// The shell's state `mask` names (SSF_*), read (SHGetSetSettings).
pub fn state(mask: SSF_MASK) -> SHELLSTATEA {
    let mut state = SHELLSTATEA::default();
    // SAFETY: an out-struct (no pointers in it); only read, not set.
    unsafe { SHGetSetSettings(&mut state, mask, 0) };
    state
}

/// How CommandLineToArgvW splits `line` (as a new process's arguments).
pub fn command_line_to_argv(line: &str) -> Vec<String> {
    let w = wz(line);
    let mut n = 0;
    // SAFETY: a NUL-terminated line; the array is read for its `n`
    // NUL-terminated strings, then freed with LocalFree as documented.
    unsafe {
        let list = CommandLineToArgvW(w.as_ptr(), &mut n);
        if list.is_null() {
            return Vec::new();
        }
        let out = (0..usize::try_from(n).unwrap_or(0))
            .map(|i| {
                let p = *list.add(i);
                let len = (0..).take_while(|&j| *p.add(j) != 0).count();
                String::from_utf16_lossy(core::slice::from_raw_parts(p, len))
            })
            .collect();
        LocalFree(list.cast());
        out
    }
}

/// Puts the file at `path` in Windows' recent items (SHAddToRecentDocs);
/// Windows leaves it out when the user turned that off.
pub fn add_to_recent_docs(path: &[u16]) {
    let path = nul_ended(path);
    // SAFETY: a NUL-terminated path, as SHARD_PATHW says.
    unsafe { SHAddToRecentDocs(SHARD_PATHW as u32, path.as_ptr().cast()) };
}

/// Shows the shell's Properties for the file at `path`, owned by `hwnd`
/// (SHObjectProperties); false if it could not.
pub fn object_properties(hwnd: HWND, path: &[u16]) -> bool {
    let path = nul_ended(path);
    // SAFETY: a NUL-terminated path; no page named.
    unsafe { SHObjectProperties(hwnd, SHOP_FILEPATH as u32, path.as_ptr(), core::ptr::null()) != 0 }
}

/// An item ID list the shell made (an absolute PIDL), freed when this
/// drops.
pub struct Pidl(NonNull<ITEMIDLIST>);

impl Pidl {
    /// The item at `path` (ILCreateFromPathW, which may read the disk);
    /// `None` if there is none.
    pub fn from_path(path: &[u16]) -> Option<Pidl> {
        let path = nul_ended(path);
        // SAFETY: a NUL-terminated path; the list made is this one's.
        NonNull::new(unsafe { ILCreateFromPathW(path.as_ptr()) }).map(Pidl)
    }

    fn ptr(&self) -> *const ITEMIDLIST {
        self.0.as_ptr()
    }

    /// A copy (ILClone); `None` if there was no memory for it.
    pub fn try_clone(&self) -> Option<Pidl> {
        // SAFETY: a valid list; the copy is the new one's.
        NonNull::new(unsafe { ILClone(self.ptr()) }).map(Pidl)
    }

    /// Takes its last ID off, making it its parent's; false at the
    /// desktop's (empty) list, which has none.
    pub fn remove_last_id(&mut self) -> bool {
        // SAFETY: a valid list of this one's, shortened in place.
        unsafe { ILRemoveLastID(self.0.as_ptr()) != 0 }
    }

    /// Whether it holds `child` (`immediate`: directly).
    pub fn is_parent_of(&self, child: &Pidl, immediate: bool) -> bool {
        // SAFETY: two valid lists, read during the call.
        unsafe { ILIsParent(self.ptr(), child.ptr(), immediate.into()) != 0 }
    }

    /// Whether it is the same item as `other`.
    pub fn is_equal(&self, other: &Pidl) -> bool {
        // SAFETY: two valid lists, read during the call.
        unsafe { ILIsEqual(self.ptr(), other.ptr()) != 0 }
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

/// IDataObject's interface id.
const IID_IDATA_OBJECT: GUID = GUID::from_u128(0x0000010e_0000_0000_c000_000000000046);

/// A data object the shell made of items (IDataObject), released when this
/// drops. It belongs to this thread's apartment (it is not `Send`).
pub struct DataObject(NonNull<core::ffi::c_void>);

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
        // for the call; no inner object; the interface id and an out-pointer
        // for an IDataObject, whose reference is this one's.
        let hr = unsafe {
            SHCreateDataObject(
                folder.ptr(),
                len_u32(children.len()),
                children.as_ptr(),
                core::ptr::null_mut(),
                &IID_IDATA_OBJECT,
                &mut data,
            )
        };
        if hr < 0 {
            return None;
        }
        NonNull::new(data).map(DataObject)
    }

    /// Shows the shell's Properties for its items (SHMultiFileProperties):
    /// one sheet for them all, which takes its own reference.
    pub fn show_properties(&self) -> bool {
        // SAFETY: a live IDataObject this holds a reference to.
        unsafe { SHMultiFileProperties(self.0.as_ptr(), 0) >= 0 }
    }
}

impl Drop for DataObject {
    fn drop(&mut self) {
        // SAFETY: the reference this holds, given up once (IUnknown's
        // Release, by the interface's drop).
        drop(unsafe {
            <windows_core::IUnknown as windows_core::Interface>::from_raw(self.0.as_ptr())
        });
    }
}

/// What [`execute`] runs, and how.
pub struct Execute<'a> {
    /// The window that owns what the shell shows (null: none).
    pub owner: HWND,
    /// The verb (`None`: the default one).
    pub verb: Option<&'a str>,
    /// The file (or program) to run the verb on.
    pub file: &'a [u16],
    /// The command line passed to a program.
    pub params: Option<&'a str>,
    /// The folder it runs in.
    pub dir: Option<&'a [u16]>,
    /// SW_SHOWNORMAL, SW_HIDE…
    pub show: SHOW_WINDOW_CMD,
    /// Keep the process started (SEE_MASK_NOCLOSEPROCESS), to wait for it.
    pub process: bool,
}

/// ShellExecuteEx, finishing what it does before it returns
/// (SEE_MASK_NOASYNC: the thread may end right after). The process started,
/// when `process` asked for it and there is one; the system's error code
/// when it failed.
pub fn execute(e: &Execute) -> core::result::Result<Option<Process>, u32> {
    let verb = e.verb.map(wz);
    let file = nul_ended(e.file);
    let params = e.params.map(wz);
    let dir = e.dir.map(nul_ended);
    let ptr = |s: Option<&[u16]>| s.map_or(core::ptr::null(), <[u16]>::as_ptr);
    let mut info = SHELLEXECUTEINFOW {
        cbSize: size_of::<SHELLEXECUTEINFOW>() as u32,
        fMask: SEE_MASK_NOASYNC
            | if e.process {
                SEE_MASK_NOCLOSEPROCESS
            } else {
                0
            },
        hwnd: e.owner,
        lpVerb: ptr(verb.as_deref()),
        lpFile: file.as_ptr(),
        lpParameters: ptr(params.as_deref()),
        lpDirectory: ptr(dir.as_deref()),
        nShow: e.show,
        ..Default::default()
    };
    // SAFETY: every string is NUL-terminated (or null) and outlives the
    // call; the mask asks for nothing that reads another field.
    if unsafe { ShellExecuteExW(&mut info) } == 0 {
        return Err(sys::last_error());
    }
    if info.hProcess.is_null() {
        return Ok(None);
    }
    // SAFETY: the process handle SEE_MASK_NOCLOSEPROCESS gave the caller,
    // owned by nothing else.
    let handle = unsafe { OwnedHandle::from_raw_handle(info.hProcess) };
    Ok(Some(Process::own(handle)))
}

/// Shell_NotifyIconW: adds, changes or deletes a notification-area icon as
/// `op` says (NIM_*), described by `d` (its size is set here).
///
/// # Panics
/// If `d` asks for its clicks (NIF_MESSAGE) as a message that is not one of
/// goz-gui's own (`WM_APP` up to the registered ones): the shell posts it
/// with integers, which a system message could take for pointers.
pub fn notify_icon(op: NOTIFY_ICON_MESSAGE, d: &NOTIFYICONDATAW) -> bool {
    let mut d = *d;
    d.cbSize = size_of::<NOTIFYICONDATAW>() as u32;
    if d.uFlags & NIF_MESSAGE == 0 {
        d.uCallbackMessage = 0;
    } else {
        assert!(
            (WM_APP..APP_END).contains(&d.uCallbackMessage),
            "not one of goz-gui's private messages"
        );
    }
    // SAFETY: a complete structure (no pointers in it) with its size.
    unsafe { Shell_NotifyIconW(op, &d) != 0 }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_known_folder_has_a_path() {
        use windows_sys::Win32::UI::Shell::FOLDERID_Windows;
        let path = known_folder(&FOLDERID_Windows).map(|p| String::from_utf16_lossy(&p));
        assert!(path.is_some_and(|p| p.to_lowercase().ends_with("windows")));
    }

    #[test]
    fn an_item_has_its_path() {
        let _com = crate::win::com::Apartment::sta().unwrap();
        let item = ShellItem::from_parsing_name(&wz(r"C:\Windows")).unwrap();
        let path = item
            .file_system_path()
            .map(|p| String::from_utf16_lossy(&p));
        assert_eq!(
            path.as_deref().map(str::to_lowercase).as_deref(),
            Some(r"c:\windows")
        );
        let a = Pidl::from_path(&wz(r"C:\Windows")).unwrap();
        let mut b = Pidl::from_path(&wz(r"C:\Windows\System32")).unwrap();
        assert!(a.is_parent_of(&b, true));
        assert!(b.remove_last_id());
        assert!(a.is_equal(&b) && a.try_clone().is_some_and(|c| c.is_equal(&a)));
    }

    #[test]
    fn command_lines_split_as_windows_splits_them() {
        assert_eq!(
            command_line_to_argv(r#"a.exe "two words" x"#),
            ["a.exe", "two words", "x"]
        );
    }
}
