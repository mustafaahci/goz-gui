//! The Folders pane (Everything's Folders sidebar, View > Folders): a tree
//! whose top item stands for no folder (the whole index, as Everything's
//! "Everything" item) and whose other top items are the drives goz can
//! search, each opening onto its folders. Choosing a folder keeps the search
//! inside it (`sidebars.rs`); choosing the top item lets it go.
//!
//! - **Listing.** A folder's subfolders are listed when it first opens, on
//!   a thread of its own (`crate::foldertree`), and again each time it is
//!   opened by hand (or F5 on it), the tree keeping what is still there
//!   (with what is open under it) and dropping what is gone. The subfolders
//!   go in a few hundred at a time, a message each, so the window never
//!   stalls on many of them.
//! - **Big folders.** At most [`foldertree::CAP`] subfolders of a folder go
//!   in the tree (and past them the one a folder being shown lies in); an
//!   item after them says how many more there are, and choosing it keeps
//!   the search inside the folder, where the results list them all. A tree
//!   view's items cost it ever more time to take out again: tens of
//!   thousands of them took seconds to close the pane.
//! - **Closing.** The tree is taken out of the pane before the pane goes,
//!   hidden, so the pane closes at once: as the window ends, the process's
//!   end frees it; otherwise it is emptied a few milliseconds at a time
//!   ([`empty_parked`]), then destroyed.
//! - **The keyboard.** Arrow keys choose a folder once they rest a moment
//!   (a click at once); Enter gives the results the keyboard; Alt+Up goes up
//!   a folder; F5 lists the chosen folder again (and refreshes the
//!   results); Shift+F10 or the menu key opens its menu.
//! - **The menu** of a folder: Expand (or Collapse), Open, Open Path, Copy
//!   Full Path, then Explorer's own menu for it (`crate::shell`), as the
//!   results' row menu has it. Properties and Delete go through goz-gui's
//!   own paths (a test's dry run, the file operations thread); the folder's
//!   parent is listed again after a command that may have changed it.
//! - **Subfolders.** Everything's Subfolders box under the tree. With a
//!   gozd that reads the new query language (`client::boolean_syntax`) it
//!   is a box to tick: unticked, the search keeps to the folder's own
//!   items (`parent:`, `sidebars::scoped_with`). An older gozd can only
//!   search a folder with its subfolders: the box shows on and greyed, and
//!   its tooltip says so.
//!
//! Re-entrancy: the tree sends this pane notifications from inside the
//! messages sent to it (selecting, deleting or opening an item), so no
//! `RefCell` here is borrowed across a message to the tree, a menu or the
//! shell.

use std::any::Any;
use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::rc::Rc;
use std::sync::Arc;

use std::time::{Duration, Instant};

use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows_sys::Win32::Graphics::Gdi::{
    ClientToScreen, CreatePen, DT_END_ELLIPSIS, DT_NOPREFIX, DT_SINGLELINE, DT_VCENTER,
    DeleteObject, DrawTextW, HDC, InvalidateRect, LineTo, MoveToEx, PS_SOLID, ScreenToClient,
    SelectObject, SetBkMode, SetTextColor, TRANSPARENT,
};
use windows_sys::Win32::System::Diagnostics::Debug::MessageBeep;
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::Controls::{
    HTREEITEM, NMHDR, NMTREEVIEWW, TOOLTIPS_CLASSW, TTF_SUBCLASS, TTM_ADDTOOLW, TTM_NEWTOOLRECTW,
    TTM_SETMAXTIPWIDTH, TTM_UPDATETIPTEXTW, TTS_ALWAYSTIP, TTS_NOPREFIX, TTTOOLINFOW, TVC_BYMOUSE,
    TVE_EXPAND, TVM_DELETEITEM, TVM_SETEXTENDEDSTYLE, TVN_ITEMEXPANDINGW, TVN_SELCHANGEDW,
    TVS_EX_AUTOHSCROLL, TVS_EX_DOUBLEBUFFER, TVS_FULLROWSELECT, TVS_HASBUTTONS, TVS_LINESATROOT,
    TVS_NOHSCROLL, TVS_NONEVENHEIGHT, TVS_SHOWSELALWAYS, TVS_TRACKSELECT, WC_TREEVIEWW,
};
use windows_sys::Win32::UI::HiDpi::GetSystemMetricsForDpi;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    VK_CONTROL, VK_F5, VK_RETURN, VK_SHIFT, VK_UP,
};
use windows_sys::Win32::UI::Shell::{DefSubclassProc, RemoveWindowSubclass, SetWindowSubclass};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CW_USEDEFAULT, CreateWindowExW, DeleteMenu, DestroyMenu, DestroyWindow, GetMenuDefaultItem,
    HMENU, HWND_MESSAGE, KillTimer, MB_ICONWARNING, MF_BYCOMMAND, MSG, MoveWindow, PostMessageW,
    SM_CXSMICON, SW_HIDE, SendMessageW, SetParent, SetTimer, ShowWindow, WM_APP, WM_CONTEXTMENU,
    WM_KEYDOWN, WM_LBUTTONDBLCLK, WM_LBUTTONDOWN, WM_NOTIFY, WM_SETFOCUS, WM_SYSKEYDOWN, WM_TIMER,
    WS_CHILD, WS_EX_TOPMOST, WS_POPUP, WS_TABSTOP, WS_VISIBLE,
};

use super::sidebars::{FOLDER_COPY_PATH, FOLDER_EXPAND, FOLDER_OPEN, FOLDER_OPEN_PATH};
use super::{App, key_down, point};
use crate::actions::{self, Target};
use crate::bookmark_dialogs::Host;
use crate::fileops::Job as FileJob;
use crate::foldertree::{self, Folder, Job, Reply, Root, Step};
use crate::menu::{self, Item};
use crate::paint::fill;
use crate::shell;
use crate::store::location;
use crate::theme;
use crate::treeview::{self, Images, NONE, New, Source};
use crate::wide::wz;

/// A thread's listing (`foldertree::Reply` in lParam).
const WM_FOUND: u32 = WM_APP + 1;
/// The next few hundred subfolders of a big listing go in.
const WM_MORE: u32 = WM_APP + 2;
/// The pane is open (its framework is done opening it).
const WM_OPENED: u32 = WM_APP + 3;
/// The folder the keyboard rests on is chosen.
const TIMER_CHOOSE: usize = 1;
const CHOOSE_MS: u32 = 150;
/// Folders a menu command may have changed are listed again.
const TIMER_RELIST: usize = 2;
const RELIST_MS: u32 = 1500;
/// A closed pane's tree is emptied every `PARK_MS`, for at most
/// `PARK_SLICE` each time.
const PARK_TIMER: usize = 1;
const PARK_MS: u32 = 10;
const PARK_SLICE: Duration = Duration::from_millis(8);
/// How long one batch of items going into the tree may hold the window's
/// thread, and the sizes a batch starts at and keeps within.
const BATCH_BUDGET: Duration = Duration::from_millis(25);
const BATCH_FIRST: usize = 300;
const BATCH_MIN: usize = 20;
const BATCH_MAX: usize = 5000;

