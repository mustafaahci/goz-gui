//! Search > Advanced Search... (F2 in the search box): Everything's dialog
//! for building a search from fields, with the fields goz can search:
//! all these words, this exact phrase (Match case for the whole search,
//! Match path per term), located in (a folder and Browse...), the size
//! from and to, the date modified from and to, the filter, and the
//! extensions. Everything's "any of these words" and "none of these
//! words" need OR and NOT: they are there (under the exact phrase) only
//! with a gozd that reads them (`client::boolean_syntax`), an OR group
//! `<a|b>` and `!a` terms. Everything's other fields (content, attributes,
//! ...) are what goz does not index; they are not here.
//!
//! [`query`] writes exactly what goz's parser takes (its tests parse every
//! form with `goz_core::query`).

use std::cell::{Cell, RefCell};

use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, SYSTEMTIME, WPARAM};
use windows_sys::Win32::System::SystemServices::SS_LEFT;
use windows_sys::Win32::UI::Controls::{
    DTM_GETMONTHCAL, DTM_GETSYSTEMTIME, DTM_SETSYSTEMTIME, DTN_DROPDOWN, DTS_SHORTDATEFORMAT,
    DTS_SHOWNONE, GDT_NONE, GDT_VALID, ICC_DATE_CLASSES, INITCOMMONCONTROLSEX,
    InitCommonControlsEx, NMHDR,
};
use windows_sys::Win32::UI::Shell::{DefSubclassProc, SetWindowSubclass};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    BM_GETCHECK, BS_AUTOCHECKBOX, BS_DEFPUSHBUTTON, CB_GETCURSEL, CBS_DROPDOWNLIST, ES_AUTOHSCROLL,
    ES_NUMBER, EndDialog, GWLP_USERDATA, GetDlgItem, IDCANCEL, IDOK, KillTimer, MB_ICONWARNING,
    MB_OK, SendMessageW, SetTimer, SetWindowLongPtrW, SetWindowTextW, USER_TIMER_MINIMUM,
    WM_COMMAND, WM_INITDIALOG, WM_NOTIFY, WM_SHOWWINDOW, WM_TIMER, WS_EX_CLIENTEDGE, WS_TABSTOP,
    WS_VSCROLL,
};

use windows_sys::Win32::UI::Input::KeyboardAndMouse::SetFocus;

use crate::dialog::{Class, Look, Template, center_on_owner, common, state, theme_new};
use crate::msgbox;
use crate::searchtools::folder_term;
use crate::wide::{window_text, wz};

/// A size's unit: Everything's list, 1024-based as goz's.
pub const UNITS: [(&str, &str); 5] = [
    ("Bytes", ""),
    ("KB", "kb"),
    ("MB", "mb"),
    ("GB", "gb"),
    ("TB", "tb"),
];
/// The unit a size starts in, as Everything's: MB.
const DEFAULT_UNIT: usize = 2;

/// A size as typed: a whole number and its unit (an index into
/// [`UNITS`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Size {
    pub value: u64,
    pub unit: usize,
}

impl Size {
    fn bytes(self) -> Option<u64> {
        let shift = 10 * self.unit.min(UNITS.len() - 1) as u32;
        self.value.checked_mul(1u64 << shift)
    }

    fn term(self) -> String {
        format!("{}{}", self.value, UNITS[self.unit.min(UNITS.len() - 1)].1)
    }
}

/// A day: year, month, day.
pub type Day = (u16, u16, u16);

fn day_text((y, m, d): Day) -> String {
    format!("{y:04}-{m:02}-{d:02}")
}

/// The dialog's fields.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Form {
    pub words: String,
    pub phrase: String,
    pub match_case: bool,
    pub match_path: bool,
    pub located_in: String,
    pub size_from: Option<Size>,
    pub size_to: Option<Size>,
    pub date_from: Option<Day>,
    pub date_to: Option<Day>,
    pub extensions: String,
    /// Any of these words (an OR group) and none of them (`!` terms): only
    /// read with `boolean`, a gozd that reads the new query language.
    pub any_words: String,
    pub none_words: String,
    pub boolean: bool,
}

/// Why the fields make no search, and which field to fix.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Problem {
    pub message: String,
    pub field: Field,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Field {
    SizeFrom,
    SizeTo,
    DateTo,
}

/// Whether `word`, as a term on its own, would be read as something else
/// than text: an operator (`|`, `!`; `<` and `>` too when gozd reads
/// groups), or a `name:` function.
fn needs_quotes(word: &str, boolean: bool) -> bool {
    if word.contains(['|', '!']) || (boolean && word.contains(['<', '>'])) {
        return true;
    }
    match word.find(':') {
        Some(colon) => {
            let name = &word[..colon];
            name.len() > 1 && name.bytes().all(|b| b.is_ascii_alphabetic())
        }
        None => false,
    }
}

/// A text term: quoted when it must be, a full path term with Match path.
fn text_term(text: &str, quoted: bool, path: bool, boolean: bool) -> String {
    let body = if quoted || needs_quotes(text, boolean) || text.contains(char::is_whitespace) {
        format!("\"{text}\"")
    } else {
        text.to_owned()
    };
    if path { format!("path:{body}") } else { body }
}

/// The words of a box, its quotes taken out.
fn words_of(text: &str) -> Vec<String> {
    text.replace('"', " ")
        .split_whitespace()
        .map(str::to_owned)
        .collect()
}

