//! Edit > Advanced > Advanced Rename... (Shift+F2, or F2 with several rows
//! selected): Everything's multi-file renamer (its engine is `rename`).
//!
//! Old format and New format (each with the formats used before in its
//! list, and a ▸ button, or Ctrl+Space, for a menu of what can go in it:
//! Everything's `%1` to `%9`, `$number:`, the case functions...), then
//! Match case, Match diacritics, Regular expressions and Ignore extension
//! (the last two make the formats again, as in Everything). Under them,
//! the files as they would be renamed: each name, its new name and its
//! folder, as the formats are typed. A new name Windows would refuse, one
//! two files would share, or one a file there already has is shown in red
//! with why, and OK waits until there is none; a name the old format does
//! not match is left as it is. The folders' names are read once, on a
//! thread of their own, as the dialog opens.
//!
//! OK hands back the renames, done as one operation (one undo record).

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};

use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows_sys::Win32::Graphics::Gdi::{
    COLOR_BTNFACE, GetSysColorBrush, HDC, InvalidateRect, SetBkMode, SetTextColor, TRANSPARENT,
};
use windows_sys::Win32::UI::Controls::{
    CDDS_ITEMPREPAINT, CDDS_SUBITEM, CDRF_NEWFONT, CDRF_NOTIFYSUBITEMDRAW, EM_GETSEL,
    EM_REPLACESEL, EM_SETSEL, LVIF_TEXT, LVIS_SELECTED, LVM_GETITEMSTATE, LVM_SETCOLUMNWIDTH,
    LVM_SETEXTENDEDLISTVIEWSTYLE, LVM_SETITEMCOUNT, LVN_GETDISPINFOW, LVN_ODFINDITEMW,
    LVS_EX_DOUBLEBUFFER, LVS_EX_FULLROWSELECT, LVS_EX_LABELTIP, LVS_OWNERDATA, LVS_REPORT,
    LVS_SHOWSELALWAYS, LVSCW_AUTOSIZE_USEHEADER, NM_CUSTOMDRAW, NMHDR, NMLVCUSTOMDRAW,
    NMLVDISPINFOW,
};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    GetKeyState, SetFocus, VK_CONTROL, VK_SPACE,
};
use windows_sys::Win32::UI::Shell::{DefSubclassProc, RemoveWindowSubclass, SetWindowSubclass};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    BN_CLICKED, BS_AUTOCHECKBOX, BS_DEFPUSHBUTTON, CB_ADDSTRING, CB_GETCURSEL, CB_GETLBTEXT,
    CB_GETLBTEXTLEN, CBN_EDITCHANGE, CBN_SELCHANGE, CBS_AUTOHSCROLL, CBS_DROPDOWN, DWLP_MSGRESULT,
    EndDialog, FindWindowExW, GWLP_USERDATA, GetWindowRect, IDCANCEL, IDOK, KillTimer, MINMAXINFO,
    PostMessageW, SetTimer, SetWindowLongPtrW, SetWindowTextW, WM_APP, WM_CHAR, WM_COMMAND,
    WM_CTLCOLORSTATIC, WM_DESTROY, WM_GETMINMAXINFO, WM_INITDIALOG, WM_KEYDOWN, WM_NCDESTROY,
    WM_NOTIFY, WM_SIZE, WM_TIMER, WS_EX_CLIENTEDGE, WS_MAXIMIZEBOX, WS_TABSTOP, WS_THICKFRAME,
    WS_VSCROLL,
};

use super::name_of;
use super::rename::{self, FormatError, Options, Problem, Subject};
use super::ui::{self, checked, copy_text, enable, error_colour, item, send, set_check};
use crate::dialog::{
    self, Class, GROW_X, GROW_Y, Look, MOVE_X, MOVE_Y, Template, Units, apply_theme,
    center_on_owner, common, list_custom_draw, state,
};
use crate::fileops::parent;
use crate::places::{self, Which};
use crate::theme::Palette;
use crate::wide::{window_text, wz};
use crate::{menu, popup};

/// Everything's caption for it.
const TITLE: &str = "Rename";

/// What the window hands the dialog: the switches and format histories
/// kept from last time.
pub struct Start {
    pub options: Options,
    pub old_history: Vec<String>,
    pub new_history: Vec<String>,
}

/// What OK chose.
#[derive(Debug)]
pub struct Chosen {
    pub options: Options,
    pub old_format: String,
    pub new_format: String,
    /// Each file to rename and its new name.
    pub renames: Vec<(Vec<u16>, Vec<u16>)>,
}

// Everything's control ids.
const ID_OLD: u16 = 1002;
const ID_OLD_MENU: u16 = 1003;
const ID_NEW: u16 = 1007;
const ID_NEW_MENU: u16 = 1008;
const ID_CASE: u16 = 1010;
const ID_REGEX: u16 = 1011;
const ID_DIACRITICS: u16 = 1070;
const ID_EXTENSION: u16 = 1083;
const ID_LIST: u16 = 1009;
const ID_STATUS: u16 = 1100;
const STATIC: u16 = 0xffff;
const OK: u16 = IDOK as u16;
const CANCEL: u16 = IDCANCEL as u16;

