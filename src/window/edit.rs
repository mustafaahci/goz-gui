//! The search box's own keys. The keyboard stays in the box while it drives
//! the results: Enter picks the result run most (`runs.rs`; the top one when
//! none was ever run) and then opens the selection,
//! Up/Down/PgUp/PgDn move in the list, Ctrl+Enter opens the selection's
//! folder. Alt+Down / Alt+Up open the search history dropdown, whose keys
//! take over while it is open (also when typing opened it with suggestions:
//! the box marks the user's own edits, `App::typing`). Ctrl+Backspace and
//! Ctrl+Delete delete a word.

use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows_sys::Win32::UI::Controls::{EM_GETSEL, EM_REPLACESEL, EM_SETSEL, EM_UNDO};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    GetActiveWindow, GetFocus, SetFocus, VK_CONTROL, VK_DELETE, VK_DOWN, VK_ESCAPE, VK_NEXT,
    VK_PRIOR, VK_RETURN, VK_SHIFT, VK_UP,
};
use windows_sys::Win32::UI::Shell::DefSubclassProc;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    GetWindowTextLengthW, SendMessageW, SetWindowTextW, WM_CHAR, WM_CLEAR, WM_CUT, WM_DROPFILES,
    WM_KEYDOWN, WM_KILLFOCUS, WM_MOUSEWHEEL, WM_PASTE, WM_SYSKEYDOWN, WM_UNDO,
};

use super::{App, key_down, send};
use crate::wide::{window_text, wz};
use crate::{dropdown, menu};

/// What a key pressed in the search box does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum EditKey {
    /// Alt+Down, Alt+Up: open or close the search history dropdown.
    ToggleHistory,
    /// While the dropdown is open: move its highlight by entries or pages,
    /// use the entry, close, forget the entry.
    HistoryMove(isize),
    HistoryPage(isize),
    HistoryUse,
    HistoryClose,
    HistoryDelete,
    /// Up, Down, PgUp, PgDn, with Shift or Ctrl as in the list: move in
    /// the results while the keyboard stays here.
    List,
    /// Enter, Shift+Enter.
    Enter,
    /// Ctrl+Enter.
    OpenPath,
    /// Esc.
    Clear,
    /// Ctrl+Delete.
    DeleteNextWord,
}

/// The search box's meaning of key `vk`; `open` while the history dropdown
/// is. `None` leaves the key to the edit control.
fn edit_key(vk: u16, ctrl: bool, shift: bool, alt: bool, open: bool) -> Option<EditKey> {
    use EditKey::*;
    if alt {
        return matches!(vk, VK_DOWN | VK_UP).then_some(ToggleHistory);
    }
    if open {
        let key = match vk {
            VK_UP => Some(HistoryMove(-1)),
            VK_DOWN => Some(HistoryMove(1)),
            VK_PRIOR => Some(HistoryPage(-1)),
            VK_NEXT => Some(HistoryPage(1)),
            VK_RETURN => Some(HistoryUse),
            VK_ESCAPE => Some(HistoryClose),
            VK_DELETE if !ctrl && !shift => Some(HistoryDelete),
            _ => None,
        };
        if key.is_some() {
            return key;
        }
    }
    Some(match (vk, ctrl, shift) {
        (VK_UP | VK_DOWN | VK_PRIOR | VK_NEXT, _, _) => List,
        (VK_RETURN, false, _) => Enter,
        (VK_RETURN, true, false) => OpenPath,
        (VK_ESCAPE, _, _) => Clear,
        (VK_DELETE, true, false) => DeleteNextWord,
        _ => return None,
    })
}

impl App {
    /// Runs `key`; false when it is left to the edit control after all.
    fn run_edit_key(&self, key: EditKey, w: WPARAM, l: LPARAM) -> bool {
        match key {
            EditKey::ToggleHistory => self.toggle_history(),
            EditKey::HistoryMove(delta) => dropdown::move_hot(self, delta),
            EditKey::HistoryPage(direction) => dropdown::page(self, direction),
            EditKey::HistoryUse => {
                if self.dropdown.hot().is_some() {
                    self.use_history_entry();
                } else {
                    dropdown::close(self);
                    self.edit_enter();
                }
            }
            EditKey::HistoryClose => dropdown::close(self),
            EditKey::HistoryDelete => return dropdown::delete_hot(self),
            // The list moves its focus and selection as for its own keys,
            // reading Shift and Ctrl itself.
            EditKey::List => {
                send(self.list, WM_KEYDOWN, w, l);
            }
            EditKey::Enter => self.edit_enter(),
            EditKey::OpenPath => {
                if !self.selected(1).is_empty() {
                    self.command(menu::REVEAL);
                }
            }
            EditKey::Clear => {
                // SAFETY: reads and clears this thread's search box.
                unsafe {
                    if GetWindowTextLengthW(self.edit) > 0 {
                        SetWindowTextW(self.edit, wz("").as_ptr());
                    }
                }
            }
            EditKey::DeleteNextWord => delete_word(self.edit, false),
        }
        true
    }