/// The search the fields make, in goz's syntax: the folder, the words, the
/// phrase, any of the words (`<a|b>`) and none of them (`!c`, with a gozd
/// that reads them), the extensions, the size, the date, then `case:`.
/// Quotes cannot be searched for (goz has no escape for them): they are
/// left out.
pub fn query(form: &Form) -> Result<String, Problem> {
    let mut terms: Vec<String> = Vec::new();
    let b = form.boolean;
    let folder = form.located_in.trim();
    if !folder.is_empty() {
        terms.push(folder_term(&folder.replace('"', "")));
    }
    for word in words_of(&form.words) {
        terms.push(text_term(&word, false, form.match_path, b));
    }
    let phrase = form.phrase.replace('"', "");
    let phrase = phrase.trim();
    if !phrase.is_empty() {
        terms.push(text_term(phrase, true, form.match_path, b));
    }
    if b {
        let any: Vec<String> = words_of(&form.any_words)
            .iter()
            .map(|w| text_term(w, false, form.match_path, b))
            .collect();
        match any.len() {
            0 => {}
            1 => terms.push(any[0].clone()),
            _ => terms.push(format!("<{}>", any.join("|"))),
        }
        for word in words_of(&form.none_words) {
            terms.push(format!("!{}", text_term(&word, false, form.match_path, b)));
        }
    }
    let exts: Vec<String> = form
        .extensions
        .split([';', ',', ' '])
        .map(|e| e.trim().trim_start_matches('*').trim_start_matches('.'))
        .filter(|e| !e.is_empty() && !e.contains(['"', '|', '!']))
        .map(str::to_owned)
        .collect();
    if !exts.is_empty() {
        terms.push(format!("ext:{}", exts.join(";")));
    }
    let too_big = |field| Problem {
        message: "The size is too large.".into(),
        field,
    };
    let from = form
        .size_from
        .map(|s| s.bytes().ok_or_else(|| too_big(Field::SizeFrom)))
        .transpose()?;
    let to = form
        .size_to
        .map(|s| s.bytes().ok_or_else(|| too_big(Field::SizeTo)))
        .transpose()?;
    match (form.size_from, form.size_to) {
        (Some(a), Some(b)) => {
            if from > to {
                return Err(Problem {
                    message: "The size's lower bound is more than its upper bound.".into(),
                    field: Field::SizeTo,
                });
            }
            terms.push(format!("size:{}..{}", a.term(), b.term()));
        }
        (Some(a), None) => terms.push(format!("size:>={}", a.term())),
        (None, Some(b)) => terms.push(format!("size:<={}", b.term())),
        (None, None) => {}
    }
    match (form.date_from, form.date_to) {
        (Some(a), Some(b)) => {
            if a > b {
                return Err(Problem {
                    message: "The date modified ends before it starts.".into(),
                    field: Field::DateTo,
                });
            }
            terms.push(format!("dm:{}..{}", day_text(a), day_text(b)));
        }
        (Some(a), None) => terms.push(format!("dm:>={}", day_text(a))),
        (None, Some(b)) => terms.push(format!("dm:<={}", day_text(b))),
        (None, None) => {}
    }
    if form.match_case {
        terms.push("case:".into());
    }
    Ok(terms.join(" "))
}

/// Asks for a folder, starting in `start` (if any); its path, or `None`
/// when cancelled.
pub fn pick_folder(owner: HWND, start: &str) -> Option<String> {
    use windows::Win32::Foundation::HWND as WHWND;
    use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, CoCreateInstance, CoTaskMemFree};
    use windows::Win32::UI::Shell::{
        FOS_FORCEFILESYSTEM, FOS_NOCHANGEDIR, FOS_PATHMUSTEXIST, FOS_PICKFOLDERS, FileOpenDialog,
        IFileOpenDialog, IShellItem, SHCreateItemFromParsingName, SIGDN_FILESYSPATH,
    };
    use windows::core::{HSTRING, PCWSTR};
    // SAFETY: the system's folder dialog, made, shown modal over a window
    // of this (COM) thread and read on it; the strings outlive the calls
    // and the path it gives is freed with the COM allocator.
    unsafe {
        let dialog: IFileOpenDialog =
            CoCreateInstance(&FileOpenDialog, None, CLSCTX_INPROC_SERVER).ok()?;
        let options = dialog.GetOptions().ok()?;
        dialog
            .SetOptions(
                options
                    | FOS_PICKFOLDERS
                    | FOS_FORCEFILESYSTEM
                    | FOS_PATHMUSTEXIST
                    | FOS_NOCHANGEDIR,
            )
            .ok()?;
        let start = start.trim();
        if !start.is_empty()
            && let Ok(folder) =
                SHCreateItemFromParsingName::<_, _, IShellItem>(&HSTRING::from(start), None)
        {
            let _ = dialog.SetFolder(&folder);
        }
        let _ = dialog.SetTitle(PCWSTR(wz("Select a folder").as_ptr()));
        dialog.Show(Some(WHWND(owner))).ok()?;
        let item = dialog.GetResult().ok()?;
        let name = item.GetDisplayName(SIGDN_FILESYSPATH).ok()?;
        let path = name.to_string().ok();
        CoTaskMemFree(Some(name.0 as *const core::ffi::c_void));
        path
    }
}

