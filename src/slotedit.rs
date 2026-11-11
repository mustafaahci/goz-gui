//! W7: Everything's Edit File List Slot dialog ("Edit File List N"): the
//! names a file-list slot holds, edited. It opens on a file-list term of the
//! search box (any of Everything's slot functions, `filelist1:`,
//! `parent-filelist1:` and the rest) in Everything's three ways: the
//! search box's right-click menu ("Edit File List Slot..."), Ctrl+click on
//! the term, or F2 with the caret in it. On a `filelist:` or
//! `child-filelist:` term with names of its own the menu says "Edit File
//! List..." and the dialog ("Edit File List") edits those names, written
//! back into the search on OK.
//!
//! The list is one column, Filename; the buttons are Everything's words:
//! New (a new name, typed in place), Add... (files, from Windows' Open
//! dialog), Add Folder..., Edit (F2: rename in place), Remove (Del),
//! Remove All, Paste (Ctrl+V: files copied in Explorer, or text, a name a
//! line), Import... (the names of an EFU, CSV or TXT list, read on a thread
//! of its own) and Export... (CSV, EFU or TXT, Everything's three types).
//! Files and folders dropped on it are added as they are. OK puts the names
//! in the slot (or the search); Cancel leaves it as it was.
//!
//! Everything's keys: the dialog's place (`file_list_slot_edit_x`, `_y`,
//! `_wide`, `_high`, `_maximized`, `file_list_slot_edit_filename_wide`, see
//! `places`), Export...'s type and file (`file_list_slot_export_type`,
//! `file_list_slot_last_export_filename`), Add Folder...'s folder
//! (`filelist_slot_last_add_folder_path`) and Import...'s file
//! (`filelist_slot_last_import_filename`). A test names Import...'s and
//! Export...'s file in `GOZ_GUI_EDITOR_FILE` (its extension the type) and
//! Add Folder...'s folder in `GOZ_GUI_PICK_FOLDER`, as for the File List
//! Editor.
//!
//! A modal dialog of the window's thread; no `RefCell` of the window is
//! borrowed while it runs.

use std::cell::{Cell, RefCell};
use std::collections::HashSet;

use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows_sys::Win32::Graphics::Gdi::HDC;
use windows_sys::Win32::System::SystemServices::{SS_ENDELLIPSIS, SS_LEFT, SS_NOPREFIX};
use windows_sys::Win32::UI::Controls::{
    LVCF_TEXT, LVCF_WIDTH, LVCOLUMNW, LVIF_TEXT, LVIS_FOCUSED, LVIS_SELECTED, LVM_EDITLABELW,
    LVM_ENSUREVISIBLE, LVM_GETNEXTITEM, LVM_GETSELECTEDCOUNT, LVM_INSERTCOLUMNW,
    LVM_SETEXTENDEDLISTVIEWSTYLE, LVM_SETITEMCOUNT, LVN_BEGINLABELEDITW, LVN_ENDLABELEDITW,
    LVN_GETDISPINFOW, LVN_ITEMCHANGED, LVN_KEYDOWN, LVN_ODFINDITEMW, LVN_ODSTATECHANGED,
    LVNI_FOCUSED, LVNI_SELECTED, LVS_EDITLABELS, LVS_EX_DOUBLEBUFFER, LVS_EX_FULLROWSELECT,
    LVS_EX_LABELTIP, LVS_NOSORTHEADER, LVS_OWNERDATA, LVS_REPORT, LVS_SHOWSELALWAYS, NM_CUSTOMDRAW,
    NM_DBLCLK, NMHDR, NMLVCUSTOMDRAW, NMLVDISPINFOW, NMLVKEYDOWN,
};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    GetKeyState, SetFocus, VK_CONTROL, VK_DELETE, VK_F2, VK_INSERT,
};
use windows_sys::Win32::UI::Shell::{
    DefSubclassProc, DragAcceptFiles, DragFinish, HDROP, RemoveWindowSubclass, SetWindowSubclass,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    BS_DEFPUSHBUTTON, DWLP_MSGRESULT, EndDialog, GWLP_USERDATA, GetClientRect, GetParent,
    GetWindowRect, IDCANCEL, IDOK, MB_ICONWARNING, MB_OK, MINMAXINFO, MoveWindow, PostMessageW,
    SetWindowLongPtrW, SetWindowTextW, WM_APP, WM_COMMAND, WM_CTLCOLOREDIT, WM_DESTROY,
    WM_DROPFILES, WM_GETMINMAXINFO, WM_INITDIALOG, WM_NCDESTROY, WM_NOTIFY, WM_SIZE,
    WS_EX_CLIENTEDGE, WS_MAXIMIZEBOX, WS_TABSTOP, WS_THICKFRAME,
};

