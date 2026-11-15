//! The File List Editor's window (Tools > File List Editor...): a dialog
//! made in memory, run on a thread of its own (so it is a window of its
//! own beside the main one, on the taskbar, and neither waits for the
//! other), with Everything's menus:
//!
//! - **File:** New (Ctrl+N), Open... (Ctrl+O), Save (Ctrl+S), Save As...
//!   (Ctrl+Shift+S), Close (Ctrl+W). New, Open and Close ask first whether
//!   to save a changed list.
//! - **Edit:** Add Files..., Add Folder..., Add Folder and Subfolders...,
//!   Remove (Del), Remove Duplicates, Select All (Ctrl+A), Stop Adding
//!   (Esc).
//!
//! The list is virtual: a million rows cost the list view nothing. Adding
//! and opening run on a thread of their own (`scan`) and fill the list as
//! they go, the status line counting; saving writes on a thread of its own
//! too, to a temporary file renamed over the list in one step. One editor
//! is open at a time; Tools > File List Editor... brings it to the front.
//! When goz-gui's window closes, the editor is asked to close too (it asks
//! about unsaved changes), and goz-gui ends once it has.
//!
//! A test names the file Open... and Save As... use in
//! `GOZ_GUI_EDITOR_FILE` (then "Save changes?" is answered No), and the
//! folder Add Folder... adds in `GOZ_GUI_PICK_FOLDER`.
//!
//! Wave 6:
//! - **File:** Import... (a list's rows added to this one: EFU, CSV, TXT)
//!   and Export... (CSV, EFU, M3U, M3U8, TSV or TXT, Everything's types,
//!   with "Export selection only"; `editor_last_export_type` and
//!   `editor_last_export_selection_only` keep the choice). A test's
//!   `GOZ_GUI_EDITOR_FILE` names their file too (its extension the type,
//!   `.tsv` for TSV), with the selection only when there is one.
//! - **Edit:** Update (Everything's "Rescan root folders in the file
//!   list": each folder no other listed folder holds is walked again, with
//!   its subfolders when the list held any, in place of what it held) and
//!   Read Extended Information (the selected rows' size, dates and
//!   attributes read from the disk again).
//! - `-edit <file>` opens the editor with that list (handed to the editor
//!   open, which asks about unsaved changes first), `-filelist-editor` the
//!   editor alone.
//! - The editor opens where it was last (Everything's `editor_x`, `_y`,
//!   `_wide`, `_high`, `_maximized`, goz-gui's `editor_dpi`), handed to it
//!   as it opens and handed back to goz-gui's window as it moves and
//!   closes ([`take_kept`]).
//! - View > Theme in goz-gui's window is followed ([`set_look`]).
//!
//! Wave 7: Everything's Extension, Type (the shell's name, asked once per
//! extension on this thread) and Attributes columns, and Edit > Paste
//! (Ctrl+V): files copied in Explorer as a drop adds them, or text, a name a
//! line (`GOZ_GUI_PASTE_TEXT` in a test).

use std::cell::{Cell, RefCell};
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread::JoinHandle;
use std::time::Duration;

use windows::Win32::UI::Shell::{
    FOS_ALLOWMULTISELECT, FOS_FILEMUSTEXIST, FOS_FORCEFILESYSTEM, FOS_NOCHANGEDIR,
};
use windows_sys::Win32::Foundation::{HWND, RECT};
use windows_sys::Win32::Graphics::Gdi::MONITOR_DEFAULTTONULL;
use windows_sys::Win32::System::SystemServices::{SS_ENDELLIPSIS, SS_LEFT, SS_NOPREFIX};
use windows_sys::Win32::UI::Controls::{
    HDF_SORTDOWN, HDF_SORTUP, HDI_FORMAT, HDITEMW, LVCF_FMT, LVCF_WIDTH, LVCFMT_LEFT, LVCFMT_RIGHT,
    LVCOLUMNW, LVIF_TEXT, LVIS_FOCUSED, LVIS_SELECTED, LVN_COLUMNCLICK, LVN_GETDISPINFOW,
    LVN_KEYDOWN, LVN_ODFINDITEMW, LVNI_SELECTED, LVS_EX_DOUBLEBUFFER, LVS_EX_FULLROWSELECT,
    LVS_EX_HEADERDRAGDROP, LVS_EX_LABELTIP, LVS_OWNERDATA, LVS_REPORT, LVS_SHOWSELALWAYS,
    LVSICF_NOINVALIDATEALL, LVSICF_NOSCROLL, NM_CUSTOMDRAW, NMLISTVIEW, NMLVCUSTOMDRAW,
    NMLVDISPINFOW, NMLVKEYDOWN,
};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{VK_CONTROL, VK_DELETE, VK_SHIFT};
use windows_sys::Win32::UI::WindowsAndMessaging::WM_DROPFILES;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    HMENU, IDCANCEL, IDNO, IDOK, IDYES, MB_ICONQUESTION, MB_ICONWARNING, MB_OK, MB_YESNOCANCEL,
    SIZE_MAXIMIZED, SIZE_RESTORED, SW_RESTORE, SWP_NOACTIVATE, SWP_NOSIZE, SWP_NOZORDER, WM_CLOSE,
    WM_COMMAND, WM_DESTROY, WM_EXITSIZEMOVE, WM_GETMINMAXINFO, WM_INITDIALOG, WM_INITMENUPOPUP,
    WM_NCACTIVATE, WM_NCPAINT, WM_NOTIFY, WM_SETTINGCHANGE, WM_SIZE, WS_EX_CLIENTEDGE,
    WS_MAXIMIZEBOX, WS_MINIMIZEBOX, WS_TABSTOP, WS_THICKFRAME,
};

use super::scan::{Job, Saving, Source};
use super::{
    COLUMNS, Column, EXPORT_DIALOG, Entry, Export, IMPORT_DIALOG, WM_EDITOR_CLOSED, WM_EDITOR_LOOK,
    WM_EDITOR_OPEN, WM_FOUND, WM_SAVED, cmd, items_text,
};
use crate::dialog::{self, Class, Look, Quiet, Template, common, list_custom_draw};
use crate::filetools::ui::{item, set_state, units};
use crate::menu::{self, Item};
use crate::msgbox;
use crate::paint;
use crate::places::{self, Place, Which};
use crate::store::location;
use crate::theme::Palette;
use crate::win::com::Apartment;
use crate::win::shell::FileDialog;
use crate::win::{self, Msg, View, dlg, header, lv, subclass, sys};

const TITLE: &str = "File List Editor";
const ID_LIST: u16 = 100;
const ID_STATUS: u16 = 101;
/// The window's first size, in dialog units.
const SIZE: (i16, i16) = (520, 300);
/// The subclass that paints the menu bar dark.
static FRAME_SUBCLASS: subclass::Key = subclass::Key::new("file list editor frame");
/// How long closing waits for a save still writing.
const SAVE_WAIT: Duration = Duration::from_secs(30);
/// How long closing waits for adding or opening to stop.
const STOP_WAIT: Duration = Duration::from_secs(5);

/// The file types File > Open... lists, and Save As... writes.
const KINDS: &[crate::filedlg::Kind] = &[
    ("Everything File List (*.efu)", "*.efu"),
    ("All Files (*.*)", "*.*"),
];

/// The editor that is open: its window (0 until it is made, and once it is
/// gone) and its thread.
struct Running {
    hwnd: Arc<AtomicUsize>,
    thread: JoinHandle<()>,
}

