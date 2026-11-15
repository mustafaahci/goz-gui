//! Tree-view messages (SysTreeView32), each sent only to a tree view. The
//! item structures go in by value, their text pointers set here.
//!
//! A tree's item (HTREEITEM) is the address of the item's memory in its
//! tree, which comctl32 follows; it checks one only by a signature in that
//! memory, never against the tree it is sent to. So items are [`Item`]s,
//! which only this module makes (from an insert, a query of the tree, a
//! hit test, a notification of the tree), each naming its tree; every
//! function taking one acts on that tree, and only while the item is in
//! it. Which items a tree has is kept here: a subclass of the tree sees
//! every item inserted and deleted (and the tree destroyed), and an item
//! deleted is refused from the moment its deletion starts. A stale item
//! is so refused, unless the tree has since put a new item at the same
//! address: then it names that item of the same tree (wrong, not
//! unsound).

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};

use windows_sys::Win32::Foundation::{COLORREF, HWND, LRESULT, POINT, RECT};
use windows_sys::Win32::UI::Controls::{
    HTREEITEM, NMTREEVIEWW, NMTVGETINFOTIPW, TVE_COLLAPSERESET, TVGN_CARET, TVGN_CHILD,
    TVGN_DROPHILITE, TVGN_NEXT, TVGN_PARENT, TVGN_ROOT, TVHITTESTINFO, TVI_FIRST, TVI_LAST,
    TVI_ROOT, TVIF_EXPANDEDIMAGE, TVIF_INTEGRAL, TVIF_STATEEX, TVIF_TEXT, TVINSERTSTRUCTW,
    TVITEMEXW, TVITEMW, TVM_DELETEITEM, TVM_ENSUREVISIBLE, TVM_EXPAND, TVM_GETITEMRECT,
    TVM_GETITEMW, TVM_GETNEXTITEM, TVM_GETTOOLTIPS, TVM_HITTEST, TVM_INSERTITEMA, TVM_INSERTITEMW,
    TVM_SELECTITEM, TVM_SETBKCOLOR, TVM_SETEXTENDEDSTYLE, TVM_SETIMAGELIST, TVM_SETITEMHEIGHT,
    TVM_SETITEMW, TVM_SETLINECOLOR, TVM_SETTEXTCOLOR,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{WM_DESTROY, WM_NCDESTROY};

use super::imagelist::{self, ImageList, SystemList};
use super::msg::{Msg, View};

/// The class these messages are for.
pub const CLASS: &str = "SysTreeView32";

/// # Safety
/// `w` and `l` are what `msg` defines for a tree view.
unsafe fn send(tv: HWND, msg: u32, w: usize, l: isize) -> isize {
    // SAFETY: as the caller promises, to a tree view (or to null).
    unsafe { super::send_raw(super::of_class(tv, CLASS), msg, w, l) }
}

/// Sends a message whose parameters are integers (no item).
fn send_int(tv: HWND, msg: u32, w: usize, l: isize) -> isize {
    // SAFETY: the callers' messages take integers.
    unsafe { send(tv, msg, w, l) }
}

/// An item of a tree view: the tree and the item in it. Only this module
/// makes one, from what the tree itself answered or notified, and takes
/// one only while it is in its tree (see the module's documentation).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Item {
    tree: HWND,
    h: HTREEITEM,
}

impl Item {
    /// The tree the item is in.
    pub fn tree(self) -> HWND {
        self.tree
    }
}

/// Where among its siblings an item goes ([`insert`]).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum After {
    /// First (TVI_FIRST).
    First,
    /// Last (TVI_LAST).
    Last,
    /// After this item, a child of the same parent.
    Item(Item),
}

/// What is kept of a tree of this thread, from the first time an item of
/// it was asked for (or an image list given it) until it is destroyed.
#[derive(Default)]
struct Tree {
    /// Its items, as it has them; `None` from the moment it is destroyed
    /// (WM_DESTROY, while its items go), when none is taken any more.
    items: Option<HashSet<HTREEITEM>>,
    /// The image lists it shows that goz-gui gave it ([`set_image_list`]):
    /// kept alive (a clone of each) while it does, as a tree never
    /// destroys its lists.
    lists: HashMap<u32, ImageList>,
}

thread_local! {
    /// The trees of this thread kept track of, by window.
    static TREES: RefCell<HashMap<usize, Tree>> = RefCell::new(HashMap::new());
}