use crate::dialog::{self, Class, Look, Template, common, list_custom_draw, state};
use crate::filelist::settings::{SLOT_EXPORT_KINDS, SlotKept};
use crate::filetools::ui::{copy_text, item, send, set_state, units};
use crate::listedit::scan::{Job, Source};
use crate::listedit::{Entry, Export, items_text};
use crate::places::{self, Which};
use crate::store::location;
use crate::wide::wz;

/// W7's commands, 4950..=4999.
pub mod cmd {
    pub const FIRST: u16 = 4950;
    /// The search box menu's "Edit File List Slot..." (a slot's term).
    pub const EDIT_SLOT: u16 = FIRST;
    /// The search box menu's "Edit File List..." (a term's own names).
    pub const EDIT_LIST: u16 = FIRST + 1;
    /// The File List Editor's Edit > Paste (Ctrl+V).
    pub const EDITOR_PASTE: u16 = FIRST + 2;
    /// One past W7's range.
    pub const END: u16 = 5000;

    const _: () = {
        assert!(crate::filelist::slots::cmd::END <= FIRST);
        assert!(EDITOR_PASTE < END);
        assert!((END as usize) <= crate::menu::SHELL_FIRST as usize);
    };
}

/// W7's WM_APP messages, `WM_APP + 380..390`.
pub const MSG_FIRST: u32 = WM_APP + 380;
/// The main window's: open the editor on the term at the search box's
/// UTF-16 position `wParam` (Ctrl+click and F2 post it, so the dialog runs
/// outside the box's own message).
pub const WM_EDIT_TERM_AT: u32 = MSG_FIRST;
/// The main window's: a `save-filelistN:` search's results were read
/// (lParam: a `Box<window::lists::Saved>`).
pub const WM_SLOT_SAVED: u32 = MSG_FIRST + 1;
/// The dialog's own: Import...'s job has names, or ended.
const WM_IMPORTED: u32 = MSG_FIRST + 2;
/// One past W7's messages.
pub const MSG_END: u32 = MSG_FIRST + 10;

const _: () = assert!(WM_IMPORTED < MSG_END && crate::listedit::W6_MSG_END <= MSG_FIRST);

const ID_LIST: u16 = 100;
const ID_COUNT: u16 = 101;
const ID_NEW: u16 = 110;
const ID_ADD: u16 = 111;
const ID_ADD_FOLDER: u16 = 112;
const ID_EDIT: u16 = 113;
const ID_REMOVE: u16 = 114;
const ID_REMOVE_ALL: u16 = 115;
const ID_PASTE: u16 = 116;
const ID_IMPORT: u16 = 117;
const ID_EXPORT: u16 = 118;
const OK: u16 = IDOK as u16;
const CANCEL: u16 = IDCANCEL as u16;

/// The buttons down the right, Everything's words, each a row of 18 units.
const BUTTONS: [(u16, &str); 9] = [
    (ID_NEW, "&New"),
    (ID_ADD, "&Add..."),
    (ID_ADD_FOLDER, "Add &Folder..."),
    (ID_EDIT, "Edi&t"),
    (ID_REMOVE, "&Remove"),
    (ID_REMOVE_ALL, "Remove A&ll"),
    (ID_PASTE, "&Paste"),
    (ID_IMPORT, "&Import..."),
    (ID_EXPORT, "&Export..."),
];

/// The template's size, in dialog units.
const SIZE: (i16, i16) = (300, 200);
/// The list's subclass: its rename box in the dialog's colours.
const LIST_SUBCLASS: usize = 0x534c;

/// Export...'s file types, Everything's three (`file_list_slot_export_type`
/// counts them from 0), then All Files.
const EXPORT_KINDS: [crate::filedlg::Kind; SLOT_EXPORT_KINDS + 1] = [
    ("CSV Comma Separated Values (*.csv)", "*.csv"),
    ("EFU Everything File List (*.efu)", "*.efu"),
    ("TXT Text Files (*.txt)", "*.txt"),
    ("All Files (*.*)", "*.*"),
];
const EXPORTS: [Export; SLOT_EXPORT_KINDS] = [Export::Csv, Export::Efu, Export::Txt];

