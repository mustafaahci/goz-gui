//! The results as files: Delete and Shift+Delete, Paste, Rename in place
//! (F2), dragging rows out to Explorer or any program, and files dropped on
//! the window, which become the search. The shell does the work:
//! `crate::fileops` runs Delete, Paste and Rename on a thread of its own
//! (Windows' own questions, progress and undo, owned by this window), and
//! `crate::shell` the drag.
//!
//! The list never takes a row out by itself. Its rows are gozd's sorted
//! result, fetched in blocks by their place in it: dropping one here would
//! shift every row after it while the blocks still to come are placed by
//! gozd's numbering, so the wrong files would show (and be acted on) until
//! the next search. After a change the search runs again instead, once
//! gozd has seen it (it reads the volume's journal within a moment): rows
//! deleted or moved away go, pasted ones come. A rename also shows at once:
//! the renamed row (and those of what a renamed folder holds) shows the new
//! path in place until that search confirms it.

use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows_sys::Win32::Storage::FileSystem::{
    FILE_ATTRIBUTE_DIRECTORY, GetFileAttributesW, INVALID_FILE_ATTRIBUTES,
};
use windows_sys::Win32::System::Diagnostics::Debug::MessageBeep;
use windows_sys::Win32::UI::Controls::{
    EM_GETMARGINS, EM_SETSEL, LVIR_BOUNDS, LVIS_FOCUSED, LVIS_SELECTED, LVM_EDITLABELW,
    LVM_ENSUREVISIBLE, LVM_GETEDITCONTROL, LVM_GETITEMRECT, LVM_GETNEXTITEM, LVM_GETSELECTEDCOUNT,
    LVM_REDRAWITEMS, LVM_SETSELECTIONMARK, LVNI_FOCUSED,
};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, GetFocus, SetFocus, VK_SHIFT,
};
use windows_sys::Win32::UI::Shell::{
    DefSubclassProc, DragFinish, HDROP, RemoveWindowSubclass, SetWindowSubclass,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    MB_ICONWARNING, MB_OK, PostMessageW, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SWP_NOZORDER,
    SetTimer, SetWindowPos, SetWindowTextW, WINDOWPOS, WM_NCDESTROY, WM_WINDOWPOSCHANGING,
};

use super::{
    App, COL_NAME, TIMER_SETTLE, WM_DRAG_DONE, WM_RENAME_AGAIN, key_down,
    results::{Act, Then},
    send,
};
use crate::dialog::Quiet;
use crate::fileops::{self, Done, Job, Outcome};
use crate::wide::{nul, window_text, wz};
use crate::{clipboard, msgbox, paint, shell, theme};

const BACKSLASH: u16 = b'\\' as u16;

/// After a change to the files, the search runs again this long after it
/// (gozd sees most changes within it)...
const SETTLE_FIRST_MS: u32 = 700;
/// ...and once more this long after that, for a change it saw late.
const SETTLE_AGAIN_MS: u32 = 2000;

/// The subclass id of Rename's box.
const RENAME_BOX: usize = 0x524e;

// What Rename says of a name it refuses: Explorer's words.
const EMPTY: &str = "You must type a file name.";
const BAD_CHARS: &str =
    "A file name can't contain any of the following characters:\n\\ / : * ? \" < > |";
const TRAILING: &str = "A file name can't end with a space or a period.";
const RESERVED: &str = "The specified device name is invalid.";
const FILE_EXISTS: &str = "There is already a file with the same name in this location.";
const FOLDER_EXISTS: &str = "There is already a folder with the same name in this location.";

/// Why `name`, typed for a rename, cannot be a file's name; `None` if it
/// can. Empty, a character Windows refuses (or a control character), a
/// trailing space or dot (Windows would drop it), or a device name (`CON`,
/// `nul.txt`, `COM1`…).
fn name_problem(name: &[u16]) -> Option<&'static str> {
    let text = String::from_utf16_lossy(name);
    if text.trim().is_empty() {
        return Some(EMPTY);
    }
    if text
        .chars()
        .any(|c| "\\/:*?\"<>|".contains(c) || u32::from(c) < 0x20)
    {
        return Some(BAD_CHARS);
    }
    if text.ends_with([' ', '.']) {
        return Some(TRAILING);
    }
    let base = text.split('.').next().unwrap_or("").trim_end();
    let upper = base.to_ascii_uppercase();
    let device = matches!(upper.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || (upper.len() == 4
            && (upper.starts_with("COM") || upper.starts_with("LPT"))
            && matches!(upper.as_bytes()[3], b'1'..=b'9'));
    device.then_some(RESERVED)
}

