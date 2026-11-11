//! Options > Keyboard: the global hotkeys that show the window or toggle it
//! (`show_window_key`, `toggle_window_key`), and every command's keyboard
//! shortcuts (`<command>_keys`, `crate::keymap`), with Everything's labels:
//! "Show commands containing", the list of commands as "Category |
//! Command", and the shortcuts of the one chosen, each Global, in the
//! Search Edit only or in the Result List only. A new shortcut is typed
//! into its box (every key but Tab and Shift+Tab, which move on); one that
//! another command has where it would work is named, and taken from it by
//! Add, as Everything unbinds the other.

use std::cell::{Cell, RefCell};

use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows_sys::Win32::System::SystemServices::SS_LEFT;
use windows_sys::Win32::UI::Controls::{HKM_GETHOTKEY, HKM_SETHOTKEY};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    GetKeyState, VK_CONTROL, VK_MENU, VK_SHIFT, VK_TAB,
};
use windows_sys::Win32::UI::Shell::{DefSubclassProc, SetWindowSubclass};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    BN_CLICKED, BS_GROUPBOX, CBS_DROPDOWNLIST, DLGC_WANTALLKEYS, EN_CHANGE, ES_AUTOHSCROLL,
    GetParent, LB_ADDSTRING, LB_ERR, LB_GETCURSEL, LB_RESETCONTENT, LB_SETCURSEL, LBN_SELCHANGE,
    LBS_NOINTEGRALHEIGHT, LBS_NOTIFY, SendMessageW, SetWindowTextW, WM_CHAR, WM_COMMAND,
    WM_GETDLGCODE, WM_KEYDOWN, WM_SYSCHAR, WM_SYSKEYDOWN, WS_BORDER, WS_EX_CLIENTEDGE, WS_TABSTOP,
    WS_VSCROLL,
};

use crate::dialog::Class;
use crate::keymap::{self, Bindings, COMMANDS, Scope};
use crate::options::{
    LABEL, Page, Rows, combo_value, enable, fill_combo, item, order, send, set_text, text,
};
use crate::wide::wz;
use crate::window::App;

/// Everything's control ids.
const SHOW_KEY: u16 = 1165;
const TOGGLE_KEY: u16 = 1166;
const FILTER: u16 = 1167;
const COMMAND_LIST: u16 = 1157;
const GROUP: u16 = 1158;
const SHORTCUTS: u16 = 1159;
const REMOVE: u16 = 1162;
/// goz-gui's: the box a new shortcut is typed into, where it is to work,
/// Add, and who has it already.
const NEW_KEY: u16 = 1170;
const SCOPE: u16 = 1171;
const ADD: u16 = 1160;
const USED_BY: u16 = 1172;

/// The key box's notification to the page: a key was typed into it.
const KEY_TYPED: u32 = 0x7001;

const SCOPES: [(Scope, &str); 3] = [
    (Scope::Global, "Global"),
    (Scope::SearchEdit, "Search Edit"),
    (Scope::ResultList, "Result List"),
];

/// The page's working copy, applied at OK or Apply.
pub(super) struct Keyboard {
    keys: RefCell<Bindings>,
    /// The commands the filter leaves, by place in `COMMANDS`.
    shown: RefCell<Vec<usize>>,
    /// The command chosen.
    current: Cell<Option<usize>>,
    /// The chord typed in the new key's box (modifiers and key, no scope).
    typed: Cell<Option<u16>>,
    /// The page is filling its own controls: their notifications are no
    /// change.
    busy: Cell<bool>,
}

impl Keyboard {
    pub(super) fn new() -> Keyboard {
        Keyboard {
            keys: RefCell::new(Bindings::default()),
            shown: RefCell::new(Vec::new()),
            current: Cell::new(None),
            typed: Cell::new(None),
            busy: Cell::new(false),
        }
    }

    fn quietly(&self, f: impl FnOnce()) {
        let was = self.busy.replace(true);
        f();
        self.busy.set(was);
    }