/// The next batch's size after one of `size` items took `took`: smaller in
/// proportion when it went over [`BATCH_BUDGET`], twice as big when it took
/// under a third of it.
fn next_batch(size: usize, took: Duration) -> usize {
    let budget = BATCH_BUDGET.as_secs_f64();
    let took = took.as_secs_f64();
    let next = if took > budget {
        (size as f64 * budget / took) as usize
    } else if took < budget / 3.0 {
        size * 2
    } else {
        size
    };
    next.clamp(BATCH_MIN, BATCH_MAX)
}
/// The tree's child id in the pane.
const TREE_ID: usize = 1;
/// The Subfolders bar under the tree, Everything's: 22 px at 100 %.
const BAR_HIGH: i32 = 22;
/// The top item's node: no folder.
const TOP: u32 = 0;
/// What the Subfolders box's tooltip says.
const SUBFOLDERS_TIP: &str = "goz searches a folder with its subfolders. Searching only the \
                              folder's own items needs a newer gozd.";
/// The same with a gozd that reads `parent:`.
const SUBFOLDERS_TIP_ON: &str = "Search the folder's subfolders too. Unticked, only the \
                                 folder's own items (parent:).";

/// The tooltip as gozd's syntax says.
fn subfolders_tip() -> &'static str {
    if crate::client::boolean_syntax() {
        SUBFOLDERS_TIP_ON
    } else {
        SUBFOLDERS_TIP
    }
}

/// How far a node's subfolders are.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Load {
    /// Not asked for yet.
    Unlisted,
    /// A thread is listing them for the first time.
    Listing,
    /// Going into the tree a batch at a time.
    Inserting,
    /// In the tree (and maybe being listed again).
    Listed,
}

/// An item of the tree.
struct Node {
    /// A folder's name; a drive's root (`C:\`); nothing for the top item.
    name: Vec<u16>,
    parent: Option<u32>,
    item: HTREEITEM,
    /// Its subfolders' nodes, in the tree's order.
    children: Vec<u32>,
    load: Load,
    /// The listing wanted: an older one's answer is dropped.
    request: u64,
    /// Taken out of the tree.
    gone: bool,
    /// The user opened it before its subfolders were in (the tree cannot
    /// show open an item with nothing under it): it opens once they come.
    open_when_listed: bool,
    /// The listing whose answer came last (`request` while none is on its
    /// way).
    answered: u64,
    /// How many of its subfolders the tree leaves out (past
    /// [`foldertree::CAP`]), and the node of the item that counts them.
    hidden: usize,
    more: Option<u32>,
    /// It is such an item: it stands for its parent's folder.
    is_more: bool,
    /// The name under it of the folder being shown as it was listed last,
    /// which went in whatever its place.
    sought: Vec<u16>,
}

/// The text of the item counting `n` subfolders the tree leaves out.
fn more_label(n: usize) -> Vec<u16> {
    let text = if n == 1 {
        "1 more folder".to_owned()
    } else {
        format!("{} more folders", crate::format::grouped(n as u64))
    };
    text.encode_utf16().collect()
}

/// Subfolders still to go into the tree.
struct Batch {
    node: u32,
    folders: VecDeque<Folder>,
}

pub(super) struct FoldersPane {
    pane: Cell<HWND>,
    tree: Cell<HWND>,
    /// The Subfolders box's tooltip.
    tip: Cell<HWND>,
    nodes: RefCell<Vec<Node>>,
    images: RefCell<Option<Images>>,
    /// The system image list's plain folder icon.
    plain_folder: Cell<i32>,
    /// Cleared as the pane closes: the threads post nothing more.
    alive: foldertree::Alive,
    requests: Cell<u64>,
    /// The drives are in.
    roots_listed: Cell<bool>,
    /// The tree's selection or opening is goz-gui's, not the user's.
    syncing: Cell<bool>,
    /// The folder to show chosen once the folders above it are listed.
    reveal: RefCell<Option<Vec<u16>>>,
    batches: RefCell<VecDeque<Batch>>,
    /// Nodes to list again when TIMER_RELIST goes off.
    relist: RefCell<Vec<u32>>,
    /// How many items the next batch puts in (`next_batch`).
    batch: Cell<usize>,
}

const fn utf16(c: u8) -> u16 {
    c as u16
}

impl FoldersPane {
    pub(super) fn new() -> FoldersPane {
        FoldersPane {
            pane: Cell::new(core::ptr::null_mut()),
            tree: Cell::new(core::ptr::null_mut()),
            tip: Cell::new(core::ptr::null_mut()),
            nodes: RefCell::new(Vec::new()),
            images: RefCell::new(None),
            plain_folder: Cell::new(0),
            alive: foldertree::alive(),
            requests: Cell::new(0),
            roots_listed: Cell::new(false),
            syncing: Cell::new(false),
            reveal: RefCell::new(None),
            batches: RefCell::new(VecDeque::new()),
            relist: RefCell::new(Vec::new()),
            batch: Cell::new(BATCH_FIRST),
        }
    }

    fn send(&self, msg: u32, w: usize, l: isize) -> isize {
        // SAFETY: a message to this pane's tree; any pointer in `l` is the
        // caller's and lives across the call.
        unsafe { SendMessageW(self.tree.get(), msg, w, l) }
    }

    /// Runs `f` with the tree's notifications taken as goz-gui's own.
    fn quietly<R>(&self, f: impl FnOnce() -> R) -> R {
        let was = self.syncing.replace(true);
        let r = f();
        self.syncing.set(was);
        r
    }

    /// The Subfolders bar's place in the content `rc`.
    fn bar(app: &App, rc: &RECT) -> RECT {
        RECT {
            top: (rc.bottom - app.scale(BAR_HIGH)).max(rc.top),
            ..*rc
        }
    }

    /// The Subfolders box and its label in the bar: where the tooltip is.
    fn subfolders_rect(app: &App, bar: &RECT) -> RECT {
        RECT {
            left: bar.left + app.scale(2),
            right: (bar.left + app.scale(96)).min(bar.right),
            top: bar.top + 1,
            bottom: bar.bottom,
        }
    }

    /// The image of the system image list's icon `sys` (the plain folder's
    /// without one).
    fn image(&self, sys: Option<i32>) -> i32 {
        let sys = sys.unwrap_or(self.plain_folder.get());
        self.images
            .borrow_mut()
            .as_mut()
            .map_or(-1, |i| i.get(Source::System(sys)))
    }

    /// The full path of node `n` (`None` for the top item); a "more
    /// folders" item's is its folder's.
    fn path(&self, n: u32) -> Option<Vec<u16>> {
        let nodes = self.nodes.borrow();
        let n = match nodes.get(n as usize) {
            Some(node) if node.is_more => node.parent?,
            _ => n,
        };
        let mut parts = Vec::new();
        let mut at = Some(n);
        while let Some(i) = at {
            let node = nodes.get(i as usize)?;
            if i == TOP {
                return None;
            }
            parts.push(i);
            at = node.parent;
        }
        let mut path: Vec<u16> = Vec::new();
        for &i in parts.iter().rev() {
            let name = &nodes[i as usize].name;
            path = if path.is_empty() {
                name.clone()
            } else {
                foldertree::join(&path, name)
            };
        }
        Some(path)
    }

    /// The node of a tree item.
    fn node_of(&self, item: HTREEITEM) -> Option<u32> {
        let n = u32::try_from(treeview::param(self.tree.get(), item)?).ok()?;
        let nodes = self.nodes.borrow();
        nodes
            .get(n as usize)
            .filter(|node| !node.gone && node.item == item)
            .map(|_| n)
    }

