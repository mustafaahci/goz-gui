//! The shell's COM side, for the results: the selection as the shell knows
//! it ([`Items`]), its Explorer context menu ([`ContextMenu`]) and its data
//! object (what the clipboard and drag and drop carry), dragging it out
//! ([`drag`]), the data object of a cut being pasted and the end of the
//! paste that moved its files ([`Cut`]), shortcuts' targets
//! ([`link_target`]), plus OLE on the UI thread and the flags file
//! operations run with (`crate::fileops` runs them).
//!
//! Everything here runs on the UI thread, an STA: the shell's objects and
//! the handlers they load belong to it. Their calls can pump messages (a
//! handler showing UI, a menu loop), so callers hold no `RefCell` borrow
//! across them.
//!
//! The `windows` crate brings the interfaces; the rest of goz-gui stays on
//! `windows-sys`, whose handles are the same pointers.

#![allow(unsafe_code)] // Not yet moved onto `crate::win`.

use std::cell::Cell;
use std::collections::HashMap;
use std::rc::Rc;

use windows::Win32::System::Com::IDataObject;
use windows::Win32::System::Ole::{
    DROPEFFECT_COPY, DROPEFFECT_LINK, DROPEFFECT_MOVE, DROPEFFECT_NONE,
};
use windows::Win32::UI::Shell::{
    CMF_CANRENAME, CMF_EXTENDEDVERBS, CMF_ITEMMENU, CMF_NORMAL, CMIC_MASK_CONTROL_DOWN,
    CMIC_MASK_PTINVOKE, CMIC_MASK_SHIFT_DOWN, FOF_ALLOWUNDO, FOF_NOCONFIRMATION,
    FOF_NOCONFIRMMKDIR, FOF_NOERRORUI, FOF_RENAMEONCOLLISION, FOF_SILENT, FOFX_ADDUNDORECORD,
    FOFX_RECYCLEONDELETE, IContextMenu, IShellFolder,
};
use windows::core::Interface;
use windows_sys::Win32::Foundation::{HWND, POINT};
use windows_sys::Win32::UI::WindowsAndMessaging::HMENU;

use crate::win::Msg;
use crate::win::ext::b7::{self, Pidl, ShellMenu};

thread_local! {
    /// OLE is initialised on this thread.
    static OLE: Cell<bool> = const { Cell::new(false) };
}

/// OLE on this thread (the UI thread), once: the shell's Copy and Cut put
/// their data object on the clipboard through it, and drag and drop needs
/// it. Made when first needed, so a window that never uses them never
/// loads it.
pub fn ole_ready() -> bool {
    if !OLE.get() {
        // On the UI thread, which is already an STA (as OLE wants).
        OLE.set(b7::ole_initialize());
    }
    OLE.get()
}

/// The process is ending: a data object of goz-gui's still on the
/// clipboard (the shell's Copy) is rendered onto it, so a paste still
/// works once goz-gui is gone.
pub fn ole_done() {
    if OLE.get() {
        // OLE is initialised on this thread; with no data object of this
        // thread on the clipboard it does nothing.
        b7::ole_flush_clipboard();
    }
}

/// The folder holding `path` (as a path the shell parses) and the name of
/// `path` in it; `None` for a path that is a root (a drive, a share), which
/// the shell parses whole.
fn split_parent(path: &[u16]) -> Option<(Vec<u16>, &[u16])> {
    const BACKSLASH: u16 = b'\\' as u16;
    let at = path.iter().rposition(|&c| c == BACKSLASH)?;
    let (parent, name) = (&path[..at], &path[at + 1..]);
    if name.is_empty() || parent.iter().all(|&c| c == BACKSLASH) {
        return None;
    }
    // `\\server\share` is a root, as `C:\` is.
    if path.starts_with(&[BACKSLASH, BACKSLASH]) && parent[2..].iter().all(|&c| c != BACKSLASH) {
        return None;
    }
    let mut parent = parent.to_vec();
    // `C:` alone is the drive's current folder: its root is `C:\`.
    if parent.last() == Some(&u16::from(b':')) {
        parent.push(BACKSLASH);
    }
    Some((parent, name))
}

/// A folder some items are in: its absolute ID list and the shell's folder.
struct Dir {
    id: Pidl,
    folder: IShellFolder,
}

/// The folders some items are in, each looked up once by path. A folder is
/// parsed by its name in the folder that holds it (itself looked up the same
/// way, down from a root), and an item by its name in its folder: each name
/// costs one lookup in one folder. `ILCreateFromPathW` parses a whole path
/// from the desktop for every item instead, which took about 1 ms an item in
/// a folder some 12 levels deep (3 s for 3,000 rows); this takes some 40 µs.
struct Dirs {
    desktop: IShellFolder,
    known: HashMap<Vec<u16>, Option<Rc<Dir>>>,
}

