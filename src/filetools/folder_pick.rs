//! The folder Copy to Folder... and Move to Folder... send the selection
//! to: the system's folder picker (the common item dialog with
//! `FOS_PICKFOLDERS`), titled as Everything's ("Copy To", "Move To"),
//! opening in the folder chosen last.
//!
//! A test names the folder in `GOZ_GUI_PICK_FOLDER` instead
//! (`store::location::pick_folder`): nothing is shown.

use windows::Win32::Foundation::HWND as WHWND;
use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, CoCreateInstance, CoTaskMemFree};
use windows::Win32::UI::Shell::{
    FOS_FORCEFILESYSTEM, FOS_NOCHANGEDIR, FOS_PATHMUSTEXIST, FOS_PICKFOLDERS, FileOpenDialog,
    IFileOpenDialog, IShellItem, SHCreateItemFromParsingName, SIGDN_FILESYSPATH,
};
use windows::core::PCWSTR;
use windows_sys::Win32::Foundation::HWND;

use crate::store::location;
use crate::wide::{nul, wz};

/// Asks for a folder, over `owner`, titled `title` with `ok` on its button,
/// opening in `start` when it is a folder; the folder chosen, `None` when
/// cancelled.
pub fn pick(owner: HWND, title: &str, ok: &str, start: Option<&[u16]>) -> Option<Vec<u16>> {
    if let Some(folder) = location::pick_folder() {
        return Some(
            folder
                .as_os_str()
                .to_string_lossy()
                .encode_utf16()
                .collect(),
        );
    }
    let (title, ok) = (wz(title), wz(ok));
    // SAFETY: COM is initialised on this (the UI) thread; the dialog runs
    // its own loop while the caller holds no borrow; every string lives
    // across the call that takes it; the path the dialog gives is freed
    // after it is read.
    unsafe {
        let dialog: IFileOpenDialog =
            CoCreateInstance(&FileOpenDialog, None, CLSCTX_INPROC_SERVER).ok()?;
        let options = dialog.GetOptions().ok()?;
        dialog
            .SetOptions(
                options
                    | FOS_PICKFOLDERS
                    | FOS_FORCEFILESYSTEM
                    | FOS_PATHMUSTEXIST
                    | FOS_NOCHANGEDIR,
            )
            .ok()?;
        let _ = dialog.SetTitle(PCWSTR(title.as_ptr()));
        let _ = dialog.SetOkButtonLabel(PCWSTR(ok.as_ptr()));
        if let Some(start) = start.filter(|s| super::attributes(s).is_some()) {
            let z = nul(start);
            if let Ok(folder) =
                SHCreateItemFromParsingName::<_, _, IShellItem>(PCWSTR(z.as_ptr()), None)
            {
                let _ = dialog.SetFolder(&folder);
            }
        }
        dialog.Show(Some(WHWND(owner))).ok()?;
        let item = dialog.GetResult().ok()?;
        let name = item.GetDisplayName(SIGDN_FILESYSPATH).ok()?;
        let path = name.as_wide().to_vec();
        CoTaskMemFree(Some(name.0.cast()));
        Some(path)
    }
}
