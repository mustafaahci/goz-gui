//! The file tools from the window's side (wave 1, F5; the work is
//! `crate::filetools`): Edit > Undo, Redo and Undo History..., Copy to
//! Folder... and Move to Folder..., Edit > Advanced > Advanced Rename...,
//! Set File Time... and New Folder, File > Create Shortcut, and the undo
//! record every file operation goz-gui runs adds to.
//!
//! Each tool acts on the selected rows that are held, fetching the others
//! first as the other result actions do (`results::Then::FileTool`). Its
//! dialog (modal, opened quietly when a command was posted from the
//! background) asks what to do; the steps then run on the file operations
//! thread, and their results come back to [`App::tool_done`], which
//! records them for Undo and searches again once gozd has seen the change.
//! No `RefCell` of the window is borrowed while a dialog or a folder picker
//! runs its loop.

use std::cell::{Cell, RefCell};

use windows_sys::Win32::Foundation::HWND;
use windows_sys::Win32::System::Com::CoTaskMemFree;
use windows_sys::Win32::System::Diagnostics::Debug::MessageBeep;
use windows_sys::Win32::UI::Controls::{
    LVM_GETNEXTITEM, LVM_GETSELECTEDCOUNT, LVM_REDRAWITEMS, LVNI_FOCUSED,
};
use windows_sys::Win32::UI::Shell::{FOLDERID_Desktop, SHGetKnownFolderPath};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    HMENU, IDYES, MB_ICONQUESTION, MB_OK, MB_YESNO, MENUITEMINFOW, MFS_ENABLED, MFS_GRAYED,
    MIIM_STATE, MIIM_STRING, PostMessageW, SetMenuItemInfoW, SetTimer,
};

use super::results::{Act, Then};
use super::slots::F5_TIMER;
use super::{App, send};
use crate::datatools::cmd::{ADVANCED_COPY_TO, ADVANCED_MOVE_TO};
use crate::dialog::Quiet;
use crate::fileops::{Job, Outcome, parent};
use crate::filetools::settings::{ToolSettings, remember};
use crate::filetools::undo::{self, Action, Item, Record, Stack};
use crate::filetools::{
    Change, Kind, Purpose, Step, StepResult, Task, cmd, folder_pick, rename_dialog, times_dialog,
    undo_dialog,
};
use crate::history::filetime_now;
use crate::menu::{self, Item as MenuItem};
use crate::msgbox;
use crate::store::location;
use crate::wide::wz;

/// The name a new folder gets, numbered by Windows when taken (Everything's
/// "New folder", "New folder (2)").
const NEW_FOLDER_NAME: &str = "New folder";
/// The timer that looks for a new folder's row, to rename it in place.
pub(super) const TIMER_NEW_FOLDER: usize = F5_TIMER;
/// How often, and how many times, it looks (about 8 s: gozd sees a new
/// folder within a moment, later when it is busy), and every how many
/// looks it searches again for it.
const NEW_FOLDER_EVERY_MS: u32 = 250;
const NEW_FOLDER_TRIES: u8 = 32;
const NEW_FOLDER_REFRESH: u8 = 4;
/// Explorer's question when a shortcut cannot be made beside its item.
const SHORTCUT_HERE: &str = "Windows can't create a shortcut here.\n\nDo you want the shortcut to be placed on the desktop instead?";

/// The Edit menu's own position for items put at its end: no item runs
/// this command.
const AT_END: u16 = u16::MAX;

/// The file tools' state in the window.
pub(super) struct Tools {
    /// What can be undone and redone.
    stack: RefCell<Stack>,
    /// A tool's dialog is open (it is modal, but commands can still be
    /// posted to the window).
    dialog: Cell<bool>,
    /// The Undo History, while it is open: told when the record changes.
    history: Cell<HWND>,
    /// A new folder to rename in place once its row comes.
    new_folder: RefCell<Option<NewFolder>>,
}

/// A new folder waiting for its row.
struct NewFolder {
    path: Vec<u16>,
    /// The search it was made under: another one since, and it is not
    /// looked for any more.
    search: String,
    /// How many more times to look for it.
    left: u8,
}

impl Tools {
    pub(super) fn new(settings: &ToolSettings) -> Tools {
        Tools {
            stack: RefCell::new(Stack::new(settings.max_undo)),
            dialog: Cell::new(false),
            history: Cell::new(core::ptr::null_mut()),
            new_folder: RefCell::new(None),
        }
    }
}

