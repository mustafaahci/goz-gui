//! Keyboard shortcuts: the window's accelerator table, and the keys only the
//! results list or the search box takes.
//!
//! The accelerator table sees every key of the window before the focused
//! control does, so it holds only keys that mean the same wherever the focus
//! is. Keys the search box needs for its text (Ctrl+A, Ctrl+C, Shift+Insert,
//! Ctrl+Backspace and the like) mean something else in the list; those are
//! the list's own, handled by its subclass through [`list_command`].
//!
//! The tables here are goz-gui's defaults. Options > Keyboard rebinds them
//! (`crate::keymap`): what is in force is asked of `keymap::current`.

use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    VK_ADD, VK_BACK, VK_DELETE, VK_ESCAPE, VK_F1, VK_F2, VK_F3, VK_F4, VK_F5, VK_F6, VK_F11,
    VK_HOME, VK_INSERT, VK_LEFT, VK_NUMPAD0, VK_OEM_4, VK_OEM_6, VK_OEM_MINUS, VK_OEM_PLUS,
    VK_RETURN, VK_RIGHT, VK_TAB,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    ACCEL, CreateAcceleratorTableW, DestroyAcceleratorTable, FALT, FCONTROL, FSHIFT, FVIRTKEY,
    HACCEL,
};

use crate::{keymap, menu, searchtools};

pub const NONE: u8 = 0;
pub const CTRL: u8 = FCONTROL;
pub const SHIFT: u8 = FSHIFT;
pub const ALT: u8 = FALT;

const fn key(c: u8) -> u16 {
    c as u16
}

