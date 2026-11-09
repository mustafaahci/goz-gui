//! Keyboard shortcuts: the window's accelerator table, and the keys only the
//! results list takes.
//!
//! The accelerator table sees every key of the window before the focused
//! control does, so it holds only keys that mean the same wherever the focus
//! is. Keys the search box needs for its text (Ctrl+A, Ctrl+C, Shift+Insert,
//! Ctrl+Backspace and the like) mean something else in the list; those are
//! the list's own, handled by its subclass through [`list_command`].

use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    VK_DELETE, VK_ESCAPE, VK_F1, VK_F2, VK_F4, VK_F5, VK_HOME, VK_INSERT, VK_LEFT, VK_RETURN,
    VK_RIGHT,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    ACCEL, CreateAcceleratorTableW, FALT, FCONTROL, FSHIFT, FVIRTKEY, HACCEL,
};

use crate::menu;

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
    // Everything 1.5's three, and 1.4's Ctrl+F.
    (ALT, key(b'D'), menu::FOCUS_SEARCH),
    (CTRL, key(b'E'), menu::FOCUS_SEARCH),
    (CTRL, key(b'L'), menu::FOCUS_SEARCH),
    (CTRL, key(b'F'), menu::FOCUS_SEARCH),
    (NONE, VK_F5, menu::REFRESH),
    (CTRL, key(b'1'), menu::SORT_NAME),
    (CTRL, key(b'2'), menu::SORT_PATH),
    (CTRL, key(b'3'), menu::SORT_SIZE),
    (CTRL, key(b'6'), menu::SORT_DATE),
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
    fn run_history_key() {
        // Everything's Tools > Run History... (`tools_run_history_keys=330`:
        // Ctrl+J).
        assert!(ACCELERATORS.contains(&(CTRL, key(b'J'), menu::RUN_HISTORY)));
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
