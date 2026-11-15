//! The folder Copy to Folder... and Move to Folder... send the selection
//! to: the system's folder picker (the common item dialog with
//! `FOS_PICKFOLDERS`), titled as Everything's ("Copy To", "Move To"),
//! opening in the folder chosen last.
//!
//! A test names the folder in `GOZ_GUI_PICK_FOLDER` instead
//! (`store::location::pick_folder`): nothing is shown.

use windows::Win32::UI::Shell::{
    FOS_FORCEFILESYSTEM, FOS_NOCHANGEDIR, FOS_PATHMUSTEXIST, FOS_PICKFOLDERS,
};
use windows_sys::Win32::Foundation::HWND;

use crate::store::location;
use crate::win::shell::FileDialog;

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
    // COM is initialised on this (the UI) thread; the dialog runs its own
    // loop while the caller holds no borrow.
    let dialog = FileDialog::open()?;
    let options = dialog.options()?;
    let folders = FOS_PICKFOLDERS | FOS_FORCEFILESYSTEM | FOS_PATHMUSTEXIST | FOS_NOCHANGEDIR;
    if !dialog.set_options(options | folders.0) {
        return None;
    }
    dialog.set_title(title);
    dialog.set_ok_label(ok);
    if let Some(start) = start.filter(|s| super::attributes(s).is_some()) {
        dialog.set_folder(start);
    }
    if !dialog.show(owner) {
        return None;
    }
    dialog.result()?.file_system_path()
}