/// The window's shortcuts: modifiers, virtual key, command.
pub const ACCELERATORS: &[(u8, u16, u16)] = &[
    (CTRL | SHIFT, key(b'Q'), menu::EXIT),
    // Everything 1.5's three (Ctrl+F is Edit > Find..., F4's).
    (ALT, key(b'D'), menu::FOCUS_SEARCH),
    (CTRL, key(b'E'), menu::FOCUS_SEARCH),
    (CTRL, key(b'L'), menu::FOCUS_SEARCH),
    (NONE, VK_F5, menu::REFRESH),
    (CTRL, key(b'1'), menu::SORT_NAME),
    (CTRL, key(b'2'), menu::SORT_PATH),
    (CTRL, key(b'3'), menu::SORT_SIZE),
    (CTRL, key(b'6'), menu::SORT_DATE),
    // Everything 1.5's views.
    (CTRL | SHIFT, key(b'1'), menu::VIEW_EXTRA_LARGE),
    (CTRL | SHIFT, key(b'2'), menu::VIEW_LARGE),
    (CTRL | SHIFT, key(b'3'), menu::VIEW_MEDIUM),
    (CTRL | SHIFT, key(b'6'), menu::VIEW_DETAILS),
    (CTRL, key(b'I'), menu::MATCH_CASE),
    (CTRL, key(b'U'), menu::MATCH_PATH),
    (ALT, VK_RETURN, menu::PROPERTIES),
    (NONE, VK_F1, menu::HELP),
    (CTRL, VK_F1, menu::ABOUT),
    (ALT, VK_LEFT, menu::NAV_BACK),
    (ALT, VK_RIGHT, menu::NAV_FORWARD),
    (ALT, VK_HOME, menu::NAV_HOME),
    (CTRL, key(b'H'), menu::FIND_SEARCH_HISTORY),
    (CTRL | SHIFT, key(b'H'), menu::ORGANIZE_SEARCH_HISTORY),
    (CTRL, key(b'D'), menu::BOOKMARK_ADD),
    (CTRL, key(b'B'), menu::BOOKMARK_FIND),
    (CTRL | SHIFT, key(b'B'), menu::BOOKMARK_ORGANIZE),
    // Everything 1.5's second key for Organize Bookmarks.
    (CTRL | SHIFT, key(b'O'), menu::BOOKMARK_ORGANIZE),
    (NONE, VK_F4, menu::NEXT_SELECTED),
    (SHIFT, VK_F4, menu::PREV_SELECTED),
    (CTRL, key(b'J'), menu::RUN_HISTORY),
    (CTRL, key(b'P'), menu::OPTIONS),
    // Everything's (and Explorer's) Ctrl+Num +.
    (CTRL, VK_ADD, menu::SIZE_ALL_COLUMNS_TO_FIT),
    // Everything's File > Export..., Search > Next and Previous Filter, and
    // View > Zoom (`view_zoom_*_keys`: Ctrl+=, Ctrl+-, Ctrl+0 and Ctrl+Num0).
    (CTRL, key(b'S'), menu::EXPORT),
    (CTRL, VK_OEM_6, menu::NEXT_FILTER),
    (CTRL, VK_OEM_4, menu::PREV_FILTER),
    (CTRL, VK_OEM_PLUS, menu::ZOOM_IN),
    (CTRL, VK_OEM_MINUS, menu::ZOOM_OUT),
    (CTRL, key(b'0'), menu::ZOOM_RESET),
    (CTRL, VK_NUMPAD0, menu::ZOOM_RESET),
    // Everything 1.5's Window | Next Frame and Previous Frame, and View |
    // Close Pane (Global).
    (NONE, VK_F6, menu::NEXT_FRAME),
    (SHIFT, VK_F6, menu::PREV_FRAME),
    (SHIFT, VK_ESCAPE, menu::CLOSE_PANE),
    // Wave 1: each feature's shortcuts go in its own section below, so
    // their lines never meet (FRAMEWORK.md).

    // F1 (preview pane).
    // Everything's View | Preview (`view_preview_keys=592`, Alt+P).
    (ALT, key(b'P'), menu::VIEW_PANE_PREVIEW),
    // F2 (sidebars).

    // F3 (tabs and windows).
    // Everything's `file_new_tab_keys=340`, `file_close_tab_keys=343,371`,
    // `history_restore_recently_closed_tab_keys=1364`,
    // `file_new_window_keys=334` and `file_close_window_keys=1367` (0x100
    // Ctrl, 0x400 Shift), and Ctrl+Tab / Ctrl+Shift+Tab (`ctrl_tab_action`).
    (CTRL, key(b'T'), crate::tabs::NEW_TAB),
    (CTRL, key(b'W'), crate::tabs::CLOSE_TAB),
    (CTRL, VK_F4, crate::tabs::CLOSE_TAB),
    (CTRL | SHIFT, key(b'T'), crate::tabs::REOPEN_TAB),
    (CTRL, VK_TAB, crate::tabs::NEXT_TAB),
    (CTRL | SHIFT, VK_TAB, crate::tabs::PREV_TAB),
    (CTRL, key(b'N'), crate::tabs::NEW_WINDOW),
    (CTRL | SHIFT, key(b'W'), crate::tabs::CLOSE_WINDOW),
    // F4 (search tools). Everything 1.5's `edit_find_keys=326` (Ctrl+F),
    // `edit_find_next_keys=114` (F3), `edit_find_previous_keys=1138`
    // (Shift+F3), `edit_go_to_keys=327` (Ctrl+G),
    // `search_organize_filters_keys=1355` (Ctrl+Shift+K),
    // `view_fullscreen_keys=122` (F11), `view_window_size_*_keys=561..564`
    // (Alt+1..4).
    (CTRL, key(b'F'), searchtools::FIND),
    (NONE, VK_F3, searchtools::FIND_NEXT),
    (SHIFT, VK_F3, searchtools::FIND_PREVIOUS),
    (CTRL, key(b'G'), searchtools::GO_TO),
    (CTRL | SHIFT, key(b'K'), searchtools::ORGANIZE_FILTERS),
    (NONE, VK_F11, searchtools::FULLSCREEN),
    (ALT, key(b'1'), searchtools::WINDOW_SMALL),
    (ALT, key(b'2'), searchtools::WINDOW_MEDIUM),
    (ALT, key(b'3'), searchtools::WINDOW_LARGE),
    (ALT, key(b'4'), searchtools::WINDOW_AUTO_FIT),
    // F5 (file tools).
    // Everything's Edit > Undo History... (`edit_undo_history_keys=1370`,
    // Global); Undo, Redo and Advanced Rename are the list's own keys.
    (CTRL | SHIFT, key(b'Z'), crate::filetools::cmd::UNDO_HISTORY),
    // F6 (integration).
];