/// How the dialog starts.
pub struct Start {
    /// "Edit File List 1", or "Edit File List" for a term's own names.
    pub title: String,
    pub names: Vec<String>,
    pub look: Look,
    pub kept: SlotKept,
}

/// How it ended: the names on OK (`None`: cancelled), and its choices kept
/// either way.
pub struct Outcome {
    pub names: Option<Vec<String>>,
    pub kept: SlotKept,
}

/// Shows the dialog over `owner` until it closes.
pub fn edit(owner: HWND, start: Start) -> Outcome {
    let state = Dlg {
        look: Cell::new(start.look),
        title: start.title.clone(),
        names: RefCell::new(start.names),
        kept: RefCell::new(start.kept),
        result: RefCell::new(None),
        min: Cell::new((0, 0)),
        job: RefCell::new(None),
        fresh: Cell::new(None),
    };
    let (cx, cy) = SIZE;
    let mut t = Template::new(&start.title, WS_THICKFRAME | WS_MAXIMIZEBOX, cx, cy);
    t.control(
        Class::Named("SysListView32"),
        "",
        ID_LIST,
        WS_TABSTOP
            | LVS_REPORT
            | LVS_OWNERDATA
            | LVS_SHOWSELALWAYS
            | LVS_EDITLABELS
            | LVS_NOSORTHEADER,
        WS_EX_CLIENTEDGE,
        (7, 7, cx - 74, cy - 35),
    );
    for (i, (id, text)) in BUTTONS.into_iter().enumerate() {
        t.control(
            Class::Button,
            text,
            id,
            WS_TABSTOP,
            0,
            (cx - 61, 7 + 18 * i as i16, 54, 14),
        );
    }
    t.control(
        Class::Static,
        "",
        ID_COUNT,
        SS_LEFT | SS_NOPREFIX | SS_ENDELLIPSIS,
        0,
        (7, cy - 18, cx - 140, 10),
    )
    .control(
        Class::Button,
        "OK",
        OK,
        WS_TABSTOP | BS_DEFPUSHBUTTON as u32,
        0,
        (cx - 119, cy - 21, 54, 14),
    )
    .control(
        Class::Button,
        "Cancel",
        CANCEL,
        WS_TABSTOP,
        0,
        (cx - 61, cy - 21, 54, 14),
    );
    t.run(owner, Some(dialog_proc), (&raw const state) as LPARAM);
    Outcome {
        names: state.result.into_inner(),
        kept: state.kept.into_inner(),
    }
}

struct Dlg {
    look: Cell<Look>,
    title: String,
    names: RefCell<Vec<String>>,
    kept: RefCell<SlotKept>,
    result: RefCell<Option<Vec<String>>>,
    min: Cell<(i32, i32)>,
    /// Import... reading its list.
    job: RefCell<Option<Job>>,
    /// The row New made, while its name is typed.
    fresh: Cell<Option<usize>>,
}

fn set_text(hwnd: HWND, text: &str) {
    let z = wz(text);
    // SAFETY: NUL-terminated text for a window of this thread.
    unsafe { SetWindowTextW(hwnd, z.as_ptr()) };
}

fn beep() {
    // SAFETY: plays a system sound.
    unsafe { windows_sys::Win32::System::Diagnostics::Debug::MessageBeep(MB_OK) };
}

fn warn(dlg: HWND, title: &str, text: &str) {
    crate::msgbox::show(dlg, text, title, MB_OK | MB_ICONWARNING);
}

/// Adds `new` to `names`, each once (any case) and not one already there;
/// how many were added.
pub fn add_unique(names: &mut Vec<String>, new: impl IntoIterator<Item = String>) -> usize {
    let mut have: HashSet<String> = names.iter().map(|n| n.to_lowercase()).collect();
    let before = names.len();
    for n in new {
        if let Some(n) = crate::filelist::slots::normalize(&n)
            && have.insert(n.to_lowercase())
        {
            names.push(n);
        }
    }
    names.len() - before
}

/// Takes out the rows `rows` (any order); how many went.
pub fn remove_rows(names: &mut Vec<String>, rows: &[usize]) -> usize {
    let gone: HashSet<usize> = rows.iter().copied().collect();
    let before = names.len();
    let mut i = 0;
    names.retain(|_| {
        let keep = !gone.contains(&i);
        i += 1;
        keep
    });
    before - names.len()
}