static TRACK: super::subclass::Key = super::subclass::Key::new("tree view items and lists");

/// Keeps track of `tree`, if it is a tree view of this thread (from the
/// first call on: its items now are noted, and each one inserted or
/// deleted after, by a subclass that sees every insert, delete and the
/// tree's end); false if it is not.
fn track(tree: HWND) -> bool {
    if !super::class_is(tree, CLASS) || !super::is_this_thread(tree) {
        return false;
    }
    if TREES.with_borrow(|t| t.contains_key(&(tree as usize))) {
        return true;
    }
    // The subclass first, so nothing that changes the tree is missed while
    // its items are read.
    if !super::subclass::set(tree, &TRACK, track_proc) {
        return false;
    }
    let items = walk(tree);
    TREES.with_borrow_mut(|t| {
        t.insert(
            tree as usize,
            Tree {
                items: Some(items),
                lists: HashMap::new(),
            },
        )
    });
    true
}

/// Every item of `tree` now, walked from its first top-level item.
fn walk(tree: HWND) -> HashSet<HTREEITEM> {
    let mut all = HashSet::new();
    let mut todo = vec![next_raw(tree, 0, TVGN_ROOT)];
    while let Some(first) = todo.pop() {
        let mut at = first;
        while at != 0 && all.insert(at) {
            todo.push(next_raw(tree, at, TVGN_CHILD));
            at = next_raw(tree, at, TVGN_NEXT);
        }
    }
    all
}

/// `h` and every item under it, in `tree`, where `h` is in it.
fn subtree(tree: HWND, h: HTREEITEM) -> Vec<HTREEITEM> {
    let mut out = vec![h];
    let mut todo = vec![next_raw(tree, h, TVGN_CHILD)];
    while let Some(first) = todo.pop() {
        let mut at = first;
        while at != 0 {
            out.push(at);
            todo.push(next_raw(tree, at, TVGN_CHILD));
            at = next_raw(tree, at, TVGN_NEXT);
        }
    }
    out
}

/// TVM_GETNEXTITEM of `h`, an item `tree` has (or 0, for the codes that
/// take none), as `code`; 0 for none.
fn next_raw(tree: HWND, h: HTREEITEM, code: u32) -> HTREEITEM {
    // SAFETY: an item the tree has (its own answers, or one noted as in
    // it), or none; the answer is an item of it or 0.
    unsafe { send(tree, TVM_GETNEXTITEM, code as usize, h) }
}

/// Changes the items noted of `tree` (nothing for a tree not kept track of,
/// or one being destroyed).
fn note(tree: HWND, f: impl FnOnce(&mut HashSet<HTREEITEM>)) {
    TREES.with_borrow_mut(|t| {
        if let Some(items) = t.get_mut(&(tree as usize)).and_then(|t| t.items.as_mut()) {
            f(items);
        }
    });
}

/// Whether `h` is an item noted of `tree`.
fn has(tree: HWND, h: HTREEITEM) -> bool {
    h != 0
        && TREES.with_borrow(|t| {
            t.get(&(tree as usize))
                .and_then(|t| t.items.as_ref())
                .is_some_and(|items| items.contains(&h))
        })
}

/// Whether `item` is in its tree (and its tree one of this thread).
fn live(item: Item) -> bool {
    has(item.tree, item.h)
}

/// `h` of `tree` as an [`Item`], if it is one the tree has.
fn item_of(tree: HWND, h: HTREEITEM) -> Option<Item> {
    has(tree, h).then_some(Item { tree, h })
}