/// The folders' names came: lParam is a `Box<Scan>`.
const WM_SCANNED: u32 = WM_APP + 2;
/// Make the new names again (after a pick in a format's list, which sets
/// the box's text only once its notification is done).
const WM_RECOMPUTE: u32 = WM_APP + 3;
/// Show a format's menu (Ctrl+Space in its box): wParam is the box's id.
const WM_FORMAT_MENU: u32 = WM_APP + 4;
/// Make the formats and the new names of many files, after the dialog
/// opened: the window's thread is not held the whole time at once.
const WM_REGENERATE: u32 = WM_APP + 5;
/// The timer that makes the new names of many files a moment after the
/// typing stops.
const TIMER_RECOMPUTE: usize = 1;
const RECOMPUTE_MS: u32 = 120;
/// The folders' names not read by then (a network folder that does not
/// answer), OK does without them: Windows' own question comes if a name
/// turns out to be taken.
const TIMER_SCAN: usize = 2;
const SCAN_MS: u32 = 3000;
/// More files than this are renamed in preview after the typing stops.
const MANY: usize = 1000;
/// The subclass id of the format boxes.
const FORMAT_BOX: usize = 0x5246;

/// Dialog units: its size, and each control's place, with how it follows
/// the dialog's size.
const SIZE: (i16, i16) = (300, 250);
const fn rect(x: i16, y: i16, w: i16, h: i16) -> Units {
    (x, y, w, h)
}
const PLACES: [(u16, Units, u8); 8] = [
    (ID_OLD, rect(52, 7, SIZE.0 - 52 - 27, 150), GROW_X),
    (ID_OLD_MENU, rect(SIZE.0 - 25, 6, 18, 14), MOVE_X),
    (ID_NEW, rect(52, 25, SIZE.0 - 52 - 27, 150), GROW_X),
    (ID_NEW_MENU, rect(SIZE.0 - 25, 24, 18, 14), MOVE_X),
    (
        ID_LIST,
        rect(7, 82, SIZE.0 - 14, SIZE.1 - 82 - 29),
        GROW_X | GROW_Y,
    ),
    // Two lines: a problem's words can take more than one.
    (
        ID_STATUS,
        rect(7, SIZE.1 - 23, SIZE.0 - 7 - 120, 18),
        MOVE_Y | GROW_X,
    ),
    (OK, rect(SIZE.0 - 111, SIZE.1 - 21, 50, 14), MOVE_X | MOVE_Y),
    (
        CANCEL,
        rect(SIZE.0 - 57, SIZE.1 - 21, 50, 14),
        MOVE_X | MOVE_Y,
    ),
];

/// A file to rename.
struct File {
    path: Vec<u16>,
    name: String,
    folder: String,
}

/// The names in each folder of the files, lower case, by folder (lower
/// case): what a new name may run into.
type Scan = HashMap<String, HashSet<String>>;

/// The new names as the formats make them, and what is wrong.
#[derive(Default)]
struct Preview {
    new: Vec<Option<String>>,
    problems: Vec<Option<Problem>>,
    /// What is wrong with a format.
    error: Option<String>,
}

struct Dlg {
    look: Cell<Look>,
    files: Vec<File>,
    subjects: Vec<Subject>,
    start: Start,
    preview: RefCell<Preview>,
    scan: RefCell<Option<Scan>>,
    min: Cell<(i32, i32)>,
    out: RefCell<Option<Chosen>>,
    /// The first formats and names are still to be made (`WM_REGENERATE`).
    regenerating: Cell<bool>,
}

/// Shows the renamer over `owner` for `files` (each path, and whether it
/// is a folder); what OK chose, `None` for Cancel or nothing to rename.
pub fn run(owner: HWND, files: &[(Vec<u16>, bool)], start: Start, look: Look) -> Option<Chosen> {
    let list: Vec<File> = files
        .iter()
        .map(|(path, _)| File {
            path: path.clone(),
            name: String::from_utf16_lossy(name_of(path)),
            folder: String::from_utf16_lossy(parent(path).unwrap_or(&[])),
        })
        .collect();
    let subjects = list
        .iter()
        .zip(files)
        .map(|(f, (_, is_dir))| Subject::new(&f.name, *is_dir))
        .collect();
    let state = Dlg {
        look: Cell::new(look),
        files: list,
        subjects,
        start,
        preview: RefCell::new(Preview::default()),
        scan: RefCell::new(None),
        min: Cell::new((0, 0)),
        out: RefCell::new(None),
        regenerating: Cell::new(false),
    };
    let mut t = Template::new(TITLE, WS_THICKFRAME | WS_MAXIMIZEBOX, SIZE.0, SIZE.1);
    let combo = WS_TABSTOP | WS_VSCROLL | (CBS_DROPDOWN | CBS_AUTOHSCROLL) as u32;
    t.control(Class::Static, "Old &format:", STATIC, 0, 0, (7, 9, 44, 8))
        .control(Class::Named("ComboBox"), "", ID_OLD, combo, 0, PLACES[0].1)
        .control(
            Class::Button,
            "\u{25B8}",
            ID_OLD_MENU,
            WS_TABSTOP,
            0,
            PLACES[1].1,
        )
        .control(Class::Static, "New for&mat:", STATIC, 0, 0, (7, 27, 44, 8))
        .control(Class::Named("ComboBox"), "", ID_NEW, combo, 0, PLACES[2].1)
        .control(
            Class::Button,
            "\u{25B8}",
            ID_NEW_MENU,
            WS_TABSTOP,
            0,
            PLACES[3].1,
        );
    let check = WS_TABSTOP | BS_AUTOCHECKBOX as u32;
    t.control(
        Class::Button,
        "Match &case",
        ID_CASE,
        check,
        0,
        (52, 43, 110, 10),
    )
    .control(
        Class::Button,
        "Match &diacritics",
        ID_DIACRITICS,
        check,
        0,
        (166, 43, 110, 10),
    )
    .control(
        Class::Button,
        "&Regular expressions",
        ID_REGEX,
        check,
        0,
        (52, 55, 110, 10),
    )
    .control(
        Class::Button,
        "Ignore &extension",
        ID_EXTENSION,
        check,
        0,
        (166, 55, 110, 10),
    )
    .control(
        Class::Static,
        "&New filenames:",
        STATIC,
        0,
        0,
        (7, 72, 100, 8),
    )
    .control(
        Class::Named("SysListView32"),
        "",
        ID_LIST,
        WS_TABSTOP | LVS_REPORT | LVS_OWNERDATA | LVS_SHOWSELALWAYS,
        WS_EX_CLIENTEDGE,
        PLACES[4].1,
    )
    .control(Class::Static, "", ID_STATUS, 0, 0, PLACES[5].1)
    .control(
        Class::Button,
        "OK",
        OK,
        WS_TABSTOP | BS_DEFPUSHBUTTON as u32,
        0,
        PLACES[6].1,
    )
    .control(Class::Button, "Cancel", CANCEL, WS_TABSTOP, 0, PLACES[7].1);
    t.run(owner, Some(dialog_proc), (&raw const state) as LPARAM);
    state.out.into_inner()
}