impl Dirs {
    fn new() -> Option<Dirs> {
        // COM is initialised on this thread (the UI thread's STA).
        let desktop = b7::desktop_folder()?;
        Some(Dirs {
            desktop,
            known: HashMap::new(),
        })
    }

    /// The folder at `path`, if the shell finds it.
    fn get(&mut self, path: &[u16]) -> Option<Rc<Dir>> {
        if let Some(dir) = self.known.get(path) {
            return dir.clone();
        }
        let dir = self.find(path).map(Rc::new);
        self.known.insert(path.to_vec(), dir.clone());
        dir
    }

    fn find(&mut self, path: &[u16]) -> Option<Dir> {
        let Some((parent, name)) = split_parent(path) else {
            let id = id_of_path(path)?;
            // The empty absolute ID list is the desktop.
            let folder = if id.is_empty() {
                self.desktop.clone()
            } else {
                b7::bind_folder(&self.desktop, &id)?
            };
            return Some(Dir { id, folder });
        };
        let parent = self.get(&parent)?;
        let child = parse_name(&parent.folder, name)?;
        // The parent's absolute ID list and the child's relative to it; the
        // combined one is a copy owned by the new value.
        let folder = b7::bind_folder(&parent.folder, &child)?;
        let id = parent.id.join(&child)?;
        Some(Dir { id, folder })
    }
}

/// The ID list of `path`, parsed whole from the desktop.
fn id_of_path(path: &[u16]) -> Option<Pidl> {
    Pidl::of_path(path)
}

/// The ID list, relative to `folder`, of the item `name` in it; `None` when
/// it is not there (gone since the search).
fn parse_name(folder: &IShellFolder, name: &[u16]) -> Option<Pidl> {
    b7::parse_name(folder, name)
}

/// Some results as the shell knows them: the folder that holds them and
/// what it calls each. All in one folder: that folder, with each item's
/// last ID, as Explorer does. Spread over several: the desktop, with each
/// item's absolute ID list (Everything's `single_parent_context_menu=0`),
/// so the menu and the data object cover them all.
pub struct Items {
    folder: IShellFolder,
    /// Each item's ID list (relative to `folder` when one folder holds them
    /// all, else absolute).
    pidls: Vec<Pidl>,
    /// The folder knows each item by its list's last ID (absolute lists of
    /// items it holds), not by its list whole.
    last: bool,
    /// The paths the items are, of those asked for (in order).
    taken: Vec<Vec<u16>>,
    /// One folder holds them all.
    #[cfg_attr(not(test), expect(dead_code, reason = "the tests check it"))]
    one_parent: bool,
}

/// Where one of some items was found: by name in a folder, or (a root)
/// whole.
enum Found {
    In(Rc<Dir>, Pidl),
    Whole(Pidl),
}

impl Items {
    /// The items at `paths` (full paths). Those the shell cannot find (gone
    /// since the search) are left out; `None` when none is left.
    pub fn of_paths(paths: &[Vec<u16>]) -> Option<Items> {
        let mut dirs = Dirs::new()?;
        let (found, taken): (Vec<Found>, Vec<Vec<u16>>) = paths
            .iter()
            .filter_map(|p| {
                let found = match split_parent(p) {
                    Some((parent, name)) => {
                        let dir = dirs.get(&parent)?;
                        let child = parse_name(&dir.folder, name)?;
                        Found::In(dir, child)
                    }
                    None => Found::Whole(id_of_path(p)?),
                };
                Some((found, p.clone()))
            })
            .unzip();
        let first = match found.first()? {
            Found::In(dir, _) => Some(dir),
            Found::Whole(_) => None,
        };
        // All in one folder: that folder, and the names as parsed in it.
        if let Some(dir) = first
            && found
                .iter()
                .all(|f| matches!(f, Found::In(d, _) if Rc::ptr_eq(d, dir)))
        {
            let folder = dir.folder.clone();
            let pidls: Vec<Pidl> = found
                .into_iter()
                .filter_map(|f| match f {
                    Found::In(_, child) => Some(child),
                    Found::Whole(_) => None,
                })
                .collect();
            return Some(Items {
                folder,
                pidls,
                last: false,
                taken,
                one_parent: true,
            });
        }
        let pidls: Option<Vec<Pidl>> = found
            .into_iter()
            .map(|f| match f {
                // The folder's absolute ID list and the item's relative to
                // it; the combined copy is owned by the result.
                Found::In(dir, child) => dir.id.join(&child),
                Found::Whole(id) => Some(id),
            })
            .collect();
        Self::of_absolute(pidls?, taken)
    }