/// The subclass that keeps track of a tree: each message passed on as it
/// came; what it changes of the items noted.
fn track_proc(m: &Msg) -> LRESULT {
    let tree = m.hwnd;
    match m.id {
        TVM_INSERTITEMW | TVM_INSERTITEMA => {
            let h = m.def();
            if h != 0 {
                note(tree, |items| {
                    items.insert(h);
                });
            }
            h
        }
        TVM_DELETEITEM => {
            // What goes is refused from now on, as the tree frees it item by
            // item (notifying its parent of each meanwhile).
            let gone: Vec<HTREEITEM> = if m.l == 0 || m.l == TVI_ROOT {
                let all = TREES.with_borrow_mut(|t| {
                    t.get_mut(&(tree as usize))
                        .and_then(|t| t.items.as_mut())
                        .map(core::mem::take)
                });
                all.map(|all| all.into_iter().collect()).unwrap_or_default()
            } else if has(tree, m.l) {
                let gone = subtree(tree, m.l);
                note(tree, |items| {
                    for h in &gone {
                        items.remove(h);
                    }
                });
                gone
            } else {
                // An item not noted (another's code's): what is left after
                // is read again.
                let answer = m.def();
                let now = walk(tree);
                note(tree, |items| items.retain(|h| now.contains(h)));
                return answer;
            };
            let answer = m.def();
            if answer == 0 {
                // Not deleted after all: still in the tree.
                note(tree, |items| items.extend(gone));
            }
            answer
        }
        TVM_EXPAND if m.w & TVE_COLLAPSERESET as usize != 0 => {
            // The item's children go: what is left is read again after.
            let answer = m.def();
            let now = walk(tree);
            note(tree, |items| items.retain(|h| now.contains(h)));
            answer
        }
        WM_DESTROY => {
            // The tree's items go now: none is taken from here on.
            TREES.with_borrow_mut(|t| {
                if let Some(t) = t.get_mut(&(tree as usize)) {
                    t.items = None;
                }
            });
            m.def()
        }
        WM_NCDESTROY => {
            let answer = m.def();
            // The tree is gone: so is what was kept of it, its image lists
            // let go (out of the map first, dropped after).
            let gone = TREES.with_borrow_mut(|t| t.remove(&(tree as usize)));
            drop(gone);
            answer
        }
        _ => m.def(),
    }
}

/// Sends `msg` with item `item` as its lParam to the item's tree, if the
/// item is in it; `None` if it is not.
fn send_item(item: Item, msg: u32, w: usize) -> Option<isize> {
    if !live(item) {
        return None;
    }
    // SAFETY: the callers' messages take an item of the tree as lParam
    // and an integer as wParam; the item is in the tree (`live`).
    Some(unsafe { send(item.tree, msg, w, item.h) })
}

/// The tree's first top-level item.
pub fn root(tree: HWND) -> Option<Item> {
    if !track(tree) {
        return None;
    }
    item_of(tree, next_raw(tree, 0, TVGN_ROOT))
}

/// The tree's selected item.
pub fn selection(tree: HWND) -> Option<Item> {
    if !track(tree) {
        return None;
    }
    item_of(tree, next_raw(tree, 0, TVGN_CARET))
}

/// The item's first child.
pub fn child(item: Item) -> Option<Item> {
    let h = send_item(item, TVM_GETNEXTITEM, TVGN_CHILD as usize)?;
    item_of(item.tree, h)
}

/// The item's next sibling.
pub fn next_sibling(item: Item) -> Option<Item> {
    let h = send_item(item, TVM_GETNEXTITEM, TVGN_NEXT as usize)?;
    item_of(item.tree, h)
}

/// How [`select`] selects an item.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Select {
    /// The selection (TVGN_CARET).
    Caret,
    /// The drop target's highlight (TVGN_DROPHILITE).
    DropHighlight,
}

/// Selects `item` of `tree` as `how` says (`None`: no item); false if it
/// could not, or `item` is not in `tree`.
pub fn select(tree: HWND, how: Select, item: Option<Item>) -> bool {
    let code = match how {
        Select::Caret => TVGN_CARET,
        Select::DropHighlight => TVGN_DROPHILITE,
    };
    match item {
        Some(item) => {
            item.tree == tree
                && send_item(item, TVM_SELECTITEM, code as usize).is_some_and(|r| r != 0)
        }
        // SAFETY: TVM_SELECTITEM of no item: a code and null.
        None => track(tree) && unsafe { send(tree, TVM_SELECTITEM, code as usize, 0) } != 0,
    }
}

/// TVM_EXPAND of `item` with TVE_* `code`.
pub fn expand(item: Item, code: u32) -> bool {
    send_item(item, TVM_EXPAND, code as usize).is_some_and(|r| r != 0)
}

/// Expands and scrolls the tree so `item` shows.
pub fn ensure_visible(item: Item) {
    send_item(item, TVM_ENSUREVISIBLE, 0);
}

/// Deletes `item` and its children.
pub fn delete_item(item: Item) -> bool {
    send_item(item, TVM_DELETEITEM, 0).is_some_and(|r| r != 0)
}

