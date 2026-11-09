//! The bookmark dialogs:
//!
//! - the bookmark editor (Bookmarks > Add to Bookmarks..., and New... and
//!   Edit... in Organize): a name, a search, and the options goz-gui has;
//! - Find Bookmark... (Ctrl+B): pick one from a filtered list;
//! - Organize Bookmarks... (Ctrl+Shift+B, Ctrl+Shift+O): the folders as a
//!   tree, what the chosen one holds, and the changes, each made at once;
//! - the small ones Organize asks with: a folder's name, a folder to move
//!   to.
//!
//! The dialogs read the bookmarks from, and change them through, a [`Host`]
//! (the window), which writes the file.

use std::sync::Arc;

mod edit;
mod find;
mod organize;
mod pick;

pub use edit::edit;
pub use find::{Chosen, find};
pub use organize::organize;

use windows_sys::Win32::Foundation::{HWND, LPARAM};
use windows_sys::Win32::Storage::FileSystem::{FILE_ATTRIBUTE_DIRECTORY, FILE_ATTRIBUTE_NORMAL};
use windows_sys::Win32::UI::Controls::{
    HIMAGELIST, HTREEITEM, TVE_EXPAND, TVGN_CARET, TVI_LAST, TVI_ROOT, TVIF_IMAGE, TVIF_PARAM,
    TVIF_SELECTEDIMAGE, TVIF_TEXT, TVINSERTSTRUCTW, TVINSERTSTRUCTW_0, TVITEMW, TVM_DELETEITEM,
    TVM_ENSUREVISIBLE, TVM_EXPAND, TVM_GETITEMW, TVM_GETNEXTITEM, TVM_INSERTITEMW, TVM_SELECTITEM,
};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::EnableWindow;
use windows_sys::Win32::UI::Shell::{
    SHFILEINFOW, SHGFI_SMALLICON, SHGFI_SYSICONINDEX, SHGFI_USEFILEATTRIBUTES, SHGetFileInfoW,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    GetDlgItem, IDYES, MB_DEFBUTTON2, MB_ICONINFORMATION, MB_ICONWARNING, MB_OK, MB_YESNO,
    MessageBoxW, SendMessageW, SetWindowTextW, WM_APP,
};

use crate::bookmarks::ops::Op;
use crate::bookmarks::tree::{Item, TOP, Tree};
use crate::bookmarks::{Bookmarks, FolderPath, Settings, within};
use crate::wide::wz;

/// Posted to an open Find or Organize dialog when the bookmarks changed
/// (another goz-gui's, read before a write): it shows them again.
pub const WM_REFRESH: u32 = WM_APP + 60;

/// What the dialogs ask of the window that keeps the bookmarks.
pub trait Host {
    /// The bookmarks now (shared, not copied), and how many times they
    /// have changed.
    fn bookmarks(&self) -> (Arc<Bookmarks>, u64);
    /// Makes a change; the file is written at once. False when it changed
    /// nothing.
    fn change(&self, op: Op) -> bool;
    /// Puts `text` on the clipboard; false when the clipboard was busy.
    fn copy(&self, text: &str) -> bool;
    /// The options a new bookmark starts with: the window's, where the
    /// `bookmark_remember_*` settings keep them.
    fn new_settings(&self) -> Settings;
    /// `dlg` shows the bookmarks (null: it closed), to be told of changes
    /// with [`WM_REFRESH`].
    fn showing(&self, dlg: HWND);
}

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

fn set_text(hwnd: HWND, text: &str) {
    // SAFETY: a NUL-terminated string for a window of this thread.
    unsafe { SetWindowTextW(hwnd, wz(text).as_ptr()) };
}

fn message(owner: HWND, title: &str, text: &str, style: u32) -> i32 {
    // SAFETY: NUL-terminated strings for a modal box owned by `owner`.
    unsafe { MessageBoxW(owner, wz(text).as_ptr(), wz(title).as_ptr(), style) }
}

/// Tells the user something went wrong with the bookmarks.
pub fn warn(owner: HWND, text: &str) {
    message(owner, "Bookmarks", text, MB_OK | MB_ICONWARNING);
}

fn inform(owner: HWND, title: &str, text: &str) {
    message(owner, title, text, MB_OK | MB_ICONINFORMATION);
}

/// Asks before something that cannot be undone; No is the default.
fn ask(owner: HWND, title: &str, text: &str) -> bool {
    message(
        owner,
        title,
        text,
        MB_YESNO | MB_ICONWARNING | MB_DEFBUTTON2,
    ) == IDYES
}

/// The shell's small icons the lists and trees show: a folder's, and a
/// plain file's for a bookmark (Everything's bookmark icon is a page too).
#[derive(Clone, Copy)]
struct Icons {
    /// The system's small image list (shared: never destroyed here).
    list: HIMAGELIST,
    folder: i32,
    bookmark: i32,
}

fn icons() -> Icons {
    let lookup = |name: &str, attributes: u32| {
        // SAFETY: an attribute-only lookup into a plain out-struct.
        unsafe {
            let mut info: SHFILEINFOW = core::mem::zeroed();
            let list = SHGetFileInfoW(
                wz(name).as_ptr(),
                attributes,
                &mut info,
                size_of::<SHFILEINFOW>() as u32,
                SHGFI_SYSICONINDEX | SHGFI_SMALLICON | SHGFI_USEFILEATTRIBUTES,
            );
            (list as HIMAGELIST, info.iIcon)
        }
    };
    let (list, folder) = lookup("folder", FILE_ATTRIBUTE_DIRECTORY);
    let (_, bookmark) = lookup("bookmark", FILE_ATTRIBUTE_NORMAL);
    Icons {
        list,
        folder,
        bookmark,
    }
}