/// How much of `name` Rename selects at first: a file's name without its
/// extension (up to its last dot, unless the name starts with it), or all
/// of a folder's, as Explorer does.
fn stem_len(name: &[u16], is_dir: bool) -> usize {
    if is_dir {
        return name.len();
    }
    match name.iter().rposition(|&c| c == u16::from(b'.')) {
        Some(i) if i > 0 => i,
        _ => name.len(),
    }
}

/// Where `path`'s name starts (after its last backslash).
fn name_start(path: &[u16]) -> usize {
    path.iter()
        .rposition(|&c| c == BACKSLASH)
        .map_or(0, |i| i + 1)
}

/// Whether the item at `path` can be renamed in place: not a volume root
/// (`C:\`), which has no name of its own (its label is the drive's).
pub(super) fn renamable(path: &[u16]) -> bool {
    name_start(path) < path.len()
}

/// The search a drop makes of `item`, the path of what was dropped (a
/// folder's ending in a backslash, so the search is what it holds), in
/// quotes when it holds a space or goz's `!`. It takes the place of
/// `current`, or with `append` (Shift held) goes after it.
fn drop_search(current: &str, item: &str, is_dir: bool, append: bool) -> String {
    let mut path = item.to_owned();
    if is_dir && !path.ends_with('\\') {
        path.push('\\');
    }
    if path.chars().any(|c| c.is_whitespace() || c == '!') {
        path = format!("\"{path}\"");
    }
    if !append || current.trim().is_empty() {
        return path;
    }
    let gap = if current.ends_with(char::is_whitespace) {
        ""
    } else {
        " "
    };
    format!("{current}{gap}{path}")
}

/// How far `row_after` looks for a row that stays.
const NEXT_REACH: usize = 1000;

/// Whether `path` is inside the folder `dir`.
fn is_inside(path: &[u16], dir: &[u16]) -> bool {
    path.len() > dir.len()
        && path.starts_with(dir)
        && (dir.last() == Some(&BACKSLASH) || path[dir.len()] == BACKSLASH)
}

/// The row the selection moves to once the `selected` rows (in order) of
/// `count`, the files `gone`, are deleted: the first row after the last of
/// them that stays, or when none does, the nearest before the first, as in
/// Explorer. A row stays when it is held (`path` gives its path), not
/// selected, and not inside a deleted folder (a search lists a folder's
/// files after it).
fn row_after<'a>(
    selected: &[usize],
    count: usize,
    path: impl Fn(usize) -> Option<&'a [u16]>,
    gone: &[Vec<u16>],
) -> Option<usize> {
    let (&first, &last) = (selected.first()?, selected.last()?);
    let stays = |i: usize| {
        selected.binary_search(&i).is_err()
            && path(i).is_some_and(|p| !gone.iter().any(|g| is_inside(p, g)))
    };
    (last + 1..count)
        .take(NEXT_REACH)
        .chain((0..first).rev().take(NEXT_REACH))
        .find(|&i| stays(i))
}

/// Where a paste puts the files: into the row at `path` when it is a
/// folder and the only one selected (`only`), else into the folder that
/// holds it (`C:\` for a file at the root of C:).
fn paste_folder(path: &[u16], is_dir: bool, only: bool) -> Option<Vec<u16>> {
    if is_dir && only {
        return Some(path.to_vec());
    }
    fileops::parent(path).map(<[u16]>::to_vec)
}

/// Whether every one of `paths` is gone from where it was: a cut's files
/// once a paste moved them all.
fn all_gone(paths: &[Vec<u16>]) -> bool {
    paths.iter().all(|p| attributes(p).is_none())
}

/// The file system's attributes of `path`; `None` when it is gone.
fn attributes(path: &[u16]) -> Option<u32> {
    let path = nul(path);
    // SAFETY: a NUL-terminated path.
    let attrs = unsafe { GetFileAttributesW(path.as_ptr()) };
    (attrs != INVALID_FILE_ATTRIBUTES).then_some(attrs)
}

fn is_dir(path: &[u16]) -> bool {
    attributes(path).is_some_and(|a| a & FILE_ATTRIBUTE_DIRECTORY != 0)
}

fn beep() {
    // SAFETY: plays a system sound.
    unsafe { MessageBeep(MB_OK) };
}

/// A rename in progress, or about to start: the file, and after a refused
/// name, the text to start from again and why it was refused.
pub(super) struct Renaming {
    path: Vec<u16>,
    text: Option<Vec<u16>>,
    problem: Option<&'static str>,
}