/// Sets the text of `menu`'s item `id`, and whether it can be chosen.
fn set_item(menu: HMENU, id: u16, text: &str, enabled: bool) {
    let mut text = wz(text);
    // SAFETY: the rest of MENUITEMINFOW is plain data.
    let mut mii: MENUITEMINFOW = unsafe { core::mem::zeroed() };
    mii.cbSize = size_of::<MENUITEMINFOW>() as u32;
    mii.fMask = MIIM_STRING | MIIM_STATE;
    mii.fState = if enabled { MFS_ENABLED } else { MFS_GRAYED };
    mii.dwTypeData = text.as_mut_ptr();
    // SAFETY: an item of a menu of this thread, by command; Windows copies
    // the text.
    unsafe { SetMenuItemInfoW(menu, u32::from(id), 0, &mii) };
}

fn beep() {
    // SAFETY: plays a system sound.
    unsafe { MessageBeep(MB_OK) };
}

fn text(path: &[u16]) -> String {
    String::from_utf16_lossy(path)
}

/// What the status bar says of the items that merged into, or replaced,
/// one of their names already there: Undo leaves them.
fn landed(results: &[StepResult]) -> Option<String> {
    let n = results
        .iter()
        .filter(|r| matches!(r, StepResult::Landed { .. }))
        .count();
    match n {
        0 => None,
        1 => Some("1 item merged into or replaced one of its name: it cannot be undone".into()),
        n => Some(format!(
            "{} items merged into or replaced ones of their names: they cannot be undone",
            crate::format::grouped(n as u64)
        )),
    }
}

/// The Desktop folder (`GOZ_GUI_DESKTOP_DIR` in a test).
fn desktop() -> Option<Vec<u16>> {
    if let Some(dir) = location::desktop_dir() {
        return Some(dir.as_os_str().to_string_lossy().encode_utf16().collect());
    }
    let mut p: *mut u16 = core::ptr::null_mut();
    // SAFETY: the shell allocates a NUL-terminated path into `p`, read up to
    // its NUL and then freed with the allocator the shell used.
    unsafe {
        let hr = SHGetKnownFolderPath(&FOLDERID_Desktop, 0, core::ptr::null_mut(), &mut p);
        let path = (hr >= 0 && !p.is_null()).then(|| {
            let len = (0..).take_while(|&i| *p.add(i) != 0).count();
            core::slice::from_raw_parts(p, len).to_vec()
        });
        CoTaskMemFree(p.cast());
        path
    }
}

/// What the status bar says of the steps that did not go: how many of
/// them, and the first reason; nothing when all went or the user stopped.
fn trouble(what: &str, results: &[StepResult]) -> Option<String> {
    let mut why = None;
    let mut failed = 0;
    for r in results {
        match r {
            StepResult::Skipped(w) | StepResult::Failed(w) => {
                failed += 1;
                why.get_or_insert_with(|| w.clone());
            }
            StepResult::Denied => failed += 1,
            _ => {}
        }
    }
    let why = why?;
    let items = if failed == 1 { "item" } else { "items" };
    Some(format!(
        "Could not {what} {} {items}: {why}",
        crate::format::grouped(failed as u64)
    ))
}

impl App {
    /// The window starts: the Edit menu gets the tools' items, in
    /// Everything's places.
    pub(super) fn start_file_tools(&self) {
        let edit = self.menus.edit;
        menu::add_items(
            edit,
            None,
            &[
                MenuItem::Cmd(cmd::UNDO, &undo::menu_label(false, None)),
                MenuItem::Cmd(cmd::REDO, &undo::menu_label(true, None)),
                MenuItem::Cmd(cmd::UNDO_HISTORY, "Undo Hist&ory...\tCtrl+Shift+Z"),
                MenuItem::Sep,
            ],
        );
        menu::add_items(
            edit,
            Some(menu::PASTE),
            &[
                MenuItem::Sep,
                MenuItem::Cmd(cmd::COPY_TO_FOLDER, "Cop&y to Folder..."),
                MenuItem::Cmd(cmd::MOVE_TO_FOLDER, "Mo&ve to Folder..."),
            ],
        );
        let advanced = menu::submenu(&[
            // W4 (G1): Everything's first two.
            MenuItem::Cmd(ADVANCED_COPY_TO, "Advanced Copy to &Folder..."),
            MenuItem::Cmd(ADVANCED_MOVE_TO, "Advanced Mo&ve to Folder..."),
            MenuItem::Cmd(cmd::ADVANCED_RENAME, "Advanced Rena&me...\tShift+F2"),
            MenuItem::Sep,
            MenuItem::Cmd(cmd::SET_FILE_TIME, "Set File &Time..."),
            MenuItem::Cmd(cmd::NEW_FOLDER, "Ne&w Folder"),
        ]);
        menu::add_items(
            edit,
            Some(AT_END),
            &[MenuItem::Sep, MenuItem::Sub("Advanc&ed", advanced)],
        );
    }

