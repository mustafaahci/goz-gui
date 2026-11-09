//! The small dialogs Organize Bookmarks asks with: a folder's name (New
//! Folder..., renaming a folder) and a folder to move to (Move To...).

use std::cell::{Cell, RefCell};

use windows_sys::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows_sys::Win32::System::SystemServices::SS_LEFT;
use windows_sys::Win32::UI::Controls::{
    HTREEITEM, NM_DBLCLK, NMHDR, TVM_SETIMAGELIST, TVN_SELCHANGEDW, TVS_HASBUTTONS, TVS_HASLINES,
    TVS_LINESATROOT, TVS_SHOWSELALWAYS, TVSIL_NORMAL,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    BS_DEFPUSHBUTTON, EN_CHANGE, ES_AUTOHSCROLL, EndDialog, GWLP_USERDATA, GetWindowTextLengthW,
    IDCANCEL, IDOK, SetWindowLongPtrW, WM_COMMAND, WM_INITDIALOG, WM_NOTIFY, WS_EX_CLIENTEDGE,
    WS_TABSTOP,
};

use super::{Icons, enable, fill_tree, item, send, set_text, tree_select, tree_selected, warn};
use crate::bookmarks::FolderPath;
use crate::bookmarks::tree::Tree;
use crate::dialog::{Class, Look, Template, apply_theme, center_on_owner, common, state};
use crate::wide::window_text;

const STATIC: u16 = 0xffff;
const ID_NAME: u16 = 100;
const ID_TREE: u16 = 101;
const OK: u16 = IDOK as u16;
const CANCEL: u16 = IDCANCEL as u16;

/// What is wrong with a name, if anything.
pub type Problem<'a> = &'a dyn Fn(&str) -> Option<String>;

struct Name<'a> {
    initial: String,
    problem: Problem<'a>,
    look: Cell<Look>,
    result: RefCell<Option<String>>,
}

/// Asks for a folder's name, starting from `initial`; OK refuses a name
/// `problem` finds fault with, saying why.
pub fn folder_name(
    owner: HWND,
    title: &str,
    initial: &str,
    problem: Problem<'_>,
    look: Look,
) -> Option<String> {
    let state = Name {
        initial: initial.to_owned(),
        problem,
        look: Cell::new(look),
        result: RefCell::new(None),
    };
    let mut t = Template::new(title, 0, 220, 50);
    t.control(Class::Static, "&Name:", STATIC, SS_LEFT, 0, (7, 9, 30, 8))
        .control(
            Class::Edit,
            "",
            ID_NAME,
            WS_TABSTOP | ES_AUTOHSCROLL as u32,
            WS_EX_CLIENTEDGE,
            (40, 7, 173, 14),
        )
        .control(
            Class::Button,
            "OK",
            OK,
            WS_TABSTOP | BS_DEFPUSHBUTTON as u32,
            0,
            (109, 29, 50, 14),
        )
        .control(
            Class::Button,
            "Cancel",
            CANCEL,
            WS_TABSTOP,
            0,
            (163, 29, 50, 14),
        );
    t.run(owner, Some(name_proc), (&raw const state) as LPARAM);
    state.result.into_inner()
}

unsafe extern "system" fn name_proc(dlg: HWND, msg: u32, w: WPARAM, l: LPARAM) -> isize {
    if msg == WM_INITDIALOG {
        // SAFETY: `l` is the `Name` that `folder_name` keeps alive until
        // the dialog has ended.
        unsafe { SetWindowLongPtrW(dlg, GWLP_USERDATA, l) };
    }
    // SAFETY: GWLP_USERDATA is the `Name` from `folder_name`, or null.
    let Some(s) = (unsafe { state::<Name<'_>>(dlg) }) else {
        return 0;
    };
    if let Some(answer) = common(dlg, msg, w, l, &s.look) {
        return answer;
    }
    match msg {
        WM_INITDIALOG => {
            set_text(item(dlg, ID_NAME), &s.initial);
            enable(dlg, OK, !s.initial.is_empty());
            apply_theme(dlg, s.look.get().dark);
            center_on_owner(dlg);
            // The keyboard in the name, its text selected.
            1
        }
        WM_COMMAND => {
            let (id, code) = ((w & 0xffff) as u16, ((w >> 16) & 0xffff) as u32);
            match id {
                OK => {
                    let name = window_text(item(dlg, ID_NAME)).trim().to_owned();
                    if let Some(why) = (s.problem)(&name) {
                        warn(dlg, &why);
                        return 1;
                    }
                    *s.result.borrow_mut() = Some(name);
                    // SAFETY: ends this modal dialog.
                    unsafe { EndDialog(dlg, 1) };
                }
                // SAFETY: ends this modal dialog.
                CANCEL => unsafe {
                    EndDialog(dlg, 0);
                },
                ID_NAME if code == EN_CHANGE => {
                    // SAFETY: reads a control of this thread's dialog.
                    let named = unsafe { GetWindowTextLengthW(item(dlg, ID_NAME)) } > 0;
                    enable(dlg, OK, named);
                }
                _ => return 0,
            }
            1
        }
        _ => 0,
    }
}

struct Pick<'a> {
    tree: &'a Tree,
    skip: &'a [FolderPath],
    select: usize,
    icons: Icons,
    look: Cell<Look>,
    made: RefCell<Vec<(usize, HTREEITEM)>>,
    result: RefCell<Option<FolderPath>>,
}