static EDITOR: Mutex<Option<Running>> = Mutex::new(None);

fn editor() -> MutexGuard<'static, Option<Running>> {
    EDITOR.lock().unwrap_or_else(|e| e.into_inner())
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// W6: what the editor hands back to goz-gui's window: its place, and
/// Export...'s last choices.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Kept {
    pub place: Option<Place>,
    pub export_type: usize,
    pub export_selection: bool,
}

/// The editor's [`Kept`], as it last moved, was sized or closed.
static KEPT: Mutex<Option<Kept>> = Mutex::new(None);
/// The list `-edit` hands to the editor open ([`WM_EDITOR_OPEN`]).
static PENDING: Mutex<Option<PathBuf>> = Mutex::new(None);

/// What the editor handed back since this was last asked (it hands its
/// place back each time it moves, is sized or closes), if anything: taken,
/// so a change made since in goz-gui's settings (Options > Advanced) is
/// not put back over by an old one.
pub fn take_kept() -> Option<Kept> {
    lock(&KEPT).take()
}

/// How the editor starts.
pub struct Start {
    pub look: Look,
    /// The main window's rectangle: a first editor opens centred over it.
    pub near: RECT,
    /// `-edit`'s list, opened at once.
    pub file: Option<PathBuf>,
    /// Where it was last (Everything's `editor_*`).
    pub place: Place,
    pub export_type: usize,
    pub export_selection: bool,
    /// goz-gui's window, told as the editor closes.
    pub owner: usize,
}

/// The editor's window, while it is open.
fn open_window() -> Option<HWND> {
    let slot = editor();
    let r = slot.as_ref().filter(|r| !r.thread.is_finished())?;
    let h = r.hwnd.load(Ordering::Acquire) as HWND;
    win::is_window(h).then_some(h)
}

/// Tools > File List Editor... (and W6's `-edit`, `-filelist-editor`):
/// opens the editor as `start` says; one already open comes to the front
/// instead, and opens `start.file` if one is given (asking about unsaved
/// changes first).
pub fn open(start: Start) {
    let mut slot = editor();
    if let Some(r) = slot.as_ref()
        && !r.thread.is_finished()
    {
        let h = r.hwnd.load(Ordering::Acquire) as HWND;
        // Shows and raises the editor's window, and posts it the list to
        // open; a window gone meanwhile is refused.
        if win::is_window(h) {
            if win::is_iconic(h) {
                win::show(h, SW_RESTORE);
            }
            win::set_foreground(h);
            if let Some(file) = start.file {
                *lock(&PENDING) = Some(file);
                win::post_app(h, WM_EDITOR_OPEN, 0, 0);
            }
        }
        return;
    }
    if let Some(done) = slot.take() {
        let _ = done.thread.join();
    }
    let hwnd = Arc::new(AtomicUsize::new(0));
    let theirs = Arc::clone(&hwnd);
    let thread = std::thread::Builder::new()
        .name("goz-list-editor".into())
        .spawn(move || run(start, theirs));
    if let Ok(thread) = thread {
        *slot = Some(Running { hwnd, thread });
    }
}

/// W6: goz-gui's window changed View > Theme: the editor open follows.
pub fn set_look(look: Look) {
    if let Some(h) = open_window() {
        // A window gone meanwhile drops it.
        win::post_app(
            h,
            WM_EDITOR_LOOK,
            usize::from(look.dark),
            isize::from(look.follow_system),
        );
    }
}

/// Whether the editor is open.
pub fn is_open() -> bool {
    open_window().is_some()
}

/// goz-gui's window has closed: the editor is asked to close (it asks
/// about unsaved changes, and may stay open), and this waits until it has.
pub fn finish() {
    let running = editor().take();
    if let Some(r) = running {
        let h = r.hwnd.load(Ordering::Acquire);
        if h != 0 {
            // A window gone meanwhile drops it.
            win::post_close(h as HWND);
        }
        let _ = r.thread.join();
    }
}

/// The editor's thread: COM for the file and folder dialogs, then the
/// window until it closes.
fn run(start: Start, hwnd: Arc<AtomicUsize>) {
    let Start {
        look,
        near,
        file,
        place,
        export_type,
        export_selection,
        owner,
    } = start;
    // This thread's own COM, given back as it ends.
    let com = Apartment::sta();
    // Message boxes on this thread take the window's look.
    dialog::set_app_look(look);
    // Opened by a command posted while goz-gui was not in front (a test),
    // it does not take the foreground.
    let quiet = Quiet::begin();
    let state = Editor {
        look: Cell::new(look),
        near,
        place,
        owner,
        start_file: RefCell::new(file),
        export_type: Cell::new(export_type),
        export_selection: Cell::new(export_selection),
        doing: Cell::new(Doing::Adding),
        importing: RefCell::new(None),
        hwnd: Arc::clone(&hwnd),
        entries: RefCell::new(Vec::new()),
        file: RefCell::new(None),
        changes: Cell::new(0),
        saved: Cell::new(0),
        sort: Cell::new(None),
        job: RefCell::new(None),
        opening: RefCell::new(None),
        saving: RefCell::new(None),
        after: Cell::new(None),
        min: Cell::new((0, 0)),
    };
    let mut t = Template::new(
        TITLE,
        WS_THICKFRAME | WS_MAXIMIZEBOX | WS_MINIMIZEBOX,
        SIZE.0,
        SIZE.1,
    );
    t.control(
        Class::Named("SysListView32"),
        "",
        ID_LIST,
        WS_TABSTOP | LVS_REPORT | LVS_OWNERDATA | LVS_SHOWSELALWAYS,
        WS_EX_CLIENTEDGE,
        (0, 0, SIZE.0, SIZE.1 - 12),
    )
    .control(
        Class::Static,
        "",
        ID_STATUS,
        SS_LEFT | SS_NOPREFIX | SS_ENDELLIPSIS,
        0,
        (4, SIZE.1 - 10, SIZE.0 - 8, 8),
    );
    t.show(core::ptr::null_mut(), &|m| dialog_proc(&state, m));
    drop(quiet);
    hwnd.store(0, Ordering::Release);
    if let Some(job) = state.job.take() {
        job.stop();
    }
    drop(com);
}

/// What waits for a save to finish: the command the "Save changes?"
/// question was asked for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum After {
    New,
    Open,
    Close,
    /// W6: the list `-edit` handed over ([`PENDING`]).
    OpenPending,
}

/// What a job does, as the status line says it (W6).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Doing {
    Adding,
    Opening,
    Importing,
    Updating,
    Reading,
    /// W7: Edit > Paste.
    Pasting,
}

impl Doing {
    fn verb(self) -> &'static str {
        match self {
            Doing::Adding => "Adding",
            Doing::Opening => "Opening",
            Doing::Importing => "Importing",
            Doing::Updating => "Updating",
            Doing::Reading => "Reading extended information",
            Doing::Pasting => "Pasting",
        }
    }
}

