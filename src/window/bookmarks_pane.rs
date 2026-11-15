//! The Bookmarks pane (Everything's Bookmarks sidebar, View > Bookmarks): a
//! "Search bookmarks" box over the bookmarks as a tree, folders holding
//! what they hold in the Bookmarks menu's order.
//!
//! - **Opening.** A click on a bookmark (or Enter on it) opens it, as the
//!   Bookmarks menu does; a folder opens and closes with its button, a
//!   double-click or Enter. Which folders are open is kept
//!   (`bookmarks_sidebar_expanded`).
//! - **The box** keeps the bookmarks whose name (with its folders) or
//!   search holds what is typed, inside their folders, all open
//!   (`crate::sidebars::bookmark_view`); its × clears it. Down goes to the
//!   tree, Enter opens the bookmark chosen there (or the first one shown),
//!   Esc clears it.
//! - **The menu**: Open, Edit..., Delete for a bookmark; Expand (or
//!   Collapse), Edit... (its name), Delete for a folder; then Add to
//!   Bookmarks..., Find Bookmark... and Organize Bookmarks..., as
//!   Everything's. F2 edits, Delete deletes (asking first). A bookmark's
//!   search shows as its tooltip.
//! - **In step** with the bookmarks wherever they change: this window's
//!   menu and dialogs, and another goz-gui's changes once the file is read
//!   again (as the menu opens, as the pane takes the keyboard). The window
//!   tells the pane (`App::bookmarks_changed`); it refills only when the
//!   bookmarks' generation moved.
//!
//! Re-entrancy: no `RefCell` here is borrowed across a message to the tree
//! or the box, a menu or a dialog.

use std::any::Any;
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::Arc;

use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT};
use windows_sys::Win32::Graphics::Gdi::HDC;
use windows_sys::Win32::UI::Controls::{
    HTREEITEM, NM_CLICK, NM_CUSTOMDRAW, NM_DBLCLK, NM_SETFOCUS, NMTREEVIEWW, NMTVCUSTOMDRAW,
    NMTVGETINFOTIPW, TVE_COLLAPSE, TVE_EXPAND, TVHT_ONITEM, TVHT_ONITEMINDENT, TVHT_ONITEMRIGHT,
    TVN_GETINFOTIPW, TVN_ITEMEXPANDEDW, TVS_EX_AUTOHSCROLL, TVS_EX_DOUBLEBUFFER, TVS_FULLROWSELECT,
    TVS_HASBUTTONS, TVS_INFOTIP, TVS_LINESATROOT, TVS_NOHSCROLL, TVS_NONEVENHEIGHT,
    TVS_SHOWSELALWAYS, TVS_TRACKSELECT, WM_MOUSELEAVE,
};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    VK_CONTROL, VK_DELETE, VK_DOWN, VK_ESCAPE, VK_F2, VK_RETURN, VK_SHIFT, VK_UP,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    EN_CHANGE, EN_SETFOCUS, ES_AUTOHSCROLL, MSG, SM_CXSMICON, WM_APP, WM_CAPTURECHANGED, WM_CHAR,
    WM_COMMAND, WM_CONTEXTMENU, WM_KEYDOWN, WM_LBUTTONDBLCLK, WM_LBUTTONDOWN, WM_LBUTTONUP,
    WM_MOUSEMOVE, WM_NOTIFY, WM_SYSKEYDOWN, WS_CHILD, WS_TABSTOP, WS_VISIBLE,
};

use super::sidebars::{MARK_DELETE, MARK_EDIT, MARK_EXPAND, MARK_OPEN};
use super::{App, key_down, point};
use crate::bookmark_dialogs::{self, Chosen, Host};
use crate::bookmarks::ops::{Id, Op};
use crate::bookmarks::tree::Item;
use crate::bookmarks::{Bookmarks, Kind, one_line};
use crate::menu::{self, Item as MenuItem};
use crate::paint::fill;
use crate::pane::PaneContent;
use crate::sidebars::{self, Shown};
use crate::treeview::{self, Images, NONE, New, Source};
use crate::win::class::{self, Create};
use crate::win::{self, Msg, edit, subclass, sys, tv};
use crate::{searchbox, theme};

/// The children's ids in the pane.
const EDIT_ID: usize = 1;
const TREE_ID: usize = 2;
/// The pane took the keyboard: the file is read again if another program
/// changed it (after the focus change is done with, not inside it).
const WM_REFRESH: u32 = WM_APP + 1;

/// What a tree item stands for (its param is its index here).
#[derive(Clone, Debug, PartialEq, Eq)]
enum Entry {
    /// A folder: its node in the tree of the bookmarks shown, and its
    /// place.
    Folder { node: usize, place: Vec<String> },
    /// A bookmark: its row in the bookmarks shown.
    Mark { row: usize },
}

