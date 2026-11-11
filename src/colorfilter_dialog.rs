//! The color filters' editor (Options > Advanced's `color_filters`, and
//! Options > Fonts and Colors' Color Filters...): the rules in order, each
//! a filter, whether it takes files and folders, and its text colour in the
//! light and the dark theme (`crate::colorfilter`).
//!
//! The colour picker is Windows' (ChooseColor); the test hook
//! `GOZ_GUI_PICK_COLOR=#RRGGBB` answers it without showing it.

use std::cell::{Cell, RefCell};

use windows_sys::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows_sys::Win32::System::SystemServices::SS_LEFT;
use windows_sys::Win32::UI::Controls::Dialogs::{
    CC_ANYCOLOR, CC_FULLOPEN, CC_RGBINIT, CHOOSECOLORW, ChooseColorW,
};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{EnableWindow, SetFocus};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    BM_GETCHECK, BM_SETCHECK, BN_CLICKED, BS_AUTOCHECKBOX, BS_DEFPUSHBUTTON, EN_CHANGE,
    ES_AUTOHSCROLL, EndDialog, GWLP_USERDATA, GetDlgItem, IDCANCEL, IDOK, LB_ADDSTRING,
    LB_GETCURSEL, LB_RESETCONTENT, LB_SETCURSEL, LBN_SELCHANGE, LBS_NOINTEGRALHEIGHT, LBS_NOTIFY,
    SendMessageW, SetWindowLongPtrW, SetWindowTextW, WM_COMMAND, WM_INITDIALOG, WS_BORDER,
    WS_EX_CLIENTEDGE, WS_TABSTOP, WS_VSCROLL,
};

use crate::colorfilter::ColorFilter;
use crate::colors::{format_color, parse_color};
use crate::dialog::{Class, Look, Template, apply_theme, center_on_owner, common, state};
use crate::wide::{window_text, wz};

const STATIC: u16 = 0xffff;
const LIST: u16 = 1000;
const FILTER: u16 = 1001;
const FILES: u16 = 1002;
const FOLDERS: u16 = 1003;
const COLOR: u16 = 1004;
const COLOR_CLEAR: u16 = 1005;
const COLOR_TEXT: u16 = 1006;
const DARK: u16 = 1007;
const DARK_CLEAR: u16 = 1008;
const DARK_TEXT: u16 = 1009;
const ADD: u16 = 1010;
const REMOVE: u16 = 1011;
const UP: u16 = 1012;
const DOWN: u16 = 1013;
const OK: u16 = IDOK as u16;
const CANCEL: u16 = IDCANCEL as u16;

struct Edit {
    rules: RefCell<Vec<ColorFilter>>,
    current: Cell<Option<usize>>,
    look: Cell<Look>,
    /// The dialog is filling its fields: their changes are its own.
    filling: Cell<bool>,
    done: Cell<bool>,
    /// The colours the picker keeps as "custom" while the dialog is open.
    custom: RefCell<[u32; 16]>,
}

/// One rule as the list shows it.
pub fn line(r: &ColorFilter) -> String {
    let kinds = match (r.files, r.folders) {
        (true, true) => "files and folders",
        (true, false) => "files",
        (false, true) => "folders",
        (false, false) => "nothing",
    };
    let c = |c: Option<u32>| c.map_or_else(|| "(none)".to_owned(), format_color);
    format!(
        "{}  \u{2014}  {kinds}  \u{2014}  {} / {}",
        if r.filter.is_empty() {
            "(empty)"
        } else {
            &r.filter
        },
        c(r.color),
        c(r.dark_color)
    )
}

/// Windows' colour picker, starting at `initial`; the test hook's colour
/// when set.
pub fn pick_color(owner: HWND, initial: Option<u32>, custom: &mut [u32; 16]) -> Option<u32> {
    if let Ok(v) = std::env::var("GOZ_GUI_PICK_COLOR") {
        return parse_color(&v);
    }
    let mut cc = CHOOSECOLORW {
        lStructSize: size_of::<CHOOSECOLORW>() as u32,
        hwndOwner: owner,
        rgbResult: initial.unwrap_or(0),
        lpCustColors: custom.as_mut_ptr(),
        Flags: CC_RGBINIT | CC_FULLOPEN | CC_ANYCOLOR,
        ..Default::default()
    };
    // SAFETY: a modal dialog over this thread's window; the struct and the
    // custom colours outlive it. Nothing of the caller's is borrowed.
    (unsafe { ChooseColorW(&mut cc) } != 0).then_some(cc.rgbResult & 0x00ff_ffff)
}

