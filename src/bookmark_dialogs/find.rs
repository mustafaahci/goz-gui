//! Bookmarks > Find Bookmark... (Ctrl+B), "Select Bookmark" as Everything
//! calls it: every bookmark in one list, `Name | Search`, a bookmark in a
//! folder named with the folder's path (`Music/Rock/Queen`), in the order
//! the Bookmarks menu shows them. The box above filters the list as it is
//! typed (any part of the name or the search, any case); Up, Down, Page Up
//! and Page Down in it move in the list. Open (Enter, a double-click) opens
//! the bookmark and closes; Copy Search puts its search on the clipboard.

use std::cell::{Cell, RefCell};

use windows_sys::Win32::Foundation::{HWND, RECT};
use windows_sys::Win32::UI::Controls::{
    LVCF_TEXT, LVCF_WIDTH, LVCOLUMNW, LVIF_TEXT, LVIS_FOCUSED, LVIS_SELECTED, LVN_GETDISPINFOW,
    LVN_ITEMCHANGED, LVN_KEYDOWN, LVN_ODFINDITEMW, LVN_ODSTATECHANGED, LVNI_SELECTED,
    LVS_EX_DOUBLEBUFFER, LVS_EX_FULLROWSELECT, LVS_EX_LABELTIP, LVS_OWNERDATA, LVS_REPORT,
    LVS_SHOWSELALWAYS, LVS_SINGLESEL, NM_CUSTOMDRAW, NM_DBLCLK, NMITEMACTIVATE, NMLVCUSTOMDRAW,
    NMLVDISPINFOW, NMLVKEYDOWN,
};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::VK_CONTROL;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    BS_DEFPUSHBUTTON, EN_CHANGE, ES_AUTOHSCROLL, IDCANCEL, IDOK, WM_COMMAND, WM_DESTROY,
    WM_GETMINMAXINFO, WM_INITDIALOG, WM_NOTIFY, WM_SIZE, WS_EX_CLIENTEDGE, WS_MAXIMIZEBOX,
    WS_TABSTOP, WS_THICKFRAME,
};

use super::{Host, WM_REFRESH, enable};
use crate::bookmarks::tree::{TOP, Tree};
use crate::bookmarks::{Bookmarks, Settings, one_line};
use crate::dialog::{
    Class, Look, Template, apply_theme, center_on_owner, common_msg, fill_last_column,
    list_custom_draw,
};
use crate::history_dialogs::filter_keys;
use crate::places::{self, Which};
use crate::win::{self, Msg, Notify, View, dlg, lv, sys};

/// What a bookmark opens: its search (`None`: the one in the box stays)
/// and its options.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Chosen {
    pub search: Option<String>,
    pub settings: Settings,
}

/// A bookmark as the list shows it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    /// Its folders' names and its own, joined by `/`.
    pub label: String,
    /// Its search, on one line.
    pub search: String,
    pub chosen: Chosen,
}

/// Every bookmark of `list`, in the menu's order.
pub fn entries(list: &Bookmarks) -> Vec<Entry> {
    let tree = Tree::build(list);
    tree.bookmarks_below(list, TOP)
        .into_iter()
        .map(|r| {
            let row = &list.rows()[r];
            let mut path = row.folder();
            path.push(one_line(row.name()));
            Entry {
                label: path.join("/"),
                search: one_line(row.search()),
                chosen: Chosen {
                    search: row.search_set().then(|| row.search().to_owned()),
                    settings: row.settings(),
                },
            }
        })
        .collect()
}

/// The entries whose name (with its folders) or search holds `filter`, in
/// any case.
pub fn view(all: &[Entry], filter: &str) -> Vec<usize> {
    let needle = filter.to_lowercase();
    (0..all.len())
        .filter(|&i| {
            all[i].label.to_lowercase().contains(&needle)
                || all[i].search.to_lowercase().contains(&needle)
        })
        .collect()
}