    /// A menu is about to open: Undo and Redo say what they would undo or
    /// redo, and the tools that act on the selection are greyed without one.
    pub(super) fn file_tools_menu_popup(&self, popup: HMENU) {
        if popup == self.menus.edit {
            let (undo, redo) = {
                let s = self.file_tools.stack.borrow();
                (
                    s.next_undo().map(|r| r.action),
                    s.next_redo().map(|r| r.action),
                )
            };
            set_item(
                popup,
                cmd::UNDO,
                &undo::menu_label(false, undo),
                undo.is_some(),
            );
            set_item(
                popup,
                cmd::REDO,
                &undo::menu_label(true, redo),
                redo.is_some(),
            );
        }
        let selection = !self.selected(1).is_empty();
        for (id, label, on) in [
            (cmd::COPY_TO_FOLDER, "Cop&y to Folder...", selection),
            (cmd::MOVE_TO_FOLDER, "Mo&ve to Folder...", selection),
            (
                cmd::ADVANCED_RENAME,
                "Advanced Rena&me...\tShift+F2",
                selection,
            ),
            (cmd::SET_FILE_TIME, "Set File &Time...", selection),
            (ADVANCED_COPY_TO, "Advanced Copy to &Folder...", selection),
            (ADVANCED_MOVE_TO, "Advanced Mo&ve to Folder...", selection),
            (cmd::NEW_FOLDER, "Ne&w Folder", self.can_make_folder()),
        ] {
            set_item(popup, id, label, on);
        }
    }

    /// A file tool's command; false for an id that is not one.
    pub(super) fn file_tools_command(&self, id: u16) -> bool {
        match id {
            cmd::UNDO => self.undo_last(),
            cmd::REDO => self.redo_last(),
            cmd::UNDO_HISTORY => self.undo_history(),
            cmd::COPY_TO_FOLDER => self.copy_move_to_folder(false),
            cmd::MOVE_TO_FOLDER => self.copy_move_to_folder(true),
            cmd::ADVANCED_RENAME => self.advanced_rename(),
            cmd::SET_FILE_TIME => self.set_file_time(),
            cmd::NEW_FOLDER => self.new_folder(),
            cmd::CREATE_SHORTCUT => self.create_shortcut(),
            ADVANCED_COPY_TO => self.advanced_copy_move(false),
            ADVANCED_MOVE_TO => self.advanced_copy_move(true),
            _ => return false,
        }
        true
    }

    /// A tool's main-window timer; false for one that is not.
    pub(super) fn file_tools_timer(&self, id: usize) -> bool {
        if id != TIMER_NEW_FOLDER {
            return false;
        }
        self.look_for_new_folder();
        true
    }

    /// The tools' settings, as the settings file holds them.
    fn tool_settings(&self) -> ToolSettings {
        self.prefs.borrow().base.file_tools.clone()
    }

    fn set_tool_settings(&self, s: ToolSettings) {
        self.prefs.borrow_mut().base.file_tools = s;
        self.settings_changed();
    }

    /// Whether a tool may open its dialog: none of goz-gui's dialogs is up.
    pub(super) fn may_open_dialog(&self) -> bool {
        !self.file_tools.dialog.get()
            && !self.bookmark_dialog.get()
            && !self.run_history_dialog_open()
            && !self.search_history_dialog_open()
    }

    /// Hands `task` to the file operations thread; false (with a beep) if
    /// it could not be.
    fn run_task(&self, steps: Vec<Step>, checked: bool, purpose: Purpose) -> bool {
        if steps.is_empty() {
            return false;
        }
        let ok = self.fileops.run(Job::Tool(Task {
            steps,
            checked,
            purpose,
        }));
        if !ok {
            beep();
        }
        ok
    }

    /// The selection's items for a tool, fetched first when some are not
    /// held (then the tool runs again once they are, and this is `None`).
    fn tool_targets(&self, id: u16) -> Option<Vec<crate::actions::Target>> {
        if self.fetch_first(Then::FileTool(id)) {
            return None;
        }
        let (targets, selected) = self.picked_targets();
        if targets.is_empty() {
            beep();
            return None;
        }
        self.record_search();
        self.note_partial(Act::Command, targets.len(), selected);
        Some(targets)
    }