/// The name the top of the folder tree shows, as Everything's.
const TOP_NAME: &str = "Bookmarks";

/// Fills a tree view with the folders of `tree`, all open: the top as
/// "Bookmarks", each item's lParam its node. The folders `skip` (and what
/// is in them) are left out. Returns each node's item.
fn fill_tree(
    view: HWND,
    tree: &Tree,
    skip: &[FolderPath],
    icons: Icons,
) -> Vec<(usize, HTREEITEM)> {
    send(view, TVM_DELETEITEM, 0, TVI_ROOT);
    let mut made = Vec::new();
    let insert = |parent: HTREEITEM, text: &str, node: usize| -> HTREEITEM {
        let mut text = wz(text);
        let ins = TVINSERTSTRUCTW {
            hParent: parent,
            hInsertAfter: TVI_LAST,
            Anonymous: TVINSERTSTRUCTW_0 {
                item: TVITEMW {
                    mask: TVIF_TEXT | TVIF_PARAM | TVIF_IMAGE | TVIF_SELECTEDIMAGE,
                    pszText: text.as_mut_ptr(),
                    iImage: icons.folder,
                    iSelectedImage: icons.folder,
                    lParam: node as LPARAM,
                    // SAFETY: the rest of TVITEMW is plain data.
                    ..unsafe { core::mem::zeroed() }
                },
            },
        };
        send(view, TVM_INSERTITEMW, 0, (&raw const ins) as isize) as HTREEITEM
    };
    let top = insert(TVI_ROOT, TOP_NAME, TOP);
    made.push((TOP, top));
    // Each folder after its parent, depth first: a parent is made first.
    for (f, _) in tree.folders_below(TOP) {
        let path = &tree.nodes[f].path;
        if skip.iter().any(|s| within(path, s)) {
            continue;
        }
        let parent_path = &path[..path.len() - 1];
        let parent = tree.find(parent_path).unwrap_or(TOP);
        let Some(&(_, at)) = made.iter().find(|(n, _)| *n == parent) else {
            continue;
        };
        let h = insert(at, tree.nodes[f].name(), f);
        made.push((f, h));
    }
    for &(_, h) in &made {
        send(view, TVM_EXPAND, TVE_EXPAND as usize, h);
    }
    made
}

/// Selects `node`'s item in a tree view `fill_tree` filled.
fn tree_select(view: HWND, made: &[(usize, HTREEITEM)], node: usize) {
    if let Some(&(_, h)) = made.iter().find(|(n, _)| *n == node) {
        send(view, TVM_SELECTITEM, TVGN_CARET as usize, h);
        send(view, TVM_ENSUREVISIBLE, 0, h);
    }
}

/// The node of the tree view's selected item.
fn tree_selected(view: HWND) -> Option<usize> {
    let h = send(view, TVM_GETNEXTITEM, TVGN_CARET as usize, 0);
    if h == 0 {
        return None;
    }
    let mut it = TVITEMW {
        mask: TVIF_PARAM,
        hItem: h as HTREEITEM,
        // SAFETY: the rest of TVITEMW is plain data.
        ..unsafe { core::mem::zeroed() }
    };
    send(view, TVM_GETITEMW, 0, (&raw mut it) as isize);
    usize::try_from(it.lParam).ok()
}

/// A name for a new folder in `node` that none there has: `New Folder`,
/// else `New Folder (2)`, … (Everything's).
fn unique_name(tree: &Tree, node: usize, base: &str) -> String {
    let taken = |name: &str| sibling_named(tree, node, name).is_some();
    if !taken(base) {
        return base.to_owned();
    }
    (2..)
        .map(|n| format!("{base} ({n})"))
        .find(|name| !taken(name))
        .unwrap_or_default()
}

/// The folder of `node` named `name` (regardless of case).
fn sibling_named(tree: &Tree, node: usize, name: &str) -> Option<usize> {
    tree.nodes[node].items.iter().find_map(|&i| match i {
        Item::Folder(f) if crate::bookmarks::same_name(tree.nodes[f].name(), name) => Some(f),
        _ => None,
    })
}

/// Why `name` cannot be a folder's name in `node` (for a rename, `own` is
/// the folder renamed): `None` when it can.
fn folder_name_problem(tree: &Tree, node: usize, name: &str, own: Option<usize>) -> Option<String> {
    let name = name.trim();
    if name.is_empty() {
        return Some("A folder needs a name.".into());
    }
    if name.contains('/') {
        return Some("A folder name cannot contain /.".into());
    }
    match sibling_named(tree, node, name) {
        Some(f) if Some(f) != own => {
            Some("The specified bookmark folder name already exists.".into())
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bookmarks::tree::tests::list;

    #[test]
    fn new_folder_names_and_their_problems() {
        let l = list(&[
            ("f", "", "New Folder"),
            ("f", "", "New Folder (2)"),
            ("f", "New Folder", "Inner"),
        ]);
        let t = Tree::build(&l);
        assert_eq!(unique_name(&t, TOP, "New Folder"), "New Folder (3)");
        let inner = t.find(&["New Folder".to_owned()]).expect("folder");
        assert_eq!(unique_name(&t, inner, "New Folder"), "New Folder");
        assert_eq!(
            folder_name_problem(&t, TOP, "new folder", None).as_deref(),
            Some("The specified bookmark folder name already exists.")
        );
        let own = t.find(&["New Folder".to_owned()]);
        assert_eq!(folder_name_problem(&t, TOP, "NEW FOLDER", own), None);
        assert!(folder_name_problem(&t, TOP, "a/b", None).is_some());
        assert!(folder_name_problem(&t, TOP, "  ", None).is_some());
        assert_eq!(folder_name_problem(&t, TOP, "Films", None), None);
    }
}