    /// Puts a node in the tree under `parent` (`None`: the top level) after
    /// `after` (`Some(NONE)`: first; `None`: last); its index.
    fn add(
        &self,
        parent: Option<u32>,
        after: Option<HTREEITEM>,
        name: Vec<u16>,
        text: &[u16],
        image: i32,
        children: bool,
    ) -> Option<u32> {
        let (parent_item, at) = {
            let nodes = self.nodes.borrow();
            (
                parent.map_or(NONE, |p| nodes[p as usize].item),
                nodes.len() as u32,
            )
        };
        let item = treeview::insert(
            self.tree.get(),
            &New {
                parent: parent_item,
                after,
                text,
                image,
                children,
                param: at as isize,
            },
        );
        if item == NONE {
            return None;
        }
        let mut nodes = self.nodes.borrow_mut();
        // Inserting sends this pane nothing, so the index still holds.
        debug_assert_eq!(nodes.len() as u32, at);
        nodes.push(Node {
            name,
            parent,
            item,
            children: Vec::new(),
            load: Load::Unlisted,
            request: 0,
            gone: false,
            open_when_listed: false,
            answered: 0,
            hidden: 0,
            more: None,
            is_more: false,
            sought: Vec::new(),
        });
        Some(at)
    }

    /// Asks a thread for node `n`'s subfolders (again, for one listed).
    fn list(&self, n: u32) {
        let Some(path) = self.path(n) else {
            return;
        };
        let request = self.requests.get() + 1;
        {
            let mut nodes = self.nodes.borrow_mut();
            let node = &mut nodes[n as usize];
            if node.gone || node.is_more || node.load == Load::Inserting {
                return;
            }
            if node.load == Load::Unlisted {
                node.load = Load::Listing;
            }
            node.request = request;
        }
        self.requests.set(request);
        foldertree::spawn(
            Job::Folders {
                node: n,
                request,
                path,
            },
            self.pane.get(),
            WM_FOUND,
            Arc::clone(&self.alive),
        );
    }

    /// The drives came: in after the top item.
    fn roots(&self, app: &App, roots: Vec<Root>) {
        if self.roots_listed.replace(true) {
            return;
        }
        for root in roots {
            let image = self.image(root.icon);
            self.add(
                None,
                None,
                root.path,
                &root.label,
                image,
                root.has_folders != Some(false),
            );
        }
        self.continue_reveal(app);
    }

    /// The name under node `n` of the folder being shown, if it is inside
    /// it.
    fn reveal_name(&self, n: u32) -> Option<Vec<u16>> {
        let target = self.reveal.borrow().clone()?;
        let path = self.path(n)?;
        foldertree::next_name(&target, &path).map(<[u16]>::to_vec)
    }

    /// Node `node`'s subfolders came (listing `request`).
    fn folders(&self, app: &App, node: u32, request: u64, found: Result<Vec<Folder>, u32>) {
        let (load, old_names, item) = {
            let mut nodes = self.nodes.borrow_mut();
            let Some(n) = nodes.get_mut(node as usize) else {
                return;
            };
            if n.gone || n.request != request {
                return;
            }
            n.answered = request;
            let (load, item, children) = (n.load, n.item, n.children.clone());
            let names: Vec<Vec<u16>> = children
                .iter()
                .map(|&c| nodes[c as usize].name.clone())
                .collect();
            (load, names, item)
        };
        if load == Load::Inserting {
            return;
        }
        let has_children = !old_names.is_empty();
        // Past the cap, only what the tree must show goes in: the folders
        // it shows already (with what is open under them) and the one on
        // the way to the folder being shown.
        let sought = self.reveal_name(node);
        let mut wanted: Vec<&[u16]> = old_names.iter().map(Vec::as_slice).collect();
        wanted.extend(sought.as_deref());
        let (found, hidden) =
            foldertree::capped(found.unwrap_or_default(), foldertree::CAP, &wanted);
        {
            let mut nodes = self.nodes.borrow_mut();
            nodes[node as usize].hidden = hidden;
            nodes[node as usize].sought = sought.unwrap_or_default();
        }
        if !has_children {
            if found.is_empty() {
                {
                    let mut nodes = self.nodes.borrow_mut();
                    nodes[node as usize].load = Load::Listed;
                    nodes[node as usize].open_when_listed = false;
                }
                self.sync_more(node);
                treeview::set_children(self.tree.get(), item, false);
                self.continue_reveal(app);
                return;
            }
            self.nodes.borrow_mut()[node as usize].load = Load::Inserting;
            self.batches.borrow_mut().push_back(Batch {
                node,
                folders: found.into(),
            });
            self.more(app);
            return;
        }
        self.merge(node, found);
        self.sync_more(node);
        self.nodes.borrow_mut()[node as usize].load = Load::Listed;
        // The folder chosen may have gone from under it, or come back.
        if self.leads_to_choice(app, node) {
            self.follow(app);
        }
    }

    /// Whether node `n` is the folder the window keeps or one above it.
    fn leads_to_choice(&self, app: &App, n: u32) -> bool {
        let (Some(folder), Some(path)) = (app.folder_selection(), self.path(n)) else {
            return false;
        };
        let folder: Vec<u16> = folder.encode_utf16().collect();
        let k = path.len();
        if folder.len() < k || !foldertree::same_name(&folder[..k], &path) {
            return false;
        }
        // The folder itself, or one inside it (a drive's root ends with its
        // backslash already).
        folder.len() == k || path.last() == Some(&utf16(b'\\')) || folder[k] == utf16(b'\\')
    }

    /// Node `n` was opened before its subfolders were in: they are listed
    /// (unless they are on their way), and it opens as they come.
    fn open_when_listed(&self, n: u32) {
        let load = {
            let mut nodes = self.nodes.borrow_mut();
            nodes[n as usize].open_when_listed = true;
            nodes[n as usize].load
        };
        if load == Load::Unlisted {
            self.list(n);
        }
    }

    /// Puts the next batch of subfolders in; the rest come with the next
    /// message. A batch is as big as goes in within [`BATCH_BUDGET`] on the
    /// window's thread, as the last ones measured it.
    fn more(&self, app: &App) {
        let next = {
            let mut batches = self.batches.borrow_mut();
            let Some(batch) = batches.front_mut() else {
                return;
            };
            let take = batch.folders.len().min(self.batch.get());
            let part: Vec<Folder> = batch.folders.drain(..take).collect();
            let node = batch.node;
            if batch.folders.is_empty() {
                batches.pop_front();
            }
            (node, part, !batches.is_empty())
        };
        let (node, part, again) = next;
        if self.nodes.borrow()[node as usize].gone {
            self.after_more(again);
            return;
        }
        let mut added = Vec::with_capacity(part.len());
        let started = Instant::now();
        self.quietly(|| {
            for f in part {
                let image = self.image(f.icon);
                let children = f.has_folders != Some(false);
                if let Some(id) =
                    self.add(Some(node), None, f.name.clone(), &f.name, image, children)
                {
                    added.push(id);
                }
            }
        });
        self.batch
            .set(next_batch(self.batch.get(), started.elapsed()));
        let (done, open) = {
            let mut nodes = self.nodes.borrow_mut();
            let n = &mut nodes[node as usize];
            n.children.extend(added);
            let done = !self.batches.borrow().iter().any(|b| b.node == node);
            if done {
                n.load = Load::Listed;
            }
            // Opened by the user before its subfolders came: it opens once
            // they are all in. A closed item takes them many times faster
            // than an open one, which works out what shows at each.
            let open = (done && n.open_when_listed && !n.children.is_empty()).then_some(n.item);
            if done {
                n.open_when_listed = false;
            }
            (done.then_some((n.item, n.children.is_empty())), open)
        };
        if let Some(item) = open {
            self.quietly(|| treeview::expand(self.tree.get(), item, true));
        }
        if let Some((item, empty)) = done {
            if empty {
                treeview::set_children(self.tree.get(), item, false);
            }
            // After the last of them: how many more there are.
            self.sync_more(node);
            self.continue_reveal(app);
        }
        self.after_more(again);
    }