/// The names in each of `folders` (lower case), read on another thread;
/// each folder once, whatever its case.
fn scan(mut folders: Vec<String>) -> Scan {
    folders.sort_by_cached_key(|f| f.to_lowercase());
    folders.dedup_by(|a, b| a.eq_ignore_ascii_case(b));
    folders
        .into_iter()
        .map(|folder| {
            let names = std::fs::read_dir(&folder)
                .map(|d| {
                    d.filter_map(|e| e.ok())
                        .map(|e| e.file_name().to_string_lossy().to_lowercase())
                        .collect()
                })
                .unwrap_or_default();
            (folder.to_lowercase(), names)
        })
        .collect()
}

/// The text of a list's item `i` of combo box `combo`.
fn combo_item(combo: HWND, i: usize) -> String {
    let len = send(combo, CB_GETLBTEXTLEN, i, 0);
    let Ok(len) = usize::try_from(len) else {
        return String::new();
    };
    let mut buf = vec![0u16; len + 1];
    send(combo, CB_GETLBTEXT, i, buf.as_mut_ptr() as isize);
    String::from_utf16_lossy(&buf[..len])
}

/// The list's last column (Path) takes the room the others leave.
fn fill_last_column(list: HWND) {
    send(
        list,
        LVM_SETCOLUMNWIDTH,
        2,
        LVSCW_AUTOSIZE_USEHEADER as isize,
    );
}

/// The edit box of a drop-down combo box.
fn combo_edit(combo: HWND) -> HWND {
    let class = wz("Edit");
    // SAFETY: looks up a child of this thread's combo box by class.
    unsafe {
        FindWindowExW(
            combo,
            core::ptr::null_mut(),
            class.as_ptr(),
            core::ptr::null(),
        )
    }
}

/// A format menu's item: what it puts in the box (`wrap`: around the
/// selected text, `<name:` and `>`), and its label.
struct Insert {
    text: &'static str,
    wrap: bool,
    label: &'static str,
}

const fn put(text: &'static str, label: &'static str) -> Insert {
    Insert {
        text,
        wrap: false,
        label,
    }
}

/// The old format's menu (Everything's labels).
const OLD_ITEMS: [Insert; 10] = [
    put(
        "%1",
        "%1   Match any number of characters and save the match in variable 1",
    ),
    put(
        "%2",
        "%2   Match any number of characters and save the match in variable 2",
    ),
    put(
        "%3",
        "%3   Match any number of characters and save the match in variable 3",
    ),
    put(
        "%4",
        "%4   Match any number of characters and save the match in variable 4",
    ),
    put(
        "%5",
        "%5   Match any number of characters and save the match in variable 5",
    ),
    put(
        "%6",
        "%6   Match any number of characters and save the match in variable 6",
    ),
    put(
        "%7",
        "%7   Match any number of characters and save the match in variable 7",
    ),
    put(
        "%8",
        "%8   Match any number of characters and save the match in variable 8",
    ),
    put(
        "%9",
        "%9   Match any number of characters and save the match in variable 9",
    ),
    put("%%", "%%   Match a single literal %"),
];

/// With regular expressions.
const OLD_REGEX_ITEMS: [Insert; 13] = [
    put(
        "(.*?)",
        "(.*?) Match and save any number of characters (lazy)",
    ),
    put(
        "(.*)",
        "(.*)  Match and save any number of characters (greedy)",
    ),
    put(".", ".    Any single character"),
    put("*", "*    Zero or more times"),
    put("+", "+    One or more times"),
    put("^", "^    Beginning of filename"),
    put("$", "$    End of filename"),
    put("\\b", "\\b   Word boundary"),
    put("[]", "[ ]  Any one character in the set"),
    put("[^]", "[^ ] Any one character not in the set"),
    put("|", "|    Or"),
    put("\\", "\\    Escape special character"),
    put("()", "( )  Save the subexpression match"),
];

const CASE_ITEMS: [Insert; 3] = [
    Insert {
        text: "uppercase",
        wrap: true,
        label: "<uppercase:>   Convert to uppercase",
    },
    Insert {
        text: "lowercase",
        wrap: true,
        label: "<lowercase:>   Convert to lowercase",
    },
    Insert {
        text: "titlecase",
        wrap: true,
        label: "<titlecase:>   Convert to title case",
    },
];

