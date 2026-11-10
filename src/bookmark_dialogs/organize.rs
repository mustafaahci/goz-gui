//! Bookmarks > Organize Bookmarks... (Ctrl+Shift+B, Ctrl+Shift+O), laid
//! out as Everything's: the folders as a tree on the left ("Bookmarks" at
//! the top), a Search box, and the list of what the chosen folder holds
//! (folders, bookmarks, separators, in their order), `Name | Search`.
//! Typing in the Search box lists every bookmark whose name (with its
//! folders) or search holds the text, from all folders.
//!
//! Buttons: New... (a bookmark in the folder shown), New Folder..., Edit...
//! (a bookmark in the editor, a folder's name), Move Up, Move Down (within
//! its folder), Move To... (into another folder), Delete (asks; a folder
//! goes with everything in it), Import... (merges a `Bookmarks.csv` of
//! Everything's or goz-gui's), Export... (all, or the selection only) and
//! Close. They act on the list's selection, or on the folder chosen in the
//! tree when nothing in the list is selected. In the list: Enter and a
//! double-click open a folder or edit a bookmark, Delete deletes, F2
//! edits, Backspace goes up a folder, Ctrl+A selects all. In the tree:
//! Delete and F2 delete or rename the folder. Each change is made at once
//! and the window writes the file.

use std::cell::{Cell, RefCell};
use std::sync::Arc;

use windows_sys::Win32::Foundation::{HWND, LPARAM, RECT, WPARAM};
use windows_sys::Win32::Graphics::Gdi::InvalidateRect;
use windows_sys::Win32::UI::Controls::{
    HTREEITEM, I_IMAGENONE, LVCF_TEXT, LVCF_WIDTH, LVCOLUMNW, LVIF_IMAGE, LVIF_TEXT, LVIS_FOCUSED,
    LVIS_SELECTED, LVITEMW, LVM_ENSUREVISIBLE, LVM_GETNEXTITEM, LVM_INSERTCOLUMNW,
    LVM_SETCOLUMNWIDTH, LVM_SETEXTENDEDLISTVIEWSTYLE, LVM_SETIMAGELIST, LVM_SETITEMCOUNT,
    LVM_SETITEMSTATE, LVM_SETSELECTIONMARK, LVN_GETDISPINFOW, LVN_ITEMCHANGED, LVN_KEYDOWN,
    LVN_ODFINDITEMW, LVN_ODSTATECHANGED, LVNI_SELECTED, LVS_EX_DOUBLEBUFFER, LVS_EX_FULLROWSELECT,
    LVS_EX_LABELTIP, LVS_OWNERDATA, LVS_REPORT, LVS_SHAREIMAGELISTS, LVS_SHOWSELALWAYS,
    LVSCW_AUTOSIZE_USEHEADER, LVSIL_SMALL, NM_CUSTOMDRAW, NM_DBLCLK, NMHDR, NMITEMACTIVATE,
    NMLVCUSTOMDRAW, NMLVDISPINFOW, NMLVKEYDOWN, NMTVKEYDOWN, TVM_SETEXTENDEDSTYLE,
    TVM_SETIMAGELIST, TVN_KEYDOWN, TVN_SELCHANGEDW, TVS_EX_DOUBLEBUFFER, TVS_HASBUTTONS,
    TVS_HASLINES, TVS_LINESATROOT, TVS_SHOWSELALWAYS, TVSIL_NORMAL,
};
use windows_sys::Win32::UI::HiDpi::GetDpiForWindow;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    GetFocus, GetKeyState, IsWindowEnabled, VK_BACK, VK_CONTROL, VK_DELETE, VK_F2,
};
use windows_sys::Win32::UI::Shell::SetWindowSubclass;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    DWLP_MSGRESULT, EN_CHANGE, ES_AUTOHSCROLL, EndDialog, GWLP_USERDATA, GetClientRect,
    GetWindowRect, IDCANCEL, IDOK, MINMAXINFO, MapDialogRect, MoveWindow, SetWindowLongPtrW,
    WM_COMMAND, WM_DESTROY, WM_GETMINMAXINFO, WM_INITDIALOG, WM_NEXTDLGCTL, WM_NOTIFY, WM_SIZE,
    WS_EX_CLIENTEDGE, WS_MAXIMIZEBOX, WS_TABSTOP, WS_THICKFRAME,
};

use super::pick::{folder_name, pick_folder};
use super::{
    Host, Icons, WM_REFRESH, ask, edit, enable, fill_tree, folder_name_problem, icons, inform,
    item, send, set_text, tree_select, tree_selected, unique_name, warn,
};
use crate::bookmarks::ops::{Id, Op, Target};
use crate::bookmarks::tree::{Item, TOP, Tree};
use crate::bookmarks::{Bookmark, Bookmarks, FolderPath, Kind, one_line, same_path, within};
use crate::dialog::{
    Class, Look, Template, apply_theme, center_on_owner, common, list_custom_draw, state,
};
use crate::filedlg::{self, Check};
use crate::history_dialogs::filter_proc;
use crate::places::{self, Which};
use crate::store;
use crate::wide::{window_text, wz};