    /// Lists the commands whose name holds the filter's words.
    fn list_commands(&self, page: HWND) {
        let words: Vec<String> = text(page, FILTER)
            .split_whitespace()
            .map(str::to_lowercase)
            .collect();
        let shown: Vec<usize> = COMMANDS
            .iter()
            .enumerate()
            .filter(|(_, c)| {
                let label = c.label.to_lowercase();
                words.iter().all(|w| label.contains(w.as_str()))
            })
            .map(|(i, _)| i)
            .collect();
        let keep = self
            .current
            .get()
            .and_then(|c| shown.iter().position(|&i| i == c));
        self.quietly(|| {
            send(page, COMMAND_LIST, LB_RESETCONTENT, 0, 0);
            for &i in &shown {
                let w = wz(COMMANDS[i].label);
                send(page, COMMAND_LIST, LB_ADDSTRING, 0, w.as_ptr() as isize);
            }
            let at = keep.unwrap_or(0);
            if !shown.is_empty() {
                send(page, COMMAND_LIST, LB_SETCURSEL, at, 0);
            }
        });
        self.current.set(shown.get(keep.unwrap_or(0)).copied());
        *self.shown.borrow_mut() = shown;
        self.show_command(page);
    }

    /// The chosen command's shortcuts, and the group's title.
    fn show_command(&self, page: HWND) {
        let current = self.current.get();
        let title = match current {
            Some(i) => format!("Shortcuts for {} command", COMMANDS[i].label),
            None => "Shortcuts".to_owned(),
        };
        let names: Vec<String> = current
            .map(|i| {
                self.keys
                    .borrow()
                    .of(i)
                    .iter()
                    .map(|&c| keymap::binding_name(c))
                    .collect()
            })
            .unwrap_or_default();
        self.quietly(|| {
            set_text(page, GROUP, &title);
            send(page, SHORTCUTS, LB_RESETCONTENT, 0, 0);
            for n in &names {
                let w = wz(n);
                send(page, SHORTCUTS, LB_ADDSTRING, 0, w.as_ptr() as isize);
            }
            if !names.is_empty() {
                send(page, SHORTCUTS, LB_SETCURSEL, 0, 0);
            }
        });
        self.update_buttons(page);
    }

    /// The new key with the scope chosen, as a code.
    fn new_code(&self, page: HWND) -> Option<u16> {
        let chord = self.typed.get()?;
        let scope: Scope = combo_value(page, SCOPE, &SCOPES);
        Some(chord | scope.bits())
    }

    /// Greys what cannot be done, and says who has the key typed.
    fn update_buttons(&self, page: HWND) {
        let current = self.current.get();
        let has = current.is_some_and(|i| !self.keys.borrow().of(i).is_empty());
        enable(page, REMOVE, has);
        let code = self.new_code(page);
        let already = match (current, code) {
            (Some(i), Some(c)) => self.keys.borrow().of(i).contains(&c),
            _ => false,
        };
        enable(page, ADD, current.is_some() && code.is_some() && !already);
        let note = match code {
            None => String::new(),
            Some(c) => match self.keys.borrow().conflict(c, current) {
                Some((other, old)) => format!(
                    "Shortcut currently used by: {} ({})",
                    COMMANDS[other].label,
                    keymap::scope(old).map_or("?", Scope::name)
                ),
                None if already => "Shortcut already assigned to this command".to_owned(),
                None => String::new(),
            },
        };
        set_text(page, USED_BY, &note);
    }

    fn add(&self, page: HWND) {
        let (Some(i), Some(code)) = (self.current.get(), self.new_code(page)) else {
            return;
        };
        self.keys.borrow_mut().add(i, code);
        self.typed.set(None);
        self.quietly(|| set_text(page, NEW_KEY, ""));
        self.show_command(page);
    }

    fn remove(&self, page: HWND) {
        let Some(i) = self.current.get() else { return };
        let at = send(page, SHORTCUTS, LB_GETCURSEL, 0, 0);
        if at == LB_ERR as isize || at < 0 {
            return;
        }
        let code = self.keys.borrow().of(i).get(at as usize).copied();
        if let Some(code) = code {
            self.keys.borrow_mut().remove(i, code);
        }
        self.show_command(page);
    }
}

fn hotkey(page: HWND, id: u16) -> u16 {
    send(page, id, HKM_GETHOTKEY, 0, 0) as u16
}

fn key_down(vk: u16) -> bool {
    // SAFETY: reads this thread's keyboard state.
    unsafe { GetKeyState(i32::from(vk)) < 0 }
}