impl App {
    /// Delete (to the Recycle Bin) or Shift+Delete (`permanent`): the
    /// selected rows that are held go to the file operations thread, which
    /// asks as Windows does. Once they are gone the selection moves to the
    /// row after them.
    pub(super) fn delete_selected(&self, permanent: bool) {
        if self.fetch_first(Then::Delete { permanent }) {
            return;
        }
        let held = self.selected_held();
        if held.is_empty() {
            return;
        }
        let selected = send(self.list, LVM_GETSELECTEDCOUNT, 0, 0).max(0) as usize;
        let (paths, next) = {
            let m = self.model.borrow();
            let path = |i: usize| m.peek(i).map(|r| &*r.path);
            let paths: Vec<Vec<u16>> = held
                .iter()
                .filter_map(|&i| path(i))
                .map(<[u16]>::to_vec)
                .collect();
            let next = row_after(&held, m.total as usize, path, &paths)
                .and_then(path)
                .map(<[u16]>::to_vec);
            (paths, next)
        };
        let done = paths.len();
        if self.fileops.run(Job::Delete {
            paths,
            permanent,
            next,
        }) {
            self.note_partial(Act::Delete, done, selected);
        } else {
            beep();
        }
    }

    /// The folder Paste puts files in: the selected folder when it is the
    /// only row selected, else the folder holding the focused (or first)
    /// selected row. `None` with nothing selected.
    fn paste_target(&self) -> Option<Vec<u16>> {
        let held = self.selected_held();
        let focused = send(
            self.list,
            LVM_GETNEXTITEM,
            usize::MAX,
            LVNI_FOCUSED as isize,
        );
        let at = usize::try_from(focused)
            .ok()
            .filter(|i| held.contains(i))
            .or_else(|| held.first().copied())?;
        let only = send(self.list, LVM_GETSELECTEDCOUNT, 0, 0) == 1;
        let m = self.model.borrow();
        let row = m.peek(at)?;
        paste_folder(&row.path, row.is_dir, only)
    }

    /// Edit > Paste has files to paste and a folder to put them in.
    pub(super) fn can_paste(&self) -> bool {
        clipboard::has_files() && !self.selected(1).is_empty()
    }

    /// Ctrl+V: the clipboard's files are copied (or, cut, moved) into the
    /// selected folder or the selected file's, on the file operations
    /// thread. A cut's data object is kept until the move is done, to be
    /// told of it (`shell::Cut`).
    pub(super) fn paste(&self) {
        let (Some(folder), Some(clip)) = (self.paste_target(), clipboard::files(self.hwnd)) else {
            beep();
            return;
        };
        let cut = clip
            .moving
            .then(|| shell::Cut::of_clipboard(clip.sequence))
            .flatten();
        let job = Job::Paste {
            sources: clip.paths,
            folder,
            moving: clip.moving,
            clipboard: clip.sequence,
        };
        if !self.fileops.run(job) {
            beep();
            return;
        }
        if let Some(cut) = cut {
            self.cuts.borrow_mut().push(cut);
        }
    }

    /// The data object kept for a paste of the cut read at clipboard
    /// sequence number `sequence`, now that it is over (any one: those of
    /// the same number are the same object).
    fn take_cut(&self, sequence: u32) -> Option<shell::Cut> {
        let mut cuts = self.cuts.borrow_mut();
        let i = cuts.iter().position(|c| c.sequence() == sequence)?;
        Some(cuts.swap_remove(i))
    }

    /// A file operation finished (`WM_FILEOP`).
    pub(super) fn file_op_done(&self, done: Done) {
        match done.job {
            Job::Delete { paths, next, .. } => {
                // Some went: the selection, if it is still on them, moves on.
                if paths.iter().any(|p| attributes(p).is_none())
                    && let Some(next) = next
                    && self.selection_within(&paths)
                {
                    self.select_file(&next);
                }
                if let Outcome::Failed(why) = &done.outcome {
                    self.show_note(format!("Could not delete: {why}"));
                }
                self.settle_refresh();
            }
            Job::Paste {
                sources,
                moving,
                clipboard,
                ..
            } => {
                let cut = if moving {
                    self.take_cut(clipboard)
                } else {
                    None
                };
                // The paste protocol of a cut, once every file moved (none
                // is left where it was, so nothing is there for the
                // program that cut them to delete): it hears so, through
                // its own data object while the clipboard still holds it,
                // and the clipboard no longer offers files that are gone.
                if moving && done.outcome == Outcome::Done && all_gone(&sources) {
                    if let Some(cut) = cut {
                        cut.moved();
                    }
                    clipboard::clear_after_move(self.hwnd, clipboard);
                }
                match &done.outcome {
                    Outcome::Failed(why) => self.show_note(format!("Could not paste: {why}")),
                    Outcome::Cancelled => {
                        self.show_note("Paste stopped: not every item was pasted".into());
                    }
                    Outcome::Done => {}
                }
                self.settle_refresh();
            }
            Job::Rename { path, name } => match done.outcome {
                Outcome::Done => {
                    let mut new = path[..name_start(&path)].to_vec();
                    new.extend_from_slice(&name);
                    let changed = self.model.borrow_mut().rename(&path, &new);
                    if let (Some(&first), Some(&last)) =
                        (changed.iter().min(), changed.iter().max())
                    {
                        send(self.list, LVM_REDRAWITEMS, first, last as isize);
                    }
                    // The search runs again: the row takes its place in the
                    // order, and gozd's own rows confirm the new paths.
                    self.settle_refresh();
                }
                Outcome::Cancelled => {}
                // The row never changed: it shows the old name still.
                Outcome::Failed(why) => {
                    let name = String::from_utf16_lossy(&path[name_start(&path)..]);
                    self.warn(&format!("Could not rename {name}:\n\n{why}"), "Rename");
                }
            },
        }
    }

