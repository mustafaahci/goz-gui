//! Edit > Undo History... (Ctrl+Shift+Z): Everything's dialog. Every item
//! of every operation that can be undone, a row each: its Date, Action,
//! Name, Path, New Name and New Path, sorted by a click on a column (the
//! newest first at first), narrowed by the Action list ("All actions",
//! "Delete", "Rename", ...) and by the Search box (any part of the names
//! and paths, any case). Undo (Enter) undoes the selected items; each goes
//! from the list as it is taken, and comes back if it could not be undone.
//! Esc closes. The place, size, columns and sort are kept (Everything's
//! `undo_history_*` keys).

use std::cell::{Cell, RefCell};

use windows_sys::Win32::Foundation::HWND;
use windows_sys::Win32::UI::Controls::{
    HDF_SORTDOWN, HDF_SORTUP, HDI_FORMAT, HDITEMW, LVIF_TEXT, LVIS_FOCUSED, LVIS_SELECTED,
    LVN_COLUMNCLICK, LVN_GETDISPINFOW, LVN_ITEMCHANGED, LVN_KEYDOWN, LVN_ODFINDITEMW,
    LVN_ODSTATECHANGED, LVNI_SELECTED, LVS_EX_DOUBLEBUFFER, LVS_EX_FULLROWSELECT,
    LVS_EX_HEADERDRAGDROP, LVS_EX_LABELTIP, LVS_OWNERDATA, LVS_REPORT, LVS_SHOWSELALWAYS,
    NM_CUSTOMDRAW, NMLISTVIEW, NMLVCUSTOMDRAW, NMLVDISPINFOW, NMLVKEYDOWN,
};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    VK_CONTROL, VK_DOWN, VK_NEXT, VK_PRIOR, VK_UP,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    BS_DEFPUSHBUTTON, CBN_SELCHANGE, CBS_DROPDOWNLIST, EN_CHANGE, ES_AUTOHSCROLL, IDCANCEL, IDOK,
    SM_CXVSCROLL, WM_APP, WM_COMMAND, WM_DESTROY, WM_GETMINMAXINFO, WM_INITDIALOG, WM_KEYDOWN,
    WM_NOTIFY, WM_SIZE, WS_EX_CLIENTEDGE, WS_MAXIMIZEBOX, WS_TABSTOP, WS_THICKFRAME, WS_VSCROLL,
};

use super::ui::{enable, item, set_state, units};
use super::undo::{Action, Record};
use super::{name_of, ui};
use crate::dialog::{
    Class, Look, Template, apply_theme, center_on_owner, common, list_custom_draw,
};
use crate::fileops::parent;
use crate::places::{self, Which};
use crate::win::{self, Msg, View, combo, dlg, header, lv, subclass, sys};

const TITLE: &str = "Undo History";

/// Posted to the dialog when the record changed: it shows it again.
pub const WM_REFRESH: u32 = WM_APP + 1;

/// What the dialog asks of the window that keeps the record.
pub trait Host {
    /// Every operation that can be undone, the oldest first.
    fn records(&self) -> Vec<Record>;
    /// The dialog is open (`dlg`): tell it of changes.
    fn opened(&self, dlg: HWND);
    /// Undoes these items (a record's number and the item's place in it).
    fn undo_items(&self, picks: &[(u64, usize)]);
    /// The sort kept (column, ascending), and a new one to keep.
    fn sort(&self) -> (u8, bool);
    fn set_sort(&self, column: u8, ascending: bool);
}

/// The columns: Everything's titles and widths (96 DPI), but for Date and
/// Action, which goz-gui's date with its time ("11/10/2026 1:04 PM") and
/// its longer actions ("Create Shortcut") need wider.
const COLUMNS: [(&str, i32); 6] = [
    ("Date", 128),
    ("Action", 96),
    ("Name", 128),
    ("Path", 128),
    ("New Name", 128),
    ("New Path", 128),
];