/// Edits `rules`; the new rules at OK, `None` at Cancel.
pub fn edit(owner: HWND, rules: Vec<ColorFilter>, look: Look) -> Option<Vec<ColorFilter>> {
    let s = Edit {
        current: Cell::new(if rules.is_empty() { None } else { Some(0) }),
        rules: RefCell::new(rules),
        look: Cell::new(look),
        filling: Cell::new(false),
        done: Cell::new(false),
        custom: RefCell::new([0x00ff_ffff; 16]),
    };
    let mut t = Template::new("Color Filters", 0, 300, 196);
    t.control(
        Class::Static,
        "Rows whose name matches a filter draw their text in its colour; the first that matches wins.",
        STATIC,
        SS_LEFT,
        0,
        (7, 7, 286, 18),
    )
    .control(
        Class::Named("LISTBOX"),
        "",
        LIST,
        WS_TABSTOP | WS_VSCROLL | WS_BORDER | (LBS_NOTIFY | LBS_NOINTEGRALHEIGHT) as u32,
        0,
        (7, 27, 228, 80),
    )
    .control(Class::Button, "&Add", ADD, WS_TABSTOP, 0, (241, 27, 52, 14))
    .control(Class::Button, "&Remove", REMOVE, WS_TABSTOP, 0, (241, 45, 52, 14))
    .control(Class::Button, "Move &Up", UP, WS_TABSTOP, 0, (241, 63, 52, 14))
    .control(Class::Button, "Move &Down", DOWN, WS_TABSTOP, 0, (241, 81, 52, 14))
    .control(Class::Static, "&Filter:", STATIC, SS_LEFT, 0, (7, 115, 46, 8))
    .control(
        Class::Edit,
        "",
        FILTER,
        WS_TABSTOP | ES_AUTOHSCROLL as u32,
        WS_EX_CLIENTEDGE,
        (55, 113, 180, 13),
    )
    .control(
        Class::Button,
        "F&iles",
        FILES,
        WS_TABSTOP | BS_AUTOCHECKBOX as u32,
        0,
        (55, 131, 50, 10),
    )
    .control(
        Class::Button,
        "F&olders",
        FOLDERS,
        WS_TABSTOP | BS_AUTOCHECKBOX as u32,
        0,
        (111, 131, 60, 10),
    )
    .control(Class::Static, "&Color:", STATIC, SS_LEFT, 0, (7, 148, 46, 8))
    .control(Class::Button, "C&hoose...", COLOR, WS_TABSTOP, 0, (55, 145, 52, 14))
    .control(Class::Button, "Cl&ear", COLOR_CLEAR, WS_TABSTOP, 0, (111, 145, 40, 14))
    .control(Class::Static, "", COLOR_TEXT, SS_LEFT, 0, (157, 148, 78, 8))
    .control(Class::Static, "Dar&k color:", STATIC, SS_LEFT, 0, (7, 166, 46, 8))
    .control(Class::Button, "Choo&se...", DARK, WS_TABSTOP, 0, (55, 163, 52, 14))
    .control(Class::Button, "Clea&r", DARK_CLEAR, WS_TABSTOP, 0, (111, 163, 40, 14))
    .control(Class::Static, "", DARK_TEXT, SS_LEFT, 0, (157, 166, 78, 8))
    .control(
        Class::Button,
        "OK",
        OK,
        WS_TABSTOP | BS_DEFPUSHBUTTON as u32,
        0,
        (187, 178, 50, 14),
    )
    .control(Class::Button, "Cancel", CANCEL, WS_TABSTOP, 0, (243, 178, 50, 14));
    t.run(owner, Some(proc), (&raw const s) as LPARAM);
    s.done.get().then(|| s.rules.into_inner())
}

fn item(dlg: HWND, id: u16) -> HWND {
    // SAFETY: looks up a child of this thread's dialog.
    unsafe { GetDlgItem(dlg, i32::from(id)) }
}

