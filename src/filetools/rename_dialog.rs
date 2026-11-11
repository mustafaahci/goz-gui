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
//!
//! Wave 4: the same dialog is Edit > Advanced > Advanced Copy to Folder...
//! and Advanced Move to Folder... ([`Mode`]): a To folder box (and
//! Browse...) above the formats, and every file goes to that folder under
//! its new name (a name the old format does not match keeps its own); the
//! names already in that folder are read on a thread of their own, again
//! a moment after the folder changes. The Presets button keeps named Old
//! and New formats (shared by the three), to put in the boxes again later.

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};

use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows_sys::Win32::Graphics::Gdi::{
    COLOR_BTNFACE, GetSysColorBrush, HDC, InvalidateRect, SetBkMode, SetTextColor, TRANSPARENT,
};
use windows_sys::Win32::UI::Controls::{
    CDDS_ITEMPREPAINT, CDDS_SUBITEM, CDRF_NEWFONT, CDRF_NOTIFYSUBITEMDRAW, EM_GETSEL,
    EM_REPLACESEL, EM_SETSEL, LVIF_TEXT, LVIS_SELECTED, LVM_GETITEMSTATE,
    LVM_SETEXTENDEDLISTVIEWSTYLE, LVM_SETITEMCOUNT, LVN_GETDISPINFOW, LVN_ODFINDITEMW,
    LVS_EX_DOUBLEBUFFER, LVS_EX_FULLROWSELECT, LVS_EX_LABELTIP, LVS_OWNERDATA, LVS_REPORT,
    LVS_SHOWSELALWAYS, NM_CUSTOMDRAW, NMHDR, NMLVCUSTOMDRAW, NMLVDISPINFOW,
};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    GetKeyState, SetFocus, VK_CONTROL, VK_SPACE,
};
use windows_sys::Win32::UI::Shell::{DefSubclassProc, RemoveWindowSubclass, SetWindowSubclass};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    BN_CLICKED, BS_AUTOCHECKBOX, BS_DEFPUSHBUTTON, CB_GETCURSEL, CB_GETLBTEXT, CB_GETLBTEXTLEN,
    CBN_EDITCHANGE, CBN_SELCHANGE, CBS_AUTOHSCROLL, CBS_DROPDOWN, DWLP_MSGRESULT, EN_CHANGE,
    ES_AUTOHSCROLL, EndDialog, FindWindowExW, GWLP_USERDATA, GetWindowRect, IDCANCEL, IDOK, IDYES,
    KillTimer, MB_ICONQUESTION, MB_YESNO, MINMAXINFO, PostMessageW, SetTimer, SetWindowLongPtrW,
    SetWindowTextW, WM_APP, WM_CHAR, WM_COMMAND, WM_CTLCOLORSTATIC, WM_DESTROY, WM_GETMINMAXINFO,
    WM_INITDIALOG, WM_KEYDOWN, WM_NCDESTROY, WM_NOTIFY, WM_SIZE, WM_TIMER, WS_EX_CLIENTEDGE,
    WS_MAXIMIZEBOX, WS_TABSTOP, WS_THICKFRAME, WS_VSCROLL,
};

use super::name_of;
use super::rename::{self, FormatError, Options, Problem, Subject};
use super::settings::{PRESETS, Preset};
use super::ui::{self, checked, copy_text, enable, error_colour, item, send, set_check};
use crate::dialog::{
    self, Class, GROW_X, GROW_Y, Look, MOVE_X, MOVE_Y, Template, Units, center_on_owner, common,
    fill_last_column, list_custom_draw, state,
};
use crate::fileops::parent;
use crate::places::{self, Which};
use crate::theme::Palette;
use crate::wide::{window_text, wz};
use crate::{menu, msgbox, popup};

/// What the dialog does with the new names.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    /// Renames each file where it is (Advanced Rename).
    Rename,
    /// Copies or moves each file into one folder under its new name
    /// (Advanced Copy to Folder / Advanced Move to Folder).
    Copy,
    Move,
}