/// The columns' widths when none are kept, for a list `room` pixels wide
/// inside (at `dpi`): Date and Action as [`COLUMNS`] has them, the names
/// and paths sharing the rest (none under 64 px at 96 DPI), so that New
/// Path is not out of view as the dialog opens.
pub fn fitted_widths(room: i32, dpi: i32) -> [i32; 6] {
    let px = |v: i32| v * dpi / 96;
    let (date, action) = (px(COLUMNS[0].1), px(COLUMNS[1].1));
    let rest = (room - date - action).max(4 * px(64));
    let each = rest / 4;
    // What dividing leaves goes to the last.
    [date, action, each, each, each, rest - 3 * each]
}

/// One row: an item of a record.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Row {
    pub id: u64,
    pub item: usize,
    pub date: u64,
    pub action: Action,
    /// The texts of Name, Path, New Name and New Path.
    pub cells: [String; 4],
}

/// The rows of `records`: one per item, its names and folders.
pub fn rows(records: &[Record]) -> Vec<Row> {
    let name = |p: &[u16]| String::from_utf16_lossy(name_of(p));
    let folder = |p: &[u16]| String::from_utf16_lossy(parent(p).unwrap_or(&[]));
    let mut out = Vec::new();
    for rec in records {
        for (i, it) in rec.items.iter().enumerate() {
            let (new_name, new_path) = match rec.action {
                // Its place in the Recycle Bin is the bin's own business.
                Action::Delete => (String::new(), "Recycle Bin".to_owned()),
                _ => (name(&it.to), folder(&it.to)),
            };
            out.push(Row {
                id: rec.id,
                item: i,
                date: rec.date,
                action: rec.action,
                cells: [name(&it.from), folder(&it.from), new_name, new_path],
            });
        }
    }
    out
}

/// The rows shown: those of `action` (all for `None`) that hold `search`
/// (any case) in a name or path, sorted by `column` (0 Date ... 5 New
/// Path); ties newest first, then in their order.
pub fn view(
    all: &[Row],
    action: Option<Action>,
    search: &str,
    column: u8,
    ascending: bool,
) -> Vec<usize> {
    let needle = search.trim().to_lowercase();
    let mut shown: Vec<usize> = (0..all.len())
        .filter(|&i| action.is_none_or(|a| all[i].action == a))
        .filter(|&i| {
            needle.is_empty()
                || all[i]
                    .cells
                    .iter()
                    .any(|c| c.to_lowercase().contains(&needle))
        })
        .collect();
    shown.sort_by(|&x, &y| {
        let (a, b) = (&all[x], &all[y]);
        let order = match column {
            0 => a.date.cmp(&b.date),
            1 => a.action.label().cmp(b.action.label()),
            c => {
                let c = usize::from(c - 2).min(3);
                a.cells[c].to_lowercase().cmp(&b.cells[c].to_lowercase())
            }
        };
        let order = if ascending { order } else { order.reverse() };
        order
            .then_with(|| b.date.cmp(&a.date))
            .then_with(|| x.cmp(&y))
    });
    shown
}

const ID_ACTION: u16 = 1004;
const ID_SEARCH: u16 = 1001;
const ID_LIST: u16 = 1005;
const ID_UNDO: u16 = 1008;
const ID_COUNT: u16 = 1014;
const STATIC: u16 = 0xffff;
const CANCEL: u16 = IDCANCEL as u16;
const OK: u16 = IDOK as u16;

/// Dialog units: the dialog's size.
const SIZE: (i16, i16) = (420, 250);

struct Dlg<'a> {
    host: &'a dyn Host,
    look: Cell<Look>,
    all: RefCell<Vec<Row>>,
    shown: RefCell<Vec<usize>>,
    sort: Cell<(u8, bool)>,
    min: Cell<(i32, i32)>,
}