    /// Edit > Copy to Folder... and Move to Folder... (`moving`): asks for
    /// the folder, opening where the last one was chosen, then copies or
    /// moves the selection there as Explorer does (its questions when a
    /// name is taken; a copy into the item's own folder named "- Copy").
    fn copy_move_to_folder(&self, moving: bool) {
        if !self.may_open_dialog() {
            return;
        }
        let id = if moving {
            cmd::MOVE_TO_FOLDER
        } else {
            cmd::COPY_TO_FOLDER
        };
        let Some(targets) = self.tool_targets(id) else {
            return;
        };
        let mut s = self.tool_settings();
        let start: Vec<u16> = if s.last_folder.is_empty() {
            parent(&targets[0].path)
                .map(<[u16]>::to_vec)
                .unwrap_or_default()
        } else {
            s.last_folder.encode_utf16().collect()
        };
        let (title, ok) = if moving {
            ("Move To", "Move")
        } else {
            ("Copy To", "Copy")
        };
        self.file_tools.dialog.set(true);
        let folder = {
            let _quiet = Quiet::begin();
            folder_pick::pick(self.hwnd, title, ok, Some(&start))
        };
        self.file_tools.dialog.set(false);
        let Some(folder) = folder else { return };
        s.last_folder = text(&folder);
        self.set_tool_settings(s);
        let steps = if moving {
            // A move into the folder an item is in already is nothing to
            // do, as a paste there; what a moved folder holds goes with it.
            let items: Vec<(&[u16], Option<&[u16]>)> =
                targets.iter().map(|t| (t.path.as_slice(), None)).collect();
            let plan = crate::filetools::plan_moves(&items, &folder);
            self.note_with_folders(plan.with_folders);
            plan.steps
                .into_iter()
                .map(|(_, path)| Step::Move {
                    path,
                    folder: folder.clone(),
                    name: None,
                })
                .collect()
        } else {
            targets
                .into_iter()
                .map(|t| Step::Copy {
                    path: t.path,
                    folder: folder.clone(),
                    name: None,
                })
                .collect()
        };
        let action = if moving { Action::Move } else { Action::Copy };
        self.run_task(steps, false, Purpose::Do(action));
    }

    /// Edit > Advanced > Advanced Rename... (Shift+F2, F2 with several rows
    /// selected): Everything's renamer on the selection.
    pub(super) fn advanced_rename(&self) {
        if !self.may_open_dialog() {
            return;
        }
        let Some(targets) = self.tool_targets(cmd::ADVANCED_RENAME) else {
            return;
        };
        let files: Vec<(Vec<u16>, bool)> = targets
            .into_iter()
            .filter(|t| super::files::renamable(&t.path))
            .map(|t| (t.path, t.is_dir))
            .collect();
        if files.is_empty() {
            beep();
            return;
        }
        let mut s = self.tool_settings();
        let start = rename_dialog::Start {
            options: s.rename,
            old_history: s.old_history.clone(),
            new_history: s.new_history.clone(),
            mode: rename_dialog::Mode::Rename,
            folder: String::new(),
            presets: s.presets.clone(),
        };
        self.file_tools.dialog.set(true);
        let (chosen, presets) = {
            let _quiet = Quiet::begin();
            rename_dialog::run(self.hwnd, &files, start, self.dialog_look())
        };
        self.file_tools.dialog.set(false);
        super::runs::give_back_memory();
        if let Some(presets) = presets {
            s.presets = presets;
            self.set_tool_settings(s.clone());
        }
        let Some(chosen) = chosen else { return };
        s.rename = chosen.options;
        remember(&mut s.old_history, &chosen.old_format);
        remember(&mut s.new_history, &chosen.new_format);
        self.set_tool_settings(s);
        let steps = chosen
            .renames
            .into_iter()
            .map(|(path, name)| Step::Rename { path, name })
            .collect();
        self.run_task(steps, false, Purpose::Do(Action::Rename));
    }

