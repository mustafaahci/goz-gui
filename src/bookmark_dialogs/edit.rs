//! The bookmark editor: New Bookmark (Add to Bookmarks..., Organize's
//! New...) and Edit Bookmark. Its fields are those goz-gui has, in the
//! order Everything's editor puts them: Name; Match case and Match path,
//! each "(No change)", "Enabled" or "Disabled"; the Search, "(No change)"
//! (the search in the box stays) or "Custom" with its text beside, on
//! several lines if wanted (Shift+Enter starts a line; Enter is OK; an
//! empty one clears the box); the Filter;
//! the Sort and its direction. "(No change)" leaves the window's own when
//! the bookmark is opened.
//!
//! A filter or sort goz-gui does not have (one of the user's Everything
//! filters, a sort by Extension) is listed as it is, and stays unless
//! another is chosen; the columns goz-gui does not edit are not touched.

use std::cell::{Cell, RefCell};

use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows_sys::Win32::Graphics::Gdi::{GetWindowDC, ReleaseDC};
use windows_sys::Win32::System::SystemServices::SS_LEFT;
use windows_sys::Win32::UI::Controls::EM_REPLACESEL;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    GetKeyState, VK_CONTROL, VK_RETURN, VK_SHIFT,
};
use windows_sys::Win32::UI::Shell::{DefSubclassProc, RemoveWindowSubclass, SetWindowSubclass};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    BS_DEFPUSHBUTTON, CB_ADDSTRING, CB_GETCURSEL, CB_SETCURSEL, CBN_SELCHANGE, CBS_DROPDOWNLIST,
    EN_CHANGE, ES_AUTOHSCROLL, ES_AUTOVSCROLL, ES_MULTILINE, EndDialog, GWLP_USERDATA, GetParent,
    GetWindowRect, GetWindowTextLengthW, IDCANCEL, IDOK, MINMAXINFO, SetWindowLongPtrW, WM_CHAR,
    WM_COMMAND, WM_DESTROY, WM_GETMINMAXINFO, WM_INITDIALOG, WM_KEYDOWN, WM_NCDESTROY, WM_NCPAINT,
    WM_SIZE, WS_EX_CLIENTEDGE, WS_TABSTOP, WS_THICKFRAME, WS_VSCROLL,
};

use super::{enable, item, send, set_text};
use crate::bookmarks::{Fields, SORT_KEYS, one_line, sort_name};
use crate::dialog::{
    Class, GROW_X, GROW_Y, Look, MOVE_X, MOVE_Y, Template, Units, anchor, apply_theme,
    center_on_owner, common, state,
};
use crate::filter::Filter;
use crate::paint::fill;
use crate::places::{self, Which};
use crate::theme::Palette;
use crate::wide::{window_text, wz};

const STATIC: u16 = 0xffff;
const ID_NAME: u16 = 100;
const ID_CASE: u16 = 101;
const ID_PATH: u16 = 102;
const ID_SEARCH: u16 = 103;
const ID_FILTER: u16 = 104;
const ID_SORT: u16 = 105;
const ID_DIR: u16 = 106;
const ID_SEARCH_MODE: u16 = 107;
const ID_FILTER_LABEL: u16 = 108;
const ID_SORT_LABEL: u16 = 109;
const OK: u16 = IDOK as u16;
const CANCEL: u16 = IDCANCEL as u16;

/// The template's size, in dialog units: the editor's smallest.
const SIZE: (i16, i16) = (280, 172);
/// Where the controls that follow the editor's size are in the template.
const NAME: Units = (72, 7, 201, 14);
const SEARCH: Units = (132, 61, 141, 40);
const FILTER_LABEL: Units = (7, 109, 62, 8);
const FILTER: Units = (72, 107, 201, 140);
const SORT_LABEL: Units = (7, 127, 62, 8);
const SORT: Units = (72, 125, 140, 100);
const DIR: Units = (216, 125, 57, 60);
const OK_AT: Units = (169, 151, 50, 14);
const CANCEL_AT: Units = (223, 151, 50, 14);
/// How they follow it: the boxes and lists wider, the search taller (the
/// rows below it lower), the buttons at the bottom right.
const ANCHORS: [(u16, Units, u8); 9] = [
    (ID_NAME, NAME, GROW_X),
    (ID_SEARCH, SEARCH, GROW_X | GROW_Y),
    (ID_FILTER_LABEL, FILTER_LABEL, MOVE_Y),
    (ID_FILTER, FILTER, MOVE_Y | GROW_X),
    (ID_SORT_LABEL, SORT_LABEL, MOVE_Y),
    (ID_SORT, SORT, MOVE_Y | GROW_X),
    (ID_DIR, DIR, MOVE_X | MOVE_Y),
    (OK, OK_AT, MOVE_X | MOVE_Y),
    (CANCEL, CANCEL_AT, MOVE_X | MOVE_Y),
];

