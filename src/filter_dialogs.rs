//! Search > Organize Filters... (Ctrl+Shift+K) and the filter editor (New
//! Filter, Edit Filter, Add to Filters...), as Everything's: the filters
//! listed with their Name, Search, Macro and Key; New..., Edit...,
//! Move Up, Move Down, Delete, Import..., Export... and Reset; OK keeps
//! the changes, Cancel none of them.
//!
//! Everything's built-in filters come first and are goz-gui's own: they
//! are listed, not edited (selecting one says so). A filter that needs a
//! match option goz cannot apply yet is listed and kept, and says so; its
//! name and search can still be edited.

use std::cell::{Cell, RefCell};

use windows_sys::Win32::Foundation::{HWND, LPARAM, RECT, WPARAM};
use windows_sys::Win32::System::SystemServices::SS_LEFT;
use windows_sys::Win32::UI::Controls::{
    LVCF_TEXT, LVCF_WIDTH, LVCOLUMNW, LVIF_TEXT, LVIS_FOCUSED, LVIS_SELECTED, LVITEMW,
    LVM_DELETEALLITEMS, LVM_ENSUREVISIBLE, LVM_GETNEXTITEM, LVM_INSERTCOLUMNW, LVM_INSERTITEMW,
    LVM_SETEXTENDEDLISTVIEWSTYLE, LVM_SETITEMSTATE, LVM_SETITEMTEXTW, LVN_ITEMCHANGED, LVN_KEYDOWN,
    LVNI_SELECTED, LVS_EX_DOUBLEBUFFER, LVS_EX_FULLROWSELECT, LVS_EX_LABELTIP, LVS_NOSORTHEADER,
    LVS_REPORT, LVS_SHOWSELALWAYS, LVS_SINGLESEL, NM_CUSTOMDRAW, NM_DBLCLK, NMHDR, NMITEMACTIVATE,
    NMLVCUSTOMDRAW, NMLVKEYDOWN,
};
use windows_sys::Win32::UI::HiDpi::GetDpiForWindow;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{EnableWindow, SetFocus, VK_DELETE, VK_F2};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    BM_GETCHECK, BM_SETCHECK, BS_AUTOCHECKBOX, BS_DEFPUSHBUTTON, DWLP_MSGRESULT, EN_CHANGE,
    ES_AUTOHSCROLL, EndDialog, GWLP_USERDATA, GetClientRect, GetDlgItem, GetWindowRect,
    GetWindowTextLengthW, IDCANCEL, IDOK, IDYES, MB_DEFBUTTON2, MB_ICONWARNING, MB_OK, MB_YESNO,
    MINMAXINFO, MapDialogRect, MoveWindow, SendMessageW, SetWindowLongPtrW, SetWindowTextW,
    WM_COMMAND, WM_DESTROY, WM_GETMINMAXINFO, WM_INITDIALOG, WM_NOTIFY, WM_SIZE, WS_EX_CLIENTEDGE,
    WS_MAXIMIZEBOX, WS_TABSTOP, WS_THICKFRAME,
};

use crate::dialog::{
    Class, Look, Template, apply_theme, center_on_owner, common, list_custom_draw, state,
};
use crate::filedlg;
use crate::filter::Filter;
use crate::filters::{FilterDef, List, name_problem, one_line};
use crate::msgbox;
use crate::places::{self, Which};
use crate::store;
use crate::wide::{window_text, wz};

const STATIC: u16 = 0xffff;
const OK: u16 = IDOK as u16;
const CANCEL: u16 = IDCANCEL as u16;

fn item(dlg: HWND, id: u16) -> HWND {
    // SAFETY: looks up a child of this thread's dialog.
    unsafe { GetDlgItem(dlg, i32::from(id)) }
}

fn send(hwnd: HWND, msg: u32, w: usize, l: isize) -> isize {
    // SAFETY: a message to a window of this thread; any pointer in `l` is
    // the caller's and lives across the synchronous call.
    unsafe { SendMessageW(hwnd, msg, w, l) }
}