/// What stays chosen when the tree is filled again: a folder by its place,
/// a bookmark by its identity.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Keep {
    Folder(String),
    Mark(Id),
}

pub(super) struct BookmarksPane {
    pane: Cell<HWND>,
    edit: Cell<HWND>,
    tree: Cell<HWND>,
    images: RefCell<Option<Images>>,
    /// The system image list's folder and bookmark (page) icons.
    plain: Cell<(i32, i32)>,
    /// The bookmarks shown, and their generation.
    list: RefCell<Arc<Bookmarks>>,
    generation: Cell<Option<u64>>,
    /// What each tree item stands for.
    entries: RefCell<Vec<Entry>>,
    /// The tree is being filled: its notifications are goz-gui's.
    filling: Cell<bool>,
    /// Where the left button last went down on the tree (its subclass,
    /// which shares it).
    click: Rc<Cell<POINT>>,
    /// The mouse is over the box's ×; it went down on it (and holds the
    /// mouse until it comes up: the × clears the box on release over it).
    hot: Cell<bool>,
    pressed: Cell<bool>,
}

impl BookmarksPane {
    pub(super) fn new() -> BookmarksPane {
        BookmarksPane {
            pane: Cell::new(core::ptr::null_mut()),
            edit: Cell::new(core::ptr::null_mut()),
            tree: Cell::new(core::ptr::null_mut()),
            images: RefCell::new(None),
            plain: Cell::new((0, 0)),
            list: RefCell::new(Arc::new(Bookmarks::default())),
            generation: Cell::new(None),
            entries: RefCell::new(Vec::new()),
            filling: Cell::new(false),
            click: Rc::new(Cell::new(POINT::default())),
            hot: Cell::new(false),
            pressed: Cell::new(false),
        }
    }

    /// Whether the box holds text (its button is the ×, not the
    /// magnifier).
    fn has_text(&self) -> bool {
        win::text_len(self.edit.get()) > 0
    }

    /// The box's button in pane window `pane`.
    fn button_in(app: &App, pane: HWND) -> RECT {
        Self::button(&Self::frame(app, &crate::pane::content_rect(app, pane)))
    }

    /// Lights the × (or not), repainting it when that changes.
    fn set_hot(&self, app: &App, pane: HWND, hot: bool) {
        if self.hot.replace(hot) != hot {
            let rc = Self::button_in(app, pane);
            win::invalidate(pane, Some(&rc), false);
        }
    }

    /// The mouse moved to `pt` (the pane's coordinates): the × lights
    /// while it is over it (while held, only if it went down there).
    fn mouse_move(&self, app: &App, pane: HWND, pt: POINT) {
        let over = inside(&Self::button_in(app, pane), pt) && self.has_text();
        if over && !self.pressed.get() {
            // WM_MOUSELEAVE, when the mouse leaves this pane's window.
            win::track_leave(pane);
        }
        self.set_hot(app, pane, over);
    }

    /// The box's frame in the content `rc`: Everything's, 3 px in from the
    /// sides, as tall as the search box (23 px at 100 %).
    fn frame(app: &App, rc: &RECT) -> RECT {
        let high = app.part_height(crate::colors::Item::BookmarksSidebar) + app.scale(8);
        let top = rc.top + app.scale(2);
        RECT {
            left: rc.left + app.scale(3),
            right: (rc.right - app.scale(3)).max(rc.left + app.scale(3)),
            top,
            bottom: (top + high).min(rc.bottom),
        }
    }

    /// The bar holding the box: the frame and 2 px below it.
    fn bar(app: &App, rc: &RECT) -> RECT {
        RECT {
            bottom: (Self::frame(app, rc).bottom + app.scale(2)).min(rc.bottom),
            ..*rc
        }
    }

    /// The box's button (× or the magnifier) at its frame's right.
    fn button(frame: &RECT) -> RECT {
        let side = frame.bottom - frame.top;
        RECT {
            left: (frame.right - side).max(frame.left),
            ..*frame
        }
    }

    fn filter(&self) -> String {
        win::text(self.edit.get())
    }

    /// The entry of a tree item.
    fn entry(&self, item: HTREEITEM) -> Option<Entry> {
        let i = usize::try_from(treeview::param(self.tree.get(), item)?).ok()?;
        self.entries.borrow().get(i).cloned()
    }

    /// What `entry` stands for, by identity: it outlives a refill.
    fn keep_of(&self, entry: &Entry) -> Keep {
        match entry {
            Entry::Folder { place, .. } => Keep::Folder(place.join("/")),
            Entry::Mark { row } => Keep::Mark(Id::of(&self.list.borrow(), *row)),
        }
    }