const STATIC: u16 = 0xffff;
const ID_TREE: u16 = 100;
const ID_FILTER: u16 = 101;
const ID_LIST: u16 = 102;
const ID_NEW: u16 = 103;
const ID_NEW_FOLDER: u16 = 104;
const ID_EDIT: u16 = 105;
const ID_UP: u16 = 106;
const ID_DOWN: u16 = 107;
const ID_MOVE_TO: u16 = 108;
const ID_DELETE: u16 = 109;
const ID_IMPORT: u16 = 110;
const ID_EXPORT: u16 = 111;
const OK: u16 = IDOK as u16;
const CANCEL: u16 = IDCANCEL as u16;

/// The buttons at the right, top to bottom, with their top edges in dialog
/// units (gaps between the groups, as Everything's).
const BUTTONS: [(u16, &str, i32); 9] = [
    (ID_NEW, "&New...", 7),
    (ID_NEW_FOLDER, "New &Folder...", 25),
    (ID_EDIT, "&Edit...", 49),
    (ID_UP, "Mo&ve Up", 67),
    (ID_DOWN, "&Move Down", 85),
    (ID_MOVE_TO, "Move &To...", 103),
    (ID_DELETE, "&Delete", 121),
    (ID_IMPORT, "&Import...", 145),
    (ID_EXPORT, "E&xport...", 163),
];
/// The list's columns: title and width at 96 DPI.
const COLUMNS: [(&str, i32); 2] = [("Name", 160), ("Search", 220)];
/// The tree's width at 96 DPI (Everything's `bookmark_organize_tree_wide`
/// is 98; a little more shows longer folder names).
const TREE_WIDTH: i32 = 140;
/// What the list shows for a separator, as Everything's.
const SEPARATOR: &str = "(Separator)";
/// The name Export suggests: not the live file's own, which the dialog
/// would offer to replace when it opens in the data folder.
const EXPORT_NAME: &str = "Bookmarks export.csv";

/// What to select once the list is shown again.
enum Keep {
    /// These, where they are now.
    Targets(Vec<Target>),
    /// The last row of the list (the one just added).
    Last,
    /// Row `n` of the bookmarks (edited in place).
    Row(usize),
    /// Row `n` of the list (kept in range); `None`: nothing.
    At(Option<usize>),
}

/// The question Delete asks, and its title (also the Bookmarks pane's).
pub(super) fn delete_question(
    list: &Bookmarks,
    tree: &Tree,
    items: &[Item],
) -> (&'static str, String) {
    match items {
        [Item::Row(r)] if list.rows()[*r].kind() == Kind::Separator => {
            ("Delete Separator", "Delete this separator?".into())
        }
        [Item::Row(r)] => (
            "Delete Bookmark",
            format!(
                "Are you sure you want to delete this bookmark?\n\n{}",
                one_line(list.rows()[*r].name())
            ),
        ),
        [Item::Folder(f)] => {
            let (bookmarks, folders) = tree.count_below(list, *f);
            let mut text = format!(
                "Delete the folder \"{}\" and everything in it?",
                tree.nodes[*f].name()
            );
            if bookmarks + folders > 0 {
                text.push_str(&format!(
                    "\n\nIt holds {} and {}.",
                    plural(bookmarks, "bookmark"),
                    plural(folders, "folder")
                ));
            }
            ("Delete Folder", text)
        }
        _ => {
            let mut text = format!("Delete the {} selected items?", items.len());
            if items.iter().any(|i| matches!(i, Item::Folder(_))) {
                text.push_str("\n\nA folder goes with everything in it.");
            }
            ("Delete Bookmarks", text)
        }
    }
}

fn plural(n: usize, what: &str) -> String {
    if n == 1 {
        format!("1 {what}")
    } else {
        format!("{n} {what}s")
    }
}

/// The rows the list shows: what folder `node` holds, or, with a `filter`,
/// every bookmark whose name (with its folders) or search holds it.
fn rows_shown(list: &Bookmarks, tree: &Tree, node: usize, filter: &str) -> Vec<Item> {
    if filter.is_empty() {
        return tree.nodes[node].items.clone();
    }
    let needle = filter.to_lowercase();
    tree.bookmarks_below(list, TOP)
        .into_iter()
        .filter(|&r| {
            let row = &list.rows()[r];
            label(row).to_lowercase().contains(&needle)
                || row.search().to_lowercase().contains(&needle)
        })
        .map(Item::Row)
        .collect()
}

/// A bookmark's name with its folders: `Music/Rock/Queen`.
fn label(row: &Bookmark) -> String {
    let mut path = row.folder();
    path.push(one_line(row.name()));
    path.join("/")
}

struct Organize<'a> {
    host: &'a dyn Host,
    look: Cell<Look>,
    icons: Icons,
    generation: Cell<u64>,
    list: RefCell<Arc<Bookmarks>>,
    tree: RefCell<Tree>,
    /// The tree view's items, by node.
    made: RefCell<Vec<(usize, HTREEITEM)>>,
    /// The folder shown.
    folder: RefCell<FolderPath>,
    /// The list's rows.
    shown: RefCell<Vec<Item>>,
    /// The list shows a search through every folder.
    filtering: Cell<bool>,
    /// The tree is being filled: its selection changes are goz-gui's.
    filling: Cell<bool>,
    min: Cell<(i32, i32)>,
}