/// The names as Export... writes them, in type `kind` (0 CSV, 1 EFU, 2
/// TXT): CSV and EFU as Everything's file lists (no sizes or dates), TXT a
/// name a line.
pub fn export_bytes(names: &[String], kind: usize) -> Vec<u8> {
    let entries: Vec<Entry> = names
        .iter()
        .map(|n| Entry {
            path: n.clone(),
            ..Entry::default()
        })
        .collect();
    EXPORTS[kind.min(SLOT_EXPORT_KINDS - 1)].bytes(&entries)
}

impl Dlg {
    fn list(dlg: HWND) -> HWND {
        item(dlg, ID_LIST)
    }

    fn init(&self, dlg: HWND) {
        let list = Dlg::list(dlg);
        let ex = LVS_EX_FULLROWSELECT | LVS_EX_DOUBLEBUFFER | LVS_EX_LABELTIP;
        send(list, LVM_SETEXTENDEDLISTVIEWSTYLE, ex as usize, ex as isize);
        let (list_w, _) = units(dlg, i32::from(SIZE.0) - 74, 0);
        let mut text = wz("Filename");
        let col = LVCOLUMNW {
            mask: LVCF_TEXT | LVCF_WIDTH,
            cx: list_w - units(dlg, 12, 0).0,
            pszText: text.as_mut_ptr(),
            // SAFETY: the rest of LVCOLUMNW is plain data.
            ..unsafe { core::mem::zeroed() }
        };
        send(list, LVM_INSERTCOLUMNW, 0, (&raw const col) as isize);
        let dark = self.look.get().dark;
        dialog::theme_new(dlg, dark);
        // SAFETY: subclasses this thread's list (the subclass goes with it)
        // and lets files be dropped on the dialog.
        unsafe {
            SetWindowSubclass(list, Some(list_proc), LIST_SUBCLASS, 0);
            DragAcceptFiles(dlg, 1);
        }
        dialog::center_on_owner(dlg);
        let mut wr = RECT::default();
        // SAFETY: reads this thread's dialog.
        unsafe { GetWindowRect(dlg, &mut wr) };
        self.min.set((wr.right - wr.left, wr.bottom - wr.top));
        places::restore(dlg, Which::FileListSlotEdit, list, self.min.get());
        self.layout(dlg);
        self.show_rows(dlg, Some(0));
    }

    /// The list takes the room, the buttons at the right, the count and OK
    /// and Cancel under it.
    fn layout(&self, dlg: HWND) {
        let mut rc = RECT::default();
        // SAFETY: reads this thread's dialog.
        unsafe { GetClientRect(dlg, &mut rc) };
        let (w, h) = (rc.right, rc.bottom);
        let (m, my) = units(dlg, 7, 7);
        let (bw, bh) = units(dlg, 54, 14);
        let (gap, row) = units(dlg, 6, 18);
        let (_, line) = units(dlg, 0, 10);
        let place = |id: u16, x: i32, y: i32, cw: i32, ch: i32| {
            let hwnd = item(dlg, id);
            if !hwnd.is_null() {
                // SAFETY: moves a control of this thread's dialog.
                unsafe { MoveWindow(hwnd, x, y, cw.max(0), ch.max(0), 1) };
            }
        };
        let bx = w - m - bw;
        let bottom = h - my - bh;
        place(ID_LIST, m, my, bx - gap - m, bottom - my - my);
        for (i, (id, _)) in BUTTONS.into_iter().enumerate() {
            place(id, bx, my + row * i as i32, bw, bh);
        }
        place(
            ID_COUNT,
            m,
            bottom + (bh - line) / 2,
            bx - 2 * gap - bw - m,
            line,
        );
        place(OK, bx - gap - bw, bottom, bw, bh);
        place(CANCEL, bx, bottom, bw, bh);
    }

    /// The list shows the names as they are now, `select` (a row) selected
    /// and in view.
    fn show_rows(&self, dlg: HWND, select: Option<usize>) {
        let n = self.names.borrow().len();
        let list = Dlg::list(dlg);
        send(list, LVM_SETITEMCOUNT, n, 0);
        set_state(list, usize::MAX, 0, LVIS_SELECTED | LVIS_FOCUSED);
        if let Some(row) = select.filter(|_| n > 0).map(|r| r.min(n - 1)) {
            set_state(
                list,
                row,
                LVIS_SELECTED | LVIS_FOCUSED,
                LVIS_SELECTED | LVIS_FOCUSED,
            );
            send(list, LVM_ENSUREVISIBLE, row, 0);
        }
        // SAFETY: repaints this thread's list.
        unsafe {
            windows_sys::Win32::Graphics::Gdi::InvalidateRect(list, core::ptr::null(), 1);
        }
        self.show_count(dlg);
    }