/// The match options' choices, Everything's.
const TRI: [&str; 3] = ["(No change)", "Enabled", "Disabled"];
const NO_CHANGE: &str = "(No change)";
/// The Search's choices, Everything's: the search in the box stays, or the
/// one in the edit beside (an empty one too) replaces it.
const SEARCH_MODES: [&str; 2] = [NO_CHANGE, "Custom"];

/// The bookmark's search for what the editor shows: none ("(No change)"),
/// or the text, which is the search as it was when it was not edited (its
/// line breaks as they were).
fn search_of(was: Option<&str>, custom: bool, text: String) -> Option<String> {
    if !custom {
        return None;
    }
    match was {
        Some(s) if crlf(s) == text => Some(s.to_owned()),
        _ => Some(text),
    }
}
const DIRECTIONS: [&str; 2] = ["Ascending", "Descending"];

fn tri_index(value: Option<bool>) -> usize {
    match value {
        None => 0,
        Some(true) => 1,
        Some(false) => 2,
    }
}

fn tri_value(index: usize) -> Option<bool> {
    match index {
        1 => Some(true),
        2 => Some(false),
        _ => None,
    }
}

/// A combo's choices after "(No change)": goz-gui's `known`, and the
/// column's text `now` when it is none of them (kept as it is). The index
/// of `now` among the combo's items (0 for "(No change)").
fn choices(known: &[&str], now: &str) -> (Vec<String>, usize) {
    let mut items: Vec<String> = known.iter().map(|&s| s.to_owned()).collect();
    let at = if now.is_empty() {
        0
    } else if let Some(i) = known
        .iter()
        .position(|k| k.eq_ignore_ascii_case(now.trim()))
    {
        i + 1
    } else {
        items.push(now.to_owned());
        items.len()
    };
    (items, at)
}

/// The column's text for the combo's item `index`: the column as it was
/// when the item stands for it unchanged.
fn chosen(items: &[String], index: usize, was: &str) -> String {
    match index.checked_sub(1).and_then(|i| items.get(i)) {
        None => String::new(),
        Some(text) if text.eq_ignore_ascii_case(was.trim()) => was.to_owned(),
        Some(text) => text.clone(),
    }
}

/// `text` with every line break as CRLF, as a multi-line box shows it.
fn crlf(text: &str) -> String {
    text.replace("\r\n", "\n")
        .replace('\r', "\n")
        .replace('\n', "\r\n")
}

/// The editor's state, in its GWLP_USERDATA while it is open.
struct Edit {
    was: Fields,
    filters: Vec<String>,
    sorts: Vec<String>,
    look: Cell<Look>,
    result: RefCell<Option<Fields>>,
    /// The editor's size as it opened, the smallest it can be made.
    min: Cell<(i32, i32)>,
}