    /// The item counting node `n`'s subfolders the tree leaves out
    /// (`Node::hidden`) comes, says the count, or goes. It is the last of
    /// its subfolders: what goes in after it is put before it.
    fn sync_more(&self, n: u32) {
        let (hidden, more) = {
            let nodes = self.nodes.borrow();
            let node = &nodes[n as usize];
            if node.gone {
                return;
            }
            (node.hidden, node.more)
        };
        let tree = self.tree.get();
        match (hidden, more) {
            (0, None) => {}
            (0, Some(m)) => {
                let item = {
                    let mut nodes = self.nodes.borrow_mut();
                    nodes[n as usize].more = None;
                    Self::forget(&mut nodes, m);
                    nodes[m as usize].item
                };
                self.quietly(|| treeview::delete(tree, item));
            }
            (h, Some(m)) => {
                let item = self.nodes.borrow()[m as usize].item;
                treeview::set_text(tree, item, &more_label(h));
            }
            (h, None) => {
                let image = self.image(None);
                let id = self
                    .quietly(|| self.add(Some(n), None, Vec::new(), &more_label(h), image, false));
                if let Some(id) = id {
                    let mut nodes = self.nodes.borrow_mut();
                    nodes[id as usize].is_more = true;
                    nodes[n as usize].more = Some(id);
                }
            }
        }
    }

    /// More batches wait: the next one comes with a message of its own.
    fn after_more(&self, again: bool) {
        if again {
            // SAFETY: posts to this pane's own window.
            unsafe { PostMessageW(self.pane.get(), WM_MORE, 0, 0) };
        }
    }

    /// Node `n` and everything under it are out of the tree.
    fn forget(nodes: &mut [Node], n: u32) {
        let mut stack = vec![n];
        while let Some(i) = stack.pop() {
            let node = &mut nodes[i as usize];
            node.gone = true;
            stack.append(&mut node.children);
            stack.extend(node.more.take());
        }
    }

    /// Node `n`'s subfolders listed again as `found`: what is gone goes,
    /// what is new comes in its place, the rest stays as it is (with what
    /// is open under it).
    fn merge(&self, n: u32, found: Vec<Folder>) {
        let (old_ids, old_names): (Vec<u32>, Vec<Vec<u16>>) = {
            let nodes = self.nodes.borrow();
            nodes[n as usize]
                .children
                .iter()
                .map(|&c| (c, nodes[c as usize].name.clone()))
                .unzip()
        };
        let old: Vec<&[u16]> = old_names.iter().map(Vec::as_slice).collect();
        let new: Vec<&[u16]> = found.iter().map(|f| f.name.as_slice()).collect();
        let (steps, gone) = foldertree::merge(&old, &new);
        let tree = self.tree.get();
        self.quietly(|| {
            for &g in &gone {
                let item = {
                    let mut nodes = self.nodes.borrow_mut();
                    let item = nodes[old_ids[g] as usize].item;
                    Self::forget(&mut nodes, old_ids[g]);
                    item
                };
                treeview::delete(tree, item);
            }
            let mut prev = NONE;
            let mut order = Vec::with_capacity(steps.len());
            // A step for each new folder, in their order.
            for (j, step) in steps.into_iter().enumerate() {
                match step {
                    Step::Keep(i) => {
                        let id = old_ids[i];
                        let f = &found[j];
                        let (item, load, name) = {
                            let nodes = self.nodes.borrow();
                            let node = &nodes[id as usize];
                            (node.item, node.load, node.name.clone())
                        };
                        if name != f.name {
                            // Renamed in case only.
                            treeview::set_text(tree, item, &f.name);
                            self.nodes.borrow_mut()[id as usize].name = f.name.clone();
                        }
                        if load == Load::Unlisted {
                            treeview::set_children(tree, item, f.has_folders != Some(false));
                        }
                        prev = item;
                        order.push(id);
                    }
                    Step::Insert(new) => {
                        let f = &found[new];
                        let image = self.image(f.icon);
                        let children = f.has_folders != Some(false);
                        if let Some(id) = self.add(
                            Some(n),
                            Some(prev),
                            f.name.clone(),
                            &f.name,
                            image,
                            children,
                        ) {
                            prev = self.nodes.borrow()[id as usize].item;
                            order.push(id);
                        }
                    }
                }
            }
            let item = {
                let mut nodes = self.nodes.borrow_mut();
                let empty = order.is_empty();
                nodes[n as usize].children = order;
                empty.then_some(nodes[n as usize].item)
            };
            if let Some(item) = item {
                treeview::set_children(tree, item, false);
            }
        });
    }

    /// The tree shows the folder the window keeps (`App::folder_selection`)
    /// chosen, listing the folders above it as needed; the top item for
    /// none. A folder the tree cannot reach (gone, hidden) leaves nothing
    /// chosen.
    pub(super) fn follow(&self, app: &App) {
        match app.folder_selection() {
            None => {
                *self.reveal.borrow_mut() = None;
                let top = self.nodes.borrow().first().map_or(NONE, |n| n.item);
                if treeview::selected(self.tree.get()) != top {
                    self.quietly(|| treeview::select(self.tree.get(), top));
                }
            }
            Some(folder) => {
                *self.reveal.borrow_mut() = Some(folder.encode_utf16().collect());
                self.continue_reveal(app);
            }
        }
    }

