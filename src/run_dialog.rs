//! Tools > Run History... (Ctrl+J): every file and folder opened from
//! goz-gui, as a list of `Name`, `Path`, `Run Count` and `Date Run`, the
//! last run first, sortable by a click on a column's header, with a box
//! above it that filters the list as it is typed (any part of the path, any
//! case). Everything's dialog, with its columns and buttons:
//!
//! - **Open** (Enter, a double-click) opens the selected files, and **Open
//!   Path** their folders with them selected; each open counts as a run, as
//!   one from the results does. The dialog stays, showing the new counts.
//! - **Remove** (the Delete key too) forgets the selected files; **Remove
//!   All** forgets every one, after asking.
//! - **Import...** merges a `Run History.csv` of Everything's or goz-gui's;
//!   **Export...** writes all of it, or the selection only.
//!
//! Each change is made at once; the window writes the file. Close (Esc)
//! closes. Up, Down, Page Up and Page Down in the filter box move in the
//! list, so the keyboard can stay in the box.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;

use windows_sys::Win32::Foundation::{HWND, LPARAM, RECT, WPARAM};
use windows_sys::Win32::Graphics::Gdi::InvalidateRect;
use windows_sys::Win32::UI::Controls::{
    HDF_SORTDOWN, HDF_SORTUP, HDI_FORMAT, HDITEMW, HDM_GETITEMW, HDM_SETITEMW, LVCF_FMT, LVCF_TEXT,
    LVCF_WIDTH, LVCFMT_LEFT, LVCFMT_RIGHT, LVCOLUMNW, LVIF_TEXT, LVIS_FOCUSED, LVIS_SELECTED,
    LVITEMW, LVM_ENSUREVISIBLE, LVM_GETHEADER, LVM_GETNEXTITEM, LVM_GETSELECTEDCOUNT,
    LVM_INSERTCOLUMNW, LVM_SETEXTENDEDLISTVIEWSTYLE, LVM_SETITEMCOUNT, LVM_SETITEMSTATE,
    LVM_SETSELECTIONMARK, LVN_COLUMNCLICK, LVN_GETDISPINFOW, LVN_ITEMCHANGED, LVN_KEYDOWN,
    LVN_ODFINDITEMW, LVN_ODSTATECHANGED, LVNI_FOCUSED, LVNI_SELECTED, LVS_EX_DOUBLEBUFFER,
    LVS_EX_FULLROWSELECT, LVS_EX_HEADERDRAGDROP, LVS_EX_LABELTIP, LVS_OWNERDATA, LVS_REPORT,
    LVS_SHOWSELALWAYS, NM_CUSTOMDRAW, NM_DBLCLK, NMHDR, NMITEMACTIVATE, NMLISTVIEW, NMLVCUSTOMDRAW,
    NMLVDISPINFOW, NMLVKEYDOWN,
};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    EnableWindow, GetFocus, GetKeyState, IsWindowEnabled, VK_CONTROL, VK_DELETE,
};
use windows_sys::Win32::UI::Shell::SetWindowSubclass;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    BS_DEFPUSHBUTTON, DM_SETDEFID, DWLP_MSGRESULT, EN_CHANGE, ES_AUTOHSCROLL, EndDialog,
    GWLP_USERDATA, GetClientRect, GetDlgItem, GetWindowRect, IDCANCEL, IDOK, IDYES, MB_DEFBUTTON2,
    MB_ICONWARNING, MB_OK, MB_YESNO, MINMAXINFO, MapDialogRect, MoveWindow, SendMessageW,
    SetWindowLongPtrW, WM_COMMAND, WM_DESTROY, WM_GETMINMAXINFO, WM_INITDIALOG, WM_NEXTDLGCTL,
    WM_NOTIFY, WM_SIZE, WS_EX_CLIENTEDGE, WS_MAXIMIZEBOX, WS_TABSTOP, WS_THICKFRAME,
};

use crate::dialog::{
    Class, Look, Template, apply_theme, center_on_owner, common, list_custom_draw, state,
};
use crate::filedlg::{self, Check};
use crate::format;
use crate::history_dialogs::filter_proc;
use crate::msgbox;
use crate::places::{self, Which};
use crate::run_history::{Ran, fold, split};
use crate::run_history_file::{self, Table};
use crate::store;
use crate::wide::{window_text, wz};