    fn show_count(&self, dlg: HWND) {
        let n = self.names.borrow().len();
        let selected = send(Dlg::list(dlg), LVM_GETSELECTEDCOUNT, 0, 0).max(0) as usize;
        let text = if self.job.borrow().is_some() {
            format!("Importing... {}", items_text(n))
        } else if selected > 1 {
            format!(
                "{} ({} selected)",
                items_text(n),
                crate::format::grouped(selected as u64)
            )
        } else {
            items_text(n)
        };
        set_text(item(dlg, ID_COUNT), &text);
    }

    /// The selected rows, in order.
    fn selected(dlg: HWND) -> Vec<usize> {
        let list = Dlg::list(dlg);
        let mut out = Vec::new();
        let mut at = usize::MAX;
        while let Ok(next) =
            usize::try_from(send(list, LVM_GETNEXTITEM, at, LVNI_SELECTED as isize))
        {
            out.push(next);
            at = next;
        }
        out
    }

    /// `names` added (each once); the first added selected.
    fn add(&self, dlg: HWND, names: Vec<String>) {
        let at = self.names.borrow().len();
        let added = add_unique(&mut self.names.borrow_mut(), names);
        if added > 0 {
            self.show_rows(dlg, Some(at));
        }
    }

    fn new_name(&self, dlg: HWND) {
        let row = {
            let mut names = self.names.borrow_mut();
            names.push(String::new());
            names.len() - 1
        };
        self.fresh.set(Some(row));
        self.show_rows(dlg, Some(row));
        let list = Dlg::list(dlg);
        // SAFETY: the keyboard to this thread's list, whose row is edited.
        unsafe { SetFocus(list) };
        send(list, LVM_EDITLABELW, row, 0);
    }

    fn edit_name(&self, dlg: HWND) {
        let list = Dlg::list(dlg);
        let Ok(row) = usize::try_from(send(
            list,
            LVM_GETNEXTITEM,
            usize::MAX,
            LVNI_FOCUSED as isize,
        )) else {
            return;
        };
        // SAFETY: the keyboard to this thread's list, whose row is edited.
        unsafe { SetFocus(list) };
        send(list, LVM_EDITLABELW, row, 0);
    }

    /// The rename box closed: `text` (`None`: Esc) is the row's name; a
    /// New row left empty goes.
    fn renamed(&self, dlg: HWND, row: usize, text: Option<String>) {
        let fresh = self.fresh.take() == Some(row);
        let name = text.as_deref().and_then(crate::filelist::slots::normalize);
        let mut names = self.names.borrow_mut();
        match name {
            Some(n) if row < names.len() => names[row] = n,
            _ if fresh && row < names.len() => {
                names.remove(row);
            }
            _ => return,
        }
        drop(names);
        self.show_rows(dlg, Some(row));
    }

    fn remove(&self, dlg: HWND) {
        let rows = Dlg::selected(dlg);
        if rows.is_empty() {
            return;
        }
        remove_rows(&mut self.names.borrow_mut(), &rows);
        self.show_rows(dlg, Some(rows[0]));
    }

    fn remove_all(&self, dlg: HWND) {
        self.names.borrow_mut().clear();
        self.show_rows(dlg, None);
    }

    fn add_files(&self, dlg: HWND) {
        let files = crate::listedit::ui::pick_files(dlg);
        self.add(dlg, files);
    }

    fn add_folder(&self, dlg: HWND) {
        let start: Vec<u16> = self.kept.borrow().last_add_folder.encode_utf16().collect();
        let start = (!start.is_empty()).then_some(start.as_slice());
        let Some(folder) = crate::filetools::folder_pick::pick(dlg, "Add Folder", "Add", start)
        else {
            return;
        };
        let folder = String::from_utf16_lossy(&folder);
        self.kept.borrow_mut().last_add_folder = folder.clone();
        self.add(dlg, vec![folder]);
    }