/// Shows the dialog, owned by `owner`, until it closes.
pub fn run(owner: HWND, host: &dyn Host, look: Look) {
    let state = Dlg {
        host,
        look: Cell::new(look),
        all: RefCell::new(Vec::new()),
        shown: RefCell::new(Vec::new()),
        sort: Cell::new(host.sort()),
        min: Cell::new((0, 0)),
    };
    let (cx, cy) = SIZE;
    let mut t = Template::new(TITLE, WS_THICKFRAME | WS_MAXIMIZEBOX, cx, cy);
    // The places are set by `layout`.
    t.control(Class::Static, "&Action:", STATIC, 0, 0, (7, 9, 30, 8))
        .control(
            Class::Named("ComboBox"),
            "",
            ID_ACTION,
            WS_TABSTOP | WS_VSCROLL | CBS_DROPDOWNLIST as u32,
            0,
            (38, 7, 90, 150),
        )
        .control(Class::Static, "&Search:", STATIC, 0, 0, (7, 25, 30, 8))
        .control(
            Class::Edit,
            "",
            ID_SEARCH,
            WS_TABSTOP | ES_AUTOHSCROLL as u32,
            WS_EX_CLIENTEDGE,
            (38, 23, cx - 45, 12),
        )
        .control(
            Class::Named("SysListView32"),
            "",
            ID_LIST,
            WS_TABSTOP | LVS_REPORT | LVS_OWNERDATA | LVS_SHOWSELALWAYS,
            WS_EX_CLIENTEDGE,
            (7, 40, cx - 70, cy - 60),
        )
        .control(
            Class::Button,
            "&Undo",
            ID_UNDO,
            WS_TABSTOP | BS_DEFPUSHBUTTON as u32,
            0,
            (cx - 57, 40, 50, 14),
        )
        .control(Class::Static, "", ID_COUNT, 0, 0, (7, cy - 14, 200, 8));
    t.show(owner, &|m| dialog_proc(&state, m));
}

/// Gives `list`'s columns [`fitted_widths`] for its width now, leaving
/// room for the vertical scroll bar that rows bring.
fn fit_columns(list: HWND) {
    // The list's size, DPI and the scroll bar's width at that DPI.
    let rc = win::client_rect(list);
    let dpi = crate::custom::window_dpi(list);
    let (dpi, bar) = (dpi as i32, sys::metric_for_dpi(SM_CXVSCROLL, dpi));
    for (i, w) in fitted_widths(rc.right - bar, dpi).into_iter().enumerate() {
        lv::set_column_width(list, i as i32, w);
    }
}

/// The search box's subclass: Up, Down, Page Up and Page Down move in the
/// list while the keyboard stays in the box (the history dialogs' filter
/// box does the same).
static SEARCH_BOX: subclass::Key = subclass::Key::new("undo history search box");

/// Subclasses `search` so its Up, Down, Page Up and Page Down go to `list`.
fn keys_to_list(search: HWND, list: HWND) {
    subclass::set(search, &SEARCH_BOX, move |m| {
        if m.id == WM_KEYDOWN && matches!(m.w as u16, VK_UP | VK_DOWN | VK_PRIOR | VK_NEXT) {
            win::send_plain(list, WM_KEYDOWN, m.w, m.l);
            return 0;
        }
        m.def()
    });
}