/// The new shortcut's box: takes the key pressed (with Ctrl, Alt and
/// Shift) instead of typing, and shows its name; Tab and Shift+Tab still
/// move on.
unsafe extern "system" fn key_box_proc(
    hwnd: HWND,
    msg: u32,
    w: WPARAM,
    l: LPARAM,
    _id: usize,
    _data: usize,
) -> LRESULT {
    // SAFETY: the box's own messages, passed on as they came.
    unsafe {
        match msg {
            WM_GETDLGCODE => {
                if w as u16 == VK_TAB && !key_down(VK_CONTROL) && !key_down(VK_MENU) {
                    return DefSubclassProc(hwnd, msg, w, l);
                }
                return DLGC_WANTALLKEYS as LRESULT;
            }
            WM_KEYDOWN | WM_SYSKEYDOWN => {
                let vk = w as u16;
                if keymap::is_modifier(vk) {
                    return 0;
                }
                let mut chord = vk & 0xff;
                if key_down(VK_CONTROL) {
                    chord |= keymap::CTRL;
                }
                if key_down(VK_MENU) || msg == WM_SYSKEYDOWN && l & (1 << 29) != 0 {
                    chord |= keymap::ALT;
                }
                if key_down(VK_SHIFT) {
                    chord |= keymap::SHIFT;
                }
                SetWindowTextW(hwnd, wz(&keymap::key_name(chord)).as_ptr());
                let parent = GetParent(hwnd);
                let id = windows_sys::Win32::UI::WindowsAndMessaging::GetDlgCtrlID(hwnd) as usize;
                SendMessageW(
                    parent,
                    WM_COMMAND,
                    (KEY_TYPED as usize) << 16 | (id & 0xffff),
                    i64::from(chord) as isize,
                );
                return 0;
            }
            WM_CHAR | WM_SYSCHAR => return 0,
            _ => {}
        }
        DefSubclassProc(hwnd, msg, w, l)
    }
}

