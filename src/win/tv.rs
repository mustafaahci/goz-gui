//! Tree-view messages (SysTreeView32), each sent only to a tree view. The
//! item structures go in by value, their text pointers set here.
//!
//! A tree's item (HTREEITEM) is the address of the item's memory in its
//! tree, which comctl32 follows; it checks one only by a signature in that
//! memory, never against the tree it is sent to. So items are [`Item`]s,
//! which only this module makes (from an insert, a query of the tree, a
//! hit test, a notification of the tree), each naming its tree; every
//! function taking one acts on that tree, and only while the item is in
//! it.
//!
//! Which items a tree has is kept here, for a tree kept track of from its
//! birth: one [`super::class::create_plain`] made, or one the dialog
//! manager made from a [`super::dlg::Template`] (seen as it is made). No
//! other tree view is taken where its items are (what goes on in it, a
//! deletion under way, is not known).
//!
//! Every message of this module's that names an item, and every answer
//! the record is made of, goes to the tree's class's own procedure, called
//! directly ([`super::system_class_proc`]), never to the window, whose
//! subclasses may answer anything, or delete the item a message names
//! before passing it on. An item is noted as [`insert`] puts it in, by the
//! tree's own answer. A deletion takes what goes out of the record before
//! the tree sees it, so an item is refused from the moment its deletion
//! starts (TVM_DELETEITEM of it, of an item above it or of every item; a
//! collapse that resets an item above it, or the top; the tree's end): this
//! module's own deletions so, and those another's code sends through the
//! window by a subclass of the tree put in as it is made. While a deletion
//! is under way the tree takes no insert, deletion or reset of goz-gui's. A
//! stale item is so refused, unless the tree has since put a new item at
//! the same address: then it names that item of the same tree (wrong, not
//! unsound).
//!
//! A tree is busy while it is in its own processing, as far as goz-gui can
//! see: while a message passes the subclass that keeps track of it, and
//! while this module calls its class's procedure ([`Busy`]). It may then be
//! using any of its items (a notification it sends is about one, the hit
//! one, the one it draws or selects...), and comctl32 does not survive
//! every one of them being deleted under it. So a deletion goz-gui asks for
//! meanwhile ([`delete_item`], [`delete_all`]) waits: what it takes is out
//! of the record at once, as for any deletion, and the tree deletes it once
//! it is out of its processing; a collapse that resets is refused. An image
//! list the tree no longer shows is let go then too.
//!
//! A message another's code sends through the window that names an item
//! (TVM_GETITEMW, TVM_EXPAND, an insert's parent...) reaches the tree
//! through its subclasses, any of which may delete the item, with this
//! module's functions, before passing it on. So each pass of one by a
//! subclass of goz-gui's ([`Msg::def`]) checks every item it names against
//! the record first ([`pass_on`]): one not there (gone, going, or put in
//! by another's code through the window, which is not noted) stops the
//! message, which answers 0. The check is made at every pass, so the last
//! one before the tree is made whatever the order of the subclasses (one
//! put in at the tree's birth is nearer the tree than the one that keeps
//! track of it).

use core::mem::offset_of;
use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};