/// The results list's own keys (Everything's "Result List" scope, 0x2000
/// in its codes): modifiers, virtual key, command.
pub const LIST_KEYS: &[(u8, u16, u16)] = &[
    (NONE, VK_RETURN, menu::OPEN),
    (SHIFT, VK_RETURN, menu::OPEN),
    (CTRL, VK_RETURN, menu::REVEAL),
    (CTRL | SHIFT, VK_RETURN, menu::RUN_AS),
    (CTRL | SHIFT, key(b'C'), menu::COPY_FULL_PATH),
    (CTRL, key(b'C'), menu::COPY),
    (CTRL, VK_INSERT, menu::COPY),
    (CTRL, key(b'X'), menu::CUT),
    (CTRL, key(b'V'), menu::PASTE),
    (SHIFT, VK_INSERT, menu::PASTE),
    (NONE, VK_DELETE, menu::DELETE),
    (SHIFT, VK_DELETE, menu::DELETE_PERMANENT),
    (NONE, VK_F2, menu::RENAME),
    (CTRL, key(b'A'), menu::SELECT_ALL),
    (NONE, VK_ESCAPE, menu::FOCUS_SEARCH),
    // Everything's result-list keys (`edit_undo_keys=8538,8712`,
    // `edit_redo_keys=8537`, `edit_advanced_advanced_rename_keys=9329`):
    // in the search box Ctrl+Z and Ctrl+Y undo and redo its text.
    (CTRL, key(b'Z'), crate::filetools::cmd::UNDO),
    (ALT, VK_BACK, crate::filetools::cmd::UNDO),
    (CTRL, key(b'Y'), crate::filetools::cmd::REDO),
    (SHIFT, VK_F2, crate::filetools::cmd::ADVANCED_RENAME),
];

/// The search box's own keys (Everything's "Search Edit" scope, 0x1000):
/// F2 is Advanced Search... there (`search_advanced_search_keys=4209`), as
/// it renames in the list.
pub const SEARCH_BOX_KEYS: &[(u8, u16, u16)] = &[(NONE, VK_F2, searchtools::ADVANCED_SEARCH)];

/// An accelerator table of the window's shortcuts in force.
pub fn accelerators() -> HACCEL {
    let current = keymap::current();
    let table: Vec<ACCEL> = current
        .global
        .iter()
        .map(|&(mods, key, cmd)| ACCEL {
            fVirt: FVIRTKEY | mods,
            key,
            cmd,
        })
        .collect();
    // SAFETY: the table and its length; Windows copies it. (An empty
    // table cannot be made: a length of 0 fails and gives null, which
    // TranslateAccelerator takes as "no shortcuts".)
    unsafe { CreateAcceleratorTableW(table.as_ptr(), table.len() as i32) }
}

thread_local! {
    /// This thread's table, and the keys' generation it was made from.
    static TABLE: core::cell::Cell<(HACCEL, u32)> =
        const { core::cell::Cell::new((core::ptr::null_mut(), u32::MAX)) };
}

/// The accelerator table of the shortcuts in force, made again once
/// Options > Keyboard changed them (the old one is destroyed).
pub fn table() -> HACCEL {
    let generation = keymap::generation();
    TABLE.with(|t| {
        let (handle, made_for) = t.get();
        if made_for == generation {
            return handle;
        }
        let new = accelerators();
        if !handle.is_null() {
            // SAFETY: a table this function made and no one else holds.
            unsafe { DestroyAcceleratorTable(handle) };
        }
        t.set((new, generation));
        new
    })
}

fn mods(ctrl: bool, shift: bool, alt: bool) -> u8 {
    (if ctrl { CTRL } else { NONE })
        | (if shift { SHIFT } else { NONE })
        | if alt { ALT } else { NONE }
}

/// The command a key pressed in the results list runs, if it is one of the
/// list's own. With Ctrl, Alt is AltGr, which types: only a key bound with
/// both takes it.
pub fn list_command(vk: u16, ctrl: bool, shift: bool, alt: bool) -> Option<u16> {
    keymap::current().list_command(mods(ctrl, shift, alt), vk)
}

/// The command an Alt+key pressed in the results list runs, if it is one of
/// the list's own: Everything's second Undo key, Alt+Backspace
/// (`edit_undo_keys=...,8712`).
pub fn list_alt_command(vk: u16, ctrl: bool, shift: bool) -> Option<u16> {
    list_command(vk, ctrl, shift, true)
}