    /// Goes on showing the folder `reveal` holds: down from its drive as
    /// far as the folders are listed, listing the next one when it is not.
    fn continue_reveal(&self, _app: &App) {
        let Some(target) = self.reveal.borrow().clone() else {
            return;
        };
        let tree = self.tree.get();
        let give_up = || {
            *self.reveal.borrow_mut() = None;
            self.quietly(|| treeview::select(tree, NONE));
        };
        // `X:\` and the names after it.
        if target.len() < 3 || target[1] != utf16(b':') || target[2] != utf16(b'\\') {
            give_up();
            return;
        }
        let drive = {
            let nodes = self.nodes.borrow();
            nodes.iter().enumerate().find_map(|(i, n)| {
                (i as u32 != TOP
                    && n.parent.is_none()
                    && !n.gone
                    && foldertree::same_name(&n.name, &target[..3]))
                .then_some(i as u32)
            })
        };
        let Some(mut at) = drive else {
            if self.roots_listed.get() {
                give_up();
            }
            return;
        };
        for name in target[3..]
            .split(|&c| c == utf16(b'\\'))
            .filter(|n| !n.is_empty())
        {
            let (load, item, child) = {
                let nodes = self.nodes.borrow();
                let node = &nodes[at as usize];
                let child = node.children.iter().copied().find(|&c| {
                    let n = &nodes[c as usize];
                    !n.gone && foldertree::same_name(&n.name, name)
                });
                (node.load, node.item, child)
            };
            match load {
                Load::Listed => match child {
                    Some(c) => {
                        if !treeview::is_expanded(tree, item) {
                            self.quietly(|| treeview::expand(tree, item, true));
                        }
                        at = c;
                    }
                    None => {
                        // Among the subfolders the tree leaves out, maybe:
                        // listed again (once for this name), it goes in.
                        let (hidden, sought, waiting) = {
                            let nodes = self.nodes.borrow();
                            let node = &nodes[at as usize];
                            (
                                node.hidden,
                                foldertree::same_name(&node.sought, name),
                                node.request != node.answered,
                            )
                        };
                        if hidden > 0 && waiting {
                            return;
                        }
                        if hidden > 0 && !sought {
                            self.nodes.borrow_mut()[at as usize].sought = name.to_vec();
                            self.list(at);
                            return;
                        }
                        give_up();
                        return;
                    }
                },
                Load::Unlisted => {
                    self.list(at);
                    return;
                }
                Load::Listing | Load::Inserting => return,
            }
        }
        *self.reveal.borrow_mut() = None;
        let (item, more) = {
            let nodes = self.nodes.borrow();
            let node = &nodes[at as usize];
            (node.item, node.more.map(|m| nodes[m as usize].item))
        };
        let selected = treeview::selected(tree);
        // Chosen through its "more folders" item, which stays chosen.
        if selected != item && Some(selected) != more {
            self.quietly(|| treeview::select(tree, item));
        }
    }

    /// The tree's selection, as the user left it, chooses the folder.
    fn choose_selected(&self, app: &App) {
        let Some(n) = self.node_of(treeview::selected(self.tree.get())) else {
            return;
        };
        let folder = self.path(n).map(|p| String::from_utf16_lossy(&p));
        app.choose_folder(folder);
    }

    /// A tree notification.
    fn notify(&self, app: &App, l: LPARAM) -> Option<LRESULT> {
        // SAFETY: the tree's notifications carry an NMHDR, and these codes an
        // NMTREEVIEWW.
        let hdr = unsafe { &*(l as *const NMHDR) };
        if hdr.hwndFrom != self.tree.get() {
            return None;
        }
        match hdr.code {
            TVN_ITEMEXPANDINGW => {
                // SAFETY: as above.
                let nm = unsafe { &*(l as *const NMTREEVIEWW) };
                if nm.action == TVE_EXPAND
                    && let Ok(n) = u32::try_from(nm.itemNew.lParam)
                {
                    let load = self.nodes.borrow().get(n as usize).map(|node| node.load);
                    match load {
                        Some(Load::Unlisted | Load::Listing | Load::Inserting) => {
                            self.open_when_listed(n);
                        }
                        // Opened by hand: listed again, so it shows what is
                        // there now. One opened on the way to a folder was
                        // just listed.
                        Some(Load::Listed) if !self.syncing.get() => self.list(n),
                        _ => {}
                    }
                }
                Some(0)
            }
            TVN_SELCHANGEDW => {
                if self.syncing.get() {
                    return Some(0);
                }
                // SAFETY: as above.
                let nm = unsafe { &*(l as *const NMTREEVIEWW) };
                // The user went elsewhere: a folder being shown is not
                // wanted any more.
                *self.reveal.borrow_mut() = None;
                let pane = self.pane.get();
                if nm.action == TVC_BYMOUSE {
                    // SAFETY: a timer of this pane's window.
                    unsafe { KillTimer(pane, TIMER_CHOOSE) };
                    self.choose_selected(app);
                } else {
                    // SAFETY: as above; setting it again restarts it.
                    unsafe { SetTimer(pane, TIMER_CHOOSE, CHOOSE_MS, None) };
                }
                Some(0)
            }
            _ => None,
        }
    }

    /// Lists `nodes` again in a moment: a menu command may have changed
    /// them.
    fn relist_soon(&self, nodes: &[u32]) {
        self.relist.borrow_mut().extend_from_slice(nodes);
        // SAFETY: a timer of this pane's window; setting it again restarts
        // it.
        unsafe { SetTimer(self.pane.get(), TIMER_RELIST, RELIST_MS, None) };
    }

    /// The context menu of the folder at `l` (screen), or of the chosen one
    /// from the keyboard (-1).
    fn menu(&self, app: &App, l: LPARAM) {
        let tree = self.tree.get();
        let (item, pt) = if l == -1 {
            let item = treeview::selected(tree);
            let rc = treeview::item_rect(tree, item, true);
            let mut pt = POINT {
                x: rc.left,
                y: rc.bottom,
            };
            // SAFETY: converts a point of this pane's tree.
            unsafe { ClientToScreen(tree, &mut pt) };
            (item, pt)
        } else {
            let pt = point(l);
            let mut at = pt;
            // SAFETY: converts a screen point to this pane's tree's.
            unsafe { ScreenToClient(tree, &mut at) };
            (treeview::hit(tree, at).0, pt)
        };
        let Some(n) = self.node_of(item) else {
            return;
        };
        if self.nodes.borrow()[n as usize].is_more {
            return;
        }
        let Some(path) = self.path(n) else {
            // The top item has nothing to act on.
            return;
        };
        // Expand while it shows a button (or holds folders).
        let can_expand = treeview::has_children(tree, item)
            || !self.nodes.borrow()[n as usize].children.is_empty();
        let expanded = treeview::is_expanded(tree, item);
        let mut items = Vec::new();
        if can_expand {
            items.push(Item::Cmd(
                FOLDER_EXPAND,
                if expanded { "Collap&se" } else { "Exp&and" },
            ));
            items.push(Item::Sep);
        }
        items.extend([
            Item::Cmd(FOLDER_OPEN, "&Open"),
            Item::Cmd(FOLDER_OPEN_PATH, "Open &Path"),
            Item::Sep,
            Item::Cmd(FOLDER_COPY_PATH, "Copy Full Path"),
            Item::Sep,
        ]);
        // The shell's part first, then goz-gui's items at the top: some
        // handlers put theirs first whatever place they are given.
        let popup = menu::submenu(&[]);
        let shell = shell_part(app, popup, &path, key_down(VK_SHIFT));
        menu::add_items(popup, None, &items);
        menu::set_default(
            popup,
            if can_expand {
                FOLDER_EXPAND
            } else {
                FOLDER_OPEN
            },
        );
        super::context::tidy_separators(popup);
        let marked = item != treeview::selected(tree);
        if marked {
            treeview::drop_highlight(tree, item);
        }
        let id = app.track_with_shell(popup, pt, "folder menu", shell.as_ref());
        if marked {
            treeview::drop_highlight(tree, NONE);
        }
        match (id, &shell) {
            (0, _) => {}
            (id, Some(cm)) if cm.owns(id, u32::from(menu::SHELL_LAST)) => {
                self.run_shell_item(app, cm, id, pt, n, path);
            }
            (id, _) => self.command(app, id as u16, n, item, path),
        }
        // SAFETY: the menu made here; the shell's handlers are done with it
        // once its command ran.
        unsafe { DestroyMenu(popup) };
    }