/// The new format's own items, after the variables and before the case
/// functions.
const NEW_ITEMS: [Insert; 2] = [
    put("$number:", "$number:   The order number of the filename"),
    put("$count:", "$count:   The total number of filenames"),
];

const NEW_ESCAPES: [Insert; 2] = [
    put("%%", "%%   A single literal %"),
    put("$$", "$$   A single literal $"),
];

/// The variables of the new format: `%1`.. or (regular expressions)
/// `\0`, `\1`...
fn variables(regex: bool) -> Vec<(String, String)> {
    if regex {
        let mut v = vec![(
            "\\0".to_owned(),
            "\\0   Text matched in old format".to_owned(),
        )];
        v.extend((1..=9).map(|n| (format!("\\{n}"), format!("\\{n}   Saved match {n}"))));
        v
    } else {
        (1..=9)
            .map(|n| {
                (
                    format!("%{n}"),
                    format!("%{n}   replace with saved match in variable {n}"),
                )
            })
            .collect()
    }
}

impl Dlg {
    fn options(dlg: HWND) -> Options {
        Options {
            match_case: checked(dlg, ID_CASE),
            match_diacritics: checked(dlg, ID_DIACRITICS),
            regex: checked(dlg, ID_REGEX),
            ignore_extension: checked(dlg, ID_EXTENSION),
        }
    }

    fn init(&self, dlg: HWND) {
        let o = self.start.options;
        set_check(dlg, ID_CASE, o.match_case);
        set_check(dlg, ID_DIACRITICS, o.match_diacritics);
        set_check(dlg, ID_REGEX, o.regex);
        set_check(dlg, ID_EXTENSION, o.ignore_extension);
        for (id, history) in [
            (ID_OLD, &self.start.old_history),
            (ID_NEW, &self.start.new_history),
        ] {
            let combo = item(dlg, id);
            for h in history {
                let text = wz(h);
                send(combo, CB_ADDSTRING, 0, text.as_ptr() as isize);
            }
            // SAFETY: subclasses this thread's box (Ctrl+Space); the
            // dialog outlives it, and the subclass removes itself.
            unsafe {
                SetWindowSubclass(
                    combo_edit(combo),
                    Some(format_box_proc),
                    FORMAT_BOX,
                    dlg as usize,
                )
            };
        }
        let list = item(dlg, ID_LIST);
        let ex = LVS_EX_FULLROWSELECT | LVS_EX_DOUBLEBUFFER | LVS_EX_LABELTIP;
        send(list, LVM_SETEXTENDEDLISTVIEWSTYLE, ex as usize, ex as isize);
        ui::columns(list, &[("Name", 170), ("New Name", 200), ("Path", 120)]);
        fill_last_column(list);
        send(list, LVM_SETITEMCOUNT, self.files.len(), 0);
        apply_theme(dlg, self.look.get().dark);
        center_on_owner(dlg);
        let mut wr = RECT::default();
        // SAFETY: reads this thread's dialog.
        unsafe { GetWindowRect(dlg, &mut wr) };
        self.min.set((wr.right - wr.left, wr.bottom - wr.top));
        places::restore(dlg, Which::Rename, core::ptr::null_mut(), self.min.get());
        dialog::anchor(dlg, SIZE, &PLACES);
        // The folders' names, read (and sorted) off this thread.
        let folders: Vec<String> = self.files.iter().map(|f| f.folder.clone()).collect();
        let to = dlg as usize;
        let spawned = std::thread::Builder::new()
            .name("goz-rename-scan".into())
            .spawn(move || {
                let raw = Box::into_raw(Box::new(scan(folders)));
                // SAFETY: posts to the dialog; when it is gone the box is
                // taken back.
                unsafe {
                    if PostMessageW(to as HWND, WM_SCANNED, 0, raw as isize) == 0 {
                        drop(Box::from_raw(raw));
                    }
                }
            });
        if spawned.is_err() {
            // Without the names, nothing is said to be taken.
            *self.scan.borrow_mut() = Some(Scan::new());
        } else {
            // SAFETY: a timer of this thread's dialog.
            unsafe { SetTimer(dlg, TIMER_SCAN, SCAN_MS, None) };
        }
        if self.files.len() > MANY {
            // Many files: the formats and names are made from a message of
            // their own, the status line saying so meanwhile.
            self.regenerating.set(true);
            let z = wz("Working...");
            // SAFETY: NUL-terminated text for this thread's static, and a
            // post to this thread's dialog.
            unsafe {
                SetWindowTextW(item(dlg, ID_STATUS), z.as_ptr());
                PostMessageW(dlg, WM_REGENERATE, 0, 0);
            }
            enable(dlg, OK, false);
        } else {
            self.regenerate(dlg);
        }
    }

    /// The formats made from the files, as Everything makes them when it
    /// opens and when Regular expressions or Ignore extension change.
    fn regenerate(&self, dlg: HWND) {
        let (old, new) = rename::generate(&self.subjects, Dlg::options(dlg));
        for (id, text) in [(ID_OLD, &old), (ID_NEW, &new)] {
            let z = wz(text);
            // SAFETY: NUL-terminated text for this thread's combo box.
            unsafe { SetWindowTextW(item(dlg, id), z.as_ptr()) };
        }
        self.recompute(dlg);
    }