fn enable(dlg: HWND, id: u16, on: bool) {
    let h = item(dlg, id);
    if !h.is_null() {
        // SAFETY: a control of this thread's dialog.
        unsafe { EnableWindow(h, on.into()) };
    }
}

fn set_text(hwnd: HWND, text: &str) {
    // SAFETY: a NUL-terminated string for a window of this thread.
    unsafe { SetWindowTextW(hwnd, wz(text).as_ptr()) };
}

fn warn(owner: HWND, title: &str, text: &str) {
    msgbox::show(owner, text, title, MB_OK | MB_ICONWARNING);
}

// ---- The editor ----

const ID_NAME: u16 = 1000;
const ID_CASE: u16 = 1001;
const ID_PATH: u16 = 1003;
const ID_SEARCH: u16 = 1006;
const ID_NOTE: u16 = 1050;

struct Editor<'a> {
    was: FilterDef,
    /// The other filters, whose names it may not take.
    others: &'a [FilterDef],
    editing: Option<usize>,
    look: Cell<Look>,
    result: RefCell<Option<FilterDef>>,
}

/// Shows the filter editor titled `title` ("New Filter", "Edit Filter")
/// over `owner` with `def`; `list` the filters it goes among (`editing`:
/// its own place there). The filter as the user left it on OK.
pub fn edit(
    owner: HWND,
    title: &str,
    def: &FilterDef,
    list: &[FilterDef],
    editing: Option<usize>,
    look: Look,
) -> Option<FilterDef> {
    let state = Editor {
        was: def.clone(),
        others: list,
        editing,
        look: Cell::new(look),
        result: RefCell::new(None),
    };
    let unsupported = def.unsupported();
    let note_h: i16 = if unsupported.is_empty() { 0 } else { 26 };
    let check = WS_TABSTOP | BS_AUTOCHECKBOX as u32;
    let edit = WS_TABSTOP | ES_AUTOHSCROLL as u32;
    let (cx, cy) = (260, 92 + note_h);
    let mut t = Template::new(title, 0, cx, cy);
    t.control(Class::Static, "&Name:", STATIC, SS_LEFT, 0, (7, 9, 40, 8))
        .control(
            Class::Edit,
            "",
            ID_NAME,
            edit,
            WS_EX_CLIENTEDGE,
            (50, 7, 203, 14),
        )
        .control(
            Class::Button,
            "Match &case",
            ID_CASE,
            check,
            0,
            (7, 26, 246, 10),
        )
        .control(
            Class::Button,
            "Match &path",
            ID_PATH,
            check,
            0,
            (7, 39, 246, 10),
        )
        .control(
            Class::Static,
            "&Search:",
            STATIC,
            SS_LEFT,
            0,
            (7, 56, 40, 8),
        )
        .control(
            Class::Edit,
            "",
            ID_SEARCH,
            edit,
            WS_EX_CLIENTEDGE,
            (50, 54, 203, 14),
        );
    if note_h > 0 {
        let note = format!(
            "goz cannot apply this filter's {} yet: it is kept, but not offered in the menus.",
            unsupported.join(", ")
        );
        t.control(
            Class::Static,
            &note,
            ID_NOTE,
            SS_LEFT,
            0,
            (7, 74, 246, note_h - 4),
        );
    }
    t.control(
        Class::Button,
        "OK",
        OK,
        WS_TABSTOP | BS_DEFPUSHBUTTON as u32,
        0,
        (149, cy - 21, 50, 14),
    )
    .control(
        Class::Button,
        "Cancel",
        CANCEL,
        WS_TABSTOP,
        0,
        (203, cy - 21, 50, 14),
    );
    t.run(owner, Some(edit_proc), (&raw const state) as LPARAM);
    state.result.into_inner()
}

