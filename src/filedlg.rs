//! The system's Open and Save dialogs (the common item dialogs), for the
//! Import... and Export... buttons of goz-gui's organizers. A Save dialog
//! can carry one check box (Everything's "Export selection only").
//!
//! windows-sys has no COM interfaces, so the few methods used are reached
//! through hand-written vtables; the slots not called are kept as plain
//! pointer-sized fields so the ones that are sit at their places.

use std::ffi::OsString;
use std::os::windows::ffi::OsStringExt;
use std::path::PathBuf;

use windows_sys::Win32::Foundation::HWND;
use windows_sys::Win32::System::Com::{CLSCTX_INPROC_SERVER, CoCreateInstance, CoTaskMemFree};
use windows_sys::Win32::UI::Shell::Common::COMDLG_FILTERSPEC;
use windows_sys::Win32::UI::Shell::{
    FOS_FILEMUSTEXIST, FOS_FORCEFILESYSTEM, FOS_NOCHANGEDIR, FOS_OVERWRITEPROMPT,
    FOS_PATHMUSTEXIST, FileOpenDialog, FileSaveDialog, SIGDN_FILESYSPATH,
};
use windows_sys::core::{GUID, HRESULT};

use crate::wide::wz;

const IID_IFILEOPENDIALOG: GUID = GUID::from_u128(0xd57c7288_d4ad_4768_be02_9d969532d960);
const IID_IFILESAVEDIALOG: GUID = GUID::from_u128(0x84bccd23_5fde_4cdb_aea4_af64b83d78ab);
const IID_IFILEDIALOGCUSTOMIZE: GUID = GUID::from_u128(0xe6fdd21a_163f_4975_9c8c_a69f1ba37034);
/// The check box's id among the dialog's added controls.
const CHECK_ID: u32 = 1;
/// CDCONTROLSTATEF: shown and usable, or shown greyed.
const CDCS_VISIBLE: u32 = 2;
const CDCS_ENABLED_VISIBLE: u32 = 3;

type Raw = *mut core::ffi::c_void;
/// A vtable slot goz-gui does not call.
type Unused = usize;

#[repr(C)]
struct UnknownVtbl {
    query_interface: unsafe extern "system" fn(Raw, *const GUID, *mut Raw) -> HRESULT,
    add_ref: Unused,
    release: unsafe extern "system" fn(Raw) -> u32,
}

/// IFileDialog (IFileOpenDialog and IFileSaveDialog start with it).
#[repr(C)]
struct FileDialogVtbl {
    unknown: UnknownVtbl,
    show: unsafe extern "system" fn(Raw, HWND) -> HRESULT,
    set_file_types: unsafe extern "system" fn(Raw, u32, *const COMDLG_FILTERSPEC) -> HRESULT,
    set_file_type_index: unsafe extern "system" fn(Raw, u32) -> HRESULT,
    get_file_type_index: unsafe extern "system" fn(Raw, *mut u32) -> HRESULT,
    advise: Unused,
    unadvise: Unused,
    set_options: unsafe extern "system" fn(Raw, u32) -> HRESULT,
    get_options: unsafe extern "system" fn(Raw, *mut u32) -> HRESULT,
    set_default_folder: Unused,
    set_folder: Unused,
    get_folder: Unused,
    get_current_selection: Unused,
    set_file_name: unsafe extern "system" fn(Raw, *const u16) -> HRESULT,
    get_file_name: Unused,
    set_title: unsafe extern "system" fn(Raw, *const u16) -> HRESULT,
    set_ok_button_label: Unused,
    set_file_name_label: Unused,
    get_result: unsafe extern "system" fn(Raw, *mut Raw) -> HRESULT,
    add_place: Unused,
    set_default_extension: unsafe extern "system" fn(Raw, *const u16) -> HRESULT,
}

/// IShellItem, up to GetDisplayName.
#[repr(C)]
struct ShellItemVtbl {
    unknown: UnknownVtbl,
    bind_to_handler: Unused,
    get_parent: Unused,
    get_display_name: unsafe extern "system" fn(Raw, i32, *mut *mut u16) -> HRESULT,
}

/// IFileDialogCustomize, up to GetCheckButtonState.
#[repr(C)]
struct CustomizeVtbl {
    unknown: UnknownVtbl,
    enable_open_drop_down: Unused,
    add_menu: Unused,
    add_push_button: Unused,
    add_combo_box: Unused,
    add_radio_button_list: Unused,
    add_check_button: unsafe extern "system" fn(Raw, u32, *const u16, i32) -> HRESULT,
    add_edit_box: Unused,
    add_separator: Unused,
    add_text: Unused,
    set_control_label: Unused,
    get_control_state: Unused,
    set_control_state: unsafe extern "system" fn(Raw, u32, u32) -> HRESULT,
    get_edit_box_text: Unused,
    set_edit_box_text: Unused,
    get_check_button_state: unsafe extern "system" fn(Raw, u32, *mut i32) -> HRESULT,
}