/// The dialog's title, as Everything's (without its name).
const TITLE: &str = "Run History";
/// What Remove All asks, in Everything's words.
const REMOVE_ALL_QUESTION: &str = "Are you sure you want to delete all run history?";

/// What the dialog asks of the window that owns the run history.
pub trait Host {
    /// The run history, the last run first.
    fn runs(&self) -> Vec<Ran>;
    /// Opens `files`, or (`reveal`) their folders with them selected, as
    /// the results' Open and Open Path do (asking over `owner` first when
    /// they are many); each counts as a run as such an open does.
    fn open_runs(&self, owner: HWND, files: &[String], reveal: bool);
    /// Forgets these files.
    fn remove_runs(&self, files: &[String]);
    /// Forgets every file.
    fn remove_all_runs(&self);
    /// Merges runs read from a file into the history.
    fn import_runs(&self, entries: Vec<Ran>);
}

/// The list's columns.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Column {
    Name,
    Path,
    Count,
    Date,
}

/// Each column: its title, its width at 96 DPI (Everything's), and whether
/// it is right-aligned.
const COLUMNS: [(Column, &str, i32, bool); 4] = [
    (Column::Name, "Name", 176, false),
    (Column::Path, "Path", 176, false),
    (Column::Count, "Run Count", 96, true),
    (Column::Date, "Date Run", 128, false),
];

/// How the list is sorted.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Sort {
    pub column: Column,
    pub ascending: bool,
}

impl Default for Sort {
    /// The last run first, as Everything opens it (`run_history_sort=3`,
    /// `run_history_sort_ascending=0`).
    fn default() -> Sort {
        Sort {
            column: Column::Date,
            ascending: false,
        }
    }
}

impl Sort {
    /// The sort after a click on `column`'s header: the other direction
    /// when it is the sort already, else A to Z for the name and the path,
    /// most first for the count and newest first for the date.
    pub fn clicked(self, column: Column) -> Sort {
        if self.column == column {
            Sort {
                ascending: !self.ascending,
                ..self
            }
        } else {
            Sort {
                column,
                ascending: matches!(column, Column::Name | Column::Path),
            }
        }
    }
}

/// The rows the list shows: the indices into `all` of the files whose path
/// contains `filter` (ignoring case), in `sort`'s order; ties the last run
/// first.
pub fn view(all: &[Ran], filter: &str, sort: Sort) -> Vec<usize> {
    let needle = filter.to_lowercase();
    let keys: Vec<String> = all.iter().map(|e| fold(&e.filename)).collect();
    let mut rows: Vec<usize> = (0..all.len())
        .filter(|&i| keys[i].contains(&needle))
        .collect();
    rows.sort_by(|&x, &y| {
        let (a, b) = (&all[x], &all[y]);
        let (an, ap) = split(&keys[x]);
        let (bn, bp) = split(&keys[y]);
        let order = match sort.column {
            Column::Name => an.cmp(bn).then_with(|| ap.cmp(bp)),
            Column::Path => ap.cmp(bp).then_with(|| an.cmp(bn)),
            Column::Count => a.count.cmp(&b.count),
            Column::Date => a.last.cmp(&b.last),
        };
        let order = if sort.ascending {
            order
        } else {
            order.reverse()
        };
        order
            .then_with(|| b.last.cmp(&a.last))
            .then_with(|| x.cmp(&y))
    });
    rows
}

const ID_FILTER: u16 = 100;
const ID_LIST: u16 = 101;
const ID_OPEN: u16 = 102;
const ID_OPEN_PATH: u16 = 103;
const ID_REMOVE: u16 = 104;
const ID_IMPORT: u16 = 105;
const ID_EXPORT: u16 = 106;
const ID_REMOVE_ALL: u16 = 107;
const STATIC: u16 = 0xffff;
/// The name Export suggests: not the live file's own, which the dialog
/// would offer to replace when it opens in the data folder.
const EXPORT_NAME: &str = "Run History export.csv";
const OK: u16 = IDOK as u16;
const CANCEL: u16 = IDCANCEL as u16;