    /// One of the folder menu's own items, for node `n` (`item`) at `path`.
    fn command(&self, app: &App, id: u16, n: u32, item: HTREEITEM, path: Vec<u16>) {
        match id {
            FOLDER_EXPAND => {
                let tree = self.tree.get();
                if treeview::is_expanded(tree, item) {
                    treeview::expand(tree, item, false);
                } else {
                    if self.nodes.borrow()[n as usize].load != Load::Listed {
                        self.open_when_listed(n);
                    }
                    treeview::expand(tree, item, true);
                }
            }
            FOLDER_OPEN => actions::open(app.hwnd, vec![Target { path, is_dir: true }]),
            FOLDER_OPEN_PATH => actions::reveal(vec![path]),
            FOLDER_COPY_PATH => {
                // The window's own copy: it says so when the clipboard is
                // busy.
                Host::copy(app, &String::from_utf16_lossy(&path));
            }
            _ => {}
        }
    }

    /// The shell's item `id` of `cm`, chosen at `pt` for node `n` at `path`.
    /// Properties and Delete take goz-gui's own ways (a dry run in a test;
    /// the file operations thread, which asks as Windows does); the others
    /// run in the shell. The folder and its parent are listed again after.
    fn run_shell_item(
        &self,
        app: &App,
        cm: &shell::ContextMenu,
        id: u32,
        pt: POINT,
        n: u32,
        path: Vec<u16>,
    ) {
        let verb = cm.verb(id).unwrap_or_default().to_ascii_lowercase();
        match verb.as_str() {
            "properties" => actions::properties(app.hwnd, vec![path]),
            "delete" => {
                let job = FileJob::Delete {
                    paths: vec![path],
                    permanent: key_down(VK_SHIFT),
                    next: None,
                };
                if !app.fileops.run(job) {
                    // SAFETY: plays a system sound.
                    unsafe { MessageBeep(MB_ICONWARNING) };
                }
            }
            _ => {
                let mask = shell::invoke_mask(
                    key_down(VK_CONTROL),
                    key_down(VK_SHIFT),
                    location::fileop_silent(),
                );
                cm.invoke(app.hwnd, id, pt, mask);
            }
        }
        let parent = self.nodes.borrow()[n as usize].parent;
        let mut again = vec![n];
        again.extend(parent);
        self.relist_soon(&again);
    }

    /// gozd's syntax or the Subfolders setting changed: the box and its
    /// tooltip follow.
    pub(super) fn syntax_changed(&self, app: &App) {
        let pane = self.pane.get();
        let tip = self.tip.get();
        let rc = crate::pane::content_rect(app, pane);
        let bar = Self::bar(app, &rc);
        // SAFETY: updates this pane's tooltip (which copies the text) and
        // repaints its bar.
        unsafe {
            if !tip.is_null() {
                let mut text = wz(subfolders_tip());
                let info = self.tool(Self::subfolders_rect(app, &bar), &mut text);
                SendMessageW(tip, TTM_UPDATETIPTEXTW, 0, (&raw const info) as isize);
            }
            InvalidateRect(pane, &bar, 0);
        }
    }

    /// Paints the Subfolders bar: Everything's box and its label; ticked
    /// and greyed while gozd cannot leave the subfolders out.
    fn paint_bar(&self, app: &App, hdc: HDC, bar: &RECT) {
        let pal = app.pal.get();
        fill(hdc, bar, pal.chrome);
        fill(
            hdc,
            &RECT {
                bottom: bar.top + 1,
                ..*bar
            },
            pal.line,
        );
        let side = app.scale(13);
        let left = bar.left + app.scale(4);
        let top = bar.top + 1 + (bar.bottom - bar.top - 1 - side) / 2;
        let live = crate::client::boolean_syntax();
        let ticked = !live || app.sidebar_settings().subfolders;
        let colour = if live { pal.fg } else { pal.dim };
        let edge = |l: i32, t: i32, r: i32, b: i32| {
            fill(
                hdc,
                &RECT {
                    left: l,
                    top: t,
                    right: r,
                    bottom: b,
                },
                colour,
            );
        };
        let px = app.scale(1).max(1);
        edge(left, top, left + side, top + px);
        edge(left, top + side - px, left + side, top + side);
        edge(left, top, left + px, top + side);
        edge(left + side - px, top, left + side, top + side);
        // The tick: down to the bottom third, then up to the right.
        let s = |v: i32| app.scale(v);
        // SAFETY: a pen made and deleted here, selected out again before.
        if ticked {
            unsafe {
                let pen = CreatePen(PS_SOLID, s(2).max(1), colour);
                let old = SelectObject(hdc, pen);
                MoveToEx(hdc, left + s(3), top + s(6), core::ptr::null_mut());
                LineTo(hdc, left + s(5), top + s(9));
                LineTo(hdc, left + s(10), top + s(3));
                SelectObject(hdc, old);
                DeleteObject(pen);
            }
        }
        let mut text: Vec<u16> = "Subfolders".encode_utf16().collect();
        let mut rc = RECT {
            left: left + side + app.scale(5),
            right: bar.right - app.scale(2),
            top: bar.top + 1,
            bottom: bar.bottom,
        };
        // SAFETY: draws into the paint buffer; the old font is put back.
        unsafe {
            let old = SelectObject(hdc, app.ui_font());
            SetBkMode(hdc, TRANSPARENT as i32);
            SetTextColor(hdc, colour);
            DrawTextW(
                hdc,
                text.as_mut_ptr(),
                text.len() as i32,
                &mut rc,
                DT_SINGLELINE | DT_VCENTER | DT_END_ELLIPSIS | DT_NOPREFIX,
            );
            SelectObject(hdc, old);
        }
    }

    /// The tooltip's tool over the Subfolders box.
    fn tool(&self, rect: RECT, text: &mut [u16]) -> TTTOOLINFOW {
        TTTOOLINFOW {
            cbSize: size_of::<TTTOOLINFOW>() as u32,
            uFlags: TTF_SUBCLASS,
            hwnd: self.pane.get(),
            uId: 1,
            rect,
            lpszText: text.as_mut_ptr(),
            // SAFETY: the rest of TTTOOLINFOW is plain data.
            ..unsafe { core::mem::zeroed() }
        }
    }

    /// The tree's font, row height and icons for the window's DPI.
    fn fit_dpi(&self, app: &App) {
        let dpi = app.dpi.get();
        // SAFETY: reads a metric.
        let icon = unsafe { GetSystemMetricsForDpi(SM_CXSMICON, dpi) };
        let resized = {
            let images = self.images.borrow();
            match images.as_ref() {
                Some(i) if i.size() == icon => None,
                Some(i) => Some(i.resized(icon)),
                None => {
                    let mut i = Images::new(icon);
                    // The top item's, then the plain folder's.
                    i.get(Source::App);
                    i.get(Source::System(self.plain_folder.get()));
                    Some(i)
                }
            }
        };
        let tree = self.tree.get();
        if let Some(new) = resized {
            treeview::set_images(tree, Some(&new));
            // The old list goes once the tree has the new one.
            *self.images.borrow_mut() = Some(new);
        }
        let fh = app.ui_fonts.borrow().height;
        treeview::set_font(tree, app.ui_font() as isize, fh, icon, |v| app.scale(v));
    }
}

/// A closed pane's tree still to empty, and its items still in it.
struct Parked {
    tree: HWND,
    items: Vec<HTREEITEM>,
}

thread_local! {
    static PARKED: RefCell<Vec<Parked>> = const { RefCell::new(Vec::new()) };
}