impl Editor<'_> {
    fn init(&self, dlg: HWND) {
        set_text(item(dlg, ID_NAME), &one_line(&self.was.name));
        set_text(item(dlg, ID_SEARCH), &one_line(&self.was.search));
        send(
            item(dlg, ID_CASE),
            BM_SETCHECK,
            usize::from(self.was.case),
            0,
        );
        send(
            item(dlg, ID_PATH),
            BM_SETCHECK,
            usize::from(self.was.path),
            0,
        );
        enable(dlg, OK, !self.was.name.trim().is_empty());
        apply_theme(dlg, self.look.get().dark);
        center_on_owner(dlg);
    }

    fn read(&self, dlg: HWND) -> FilterDef {
        let was = &self.was;
        // A text shown on one line comes back as it was unless edited.
        let keep = |id: u16, old: &str| {
            let now = window_text(item(dlg, id));
            if now == one_line(old) {
                old.to_owned()
            } else {
                now
            }
        };
        FilterDef {
            name: keep(ID_NAME, &was.name).trim().to_owned(),
            search: keep(ID_SEARCH, &was.search),
            case: send(item(dlg, ID_CASE), BM_GETCHECK, 0, 0) == 1,
            path: send(item(dlg, ID_PATH), BM_GETCHECK, 0, 0) == 1,
            other: was.other.clone(),
        }
    }

    fn ok(&self, dlg: HWND) {
        let def = self.read(dlg);
        if let Some(problem) = name_problem(self.others, &def.name, self.editing) {
            warn(dlg, "Filter", &problem);
            // SAFETY: focuses a control of this thread's dialog.
            unsafe { SetFocus(item(dlg, ID_NAME)) };
            return;
        }
        *self.result.borrow_mut() = Some(def);
        // SAFETY: ends this modal dialog.
        unsafe { EndDialog(dlg, 1) };
    }
}