    /// The chosen item's entry, kept by identity.
    fn chosen_key(&self) -> Option<Keep> {
        let entry = self.entry(treeview::selected(self.tree.get()))?;
        Some(self.keep_of(&entry))
    }

    /// The tree item, and its entry, that stands for `keep` now; `None`
    /// when it is not shown any more.
    fn find(&self, keep: &Keep) -> Option<(HTREEITEM, Entry)> {
        let tree = self.tree.get();
        let mut todo = treeview::children(tree, NONE);
        while let Some(item) = todo.pop() {
            if let Some(entry) = self.entry(item)
                && self.keep_of(&entry) == *keep
            {
                return Some((item, entry));
            }
            todo.extend(treeview::children(tree, item));
        }
        None
    }

    /// Shows the bookmarks again if they changed since they were shown.
    pub(super) fn sync(&self, app: &App) {
        let (_, generation) = Host::bookmarks(app);
        if self.generation.get() != Some(generation) {
            self.fill(app);
        }
    }

    /// Fills the tree with the bookmarks as the box filters them, keeping
    /// the item chosen where it still shows.
    fn fill(&self, app: &App) {
        let (list, generation) = Host::bookmarks(app);
        let filter = self.filter();
        let expanded = app.sidebar_settings().bookmarks_expanded;
        let shown = sidebars::bookmark_view(&list, &filter, &expanded);
        let keep = self.chosen_key();
        let (folder, mark) = self.plain.get();
        let (folder_image, mark_image) = {
            let mut images = self.images.borrow_mut();
            images.as_mut().map_or((-1, -1), |i| {
                (i.get(Source::System(folder)), i.get(Source::System(mark)))
            })
        };
        let tree = self.tree.get();
        let was = self.filling.replace(true);
        treeview::delete(tree, NONE);
        self.entries.borrow_mut().clear();
        let mut again = None;
        let mut first = NONE;
        self.insert(
            &shown,
            NONE,
            (folder_image, mark_image),
            &list,
            keep.as_ref(),
            &mut again,
            &mut first,
        );
        if let Some(item) = again.or((first != NONE && !filter.is_empty()).then_some(first)) {
            treeview::select(tree, item);
        }
        self.filling.set(was);
        *self.list.borrow_mut() = list;
        self.generation.set(Some(generation));
    }

    /// Puts `shown` in the tree under `parent`; notes the item of `keep`
    /// and the first bookmark.
    #[allow(clippy::too_many_arguments, reason = "one recursive walk's state")]
    fn insert(
        &self,
        shown: &[Shown],
        parent: HTREEITEM,
        images: (i32, i32),
        list: &Bookmarks,
        keep: Option<&Keep>,
        found: &mut Option<HTREEITEM>,
        first: &mut HTREEITEM,
    ) {
        let tree = self.tree.get();
        for s in shown {
            let (entry, text, image, children) = match s {
                Shown::Folder {
                    node,
                    place,
                    name,
                    items,
                    ..
                } => (
                    Entry::Folder {
                        node: *node,
                        place: place.clone(),
                    },
                    name.clone(),
                    images.0,
                    !items.is_empty(),
                ),
                Shown::Mark { row, name } => {
                    (Entry::Mark { row: *row }, name.clone(), images.1, false)
                }
            };
            let param = {
                let mut entries = self.entries.borrow_mut();
                entries.push(entry.clone());
                entries.len() as isize - 1
            };
            let text: Vec<u16> = text.encode_utf16().collect();
            let item = treeview::insert(
                tree,
                &New {
                    parent,
                    after: None,
                    text: &text,
                    image,
                    children,
                    param,
                },
            );
            if item == NONE {
                continue;
            }
            let key = match &entry {
                Entry::Folder { place, .. } => Keep::Folder(place.join("/")),
                Entry::Mark { row } => Keep::Mark(Id::of(list, *row)),
            };
            if keep == Some(&key) {
                *found = Some(item);
            }
            match s {
                Shown::Folder { items, open, .. } => {
                    self.insert(items, item, images, list, keep, found, first);
                    if *open {
                        treeview::expand(tree, item, true);
                    }
                }
                Shown::Mark { .. } if *first == NONE => *first = item,
                Shown::Mark { .. } => {}
            }
        }
    }

    /// Opens bookmark `row` of the bookmarks shown, as the Bookmarks menu
    /// does.
    fn open(&self, app: &App, row: usize) {
        let chosen = {
            let list = self.list.borrow();
            let Some(r) = list.rows().get(row) else {
                return;
            };
            if r.kind() != Kind::Bookmark {
                return;
            }
            Chosen {
                search: r.search_set().then(|| r.search().to_owned()),
                settings: r.settings(),
            }
        };
        app.open_chosen(&chosen);
    }