use windows_sys::Win32::Foundation::{COLORREF, HWND, LRESULT, POINT, RECT};
use windows_sys::Win32::UI::Controls::{
    HTREEITEM, NMTREEVIEWW, NMTVGETINFOTIPW, TVE_COLLAPSERESET, TVGETITEMPARTRECTINFO, TVGN_CARET,
    TVGN_CHILD, TVGN_DROPHILITE, TVGN_NEXT, TVGN_PARENT, TVGN_ROOT, TVHITTESTINFO, TVI_FIRST,
    TVI_LAST, TVI_ROOT, TVI_SORT, TVIF_EXPANDEDIMAGE, TVIF_INTEGRAL, TVIF_STATEEX, TVIF_TEXT,
    TVINSERTSTRUCTA, TVINSERTSTRUCTW, TVITEMA, TVITEMEXW, TVITEMW, TVM_CREATEDRAGIMAGE,
    TVM_DELETEITEM, TVM_EDITLABELA, TVM_EDITLABELW, TVM_ENSUREVISIBLE, TVM_EXPAND,
    TVM_GETIMAGELIST, TVM_GETITEMA, TVM_GETITEMPARTRECT, TVM_GETITEMRECT, TVM_GETITEMSTATE,
    TVM_GETITEMW, TVM_GETNEXTITEM, TVM_GETTOOLTIPS, TVM_HITTEST, TVM_INSERTITEMA, TVM_INSERTITEMW,
    TVM_MAPHTREEITEMTOACCID, TVM_SELECTITEM, TVM_SETBKCOLOR, TVM_SETEXTENDEDSTYLE, TVM_SETHOT,
    TVM_SETIMAGELIST, TVM_SETINSERTMARK, TVM_SETITEMA, TVM_SETITEMHEIGHT, TVM_SETITEMW,
    TVM_SETLINECOLOR, TVM_SETTEXTCOLOR, TVM_SHOWINFOTIP, TVM_SORTCHILDREN, TVM_SORTCHILDRENCB,
    TVSORTCB,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{CallWindowProcW, WM_DESTROY, WM_NCDESTROY};

use super::WndProc;
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

/// The tree view class's own procedure for `tree`, a tree view of this
/// thread whose class's procedure it is: called directly, it answers for
/// the tree itself, with no subclass of the window in between.
fn class_proc(tree: HWND) -> Option<WndProc> {
    super::system_class_proc(tree, CLASS)
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

/// What is kept of a tree of this thread, from its birth until it is
/// destroyed.
struct Tree {
    /// Which record this is, a number no other record of the thread has:
    /// a window's handle may name another window once it is gone, which
    /// may be a tree with a record of its own.
    serial: u64,
    /// Its items, as it has them; `None` from the moment it is destroyed
    /// (WM_DESTROY, while its items go), when none is taken any more. An
    /// item is noted only under one noted (or at the top), so an item not
    /// noted has none noted under it.
    items: Option<HashSet<HTREEITEM>>,
    /// How many deletions of its items are under way (its own messages
    /// nest): while any is, goz-gui inserts, deletes and resets none.
    deleting: u32,
    /// How many deletions of its items have begun: an insert that sees
    /// this change while it runs notes nothing, as the item it made may
    /// be gone already.
    deletions: u64,
    /// The deletions of an item that another's code sent through the
    /// window and the subclass that keeps track of the tree is passing on
    /// (the item out of the record by then): the item, and whether a pass
    /// of it to the next procedure has come back, after which it goes no
    /// further ([`pass_on`]).
    passing: Vec<(HTREEITEM, bool)>,
    /// The image lists it shows that goz-gui gave it ([`set_image_list`]):
    /// kept alive (a clone of each) while it does, as a tree never
    /// destroys its lists.
    lists: HashMap<u32, ImageList>,
    /// How deep it is in its own processing, as far as goz-gui sees
    /// ([`Busy`]).
    busy: u32,
    /// Items goz-gui deleted while the tree was busy, each out of the
    /// record with what is under it ([`postpone`]): the tree deletes them
    /// once it is not. Each is in the tree: one a deletion took meanwhile
    /// is taken off ([`recheck_waiting`]).
    waiting: Vec<HTREEITEM>,
    /// Image lists it no longer shows, let go once it is not busy: it may
    /// still be using one it just gave up.
    retired: Vec<ImageList>,
}

thread_local! {
    /// The trees of this thread kept track of, by window.
    static TREES: RefCell<HashMap<usize, Tree>> = RefCell::new(HashMap::new());
    /// The serial of the last record made ([`Tree::serial`]).
    static SERIALS: Cell<u64> = const { Cell::new(0) };
}

static TRACK: super::subclass::Key = super::subclass::Key::new("tree view items and lists");

/// Keeps track of `tree` from now on, if it is a tree view of this thread
/// whose class's procedure is the system's: `born`, as it is being made
/// (no message has reached it and it has no items), or else just made (its
/// items, none but what another's code put in, are read). False if it is
/// not such a tree, or could not be subclassed.
///
/// Only for a tree that new: what goes on in an older one (a deletion
/// under way, whose end the subclass put in now would not see) is not
/// known here.
fn adopt(tree: HWND, born: bool) -> bool {
    if !super::class_is(tree, CLASS) || class_proc(tree).is_none() {
        return false;
    }
    if tracked(tree) {
        return true;
    }
    // A record under this handle is of a window that went without its end
    // being seen (a subclass in front swallowed it): not this one's.
    let stale = TREES.with_borrow_mut(|t| t.remove(&(tree as usize)));
    drop(stale);
    if !super::subclass::set(tree, &TRACK, track_proc) {
        return false;
    }
    let serial = SERIALS.get() + 1;
    SERIALS.set(serial);
    TREES.with_borrow_mut(|t| {
        t.insert(
            tree as usize,
            Tree {
                serial,
                items: Some(HashSet::new()),
                deleting: 0,
                deletions: 0,
                passing: Vec::new(),
                lists: HashMap::new(),
                busy: 0,
                waiting: Vec::new(),
                retired: Vec::new(),
            },
        )
    });
    if !born && let Some(busy) = Busy::enter(tree) {
        let items = busy.walk();
        note(tree, |noted| *noted = items);
    }
    true
}

/// Keeps track of `hwnd` from now on if it is a tree view (nothing for any
/// other window): [`super::class::create_plain`]'s, for what it makes.
///
/// # Safety
/// `hwnd` is null or a window the caller made just now (its
/// CreateWindowExW returned it), so no deletion of its items is under way.
pub(crate) unsafe fn adopt_made(hwnd: HWND) {
    adopt(hwnd, false);
}

/// While one lives, each tree view this thread makes is kept track of
/// from its birth (HCBT_CREATEWND, before any message reaches it): for the
/// trees the dialog manager makes from a template, which goz-gui does not
/// make itself. Through a CBT hook of the thread's, kept while any lives;
/// a tree whose birth a hook set before answered is not seen, and so not
/// taken by this module.
pub(crate) struct Births {
    _not_send: core::marker::PhantomData<*mut ()>,
}

thread_local! {
    /// How many [`Births`] live on this thread, and the hook they keep.
    static BIRTHS: RefCell<(u32, Option<super::hook::Hook>)> = const { RefCell::new((0, None)) };
}

impl Births {
    /// Watches the births of this thread's trees until this drops.
    pub(crate) fn watch() -> Births {
        let first = BIRTHS.with_borrow_mut(|b| {
            b.0 += 1;
            b.0 == 1
        });
        if first {
            let hook = super::hook::set(super::hook::Kind::Cbt, |event| {
                if let super::hook::Event::Create { hwnd, .. } = *event {
                    // Being made: no message has reached it yet.
                    adopt(hwnd, true);
                }
                None
            });
            BIRTHS.with_borrow_mut(|b| b.1 = hook);
        }
        Births {
            _not_send: core::marker::PhantomData,
        }
    }
}

impl Drop for Births {
    fn drop(&mut self) {
        // The hook, once no `Births` is left, dropped out of the cell.
        let hook = BIRTHS
            .try_with(|b| {
                let mut b = b.borrow_mut();
                b.0 = b.0.saturating_sub(1);
                if b.0 == 0 { b.1.take() } else { None }
            })
            .ok()
            .flatten();
        drop(hook);
    }
}

/// Whether `tree` is kept track of: it has a record, and the subclass that
/// keeps it (a window gone without its end seen leaves a record its handle
/// may name again, for a window without the subclass).
fn tracked(tree: HWND) -> bool {
    TREES.with_borrow(|t| t.contains_key(&(tree as usize))) && super::subclass::has(tree, &TRACK)
}

/// The count of the deletions `tree` has begun, if it is kept track of and
/// takes changes now: it is not being destroyed, and no deletion of its
/// items is under way.
fn calm(tree: HWND) -> Option<u64> {
    if !tracked(tree) {
        return None;
    }
    TREES.with_borrow(|t| {
        t.get(&(tree as usize))
            .filter(|t| t.items.is_some() && t.deleting == 0)
            .map(|t| t.deletions)
    })
}

/// Whether `tree` is busy: in its own processing ([`Busy`]).
fn is_busy(tree: HWND) -> bool {
    TREES.with_borrow(|t| t.get(&(tree as usize)).is_some_and(|t| t.busy > 0))
}

/// A tree view and its class's own procedure, for calls to it: the tree is
/// busy while one lives (counted in its record, if it has one), and as the
/// last one drops ([`settle`]) the deletions that waited are made and the
/// image lists it gave up let go. Every call this module makes to a tree's
/// class's procedure is made through one.
struct Busy {
    tree: HWND,
    proc: WndProc,
    /// The serial of the record it counts in ([`Tree::serial`]): the tree's
    /// as this began, if it had one.
    serial: Option<u64>,
}

impl Busy {
    /// `tree` busy until this drops, if it is a tree view of this thread
    /// whose class's procedure is the system's.
    fn enter(tree: HWND) -> Option<Busy> {
        let proc = class_proc(tree)?;
        let serial = TREES.with_borrow_mut(|t| {
            t.get_mut(&(tree as usize)).map(|t| {
                t.busy += 1;
                t.serial
            })
        });
        Some(Busy { tree, proc, serial })
    }

    /// Calls the tree's class's own procedure with message `msg`: the tree
    /// itself answers, with no subclass of it in between, so nothing runs
    /// between the caller's check of what it passes and the tree's reading
    /// it.
    ///
    /// # Safety
    /// `w` and `l` are what `msg` defines for a tree view: an item among
    /// them is one the tree has now.
    unsafe fn call(&self, msg: u32, w: usize, l: isize) -> isize {
        // SAFETY: `proc` is the tree's class's own procedure, for this tree
        // (`enter`); the parameters as the caller promises.
        unsafe { CallWindowProcW(Some(self.proc), self.tree, msg, w, l) }
    }

    /// TVM_GETNEXTITEM of `h`, an item the tree has (or 0, for the codes
    /// that take none), as `code`; 0 for none.
    fn next(&self, h: HTREEITEM, code: u32) -> HTREEITEM {
        // SAFETY: TVM_GETNEXTITEM takes a code and an item the tree has
        // (noted as in it, or its own answer just now) or none. The answer
        // is an item of it, or 0.
        unsafe { self.call(TVM_GETNEXTITEM, code as usize, h) }
    }

    /// Every item of the tree now, walked from its first top-level item.
    fn walk(&self) -> HashSet<HTREEITEM> {
        let mut all = HashSet::new();
        let mut todo = vec![self.next(0, TVGN_ROOT)];
        while let Some(first) = todo.pop() {
            let mut at = first;
            while at != 0 && all.insert(at) {
                todo.push(self.next(at, TVGN_CHILD));
                at = self.next(at, TVGN_NEXT);
            }
        }
        all
    }

    /// Every item under `h` (not `h` itself), an item the tree has.
    fn under(&self, h: HTREEITEM) -> Vec<HTREEITEM> {
        let mut out = Vec::new();
        let mut todo = vec![self.next(h, TVGN_CHILD)];
        while let Some(first) = todo.pop() {
            let mut at = first;
            while at != 0 {
                out.push(at);
                todo.push(self.next(at, TVGN_CHILD));
                at = self.next(at, TVGN_NEXT);
            }
        }
        out
    }

    /// The tree's top-level items now, first to last.
    fn tops(&self) -> Vec<HTREEITEM> {
        let mut out = Vec::new();
        let mut at = self.next(0, TVGN_ROOT);
        while at != 0 && !out.contains(&at) {
            out.push(at);
            at = self.next(at, TVGN_NEXT);
        }
        out
    }

    /// Whether the window is still the tree this began on: kept track of,
    /// under the same record, its class's procedure the same. A call to it
    /// may have let it be destroyed (by another's code), and its handle
    /// name another window since.
    fn same(&self) -> bool {
        self.serial.is_some()
            && tracked(self.tree)
            && TREES.with_borrow(|t| t.get(&(self.tree as usize)).map(|t| t.serial)) == self.serial
            && class_proc(self.tree).is_some_and(|p| p as usize == self.proc as usize)
    }
}

impl Drop for Busy {
    fn drop(&mut self) {
        let Some(serial) = self.serial else {
            return;
        };
        let key = self.tree as usize;
        let settles = TREES
            .try_with(|t| match t.borrow_mut().get_mut(&key) {
                Some(t) if t.serial == serial => {
                    t.busy = t.busy.saturating_sub(1);
                    t.busy == 0 && !(t.waiting.is_empty() && t.retired.is_empty())
                }
                _ => false,
            })
            .unwrap_or(false);
        if settles {
            settle(self.tree, serial);
        }
    }
}

/// `tree` (whose record is `serial`'s) is out of its own processing: the
/// deletions that waited are made, first asked first, and the image lists
/// it gave up meanwhile let go. Busy again meanwhile, so what is asked for
/// during these deletions waits in turn (or is refused, as any deletion is
/// while another is under way).
fn settle(tree: HWND, serial: u64) {
    let key = tree as usize;
    let Some(busy) = Busy::enter(tree) else {
        return;
    };
    if busy.serial != Some(serial) {
        return;
    }
    loop {
        let next = TREES.with_borrow_mut(|t| match t.get_mut(&key) {
            Some(t) if t.serial == serial && !t.waiting.is_empty() => Some(t.waiting.remove(0)),
            _ => None,
        });
        let Some(h) = next else {
            break;
        };
        deletion(&busy, Deletion::Item(h), || {
            // SAFETY: TVM_DELETEITEM of an item the tree has: one that
            // waits is in it (`recheck_waiting`, after every deletion), and
            // `deletion` only walked the tree since this took it off.
            unsafe { busy.call(TVM_DELETEITEM, 0, h) }
        });
        if !busy.same() {
            return;
        }
    }
    let retired = TREES.with_borrow_mut(|t| match t.get_mut(&key) {
        Some(t) if t.serial == serial => core::mem::take(&mut t.retired),
        _ => Vec::new(),
    });
    // Out of the record first, then let go: the tree shows none of them.
    drop(retired);
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
    h != 0 && tracked(tree) && noted(tree, h)
}

/// Whether `h` is in `tree`'s record (whether the tree is kept track of is
/// the caller's to know): a lookup in its set.
fn noted(tree: HWND, h: HTREEITEM) -> bool {
    TREES.with_borrow(|t| {
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

/// What a deletion of a tree's items takes.
#[derive(Clone, Copy)]
enum Deletion {
    /// Every item (TVM_DELETEITEM of TVI_ROOT).
    All,
    /// Every item, by a collapse that resets the top (TVM_EXPAND of
    /// TVI_ROOT with TVE_COLLAPSERESET): what the tree still has after,
    /// whatever it answered, is noted again.
    Top,
    /// An item noted, and every item under it (TVM_DELETEITEM of it).
    Item(HTREEITEM),
    /// Every item under an item noted, which stays (a collapse that resets
    /// it, TVE_COLLAPSERESET).
    Under(HTREEITEM),
}

/// Runs `pass`, which deletes of the tree's items what `what` names (the
/// message passed on by the tree's subclass, or sent by this module to its
/// class's own procedure, through `busy`): those of them noted are out of
/// the record before the tree sees the deletion, and the tree is marked as
/// deleting until it is done. Those the tree still has after (it did not
/// delete them, whatever it answered) are noted again. `pass`'s answer.
fn deletion(busy: &Busy, what: Deletion, pass: impl FnOnce() -> LRESULT) -> LRESULT {
    let Some(serial) = busy.serial else {
        return pass();
    };
    let tree = busy.tree;
    let key = tree as usize;
    // What goes, walked before anything of it goes.
    let these = match what {
        Deletion::All | Deletion::Top => None,
        Deletion::Item(h) => {
            let mut gone = busy.under(h);
            gone.push(h);
            Some(gone)
        }
        Deletion::Under(h) => Some(busy.under(h)),
    };
    let taken: Vec<HTREEITEM> = TREES.with_borrow_mut(|t| {
        let Some(t) = t.get_mut(&key).filter(|t| t.serial == serial) else {
            return Vec::new();
        };
        t.deleting += 1;
        t.deletions += 1;
        let Some(items) = t.items.as_mut() else {
            return Vec::new();
        };
        match these {
            None => core::mem::take(items).into_iter().collect(),
            Some(these) => these.into_iter().filter(|h| items.remove(h)).collect(),
        }
    });
    let answer = pass();
    // The tree may be gone meanwhile (by another's code), its handle another
    // window's since: only the same tree's record is changed, and only that
    // tree is walked, by its class's own procedure, the same as before.
    let same = TREES.with_borrow_mut(|t| match t.get_mut(&key) {
        Some(t) if t.serial == serial => {
            t.deleting = t.deleting.saturating_sub(1);
            true
        }
        _ => false,
    });
    if !same || !busy.same() {
        return answer;
    }
    recheck_waiting(busy);
    if taken.is_empty() {
        return answer;
    }
    // Only items the tree's own procedure finds in it now are noted again.
    let now: HashSet<HTREEITEM> = match what {
        // A deletion that answers it did, did (or left items that are then
        // refused: wrong, not unsound). One that answers it did not is
        // checked: a subclass may answer so having passed it on.
        Deletion::All | Deletion::Item(_) if answer != 0 => return answer,
        Deletion::All | Deletion::Item(_) | Deletion::Top => busy.walk(),
        // The item stays (unless a deletion of it ran meanwhile, which took
        // it out of the record): what is still under it was not reset (it
        // was not a collapse, or the tree did not reset it).
        Deletion::Under(h) if has(tree, h) => busy.under(h).into_iter().collect(),
        Deletion::Under(_) => return answer,
    };
    note(tree, |items| {
        items.extend(taken.into_iter().filter(|h| now.contains(h)));
    });
    answer
}

/// Has `roots`, items of `busy`'s tree (busy before `busy` began), deleted
/// once the tree is out of its processing: each is out of the record now,
/// with every item under it, so refused from now on, as an item is from
/// the moment its deletion starts.
fn postpone(busy: &Busy, roots: Vec<HTREEITEM>) {
    let mut gone = Vec::new();
    for &h in &roots {
        gone.push(h);
        gone.extend(busy.under(h));
    }
    TREES.with_borrow_mut(|t| {
        let Some(t) = t
            .get_mut(&(busy.tree as usize))
            .filter(|t| Some(t.serial) == busy.serial)
        else {
            return;
        };
        if let Some(items) = t.items.as_mut() {
            for h in &gone {
                items.remove(h);
            }
        }
        for h in roots {
            if !t.waiting.contains(&h) {
                t.waiting.push(h);
            }
        }
    });
}

/// Takes off the deletions waiting in `busy`'s tree ([`postpone`]) whose
/// item it no longer has: after any deletion, which may have taken one
/// (with an item above it), so that each that waits is in the tree.
fn recheck_waiting(busy: &Busy) {
    let key = busy.tree as usize;
    let waits = TREES.with_borrow(|t| t.get(&key).is_some_and(|t| !t.waiting.is_empty()));
    if !waits {
        return;
    }
    let present = busy.walk();
    TREES.with_borrow_mut(|t| {
        if let Some(t) = t.get_mut(&key).filter(|t| Some(t.serial) == busy.serial) {
            t.waiting.retain(|h| present.contains(h));
        }
    });
}

/// The subclass that keeps track of a tree: each message passed on as it
/// came (checked, as every subclass's pass is: [`pass_on`]), the tree busy
/// meanwhile ([`Busy`]); what another's code deletes through the window
/// (goz-gui's own deletions go to the class's own procedure, see
/// [`deletion`]) taken out of the items noted first.
fn track_proc(m: &Msg) -> LRESULT {
    let tree = m.hwnd;
    let busy = Busy::enter(tree);
    match m.id {
        TVM_DELETEITEM => {
            let Some(busy) = &busy else {
                return 0;
            };
            let what = if m.l == 0 || m.l == TVI_ROOT {
                Deletion::All
            } else if has(tree, m.l) {
                Deletion::Item(m.l)
            } else {
                // An item not noted is refused: it may be one deleted
                // already (a deletion passed on twice by a subclass in
                // front), which the tree would follow. A tree kept track of
                // has no other, but another's code's inserts and the items
                // whose deletion waits.
                return 0;
            };
            match what {
                // Out of the record now, the item goes on through the
                // subclasses nearer the tree for one pass ([`through`]).
                Deletion::Item(h) => deletion(busy, what, || through(busy, h, || m.def())),
                _ => deletion(busy, what, || m.def()),
            }
        }
        TVM_EXPAND if m.w & TVE_COLLAPSERESET as usize != 0 => match &busy {
            // Every item goes, as for TVM_DELETEITEM of TVI_ROOT.
            Some(busy) if m.l == TVI_ROOT => deletion(busy, Deletion::Top, || m.def()),
            Some(busy) if has(tree, m.l) => deletion(busy, Deletion::Under(m.l), || m.def()),
            // An item not noted has none noted under it (and is refused at
            // the pass, [`pass_on`]).
            Some(_) => m.def(),
            None => 0,
        },
        WM_DESTROY => {
            // The tree's items go now: none is taken from here on, and none
            // waits to be deleted.
            TREES.with_borrow_mut(|t| {
                if let Some(t) = t.get_mut(&(tree as usize)) {
                    t.items = None;
                    t.deletions += 1;
                    t.waiting.clear();
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

/// Runs `pass`, which passes on a TVM_DELETEITEM of `h` that another's code
/// sent `busy`'s tree through its window, `h` out of the record already
/// (see [`deletion`]): meanwhile the deletion goes on through the subclasses
/// nearer the tree until one of them has passed it on and had the answer
/// back; a second pass of it goes no further ([`pass_on`]), as the tree
/// would follow the item it freed.
fn through(busy: &Busy, h: HTREEITEM, pass: impl FnOnce() -> LRESULT) -> LRESULT {
    passing_of(busy, |p| p.push((h, false)));
    let answer = pass();
    passing_of(busy, |p| {
        if let Some(at) = p.iter().rposition(|&(q, _)| q == h) {
            p.remove(at);
        }
    });
    answer
}

/// Changes [`Tree::passing`] of `busy`'s tree (nothing if its record is
/// not the one `busy` began on).
fn passing_of(busy: &Busy, f: impl FnOnce(&mut Vec<(HTREEITEM, bool)>)) {
    TREES.with_borrow_mut(|t| {
        if let Some(t) = t
            .get_mut(&(busy.tree as usize))
            .filter(|t| Some(t.serial) == busy.serial)
        {
            f(&mut t.passing);
        }
    });
}

/// Whether tree message `id` names an item of the tree it is sent to (in
/// its wParam, its lParam, or the structure its lParam points at), as the
/// documentation and windows-sys give the messages: what [`pass_on`]
/// checks.
fn names_an_item(id: u32) -> bool {
    matches!(
        id,
        TVM_INSERTITEMA
            | TVM_INSERTITEMW
            | TVM_DELETEITEM
            | TVM_EXPAND
            | TVM_GETITEMRECT
            | TVM_GETNEXTITEM
            | TVM_SELECTITEM
            | TVM_GETITEMA
            | TVM_GETITEMW
            | TVM_SETITEMA
            | TVM_SETITEMW
            | TVM_EDITLABELA
            | TVM_EDITLABELW
            | TVM_CREATEDRAGIMAGE
            | TVM_SORTCHILDREN
            | TVM_ENSUREVISIBLE
            | TVM_SORTCHILDRENCB
            | TVM_SETINSERTMARK
            | TVM_GETITEMSTATE
            | TVM_MAPHTREEITEMTOACCID
            | TVM_SETHOT
            | TVM_SHOWINFOTIP
            | TVM_GETITEMPARTRECT
    )
}

/// A handle a message names where an item goes, and whether 0 there is no
/// item by the message's documentation (every item, the top, no selection)
/// rather than a missing one.
#[derive(Clone, Copy)]
struct Named {
    h: HTREEITEM,
    none: bool,
}

// The ANSI messages' structures, and TVITEMEXW, have their handles where
// the structures [`named`] reads have them.
const _: () = assert!(
    offset_of!(TVITEMA, hItem) == offset_of!(TVITEMW, hItem)
        && offset_of!(TVITEMEXW, hItem) == offset_of!(TVITEMW, hItem)
        && offset_of!(TVINSERTSTRUCTA, hParent) == offset_of!(TVINSERTSTRUCTW, hParent)
        && offset_of!(TVINSERTSTRUCTA, hInsertAfter) == offset_of!(TVINSERTSTRUCTW, hInsertAfter)
);

/// The handles tree message `id` (one [`names_an_item`] lists) names where
/// an item goes, with parameters `w` and `l`: at most two (an insert's
/// parent and place). A structure's field is read as a copy; a message
/// whose structure is missing (`l` null) names a missing item.
///
/// # Safety
/// `w` and `l` are what `id` defines for a tree view, as the system
/// delivered them: for a message whose lParam is a structure, `l` is null
/// or points at one, valid for reads while the message is delivered.
unsafe fn named(id: u32, w: usize, l: isize) -> [Option<Named>; 2] {
    // The handle at byte `at` of the structure `l` points at.
    let field = |at: usize, none: bool| {
        if l == 0 {
            return Some(Named { h: 0, none: false });
        }
        // SAFETY: `l` points at the structure `id` defines, valid for reads
        // (the caller's word), with a handle at `at` (each caller below
        // gives that structure's offset); copied unaligned, as a RECT that
        // holds one may be aligned for its 32-bit fields only.
        let h = unsafe {
            (l as *const u8)
                .add(at)
                .cast::<HTREEITEM>()
                .read_unaligned()
        };
        Some(Named { h, none })
    };
    let lparam = |none: bool| Some(Named { h: l, none });
    match id {
        // A parent of NULL is the top, as TVI_ROOT is; a place of NULL is
        // none the documentation gives.
        TVM_INSERTITEMA | TVM_INSERTITEMW => [
            field(offset_of!(TVINSERTSTRUCTW, hParent), true),
            field(offset_of!(TVINSERTSTRUCTW, hInsertAfter), false),
        ],
        // NULL: every item (TVM_DELETEITEM); for the codes that take no
        // item (TVM_GETNEXTITEM); no selection, insertion mark or hot item.
        TVM_DELETEITEM | TVM_GETNEXTITEM | TVM_SELECTITEM | TVM_SETINSERTMARK | TVM_SETHOT => {
            [lparam(true), None]
        }
        TVM_EXPAND | TVM_ENSUREVISIBLE | TVM_EDITLABELA | TVM_EDITLABELW | TVM_CREATEDRAGIMAGE
        | TVM_SORTCHILDREN | TVM_SHOWINFOTIP => [lparam(false), None],
        // The item in the rectangle's first bytes.
        TVM_GETITEMRECT => [field(0, false), None],
        TVM_GETITEMA | TVM_GETITEMW | TVM_SETITEMA | TVM_SETITEMW => {
            [field(offset_of!(TVITEMW, hItem), false), None]
        }
        TVM_SORTCHILDRENCB => [field(offset_of!(TVSORTCB, hParent), false), None],
        TVM_GETITEMPARTRECT => [field(offset_of!(TVGETITEMPARTRECTINFO, hti), false), None],
        TVM_GETITEMSTATE | TVM_MAPHTREEITEMTOACCID => [
            Some(Named {
                h: w as HTREEITEM,
                none: false,
            }),
            None,
        ],
        _ => [None, None],
    }
}

/// Whether `n`, named by a message passed on to `tree` (kept track of), may
/// reach the tree: an item in its record; a special handle (TVI_ROOT,
/// TVI_FIRST, TVI_LAST, TVI_SORT), no item's memory; 0 where the message
/// takes it for no item.
fn may_pass(tree: HWND, n: Named) -> bool {
    match n.h {
        0 => n.none,
        TVI_ROOT | TVI_FIRST | TVI_LAST | TVI_SORT => true,
        h => noted(tree, h),
    }
}

/// How a subclass passes a message on to the next procedure of its window
/// ([`Msg::def`]): `next`, unless the window is a tree kept track of and
/// the message names an item (in its parameters, or the structure its
/// lParam points at) that is not in the tree's record now. A special
/// handle (TVI_ROOT, TVI_FIRST, TVI_LAST, TVI_SORT) is no item, nor is 0
/// where the message means no item by it; 0 anywhere else, or a structure
/// missing, names a missing one. Such a message goes no further and
/// answers 0 (the failure of each), as does one to a window no longer
/// there. `tv`'s own messages go to the tree's class's procedure and never
/// come here; another's code's come through the window, and a subclass of
/// it may delete the item one names (out of the record from then) before
/// passing it on, which the tree would follow.
///
/// The check is at every pass, so the last one before the tree is made
/// whatever the order of the subclasses (one put in at the tree's birth,
/// nearer the tree than the one that keeps track of it, makes its own). A
/// TVM_DELETEITEM of an item out of the record goes on only while that
/// subclass passes it on, until one pass of it has come back ([`through`]).
///
/// # Safety
/// `w` and `l` are the parameters the system delivered with `id` to a
/// subclass procedure of `tree` that is running.
pub(crate) unsafe fn pass_on(
    tree: HWND,
    id: u32,
    w: usize,
    l: isize,
    next: impl FnOnce() -> LRESULT,
) -> LRESULT {
    if !names_an_item(id) {
        return next();
    }
    if !tracked(tree) {
        // A window gone has freed its items (a tree's record went with
        // them); a tree not kept track of has none `tv` takes, so none
        // goz-gui's code can delete.
        return if super::is_window(tree) { next() } else { 0 };
    }
    // SAFETY: `tree`, kept track of, is a tree view, and `w` and `l` are
    // what the system delivered with `id` to it (the caller's word), so
    // what `id` defines for a tree view.
    let named = unsafe { named(id, w, l) };
    if id == TVM_DELETEITEM
        && let Some(n) = named[0]
        && !may_pass(tree, n)
    {
        return pass_deletion(tree, n.h, next);
    }
    if named.iter().flatten().all(|&n| may_pass(tree, n)) {
        next()
    } else {
        0
    }
}

/// Passes on (`next`) a TVM_DELETEITEM of `h`, an item out of `tree`'s
/// record, if its deletion through the window is under way and no pass of
/// it has come back yet; 0 if not.
fn pass_deletion(tree: HWND, h: HTREEITEM, next: impl FnOnce() -> LRESULT) -> LRESULT {
    let key = tree as usize;
    let on = TREES.with_borrow(|t| {
        t.get(&key)
            .is_some_and(|t| t.passing.iter().any(|&(p, back)| p == h && !back))
    });
    if !on {
        return 0;
    }
    let answer = next();
    TREES.with_borrow_mut(|t| {
        if let Some(t) = t.get_mut(&key) {
            for (p, back) in &mut t.passing {
                if *p == h {
                    *back = true;
                }
            }
        }
    });
    answer
}

/// Sends `msg` with item `item` as its lParam (and an integer as wParam)
/// to the item's tree's class's own procedure, if the item is in it;
/// `None` if it is not.
fn send_item(item: Item, msg: u32, w: usize) -> Option<isize> {
    if !live(item) {
        return None;
    }
    let busy = Busy::enter(item.tree)?;
    // SAFETY: the callers' messages take an item of the tree as lParam and
    // an integer as wParam; the item is in the tree (`live`), and is so
    // when the tree reads it (its own procedure, called directly).
    Some(unsafe { busy.call(msg, w, item.h) })
}

/// TVM_GETNEXTITEM as `code` of `h` (0 for the codes that take no item)
/// in `tree`, a tree kept track of, as an [`Item`] if it is one noted.
fn next_of(tree: HWND, h: HTREEITEM, code: u32) -> Option<Item> {
    if !tracked(tree) || (h != 0 && !has(tree, h)) {
        return None;
    }
    let busy = Busy::enter(tree)?;
    item_of(tree, busy.next(h, code))
}

/// The tree's first top-level item.
pub fn root(tree: HWND) -> Option<Item> {
    next_of(tree, 0, TVGN_ROOT)
}

/// The tree's selected item.
pub fn selection(tree: HWND) -> Option<Item> {
    next_of(tree, 0, TVGN_CARET)
}

/// The item's first child.
pub fn child(item: Item) -> Option<Item> {
    next_of(item.tree, item.h, TVGN_CHILD)
}

/// The item's next sibling.
pub fn next_sibling(item: Item) -> Option<Item> {
    next_of(item.tree, item.h, TVGN_NEXT)
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
        None => match Busy::enter(tree) {
            // SAFETY: TVM_SELECTITEM of no item: a code and null; to the
            // tree's class's own procedure.
            Some(busy) if tracked(tree) => unsafe {
                busy.call(TVM_SELECTITEM, code as usize, 0) != 0
            },
            _ => false,
        },
    }
}

/// TVM_EXPAND of `item` with TVE_* `code`. False if it did not; and for a
/// collapse that resets the item (TVE_COLLAPSERESET, which deletes what is
/// under it) while another deletion of the tree's items is under way, or
/// while the tree is in its own processing (see the module's
/// documentation), as from one of its notifications.
pub fn expand(item: Item, code: u32) -> bool {
    if code & TVE_COLLAPSERESET == 0 {
        return send_item(item, TVM_EXPAND, code as usize).is_some_and(|r| r != 0);
    }
    let tree = item.tree;
    if calm(tree).is_none() || !live(item) || is_busy(tree) {
        return false;
    }
    let Some(busy) = Busy::enter(tree) else {
        return false;
    };
    deletion(&busy, Deletion::Under(item.h), || {
        // SAFETY: TVM_EXPAND takes a code and an item of the tree: this
        // one, still in it (a reset keeps it; `deletion` only walks the
        // tree before this, which sends nothing); to its class's own
        // procedure.
        unsafe { busy.call(TVM_EXPAND, code as usize, item.h) }
    }) != 0
}

/// Expands and scrolls the tree so `item` shows.
pub fn ensure_visible(item: Item) {
    send_item(item, TVM_ENSUREVISIBLE, 0);
}

/// Deletes `item` and its children; false if it did not, and while
/// another deletion of the tree's items is under way. While the tree is
/// in its own processing (as from one of its notifications, see the
/// module's documentation), the item and its children are refused from
/// now on and the tree deletes them once it is out of it: true then.
pub fn delete_item(item: Item) -> bool {
    let tree = item.tree;
    if calm(tree).is_none() || !live(item) {
        return false;
    }
    let waits = is_busy(tree);
    let Some(busy) = Busy::enter(tree) else {
        return false;
    };
    if waits {
        postpone(&busy, vec![item.h]);
        return true;
    }
    deletion(&busy, Deletion::Item(item.h), || {
        // SAFETY: TVM_DELETEITEM of an item of the tree, still in it (it
        // is out of the record, but `deletion` only walked the tree since
        // `live`, which sends nothing); to its class's own procedure.
        unsafe { busy.call(TVM_DELETEITEM, 0, item.h) }
    }) != 0
}

/// Deletes every item of the tree; false if it did not, and while another
/// deletion of its items is under way. While the tree is in its own
/// processing, as [`delete_item`] of each of its top-level items (those
/// put in after are left).
pub fn delete_all(tree: HWND) -> bool {
    if calm(tree).is_none() {
        return false;
    }
    let waits = is_busy(tree);
    let Some(busy) = Busy::enter(tree) else {
        return false;
    };
    if waits {
        let tops = busy.tops();
        postpone(&busy, tops);
        return true;
    }
    deletion(&busy, Deletion::All, || {
        // SAFETY: TVM_DELETEITEM of TVI_ROOT: every item; to the tree's
        // class's own procedure.
        unsafe { busy.call(TVM_DELETEITEM, 0, TVI_ROOT) }
    }) != 0
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

/// Gives the tree `tv` image list `which` (TVSIL_*): `list` (`None`:
/// none). A tree never destroys its lists, so this keeps `list` alive (a
/// clone of it) for as long as the tree shows it: until it shows another,
/// or is destroyed. Sent to the tree's class's own procedure, so no
/// subclass keeps the tree from taking it. False if `tv` is not a tree
/// kept track of (see the module's documentation).
pub fn set_image_list(tv: HWND, which: u32, list: Option<&ImageList>) -> bool {
    let Some(busy) = given_list(tv, which, list.map_or(0, imagelist::raw_of)) else {
        return false;
    };
    keep_shown(&busy, which, list.cloned());
    true
}

/// Gives the tree `tv` the shell's system image list `list` as its image
/// list `which` (TVSIL_*): the list lives as long as the process. False if
/// `tv` is not a tree kept track of.
pub fn set_system_image_list(tv: HWND, which: u32, list: SystemList) -> bool {
    let Some(busy) = given_list(tv, which, imagelist::raw_of(&list)) else {
        return false;
    };
    keep_shown(&busy, which, None);
    true
}

/// TVM_SETIMAGELIST of `il` as list `which` to `tv`'s class's own
/// procedure, if `tv` is a tree kept track of: the tree takes `il`,
/// whatever a subclass of it would answer. The tree, still busy.
fn given_list(tv: HWND, which: u32, il: isize) -> Option<Busy> {
    if !tracked(tv) {
        return None;
    }
    let busy = Busy::enter(tv)?;
    // SAFETY: TVM_SETIMAGELIST takes a kind and a list's handle (alive
    // while the tree shows it: held by the caller across the call, then
    // kept by `keep_shown`; or a system list) or none; called on the tree's
    // class's own procedure.
    unsafe { busy.call(TVM_SETIMAGELIST, which as usize, il) };
    Some(busy)
}

/// After an image list was given to `busy`'s tree as `which`: keeps alive
/// the list of goz-gui's the tree shows now, as its own procedure answers.
/// That is `offered`, the one just given; or the one kept before, given by
/// a call nested in the giving (from a notification the tree sent as it
/// took `offered`), which the tree took after; or none (a system list, or
/// nothing). The list kept before, if the tree no longer shows it, is let
/// go once the tree is out of its processing.
fn keep_shown(busy: &Busy, which: u32, offered: Option<ImageList>) {
    if !busy.same() {
        return;
    }
    // SAFETY: TVM_GETIMAGELIST takes a kind and answers a handle; to the
    // tree's class's own procedure.
    let shown = unsafe { busy.call(TVM_GETIMAGELIST, which as usize, 0) };
    let key = busy.tree as usize;
    let unused = TREES.with_borrow_mut(|t| {
        let Some(t) = t.get_mut(&key).filter(|t| Some(t.serial) == busy.serial) else {
            return offered;
        };
        let kept = t.lists.get(&which).map(imagelist::raw_of);
        let given_back = match offered {
            Some(list) if imagelist::raw_of(&list) == shown => t.lists.insert(which, list),
            // The caller's: it holds its own.
            not_shown if kept == Some(shown) => return not_shown,
            not_shown => {
                t.retired.extend(t.lists.remove(&which));
                return not_shown;
            }
        };
        t.retired.extend(given_back);
        None
    });
    drop(unused);
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
    let busy = Busy::enter(item.tree)?;
    let mut rc = RECT::default();
    // TVM_GETITEMRECT takes the item in the rectangle's first bytes.
    // SAFETY: a RECT is 16 bytes, room for the handle; the item is in the
    // tree (`live`), and so when its own procedure, called directly, reads
    // it; written by the tree, read during the call.
    let ok = unsafe {
        (&raw mut rc).cast::<HTREEITEM>().write_unaligned(item.h);
        busy.call(
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
    if !tracked(tree) {
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
    let busy = Busy::enter(item.tree)?;
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
    // its item is in the tree (`live`), and so when the tree's own
    // procedure, called directly, reads it.
    let ok = unsafe { busy.call(TVM_GETITEMW, 0, (&raw mut it) as isize) } != 0;
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
    let Some(busy) = Busy::enter(item.tree) else {
        return false;
    };
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
    // named) whose text is set above, mutable as the tree may write it; its
    // item is in the tree (`live`), and so when the tree's own procedure,
    // called directly, reads it.
    unsafe { busy.call(TVM_SETITEMW, 0, (&raw mut it) as isize) != 0 }
}

/// Where an insert goes: its parent and place as TVINSERTSTRUCTW takes
/// them, and the count of deletions the tree had begun ([`calm`]).
struct Place {
    parent: HTREEITEM,
    after: HTREEITEM,
    begun: u64,
}

/// Where an insert in `busy`'s tree under `parent`, placed as `after`
/// says, goes; `None` if the tree takes no insert now, `parent` or `after`
/// is not an item of the tree, or `after` is not a child of `parent`.
fn place(busy: &Busy, parent: Option<Item>, after: After) -> Option<Place> {
    let tree = busy.tree;
    let begun = calm(tree)?;
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
            if a.tree != tree || !live(a) || busy.next(a.h, TVGN_PARENT) != under {
                return None;
            }
            a.h
        }
    };
    Some(Place {
        parent,
        after,
        begun,
    })
}

/// Sends TVM_INSERTITEMW of `ins` (its parent and place from `at`) to
/// `busy`'s tree's class's own procedure, and notes the item it answers:
/// the tree's own new item, which no subclass answered for. Not noted
/// (`None`) if a deletion of the tree's items began meanwhile, as it may
/// have taken the item already, or the item's parent is out of the record
/// since (its deletion waits, and so does the new item's with it).
fn insert_struct(busy: &Busy, ins: &mut TVINSERTSTRUCTW, at: &Place) -> Option<Item> {
    let tree = busy.tree;
    // SAFETY: TVM_INSERTITEMW's lParam is a TVINSERTSTRUCTW whose text the
    // caller set (a NUL-terminated string alive across the call, or the
    // callback marker), mutable as the tree may write it; its parent and
    // place are items in the tree, or its markers (`place`, which only
    // walked the tree since it checked them). Called on the tree's class's
    // own procedure; the tree is busy meanwhile, so no deletion of goz-gui's
    // takes them while it reads them.
    let h = unsafe { busy.call(TVM_INSERTITEMW, 0, (&raw mut *ins) as isize) };
    if h == 0 || !busy.same() {
        return None;
    }
    let under_noted = at.parent == TVI_ROOT || noted(tree, at.parent);
    let noted = TREES.with_borrow_mut(|t| match t.get_mut(&(tree as usize)) {
        Some(t) if t.deletions == at.begun && t.deleting == 0 && under_noted => {
            t.items.as_mut().is_some_and(|items| {
                items.insert(h);
                true
            })
        }
        _ => false,
    });
    noted.then_some(Item { tree, h })
}

/// Inserts `item` (its TVIF_* fields) in `tree` under `parent` (`None`: at
/// the top), placed as `after` says, with `text` when given (TVIF_TEXT
/// then; for a callback item, leave it out and answer TVN_GETDISPINFO).
/// The new item; `None` if not, and while a deletion of the tree's items
/// is under way.
pub fn insert(
    tree: HWND,
    parent: Option<Item>,
    after: After,
    item: TVITEMEXW,
    text: Option<&str>,
) -> Option<Item> {
    let busy = Busy::enter(tree)?;
    let at = place(&busy, parent, after)?;
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
        hParent: at.parent,
        hInsertAfter: at.after,
        ..Default::default()
    };
    ins.Anonymous.itemex = it;
    insert_struct(&busy, &mut ins, &at)
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
    let busy = Busy::enter(tree)?;
    let at = place(&busy, parent, after)?;
    let z = super::wz_units(super::until_nul(text));
    let mut it = item;
    it.mask = (it.mask | TVIF_TEXT) & !EX_ONLY;
    it.pszText = z.as_ptr().cast_mut();
    it.cchTextMax = 0;
    let mut ins = TVINSERTSTRUCTW {
        hParent: at.parent,
        hInsertAfter: at.after,
        ..Default::default()
    };
    ins.Anonymous.item = it;
    insert_struct(&busy, &mut ins, &at)
}

/// Sets tree item `item`'s text from UTF-16 (up to a NUL, if it has one).
pub fn set_text_units(item: Item, text: &[u16]) -> bool {
    if !live(item) {
        return false;
    }
    let Some(busy) = Busy::enter(item.tree) else {
        return false;
    };
    let z = super::wz_units(super::until_nul(text));
    let mut it = TVITEMW {
        mask: TVIF_TEXT,
        hItem: item.h,
        pszText: z.as_ptr().cast_mut(),
        ..Default::default()
    };
    // SAFETY: TVM_SETITEMW's lParam is a TVITEMW whose only pointer is the
    // text set above, read during the call (the structure mutable, as the
    // tree may write it); its item is in the tree, and so when the tree's
    // own procedure, called directly, reads it.
    unsafe { busy.call(TVM_SETITEMW, 0, (&raw mut it) as isize) != 0 }
}

/// An item a tree notified, as an [`Item`]: `tree` is the notifying tree.
fn notified(tree: HWND, h: HTREEITEM) -> Option<Item> {
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

#[cfg(test)]
mod tests {
    use std::cell::{Cell, RefCell};
    use std::rc::Rc;

    use windows_sys::Win32::UI::Controls::{
        TVE_COLLAPSE, TVE_EXPAND, TVIF_PARAM, TVM_GETIMAGELIST, TVN_DELETEITEMW,
    };

    use super::super::hook::{self, Event, Kind};
    use super::super::imagelist::ImageList;
    use super::super::subclass;
    use super::super::trampolines_tests::hidden_tree;
    use super::*;

    fn item(param: isize) -> TVITEMEXW {
        TVITEMEXW {
            mask: TVIF_PARAM,
            lParam: param,
            ..Default::default()
        }
    }

    /// What an inner subclass answers for an insert: an item of no tree.
    const MADE_UP: HTREEITEM = 0x10;

    static INNER: subclass::Key = subclass::Key::new("test tree answers, inner");

    /// A subclass between the tree's own procedure and the one that keeps
    /// track of it: it inserts but answers a made-up item, hides every
    /// item's children, and deletes but answers that it did not.
    fn lying(m: &Msg) -> LRESULT {
        match m.id {
            TVM_INSERTITEMW => {
                m.def();
                MADE_UP
            }
            TVM_GETNEXTITEM if m.w == TVGN_CHILD as usize => 0,
            TVM_DELETEITEM => {
                m.def();
                0
            }
            _ => m.def(),
        }
    }

    #[test]
    fn a_tree_s_record_is_made_of_its_own_answers() {
        // The lying subclass goes in as the tree is made, before the one
        // that keeps track of it: the nearer to the tree.
        let hook = hook::set(Kind::Cbt, |event| {
            if let Event::Create { hwnd, .. } = *event
                && super::super::class_is(hwnd, CLASS)
            {
                subclass::set(hwnd, &INNER, lying);
            }
            None
        });
        let (tree, host) = hidden_tree(|m| m.def());
        drop(hook);
        assert!(subclass::has(tree, &INNER));
        let p = insert(tree, None, After::Last, item(1), Some("p")).expect("p");
        assert_ne!(p.h, MADE_UP);
        assert_eq!(root(tree), Some(p));
        assert_eq!(item_param(p), Some(1));
        let c = insert(tree, Some(p), After::Last, item(2), Some("c")).expect("c");
        assert_eq!(child(p), Some(c));
        // Deleted through the window, as another's code deletes (its child
        // too, which the subclass hides), though the answer says it was
        // not: neither is taken after.
        // SAFETY: TVM_DELETEITEM of an item the tree has.
        let answer = unsafe { super::super::send_raw(tree, TVM_DELETEITEM, 0, p.h) };
        assert_eq!(answer, 0);
        assert!(!live(p) && !live(c));
        assert_eq!(root(tree), None);
        // goz-gui's own deletion goes to the tree itself, past the subclass.
        let q = insert(tree, None, After::Last, item(3), Some("q")).expect("q");
        assert!(delete_item(q));
        assert!(!live(q));
        // A deletion passed on twice by a subclass in front: the second
        // time its item is refused, never followed.
        assert!(subclass::set(tree, &TWICE, |m| {
            if m.id == TVM_DELETEITEM {
                m.def();
            }
            m.def()
        }));
        let r = insert(tree, None, After::Last, item(4), Some("r")).expect("r");
        // SAFETY: TVM_DELETEITEM of an item the tree has.
        unsafe { super::super::send_raw(tree, TVM_DELETEITEM, 0, r.h) };
        assert!(!live(r));
        assert_eq!(root(tree), None);
        super::super::destroy(host);
    }

    static TWICE: subclass::Key = subclass::Key::new("test tree deletion passed on twice");

    static DELETES_FIRST: subclass::Key = subclass::Key::new("test tree subclass deleting first");

    #[test]
    fn goz_gui_s_item_messages_go_to_the_tree_itself() {
        let (tree, host) = hidden_tree(|m| m.def());
        let x = insert(tree, None, After::Last, item(5), Some("x")).expect("x");
        // A subclass that would delete the item a message names before
        // passing the message on: it never sees goz-gui's.
        let seen = Rc::new(Cell::new(0));
        let s = Rc::clone(&seen);
        assert!(subclass::set(tree, &DELETES_FIRST, move |m| {
            if (0x1100..0x1200).contains(&m.id) {
                s.set(s.get() + 1);
                if let Some(first) = root(m.hwnd) {
                    delete_item(first);
                }
            }
            m.def()
        }));
        assert_eq!(item_param(x), Some(5));
        assert!(set_text_units(x, &[120]));
        let _ = item_rect(x, false);
        ensure_visible(x);
        assert!(select(tree, Select::Caret, Some(x)));
        assert_eq!(selection(tree), Some(x));
        expand(x, TVE_EXPAND);
        assert_eq!(child(x), None);
        assert_eq!((seen.get(), live(x)), (0, true));
        assert!(delete_item(x));
        assert_eq!(seen.get(), 0);
        super::super::destroy(host);
    }

    /// A subclass that, for a TVM_GETITEMW or a TVM_EXPAND (another's
    /// code's, through the window), deletes the tree's first item, the one
    /// the message names, then passes the message on; and passes a
    /// TVM_DELETEITEM on twice. What each pass answered goes in `passes`.
    fn deletes_then_passes(passes: Rc<RefCell<Vec<isize>>>) -> impl Fn(&Msg) -> LRESULT + 'static {
        move |m| match m.id {
            TVM_GETITEMW | TVM_EXPAND => {
                if let Some(first) = root(m.hwnd) {
                    assert!(delete_item(first));
                }
                let answer = m.def();
                passes.borrow_mut().push(answer);
                answer
            }
            TVM_DELETEITEM => {
                let first = m.def();
                let second = m.def();
                passes.borrow_mut().extend([first, second]);
                first
            }
            _ => m.def(),
        }
    }

    /// A subclass put in nearest the tree, at its birth: it notes each
    /// message naming an item that reaches it (in `reached`), and passes
    /// it on to the tree.
    fn nearest(reached: Rc<RefCell<Vec<u32>>>) -> impl Fn(&Msg) -> LRESULT + 'static {
        move |m| {
            if names_an_item(m.id) {
                reached.borrow_mut().push(m.id);
            }
            m.def()
        }
    }

    /// Sends `tree`, a subclass of which is [`deletes_then_passes`] (what
    /// its passes answered in `passes`), as another's code would, through
    /// its window: a TVM_GETITEMW and a TVM_EXPAND of its first item, which
    /// the subclass deletes before passing each on, and a TVM_DELETEITEM it
    /// passes on twice. What was passed on after its item went answers 0,
    /// and never reaches the tree: [`nearest`], nearer it than that
    /// subclass, notes in `reached` what does.
    fn passed_on_after_its_item_went(
        tree: HWND,
        passes: &RefCell<Vec<isize>>,
        reached: &RefCell<Vec<u32>>,
    ) {
        use super::super::send_raw;
        let x = insert(tree, None, After::Last, item(1), Some("x")).expect("x");
        let c = insert(tree, Some(x), After::Last, item(2), Some("c")).expect("c");
        let mut it = TVITEMW {
            mask: TVIF_PARAM,
            hItem: x.h,
            lParam: 7,
            ..Default::default()
        };
        // SAFETY: TVM_GETITEMW of a TVITEMW asking for no text, alive
        // across the call, of an item the tree has as it is sent.
        let answer = unsafe { send_raw(tree, TVM_GETITEMW, 0, (&raw mut it) as isize) };
        assert_eq!((answer, passes.take()), (0, vec![0]));
        // The tree did not write it.
        assert_eq!(it.lParam, 7);
        assert!(!live(x) && !live(c));
        let y = insert(tree, None, After::Last, item(3), Some("y")).expect("y");
        insert(tree, Some(y), After::Last, item(4), Some("d")).expect("d");
        // SAFETY: TVM_EXPAND of an item the tree has as it is sent.
        let answer = unsafe { send_raw(tree, TVM_EXPAND, TVE_EXPAND as usize, y.h) };
        assert_eq!((answer, passes.take()), (0, vec![0]));
        assert!(!live(y));
        assert_eq!(root(tree), None);
        assert_eq!(reached.take(), Vec::<u32>::new());
        // A deletion passed on twice: the second time it goes no further.
        let z = insert(tree, None, After::Last, item(5), Some("z")).expect("z");
        // SAFETY: TVM_DELETEITEM of an item the tree has as it is sent.
        let answer = unsafe { send_raw(tree, TVM_DELETEITEM, 0, z.h) };
        assert_ne!(answer, 0);
        let both = passes.take();
        assert!(both.len() == 2 && both[0] != 0 && both[1] == 0, "{both:?}");
        assert_eq!(reached.take(), [TVM_DELETEITEM]);
        assert!(!live(z));
        assert_eq!(root(tree), None);
        // An item the tree has goes through each subclass to it.
        let v = insert(tree, None, After::Last, item(6), Some("v")).expect("v");
        let mut it = TVITEMW {
            mask: TVIF_PARAM,
            hItem: v.h,
            ..Default::default()
        };
        // SAFETY: TVM_SETITEMW of a TVITEMW naming no text, alive across
        // the call, of an item the tree has.
        let answer = unsafe { send_raw(tree, TVM_SETITEMW, 0, (&raw mut it) as isize) };
        assert_ne!(answer, 0);
        assert_eq!(reached.take(), [TVM_SETITEMW]);
        assert!(live(v));
    }

    static DELETES_AFTER: subclass::Key = subclass::Key::new("test tree subclass, put in after");

    static DELETES_AT_BIRTH: subclass::Key = subclass::Key::new("test tree subclass, at birth");

    static NEAREST_PASSING: subclass::Key =
        subclass::Key::new("test tree messages that reach it, passed on");

    /// A hidden tree with [`nearest`] put in at its birth by a CBT hook,
    /// and then the subclass [`deletes_then_passes`] as `key`, if given.
    fn tree_with_nearest(
        reached: &Rc<RefCell<Vec<u32>>>,
        then: Option<(&'static subclass::Key, &Rc<RefCell<Vec<isize>>>)>,
    ) -> (HWND, HWND) {
        let r = Rc::clone(reached);
        let then = then.map(|(key, passes)| (key, Rc::clone(passes)));
        let hook = hook::set(Kind::Cbt, move |event| {
            if let Event::Create { hwnd, .. } = *event
                && super::super::class_is(hwnd, CLASS)
            {
                // Each subclass put in goes in front of those before it.
                subclass::set(hwnd, &NEAREST_PASSING, nearest(Rc::clone(&r)));
                if let Some((key, passes)) = &then {
                    subclass::set(hwnd, key, deletes_then_passes(Rc::clone(passes)));
                }
            }
            None
        });
        let made = hidden_tree(|m| m.def());
        drop(hook);
        assert!(subclass::has(made.0, &NEAREST_PASSING));
        made
    }

    #[test]
    fn a_subclass_passes_on_no_message_whose_item_it_deleted() {
        let passes = Rc::new(RefCell::new(Vec::new()));
        let reached = Rc::new(RefCell::new(Vec::new()));
        // Put in after the tree was made: farther from the tree than the
        // subclass that keeps track of it.
        let (tree, host) = tree_with_nearest(&reached, None);
        assert!(subclass::set(
            tree,
            &DELETES_AFTER,
            deletes_then_passes(Rc::clone(&passes))
        ));
        passed_on_after_its_item_went(tree, &passes, &reached);
        super::super::destroy(host);
        // Put in at the tree's birth by a CBT hook: nearer the tree.
        let (tree, host) = tree_with_nearest(&reached, Some((&DELETES_AT_BIRTH, &passes)));
        assert!(subclass::has(tree, &DELETES_AT_BIRTH));
        passed_on_after_its_item_went(tree, &passes, &reached);
        super::super::destroy(host);
    }

    static DESTROYS: subclass::Key = subclass::Key::new("test tree subclass destroying it");

    #[test]
    fn a_subclass_passes_on_no_item_message_to_a_tree_it_destroyed() {
        let (tree, host) = hidden_tree(|m| m.def());
        let x = insert(tree, None, After::Last, item(1), Some("x")).expect("x");
        let passed = Rc::new(Cell::new(None));
        let p = Rc::clone(&passed);
        assert!(subclass::set(tree, &DESTROYS, move |m| {
            if m.id == TVM_GETITEMW {
                super::super::destroy(m.hwnd);
                let answer = m.def();
                p.set(Some(answer));
                return answer;
            }
            m.def()
        }));
        let mut it = TVITEMW {
            mask: TVIF_PARAM,
            hItem: x.h,
            lParam: 7,
            ..Default::default()
        };
        // SAFETY: TVM_GETITEMW of a TVITEMW asking for no text, alive
        // across the call, of an item the tree has as it is sent.
        let answer =
            unsafe { super::super::send_raw(tree, TVM_GETITEMW, 0, (&raw mut it) as isize) };
        assert_eq!((answer, passed.get(), it.lParam), (0, Some(0), 7));
        assert!(!super::super::is_window(tree) && !live(x));
        super::super::destroy(host);
    }

    /// Sends `tree`, through its window, each message that names an item,
    /// naming `h` where the item goes (each of an insert's two places in
    /// turn, the other a special handle). The messages sent, in order.
    fn send_each_naming(tree: HWND, h: HTREEITEM) -> Vec<u32> {
        let mut sent = Vec::new();
        let mut send = |id: u32, w: usize, l: isize| {
            // SAFETY: the parameters are what `id` defines for a tree view
            // (any structure alive across the call); the item they name may
            // be gone, but the subclass nearest the tree answers every such
            // message itself, so the tree reads none of them.
            unsafe { super::super::send_raw(tree, id, w, l) };
            sent.push(id);
        };
        for (id, w) in [
            (TVM_DELETEITEM, 0),
            (TVM_EXPAND, TVE_EXPAND as usize),
            (TVM_GETNEXTITEM, TVGN_CHILD as usize),
            (TVM_SELECTITEM, TVGN_CARET as usize),
            (TVM_EDITLABELA, 0),
            (TVM_EDITLABELW, 0),
            (TVM_CREATEDRAGIMAGE, 0),
            (TVM_SORTCHILDREN, 0),
            (TVM_ENSUREVISIBLE, 0),
            (TVM_SETINSERTMARK, 0),
            (TVM_SETHOT, 0),
            (TVM_SHOWINFOTIP, 0),
        ] {
            send(id, w, h);
        }
        for id in [TVM_GETITEMSTATE, TVM_MAPHTREEITEMTOACCID] {
            send(id, h as usize, 0);
        }
        let mut rc = RECT::default();
        // SAFETY: a RECT is 16 bytes, room for the handle.
        unsafe { (&raw mut rc).cast::<HTREEITEM>().write_unaligned(h) };
        send(TVM_GETITEMRECT, 0, (&raw mut rc) as isize);
        for id in [TVM_GETITEMA, TVM_GETITEMW, TVM_SETITEMA, TVM_SETITEMW] {
            let mut it = TVITEMW {
                hItem: h,
                ..Default::default()
            };
            send(id, 0, (&raw mut it) as isize);
        }
        for id in [TVM_INSERTITEMA, TVM_INSERTITEMW] {
            for (parent, after) in [(h, TVI_LAST), (TVI_ROOT, h)] {
                let mut ins = TVINSERTSTRUCTW {
                    hParent: parent,
                    hInsertAfter: after,
                    ..Default::default()
                };
                send(id, 0, (&raw mut ins) as isize);
            }
        }
        let mut cb = TVSORTCB {
            hParent: h,
            ..Default::default()
        };
        send(TVM_SORTCHILDRENCB, 0, (&raw mut cb) as isize);
        let mut part = RECT::default();
        let mut info = TVGETITEMPARTRECTINFO {
            hti: h,
            prc: &raw mut part,
            ..Default::default()
        };
        send(TVM_GETITEMPARTRECT, 0, (&raw mut info) as isize);
        sent
    }

    static NEAREST: subclass::Key = subclass::Key::new("test tree messages that reach it");

    #[test]
    fn a_message_passed_on_to_a_tree_names_only_its_items() {
        // Nearest the tree, put in at its birth: it notes the messages
        // naming an item that reach it, and answers them itself.
        let reached = Rc::new(RefCell::new(Vec::new()));
        let r = Rc::clone(&reached);
        let hook = hook::set(Kind::Cbt, move |event| {
            if let Event::Create { hwnd, .. } = *event
                && super::super::class_is(hwnd, CLASS)
            {
                let r = Rc::clone(&r);
                subclass::set(hwnd, &NEAREST, move |m| {
                    if names_an_item(m.id) {
                        r.borrow_mut().push(m.id);
                        return 0;
                    }
                    m.def()
                });
            }
            None
        });
        let (tree, host) = hidden_tree(|m| m.def());
        drop(hook);
        assert!(subclass::has(tree, &NEAREST));
        let x = insert(tree, None, After::Last, item(1), Some("x")).expect("x");
        let gone = insert(tree, None, After::Last, item(2), Some("gone")).expect("gone");
        assert!(delete_item(gone));
        // Each message naming an item the tree has reaches it, and so does
        // each naming a special handle.
        let sent = send_each_naming(tree, x.h);
        assert_eq!(reached.take(), sent);
        assert!(live(x));
        let sent = send_each_naming(tree, TVI_ROOT);
        assert_eq!(reached.take(), sent);
        // None naming an item that went.
        send_each_naming(tree, gone.h);
        assert_eq!(reached.take(), Vec::<u32>::new());
        // 0 only where the message means no item by it.
        send_each_naming(tree, 0);
        assert_eq!(
            reached.take(),
            [
                TVM_DELETEITEM,
                TVM_GETNEXTITEM,
                TVM_SELECTITEM,
                TVM_SETINSERTMARK,
                TVM_SETHOT,
                TVM_INSERTITEMA,
                TVM_INSERTITEMW,
            ]
        );
        // Nor a message whose structure is missing.
        // SAFETY: TVM_GETITEMW with no structure, answered by the subclass
        // nearest the tree if it gets that far.
        let answer = unsafe { super::super::send_raw(tree, TVM_GETITEMW, 0, 0) };
        assert_eq!(answer, 0);
        assert_eq!(reached.take(), Vec::<u32>::new());
        assert!(live(x));
        super::super::destroy(host);
    }

    /// What the parent of a tree sees of the children `c1`, `c2` of its
    /// item `p` at each TVN_DELETEITEMW while `delete` deletes: whether
    /// each is still taken, and whether an insert went in.
    fn seen_while_deleting(delete: impl FnOnce(HWND, Item)) -> Vec<(Vec<bool>, bool)> {
        let watched: Rc<RefCell<Vec<Item>>> = Rc::new(RefCell::new(Vec::new()));
        let during = Rc::new(RefCell::new(Vec::new()));
        let (w, d) = (Rc::clone(&watched), Rc::clone(&during));
        let (tree, host) = hidden_tree(move |m| {
            if let Some(n) = m.notify()
                && n.code() == TVN_DELETEITEMW
                && !w.borrow().is_empty()
            {
                let items = w.borrow().clone();
                let taken = items.iter().map(|&i| live(i)).collect();
                let put = items
                    .first()
                    .is_some_and(|i| insert(i.tree(), None, After::Last, item(9), None).is_some());
                d.borrow_mut().push((taken, put));
            }
            m.def()
        });
        let p = insert(tree, None, After::Last, item(1), Some("p")).expect("p");
        let c1 = insert(tree, Some(p), After::Last, item(2), Some("c1")).expect("c1");
        let c2 = insert(tree, Some(p), After::Last, item(3), Some("c2")).expect("c2");
        *watched.borrow_mut() = vec![c1, c2];
        delete(host, p);
        watched.borrow_mut().clear();
        super::super::destroy(host);
        during.take()
    }

    #[test]
    fn an_item_is_refused_from_the_moment_its_deletion_starts() {
        let none_taken = (vec![false, false], false);
        // A collapse that resets the parent: its children go, it stays.
        let seen = seen_while_deleting(|_, p| {
            assert!(expand(p, TVE_EXPAND));
            expand(p, TVE_COLLAPSE | TVE_COLLAPSERESET);
            assert!(live(p));
            assert_eq!(child(p), None);
        });
        assert_eq!(seen, vec![none_taken.clone(); 2]);
        // The parent deleted, then every item, then the tree destroyed:
        // each child's deletion (and the parent's) is seen with neither
        // child taken.
        let seen = seen_while_deleting(|_, p| assert!(delete_item(p)));
        assert_eq!(seen, vec![none_taken.clone(); 3]);
        let seen = seen_while_deleting(|_, p| assert!(delete_all(p.tree())));
        assert_eq!(seen, vec![none_taken.clone(); 3]);
        let seen = seen_while_deleting(|host, _| {
            super::super::destroy(host);
        });
        assert_eq!(seen, vec![none_taken; 3]);
    }

    /// Every item `tree` has, as its own procedure walks them.
    fn walked(tree: HWND) -> HashSet<HTREEITEM> {
        Busy::enter(tree)
            .expect("a tree view of this thread")
            .walk()
    }

    /// The image list `tree` shows as `which`, as its own procedure answers.
    fn shown_list(tree: HWND, which: u32) -> isize {
        let busy = Busy::enter(tree).expect("a tree view of this thread");
        // SAFETY: TVM_GETIMAGELIST takes a kind and answers a handle; to
        // the tree's class's own procedure.
        unsafe { busy.call(TVM_GETIMAGELIST, which as usize, 0) }
    }

    /// The image list goz-gui keeps alive for `tree` as `which` (0: none).
    fn kept_list(tree: HWND, which: u32) -> isize {
        TREES.with_borrow(|t| {
            t.get(&(tree as usize))
                .and_then(|t| t.lists.get(&which))
                .map_or(0, imagelist::raw_of)
        })
    }

    #[test]
    fn a_tree_keeps_the_list_it_shows_whatever_is_given_meanwhile() {
        use windows_sys::Win32::UI::Controls::{ILC_COLOR32, NM_CUSTOMDRAW, TVSIL_NORMAL};
        let list = || ImageList::create(16, 16, ILC_COLOR32, 1, 1).expect("an image list");
        // While the tree takes a list it notifies its parent, which gives
        // it another: an image list of its own, which goes as the handler
        // returns (`Some(false)`), or a system list (`Some(true)`).
        let nested: Rc<Cell<Option<bool>>> = Rc::new(Cell::new(None));
        let n = Rc::clone(&nested);
        let (tree, host) = hidden_tree(move |m| {
            if let Some(note) = m.notify()
                && note.code() == NM_CUSTOMDRAW
                && let Some(system) = n.take()
            {
                if system {
                    let small = imagelist::system(1).expect("the small icons");
                    assert!(set_system_image_list(note.from(), TVSIL_NORMAL, small));
                } else {
                    let b = list();
                    assert!(set_image_list(note.from(), TVSIL_NORMAL, Some(&b)));
                }
            }
            m.def()
        });
        let a = list();
        nested.set(Some(false));
        assert!(set_image_list(tree, TVSIL_NORMAL, Some(&a)));
        assert_eq!(nested.get(), None, "the tree notified as it took A");
        // What the tree shows is what goz-gui keeps alive: the nested list
        // (A is the caller's), not one let go.
        let shown = shown_list(tree, TVSIL_NORMAL);
        assert_ne!(shown, imagelist::raw_of(&a));
        assert_eq!(kept_list(tree, TVSIL_NORMAL), shown);
        // A system list given meanwhile: no list of goz-gui's is kept.
        let c = list();
        nested.set(Some(true));
        assert!(set_image_list(tree, TVSIL_NORMAL, Some(&c)));
        assert_eq!(nested.get(), None);
        assert_ne!(shown_list(tree, TVSIL_NORMAL), imagelist::raw_of(&c));
        assert_eq!(kept_list(tree, TVSIL_NORMAL), 0);
        super::super::destroy(host);
    }

    #[test]
    fn an_item_deleted_inside_a_notification_about_it_goes_after() {
        use windows_sys::Win32::UI::Controls::TVN_SELCHANGINGW;
        use windows_sys::Win32::UI::Input::KeyboardAndMouse::VK_DOWN;
        use windows_sys::Win32::UI::WindowsAndMessaging::WM_KEYDOWN;
        // The parent deletes the item a TVN_SELCHANGINGW is about, as the
        // tree is about to select it.
        let doomed: Rc<Cell<Option<Item>>> = Rc::new(Cell::new(None));
        let d = Rc::clone(&doomed);
        let (tree, host) = hidden_tree(move |m| {
            if let Some(n) = m.notify()
                && n.code() == TVN_SELCHANGINGW
                && let Some(item) = d.take()
            {
                let about = n.get::<NMTREEVIEWW>().and_then(|v| v.item_new());
                assert_eq!(about, Some(item));
                assert!(delete_item(item));
                // Refused from the moment its deletion is asked for.
                assert!(!live(item));
            }
            m.def()
        });
        let a = insert(tree, None, After::Last, item(1), Some("a")).expect("a");
        let b = insert(tree, None, After::Last, item(2), Some("b")).expect("b");
        insert(tree, Some(b), After::Last, item(3), Some("b1")).expect("b1");
        doomed.set(Some(b));
        select(tree, Select::Caret, Some(b));
        assert_eq!(doomed.get(), None, "the parent was asked");
        // Gone by the time the selection is done.
        assert_eq!(walked(tree), HashSet::from([a.h]));
        assert_eq!((root(tree), next_sibling(a)), (Some(a), None));
        // The same from the keyboard: the tree moves the selection itself.
        assert!(select(tree, Select::Caret, Some(a)));
        let c = insert(tree, None, After::Last, item(4), Some("c")).expect("c");
        doomed.set(Some(c));
        super::super::send_plain(tree, WM_KEYDOWN, usize::from(VK_DOWN), 0);
        assert_eq!(doomed.get(), None, "the parent was asked");
        assert_eq!(walked(tree), HashSet::from([a.h]));
        assert_eq!(selection(tree), Some(a));
        super::super::destroy(host);
    }

    #[test]
    fn an_insert_s_place_stays_while_the_item_goes_in() {
        use windows_sys::Win32::UI::Controls::NM_CUSTOMDRAW;
        // While an item goes in, the parent deletes the item it goes after.
        let doomed: Rc<Cell<Option<Item>>> = Rc::new(Cell::new(None));
        let d = Rc::clone(&doomed);
        let (tree, host) = hidden_tree(move |m| {
            if let Some(n) = m.notify()
                && n.code() == NM_CUSTOMDRAW
                && let Some(item) = d.take()
            {
                assert!(delete_item(item));
            }
            m.def()
        });
        let a = insert(tree, None, After::Last, item(1), Some("a")).expect("a");
        doomed.set(Some(a));
        let b = insert(tree, None, After::Item(a), item(2), Some("b")).expect("b");
        assert_eq!(doomed.get(), None, "the parent was asked");
        assert_eq!(walked(tree), HashSet::from([b.h]));
        assert!(live(b) && !live(a));
        super::super::destroy(host);
    }

    #[test]
    fn a_collapse_that_resets_the_top_takes_every_item() {
        use super::super::send_raw;
        let (tree, host) = hidden_tree(|m| m.def());
        let x = insert(tree, None, After::Last, item(1), Some("x")).expect("x");
        let c = insert(tree, Some(x), After::Last, item(2), Some("c")).expect("c");
        let code = (TVE_COLLAPSE | TVE_COLLAPSERESET) as usize;
        // 0 names no item there: refused before the tree.
        // SAFETY: TVM_EXPAND of a code and no item.
        assert_eq!(unsafe { send_raw(tree, TVM_EXPAND, code, 0) }, 0);
        assert!(live(x) && live(c));
        // Another's code resets the top through the window: whatever the
        // tree takes out goes out of the record too.
        // SAFETY: TVM_EXPAND of a code and TVI_ROOT, a special handle.
        unsafe { send_raw(tree, TVM_EXPAND, code, TVI_ROOT) };
        let left = walked(tree);
        for i in [x, c] {
            assert_eq!(live(i), left.contains(&i.h), "{i:?}");
        }
        super::super::destroy(host);
    }

    static RECYCLES: subclass::Key = subclass::Key::new("test tree destroyed while it deletes");

    /// A tree with a subclass nearest it, put in at its birth: it passes a
    /// deletion of every item on, then destroys the tree (as another's code
    /// would, past goz-gui), runs `then` with the tree's handle, and answers
    /// that nothing was deleted (so the deletion is checked by a walk).
    fn tree_destroyed_while_it_deletes(then: fn(HWND)) -> (HWND, HWND, Rc<Cell<bool>>) {
        use windows_sys::Win32::UI::WindowsAndMessaging::DestroyWindow;
        let gone = Rc::new(Cell::new(false));
        let g = Rc::clone(&gone);
        let hook = hook::set(Kind::Cbt, move |event| {
            if let Event::Create { hwnd, .. } = *event
                && super::super::class_is(hwnd, CLASS)
            {
                let g = Rc::clone(&g);
                subclass::set(hwnd, &RECYCLES, move |m| {
                    if m.id == TVM_DELETEITEM && m.l == TVI_ROOT {
                        m.def();
                        // SAFETY: no pointers; a window of this thread.
                        g.set(unsafe { DestroyWindow(m.hwnd) } != 0);
                        then(m.hwnd);
                        return 0;
                    }
                    m.def()
                });
            }
            None
        });
        let (tree, host) = hidden_tree(|m| m.def());
        drop(hook);
        (tree, host, gone)
    }

    #[test]
    fn a_tree_gone_while_it_deletes_is_not_walked() {
        let (tree, host, gone) = tree_destroyed_while_it_deletes(|_| {});
        let x = insert(tree, None, After::Last, item(1), Some("x")).expect("x");
        // SAFETY: TVM_DELETEITEM of TVI_ROOT: every item.
        unsafe { super::super::send_raw(tree, TVM_DELETEITEM, 0, TVI_ROOT) };
        assert!(gone.get() && !super::super::is_window(tree));
        assert!(!live(x));
        assert!(!TREES.with_borrow(|t| t.contains_key(&(tree as usize))));
        super::super::destroy(host);
    }

    /// Makes and destroys windows until the next one made gets `handle`
    /// again, then makes a list view there. A handle is a slot's index and
    /// a count of the slot's reuses, which goes round; a slot freed last is
    /// the next one taken.
    fn list_view_at(handle: HWND) {
        use super::super::class::{self, Create};
        use windows_sys::Win32::UI::WindowsAndMessaging::{DestroyWindow, HWND_MESSAGE};
        let make = |class: &str| {
            class::create_plain(&Create {
                ex_style: 0,
                class,
                title: "",
                style: 0,
                rect: (0, 0, 10, 10),
                parent: HWND_MESSAGE,
                menu: core::ptr::null_mut(),
            })
        };
        // SAFETY: no pointers; a window made here.
        let destroy = |w: HWND| unsafe { DestroyWindow(w) };
        let slot = |w: HWND| w as usize & 0xffff;
        // Windows made and kept until one is in `handle`'s slot; that one
        // freed last, so the windows made next take its slot.
        let mut held = Vec::new();
        let mut at = make("Static");
        while slot(at) != slot(handle) {
            assert!(held.len() < 10_000, "the slot is not free");
            held.push(at);
            at = make("Static");
        }
        held.into_iter().for_each(|w| {
            destroy(w);
        });
        destroy(at);
        // The handle made just before `handle` comes round, once seen.
        let mut before = None;
        let mut last = at;
        for _ in 0..100_000 {
            let w = make("Static");
            if w == handle {
                before = Some(last);
            }
            destroy(w);
            let next = (w as usize).wrapping_add(0x1_0000) as HWND;
            if next == handle || Some(w) == before {
                let lv = make(super::super::lv::CLASS);
                assert_eq!(lv, handle, "the handle came round");
                return;
            }
            last = w;
        }
        panic!("the handle did not come round");
    }

    #[test]
    #[ignore = "recycles a window handle: some 33,000 windows made and destroyed"]
    fn a_tree_whose_handle_is_now_a_list_view_s_is_not_walked() {
        let (tree, host, gone) = tree_destroyed_while_it_deletes(list_view_at);
        let x = insert(tree, None, After::Last, item(1), Some("x")).expect("x");
        // SAFETY: TVM_DELETEITEM of TVI_ROOT: every item.
        unsafe { super::super::send_raw(tree, TVM_DELETEITEM, 0, TVI_ROOT) };
        assert!(gone.get());
        assert!(super::super::class_is(tree, super::super::lv::CLASS));
        assert!(!live(x));
        super::super::destroy(tree);
        super::super::destroy(host);
    }
}