/// Shows the editor titled `title` over `owner` with `fields`; the fields
/// as the user left them on OK.
pub fn edit(owner: HWND, title: &str, fields: &Fields, look: Look) -> Option<Fields> {
    let filter_titles: Vec<&str> = Filter::all().iter().map(|f| f.title()).collect();
    let sort_names: Vec<&str> = SORT_KEYS.iter().map(|&k| sort_name(k)).collect();
    let (filters, _) = choices(&filter_titles, &fields.filter);
    let (sorts, _) = choices(&sort_names, &fields.sort);
    let state = Edit {
        was: fields.clone(),
        filters,
        sorts,
        look: Cell::new(look),
        result: RefCell::new(None),
        min: Cell::new((0, 0)),
    };
    let label = SS_LEFT;
    let combo = WS_TABSTOP | WS_VSCROLL | CBS_DROPDOWNLIST as u32;
    let (lx, lw, fx) = (7, 62, 72);
    let mut t = Template::new(title, WS_THICKFRAME, SIZE.0, SIZE.1);
    t.control(Class::Static, "&Name:", STATIC, label, 0, (lx, 9, lw, 8))
        .control(
            Class::Edit,
            "",
            ID_NAME,
            WS_TABSTOP | ES_AUTOHSCROLL as u32,
            WS_EX_CLIENTEDGE,
            NAME,
        )
        .control(
            Class::Static,
            "Match &case:",
            STATIC,
            label,
            0,
            (lx, 27, lw, 8),
        )
        .control(
            Class::Named("ComboBox"),
            "",
            ID_CASE,
            combo,
            0,
            (fx, 25, 80, 80),
        )
        .control(
            Class::Static,
            "Match &path:",
            STATIC,
            label,
            0,
            (lx, 45, lw, 8),
        )
        .control(
            Class::Named("ComboBox"),
            "",
            ID_PATH,
            combo,
            0,
            (fx, 43, 80, 80),
        )
        .control(Class::Static, "&Search:", STATIC, label, 0, (lx, 63, lw, 8))
        // Everything's: "(No change)" or "Custom", the text beside it.
        .control(
            Class::Named("ComboBox"),
            "",
            ID_SEARCH_MODE,
            combo,
            0,
            (fx, 61, 56, 60),
        )
        .control(
            Class::Edit,
            "",
            ID_SEARCH,
            WS_TABSTOP | WS_VSCROLL | (ES_MULTILINE | ES_AUTOVSCROLL) as u32,
            WS_EX_CLIENTEDGE,
            SEARCH,
        )
        .control(
            Class::Static,
            "&Filter:",
            ID_FILTER_LABEL,
            label,
            0,
            FILTER_LABEL,
        )
        .control(Class::Named("ComboBox"), "", ID_FILTER, combo, 0, FILTER)
        .control(Class::Static, "Sor&t:", ID_SORT_LABEL, label, 0, SORT_LABEL)
        .control(Class::Named("ComboBox"), "", ID_SORT, combo, 0, SORT)
        .control(Class::Named("ComboBox"), "", ID_DIR, combo, 0, DIR)
        .control(
            Class::Button,
            "OK",
            OK,
            WS_TABSTOP | BS_DEFPUSHBUTTON as u32,
            0,
            OK_AT,
        )
        .control(Class::Button, "Cancel", CANCEL, WS_TABSTOP, 0, CANCEL_AT);
    t.run(owner, Some(edit_proc), (&raw const state) as LPARAM);
    state.result.into_inner()
}

fn fill_combo(dlg: HWND, id: u16, items: impl IntoIterator<Item = String>, select: usize) {
    let combo = item(dlg, id);
    for text in items {
        let text = wz(&text);
        send(combo, CB_ADDSTRING, 0, text.as_ptr() as isize);
    }
    send(combo, CB_SETCURSEL, select, 0);
}

fn selection(dlg: HWND, id: u16) -> usize {
    usize::try_from(send(item(dlg, id), CB_GETCURSEL, 0, 0)).unwrap_or(0)
}

impl Edit {
    fn init(&self, dlg: HWND) {
        let f = &self.was;
        set_text(item(dlg, ID_NAME), &one_line(&f.name));
        set_text(
            item(dlg, ID_SEARCH),
            &crlf(f.search.as_deref().unwrap_or_default()),
        );
        let custom = f.search.is_some();
        fill_combo(
            dlg,
            ID_SEARCH_MODE,
            SEARCH_MODES.iter().map(|&s| s.to_owned()),
            usize::from(custom),
        );
        enable(dlg, ID_SEARCH, custom);
        let tri = || TRI.iter().map(|&s| s.to_owned());
        fill_combo(dlg, ID_CASE, tri(), tri_index(f.match_case));
        fill_combo(dlg, ID_PATH, tri(), tri_index(f.match_path));
        let no_change = || std::iter::once(NO_CHANGE.to_owned());
        let filter_titles: Vec<&str> = Filter::all().iter().map(|f| f.title()).collect();
        let (_, at) = choices(&filter_titles, &f.filter);
        fill_combo(
            dlg,
            ID_FILTER,
            no_change().chain(self.filters.iter().cloned()),
            at,
        );
        let sort_names: Vec<&str> = SORT_KEYS.iter().map(|&k| sort_name(k)).collect();
        let (_, at) = choices(&sort_names, &f.sort);
        fill_combo(
            dlg,
            ID_SORT,
            no_change().chain(self.sorts.iter().cloned()),
            at,
        );
        fill_combo(
            dlg,
            ID_DIR,
            DIRECTIONS.iter().map(|&s| s.to_owned()),
            usize::from(f.descending),
        );
        enable(dlg, ID_DIR, at != 0);
        enable(dlg, OK, !f.name.is_empty());
        // SAFETY: subclasses this thread's search box; the subclass removes
        // itself when the box goes.
        unsafe { SetWindowSubclass(item(dlg, ID_SEARCH), Some(search_proc), 1, 0) };
        apply_theme(dlg, self.look.get().dark);
        center_on_owner(dlg);
        let mut wr = RECT::default();
        // SAFETY: reads this thread's dialog.
        unsafe { GetWindowRect(dlg, &mut wr) };
        self.min.set((wr.right - wr.left, wr.bottom - wr.top));
        places::restore(
            dlg,
            Which::BookmarkEdit,
            core::ptr::null_mut(),
            self.min.get(),
        );
        anchor(dlg, SIZE, &ANCHORS);
    }