struct Editor {
    look: Cell<Look>,
    near: RECT,
    /// W6: where it opens (its place kept), and whose window it tells as
    /// it closes.
    place: Place,
    owner: usize,
    /// `-edit`'s list, opened as the window starts.
    start_file: RefCell<Option<PathBuf>>,
    /// Export...'s last type and "Export selection only".
    export_type: Cell<usize>,
    export_selection: Cell<bool>,
    /// What the job running does, and the file an Import... reads.
    doing: Cell<Doing>,
    importing: RefCell<Option<PathBuf>>,
    hwnd: Arc<AtomicUsize>,
    /// The list's rows, as shown.
    entries: RefCell<Vec<Entry>>,
    /// The file it was opened from or saved to last.
    file: RefCell<Option<PathBuf>>,
    /// How many changes were made, and how many there were when it was
    /// last opened or saved: it is changed while they differ.
    changes: Cell<u64>,
    saved: Cell<u64>,
    /// The column it is sorted by, and whether A to Z.
    sort: Cell<Option<(Column, bool)>>,
    /// Adding or opening, on a thread of its own.
    job: RefCell<Option<Job>>,
    /// The file being opened, while the job reads it.
    opening: RefCell<Option<PathBuf>>,
    /// A save on its way.
    saving: RefCell<Option<Saving>>,
    after: Cell<Option<After>>,
    min: Cell<(i32, i32)>,
}

fn beep() {
    sys::beep(MB_OK);
}

fn set_text(hwnd: HWND, text: &str) {
    win::set_text(hwnd, text);
}