    /// Whether a bookmark dialog may open now (no other is up), taking the
    /// window's flag for it.
    fn dialog_free(app: &App) -> bool {
        !(app.run_history_dialog_open()
            || app.search_history_dialog_open()
            || app.bookmark_dialog.replace(true))
    }

    /// Edit...: a bookmark in the editor, a folder's name.
    fn edit_entry(&self, app: &App, entry: &Entry) {
        if !Self::dialog_free(app) {
            return;
        }
        let list = Arc::clone(&self.list.borrow());
        match entry {
            Entry::Mark { row } => {
                if let Some(old) = list.rows().get(*row).map(|r| (**r).clone())
                    && old.kind() == Kind::Bookmark
                    && let Some(fields) = bookmark_dialogs::edit(
                        app.hwnd,
                        "Edit Bookmark",
                        &old.fields(),
                        app.dialog_look(),
                    )
                {
                    let mut new = old.clone();
                    new.set_fields(&fields);
                    if new != old {
                        Host::change(
                            app,
                            Op::Edit {
                                id: Id::of(&list, *row),
                                row: new,
                            },
                        );
                    }
                }
            }
            Entry::Folder { place, .. } => {
                bookmark_dialogs::rename_folder(app.hwnd, app, &list, place, app.dialog_look());
            }
        }
        app.bookmark_dialog.set(false);
    }

    /// Delete: asks first, as Organize Bookmarks does.
    fn delete_entry(&self, app: &App, entry: &Entry) {
        if !Self::dialog_free(app) {
            return;
        }
        let list = Arc::clone(&self.list.borrow());
        let item = match entry {
            Entry::Mark { row } => Item::Row(*row),
            Entry::Folder { node, .. } => Item::Folder(*node),
        };
        bookmark_dialogs::delete(app.hwnd, app, &list, &[item]);
        app.bookmark_dialog.set(false);
    }

    /// What Enter (or the menu's first item) does to `item`: opens a
    /// bookmark, opens or closes a folder.
    fn activate(&self, app: &App, item: HTREEITEM) {
        match self.entry(item) {
            Some(Entry::Mark { row }) => self.open(app, row),
            Some(Entry::Folder { .. }) => {
                let tree = self.tree.get();
                treeview::expand(tree, item, !treeview::is_expanded(tree, item));
            }
            None => {}
        }
    }

    /// A folder opened or closed by the user: the open ones are kept, while
    /// the box filters nothing (filtered, they all show open).
    fn expanded(&self, app: &App, nm: &NMTREEVIEWW) {
        if self.filling.get() || !self.filter().is_empty() {
            return;
        }
        let Some(Entry::Folder { place, .. }) = self.entry(nm.itemNew.hItem) else {
            return;
        };
        let place = place.join("/");
        let mut open = app.sidebar_settings().bookmarks_expanded;
        open.retain(|p| !p.eq_ignore_ascii_case(&place));
        if nm.action == TVE_EXPAND {
            open.push(place);
        } else if nm.action != TVE_COLLAPSE {
            return;
        }
        app.set_bookmarks_expanded(open);
    }

    /// The menu at `l` (screen) for the item there, or the chosen one from
    /// the keyboard (-1).
    fn menu(&self, app: &App, l: LPARAM) {
        let tree = self.tree.get();
        // Another program may have changed the file: the tree shows it
        // before its item is looked for.
        app.refresh_bookmarks();
        self.sync(app);
        let (item, pt) = if l == -1 {
            let item = treeview::selected(tree);
            let rc = treeview::item_rect(tree, item, true);
            let pt = win::client_to_screen(
                tree,
                POINT {
                    x: rc.left,
                    y: rc.bottom,
                },
            );
            (item, pt)
        } else {
            let pt = point(l);
            let at = win::screen_to_client(tree, pt);
            (treeview::hit(tree, at).0, pt)
        };
        let entry = self.entry(item);
        // What it stands for, looked for again after the menu.
        let keep = entry.as_ref().map(|e| self.keep_of(e));
        let mut items = Vec::new();
        let default = match &entry {
            Some(Entry::Mark { .. }) => {
                items.extend([MenuItem::Cmd(MARK_OPEN, "&Open"), MenuItem::Sep]);
                Some(MARK_OPEN)
            }
            Some(Entry::Folder { .. }) => {
                let label = if treeview::is_expanded(tree, item) {
                    "Collap&se"
                } else {
                    "Exp&and"
                };
                items.extend([MenuItem::Cmd(MARK_EXPAND, label), MenuItem::Sep]);
                Some(MARK_EXPAND)
            }
            None => None,
        };
        if entry.is_some() {
            items.extend([
                MenuItem::Cmd(MARK_EDIT, "&Edit..."),
                MenuItem::Cmd(MARK_DELETE, "&Delete"),
                MenuItem::Sep,
            ]);
        }
        // Everything's labels, as its sidebar's menu has them.
        items.extend([
            MenuItem::Cmd(menu::BOOKMARK_ADD, "Add to Bookmarks..."),
            MenuItem::Cmd(menu::BOOKMARK_FIND, "Find Bookmark..."),
            MenuItem::Cmd(menu::BOOKMARK_ORGANIZE, "Organize Bookmarks..."),
        ]);
        let popup = menu::submenu(&items);
        if let Some(d) = default {
            menu::set_default(popup, d);
        }
        let marked = entry.is_some() && item != treeview::selected(tree);
        if marked {
            treeview::drop_highlight(tree, item);
        }
        let id = app.track_with_shell(popup, pt, "bookmarks menu", None);
        if marked {
            treeview::drop_highlight(tree, NONE);
        }
        // The menu made above.
        win::menu::destroy(popup);
        // The menu's loop may have filled the tree again (another goz-gui
        // changed the bookmarks, read as the pane took the keyboard): the
        // item and its row are what they stand for now, or nothing if it
        // is gone.
        let now = keep.as_ref().and_then(|k| self.find(k));
        match (id as u16, now) {
            (0, _) => {}
            (MARK_OPEN | MARK_EXPAND, Some((item, _))) => self.activate(app, item),
            (MARK_EDIT, Some((_, e))) => self.edit_entry(app, &e),
            (MARK_DELETE, Some((_, e))) => self.delete_entry(app, &e),
            (MARK_OPEN | MARK_EXPAND | MARK_EDIT | MARK_DELETE, None) => {}
            (id, _) => app.command(id),
        }
    }

