//! goz-gui: a native Windows front end for goz.
//!
//! Plain Win32 and common controls, no UI framework: the results list is a
//! virtual list view that holds only the rows near the screen, and searches
//! run on a background thread that talks to gozd's named pipe.

#![windows_subsystem = "windows"]

mod actions;
mod bigicons;
mod bookmark_dialogs;
mod bookmarks;
mod celltip;
mod client;
mod clipboard;
mod cmdline;
mod columns;
mod copytext;
mod csv;
mod dialog;
mod dock;
mod dropdown;
mod export;
mod filedlg;
mod fileops;
mod filetype;
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
mod layout;
mod menu;
mod model;
mod msgbox;
mod options;
mod paint;
mod pane;
mod placement;
mod places;
mod popup;
mod run_count;
mod run_dialog;
mod run_history;
mod run_history_file;
mod searchbox;
mod selection;
mod session;
mod settings;
mod shell;
mod status;
mod store;
mod strip;
mod syntax;
mod testpane;
mod theme;
mod thumbs;
mod tray;
mod typeahead;
mod view;
mod wide;
mod window;
mod zoom;
// Wave 1: each feature declares its modules on its own lines below, so
// their changes never meet (FRAMEWORK.md).

// F1 (preview pane).
mod preview;

// F2 (sidebars).
mod foldertree;
mod sidebars;
mod treeview;

// F3 (tabs and windows).
mod tabs;

// F4 (search tools).
mod advsearch;
mod filter_dialogs;
mod filters;
mod find;
mod goto;
mod searchtools;

// F5 (file tools).
mod filename;
mod filetools;

// F6 (integration and the Options pages).
mod ui_options;
mod url;

use windows_sys::Win32::System::Com::{
    COINIT_APARTMENTTHREADED, COINIT_DISABLE_OLE1DDE, CoInitializeEx,
};
use windows_sys::Win32::UI::Controls::{
    ICC_BAR_CLASSES, ICC_HOTKEY_CLASS, ICC_LISTVIEW_CLASSES, ICC_STANDARD_CLASSES, ICC_TAB_CLASSES,
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
            // the tree view is Organize Bookmarks' folders and Options'
            // pages, the tab Options' page name.
            dwICC: ICC_LISTVIEW_CLASSES
                | ICC_STANDARD_CLASSES
                | ICC_BAR_CLASSES
                | ICC_TREEVIEW_CLASSES
                | ICC_TAB_CLASSES
                | ICC_HOTKEY_CLASS,
        };
        InitCommonControlsEx(&icc);
    }
    theme::init();
    std::process::exit(window::run());
}