const STATIC: u16 = 0xffff;
const ID_WORDS: u16 = 1004;
const ID_PHRASE: u16 = 1009;
const ID_ANY: u16 = 1006;
const ID_NONE: u16 = 1007;
const ID_CASE: u16 = 1005;
const ID_PATH: u16 = 1145;
const ID_FOLDER: u16 = 1032;
const ID_BROWSE: u16 = 1033;
const ID_SIZE_FROM: u16 = 1063;
const ID_SIZE_FROM_UNIT: u16 = 1064;
const ID_SIZE_TO: u16 = 1066;
const ID_SIZE_TO_UNIT: u16 = 1067;
const ID_DATE_FROM: u16 = 1036;
const ID_DATE_TO: u16 = 1038;
const ID_FILTER_LABEL: u16 = 1056;
const ID_FILTER: u16 = 1057;
const ID_EXT: u16 = 1059;
/// Where the Filter list goes (it is made once the dialog shows), `dy`
/// lower with the any and none rows.
fn filter_at(dy: i16) -> crate::dialog::Units {
    (7, 183 + dy, 286, 160)
}
/// How much lower everything under the exact phrase is with the any and
/// none rows.
const BOOLEAN_ROWS: i16 = 36;
/// The dialog's timer: it has been drawn, the Filter list can be made.
const MAKE_FILTER: usize = 1;
const OK: u16 = IDOK as u16;
const CANCEL: u16 = IDCANCEL as u16;

/// What the dialog starts with.
pub struct Start {
    /// The filters' names, in the menus' order, and the one in force.
    pub filters: Vec<String>,
    pub filter: usize,
    /// Where Browse... starts.
    pub last_folder: String,
    /// gozd reads OR and NOT: the any and none rows are there.
    pub boolean_syntax: bool,
}

/// What the dialog ended with.
pub struct Done {
    pub query: String,
    /// The filter chosen, an index into `Start::filters`.
    pub filter: usize,
    /// The folder Browse... chose, if it was used.
    pub browsed: Option<String>,
}

struct Dlg<'a> {
    start: &'a Start,
    look: Cell<Look>,
    browsed: RefCell<Option<String>>,
    result: RefCell<Option<Done>>,
    /// Whether the Filter list is made yet (`make_filter`).
    filter_made: Cell<bool>,
    /// How much lower the rows under the phrase are (`BOOLEAN_ROWS`).
    dy: i16,
}

fn item(dlg: HWND, id: u16) -> HWND {
    // SAFETY: looks up a child of this thread's dialog.
    unsafe { GetDlgItem(dlg, i32::from(id)) }
}

fn send(hwnd: HWND, msg: u32, w: usize, l: isize) -> isize {
    // SAFETY: a message to a control of this thread's dialog; any pointer
    // in `l` is the caller's and lives across the call.
    unsafe { SendMessageW(hwnd, msg, w, l) }
}