unsafe extern "system" fn edit_proc(dlg: HWND, msg: u32, w: WPARAM, l: LPARAM) -> isize {
    if msg == WM_INITDIALOG {
        // SAFETY: `l` is the `Editor` that `edit` keeps alive until the
        // dialog has ended.
        unsafe { SetWindowLongPtrW(dlg, GWLP_USERDATA, l) };
    }
    // SAFETY: GWLP_USERDATA is the `Editor` from `edit`, or null before.
    let Some(s) = (unsafe { state::<Editor<'_>>(dlg) }) else {
        return 0;
    };
    if let Some(answer) = common(dlg, msg, w, l, &s.look) {
        return answer;
    }
    match msg {
        WM_INITDIALOG => {
            s.init(dlg);
            // The keyboard in the name, its text selected.
            1
        }
        WM_COMMAND => {
            let (id, code) = ((w & 0xffff) as u16, ((w >> 16) & 0xffff) as u32);
            match id {
                OK => s.ok(dlg),
                // SAFETY: ends this modal dialog.
                CANCEL => unsafe {
                    EndDialog(dlg, 0);
                },
                // A filter needs a name.
                ID_NAME if code == EN_CHANGE => {
                    // SAFETY: reads a control of this thread's dialog.
                    let named = unsafe { GetWindowTextLengthW(item(dlg, ID_NAME)) } > 0;
                    enable(dlg, OK, named);
                }
                _ => return 0,
            }
            1
        }
        _ => 0,
    }
}

// ---- Organize Filters ----

const ID_LIST: u16 = 1009;
const ID_NEW: u16 = 1010;
const ID_EDIT: u16 = 1011;
const ID_DELETE: u16 = 1012;
const ID_UP: u16 = 1013;
const ID_DOWN: u16 = 1014;
const ID_IMPORT: u16 = 1015;
const ID_EXPORT: u16 = 1016;
const ID_RESET: u16 = 1018;
const ID_ABOUT: u16 = 1019;

/// The buttons down the right, Everything's, and the top of each (dialog
/// units).
const BUTTONS: [(u16, &str, i32); 8] = [
    (ID_NEW, "&New...", 7),
    (ID_EDIT, "&Edit...", 31),
    (ID_UP, "Mo&ve Up", 49),
    (ID_DOWN, "&Move Down", 67),
    (ID_DELETE, "&Delete", 85),
    (ID_IMPORT, "&Import...", 109),
    (ID_EXPORT, "E&xport...", 127),
    (ID_RESET, "&Reset", 145),
];

/// The list's columns, Everything's, with their widths at 96 DPI.
const COLUMNS: [(&str, i32); 4] = [("Name", 96), ("Search", 256), ("Macro", 96), ("Key", 96)];

/// The file name an export suggests.
const EXPORT_NAME: &str = "Filters.csv";

struct Organize {
    list: RefCell<List>,
    look: Cell<Look>,
    result: RefCell<Option<List>>,
    min: Cell<(i32, i32)>,
}

/// Shows Organize Filters over `owner` with `list`; the filters as the
/// user left them on OK.
pub fn organize(owner: HWND, list: List, look: Look) -> Option<List> {
    let state = Organize {
        list: RefCell::new(list),
        look: Cell::new(look),
        result: RefCell::new(None),
        min: Cell::new((0, 0)),
    };
    let (cx, cy) = (420, 250);
    let style = WS_TABSTOP | LVS_REPORT | LVS_SHOWSELALWAYS | LVS_SINGLESEL | LVS_NOSORTHEADER;
    let mut t = Template::new("Organize Filters", WS_THICKFRAME | WS_MAXIMIZEBOX, cx, cy);
    t.control(
        Class::Named("SysListView32"),
        "",
        ID_LIST,
        style,
        WS_EX_CLIENTEDGE,
        (7, 7, cx - 74, cy - 50),
    );
    for (id, text, top) in BUTTONS {
        let style = if id == ID_EDIT {
            WS_TABSTOP | BS_DEFPUSHBUTTON as u32
        } else {
            WS_TABSTOP
        };
        t.control(
            Class::Button,
            text,
            id,
            style,
            0,
            (cx - 61, top as i16, 54, 14),
        );
    }
    t.control(
        Class::Static,
        "",
        ID_ABOUT,
        SS_LEFT,
        0,
        (7, cy - 40, cx - 74, 16),
    )
    .control(
        Class::Button,
        "OK",
        OK,
        WS_TABSTOP,
        0,
        (cx - 119, cy - 21, 54, 14),
    )
    .control(
        Class::Button,
        "Cancel",
        CANCEL,
        WS_TABSTOP,
        0,
        (cx - 61, cy - 21, 54, 14),
    );
    t.run(owner, Some(organize_proc), (&raw const state) as LPARAM);
    state.result.into_inner()
}

/// A list row: a built-in filter (`Filter::ALL`'s index) or the user's
/// (an index into `List::custom`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Row {
    Builtin(usize),
    Custom(usize),
}

fn row_of(list: &List, index: usize) -> Option<Row> {
    let n = Filter::ALL.len();
    if index < n {
        Some(Row::Builtin(index))
    } else {
        (index - n < list.custom.len()).then_some(Row::Custom(index - n))
    }
}

/// The text Organize Filters shows for a row: its name (a built-in's as
/// the menus call it), search, macro and key.
fn row_texts(list: &List, row: Row) -> [String; 4] {
    match row {
        Row::Builtin(i) => {
            let f = Filter::ALL[i];
            let def = &list.builtins[i];
            [
                f.title().to_owned(),
                f.search(),
                def.macro_name().to_owned(),
                key_text(def.key()),
            ]
        }
        Row::Custom(i) => {
            let def = &list.custom[i];
            [
                one_line(&def.name),
                one_line(&def.search),
                def.macro_name().to_owned(),
                key_text(def.key()),
            ]
        }
    }
}

