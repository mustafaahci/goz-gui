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
use crate::filelist::slots::{self, Func, Slots, Term, cmd};
use crate::places::{self, Which};
use crate::slotedit;

/// W7: the most results `save-filelistN:` puts in a slot (each is kept in
/// `Session.json` too).
pub const SAVE_MAX: usize = 100_000;

/// W7: what a `save-filelistN:` search's save thread hands the window: the
/// search, its slots, and its results' full paths (or why not).
pub struct Saved {
    query: String,
    slots: Vec<usize>,
    names: Result<Vec<String>, String>,
}

/// `save-filelist1:` (and `save-filelist2:` ...), as notes name them.
fn slots_text(saves: &[usize]) -> String {
    saves
        .iter()
        .map(|n| format!("save-filelist{n}:"))
        .collect::<Vec<_>>()
        .join(" and ")
}

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
        self.put_in_slot(n, names);
        if count < selected {
            self.note_partial(Act::Command, count, selected);
        } else {
            self.show_note(format!(
                "File list slot {n} holds {} (filelist{n}: finds them)",
                crate::listedit::items_text(count)
            ));
        }
    }

    /// Slot `n` of the shown tab holds `names` from now on: a search that
    /// reads it runs again (not one that only saves to it), and the
    /// session keeps it.
    fn put_in_slot(&self, n: usize, names: Vec<String>) {
        let mut now = (*self.file_list_slots()).clone();
        now.set(n, names);
        *self.data_tools.slots.borrow_mut() = Arc::new(now);
        // A search that reads the slot finds what it holds now.
        let text = crate::wide::window_text(self.edit);
        if slots::terms(&text)
            .iter()
            .any(|t| t.slot == Some(n) && t.func != Func::Save)
        {
            self.start_search(true);
        }
        self.session_timer();
    }

    /// W7's WM_APP messages (`WM_APP + 380..390`).
    pub(super) fn w7_message(&self, msg: u32, w: WPARAM, l: LPARAM) {
        match msg {
            slotedit::WM_EDIT_TERM_AT => {
                let text = crate::wide::window_text(self.edit);
                if let Some(term) = slots::term_at(&text, w) {
                    self.edit_file_list_term(term);
                }
            }
            slotedit::WM_SLOT_SAVED if l != 0 => {
                // SAFETY: the box the save thread posted, ours once
                // delivered.
                let saved = unsafe { Box::from_raw(l as *mut Saved) };
                self.slots_saved(*saved);
            }
            _ => {}
        }
    }

    /// W7: Edit File List Slot... (the search box's menu, Ctrl+click or F2
    /// on a file-list term): the slot's names, or the term's own, edited;
    /// on OK they go in the slot, or in the search.
    pub(super) fn edit_file_list_term(&self, term: Term) {
        let text = crate::wide::window_text(self.edit);
        let (title, names) = match term.slot {
            Some(n) => (
                format!("Edit File List {n}"),
                self.file_list_slots().get(n).to_vec(),
            ),
            None => (
                "Edit File List".to_owned(),
                slots::names(&term, &Slots::default()).unwrap_or_default(),
            ),
        };
        let kept = self.prefs.borrow().base.file_lists.slot.clone();
        let outcome = {
            let _quiet = crate::dialog::Quiet::begin();
            slotedit::edit(
                self.hwnd,
                slotedit::Start {
                    title,
                    names: names.clone(),
                    look: self.dialog_look(),
                    kept: kept.clone(),
                },
            )
        };
        if outcome.kept != kept {
            self.prefs.borrow_mut().base.file_lists.slot = outcome.kept;
            self.settings_changed();
        }
        let Some(now) = outcome.names else {
            return;
        };
        if now == names {
            return;
        }
        match term.slot {
            Some(n) => {
                let count = now.len();
                self.put_in_slot(n, now);
                self.show_note(format!(
                    "File list slot {n} holds {}",
                    crate::listedit::items_text(count)
                ));
            }
            None => match slots::with_names(&text, &term, &now) {
                Some(new) => self.set_search_text(&new),
                None => {
                    beep();
                    self.show_note(format!(
                        "A name holding ; or \" cannot be written after {}; put the names in \
                         a file list slot instead (filelist1:)",
                        term.name
                    ));
                }
            },
        }
    }

    /// W7: a search was made (not a refresh of it) and its first rows came:
    /// its `save-filelistN:` terms put all its results in those slots, read
    /// on a thread of its own (a search refused for reading a slot it saves
    /// never gets here).
    pub(super) fn save_to_slots(&self, total: u64) {
        let (search, saves) = {
            let m = self.model.borrow();
            let Ok(saves) = slots::saves(&m.search.query) else {
                return;
            };
            if saves.is_empty() {
                return;
            }
            let s = &m.search;
            (
                crate::client::Search {
                    query: s.query.clone(),
                    sort: s.sort,
                    match_case: s.match_case,
                    filelist: s.filelist.clone(),
                    created_sort: s.created_sort,
                    slots: Arc::clone(&s.slots),
                },
                saves,
            )
        };
        let names = slots_text(&saves);
        if total > SAVE_MAX as u64 {
            beep();
            self.show_note(format!(
                "{names} saves at most {} results, and this search has {}: the slot is as it was",
                crate::format::grouped(SAVE_MAX as u64),
                crate::format::grouped(total)
            ));
            return;
        }
        // One save at a time: a newer search's replaces an older one's.
        if let Some(stop) = self.data_tools.saving.borrow_mut().take() {
            stop.stop();
        }
        let stop = crate::client::Stop::new();
        *self.data_tools.saving.borrow_mut() = Some(stop.clone());
        let hwnd = self.hwnd as usize;
        let spawned = std::thread::Builder::new()
            .name("goz-save-filelist".into())
            .spawn(move || {
                let mut paths = Vec::new();
                let range = crate::client::RowRange {
                    offset: 0,
                    limit: None,
                    details: false,
                };
                let streamed = crate::client::stream_range(&stop, &search, range, |rows, _| {
                    paths.extend(rows.iter().map(|r| String::from_utf16_lossy(&r.path)));
                    Ok(paths.len() <= SAVE_MAX)
                });
                let names = match streamed {
                    crate::client::Streamed::Done(_) if paths.len() <= SAVE_MAX => Ok(paths),
                    crate::client::Streamed::Done(_) => Err(format!(
                        "it has more than {} results",
                        crate::format::grouped(SAVE_MAX as u64)
                    )),
                    crate::client::Streamed::Stopped => return,
                    crate::client::Streamed::Failed(f) => Err(match f {
                        crate::client::Failure::NotRunning => crate::client::not_running_text(),
                        crate::client::Failure::Untrusted => "gozd could not be verified".into(),
                        crate::client::Failure::Refused(m) | crate::client::Failure::Other(m) => m,
                    }),
                };
                let raw = Box::into_raw(Box::new(Saved {
                    query: search.query.clone(),
                    slots: saves,
                    names,
                }));
                // SAFETY: posts to the window, which takes the box; if it is
                // gone the box is taken back.
                unsafe {
                    if PostMessageW(hwnd as _, slotedit::WM_SLOT_SAVED, 0, raw as isize) == 0 {
                        drop(Box::from_raw(raw));
                    }
                }
            });
        if spawned.is_err() {
            self.data_tools.saving.borrow_mut().take();
        }
    }

    /// W7: a `save-filelistN:` search's results were read: they go in its
    /// slots while that search is still the one shown.
    fn slots_saved(&self, saved: Saved) {
        self.data_tools.saving.borrow_mut().take();
        let shown = self.model.borrow().search.query == saved.query;
        if !shown {
            return;
        }
        let what = slots_text(&saved.slots);
        match saved.names {
            Ok(names) => {
                let count = names.len();
                for &n in &saved.slots {
                    self.put_in_slot(n, names.clone());
                }
                self.show_note(format!(
                    "{what} saved {} in file list slot{} {}",
                    crate::listedit::items_text(count),
                    if saved.slots.len() == 1 { "" } else { "s" },
                    saved
                        .slots
                        .iter()
                        .map(usize::to_string)
                        .collect::<Vec<_>>()
                        .join(", ")
                ));
            }
            Err(why) => {
                beep();
                self.show_note(format!("{what} could not save the results: {why}"));
            }
        }
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