    /// Enter in the search box: the search counts as made; then the first
    /// Enter after the search changed selects the result run most (or as
    /// `search_edit_enter_action` says; `runs.rs`), the keyboard staying
    /// here, and the next opens the selection.
    fn edit_enter(&self) {
        self.record_search();
        let searching = self
            .model
            .borrow()
            .pending
            .as_ref()
            .is_some_and(|p| !p.keep_place);
        if searching {
            // Its results replace the selection when they come: pick from
            // them then.
            self.enter_pending.set(true);
        } else if self.selected(1).is_empty() {
            self.enter_select();
        } else {
            self.command(menu::OPEN);
        }
    }

    /// Opens the search history dropdown with the keyboard in the search
    /// box, or closes it.
    pub(super) fn toggle_history(&self) {
        if self.dropdown.is_open() {
            dropdown::close(self);
            return;
        }
        self.focus_box();
        dropdown::open(self, &window_text(self.edit));
    }

    /// Puts the keyboard in the search box. While the window is not the
    /// active one, the box is where the keyboard goes when it is: focusing
    /// would activate the window, which only the user should do.
    pub(super) fn focus_box(&self) {
        // SAFETY: reads and moves the focus within this thread's window.
        unsafe {
            if GetActiveWindow() != self.hwnd {
                self.last_focus.set(self.edit);
            } else if GetFocus() != self.edit {
                SetFocus(self.edit);
            }
        }
    }

    /// Searches for the dropdown's highlighted entry.
    pub fn use_history_entry(&self) {
        let Some(text) = dropdown::hot_text(self) else {
            return;
        };
        dropdown::close(self);
        self.set_search_text(&text);
    }
}

pub(super) unsafe extern "system" fn edit_proc(
    hwnd: HWND,
    msg: u32,
    w: WPARAM,
    l: LPARAM,
    _id: usize,
    data: usize,
) -> LRESULT {
    // SAFETY: `data` is the leaked `App` passed to SetWindowSubclass.
    let app = unsafe { &*(data as *const App) };
    // The user's own editing (a character, Delete, a paste, an undo): the
    // change of text it makes may bring suggestions (`App::text_changed`).
    let typed = match msg {
        WM_CHAR | WM_PASTE | WM_CUT | WM_CLEAR | WM_UNDO | EM_UNDO => true,
        WM_KEYDOWN => w as u16 == VK_DELETE,
        _ => false,
    };
    let was = app.typing.replace(app.typing.get() || typed);
    // SAFETY: the message as it came, for the box it came to.
    let r = unsafe { edit_message(app, hwnd, msg, w, l) };
    app.typing.set(was);
    r
}

/// The search box's messages.
///
/// # Safety
/// `w` and `l` are what `msg` carries, for the box `hwnd`.
unsafe fn edit_message(app: &App, hwnd: HWND, msg: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    // SAFETY: `w` and `l` are passed on as they came.
    unsafe {
        match msg {
            WM_KEYDOWN | WM_SYSKEYDOWN => {
                // Bit 29: Alt is down (WM_SYSKEYDOWN also comes for F10).
                let alt = msg == WM_SYSKEYDOWN && l & (1 << 29) != 0;
                let key = edit_key(
                    w as u16,
                    key_down(VK_CONTROL),
                    key_down(VK_SHIFT),
                    alt,
                    app.dropdown.is_open(),
                );
                if let Some(key) = key
                    && app.run_edit_key(key, w, l)
                {
                    return 0;
                }
            }
            // Enter, Ctrl+Enter and Esc would beep in a single-line edit.
            WM_CHAR if matches!(w, 0x0a | 0x0d | 0x1b) => return 0,
            // Ctrl+Backspace arrives as DEL; a single-line edit types it.
            WM_CHAR if w == 0x7f => {
                delete_word(hwnd, true);
                return 0;
            }
            WM_KILLFOCUS => dropdown::close(app),
            // A drop on the box is a drop on the window.
            WM_DROPFILES => {
                app.drop_files(w as _);
                return 0;
            }
            // The wheel goes to the focus, which stays here while the
            // dropdown is open.
            WM_MOUSEWHEEL if app.dropdown.is_open() => {
                SendMessageW(app.dropdown.hwnd(), msg, w, l);
                return 0;
            }
            _ => {}
        }
        DefSubclassProc(hwnd, msg, w, l)
    }
}

/// Characters that end a word for Ctrl+Backspace and Ctrl+Delete: spaces
/// and the punctuation inside paths and file names.
fn is_separator(c: u16) -> bool {
    matches!(c, 0x20 | 0x5c | 0x2f | 0x2e | 0x2d | 0x5f)
}

/// Where Ctrl+Backspace deletes back to from `caret`: over separators, then
/// the word before them (where Ctrl+Left goes).
fn word_start_before(text: &[u16], caret: usize) -> usize {
    let mut i = caret.min(text.len());
    while i > 0 && is_separator(text[i - 1]) {
        i -= 1;
    }
    while i > 0 && !is_separator(text[i - 1]) {
        i -= 1;
    }
    i
}