/// Shows the dialog over `owner`; the search it built on OK.
pub fn ask(owner: HWND, start: &Start, look: Look) -> Option<Done> {
    // The date pickers' class.
    let icc = INITCOMMONCONTROLSEX {
        dwSize: size_of::<INITCOMMONCONTROLSEX>() as u32,
        dwICC: ICC_DATE_CLASSES,
    };
    // SAFETY: registers the common controls' date classes, once per
    // process whatever the number of calls.
    unsafe { InitCommonControlsEx(&icc) };
    let state = Dlg {
        start,
        look: Cell::new(look),
        browsed: RefCell::new(None),
        result: RefCell::new(None),
        filter_made: Cell::new(false),
        dy: if start.boolean_syntax {
            BOOLEAN_ROWS
        } else {
            0
        },
    };
    let dy = state.dy;
    let label = SS_LEFT;
    let edit = WS_TABSTOP | ES_AUTOHSCROLL as u32;
    // Five units need no scroll bar, which costs a themed list a few
    // milliseconds to make.
    let units = WS_TABSTOP | CBS_DROPDOWNLIST as u32;
    let check = WS_TABSTOP | BS_AUTOCHECKBOX as u32;
    let picker = WS_TABSTOP | DTS_SHORTDATEFORMAT | DTS_SHOWNONE;
    let (lx, fx, right) = (7, 82, 293);
    let mut t = Template::new("Advanced Search", 0, 300, 262 + dy);
    t.control(
        Class::Static,
        "Filenames containing...",
        STATIC,
        label,
        0,
        (lx, 7, 286, 8),
    )
    .control(
        Class::Static,
        "&all these words:",
        STATIC,
        label,
        0,
        (lx + 6, 23, 70, 8),
    )
    .control(
        Class::Edit,
        "",
        ID_WORDS,
        edit,
        WS_EX_CLIENTEDGE,
        (fx, 21, right - fx, 14),
    )
    .control(
        Class::Static,
        "this &exact phrase:",
        STATIC,
        label,
        0,
        (lx + 6, 41, 70, 8),
    )
    .control(
        Class::Edit,
        "",
        ID_PHRASE,
        edit,
        WS_EX_CLIENTEDGE,
        (fx, 39, right - fx, 14),
    );
    if start.boolean_syntax {
        t.control(
            Class::Static,
            "an&y of these words:",
            STATIC,
            label,
            0,
            (lx + 6, 59, 70, 8),
        )
        .control(
            Class::Edit,
            "",
            ID_ANY,
            edit,
            WS_EX_CLIENTEDGE,
            (fx, 57, right - fx, 14),
        )
        .control(
            Class::Static,
            "&none of these words:",
            STATIC,
            label,
            0,
            (lx + 6, 77, 74, 8),
        )
        .control(
            Class::Edit,
            "",
            ID_NONE,
            edit,
            WS_EX_CLIENTEDGE,
            (fx, 75, right - fx, 14),
        );
    }
    t.control(
        Class::Button,
        "Match &case",
        ID_CASE,
        check,
        0,
        (fx, 57 + dy, 70, 10),
    )
    .control(
        Class::Button,
        "Match &path",
        ID_PATH,
        check,
        0,
        (fx + 76, 57 + dy, 70, 10),
    )
    .control(
        Class::Static,
        "&Located in:",
        STATIC,
        label,
        0,
        (lx, 76 + dy, 200, 8),
    )
    .control(
        Class::Edit,
        "",
        ID_FOLDER,
        edit,
        WS_EX_CLIENTEDGE,
        (lx, 87 + dy, 232, 14),
    )
    .control(
        Class::Button,
        "Bro&wse...",
        ID_BROWSE,
        WS_TABSTOP,
        0,
        (243, 86 + dy, 50, 16),
    )
    .control(
        Class::Static,
        "&Size:",
        STATIC,
        label,
        0,
        (lx, 108 + dy, 100, 8),
    )
    // The size boxes are as tall as the unit lists beside them (whose
    // height is the font's): 12 units, the row's edges in line.
    .control(
        Class::Edit,
        "",
        ID_SIZE_FROM,
        edit | ES_NUMBER as u32,
        WS_EX_CLIENTEDGE,
        (lx, 119 + dy, 60, 12),
    )
    .control(
        Class::Named("ComboBox"),
        "",
        ID_SIZE_FROM_UNIT,
        units,
        0,
        (71, 119 + dy, 44, 120),
    )
    .control(
        Class::Static,
        "to",
        STATIC,
        label,
        0,
        (121, 121 + dy, 12, 8),
    )
    .control(
        Class::Edit,
        "",
        ID_SIZE_TO,
        edit | ES_NUMBER as u32,
        WS_EX_CLIENTEDGE,
        (135, 119 + dy, 60, 12),
    )
    .control(
        Class::Named("ComboBox"),
        "",
        ID_SIZE_TO_UNIT,
        units,
        0,
        (199, 119 + dy, 44, 120),
    )
    .control(
        Class::Static,
        "&Date modified:",
        STATIC,
        label,
        0,
        (lx, 140 + dy, 100, 8),
    )
    .control(
        Class::Named("SysDateTimePick32"),
        "",
        ID_DATE_FROM,
        picker,
        0,
        (lx, 151 + dy, 108, 14),
    )
    .control(
        Class::Static,
        "to",
        STATIC,
        label,
        0,
        (121, 154 + dy, 12, 8),
    )
    .control(
        Class::Named("SysDateTimePick32"),
        "",
        ID_DATE_TO,
        picker,
        0,
        (135, 151 + dy, 108, 14),
    )
    .control(
        Class::Static,
        "&Filter:",
        ID_FILTER_LABEL,
        label,
        0,
        (lx, 172 + dy, 100, 8),
    )
    // The Filter list comes once the dialog shows (`make_filter`).
    .control(
        Class::Static,
        "E&xtension:",
        STATIC,
        label,
        0,
        (lx, 204 + dy, 100, 8),
    )
    .control(
        Class::Edit,
        "",
        ID_EXT,
        edit,
        WS_EX_CLIENTEDGE,
        (lx, 215 + dy, right - lx, 14),
    )
    .control(
        Class::Button,
        "OK",
        OK,
        WS_TABSTOP | BS_DEFPUSHBUTTON as u32,
        0,
        (189, 241 + dy, 50, 14),
    )
    .control(
        Class::Button,
        "Cancel",
        CANCEL,
        WS_TABSTOP,
        0,
        (243, 241 + dy, 50, 14),
    );
    t.run(owner, Some(proc), (&raw const state) as LPARAM);
    state.result.into_inner()
}

fn fill_combo(dlg: HWND, id: u16, items: impl IntoIterator<Item = String>, select: usize) {
    // A themed list takes half a millisecond an item: the chosen item now,
    // the rest once the dialog shows (`dialog::fill_later`).
    crate::dialog::fill_later(item(dlg, id), items.into_iter().collect(), Some(select));
}

fn selection(dlg: HWND, id: u16) -> usize {
    usize::try_from(send(item(dlg, id), CB_GETCURSEL, 0, 0)).unwrap_or(0)
}

fn checked(dlg: HWND, id: u16) -> bool {
    send(item(dlg, id), BM_GETCHECK, 1, 0) == 1
}

/// A size box's value with its unit; `Err` when it holds something that
/// is no whole number.
fn size_of_box(dlg: HWND, id: u16, unit: u16) -> Result<Option<Size>, ()> {
    let text = window_text(item(dlg, id));
    let text = text.trim();
    if text.is_empty() {
        return Ok(None);
    }
    let value = text.parse::<u64>().map_err(|_| ())?;
    Ok(Some(Size {
        value,
        unit: selection(dlg, unit),
    }))
}

/// A date picker's day, `None` while its box is unticked.
fn day_of(dlg: HWND, id: u16) -> Option<Day> {
    let mut st = SYSTEMTIME::default();
    let r = send(item(dlg, id), DTM_GETSYSTEMTIME, 0, (&raw mut st) as isize);
    (r == GDT_VALID as isize).then_some((st.wYear, st.wMonth, st.wDay))
}