/// Dialog units: the dialog's size, the margin, a button's size.
const SIZE: (i16, i16) = (410, 262);
const MARGIN: i32 = 7;
const BUTTON: (i32, i32) = (50, 14);
/// Where the list starts, under the filter box.
const LIST_TOP: i32 = 24;
/// The width of the "Search:" label and its gap.
const LABEL: i32 = 29;
/// The buttons at the right, top edges in dialog units, grouped as
/// Everything's: the selection's, then the whole history's.
const BUTTONS: [(u16, &str, i32); 6] = [
    (ID_OPEN, "&Open", 24),
    (ID_OPEN_PATH, "Open &Path", 42),
    (ID_REMOVE, "&Remove", 60),
    (ID_IMPORT, "&Import...", 84),
    (ID_EXPORT, "E&xport...", 102),
    (ID_REMOVE_ALL, "Remove &All", 120),
];

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

fn set_state(list: HWND, row: usize, state: u32, mask: u32) {
    let it = LVITEMW {
        state,
        stateMask: mask,
        // SAFETY: the rest of LVITEMW is plain data.
        ..unsafe { core::mem::zeroed() }
    };
    send(list, LVM_SETITEMSTATE, row, (&raw const it) as isize);
}

/// Says what went wrong, over the dialog.
fn warn(dlg: HWND, text: &str) {
    msgbox::show(dlg, text, TITLE, MB_OK | MB_ICONWARNING);
}

/// The dialog's state, in its GWLP_USERDATA while it is open.
struct Dlg<'a> {
    host: &'a dyn Host,
    look: Cell<Look>,
    /// The history as last read from the host, the last run first.
    all: RefCell<Vec<Ran>>,
    /// The rows shown: indices into `all`.
    shown: RefCell<Vec<usize>>,
    sort: Cell<Sort>,
    /// The dialog's size as it opened, the smallest it can be made.
    min: Cell<(i32, i32)>,
}

/// Shows the dialog, owned by `owner`, until it closes.
pub fn run(owner: HWND, host: &dyn Host, look: Look) {
    let state = Dlg {
        host,
        look: Cell::new(look),
        all: RefCell::new(Vec::new()),
        shown: RefCell::new(Vec::new()),
        sort: Cell::new(Sort::default()),
        min: Cell::new((0, 0)),
    };
    let (cx, cy) = SIZE;
    let mut t = Template::new(TITLE, WS_THICKFRAME | WS_MAXIMIZEBOX, cx, cy);
    // The places are set by `layout`; these are the first.
    t.control(Class::Static, "&Search:", STATIC, 0, 0, (7, 9, 28, 8))
        .control(
            Class::Edit,
            "",
            ID_FILTER,
            WS_TABSTOP | ES_AUTOHSCROLL as u32,
            WS_EX_CLIENTEDGE,
            (36, 7, cx - 43, 12),
        )
        .control(
            Class::Named("SysListView32"),
            "",
            ID_LIST,
            WS_TABSTOP | LVS_REPORT | LVS_OWNERDATA | LVS_SHOWSELALWAYS,
            WS_EX_CLIENTEDGE,
            (7, 24, cx - 70, cy - 31),
        );
    for (id, text, _) in BUTTONS {
        let style = if id == ID_OPEN {
            WS_TABSTOP | BS_DEFPUSHBUTTON as u32
        } else {
            WS_TABSTOP
        };
        t.control(Class::Button, text, id, style, 0, (0, 0, 50, 14));
    }
    t.control(
        Class::Button,
        "Close",
        CANCEL,
        WS_TABSTOP,
        0,
        (0, 0, 50, 14),
    );
    t.run(owner, Some(dialog_proc), (&raw const state) as LPARAM);
}

