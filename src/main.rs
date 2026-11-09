//! goz-gui: a native Windows front end for goz.
//!
//! Plain Win32 and common controls, no UI framework: the results list is a
//! virtual list view that holds only the rows near the screen, and searches
//! run on a background thread that talks to gozd's named pipe.

#![windows_subsystem = "windows"]

mod actions;
mod bookmark_dialogs;
mod bookmarks;
mod client;
mod clipboard;
mod cmdline;
mod columns;
mod copytext;
mod csv;
mod dialog;
mod dropdown;
mod filedlg;
mod fileops;
mod filter;
mod format;
mod header;
mod highlight;
mod history;
mod history_dialogs;
mod history_file;
mod hover;
mod icons;
mod instance;
mod integration;
mod json;
mod keys;
mod menu;
mod model;
mod msgbox;
mod options;
mod paint;
mod placement;
mod places;
mod popup;
mod run_count;
mod run_dialog;
mod run_history;
mod run_history_file;
mod searchbox;
mod session;
mod settings;
mod shell;
mod status;
mod store;
mod syntax;
mod theme;
mod tray;
mod wide;
mod window;

use windows_sys::Win32::System::Com::{
    COINIT_APARTMENTTHREADED, COINIT_DISABLE_OLE1DDE, CoInitializeEx,
};
use windows_sys::Win32::UI::Controls::{
    ICC_BAR_CLASSES, ICC_HOTKEY_CLASS, ICC_LISTVIEW_CLASSES, ICC_STANDARD_CLASSES,
    ICC_TREEVIEW_CLASSES, INITCOMMONCONTROLSEX, InitCommonControlsEx,
};

fn main() {
    // SAFETY: once, on the UI thread, before any window exists.
    unsafe {
        CoInitializeEx(
            core::ptr::null(),
            (COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE) as u32,
        );
        let icc = INITCOMMONCONTROLSEX {
            dwSize: size_of::<INITCOMMONCONTROLSEX>() as u32,
            // Bar classes bring the tooltips of the search box buttons;
            // the tree view is Organize Bookmarks' folders.
            dwICC: ICC_LISTVIEW_CLASSES
                | ICC_STANDARD_CLASSES
                | ICC_BAR_CLASSES
                | ICC_TREEVIEW_CLASSES
                | ICC_HOTKEY_CLASS,
        };
        InitCommonControlsEx(&icc);
    }
    theme::init();
    std::process::exit(window::run());
}