/// Asks which folder of `tree` to move to, `select`'s chosen first; the
/// folders `skip` (those being moved) and theirs are not offered.
pub fn pick_folder(
    owner: HWND,
    tree: &Tree,
    skip: &[FolderPath],
    select: usize,
    look: Look,
) -> Option<FolderPath> {
    let state = Pick {
        tree,
        skip,
        select,
        icons: super::icons(),
        look: Cell::new(look),
        made: RefCell::new(Vec::new()),
        result: RefCell::new(None),
    };
    let view = WS_TABSTOP | TVS_HASBUTTONS | TVS_HASLINES | TVS_LINESATROOT | TVS_SHOWSELALWAYS;
    let mut t = Template::new("Move To Folder", 0, 200, 190);
    t.control(
        Class::Static,
        "&Move to:",
        STATIC,
        SS_LEFT,
        0,
        (7, 7, 186, 8),
    )
    .control(
        Class::Named("SysTreeView32"),
        "",
        ID_TREE,
        view,
        WS_EX_CLIENTEDGE,
        (7, 18, 186, 145),
    )
    .control(
        Class::Button,
        "OK",
        OK,
        WS_TABSTOP | BS_DEFPUSHBUTTON as u32,
        0,
        (89, 169, 50, 14),
    )
    .control(
        Class::Button,
        "Cancel",
        CANCEL,
        WS_TABSTOP,
        0,
        (143, 169, 50, 14),
    );
    t.run(owner, Some(pick_proc), (&raw const state) as LPARAM);
    state.result.into_inner()
}

impl Pick<'_> {
    fn choose(&self, dlg: HWND) {
        let Some(node) = tree_selected(item(dlg, ID_TREE)) else {
            return;
        };
        *self.result.borrow_mut() = Some(self.tree.nodes[node].path.clone());
        // SAFETY: ends this modal dialog.
        unsafe { EndDialog(dlg, 1) };
    }
}

unsafe extern "system" fn pick_proc(dlg: HWND, msg: u32, w: WPARAM, l: LPARAM) -> isize {
    if msg == WM_INITDIALOG {
        // SAFETY: `l` is the `Pick` that `pick_folder` keeps alive until
        // the dialog has ended.
        unsafe { SetWindowLongPtrW(dlg, GWLP_USERDATA, l) };
    }
    // SAFETY: GWLP_USERDATA is the `Pick` from `pick_folder`, or null.
    let Some(s) = (unsafe { state::<Pick<'_>>(dlg) }) else {
        return 0;
    };
    if let Some(answer) = common(dlg, msg, w, l, &s.look) {
        return answer;
    }
    match msg {
        WM_INITDIALOG => {
            let view = item(dlg, ID_TREE);
            send(view, TVM_SETIMAGELIST, TVSIL_NORMAL as usize, s.icons.list);
            let made = fill_tree(view, s.tree, s.skip, s.icons);
            tree_select(view, &made, s.select);
            *s.made.borrow_mut() = made;
            apply_theme(dlg, s.look.get().dark);
            center_on_owner(dlg);
            1
        }
        WM_NOTIFY => {
            // SAFETY: WM_NOTIFY's lParam is an NMHDR.
            let hdr = unsafe { &*(l as *const NMHDR) };
            if hdr.idFrom == usize::from(ID_TREE) {
                match hdr.code {
                    TVN_SELCHANGEDW => {
                        let picked = tree_selected(item(dlg, ID_TREE)).is_some();
                        enable(dlg, OK, picked);
                    }
                    NM_DBLCLK => s.choose(dlg),
                    _ => {}
                }
            }
            0
        }
        WM_COMMAND => {
            match (w & 0xffff) as u16 {
                OK => s.choose(dlg),
                // SAFETY: ends this modal dialog.
                CANCEL => unsafe {
                    EndDialog(dlg, 0);
                },
                _ => return 0,
            }
            1
        }
        _ => 0,
    }
}