    /// Edit > Advanced > Advanced Copy to Folder... and Advanced Move to
    /// Folder... (`moving`, wave 4): the renamer with a To folder; each
    /// selected item is copied or moved there under its new name, as one
    /// operation (one undo record: Undo puts moved items back where they
    /// were, under their old names, and sends copies to the Recycle Bin).
    fn advanced_copy_move(&self, moving: bool) {
        if !self.may_open_dialog() {
            return;
        }
        let id = if moving {
            ADVANCED_MOVE_TO
        } else {
            ADVANCED_COPY_TO
        };
        let Some(targets) = self.tool_targets(id) else {
            return;
        };
        let files: Vec<(Vec<u16>, bool)> =
            targets.into_iter().map(|t| (t.path, t.is_dir)).collect();
        let mut s = self.tool_settings();
        let formats = if moving {
            s.move_to.clone()
        } else {
            s.copy_to.clone()
        };
        let start = rename_dialog::Start {
            options: formats.options,
            old_history: formats.old_history.clone(),
            new_history: formats.new_history.clone(),
            mode: if moving {
                rename_dialog::Mode::Move
            } else {
                rename_dialog::Mode::Copy
            },
            folder: s.last_folder.clone(),
            presets: s.presets.clone(),
        };
        self.file_tools.dialog.set(true);
        let (chosen, presets) = {
            let _quiet = Quiet::begin();
            rename_dialog::run(self.hwnd, &files, start, self.dialog_look())
        };
        self.file_tools.dialog.set(false);
        super::runs::give_back_memory();
        if let Some(presets) = presets {
            s.presets = presets;
        }
        let Some(chosen) = chosen else {
            self.set_tool_settings(s);
            return;
        };
        let mut formats = formats;
        formats.options = chosen.options;
        remember(&mut formats.old_history, &chosen.old_format);
        remember(&mut formats.new_history, &chosen.new_format);
        if moving {
            s.move_to = formats;
        } else {
            s.copy_to = formats;
        }
        s.last_folder = text(&chosen.folder);
        self.set_tool_settings(s);
        let folder = chosen.folder;
        // Its own name, as Copy and Move to Folder give it (the dialog
        // refused a copy into its own folder under it, and left out a move
        // into its own folder under it).
        let new_name = |path: &[u16], name: &[u16]| {
            (crate::filetools::name_of(path) != name).then(|| name.to_vec())
        };
        let steps = if moving {
            let items: Vec<(&[u16], Option<Vec<u16>>)> = chosen
                .renames
                .iter()
                .map(|(path, name)| (path.as_slice(), new_name(path, name)))
                .collect();
            let refs: Vec<(&[u16], Option<&[u16]>)> =
                items.iter().map(|(p, n)| (*p, n.as_deref())).collect();
            let plan = crate::filetools::plan_moves(&refs, &folder);
            self.note_with_folders(plan.with_folders);
            plan.steps
                .into_iter()
                .map(|(i, path)| Step::Move {
                    path,
                    folder: folder.clone(),
                    name: items[i].1.clone(),
                })
                .collect()
        } else {
            chosen
                .renames
                .into_iter()
                .map(|(path, name)| Step::Copy {
                    name: new_name(&path, &name),
                    path,
                    folder: folder.clone(),
                })
                .collect()
        };
        let action = if moving { Action::Move } else { Action::Copy };
        self.run_task(steps, false, Purpose::Do(action));
    }

    /// W5: says how many of the items a move takes go with a selected
    /// folder that holds them (`filetools::plan_moves`), when any do.
    fn note_with_folders(&self, n: usize) {
        if n == 0 {
            return;
        }
        self.show_note(if n == 1 {
            "1 item inside a selected folder moves with its folder".into()
        } else {
            format!(
                "{} items inside selected folders move with their folders",
                crate::format::grouped(n as u64)
            )
        });
    }

    /// Edit > Advanced > Set File Time...: asks what each time becomes,
    /// then sets them on every selected item.
    fn set_file_time(&self) {
        if !self.may_open_dialog() {
            return;
        }
        let Some(targets) = self.tool_targets(cmd::SET_FILE_TIME) else {
            return;
        };
        let mut s = self.tool_settings();
        self.file_tools.dialog.set(true);
        let chosen = {
            let _quiet = Quiet::begin();
            times_dialog::run(
                self.hwnd,
                s.time_kinds,
                s.time_custom.clone(),
                self.dialog_look(),
            )
        };
        self.file_tools.dialog.set(false);
        let Some(chosen) = chosen else { return };
        s.time_kinds = chosen.kinds;
        s.time_custom = chosen.custom;
        self.set_tool_settings(s);
        let steps = targets
            .into_iter()
            .map(|t| Step::Times {
                path: t.path,
                set: chosen.set,
            })
            .collect();
        self.run_task(steps, false, Purpose::Do(Action::SetFileTime));
    }

    /// File > Create Shortcut: a shortcut beside each selected item
    /// ("name - Shortcut.lnk"), or on the Desktop where the folder does not
    /// let one be made (asked first, as Explorer does).
    fn create_shortcut(&self) {
        let Some(targets) = self.tool_targets(cmd::CREATE_SHORTCUT) else {
            return;
        };
        let steps = targets
            .into_iter()
            .map(|t| Step::Shortcut {
                target: t.path,
                link: None,
                desktop: None,
            })
            .collect();
        self.run_task(steps, false, Purpose::Do(Action::CreateShortcut));
    }