    /// The fields as the dialog shows them.
    fn read(&self, dlg: HWND) -> Fields {
        let was = &self.was;
        // A text shown on one line (a name on several) comes back as it
        // was unless it was edited.
        let name = window_text(item(dlg, ID_NAME));
        let name = if name == one_line(&was.name) {
            was.name.clone()
        } else {
            name
        };
        let search = search_of(
            was.search.as_deref(),
            selection(dlg, ID_SEARCH_MODE) == 1,
            window_text(item(dlg, ID_SEARCH)),
        );
        let sort_index = selection(dlg, ID_SORT);
        let sort = chosen(&self.sorts, sort_index, &was.sort);
        Fields {
            name,
            search,
            match_case: tri_value(selection(dlg, ID_CASE)),
            match_path: tri_value(selection(dlg, ID_PATH)),
            filter: chosen(&self.filters, selection(dlg, ID_FILTER), &was.filter),
            // With no sort the direction is left as it was.
            descending: if sort_index == 0 {
                was.descending
            } else {
                selection(dlg, ID_DIR) == 1
            },
            sort,
        }
    }

    fn command(&self, dlg: HWND, id: u16, code: u32) -> bool {
        match id {
            OK => {
                *self.result.borrow_mut() = Some(self.read(dlg));
                // SAFETY: ends this modal dialog.
                unsafe { EndDialog(dlg, 1) };
            }
            // SAFETY: ends this modal dialog.
            CANCEL => unsafe {
                EndDialog(dlg, 0);
            },
            // A bookmark needs a name.
            ID_NAME if code == EN_CHANGE => {
                // SAFETY: reads a control of this thread's dialog.
                let named = unsafe { GetWindowTextLengthW(item(dlg, ID_NAME)) } > 0;
                enable(dlg, OK, named);
            }
            // The direction goes with a sort.
            ID_SORT if code == CBN_SELCHANGE => enable(dlg, ID_DIR, selection(dlg, ID_SORT) != 0),
            // The text goes with "Custom".
            ID_SEARCH_MODE if code == CBN_SELCHANGE => {
                enable(dlg, ID_SEARCH, selection(dlg, ID_SEARCH_MODE) == 1);
            }
            _ => return false,
        }
        true
    }
}

fn key_down(vk: u16) -> bool {
    // SAFETY: reads this thread's keyboard state.
    unsafe { GetKeyState(i32::from(vk)) < 0 }
}

/// The search box's subclass: Shift+Enter starts a new line (Enter alone
/// is OK, Ctrl+Enter a new line, as in any dialog).
unsafe extern "system" fn search_proc(
    hwnd: HWND,
    msg: u32,
    w: WPARAM,
    l: LPARAM,
    _id: usize,
    _data: usize,
) -> LRESULT {
    let shift_enter = key_down(VK_SHIFT) && !key_down(VK_CONTROL);
    // SAFETY: `w` and `l` are passed on as they came; the line break is a
    // NUL-terminated string for this thread's box; its parent is the
    // editor, whose GWLP_USERDATA is its `Edit`; the window DC is this
    // box's own, released after.
    unsafe {
        match msg {
            WM_KEYDOWN if w as u16 == VK_RETURN && shift_enter => {
                let line = wz("\r\n");
                send(hwnd, EM_REPLACESEL, 1, line.as_ptr() as isize);
                return 0;
            }
            // The character the key makes: the line is in already.
            WM_CHAR if w == 0x0d && shift_enter => return 0,
            // In the dark theme the box's edge is drawn as the one-line
            // boxes' is (the system draws a several-line box's edge
            // white).
            WM_NCPAINT => {
                let done = DefSubclassProc(hwnd, msg, w, l);
                let dark = state::<Edit>(GetParent(hwnd)).is_some_and(|s| s.look.get().dark);
                if dark {
                    let pal = Palette::new(true);
                    let mut rc = RECT::default();
                    GetWindowRect(hwnd, &mut rc);
                    let (w, h) = (rc.right - rc.left, rc.bottom - rc.top);
                    let dc = GetWindowDC(hwnd);
                    for (inset, colour) in [(0, pal.border), (1, pal.bg)] {
                        let edges = [
                            RECT {
                                left: inset,
                                top: inset,
                                right: w - inset,
                                bottom: inset + 1,
                            },
                            RECT {
                                left: inset,
                                top: h - inset - 1,
                                right: w - inset,
                                bottom: h - inset,
                            },
                            RECT {
                                left: inset,
                                top: inset,
                                right: inset + 1,
                                bottom: h - inset,
                            },
                            RECT {
                                left: w - inset - 1,
                                top: inset,
                                right: w - inset,
                                bottom: h - inset,
                            },
                        ];
                        for edge in &edges {
                            fill(dc, edge, colour);
                        }
                    }
                    ReleaseDC(hwnd, dc);
                }
                return done;
            }
            WM_NCDESTROY => {
                RemoveWindowSubclass(hwnd, Some(search_proc), 1);
            }
            _ => {}
        }
        DefSubclassProc(hwnd, msg, w, l)
    }
}