    /// A warning box over the window, which does not bring goz-gui to the
    /// front when a posted command opened it. Nothing of `App` is borrowed
    /// while it runs its loop.
    fn warn(&self, text: &str, title: &str) {
        msgbox::show(self.hwnd, text, title, MB_OK | MB_ICONWARNING);
    }

    /// Every selected row that is held is one of `paths` (or none is
    /// selected): the user has not chosen other rows meanwhile.
    fn selection_within(&self, paths: &[Vec<u16>]) -> bool {
        let held = self.selected_held();
        let m = self.model.borrow();
        held.iter().all(|&i| {
            m.peek(i)
                .is_some_and(|r| paths.iter().any(|p| **p == *r.path))
        })
    }

    /// Selects and focuses the held row of the file at `path`, alone.
    fn select_file(&self, path: &[u16]) {
        let Some(i) = self.model.borrow().find(path) else {
            return;
        };
        self.quietly(|| {
            self.clear_selection();
            self.set_state(
                i,
                LVIS_SELECTED | LVIS_FOCUSED,
                LVIS_SELECTED | LVIS_FOCUSED,
            );
            send(self.list, LVM_SETSELECTIONMARK, 0, i as isize);
        });
        send(self.list, LVM_ENSUREVISIBLE, i, 0);
    }

    /// Files changed: the search runs again once gozd has had a moment to
    /// see it, and once more a little later. A refresh keeps the selection
    /// on the same files.
    pub(super) fn settle_refresh(&self) {
        self.settle.set(2);
        // SAFETY: a timer of this window; setting it again restarts it.
        unsafe { SetTimer(self.hwnd, TIMER_SETTLE, SETTLE_FIRST_MS, None) };
    }

    /// TIMER_SETTLE: one of `settle_refresh`'s refreshes.
    pub(super) fn settle_tick(&self) {
        self.start_search(true);
        let left = self.settle.get().saturating_sub(1);
        self.settle.set(left);
        if left > 0 {
            // SAFETY: as above.
            unsafe { SetTimer(self.hwnd, TIMER_SETTLE, SETTLE_AGAIN_MS, None) };
        }
    }

    /// The list's rename box is open.
    pub(super) fn renaming(&self) -> bool {
        send(self.list, LVM_GETEDITCONTROL, 0, 0) != 0
    }

    /// F2 (and File > Rename, the Explorer menu's Rename): renames the
    /// focused row in place; with several rows selected, the focused one.
    pub(super) fn rename_focused(&self) {
        let held = self.selected_held();
        let focused = send(
            self.list,
            LVM_GETNEXTITEM,
            usize::MAX,
            LVNI_FOCUSED as isize,
        );
        let at = usize::try_from(focused)
            .ok()
            .filter(|i| held.contains(i))
            .or_else(|| held.first().copied());
        let row = at.and_then(|i| {
            let m = self.model.borrow();
            m.peek(i).map(|r| (i, r.path.to_vec()))
        });
        let Some((index, path)) = row.filter(|(_, path)| renamable(path)) else {
            beep();
            return;
        };
        *self.renaming.borrow_mut() = Some(Renaming {
            path,
            text: None,
            problem: None,
        });
        self.edit_label(index);
    }