    /// Puts the keyboard in New format with its name, without the
    /// extension, selected (as Everything opens).
    fn focus_new(&self, dlg: HWND) {
        let new = window_text(item(dlg, ID_NEW));
        let stem = match new.rfind('.') {
            Some(i) if i > 0 && !Dlg::options(dlg).ignore_extension => {
                new[..i].encode_utf16().count()
            }
            _ => new.encode_utf16().count(),
        };
        let edit = combo_edit(item(dlg, ID_NEW));
        // SAFETY: focuses this thread's box.
        unsafe { SetFocus(edit) };
        send(edit, EM_SETSEL, 0, stem as isize);
    }

    /// The text of a format box; a pick in its list is the list's item
    /// until the box shows it.
    fn format(dlg: HWND, id: u16) -> String {
        window_text(item(dlg, id))
    }

    /// Makes the new names, finds what is wrong, and shows them.
    fn recompute(&self, dlg: HWND) {
        let o = Dlg::options(dlg);
        let (old, new) = (Dlg::format(dlg, ID_OLD), Dlg::format(dlg, ID_NEW));
        let n = self.files.len();
        let preview = match rename::rename_all(&self.subjects, &old, &new, o) {
            Ok(names) => {
                let folders: Vec<&str> = self.files.iter().map(|f| f.folder.as_str()).collect();
                let olds: Vec<&str> = self.files.iter().map(|f| f.name.as_str()).collect();
                let scan = self.scan.borrow();
                let problems = rename::problems(&folders, &olds, &names, |folder, name| {
                    scan.as_ref().is_some_and(|s| {
                        s.get(&folder.to_lowercase())
                            .is_some_and(|names| names.contains(name))
                    })
                });
                Preview {
                    new: names,
                    problems,
                    error: None,
                }
            }
            Err(e) => Preview {
                new: vec![None; n],
                problems: vec![None; n],
                error: Some(match e {
                    FormatError::Old(_) | FormatError::New(_) => e.to_string(),
                }),
            },
        };
        *self.preview.borrow_mut() = preview;
        // SAFETY: repaints this thread's list.
        unsafe { InvalidateRect(item(dlg, ID_LIST), core::ptr::null(), 0) };
        self.show_status(dlg);
    }

    /// The status line and OK: what will be renamed, or what is wrong.
    fn show_status(&self, dlg: HWND) {
        let (text, ok) = {
            let p = self.preview.borrow();
            status(&self.files, &p, self.scan.borrow().is_some())
        };
        let z = wz(&text);
        // SAFETY: NUL-terminated text for this thread's static.
        unsafe { SetWindowTextW(item(dlg, ID_STATUS), z.as_ptr()) };
        enable(dlg, OK, ok);
    }

    /// Whether the status line says something is wrong (it is red then).
    fn status_is_bad(&self) -> bool {
        let p = self.preview.borrow();
        p.error.is_some() || p.problems.iter().any(Option::is_some)
    }

    /// OK: the renames, when there are any and nothing is wrong. The names
    /// are made again first: the typing a moment ago may not be in them yet
    /// (many files wait for it to stop).
    fn ok(&self, dlg: HWND) -> bool {
        if self.regenerating.get() {
            return false;
        }
        // SAFETY: a timer of this thread's dialog; a missing one is fine.
        unsafe { KillTimer(dlg, TIMER_RECOMPUTE) };
        self.recompute(dlg);
        let (options, old_format, new_format) = (
            Dlg::options(dlg),
            Dlg::format(dlg, ID_OLD),
            Dlg::format(dlg, ID_NEW),
        );
        let p = self.preview.borrow();
        if p.error.is_some()
            || p.problems.iter().any(Option::is_some)
            || self.scan.borrow().is_none()
        {
            return false;
        }
        let mut renames: Vec<(Vec<u16>, Vec<u16>)> = self
            .files
            .iter()
            .zip(&p.new)
            .filter_map(|(f, new)| {
                let new = new.as_deref()?;
                (new != f.name).then(|| (f.path.clone(), new.encode_utf16().collect()))
            })
            .collect();
        // A folder and what it holds may both be renamed: the folder last.
        rename::children_first(&mut renames);
        if !renames.is_empty() {
            *self.out.borrow_mut() = Some(Chosen {
                options,
                old_format,
                new_format,
                renames,
            });
        }
        true
    }

    /// A format box's menu, under its ▸ button: what can be put in it.
    fn format_menu(&self, dlg: HWND, combo_id: u16) {
        let regex = Dlg::options(dlg).regex;
        let mut entries: Vec<(String, bool, String)> = Vec::new();
        let mut separators = Vec::new();
        let push = |e: &mut Vec<(String, bool, String)>, i: &Insert| {
            e.push((i.text.to_owned(), i.wrap, i.label.to_owned()));
        };
        if combo_id == ID_OLD {
            let items: &[Insert] = if regex { &OLD_REGEX_ITEMS } else { &OLD_ITEMS };
            for i in items {
                push(&mut entries, i);
            }
        } else {
            entries.extend(variables(regex).into_iter().map(|(t, l)| (t, false, l)));
            separators.push(entries.len());
            for i in &NEW_ITEMS {
                push(&mut entries, i);
            }
            separators.push(entries.len());
            for i in &CASE_ITEMS {
                push(&mut entries, i);
            }
            separators.push(entries.len());
            for i in &NEW_ESCAPES {
                push(&mut entries, i);
            }
        }
        let popup_menu = menu::empty();
        for (n, (_, _, label)) in entries.iter().enumerate() {
            if separators.contains(&n) && n > 0 {
                menu::add_items(popup_menu, Some(u16::MAX), &[menu::Item::Sep]);
            }
            // A menu shows `&` as an access key: these labels have none.
            let label = label.replace('&', "&&");
            menu::add_items(
                popup_menu,
                Some(u16::MAX),
                &[menu::Item::Cmd(n as u16 + 1, &label)],
            );
        }
        let button = item(
            dlg,
            if combo_id == ID_OLD {
                ID_OLD_MENU
            } else {
                ID_NEW_MENU
            },
        );
        let mut rc = RECT::default();
        // SAFETY: reads this thread's button.
        unsafe { GetWindowRect(button, &mut rc) };
        let pt = POINT {
            x: rc.left,
            y: rc.bottom,
        };
        let id = popup::track(popup_menu, dlg, pt, "rename format menu", &|_| None);
        // SAFETY: the menu this function made.
        unsafe { windows_sys::Win32::UI::WindowsAndMessaging::DestroyMenu(popup_menu) };
        let Some((text, wrap, _)) = usize::try_from(id)
            .ok()
            .and_then(|i| i.checked_sub(1))
            .and_then(|i| entries.get(i))
        else {
            return;
        };
        self.insert(dlg, combo_id, text, *wrap);
    }

