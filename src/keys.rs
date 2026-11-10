//! Keyboard shortcuts: the window's accelerator table, and the keys only the
//! results list takes.
//!
//! The accelerator table sees every key of the window before the focused
//! control does, so it holds only keys that mean the same wherever the focus
//! is. Keys the search box needs for its text (Ctrl+A, Ctrl+C, Shift+Insert,
//! Ctrl+Backspace and the like) mean something else in the list; those are
//! the list's own, handled by its subclass through [`list_command`].

use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    VK_ADD, VK_DELETE, VK_ESCAPE, VK_F1, VK_F2, VK_F3, VK_F4, VK_F5, VK_F6, VK_F11, VK_HOME,
    VK_INSERT, VK_LEFT, VK_NUMPAD0, VK_OEM_4, VK_OEM_6, VK_OEM_MINUS, VK_OEM_PLUS, VK_RETURN,
    VK_RIGHT,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    ACCEL, CreateAcceleratorTableW, FALT, FCONTROL, FSHIFT, FVIRTKEY, HACCEL,
};

use crate::{menu, searchtools};

const NONE: u8 = 0;
const CTRL: u8 = FCONTROL;
const SHIFT: u8 = FSHIFT;
const ALT: u8 = FALT;

const fn key(c: u8) -> u16 {
    c as u16
}

/// The window's shortcuts: modifiers, virtual key, command.
const ACCELERATORS: &[(u8, u16, u16)] = &[
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

    // F2 (sidebars).

    // F3 (tabs and windows).

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

    // F6 (integration).
];

pub fn accelerators() -> HACCEL {
    let table: Vec<ACCEL> = ACCELERATORS
        .iter()
        .map(|&(mods, key, cmd)| ACCEL {
            fVirt: FVIRTKEY | mods,
            key,
            cmd,
        })
        .collect();
    // SAFETY: the table and its length; Windows copies it.
    unsafe { CreateAcceleratorTableW(table.as_ptr(), table.len() as i32) }
}

/// The command a key pressed in the results list runs, if it is one of the
/// list's own. Alt+key never is: with Ctrl it is AltGr, which types.
pub fn list_command(vk: u16, ctrl: bool, shift: bool, alt: bool) -> Option<u16> {
    if alt {
        return None;
    }
    Some(match (vk, ctrl, shift) {
        (VK_RETURN, false, _) => menu::OPEN,
        (VK_RETURN, true, false) => menu::REVEAL,
        (VK_RETURN, true, true) => menu::RUN_AS,
        (k, true, true) if k == key(b'C') => menu::COPY_FULL_PATH,
        (k, true, false) if k == key(b'C') => menu::COPY,
        (VK_INSERT, true, false) => menu::COPY,
        (k, true, false) if k == key(b'X') => menu::CUT,
        (k, true, false) if k == key(b'V') => menu::PASTE,
        (VK_INSERT, false, true) => menu::PASTE,
        (VK_DELETE, false, false) => menu::DELETE,
        (VK_DELETE, false, true) => menu::DELETE_PERMANENT,
        (VK_F2, false, false) => menu::RENAME,
        (k, true, false) if k == key(b'A') => menu::SELECT_ALL,
        (VK_ESCAPE, false, false) => menu::FOCUS_SEARCH,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
        VK_BACK, VK_DOWN, VK_END, VK_NEXT, VK_PRIOR, VK_TAB, VK_UP,
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
        (if ctrl { CTRL } else { NONE }) | if shift { SHIFT } else { NONE }
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
}