    /// Opens the list's rename box on row `index` (`begin_rename` places
    /// it); the list wants the keyboard for it. Giving it the keyboard
    /// activates the window, so a rename asked for while goz-gui is in the
    /// background (a posted command) is refused rather than let it take the
    /// foreground from the program the user is in.
    fn edit_label(&self, index: usize) {
        let _quiet = Quiet::begin();
        // SAFETY: focuses this thread's list, and reads where the focus is.
        let focused = unsafe {
            SetFocus(self.list);
            GetFocus() == self.list
        };
        if !focused || send(self.list, LVM_EDITLABELW, index, 0) == 0 {
            self.renaming.borrow_mut().take();
        }
    }

    /// Whether the last click on the list was on row `index`'s name, past
    /// its icon: where a slow second click starts a rename.
    fn click_on_name(&self, index: usize) -> bool {
        let pt = self.last_click.get();
        let Some((row, sub)) = self.hit_row(pt) else {
            return false;
        };
        if row != index || self.column_id(sub.max(0) as usize) != Some(COL_NAME) {
            return false;
        }
        self.name_cell(index)
            .is_some_and(|cell| pt.x >= cell.left + paint::name_text_offset(self))
    }

    /// Row `index`'s Name cell, in the list's coordinates.
    fn name_cell(&self, index: usize) -> Option<RECT> {
        let mut row = RECT {
            left: LVIR_BOUNDS as i32,
            ..RECT::default()
        };
        if send(self.list, LVM_GETITEMRECT, index, (&raw mut row) as isize) == 0 {
            return None;
        }
        let cells = self.column_rects(&row);
        cells
            .iter()
            .find(|&&(id, _)| id == COL_NAME)
            .map(|&(_, rc)| rc)
    }

    /// Where Rename's box goes on row `index`: over the name's text (not
    /// the icon) to the end of the Name column, its text where the row's
    /// is. `edit` is the box, for its margin.
    fn rename_box(&self, index: usize, edit: HWND) -> Option<RECT> {
        let cell = self.name_cell(index)?;
        // The box's border and its own margin come before its text.
        let margin = (send(edit, EM_GETMARGINS, 0, 0) & 0xffff) as i32;
        let left = cell.left + paint::name_text_offset(self) - margin - 1;
        let height = self.fonts.borrow().height + 2;
        let top = cell.top + (cell.bottom - cell.top - height) / 2;
        Some(RECT {
            left,
            top,
            right: cell.right.max(left + self.scale(60)),
            bottom: top + height,
        })
    }

    /// LVN_BEGINLABELEDIT for row `index`: true keeps the box from
    /// opening. F2 and the menus ask for it (`renaming` names the file);
    /// otherwise it is the list's own slow second click, which renames only
    /// when it was on the name. The box is placed over the name, themed,
    /// and its name selected without the extension.
    pub(super) fn begin_rename(&self, index: i32) -> bool {
        let Ok(index) = usize::try_from(index) else {
            return true;
        };
        let row = {
            let m = self.model.borrow();
            m.peek(index).map(|r| (r.path.to_vec(), r.is_dir))
        };
        let Some((path, is_dir)) = row.filter(|(path, _)| renamable(path)) else {
            self.renaming.borrow_mut().take();
            return true;
        };
        let asked = self.renaming.borrow().as_ref().map(|r| r.path == path);
        match asked {
            Some(true) => {}
            // Rows changed under a box asked for: not this one.
            Some(false) => {
                self.renaming.borrow_mut().take();
                return true;
            }
            None if self.click_on_name(index) => {
                *self.renaming.borrow_mut() = Some(Renaming {
                    path: path.clone(),
                    text: None,
                    problem: None,
                });
            }
            None => return true,
        }
        let edit = send(self.list, LVM_GETEDITCONTROL, 0, 0) as HWND;
        if edit.is_null() {
            return false;
        }
        let dark = self.pal.get().dark;
        theme::allow(edit, dark);
        theme::set_class(edit, dark.then_some("DarkMode_CFD"));
        let again = self
            .renaming
            .borrow_mut()
            .as_mut()
            .and_then(|r| r.text.take());
        let name = match again {
            Some(text) => {
                let z = nul(&text);
                // SAFETY: a NUL-terminated text for the box of this thread.
                unsafe { SetWindowTextW(edit, z.as_ptr()) };
                text
            }
            None => path[name_start(&path)..].to_vec(),
        };
        if let Some(rc) = self.rename_box(index, edit) {
            self.rename_rect.set(rc);
            // SAFETY: the box is the list's child, on this thread; `self`
            // is leaked, so the subclass data stays valid. The list moves
            // it as its text grows; the subclass keeps it here.
            unsafe {
                SetWindowSubclass(
                    edit,
                    Some(rename_box_proc),
                    RENAME_BOX,
                    self as *const App as usize,
                );
                SetWindowPos(
                    edit,
                    core::ptr::null_mut(),
                    rc.left,
                    rc.top,
                    rc.right - rc.left,
                    rc.bottom - rc.top,
                    SWP_NOZORDER | SWP_NOACTIVATE,
                );
            }
        }
        // After the list selects all of it, which it does once this returns.
        // SAFETY: posts to the box of this thread.
        unsafe { PostMessageW(edit, EM_SETSEL, 0, stem_len(&name, is_dir) as isize) };
        false
    }