    /// A tree notification.
    fn notify(&self, app: &App, m: &Msg) -> Option<LRESULT> {
        let n = m.notify()?;
        if n.from() != self.tree.get() {
            return None;
        }
        let tree = self.tree.get();
        match n.code() {
            // Options > Fonts and Colors' Bookmarks Sidebar states.
            NM_CUSTOMDRAW => {
                let pal = app.pal.get();
                let bold = app.part_font(crate::colors::Item::BookmarksSidebar, true);
                let mut cd = n.get::<NMTVCUSTOMDRAW>()?;
                Some(treeview::custom_draw(
                    tree,
                    &mut cd,
                    &pal,
                    crate::colors::Item::BookmarksSidebar,
                    bold,
                ))
            }
            NM_CLICK => {
                // A click on a bookmark opens it, whichever part of its row
                // (not the button of a folder): where the button went down
                // on the tree, as its subclass saw it.
                let (item, flags) = treeview::hit(tree, self.click.get());
                if flags & (TVHT_ONITEM | TVHT_ONITEMRIGHT | TVHT_ONITEMINDENT) != 0
                    && let Some(Entry::Mark { row }) = self.entry(item)
                {
                    self.open(app, row);
                }
                Some(0)
            }
            // The click opened the bookmark already; a folder's double-click
            // opens or closes it, as the tree does.
            NM_DBLCLK => {
                let item = treeview::selected(tree);
                Some(LRESULT::from(matches!(
                    self.entry(item),
                    Some(Entry::Mark { .. })
                )))
            }
            // Taking the keyboard is a moment to see whether another goz-gui
            // changed the file.
            NM_SETFOCUS => {
                self.refresh_soon();
                Some(0)
            }
            TVN_ITEMEXPANDEDW => {
                // Copied out: nothing of the message is held meanwhile.
                if let Some(nm) = n.get::<NMTREEVIEWW>().map(|v| *v) {
                    self.expanded(app, &nm);
                }
                Some(0)
            }
            TVN_GETINFOTIPW => {
                let Some(mut tip) = n.get::<NMTVGETINFOTIPW>() else {
                    return Some(0);
                };
                if let Some(Entry::Mark { row }) = self.entry(tip.hItem) {
                    let text = {
                        let list = self.list.borrow();
                        list.rows().get(row).map(|r| {
                            if r.search_set() {
                                one_line(r.search())
                            } else {
                                "(No change)".to_owned()
                            }
                        })
                    };
                    if let Some(text) = text {
                        // Into the tree's buffer, cut to fit.
                        let units: Vec<u16> = text.encode_utf16().collect();
                        tip.set_text(&units);
                    }
                }
                Some(0)
            }
            _ => None,
        }
    }

    /// Reads the bookmarks file again (if another program changed it) once
    /// the message being handled is done with.
    fn refresh_soon(&self) {
        win::post_app(self.pane.get(), WM_REFRESH, 0, 0);
    }

    /// Clears the box (the search through the bookmarks).
    fn clear(&self) {
        // EN_CHANGE fills the tree again.
        win::set_text(self.edit.get(), "");
    }