impl Dlg<'_> {
    fn init(&self, dlg: HWND) {
        let units = || UNITS.iter().map(|(n, _)| (*n).to_owned());
        fill_combo(dlg, ID_SIZE_FROM_UNIT, units(), DEFAULT_UNIT);
        fill_combo(dlg, ID_SIZE_TO_UNIT, units(), DEFAULT_UNIT);
        // No date until one is ticked.
        for id in [ID_DATE_FROM, ID_DATE_TO] {
            send(item(dlg, id), DTM_SETSYSTEMTIME, GDT_NONE as usize, 0);
        }
        theme_new(dlg, self.look.get().dark);
        for id in [ID_DATE_FROM, ID_DATE_TO] {
            let picker = item(dlg, id);
            // SAFETY: subclasses this thread's date picker; the subclass
            // removes itself when the picker goes.
            unsafe { SetWindowSubclass(picker, Some(picker_proc), 1, dlg as usize) };
        }
        center_on_owner(dlg);
    }

    /// Makes the Filter list, once the dialog has been drawn or as soon as
    /// it is read: each themed list costs 5 to 7 ms to make in comctl32,
    /// and the dialog has three (the units' two before it).
    fn make_filter(&self, dlg: HWND) {
        if self.filter_made.replace(true) {
            return;
        }
        // SAFETY: a timer of this thread's dialog (fired or not).
        unsafe { KillTimer(dlg, MAKE_FILTER) };
        let list = crate::dialog::make_control(
            dlg,
            "ComboBox",
            ID_FILTER,
            WS_TABSTOP | WS_VSCROLL | CBS_DROPDOWNLIST as u32,
            filter_at(self.dy),
            item(dlg, ID_FILTER_LABEL),
        );
        fill_combo(
            dlg,
            ID_FILTER,
            self.start.filters.iter().cloned(),
            self.start.filter,
        );
        crate::dialog::theme_control(list, self.look.get().dark, true);
    }

    fn form(&self, dlg: HWND) -> Result<Form, Problem> {
        let bad = |field, what: &str| Problem {
            message: format!("Type the {what} size as a whole number."),
            field,
        };
        Ok(Form {
            words: window_text(item(dlg, ID_WORDS)),
            phrase: window_text(item(dlg, ID_PHRASE)),
            match_case: checked(dlg, ID_CASE),
            match_path: checked(dlg, ID_PATH),
            located_in: window_text(item(dlg, ID_FOLDER)),
            size_from: size_of_box(dlg, ID_SIZE_FROM, ID_SIZE_FROM_UNIT)
                .map_err(|()| bad(Field::SizeFrom, "smallest"))?,
            size_to: size_of_box(dlg, ID_SIZE_TO, ID_SIZE_TO_UNIT)
                .map_err(|()| bad(Field::SizeTo, "largest"))?,
            date_from: day_of(dlg, ID_DATE_FROM),
            date_to: day_of(dlg, ID_DATE_TO),
            extensions: window_text(item(dlg, ID_EXT)),
            any_words: self.boolean_text(dlg, ID_ANY),
            none_words: self.boolean_text(dlg, ID_NONE),
            boolean: self.start.boolean_syntax,
        })
    }

    /// An any or none box's text; nothing when the dialog has none.
    fn boolean_text(&self, dlg: HWND, id: u16) -> String {
        if self.start.boolean_syntax {
            window_text(item(dlg, id))
        } else {
            String::new()
        }
    }

    fn ok(&self, dlg: HWND) {
        self.make_filter(dlg);
        match self.form(dlg).and_then(|f| query(&f)) {
            Ok(query) => {
                *self.result.borrow_mut() = Some(Done {
                    query,
                    filter: selection(dlg, ID_FILTER),
                    browsed: self.browsed.borrow_mut().take(),
                });
                // SAFETY: ends this modal dialog.
                unsafe { EndDialog(dlg, 1) };
            }
            Err(p) => {
                msgbox::show(dlg, &p.message, "Advanced Search", MB_OK | MB_ICONWARNING);
                let id = match p.field {
                    Field::SizeFrom => ID_SIZE_FROM,
                    Field::SizeTo => ID_SIZE_TO,
                    Field::DateTo => ID_DATE_TO,
                };
                // SAFETY: focuses a control of this thread's dialog.
                unsafe { SetFocus(item(dlg, id)) };
            }
        }
    }

    fn browse(&self, dlg: HWND) {
        let typed = window_text(item(dlg, ID_FOLDER));
        let from = if typed.trim().is_empty() {
            self.browsed
                .borrow()
                .clone()
                .unwrap_or_else(|| self.start.last_folder.clone())
        } else {
            typed
        };
        if let Some(folder) = pick_folder(dlg, &from) {
            // SAFETY: a NUL-terminated string for a control of this
            // thread's dialog.
            unsafe { SetWindowTextW(item(dlg, ID_FOLDER), wz(&folder).as_ptr()) };
            *self.browsed.borrow_mut() = Some(folder);
        }
    }
}

/// A pixel (BGRA) with its lightness turned over and its hue and
/// saturation kept: white becomes black, black text white, the blue of a
/// selected field a lighter blue. (In HSL, lightness `1 - L` with the same
/// hue and saturation adds `255 - max - min` to each channel.)
fn flip_lightness(bgra: u32) -> u32 {
    let ch = |shift: u32| ((bgra >> shift) & 0xff) as i32;
    let (b, g, r) = (ch(0), ch(8), ch(16));
    let shift = 255 - r.max(g).max(b) - r.min(g).min(b);
    let put = |c: i32, at: u32| ((c + shift).clamp(0, 255) as u32) << at;
    (bgra & 0xff00_0000) | put(r, 16) | put(g, 8) | put(b, 0)
}