    /// LVN_ENDLABELEDIT: `text` is the name typed (null: Esc). A name
    /// Windows would refuse, or one taken in that folder, is said why and
    /// the box opens again with it; a good one is renamed on the file
    /// operations thread. The list's own text is never set (the row
    /// changes once the rename is done).
    pub(super) fn end_rename(&self, text: *const u16) {
        let Some(mut r) = self.renaming.borrow_mut().take() else {
            return;
        };
        if text.is_null() {
            return;
        }
        // SAFETY: the list's NUL-terminated text, read before this returns.
        let new: Vec<u16> = unsafe {
            let len = (0..).take_while(|&i| *text.add(i) != 0).count();
            core::slice::from_raw_parts(text, len).to_vec()
        };
        let start = name_start(&r.path);
        let old = &r.path[start..];
        if new == old {
            return;
        }
        let problem = name_problem(&new).or_else(|| {
            // Another file of that name (a change of case is the same file).
            let same = String::from_utf16_lossy(old).to_lowercase()
                == String::from_utf16_lossy(&new).to_lowercase();
            let mut target = r.path[..start].to_vec();
            target.extend_from_slice(&new);
            match attributes(&target) {
                Some(a) if !same && a & FILE_ATTRIBUTE_DIRECTORY != 0 => Some(FOLDER_EXISTS),
                Some(_) if !same => Some(FILE_EXISTS),
                _ => None,
            }
        });
        if let Some(problem) = problem {
            r.text = Some(new);
            r.problem = Some(problem);
            *self.renaming.borrow_mut() = Some(r);
            // Once the list has closed this box.
            // SAFETY: posts to this thread's window.
            unsafe { PostMessageW(self.hwnd, WM_RENAME_AGAIN, 0, 0) };
            return;
        }
        self.record_search();
        if !self.fileops.run(Job::Rename {
            path: r.path,
            name: new,
        }) {
            beep();
        }
    }

    /// WM_RENAME_AGAIN: says why the name was refused, then opens the box
    /// again on the same file with the name typed.
    pub(super) fn rename_again(&self) {
        let (path, problem) = match self.renaming.borrow_mut().as_mut() {
            Some(r) => (r.path.clone(), r.problem.take()),
            None => return,
        };
        if let Some(problem) = problem {
            self.warn(problem, "Rename");
        }
        let index = self.model.borrow().find(&path);
        match index {
            Some(i) => self.edit_label(i),
            None => {
                self.renaming.borrow_mut().take();
            }
        }
    }

    /// LVN_BEGINDRAG and LVN_BEGINRDRAG: the selected rows that are held
    /// are dragged out as Explorer drags files (copy, move or link; a
    /// right-drag gets the target's menu). Nothing counts as a run. After
    /// a drop that did something (a move, say) the search runs again, and
    /// the status bar says when it took only part of the selection. Its
    /// rows are never fetched first (`results::Fetching`): the drag starts
    /// inside LVN_BEGINDRAG, while the button is down, or not at all.
    ///
    /// A target that copied the files for a move and leaves their deletion
    /// to the source (a move it did not optimize, in the shell's protocol)
    /// gets it: what is left of them goes to the Recycle Bin, as a Delete,
    /// so it can be undone. A move Explorer makes, it makes whole itself.
    pub(super) fn begin_drag(&self) {
        let (paths, selected) = self.picked_paths();
        if paths.is_empty() || !shell::ole_ready() {
            return;
        }
        let Some(items) = shell::Items::of_paths(&paths) else {
            return;
        };
        let Some(data) = items.data_object(self.hwnd) else {
            return;
        };
        self.record_search();
        self.own_drag.set(true);
        let effect = shell::drag(self.hwnd, &data);
        // A drop on goz-gui's own window came as a WM_DROPFILES posted
        // during the drag: this comes after it, and ends the drag for it.
        // SAFETY: posts to this thread's window.
        unsafe { PostMessageW(self.hwnd, WM_DRAG_DONE, 0, 0) };
        if effect == 0 && shell::performed_effect(&data) == 0 {
            return;
        }
        self.note_partial(Act::Drag, items.len(), selected);
        if shell::source_deletes(effect, shell::performed(&data)) {
            // Only what the target was given, and is still there.
            let left: Vec<Vec<u16>> = items
                .paths()
                .iter()
                .filter(|p| attributes(p).is_some())
                .cloned()
                .collect();
            if !left.is_empty() {
                let job = Job::Delete {
                    paths: left,
                    permanent: false,
                    next: None,
                };
                if !self.fileops.run(job) {
                    beep();
                }
                // Its end refreshes.
                return;
            }
        }
        self.settle_refresh();
    }