    /// Paste (Ctrl+V): files copied in Explorer, else text, a name a line.
    fn paste(&self, dlg: HWND) {
        // A test's names (`GOZ_GUI_PASTE_TEXT`) stand for the clipboard.
        let test = location::paste_text();
        let files = if test.is_some() {
            None
        } else {
            crate::clipboard::files(dlg)
        };
        let names = match files {
            Some(clip) => clip
                .paths
                .iter()
                .map(|p| String::from_utf16_lossy(p))
                .collect(),
            None => test
                .or_else(|| crate::clipboard::text(dlg))
                .map(|t| crate::listedit::pasted_names(&t))
                .unwrap_or_default(),
        };
        if names.is_empty() {
            beep();
            return;
        }
        self.add(dlg, names);
    }

    fn dropped(&self, dlg: HWND, drop: HDROP) {
        // SAFETY: the HDROP WM_DROPFILES brought, read whole, then freed.
        let paths = unsafe {
            let paths = crate::clipboard::dropped_files(drop);
            DragFinish(drop);
            paths
        };
        self.add(
            dlg,
            paths.iter().map(|p| String::from_utf16_lossy(p)).collect(),
        );
    }

    /// Import...: the names of a list file (EFU, CSV, a text file of
    /// paths), read on a thread of its own.
    fn import(&self, dlg: HWND) {
        if self.job.borrow().is_some() {
            beep();
            return;
        }
        let path = match location::editor_file() {
            Some(p) => Some(p),
            None => crate::filedlg::open(
                dlg,
                "Import File List",
                crate::listedit::IMPORT_DIALOG,
                "efu",
            ),
        };
        let Some(path) = path else { return };
        self.kept.borrow_mut().last_import = path.display().to_string();
        match Job::start(vec![Source::List(path.clone())], dlg as usize, WM_IMPORTED) {
            Ok(job) => {
                *self.job.borrow_mut() = Some(job);
                self.show_count(dlg);
            }
            Err(e) => warn(
                dlg,
                "Import File List",
                &format!("{} could not be imported: {e}", path.display()),
            ),
        }
    }

    /// Import...'s job has names, or ended.
    fn imported(&self, dlg: HWND) {
        let found = match self.job.borrow().as_ref() {
            Some(job) => job.take(),
            None => return,
        };
        if !found.entries.is_empty() {
            self.add(dlg, found.entries.into_iter().map(|e| e.path).collect());
        }
        if !found.done {
            self.show_count(dlg);
            return;
        }
        self.job.take();
        self.show_count(dlg);
        if let Some(why) = found.error {
            let file = self.kept.borrow().last_import.clone();
            warn(
                dlg,
                "Import File List",
                &format!("The file list could not be imported:\n\n{file}\n\n{why}"),
            );
        }
    }

    /// Export...: the names written as a CSV, an EFU or a text file.
    fn export(&self, dlg: HWND) {
        let last = self.kept.borrow().export_type;
        let chosen = match location::editor_file() {
            Some(p) => {
                let kind = match Export::of_path(&p) {
                    Some(Export::Csv) => 0,
                    Some(Export::Efu) => 1,
                    Some(Export::Txt) => 2,
                    _ => last,
                };
                Some((p, kind))
            }
            None => {
                let name = {
                    let k = self.kept.borrow();
                    if k.last_export.is_empty() {
                        self.title.clone()
                    } else {
                        k.last_export.clone()
                    }
                };
                crate::filedlg::save_kind(dlg, "Export File List", &name, &EXPORT_KINDS, last).map(
                    |(p, kind)| {
                        let kind = if kind < SLOT_EXPORT_KINDS {
                            kind
                        } else {
                            match Export::of_path(&p) {
                                Some(Export::Efu) => 1,
                                Some(Export::Txt) => 2,
                                Some(Export::Csv) => 0,
                                _ => last,
                            }
                        };
                        (p, kind)
                    },
                )
            }
        };
        let Some((path, kind)) = chosen else { return };
        {
            let mut k = self.kept.borrow_mut();
            k.export_type = kind;
            k.last_export = path.display().to_string();
        }
        let bytes = export_bytes(&self.names.borrow(), kind);
        // A dialog's export is tried once: it never waits on a held file.
        if let Err(e) = crate::store::file::write_once(&path, &bytes) {
            warn(
                dlg,
                "Export File List",
                &format!(
                    "The file list could not be exported to {}: {e}",
                    path.display()
                ),
            );
        } else {
            set_text(
                item(dlg, ID_COUNT),
                &format!(
                    "Exported {} to {}",
                    items_text(self.names.borrow().len()),
                    path.file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_default()
                ),
            );
        }
    }