    /// Edit > Advanced > New Folder (and the empty space's menu): a folder
    /// "New folder" (numbered when taken) in the selected folder, or the
    /// folder of the focused item; once its row shows, it is renamed in
    /// place.
    fn new_folder(&self) {
        let Some(folder) = self.new_folder_target() else {
            beep();
            return;
        };
        let steps = vec![Step::NewFolder {
            folder,
            name: NEW_FOLDER_NAME.encode_utf16().collect(),
            exact: false,
        }];
        self.run_task(steps, false, Purpose::Do(Action::NewFolder));
    }

    /// Where New Folder makes its folder: in the selected folder when it is
    /// the only row selected, else in the folder of the focused row (or
    /// the first selected one); `None` with neither held.
    fn new_folder_target(&self) -> Option<Vec<u16>> {
        let selected = self.selected(2);
        let focused = usize::try_from(send(
            self.list,
            LVM_GETNEXTITEM,
            usize::MAX,
            LVNI_FOCUSED as isize,
        ))
        .ok();
        let m = self.model.borrow();
        if let [only] = selected[..]
            && let Some(row) = m.peek(only)
            && row.is_dir
        {
            return Some(row.path.to_vec());
        }
        let row = focused
            .and_then(|i| m.peek(i))
            .or_else(|| selected.first().and_then(|&i| m.peek(i)))?;
        parent(&row.path).map(<[u16]>::to_vec)
    }

    /// Whether New Folder has a folder to make one in.
    pub(super) fn can_make_folder(&self) -> bool {
        self.new_folder_target().is_some()
    }

    /// A new folder was made at `path`: its row is looked for, to rename
    /// it in place.
    fn await_new_folder(&self, path: Vec<u16>) {
        *self.file_tools.new_folder.borrow_mut() = Some(NewFolder {
            path,
            search: crate::wide::window_text(self.edit),
            left: NEW_FOLDER_TRIES,
        });
        // SAFETY: a timer of this window.
        unsafe { SetTimer(self.hwnd, TIMER_NEW_FOLDER, NEW_FOLDER_EVERY_MS, None) };
    }

    /// The new folder's row is looked for (searching again now and then:
    /// gozd may see it late); once it is there it is selected and renamed
    /// in place, as Explorer does, unless the search changed meanwhile.
    /// After a few seconds without it the status bar says where it was
    /// made: a search that does not list it never will.
    fn look_for_new_folder(&self) {
        let Some(mut nf) = self.file_tools.new_folder.borrow_mut().take() else {
            return;
        };
        if crate::wide::window_text(self.edit) != nf.search {
            return;
        }
        if self.model.borrow().find(&nf.path).is_some() {
            self.select_file(&nf.path);
            self.rename_focused();
            return;
        }
        nf.left = nf.left.saturating_sub(1);
        if nf.left > 0 {
            if nf.left % NEW_FOLDER_REFRESH == 0 {
                self.start_search(true);
            }
            *self.file_tools.new_folder.borrow_mut() = Some(nf);
            // SAFETY: a timer of this window.
            unsafe { SetTimer(self.hwnd, TIMER_NEW_FOLDER, NEW_FOLDER_EVERY_MS, None) };
            return;
        }
        let folder = parent(&nf.path).map(text).unwrap_or_default();
        self.show_note(format!(
            "Made \"{}\" in {folder}; it is not among the results of this search",
            text(crate::filetools::name_of(&nf.path))
        ));
    }

    /// Edit > Undo (Ctrl+Z, Alt+Backspace in the results): undoes the last
    /// operation recorded.
    fn undo_last(&self) {
        let Some(rec) = self.file_tools.stack.borrow_mut().take_undo() else {
            beep();
            return;
        };
        self.start_undo(rec);
    }

    fn start_undo(&self, rec: Record) {
        let steps = rec.undo_steps();
        let job = Job::Tool(Task {
            steps,
            checked: true,
            purpose: Purpose::Undo(rec.clone()),
        });
        if !self.fileops.run(job) {
            beep();
            self.file_tools.stack.borrow_mut().undone(
                Record {
                    items: Vec::new(),
                    ..rec.clone()
                },
                rec,
            );
        }
        self.undo_changed();
    }

    /// Edit > Redo (Ctrl+Y): does again what was undone last.
    fn redo_last(&self) {
        let Some(rec) = self.file_tools.stack.borrow_mut().take_redo() else {
            beep();
            return;
        };
        let steps = rec.redo_steps();
        let job = Job::Tool(Task {
            steps,
            checked: true,
            purpose: Purpose::Redo(rec.clone()),
        });
        if !self.fileops.run(job) {
            beep();
            self.file_tools.stack.borrow_mut().redone(
                Record {
                    items: Vec::new(),
                    ..rec.clone()
                },
                rec,
            );
        }
        self.undo_changed();
    }