    /// The tree's font, rows and icons for the window's DPI.
    fn fit_dpi(&self, app: &App) {
        let dpi = app.dpi.get();
        let icon = sys::metric_for_dpi(SM_CXSMICON, dpi);
        let resized = {
            let images = self.images.borrow();
            match images.as_ref() {
                Some(i) if i.size() == icon => None,
                Some(i) => Some(i.resized(icon)),
                None => {
                    let (folder, mark) = self.plain.get();
                    let mut i = Images::new(icon);
                    i.get(Source::System(folder));
                    i.get(Source::System(mark));
                    Some(i)
                }
            }
        };
        let tree = self.tree.get();
        if let Some(new) = resized {
            treeview::set_images(tree, Some(&new));
            *self.images.borrow_mut() = Some(new);
        }
        let font = app.part_font(crate::colors::Item::BookmarksSidebar, false);
        let fh = app.part_height(crate::colors::Item::BookmarksSidebar);
        treeview::set_font(tree, font, fh, icon, |v| app.scale(v));
        win::set_font(self.edit.get(), font, true);
    }

    fn apply_theme(&self, app: &App) {
        let pal = app.pal.get();
        treeview::apply_theme(self.tree.get(), &pal, crate::colors::Item::BookmarksSidebar);
        let edit = self.edit.get();
        theme::allow(edit, pal.dark);
        theme::set_class(edit, pal.dark.then_some("DarkMode_CFD"));
    }
}

fn inside(r: &RECT, pt: POINT) -> bool {
    pt.x >= r.left && pt.x < r.right && pt.y >= r.top && pt.y < r.bottom
}

/// The box's × glyph and face colours: lit under the mouse (as the search
/// box's is), darker while held there, else the box's own (painted inside
/// the frame's line).
fn clear_colours(pal: &crate::theme::Palette, bg: u32, hot: bool, pressed: bool) -> (u32, u32) {
    match (hot, pressed) {
        (true, true) => (pal.glyph_hot, pal.header_pressed),
        (true, false) => (pal.glyph_hot, pal.glyph_hot_bg),
        _ => (pal.glyph, bg),
    }
}

/// The tree's subclass.
static TREE: subclass::Key = subclass::Key::new("the Bookmarks pane's tree");

/// The tree's subclass, `click` the pane's cell: notes where its left
/// button goes down (the tree's coordinates), which its NM_CLICK then
/// hit-tests.
fn tree_proc(click: &Cell<POINT>, m: &Msg) -> LRESULT {
    if matches!(m.id, WM_LBUTTONDOWN | WM_LBUTTONDBLCLK) {
        click.set(m.point());
    }
    m.def()
}

impl PaneContent for BookmarksPane {
    fn colours(&self, app: &App) -> crate::theme::Ink {
        app.pal.get().ink(
            crate::colors::Item::BookmarksSidebar,
            crate::colors::State::Normal,
        )
    }

    fn create(&self, app: &App, pane: HWND) {
        self.pane.set(pane);
        // Children of this thread's pane window.
        let edit = class::create_plain(&Create {
            ex_style: 0,
            class: "EDIT",
            title: "",
            style: WS_CHILD | WS_VISIBLE | WS_TABSTOP | ES_AUTOHSCROLL as u32,
            rect: (0, 0, 0, 0),
            parent: pane,
            menu: class::id_menu(EDIT_ID),
        });
        let tree = class::create_plain(&Create {
            ex_style: 0,
            class: tv::CLASS,
            title: "",
            style: WS_CHILD
                | WS_VISIBLE
                | WS_TABSTOP
                | TVS_HASBUTTONS
                | TVS_LINESATROOT
                | TVS_SHOWSELALWAYS
                | TVS_TRACKSELECT
                | TVS_FULLROWSELECT
                | TVS_NOHSCROLL
                | TVS_NONEVENHEIGHT
                | TVS_INFOTIP,
            rect: (0, 0, 0, 0),
            parent: pane,
            menu: class::id_menu(TREE_ID),
        });
        self.edit.set(edit);
        self.tree.set(tree);
        // The subclass shares the pane's cell; `destroy` takes it off
        // before the tree goes.
        let click = Rc::clone(&self.click);
        subclass::set(tree, &TREE, move |m| tree_proc(&click, m));
        // The box copies its cue; shown even with the keyboard in it.
        edit::set_cue_banner(edit, "Search bookmarks", true);
        let ex = TVS_EX_DOUBLEBUFFER | TVS_EX_AUTOHSCROLL;
        tv::set_extended_style(tree, ex, ex);
        self.plain.set(treeview::plain_icons());
        self.fit_dpi(app);
        self.apply_theme(app);
        // Another program may have changed the file since it was read.
        app.refresh_bookmarks();
        self.fill(app);
    }

