//! History > Find Search History... (Ctrl+H) and Organize Search
//! History... (Ctrl+Shift+H): the search history as a list of `Search`,
//! `Search Count` and `Last Search Date`, newest first, sortable by a click
//! on a column's header, with a box above it that filters the list as it is
//! typed (any part of the search, any case).
//!
//! Find ("Select Search History", as Everything calls it) picks one: Open
//! (Enter, a double-click) puts it in the search box and closes, Copy puts
//! its text on the clipboard. Organize edits the history: Delete (the
//! selected searches; the Delete key too), Delete All (asks first),
//! Import... (merges a `Search History.csv` of Everything's or goz-gui's)
//! and Export... (all, or the selection only). Each change is made at once;
//! the window writes the file.
//!
//! Up, Down, Page Up and Page Down in the filter box move in the list, so
//! the keyboard can stay in the box.

use std::cell::{Cell, RefCell};

use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows_sys::Win32::Graphics::Gdi::InvalidateRect;
use windows_sys::Win32::UI::Controls::{
    HDF_SORTDOWN, HDF_SORTUP, HDI_FORMAT, HDITEMW, HDM_GETITEMW, HDM_SETITEMW, LVCF_FMT, LVCF_TEXT,
    LVCF_WIDTH, LVCFMT_LEFT, LVCFMT_RIGHT, LVCOLUMNW, LVIF_TEXT, LVIS_FOCUSED, LVIS_SELECTED,
    LVITEMW, LVM_ENSUREVISIBLE, LVM_GETHEADER, LVM_GETNEXTITEM, LVM_GETSELECTEDCOUNT,
    LVM_INSERTCOLUMNW, LVM_SETEXTENDEDLISTVIEWSTYLE, LVM_SETITEMCOUNT, LVM_SETITEMSTATE,
    LVM_SETSELECTIONMARK, LVN_COLUMNCLICK, LVN_GETDISPINFOW, LVN_ITEMCHANGED, LVN_KEYDOWN,
    LVN_ODFINDITEMW, LVN_ODSTATECHANGED, LVNI_FOCUSED, LVNI_SELECTED, LVS_EX_DOUBLEBUFFER,
    LVS_EX_FULLROWSELECT, LVS_EX_HEADERDRAGDROP, LVS_EX_LABELTIP, LVS_OWNERDATA, LVS_REPORT,
    LVS_SHOWSELALWAYS, LVS_SINGLESEL, NM_CUSTOMDRAW, NM_DBLCLK, NMHDR, NMITEMACTIVATE, NMLISTVIEW,
    NMLVCUSTOMDRAW, NMLVDISPINFOW, NMLVKEYDOWN,
};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    EnableWindow, GetFocus, GetKeyState, IsWindowEnabled, VK_CONTROL, VK_DELETE, VK_DOWN, VK_NEXT,
    VK_PRIOR, VK_UP,
};
use windows_sys::Win32::UI::Shell::{DefSubclassProc, RemoveWindowSubclass, SetWindowSubclass};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    BS_DEFPUSHBUTTON, DM_SETDEFID, DWLP_MSGRESULT, EN_CHANGE, ES_AUTOHSCROLL, EndDialog,
    GWLP_USERDATA, GetClientRect, GetDlgItem, GetWindowRect, IDCANCEL, IDOK, IDYES, MB_DEFBUTTON2,
    MB_ICONWARNING, MB_OK, MB_YESNO, MINMAXINFO, MapDialogRect, MoveWindow, SendMessageW,
    SetWindowLongPtrW, WM_COMMAND, WM_DESTROY, WM_GETMINMAXINFO, WM_INITDIALOG, WM_KEYDOWN,
    WM_NCDESTROY, WM_NEXTDLGCTL, WM_NOTIFY, WM_SIZE, WS_EX_CLIENTEDGE, WS_MAXIMIZEBOX, WS_TABSTOP,
    WS_THICKFRAME,
};

use crate::bookmarks::one_line;
use crate::dialog::{
    Class, Look, Template, apply_theme, center_on_owner, common, list_custom_draw, state,
};
use crate::filedlg::{self, Check};
use crate::format;
use crate::history::Searched;
use crate::history_file::{self, Table};
use crate::msgbox;
use crate::places::{self, Which};
use crate::store;
use crate::wide::{window_text, wz};