/// Paints a date picker as the system draws it, its lightness turned over:
/// its themes draw it light whatever the dark ones ask, and drawn this way
/// it keeps every part the system draws (the field being typed in lit,
/// the check box, the button, the hover edge), only dark.
fn paint_dark_picker(hwnd: HWND) {
    use windows_sys::Win32::Graphics::Gdi::{
        BI_RGB, BITMAPINFO, BITMAPINFOHEADER, BeginPaint, BitBlt, CreateCompatibleDC,
        CreateDIBSection, DIB_RGB_COLORS, DeleteDC, DeleteObject, EndPaint, GdiFlush, PAINTSTRUCT,
        SRCCOPY, SelectObject,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::{GetClientRect, WM_PAINT};
    // SAFETY: a standard WM_PAINT of this thread's picker, drawn by its own
    // procedure into a bitmap of the client's size, whose pixels (32-bit,
    // top-down, `w * h` of them) are changed in place; every GDI object
    // made is let go before returning.
    unsafe {
        let mut ps: PAINTSTRUCT = core::mem::zeroed();
        let hdc = BeginPaint(hwnd, &mut ps);
        let mut rc = windows_sys::Win32::Foundation::RECT::default();
        GetClientRect(hwnd, &mut rc);
        let (w, h) = (rc.right, rc.bottom);
        if w > 0 && h > 0 {
            let mut bmi: BITMAPINFO = core::mem::zeroed();
            bmi.bmiHeader = BITMAPINFOHEADER {
                biSize: size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: w,
                biHeight: -h,
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB,
                ..core::mem::zeroed()
            };
            let mut bits: *mut core::ffi::c_void = core::ptr::null_mut();
            let dib = CreateDIBSection(
                hdc,
                &bmi,
                DIB_RGB_COLORS,
                &mut bits,
                core::ptr::null_mut(),
                0,
            );
            if !dib.is_null() && !bits.is_null() {
                let mem = CreateCompatibleDC(hdc);
                let old = SelectObject(mem, dib);
                DefSubclassProc(hwnd, WM_PAINT, mem as usize, 0);
                GdiFlush();
                let px = core::slice::from_raw_parts_mut(bits.cast::<u32>(), (w * h) as usize);
                for p in px.iter_mut() {
                    *p = flip_lightness(*p);
                }
                BitBlt(hdc, 0, 0, w, h, mem, 0, 0, SRCCOPY);
                SelectObject(mem, old);
                DeleteDC(mem);
            }
            if !dib.is_null() {
                DeleteObject(dib);
            }
        }
        EndPaint(hwnd, &ps);
    }
}

/// The subclass of the window a picker's calendar drops down in, in the
/// dark theme: its one-pixel frame in the theme's border colour, not white.
unsafe extern "system" fn frame_proc(
    hwnd: HWND,
    msg: u32,
    w: WPARAM,
    l: LPARAM,
    _id: usize,
    _data: usize,
) -> LRESULT {
    use windows_sys::Win32::Foundation::RECT;
    use windows_sys::Win32::Graphics::Gdi::{GetWindowDC, ReleaseDC};
    use windows_sys::Win32::UI::Shell::RemoveWindowSubclass;
    use windows_sys::Win32::UI::WindowsAndMessaging::{GetWindowRect, WM_NCDESTROY, WM_NCPAINT};
    // SAFETY: `w` and `l` are passed on as they came; the frame is drawn
    // on this thread's window's own DC, released after.
    unsafe {
        match msg {
            WM_NCPAINT => {
                let done = DefSubclassProc(hwnd, msg, w, l);
                let mut rc = RECT::default();
                GetWindowRect(hwnd, &mut rc);
                let (w, h) = (rc.right - rc.left, rc.bottom - rc.top);
                let dc = GetWindowDC(hwnd);
                let border = crate::theme::Palette::new(true).border;
                for edge in [
                    RECT {
                        left: 0,
                        top: 0,
                        right: w,
                        bottom: 1,
                    },
                    RECT {
                        left: 0,
                        top: h - 1,
                        right: w,
                        bottom: h,
                    },
                    RECT {
                        left: 0,
                        top: 0,
                        right: 1,
                        bottom: h,
                    },
                    RECT {
                        left: w - 1,
                        top: 0,
                        right: w,
                        bottom: h,
                    },
                ] {
                    crate::paint::fill(dc, &edge, border);
                }
                ReleaseDC(hwnd, dc);
                return done;
            }
            WM_NCDESTROY => {
                RemoveWindowSubclass(hwnd, Some(frame_proc), 1);
            }
            _ => {}
        }
        DefSubclassProc(hwnd, msg, w, l)
    }
}

/// The subclass of the date pickers and of their calendars as they drop
/// down (`data` is the dialog): drawn dark in the dark theme.
unsafe extern "system" fn picker_proc(
    hwnd: HWND,
    msg: u32,
    w: WPARAM,
    l: LPARAM,
    _id: usize,
    data: usize,
) -> LRESULT {
    use windows_sys::Win32::UI::Shell::RemoveWindowSubclass;
    use windows_sys::Win32::UI::WindowsAndMessaging::{WM_ERASEBKGND, WM_NCDESTROY, WM_PAINT};
    // SAFETY: `data` is the dialog, whose GWLP_USERDATA is its `Dlg` while
    // it lives (a calendar is gone before it); `w` and `l` are passed on as
    // they came.
    unsafe {
        let dark = state::<Dlg<'_>>(data as HWND).is_some_and(|s| s.look.get().dark);
        match msg {
            WM_ERASEBKGND if dark => return 1,
            WM_PAINT if dark && w == 0 => {
                paint_dark_picker(hwnd);
                return 0;
            }
            WM_NCDESTROY => {
                RemoveWindowSubclass(hwnd, Some(picker_proc), 1);
            }
            _ => {}
        }
        DefSubclassProc(hwnd, msg, w, l)
    }
}

unsafe extern "system" fn proc(dlg: HWND, msg: u32, w: WPARAM, l: LPARAM) -> isize {
    if msg == WM_INITDIALOG {
        // SAFETY: `l` is the `Dlg` that `ask` keeps alive until the dialog
        // has ended.
        unsafe { SetWindowLongPtrW(dlg, GWLP_USERDATA, l) };
    }
    // SAFETY: GWLP_USERDATA is the `Dlg` from `ask`, or null.
    let Some(s) = (unsafe { state::<Dlg<'_>>(dlg) }) else {
        return 0;
    };
    if let Some(answer) = common(dlg, msg, w, l, &s.look) {
        return answer;
    }
    match msg {
        WM_INITDIALOG => {
            s.init(dlg);
            // The keyboard in the first box, the words.
            1
        }
        // Shown (a modal dialog shows once its queue is empty): the Filter
        // list follows once it has been drawn, as timers come after the
        // paints waiting.
        WM_SHOWWINDOW if w != 0 => {
            // SAFETY: a timer of this thread's dialog.
            unsafe { SetTimer(dlg, MAKE_FILTER, USER_TIMER_MINIMUM, None) };
            0
        }
        WM_TIMER if w == MAKE_FILTER => {
            s.make_filter(dlg);
            1
        }
        WM_COMMAND => {
            match (w & 0xffff) as u16 {
                OK => s.ok(dlg),
                // SAFETY: ends this modal dialog.
                CANCEL => unsafe {
                    EndDialog(dlg, 0);
                },
                ID_BROWSE => s.browse(dlg),
                _ => return 0,
            }
            1
        }
        // A picker's calendar drops down: dark too, as the picker is.
        WM_NOTIFY => {
            // SAFETY: WM_NOTIFY's lParam is an NMHDR from a control.
            let hdr = unsafe { &*(l as *const NMHDR) };
            if hdr.code == DTN_DROPDOWN && s.look.get().dark {
                let calendar = send(hdr.hwndFrom, DTM_GETMONTHCAL, 0, 0) as HWND;
                if !calendar.is_null() {
                    // SAFETY: subclasses this thread's calendar; the
                    // subclass removes itself when it goes.
                    unsafe {
                        SetWindowSubclass(calendar, Some(picker_proc), 1, dlg as usize);
                        windows_sys::Win32::Graphics::Gdi::InvalidateRect(
                            calendar,
                            core::ptr::null(),
                            0,
                        );
                        // Its frame, the window it drops down in, too.
                        let frame =
                            windows_sys::Win32::UI::WindowsAndMessaging::GetParent(calendar);
                        if !frame.is_null() {
                            use windows_sys::Win32::Graphics::Gdi::{
                                RDW_FRAME, RDW_INVALIDATE, RedrawWindow,
                            };
                            SetWindowSubclass(frame, Some(frame_proc), 1, 0);
                            RedrawWindow(
                                frame,
                                core::ptr::null(),
                                core::ptr::null_mut(),
                                RDW_FRAME | RDW_INVALIDATE,
                            );
                        }
                    }
                }
            }
            0
        }
        _ => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use goz_core::query::{Kind, LocalClock, parse_query_at};

    struct Clock;
    impl LocalClock for Clock {
        fn now(&self) -> i64 {
            133_801_632_000_000_000
        }
        fn to_local(&self, utc: i64) -> Option<i64> {
            Some(utc)
        }
        fn to_utc(&self, local: i64) -> Option<i64> {
            Some(local)
        }
        fn week_start(&self) -> u8 {
            0
        }
        fn date_order(&self) -> goz_core::query::DateOrder {
            goz_core::query::DateOrder::MonthDayYear
        }
    }

    fn parse(q: &str) -> goz_core::query::ParsedQuery {
        parse_query_at(q, false, &Clock).unwrap_or_else(|e| panic!("{q}: {e}"))
    }

    #[test]
    fn a_dark_picker_keeps_its_hues() {
        // White to black, black to white, greys mirrored.
        assert_eq!(flip_lightness(0xffff_ffff), 0xff00_0000);
        assert_eq!(flip_lightness(0xff00_0000), 0xffff_ffff);
        assert_eq!(flip_lightness(0x006d_6d6d), 0x0092_9292);
        // Windows' selection blue (0, 120, 215) stays blue, lighter.
        assert_eq!(flip_lightness(0x0000_78d7), 0x0028_a0ff);
    }

    /// The shape of a search in the new query language, as its grammar
    /// documents it (goz-core v0.6.0 cannot parse it): `<` and `>` balanced
    /// outside quotes, quotes paired, and every `!` right before its term.
    fn well_formed(q: &str) -> bool {
        let (mut depth, mut quoted) = (0i32, false);
        let chars: Vec<char> = q.chars().collect();
        for (i, &c) in chars.iter().enumerate() {
            match c {
                '"' => quoted = !quoted,
                _ if quoted => {}
                '<' => depth += 1,
                '>' => {
                    depth -= 1;
                    if depth < 0 {
                        return false;
                    }
                }
                '!' if chars.get(i + 1).is_none_or(|n| n.is_whitespace()) => return false,
                _ => {}
            }
        }
        depth == 0 && !quoted
    }

    #[test]
    fn any_and_none_make_an_or_group_and_not_terms() {
        let form = Form {
            words: "report".into(),
            any_words: "pdf  docx".into(),
            none_words: "draft old".into(),
            boolean: true,
            ..Form::default()
        };
        let q = query(&form).expect("a search");
        assert_eq!(q, "report <pdf|docx> !draft !old");
        assert!(well_formed(&q));
        // One word needs no group; words the syntax would read are quoted;
        // Match path reaches every term.
        let form = Form {
            any_words: "a|b".into(),
            none_words: "<x> y!".into(),
            match_path: true,
            boolean: true,
            ..Form::default()
        };
        let q = query(&form).expect("a search");
        assert_eq!(q, r#"path:"a|b" !path:"<x>" !path:"y!""#);
        assert!(well_formed(&q));
        let form = Form {
            any_words: "a\"b c".into(),
            phrase: "x <y>".into(),
            boolean: true,
            ..Form::default()
        };
        let q = query(&form).expect("a search");
        assert_eq!(q, r#""x <y>" <a|b|c>"#);
        assert!(well_formed(&q));
        // Without a gozd that reads them, the boxes are not read at all,
        // and `<` stays text.
        let form = Form {
            words: "<a>".into(),
            any_words: "pdf docx".into(),
            none_words: "draft".into(),
            ..Form::default()
        };
        assert_eq!(query(&form).as_deref(), Ok("<a>"));
    }

    #[test]
    fn nothing_filled_is_every_result() {
        assert_eq!(query(&Form::default()).as_deref(), Ok(""));
    }

    #[test]
    fn every_field_makes_what_goz_parses() {
        let form = Form {
            words: "annual  report".into(),
            phrase: "final draft".into(),
            match_case: true,
            match_path: false,
            located_in: r"C:\Program Files".into(),
            size_from: Some(Size { value: 10, unit: 1 }),
            size_to: Some(Size { value: 2, unit: 2 }),
            date_from: Some((2024, 5, 1)),
            date_to: Some((2024, 6, 30)),
            extensions: ".pdf; *.docx,txt".into(),
            ..Form::default()
        };
        let q = query(&form).expect("a search");
        assert_eq!(
            q,
            r#""C:\Program Files\" annual report "final draft" ext:pdf;docx;txt size:10kb..2mb dm:2024-05-01..2024-06-30 case:"#
        );
        let p = parse(&q);
        assert!(p.match_case);
        assert_eq!(
            p.name_terms,
            [
                b"annual".to_vec(),
                b"report".to_vec(),
                b"final draft".to_vec()
            ]
        );
        assert_eq!(p.path_terms, [br"C:\Program Files\".to_vec()]);
        assert_eq!(p.filters.ext.map(|e| e.len()), Some(3));
        let size = p.filters.size.expect("size");
        assert_eq!((size.min, size.max), (10 * 1024, 2 * 1024 * 1024));
        assert!(p.filters.date.is_some());
    }

    #[test]
    fn match_path_makes_path_terms() {
        let form = Form {
            words: "src main".into(),
            phrase: "my docs".into(),
            match_path: true,
            ..Form::default()
        };
        let q = query(&form).expect("a search");
        assert_eq!(q, r#"path:src path:main path:"my docs""#);
        let p = parse(&q);
        assert_eq!(p.path_terms.len(), 3);
        assert!(p.name_terms.is_empty());
    }

    #[test]
    fn open_ended_ranges() {
        let only = |from: Option<Size>, to: Option<Size>| {
            query(&Form {
                size_from: from,
                size_to: to,
                ..Form::default()
            })
            .expect("a search")
        };
        let q = only(Some(Size { value: 1, unit: 3 }), None);
        assert_eq!(q, "size:>=1gb");
        assert_eq!(parse(&q).filters.size.map(|s| s.min), Some(1 << 30));
        let q = only(
            None,
            Some(Size {
                value: 500,
                unit: 0,
            }),
        );
        assert_eq!(q, "size:<=500");
        assert_eq!(parse(&q).filters.size.map(|s| s.max), Some(500));
        let q = query(&Form {
            date_from: Some((2023, 1, 9)),
            ..Form::default()
        })
        .expect("a search");
        assert_eq!(q, "dm:>=2023-01-09");
        parse(&q);
        let q = query(&Form {
            date_to: Some((2023, 12, 31)),
            ..Form::default()
        })
        .expect("a search");
        assert_eq!(q, "dm:<=2023-12-31");
        parse(&q);
    }

    #[test]
    fn what_goz_would_misread_is_quoted() {
        let q = query(&Form {
            words: "a|b !x ext:pdf C:\\Windows 12:30".into(),
            ..Form::default()
        })
        .expect("a search");
        assert_eq!(q, r#""a|b" "!x" "ext:pdf" C:\Windows 12:30"#);
        let p = parse(&q);
        assert_eq!(p.filters.ext, None);
        assert_eq!(p.name_terms.len(), 4);
        assert_eq!(p.path_terms.len(), 1);
        // Quotes cannot be searched: they go.
        let q = query(&Form {
            phrase: "say \"hi\"".into(),
            ..Form::default()
        })
        .expect("a search");
        assert_eq!(q, "\"say hi\"");
        assert_eq!(parse(&q).filters.kind, None::<Kind>);
    }

    #[test]
    fn backwards_ranges_are_refused() {
        let p = query(&Form {
            size_from: Some(Size { value: 2, unit: 2 }),
            size_to: Some(Size { value: 1, unit: 2 }),
            ..Form::default()
        })
        .expect_err("refused");
        assert_eq!(p.field, Field::SizeTo);
        let p = query(&Form {
            date_from: Some((2024, 2, 1)),
            date_to: Some((2024, 1, 31)),
            ..Form::default()
        })
        .expect_err("refused");
        assert_eq!(p.field, Field::DateTo);
        // Too large to count in bytes.
        let p = query(&Form {
            size_from: Some(Size {
                value: u64::MAX,
                unit: 4,
            }),
            ..Form::default()
        })
        .expect_err("refused");
        assert_eq!(p.field, Field::SizeFrom);
    }
}