    /// Items given by absolute ID lists (several folders, or a root).
    fn of_absolute(pidls: Vec<Pidl>, taken: Vec<Vec<u16>>) -> Option<Items> {
        let first = pidls.first()?;
        // The first item's parent, a copy freed with `parent`.
        let parent = first.parent()?;
        let one_parent = pidls.iter().skip(1).all(|p| parent.is_parent_of(p, true));
        let folder = if one_parent {
            b7::bind_to_parent(first)?
        } else {
            b7::desktop_folder()?
        };
        Some(Items {
            folder,
            pidls,
            last: one_parent,
            taken,
            one_parent,
        })
    }

    /// How many items the shell found, of the paths asked for.
    pub fn len(&self) -> usize {
        self.taken.len()
    }

    /// The paths of the items, in order: those asked for less the gone.
    pub fn paths(&self) -> &[Vec<u16>] {
        &self.taken
    }

    /// Whether one folder holds them all (not the desktop for several).
    #[cfg(test)]
    pub fn one_parent(&self) -> bool {
        self.one_parent
    }

    /// One of the folder's UI objects for the items (a context menu, a
    /// data object), owned by `owner`.
    fn ui_object<T: Interface>(&self, owner: HWND) -> Option<T> {
        b7::ui_object(&self.folder, owner, &self.pidls, self.last)
    }

    /// The items' data object, as Explorer puts on the clipboard or drags:
    /// CF_HDROP, the Shell IDList Array, file names and the rest.
    pub fn data_object(&self, owner: HWND) -> Option<IDataObject> {
        self.ui_object(owner)
    }

    /// The items' Explorer context menu.
    pub fn context_menu(&self, owner: HWND) -> Option<ContextMenu> {
        let menu: IContextMenu = self.ui_object(owner)?;
        Some(ContextMenu {
            menu: ShellMenu::new(menu),
            first: Cell::new(0),
        })
    }
}

/// What the shell's menu is asked for. `extended`: Shift was down, so the
/// extended verbs come too (Copy as path, Open in new process…).
/// `can_rename`: goz-gui renames in place, so the shell adds Rename.
pub fn query_flags(extended: bool, can_rename: bool) -> u32 {
    let mut flags = CMF_NORMAL | CMF_ITEMMENU;
    if extended {
        flags |= CMF_EXTENDEDVERBS;
    }
    if can_rename {
        flags |= CMF_CANRENAME;
    }
    flags
}

/// The `CMINVOKECOMMANDINFOEX` mask: Unicode, the point the menu was
/// shown at, Ctrl and Shift as they were (Shift+Delete deletes for good),
/// and in a test that asked for it no UI at all.
pub fn invoke_mask(ctrl: bool, shift: bool, silent: bool) -> u32 {
    /// SEE_MASK_UNICODE and SEE_MASK_FLAG_NO_UI, which the CMIC_MASK
    /// names share.
    const UNICODE: u32 = 0x4000;
    const NO_UI: u32 = 0x400;
    let mut mask = UNICODE | CMIC_MASK_PTINVOKE;
    if ctrl {
        mask |= CMIC_MASK_CONTROL_DOWN;
    }
    if shift {
        mask |= CMIC_MASK_SHIFT_DOWN;
    }
    if silent {
        mask |= NO_UI;
    }
    mask
}

/// What a file operation does, for its flags.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FileOp {
    /// Delete: to the Recycle Bin, or (`permanent`, Shift+Delete) for good.
    Delete { permanent: bool },
    /// Copy, move (paste) or rename.
    Transfer,
    /// A copy into the folder that holds the items: each gets a name of its
    /// own ("name - Copy.txt"), as Explorer pastes a copy beside itself.
    Duplicate,
}

/// The `IFileOperation::SetOperationFlags` flags for `op`: undoable as in
/// Explorer (Explorer's Undo brings it back), recycling unless permanent;
/// and with `silent` (a test, `GOZ_GUI_FILEOP_SILENT`) no question, no
/// progress and no error box, so nothing shows on the screen. Without it
/// Windows asks its own questions.
pub fn fileop_flags(op: FileOp, silent: bool) -> u32 {
    let mut flags = match op {
        FileOp::Delete { permanent: false } => {
            FOF_ALLOWUNDO.0 | FOFX_ADDUNDORECORD.0 | FOFX_RECYCLEONDELETE.0
        }
        // Gone for good: nothing to undo.
        FileOp::Delete { permanent: true } => 0,
        FileOp::Transfer => FOF_ALLOWUNDO.0 | FOFX_ADDUNDORECORD.0,
        FileOp::Duplicate => FOF_ALLOWUNDO.0 | FOFX_ADDUNDORECORD.0 | FOF_RENAMEONCOLLISION.0,
    };
    if silent {
        flags |= FOF_NOCONFIRMATION.0 | FOF_SILENT.0 | FOF_NOERRORUI.0 | FOF_NOCONFIRMMKDIR.0;
    }
    flags
}