const ID_FILTER: u16 = 100;
const ID_LIST: u16 = 101;
const ID_OPEN: u16 = 102;
const ID_COPY: u16 = 103;
const STATIC: u16 = 0xffff;
const OK: u16 = IDOK as u16;
const CANCEL: u16 = IDCANCEL as u16;
/// The columns: title and width at 96 DPI.
const COLUMNS: [(&str, i32); 2] = [("Name", 180), ("Search", 256)];

struct Find<'a> {
    host: &'a dyn Host,
    look: Cell<Look>,
    generation: Cell<u64>,
    all: RefCell<Vec<Entry>>,
    shown: RefCell<Vec<usize>>,
    /// The list's selected row, as last seen.
    selected_row: Cell<Option<usize>>,
    min: Cell<(i32, i32)>,
    chosen: RefCell<Option<Chosen>>,
}

/// Shows Find Bookmark over `owner`; what Open chose.
pub fn find(owner: HWND, host: &dyn Host, look: Look) -> Option<Chosen> {
    let state = Find {
        host,
        look: Cell::new(look),
        generation: Cell::new(u64::MAX),
        all: RefCell::new(Vec::new()),
        shown: RefCell::new(Vec::new()),
        selected_row: Cell::new(None),
        min: Cell::new((0, 0)),
        chosen: RefCell::new(None),
    };
    let (cx, cy) = (300, 220);
    let mut t = Template::new("Select Bookmark", WS_THICKFRAME | WS_MAXIMIZEBOX, cx, cy);
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
            WS_TABSTOP | LVS_REPORT | LVS_OWNERDATA | LVS_SHOWSELALWAYS | LVS_SINGLESEL,
            WS_EX_CLIENTEDGE,
            (7, 24, cx - 14, cy - 52),
        );
    for (id, text, style) in [
        (ID_OPEN, "&Open", WS_TABSTOP | BS_DEFPUSHBUTTON as u32),
        (ID_COPY, "&Copy Search", WS_TABSTOP),
        (CANCEL, "Close", WS_TABSTOP),
    ] {
        t.control(Class::Button, text, id, style, 0, (0, 0, 50, 14));
    }
    t.show(owner, &|m| find_proc(&state, m));
    state.chosen.into_inner()
}