    fn close(&self, dlg: HWND, ok: bool) {
        if let Some(job) = self.job.take() {
            job.stop();
        }
        if ok {
            let names: Vec<String> = self
                .names
                .borrow()
                .iter()
                .filter_map(|n| crate::filelist::slots::normalize(n))
                .collect();
            *self.result.borrow_mut() = Some(names);
        }
        // SAFETY: ends this modal dialog.
        unsafe { EndDialog(dlg, isize::from(ok)) };
    }

    fn command(&self, dlg: HWND, id: u16) -> bool {
        match id {
            OK => self.close(dlg, true),
            CANCEL => self.close(dlg, false),
            ID_NEW => self.new_name(dlg),
            ID_ADD => self.add_files(dlg),
            ID_ADD_FOLDER => self.add_folder(dlg),
            ID_EDIT => self.edit_name(dlg),
            ID_REMOVE => self.remove(dlg),
            ID_REMOVE_ALL => self.remove_all(dlg),
            ID_PASTE => self.paste(dlg),
            ID_IMPORT => self.import(dlg),
            ID_EXPORT => self.export(dlg),
            _ => return false,
        }
        true
    }

    fn key(&self, dlg: HWND, vk: u16) {
        // SAFETY: reads this thread's keyboard state.
        let ctrl = unsafe { GetKeyState(i32::from(VK_CONTROL)) } < 0;
        match (ctrl, vk) {
            (false, VK_DELETE) => self.remove(dlg),
            (false, VK_F2) => self.edit_name(dlg),
            (false, VK_INSERT) => self.new_name(dlg),
            (true, k) if k == u16::from(b'V') => self.paste(dlg),
            (true, k) if k == u16::from(b'A') => {
                set_state(Dlg::list(dlg), usize::MAX, LVIS_SELECTED, LVIS_SELECTED);
                self.show_count(dlg);
            }
            _ => {}
        }
    }

    /// The list's notifications; `Some` is DWLP_MSGRESULT's value.
    ///
    /// # Safety
    /// `l` is a WM_NOTIFY's lParam from the list.
    unsafe fn notify(&self, dlg: HWND, l: LPARAM) -> Option<isize> {
        // SAFETY: as the caller promises; each code's struct is read as the
        // list sends it.
        unsafe {
            let hdr = &*(l as *const NMHDR);
            match hdr.code {
                NM_CUSTOMDRAW => {
                    let cd = &*(l as *const NMLVCUSTOMDRAW);
                    return Some(list_custom_draw(hdr.hwndFrom, cd, self.look.get().dark));
                }
                LVN_GETDISPINFOW => {
                    let info = &mut *(l as *mut NMLVDISPINFOW);
                    if info.item.mask & LVIF_TEXT != 0 {
                        let text: Vec<u16> = usize::try_from(info.item.iItem)
                            .ok()
                            .and_then(|r| self.names.borrow().get(r).cloned())
                            .map(|n| n.encode_utf16().collect())
                            .unwrap_or_default();
                        copy_text(&text, info.item.pszText, info.item.cchTextMax);
                    }
                }
                LVN_ODFINDITEMW => return Some(-1),
                // The rename box may open (its colours: `list_proc`).
                LVN_BEGINLABELEDITW => return Some(0),
                LVN_ENDLABELEDITW => {
                    let info = &*(l as *const NMLVDISPINFOW);
                    let text = (!info.item.pszText.is_null()).then(|| {
                        let p = info.item.pszText;
                        let mut n = 0;
                        while *p.add(n) != 0 {
                            n += 1;
                        }
                        String::from_utf16_lossy(core::slice::from_raw_parts(p, n))
                    });
                    if let Ok(row) = usize::try_from(info.item.iItem) {
                        self.renamed(dlg, row, text);
                    }
                    // The list keeps no text of its own (owner data).
                    return Some(0);
                }
                NM_DBLCLK => self.edit_name(dlg),
                LVN_KEYDOWN => self.key(dlg, (*(l as *const NMLVKEYDOWN)).wVKey),
                LVN_ITEMCHANGED | LVN_ODSTATECHANGED => self.show_count(dlg),
                _ => {}
            }
            None
        }
    }
}