/// Everything's Key column: a key code, 0 for none (shown as nothing).
fn key_text(key: &str) -> String {
    match key.trim() {
        "" | "0" => String::new(),
        k => k.to_owned(),
    }
}

impl Organize {
    fn list_view(dlg: HWND) -> HWND {
        item(dlg, ID_LIST)
    }

    fn init(&self, dlg: HWND) {
        let lv = Organize::list_view(dlg);
        let ex = LVS_EX_FULLROWSELECT | LVS_EX_DOUBLEBUFFER | LVS_EX_LABELTIP;
        send(lv, LVM_SETEXTENDEDLISTVIEWSTYLE, ex as usize, ex as isize);
        // SAFETY: reads this thread's dialog's DPI.
        let dpi = unsafe { GetDpiForWindow(dlg) }.max(96) as i32;
        for (i, (title, width)) in COLUMNS.into_iter().enumerate() {
            let mut text = wz(title);
            let col = LVCOLUMNW {
                mask: LVCF_TEXT | LVCF_WIDTH,
                cx: width * dpi / 96,
                pszText: text.as_mut_ptr(),
                // SAFETY: the rest of LVCOLUMNW is plain data.
                ..unsafe { core::mem::zeroed() }
            };
            send(lv, LVM_INSERTCOLUMNW, i, (&raw const col) as isize);
        }
        apply_theme(dlg, self.look.get().dark);
        center_on_owner(dlg);
        let mut wr = RECT::default();
        // SAFETY: reads this thread's dialog.
        unsafe { GetWindowRect(dlg, &mut wr) };
        self.min.set((wr.right - wr.left, wr.bottom - wr.top));
        places::restore(dlg, Which::FilterOrganize, lv, self.min.get());
        self.layout(dlg);
        // The first of the user's own, or the first.
        let first = if self.list.borrow().custom.is_empty() {
            0
        } else {
            Filter::ALL.len()
        };
        self.fill(dlg, Some(first));
    }

    /// Places the controls for the dialog's size: the list taking the room,
    /// the buttons at the right, the note under the list.
    fn layout(&self, dlg: HWND) {
        let px = |x: i32, y: i32| {
            let mut r = RECT {
                left: 0,
                top: 0,
                right: x,
                bottom: y,
            };
            // SAFETY: converts within this thread's dialog.
            unsafe { MapDialogRect(dlg, &mut r) };
            (r.right, r.bottom)
        };
        let mut rc = RECT::default();
        // SAFETY: reads this thread's dialog.
        unsafe { GetClientRect(dlg, &mut rc) };
        let (w, h) = (rc.right, rc.bottom);
        let (m, my) = px(7, 7);
        let (bw, bh) = px(54, 14);
        let (gap, _) = px(6, 0);
        let (_, note_h) = px(0, 16);
        let place = |id: u16, x: i32, y: i32, cw: i32, ch: i32| {
            let hwnd = item(dlg, id);
            if !hwnd.is_null() {
                // SAFETY: moves a control of this thread's dialog.
                unsafe { MoveWindow(hwnd, x, y, cw.max(0), ch.max(0), 1) };
            }
        };
        let bx = w - m - bw;
        let lw = bx - gap - m;
        let bottom = h - my - bh;
        place(ID_LIST, m, my, lw, bottom - my - note_h - my);
        place(ID_ABOUT, m, bottom - my - note_h + my / 2, lw, note_h);
        for (id, _, y) in BUTTONS {
            place(id, bx, px(0, y).1, bw, bh);
        }
        place(OK, bx - gap - bw, bottom, bw, bh);
        place(CANCEL, bx, bottom, bw, bh);
    }