/// A menu bar of popups `(title, items)`.
fn menu_bar(menus: &[(&str, &[Item<'_>])]) -> HMENU {
    // A new menu, filled with popups this function makes; the window it is
    // set on destroys it.
    let bar = win::menu::create_bar();
    for &(title, items) in menus {
        let popup = menu::submenu(items);
        win::menu::append(bar, &win::menu::NewItem::submenu(title, popup));
    }
    bar
}

fn enable_item(m: HMENU, id: u16, on: bool) {
    win::menu::enable(m, win::menu::By::Command(u32::from(id)), on);
}

impl Editor {
    fn list(dlg: HWND) -> HWND {
        item(dlg, ID_LIST)
    }

    fn modified(&self) -> bool {
        self.changes.get() != self.saved.get()
    }

    fn changed(&self) {
        self.changes.set(self.changes.get() + 1);
    }

    /// The list's name, as the title and the questions say it.
    fn name(&self) -> String {
        self.file
            .borrow()
            .as_ref()
            .and_then(|p| p.file_name())
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "Untitled".to_owned())
    }

    fn init(&self, dlg: HWND) {
        self.hwnd.store(dlg as usize, Ordering::Release);
        let list = Editor::list(dlg);
        let ex =
            LVS_EX_FULLROWSELECT | LVS_EX_DOUBLEBUFFER | LVS_EX_LABELTIP | LVS_EX_HEADERDRAGDROP;
        lv::set_extended_style(list, ex, ex);
        let dpi = crate::custom::window_dpi(list) as i32;
        for (i, &(column, title, width)) in COLUMNS.iter().enumerate() {
            let col = LVCOLUMNW {
                mask: LVCF_WIDTH | LVCF_FMT,
                fmt: if column == Column::Size {
                    LVCFMT_RIGHT
                } else {
                    LVCFMT_LEFT
                },
                cx: width * dpi / 96,
                ..Default::default()
            };
            // The title goes in as the column's text (LVCF_TEXT).
            lv::insert_column(list, i as i32, col, Some(title));
        }
        let bar = menu_bar(&[
            (
                "&File",
                &[
                    Item::Cmd(cmd::NEW, "&New\tCtrl+N"),
                    Item::Cmd(cmd::OPEN, "&Open...\tCtrl+O"),
                    Item::Cmd(cmd::SAVE, "&Save\tCtrl+S"),
                    Item::Cmd(cmd::SAVE_AS, "Save &As...\tCtrl+Shift+S"),
                    Item::Sep,
                    Item::Cmd(cmd::IMPORT, "&Import..."),
                    Item::Cmd(cmd::EXPORT, "&Export..."),
                    Item::Sep,
                    Item::Cmd(cmd::CLOSE, "&Close\tCtrl+W"),
                ],
            ),
            (
                "&Edit",
                &[
                    Item::Cmd(cmd::ADD_FILES, "Add &Files..."),
                    Item::Cmd(cmd::ADD_FOLDER, "Add F&older..."),
                    Item::Cmd(cmd::ADD_FOLDER_SUBFOLDERS, "Add Folder and &Subfolders..."),
                    Item::Cmd(crate::slotedit::cmd::EDITOR_PASTE, "&Paste\tCtrl+V"),
                    Item::Sep,
                    Item::Cmd(cmd::REMOVE, "&Remove\tDel"),
                    Item::Cmd(cmd::REMOVE_DUPLICATES, "Remove &Duplicates"),
                    Item::Sep,
                    Item::Cmd(cmd::UPDATE, "&Update"),
                    Item::Cmd(cmd::READ_EXTENDED, "Read Extended &Information"),
                    Item::Sep,
                    Item::Cmd(cmd::SELECT_ALL, "Select &All\tCtrl+A"),
                    Item::Sep,
                    Item::Cmd(cmd::STOP, "S&top Adding\tEsc"),
                ],
            ),
        ]);
        // The dialog's menu bar; files may be dropped on it.
        win::menu::set_for_window(dlg, bar);
        win::dropfiles::accept(dlg, true);
        let dark = self.look.get().dark;
        dialog::theme_new(dlg, dark);
        // The subclass removes itself as the dialog goes.
        set_frame(dlg, dark);
        self.place(dlg);
        let wr = win::window_rect(dlg);
        let min = ((wr.right - wr.left) / 2, (wr.bottom - wr.top) / 2);
        self.min.set(min);
        // W6: where it was last, on the monitors there are now.
        if self.place.sized() {
            places::restore_at(
                dlg,
                Which::FileListEditor,
                &self.place,
                core::ptr::null_mut(),
                min,
            );
        }
        self.layout(dlg);
        self.show_title(dlg);
        self.show_count(dlg);
        win::set_focus(list);
        // W6: `-edit`'s list.
        let file = self.start_file.take();
        if let Some(path) = file {
            self.open_path(dlg, path);
        }
    }

    /// W6: what goz-gui's window keeps of the editor now: where it is, and
    /// Export...'s choices.
    fn keep(&self, dlg: HWND) {
        let place = places::measured(
            dlg,
            Which::FileListEditor,
            core::ptr::null_mut(),
            self.place.clone(),
        );
        *lock(&KEPT) = Some(Kept {
            place: Some(place),
            export_type: self.export_type.get(),
            export_selection: self.export_selection.get(),
        });
    }

    /// Reads the list at `path` into the editor, in place of what it holds.
    fn open_path(&self, dlg: HWND, path: PathBuf) {
        self.clear(dlg);
        self.start(
            dlg,
            vec![Source::List(path.clone())],
            Some(path),
            Doing::Opening,
        );
    }

    /// W6: `-edit` handed a list to the editor open: it opens once a
    /// changed list is saved or let go.
    fn open_pending(&self, dlg: HWND) {
        if self.saving.borrow().is_some() || !self.settle(dlg, After::OpenPending) {
            return;
        }
        let path = lock(&PENDING).take();
        if let Some(path) = path {
            self.open_path(dlg, path);
        }
    }

    /// W6: goz-gui's window changed View > Theme.
    fn restyle(&self, dlg: HWND, look: Look) {
        if look == self.look.get() {
            return;
        }
        self.look.set(look);
        dialog::set_app_look(look);
        dialog::apply_theme(dlg, look.dark);
        // The dialog's frame subclass anew, and its menu bar and list
        // repainted.
        set_frame(dlg, look.dark);
        win::menu::draw_bar(dlg);
        win::invalidate(Editor::list(dlg), None, true);
    }

    /// Centred over the main window, inside the work area of the monitor
    /// it is on (a window off every monitor stays where it is put).
    fn place(&self, dlg: HWND) {
        let near = self.near;
        let d = win::window_rect(dlg);
        let (w, h) = (d.right - d.left, d.bottom - d.top);
        let mut x = near.left + (near.right - near.left - w) / 2;
        let mut y = near.top + (near.bottom - near.top - h) / 2;
        let monitor = sys::monitor_from_rect(&near, MONITOR_DEFAULTTONULL);
        if !monitor.is_null()
            && let Some(mi) = sys::monitor_info(monitor)
        {
            let work = mi.rcWork;
            x = x.min(work.right - w).max(work.left);
            y = y.min(work.bottom - h).max(work.top);
        }
        win::set_pos(
            dlg,
            core::ptr::null_mut(),
            (x, y, 0, 0),
            SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE,
        );
    }

    /// The list takes the window but for the status line under it.
    fn layout(&self, dlg: HWND) {
        let rc = win::client_rect(dlg);
        let (m, line) = units(dlg, 4, 12);
        let (w, h) = (rc.right, rc.bottom);
        win::move_window(Editor::list(dlg), 0, 0, w, (h - line).max(0), true);
        win::move_window(
            item(dlg, ID_STATUS),
            m,
            h - line + m / 2,
            (w - 2 * m).max(0),
            line - m,
            true,
        );
    }

    fn show_title(&self, dlg: HWND) {
        let star = if self.modified() { "*" } else { "" };
        set_text(dlg, &format!("{}{star} - goz {TITLE}", self.name()));
    }

    fn set_status(&self, dlg: HWND, text: &str) {
        set_text(item(dlg, ID_STATUS), text);
    }

    /// The status line at rest: how many items, and what is selected.
    fn show_count(&self, dlg: HWND) {
        let n = self.entries.borrow().len();
        let selected = lv::selected_count(Editor::list(dlg));
        let text = if selected > 0 {
            format!(
                "{} ({} selected)",
                items_text(n),
                crate::format::grouped(selected as u64)
            )
        } else {
            items_text(n)
        };
        self.set_status(dlg, &text);
    }

    /// The list shows the rows as they are now (`keep`: rows added at the
    /// end, the rest as they were; else all of them anew, nothing
    /// selected).
    fn show_rows(&self, dlg: HWND, keep: bool) {
        let n = self.entries.borrow().len();
        let list = Editor::list(dlg);
        if keep {
            lv::set_item_count(list, n, LVSICF_NOINVALIDATEALL | LVSICF_NOSCROLL);
        } else {
            lv::set_item_count(list, n, 0);
            set_state(list, usize::MAX, 0, LVIS_SELECTED | LVIS_FOCUSED);
            if n > 0 {
                set_state(list, 0, LVIS_FOCUSED, LVIS_FOCUSED);
            }
        }
        win::invalidate(list, None, true);
    }

    /// The selected rows, in order.
    fn selected(dlg: HWND) -> Vec<usize> {
        let list = Editor::list(dlg);
        let mut out = Vec::new();
        let mut at = usize::MAX;
        while let Ok(next) = usize::try_from(lv::next_item(list, at as i32, LVNI_SELECTED)) {
            out.push(next);
            at = next;
        }
        out
    }

    fn busy(&self) -> bool {
        self.job.borrow().is_some()
    }

    fn command(&self, dlg: HWND, id: u16) {
        match id {
            cmd::NEW => self.new_list(dlg),
            cmd::OPEN => self.open_list(dlg),
            cmd::SAVE => self.save(dlg, false),
            cmd::SAVE_AS => self.save(dlg, true),
            cmd::IMPORT => self.import(dlg),
            cmd::EXPORT => self.export(dlg),
            cmd::UPDATE => self.update(dlg),
            cmd::READ_EXTENDED => self.read_extended(dlg),
            cmd::CLOSE => self.close(dlg),
            cmd::ADD_FILES => self.add_files(dlg),
            cmd::ADD_FOLDER => self.add_folder(dlg, false),
            cmd::ADD_FOLDER_SUBFOLDERS => self.add_folder(dlg, true),
            cmd::REMOVE => self.remove(dlg),
            cmd::REMOVE_DUPLICATES => self.remove_duplicates(dlg),
            crate::slotedit::cmd::EDITOR_PASTE => self.paste(dlg),
            cmd::SELECT_ALL => {
                set_state(Editor::list(dlg), usize::MAX, LVIS_SELECTED, LVIS_SELECTED);
                self.show_count(dlg);
            }
            cmd::STOP => self.stop(),
            _ => {}
        }
    }

    /// The menus' items, greyed where they cannot run now.
    fn menu_opens(&self, dlg: HWND, m: HMENU) {
        let busy = self.busy();
        let saving = self.saving.borrow().is_some();
        let rows = !self.entries.borrow().is_empty();
        let selected = lv::selected_count(Editor::list(dlg)) > 0;
        enable_item(m, cmd::SAVE, !busy && !saving);
        enable_item(m, cmd::SAVE_AS, !busy && !saving);
        enable_item(m, cmd::EXPORT, !busy && !saving && rows);
        for id in [
            cmd::ADD_FILES,
            cmd::ADD_FOLDER,
            cmd::ADD_FOLDER_SUBFOLDERS,
            cmd::IMPORT,
        ] {
            enable_item(m, id, !busy);
        }
        enable_item(
            m,
            crate::slotedit::cmd::EDITOR_PASTE,
            !busy
                && (crate::clipboard::has_files()
                    || crate::clipboard::has_text()
                    || location::paste_text().is_some()),
        );
        enable_item(m, cmd::REMOVE, !busy && selected);
        enable_item(m, cmd::REMOVE_DUPLICATES, !busy && rows);
        let folders = self.entries.borrow().iter().any(|e| e.is_dir);
        enable_item(m, cmd::UPDATE, !busy && folders);
        enable_item(m, cmd::READ_EXTENDED, !busy && selected);
        enable_item(m, cmd::SELECT_ALL, rows);
        enable_item(m, cmd::STOP, busy);
    }

    /// Adding or opening stops (Esc, Edit > Stop Adding); what it found
    /// so far stays.
    fn stop(&self) {
        if let Some(job) = self.job.borrow().as_ref() {
            job.stop();
        }
    }

    /// Starts a job adding `sources` (`opening`: the file File > Open...
    /// reads), as `doing` says.
    fn start(&self, dlg: HWND, sources: Vec<Source>, opening: Option<PathBuf>, doing: Doing) {
        if self.busy() || sources.is_empty() {
            return;
        }
        match Job::start(sources, dlg as usize, WM_FOUND) {
            Ok(job) => {
                *self.job.borrow_mut() = Some(job);
                *self.opening.borrow_mut() = opening;
                self.doing.set(doing);
                self.set_status(dlg, &format!("{}...", doing.verb()));
            }
            Err(e) => self.say(dlg, &format!("Could not start: {e}")),
        }
    }

    /// W6: rows read again from the disk (Read Extended Information) take
    /// their place, under the path the list has them by.
    fn take_refreshed(&self, dlg: HWND, refreshed: Vec<(usize, Option<Entry>)>) {
        if refreshed.is_empty() {
            return;
        }
        let mut changed = false;
        {
            let mut entries = self.entries.borrow_mut();
            for (at, now) in refreshed {
                if let (Some(row), Some(now)) = (entries.get_mut(at), now) {
                    let now = Entry {
                        path: row.path.clone(),
                        ..now
                    };
                    if *row != now {
                        *row = now;
                        changed = true;
                    }
                }
            }
        }
        if changed {
            self.changed();
        }
        win::invalidate(Editor::list(dlg), None, true);
    }

    fn say(&self, dlg: HWND, text: &str) {
        msgbox::show(dlg, text, TITLE, MB_OK | MB_ICONWARNING);
    }

    /// A job has rows, or is over.
    fn found(&self, dlg: HWND) {
        let found = match self.job.borrow().as_ref() {
            Some(job) => job.take(),
            None => return,
        };
        let opening = self.opening.borrow().is_some();
        if !found.entries.is_empty() {
            self.entries.borrow_mut().extend(found.entries);
            if !opening {
                self.changed();
            }
            if self.sort.take().is_some() {
                self.show_arrow(dlg);
            }
            self.show_rows(dlg, true);
        }
        self.take_refreshed(dlg, found.refreshed);
        let doing = self.doing.get();
        if !found.done {
            self.set_status(
                dlg,
                &format!(
                    "{}: {} (Esc stops)",
                    doing.verb(),
                    items_text(found.seen as usize)
                ),
            );
            return;
        }
        self.job.take();
        // W6: an Import... that could not read its file says so.
        if let Some(path) = self.importing.take()
            && let Some(why) = &found.error
        {
            self.show_title(dlg);
            self.show_count(dlg);
            self.say(
                dlg,
                &format!(
                    "The file list could not be imported:\n\n{}\n\n{why}",
                    path.display()
                ),
            );
            return;
        }
        if doing == Doing::Reading {
            self.show_title(dlg);
            let n = found.seen.saturating_sub(found.unreadable);
            let mut text = format!("Read extended information for {}", items_text(n as usize));
            if found.unreadable > 0 {
                text.push_str(&format!(
                    "; {} not there now",
                    items_text(found.unreadable as usize)
                ));
            }
            if found.stopped {
                text.push_str(" (stopped)");
            }
            self.set_status(dlg, &text);
            return;
        }
        if let Some(path) = self.opening.take() {
            match found.error {
                Some(why) => {
                    self.entries.borrow_mut().clear();
                    self.show_rows(dlg, false);
                    self.say(
                        dlg,
                        &format!(
                            "The file list could not be opened:\n\n{}\n\n{why}",
                            path.display()
                        ),
                    );
                }
                // Stopped part way: what was read is a new list, not the
                // file (saving it there would cut the file short).
                None if found.stopped => self.changed(),
                None => {
                    *self.file.borrow_mut() = Some(path);
                    self.saved.set(self.changes.get());
                }
            }
        }
        self.show_title(dlg);
        self.show_count(dlg);
        if doing == Doing::Pasting && found.unreadable > 0 {
            // W7: a name not on the disk is listed by its name alone.
            self.set_status(
                dlg,
                &format!(
                    "{}; {} not on the disk, listed without size or dates",
                    items_text(self.entries.borrow().len()),
                    items_text(found.unreadable as usize)
                ),
            );
        } else if found.unreadable > 0 {
            let n = crate::format::grouped(found.unreadable);
            let n = if found.unreadable == 1 {
                "1 item or folder".to_owned()
            } else {
                format!("{n} items or folders")
            };
            self.set_status(
                dlg,
                &format!(
                    "{}; {n} could not be read",
                    items_text(self.entries.borrow().len())
                ),
            );
        } else if found.stopped {
            let n = self.entries.borrow().len();
            self.set_status(dlg, &format!("Stopped: {}", items_text(n)));
        }
    }

    /// Stops adding or opening and takes in what it found: the window is
    /// closing, and the list must be whole before the question.
    fn finish_job(&self, dlg: HWND) {
        let Some(job) = self.job.borrow_mut().take() else {
            return;
        };
        job.stop();
        let end = std::time::Instant::now() + STOP_WAIT;
        let mut taken = Vec::new();
        loop {
            let f = job.take();
            taken.extend(f.entries);
            if f.done || std::time::Instant::now() >= end {
                break;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        let opening = self.opening.take().is_some();
        if !taken.is_empty() || opening {
            self.entries.borrow_mut().extend(taken);
            // Part of a file opened, or items added: not what was saved.
            self.changed();
            self.show_rows(dlg, true);
            self.show_title(dlg);
        }
    }

    /// Asks whether to save a changed list before `then` (New, Open,
    /// Close); true when `then` may go on now. Yes saves first, and `then`
    /// runs once the save is done.
    fn settle(&self, dlg: HWND, then: After) -> bool {
        if !self.modified() {
            return true;
        }
        let answer = if location::editor_file().is_some() {
            IDNO
        } else {
            msgbox::show(
                dlg,
                &format!("Save changes to {}?", self.name()),
                TITLE,
                MB_YESNOCANCEL | MB_ICONQUESTION,
            )
        };
        match answer {
            IDYES => {
                self.after.set(Some(then));
                self.save(dlg, false);
                if self.saving.borrow().is_none() {
                    // No save after all (Save As... cancelled).
                    self.after.set(None);
                }
                false
            }
            IDNO => true,
            _ => false,
        }
    }

    /// Empties the list, which has no file then.
    fn clear(&self, dlg: HWND) {
        if let Some(job) = self.job.take() {
            job.stop();
        }
        self.opening.take();
        self.entries.borrow_mut().clear();
        *self.file.borrow_mut() = None;
        self.changes.set(0);
        self.saved.set(0);
        if self.sort.take().is_some() {
            self.show_arrow(dlg);
        }
        self.show_rows(dlg, false);
        self.show_title(dlg);
        self.show_count(dlg);
    }

    /// File > New.
    fn new_list(&self, dlg: HWND) {
        if self.saving.borrow().is_some() || !self.settle(dlg, After::New) {
            return;
        }
        self.clear(dlg);
    }

    /// File > Open...: asks for a list, then reads it into the editor.
    fn open_list(&self, dlg: HWND) {
        if self.saving.borrow().is_some() || !self.settle(dlg, After::Open) {
            return;
        }
        let path = match location::editor_file() {
            Some(p) => Some(p),
            None => crate::filedlg::open(dlg, "Open File List", KINDS, "efu"),
        };
        let Some(path) = path else { return };
        self.open_path(dlg, path);
    }

    /// File > Save (`as_new`: Save As..., or a list with no file yet).
    fn save(&self, dlg: HWND, as_new: bool) {
        if self.busy() || self.saving.borrow().is_some() {
            beep();
            return;
        }
        let known = self.file.borrow().clone();
        let path = match (known, as_new) {
            (Some(p), false) => Some(p),
            _ => match location::editor_file() {
                Some(p) => Some(p),
                None => crate::filedlg::save(
                    dlg,
                    "Save File List",
                    &format!("{}.efu", self.name().trim_end_matches(".efu")),
                    KINDS,
                    "efu",
                    None,
                )
                .map(|(p, _)| p),
            },
        };
        let Some(path) = path else { return };
        let entries = self.entries.borrow().clone();
        match Saving::start(entries, path, self.changes.get(), dlg as usize, WM_SAVED) {
            Ok(s) => {
                *self.saving.borrow_mut() = Some(s);
                self.set_status(dlg, "Saving...");
            }
            Err(e) => self.say(dlg, &format!("The file list could not be saved: {e}")),
        }
    }

    /// A save is over: said, and what waited for it runs.
    fn save_done(&self, dlg: HWND) {
        let outcome = self.saving.borrow().as_ref().and_then(Saving::outcome);
        let Some(outcome) = outcome else { return };
        let Some(s) = self.saving.take() else { return };
        if s.exported.is_some() {
            // W6: an Export...: the list keeps its file and its changes.
            self.show_count(dlg);
            match outcome {
                Ok(()) => {
                    let name = s
                        .path
                        .file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_default();
                    self.set_status(dlg, &format!("Exported {} to {name}", items_text(s.rows)));
                }
                Err(why) => self.say(
                    dlg,
                    &format!(
                        "The file list could not be exported:\n\n{}\n\n{why}",
                        s.path.display()
                    ),
                ),
            }
            // Close waited for it.
            if self.after.take() == Some(After::Close) {
                self.close(dlg);
            }
            return;
        }
        let after = self.after.take();
        match outcome {
            Ok(()) => {
                *self.file.borrow_mut() = Some(s.path.clone());
                self.saved.set(s.generation);
                self.show_title(dlg);
                let name = self.name();
                self.set_status(dlg, &format!("Saved {} to {name}", items_text(s.rows)));
                match after {
                    Some(After::New) => self.new_list(dlg),
                    Some(After::Open) => self.open_list(dlg),
                    Some(After::OpenPending) => self.open_pending(dlg),
                    Some(After::Close) => self.close(dlg),
                    None => {}
                }
            }
            Err(why) => {
                self.show_count(dlg);
                self.say(
                    dlg,
                    &format!(
                        "The file list could not be saved:\n\n{}\n\n{why}",
                        s.path.display()
                    ),
                );
            }
        }
    }

    /// File > Close (and the close box, and goz-gui's window closing): asks
    /// about unsaved changes, then the window goes. A save still writing
    /// is finished first.
    fn close(&self, dlg: HWND) {
        if self.saving.borrow().is_some() {
            // Close again once it is written (now, or when it says so).
            if let Some(s) = self.saving.borrow().as_ref() {
                s.wait(SAVE_WAIT);
            }
            self.after.set(Some(After::Close));
            self.save_done(dlg);
            return;
        }
        self.finish_job(dlg);
        if !self.settle(dlg, After::Close) {
            return;
        }
        dlg::end(dlg, 0);
    }

    /// Edit > Add Files...: files chosen in Windows' Open dialog.
    fn add_files(&self, dlg: HWND) {
        if self.busy() {
            return;
        }
        let files = pick_files(dlg);
        self.start(
            dlg,
            files.into_iter().map(Source::Item).collect(),
            None,
            Doing::Adding,
        );
    }

    /// Edit > Add Folder... (`subfolders`: and Subfolders...): a folder
    /// chosen in Windows' folder picker, with what it holds.
    fn add_folder(&self, dlg: HWND, subfolders: bool) {
        if self.busy() {
            return;
        }
        let title = if subfolders {
            "Add Folder and Subfolders"
        } else {
            "Add Folder"
        };
        let Some(folder) = crate::filetools::folder_pick::pick(dlg, title, "Add", None) else {
            return;
        };
        let path = String::from_utf16_lossy(&folder);
        self.start(
            dlg,
            vec![Source::Folder { path, subfolders }],
            None,
            Doing::Adding,
        );
    }

    /// Files and folders dropped on the editor (`paths`, read whole from
    /// the drop, which is freed by now): files as they are, folders with
    /// everything under them.
    fn dropped(&self, dlg: HWND, paths: Vec<Vec<u16>>) {
        if self.busy() {
            beep();
            return;
        }
        let sources = paths
            .iter()
            .map(|p| {
                let path = String::from_utf16_lossy(p);
                let dir = crate::filetools::attributes(p).is_some_and(|a| a & 0x10 != 0);
                if dir {
                    Source::Folder {
                        path,
                        subfolders: true,
                    }
                } else {
                    Source::Item(path)
                }
            })
            .collect();
        self.start(dlg, sources, None, Doing::Adding);
    }

    /// W7: Edit > Paste (Ctrl+V), Everything's "Paste filenames on the
    /// clipboard into the file list": files copied in Explorer as a drop
    /// adds them (folders with what is under them), or text, a name a
    /// line, each as the disk has it or by its name alone.
    fn paste(&self, dlg: HWND) {
        if self.busy() {
            beep();
            return;
        }
        // A test's names (`GOZ_GUI_PASTE_TEXT`) stand for the clipboard.
        let test = location::paste_text();
        let files = if test.is_some() {
            None
        } else {
            crate::clipboard::files(dlg)
        };
        let sources = match files {
            Some(clip) => clip
                .paths
                .iter()
                .map(|p| {
                    let path = String::from_utf16_lossy(p);
                    if crate::filetools::attributes(p).is_some_and(|a| a & 0x10 != 0) {
                        Source::Folder {
                            path,
                            subfolders: true,
                        }
                    } else {
                        Source::Item(path)
                    }
                })
                .collect(),
            None => {
                let names = test
                    .or_else(|| crate::clipboard::text(dlg))
                    .map(|t| super::pasted_names(&t))
                    .unwrap_or_default();
                if names.is_empty() {
                    Vec::new()
                } else {
                    vec![Source::Named(names)]
                }
            }
        };
        if sources.is_empty() {
            beep();
            self.set_status(
                dlg,
                "Nothing to paste: the clipboard holds no files or names",
            );
            return;
        }
        self.start(dlg, sources, None, Doing::Pasting);
    }

    /// Edit > Remove (Del): the selected rows go.
    fn remove(&self, dlg: HWND) {
        if self.busy() {
            return;
        }
        let rows = Editor::selected(dlg);
        if rows.is_empty() {
            return;
        }
        let first = rows[0];
        let gone = super::remove_rows(&mut self.entries.borrow_mut(), &rows);
        if gone > 0 {
            self.changed();
        }
        self.show_rows(dlg, false);
        // The row after the last one taken has the keyboard.
        let n = self.entries.borrow().len();
        if n > 0 {
            let at = first.min(n - 1);
            let list = Editor::list(dlg);
            set_state(list, usize::MAX, 0, LVIS_FOCUSED);
            set_state(
                list,
                at,
                LVIS_SELECTED | LVIS_FOCUSED,
                LVIS_SELECTED | LVIS_FOCUSED,
            );
        }
        self.show_title(dlg);
        self.show_count(dlg);
    }

    /// Edit > Remove Duplicates: a path listed again (any case) goes.
    fn remove_duplicates(&self, dlg: HWND) {
        if self.busy() {
            return;
        }
        let gone = super::remove_duplicates(&mut self.entries.borrow_mut());
        if gone > 0 {
            self.changed();
            self.show_rows(dlg, false);
            self.show_title(dlg);
        }
        let n = self.entries.borrow().len();
        self.set_status(
            dlg,
            &format!("{}; {} removed", items_text(n), items_text(gone)),
        );
    }

    /// W6: File > Import...: the rows of a list file (EFU, CSV, a text file
    /// of paths) added to this list.
    fn import(&self, dlg: HWND) {
        if self.busy() {
            beep();
            return;
        }
        let path = match location::editor_file() {
            Some(p) => Some(p),
            None => crate::filedlg::open(dlg, "Import File List", IMPORT_DIALOG, "efu"),
        };
        let Some(path) = path else { return };
        *self.importing.borrow_mut() = Some(path.clone());
        self.start(dlg, vec![Source::List(path)], None, Doing::Importing);
    }

    /// W6: File > Export...: the list (or the selected rows, "Export
    /// selection only") written in one of Everything's types; the list
    /// keeps its own file.
    fn export(&self, dlg: HWND) {
        if self.busy() || self.saving.borrow().is_some() {
            beep();
            return;
        }
        let selected = Editor::selected(dlg);
        let last = Export::at(self.export_type.get());
        let chosen = match location::editor_file() {
            Some(p) => {
                let format = Export::of_path(&p).unwrap_or(last);
                Some((p, !selected.is_empty(), format))
            }
            None => {
                let name = self.name();
                let stem = std::path::Path::new(&name)
                    .file_stem()
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or(name);
                let check = crate::filedlg::Check {
                    label: "Export selection only",
                    checked: self.export_selection.get() && !selected.is_empty(),
                    enabled: !selected.is_empty(),
                };
                crate::filedlg::save_kind_check(
                    dlg,
                    "Export File List",
                    &stem,
                    EXPORT_DIALOG,
                    last.index(),
                    check,
                )
                .map(|(p, checked, kind)| {
                    // All Files: the type its extension names, else the last.
                    let format = Export::ALL
                        .get(kind)
                        .copied()
                        .or_else(|| Export::of_path(&p))
                        .unwrap_or(last);
                    (p, checked, format)
                })
            }
        };
        let Some((path, only_selected, format)) = chosen else {
            return;
        };
        self.export_type.set(format.index());
        if !selected.is_empty() {
            self.export_selection.set(only_selected);
        }
        let entries: Vec<Entry> = {
            let all = self.entries.borrow();
            if only_selected && !selected.is_empty() {
                selected
                    .iter()
                    .filter_map(|&r| all.get(r).cloned())
                    .collect()
            } else {
                all.clone()
            }
        };
        self.keep(dlg);
        match Saving::export(entries, path, format, dlg as usize, WM_SAVED) {
            Ok(s) => {
                *self.saving.borrow_mut() = Some(s);
                self.set_status(dlg, "Exporting...");
            }
            Err(e) => self.say(dlg, &format!("The file list could not be exported: {e}")),
        }
    }

    /// W6: Edit > Update: Everything's "Rescan root folders in the file
    /// list": each folder no other listed folder holds is walked again
    /// (with its subfolders when the list held any), in place of what the
    /// list held under it; the files listed on their own stay as they are.
    fn update(&self, dlg: HWND) {
        if self.busy() {
            beep();
            return;
        }
        let roots = super::roots(&self.entries.borrow());
        if roots.is_empty() {
            beep();
            self.set_status(dlg, "Nothing to update: the list holds no folder");
            return;
        }
        let paths: Vec<String> = roots.iter().map(|(p, _)| p.clone()).collect();
        let gone = super::remove_under(&mut self.entries.borrow_mut(), &paths);
        if gone > 0 {
            self.changed();
        }
        if self.sort.take().is_some() {
            self.show_arrow(dlg);
        }
        self.show_rows(dlg, false);
        let sources = roots
            .into_iter()
            .map(|(path, subfolders)| Source::Folder { path, subfolders })
            .collect();
        self.start(dlg, sources, None, Doing::Updating);
        self.show_title(dlg);
    }

    /// W6: Edit > Read Extended Information: the selected rows' size, dates
    /// and attributes read from the disk again.
    fn read_extended(&self, dlg: HWND) {
        if self.busy() {
            beep();
            return;
        }
        let rows: Vec<(usize, String)> = {
            let entries = self.entries.borrow();
            Editor::selected(dlg)
                .into_iter()
                .filter_map(|r| entries.get(r).map(|e| (r, e.path.clone())))
                .collect()
        };
        if rows.is_empty() {
            beep();
            return;
        }
        self.start(dlg, vec![Source::Refresh(rows)], None, Doing::Reading);
    }

    /// A click on a column's header: sorted by it, the other way round
    /// when it is the sort already.
    fn sort_by(&self, dlg: HWND, column: Column) {
        if self.busy() {
            beep();
            return;
        }
        let ascending = match self.sort.get() {
            Some((c, a)) if c == column => !a,
            _ => column.first_ascending(),
        };
        super::sort(&mut self.entries.borrow_mut(), column, ascending);
        self.sort.set(Some((column, ascending)));
        self.show_arrow(dlg);
        self.show_rows(dlg, false);
        self.show_count(dlg);
    }

    fn show_arrow(&self, dlg: HWND) {
        let columns = lv::header(Editor::list(dlg));
        let sort = self.sort.get();
        for (i, &(column, _, _)) in COLUMNS.iter().enumerate() {
            let mut hd = header::get_item(columns, i as i32, HDI_FORMAT).map_or(
                HDITEMW {
                    mask: HDI_FORMAT,
                    ..Default::default()
                },
                |(hd, _)| hd,
            );
            hd.fmt &= !(HDF_SORTUP | HDF_SORTDOWN);
            if let Some((c, ascending)) = sort
                && c == column
            {
                hd.fmt |= if ascending { HDF_SORTUP } else { HDF_SORTDOWN };
            }
            header::set_item(columns, i as i32, hd, None);
        }
    }

    fn disp_info(&self, info: &mut View<'_, NMLVDISPINFOW>) {
        if info.item.mask & LVIF_TEXT == 0 {
            return;
        }
        // W7: a Type cell's kind, asked of the shell once the rows are let
        // go (no borrow is held across its call).
        let mut kind = None;
        let mut text: Vec<u16> = {
            let entries = self.entries.borrow();
            let row = usize::try_from(info.item.iItem)
                .ok()
                .and_then(|r| entries.get(r));
            let column = usize::try_from(info.item.iSubItem)
                .ok()
                .and_then(Column::at);
            let mut out = Vec::new();
            if let (Some(e), Some(column)) = (row, column) {
                match column {
                    Column::Name => out.extend(e.name().encode_utf16()),
                    Column::Path => out.extend(e.folder().encode_utf16()),
                    Column::Size => {
                        if let (false, Some(size)) = (e.is_dir, e.size) {
                            out.extend(crate::format::size_cell(size).encode_utf16());
                        }
                    }
                    Column::Extension => out.extend(e.extension().encode_utf16()),
                    Column::Type => {
                        kind = Some(Entry {
                            path: e.name().to_owned(),
                            is_dir: e.is_dir,
                            ..Entry::default()
                        });
                    }
                    Column::Modified => {
                        if let Some(t) = e.modified {
                            crate::format::date(t, &mut out);
                        }
                    }
                    Column::Created => {
                        if let Some(t) = e.created {
                            crate::format::date(t, &mut out);
                        }
                    }
                    Column::Attributes => out.extend(e.attributes_text().encode_utf16()),
                }
            }
            out
        };
        if let Some(e) = kind {
            text.extend_from_slice(&super::type_name(&e));
        }
        // Into the list's buffer of `cchTextMax` code units.
        info.set_text(&text);
    }

    /// A key in the list: the menus' shortcuts.
    fn key(&self, dlg: HWND, vk: u16) {
        let (ctrl, shift) = (sys::key_down(VK_CONTROL), sys::key_down(VK_SHIFT));
        let id = match (ctrl, shift, vk) {
            (false, false, VK_DELETE) => cmd::REMOVE,
            (true, false, k) if k == u16::from(b'A') => cmd::SELECT_ALL,
            (true, false, k) if k == u16::from(b'V') => crate::slotedit::cmd::EDITOR_PASTE,
            (true, false, k) if k == u16::from(b'N') => cmd::NEW,
            (true, false, k) if k == u16::from(b'O') => cmd::OPEN,
            (true, false, k) if k == u16::from(b'S') => cmd::SAVE,
            (true, true, k) if k == u16::from(b'S') => cmd::SAVE_AS,
            (true, false, k) if k == u16::from(b'W') => cmd::CLOSE,
            _ => return,
        };
        self.command(dlg, id);
    }

    /// The list's notifications; `Some` is DWLP_MSGRESULT's value.
    fn notify(&self, dlg: HWND, m: &Msg) -> Option<isize> {
        let n = m.notify()?;
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
            LVN_ODFINDITEMW => return Some(-1),
            LVN_COLUMNCLICK => {
                let sub = n.get::<NMLISTVIEW>().map(|nm| nm.iSubItem);
                if let Some(c) = sub
                    .and_then(|s| usize::try_from(s).ok())
                    .and_then(Column::at)
                {
                    self.sort_by(dlg, c);
                }
            }
            LVN_KEYDOWN => {
                let vk = n.get::<NMLVKEYDOWN>()?.wVKey;
                self.key(dlg, vk);
            }
            // The count with what is selected, unless the line says
            // something else now.
            windows_sys::Win32::UI::Controls::LVN_ITEMCHANGED
            | windows_sys::Win32::UI::Controls::LVN_ODSTATECHANGED
                if !self.busy() && self.saving.borrow().is_none() =>
            {
                self.show_count(dlg);
            }
            _ => {}
        }
        None
    }
}

/// Edit > Add Files...: Windows' Open dialog, several files at once; the
/// files chosen (none when cancelled).
pub(crate) fn pick_files(owner: HWND) -> Vec<String> {
    let mut out = Vec::new();
    // COM is initialised on this (the editor's) thread; the dialog runs its
    // own loop while nothing of the editor is borrowed.
    let Some(dialog) = FileDialog::open() else {
        return out;
    };
    let Some(options) = dialog.options() else {
        return out;
    };
    let files = FOS_ALLOWMULTISELECT | FOS_FILEMUSTEXIST | FOS_FORCEFILESYSTEM | FOS_NOCHANGEDIR;
    dialog.set_options(options | files.0);
    dialog.set_title("Add Files");
    if !dialog.show(owner) {
        return out;
    }
    let Some(items) = dialog.results() else {
        return out;
    };
    let n = items.count().unwrap_or(0);
    for i in 0..n {
        let Some(item) = items.get(i) else {
            continue;
        };
        if let Some(path) = item
            .file_system_path()
            .and_then(|p| String::from_utf16(&p).ok())
        {
            out.push(path);
        }
    }
    out
}

/// Sets (or sets anew) the dialog's frame subclass, which paints its menu
/// bar dark when `dark`.
fn set_frame(dlg: HWND, dark: bool) {
    subclass::set(dlg, &FRAME_SUBCLASS, move |m| frame_proc(m, dark));
}

/// Paints the dialog's menu bar dark (when `dark`).
fn frame_proc(m: &Msg, dark: bool) -> isize {
    // Each call goes to the next procedure in the chain.
    let hwnd = m.hwnd;
    if dark {
        let pal = Palette::new(true);
        match m.id {
            win::menubar::WM_UAHDRAWMENU => {
                if let Some(um) = win::menubar::draw_menu(m) {
                    paint::menubar_of(hwnd, &pal, &um);
                }
                return 1;
            }
            win::menubar::WM_UAHDRAWMENUITEM => {
                if let Some(item) = win::menubar::draw_menu_item(m) {
                    paint::menubar_item_of(&pal, &item);
                }
                return 1;
            }
            WM_NCPAINT | WM_NCACTIVATE => {
                let r = m.def();
                paint::menubar_line_of(hwnd, &pal);
                return r;
            }
            _ => {}
        }
    }
    if m.id == WM_DESTROY {
        subclass::remove(hwnd, &FRAME_SUBCLASS);
    }
    m.def()
}

/// The menu bar's UAHMENU as `paint` takes it.
fn dialog_proc(s: &Editor, m: &Msg) -> isize {
    let (dlg, msg, w, l) = (m.hwnd, m.id, m.w, m.l);
    let dark = s.look.get().dark;
    if let Some(answer) = common(m, &s.look) {
        if msg == WM_SETTINGCHANGE && s.look.get().dark != dark {
            // The system's theme changed, and the editor follows it: the
            // menu bar too (the dialog's frame subclass anew, its menu bar
            // repainted).
            let dark = s.look.get().dark;
            set_frame(dlg, dark);
            win::menu::draw_bar(dlg);
        }
        return answer;
    }
    match msg {
        WM_INITDIALOG => {
            s.init(dlg);
            // The keyboard is put on the list by `init`.
            0
        }
        WM_SIZE => {
            s.layout(dlg);
            // W6: maximized or back: kept at once (a drag at its end).
            if matches!(w as u32, SIZE_MAXIMIZED | SIZE_RESTORED) {
                s.keep(dlg);
            }
            1
        }
        WM_EXITSIZEMOVE => {
            s.keep(dlg);
            0
        }
        WM_EDITOR_OPEN => {
            s.open_pending(dlg);
            1
        }
        WM_EDITOR_LOOK => {
            s.restyle(
                dlg,
                Look {
                    dark: w != 0,
                    follow_system: l != 0,
                },
            );
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
            let from_list = m.notify().is_some_and(|n| n.id() == usize::from(ID_LIST));
            if !from_list {
                return 0;
            }
            if let Some(answer) = s.notify(dlg, m) {
                dlg::set_result(dlg, answer);
            }
            1
        }
        WM_INITMENUPOPUP => {
            s.menu_opens(dlg, w as HMENU);
            0
        }
        WM_COMMAND => {
            let id = win::loword(w);
            match i32::from(id) {
                // Esc: adding or opening stops; the window stays.
                IDCANCEL => s.stop(),
                // Enter does nothing.
                IDOK => {}
                _ => s.command(dlg, id),
            }
            1
        }
        WM_CLOSE => {
            s.close(dlg);
            1
        }
        WM_DROPFILES => {
            // The paths of files dropped on the editor (the drop is
            // finished once this returns).
            if let Some(paths) = m.dropped_files() {
                s.dropped(dlg, paths);
            }
            1
        }
        WM_FOUND => {
            s.found(dlg);
            1
        }
        WM_SAVED => {
            s.save_done(dlg);
            1
        }
        WM_DESTROY => {
            s.hwnd.store(0, Ordering::Release);
            // W6: its place for goz-gui's window, which is told.
            s.keep(dlg);
            if s.owner != 0 {
                // A window gone meanwhile drops it.
                win::post_app(s.owner as HWND, WM_EDITOR_CLOSED, 0, 0);
            }
            0
        }
        _ => 0,
    }
}