/// Takes `tree` (holding `items`) out of its pane, hidden, so the pane's
/// window goes at once without it: destroyed, a tree view takes its items
/// out one by one, in time that grows faster than their number (seconds
/// for tens of thousands). As the window ends (`ending`) it stays so until
/// the process ends, which frees it at once; otherwise it is emptied a
/// slice at a time from a timer of its own ([`empty_parked`]).
fn park(tree: HWND, items: Vec<HTREEITEM>, ending: bool) {
    // SAFETY: hides this thread's tree and makes it a message-only window
    // of this thread, no longer a child of the pane.
    unsafe {
        ShowWindow(tree, SW_HIDE);
        SetParent(tree, HWND_MESSAGE);
    }
    if ending {
        return;
    }
    PARKED.with_borrow_mut(|p| p.push(Parked { tree, items }));
    // SAFETY: a timer of this thread's tree, which calls `empty_parked`.
    unsafe { SetTimer(tree, PARK_TIMER, PARK_MS, Some(empty_parked)) };
}

/// A parked tree's timer: its items go, the last made first (so each is
/// one with nothing under it), for at most [`PARK_SLICE`]; once there are
/// none, the tree is destroyed.
unsafe extern "system" fn empty_parked(tree: HWND, _msg: u32, id: usize, _time: u32) {
    let started = Instant::now();
    loop {
        let next = PARKED.with_borrow_mut(|parked| {
            let i = parked.iter().position(|p| p.tree == tree)?;
            let item = parked[i].items.pop();
            if item.is_none() {
                parked.swap_remove(i);
            }
            Some(item)
        });
        // SAFETY: this thread's parked tree, its timer, and one of its
        // items (none goes but here, nothing is borrowed meanwhile).
        unsafe {
            match next {
                Some(Some(item)) => {
                    SendMessageW(tree, TVM_DELETEITEM, 0, item);
                }
                Some(None) | None => {
                    KillTimer(tree, id);
                    DestroyWindow(tree);
                    return;
                }
            }
        }
        if started.elapsed() >= PARK_SLICE {
            return;
        }
    }
}

/// The tree's subclass id.
const TREE_SUBCLASS: usize = 1;

/// The tree's subclass. A tree view taking the keyboard with nothing chosen
/// chooses its first item itself; with a folder the tree cannot show (in a
/// hidden folder) that would let the folder go, so it is taken as goz-gui's
/// own change and undone.
unsafe extern "system" fn tree_proc(
    hwnd: HWND,
    msg: u32,
    w: WPARAM,
    l: LPARAM,
    _id: usize,
    data: usize,
) -> LRESULT {
    // SAFETY: `data` is the pane, which outlives its tree's subclass (taken
    // off in `destroy`); the rest goes on as it came.
    unsafe {
        if msg != WM_SETFOCUS {
            return DefSubclassProc(hwnd, msg, w, l);
        }
        let pane = &*(data as *const FoldersPane);
        let none = treeview::selected(hwnd) == NONE;
        let was = pane.syncing.replace(true);
        let r = DefSubclassProc(hwnd, msg, w, l);
        if none && treeview::selected(hwnd) != NONE {
            treeview::select(hwnd, NONE);
        }
        pane.syncing.set(was);
        r
    }
}

/// Fills the empty `popup` with Explorer's menu for the folder at `path`
/// (goz-gui's items go above it after), without its default item (the
/// menu's own Expand or Open is) and without Rename (the tree renames
/// nothing): `None` when the shell has none for it (the folder is gone).
fn shell_part(
    app: &App,
    popup: HMENU,
    path: &[u16],
    extended: bool,
) -> Option<Rc<shell::ContextMenu>> {
    // The shell's Copy and Cut put their data object on the clipboard
    // through OLE; without it the rest of the menu still works.
    shell::ole_ready();
    let items = shell::Items::of_paths(&[path.to_vec()])?;
    let cm = items.context_menu(app.hwnd)?;
    if !cm.fill(
        popup,
        0,
        u32::from(menu::SHELL_FIRST),
        u32::from(menu::SHELL_LAST),
        shell::query_flags(extended, false),
    ) {
        return None;
    }
    // SAFETY: reads and deletes an item of a menu of this thread.
    unsafe {
        let default = GetMenuDefaultItem(popup, 0, 0);
        if cm.owns(default, u32::from(menu::SHELL_LAST)) {
            DeleteMenu(popup, default, MF_BYCOMMAND);
        }
    }
    Some(Rc::new(cm))
}

impl crate::pane::PaneContent for FoldersPane {
    fn create(&self, app: &App, pane: HWND) {
        self.pane.set(pane);
        let style = WS_CHILD
            | WS_VISIBLE
            | WS_TABSTOP
            | TVS_HASBUTTONS
            | TVS_LINESATROOT
            | TVS_SHOWSELALWAYS
            | TVS_TRACKSELECT
            | TVS_FULLROWSELECT
            | TVS_NOHSCROLL
            | TVS_NONEVENHEIGHT;
        // SAFETY: a child of this thread's pane window, and a tooltip popup
        // (owned by the main window, as a child cannot own one) that
        // `destroy` destroys.
        let (tree, tip) = unsafe {
            let hinst = GetModuleHandleW(core::ptr::null());
            let tree = CreateWindowExW(
                0,
                WC_TREEVIEWW,
                core::ptr::null(),
                style,
                0,
                0,
                0,
                0,
                pane,
                TREE_ID as HMENU,
                hinst,
                core::ptr::null(),
            );
            let tip = CreateWindowExW(
                WS_EX_TOPMOST,
                TOOLTIPS_CLASSW,
                core::ptr::null(),
                WS_POPUP | TTS_ALWAYSTIP | TTS_NOPREFIX,
                CW_USEDEFAULT,
                CW_USEDEFAULT,
                CW_USEDEFAULT,
                CW_USEDEFAULT,
                pane,
                core::ptr::null_mut(),
                hinst,
                core::ptr::null(),
            );
            (tree, tip)
        };
        self.tree.set(tree);
        self.tip.set(tip);
        // SAFETY: the pane outlives the subclass (`destroy` takes it off
        // before the tree goes).
        unsafe {
            SetWindowSubclass(
                tree,
                Some(tree_proc),
                TREE_SUBCLASS,
                self as *const FoldersPane as usize,
            );
        }
        let ex = TVS_EX_DOUBLEBUFFER | TVS_EX_AUTOHSCROLL;
        self.send(TVM_SETEXTENDEDSTYLE, ex as usize, ex as isize);
        self.plain_folder.set(treeview::plain_icons().0);
        self.fit_dpi(app);
        treeview::apply_theme(tree, &app.pal.get());
        if !tip.is_null() {
            let mut text = wz(subfolders_tip());
            let info = self.tool(RECT::default(), &mut text);
            // SAFETY: the tooltip copies the tool and its text.
            unsafe {
                SendMessageW(tip, TTM_ADDTOOLW, 0, (&raw const info) as isize);
                SendMessageW(tip, TTM_SETMAXTIPWIDTH, 0, app.scale(360) as isize);
            }
            theme_tip(tip, app.pal.get().dark);
        }
        // The top item: no folder, the whole index.
        let top = {
            let mut images = self.images.borrow_mut();
            images.as_mut().map_or(-1, |i| i.get(Source::App))
        };
        let goz: Vec<u16> = "goz".encode_utf16().collect();
        self.quietly(|| {
            self.add(None, None, Vec::new(), &goz, top, false);
            treeview::select(tree, self.nodes.borrow()[TOP as usize].item);
        });
        let labels = app.sidebar_settings().labels;
        foldertree::spawn(
            Job::Roots { labels },
            pane,
            WM_FOUND,
            Arc::clone(&self.alive),
        );
        // The window's side follows once the framework has the pane open.
        // SAFETY: posts to this pane's own window.
        unsafe { PostMessageW(pane, WM_OPENED, 0, 0) };
    }