/// Drags `data` (a selection's data object) out of `owner` with the
/// system's drag image and cursors (SHDoDragDrop's own drop source, which
/// follows either mouse button, so a right-drag gets the target's menu),
/// allowing a copy, a move or a link. Returns once it is dropped or
/// cancelled, with the effect the target reported (0: none, or a move the
/// target made itself: see [`performed_effect`]).
pub fn drag(owner: HWND, data: &IDataObject) -> u32 {
    let allowed = DROPEFFECT_COPY.0 | DROPEFFECT_MOVE.0 | DROPEFFECT_LINK.0;
    // A data object of this thread; the drag's loop pumps messages and the
    // caller holds no borrow across it.
    b7::do_drag_drop(owner, data, allowed)
}

/// The DROPEFFECT a drop target wrote back into `data` once it was done
/// ("Performed DropEffect", or the "Logical Performed DropEffect" of an
/// optimized move, which SHDoDragDrop reports as no effect); 0 if none.
pub fn performed_effect(data: &IDataObject) -> u32 {
    ["Performed DropEffect", "Logical Performed DropEffect"]
        .iter()
        .filter_map(|name| get_effect(data, name))
        .fold(0, |acc, e| acc | e)
}

/// After a drag that ended with `effect` (what SHDoDragDrop returned) and
/// `performed` (the target's "Performed DropEffect"): whether the target
/// copied the items and left their deletion to the source, which is what
/// a move it did not optimize is in the shell's protocol (both say
/// DROPEFFECT_MOVE). A move the target made itself (an optimized one)
/// returns no effect: Explorer's folders do so, though their "Performed
/// DropEffect" says move (measured). A target that says nothing of it
/// leaves the files alone. The shell's data objects drop synchronously
/// (goz-gui never asks for async), so the target is done by then.
pub fn source_deletes(effect: u32, performed: Option<u32>) -> bool {
    effect == DROPEFFECT_MOVE.0 && performed == Some(DROPEFFECT_MOVE.0)
}

/// The "Performed DropEffect" a drop target wrote back into `data`.
pub fn performed(data: &IDataObject) -> Option<u32> {
    get_effect(data, "Performed DropEffect")
}

/// The registered clipboard format `name`.
fn format(name: &str) -> u16 {
    b7::register_clipboard_format(name) as u16
}

/// A DROPEFFECT `data` holds as format `name`.
fn get_effect(data: &IDataObject, name: &str) -> Option<u32> {
    // The medium the object hands out is copied out, then released.
    let bytes = b7::data_get(data, format(name))?;
    Some(u32::from_ne_bytes(bytes.get(..4)?.try_into().ok()?))
}

/// Gives `data` the DROPEFFECT `effect` as format `name`; false if it
/// would not take it.
fn set_effect(data: &IDataObject, name: &str, effect: u32) -> bool {
    // A block of 4 bytes, which the object owns once it takes it.
    b7::data_set(data, format(name), &effect.to_ne_bytes())
}

/// What a paste that moved the files of a cut writes back into its data
/// object, in this order: the report of a move the paste made itself (an
/// optimized move, as the shell's delete-on-paste protocol has it:
/// "Performed DropEffect" none, "Logical Performed DropEffect" a move),
/// then "Paste Succeeded" a move, which tells the program that cut them the
/// paste is over. "Performed DropEffect" = move with it would ask a source
/// that follows the protocol to delete them itself: never so here, as
/// `IFileOperation` moved them. (shell32's own folders say move in both,
/// and its data object deletes what is left on "Paste Succeeded" alone;
/// so the reply waits until nothing is left: [`Cut`].)
const MOVED: [(&str, u32); 3] = [
    ("Performed DropEffect", DROPEFFECT_NONE.0),
    ("Logical Performed DropEffect", DROPEFFECT_MOVE.0),
    ("Paste Succeeded", DROPEFFECT_MOVE.0),
];

/// The data object of a cut on the clipboard, taken when a paste reads its
/// files, to be told once they are moved ([`Cut::moved`]).
///
/// The shell's own file data object (Explorer's Cut and Copy, and the
/// Explorer menu's in goz-gui) deletes, to the Recycle Bin, whichever of
/// its items are still there when it hears "Paste Succeeded" = move, even
/// for a Copy. So the reply goes to this object alone, only while it is
/// still the clipboard's, and only once none of the files is left
/// (`crate::window`): never to whatever the user copied or cut since.
pub struct Cut {
    data: IDataObject,
    /// The clipboard's sequence number when its files were read.
    sequence: u32,
}

impl Cut {
    /// The clipboard's data object, whose files a paste read when the
    /// clipboard's sequence number was `sequence`; `None` when the
    /// clipboard changed since (or OLE is not there). A clipboard put
    /// there without OLE (goz-gui's own Ctrl+X) gives an object that takes
    /// no reply, which is fine: nothing waits on it.
    pub fn of_clipboard(sequence: u32) -> Option<Cut> {
        if !ole_ready() {
            return None;
        }
        // OLE is initialised on this thread.
        let data = b7::ole_get_clipboard()?;
        (clipboard_sequence() == sequence).then_some(Cut { data, sequence })
    }