/// Deletes every item of the tree.
pub fn delete_all(tree: HWND) -> bool {
    // SAFETY: TVM_DELETEITEM of TVI_ROOT: every item.
    track(tree) && unsafe { send(tree, TVM_DELETEITEM, 0, TVI_ROOT) } != 0
}

/// Sets the items' height, in pixels (-1: the default).
pub fn set_item_height(tv: HWND, height: i32) {
    send_int(tv, TVM_SETITEMHEIGHT, height as isize as usize, 0);
}

/// The items' text colour.
pub fn set_text_color(tv: HWND, c: COLORREF) {
    send_int(tv, TVM_SETTEXTCOLOR, 0, c as isize);
}

/// The tree's background colour.
pub fn set_bk_color(tv: HWND, c: COLORREF) {
    send_int(tv, TVM_SETBKCOLOR, 0, c as isize);
}

/// The colour of the lines between the items.
pub fn set_line_color(tv: HWND, c: COLORREF) {
    send_int(tv, TVM_SETLINECOLOR, 0, c as isize);
}

/// Sets the TVS_EX_* styles in `mask` to `style`.
pub fn set_extended_style(tv: HWND, mask: u32, style: u32) {
    send_int(tv, TVM_SETEXTENDEDSTYLE, mask as usize, style as isize);
}

/// Gives the tree `tv` (one of this thread) image list `which` (TVSIL_*):
/// `list` (`None`: none). A tree never destroys its lists, so this keeps
/// `list` alive (a clone of it) for as long as the tree shows it: until
/// it is given another, or is destroyed. False if `tv` is not a tree of
/// this thread.
pub fn set_image_list(tv: HWND, which: u32, list: Option<&ImageList>) -> bool {
    if !track(tv) {
        return false;
    }
    let il = list.map_or(0, imagelist::raw_of);
    send_int(tv, TVM_SETIMAGELIST, which as usize, il);
    // The list it showed before, if it was one given here, is let go
    // after the tree no longer shows it.
    let before = keep_list(tv, which, list.cloned());
    drop(before);
    true
}

/// Gives the tree `tv` the shell's system image list `list` as its image
/// list `which` (TVSIL_*): the list lives as long as the process. False if
/// `tv` is not a tree of this thread.
pub fn set_system_image_list(tv: HWND, which: u32, list: SystemList) -> bool {
    if !track(tv) {
        return false;
    }
    send_int(
        tv,
        TVM_SETIMAGELIST,
        which as usize,
        imagelist::raw_of(&list),
    );
    let before = keep_list(tv, which, None);
    drop(before);
    true
}

/// Keeps `list` as what tree `tv` shows as list `which`; the one kept
/// before.
fn keep_list(tv: HWND, which: u32, list: Option<ImageList>) -> Option<ImageList> {
    TREES.with_borrow_mut(|t| {
        let lists = &mut t.entry(tv as usize).or_default().lists;
        match list {
            Some(list) => lists.insert(which, list),
            None => lists.remove(&which),
        }
    })
}

/// The tree's tooltip control.
pub fn tooltips(tv: HWND) -> HWND {
    send_int(tv, TVM_GETTOOLTIPS, 0, 0) as HWND
}

/// The item's rectangle (only its text's, when `text_only`).
pub fn item_rect(item: Item, text_only: bool) -> Option<RECT> {
    if !live(item) {
        return None;
    }
    let mut rc = RECT::default();
    // TVM_GETITEMRECT takes the item in the rectangle's first bytes.
    // SAFETY: a RECT is 16 bytes, room for the handle; the item is in the
    // tree (`live`); written by the tree, read during the call.
    let ok = unsafe {
        (&raw mut rc).cast::<HTREEITEM>().write_unaligned(item.h);
        send(
            item.tree,
            TVM_GETITEMRECT,
            usize::from(text_only),
            (&raw mut rc) as isize,
        )
    } != 0;
    ok.then_some(rc)
}

/// TVM_HITTEST at `pt` (client coordinates): the item there and where on
/// it (TVHT_*).
pub fn hit_test(tree: HWND, pt: POINT) -> (Option<Item>, u32) {
    if !track(tree) {
        return (None, 0);
    }
    let mut ht = TVHITTESTINFO {
        pt,
        ..Default::default()
    };
    // SAFETY: TVM_HITTEST's lParam is a TVHITTESTINFO, in and out.
    unsafe { send(tree, TVM_HITTEST, 0, (&raw mut ht) as isize) };
    (item_of(tree, ht.hItem), ht.flags)
}