/// Shows Organize Bookmarks over `owner` until it closes.
pub fn organize(owner: HWND, host: &dyn Host, look: Look) {
    let (list, _) = host.bookmarks();
    let state = Organize {
        host,
        look: Cell::new(look),
        icons: icons(),
        generation: Cell::new(u64::MAX),
        tree: RefCell::new(Tree::build(&list)),
        list: RefCell::new(list),
        made: RefCell::new(Vec::new()),
        folder: RefCell::new(Vec::new()),
        shown: RefCell::new(Vec::new()),
        filtering: Cell::new(false),
        filling: Cell::new(false),
        min: Cell::new((0, 0)),
    };
    let (cx, cy) = (380, 240);
    let list_style =
        WS_TABSTOP | LVS_REPORT | LVS_OWNERDATA | LVS_SHOWSELALWAYS | LVS_SHAREIMAGELISTS;
    let tree_style =
        WS_TABSTOP | TVS_HASBUTTONS | TVS_HASLINES | TVS_LINESATROOT | TVS_SHOWSELALWAYS;
    let mut t = Template::new("Organize Bookmarks", WS_THICKFRAME | WS_MAXIMIZEBOX, cx, cy);
    // The places are set by `layout`; these are the first. Tab order, as
    // Everything's: the search box, the list, the buttons, the tree.
    t.control(Class::Static, "&Search:", STATIC, 0, 0, (93, 9, 28, 8))
        .control(
            Class::Edit,
            "",
            ID_FILTER,
            WS_TABSTOP | ES_AUTOHSCROLL as u32,
            WS_EX_CLIENTEDGE,
            (122, 7, 190, 12),
        )
        .control(
            Class::Named("SysListView32"),
            "",
            ID_LIST,
            list_style,
            WS_EX_CLIENTEDGE,
            (93, 24, 219, cy - 31),
        );
    for (id, text, top) in BUTTONS {
        t.control(
            Class::Button,
            text,
            id,
            WS_TABSTOP,
            0,
            (cx - 63, top as i16, 56, 14),
        );
    }
    t.control(
        Class::Button,
        "Close",
        CANCEL,
        WS_TABSTOP,
        0,
        (cx - 63, cy - 21, 56, 14),
    )
    .control(
        Class::Named("SysTreeView32"),
        "",
        ID_TREE,
        tree_style,
        WS_EX_CLIENTEDGE,
        (7, 7, 80, cy - 14),
    );
    t.run(owner, Some(organize_proc), (&raw const state) as LPARAM);
}

