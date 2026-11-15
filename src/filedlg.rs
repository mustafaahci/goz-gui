//! The system's Open and Save dialogs (the common item dialogs), for the
//! Import... and Export... buttons of goz-gui's organizers. A Save dialog
//! can carry one check box (Everything's "Export selection only").
//!
//! The dialog is the `windows` crate's IFileOpenDialog or IFileSaveDialog,
//! reached through `win`'s [`FileDialog`], which keeps the strings it is
//! given for as long as the dialog.

use std::ffi::OsString;
use std::os::windows::ffi::OsStringExt;
use std::path::PathBuf;

use windows_sys::Win32::Foundation::HWND;
use windows_sys::Win32::UI::Shell::{
    FOS_FILEMUSTEXIST, FOS_FORCEFILESYSTEM, FOS_NOCHANGEDIR, FOS_OVERWRITEPROMPT, FOS_PATHMUSTEXIST,
};

use crate::win::ext::b9::shell::FileDialog;

/// The check box's id among the dialog's added controls.
const CHECK_ID: u32 = 1;
/// CDCONTROLSTATEF: shown and usable, or shown greyed.
const CDCS_VISIBLE: u32 = 2;
const CDCS_ENABLED_VISIBLE: u32 = 3;

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
    let dialog = if ask.save {
        FileDialog::save()
    } else {
        FileDialog::open()
    }?;
    // The strings and the kinds' table live until the dialog has gone (the
    // dialog keeps them).
    let mut options = dialog.options().unwrap_or(0);
    options |= FOS_FORCEFILESYSTEM | FOS_NOCHANGEDIR | FOS_PATHMUSTEXIST;
    options |= if ask.save {
        FOS_OVERWRITEPROMPT
    } else {
        FOS_FILEMUSTEXIST
    };
    dialog.set_options(options);
    dialog.set_title(ask.title);
    if !ask.kinds.is_empty() {
        dialog.set_file_types(ask.kinds);
        dialog.set_file_type_index(ask.kind);
    }
    dialog.set_default_extension(ask.extension);
    if !ask.file_name.is_empty() {
        dialog.set_file_name(ask.file_name);
    }
    let customize = match &ask.check {
        Some(check) => dialog.customize().inspect(|c| {
            c.add_check_button(CHECK_ID, check.label, check.checked);
            let state = if check.enabled {
                CDCS_ENABLED_VISIBLE
            } else {
                CDCS_VISIBLE
            };
            c.set_control_state(CHECK_ID, state);
        }),
        None => None,
    };
    // Modal, owned by a window of this thread.
    if !dialog.show(owner) {
        // Cancelled (or failed to show).
        return None;
    }
    let checked = customize
        .as_ref()
        .is_some_and(|c| c.check_button_state(CHECK_ID));
    let item = dialog.result()?;
    // The dialog's type index counts from 1.
    let kind = dialog.file_type_index().unwrap_or(1);
    let name = item.file_system_path()?;
    let path = PathBuf::from(OsString::from_wide(&name));
    Some((path, checked, kind.saturating_sub(1) as usize))
}