fn send(dlg: HWND, id: u16, msg: u32, w: usize, l: isize) -> isize {
    // SAFETY: a message to a control of this thread's dialog; a pointer in
    // `l` is the caller's and outlives the call.
    unsafe { SendMessageW(item(dlg, id), msg, w, l) }
}

fn set_text(dlg: HWND, id: u16, text: &str) {
    // SAFETY: a NUL-terminated string for a control of this dialog.
    unsafe { SetWindowTextW(item(dlg, id), wz(text).as_ptr()) };
}

fn enable(dlg: HWND, id: u16, on: bool) {
    // SAFETY: a control of this thread's dialog.
    unsafe { EnableWindow(item(dlg, id), on.into()) };
}

impl Edit {
    fn fill_list(&self, dlg: HWND) {
        self.filling.set(true);
        send(dlg, LIST, LB_RESETCONTENT, 0, 0);
        for r in self.rules.borrow().iter() {
            let w = wz(&line(r));
            send(dlg, LIST, LB_ADDSTRING, 0, w.as_ptr() as isize);
        }
        if let Some(i) = self.current.get() {
            send(dlg, LIST, LB_SETCURSEL, i, 0);
        }
        self.filling.set(false);
        self.fill_fields(dlg);
    }

    fn fill_fields(&self, dlg: HWND) {
        let rule = self
            .current
            .get()
            .and_then(|i| self.rules.borrow().get(i).cloned());
        self.filling.set(true);
        let r = rule.clone().unwrap_or_default();
        set_text(dlg, FILTER, &r.filter);
        send(dlg, FILES, BM_SETCHECK, usize::from(r.files), 0);
        send(dlg, FOLDERS, BM_SETCHECK, usize::from(r.folders), 0);
        let c = |c: Option<u32>| c.map_or_else(|| "(none)".to_owned(), format_color);
        set_text(dlg, COLOR_TEXT, &c(r.color));
        set_text(dlg, DARK_TEXT, &c(r.dark_color));
        self.filling.set(false);
        let any = rule.is_some();
        for id in [
            FILTER,
            FILES,
            FOLDERS,
            COLOR,
            COLOR_CLEAR,
            DARK,
            DARK_CLEAR,
            REMOVE,
        ] {
            enable(dlg, id, any);
        }
        let i = self.current.get().unwrap_or(0);
        let n = self.rules.borrow().len();
        enable(dlg, UP, any && i > 0);
        enable(dlg, DOWN, any && i + 1 < n);
    }

    /// Changes the chosen rule as `f` says and shows it again.
    fn change(&self, dlg: HWND, f: impl FnOnce(&mut ColorFilter)) {
        let Some(i) = self.current.get() else { return };
        {
            let mut rules = self.rules.borrow_mut();
            let Some(r) = rules.get_mut(i) else { return };
            f(r);
        }
        let text = line(&self.rules.borrow()[i]);
        self.filling.set(true);
        // The list's line again, in place.
        send(
            dlg,
            LIST,
            windows_sys::Win32::UI::WindowsAndMessaging::LB_DELETESTRING,
            i,
            0,
        );
        let w = wz(&text);
        send(
            dlg,
            LIST,
            windows_sys::Win32::UI::WindowsAndMessaging::LB_INSERTSTRING,
            i,
            w.as_ptr() as isize,
        );
        send(dlg, LIST, LB_SETCURSEL, i, 0);
        self.filling.set(false);
    }

    fn pick(&self, dlg: HWND, dark: bool) {
        let Some(i) = self.current.get() else { return };
        let initial = self
            .rules
            .borrow()
            .get(i)
            .and_then(|r| if dark { r.dark_color } else { r.color });
        let mut custom = *self.custom.borrow();
        let picked = pick_color(dlg, initial, &mut custom);
        *self.custom.borrow_mut() = custom;
        if let Some(c) = picked {
            self.change(dlg, |r| {
                if dark {
                    r.dark_color = Some(c);
                } else {
                    r.color = Some(c);
                }
            });
            self.fill_fields(dlg);
        }
    }