impl Dlg<'_> {
    fn list(dlg: HWND) -> HWND {
        item(dlg, ID_LIST)
    }

    fn init(&self, dlg: HWND) {
        let list = Dlg::list(dlg);
        let ex =
            LVS_EX_FULLROWSELECT | LVS_EX_DOUBLEBUFFER | LVS_EX_LABELTIP | LVS_EX_HEADERDRAGDROP;
        send(list, LVM_SETEXTENDEDLISTVIEWSTYLE, ex as usize, ex as isize);
        let dpi = crate::custom::window_dpi(dlg) as i32;
        for (i, &(_, title, width, right)) in COLUMNS.iter().enumerate() {
            let mut text = wz(title);
            let col = LVCOLUMNW {
                mask: LVCF_TEXT | LVCF_WIDTH | LVCF_FMT,
                fmt: if right { LVCFMT_RIGHT } else { LVCFMT_LEFT },
                cx: width * dpi / 96,
                pszText: text.as_mut_ptr(),
                // SAFETY: the rest of LVCOLUMNW is plain data.
                ..unsafe { core::mem::zeroed() }
            };
            send(list, LVM_INSERTCOLUMNW, i, (&raw const col) as isize);
        }
        // SAFETY: subclasses this thread's filter box; the list outlives it
        // (both go with the dialog), and the subclass removes itself.
        unsafe {
            SetWindowSubclass(item(dlg, ID_FILTER), Some(filter_proc), 1, list as usize);
        }
        apply_theme(dlg, self.look.get().dark);
        center_on_owner(dlg);
        let mut wr = RECT::default();
        // SAFETY: reads this thread's dialog.
        unsafe { GetWindowRect(dlg, &mut wr) };
        self.min.set((wr.right - wr.left, wr.bottom - wr.top));
        places::restore(dlg, Which::RunHistory, list, self.min.get());
        self.layout(dlg);
        send(dlg, DM_SETDEFID, usize::from(ID_OPEN), 0);
        self.show_arrow(dlg);
        self.refill(dlg, None, Some(0));
    }

    /// Places the controls for the dialog's size: the filter box across the
    /// top, the list taking the room, the buttons at the right with Close
    /// at the bottom.
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
        let (mx, my) = px(MARGIN, MARGIN);
        let (bw, bh) = px(BUTTON.0, BUTTON.1);
        let (label, top) = px(LABEL, LIST_TOP);
        let (_, edit_h) = px(0, 12);
        let (column_gap, _) = px(6, 0);
        let place = |id: u16, x: i32, y: i32, cw: i32, ch: i32| {
            let hwnd = item(dlg, id);
            if !hwnd.is_null() {
                // SAFETY: moves a control of this thread's dialog.
                unsafe { MoveWindow(hwnd, x, y, cw.max(0), ch.max(0), 1) };
            }
        };
        place(ID_FILTER, mx + label, my, w - 2 * mx - label, edit_h);
        let bx = w - mx - bw;
        place(ID_LIST, mx, top, bx - column_gap - mx, h - my - top);
        for (id, _, y) in BUTTONS {
            place(id, bx, px(0, y).1, bw, bh);
        }
        place(CANCEL, bx, h - my - bh, bw, bh);
    }

    /// Reads the history from the host again and shows it, with the files
    /// `keep` selected if they are shown, else row `at`.
    fn refill(&self, dlg: HWND, keep: Option<&[String]>, at: Option<usize>) {
        *self.all.borrow_mut() = self.host.runs();
        self.reshow(dlg, keep, at);
    }

    /// Shows the history through the filter, in the sort, with the files
    /// `keep` (folded paths) selected where they are shown, else row `at`
    /// (kept in range).
    fn reshow(&self, dlg: HWND, keep: Option<&[String]>, at: Option<usize>) {
        let filter = window_text(item(dlg, ID_FILTER));
        let (rows, select) = {
            let all = self.all.borrow();
            let rows = view(&all, &filter, self.sort.get());
            let mut kept: Vec<usize> = Vec::new();
            if let Some(keep) = keep.filter(|k| !k.is_empty()) {
                let wanted: HashMap<&str, usize> = keep
                    .iter()
                    .enumerate()
                    .map(|(k, s)| (s.as_str(), k))
                    .collect();
                let mut found: Vec<(usize, usize)> = rows
                    .iter()
                    .enumerate()
                    .filter_map(|(r, &i)| {
                        wanted.get(fold(&all[i].filename).as_str()).map(|&k| (k, r))
                    })
                    .collect();
                // The first of `keep` (the focused file) first.
                found.sort_unstable();
                kept = found.into_iter().map(|(_, r)| r).collect();
            }
            let select = if kept.is_empty() {
                at.map(|a| a.min(rows.len().saturating_sub(1)))
                    .filter(|&i| i < rows.len())
                    .into_iter()
                    .collect()
            } else {
                kept
            };
            (rows, select)
        };
        let n = rows.len();
        *self.shown.borrow_mut() = rows;
        // The list's notifications come back into this dialog: no borrow
        // is held from here on.
        let list = Dlg::list(dlg);
        send(list, LVM_SETITEMCOUNT, n, 0);
        set_state(list, usize::MAX, 0, LVIS_SELECTED | LVIS_FOCUSED);
        for (k, &i) in select.iter().enumerate() {
            let state = if k == 0 {
                LVIS_SELECTED | LVIS_FOCUSED
            } else {
                LVIS_SELECTED
            };
            set_state(list, i, state, state);
        }
        if let Some(&first) = select.first() {
            send(list, LVM_SETSELECTIONMARK, 0, first as isize);
            send(list, LVM_ENSUREVISIBLE, first, 0);
        }
        // SAFETY: repaints this thread's list.
        unsafe { InvalidateRect(list, core::ptr::null(), 1) };
        self.update_buttons(dlg);
    }

    /// The selected rows' indices into `all`, top to bottom as shown.
    fn selected(&self, dlg: HWND) -> Vec<usize> {
        let list = Dlg::list(dlg);
        let mut rows = Vec::new();
        let mut at = usize::MAX;
        loop {
            let next = send(list, LVM_GETNEXTITEM, at, LVNI_SELECTED as isize);
            if next < 0 {
                break;
            }
            rows.push(next as usize);
            at = next as usize;
        }
        let shown = self.shown.borrow();
        rows.into_iter()
            .filter_map(|r| shown.get(r).copied())
            .collect()
    }

    /// The selected files' paths, top to bottom as shown.
    fn selected_files(&self, dlg: HWND) -> Vec<String> {
        let rows = self.selected(dlg);
        let all = self.all.borrow();
        rows.iter().map(|&i| all[i].filename.clone()).collect()
    }

    /// The selected files' folded paths, the focused one first: what to
    /// select again after a refill.
    fn selection_keys(&self, dlg: HWND) -> Vec<String> {
        let focused = send(
            Dlg::list(dlg),
            LVM_GETNEXTITEM,
            usize::MAX,
            (LVNI_FOCUSED | LVNI_SELECTED) as isize,
        );
        let focused = usize::try_from(focused)
            .ok()
            .and_then(|r| self.shown.borrow().get(r).copied());
        let mut rows = self.selected(dlg);
        if let Some(f) = focused {
            rows.retain(|&i| i != f);
            rows.insert(0, f);
        }
        let all = self.all.borrow();
        rows.iter().map(|&i| fold(&all[i].filename)).collect()
    }

    /// Greys what the selection does not allow.
    fn update_buttons(&self, dlg: HWND) {
        let selected = send(Dlg::list(dlg), LVM_GETSELECTEDCOUNT, 0, 0) > 0;
        let any = !self.all.borrow().is_empty();
        for id in [ID_OPEN, ID_OPEN_PATH, ID_REMOVE] {
            enable(dlg, id, selected);
        }
        enable(dlg, ID_REMOVE_ALL, any);
        enable(dlg, ID_EXPORT, any);
        // A button greyed while it had the keyboard hands it to the list.
        // SAFETY: reads this thread's focus and one of its windows.
        let focus = unsafe { GetFocus() };
        if !focus.is_null() && unsafe { IsWindowEnabled(focus) } == 0 {
            send(dlg, WM_NEXTDLGCTL, Dlg::list(dlg) as usize, 1);
        }
    }

    /// The header shows the sort as an arrow on the sorted column.
    fn show_arrow(&self, dlg: HWND) {
        let header = send(Dlg::list(dlg), LVM_GETHEADER, 0, 0) as HWND;
        let sort = self.sort.get();
        for (i, &(column, ..)) in COLUMNS.iter().enumerate() {
            let mut hd = HDITEMW {
                mask: HDI_FORMAT,
                // SAFETY: the rest of HDITEMW is plain data.
                ..unsafe { core::mem::zeroed() }
            };
            send(header, HDM_GETITEMW, i, (&raw mut hd) as isize);
            hd.fmt &= !(HDF_SORTUP | HDF_SORTDOWN);
            if column == sort.column {
                hd.fmt |= if sort.ascending {
                    HDF_SORTUP
                } else {
                    HDF_SORTDOWN
                };
            }
            send(header, HDM_SETITEMW, i, (&raw const hd) as isize);
        }
    }

    fn sort_by(&self, dlg: HWND, column: Column) {
        let keep = self.selection_keys(dlg);
        self.sort.set(self.sort.get().clicked(column));
        self.show_arrow(dlg);
        self.reshow(dlg, Some(&keep), Some(0));
    }

    /// The text of a cell, for the list (an owner-data list asks for it).
    fn disp_info(&self, info: &mut NMLVDISPINFOW) {
        if info.item.mask & LVIF_TEXT == 0
            || info.item.pszText.is_null()
            || info.item.cchTextMax <= 0
        {
            return;
        }
        let mut text: Vec<u16> = Vec::new();
        {
            let shown = self.shown.borrow();
            let all = self.all.borrow();
            let entry = usize::try_from(info.item.iItem)
                .ok()
                .and_then(|row| shown.get(row))
                .and_then(|&i| all.get(i));
            let column = usize::try_from(info.item.iSubItem)
                .ok()
                .and_then(|c| COLUMNS.get(c))
                .map(|c| c.0);
            if let (Some(e), Some(column)) = (entry, column) {
                let (name, folder) = split(&e.filename);
                match column {
                    Column::Name => text.extend(name.encode_utf16()),
                    Column::Path => text.extend(folder.encode_utf16()),
                    Column::Count => text.extend(e.count.to_string().encode_utf16()),
                    Column::Date if e.last > 0 => format::date(e.last as i64, &mut text),
                    Column::Date => {}
                }
            }
        }
        let n = text.len().min(info.item.cchTextMax as usize - 1);
        // SAFETY: the list gave a buffer of `cchTextMax` code units.
        unsafe {
            core::ptr::copy_nonoverlapping(text.as_ptr(), info.item.pszText, n);
            *info.item.pszText.add(n) = 0;
        }
    }

    /// Open, or Open Path (`reveal`): the selected files; the list then
    /// shows their new counts, with them still selected.
    fn open(&self, dlg: HWND, reveal: bool) {
        let files = self.selected_files(dlg);
        if files.is_empty() {
            return;
        }
        let keep = self.selection_keys(dlg);
        self.host.open_runs(dlg, &files, reveal);
        self.refill(dlg, Some(&keep), Some(0));
    }

    fn remove(&self, dlg: HWND) {
        let list = Dlg::list(dlg);
        let first = send(list, LVM_GETNEXTITEM, usize::MAX, LVNI_SELECTED as isize);
        let files = self.selected_files(dlg);
        if files.is_empty() {
            return;
        }
        self.host.remove_runs(&files);
        self.refill(dlg, None, usize::try_from(first).ok());
    }

    fn remove_all(&self, dlg: HWND) {
        if self.all.borrow().is_empty() {
            return;
        }
        let answer = msgbox::show(
            dlg,
            REMOVE_ALL_QUESTION,
            TITLE,
            MB_YESNO | MB_ICONWARNING | MB_DEFBUTTON2,
        );
        if answer == IDYES {
            self.host.remove_all_runs();
            self.refill(dlg, None, None);
        }
    }

    fn import(&self, dlg: HWND) {
        let Some(path) = filedlg::open(dlg, "Import Run History", filedlg::CSV, "csv") else {
            return;
        };
        let read = std::fs::read(&path)
            .map_err(|e| e.to_string())
            .and_then(|bytes| run_history_file::parse_import(&bytes));
        match read {
            Ok(table) => {
                let keep = self.selection_keys(dlg);
                self.host.import_runs(table.entries);
                self.refill(dlg, Some(&keep), Some(0));
            }
            Err(why) => warn(
                dlg,
                &format!("{} could not be imported: {why}.", path.display()),
            ),
        }
    }

    fn export(&self, dlg: HWND) {
        let mut selected = self.selected(dlg);
        let check = Check {
            label: "Export selection only",
            checked: false,
            enabled: !selected.is_empty(),
        };
        let Some((path, only)) = filedlg::save(
            dlg,
            "Export Run History",
            EXPORT_NAME,
            filedlg::CSV,
            "csv",
            Some(check),
        ) else {
            return;
        };
        if store::location::is_data_file(&path) {
            // The live file (or its backup) is not written behind the
            // merge's back.
            warn(
                dlg,
                &format!(
                    "{} is goz-gui's own file. Export the run history to another file or folder.",
                    path.display()
                ),
            );
            return;
        }
        let text = {
            let all = self.all.borrow();
            // The history's order (the last run first), as the file keeps it.
            let entries: Vec<Ran> = if only {
                selected.sort_unstable();
                selected.iter().map(|&i| all[i].clone()).collect()
            } else {
                all.clone()
            };
            run_history_file::to_csv(&entries, &Table::default())
        };
        if let Err(e) = store::file::write(&path, text.as_bytes(), None) {
            warn(
                dlg,
                &format!(
                    "The run history could not be exported to {}: {e}",
                    path.display()
                ),
            );
        }
    }

    fn command(&self, dlg: HWND, id: u16, code: u32) {
        match id {
            // Enter is Open, the default button.
            ID_OPEN | OK => self.open(dlg, false),
            ID_OPEN_PATH => self.open(dlg, true),
            ID_REMOVE => self.remove(dlg),
            ID_REMOVE_ALL => self.remove_all(dlg),
            ID_IMPORT => self.import(dlg),
            ID_EXPORT => self.export(dlg),
            ID_FILTER if code == EN_CHANGE => {
                let keep = self.selection_keys(dlg);
                self.reshow(dlg, Some(&keep[..keep.len().min(1)]), Some(0));
            }
            // SAFETY: ends this modal dialog.
            CANCEL => unsafe {
                EndDialog(dlg, 0);
            },
            _ => {}
        }
    }

    /// A key pressed in the list: Delete removes, Ctrl+A selects every row.
    fn key(&self, dlg: HWND, vk: u16) {
        // SAFETY: reads this thread's keyboard state.
        let ctrl = unsafe { GetKeyState(i32::from(VK_CONTROL)) } < 0;
        match (vk, ctrl) {
            (VK_DELETE, false) => self.remove(dlg),
            (0x41, true) => set_state(Dlg::list(dlg), usize::MAX, LVIS_SELECTED, LVIS_SELECTED),
            _ => {}
        }
    }

    /// Answers the list's notifications; `Some` is DWLP_MSGRESULT's value.
    ///
    /// # Safety
    /// `l` is a WM_NOTIFY's lParam from the list.
    unsafe fn notify(&self, dlg: HWND, l: LPARAM) -> Option<isize> {
        // SAFETY: as the caller promises; each code's struct is read as
        // the list sends it.
        unsafe {
            let hdr = &*(l as *const NMHDR);
            match hdr.code {
                NM_CUSTOMDRAW => {
                    let cd = &*(l as *const NMLVCUSTOMDRAW);
                    return Some(list_custom_draw(hdr.hwndFrom, cd, self.look.get().dark));
                }
                LVN_GETDISPINFOW => self.disp_info(&mut *(l as *mut NMLVDISPINFOW)),
                // No type-ahead: the filter box is for finding.
                LVN_ODFINDITEMW => return Some(-1),
                LVN_ITEMCHANGED | LVN_ODSTATECHANGED => self.update_buttons(dlg),
                LVN_COLUMNCLICK => {
                    let nm = &*(l as *const NMLISTVIEW);
                    if let Some(&(column, ..)) = usize::try_from(nm.iSubItem)
                        .ok()
                        .and_then(|c| COLUMNS.get(c))
                    {
                        self.sort_by(dlg, column);
                    }
                }
                NM_DBLCLK => {
                    if (*(l as *const NMITEMACTIVATE)).iItem >= 0 {
                        self.open(dlg, false);
                    }
                }
                LVN_KEYDOWN => self.key(dlg, (*(l as *const NMLVKEYDOWN)).wVKey),
                _ => {}
            }
            None
        }
    }
}