/// The command a key pressed in the search box runs, if one of its own.
pub fn search_box_command(vk: u16, ctrl: bool, shift: bool, alt: bool) -> Option<u16> {
    keymap::current().edit_command(mods(ctrl, shift, alt), vk)
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
        VK_DOWN, VK_END, VK_NEXT, VK_PRIOR, VK_TAB, VK_UP,
    };

    /// Keys the search box needs for editing, which no shortcut may take.
    const EDIT_KEYS: &[(u8, u16)] = &[
        (CTRL, key(b'A')),
        (CTRL, key(b'C')),
        (CTRL, key(b'X')),
        (CTRL, key(b'V')),
        (CTRL, key(b'Z')),
        (CTRL, key(b'Y')),
        (CTRL, VK_INSERT),
        (SHIFT, VK_INSERT),
        (SHIFT, VK_DELETE),
        (CTRL, VK_BACK),
        (CTRL, VK_DELETE),
        (NONE, VK_BACK),
        (NONE, VK_DELETE),
        (NONE, VK_LEFT),
        (NONE, VK_RIGHT),
        (SHIFT, VK_LEFT),
        (SHIFT, VK_RIGHT),
        (CTRL, VK_LEFT),
        (CTRL, VK_RIGHT),
        (CTRL | SHIFT, VK_LEFT),
        (CTRL | SHIFT, VK_RIGHT),
        (NONE, VK_HOME),
        (NONE, VK_END),
        (SHIFT, VK_HOME),
        (SHIFT, VK_END),
        (CTRL, VK_HOME),
        (CTRL, VK_END),
        (NONE, VK_UP),
        (NONE, VK_DOWN),
        (NONE, VK_PRIOR),
        (NONE, VK_NEXT),
        (ALT, VK_UP),
        (ALT, VK_DOWN),
        (NONE, VK_RETURN),
        (CTRL, VK_RETURN),
        (NONE, VK_ESCAPE),
        (NONE, VK_TAB),
    ];

    fn mods(ctrl: bool, shift: bool) -> u8 {
        super::mods(ctrl, shift, false)
    }

    #[test]
    fn shortcuts_leave_the_search_box_its_keys() {
        for &(m, vk, cmd) in ACCELERATORS {
            assert!(
                !EDIT_KEYS.contains(&(m, vk)),
                "shortcut for {cmd} takes an editing key"
            );
        }
    }

    #[test]
    fn list_keys_reach_the_list() {
        // The table goes first, so a list key in it would never arrive.
        for vk in 0..=255u16 {
            for (ctrl, shift) in [(false, false), (true, false), (false, true), (true, true)] {
                if list_command(vk, ctrl, shift, false).is_some() {
                    let m = mods(ctrl, shift);
                    assert!(
                        !ACCELERATORS.iter().any(|&(am, ak, _)| am == m && ak == vk),
                        "list key {vk:#x} is also a shortcut"
                    );
                }
            }
        }
    }

    #[test]
    fn no_shortcut_is_listed_twice() {
        for (i, a) in ACCELERATORS.iter().enumerate() {
            for b in &ACCELERATORS[i + 1..] {
                assert!((a.0, a.1) != (b.0, b.1), "{a:?} twice");
            }
        }
    }

    /// Every key chord runs one command wherever the keyboard is: the
    /// accelerators (all six features' and the window's own, merged) never
    /// share a chord, and none of them takes a chord the results list
    /// keeps for itself (its Ctrl, Shift and Alt keys).
    #[test]
    fn no_two_commands_share_a_key_chord() {
        use std::collections::HashMap;
        let mut chords: HashMap<(u8, u16), u16> = HashMap::new();
        for &(m, vk, cmd) in ACCELERATORS {
            if let Some(other) = chords.insert((m, vk), cmd) {
                panic!("{m:#x}+{vk:#x} runs both {other} and {cmd}");
            }
        }
        for vk in 0..=255u16 {
            for (ctrl, shift) in [(false, false), (true, false), (false, true), (true, true)] {
                let m = mods(ctrl, shift);
                if let Some(cmd) = list_command(vk, ctrl, shift, false)
                    && let Some(&global) = chords.get(&(m, vk))
                {
                    panic!("{m:#x}+{vk:#x}: the list's {cmd} and the window's {global}");
                }
                if let Some(cmd) = list_alt_command(vk, ctrl, shift)
                    && let Some(&global) = chords.get(&(m | ALT, vk))
                {
                    panic!("Alt+{vk:#x}: the list's {cmd} and the window's {global}");
                }
            }
        }
    }

    #[test]
    fn alt_shortcuts_miss_the_menu_access_keys() {
        let keys = menu::bar_access_keys();
        assert_eq!(keys.len(), 9, "one per top-level menu");
        for &(m, vk, _) in ACCELERATORS {
            if m == ALT && vk < 0x80 {
                assert!(
                    !keys.contains(&(vk as u8 as char)),
                    "Alt+{}",
                    vk as u8 as char
                );
            }
        }
    }

    #[test]
    fn bookmark_keys() {
        assert!(ACCELERATORS.contains(&(CTRL, key(b'D'), menu::BOOKMARK_ADD)));
        assert!(ACCELERATORS.contains(&(CTRL | SHIFT, key(b'B'), menu::BOOKMARK_ORGANIZE)));
        assert!(ACCELERATORS.contains(&(CTRL | SHIFT, key(b'O'), menu::BOOKMARK_ORGANIZE)));
        assert!(ACCELERATORS.contains(&(CTRL, key(b'B'), menu::BOOKMARK_FIND)));
    }

    #[test]
    fn search_history_keys() {
        assert!(ACCELERATORS.contains(&(CTRL, key(b'H'), menu::FIND_SEARCH_HISTORY)));
        assert!(ACCELERATORS.contains(&(CTRL | SHIFT, key(b'H'), menu::ORGANIZE_SEARCH_HISTORY)));
    }

    #[test]
    fn export_filter_and_zoom_keys() {
        assert!(ACCELERATORS.contains(&(CTRL, key(b'S'), menu::EXPORT)));
        // Ctrl+] and Ctrl+[ on a US layout.
        assert!(ACCELERATORS.contains(&(CTRL, 0xdd, menu::NEXT_FILTER)));
        assert!(ACCELERATORS.contains(&(CTRL, 0xdb, menu::PREV_FILTER)));
        // 443, 445 and 304 in Everything's ini: 0x100 is Ctrl.
        assert!(ACCELERATORS.contains(&(CTRL, 443 - 0x100, menu::ZOOM_IN)));
        assert!(ACCELERATORS.contains(&(CTRL, 445 - 0x100, menu::ZOOM_OUT)));
        assert!(ACCELERATORS.contains(&(CTRL, 304 - 0x100, menu::ZOOM_RESET)));
    }

    #[test]
    fn frame_keys() {
        // Everything's `window_next_frame_keys=117`,
        // `window_previous_frame_keys=1141` and `view_close_pane_keys=1051`
        // (0x400 is Shift).
        assert!(ACCELERATORS.contains(&(NONE, 117, menu::NEXT_FRAME)));
        assert!(ACCELERATORS.contains(&(SHIFT, 1141 - 0x400, menu::PREV_FRAME)));
        assert!(ACCELERATORS.contains(&(SHIFT, 1051 - 0x400, menu::CLOSE_PANE)));
        // Plain Esc stays the search box's and the list's.
        assert!(
            !ACCELERATORS
                .iter()
                .any(|&(m, k, _)| m == NONE && k == VK_ESCAPE)
        );
    }

    #[test]
    fn tab_and_window_keys() {
        // Everything's INI values, less 0x100 (Ctrl) and 0x400 (Shift).
        let has = |m: u8, vk: u16, cmd: u16| ACCELERATORS.contains(&(m, vk, cmd));
        assert!(has(CTRL, 340 - 0x100, crate::tabs::NEW_TAB));
        assert!(has(CTRL, 343 - 0x100, crate::tabs::CLOSE_TAB));
        assert!(has(CTRL, 371 - 0x100, crate::tabs::CLOSE_TAB));
        assert!(has(CTRL | SHIFT, 1364 - 0x500, crate::tabs::REOPEN_TAB));
        assert!(has(CTRL, 334 - 0x100, crate::tabs::NEW_WINDOW));
        assert!(has(CTRL | SHIFT, 1367 - 0x500, crate::tabs::CLOSE_WINDOW));
        assert!(has(CTRL, VK_TAB, crate::tabs::NEXT_TAB));
        assert!(has(CTRL | SHIFT, VK_TAB, crate::tabs::PREV_TAB));
    }

    #[test]
    fn run_history_key() {
        // Everything's Tools > Run History... (`tools_run_history_keys=330`:
        // Ctrl+J).
        assert!(ACCELERATORS.contains(&(CTRL, key(b'J'), menu::RUN_HISTORY)));
    }

    #[test]
    fn search_tools_keys() {
        // Everything's: 0x100 Ctrl, 0x200 Alt, 0x400 Shift.
        let has = |code: u16, cmd: u16| {
            let m = (if code & 0x100 != 0 { CTRL } else { NONE })
                | (if code & 0x200 != 0 { ALT } else { NONE })
                | (if code & 0x400 != 0 { SHIFT } else { NONE });
            ACCELERATORS.contains(&(m, code & 0xff, cmd))
        };
        assert!(has(326, searchtools::FIND));
        assert!(has(114, searchtools::FIND_NEXT));
        assert!(has(1138, searchtools::FIND_PREVIOUS));
        assert!(has(327, searchtools::GO_TO));
        assert!(has(1355, searchtools::ORGANIZE_FILTERS));
        assert!(has(122, searchtools::FULLSCREEN));
        assert!(has(561, searchtools::WINDOW_SMALL));
        assert!(has(562, searchtools::WINDOW_MEDIUM));
        assert!(has(563, searchtools::WINDOW_LARGE));
        assert!(has(564, searchtools::WINDOW_AUTO_FIT));
        // Ctrl+F is the find bar's now, no longer the search box's.
        assert!(!ACCELERATORS.contains(&(CTRL, key(b'F'), menu::FOCUS_SEARCH)));
    }

    #[test]
    fn list_keys() {
        let k = |vk, ctrl, shift| list_command(vk, ctrl, shift, false);
        assert_eq!(k(VK_RETURN, false, false), Some(menu::OPEN));
        assert_eq!(k(VK_RETURN, false, true), Some(menu::OPEN));
        assert_eq!(k(VK_RETURN, true, false), Some(menu::REVEAL));
        assert_eq!(k(VK_RETURN, true, true), Some(menu::RUN_AS));
        assert_eq!(k(key(b'C'), true, true), Some(menu::COPY_FULL_PATH));
        assert_eq!(k(key(b'C'), true, false), Some(menu::COPY));
        assert_eq!(k(VK_INSERT, true, false), Some(menu::COPY));
        assert_eq!(k(key(b'X'), true, false), Some(menu::CUT));
        assert_eq!(k(key(b'A'), true, false), Some(menu::SELECT_ALL));
        assert_eq!(k(VK_ESCAPE, false, false), Some(menu::FOCUS_SEARCH));
        assert_eq!(k(key(b'C'), false, false), None, "type-ahead");
        // The files on the clipboard, Delete and Shift+Delete, F2.
        assert_eq!(k(key(b'V'), true, false), Some(menu::PASTE));
        assert_eq!(k(VK_INSERT, false, true), Some(menu::PASTE));
        assert_eq!(k(VK_DELETE, false, false), Some(menu::DELETE));
        assert_eq!(k(VK_DELETE, false, true), Some(menu::DELETE_PERMANENT));
        assert_eq!(k(VK_DELETE, true, false), None);
        assert_eq!(k(VK_F2, false, false), Some(menu::RENAME));
        assert_eq!(k(key(b'V'), false, false), None, "type-ahead");
        // AltGr+C types a character on some layouts.
        assert_eq!(list_command(key(b'C'), true, false, true), None);
    }

    #[test]
    fn file_tool_keys() {
        use crate::filetools::cmd;
        let k = |vk, ctrl, shift| list_command(vk, ctrl, shift, false);
        // Everything's: 8538 (0x215a) Ctrl+Z, 8537 Ctrl+Y and 9329 (0x2471)
        // Shift+F2 in the result list; 8712 (0x2208) Alt+Backspace there;
        // 1370 (0x55a) Ctrl+Shift+Z anywhere.
        assert_eq!(k(key(b'Z'), true, false), Some(cmd::UNDO));
        assert_eq!(k(key(b'Y'), true, false), Some(cmd::REDO));
        assert_eq!(k(VK_F2, false, true), Some(cmd::ADVANCED_RENAME));
        assert_eq!(list_alt_command(VK_BACK, false, false), Some(cmd::UNDO));
        assert_eq!(list_alt_command(VK_BACK, true, false), None, "AltGr");
        assert_eq!(list_alt_command(key(b'Z'), false, false), None);
        assert!(ACCELERATORS.contains(&(CTRL | SHIFT, 0x5a, cmd::UNDO_HISTORY)));
        // Ctrl+Z and Ctrl+Y stay the search box's.
        assert!(
            !ACCELERATORS
                .iter()
                .any(|&(m, k, _)| m == CTRL && (k == 0x5a || k == 0x59))
        );
    }
}
