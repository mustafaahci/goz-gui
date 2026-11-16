//! The common item dialog: the system's Open and Save dialogs.

use std::cell::RefCell;

use windows::Win32::Foundation::HWND as WHWND;
use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, CoCreateInstance};
use windows::Win32::UI::Shell::Common::COMDLG_FILTERSPEC;
use windows::Win32::UI::Shell::{
    CDCONTROLSTATEF, FILEOPENDIALOGOPTIONS, FileOpenDialog, FileSaveDialog, IFileDialog,
    IFileDialogCustomize, IFileOpenDialog, IFileSaveDialog,
};
use windows::core::{Interface, PCWSTR};
use windows_sys::Win32::Foundation::HWND;

use super::{ShellItem, ShellItems};
use crate::win::wz;

/// The system's Open or Save dialog (the common item dialog), set up and
/// shown modal. The strings it is given are kept for as long as it is,
/// whether it copies them or not.
pub struct FileDialog {
    // Released before what it was given is freed (fields drop in order).
    dialog: Dialog,
    strings: RefCell<Vec<Vec<u16>>>,
    specs: RefCell<Vec<Vec<COMDLG_FILTERSPEC>>>,
}

enum Dialog {
    Open(IFileOpenDialog),
    Save(IFileSaveDialog),
}

impl FileDialog {
    /// An Open dialog; `None` if it could not be made.
    pub fn open() -> Option<FileDialog> {
        // SAFETY: the system's class, in this process.
        let d = unsafe { CoCreateInstance(&FileOpenDialog, None, CLSCTX_INPROC_SERVER) };
        d.ok().map(|d| FileDialog::of(Dialog::Open(d)))
    }

    /// A Save dialog; `None` if it could not be made.
    pub fn save() -> Option<FileDialog> {
        // SAFETY: as in `open`.
        let d = unsafe { CoCreateInstance(&FileSaveDialog, None, CLSCTX_INPROC_SERVER) };
        d.ok().map(|d| FileDialog::of(Dialog::Save(d)))
    }

    fn of(dialog: Dialog) -> FileDialog {
        FileDialog {
            dialog,
            strings: RefCell::new(Vec::new()),
            specs: RefCell::new(Vec::new()),
        }
    }

    fn get(&self) -> &IFileDialog {
        match &self.dialog {
            Dialog::Open(d) => d,
            Dialog::Save(d) => d,
        }
    }

    /// A NUL-terminated copy of `s`, kept as long as the dialog (its
    /// units never move: each copy is a buffer of its own).
    fn keep(&self, s: &str) -> PCWSTR {
        let mut strings = self.strings.borrow_mut();
        strings.push(wz(s));
        PCWSTR(strings.last().map_or(core::ptr::null(), |v| v.as_ptr()))
    }

    /// Its FOS_* options.
    pub fn options(&self) -> Option<u32> {
        // SAFETY: the dialog this holds.
        unsafe { self.get().GetOptions() }.ok().map(|o| o.0)
    }

    /// Sets its FOS_* options; false if it could not.
    pub fn set_options(&self, options: u32) -> bool {
        // SAFETY: the dialog this holds; flags.
        unsafe { self.get().SetOptions(FILEOPENDIALOGOPTIONS(options)) }.is_ok()
    }

    /// Its title.
    pub fn set_title(&self, title: &str) -> bool {
        let title = self.keep(title);
        // SAFETY: the dialog this holds; a string kept with it.
        unsafe { self.get().SetTitle(title) }.is_ok()
    }

    /// The text of its OK button ("Open", "Save" by default).
    pub fn set_ok_label(&self, label: &str) -> bool {
        let label = self.keep(label);
        // SAFETY: as in `set_title`.
        unsafe { self.get().SetOkButtonLabel(label) }.is_ok()
    }

    /// The kinds of file it lists, each a name and a pattern
    /// (`("CSV Files (*.csv)", "*.csv")`).
    pub fn set_file_types(&self, kinds: &[(&str, &str)]) -> bool {
        let specs: Vec<COMDLG_FILTERSPEC> = kinds
            .iter()
            .map(|&(name, pattern)| COMDLG_FILTERSPEC {
                pszName: self.keep(name),
                pszSpec: self.keep(pattern),
            })
            .collect();
        let mut kept = self.specs.borrow_mut();
        kept.push(specs);
        let specs = kept.last().map_or(&[][..], Vec::as_slice);
        // SAFETY: the dialog this holds; a table whose strings and
        // entries are kept with it.
        unsafe { self.get().SetFileTypes(specs) }.is_ok()
    }