unsafe extern "system" fn dialog_proc(dlg: HWND, msg: u32, w: WPARAM, l: LPARAM) -> isize {
    if msg == WM_INITDIALOG {
        // SAFETY: `l` is the `Dlg` that `run` keeps alive until the dialog
        // has ended.
        unsafe { SetWindowLongPtrW(dlg, GWLP_USERDATA, l) };
    }
    // SAFETY: GWLP_USERDATA is the `Dlg` from `run`, or null before.
    let Some(s) = (unsafe { state::<Dlg<'_>>(dlg) }) else {
        return 0;
    };
    if let Some(answer) = common(dlg, msg, w, l, &s.look) {
        return answer;
    }
    match msg {
        WM_INITDIALOG => {
            s.init(dlg);
            // The dialog manager puts the keyboard in the filter box.
            1
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
            // SAFETY: WM_NOTIFY's lParam is an NMHDR.
            let from_list = unsafe { (*(l as *const NMHDR)).idFrom } == usize::from(ID_LIST);
            if !from_list {
                return 0;
            }
            // SAFETY: a notification from the list.
            if let Some(answer) = unsafe { s.notify(dlg, l) } {
                // SAFETY: the answer of this dialog's WM_NOTIFY.
                unsafe { SetWindowLongPtrW(dlg, DWLP_MSGRESULT as i32, answer) };
            }
            1
        }
        WM_COMMAND => {
            s.command(dlg, (w & 0xffff) as u16, ((w >> 16) & 0xffff) as u32);
            1
        }
        WM_DESTROY => {
            places::keep(dlg, Which::RunHistory, Dlg::list(dlg));
            0
        }
        _ => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(filename: &str, count: u32, last: u64) -> Ran {
        Ran {
            filename: filename.into(),
            count,
            last,
        }
    }

    fn shown(all: &[Ran], filter: &str, sort: Sort) -> Vec<String> {
        view(all, filter, sort)
            .into_iter()
            .map(|i| all[i].filename.clone())
            .collect()
    }

    #[test]
    fn the_list_filters_and_sorts() {
        let all = [
            r(r"C:\Tools\beta.exe", 2, 40),
            r(r"D:\alpha.txt", 5, 30),
            r(r"C:\Tools\Gamma.exe", 2, 20),
            r(r"C:\Apps\alpha.txt", 1, 10),
        ];
        // The last run first by default.
        let sort = Sort::default();
        assert_eq!(
            shown(&all, "", sort),
            [
                r"C:\Tools\beta.exe",
                r"D:\alpha.txt",
                r"C:\Tools\Gamma.exe",
                r"C:\Apps\alpha.txt"
            ]
        );
        // Any part of the path, any case.
        assert_eq!(
            shown(&all, "TOOLS\\", sort),
            [r"C:\Tools\beta.exe", r"C:\Tools\Gamma.exe"]
        );
        assert_eq!(
            shown(&all, "ALPHA", sort),
            [r"D:\alpha.txt", r"C:\Apps\alpha.txt"]
        );
        assert!(shown(&all, "zzz", sort).is_empty());
        // By name A to Z ignoring case, the same name by its folder; then
        // the other way round.
        let by_name = sort.clicked(Column::Name);
        assert!(by_name.ascending);
        assert_eq!(
            shown(&all, "", by_name),
            [
                r"C:\Apps\alpha.txt",
                r"D:\alpha.txt",
                r"C:\Tools\beta.exe",
                r"C:\Tools\Gamma.exe"
            ]
        );
        assert_eq!(
            shown(&all, "", by_name.clicked(Column::Name))[0],
            r"C:\Tools\Gamma.exe"
        );
        // By folder, then name.
        let by_path = by_name.clicked(Column::Path);
        assert_eq!(
            by_path,
            Sort {
                column: Column::Path,
                ascending: true
            }
        );
        assert_eq!(
            shown(&all, "", by_path),
            [
                r"C:\Apps\alpha.txt",
                r"C:\Tools\beta.exe",
                r"C:\Tools\Gamma.exe",
                r"D:\alpha.txt"
            ]
        );
        // Most run first; a tie the last run first; then fewest first.
        let by_count = by_path.clicked(Column::Count);
        assert!(!by_count.ascending);
        assert_eq!(
            shown(&all, "", by_count),
            [
                r"D:\alpha.txt",
                r"C:\Tools\beta.exe",
                r"C:\Tools\Gamma.exe",
                r"C:\Apps\alpha.txt"
            ]
        );
        assert_eq!(
            shown(&all, "", by_count.clicked(Column::Count))[0],
            r"C:\Apps\alpha.txt"
        );
        // Back to the date: the last run first, then the first run first.
        let by_date = by_count.clicked(Column::Date);
        assert_eq!(by_date, Sort::default());
        assert_eq!(
            shown(&all, "", by_date.clicked(Column::Date))[0],
            r"C:\Apps\alpha.txt"
        );
    }

    #[test]
    fn columns_and_buttons_are_everythings() {
        let titles: Vec<&str> = COLUMNS.iter().map(|c| c.1).collect();
        assert_eq!(titles, ["Name", "Path", "Run Count", "Date Run"]);
        let widths: Vec<i32> = COLUMNS.iter().map(|c| c.2).collect();
        assert_eq!(widths, [176, 176, 96, 128]);
        let buttons: Vec<&str> = BUTTONS.iter().map(|b| b.1).collect();
        assert_eq!(
            buttons,
            [
                "&Open",
                "Open &Path",
                "&Remove",
                "&Import...",
                "E&xport...",
                "Remove &All"
            ]
        );
    }
}