    /// WM_DRAG_DONE: drops are other programs' again.
    pub(super) fn drag_done(&self) {
        self.own_drag.set(false);
    }

    /// WM_DROPFILES on the window or the search box: what was dropped
    /// becomes the search (Shift held: is added to it). A shortcut stands
    /// for its target. goz searches one path at a time (it has no OR), so
    /// of several items the first is taken and the status bar says so. A
    /// drop of goz-gui's own rows on itself does nothing.
    pub(super) fn drop_files(&self, drop: HDROP) {
        // SAFETY: the HDROP WM_DROPFILES brought, read whole, then freed.
        let paths = unsafe {
            let paths = clipboard::dropped_files(drop);
            DragFinish(drop);
            paths
        };
        let Some(first) = paths.first() else {
            return;
        };
        if self.own_drag.get() {
            return;
        }
        // Shift as the drop came: this thread's keys, or the keyboard's own
        // when another program's drag had it.
        // SAFETY: reads the keyboard's state.
        let append = key_down(VK_SHIFT) || unsafe { GetAsyncKeyState(i32::from(VK_SHIFT)) } < 0;
        let lnk = String::from_utf16_lossy(first)
            .to_lowercase()
            .ends_with(".lnk");
        let target = lnk
            .then(|| shell::link_target(first))
            .flatten()
            .unwrap_or_else(|| first.clone());
        let text = drop_search(
            &window_text(self.edit),
            &String::from_utf16_lossy(&target),
            is_dir(&target),
            append,
        );
        let z = wz(&text);
        // SAFETY: a NUL-terminated text for this thread's box; EN_CHANGE
        // searches.
        unsafe { SetWindowTextW(self.edit, z.as_ptr()) };
        let end = z.len() - 1;
        send(self.edit, EM_SETSEL, end, end as isize);
        if paths.len() > 1 {
            self.show_note(format!(
                "{} items dropped; goz searches one path at a time, so this is the first",
                paths.len()
            ));
        }
    }
}