/// Which of the two dialogs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    /// Find Search History: pick one.
    Find,
    /// Organize Search History: delete, import, export.
    Organize,
}

/// What the dialogs ask of the window that owns the search history.
pub trait Host {
    /// The search history, newest first.
    fn searches(&self) -> Vec<Searched>;
    /// Deletes these searches.
    fn forget(&self, searches: &[String]);
    /// Deletes every search.
    fn forget_all(&self);
    /// Merges searches read from a file into the history.
    fn import(&self, entries: Vec<Searched>);
    /// Puts `text` on the clipboard; false when the clipboard was busy.
    fn copy(&self, text: &str) -> bool;
}

/// The list's columns.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Column {
    Search,
    Count,
    Date,
}

/// Each column: its title, its width at 96 DPI (Everything's), and whether
/// it is right-aligned.
const COLUMNS: [(Column, &str, i32, bool); 3] = [
    (Column::Search, "Search", 256, false),
    (Column::Count, "Search Count", 96, true),
    (Column::Date, "Last Search Date", 128, false),
];

/// How the list is sorted.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Sort {
    pub column: Column,
    pub ascending: bool,
}

impl Default for Sort {
    /// Newest first, as Everything opens them.
    fn default() -> Sort {
        Sort {
            column: Column::Date,
            ascending: false,
        }
    }
}

impl Sort {
    /// The sort after a click on `column`'s header: the other direction
    /// when it is the sort already, else A to Z for the search, most first
    /// for the count and newest first for the date.
    pub fn clicked(self, column: Column) -> Sort {
        if self.column == column {
            Sort {
                ascending: !self.ascending,
                ..self
            }
        } else {
            Sort {
                column,
                ascending: column == Column::Search,
            }
        }
    }
}