/// The list's rename box takes the dialog's colours (it is the list's
/// child, so the list is asked for them).
unsafe extern "system" fn list_proc(
    hwnd: HWND,
    msg: u32,
    w: WPARAM,
    l: LPARAM,
    _id: usize,
    _data: usize,
) -> LRESULT {
    // SAFETY: the messages are the list's own, their parameters as Windows
    // sends them; the dialog's state is read as its procedure keeps it.
    unsafe {
        match msg {
            WM_CTLCOLOREDIT => {
                let dark = state::<Dlg>(GetParent(hwnd)).is_some_and(|s| s.look.get().dark);
                if let Some(brush) = dialog::ctl_color(msg, w as HDC, dark) {
                    return brush;
                }
            }
            WM_NCDESTROY => {
                RemoveWindowSubclass(hwnd, Some(list_proc), LIST_SUBCLASS);
            }
            _ => {}
        }
        DefSubclassProc(hwnd, msg, w, l)
    }
}

unsafe extern "system" fn dialog_proc(dlg: HWND, msg: u32, w: WPARAM, l: LPARAM) -> isize {
    if msg == WM_INITDIALOG {
        // SAFETY: `l` is the `Dlg` that `edit` keeps alive until the dialog
        // has ended.
        unsafe { SetWindowLongPtrW(dlg, GWLP_USERDATA, l) };
    }
    // SAFETY: GWLP_USERDATA is the `Dlg` from `edit`, or null before.
    let Some(s) = (unsafe { state::<Dlg>(dlg) }) else {
        return 0;
    };
    if let Some(answer) = common(dlg, msg, w, l, &s.look) {
        return answer;
    }
    match msg {
        WM_INITDIALOG => {
            s.init(dlg);
            // SAFETY: the keyboard in the list, a control of this dialog.
            unsafe { SetFocus(Dlg::list(dlg)) };
            0
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
            // SAFETY: WM_NOTIFY's lParam is an NMHDR.
            let from_list = unsafe { (*(l as *const NMHDR)).idFrom } == usize::from(ID_LIST);
            if !from_list {
                return 0;
            }
            // SAFETY: a notification from the list.
            if let Some(answer) = unsafe { s.notify(dlg, l) } {
                // SAFETY: the answer of this dialog's WM_NOTIFY.
                unsafe { SetWindowLongPtrW(dlg, DWLP_MSGRESULT as i32, answer) };
            }
            1
        }
        WM_COMMAND => isize::from(s.command(dlg, (w & 0xffff) as u16)),
        WM_DROPFILES => {
            s.dropped(dlg, w as HDROP);
            1
        }
        WM_IMPORTED => {
            s.imported(dlg);
            1
        }
        WM_DESTROY => {
            places::keep(dlg, Which::FileListSlotEdit, Dlg::list(dlg));
            if let Some(job) = s.job.take() {
                job.stop();
            }
            0
        }
        _ => 0,
    }
}

/// Posts the main window W7's "edit the term at `unit`" (Ctrl+click, F2).
pub fn post_edit_at(main: HWND, unit: usize) {
    // SAFETY: a plain post to this thread's window.
    unsafe { PostMessageW(main, WM_EDIT_TERM_AT, unit, 0) };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_added_once_and_removed() {
        let mut names = vec![r"C:\a.txt".to_owned()];
        let added = add_unique(
            &mut names,
            [
                r"c:\A.TXT".to_owned(),
                " C:/b/ ".to_owned(),
                String::new(),
                r"C:\b".to_owned(),
                "name.doc".to_owned(),
            ],
        );
        assert_eq!(added, 2);
        assert_eq!(names, [r"C:\a.txt", r"C:\b", "name.doc"]);
        assert_eq!(remove_rows(&mut names, &[2, 0, 7]), 2);
        assert_eq!(names, [r"C:\b"]);
    }

    #[test]
    fn export_is_everythings_three_types() {
        let names = vec![r"C:\x\a, b.txt".to_owned(), "c.doc".to_owned()];
        let text = |k| String::from_utf8(export_bytes(&names, k)).unwrap();
        assert_eq!(
            text(0),
            "Filename,Size,Date Modified,Date Created,Attributes\r\n\
             \"C:\\x\\a, b.txt\",,,,\r\n\"c.doc\",,,,\r\n"
        );
        assert_eq!(text(1), text(0));
        assert_eq!(text(2), "C:\\x\\a, b.txt\r\nc.doc\r\n");
        assert_eq!(text(9), text(2), "out of range: the last");
        assert_eq!(EXPORT_KINDS.len(), SLOT_EXPORT_KINDS + 1);
    }
}