    /// Puts `text` in a format box at its caret (or around its selected
    /// text, as `<text:...>`, for a case function).
    fn insert(&self, dlg: HWND, combo_id: u16, text: &str, wrap: bool) {
        let edit = combo_edit(item(dlg, combo_id));
        let put = if wrap {
            let (mut start, mut end) = (0u32, 0u32);
            send(
                edit,
                EM_GETSEL,
                (&raw mut start) as usize,
                (&raw mut end) as isize,
            );
            let all: Vec<u16> = window_text(edit).encode_utf16().collect();
            let (a, b) = (start.min(end) as usize, start.max(end) as usize);
            let selected = String::from_utf16_lossy(all.get(a..b.min(all.len())).unwrap_or(&[]));
            format!("<{text}:{selected}>")
        } else {
            text.to_owned()
        };
        let z = wz(&put);
        // SAFETY: focuses this thread's box; the text outlives the call.
        unsafe { SetFocus(edit) };
        send(edit, EM_REPLACESEL, 1, z.as_ptr() as isize);
        self.recompute(dlg);
    }

    /// The text of a cell, for the list.
    fn disp_info(&self, info: &mut NMLVDISPINFOW) {
        if info.item.mask & LVIF_TEXT == 0 {
            return;
        }
        let text: String = {
            let p = self.preview.borrow();
            let row = usize::try_from(info.item.iItem).ok();
            match (row.and_then(|r| self.files.get(r)), info.item.iSubItem) {
                (Some(f), 0) => f.name.clone(),
                (Some(f), 2) => f.folder.clone(),
                (Some(_), 1) => {
                    let r = row.unwrap_or(0);
                    new_cell(&p, r)
                }
                _ => String::new(),
            }
        };
        let text: Vec<u16> = text.encode_utf16().collect();
        // SAFETY: the list gave a buffer of `cchTextMax` code units.
        unsafe { copy_text(&text, info.item.pszText, info.item.cchTextMax) };
    }

    /// The list's drawing: a new name the old format does not match dim, one
    /// with a problem red; the rest as dialog lists draw.
    fn custom_draw(&self, list: HWND, cd: &mut NMLVCUSTOMDRAW) -> isize {
        let dark = self.look.get().dark;
        let stage = cd.nmcd.dwDrawStage;
        if stage == CDDS_ITEMPREPAINT {
            let row = cd.nmcd.dwItemSpec;
            let selected = send(list, LVM_GETITEMSTATE, row, LVIS_SELECTED as isize) as u32
                & LVIS_SELECTED
                != 0;
            if dark && selected {
                return list_custom_draw(list, cd, dark);
            }
            return CDRF_NOTIFYSUBITEMDRAW as isize;
        }
        if stage == CDDS_ITEMPREPAINT | CDDS_SUBITEM {
            let pal = Palette::new(dark);
            let row = cd.nmcd.dwItemSpec;
            let colour = if cd.iSubItem == 1 {
                let p = self.preview.borrow();
                match (p.new.get(row), p.problems.get(row)) {
                    (_, Some(Some(_))) => error_colour(dark),
                    (Some(None), _) => pal.dim,
                    _ => pal.fg,
                }
            } else {
                pal.fg
            };
            cd.clrText = colour;
            if dark {
                cd.clrTextBk = pal.bg;
            }
            return CDRF_NEWFONT as isize;
        }
        list_custom_draw(list, cd, dark)
    }
}

/// The New Name cell of row `r`: the new name, Everything's words when the
/// old format does not match it, and what is wrong with it.
fn new_cell(p: &Preview, r: usize) -> String {
    if p.error.is_some() {
        return String::new();
    }
    match (p.new.get(r), p.problems.get(r)) {
        (Some(Some(name)), Some(Some(problem))) => format!("{name}   <{}>", problem.text()),
        (Some(Some(name)), _) => name.clone(),
        _ => "<Old filename does not match old format>".to_owned(),
    }
}

