//! Wave 6 (stream C) from the window's side (the work is
//! `crate::filelist::slots` and `crate::listedit`):
//!
//! - the shown tab's ten file-list slots, which `filelistN:` searches for
//!   and every search the window makes carries (`client::Search::slots`);
//!   each tab keeps its own (`session::View::slots`, Everything's
//!   `filelist_slot0`..`9`);
//! - File | Set File List Slot 0..9 (keyboard commands, Everything's
//!   `file_set_file_list_slot_N_keys`, no key by default): the selected
//!   results' full paths go in the slot, and a search that reads it runs
//!   again;
//! - the File List Editor's start (Tools > File List Editor..., `-edit`,
//!   `-filelist-editor`) with its kept place and Export... choices, which
//!   it hands back as it moves and closes; and a goz-gui started for the
//!   editor alone ending with it.
//!
//! No `RefCell` of the window is borrowed while a dialog runs.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use windows_sys::Win32::Foundation::{LPARAM, RECT, WPARAM};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    GetWindowRect, IsWindowVisible, PostMessageW, WM_CLOSE,
};

use super::App;
use super::results::{Act, Then};
use crate::filelist::slots::{self, Slots, cmd};
use crate::places::{self, Which};

fn beep() {
    // SAFETY: plays a system sound.
    unsafe { windows_sys::Win32::System::Diagnostics::Debug::MessageBeep(0) };
}

impl App {
    /// The shown tab's file-list slots.
    pub(super) fn file_list_slots(&self) -> Arc<Slots> {
        Arc::clone(&self.data_tools.slots.borrow())
    }

    /// A tab shows (a tab switch, a restored session): its slots are the
    /// window's.
    pub(super) fn show_tab_slots(&self, slots: &Slots) {
        if **self.data_tools.slots.borrow() != *slots {
            *self.data_tools.slots.borrow_mut() = Arc::new(slots.clone());
        }
    }

    /// A wave-6 command (4850..=4899): File | Set File List Slot 0..9;
    /// false for another id.
    pub(super) fn lists_command(&self, id: u16) -> bool {
        match cmd::slot_of(id) {
            Some(n) => {
                self.set_file_list_slot(id, n);
                true
            }
            None => false,
        }
    }

    /// File | Set File List Slot `n`: the selected results' full paths (the
    /// rows held; those still to come are fetched first, as for the other
    /// commands) go in the slot, in place of what it held.
    fn set_file_list_slot(&self, id: u16, n: usize) {
        if self.fetch_first(Then::FileTool(id)) {
            return;
        }
        let (paths, selected) = self.picked_paths();
        if paths.is_empty() {
            beep();
            self.show_note(format!("Select the results to put in file list slot {n}"));
            return;
        }
        let names: Vec<String> = paths.iter().map(|p| String::from_utf16_lossy(p)).collect();
        let count = names.len();
        let mut now = (*self.file_list_slots()).clone();
        now.set(n, names);
        *self.data_tools.slots.borrow_mut() = Arc::new(now);
        if count < selected {
            self.note_partial(Act::Command, count, selected);
        } else {
            self.show_note(format!(
                "File list slot {n} holds {} (filelist{n}: finds them)",
                crate::listedit::items_text(count)
            ));
        }
        // A search that reads the slot finds what it holds now.
        let text = crate::wide::window_text(self.edit);
        if slots::terms(&text).iter().any(|t| t.slot == Some(n)) {
            self.start_search(true);
        }
        self.session_timer();
    }

    /// Wave 6's WM_APP messages (`WM_APP + 360..370`).
    pub(super) fn lists_message(&self, msg: u32, _w: WPARAM, _l: LPARAM) {
        if msg == crate::listedit::WM_EDITOR_CLOSED {
            self.editor_closed();
        }
    }

    /// Tools > File List Editor..., `-edit <file>` (`file`) and
    /// `-filelist-editor`: the editor, where it was last, in the window's
    /// look; one open comes to the front (and opens `file`).
    pub(super) fn open_list_editor(&self, file: Option<&str>) {
        let mut near = RECT::default();
        // SAFETY: reads this thread's window's place.
        unsafe { GetWindowRect(self.hwnd, &mut near) };
        let (export_type, export_selection) = {
            let p = self.prefs.borrow();
            let f = &p.base.file_lists;
            (f.editor_export_type, f.editor_export_selection)
        };
        let file =
            file.map(|f| std::path::absolute(Path::new(f)).unwrap_or_else(|_| PathBuf::from(f)));
        crate::listedit::ui::open(crate::listedit::ui::Start {
            look: self.dialog_look(),
            near,
            file,
            place: places::kept(Which::FileListEditor),
            export_type,
            export_selection,
            owner: self.hwnd as usize,
        });
    }

    /// The editor closed: what it handed back is kept; a goz-gui started
    /// for it alone, whose window was never shown, ends with it.
    fn editor_closed(&self) {
        self.take_editor_kept();
        // SAFETY: reads whether this thread's window shows.
        let shown = unsafe { IsWindowVisible(self.hwnd) } != 0;
        if self.data_tools.editor_alone.get() && !shown && !crate::listedit::ui::is_open() {
            // SAFETY: posts to this thread's window; with "run in the
            // background" WM_CLOSE keeps goz-gui running, as -startup does.
            unsafe { PostMessageW(self.hwnd, WM_CLOSE, 0, 0) };
        }
    }

    /// The editor's place and Export... choices, as it last handed them
    /// back, into the settings (as they are written too).
    pub(super) fn take_editor_kept(&self) {
        let Some(k) = crate::listedit::ui::take_kept() else {
            return;
        };
        if let Some(place) = k.place {
            places::put_place(Which::FileListEditor, place);
        }
        let changed = {
            let mut p = self.prefs.borrow_mut();
            let f = &mut p.base.file_lists;
            let changed = f.editor_export_type != k.export_type
                || f.editor_export_selection != k.export_selection;
            f.editor_export_type = k.export_type;
            f.editor_export_selection = k.export_selection;
            changed
        };
        if changed {
            self.settings_changed();
        }
    }

    /// The window starts for `-edit` or `-filelist-editor` and nothing
    /// else of it (`alone`: it was not shown): the editor opens.
    pub(super) fn start_list_editor(&self, file: Option<&str>, alone: bool) {
        self.data_tools.editor_alone.set(alone);
        self.open_list_editor(file);
    }
}