/// A COM object this module holds a reference to, released on drop.
struct Com(Raw);

impl Com {
    /// The object's vtable, as `V`.
    ///
    /// # Safety
    /// The object implements the interface whose vtable `V` lays out.
    unsafe fn vtbl<V>(&self) -> &V {
        // SAFETY: a COM object starts with a pointer to its vtable, and the
        // caller promises its layout.
        unsafe { &**(self.0 as *const *const V) }
    }

    /// The object's `iid` interface.
    fn query(&self, iid: &GUID) -> Option<Com> {
        let mut out: Raw = core::ptr::null_mut();
        // SAFETY: every interface starts with IUnknown's three methods.
        let hr = unsafe { (self.vtbl::<UnknownVtbl>().query_interface)(self.0, iid, &mut out) };
        (hr >= 0 && !out.is_null()).then_some(Com(out))
    }
}

impl Drop for Com {
    fn drop(&mut self) {
        // SAFETY: the reference this wrapper holds, given back once.
        unsafe { (self.vtbl::<UnknownVtbl>().release)(self.0) };
    }
}

/// A kind of file the dialog lists: its name and pattern
/// (`("CSV Files (*.csv)", "*.csv")`).
pub type Kind = (&'static str, &'static str);

/// The files goz-gui's lists are kept in.
pub const CSV: &[Kind] = &[("CSV Files (*.csv)", "*.csv"), ("All Files (*.*)", "*.*")];

/// A check box under a Save dialog's file name.
pub struct Check<'a> {
    pub label: &'a str,
    pub checked: bool,
    pub enabled: bool,
}

/// What the dialog asks for.
struct Ask<'a> {
    save: bool,
    title: &'a str,
    file_name: &'a str,
    kinds: &'a [Kind],
    extension: &'a str,
    check: Option<Check<'a>>,
    /// The file type listed first (from 1).
    kind: u32,
}

/// Asks for a file to read, owned by `owner`; `None` when cancelled.
pub fn open(owner: HWND, title: &str, kinds: &[Kind], extension: &str) -> Option<PathBuf> {
    let ask = Ask {
        save: false,
        title,
        file_name: "",
        kinds,
        extension,
        check: None,
        kind: 1,
    };
    show(owner, &ask).map(|(path, _, _)| path)
}

/// Asks where to write a file, suggesting `file_name`; with `check`, also
/// whether that box was ticked. `None` when cancelled. Writing over a file
/// is asked about first.
pub fn save(
    owner: HWND,
    title: &str,
    file_name: &str,
    kinds: &[Kind],
    extension: &str,
    check: Option<Check<'_>>,
) -> Option<(PathBuf, bool)> {
    let ask = Ask {
        save: true,
        title,
        file_name,
        kinds,
        extension,
        check,
        kind: 1,
    };
    show(owner, &ask).map(|(path, checked, _)| (path, checked))
}

/// Asks where to write a file of one of `kinds`, suggesting `file_name` and
/// the kind at `kind` (from 0); the path and the kind chosen (from 0).
/// `None` when cancelled. Writing over a file is asked about first.
pub fn save_kind(
    owner: HWND,
    title: &str,
    file_name: &str,
    kinds: &[Kind],
    kind: usize,
) -> Option<(PathBuf, usize)> {
    let extension = kinds.get(kind).map_or("", |k| k.1.trim_start_matches("*."));
    let ask = Ask {
        save: true,
        title,
        file_name,
        kinds,
        extension,
        check: None,
        kind: kind as u32 + 1,
    };
    show(owner, &ask).map(|(path, _, kind)| (path, kind))
}

/// [`save_kind`] with a check box (W6: the File List Editor's Export...,
/// with Everything's "Export selection only"): the path, whether the box
/// was ticked, and the file type chosen (from 0).
pub fn save_kind_check(
    owner: HWND,
    title: &str,
    file_name: &str,
    kinds: &[Kind],
    kind: usize,
    check: Check<'_>,
) -> Option<(PathBuf, bool, usize)> {
    let extension = kinds.get(kind).map_or("", |k| k.1.trim_start_matches("*."));
    let ask = Ask {
        save: true,
        title,
        file_name,
        kinds,
        extension,
        check: Some(check),
        kind: kind as u32 + 1,
    };
    show(owner, &ask)
}