/// The status line for `files` renamed as `p` says, and whether OK may
/// rename (`scanned`: the folders' names are here).
fn status(files: &[File], p: &Preview, scanned: bool) -> (String, bool) {
    if let Some(error) = &p.error {
        return (error.clone(), false);
    }
    let n = files.len();
    let renamed = files
        .iter()
        .zip(&p.new)
        .filter(|(f, new)| new.as_deref().is_some_and(|n| n != f.name))
        .count();
    let bad: Vec<Problem> = p.problems.iter().flatten().copied().collect();
    let count = |k: usize, one: &str, many: &str| {
        format!(
            "{} {}",
            crate::format::grouped(k as u64),
            if k == 1 { one } else { many }
        )
    };
    let mut text = format!(
        "{}: {} renamed, {} unchanged",
        count(n, "file", "files"),
        crate::format::grouped(renamed as u64),
        crate::format::grouped((n - renamed) as u64)
    );
    if let Some(first) = bad.first() {
        text.push_str(&format!(
            "; {} ({})",
            count(bad.len(), "problem", "problems"),
            first.text().replace('\n', " ")
        ));
    } else if !scanned {
        text.push_str("; checking the folders...");
    }
    (text, bad.is_empty() && scanned && renamed > 0)
}

/// A format box: Ctrl+Space shows its menu (Everything's key).
unsafe extern "system" fn format_box_proc(
    hwnd: HWND,
    msg: u32,
    w: WPARAM,
    l: LPARAM,
    _id: usize,
    dlg: usize,
) -> LRESULT {
    // SAFETY: reads the keyboard; posts to this thread's dialog; the
    // subclass goes with the box.
    unsafe {
        let ctrl = || GetKeyState(i32::from(VK_CONTROL)) < 0;
        match msg {
            WM_KEYDOWN if w as u16 == VK_SPACE && ctrl() => {
                // The box's combo box: which format.
                let combo = windows_sys::Win32::UI::WindowsAndMessaging::GetParent(hwnd);
                let id = windows_sys::Win32::UI::WindowsAndMessaging::GetDlgCtrlID(combo);
                PostMessageW(dlg as HWND, WM_FORMAT_MENU, id as usize, 0);
                return 0;
            }
            WM_CHAR if w == 0x20 && ctrl() => return 0,
            WM_NCDESTROY => {
                RemoveWindowSubclass(hwnd, Some(format_box_proc), FORMAT_BOX);
            }
            _ => {}
        }
        DefSubclassProc(hwnd, msg, w, l)
    }
}

/// The status line in red while something is wrong.
fn status_colour(s: &Dlg, hdc: HDC, msg: u32) -> isize {
    let dark = s.look.get().dark;
    let brush = dialog::ctl_color(msg, hdc, dark)
        // SAFETY: a system brush, not to be freed.
        .unwrap_or_else(|| unsafe { GetSysColorBrush(COLOR_BTNFACE) } as isize);
    // SAFETY: the DC the static is about to draw with.
    unsafe {
        SetTextColor(hdc, error_colour(dark));
        SetBkMode(hdc, TRANSPARENT as i32);
    }
    brush
}