/// The TVIF_* fields that are TVITEMEXW's only: a TVITEMW asking for one
/// of them would have the tree read or write past its end.
const EX_ONLY: u32 = TVIF_INTEGRAL | TVIF_STATEEX | TVIF_EXPANDEDIMAGE;

/// The fields `fields.mask` asks for of `item` (TVITEMEXW's own masked
/// off), and its text when TVIF_TEXT is among them (up to `max` units).
/// The answer's hItem is the item's.
pub fn get_item(item: Item, fields: TVITEMW, max: usize) -> Option<(TVITEMW, Option<String>)> {
    if !live(item) {
        return None;
    }
    let mut it = fields;
    it.mask &= !EX_ONLY;
    it.hItem = item.h;
    let mut buf = vec![
        0u16;
        if it.mask & TVIF_TEXT != 0 {
            max.max(1)
        } else {
            0
        }
    ];
    it.pszText = buf.as_mut_ptr();
    it.cchTextMax = buf.len().min(i32::MAX as usize) as i32;
    // SAFETY: TVM_GETITEMW's lParam is a TVITEMW (no field of TVITEMEXW's
    // asked for) whose only pointer is the text buffer of cchTextMax units;
    // its item is in the tree (`live`).
    let ok = unsafe { send(item.tree, TVM_GETITEMW, 0, (&raw mut it) as isize) } != 0;
    if !ok {
        return None;
    }
    let text = (it.mask & TVIF_TEXT != 0).then(|| {
        if it.pszText == buf.as_mut_ptr() || it.pszText.is_null() || it.pszText as isize == -1 {
            String::from_utf16_lossy(super::until_nul(&buf))
        } else {
            // SAFETY: a tree may point pszText at its own NUL-terminated
            // copy instead, alive until the next message to it.
            unsafe { super::msg::read_nul(it.pszText) }
                .map(|u| String::from_utf16_lossy(&u))
                .unwrap_or_default()
        }
    });
    it.pszText = core::ptr::null_mut();
    it.cchTextMax = 0;
    Some((it, text))
}

/// The item's `lParam`.
pub fn item_param(item: Item) -> Option<isize> {
    let it = TVITEMW {
        mask: windows_sys::Win32::UI::Controls::TVIF_PARAM,
        ..Default::default()
    };
    get_item(item, it, 0).map(|(it, _)| it.lParam)
}

/// Sets the fields `fields.mask` names of `item` (TVITEMEXW's own masked
/// off), with `text` as its text when given (TVIF_TEXT then).
pub fn set_item(item: Item, fields: TVITEMW, text: Option<&str>) -> bool {
    if !live(item) {
        return false;
    }
    let mut it = fields;
    it.mask &= !EX_ONLY;
    it.hItem = item.h;
    let z = text.map(super::wz);
    match &z {
        Some(z) => {
            it.mask |= TVIF_TEXT;
            it.pszText = z.as_ptr().cast_mut();
        }
        None => {
            it.mask &= !TVIF_TEXT;
            it.pszText = core::ptr::null_mut();
        }
    }
    it.cchTextMax = 0;
    // SAFETY: TVM_SETITEMW's lParam is a TVITEMW (no field of TVITEMEXW's
    // named) whose text is set above; its item is in the tree (`live`).
    unsafe { send(item.tree, TVM_SETITEMW, 0, (&raw const it) as isize) != 0 }
}

/// The parent and the place an insert names, as TVINSERTSTRUCTW takes
/// them; `None` if `parent` or `after` is not an item of `tree`, or
/// `after` is not a child of `parent`.
fn place(tree: HWND, parent: Option<Item>, after: After) -> Option<(HTREEITEM, HTREEITEM)> {
    if !track(tree) {
        return None;
    }
    let parent = match parent {
        Some(p) if p.tree == tree && live(p) => p.h,
        Some(_) => return None,
        None => TVI_ROOT,
    };
    let after = match after {
        After::First => TVI_FIRST,
        After::Last => TVI_LAST,
        After::Item(a) => {
            let under = if parent == TVI_ROOT { 0 } else { parent };
            if a.tree != tree || !live(a) || next_raw(tree, a.h, TVGN_PARENT) != under {
                return None;
            }
            a.h
        }
    };
    Some((parent, after))
}