    /// Shows the filters again, `select` (a row) selected.
    fn fill(&self, dlg: HWND, select: Option<usize>) {
        let lv = Organize::list_view(dlg);
        send(lv, LVM_DELETEALLITEMS, 0, 0);
        let list = self.list.borrow();
        let n = Filter::ALL.len() + list.custom.len();
        for index in 0..n {
            let Some(row) = row_of(&list, index) else {
                continue;
            };
            for (col, text) in row_texts(&list, row).iter().enumerate() {
                let mut t = wz(text);
                let item = LVITEMW {
                    mask: LVIF_TEXT,
                    iItem: index as i32,
                    iSubItem: col as i32,
                    pszText: t.as_mut_ptr(),
                    // SAFETY: the rest of LVITEMW is plain data.
                    ..unsafe { core::mem::zeroed() }
                };
                let msg = if col == 0 {
                    LVM_INSERTITEMW
                } else {
                    LVM_SETITEMTEXTW
                };
                let at = if col == 0 { 0 } else { index };
                send(lv, msg, at, (&raw const item) as isize);
            }
        }
        drop(list);
        if let Some(i) = select.filter(|&i| i < n) {
            let state = LVIS_SELECTED | LVIS_FOCUSED;
            let item = LVITEMW {
                state,
                stateMask: state,
                // SAFETY: the rest of LVITEMW is plain data.
                ..unsafe { core::mem::zeroed() }
            };
            send(lv, LVM_SETITEMSTATE, i, (&raw const item) as isize);
            send(lv, LVM_ENSUREVISIBLE, i, 0);
        }
        self.update(dlg);
    }

    fn selected(&self, dlg: HWND) -> Option<Row> {
        let i = send(
            Organize::list_view(dlg),
            LVM_GETNEXTITEM,
            usize::MAX,
            LVNI_SELECTED as isize,
        );
        usize::try_from(i)
            .ok()
            .and_then(|i| row_of(&self.list.borrow(), i))
    }

    /// Greys what the selection cannot have, and says why a row cannot be
    /// edited or searched with.
    fn update(&self, dlg: HWND) {
        let row = self.selected(dlg);
        let count = self.list.borrow().custom.len();
        let custom = match row {
            Some(Row::Custom(i)) => Some(i),
            _ => None,
        };
        enable(dlg, ID_EDIT, custom.is_some());
        enable(dlg, ID_DELETE, custom.is_some());
        enable(dlg, ID_UP, custom.is_some_and(|i| i > 0));
        enable(dlg, ID_DOWN, custom.is_some_and(|i| i + 1 < count));
        let about = match row {
            Some(Row::Builtin(_)) => {
                "Everything's own filters are goz-gui's: they are not edited.".to_owned()
            }
            Some(Row::Custom(i)) => {
                let list = self.list.borrow();
                let missing = list.custom[i].unsupported();
                if missing.is_empty() {
                    String::new()
                } else {
                    format!(
                        "goz cannot apply this filter's {} yet: it is kept, but not offered in \
                         the menus.",
                        missing.join(", ")
                    )
                }
            }
            None => String::new(),
        };
        set_text(item(dlg, ID_ABOUT), &about);
    }

    fn new_filter(&self, dlg: HWND) {
        let others = self.list.borrow().custom.clone();
        let def = FilterDef::new("", "", false, false);
        if let Some(def) = edit(dlg, "New Filter", &def, &others, None, self.look.get()) {
            let at = {
                let mut list = self.list.borrow_mut();
                list.custom.push(def);
                Filter::ALL.len() + list.custom.len() - 1
            };
            self.fill(dlg, Some(at));
        }
    }

    fn edit_filter(&self, dlg: HWND) {
        let Some(Row::Custom(i)) = self.selected(dlg) else {
            return;
        };
        let (def, others) = {
            let list = self.list.borrow();
            (list.custom[i].clone(), list.custom.clone())
        };
        if let Some(def) = edit(dlg, "Edit Filter", &def, &others, Some(i), self.look.get()) {
            self.list.borrow_mut().custom[i] = def;
            self.fill(dlg, Some(Filter::ALL.len() + i));
        }
    }