unsafe extern "system" fn dialog_proc(dlg: HWND, msg: u32, w: WPARAM, l: LPARAM) -> isize {
    if msg == WM_INITDIALOG {
        // SAFETY: `l` is the `Dlg` that `run` keeps alive until the dialog
        // has ended.
        unsafe { SetWindowLongPtrW(dlg, GWLP_USERDATA, l) };
    }
    // SAFETY: GWLP_USERDATA is the `Dlg` from `run`, or null before.
    let Some(s) = (unsafe { state::<Dlg>(dlg) }) else {
        return 0;
    };
    if msg == WM_CTLCOLORSTATIC && l as HWND == item(dlg, ID_STATUS) && s.status_is_bad() {
        return status_colour(s, w as HDC, msg);
    }
    if let Some(answer) = common(dlg, msg, w, l, &s.look) {
        return answer;
    }
    match msg {
        WM_INITDIALOG => {
            s.init(dlg);
            s.focus_new(dlg);
            // The keyboard was put in New format.
            0
        }
        WM_SCANNED => {
            // SAFETY: the box the scanning thread posted, taken once; then
            // a timer of this thread's dialog.
            let scan = unsafe {
                KillTimer(dlg, TIMER_SCAN);
                Box::from_raw(l as *mut Scan)
            };
            *s.scan.borrow_mut() = Some(*scan);
            // The first names, still to be made, take the scan into
            // account then.
            if !s.regenerating.get() {
                s.recompute(dlg);
            }
            1
        }
        WM_RECOMPUTE => {
            s.recompute(dlg);
            1
        }
        WM_REGENERATE => {
            if s.regenerating.replace(false) {
                s.regenerate(dlg);
                s.focus_new(dlg);
            }
            1
        }
        WM_FORMAT_MENU => {
            s.format_menu(dlg, w as u16);
            1
        }
        WM_TIMER => {
            // SAFETY: a timer of this thread's dialog.
            unsafe { KillTimer(dlg, w) };
            if w == TIMER_RECOMPUTE {
                s.recompute(dlg);
            } else if w == TIMER_SCAN && s.scan.borrow().is_none() {
                *s.scan.borrow_mut() = Some(Scan::new());
                s.recompute(dlg);
            }
            1
        }
        WM_SIZE => {
            dialog::anchor(dlg, SIZE, &PLACES);
            fill_last_column(item(dlg, ID_LIST));
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
        WM_NOTIFY => {
            // SAFETY: WM_NOTIFY's lParam is an NMHDR.
            let hdr = unsafe { &*(l as *const NMHDR) };
            if hdr.idFrom != usize::from(ID_LIST) {
                return 0;
            }
            let answer = match hdr.code {
                // SAFETY: the list's custom draw structure, as it sent it.
                NM_CUSTOMDRAW => {
                    Some(s.custom_draw(hdr.hwndFrom, unsafe { &mut *(l as *mut NMLVCUSTOMDRAW) }))
                }
                LVN_GETDISPINFOW => {
                    // SAFETY: the list's display info, as it sent it.
                    s.disp_info(unsafe { &mut *(l as *mut NMLVDISPINFOW) });
                    None
                }
                // No type-ahead in this list.
                LVN_ODFINDITEMW => Some(-1),
                _ => None,
            };
            if let Some(answer) = answer {
                // SAFETY: the answer of this dialog's WM_NOTIFY.
                unsafe { SetWindowLongPtrW(dlg, DWLP_MSGRESULT as i32, answer) };
            }
            1
        }
        WM_COMMAND => {
            let (id, code) = ((w & 0xffff) as u16, ((w >> 16) & 0xffff) as u32);
            match (id, code) {
                (OK, _) => {
                    if s.ok(dlg) {
                        // SAFETY: ends this modal dialog.
                        unsafe { EndDialog(dlg, 1) };
                    }
                }
                // SAFETY: ends this modal dialog.
                (CANCEL, _) => unsafe {
                    EndDialog(dlg, 0);
                },
                (ID_OLD | ID_NEW, CBN_EDITCHANGE) => {
                    if s.files.len() > MANY {
                        // SAFETY: a timer of this thread's dialog; set
                        // again, it starts again.
                        unsafe { SetTimer(dlg, TIMER_RECOMPUTE, RECOMPUTE_MS, None) };
                    } else {
                        s.recompute(dlg);
                    }
                }
                (ID_OLD | ID_NEW, CBN_SELCHANGE) => {
                    // The box shows the pick once this returns: its text
                    // now, then the names.
                    let combo = item(dlg, id);
                    if let Ok(i) = usize::try_from(send(combo, CB_GETCURSEL, 0, 0)) {
                        let z = wz(&combo_item(combo, i));
                        // SAFETY: NUL-terminated text for this thread's
                        // combo box.
                        unsafe { SetWindowTextW(combo, z.as_ptr()) };
                    }
                    // SAFETY: posts to this thread's dialog.
                    unsafe { PostMessageW(dlg, WM_RECOMPUTE, 0, 0) };
                }
                (ID_OLD_MENU, BN_CLICKED) => s.format_menu(dlg, ID_OLD),
                (ID_NEW_MENU, BN_CLICKED) => s.format_menu(dlg, ID_NEW),
                // Everything makes the formats again for these two.
                (ID_REGEX | ID_EXTENSION, BN_CLICKED) => s.regenerate(dlg),
                (ID_CASE | ID_DIACRITICS, BN_CLICKED) => s.recompute(dlg),
                _ => {}
            }
            1
        }
        WM_DESTROY => {
            places::keep(dlg, Which::Rename, core::ptr::null_mut());
            0
        }
        _ => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(name: &str) -> File {
        File {
            path: format!(r"C:\a\{name}").encode_utf16().collect(),
            name: name.to_owned(),
            folder: r"C:\a".to_owned(),
        }
    }

    #[test]
    fn the_status_line_says_what_will_happen() {
        let files = [file("a.txt"), file("b.txt"), file("c.txt")];
        let p = Preview {
            new: vec![Some("x.txt".into()), Some("b.txt".into()), None],
            problems: vec![None; 3],
            error: None,
        };
        assert_eq!(
            status(&files, &p, true),
            ("3 files: 1 renamed, 2 unchanged".into(), true)
        );
        // Not before the folders' names are read.
        let (text, ok) = status(&files, &p, false);
        assert!(text.ends_with("checking the folders...") && !ok);
        let p = Preview {
            new: vec![Some("x.txt".into()), Some("x.txt".into()), None],
            problems: vec![Some(Problem::Duplicate), Some(Problem::Duplicate), None],
            error: None,
        };
        assert_eq!(
            status(&files, &p, true),
            (
                "3 files: 2 renamed, 1 unchanged; 2 problems (Duplicated new filename)".into(),
                false
            )
        );
        assert_eq!(new_cell(&p, 0), "x.txt   <Duplicated new filename>");
        assert_eq!(new_cell(&p, 2), "<Old filename does not match old format>");
        // Nothing to rename: OK waits.
        let p = Preview {
            new: vec![None; 3],
            problems: vec![None; 3],
            error: None,
        };
        assert!(!status(&files, &p, true).1);
        let p = Preview {
            error: Some("New format: a function has no closing )".into()),
            ..Preview::default()
        };
        assert_eq!(
            status(&files, &p, true),
            ("New format: a function has no closing )".into(), false)
        );
    }

    #[test]
    fn menus_offer_what_the_formats_take() {
        assert_eq!(variables(false)[0].0, "%1");
        assert_eq!(variables(true)[0].0, "\\0");
        assert_eq!(variables(true).len(), 10);
        // Every text the menus put in a format means what its label says.
        let o = Options::default();
        let names = [Subject::new("ab", false)];
        for i in OLD_ITEMS.iter().filter(|i| i.text.starts_with('%')) {
            assert!(
                rename::rename_all(&names, i.text, "x", o).is_ok(),
                "{}",
                i.text
            );
        }
        for i in NEW_ITEMS.iter().chain(&NEW_ESCAPES) {
            assert!(
                rename::rename_all(&names, "%1", i.text, o).is_ok(),
                "{}",
                i.text
            );
        }
        assert_eq!(
            rename::rename_all(&names, "%1", "<uppercase:%1>", o).unwrap(),
            vec![Some("AB".to_owned())]
        );
    }
}