    /// Edit > Undo History... (Ctrl+Shift+Z): every operation that can be
    /// undone, an item a row; its Undo undoes the selected ones.
    fn undo_history(&self) {
        if !self.may_open_dialog() {
            return;
        }
        self.file_tools.dialog.set(true);
        {
            let _quiet = Quiet::begin();
            undo_dialog::run(self.hwnd, self, self.dialog_look());
        }
        self.file_tools.dialog.set(false);
        self.file_tools.history.set(core::ptr::null_mut());
        super::runs::give_back_memory();
    }

    /// The record changed: an Undo History that is open shows it again.
    fn undo_changed(&self) {
        let dlg = self.file_tools.history.get();
        if !dlg.is_null() {
            // SAFETY: posts to this thread's dialog, which ignores it once
            // gone.
            unsafe { PostMessageW(dlg, undo_dialog::WM_REFRESH, 0, 0) };
        }
    }

    /// A Delete, Paste or Rename finished: what it did goes into the undo
    /// record, as Everything's do.
    pub(super) fn record_file_op(&self, job: &Job, log: &[Change]) {
        let (action, kind) = match job {
            Job::Delete {
                permanent: false, ..
            } => (Action::Delete, Kind::Delete),
            Job::Paste { moving: true, .. } => (Action::Move, Kind::Move),
            Job::Paste { moving: false, .. } => (Action::Copy, Kind::Copy),
            Job::Rename { .. } => (Action::Rename, Kind::Rename),
            _ => return,
        };
        let items: Vec<Item> = log
            .iter()
            .filter(|c| c.kind == kind)
            .filter_map(|c| {
                let to = c.to.clone().filter(|t| !t.is_empty())?;
                Some(Item {
                    from: c.from.clone(),
                    to,
                    before: None,
                    after: None,
                })
            })
            .collect();
        self.push_undo(action, items);
    }

    fn push_undo(&self, action: Action, items: Vec<Item>) {
        let pushed = self
            .file_tools
            .stack
            .borrow_mut()
            .push(action, filetime_now(), items);
        if pushed.is_some() {
            self.undo_changed();
        }
    }

    /// A file tool's task finished (`WM_FILEOP`).
    pub(super) fn tool_done(&self, task: Task, outcome: Outcome, results: Vec<StepResult>) {
        let sources: Vec<Vec<u16>> = task.steps.iter().map(|s| s.source().to_vec()).collect();
        match task.purpose {
            Purpose::Do(action) => {
                let mut items = undo::items_of(action, &sources, &results);
                if action == Action::Rename {
                    undo::settle_renames(&mut items);
                }
                match action {
                    Action::Rename => self.show_renames(&task.steps, &results),
                    Action::NewFolder => {
                        if let Some(first) = items.first() {
                            self.await_new_folder(first.to.clone());
                        }
                    }
                    Action::CreateShortcut => self.shortcuts_denied(&task.steps, &results),
                    _ => {}
                }
                self.push_undo(action, items);
                let what = match action {
                    Action::Rename => "rename",
                    Action::Move => "move",
                    Action::Copy => "copy",
                    Action::Delete => "delete",
                    Action::NewFolder => "make",
                    Action::SetFileTime => "set the time of",
                    Action::CreateShortcut => "make a shortcut to",
                };
                match (&outcome, trouble(what, &results)) {
                    (Outcome::Cancelled, _) => self.show_note(format!(
                        "{} stopped: not every item was done",
                        action.label()
                    )),
                    (_, Some(note)) => self.show_note(note),
                    (Outcome::Failed(why), None) => {
                        self.show_note(format!("Could not {what}: {why}"))
                    }
                    _ => {
                        if let Some(note) = landed(&results) {
                            self.show_note(note);
                        }
                    }
                }
            }
            Purpose::Undo(rec) => {
                if rec.action == Action::Rename {
                    // The undo's steps renamed `to` back to `from`.
                    self.show_renames(&task.steps, &results);
                }
                let (did, left) = rec.split_undone(&results);
                self.undo_note("Undid", "undo", &did, &left, &results);
                self.file_tools.stack.borrow_mut().undone(did, left);
                self.undo_changed();
            }
            Purpose::Redo(rec) => {
                if rec.action == Action::Rename {
                    self.show_renames(&task.steps, &results);
                }
                let rename = rec.action == Action::Rename;
                let (mut did, left) = rec.split_redone(&results);
                if rename {
                    undo::settle_renames(&mut did.items);
                }
                self.undo_note("Redid", "redo", &did, &left, &results);
                self.file_tools.stack.borrow_mut().redone(did, left);
                self.undo_changed();
            }
        }
        self.settle_refresh();
        // What the shell's operation of many items took is freed.
        super::runs::give_back_memory();
    }