/// Rename's box: kept where `begin_rename` put it, over the name, however
/// the list moves it as the text changes.
unsafe extern "system" fn rename_box_proc(
    hwnd: HWND,
    msg: u32,
    w: WPARAM,
    l: LPARAM,
    _id: usize,
    data: usize,
) -> LRESULT {
    // SAFETY: `data` is the leaked `App`; `l` is read as each message
    // defines it; the subclass goes with the box.
    unsafe {
        let app = &*(data as *const App);
        match msg {
            WM_WINDOWPOSCHANGING if l != 0 => {
                let pos = &mut *(l as *mut WINDOWPOS);
                let rc = app.rename_rect.get();
                pos.x = rc.left;
                pos.y = rc.top;
                pos.cx = rc.right - rc.left;
                pos.cy = rc.bottom - rc.top;
                pos.flags &= !(SWP_NOMOVE | SWP_NOSIZE);
            }
            WM_NCDESTROY => {
                RemoveWindowSubclass(hwnd, Some(rename_box_proc), RENAME_BOX);
            }
            _ => {}
        }
        DefSubclassProc(hwnd, msg, w, l)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn w(s: &str) -> Vec<u16> {
        s.encode_utf16().collect()
    }

    #[test]
    fn names_windows_refuses() {
        let p = |s: &str| name_problem(&w(s));
        for good in [
            "a.txt",
            "report 2024.pdf",
            " lead",
            "CONSOLE.txt",
            "com10",
            "x.con",
        ] {
            assert_eq!(p(good), None, "{good}");
        }
        assert_eq!(p(""), Some(EMPTY));
        assert_eq!(p("   "), Some(EMPTY));
        for bad in [
            "a/b", "a\\b", "a:b", "a*", "a?", "\"a\"", "<a>", "a|b", "a\tb",
        ] {
            assert_eq!(p(bad), Some(BAD_CHARS), "{bad}");
        }
        assert_eq!(p("name."), Some(TRAILING));
        assert_eq!(p("name "), Some(TRAILING));
        for device in [
            "CON",
            "nul",
            "nul.txt",
            "Aux.tar.gz",
            "COM1",
            "lpt9.log",
            "con .txt",
        ] {
            assert_eq!(p(device), Some(RESERVED), "{device}");
        }
    }

    #[test]
    fn the_name_is_selected_without_its_extension() {
        assert_eq!(stem_len(&w("report.final.pdf"), false), 12);
        assert_eq!(stem_len(&w("README"), false), 6);
        assert_eq!(stem_len(&w(".gitignore"), false), 10);
        assert_eq!(
            stem_len(&w("photos.2024"), true),
            11,
            "a folder's whole name"
        );
    }

    #[test]
    fn a_drop_becomes_the_search() {
        assert_eq!(
            drop_search("old", r"C:\Users\me\a.txt", false, false),
            r"C:\Users\me\a.txt"
        );
        assert_eq!(
            drop_search("", r"C:\Users\me", true, false),
            r"C:\Users\me\"
        );
        assert_eq!(
            drop_search("", r"D:\", true, false),
            r"D:\",
            "a root keeps one"
        );
        assert_eq!(
            drop_search("", r"C:\Program Files", true, false),
            r#""C:\Program Files\""#
        );
        assert_eq!(
            drop_search("", r"C:\x\hi!.txt", false, false),
            r#""C:\x\hi!.txt""#
        );
        // Shift: added after what is there.
        assert_eq!(
            drop_search("ext:pdf", r"C:\docs", true, true),
            r"ext:pdf C:\docs\"
        );
        assert_eq!(
            drop_search("ext:pdf ", r"C:\a", false, true),
            r"ext:pdf C:\a"
        );
        assert_eq!(drop_search("  ", r"C:\a", false, true), r"C:\a");
    }

    #[test]
    fn the_selection_moves_past_what_was_deleted() {
        let rows: Vec<Vec<u16>> = [
            r"C:\a",
            r"C:\b",
            r"C:\dir",
            r"C:\dir\x",
            r"C:\dir\y",
            r"C:\dirt",
            r"C:\e",
        ]
        .iter()
        .map(|s| w(s))
        .collect();
        let path = |i: usize| rows.get(i).map(|p| &p[..]);
        let after = |sel: &[usize]| {
            let gone: Vec<Vec<u16>> = sel.iter().map(|&i| rows[i].clone()).collect();
            row_after(sel, rows.len(), path, &gone)
        };
        assert_eq!(after(&[0]), Some(1));
        assert_eq!(after(&[0, 2]), Some(5), "not the deleted folder's files");
        assert_eq!(
            after(&[1, 3]),
            Some(4),
            "an unselected row between them stays"
        );
        // The last rows: the nearest before them.
        assert_eq!(after(&[5, 6]), Some(4));
        assert_eq!(after(&[2, 5, 6]), Some(1));
        assert_eq!(after(&[0, 1, 2, 5, 6]), None, "nothing is left");
        assert_eq!(after(&[]), None);
        // A row not held is passed over.
        let held = |i: usize| (i != 1).then(|| &rows[i][..]);
        assert_eq!(
            row_after(&[0], rows.len(), held, &[rows[0].clone()]),
            Some(2)
        );
        assert!(is_inside(&w(r"C:\dir\x"), &w(r"C:\dir")));
        assert!(is_inside(&w(r"D:\x"), &w(r"D:\")));
        assert!(!is_inside(&w(r"C:\dirt"), &w(r"C:\dir")));
        assert!(!is_inside(&w(r"C:\dir"), &w(r"C:\dir")));
    }

    #[test]
    fn paste_goes_into_a_folder() {
        let s = |v: Option<Vec<u16>>| v.map(|p| String::from_utf16_lossy(&p));
        // The only row selected, a folder: into it.
        assert_eq!(
            s(paste_folder(&w(r"C:\a\dir"), true, true)).as_deref(),
            Some(r"C:\a\dir")
        );
        // A folder among others, or a file: beside it.
        assert_eq!(
            s(paste_folder(&w(r"C:\a\dir"), true, false)).as_deref(),
            Some(r"C:\a")
        );
        assert_eq!(
            s(paste_folder(&w(r"C:\a\f.txt"), false, true)).as_deref(),
            Some(r"C:\a")
        );
        assert_eq!(
            s(paste_folder(&w(r"C:\f.txt"), false, true)).as_deref(),
            Some(r"C:\")
        );
        assert_eq!(paste_folder(&w("f.txt"), false, true), None);
    }

    #[test]
    fn a_volume_root_is_not_renamed() {
        assert!(renamable(&w(r"C:\docs\a.txt")));
        assert!(renamable(&w(r"C:\docs")));
        assert!(!renamable(&w(r"C:\")));
        assert!(!renamable(&w(r"D:\")));
        assert!(!renamable(&w("")));
    }
}