    fn place(&self, app: &App, rc: RECT) {
        let frame = Self::frame(app, &rc);
        let bar = Self::bar(app, &rc);
        let button = Self::button(&frame);
        let fh = app.part_height(crate::colors::Item::BookmarksSidebar);
        let left = frame.left + app.scale(4);
        let top = frame.top + (frame.bottom - frame.top - fh) / 2;
        win::move_window(
            self.edit.get(),
            left,
            top,
            (button.left - left).max(0),
            fh.min(frame.bottom - top).max(0),
            true,
        );
        win::move_window(
            self.tree.get(),
            rc.left,
            bar.bottom,
            rc.right - rc.left,
            (rc.bottom - bar.bottom).max(0),
            true,
        );
        win::invalidate(self.pane.get(), Some(&bar), false);
    }

    fn destroy(&self, _app: &App) {
        let tree = self.tree.get();
        // Takes off the subclass `create` put on this pane's tree.
        subclass::remove(tree, &TREE);
        treeview::set_images(tree, None);
        self.images.borrow_mut().take();
    }

    fn focus(&self) -> Option<HWND> {
        Some(self.edit.get())
    }

    fn tab(&self, _app: &App, from: HWND, forward: bool) -> bool {
        let (edit, tree) = (self.edit.get(), self.tree.get());
        let to = match (from == edit, forward) {
            (true, true) => tree,
            (false, false) if from == tree => edit,
            _ => return false,
        };
        win::set_focus(to);
        if to == edit {
            edit::select_all(edit);
        }
        true
    }

    fn key(&self, app: &App, msg: &MSG) -> bool {
        let (edit, tree) = (self.edit.get(), self.tree.get());
        let vk = msg.wParam as u16;
        // Alt+Up, Everything's global Up One Dir, from either.
        if msg.message == WM_SYSKEYDOWN && vk == VK_UP && (msg.hwnd == edit || msg.hwnd == tree) {
            app.up_one_folder();
            return true;
        }
        if msg.hwnd == edit {
            return match msg.message {
                WM_KEYDOWN => match vk {
                    VK_DOWN => {
                        let tree_item = treeview::selected(tree);
                        if tree_item == NONE
                            && let Some(&first) = treeview::children(tree, NONE).first()
                        {
                            treeview::select(tree, first);
                        }
                        win::set_focus(tree);
                        true
                    }
                    VK_RETURN => {
                        let chosen = match self.entry(treeview::selected(tree)) {
                            Some(Entry::Mark { row }) => Some(row),
                            _ => {
                                let list = Arc::clone(&self.list.borrow());
                                let expanded = app.sidebar_settings().bookmarks_expanded;
                                sidebars::first_mark(&sidebars::bookmark_view(
                                    &list,
                                    &self.filter(),
                                    &expanded,
                                ))
                            }
                        };
                        if let Some(row) = chosen {
                            self.open(app, row);
                        }
                        true
                    }
                    // Shift+Esc is Close Pane: it goes on to the window's
                    // shortcuts.
                    VK_ESCAPE if !key_down(VK_SHIFT) => {
                        if win::text_len(edit) > 0 {
                            self.clear();
                        }
                        true
                    }
                    k if k == u16::from(b'A') && key_down(VK_CONTROL) => {
                        edit::select_all(edit);
                        true
                    }
                    VK_DELETE if key_down(VK_CONTROL) => {
                        super::edit::delete_word(edit, false);
                        true
                    }
                    _ => false,
                },
                // Ctrl+Backspace arrives as DEL, which a box would type.
                WM_CHAR if msg.wParam == 0x7f => {
                    super::edit::delete_word(edit, true);
                    true
                }
                _ => false,
            };
        }
        if msg.hwnd != tree || msg.message != WM_KEYDOWN {
            return false;
        }
        let item = treeview::selected(tree);
        match vk {
            VK_RETURN => {
                self.activate(app, item);
                true
            }
            VK_F2 => {
                if let Some(e) = self.entry(item) {
                    self.edit_entry(app, &e);
                }
                true
            }
            VK_DELETE => {
                if let Some(e) = self.entry(item) {
                    self.delete_entry(app, &e);
                }
                true
            }
            _ => false,
        }
    }