impl Mode {
    /// The caption: Everything's for the renamer.
    fn title(self) -> &'static str {
        match self {
            Mode::Rename => "Rename",
            Mode::Copy => "Advanced Copy to Folder",
            Mode::Move => "Advanced Move to Folder",
        }
    }

    /// Where its place is kept.
    fn place(self) -> Which {
        match self {
            Mode::Rename => Which::Rename,
            Mode::Copy => Which::AdvancedCopyTo,
            Mode::Move => Which::AdvancedMoveTo,
        }
    }

    /// How far the controls under the To folder row move down.
    fn dy(self) -> i16 {
        if self == Mode::Rename { 0 } else { 18 }
    }
}

/// What the window hands the dialog: the switches and format histories
/// kept from last time.
pub struct Start {
    pub options: Options,
    pub old_history: Vec<String>,
    pub new_history: Vec<String>,
    pub mode: Mode,
    /// Copy and Move: the folder the To folder box starts with.
    pub folder: String,
    /// The presets kept.
    pub presets: Vec<Preset>,
}

/// What OK chose.
#[derive(Debug)]
pub struct Chosen {
    pub options: Options,
    pub old_format: String,
    pub new_format: String,
    /// Each file to rename and its new name; for Copy and Move, each file
    /// to copy or move and its name in the folder.
    pub renames: Vec<(Vec<u16>, Vec<u16>)>,
    /// Copy and Move: the folder.
    pub folder: Vec<u16>,
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
const ID_PRESETS: u16 = 1084;
const ID_STATUS: u16 = 1100;
// The To folder box and its Browse... (goz-gui's ids).
const ID_FOLDER: u16 = 1090;
const ID_BROWSE: u16 = 1091;
const STATIC: u16 = 0xffff;
const OK: u16 = IDOK as u16;
const CANCEL: u16 = IDCANCEL as u16;

/// The folders' names came: lParam is a `Box<Scanned>`.
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
/// The To folder's names are read again this long after it last changed.
const TIMER_FOLDER: usize = 3;
const FOLDER_MS: u32 = 300;
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
/// The controls that follow the dialog's size, with the rows under the To
/// folder row `dy` lower (Copy and Move), in a dialog `dy` taller.
fn places(dy: i16) -> [(u16, Units, u8); 11] {
    let h = SIZE.1 + dy;
    [
        (ID_FOLDER, rect(52, 7, SIZE.0 - 52 - 57, 13), GROW_X),
        (ID_BROWSE, rect(SIZE.0 - 55, 6, 48, 14), MOVE_X),
        (ID_OLD, rect(52, 7 + dy, SIZE.0 - 52 - 27, 150), GROW_X),
        (ID_OLD_MENU, rect(SIZE.0 - 25, 6 + dy, 18, 14), MOVE_X),
        (ID_NEW, rect(52, 25 + dy, SIZE.0 - 52 - 27, 150), GROW_X),
        (ID_NEW_MENU, rect(SIZE.0 - 25, 24 + dy, 18, 14), MOVE_X),
        (
            ID_LIST,
            rect(7, 82 + dy, SIZE.0 - 14, SIZE.1 - 82 - 29),
            GROW_X | GROW_Y,
        ),
        (ID_PRESETS, rect(7, h - 21, 50, 14), MOVE_Y),
        // Two lines: a problem's words can take more than one.
        (
            ID_STATUS,
            rect(61, h - 23, SIZE.0 - 61 - 116, 18),
            MOVE_Y | GROW_X,
        ),
        (OK, rect(SIZE.0 - 111, h - 21, 50, 14), MOVE_X | MOVE_Y),
        (CANCEL, rect(SIZE.0 - 57, h - 21, 50, 14), MOVE_X | MOVE_Y),
    ]
}

/// A file to rename.
struct File {
    path: Vec<u16>,
    name: String,
    folder: String,
}

/// The names in each folder of the files, lower case, by folder (lower
/// case): what a new name may run into.
type Scan = HashMap<String, HashSet<String>>;

/// What a scanning thread found: for Copy and Move, of which To folder
/// (as [`folder_key`] makes it) and whether it is one.
struct Scanned {
    folder: Option<String>,
    exists: bool,
    scan: Scan,
}

/// A folder as typed, as the scans key it: without a trailing `\` (but a
/// drive's root keeps it), lower case.
fn folder_key(folder: &str) -> String {
    trim_folder(folder).to_lowercase()
}

/// A folder as typed, without spaces around it or a trailing `\` (but a
/// drive's root keeps its own).
fn trim_folder(folder: &str) -> &str {
    let t = folder.trim();
    if t.len() > 3 {
        t.trim_end_matches('\\')
    } else {
        t
    }
}

/// The new names as the formats make them, and what is wrong.
#[derive(Default)]
struct Preview {
    new: Vec<Option<String>>,
    problems: Vec<Option<Problem>>,
    /// What is wrong with a format.
    error: Option<String>,
    /// Copy and Move: the files to leave out (moved into their own folder
    /// under their own name: nothing to do).
    skip: Vec<bool>,
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
    mode: Mode,
    /// Copy and Move: whether the To folder (as last scanned) is a folder;
    /// `None` while it is being read.
    dest: Cell<Option<bool>>,
    presets: RefCell<Vec<Preset>>,
    presets_changed: Cell<bool>,
}

/// Shows the renamer over `owner` for `files` (each path, and whether it
/// is a folder); what OK chose, `None` for Cancel or nothing to rename,
/// and the presets when they were changed (kept whatever the answer).
pub fn run(
    owner: HWND,
    files: &[(Vec<u16>, bool)],
    start: Start,
    look: Look,
) -> (Option<Chosen>, Option<Vec<Preset>>) {
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
    let mode = start.mode;
    let presets = RefCell::new(start.presets.clone());
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
        mode,
        dest: Cell::new(None),
        presets,
        presets_changed: Cell::new(false),
    };
    let dy = mode.dy();
    let places = places(dy);
    let mut t = Template::new(
        mode.title(),
        WS_THICKFRAME | WS_MAXIMIZEBOX,
        SIZE.0,
        SIZE.1 + dy,
    );
    let combo = WS_TABSTOP | WS_VSCROLL | (CBS_DROPDOWN | CBS_AUTOHSCROLL) as u32;
    if mode != Mode::Rename {
        t.control(Class::Static, "&To folder:", STATIC, 0, 0, (7, 9, 44, 8))
            .control(
                Class::Edit,
                "",
                ID_FOLDER,
                WS_TABSTOP | ES_AUTOHSCROLL as u32,
                WS_EX_CLIENTEDGE,
                places[0].1,
            )
            .control(
                Class::Button,
                "&Browse...",
                ID_BROWSE,
                WS_TABSTOP,
                0,
                places[1].1,
            );
    }
    t.control(
        Class::Static,
        "Old &format:",
        STATIC,
        0,
        0,
        (7, 9 + dy, 44, 8),
    )
    .control(Class::Named("ComboBox"), "", ID_OLD, combo, 0, places[2].1)
    .control(
        Class::Button,
        "\u{25B8}",
        ID_OLD_MENU,
        WS_TABSTOP,
        0,
        places[3].1,
    )
    .control(
        Class::Static,
        "New for&mat:",
        STATIC,
        0,
        0,
        (7, 27 + dy, 44, 8),
    )
    .control(Class::Named("ComboBox"), "", ID_NEW, combo, 0, places[4].1)
    .control(
        Class::Button,
        "\u{25B8}",
        ID_NEW_MENU,
        WS_TABSTOP,
        0,
        places[5].1,
    );
    let check = WS_TABSTOP | BS_AUTOCHECKBOX as u32;
    t.control(
        Class::Button,
        "Match &case",
        ID_CASE,
        check,
        0,
        (52, 43 + dy, 110, 10),
    )
    .control(
        Class::Button,
        "Match &diacritics",
        ID_DIACRITICS,
        check,
        0,
        (166, 43 + dy, 110, 10),
    )
    .control(
        Class::Button,
        "&Regular expressions",
        ID_REGEX,
        check,
        0,
        (52, 55 + dy, 110, 10),
    )
    .control(
        Class::Button,
        "Ignore &extension",
        ID_EXTENSION,
        check,
        0,
        (166, 55 + dy, 110, 10),
    )
    .control(
        Class::Static,
        "&New filenames:",
        STATIC,
        0,
        0,
        (7, 72 + dy, 100, 8),
    )
    .control(
        Class::Named("SysListView32"),
        "",
        ID_LIST,
        WS_TABSTOP | LVS_REPORT | LVS_OWNERDATA | LVS_SHOWSELALWAYS,
        WS_EX_CLIENTEDGE,
        places[6].1,
    )
    .control(
        Class::Button,
        "&Presets",
        ID_PRESETS,
        WS_TABSTOP,
        0,
        places[7].1,
    )
    .control(Class::Static, "", ID_STATUS, 0, 0, places[8].1)
    .control(
        Class::Button,
        "OK",
        OK,
        WS_TABSTOP | BS_DEFPUSHBUTTON as u32,
        0,
        places[9].1,
    )
    .control(Class::Button, "Cancel", CANCEL, WS_TABSTOP, 0, places[10].1);
    t.run(owner, Some(dialog_proc), (&raw const state) as LPARAM);
    let presets = state
        .presets_changed
        .get()
        .then(|| state.presets.into_inner());
    (state.out.into_inner(), presets)
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
            // A themed list takes half a millisecond an item (20 formats
            // kept for each box): put in once the dialog shows.
            dialog::fill_later(combo, history.clone(), None);
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
        send(list, LVM_SETITEMCOUNT, self.files.len(), 0);
        // The last column (Path) takes the room the others leave, sized
        // once the rows are in (they may bring the vertical scroll bar).
        fill_last_column(list);
        dialog::theme_new(dlg, self.look.get().dark);
        center_on_owner(dlg);
        let mut wr = RECT::default();
        // SAFETY: reads this thread's dialog.
        unsafe { GetWindowRect(dlg, &mut wr) };
        self.min.set((wr.right - wr.left, wr.bottom - wr.top));
        places::restore(
            dlg,
            self.mode.place(),
            core::ptr::null_mut(),
            self.min.get(),
        );
        let dy = self.mode.dy();
        dialog::anchor(dlg, (SIZE.0, SIZE.1 + dy), &places(dy));
        if self.mode == Mode::Rename {
            // The folders' names, read (and sorted) off this thread.
            let folders: Vec<String> = self.files.iter().map(|f| f.folder.clone()).collect();
            self.start_scan(dlg, folders, None);
        } else {
            let z = wz(&self.start.folder);
            // SAFETY: NUL-terminated text for this thread's box; its
            // EN_CHANGE comes before the dialog is shown, and the names
            // are read below.
            unsafe { SetWindowTextW(item(dlg, ID_FOLDER), z.as_ptr()) };
            // SAFETY: a timer of this thread's dialog (set by EN_CHANGE).
            unsafe { KillTimer(dlg, TIMER_FOLDER) };
            self.scan_folder(dlg);
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

    /// Reads the names in `folders` on a thread of its own (`folder`: the
    /// To folder they are of); OK waits for them, at most `SCAN_MS`.
    fn start_scan(&self, dlg: HWND, folders: Vec<String>, folder: Option<String>) {
        let to = dlg as usize;
        let spawned = std::thread::Builder::new()
            .name("goz-rename-scan".into())
            .spawn(move || {
                let exists = folders
                    .first()
                    .is_some_and(|f| std::path::Path::new(f).is_dir());
                let scanned = Scanned {
                    folder,
                    exists,
                    scan: scan(folders),
                };
                let raw = Box::into_raw(Box::new(scanned));
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
            self.dest.set(Some(true));
        } else {
            // SAFETY: a timer of this thread's dialog.
            unsafe { SetTimer(dlg, TIMER_SCAN, SCAN_MS, None) };
        }
    }

    /// The To folder as typed.
    fn folder(dlg: HWND) -> String {
        window_text(item(dlg, ID_FOLDER)).trim().to_owned()
    }

    /// Reads the To folder's names again (it changed): OK waits meanwhile.
    fn scan_folder(&self, dlg: HWND) {
        let folder = Dlg::folder(dlg);
        *self.scan.borrow_mut() = None;
        if folder.is_empty() {
            self.dest.set(Some(false));
            self.recompute(dlg);
            return;
        }
        self.dest.set(None);
        let trimmed = trim_folder(&folder).to_owned();
        self.start_scan(dlg, vec![trimmed], Some(folder_key(&folder)));
        self.recompute(dlg);
    }

    /// The scanning thread's names came.
    fn scanned(&self, dlg: HWND, s: Scanned) {
        if let Some(folder) = &s.folder
            && *folder != folder_key(&Dlg::folder(dlg))
        {
            // The folder changed since: its own scan comes.
            return;
        }
        // SAFETY: a timer of this thread's dialog.
        unsafe { KillTimer(dlg, TIMER_SCAN) };
        self.dest.set(Some(s.folder.is_none() || s.exists));
        *self.scan.borrow_mut() = Some(s.scan);
        // The first names, still to be made, take the scan into account
        // then.
        if !self.regenerating.get() {
            self.recompute(dlg);
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
            Ok(names) if self.mode != Mode::Rename => {
                let dest = folder_key(&Dlg::folder(dlg));
                let olds: Vec<&str> = self.files.iter().map(|f| f.name.as_str()).collect();
                let in_dest: Vec<bool> = self
                    .files
                    .iter()
                    .map(|f| folder_key(&f.folder) == dest)
                    .collect();
                let moving = self.mode == Mode::Move;
                let scan = self.scan.borrow();
                let there = scan.as_ref().and_then(|s| s.get(&dest));
                let problems = rename::transfer_problems(&olds, &names, &in_dest, moving, |n| {
                    there.is_some_and(|names| names.contains(n))
                });
                let skip = (0..n)
                    .map(|i| {
                        let name = names[i].as_deref().unwrap_or(olds[i]);
                        moving && in_dest[i] && name.to_lowercase() == olds[i].to_lowercase()
                    })
                    .collect();
                Preview {
                    new: names,
                    problems,
                    error: None,
                    skip,
                }
            }
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
                    skip: Vec::new(),
                }
            }
            Err(e) => Preview {
                new: vec![None; n],
                problems: vec![None; n],
                error: Some(match e {
                    FormatError::Old(_) | FormatError::New(_) => e.to_string(),
                }),
                skip: Vec::new(),
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
            match self.mode {
                Mode::Rename => status(&self.files, &p, self.scan.borrow().is_some()),
                mode => transfer_status(
                    &self.files,
                    &p,
                    mode == Mode::Move,
                    Dlg::folder(dlg).is_empty(),
                    self.dest.get(),
                ),
            }
        };
        let z = wz(&text);
        // SAFETY: NUL-terminated text for this thread's static.
        unsafe { SetWindowTextW(item(dlg, ID_STATUS), z.as_ptr()) };
        enable(dlg, OK, ok);
    }

    /// Whether the status line says something is wrong (it is red then).
    fn status_is_bad(&self) -> bool {
        let p = self.preview.borrow();
        p.error.is_some()
            || p.problems.iter().any(Option::is_some)
            || (self.mode != Mode::Rename && self.dest.get() == Some(false))
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
        if self.mode != Mode::Rename {
            if self.dest.get() != Some(true) {
                return false;
            }
            let moves: Vec<(Vec<u16>, Vec<u16>)> = self
                .files
                .iter()
                .zip(&p.new)
                .zip(&p.skip)
                .filter(|(_, skip)| !**skip)
                .map(|((f, new), _)| {
                    let name = new.as_deref().unwrap_or(&f.name);
                    (f.path.clone(), name.encode_utf16().collect())
                })
                .collect();
            if !moves.is_empty() {
                let folder = Dlg::folder(dlg);
                *self.out.borrow_mut() = Some(Chosen {
                    options,
                    old_format,
                    new_format,
                    renames: moves,
                    folder: trim_folder(&folder).encode_utf16().collect(),
                });
            }
            return true;
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
                folder: Vec::new(),
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

    /// Browse... beside the To folder: a folder picker, starting there.
    fn browse(&self, dlg: HWND) {
        let now: Vec<u16> = Dlg::folder(dlg).encode_utf16().collect();
        let title = if self.mode == Mode::Move {
            "Move To"
        } else {
            "Copy To"
        };
        let Some(folder) = super::folder_pick::pick(dlg, title, "Select Folder", Some(&now)) else {
            return;
        };
        let z = crate::wide::nul(&folder);
        // SAFETY: NUL-terminated text for this thread's box (its EN_CHANGE
        // reads the folder's names again).
        unsafe { SetWindowTextW(item(dlg, ID_FOLDER), z.as_ptr()) };
    }

    /// The Presets button's menu: each preset (puts its formats in the
    /// boxes), Save Preset... (the formats as they are, under a name) and
    /// Delete Preset > (asks first).
    fn presets_menu(&self, dlg: HWND) {
        const SAVE: u32 = 500;
        const DELETE: u32 = 1000;
        let list = self.presets.borrow().clone();
        let names: Vec<String> = list.iter().map(|p| menu::label(&p.name)).collect();
        let m = menu::empty();
        if list.is_empty() {
            menu::add_items(m, Some(u16::MAX), &[menu::Item::Greyed("(No presets)")]);
        }
        let delete = menu::empty();
        for (i, name) in names.iter().enumerate() {
            menu::add_items(m, Some(u16::MAX), &[menu::Item::Cmd(1 + i as u16, name)]);
            menu::add_items(
                delete,
                Some(u16::MAX),
                &[menu::Item::Cmd((DELETE as usize + i) as u16, name)],
            );
        }
        menu::add_items(
            m,
            Some(u16::MAX),
            &[
                menu::Item::Sep,
                menu::Item::Cmd(SAVE as u16, "&Save Preset..."),
            ],
        );
        if list.is_empty() {
            menu::add_items(m, Some(u16::MAX), &[menu::Item::Greyed("&Delete Preset")]);
            // SAFETY: a menu this function made and put nowhere.
            unsafe { windows_sys::Win32::UI::WindowsAndMessaging::DestroyMenu(delete) };
        } else {
            menu::add_items(
                m,
                Some(u16::MAX),
                &[menu::Item::Sub("&Delete Preset", delete)],
            );
        }
        let mut rc = RECT::default();
        // SAFETY: reads this thread's button.
        unsafe { GetWindowRect(item(dlg, ID_PRESETS), &mut rc) };
        let pt = POINT {
            x: rc.left,
            y: rc.bottom,
        };
        let id = popup::track(m, dlg, pt, "rename presets menu", &|_| None);
        // SAFETY: the menu this function made (its submenu goes with it).
        unsafe { windows_sys::Win32::UI::WindowsAndMessaging::DestroyMenu(m) };
        match id {
            0 => {}
            SAVE => self.save_preset(dlg),
            n if n >= DELETE => {
                let Some(p) = list.get((n - DELETE) as usize) else {
                    return;
                };
                let ask = format!("Delete the preset \"{}\"?", p.name);
                if msgbox::show(dlg, &ask, "Presets", MB_YESNO | MB_ICONQUESTION) == IDYES {
                    self.presets.borrow_mut().retain(|q| q.name != p.name);
                    self.presets_changed.set(true);
                }
            }
            n => {
                let Some(p) = list.get((n - 1) as usize) else {
                    return;
                };
                for (id, text) in [(ID_OLD, &p.old_format), (ID_NEW, &p.new_format)] {
                    let z = wz(text);
                    // SAFETY: NUL-terminated text for this thread's box.
                    unsafe { SetWindowTextW(item(dlg, id), z.as_ptr()) };
                }
                // And its switches, as Everything's presets keep them.
                let o = p.options;
                set_check(dlg, ID_CASE, o.match_case);
                set_check(dlg, ID_DIACRITICS, o.match_diacritics);
                set_check(dlg, ID_REGEX, o.regex);
                set_check(dlg, ID_EXTENSION, o.ignore_extension);
                self.recompute(dlg);
            }
        }
    }

    /// Save Preset...: the formats in the boxes, under a name asked for
    /// (one of that name already is replaced, after asking).
    fn save_preset(&self, dlg: HWND) {
        let Some(name) = ask_name(dlg, self.look.get()) else {
            return;
        };
        let taken = self
            .presets
            .borrow()
            .iter()
            .any(|p| p.name.eq_ignore_ascii_case(&name));
        if taken {
            let ask = format!("A preset named \"{name}\" already exists. Replace it?");
            if msgbox::show(dlg, &ask, "Presets", MB_YESNO | MB_ICONQUESTION) != IDYES {
                return;
            }
        }
        let preset = Preset {
            name,
            old_format: Dlg::format(dlg, ID_OLD),
            new_format: Dlg::format(dlg, ID_NEW),
            options: Dlg::options(dlg),
        };
        let mut list = self.presets.borrow_mut();
        let at = list
            .iter()
            .position(|p| p.name.eq_ignore_ascii_case(&preset.name));
        match at {
            Some(i) => list[i] = preset,
            None if list.len() < PRESETS => list.push(preset),
            None => return,
        }
        drop(list);
        self.presets_changed.set(true);
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
                (Some(f), 1) => {
                    let r = row.unwrap_or(0);
                    if self.mode == Mode::Rename {
                        new_cell(&p, r)
                    } else {
                        transfer_cell(&p, r, &f.name)
                    }
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

/// The New Name cell of row `r` for Copy and Move: the name it gets there,
/// its own (`old`) when the old format does not match it, and what is
/// wrong with it.
fn transfer_cell(p: &Preview, r: usize, old: &str) -> String {
    if p.error.is_some() {
        return String::new();
    }
    let name = p.new.get(r).cloned().flatten();
    let name = name.as_deref().unwrap_or(old);
    match p.problems.get(r) {
        Some(Some(problem)) => format!("{name}   <{}>", problem.text()),
        _ => name.to_owned(),
    }
}

/// The status line for Copy and Move (`moving`): how many files go to the
/// folder and how many of them get a new name, or what is wrong; and
/// whether OK may run. `dest`: whether the To folder is one (`None`:
/// still reading it).
fn transfer_status(
    files: &[File],
    p: &Preview,
    moving: bool,
    no_folder: bool,
    dest: Option<bool>,
) -> (String, bool) {
    if let Some(error) = &p.error {
        return (error.clone(), false);
    }
    let verb = if moving { "move" } else { "copy" };
    if no_folder {
        return (
            format!("Type or browse for the folder to {verb} to."),
            false,
        );
    }
    if dest == Some(false) {
        return ("The To folder does not exist.".into(), false);
    }
    let going = files.len() - p.skip.iter().filter(|s| **s).count();
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
        "{}: {} to {verb}, {} renamed",
        count(files.len(), "file", "files"),
        crate::format::grouped(going as u64),
        crate::format::grouped(renamed as u64)
    );
    if let Some(first) = bad.first() {
        text.push_str(&format!(
            "; {} ({})",
            count(bad.len(), "problem", "problems"),
            first.text().replace('\n', " ")
        ));
    } else if dest.is_none() {
        text.push_str("; checking the folder...");
    }
    (text, bad.is_empty() && dest == Some(true) && going > 0)
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
            // SAFETY: the box the scanning thread posted, taken once.
            let scanned = unsafe { Box::from_raw(l as *mut Scanned) };
            s.scanned(dlg, *scanned);
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
                if s.dest.get().is_none() {
                    // A To folder that does not answer: Windows' own
                    // questions come if a name is taken there.
                    s.dest.set(Some(true));
                }
                s.recompute(dlg);
            } else if w == TIMER_FOLDER {
                s.scan_folder(dlg);
            }
            1
        }
        WM_SIZE => {
            let dy = s.mode.dy();
            dialog::anchor(dlg, (SIZE.0, SIZE.1 + dy), &places(dy));
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
                (ID_FOLDER, EN_CHANGE) => {
                    // SAFETY: a timer of this thread's dialog; set again,
                    // it starts again.
                    unsafe { SetTimer(dlg, TIMER_FOLDER, FOLDER_MS, None) };
                    s.dest.set(None);
                    s.show_status(dlg);
                }
                (ID_BROWSE, BN_CLICKED) => s.browse(dlg),
                (ID_PRESETS, BN_CLICKED) => s.presets_menu(dlg),
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
            places::keep(dlg, s.mode.place(), core::ptr::null_mut());
            0
        }
        _ => 0,
    }
}

/// Save Preset...'s question: the preset's name.
struct AskName {
    look: Cell<Look>,
    name: RefCell<Option<String>>,
}

const ID_NAME: u16 = 1000;

/// Asks for a preset's name over `owner`; `None` for Cancel.
fn ask_name(owner: HWND, look: Look) -> Option<String> {
    let state = AskName {
        look: Cell::new(look),
        name: RefCell::new(None),
    };
    let mut t = Template::new("Save Preset", 0, 186, 62);
    t.control(Class::Static, "&Name:", STATIC, 0, 0, (7, 9, 30, 8))
        .control(
            Class::Edit,
            "",
            ID_NAME,
            WS_TABSTOP | ES_AUTOHSCROLL as u32,
            WS_EX_CLIENTEDGE,
            (40, 7, 139, 13),
        )
        .control(
            Class::Button,
            "OK",
            OK,
            WS_TABSTOP | BS_DEFPUSHBUTTON as u32,
            0,
            (75, 41, 50, 14),
        )
        .control(
            Class::Button,
            "Cancel",
            CANCEL,
            WS_TABSTOP,
            0,
            (129, 41, 50, 14),
        );
    t.run(owner, Some(name_proc), (&raw const state) as LPARAM);
    state.name.into_inner()
}

unsafe extern "system" fn name_proc(dlg: HWND, msg: u32, w: WPARAM, l: LPARAM) -> isize {
    if msg == WM_INITDIALOG {
        // SAFETY: `l` is the `AskName` that `ask_name` keeps alive until
        // the dialog has ended.
        unsafe { SetWindowLongPtrW(dlg, GWLP_USERDATA, l) };
    }
    // SAFETY: GWLP_USERDATA is the `AskName` from `ask_name`, or null.
    let Some(s) = (unsafe { state::<AskName>(dlg) }) else {
        return 0;
    };
    if let Some(answer) = common(dlg, msg, w, l, &s.look) {
        return answer;
    }
    match msg {
        WM_INITDIALOG => {
            dialog::apply_theme(dlg, s.look.get().dark);
            center_on_owner(dlg);
            enable(dlg, OK, false);
            // SAFETY: focuses this thread's box.
            unsafe { SetFocus(item(dlg, ID_NAME)) };
            0
        }
        WM_COMMAND => {
            let (id, code) = ((w & 0xffff) as u16, ((w >> 16) & 0xffff) as u32);
            match (id, code) {
                (ID_NAME, EN_CHANGE) => {
                    let typed = !window_text(item(dlg, ID_NAME)).trim().is_empty();
                    enable(dlg, OK, typed);
                }
                (OK, _) => {
                    let name = window_text(item(dlg, ID_NAME)).trim().to_owned();
                    if !name.is_empty() {
                        *s.name.borrow_mut() = Some(name);
                        // SAFETY: ends this modal dialog.
                        unsafe { EndDialog(dlg, 1) };
                    }
                }
                // SAFETY: ends this modal dialog.
                (CANCEL, _) => unsafe {
                    EndDialog(dlg, 0);
                },
                _ => return 0,
            }
            1
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
            skip: Vec::new(),
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
            skip: Vec::new(),
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
            skip: Vec::new(),
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
    fn copy_and_move_say_where_the_files_go() {
        let files = [file("a.txt"), file("b.txt"), file("c.txt")];
        let p = Preview {
            new: vec![Some("x.txt".into()), None, None],
            problems: vec![None; 3],
            error: None,
            skip: vec![false, false, true],
        };
        assert_eq!(
            transfer_status(&files, &p, true, false, Some(true)),
            ("3 files: 2 to move, 1 renamed".into(), true)
        );
        assert!(
            !transfer_status(&files, &p, false, false, None).1,
            "not before the folder is read"
        );
        assert!(
            transfer_status(&files, &p, false, false, None)
                .0
                .ends_with("checking the folder...")
        );
        assert_eq!(
            transfer_status(&files, &p, false, true, None).0,
            "Type or browse for the folder to copy to."
        );
        assert_eq!(
            transfer_status(&files, &p, false, false, Some(false)),
            ("The To folder does not exist.".into(), false)
        );
        assert_eq!(transfer_cell(&p, 0, "a.txt"), "x.txt");
        assert_eq!(
            transfer_cell(&p, 1, "b.txt"),
            "b.txt",
            "unmatched keeps its name"
        );
        assert_eq!(folder_key(r" D:\Copies\ "), r"d:\copies");
        assert_eq!(folder_key(r"C:\"), r"c:\");
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