    fn place(&self, app: &App, rc: RECT) {
        let bar = Self::bar(app, &rc);
        // SAFETY: moves this pane's tree; updates its tooltip's tool.
        unsafe {
            MoveWindow(
                self.tree.get(),
                rc.left,
                rc.top,
                rc.right - rc.left,
                (bar.top - rc.top).max(0),
                1,
            );
            let tip = self.tip.get();
            if !tip.is_null() {
                let mut text = wz(subfolders_tip());
                let info = self.tool(Self::subfolders_rect(app, &bar), &mut text);
                SendMessageW(tip, TTM_NEWTOOLRECTW, 0, (&raw const info) as isize);
            }
            InvalidateRect(self.pane.get(), &bar, 0);
        }
    }

    fn destroy(&self, app: &App) {
        let pane = self.pane.get();
        // SAFETY: this pane's own timers.
        unsafe {
            KillTimer(pane, TIMER_CHOOSE);
            KillTimer(pane, TIMER_RELIST);
        }
        // No thread posts a reply after this; those posted are freed.
        foldertree::close(&self.alive, pane, WM_FOUND);
        self.batches.borrow_mut().clear();
        // The tooltip is a popup, owned by the main window (a child cannot
        // own one): it goes now, not with the pane.
        let tip = self.tip.replace(core::ptr::null_mut());
        if !tip.is_null() {
            // SAFETY: destroys this pane's own tooltip window.
            unsafe { DestroyWindow(tip) };
        }
        let tree = self.tree.get();
        // SAFETY: takes off the subclass `create` put on this pane's tree.
        unsafe { RemoveWindowSubclass(tree, Some(tree_proc), TREE_SUBCLASS) };
        // The tree lets go of the image list before it is destroyed.
        treeview::set_images(tree, None);
        self.images.borrow_mut().take();
        // Its items in the order they were made: a folder's subfolders
        // after it, so the last first can be taken out one at a time.
        let items: Vec<HTREEITEM> = self
            .nodes
            .borrow()
            .iter()
            .filter(|n| !n.gone && n.item != NONE)
            .map(|n| n.item)
            .collect();
        park(tree, items, app.window_ending());
        app.folders_pane_shown(false);
    }

    fn focus(&self) -> Option<HWND> {
        Some(self.tree.get())
    }

    fn key(&self, app: &App, msg: &MSG) -> bool {
        if msg.hwnd != self.tree.get() {
            return false;
        }
        let vk = msg.wParam as u16;
        match msg.message {
            WM_KEYDOWN if vk == VK_RETURN => {
                app.focus_list();
                true
            }
            WM_KEYDOWN if vk == VK_F5 => {
                // The chosen folder is listed again; the shortcut refreshes
                // the results after.
                if let Some(n) = self.node_of(treeview::selected(self.tree.get()))
                    && n != TOP
                    && self.nodes.borrow()[n as usize].load == Load::Listed
                {
                    self.list(n);
                }
                false
            }
            // Alt+Up, Everything's global Up One Dir.
            WM_SYSKEYDOWN if vk == VK_UP => {
                app.up_one_folder();
                true
            }
            _ => false,
        }
    }

    fn message(&self, app: &App, _pane: HWND, msg: u32, w: WPARAM, l: LPARAM) -> Option<LRESULT> {
        match msg {
            WM_NOTIFY => self.notify(app, l),
            WM_CONTEXTMENU if w as HWND == self.tree.get() => {
                self.menu(app, l);
                Some(0)
            }
            // A click on the Subfolders box ticks it or not, with a gozd
            // that reads parent:.
            WM_LBUTTONDOWN | WM_LBUTTONDBLCLK if crate::client::boolean_syntax() => {
                let rc = crate::pane::content_rect(app, _pane);
                let hit = Self::subfolders_rect(app, &Self::bar(app, &rc));
                let pt = point(l);
                if (hit.left..hit.right).contains(&pt.x) && (hit.top..hit.bottom).contains(&pt.y) {
                    app.toggle_subfolders();
                    Some(0)
                } else {
                    None
                }
            }
            WM_FOUND => {
                // SAFETY: a thread's reply, posted with `foldertree::spawn`;
                // ours to free.
                let reply = unsafe { Box::from_raw(l as *mut Reply) };
                match *reply {
                    Reply::Roots(roots) => self.roots(app, roots),
                    Reply::Folders {
                        node,
                        request,
                        found,
                    } => self.folders(app, node, request, found),
                }
                Some(0)
            }
            WM_MORE => {
                self.more(app);
                Some(0)
            }
            WM_OPENED => {
                app.folders_pane_shown(true);
                self.follow(app);
                Some(0)
            }
            WM_TIMER => {
                // SAFETY: this pane's own timer.
                unsafe { KillTimer(self.pane.get(), w) };
                match w {
                    TIMER_CHOOSE => self.choose_selected(app),
                    TIMER_RELIST => {
                        let nodes: Vec<u32> = self.relist.borrow_mut().drain(..).collect();
                        for n in nodes {
                            let listed = self
                                .nodes
                                .borrow()
                                .get(n as usize)
                                .is_some_and(|node| !node.gone && node.load == Load::Listed);
                            if listed {
                                self.list(n);
                            }
                        }
                    }
                    _ => return None,
                }
                Some(0)
            }
            _ => None,
        }
    }

    fn paint(&self, app: &App, hdc: HDC, rc: &RECT) {
        self.paint_bar(app, hdc, &Self::bar(app, rc));
    }

    fn theme(&self, app: &App, dark: bool) {
        treeview::apply_theme(self.tree.get(), &app.pal.get());
        theme_tip(self.tip.get(), dark);
        // SAFETY: repaints this pane's tree.
        unsafe { InvalidateRect(self.tree.get(), core::ptr::null(), 1) };
    }

    fn dpi(&self, app: &App, _dpi: u32) {
        self.fit_dpi(app);
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// The tooltip dark or light with the window.
fn theme_tip(tip: HWND, dark: bool) {
    if tip.is_null() {
        return;
    }
    theme::allow(tip, dark);
    theme::set_class(
        tip,
        Some(if dark {
            "DarkMode_Explorer"
        } else {
            "Explorer"
        }),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn batches_keep_within_their_time() {
        let ms = Duration::from_millis;
        // Well under the budget: twice as many next time.
        assert_eq!(next_batch(300, ms(2)), 600);
        // Near it: as many.
        assert_eq!(next_batch(300, ms(20)), 300);
        // Ten times over: a tenth.
        assert_eq!(next_batch(300, ms(250)), 30);
        // Never fewer than the least, nor more than the most.
        assert_eq!(next_batch(30, Duration::from_secs(5)), BATCH_MIN);
        assert_eq!(next_batch(BATCH_MAX, ms(1)), BATCH_MAX);
    }
}