    fn delete(&self, dlg: HWND) {
        let Some(Row::Custom(i)) = self.selected(dlg) else {
            return;
        };
        let sure = msgbox::show(
            dlg,
            "Are you sure you want to delete this filter?",
            "Delete Filter",
            MB_YESNO | MB_ICONWARNING | MB_DEFBUTTON2,
        ) == IDYES;
        if !sure {
            return;
        }
        let left = {
            let mut list = self.list.borrow_mut();
            list.custom.remove(i);
            list.custom.len()
        };
        let next = if left == 0 {
            None
        } else {
            Some(Filter::ALL.len() + i.min(left - 1))
        };
        self.fill(dlg, next);
    }

    fn move_by(&self, dlg: HWND, up: bool) {
        let Some(Row::Custom(i)) = self.selected(dlg) else {
            return;
        };
        let to = {
            let mut list = self.list.borrow_mut();
            let to = if up {
                i.checked_sub(1)
            } else {
                (i + 1 < list.custom.len()).then_some(i + 1)
            };
            let Some(to) = to else { return };
            list.custom.swap(i, to);
            to
        };
        self.fill(dlg, Some(Filter::ALL.len() + to));
    }

    fn import(&self, dlg: HWND) {
        let Some(path) = filedlg::open(dlg, "Import Filters", filedlg::CSV, "csv") else {
            return;
        };
        let read = std::fs::read(&path)
            .map_err(|e| e.to_string())
            .and_then(|bytes| List::parse(&bytes));
        match read {
            Ok(other) => {
                if other.custom.is_empty() {
                    warn(
                        dlg,
                        "Import Filters",
                        &format!("{} holds no filters of the user's own.", path.display()),
                    );
                    return;
                }
                let at = {
                    let mut list = self.list.borrow_mut();
                    list.import(other);
                    Filter::ALL.len() + list.custom.len() - 1
                };
                self.fill(dlg, Some(at));
            }
            Err(why) => warn(
                dlg,
                "Import Filters",
                &format!("{} could not be imported: {why}.", path.display()),
            ),
        }
    }

    fn export(&self, dlg: HWND) {
        let text = self.list.borrow().to_csv();
        let Some((path, _)) = filedlg::save(
            dlg,
            "Export Filters",
            EXPORT_NAME,
            filedlg::CSV,
            "csv",
            None,
        ) else {
            return;
        };
        if store::location::is_data_file(&path) {
            warn(
                dlg,
                "Export Filters",
                &format!(
                    "{} is goz-gui's own file. Export the filters to another file or folder.",
                    path.display()
                ),
            );
            return;
        }
        if let Err(e) = store::file::write(&path, text.as_bytes(), None) {
            warn(
                dlg,
                "Export Filters",
                &format!(
                    "The filters could not be exported to {}: {e}",
                    path.display()
                ),
            );
        }
    }

    /// Everything's Reset: the default filters, the user's own gone (OK
    /// keeps it, Cancel does not).
    fn reset(&self, dlg: HWND) {
        let sure = msgbox::show(
            dlg,
            "Reset the filters to their defaults? Your own filters will be removed.",
            "Reset Filters",
            MB_YESNO | MB_ICONWARNING | MB_DEFBUTTON2,
        ) == IDYES;
        if sure {
            *self.list.borrow_mut() = List::default();
            self.fill(dlg, Some(0));
        }
    }

    fn command(&self, dlg: HWND, id: u16) -> bool {
        match id {
            OK => {
                *self.result.borrow_mut() = Some(self.list.borrow().clone());
                // SAFETY: ends this modal dialog.
                unsafe { EndDialog(dlg, 1) };
            }
            // SAFETY: ends this modal dialog.
            CANCEL => unsafe {
                EndDialog(dlg, 0);
            },
            ID_NEW => self.new_filter(dlg),
            ID_EDIT => self.edit_filter(dlg),
            ID_DELETE => self.delete(dlg),
            ID_UP => self.move_by(dlg, true),
            ID_DOWN => self.move_by(dlg, false),
            ID_IMPORT => self.import(dlg),
            ID_EXPORT => self.export(dlg),
            ID_RESET => self.reset(dlg),
            _ => return false,
        }
        true
    }