    /// The clipboard's sequence number its files were read at.
    pub fn sequence(&self) -> u32 {
        self.sequence
    }

    /// The paste moved every file of the cut: tells the program that cut
    /// them, as Explorer does ([`MOVED`]), as long as the clipboard still
    /// holds them; false when it holds something else by now, and then
    /// nothing more is said.
    pub fn moved(&self) -> bool {
        MOVED.iter().all(|&(name, effect)| {
            // Checked before each write: none is sent once the clipboard
            // changed, however late in the paste that was.
            clipboard_sequence() == self.sequence && {
                set_effect(&self.data, name, effect);
                true
            }
        })
    }
}

/// The clipboard's sequence number, which changes with whatever is put on
/// it.
fn clipboard_sequence() -> u32 {
    b7::clipboard_sequence()
}

/// What the shortcut at `path` (a `.lnk`) points to, as a path; `None`
/// when it cannot be read or points to no file or folder.
pub fn link_target(path: &[u16]) -> Option<Vec<u16>> {
    if !ole_ready() {
        return None;
    }
    // COM is initialised on this thread (OLE); the ID list is the link's
    // copy, freed here.
    let link = b7::ShellLink::new()?;
    if !link.load(path) {
        return None;
    }
    link.id_list()?.path()
}

/// The Explorer context menu of some items, as long as it is shown and
/// its command run: the handlers it loaded live with it.
///
/// Dropping it releases the menu (its three interfaces are all the
/// references there are then: measured) and with it every handler. What
/// some handlers leave behind is theirs: Windows' Work Folders handler
/// (`WorkfoldersShell.dll`, registered for `*`) creates one thread per
/// menu in its QueryContextMenu, suspended, and never runs or ends it;
/// Locale Emulator's leaks 4 GDI objects a menu, PowerToys' File Locksmith
/// 1 (each measured alone, made and released through `IShellExtInit` as
/// the menu does). Nothing goz-gui can release frees them, and
/// `CoFreeUnusedLibrariesEx` must not be called for it: it unloads
/// `WorkfoldersShell.dll` under its live threads.
pub struct ContextMenu {
    /// The menu, with the versions that draw their own items (icons, Open
    /// with, Send to) and want the menu's messages for it.
    menu: ShellMenu,
    /// The id of the menu's first item, once [`fill`](Self::fill)ed.
    first: Cell<u32>,
}

impl ContextMenu {
    /// Adds the shell's items to `menu` from position `at`, with ids from
    /// `first` to `last`; false if it added none.
    pub fn fill(&self, menu: HMENU, at: u32, first: u32, last: u32, flags: u32) -> bool {
        self.first.set(first);
        // A menu of this thread; the handlers add to it. Success carries
        // how many ids were used.
        self.menu
            .query(menu, at, first, last, flags)
            .is_some_and(|used| used > 0)
    }

    /// Whether `id` is one of the shell's (from `fill`'s range).
    pub fn owns(&self, id: u32, last: u32) -> bool {
        (self.first.get()..=last).contains(&id) && self.first.get() > 0
    }

    /// The canonical verb of the shell's item `id` (`copy`, `delete`,
    /// `properties`…), if its handler says.
    pub fn verb(&self, id: u32) -> Option<String> {
        let offset = id.checked_sub(self.first.get())? as usize;
        self.menu.verb(offset)
    }

    /// Runs the shell's item `id` for `owner`, as chosen at `pt` (screen)
    /// with the mask [`invoke_mask`] makes. False if the handler failed.
    pub fn invoke(&self, owner: HWND, id: u32, pt: POINT, mask: u32) -> bool {
        let Some(offset) = id.checked_sub(self.first.get()) else {
            return false;
        };
        // The handler may pump messages, and no borrow of the caller's is
        // held.
        self.menu.invoke(owner, offset, pt, mask)
    }

    /// A message of the menu while it is up (WM_INITMENUPOPUP,
    /// WM_MEASUREITEM, WM_DRAWITEM, WM_MENUCHAR), for the handlers that
    /// fill or draw their items; `Some` with the answer when one took it.
    ///
    /// Its parameters must be the message's as the system delivered it
    /// (WM_MEASUREITEM's and WM_DRAWITEM's lParam is a pointer the handler
    /// writes through), which this signature cannot vouch for: so this file
    /// is not yet on `win`. [`ContextMenu::message_of`] takes the message
    /// itself.
    pub fn message(&self, msg: u32, w: usize, l: isize) -> Option<isize> {
        // SAFETY: the message's own parameters, passed on unchanged, while
        // the menu they are about is up (the callers pass on what their
        // window procedure was delivered).
        unsafe { self.menu.message_raw(msg, w, l) }
    }

