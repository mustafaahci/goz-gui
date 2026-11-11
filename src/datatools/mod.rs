//! Wave 4's data and file tools (stream G1): file lists (`crate::filelist`)
//! and their menu items, Edit > Advanced > Advanced Copy to Folder... and
//! Advanced Move to Folder..., Tools > Import and Export Settings..., and
//! Help > Troubleshooting Information. The window's side is
//! `window/datatools.rs`.

pub mod settings_io;
pub mod troubleshoot;

/// The commands, from 4600 (past the closed tabs' ids, 4000..=4255, and
/// clear of the features' 3000..=3999).
pub mod cmd {
    /// The first of them.
    pub const FIRST: u16 = 4600;
    /// File > Open File List... (Ctrl+O), Close File List and Recent File
    /// Lists > Clear Recent File Lists.
    pub const OPEN_FILE_LIST: u16 = FIRST;
    pub const CLOSE_FILE_LIST: u16 = FIRST + 1;
    pub const CLEAR_RECENT_FILE_LISTS: u16 = FIRST + 2;
    /// Edit > Advanced > Advanced Copy to Folder... and Advanced Move to
    /// Folder....
    pub const ADVANCED_COPY_TO: u16 = FIRST + 3;
    pub const ADVANCED_MOVE_TO: u16 = FIRST + 4;
    /// Tools > Import and Export Settings....
    pub const IMPORT_EXPORT_SETTINGS: u16 = FIRST + 5;
    /// Help > Troubleshooting Information.
    pub const TROUBLESHOOTING: u16 = FIRST + 6;
    /// Recent File Lists' entries: this plus the entry's place, at most
    /// `filelist::settings::MENU_MAX`.
    pub const RECENT_FIRST: u16 = FIRST + 50;
    /// One past the last.
    pub const END: u16 = FIRST + 100;

    const _: () = {
        assert!(TROUBLESHOOTING < RECENT_FIRST);
        assert!(RECENT_FIRST as usize + crate::filelist::settings::MENU_MAX <= END as usize);
        assert!(crate::menu::TAB_ENTRY_FIRST as usize + crate::menu::TAB_ENTRIES <= FIRST as usize);
        assert!((END as usize) <= crate::menu::SHELL_FIRST as usize);
    };
}

/// The main window's WM_APP messages: `WM_APP + 250..300`.
pub const MSG_FIRST: u32 = windows_sys::Win32::UI::WindowsAndMessaging::WM_APP + 250;
pub const MSG_END: u32 = MSG_FIRST + 50;