    fn handle(&self, app: &App, pane: HWND, m: &Msg) -> Option<LRESULT> {
        let (w, l) = (m.w, m.l);
        match m.id {
            WM_NOTIFY => self.notify(app, m),
            // lParam is the control that sent it.
            WM_COMMAND if l as HWND == self.edit.get() => {
                match u32::from((w >> 16) as u16) {
                    EN_CHANGE => {
                        self.fill(app);
                        let rc = Self::bar(app, &crate::pane::content_rect(app, pane));
                        // The box's button turns from × to the magnifier
                        // and back.
                        win::invalidate(self.pane.get(), Some(&rc), false);
                    }
                    EN_SETFOCUS => self.refresh_soon(),
                    _ => {}
                }
                Some(0)
            }
            // wParam is the window clicked.
            WM_CONTEXTMENU if w as HWND == self.tree.get() => {
                self.menu(app, l);
                Some(0)
            }
            WM_REFRESH => {
                app.refresh_bookmarks();
                self.sync(app);
                Some(0)
            }
            WM_LBUTTONDOWN => {
                let pt = point(l);
                let frame = Self::frame(app, &crate::pane::content_rect(app, pane));
                if inside(&frame, pt) {
                    // The × (held until the button comes up), or the frame
                    // round the text: the box.
                    if inside(&Self::button(&frame), pt) && self.has_text() {
                        self.pressed.set(true);
                        // The mouse goes to this pane's window until the
                        // button comes up.
                        win::set_capture(pane);
                        // Repainted darker, whether it was lit or not.
                        self.hot.set(false);
                        self.set_hot(app, pane, true);
                    }
                    win::set_focus(self.edit.get());
                    return Some(0);
                }
                None
            }
            WM_MOUSEMOVE => {
                self.mouse_move(app, pane, point(l));
                Some(0)
            }
            WM_MOUSELEAVE => {
                if !self.pressed.get() {
                    self.set_hot(app, pane, false);
                }
                Some(0)
            }
            WM_LBUTTONUP if self.pressed.get() => {
                self.pressed.set(false);
                // Lets go of the mouse the press took, if it is still this
                // pane's.
                if win::capture() == pane {
                    win::release_capture();
                }
                let pt = point(l);
                let over = inside(&Self::button_in(app, pane), pt);
                // Repainted either way: held, it was darker.
                self.hot.set(!over);
                self.set_hot(app, pane, over);
                if over {
                    self.clear();
                    // Hovering on: its leaving is tracked anew.
                    self.mouse_move(app, pane, pt);
                }
                Some(0)
            }
            WM_CAPTURECHANGED => {
                if self.pressed.replace(false) {
                    self.set_hot(app, pane, false);
                }
                Some(0)
            }
            _ => None,
        }
    }

    fn paint(&self, app: &App, hdc: HDC, rc: &RECT) {
        let pal = app.pal.get();
        let ink = pal.ink(
            crate::colors::Item::BookmarksSidebar,
            crate::colors::State::Normal,
        );
        fill(hdc, rc, ink.bg);
        // `rc` is the content's place, below the title row.
        let frame = Self::frame(app, rc);
        fill(hdc, &frame, pal.border);
        let inner = RECT {
            left: frame.left + 1,
            top: frame.top + 1,
            right: frame.right - 1,
            bottom: frame.bottom - 1,
        };
        fill(hdc, &inner, ink.bg);
        let button = Self::button(&frame);
        let cx = (button.left + button.right) / 2;
        let cy = (button.top + button.bottom) / 2;
        let scale = app.dpi.get() as f32 / 96.0;
        if self.has_text() {
            let (fg, bg) = clear_colours(&pal, ink.bg, self.hot.get(), self.pressed.get());
            let face = RECT {
                top: button.top + 1,
                right: button.right - 1,
                bottom: button.bottom - 1,
                ..button
            };
            fill(hdc, &face, bg);
            searchbox::paint_cross(hdc, cx, cy, 3.0, scale, fg, bg);
        } else {
            searchbox::paint_magnifier(hdc, cx, cy, scale, pal.glyph, ink.bg);
        }
    }

    fn theme(&self, app: &App, _dark: bool) {
        self.apply_theme(app);
        win::invalidate(self.tree.get(), None, true);
        win::invalidate(self.edit.get(), None, true);
    }

    fn dpi(&self, app: &App, _dpi: u32) {
        self.fit_dpi(app);
    }

    /// The search bar and one tree row, in the sidebar's font.
    fn least_high(&self, app: &App) -> i32 {
        let bar = Self::bar(
            app,
            &RECT {
                bottom: i32::MAX / 2,
                ..RECT::default()
            },
        );
        bar.bottom + app.part_height(crate::colors::Item::BookmarksSidebar) + app.scale(4)
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::Palette;

    #[test]
    fn the_clear_button_lights_and_darkens() {
        for dark in [true, false] {
            let pal = Palette::new(dark);
            let idle = clear_colours(&pal, pal.bg, false, false);
            let hot = clear_colours(&pal, pal.bg, true, false);
            let held = clear_colours(&pal, pal.bg, true, true);
            // Idle: the box's background; lit and held: each its own.
            assert_eq!(idle, (pal.glyph, pal.bg));
            assert_ne!(hot.1, idle.1);
            assert_ne!(held.1, hot.1);
            // Held but dragged off: as idle.
            assert_eq!(clear_colours(&pal, pal.bg, false, true), idle);
        }
    }
}