impl Dlg<'_> {
    fn list(dlg: HWND) -> HWND {
        item(dlg, ID_LIST)
    }

    fn init(&self, dlg: HWND) {
        self.host.opened(dlg);
        let list = Dlg::list(dlg);
        let ex =
            LVS_EX_FULLROWSELECT | LVS_EX_DOUBLEBUFFER | LVS_EX_LABELTIP | LVS_EX_HEADERDRAGDROP;
        lv::set_extended_style(list, ex, ex);
        ui::columns(list, &COLUMNS);
        let actions = item(dlg, ID_ACTION);
        // A themed list redraws for each item added (a few ms each): filled
        // unseen, as the dialog is not shown yet.
        win::set_redraw(actions, false);
        for label in std::iter::once("All actions").chain(Action::ALL.iter().map(|a| a.label())) {
            combo::add(actions, label);
        }
        combo::set_cur_sel(actions, 0);
        win::set_redraw(actions, true);
        // The search box's keys that move in the list (the subclass goes
        // with the box).
        keys_to_list(item(dlg, ID_SEARCH), list);
        apply_theme(dlg, self.look.get().dark);
        center_on_owner(dlg);
        let wr = win::window_rect(dlg);
        self.min.set((wr.right - wr.left, wr.bottom - wr.top));
        let kept = places::columns_kept(Which::UndoHistory);
        places::restore(dlg, Which::UndoHistory, list, self.min.get());
        self.layout(dlg);
        if !kept {
            fit_columns(list);
        }
        dlg::set_def_id(dlg, ID_UNDO);
        self.show_arrow(dlg);
        self.refill(dlg);
    }

    /// The controls for the dialog's size: Action and Search across the
    /// top, the list taking the room, Undo at its right, the count under.
    fn layout(&self, dlg: HWND) {
        let rc = win::client_rect(dlg);
        let (w, h) = (rc.right, rc.bottom);
        let (m, _) = units(dlg, 7, 0);
        let (label, row) = units(dlg, 31, 16);
        let (combo_w, edit_h) = units(dlg, 90, 12);
        let (bw, bh) = units(dlg, 50, 14);
        let (gap, text_h) = units(dlg, 6, 10);
        let (_, top) = units(dlg, 0, 40);
        let place = |id: u16, x: i32, y: i32, cw: i32, ch: i32| {
            let hwnd = item(dlg, id);
            if !hwnd.is_null() {
                win::move_window(hwnd, x, y, cw.max(0), ch.max(0), true);
            }
        };
        let (_, list_combo_h) = units(dlg, 0, 150);
        place(ID_ACTION, m + label, m, combo_w, list_combo_h);
        place(ID_SEARCH, m + label, m + row, w - 2 * m - label, edit_h);
        let bx = w - m - bw;
        let list_h = h - top - text_h - 2 * m;
        place(ID_LIST, m, top, bx - gap - m, list_h);
        place(ID_UNDO, bx, top, bw, bh);
        place(ID_COUNT, m, top + list_h + m / 2, w - 2 * m, text_h);
    }

    /// The Action list's choice: `None` for all actions.
    fn action(dlg: HWND) -> Option<Action> {
        let i = combo::cur_sel(item(dlg, ID_ACTION));
        usize::try_from(i - 1)
            .ok()
            .and_then(|i| Action::ALL.get(i).copied())
    }

    /// Reads the record from the host again and shows it.
    fn refill(&self, dlg: HWND) {
        *self.all.borrow_mut() = rows(&self.host.records());
        self.reshow(dlg);
    }

    /// Shows the rows through the Action list and the Search box, in the
    /// sort; the selection goes back to the first row.
    fn reshow(&self, dlg: HWND) {
        let (column, ascending) = self.sort.get();
        let search = win::text(item(dlg, ID_SEARCH));
        let shown = view(
            &self.all.borrow(),
            Dlg::action(dlg),
            &search,
            column,
            ascending,
        );
        let n = shown.len();
        *self.shown.borrow_mut() = shown;
        // The list's notifications come back here: nothing is borrowed.
        let list = Dlg::list(dlg);
        lv::set_item_count(list, n, 0);
        set_state(list, usize::MAX, 0, LVIS_SELECTED | LVIS_FOCUSED);
        if n > 0 {
            set_state(list, 0, LVIS_FOCUSED, LVIS_FOCUSED);
        }
        win::invalidate(list, None, true);
        let count = if n == 1 {
            "1 item".to_owned()
        } else {
            format!("{} items", crate::format::grouped(n as u64))
        };
        win::set_text(item(dlg, ID_COUNT), &count);
        self.update_buttons(dlg);
    }

    fn update_buttons(&self, dlg: HWND) {
        let selected = lv::selected_count(Dlg::list(dlg)) > 0;
        enable(dlg, ID_UNDO, selected);
        // A button greyed while it had the keyboard hands it to the list.
        let focus = win::focus();
        if !focus.is_null() && !win::is_enabled(focus) {
            dlg::focus_ctl(dlg, Dlg::list(dlg));
        }
    }

    fn show_arrow(&self, dlg: HWND) {
        let columns = lv::header(Dlg::list(dlg));
        let (column, ascending) = self.sort.get();
        for i in 0..COLUMNS.len() {
            let mut hd = header::get_item(columns, i as i32, HDI_FORMAT).map_or(
                HDITEMW {
                    mask: HDI_FORMAT,
                    ..Default::default()
                },
                |(hd, _)| hd,
            );
            hd.fmt &= !(HDF_SORTUP | HDF_SORTDOWN);
            if i == usize::from(column) {
                hd.fmt |= if ascending { HDF_SORTUP } else { HDF_SORTDOWN };
            }
            header::set_item(columns, i as i32, hd, None);
        }
    }

    /// A click on column `c`'s header: the other way round when it is the
    /// sort, else A to Z (the date: newest first).
    fn sort_by(&self, dlg: HWND, c: u8) {
        let (column, ascending) = self.sort.get();
        let sort = if column == c {
            (c, !ascending)
        } else {
            (c, c != 0)
        };
        self.sort.set(sort);
        self.host.set_sort(sort.0, sort.1);
        self.show_arrow(dlg);
        self.reshow(dlg);
    }

    /// Undo: the selected rows' items.
    fn undo(&self, dlg: HWND) {
        let list = Dlg::list(dlg);
        let mut picks = Vec::new();
        let mut at = usize::MAX;
        {
            let shown = self.shown.borrow();
            let all = self.all.borrow();
            loop {
                let next = lv::next_item(list, at as i32, LVNI_SELECTED);
                let Ok(row) = usize::try_from(next) else {
                    break;
                };
                if let Some(r) = shown.get(row).and_then(|&i| all.get(i)) {
                    picks.push((r.id, r.item));
                }
                at = row;
            }
        }
        if picks.is_empty() {
            return;
        }
        self.host.undo_items(&picks);
        self.refill(dlg);
    }

    fn disp_info(&self, info: &mut View<'_, NMLVDISPINFOW>) {
        if info.item.mask & LVIF_TEXT == 0 {
            return;
        }
        let text: Vec<u16> = {
            let shown = self.shown.borrow();
            let all = self.all.borrow();
            let row = usize::try_from(info.item.iItem)
                .ok()
                .and_then(|r| shown.get(r))
                .and_then(|&i| all.get(i));
            match (row, info.item.iSubItem) {
                (Some(r), 0) => {
                    let mut out = Vec::new();
                    crate::format::date(r.date as i64, &mut out);
                    out
                }
                (Some(r), 1) => r.action.label().encode_utf16().collect(),
                (Some(r), c @ 2..=5) => r.cells[(c - 2) as usize].encode_utf16().collect(),
                _ => Vec::new(),
            }
        };
        // Into the list's buffer of `cchTextMax` code units.
        info.set_text(&text);
    }

    /// Answers the list's notifications; `Some` is DWLP_MSGRESULT's value.
    fn notify(&self, dlg: HWND, m: &Msg) -> Option<isize> {
        let n = m.notify()?;
        match n.code() {
            NM_CUSTOMDRAW => {
                let cd = n.get::<NMLVCUSTOMDRAW>()?;
                return Some(list_custom_draw(n.from(), &cd, self.look.get().dark));
            }
            LVN_GETDISPINFOW => {
                if let Some(mut info) = n.get::<NMLVDISPINFOW>() {
                    self.disp_info(&mut info);
                }
            }
            LVN_ODFINDITEMW => return Some(-1),
            LVN_ITEMCHANGED | LVN_ODSTATECHANGED => self.update_buttons(dlg),
            LVN_COLUMNCLICK => {
                let sub = n.get::<NMLISTVIEW>().map(|nm| nm.iSubItem);
                if let Some(Ok(c)) = sub.map(u8::try_from)
                    && usize::from(c) < COLUMNS.len()
                {
                    self.sort_by(dlg, c);
                }
            }
            LVN_KEYDOWN => {
                let vk = n.get::<NMLVKEYDOWN>()?.wVKey;
                let ctrl = sys::key_down(VK_CONTROL);
                if ctrl && vk == u16::from(b'A') {
                    set_state(Dlg::list(dlg), usize::MAX, LVIS_SELECTED, LVIS_SELECTED);
                }
            }
            _ => {}
        }
        None
    }
}