    /// Answers a notification; `Some` is DWLP_MSGRESULT's value.
    ///
    /// # Safety
    /// `l` is a WM_NOTIFY's lParam.
    unsafe fn notify(&self, dlg: HWND, l: LPARAM) -> Option<isize> {
        // SAFETY: as the caller promises; each code's struct is read as
        // the list sends it.
        unsafe {
            let hdr = &*(l as *const NMHDR);
            if hdr.idFrom != usize::from(ID_LIST) {
                return None;
            }
            match hdr.code {
                NM_CUSTOMDRAW => {
                    let cd = &*(l as *const NMLVCUSTOMDRAW);
                    return Some(list_custom_draw(hdr.hwndFrom, cd, self.look.get().dark));
                }
                LVN_ITEMCHANGED => self.update(dlg),
                NM_DBLCLK => {
                    if (*(l as *const NMITEMACTIVATE)).iItem >= 0 {
                        self.edit_filter(dlg);
                    }
                }
                LVN_KEYDOWN => match (*(l as *const NMLVKEYDOWN)).wVKey {
                    VK_DELETE => self.delete(dlg),
                    VK_F2 => self.edit_filter(dlg),
                    _ => {}
                },
                _ => {}
            }
            None
        }
    }
}

unsafe extern "system" fn organize_proc(dlg: HWND, msg: u32, w: WPARAM, l: LPARAM) -> isize {
    if msg == WM_INITDIALOG {
        // SAFETY: `l` is the `Organize` that `organize` keeps alive until
        // the dialog has ended.
        unsafe { SetWindowLongPtrW(dlg, GWLP_USERDATA, l) };
    }
    // SAFETY: GWLP_USERDATA is the `Organize` from `organize`, or null.
    let Some(s) = (unsafe { state::<Organize>(dlg) }) else {
        return 0;
    };
    if let Some(answer) = common(dlg, msg, w, l, &s.look) {
        return answer;
    }
    match msg {
        WM_INITDIALOG => {
            s.init(dlg);
            // SAFETY: the keyboard in the list, a control of this dialog.
            unsafe { SetFocus(Organize::list_view(dlg)) };
            0
        }
        WM_SIZE => {
            s.layout(dlg);
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
            // SAFETY: WM_NOTIFY's lParam is an NMHDR from a control.
            if let Some(answer) = unsafe { s.notify(dlg, l) } {
                // SAFETY: the answer of this dialog's WM_NOTIFY.
                unsafe { SetWindowLongPtrW(dlg, DWLP_MSGRESULT as i32, answer) };
            }
            1
        }
        WM_COMMAND => isize::from(s.command(dlg, (w & 0xffff) as u16)),
        WM_DESTROY => {
            places::keep(dlg, Which::FilterOrganize, Organize::list_view(dlg));
            0
        }
        _ => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rows_are_the_built_in_filters_then_the_users() {
        let mut list = List::default();
        list.custom
            .push(FilterDef::new("Mine", "*.rs", false, false));
        assert_eq!(row_of(&list, 0), Some(Row::Builtin(0)));
        assert_eq!(row_of(&list, 7), Some(Row::Builtin(7)));
        assert_eq!(row_of(&list, 8), Some(Row::Custom(0)));
        assert_eq!(row_of(&list, 9), None);
        let audio = row_texts(&list, Row::Builtin(1));
        assert_eq!(audio[0], "Audio");
        assert!(audio[1].starts_with("ext:aac;"));
        assert_eq!(audio[2], "audio");
        assert_eq!(audio[3], "");
        assert_eq!(row_texts(&list, Row::Custom(0))[..2], ["Mine", "*.rs"]);
        assert_eq!(key_text(" 0 "), "");
        assert_eq!(key_text("1092"), "1092");
    }
}