impl Organize<'_> {
    fn list_view(dlg: HWND) -> HWND {
        item(dlg, ID_LIST)
    }

    fn tree_view(dlg: HWND) -> HWND {
        item(dlg, ID_TREE)
    }

    fn init(&self, dlg: HWND) {
        let list = Organize::list_view(dlg);
        let ex = LVS_EX_FULLROWSELECT | LVS_EX_DOUBLEBUFFER | LVS_EX_LABELTIP;
        send(list, LVM_SETEXTENDEDLISTVIEWSTYLE, ex as usize, ex as isize);
        send(
            list,
            LVM_SETIMAGELIST,
            LVSIL_SMALL as usize,
            self.icons.list,
        );
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
            send(list, LVM_INSERTCOLUMNW, i, (&raw const col) as isize);
        }
        let tree = Organize::tree_view(dlg);
        send(
            tree,
            TVM_SETIMAGELIST,
            TVSIL_NORMAL as usize,
            self.icons.list,
        );
        send(
            tree,
            TVM_SETEXTENDEDSTYLE,
            TVS_EX_DOUBLEBUFFER as usize,
            TVS_EX_DOUBLEBUFFER as isize,
        );
        // SAFETY: subclasses this thread's filter box; the list outlives it
        // (both go with the dialog), and the subclass removes itself.
        unsafe { SetWindowSubclass(item(dlg, ID_FILTER), Some(filter_proc), 1, list as usize) };
        apply_theme(dlg, self.look.get().dark);
        center_on_owner(dlg);
        let mut wr = RECT::default();
        // SAFETY: reads this thread's dialog.
        unsafe { GetWindowRect(dlg, &mut wr) };
        self.min.set((wr.right - wr.left, wr.bottom - wr.top));
        places::restore(dlg, Which::BookmarkOrganize, list, self.min.get());
        self.layout(dlg);
        self.host.showing(dlg);
        self.load(dlg, Keep::At(Some(0)));
    }

    /// Places the controls for the dialog's size: the tree at the left, the
    /// search box and the list taking the room, the buttons at the right.
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
        // SAFETY: reads this thread's dialog and its DPI.
        let dpi = unsafe {
            GetClientRect(dlg, &mut rc);
            GetDpiForWindow(dlg).max(96) as i32
        };
        let (w, h) = (rc.right, rc.bottom);
        let (m, my) = px(7, 7);
        let (bw, bh) = px(56, 14);
        let (gap, _) = px(6, 0);
        let (label, top) = px(29, 24);
        let (_, edit_h) = px(0, 12);
        let (_, label_y) = px(0, 9);
        let (_, label_h) = px(0, 8);
        let place = |id: u16, x: i32, y: i32, cw: i32, ch: i32| {
            let hwnd = item(dlg, id);
            if !hwnd.is_null() {
                // SAFETY: moves a control of this thread's dialog.
                unsafe { MoveWindow(hwnd, x, y, cw.max(0), ch.max(0), 1) };
            }
        };
        let tree_w = TREE_WIDTH * dpi / 96;
        place(ID_TREE, m, my, tree_w, h - 2 * my);
        let lx = m + tree_w + gap;
        let bx = w - m - bw;
        let lw = bx - gap - lx;
        // The Search label is the only static control.
        place(STATIC, lx, label_y, label, label_h);
        place(ID_FILTER, lx + label, my, lw - label, edit_h);
        place(ID_LIST, lx, top, lw, h - my - top);
        // The Search column takes what the Name column leaves.
        send(
            item(dlg, ID_LIST),
            LVM_SETCOLUMNWIDTH,
            1,
            LVSCW_AUTOSIZE_USEHEADER as isize,
        );
        for (id, _, y) in BUTTONS {
            place(id, bx, px(0, y).1, bw, bh);
        }
        place(CANCEL, bx, h - my - bh, bw, bh);
    }

    /// Reads the bookmarks from the window again and shows them.
    fn load(&self, dlg: HWND, keep: Keep) {
        let (list, generation) = self.host.bookmarks();
        self.generation.set(generation);
        let tree = Tree::build(&list);
        // The folder shown, or the nearest one above it still there.
        let mut folder = self.folder.borrow().clone();
        while !folder.is_empty() && tree.find(&folder).is_none() {
            folder.pop();
        }
        let node = tree.find(&folder).unwrap_or(TOP);
        *self.folder.borrow_mut() = tree.nodes[node].path.clone();
        *self.list.borrow_mut() = list;
        *self.tree.borrow_mut() = tree;
        self.fill_tree(dlg);
        self.reshow(dlg, keep);
    }

    fn node(&self) -> usize {
        self.tree
            .borrow()
            .find(&self.folder.borrow())
            .unwrap_or(TOP)
    }

    fn fill_tree(&self, dlg: HWND) {
        // The tree's notifications come back into this dialog: it works on
        // a copy, and its selection changes are ignored meanwhile.
        let tree = self.tree.borrow().clone();
        let node = self.node();
        let view = Organize::tree_view(dlg);
        self.filling.set(true);
        let made = fill_tree(view, &tree, &[], self.icons);
        tree_select(view, &made, node);
        self.filling.set(false);
        *self.made.borrow_mut() = made;
    }

    /// Shows the folder (or the search) with `keep` selected.
    fn reshow(&self, dlg: HWND, keep: Keep) {
        let filter = window_text(item(dlg, ID_FILTER));
        let (items, select) = {
            let list = self.list.borrow();
            let tree = self.tree.borrow();
            let items = rows_shown(&list, &tree, self.node(), &filter);
            let wanted: Vec<Target> = match &keep {
                Keep::Targets(t) => t.clone(),
                Keep::Last => match list.rows().last() {
                    Some(r) if r.kind() == Kind::Folder => vec![Target::Folder(r.place())],
                    Some(_) => vec![Target::Row(Id::of(&list, list.len() - 1))],
                    None => Vec::new(),
                },
                Keep::Row(r) if *r < list.len() => vec![Target::Row(Id::of(&list, *r))],
                Keep::Row(_) => Vec::new(),
                Keep::At(_) => Vec::new(),
            };
            // Where each is now, then which rows those are.
            let found: Vec<Item> = wanted.iter().filter_map(|t| t.item(&list, &tree)).collect();
            let mut select: Vec<usize> = items
                .iter()
                .enumerate()
                .filter(|(_, i)| found.contains(i))
                .map(|(n, _)| n)
                .collect();
            if let (Keep::At(Some(at)), false) = (&keep, items.is_empty()) {
                select.push((*at).min(items.len() - 1));
            }
            (items, select)
        };
        self.filtering.set(!filter.is_empty());
        let n = items.len();
        *self.shown.borrow_mut() = items;
        // The list's notifications come back into this dialog: no borrow
        // is held from here on.
        let list = Organize::list_view(dlg);
        send(list, LVM_SETITEMCOUNT, n, 0);
        let set = |row: usize, on: u32, mask: u32| {
            let it = LVITEMW {
                state: on,
                stateMask: mask,
                // SAFETY: the rest of LVITEMW is plain data.
                ..unsafe { core::mem::zeroed() }
            };
            send(list, LVM_SETITEMSTATE, row, (&raw const it) as isize);
        };
        set(usize::MAX, 0, LVIS_SELECTED | LVIS_FOCUSED);
        for &row in &select {
            set(row, LVIS_SELECTED, LVIS_SELECTED);
        }
        if let Some(&first) = select.first() {
            set(first, LVIS_FOCUSED, LVIS_FOCUSED);
            send(list, LVM_SETSELECTIONMARK, 0, first as isize);
            send(list, LVM_ENSUREVISIBLE, first, 0);
        }
        // SAFETY: repaints this thread's list.
        unsafe { InvalidateRect(list, core::ptr::null(), 1) };
        self.update_buttons(dlg);
    }

    /// The list's selected rows, top to bottom.
    fn selected(&self, dlg: HWND) -> Vec<Item> {
        let list = Organize::list_view(dlg);
        let mut rows = Vec::new();
        let mut at = usize::MAX;
        loop {
            let next = send(list, LVM_GETNEXTITEM, at, LVNI_SELECTED as isize);
            let Ok(row) = usize::try_from(next) else {
                break;
            };
            rows.push(row);
            at = row;
        }
        let shown = self.shown.borrow();
        rows.into_iter()
            .filter_map(|row| shown.get(row).copied())
            .collect()
    }

    /// What the buttons act on: the list's selection, else the folder the
    /// tree has chosen (not the top).
    fn acting(&self, dlg: HWND) -> Vec<Item> {
        let selected = self.selected(dlg);
        if !selected.is_empty() {
            return selected;
        }
        match self.node() {
            TOP => Vec::new(),
            node => vec![Item::Folder(node)],
        }
    }

    /// Whether `item` has a neighbour before it (`up`) or after it in its
    /// folder.
    fn can_move(&self, item: Item, up: bool) -> bool {
        let tree = self.tree.borrow();
        tree.parent_of(item).is_some_and(|(parent, at)| {
            if up {
                at > 0
            } else {
                at + 1 < tree.nodes[parent].items.len()
            }
        })
    }

    fn is_separator(&self, item: Item) -> bool {
        matches!(item, Item::Row(r) if self.list.borrow().rows()[r].kind() == Kind::Separator)
    }

    /// Greys what the selection does not allow.
    fn update_buttons(&self, dlg: HWND) {
        let acting = self.acting(dlg);
        let one = match acting.as_slice() {
            [only] => Some(*only),
            _ => None,
        };
        // Moving within a folder means nothing in a search's list.
        let from_list = !self.selected(dlg).is_empty();
        let movable = one.filter(|_| !(from_list && self.filtering.get()));
        enable(dlg, ID_EDIT, one.is_some_and(|i| !self.is_separator(i)));
        enable(dlg, ID_DELETE, !acting.is_empty());
        enable(dlg, ID_MOVE_TO, !acting.is_empty());
        enable(dlg, ID_UP, movable.is_some_and(|i| self.can_move(i, true)));
        enable(
            dlg,
            ID_DOWN,
            movable.is_some_and(|i| self.can_move(i, false)),
        );
        enable(dlg, ID_EXPORT, !self.list.borrow().is_empty());
        // A button greyed while it had the keyboard hands it to the list.
        // SAFETY: reads this thread's focus and one of its windows.
        let focus = unsafe { GetFocus() };
        if !focus.is_null() && unsafe { IsWindowEnabled(focus) } == 0 {
            send(dlg, WM_NEXTDLGCTL, Organize::list_view(dlg) as usize, 1);
        }
    }

    fn targets(&self, items: &[Item]) -> Vec<Target> {
        let list = self.list.borrow();
        let tree = self.tree.borrow();
        items.iter().map(|&i| Target::of(&list, &tree, i)).collect()
    }

    /// Makes `op`, then shows the bookmarks again with `keep` selected.
    fn change(&self, dlg: HWND, op: Op, keep: Keep) -> bool {
        let changed = self.host.change(op);
        self.load(dlg, keep);
        changed
    }

    /// Shows folder `node` (a search is cleared).
    fn open_folder(&self, dlg: HWND, node: usize) {
        *self.folder.borrow_mut() = self.tree.borrow().nodes[node].path.clone();
        if !window_text(item(dlg, ID_FILTER)).is_empty() {
            set_text(item(dlg, ID_FILTER), "");
        }
        let view = Organize::tree_view(dlg);
        self.filling.set(true);
        tree_select(view, &self.made.borrow(), node);
        self.filling.set(false);
        self.reshow(dlg, Keep::At(Some(0)));
    }

    fn clear_search(&self, dlg: HWND) {
        if !window_text(item(dlg, ID_FILTER)).is_empty() {
            set_text(item(dlg, ID_FILTER), "");
        }
    }

    fn new_bookmark(&self, dlg: HWND) {
        let folder = self.folder.borrow().clone();
        let mut row = Bookmark::new(&folder, "New Bookmark", "", &self.host.new_settings());
        let Some(fields) = edit(dlg, "New Bookmark", &row.fields(), self.look.get()) else {
            return;
        };
        row.set_fields(&fields);
        self.clear_search(dlg);
        self.change(dlg, Op::Add(row), Keep::Last);
    }

    fn new_folder(&self, dlg: HWND) {
        let tree = self.tree.borrow().clone();
        let node = self.node();
        let folder = tree.nodes[node].path.clone();
        let first = unique_name(&tree, node, "New Folder");
        let problem = |name: &str| folder_name_problem(&tree, node, name, None);
        let Some(name) = folder_name(dlg, "New Folder", &first, &problem, self.look.get()) else {
            return;
        };
        self.clear_search(dlg);
        self.change(
            dlg,
            Op::Add(Bookmark::new_folder(&folder, &name)),
            Keep::Last,
        );
    }

    /// Edit...: a bookmark in the editor, a folder's name.
    fn edit(&self, dlg: HWND, item: Item) {
        match item {
            Item::Row(r) => {
                let row: Bookmark = (*self.list.borrow().rows()[r]).clone();
                if row.kind() != Kind::Bookmark {
                    return;
                }
                let id = Id::of(&self.list.borrow(), r);
                let Some(fields) = edit(dlg, "Edit Bookmark", &row.fields(), self.look.get())
                else {
                    return;
                };
                let mut new = row.clone();
                new.set_fields(&fields);
                if new == row {
                    return;
                }
                self.change(dlg, Op::Edit { id, row: new }, Keep::Row(r));
            }
            Item::Folder(f) => self.rename(dlg, f),
        }
    }

    fn rename(&self, dlg: HWND, node: usize) {
        let tree = self.tree.borrow().clone();
        let path = tree.nodes[node].path.clone();
        let Some((old, parent)) = path.split_last() else {
            return;
        };
        let parent_node = tree.find(parent).unwrap_or(TOP);
        let problem = |name: &str| folder_name_problem(&tree, parent_node, name, Some(node));
        let Some(name) = folder_name(dlg, "Rename Folder", old, &problem, self.look.get()) else {
            return;
        };
        if &name == old {
            return;
        }
        let mut new = parent.to_vec();
        new.push(name.clone());
        // The folder shown follows the rename.
        {
            let mut shown = self.folder.borrow_mut();
            if within(&shown, &path) {
                let rest: Vec<String> = shown[path.len()..].to_vec();
                *shown = new.iter().cloned().chain(rest).collect();
            }
        }
        self.change(
            dlg,
            Op::Rename { folder: path, name },
            Keep::Targets(vec![Target::Folder(new)]),
        );
    }

    fn delete(&self, dlg: HWND, items: &[Item]) {
        if items.is_empty() {
            return;
        }
        // Named before asking: while the question is up the list can be
        // read again (another goz-gui wrote the file).
        let targets = self.targets(items);
        let (title, question) = {
            let list = self.list.borrow();
            let tree = self.tree.borrow();
            delete_question(&list, &tree, items)
        };
        let first = usize::try_from(send(
            Organize::list_view(dlg),
            LVM_GETNEXTITEM,
            usize::MAX,
            LVNI_SELECTED as isize,
        ))
        .ok();
        if !ask(dlg, title, &question) {
            return;
        }
        self.change(dlg, Op::Delete(targets), Keep::At(first.or(Some(0))));
    }

    fn move_one(&self, dlg: HWND, up: bool) {
        let acting = self.acting(dlg);
        let [item] = acting.as_slice() else { return };
        let target = self.targets(&[*item]).remove(0);
        self.change(
            dlg,
            Op::Move {
                target: target.clone(),
                up,
            },
            Keep::Targets(vec![target]),
        );
    }

    fn move_to(&self, dlg: HWND) {
        let acting = self.acting(dlg);
        if acting.is_empty() {
            return;
        }
        let targets = self.targets(&acting);
        let skip: Vec<FolderPath> = targets
            .iter()
            .filter_map(|t| match t {
                Target::Folder(p) => Some(p.clone()),
                Target::Row(_) => None,
            })
            .collect();
        let tree = self.tree.borrow().clone();
        let Some(to) = pick_folder(dlg, &tree, &skip, self.node(), self.look.get()) else {
            return;
        };
        // By identity: the list can be read again while the picker is up.
        let names: Vec<(Option<Kind>, String)> = targets
            .iter()
            .map(|t| match t {
                Target::Row(id) => (Some(id.kind), id.name.clone()),
                Target::Folder(path) => (None, path.last().cloned().unwrap_or_default()),
            })
            .collect();
        if !self.host.change(Op::MoveTo {
            targets,
            to: to.clone(),
        }) {
            return;
        }
        // Shown where they went, selected.
        let (list, _) = self.host.bookmarks();
        let keep = names
            .into_iter()
            .filter_map(|(kind, name)| match kind {
                None => {
                    let mut path = to.clone();
                    path.push(name);
                    Some(Target::Folder(path))
                }
                Some(kind) => list
                    .rows()
                    .iter()
                    .rposition(|r| {
                        r.kind() == kind && r.name() == name && same_path(&r.folder(), &to)
                    })
                    .map(|i| Target::Row(Id::of(&list, i))),
            })
            .collect();
        *self.folder.borrow_mut() = to;
        self.clear_search(dlg);
        self.load(dlg, Keep::Targets(keep));
    }

    fn import(&self, dlg: HWND) {
        let Some(path) = filedlg::open(dlg, "Import Bookmarks", filedlg::CSV, "csv") else {
            return;
        };
        let read = std::fs::read(&path)
            .map_err(|e| e.to_string())
            .and_then(|bytes| Bookmarks::parse_import(&bytes));
        match read {
            Ok(other) => {
                let keep = self.targets(&self.selected(dlg));
                if !self.change(dlg, Op::Import(other), Keep::Targets(keep)) {
                    inform(
                        dlg,
                        "Import Bookmarks",
                        &format!("Every bookmark in {} is here already.", path.display()),
                    );
                }
            }
            Err(why) => warn(
                dlg,
                &format!("{} could not be imported: {why}.", path.display()),
            ),
        }
    }

    fn export(&self, dlg: HWND) {
        let selected = self.selected(dlg);
        // Made before asking where: the list can be read again meanwhile.
        let (all, chosen) = {
            let list = self.list.borrow();
            (
                list.to_csv(),
                list.export(&self.tree.borrow(), &selected).to_csv(),
            )
        };
        let check = Check {
            label: "Export selection only",
            checked: false,
            enabled: !selected.is_empty(),
        };
        let Some((path, only)) = filedlg::save(
            dlg,
            "Export Bookmarks",
            EXPORT_NAME,
            filedlg::CSV,
            "csv",
            Some(check),
        ) else {
            return;
        };
        if store::location::is_data_file(&path) {
            // The live file (or its backup) replaced behind the merge's
            // back would lose every bookmark not exported.
            warn(
                dlg,
                &format!(
                    "{} is goz-gui's own file. Export the bookmarks to another file or folder.",
                    path.display()
                ),
            );
            return;
        }
        let text = if only { chosen } else { all };
        if let Err(e) = store::file::write(&path, text.as_bytes(), None) {
            warn(
                dlg,
                &format!(
                    "The bookmarks could not be exported to {}: {e}",
                    path.display()
                ),
            );
        }
    }

    /// Enter, a double-click: a folder opens, a bookmark is edited.
    fn activate(&self, dlg: HWND) {
        match self.acting(dlg).as_slice() {
            [Item::Folder(f)] if !self.selected(dlg).is_empty() => self.open_folder(dlg, *f),
            [only] => self.edit(dlg, *only),
            _ => {}
        }
    }

    fn command(&self, dlg: HWND, id: u16, code: u32) {
        match id {
            OK => self.activate(dlg),
            ID_NEW => self.new_bookmark(dlg),
            ID_NEW_FOLDER => self.new_folder(dlg),
            ID_EDIT => {
                if let [only] = self.acting(dlg).as_slice() {
                    self.edit(dlg, *only);
                }
            }
            ID_DELETE => {
                let acting = self.acting(dlg);
                self.delete(dlg, &acting);
            }
            ID_UP => self.move_one(dlg, true),
            ID_DOWN => self.move_one(dlg, false),
            ID_MOVE_TO => self.move_to(dlg),
            ID_IMPORT => self.import(dlg),
            ID_EXPORT => self.export(dlg),
            ID_FILTER if code == EN_CHANGE => {
                let keep = self.targets(&self.selected(dlg));
                self.reshow(dlg, Keep::Targets(keep));
                if self.selected(dlg).is_empty() {
                    self.reshow(dlg, Keep::At(Some(0)));
                }
            }
            // SAFETY: ends this modal dialog.
            CANCEL => unsafe {
                EndDialog(dlg, 0);
            },
            _ => {}
        }
    }

    fn disp_info(&self, info: &mut NMLVDISPINFOW) {
        let item = usize::try_from(info.item.iItem)
            .ok()
            .and_then(|row| self.shown.borrow().get(row).copied());
        let Some(item) = item else { return };
        let list = self.list.borrow();
        let tree = self.tree.borrow();
        if info.item.mask & LVIF_IMAGE != 0 && info.item.iSubItem == 0 {
            info.item.iImage = match item {
                Item::Folder(_) => self.icons.folder,
                Item::Row(r) if list.rows()[r].kind() == Kind::Bookmark => self.icons.bookmark,
                Item::Row(_) => I_IMAGENONE,
            };
        }
        if info.item.mask & LVIF_TEXT == 0
            || info.item.pszText.is_null()
            || info.item.cchTextMax <= 0
        {
            return;
        }
        let text = match (item, info.item.iSubItem) {
            (Item::Folder(f), 0) => tree.nodes[f].name().to_owned(),
            (Item::Row(r), 0) => {
                let row = &list.rows()[r];
                match row.kind() {
                    Kind::Separator => SEPARATOR.to_owned(),
                    _ if self.filtering.get() => label(row),
                    _ => one_line(row.name()),
                }
            }
            (Item::Row(r), 1) if list.rows()[r].kind() == Kind::Bookmark => {
                one_line(list.rows()[r].search())
            }
            _ => String::new(),
        };
        let text: Vec<u16> = text.encode_utf16().collect();
        let n = text.len().min(info.item.cchTextMax as usize - 1);
        // SAFETY: the list gave a buffer of `cchTextMax` code units.
        unsafe {
            core::ptr::copy_nonoverlapping(text.as_ptr(), info.item.pszText, n);
            *info.item.pszText.add(n) = 0;
        }
    }

    /// A key pressed in the list.
    fn list_key(&self, dlg: HWND, vk: u16) {
        // SAFETY: reads this thread's keyboard state.
        let ctrl = unsafe { GetKeyState(i32::from(VK_CONTROL)) } < 0;
        match (vk, ctrl) {
            (VK_DELETE, false) => {
                let selected = self.selected(dlg);
                self.delete(dlg, &selected);
            }
            (VK_F2, false) => {
                if let [only] = self.selected(dlg).as_slice() {
                    self.edit(dlg, *only);
                }
            }
            (VK_BACK, false) if !self.filtering.get() => {
                let node = self.node();
                if node != TOP {
                    let parent = {
                        let tree = self.tree.borrow();
                        let path = &tree.nodes[node].path;
                        tree.find(&path[..path.len() - 1]).unwrap_or(TOP)
                    };
                    self.open_folder(dlg, parent);
                }
            }
            (0x41, true) => {
                let it = LVITEMW {
                    state: LVIS_SELECTED,
                    stateMask: LVIS_SELECTED,
                    // SAFETY: the rest of LVITEMW is plain data.
                    ..unsafe { core::mem::zeroed() }
                };
                send(
                    Organize::list_view(dlg),
                    LVM_SETITEMSTATE,
                    usize::MAX,
                    (&raw const it) as isize,
                );
            }
            _ => {}
        }
    }

    /// A key pressed in the tree: Delete and F2 act on its folder.
    fn tree_key(&self, dlg: HWND, vk: u16) {
        let node = self.node();
        if node == TOP {
            return;
        }
        match vk {
            VK_DELETE => self.delete(dlg, &[Item::Folder(node)]),
            VK_F2 => self.rename(dlg, node),
            _ => {}
        }
    }

    /// Answers a notification; `Some` is DWLP_MSGRESULT's value.
    ///
    /// # Safety
    /// `l` is a WM_NOTIFY's lParam.
    unsafe fn notify(&self, dlg: HWND, l: LPARAM) -> Option<isize> {
        // SAFETY: as the caller promises; each code's struct is read as
        // its control sends it.
        unsafe {
            let hdr = &*(l as *const NMHDR);
            if hdr.idFrom == usize::from(ID_TREE) {
                match hdr.code {
                    TVN_SELCHANGEDW if !self.filling.get() => {
                        if let Some(node) = tree_selected(hdr.hwndFrom) {
                            *self.folder.borrow_mut() = self.tree.borrow().nodes[node].path.clone();
                            self.clear_search(dlg);
                            self.reshow(dlg, Keep::At(Some(0)));
                        }
                    }
                    TVN_KEYDOWN => self.tree_key(dlg, (*(l as *const NMTVKEYDOWN)).wVKey),
                    _ => {}
                }
                return None;
            }
            if hdr.idFrom != usize::from(ID_LIST) {
                return None;
            }
            match hdr.code {
                NM_CUSTOMDRAW => {
                    let cd = &*(l as *const NMLVCUSTOMDRAW);
                    return Some(list_custom_draw(hdr.hwndFrom, cd, self.look.get().dark));
                }
                LVN_GETDISPINFOW => self.disp_info(&mut *(l as *mut NMLVDISPINFOW)),
                // No type-ahead: the search box is for finding.
                LVN_ODFINDITEMW => return Some(-1),
                LVN_ITEMCHANGED | LVN_ODSTATECHANGED => self.update_buttons(dlg),
                NM_DBLCLK => {
                    if (*(l as *const NMITEMACTIVATE)).iItem >= 0 {
                        self.activate(dlg);
                    }
                }
                LVN_KEYDOWN => self.list_key(dlg, (*(l as *const NMLVKEYDOWN)).wVKey),
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
    // SAFETY: GWLP_USERDATA is the `Organize` from `organize`, or null
    // before.
    let Some(s) = (unsafe { state::<Organize<'_>>(dlg) }) else {
        return 0;
    };
    if let Some(answer) = common(dlg, msg, w, l, &s.look) {
        return answer;
    }
    match msg {
        WM_INITDIALOG => {
            s.init(dlg);
            // The dialog manager puts the keyboard in the search box; Up
            // and Down there move in the list, Enter acts on it.
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
            // SAFETY: WM_NOTIFY's lParam is an NMHDR from a control.
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
        WM_REFRESH => {
            if s.host.bookmarks().1 != s.generation.get() {
                let keep = s.targets(&s.selected(dlg));
                s.load(dlg, Keep::Targets(keep));
            }
            1
        }
        WM_DESTROY => {
            places::keep(dlg, Which::BookmarkOrganize, Organize::list_view(dlg));
            s.host.showing(core::ptr::null_mut());
            0
        }
        _ => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bookmarks::split;
    use crate::bookmarks::tree::tests::list;

    fn sample() -> Bookmarks {
        list(&[
            ("b", "", "Notes"),
            ("f", "", "Music"),
            ("b", "Music", "Abba"),
            ("f", "Music", "Rock"),
            ("b", "Music/Rock", "Queen"),
            ("-", "", ""),
        ])
    }

    #[test]
    fn the_list_shows_a_folder_or_a_search() {
        let l = sample();
        let t = Tree::build(&l);
        let music = t.find(&split("Music")).expect("music");
        assert_eq!(rows_shown(&l, &t, TOP, "").len(), 3);
        assert_eq!(
            rows_shown(&l, &t, music, ""),
            [
                Item::Row(2),
                Item::Folder(t.find(&split("Music/Rock")).expect("rock"))
            ]
        );
        // A search goes through every folder: name with folders, search.
        assert_eq!(rows_shown(&l, &t, music, "q"), [Item::Row(4)]);
        assert_eq!(
            rows_shown(&l, &t, TOP, "music/"),
            [Item::Row(2), Item::Row(4)]
        );
        assert_eq!(label(&l.rows()[4]), "Music/Rock/Queen");
    }

    #[test]
    fn delete_says_what_goes() {
        let l = sample();
        let t = Tree::build(&l);
        let music = t.find(&split("Music")).expect("music");
        let (title, text) = delete_question(&l, &t, &[Item::Folder(music)]);
        assert_eq!(title, "Delete Folder");
        assert_eq!(
            text,
            "Delete the folder \"Music\" and everything in it?\n\nIt holds 2 bookmarks and 1 folder."
        );
        assert_eq!(
            delete_question(&l, &t, &[Item::Row(0)]),
            (
                "Delete Bookmark",
                "Are you sure you want to delete this bookmark?\n\nNotes".to_owned()
            )
        );
        assert_eq!(
            delete_question(&l, &t, &[Item::Row(5)]).0,
            "Delete Separator"
        );
        let (_, many) = delete_question(&l, &t, &[Item::Row(0), Item::Folder(music)]);
        assert!(many.starts_with("Delete the 2 selected items?"));
        assert!(many.contains("everything in it"));
    }
}