/// The rows the list shows: the indices into `all` of the searches that
/// contain `filter` (ignoring case), in `sort`'s order; ties newest first.
pub fn view(all: &[Searched], filter: &str, sort: Sort) -> Vec<usize> {
    let needle = filter.to_lowercase();
    let keys: Vec<String> = all.iter().map(|e| e.search.to_lowercase()).collect();
    let mut rows: Vec<usize> = (0..all.len())
        .filter(|&i| keys[i].contains(&needle))
        .collect();
    rows.sort_by(|&x, &y| {
        let (a, b) = (&all[x], &all[y]);
        let order = match sort.column {
            Column::Search => keys[x].cmp(&keys[y]).then_with(|| a.search.cmp(&b.search)),
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
const ID_COPY: u16 = 103;
const ID_DELETE: u16 = 104;
const ID_DELETE_ALL: u16 = 105;
const ID_IMPORT: u16 = 106;
const ID_EXPORT: u16 = 107;
const STATIC: u16 = 0xffff;
/// The name Export suggests: not the live file's own, which the dialog
/// would offer to replace when it opens in the data folder.
const EXPORT_NAME: &str = "Search History export.csv";
const OK: u16 = IDOK as u16;
const CANCEL: u16 = IDCANCEL as u16;

/// Dialog units: the margin, a button's size, the gap between buttons.
const MARGIN: i32 = 7;
const BUTTON: (i32, i32) = (50, 14);
const GAP: i32 = 4;
/// Where the list starts, under the filter box.
const LIST_TOP: i32 = 24;
/// The width of the "Search:" label and its gap.
const LABEL: i32 = 29;
/// Organize's buttons at the right, top edges in dialog units.
const ORGANIZE_BUTTONS: [(u16, i32); 5] = [
    (ID_OPEN, 24),
    (ID_DELETE, 42),
    (ID_IMPORT, 66),
    (ID_EXPORT, 84),
    (ID_DELETE_ALL, 102),
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
fn warn(dlg: HWND, title: &str, text: &str) {
    msgbox::show(dlg, text, title, MB_OK | MB_ICONWARNING);
}

/// A dialog's state, in its GWLP_USERDATA while it is open.
struct Dlg<'a> {
    mode: Mode,
    title: &'static str,
    host: &'a dyn Host,
    look: Cell<Look>,
    /// The history as last read from the host, newest first.
    all: RefCell<Vec<Searched>>,
    /// The rows shown: indices into `all`.
    shown: RefCell<Vec<usize>>,
    sort: Cell<Sort>,
    /// The dialog's size as it opened, the smallest it can be made.
    min: Cell<(i32, i32)>,
    /// The search Open chose.
    chosen: RefCell<Option<String>>,
}

/// Shows the dialog `mode`, owned by `owner`, until it closes; the search
/// Open chose, if any.
pub fn run(owner: HWND, mode: Mode, host: &dyn Host, look: Look) -> Option<String> {
    let title = match mode {
        Mode::Find => "Select Search History",
        Mode::Organize => "Organize Search History",
    };
    let state = Dlg {
        mode,
        title,
        host,
        look: Cell::new(look),
        all: RefCell::new(Vec::new()),
        shown: RefCell::new(Vec::new()),
        sort: Cell::new(Sort::default()),
        min: Cell::new((0, 0)),
        chosen: RefCell::new(None),
    };
    let (cx, cy) = match mode {
        Mode::Find => (300, 256),
        Mode::Organize => (350, 256),
    };
    let mut list = WS_TABSTOP | LVS_REPORT | LVS_OWNERDATA | LVS_SHOWSELALWAYS;
    if mode == Mode::Find {
        list |= LVS_SINGLESEL;
    }
    let button = WS_TABSTOP;
    let default = WS_TABSTOP | BS_DEFPUSHBUTTON as u32;
    let mut t = Template::new(title, WS_THICKFRAME | WS_MAXIMIZEBOX, cx, cy);
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
            list,
            WS_EX_CLIENTEDGE,
            (7, 24, cx - 14, cy - 52),
        );
    let buttons: &[(u16, &str, u32)] = match mode {
        Mode::Find => &[
            (ID_OPEN, "&Open", default),
            (ID_COPY, "&Copy", button),
            (CANCEL, "Close", button),
        ],
        Mode::Organize => &[
            (ID_OPEN, "&Open", default),
            (ID_DELETE, "&Delete", button),
            (ID_IMPORT, "&Import...", button),
            (ID_EXPORT, "E&xport...", button),
            (ID_DELETE_ALL, "Delete &All", button),
            (CANCEL, "Close", button),
        ],
    };
    for &(id, text, style) in buttons {
        t.control(Class::Button, text, id, style, 0, (0, 0, 50, 14));
    }
    t.run(owner, Some(dialog_proc), (&raw const state) as LPARAM);
    state.chosen.into_inner()
}

impl Dlg<'_> {
    fn list(dlg: HWND) -> HWND {
        item(dlg, ID_LIST)
    }

    /// Where the dialog's place is kept.
    fn which(&self) -> Which {
        match self.mode {
            Mode::Find => Which::SearchHistorySelect,
            Mode::Organize => Which::SearchHistory,
        }
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
        places::restore(dlg, self.which(), list, self.min.get());
        self.layout(dlg);
        send(dlg, DM_SETDEFID, usize::from(ID_OPEN), 0);
        self.show_arrow(dlg);
        self.refill(dlg, None, Some(0));
    }

    /// Places the controls for the dialog's size: the filter box across the
    /// top, the list taking the room, the buttons at the bottom (Find) or
    /// the right (Organize).
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
        let (gap, _) = px(GAP, 0);
        let (label, top) = px(LABEL, LIST_TOP);
        let (_, edit_h) = px(0, 12);
        let place = |id: u16, x: i32, y: i32, cw: i32, ch: i32| {
            let hwnd = item(dlg, id);
            if !hwnd.is_null() {
                // SAFETY: moves a control of this thread's dialog.
                unsafe { MoveWindow(hwnd, x, y, cw.max(0), ch.max(0), 1) };
            }
        };
        place(ID_FILTER, mx + label, my, w - 2 * mx - label, edit_h);
        match self.mode {
            Mode::Find => {
                let by = h - my - bh;
                place(ID_LIST, mx, top, w - 2 * mx, by - px(0, MARGIN).1 - top);
                let mut x = w - mx - bw;
                for id in [CANCEL, ID_COPY, ID_OPEN] {
                    place(id, x, by, bw, bh);
                    x -= bw + gap;
                }
            }
            Mode::Organize => {
                let bx = w - mx - bw;
                let (column_gap, _) = px(6, 0);
                place(ID_LIST, mx, top, bx - column_gap - mx, h - my - top);
                for (id, y) in ORGANIZE_BUTTONS {
                    place(id, bx, px(0, y).1, bw, bh);
                }
                place(CANCEL, bx, h - my - bh, bw, bh);
            }
        }
    }

    /// Reads the history from the host again and shows it.
    fn refill(&self, dlg: HWND, keep: Option<&str>, at: Option<usize>) {
        *self.all.borrow_mut() = self.host.searches();
        self.reshow(dlg, keep, at);
    }

    /// Shows the history through the filter, in the sort, with the search
    /// `keep` selected if it is shown, else row `at` (kept in range).
    fn reshow(&self, dlg: HWND, keep: Option<&str>, at: Option<usize>) {
        let filter = window_text(item(dlg, ID_FILTER));
        let (rows, select) = {
            let all = self.all.borrow();
            let rows = view(&all, &filter, self.sort.get());
            let kept = keep.and_then(|k| rows.iter().position(|&i| all[i].search == k));
            let select = kept
                .or(at.map(|a| a.min(rows.len().saturating_sub(1))))
                .filter(|&i| i < rows.len());
            (rows, select)
        };
        let n = rows.len();
        *self.shown.borrow_mut() = rows;
        // The list's notifications come back into this dialog: no borrow
        // is held from here on.
        let list = Dlg::list(dlg);
        send(list, LVM_SETITEMCOUNT, n, 0);
        set_state(list, usize::MAX, 0, LVIS_SELECTED | LVIS_FOCUSED);
        if let Some(i) = select {
            set_state(
                list,
                i,
                LVIS_SELECTED | LVIS_FOCUSED,
                LVIS_SELECTED | LVIS_FOCUSED,
            );
            send(list, LVM_SETSELECTIONMARK, 0, i as isize);
            send(list, LVM_ENSUREVISIBLE, i, 0);
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

    /// The search the keyboard is on (the focused row if selected, else
    /// the first selected).
    fn current(&self, dlg: HWND) -> Option<String> {
        let list = Dlg::list(dlg);
        let focused = send(
            list,
            LVM_GETNEXTITEM,
            usize::MAX,
            (LVNI_FOCUSED | LVNI_SELECTED) as isize,
        );
        let index = if focused >= 0 {
            self.shown.borrow().get(focused as usize).copied()
        } else {
            self.selected(dlg).first().copied()
        };
        index.and_then(|i| self.all.borrow().get(i).map(|e| e.search.clone()))
    }

    /// Greys what the selection does not allow.
    fn update_buttons(&self, dlg: HWND) {
        let selected = send(Dlg::list(dlg), LVM_GETSELECTEDCOUNT, 0, 0) > 0;
        let any = !self.all.borrow().is_empty();
        enable(dlg, ID_OPEN, selected);
        enable(dlg, ID_COPY, selected);
        enable(dlg, ID_DELETE, selected);
        enable(dlg, ID_DELETE_ALL, any);
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
        let keep = self.current(dlg);
        self.sort.set(self.sort.get().clicked(column));
        self.show_arrow(dlg);
        self.reshow(dlg, keep.as_deref(), Some(0));
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
                match column {
                    Column::Search => text.extend(one_line(&e.search).encode_utf16()),
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

    /// Open: the search goes to the search box, and the dialog closes.
    fn open(&self, dlg: HWND) {
        let Some(search) = self.current(dlg) else {
            return;
        };
        *self.chosen.borrow_mut() = Some(search);
        // SAFETY: ends this modal dialog.
        unsafe { EndDialog(dlg, 1) };
    }

    /// Copy: the selected searches' texts, one per line.
    fn copy(&self, dlg: HWND) {
        let text = {
            let all = self.all.borrow();
            self.selected(dlg)
                .iter()
                .map(|&i| all[i].search.as_str())
                .collect::<Vec<_>>()
                .join("\r\n")
        };
        // The host says so when the clipboard is busy.
        if !text.is_empty() {
            self.host.copy(&text);
        }
    }

    fn delete(&self, dlg: HWND) {
        let list = Dlg::list(dlg);
        let first = send(list, LVM_GETNEXTITEM, usize::MAX, LVNI_SELECTED as isize);
        let searches: Vec<String> = {
            let all = self.all.borrow();
            self.selected(dlg)
                .iter()
                .map(|&i| all[i].search.clone())
                .collect()
        };
        if searches.is_empty() {
            return;
        }
        self.host.forget(&searches);
        self.refill(dlg, None, usize::try_from(first).ok());
    }

    fn delete_all(&self, dlg: HWND) {
        let n = self.all.borrow().len();
        if n == 0 {
            return;
        }
        let question = if n == 1 {
            "Delete the 1 search in the search history?".to_owned()
        } else {
            format!(
                "Delete all {} searches in the search history?",
                format::grouped(n as u64)
            )
        };
        let answer = msgbox::show(
            dlg,
            &question,
            self.title,
            MB_YESNO | MB_ICONWARNING | MB_DEFBUTTON2,
        );
        if answer == IDYES {
            self.host.forget_all();
            self.refill(dlg, None, None);
        }
    }

    fn import(&self, dlg: HWND) {
        let Some(path) = filedlg::open(dlg, "Import Search History", filedlg::CSV, "csv") else {
            return;
        };
        let read = std::fs::read(&path)
            .map_err(|e| e.to_string())
            .and_then(|bytes| history_file::parse_import(&bytes));
        match read {
            Ok(table) => {
                let keep = self.current(dlg);
                self.host.import(table.entries);
                self.refill(dlg, keep.as_deref(), Some(0));
            }
            Err(why) => warn(
                dlg,
                self.title,
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
            "Export Search History",
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
                self.title,
                &format!(
                    "{} is goz-gui's own file. Export the search history to another file or \
                     folder.",
                    path.display()
                ),
            );
            return;
        }
        let text = {
            let all = self.all.borrow();
            // The history's order (newest first), as the file keeps it.
            let entries: Vec<Searched> = if only {
                selected.sort_unstable();
                selected.iter().map(|&i| all[i].clone()).collect()
            } else {
                all.clone()
            };
            history_file::to_csv(&entries, &Table::default())
        };
        if let Err(e) = store::file::write(&path, text.as_bytes(), None) {
            warn(
                dlg,
                self.title,
                &format!(
                    "The search history could not be exported to {}: {e}",
                    path.display()
                ),
            );
        }
    }

    fn command(&self, dlg: HWND, id: u16, code: u32) {
        match id {
            // Enter is Open, the default button.
            ID_OPEN | OK => self.open(dlg),
            ID_COPY => self.copy(dlg),
            ID_DELETE if self.mode == Mode::Organize => self.delete(dlg),
            ID_DELETE_ALL => self.delete_all(dlg),
            ID_IMPORT => self.import(dlg),
            ID_EXPORT => self.export(dlg),
            ID_FILTER if code == EN_CHANGE => {
                let keep = self.current(dlg);
                self.reshow(dlg, keep.as_deref(), Some(0));
            }
            // SAFETY: ends this modal dialog.
            CANCEL => unsafe {
                EndDialog(dlg, 0);
            },
            _ => {}
        }
    }

    /// A key pressed in the list: Delete deletes (Organize), Ctrl+A selects
    /// every row (Organize), Ctrl+C copies.
    fn key(&self, dlg: HWND, vk: u16) {
        // SAFETY: reads this thread's keyboard state.
        let ctrl = unsafe { GetKeyState(i32::from(VK_CONTROL)) } < 0;
        match (vk, ctrl) {
            (VK_DELETE, false) if self.mode == Mode::Organize => self.delete(dlg),
            (0x41, true) if self.mode == Mode::Organize => {
                set_state(Dlg::list(dlg), usize::MAX, LVIS_SELECTED, LVIS_SELECTED);
            }
            (0x43, true) => self.copy(dlg),
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
                        self.open(dlg);
                    }
                }
                LVN_KEYDOWN => self.key(dlg, (*(l as *const NMLVKEYDOWN)).wVKey),
                _ => {}
            }
            None
        }
    }
}

/// The filter box's subclass: Up, Down, Page Up and Page Down move in the
/// list (`list`, the subclass's data) while the keyboard stays here.
pub(crate) unsafe extern "system" fn filter_proc(
    hwnd: HWND,
    msg: u32,
    w: WPARAM,
    l: LPARAM,
    _id: usize,
    list: usize,
) -> LRESULT {
    // SAFETY: `list` is the dialog's list, alive as long as this box;
    // `w` and `l` are passed on as they came.
    unsafe {
        match msg {
            WM_KEYDOWN if matches!(w as u16, VK_UP | VK_DOWN | VK_PRIOR | VK_NEXT) => {
                SendMessageW(list as HWND, WM_KEYDOWN, w, l);
                return 0;
            }
            WM_NCDESTROY => {
                RemoveWindowSubclass(hwnd, Some(filter_proc), 1);
            }
            _ => {}
        }
        DefSubclassProc(hwnd, msg, w, l)
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
            places::keep(dlg, s.which(), Dlg::list(dlg));
            0
        }
        _ => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(search: &str, count: u32, last: u64) -> Searched {
        Searched {
            search: search.into(),
            count,
            last,
        }
    }

    fn shown(all: &[Searched], filter: &str, sort: Sort) -> Vec<String> {
        view(all, filter, sort)
            .into_iter()
            .map(|i| all[i].search.clone())
            .collect()
    }

    #[test]
    fn the_list_filters_and_sorts() {
        let all = [
            s("beta", 2, 40),
            s("Alpha", 5, 30),
            s("gamma.exe", 2, 20),
            s("alpha", 1, 10),
        ];
        // Newest first by default.
        let sort = Sort::default();
        assert_eq!(
            shown(&all, "", sort),
            ["beta", "Alpha", "gamma.exe", "alpha"]
        );
        // Any part, any case.
        assert_eq!(shown(&all, "ALP", sort), ["Alpha", "alpha"]);
        assert_eq!(shown(&all, ".EXE", sort), ["gamma.exe"]);
        assert!(shown(&all, "zzz", sort).is_empty());
        // A to Z ignoring case (then by case); the other way round.
        let by_search = sort.clicked(Column::Search);
        assert_eq!(
            by_search,
            Sort {
                column: Column::Search,
                ascending: true
            }
        );
        assert_eq!(
            shown(&all, "", by_search),
            ["Alpha", "alpha", "beta", "gamma.exe"]
        );
        let z_to_a = by_search.clicked(Column::Search);
        assert_eq!(
            shown(&all, "", z_to_a),
            ["gamma.exe", "beta", "alpha", "Alpha"]
        );
        // Most searched first; a tie newest first.
        let by_count = z_to_a.clicked(Column::Count);
        assert_eq!(
            by_count,
            Sort {
                column: Column::Count,
                ascending: false
            }
        );
        assert_eq!(
            shown(&all, "", by_count),
            ["Alpha", "beta", "gamma.exe", "alpha"]
        );
        let fewest = by_count.clicked(Column::Count);
        assert_eq!(
            shown(&all, "", fewest),
            ["alpha", "beta", "gamma.exe", "Alpha"]
        );
        // Back to the date: newest first, then oldest first.
        let by_date = fewest.clicked(Column::Date);
        assert_eq!(by_date, Sort::default());
        let oldest = by_date.clicked(Column::Date);
        assert_eq!(
            shown(&all, "", oldest),
            ["alpha", "gamma.exe", "Alpha", "beta"]
        );
    }

    #[test]
    fn columns_are_everythings() {
        let titles: Vec<&str> = COLUMNS.iter().map(|c| c.1).collect();
        assert_eq!(titles, ["Search", "Search Count", "Last Search Date"]);
        let widths: Vec<i32> = COLUMNS.iter().map(|c| c.2).collect();
        assert_eq!(widths, [256, 96, 128]);
    }
}