    /// [`ContextMenu::message`] for a message as the window procedure got
    /// it: WM_MEASUREITEM and WM_DRAWITEM only as the system delivered
    /// them.
    #[cfg_attr(
        not(test),
        expect(dead_code, reason = "the window's procedures are not on win yet")
    )]
    pub fn message_of(&self, m: &Msg) -> Option<isize> {
        self.menu.message(m)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::win::com::Apartment;
    use windows_sys::Win32::UI::WindowsAndMessaging::WM_INITMENUPOPUP;

    fn w(s: &str) -> Vec<u16> {
        s.encode_utf16().collect()
    }

    fn windir() -> String {
        std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".into())
    }

    /// The files of a data object's CF_HDROP.
    fn hdrop_files(data: &IDataObject) -> Vec<String> {
        b7::data_files(data)
            .expect("CF_HDROP")
            .iter()
            .map(|f| String::from_utf16_lossy(f))
            .collect()
    }

    #[test]
    fn a_path_splits_into_its_folder_and_name() {
        let split = |p: &str| {
            split_parent(&w(p)).map(|(parent, name)| {
                (
                    String::from_utf16_lossy(&parent),
                    String::from_utf16_lossy(name),
                )
            })
        };
        let some = |a: &str, b: &str| Some((a.to_owned(), b.to_owned()));
        assert_eq!(split(r"C:\dir\a.txt"), some(r"C:\dir", "a.txt"));
        assert_eq!(split(r"C:\a.txt"), some(r"C:\", "a.txt"));
        assert_eq!(split(r"C:\dir\sub"), some(r"C:\dir", "sub"));
        assert_eq!(split(r"\\srv\share\a.txt"), some(r"\\srv\share", "a.txt"));
        // Roots, which the shell parses whole.
        for root in [r"C:\", "C:", r"\\srv\share", r"\\srv", r"\x", "name"] {
            assert_eq!(split(root), None, "{root}");
        }
    }

    /// A drive, a folder and files of several folders in one go: each found,
    /// as the paths they are.
    #[test]
    fn roots_folders_and_files_mix() {
        // COM for this test thread, while the test runs.
        let _com = Apartment::sta();
        let dir = windir();
        let drive = format!("{}\\", &dir[..2]);
        let paths: Vec<Vec<u16>> = [
            drive.clone(),
            format!(r"{dir}\System32"),
            format!(r"{dir}\System32\notepad.exe"),
            format!(r"{dir}\win.ini"),
        ]
        .iter()
        .map(|p| w(p))
        .collect();
        let items = Items::of_paths(&paths).expect("items");
        assert_eq!(items.paths(), &paths[..]);
        assert!(!items.one_parent());
        let data = items.data_object(core::ptr::null_mut()).expect("data");
        let got: Vec<String> = hdrop_files(&data)
            .iter()
            .map(|s| s.to_lowercase())
            .collect();
        let want: Vec<String> = paths
            .iter()
            .map(|p| String::from_utf16_lossy(p).to_lowercase())
            .collect();
        assert_eq!(got, want);
        // A drive alone: through its parent (This PC), as before.
        let one = Items::of_paths(&[w(&drive)]).expect("drive");
        assert!(one.one_parent());
        let menu = one.context_menu(core::ptr::null_mut()).expect("menu");
        // A popup menu of this thread, destroyed below.
        let popup = b7::create_popup_menu();
        assert!(menu.fill(popup, 0, 0x7000, 0x7fff, query_flags(false, false)));
        b7::destroy_menu(popup);
    }

    #[test]
    fn flags_follow_the_switches() {
        let base = 0x4000 | CMIC_MASK_PTINVOKE;
        assert_eq!(invoke_mask(false, false, false), base);
        assert_eq!(
            invoke_mask(true, true, false),
            base | CMIC_MASK_CONTROL_DOWN | CMIC_MASK_SHIFT_DOWN
        );
        assert_eq!(invoke_mask(false, false, true), base | 0x400);
        assert_eq!(query_flags(false, false), CMF_ITEMMENU);
        assert_eq!(
            query_flags(true, true),
            CMF_ITEMMENU | CMF_EXTENDEDVERBS | CMF_CANRENAME
        );
    }

    #[test]
    fn file_operations_ask_unless_silent() {
        let undo = FOF_ALLOWUNDO.0 | FOFX_ADDUNDORECORD.0;
        let quiet = FOF_NOCONFIRMATION.0 | FOF_SILENT.0 | FOF_NOERRORUI.0 | FOF_NOCONFIRMMKDIR.0;
        let recycle = FileOp::Delete { permanent: false };
        let permanent = FileOp::Delete { permanent: true };
        // Windows' own questions and progress: none of the quiet flags.
        assert_eq!(fileop_flags(recycle, false), undo | FOFX_RECYCLEONDELETE.0);
        assert_eq!(fileop_flags(permanent, false), 0);
        assert_eq!(fileop_flags(FileOp::Transfer, false), undo);
        assert_eq!(
            fileop_flags(FileOp::Duplicate, false),
            undo | FOF_RENAMEONCOLLISION.0
        );
        for op in [recycle, permanent, FileOp::Transfer, FileOp::Duplicate] {
            assert_eq!(fileop_flags(op, false) & quiet, 0, "{op:?} asks");
            assert_eq!(fileop_flags(op, true) & quiet, quiet, "{op:?} silent");
        }
        assert_eq!(fileop_flags(permanent, true) & FOFX_RECYCLEONDELETE.0, 0);
    }

    #[test]
    fn a_move_is_reported_as_one_the_paste_made_itself() {
        // Never "Performed DropEffect" = move: with "Paste Succeeded" =
        // move that asks the program that cut the files to delete them.
        let get = |name: &str| MOVED.iter().find(|m| m.0 == name).map(|m| m.1);
        assert_eq!(get("Performed DropEffect"), Some(DROPEFFECT_NONE.0));
        assert_eq!(get("Logical Performed DropEffect"), Some(DROPEFFECT_MOVE.0));
        assert_eq!(MOVED.last().map(|m| m.0), Some("Paste Succeeded"));
        assert_eq!(get("Paste Succeeded"), Some(DROPEFFECT_MOVE.0));
    }

    #[test]
    fn a_drag_deletes_only_what_the_target_asks_it_to() {
        let (none, copy, mv) = (DROPEFFECT_NONE.0, DROPEFFECT_COPY.0, DROPEFFECT_MOVE.0);
        // A move the target did not optimize: it copied, the source deletes.
        assert!(source_deletes(mv, Some(mv)));
        // An optimized move (the target moved them), a copy, a cancel, or
        // a target that does not say: the files are left alone.
        assert!(!source_deletes(mv, Some(none)));
        assert!(!source_deletes(mv, None));
        // What Explorer's folders report for a move they made (measured on
        // shell32's folder drop target): no effect, "Performed" move.
        assert!(!source_deletes(none, Some(mv)));
        assert!(!source_deletes(copy, Some(copy)));
        assert!(!source_deletes(none, None));
    }

    /// The shell's side of the row menu and of drag and drop, in process,
    /// on files that are only read: items in one folder and in two, their
    /// data object's CF_HDROP, and the context menu's verbs.
    #[test]
    fn items_menus_and_data_objects() {
        // COM for this test thread, while the test runs.
        let _com = Apartment::sta();
        let dir = windir();
        let one = [
            format!(r"{dir}\win.ini"),
            format!(r"{dir}\System32\notepad.exe"),
        ];
        let paths: Vec<Vec<u16>> = one.iter().map(|p| w(p)).collect();
        let two = Items::of_paths(&paths).expect("items in two folders");
        assert_eq!(two.len(), 2);
        assert!(!two.one_parent(), "two folders: the desktop holds them");
        let same: Vec<Vec<u16>> = [r"System32\notepad.exe", r"System32\cmd.exe"]
            .iter()
            .map(|p| w(&format!(r"{dir}\{p}")))
            .collect();
        let items = Items::of_paths(&same).expect("items in one folder");
        assert!(items.one_parent());
        // A path that is gone is left out; none at all is no items.
        let mut with_gone = same.clone();
        with_gone.push(w(&format!(r"{dir}\no-such-file-gozm3.txt")));
        let some = Items::of_paths(&with_gone).expect("two of three");
        assert_eq!(some.len(), 2);
        assert_eq!(some.paths(), &same[..], "the paths the items are");
        assert!(Items::of_paths(&[w(r"C:\no-such-dir-gozm3\x.txt")]).is_none());

        for (items, want) in [(&items, &same), (&two, &paths)] {
            let data = items
                .data_object(core::ptr::null_mut())
                .expect("data object");
            let want: Vec<String> = want.iter().map(|p| String::from_utf16_lossy(p)).collect();
            let got = hdrop_files(&data);
            assert_eq!(
                got.iter().map(|s| s.to_lowercase()).collect::<Vec<_>>(),
                want.iter().map(|s| s.to_lowercase()).collect::<Vec<_>>()
            );
        }

        let menu = items.context_menu(core::ptr::null_mut()).expect("menu");
        // A popup menu of this thread, destroyed below.
        let popup = b7::create_popup_menu();
        assert!(menu.fill(popup, 0, 0x7000, 0x7fff, query_flags(false, false)));
        let n = b7::menu_item_count(popup);
        assert!(n > 3, "the shell added {n} items");
        let verbs: Vec<String> = (0x7000..0x7100).filter_map(|id| menu.verb(id)).collect();
        for verb in ["copy", "properties"] {
            assert!(
                verbs.iter().any(|v| v.eq_ignore_ascii_case(verb)),
                "{verb} in {verbs:?}"
            );
        }
        assert!(menu.owns(0x7000, 0x7fff) && !menu.owns(0x6fff, 0x7fff));
        // Submenus the shell fills as they open (Send to, Open with) fill
        // only from the WM_INITMENUPOPUP the window passes on: the row
        // menu's owner must forward it.
        let mut grew = Vec::new();
        for pos in 0..n {
            // Reads and fills submenus of the menu made above, as the menu
            // loop would when each opens.
            let sub = b7::sub_menu(popup, pos);
            if sub.is_null() {
                continue;
            }
            let before = b7::menu_item_count(sub);
            let opened = Msg::new(
                core::ptr::null_mut(),
                WM_INITMENUPOPUP,
                sub as usize,
                pos as isize,
            );
            menu.message_of(&opened);
            let after = b7::menu_item_count(sub);
            if after > before {
                grew.push((pos, before, after));
            }
        }
        // A CI runner's shell may have no handler that fills a submenu
        // late (no Send to targets, no Open with apps): there the check
        // can only be reported, not required.
        if std::env::var_os("CI").is_some() && grew.is_empty() {
            eprintln!("note: no submenu filled itself on WM_INITMENUPOPUP (CI runner)");
        } else {
            assert!(
                !grew.is_empty(),
                "no submenu filled itself on WM_INITMENUPOPUP"
            );
        }
        // The menu made above.
        b7::destroy_menu(popup);
    }

    /// The names of the formats a data object offers.
    fn format_names(data: &IDataObject) -> Vec<String> {
        b7::data_formats(data)
            .into_iter()
            .map(|cf| b7::clipboard_format_name(u32::from(cf)).unwrap_or_else(|| format!("#{cf}")))
            .collect()
    }

    /// How many items a data object's Shell IDList Array holds (a CIDA,
    /// whose count comes first).
    fn cida_count(data: &IDataObject) -> u32 {
        let cida = b7::data_get(data, format("Shell IDList Array")).expect("Shell IDList Array");
        u32::from_ne_bytes(cida[..4].try_into().expect("a CIDA"))
    }

    /// What a drag of scratch files carries (made in process, never
    /// dragged): CF_HDROP with their paths, the Shell IDList Array with one
    /// entry each, and a drop target's reply written back and read again.
    #[test]
    fn drag_data_of_scratch_files() {
        // COM for this test thread, while the test runs.
        let _com = Apartment::sta();
        let dir = crate::store::file::tests::fresh_dir("goz-gui-drag", "");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("sub")).unwrap();
        let files = [
            dir.join("a.txt"),
            dir.join("b.txt"),
            dir.join("sub").join("c.txt"),
        ];
        for f in &files {
            std::fs::write(f, "x").unwrap();
        }
        let paths: Vec<Vec<u16>> = files.iter().map(|f| w(&f.to_string_lossy())).collect();
        for (picked, one_parent) in [(&paths[..2], true), (&paths[..], false)] {
            let items = Items::of_paths(picked).expect("items");
            assert_eq!(items.one_parent(), one_parent);
            let data = items
                .data_object(core::ptr::null_mut())
                .expect("data object");
            let names = format_names(&data);
            assert!(
                names.iter().any(|n| n == "Shell IDList Array") && names.iter().any(|n| n == "#15"),
                "{names:?}"
            );
            assert_eq!(cida_count(&data) as usize, picked.len());
            let want: Vec<String> = picked.iter().map(|p| String::from_utf16_lossy(p)).collect();
            assert_eq!(hdrop_files(&data), want);
            // A target's "Performed DropEffect" comes back as the drag's.
            assert_eq!(performed_effect(&data), 0);
            assert!(set_effect(&data, "Performed DropEffect", DROPEFFECT_MOVE.0));
            assert_eq!(performed_effect(&data), DROPEFFECT_MOVE.0);
        }
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// A dropped shortcut stands for its target.
    #[test]
    fn shortcuts_resolve_to_their_targets() {
        assert!(ole_ready());
        let dir = crate::store::file::tests::fresh_dir("goz-gui-lnk", "");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let target = dir.join("target folder");
        std::fs::create_dir_all(&target).unwrap();
        let lnk = dir.join("to target.lnk");
        let (t, l) = (w(&target.to_string_lossy()), w(&lnk.to_string_lossy()));
        // COM is ready (OLE).
        let link = b7::ShellLink::new().expect("link");
        assert!(link.set_path(&t), "set path");
        assert!(link.save(&l), "save");
        let got = link_target(&l).map(|p| String::from_utf16_lossy(&p));
        assert_eq!(got.as_deref(), Some(&*target.to_string_lossy()));
        assert_eq!(
            link_target(&w(&dir.join("none.lnk").to_string_lossy())),
            None
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