    /// The kind of file listed, from 1.
    pub fn set_file_type_index(&self, index: u32) -> bool {
        // SAFETY: the dialog this holds; a number.
        unsafe { self.get().SetFileTypeIndex(index) }.is_ok()
    }

    /// The kind of file chosen, from 1.
    pub fn file_type_index(&self) -> Option<u32> {
        // SAFETY: the dialog this holds.
        unsafe { self.get().GetFileTypeIndex() }.ok()
    }

    /// The extension a name typed without one gets (no dot).
    pub fn set_default_extension(&self, extension: &str) -> bool {
        let extension = self.keep(extension);
        // SAFETY: as in `set_title`.
        unsafe { self.get().SetDefaultExtension(extension) }.is_ok()
    }

    /// The name in its file name box.
    pub fn set_file_name(&self, name: &str) -> bool {
        let name = self.keep(name);
        // SAFETY: as in `set_title`.
        unsafe { self.get().SetFileName(name) }.is_ok()
    }

    /// The folder it opens in; false if `path` is no item.
    pub fn set_folder(&self, path: &[u16]) -> bool {
        let Some(folder) = ShellItem::from_parsing_name(path) else {
            return false;
        };
        // SAFETY: the dialog this holds; an item it keeps a reference
        // to.
        unsafe { self.get().SetFolder(&folder.0) }.is_ok()
    }

    /// Its added controls (IFileDialogCustomize), for as long as it.
    pub fn customize(&self) -> Option<Customize<'_>> {
        let iface = self.get().cast::<IFileDialogCustomize>().ok()?;
        Some(Customize {
            iface,
            dialog: self,
        })
    }

    /// Shows it modal, owned by `owner`, until it closes; whether an
    /// item was chosen (false when cancelled).
    pub fn show(&self, owner: HWND) -> bool {
        // SAFETY: the dialog this holds; a window handle. It runs a
        // message loop meanwhile, while this wrapper holds no borrow.
        unsafe { self.get().Show(Some(WHWND(owner))) }.is_ok()
    }

    /// The item chosen.
    pub fn result(&self) -> Option<ShellItem> {
        // SAFETY: the dialog this holds; the item it gives is the
        // caller's.
        unsafe { self.get().GetResult() }.ok().map(ShellItem)
    }

    /// The items chosen, in an Open dialog (with FOS_ALLOWMULTISELECT, it
    /// may be several); `None` in a Save dialog.
    pub fn results(&self) -> Option<ShellItems> {
        let Dialog::Open(d) = &self.dialog else {
            return None;
        };
        // SAFETY: the dialog this holds; the list it gives is the
        // caller's.
        unsafe { d.GetResults() }.ok().map(ShellItems)
    }
}

/// The controls added to a [`FileDialog`] (IFileDialogCustomize).
pub struct Customize<'a> {
    iface: IFileDialogCustomize,
    /// Where the labels are kept.
    dialog: &'a FileDialog,
}

impl Customize<'_> {
    /// Adds check box `id`, labelled `label`.
    pub fn add_check_button(&self, id: u32, label: &str, checked: bool) -> bool {
        let label = self.dialog.keep(label);
        // SAFETY: the dialog's interface this holds; a string kept with
        // the dialog.
        unsafe { self.iface.AddCheckButton(id, label, checked) }.is_ok()
    }

    /// Sets control `id`'s CDCS_* state (shown, usable).
    pub fn set_control_state(&self, id: u32, state: u32) -> bool {
        // SAFETY: the dialog's interface this holds; numbers.
        unsafe {
            self.iface
                .SetControlState(id, CDCONTROLSTATEF(state as i32))
        }
        .is_ok()
    }

    /// Whether check box `id` is ticked (false if it could not be
    /// read).
    pub fn check_button_state(&self, id: u32) -> bool {
        // SAFETY: the dialog's interface this holds; a number.
        unsafe { self.iface.GetCheckButtonState(id) }.is_ok_and(|on| on.as_bool())
    }
}