fn dialog_proc(s: &Dlg<'_>, m: &Msg) -> isize {
    let dlg = m.hwnd;
    if let Some(answer) = common(m, &s.look) {
        return answer;
    }
    match m.id {
        WM_INITDIALOG => {
            s.init(dlg);
            1
        }
        WM_REFRESH => {
            s.refill(dlg);
            1
        }
        WM_SIZE => {
            s.layout(dlg);
            1
        }
        WM_GETMINMAXINFO => {
            let (w, h) = s.min.get();
            if let Some(mut mmi) = m.min_max_info() {
                mmi.ptMinTrackSize.x = w;
                mmi.ptMinTrackSize.y = h;
            }
            1
        }
        WM_NOTIFY => {
            let from_list = m.notify().is_some_and(|n| n.id() == usize::from(ID_LIST));
            if !from_list {
                return 0;
            }
            if let Some(answer) = s.notify(dlg, m) {
                dlg::set_result(dlg, answer);
            }
            1
        }
        WM_COMMAND => {
            let (id, code) = (win::loword(m.w), u32::from(win::hiword(m.w)));
            match id {
                ID_UNDO | OK => s.undo(dlg),
                ID_SEARCH if code == EN_CHANGE => s.reshow(dlg),
                ID_ACTION if code == CBN_SELCHANGE => s.reshow(dlg),
                CANCEL => dlg::end(dlg, 0),
                _ => {}
            }
            1
        }
        WM_DESTROY => {
            places::keep(dlg, Which::UndoHistory, Dlg::list(dlg));
            0
        }
        _ => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::filetools::undo::Item;

    fn w(s: &str) -> Vec<u16> {
        s.encode_utf16().collect()
    }

    fn rec(id: u64, date: u64, action: Action, items: &[(&str, &str)]) -> Record {
        Record {
            id,
            date,
            action,
            items: items
                .iter()
                .map(|(f, t)| Item {
                    from: w(f),
                    to: w(t),
                    before: None,
                    after: None,
                })
                .collect(),
        }
    }

    #[test]
    fn the_columns_fit_the_list_as_it_opens() {
        // The list as the dialog opens at 100 % (608 px inside), less a
        // scroll bar: every column in view, Date and Action as given.
        let w = fitted_widths(591, 96);
        assert_eq!(w.iter().sum::<i32>(), 591);
        assert_eq!((w[0], w[1]), (128, 96));
        assert!(w[2..].iter().all(|&c| c >= 91));
        // 150 %.
        let w = fitted_widths(887, 144);
        assert_eq!(w.iter().sum::<i32>(), 887);
        assert_eq!((w[0], w[1]), (192, 144));
        // Too narrow: the names and paths keep 64 px (at 96 DPI) each.
        assert_eq!(fitted_widths(100, 96)[2..], [64, 64, 64, 64]);
    }

    #[test]
    fn rows_list_each_item() {
        let all = rows(&[
            rec(1, 10, Action::Rename, &[(r"C:\a\x.txt", r"C:\a\y.txt")]),
            rec(
                2,
                20,
                Action::Delete,
                &[(r"C:\a\z.txt", r"C:\$Recycle.Bin\S\$R1.txt")],
            ),
            rec(
                3,
                30,
                Action::Move,
                &[(r"C:\a\m.txt", r"D:\m.txt"), (r"C:\a\n.txt", r"D:\n.txt")],
            ),
        ]);
        assert_eq!(all.len(), 4);
        assert_eq!(
            all[0].cells,
            ["x.txt", r"C:\a", "y.txt", r"C:\a"].map(String::from)
        );
        assert_eq!(all[1].cells[2..], ["", "Recycle Bin"].map(String::from));
        assert_eq!((all[3].id, all[3].item), (3, 1));
        assert_eq!(all[3].cells[3], r"D:\");
        // Newest first by default.
        let ids = |v: Vec<usize>| v.into_iter().map(|i| all[i].id).collect::<Vec<_>>();
        assert_eq!(ids(view(&all, None, "", 0, false)), [3, 3, 2, 1]);
        assert_eq!(ids(view(&all, None, "", 0, true)), [1, 2, 3, 3]);
        // By action, by search (any case), by a name column.
        assert_eq!(ids(view(&all, Some(Action::Delete), "", 0, false)), [2]);
        assert_eq!(ids(view(&all, None, "Y.TXT", 0, false)), [1]);
        assert_eq!(ids(view(&all, None, "d:", 0, false)), [3, 3]);
        let by_name = view(&all, None, "", 2, true);
        let names: Vec<&str> = by_name.iter().map(|&i| all[i].cells[0].as_str()).collect();
        assert_eq!(names, ["m.txt", "n.txt", "x.txt", "z.txt"]);
        assert_eq!(
            COLUMNS.map(|c| c.0),
            ["Date", "Action", "Name", "Path", "New Name", "New Path"]
        );
    }
}