/// Inserts `item` (its TVIF_* fields) in `tree` under `parent` (`None`: at
/// the top), placed as `after` says, with `text` when given (TVIF_TEXT
/// then; for a callback item, leave it out and answer TVN_GETDISPINFO).
/// The new item; `None` if not.
pub fn insert(
    tree: HWND,
    parent: Option<Item>,
    after: After,
    item: TVITEMEXW,
    text: Option<&str>,
) -> Option<Item> {
    let (parent, after) = place(tree, parent, after)?;
    let mut it = item;
    let z = text.map(super::wz);
    match &z {
        Some(z) => {
            it.mask |= TVIF_TEXT;
            it.pszText = z.as_ptr().cast_mut();
        }
        None if it.pszText as isize == -1 => {
            // LPSTR_TEXTCALLBACKW: a marker the tree keeps, not a pointer.
        }
        None => {
            it.mask &= !TVIF_TEXT;
            it.pszText = core::ptr::null_mut();
        }
    }
    it.cchTextMax = 0;
    let mut ins = TVINSERTSTRUCTW {
        hParent: parent,
        hInsertAfter: after,
        ..Default::default()
    };
    ins.Anonymous.itemex = it;
    // SAFETY: TVM_INSERTITEMW's lParam is a TVINSERTSTRUCTW whose text is
    // set above (or the callback marker), read during the call; its parent
    // and place are items in the tree, or its markers (`place`).
    let h = unsafe { send(tree, TVM_INSERTITEMW, 0, (&raw const ins) as isize) };
    item_of(tree, h)
}

/// [`insert`] with UTF-16 text (a file name need not be valid Unicode, and
/// is read up to a NUL, if it has one): `item` (its TVIF_* fields,
/// TVIF_TEXT added, TVITEMEXW's own masked off) in `tree` under `parent`,
/// placed as `after` says, with `text`. The new item; `None` if not.
pub fn insert_units(
    tree: HWND,
    parent: Option<Item>,
    after: After,
    item: TVITEMW,
    text: &[u16],
) -> Option<Item> {
    let (parent, after) = place(tree, parent, after)?;
    let z = super::wz_units(super::until_nul(text));
    let mut it = item;
    it.mask = (it.mask | TVIF_TEXT) & !EX_ONLY;
    it.pszText = z.as_ptr().cast_mut();
    it.cchTextMax = 0;
    let mut ins = TVINSERTSTRUCTW {
        hParent: parent,
        hInsertAfter: after,
        ..Default::default()
    };
    ins.Anonymous.item = it;
    // SAFETY: TVM_INSERTITEMW's lParam is a TVINSERTSTRUCTW whose only
    // pointer is the text set above, read during the call; its parent and
    // place are items in the tree, or its markers (`place`).
    let h = unsafe { send(tree, TVM_INSERTITEMW, 0, (&raw const ins) as isize) };
    item_of(tree, h)
}

/// Sets tree item `item`'s text from UTF-16 (up to a NUL, if it has one).
pub fn set_text_units(item: Item, text: &[u16]) -> bool {
    if !live(item) {
        return false;
    }
    let z = super::wz_units(super::until_nul(text));
    let it = TVITEMW {
        mask: TVIF_TEXT,
        hItem: item.h,
        pszText: z.as_ptr().cast_mut(),
        ..Default::default()
    };
    // SAFETY: TVM_SETITEMW's lParam is a TVITEMW whose only pointer is the
    // text set above, read during the call; its item is in the tree.
    unsafe { send(item.tree, TVM_SETITEMW, 0, (&raw const it) as isize) != 0 }
}

/// An item a tree notified, as an [`Item`]: `tree` is the notifying tree.
fn notified(tree: HWND, h: HTREEITEM) -> Option<Item> {
    if !track(tree) {
        return None;
    }
    item_of(tree, h)
}

impl View<'_, NMTREEVIEWW> {
    /// The notification's new item (TVN_SELCHANGED's selected one,
    /// TVN_ITEMEXPANDING's...), if it is in its tree.
    pub fn item_new(&self) -> Option<Item> {
        notified(self.hdr.hwndFrom, self.itemNew.hItem)
    }
}

impl View<'_, NMTVGETINFOTIPW> {
    /// The item whose tip is asked for, if it is in its tree.
    pub fn item(&self) -> Option<Item> {
        notified(self.hdr.hwndFrom, self.hItem)
    }
}