    fn command(&self, dlg: HWND, id: u16, code: u32) -> bool {
        match (id, code) {
            (OK, _) => {
                self.done.set(true);
                // SAFETY: ends this modal dialog.
                unsafe { EndDialog(dlg, 1) };
            }
            // SAFETY: ends this modal dialog.
            (CANCEL, _) => unsafe {
                EndDialog(dlg, 0);
            },
            (LIST, LBN_SELCHANGE) if !self.filling.get() => {
                let at = send(dlg, LIST, LB_GETCURSEL, 0, 0);
                self.current.set(usize::try_from(at).ok());
                self.fill_fields(dlg);
            }
            (FILTER, EN_CHANGE) if !self.filling.get() => {
                let text = window_text(item(dlg, FILTER));
                self.change(dlg, |r| r.filter = text);
            }
            (FILES | FOLDERS, BN_CLICKED) => {
                let on = send(dlg, id, BM_GETCHECK, 0, 0) == 1;
                self.change(dlg, |r| {
                    if id == FILES {
                        r.files = on;
                    } else {
                        r.folders = on;
                    }
                });
            }
            (COLOR, BN_CLICKED) => self.pick(dlg, false),
            (DARK, BN_CLICKED) => self.pick(dlg, true),
            (COLOR_CLEAR | DARK_CLEAR, BN_CLICKED) => {
                self.change(dlg, |r| {
                    if id == COLOR_CLEAR {
                        r.color = None;
                    } else {
                        r.dark_color = None;
                    }
                });
                self.fill_fields(dlg);
            }
            (ADD, BN_CLICKED) => {
                let n = {
                    let mut rules = self.rules.borrow_mut();
                    rules.push(ColorFilter {
                        filter: "*.txt".into(),
                        ..ColorFilter::default()
                    });
                    rules.len()
                };
                self.current.set(Some(n - 1));
                self.fill_list(dlg);
                // SAFETY: a control of this thread's dialog.
                unsafe { SetFocus(item(dlg, FILTER)) };
            }
            (REMOVE, BN_CLICKED) => {
                if let Some(i) = self.current.get() {
                    let n = {
                        let mut rules = self.rules.borrow_mut();
                        if i < rules.len() {
                            rules.remove(i);
                        }
                        rules.len()
                    };
                    self.current
                        .set(if n == 0 { None } else { Some(i.min(n - 1)) });
                    self.fill_list(dlg);
                }
            }
            (UP | DOWN, BN_CLICKED) => {
                if let Some(i) = self.current.get() {
                    let j = if id == UP {
                        i.checked_sub(1)
                    } else {
                        Some(i + 1)
                    };
                    let moved = {
                        let mut rules = self.rules.borrow_mut();
                        match j {
                            Some(j) if j < rules.len() => {
                                rules.swap(i, j);
                                Some(j)
                            }
                            _ => None,
                        }
                    };
                    if let Some(j) = moved {
                        self.current.set(Some(j));
                        self.fill_list(dlg);
                    }
                }
            }
            _ => return false,
        }
        true
    }
}

unsafe extern "system" fn proc(dlg: HWND, msg: u32, w: WPARAM, l: LPARAM) -> isize {
    if msg == WM_INITDIALOG {
        // SAFETY: `l` is the `Edit` that `edit` keeps alive until the
        // dialog has ended.
        unsafe { SetWindowLongPtrW(dlg, GWLP_USERDATA, l) };
    }
    // SAFETY: GWLP_USERDATA is the `Edit` from `edit`, or null.
    let Some(s) = (unsafe { state::<Edit>(dlg) }) else {
        return 0;
    };
    if let Some(answer) = common(dlg, msg, w, l, &s.look) {
        return answer;
    }
    match msg {
        WM_INITDIALOG => {
            s.fill_list(dlg);
            apply_theme(dlg, s.look.get().dark);
            center_on_owner(dlg);
            1
        }
        WM_COMMAND => {
            let (id, code) = ((w & 0xffff) as u16, ((w >> 16) & 0xffff) as u32);
            isize::from(s.command(dlg, id, code))
        }
        _ => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_rule_reads_as_one_line() {
        let r = ColorFilter {
            filter: "*.exe".into(),
            files: true,
            folders: false,
            color: parse_color("#ff0000"),
            dark_color: None,
        };
        assert_eq!(
            line(&r),
            "*.exe  \u{2014}  files  \u{2014}  #ff0000 / (none)"
        );
        assert!(line(&ColorFilter::default()).starts_with("(empty)"));
    }
}