impl Find<'_> {
    fn list(dlg: HWND) -> HWND {
        dlg::item(dlg, ID_LIST)
    }

    fn init(&self, dlg: HWND) {
        let list = Find::list(dlg);
        let ex = LVS_EX_FULLROWSELECT | LVS_EX_DOUBLEBUFFER | LVS_EX_LABELTIP;
        lv::set_extended_style(list, ex, ex);
        let dpi = crate::custom::window_dpi(dlg) as i32;
        for (i, (title, width)) in COLUMNS.into_iter().enumerate() {
            let col = LVCOLUMNW {
                mask: LVCF_TEXT | LVCF_WIDTH,
                cx: width * dpi / 96,
                ..Default::default()
            };
            lv::insert_column(list, i as i32, col, Some(title));
        }
        // The list outlives the box (both go with the dialog), and the
        // subclass goes with the box.
        filter_keys(dlg::item(dlg, ID_FILTER), list);
        apply_theme(dlg, self.look.get().dark);
        center_on_owner(dlg);
        let wr = win::window_rect(dlg);
        self.min.set((wr.right - wr.left, wr.bottom - wr.top));
        places::restore(dlg, Which::BookmarkSelect, list, self.min.get());
        self.layout(dlg);
        dlg::set_def_id(dlg, ID_OPEN);
        self.host.showing(dlg);
        self.reload(dlg);
    }

    fn layout(&self, dlg: HWND) {
        let px = |x: i32, y: i32| {
            let r = dlg::map_rect(
                dlg,
                RECT {
                    left: 0,
                    top: 0,
                    right: x,
                    bottom: y,
                },
            );
            (r.right, r.bottom)
        };
        let rc = win::client_rect(dlg);
        let (w, h) = (rc.right, rc.bottom);
        let (m, _) = px(7, 7);
        let (_, my) = px(0, 7);
        let (bw, bh) = px(50, 14);
        let (wide, _) = px(60, 0);
        let (gap, _) = px(4, 0);
        let (label, top) = px(29, 24);
        let (_, edit_h) = px(0, 12);
        let place = |id: u16, x: i32, y: i32, cw: i32, ch: i32| {
            win::move_window(dlg::item(dlg, id), x, y, cw.max(0), ch.max(0), true);
        };
        place(ID_FILTER, m + label, my, w - 2 * m - label, edit_h);
        let by = h - my - bh;
        place(ID_LIST, m, top, w - 2 * m, by - my - top);
        // The Search column takes what the Name column leaves (again in
        // `reshow`, once the rows are in).
        fill_last_column(Find::list(dlg));
        let mut x = w - m;
        for (id, width) in [(CANCEL, bw), (ID_COPY, wide), (ID_OPEN, bw)] {
            x -= width;
            place(id, x, by, width, bh);
            x -= gap;
        }
    }

    /// Reads the bookmarks again, keeping the selected one.
    fn reload(&self, dlg: HWND) {
        let keep = self.current().map(|i| self.all.borrow()[i].label.clone());
        let (list, generation) = self.host.bookmarks();
        self.generation.set(generation);
        *self.all.borrow_mut() = entries(&list);
        self.reshow(dlg, keep.as_deref());
    }

    fn reshow(&self, dlg: HWND, keep: Option<&str>) {
        let filter = dlg::item_text(dlg, ID_FILTER);
        let (n, select) = {
            let all = self.all.borrow();
            let rows = view(&all, &filter);
            let select = keep
                .and_then(|k| rows.iter().position(|&i| all[i].label == k))
                .or((!rows.is_empty()).then_some(0));
            let n = rows.len();
            *self.shown.borrow_mut() = rows;
            (n, select)
        };
        // The list's notifications come back into this dialog: no borrow
        // is held from here on.
        let list = Find::list(dlg);
        lv::set_item_count(list, n, 0);
        // The rows may have brought the vertical scroll bar (or taken it
        // away): the Search column fills what the list has now.
        fill_last_column(list);
        let state = |row: i32, on: u32| {
            lv::set_item_state(list, row, on, LVIS_SELECTED | LVIS_FOCUSED);
        };
        state(-1, 0);
        if let Some(i) = select {
            state(i as i32, LVIS_SELECTED | LVIS_FOCUSED);
            lv::set_selection_mark(list, i as i32);
            lv::ensure_visible(list, i as i32, false);
        }
        win::invalidate(list, None, true);
        self.update_buttons(dlg);
    }

    /// The selected entry.
    fn current(&self) -> Option<usize> {
        self.selected_row
            .get()
            .and_then(|row| self.shown.borrow().get(row).copied())
    }

    fn update_buttons(&self, dlg: HWND) {
        let row = lv::next_item(Find::list(dlg), -1, LVNI_SELECTED);
        self.selected_row.set(usize::try_from(row).ok());
        let any = self.current().is_some();
        enable(dlg, ID_OPEN, any);
        enable(dlg, ID_COPY, any);
    }

    fn open(&self, dlg: HWND) {
        let Some(i) = self.current() else { return };
        *self.chosen.borrow_mut() = Some(self.all.borrow()[i].chosen.clone());
        dlg::end(dlg, 1);
    }

    fn copy(&self) {
        let Some(i) = self.current() else { return };
        let all = self.all.borrow();
        let c = &all[i].chosen;
        let text = c.search.clone().unwrap_or_default();
        drop(all);
        // The host says so when the clipboard is busy.
        self.host.copy(&text);
    }

    fn disp_info(&self, info: &mut View<'_, NMLVDISPINFOW>) {
        if info.item.mask & LVIF_TEXT == 0
            || info.item.pszText.is_null()
            || info.item.cchTextMax <= 0
        {
            return;
        }
        let text: Vec<u16> = {
            let shown = self.shown.borrow();
            let all = self.all.borrow();
            let entry = usize::try_from(info.item.iItem)
                .ok()
                .and_then(|row| shown.get(row))
                .and_then(|&i| all.get(i));
            match (entry, info.item.iSubItem) {
                (Some(e), 0) => e.label.encode_utf16().collect(),
                (Some(e), 1) => e.search.encode_utf16().collect(),
                _ => Vec::new(),
            }
        };
        info.set_text(&text);
    }

    /// Answers the list's notifications; `Some` is DWLP_MSGRESULT's value.
    fn notify(&self, dlg: HWND, n: &Notify) -> Option<isize> {
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
            // No type-ahead: the filter box is for finding.
            LVN_ODFINDITEMW => return Some(-1),
            LVN_ITEMCHANGED | LVN_ODSTATECHANGED => self.update_buttons(dlg),
            NM_DBLCLK => {
                if n.get::<NMITEMACTIVATE>().is_some_and(|a| a.iItem >= 0) {
                    self.open(dlg);
                }
            }
            LVN_KEYDOWN => {
                let ctrl = sys::key_down(VK_CONTROL);
                if ctrl && n.get::<NMLVKEYDOWN>().is_some_and(|k| k.wVKey == 0x43) {
                    self.copy();
                }
            }
            _ => {}
        }
        None
    }
}