impl Page for Keyboard {
    fn name(&self) -> &'static str {
        "Keyboard"
    }

    fn parent(&self) -> Option<&'static str> {
        Some("General")
    }

    fn order(&self) -> u32 {
        order::KEYBOARD
    }

    fn build(&self, rows: &mut Rows<'_>) {
        // Everything's: the three boxes in a column after the labels.
        let label = 108;
        let field = rows.width() - label;
        rows.hotkey("&Show window Hotkey:", SHOW_KEY, label, field)
            .hotkey("&Toggle window Hotkey:", TOGGLE_KEY, label, field);
        rows.control(
            Class::Static,
            "Show &commands containing:",
            LABEL,
            SS_LEFT,
            0,
            (0, 2, label, 9),
        )
        .control(
            Class::Edit,
            "",
            FILTER,
            WS_TABSTOP | ES_AUTOHSCROLL as u32,
            WS_EX_CLIENTEDGE,
            (label, 0, field, 13),
        )
        .skip(17);
        let w = rows.width();
        rows.control(
            Class::Named("LISTBOX"),
            "",
            COMMAND_LIST,
            WS_TABSTOP | WS_VSCROLL | WS_BORDER | (LBS_NOTIFY | LBS_NOINTEGRALHEIGHT) as u32,
            0,
            (0, 0, w, 72),
        )
        .skip(76);
        rows.control(
            Class::Button,
            "Shortcuts",
            GROUP,
            BS_GROUPBOX as u32,
            0,
            (0, 0, w, 82),
        );
        let inner = w - 16;
        rows.indent(8).skip(12);
        rows.control(
            Class::Named("LISTBOX"),
            "",
            SHORTCUTS,
            WS_TABSTOP | WS_VSCROLL | WS_BORDER | (LBS_NOTIFY | LBS_NOINTEGRALHEIGHT) as u32,
            0,
            (0, 0, inner - 60, 34),
        )
        .control(
            Class::Button,
            "Remo&ve",
            REMOVE,
            WS_TABSTOP,
            0,
            (inner - 54, 0, 54, 14),
        )
        .skip(38);
        rows.control(
            Class::Static,
            "Ne&w shortcut:",
            LABEL,
            SS_LEFT,
            0,
            (0, 2, 50, 9),
        )
        .control(
            Class::Edit,
            "",
            NEW_KEY,
            WS_TABSTOP | ES_AUTOHSCROLL as u32,
            WS_EX_CLIENTEDGE,
            (50, 0, 72, 13),
        )
        .control(
            Class::Named("COMBOBOX"),
            "",
            SCOPE,
            WS_TABSTOP | WS_VSCROLL | CBS_DROPDOWNLIST as u32,
            0,
            (126, 0, inner - 126 - 58, 80),
        )
        .control(
            Class::Button,
            "A&dd",
            ADD,
            WS_TABSTOP,
            0,
            (inner - 54, 0, 54, 14),
        )
        .skip(18);
        rows.control(Class::Static, "", USED_BY, SS_LEFT, 0, (0, 0, inner, 9))
            .skip(16)
            .indent(-8);
    }

    fn fill(&self, app: &App, page: HWND) {
        let b = app.background();
        send(
            page,
            SHOW_KEY,
            HKM_SETHOTKEY,
            usize::from(b.show_window_key),
            0,
        );
        send(
            page,
            TOGGLE_KEY,
            HKM_SETHOTKEY,
            usize::from(b.toggle_window_key),
            0,
        );
        *self.keys.borrow_mut() = app.custom_settings().keys;
        fill_combo(page, SCOPE, &SCOPES, Scope::Global);
        // SAFETY: subclasses this page's own box, which it outlives.
        unsafe { SetWindowSubclass(item(page, NEW_KEY), Some(key_box_proc), 1, 0) };
        self.list_commands(page);
    }

    fn update_enabled(&self, page: HWND) {
        self.update_buttons(page);
    }

    /// A hotkey box tells its parent of a change as EN_CHANGE; the list of
    /// shortcuts changes by Add and Remove (the filter, the lists and the
    /// new key's box are no change themselves).
    fn changed(&self, _page: HWND, id: u16, code: u32) -> bool {
        (id == SHOW_KEY || id == TOGGLE_KEY) && code == EN_CHANGE
            || (id == ADD || id == REMOVE) && code == BN_CLICKED
    }

    fn message(&self, _app: &App, page: HWND, msg: u32, w: WPARAM, l: LPARAM) -> Option<isize> {
        if msg != WM_COMMAND {
            return None;
        }
        let (id, code) = ((w & 0xffff) as u16, ((w >> 16) & 0xffff) as u32);
        if self.busy.get() && !matches!(id, SHOW_KEY | TOGGLE_KEY) {
            return Some(0);
        }
        match (id, code) {
            (FILTER, EN_CHANGE) => {
                self.list_commands(page);
                Some(0)
            }
            (COMMAND_LIST, LBN_SELCHANGE) => {
                let at = send(page, COMMAND_LIST, LB_GETCURSEL, 0, 0);
                let i = usize::try_from(at)
                    .ok()
                    .and_then(|at| self.shown.borrow().get(at).copied());
                self.current.set(i);
                self.show_command(page);
                Some(0)
            }
            (SHORTCUTS, LBN_SELCHANGE) => Some(0),
            (NEW_KEY, KEY_TYPED) => {
                self.typed.set(Some(l as u16));
                self.update_buttons(page);
                Some(0)
            }
            (NEW_KEY, _) => Some(0),
            (SCOPE, _) => {
                self.update_buttons(page);
                Some(0)
            }
            // Add and Remove: done here, then counted as a change.
            (ADD, BN_CLICKED) => {
                self.add(page);
                None
            }
            (REMOVE, BN_CLICKED) => {
                self.remove(page);
                None
            }
            _ => None,
        }
    }

    fn apply(&self, app: &App, page: HWND, problems: &mut Vec<String>) {
        app.change_background(
            |b| {
                b.show_window_key = hotkey(page, SHOW_KEY);
                b.toggle_window_key = hotkey(page, TOGGLE_KEY);
            },
            problems,
        );
        app.set_bindings(self.keys.borrow().clone());
    }

    /// Everything's: no hotkeys; goz-gui's own shortcuts.
    fn defaults(&self, page: HWND) {
        send(page, SHOW_KEY, HKM_SETHOTKEY, 0, 0);
        send(page, TOGGLE_KEY, HKM_SETHOTKEY, 0, 0);
        *self.keys.borrow_mut() = Bindings::default();
        self.show_command(page);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_new_keys_box_is_noticed_as_a_command() {
        assert_ne!(KEY_TYPED, EN_CHANGE);
        assert_ne!(KEY_TYPED, BN_CLICKED);
        for (scope, name) in SCOPES {
            assert_eq!(scope.name(), name);
        }
    }
}