    /// What the status bar says after an undo or redo.
    fn undo_note(
        &self,
        did: &str,
        verb: &str,
        done: &Record,
        left: &Record,
        results: &[StepResult],
    ) {
        if left.items.is_empty() {
            if !done.items.is_empty() {
                self.show_note(undo::describe(did, done));
            }
            return;
        }
        let note = trouble(verb, results).unwrap_or_else(|| {
            format!(
                "{} stopped: {} items were not done",
                if verb == "undo" { "Undo" } else { "Redo" },
                left.items.len()
            )
        });
        self.show_note(note);
    }

    /// Renames done show in their rows at once, as Rename in place's do;
    /// the search after confirms them.
    fn show_renames(&self, steps: &[Step], results: &[StepResult]) {
        let mut first = usize::MAX;
        let mut last = 0;
        for (step, r) in steps.iter().zip(results) {
            let (Step::Rename { path, .. }, StepResult::Done { to, .. }) = (step, r) else {
                continue;
            };
            let changed = self.model.borrow_mut().rename(path, to);
            for i in changed {
                first = first.min(i);
                last = last.max(i);
            }
        }
        if first <= last {
            send(self.list, LVM_REDRAWITEMS, first, last as isize);
        }
    }

    /// Shortcuts a folder would not take: asks (Explorer's question) to
    /// put them on the Desktop instead.
    fn shortcuts_denied(&self, steps: &[Step], results: &[StepResult]) {
        let denied: Vec<Vec<u16>> = steps
            .iter()
            .zip(results)
            .filter(|(_, r)| **r == StepResult::Denied)
            .map(|(s, _)| s.source().to_vec())
            .collect();
        if denied.is_empty() {
            return;
        }
        let answer = {
            let _quiet = Quiet::begin();
            msgbox::show(
                self.hwnd,
                SHORTCUT_HERE,
                "Shortcut",
                MB_YESNO | MB_ICONQUESTION,
            )
        };
        if answer != IDYES {
            return;
        }
        let Some(desk) = desktop() else {
            beep();
            return;
        };
        let steps = denied
            .into_iter()
            .map(|target| Step::Shortcut {
                target,
                link: None,
                desktop: Some(desk.clone()),
            })
            .collect();
        self.run_task(steps, false, Purpose::Do(Action::CreateShortcut));
    }

    /// The empty space's menu: New Folder after Refresh.
    pub(super) fn add_background_items(&self, popup: HMENU) {
        menu::add_items(
            popup,
            Some(menu::REFRESH),
            &[MenuItem::Sep, MenuItem::Cmd(cmd::NEW_FOLDER, "Ne&w Folder")],
        );
        set_item(
            popup,
            cmd::NEW_FOLDER,
            "Ne&w Folder",
            self.can_make_folder(),
        );
    }

    /// F2 (File > Rename): with several rows selected, Advanced Rename, as
    /// in Everything.
    pub(super) fn rename_many(&self) -> bool {
        if send(self.list, LVM_GETSELECTEDCOUNT, 0, 0) < 2 {
            return false;
        }
        self.advanced_rename();
        true
    }
}

impl undo_dialog::Host for App {
    fn records(&self) -> Vec<Record> {
        self.file_tools.stack.borrow().done().to_vec()
    }

    fn opened(&self, dlg: HWND) {
        self.file_tools.history.set(dlg);
    }

    fn undo_items(&self, picks: &[(u64, usize)]) {
        let taken = self.file_tools.stack.borrow_mut().take_items(picks);
        for rec in taken {
            self.start_undo(rec);
        }
    }

    fn sort(&self) -> (u8, bool) {
        let s = &self.prefs.borrow().base.file_tools;
        (s.undo_sort, s.undo_ascending)
    }

    fn set_sort(&self, column: u8, ascending: bool) {
        let mut s = self.tool_settings();
        if (s.undo_sort, s.undo_ascending) != (column, ascending) {
            s.undo_sort = column;
            s.undo_ascending = ascending;
            self.set_tool_settings(s);
        }
    }
}

// The commands are the feature's own range, clear of every other.
const _: () = assert!(cmd::UNDO >= menu::F5_FIRST && cmd::LAST < menu::F6_FIRST);