fn find_proc(s: &Find<'_>, m: &Msg) -> isize {
    let dlg = m.hwnd;
    if let Some(answer) = common_msg(m, &s.look) {
        return answer;
    }
    match m.id {
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
            if let Some(mut mmi) = m.min_max_info() {
                mmi.ptMinTrackSize.x = w;
                mmi.ptMinTrackSize.y = h;
            }
            1
        }
        WM_NOTIFY => {
            let Some(n) = m.notify() else {
                return 0;
            };
            if n.id() != usize::from(ID_LIST) {
                return 0;
            }
            if let Some(answer) = s.notify(dlg, &n) {
                dlg::set_result(dlg, answer);
            }
            1
        }
        WM_COMMAND => {
            let (id, code) = (win::loword(m.w), u32::from(win::hiword(m.w)));
            match id {
                // Enter is Open, the default button.
                ID_OPEN | OK => s.open(dlg),
                ID_COPY => s.copy(),
                ID_FILTER if code == EN_CHANGE => {
                    let keep = s.current().map(|i| s.all.borrow()[i].label.clone());
                    s.reshow(dlg, keep.as_deref());
                }
                CANCEL => dlg::end(dlg, 0),
                _ => return 0,
            }
            1
        }
        WM_REFRESH => {
            if s.host.bookmarks().1 != s.generation.get() {
                s.reload(dlg);
            }
            1
        }
        WM_DESTROY => {
            places::keep(dlg, Which::BookmarkSelect, Find::list(dlg));
            s.host.showing(core::ptr::null_mut());
            0
        }
        _ => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bookmarks::tree::tests::list;

    #[test]
    fn every_bookmark_with_its_folders_and_a_filter() {
        let l = list(&[
            ("b", "", "Notes"),
            ("f", "", "Music"),
            ("b", "Music/Rock", "Queen"),
            ("-", "", ""),
            ("b", "Music", "Abba"),
        ]);
        let all = entries(&l);
        let labels: Vec<&str> = all.iter().map(|e| e.label.as_str()).collect();
        assert_eq!(labels, ["Notes", "Music/Rock/Queen", "Music/Abba"]);
        assert_eq!(all[1].chosen.search.as_deref(), Some("Queen"));
        // Name with its folders, or search; any case.
        assert_eq!(view(&all, "music"), [1, 2]);
        assert_eq!(view(&all, "QUEEN"), [1]);
        assert_eq!(view(&all, "rock/q"), [1]);
        assert!(view(&all, "zzz").is_empty());
        assert_eq!(view(&all, ""), [0, 1, 2]);
    }
}