unsafe extern "system" fn edit_proc(dlg: HWND, msg: u32, w: WPARAM, l: LPARAM) -> isize {
    if msg == WM_INITDIALOG {
        // SAFETY: `l` is the `Edit` that `edit` keeps alive until the
        // dialog has ended.
        unsafe { SetWindowLongPtrW(dlg, GWLP_USERDATA, l) };
    }
    // SAFETY: GWLP_USERDATA is the `Edit` from `edit`, or null before.
    let Some(s) = (unsafe { state::<Edit>(dlg) }) else {
        return 0;
    };
    if let Some(answer) = common(dlg, msg, w, l, &s.look) {
        return answer;
    }
    match msg {
        WM_INITDIALOG => {
            s.init(dlg);
            // The dialog manager puts the keyboard in the first box, Name,
            // with its text selected.
            1
        }
        WM_COMMAND => {
            let (id, code) = ((w & 0xffff) as u16, ((w >> 16) & 0xffff) as u32);
            isize::from(s.command(dlg, id, code))
        }
        WM_SIZE => {
            anchor(dlg, SIZE, &ANCHORS);
            1
        }
        WM_GETMINMAXINFO => {
            let (w, h) = s.min.get();
            // SAFETY: WM_GETMINMAXINFO's lParam is a MINMAXINFO to fill.
            let mmi = unsafe { &mut *(l as *mut MINMAXINFO) };
            mmi.ptMinTrackSize.x = w;
            mmi.ptMinTrackSize.y = h;
            1
        }
        WM_DESTROY => {
            places::keep(dlg, Which::BookmarkEdit, core::ptr::null_mut());
            0
        }
        _ => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn combos_keep_what_goz_gui_lacks() {
        let known = ["Name", "Path", "Size", "Date Modified"];
        assert_eq!(choices(&known, ""), (known.map(String::from).to_vec(), 0));
        assert_eq!(choices(&known, " size ").1, 3);
        let (items, at) = choices(&known, "Extension");
        assert_eq!((items.len(), at), (5, 5));
        assert_eq!(items[4], "Extension");
        // Left as it was: the column's own text; another: its name.
        assert_eq!(chosen(&items, 5, "Extension"), "Extension");
        assert_eq!(chosen(&items, 3, " size "), " size ");
        assert_eq!(chosen(&items, 3, "Name"), "Size");
        assert_eq!(chosen(&items, 0, "Name"), "");
        for v in [None, Some(true), Some(false)] {
            assert_eq!(tri_value(tri_index(v)), v);
        }
        assert_eq!(crlf("a\nb\r\nc\rd"), "a\r\nb\r\nc\r\nd");
    }

    #[test]
    fn the_search_is_no_change_or_custom_as_chosen() {
        // "(No change)": none, whatever the text beside says.
        assert_eq!(search_of(Some("abc"), false, "abc".into()), None);
        assert_eq!(search_of(None, false, String::new()), None);
        // "Custom": the text, an empty one too (it clears the box).
        assert_eq!(search_of(None, true, String::new()), Some(String::new()));
        assert_eq!(
            search_of(Some("abc"), true, String::new()),
            Some(String::new())
        );
        assert_eq!(search_of(None, true, "x".into()), Some("x".into()));
        // Unedited: as it was, its line breaks too.
        assert_eq!(
            search_of(Some("a\nb"), true, "a\r\nb".into()),
            Some("a\nb".into())
        );
        assert_eq!(
            search_of(Some("a\nb"), true, "a\r\nc".into()),
            Some("a\r\nc".into())
        );
    }
}