/// Where Ctrl+Delete deletes up to from `caret`: over the rest of the word,
/// then the separators after it (where Ctrl+Right goes).
fn word_end_after(text: &[u16], caret: usize) -> usize {
    let mut i = caret.min(text.len());
    while i < text.len() && !is_separator(text[i]) {
        i += 1;
    }
    while i < text.len() && is_separator(text[i]) {
        i += 1;
    }
    i
}

/// Deletes the selection, or with none the word before the caret
/// (`back`, Ctrl+Backspace) or after it (Ctrl+Delete); undoable.
pub(super) fn delete_word(edit: HWND, back: bool) {
    let (mut start, mut end) = (0u32, 0u32);
    send(
        edit,
        EM_GETSEL,
        (&raw mut start) as usize,
        (&raw mut end) as isize,
    );
    if start == end {
        let text: Vec<u16> = window_text(edit).encode_utf16().collect();
        let caret = start as usize;
        let (from, to) = if back {
            (word_start_before(&text, caret), caret)
        } else {
            (caret, word_end_after(&text, caret))
        };
        if from == to {
            return;
        }
        send(edit, EM_SETSEL, from, to as isize);
    }
    let empty = [0u16];
    send(edit, EM_REPLACESEL, 1, empty.as_ptr() as isize);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn w(s: &str) -> Vec<u16> {
        s.encode_utf16().collect()
    }

    #[test]
    fn ctrl_backspace_takes_the_word_before() {
        let t = w(r"C:\Windows\System32 foo");
        assert_eq!(word_start_before(&t, t.len()), t.len() - 3);
        // From after the space: the space and the word before it.
        assert_eq!(word_start_before(&t, t.len() - 3), 11);
        assert_eq!(word_start_before(&t, 11), 3);
        assert_eq!(word_start_before(&t, 3), 0);
        assert_eq!(word_start_before(&t, 0), 0);
        assert_eq!(word_start_before(&t, 999), t.len() - 3);
    }

    #[test]
    fn ctrl_delete_takes_the_word_after() {
        let t = w("foo bar.txt baz");
        // From a word's start: the word and the space after it.
        assert_eq!(word_end_after(&t, 0), 4);
        // From inside a word: the rest of it.
        assert_eq!(word_end_after(&t, 1), 4);
        // From a separator: just the separators.
        assert_eq!(word_end_after(&t, 3), 4);
        assert_eq!(word_end_after(&t, 4), 8);
        assert_eq!(word_end_after(&t, 8), 12);
        assert_eq!(word_end_after(&t, 12), t.len());
        assert_eq!(word_end_after(&t, t.len()), t.len());
        assert_eq!(word_end_after(&t, 999), t.len());
    }

    #[test]
    fn keys_in_the_search_box() {
        use EditKey::*;
        let k = |vk, ctrl, shift| edit_key(vk, ctrl, shift, false, false);
        assert_eq!(k(VK_RETURN, false, false), Some(Enter));
        assert_eq!(k(VK_RETURN, false, true), Some(Enter));
        assert_eq!(k(VK_RETURN, true, false), Some(OpenPath));
        assert_eq!(k(VK_RETURN, true, true), None);
        for vk in [VK_UP, VK_DOWN, VK_PRIOR, VK_NEXT] {
            for (ctrl, shift) in [(false, false), (true, false), (false, true)] {
                assert_eq!(k(vk, ctrl, shift), Some(List));
            }
        }
        assert_eq!(k(VK_ESCAPE, false, false), Some(Clear));
        assert_eq!(k(VK_DELETE, true, false), Some(DeleteNextWord));
        // Plain Delete and Shift+Delete (cut) stay the edit control's.
        assert_eq!(k(VK_DELETE, false, false), None);
        assert_eq!(k(VK_DELETE, false, true), None);
        // Ctrl+Home and Ctrl+End move the caret.
        assert_eq!(k(0x24, true, false), None);
        assert_eq!(k(0x23, true, false), None);
        for open in [false, true] {
            assert_eq!(
                edit_key(VK_DOWN, false, false, true, open),
                Some(ToggleHistory)
            );
            assert_eq!(
                edit_key(VK_UP, false, false, true, open),
                Some(ToggleHistory)
            );
            assert_eq!(edit_key(VK_RETURN, false, false, true, open), None);
        }
    }

    #[test]
    fn keys_while_the_dropdown_is_open() {
        use EditKey::*;
        let k = |vk, ctrl, shift| edit_key(vk, ctrl, shift, false, true);
        assert_eq!(k(VK_UP, false, false), Some(HistoryMove(-1)));
        assert_eq!(k(VK_DOWN, false, false), Some(HistoryMove(1)));
        assert_eq!(k(VK_PRIOR, false, false), Some(HistoryPage(-1)));
        assert_eq!(k(VK_NEXT, false, false), Some(HistoryPage(1)));
        assert_eq!(k(VK_RETURN, false, false), Some(HistoryUse));
        assert_eq!(k(VK_ESCAPE, false, false), Some(HistoryClose));
        assert_eq!(k(VK_DELETE, false, false), Some(HistoryDelete));
        // Ctrl+Delete still edits the text.
        assert_eq!(k(VK_DELETE, true, false), Some(DeleteNextWord));
        assert_eq!(k(u16::from(b'A'), false, false), None);
    }
}