/// The path chosen, whether the check box was ticked, and the file type
/// chosen (from 0).
fn show(owner: HWND, ask: &Ask<'_>) -> Option<(PathBuf, bool, usize)> {
    let (clsid, iid) = if ask.save {
        (&FileSaveDialog, &IID_IFILESAVEDIALOG)
    } else {
        (&FileOpenDialog, &IID_IFILEOPENDIALOG)
    };
    let mut raw: Raw = core::ptr::null_mut();
    // SAFETY: creates the system's dialog object on this (COM) thread.
    let hr = unsafe {
        CoCreateInstance(
            clsid,
            core::ptr::null_mut(),
            CLSCTX_INPROC_SERVER,
            iid,
            &mut raw,
        )
    };
    if hr < 0 || raw.is_null() {
        return None;
    }
    let dialog = Com(raw);
    // The strings and the kinds' table live until the dialog has closed.
    let title = wz(ask.title);
    let file_name = wz(ask.file_name);
    let extension = wz(ask.extension);
    let names: Vec<(Vec<u16>, Vec<u16>)> = ask.kinds.iter().map(|&(n, p)| (wz(n), wz(p))).collect();
    let specs: Vec<COMDLG_FILTERSPEC> = names
        .iter()
        .map(|(n, p)| COMDLG_FILTERSPEC {
            pszName: n.as_ptr(),
            pszSpec: p.as_ptr(),
        })
        .collect();
    let label = ask.check.as_ref().map(|c| wz(c.label));
    // SAFETY: IFileOpenDialog and IFileSaveDialog begin with IFileDialog;
    // every pointer passed lives across its call.
    unsafe {
        let v = dialog.vtbl::<FileDialogVtbl>();
        let mut options = 0u32;
        (v.get_options)(dialog.0, &mut options);
        options |= FOS_FORCEFILESYSTEM | FOS_NOCHANGEDIR | FOS_PATHMUSTEXIST;
        options |= if ask.save {
            FOS_OVERWRITEPROMPT
        } else {
            FOS_FILEMUSTEXIST
        };
        (v.set_options)(dialog.0, options);
        (v.set_title)(dialog.0, title.as_ptr());
        if !specs.is_empty() {
            (v.set_file_types)(dialog.0, specs.len() as u32, specs.as_ptr());
            (v.set_file_type_index)(dialog.0, ask.kind);
        }
        (v.set_default_extension)(dialog.0, extension.as_ptr());
        if !ask.file_name.is_empty() {
            (v.set_file_name)(dialog.0, file_name.as_ptr());
        }
    }
    let customize = match (&ask.check, &label) {
        (Some(check), Some(label)) => dialog.query(&IID_IFILEDIALOGCUSTOMIZE).inspect(|c| {
            // SAFETY: the object is an IFileDialogCustomize; the label
            // lives across the calls.
            unsafe {
                let cv = c.vtbl::<CustomizeVtbl>();
                (cv.add_check_button)(c.0, CHECK_ID, label.as_ptr(), i32::from(check.checked));
                let state = if check.enabled {
                    CDCS_ENABLED_VISIBLE
                } else {
                    CDCS_VISIBLE
                };
                (cv.set_control_state)(c.0, CHECK_ID, state);
            }
        }),
        _ => None,
    };
    // SAFETY: shows the modal dialog owned by a window of this thread.
    let hr = unsafe { (dialog.vtbl::<FileDialogVtbl>().show)(dialog.0, owner) };
    if hr < 0 {
        // Cancelled (or failed to show).
        return None;
    }
    let checked = customize.as_ref().is_some_and(|c| {
        let mut on = 0i32;
        // SAFETY: an IFileDialogCustomize and a plain out-value.
        unsafe { (c.vtbl::<CustomizeVtbl>().get_check_button_state)(c.0, CHECK_ID, &mut on) };
        on != 0
    });
    let mut item: Raw = core::ptr::null_mut();
    // SAFETY: the dialog's chosen item, an IShellItem this code then owns.
    let hr = unsafe { (dialog.vtbl::<FileDialogVtbl>().get_result)(dialog.0, &mut item) };
    if hr < 0 || item.is_null() {
        return None;
    }
    let item = Com(item);
    let mut kind = 1u32;
    // SAFETY: a plain out-value; the dialog's type index counts from 1.
    unsafe { (dialog.vtbl::<FileDialogVtbl>().get_file_type_index)(dialog.0, &mut kind) };
    let mut name: *mut u16 = core::ptr::null_mut();
    // SAFETY: the item is an IShellItem; the name it gives is a
    // NUL-terminated string from the COM allocator, read and then freed.
    unsafe {
        let hr =
            (item.vtbl::<ShellItemVtbl>().get_display_name)(item.0, SIGDN_FILESYSPATH, &mut name);
        if hr < 0 || name.is_null() {
            return None;
        }
        let len = (0..).take_while(|&i| *name.add(i) != 0).count();
        let path = PathBuf::from(OsString::from_wide(core::slice::from_raw_parts(name, len)));
        CoTaskMemFree(name.cast());
        Some((path, checked, kind.saturating_sub(1) as usize))
    }
}
