//! The file tools (wave 1, F5): Advanced Rename, Copy and Move to Folder,
//! New Folder, Create Shortcut, Set File Time, and Undo and Redo of what
//! goz-gui did to the files.
//!
//! The work runs on the file operations thread (`crate::fileops`), as a
//! [`Task`] of [`Step`]s: what the shell's `IFileOperation` does (rename,
//! move, copy, to the Recycle Bin, a new folder, back from the Recycle
//! Bin) goes through it, in as few operations as there are kinds of flags
//! (one progress window, one undo record in Explorer); a file's times and a
//! shortcut are set and made here. A progress sink ([`Sink`]) notes where
//! each item went, for every operation goz-gui runs, so the window can keep
//! an undo record of it (`undo`).
//!
//! An undo or a redo is [`Task::checked`]: each step first checks that its
//! item is still where the record left it and that nothing is in its way,
//! and is skipped (with the reason) otherwise. Nothing is ever written
//! over, even with the questions turned off.

/// The file tools' commands, from `menu::F5_FIRST` (3400..=3499).
pub mod cmd {
    use crate::menu::F5_FIRST;
    /// Edit > Undo (Ctrl+Z, Alt+Backspace in the results), Redo (Ctrl+Y)
    /// and Undo History... (Ctrl+Shift+Z).
    pub const UNDO: u16 = F5_FIRST;
    pub const REDO: u16 = F5_FIRST + 1;
    pub const UNDO_HISTORY: u16 = F5_FIRST + 2;
    /// Edit > Copy to Folder... and Move to Folder....
    pub const COPY_TO_FOLDER: u16 = F5_FIRST + 3;
    pub const MOVE_TO_FOLDER: u16 = F5_FIRST + 4;
    /// Edit > Advanced > Advanced Rename... (Shift+F2; F2 with several
    /// selected), Set File Time... and New Folder.
    pub const ADVANCED_RENAME: u16 = F5_FIRST + 5;
    pub const SET_FILE_TIME: u16 = F5_FIRST + 6;
    pub const NEW_FOLDER: u16 = F5_FIRST + 7;
    /// File > Create Shortcut (with a selection).
    pub const CREATE_SHORTCUT: u16 = F5_FIRST + 8;
    /// The last of them.
    pub const LAST: u16 = CREATE_SHORTCUT;
}

pub mod folder_pick;
pub mod rename;
pub mod rename_dialog;
pub mod settings;
pub mod times;
pub mod times_dialog;
pub mod ui;
pub mod undo;
pub mod undo_dialog;

use std::cell::RefCell;
use std::rc::Rc;

use windows::Win32::Foundation::HWND as WHWND;
use windows::Win32::System::Com::{
    CLSCTX_ALL, CLSCTX_INPROC_SERVER, CoCreateInstance, CoTaskMemFree, IPersistFile,
};
use windows::Win32::UI::Shell::{
    FILEOPERATION_FLAGS, FileOperation, IFileOperation, IFileOperationProgressSink,
    IFileOperationProgressSink_Impl, IShellItem, IShellLinkW, SHCreateItemFromParsingName,
    SIGDN_FILESYSPATH, ShellLink,
};
use windows::core::{HRESULT, Interface, PCWSTR, implement};
use windows_sys::Win32::Foundation::{CloseHandle, FILETIME, HWND, INVALID_HANDLE_VALUE};
use windows_sys::Win32::Globalization::{CSTR_EQUAL, CompareStringOrdinal};
use windows_sys::Win32::Storage::FileSystem::{
    CreateFileW, DeleteFileW, FILE_ATTRIBUTE_DIRECTORY, FILE_FLAG_BACKUP_SEMANTICS,
    FILE_READ_ATTRIBUTES, FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE,
    FILE_WRITE_ATTRIBUTES, GetFileAttributesW, GetFileTime, INVALID_FILE_ATTRIBUTES, OPEN_EXISTING,
    SetFileTime,
};
use windows_sys::Win32::UI::Shell::COPYENGINE_S_USER_IGNORED;

use crate::fileops::{Outcome, parent};
use crate::shell::{self, FileOp};
use crate::wide::nul;

pub use times::{Set, Times};
pub use undo::{Action, Record};

const BACKSLASH: u16 = b'\\' as u16;

/// One thing a task does to the files.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Step {
    /// Renames the item at `path` to `name` (a name, in the same folder).
    Rename { path: Vec<u16>, name: Vec<u16> },
    /// Moves the item at `path` into `folder`, as `name` when given.
    Move {
        path: Vec<u16>,
        folder: Vec<u16>,
        name: Option<Vec<u16>>,
    },
    /// Copies the item at `path` into `folder`, as `name` when given; a
    /// copy into its own folder gets a name of its own ("name - Copy").
    Copy {
        path: Vec<u16>,
        folder: Vec<u16>,
        name: Option<Vec<u16>>,
    },
    /// Sends the item at `path` to the Recycle Bin; with `empty_only`, only
    /// an empty folder (an undone New Folder never takes what was put in
    /// it since).
    Recycle { path: Vec<u16>, empty_only: bool },
    /// Makes a folder `name` in `folder`; unless `exact`, a name taken is
    /// numbered as Windows does ("New folder (2)").
    NewFolder {
        folder: Vec<u16>,
        name: Vec<u16>,
        exact: bool,
    },
    /// Sets the times of the item at `path`: created, modified, accessed.
    Times { path: Vec<u16>, set: [Set; 3] },
    /// Makes a shortcut to `target`: at `link`, or beside the target as
    /// "<name> - Shortcut.lnk" (in `desktop` instead when given).
    Shortcut {
        target: Vec<u16>,
        link: Option<Vec<u16>>,
        desktop: Option<Vec<u16>>,
    },
    /// Brings `recycled` (an item in the Recycle Bin, its `$R` file) back
    /// to `path`, and drops its `$I` record.
    Restore { recycled: Vec<u16>, path: Vec<u16> },
}

impl Step {
    /// The item the step acts on (for a new folder, the folder it goes in).
    pub fn source(&self) -> &[u16] {
        match self {
            Step::Rename { path, .. }
            | Step::Move { path, .. }
            | Step::Copy { path, .. }
            | Step::Recycle { path, .. }
            | Step::Times { path, .. } => path,
            Step::NewFolder { folder, .. } => folder,
            Step::Shortcut { target, .. } => target,
            Step::Restore { recycled, .. } => recycled,
        }
    }
}

/// How a step went.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StepResult {
    /// Done: the item is at `to` now (a copy, a new folder or shortcut,
    /// the item in the Recycle Bin, the item itself), with its times
    /// before and after for a change of times.
    Done {
        to: Vec<u16>,
        before: Option<Times>,
        after: Option<Times>,
    },
    /// Not tried: why (an undo whose item moved, a name taken...).
    Skipped(String),
    /// Tried and refused: the system's reason.
    Failed(String),
    /// A shortcut the folder would not take (no write access there).
    Denied,
    /// Done, but onto an item of the same name that was there already: a
    /// folder merged into it, or a file put in its place (Explorer's
    /// questions answered so, or asked none). It is not undone: Undo would
    /// take that item along, with what it held before.
    Landed { to: Vec<u16> },
    /// Not reached: the user stopped the operation first.
    NotRun,
}

/// What a task is for, which the window acts on when it is done.
#[derive(Clone, Debug)]
pub enum Purpose {
    /// A new action, to record for Undo.
    Do(Action),
    /// Undo of `record` (its items, step by step in this order).
    Undo(Record),
    /// Redo of `record`.
    Redo(Record),
}

/// A file tool's work for the file operations thread.
#[derive(Debug)]
pub struct Task {
    pub steps: Vec<Step>,
    /// Each step first checks its item is where it should be and that
    /// nothing is in its way (an undo or a redo).
    pub checked: bool,
    pub purpose: Purpose,
}

/// What a progress sink saw an operation do to one item.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Change {
    pub kind: Kind,
    /// The item's path before (for a new folder, the folder it went in).
    pub from: Vec<u16>,
    /// Where it is now: the new path, the copy, the item in the Recycle Bin
    /// (`None`: deleted for good, or not done).
    pub to: Option<Vec<u16>>,
    /// How it went.
    pub hr: i32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Rename,
    Move,
    Copy,
    Delete,
    New,
}

/// Records what an operation does to each item, as it is done.
#[implement(IFileOperationProgressSink)]
pub struct Sink {
    log: Rc<RefCell<Vec<Change>>>,
}

impl Sink {
    /// A sink, and the log it writes to.
    pub fn make() -> (IFileOperationProgressSink, Rc<RefCell<Vec<Change>>>) {
        let log = Rc::new(RefCell::new(Vec::new()));
        let sink: IFileOperationProgressSink = Sink {
            log: Rc::clone(&log),
        }
        .into();
        (sink, log)
    }

    fn note(&self, kind: Kind, from: Option<&IShellItem>, to: Option<&IShellItem>, hr: HRESULT) {
        let Some(from) = from.and_then(path_of) else {
            return;
        };
        let to = if hr.is_ok() {
            to.and_then(path_of)
        } else {
            None
        };
        self.log.borrow_mut().push(Change {
            kind,
            from,
            to,
            hr: hr.0,
        });
    }
}

type R = windows::core::Result<()>;
type ItemRef<'a> = windows::core::Ref<'a, IShellItem>;

#[allow(non_snake_case, reason = "the interface's method names")]
impl IFileOperationProgressSink_Impl for Sink_Impl {
    fn StartOperations(&self) -> R {
        Ok(())
    }
    fn FinishOperations(&self, _hr: HRESULT) -> R {
        Ok(())
    }
    fn PreRenameItem(&self, _f: u32, _item: ItemRef<'_>, _name: &PCWSTR) -> R {
        Ok(())
    }
    fn PostRenameItem(
        &self,
        _f: u32,
        item: ItemRef<'_>,
        _name: &PCWSTR,
        hr: HRESULT,
        new: ItemRef<'_>,
    ) -> R {
        self.note(Kind::Rename, item.as_ref(), new.as_ref(), hr);
        Ok(())
    }
    fn PreMoveItem(&self, _f: u32, _item: ItemRef<'_>, _to: ItemRef<'_>, _name: &PCWSTR) -> R {
        Ok(())
    }
    fn PostMoveItem(
        &self,
        _f: u32,
        item: ItemRef<'_>,
        _to: ItemRef<'_>,
        _name: &PCWSTR,
        hr: HRESULT,
        new: ItemRef<'_>,
    ) -> R {
        self.note(Kind::Move, item.as_ref(), new.as_ref(), hr);
        Ok(())
    }
    fn PreCopyItem(&self, _f: u32, _item: ItemRef<'_>, _to: ItemRef<'_>, _name: &PCWSTR) -> R {
        Ok(())
    }
    fn PostCopyItem(
        &self,
        _f: u32,
        item: ItemRef<'_>,
        _to: ItemRef<'_>,
        _name: &PCWSTR,
        hr: HRESULT,
        new: ItemRef<'_>,
    ) -> R {
        self.note(Kind::Copy, item.as_ref(), new.as_ref(), hr);
        Ok(())
    }
    fn PreDeleteItem(&self, _f: u32, _item: ItemRef<'_>) -> R {
        Ok(())
    }
    fn PostDeleteItem(&self, _f: u32, item: ItemRef<'_>, hr: HRESULT, new: ItemRef<'_>) -> R {
        self.note(Kind::Delete, item.as_ref(), new.as_ref(), hr);
        Ok(())
    }
    fn PreNewItem(&self, _f: u32, _folder: ItemRef<'_>, _name: &PCWSTR) -> R {
        Ok(())
    }
    fn PostNewItem(
        &self,
        _f: u32,
        folder: ItemRef<'_>,
        _name: &PCWSTR,
        _template: &PCWSTR,
        _attrs: u32,
        hr: HRESULT,
        new: ItemRef<'_>,
    ) -> R {
        self.note(Kind::New, folder.as_ref(), new.as_ref(), hr);
        Ok(())
    }
    fn UpdateProgress(&self, _total: u32, _done: u32) -> R {
        Ok(())
    }
    fn ResetTimer(&self) -> R {
        Ok(())
    }
    fn PauseTimer(&self) -> R {
        Ok(())
    }
    fn ResumeTimer(&self) -> R {
        Ok(())
    }
}

/// The file system path of a shell item.
fn path_of(item: &IShellItem) -> Option<Vec<u16>> {
    // SAFETY: the item is a live COM object of this thread; the name it
    // gives is a NUL-terminated string from the COM allocator, read and
    // then freed.
    unsafe {
        let name = item.GetDisplayName(SIGDN_FILESYSPATH).ok()?;
        if name.is_null() {
            return None;
        }
        let path = name.as_wide().to_vec();
        CoTaskMemFree(Some(name.0.cast()));
        Some(path)
    }
}

/// The shell item at `path`; `None` when it is gone.
fn item(path: &[u16]) -> Option<IShellItem> {
    let path = nul(path);
    // SAFETY: a NUL-terminated path that outlives the call.
    unsafe { SHCreateItemFromParsingName(PCWSTR(path.as_ptr()), None).ok() }
}

/// Whether `a` and `b` are the same path as Windows compares them.
pub fn same_path(a: &[u16], b: &[u16]) -> bool {
    fn trim(mut p: &[u16]) -> &[u16] {
        while let [rest @ .., BACKSLASH] = p {
            p = rest;
        }
        p
    }
    let (a, b) = (trim(a), trim(b));
    // SAFETY: two buffers, each read for its own length.
    unsafe {
        CompareStringOrdinal(a.as_ptr(), a.len() as i32, b.as_ptr(), b.len() as i32, 1)
            == CSTR_EQUAL
    }
}

/// The name in `path` (after its last backslash).
pub fn name_of(path: &[u16]) -> &[u16] {
    let start = path
        .iter()
        .rposition(|&c| c == BACKSLASH)
        .map_or(0, |i| i + 1);
    &path[start..]
}

/// `folder` joined with `name`.
pub fn join(folder: &[u16], name: &[u16]) -> Vec<u16> {
    let mut out = folder.to_vec();
    if out.last() != Some(&BACKSLASH) {
        out.push(BACKSLASH);
    }
    out.extend_from_slice(name);
    out
}

/// The attributes of `path`; `None` when there is nothing there.
pub fn attributes(path: &[u16]) -> Option<u32> {
    let z = nul(path);
    // SAFETY: a NUL-terminated path.
    let a = unsafe { GetFileAttributesW(z.as_ptr()) };
    (a != INVALID_FILE_ATTRIBUTES).then_some(a)
}

fn exists(path: &[u16]) -> bool {
    attributes(path).is_some()
}

fn is_dir(path: &[u16]) -> bool {
    attributes(path).is_some_and(|a| a & FILE_ATTRIBUTE_DIRECTORY != 0)
}

/// Whether the folder at `path` holds nothing.
fn is_empty_dir(path: &[u16]) -> bool {
    let path = std::path::PathBuf::from(String::from_utf16_lossy(path));
    std::fs::read_dir(path).is_ok_and(|mut d| d.next().is_none())
}

fn text(path: &[u16]) -> String {
    String::from_utf16_lossy(path)
}

// What a checked step says when it does not run.
const GONE: &str = "It is not there any more";
const TAKEN: &str = "Something of that name is there now";
const NO_FOLDER: &str = "The folder is not there any more";
const NOT_EMPTY: &str = "The folder is not empty any more";
const NOT_IN_BIN: &str = "It is not in the Recycle Bin any more";

/// Why `step` cannot be done now, if it cannot: its item gone, its folder
/// gone, or something in the way.
fn check(step: &Step) -> Option<&'static str> {
    let free = |target: &[u16], item: &[u16]| !exists(target) || same_path(target, item);
    match step {
        Step::Rename { path, name } => {
            let target = join(parent(path).unwrap_or(&[]), name);
            if !exists(path) {
                Some(GONE)
            } else if !free(&target, path) {
                Some(TAKEN)
            } else {
                None
            }
        }
        Step::Move { path, folder, name } | Step::Copy { path, folder, name } => {
            let target = join(folder, name.as_deref().unwrap_or(name_of(path)));
            if !exists(path) {
                Some(GONE)
            } else if !is_dir(folder) {
                Some(NO_FOLDER)
            } else if exists(&target) {
                Some(TAKEN)
            } else {
                None
            }
        }
        Step::Recycle { path, empty_only } => {
            if !exists(path) {
                Some(GONE)
            } else if *empty_only && !is_empty_dir(path) {
                Some(NOT_EMPTY)
            } else {
                None
            }
        }
        Step::NewFolder { folder, name, .. } => {
            if !is_dir(folder) {
                Some(NO_FOLDER)
            } else if exists(&join(folder, name)) {
                Some(TAKEN)
            } else {
                None
            }
        }
        Step::Times { path, .. } => (!exists(path)).then_some(GONE),
        Step::Shortcut { target, link, .. } => {
            if !exists(target) {
                Some(GONE)
            } else if link.as_deref().is_some_and(exists) {
                Some(TAKEN)
            } else {
                None
            }
        }
        Step::Restore { recycled, path } => {
            if !exists(recycled) {
                Some(NOT_IN_BIN)
            } else if !parent(path).is_some_and(is_dir) {
                Some(NO_FOLDER)
            } else if exists(path) {
                Some(TAKEN)
            } else {
                None
            }
        }
    }
}

/// The flags group a step's operation needs.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Group {
    Transfer,
    Duplicate,
    Recycle,
    Direct,
}

fn group(step: &Step) -> Group {
    match step {
        Step::Rename { .. } | Step::Move { .. } | Step::Restore { .. } => Group::Transfer,
        Step::NewFolder { exact: true, .. } => Group::Transfer,
        Step::NewFolder { exact: false, .. } => Group::Duplicate,
        Step::Copy { path, folder, name } => {
            if name.is_none() && parent(path).is_some_and(|p| same_path(p, folder)) {
                Group::Duplicate
            } else {
                Group::Transfer
            }
        }
        Step::Recycle { .. } => Group::Recycle,
        Step::Times { .. } | Step::Shortcut { .. } => Group::Direct,
    }
}

/// Where a copy or move into another folder lands under its own name,
/// when something of that name is there already: the step then merges
/// into it or replaces it (`StepResult::Landed`). Not for a copy beside
/// itself, which gets a name of its own ("name - Copy").
fn occupied(step: &Step) -> Option<Vec<u16>> {
    match step {
        Step::Move { path, folder, name } | Step::Copy { path, folder, name }
            if group(step) == Group::Transfer =>
        {
            let target = join(folder, name.as_deref().unwrap_or(name_of(path)));
            exists(&target).then_some(target)
        }
        _ => None,
    }
}

/// HRESULT_FROM_WIN32(ERROR_CANCELLED), and the copy engine's own
/// "cancelled by the user".
const CANCELLED: [i32; 2] = [0x8007_04C7_u32 as i32, 0x8027_0000_u32 as i32];

/// The system's text for `hr`.
pub fn hr_text(hr: i32) -> String {
    let text = windows::core::Error::from_hresult(HRESULT(hr)).message();
    let text = text.trim();
    if text.is_empty() {
        format!("Error 0x{:08X}", hr as u32)
    } else {
        text.to_owned()
    }
}

/// Runs `task` (on the file operations thread, an STA): its results, one
/// per step, and how it ended as a whole.
pub fn run(owner: HWND, task: &Task, silent: bool) -> (Outcome, Vec<StepResult>) {
    let mut results: Vec<StepResult> = vec![StepResult::NotRun; task.steps.len()];
    let mut todo: Vec<usize> = Vec::new();
    for (i, step) in task.steps.iter().enumerate() {
        match task.checked.then(|| check(step)).flatten() {
            Some(why) => results[i] = StepResult::Skipped(why.to_owned()),
            None => todo.push(i),
        }
    }
    // Noted before anything is done: afterwards every target is there.
    let occupied: Vec<(usize, Vec<u16>)> = todo
        .iter()
        .filter_map(|&i| occupied(&task.steps[i]).map(|t| (i, t)))
        .collect();
    let mut cancelled = false;
    let mut failure: Option<String> = None;
    for g in [Group::Transfer, Group::Duplicate, Group::Recycle] {
        let mine: Vec<usize> = todo
            .iter()
            .copied()
            .filter(|&i| group(&task.steps[i]) == g)
            .collect();
        if mine.is_empty() || cancelled {
            continue;
        }
        match shell_steps(owner, &task.steps, &mine, g, silent, &mut results) {
            Ok(true) => {}
            Ok(false) => cancelled = true,
            Err(e) if CANCELLED.contains(&e.code().0) => cancelled = true,
            Err(e) => {
                let why = hr_text(e.code().0);
                for &i in &mine {
                    if results[i] == StepResult::NotRun {
                        results[i] = StepResult::Failed(why.clone());
                    }
                }
                failure.get_or_insert(why);
            }
        }
    }
    if !cancelled {
        for &i in &todo {
            match &task.steps[i] {
                Step::Times { path, set } => results[i] = set_times(path, set),
                Step::Shortcut {
                    target,
                    link,
                    desktop,
                } => results[i] = make_shortcut(target, link.as_deref(), desktop.as_deref()),
                _ => {}
            }
        }
    }
    // A step that went onto what was there is no undoable copy or move.
    for (i, target) in occupied {
        if let StepResult::Done { to, .. } = &results[i]
            && same_path(to, &target)
        {
            results[i] = StepResult::Landed { to: to.clone() };
        }
    }
    let done = results
        .iter()
        .filter(|r| matches!(r, StepResult::Done { .. } | StepResult::Landed { .. }))
        .count();
    let outcome = if cancelled {
        Outcome::Cancelled
    } else if let Some(why) = failure {
        Outcome::Failed(why)
    } else if done == 0 && !results.is_empty() {
        // Nothing done: the first reason. (A shortcut a folder refused is
        // no failure yet: the window offers the Desktop.)
        let why = results.iter().find_map(|r| match r {
            StepResult::Skipped(w) | StepResult::Failed(w) => Some(w.clone()),
            _ => None,
        });
        why.map_or(Outcome::Done, Outcome::Failed)
    } else {
        Outcome::Done
    };
    (outcome, results)
}

/// Runs the steps `mine` (of `steps`, all of group `g`) in one
/// `IFileOperation` with a sink, and fills in their results. False when
/// the user stopped it.
fn shell_steps(
    owner: HWND,
    steps: &[Step],
    mine: &[usize],
    g: Group,
    silent: bool,
    results: &mut [StepResult],
) -> windows::core::Result<bool> {
    // Duplicate's flags name a copy (or new folder) whose name is taken
    // as Windows does: "name - Copy", "New folder (2)".
    let kind = match g {
        Group::Recycle => FileOp::Delete { permanent: false },
        Group::Duplicate => FileOp::Duplicate,
        _ => FileOp::Transfer,
    };
    // SAFETY: COM is initialised on this thread (an STA); every item and
    // string lives across the calls that take it, and the operation is
    // released when this ends.
    unsafe {
        let op: IFileOperation = CoCreateInstance(&FileOperation, None, CLSCTX_ALL)?;
        if !owner.is_null() {
            op.SetOwnerWindow(WHWND(owner))?;
        }
        op.SetOperationFlags(FILEOPERATION_FLAGS(shell::fileop_flags(kind, silent)))?;
        let (sink, log) = Sink::make();
        let cookie = op.Advise(&sink)?;
        let mut queued = 0;
        // Names kept alive until the operation has run.
        let mut names: Vec<Vec<u16>> = Vec::new();
        for &i in mine {
            let step = &steps[i];
            let Some(source) = item(step.source()) else {
                results[i] = StepResult::Failed(hr_text(0x8007_0002_u32 as i32));
                continue;
            };
            let name_ptr = |names: &mut Vec<Vec<u16>>, name: Option<&[u16]>| match name {
                Some(n) => {
                    names.push(nul(n));
                    PCWSTR(names.last().map_or(core::ptr::null(), |n| n.as_ptr()))
                }
                None => PCWSTR::null(),
            };
            let none = None::<&IFileOperationProgressSink>;
            let queued_one = match step {
                Step::Rename { name, .. } => {
                    let n = name_ptr(&mut names, Some(name));
                    op.RenameItem(&source, n, none)
                }
                Step::Move { folder, name, .. } | Step::Copy { folder, name, .. } => {
                    let Some(to) = item(folder) else {
                        results[i] = StepResult::Failed(hr_text(0x8007_0003_u32 as i32));
                        continue;
                    };
                    let n = name_ptr(&mut names, name.as_deref());
                    if matches!(step, Step::Move { .. }) {
                        op.MoveItem(&source, &to, n, none)
                    } else {
                        op.CopyItem(&source, &to, n, none)
                    }
                }
                Step::Restore { path, .. } => {
                    let Some(to) = parent(path).and_then(item) else {
                        results[i] = StepResult::Failed(hr_text(0x8007_0003_u32 as i32));
                        continue;
                    };
                    let n = name_ptr(&mut names, Some(name_of(path)));
                    op.MoveItem(&source, &to, n, none)
                }
                Step::Recycle { .. } => op.DeleteItem(&source, none),
                Step::NewFolder { name, .. } => {
                    let n = name_ptr(&mut names, Some(name));
                    op.NewItem(&source, FILE_ATTRIBUTE_DIRECTORY, n, PCWSTR::null(), none)
                }
                Step::Times { .. } | Step::Shortcut { .. } => continue,
            };
            match queued_one {
                Ok(()) => queued += 1,
                Err(e) => results[i] = StepResult::Failed(hr_text(e.code().0)),
            }
        }
        if queued == 0 {
            return Ok(true);
        }
        let performed = op.PerformOperations();
        let aborted = op
            .GetAnyOperationsAborted()
            .map(|b| b.as_bool())
            .unwrap_or(false);
        let log = log.borrow();
        for &i in mine {
            if results[i] != StepResult::NotRun {
                continue;
            }
            let step = &steps[i];
            let want = match step {
                Step::Rename { .. } => Kind::Rename,
                Step::Move { .. } | Step::Restore { .. } => Kind::Move,
                Step::Copy { .. } => Kind::Copy,
                Step::Recycle { .. } => Kind::Delete,
                Step::NewFolder { .. } => Kind::New,
                _ => continue,
            };
            // The operation reports the items inside a folder too: only
            // the one given counts.
            let seen = log
                .iter()
                .find(|c| c.kind == want && same_path(&c.from, step.source()));
            let done = |to: Vec<u16>| StepResult::Done {
                to,
                before: None,
                after: None,
            };
            results[i] = match seen {
                // Sent to the Recycle Bin: done once it is gone from its
                // place (a question answered "skip" leaves it), wherever
                // the bin keeps it.
                // "Skip" in Explorer's question: nothing was done to it,
                // whatever item the operation names.
                Some(c) if c.hr == COPYENGINE_S_USER_IGNORED => StepResult::NotRun,
                Some(c) if c.hr >= 0 && want == Kind::Delete => {
                    if exists(step.source()) {
                        StepResult::NotRun
                    } else {
                        done(c.to.clone().unwrap_or_default())
                    }
                }
                Some(c) if c.hr >= 0 => match &c.to {
                    Some(to) => {
                        if let Step::Restore { recycled, .. } = step {
                            drop_recycle_record(recycled);
                        }
                        done(to.clone())
                    }
                    // Skipped by the user in a question.
                    None => StepResult::NotRun,
                },
                Some(c) if CANCELLED.contains(&c.hr) => StepResult::NotRun,
                Some(c) => StepResult::Failed(hr_text(c.hr)),
                None => StepResult::NotRun,
            };
        }
        drop(log);
        let _ = op.Unadvise(cookie);
        performed?;
        Ok(!aborted)
    }
}

/// After a `$R` item came back out of the Recycle Bin, its `$I` record
/// (the original path and date) goes too, as Explorer's Restore does: the
/// bin would otherwise list an entry with nothing behind it.
fn drop_recycle_record(recycled: &[u16]) {
    let name = name_of(recycled);
    if name.len() < 2 || name[0] != u16::from(b'$') || (name[1] | 0x20) != u16::from(b'r') {
        return;
    }
    let mut record = recycled.to_vec();
    let at = record.len() - name.len() + 1;
    record[at] = u16::from(b'I');
    let z = nul(&record);
    // SAFETY: a NUL-terminated path; a missing file is reported and ignored.
    unsafe { DeleteFileW(z.as_ptr()) };
}

/// Opens `path` (a file or a folder) to read and set its times.
fn open_for_times(path: &[u16]) -> Result<Handle, String> {
    let z = nul(path);
    // SAFETY: a NUL-terminated path; the handle is closed by `Handle`.
    let h = unsafe {
        CreateFileW(
            z.as_ptr(),
            FILE_READ_ATTRIBUTES | FILE_WRITE_ATTRIBUTES,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            core::ptr::null(),
            OPEN_EXISTING,
            FILE_FLAG_BACKUP_SEMANTICS,
            core::ptr::null_mut(),
        )
    };
    if h == INVALID_HANDLE_VALUE {
        return Err(last_error());
    }
    Ok(Handle(h))
}

struct Handle(windows_sys::Win32::Foundation::HANDLE);

impl Drop for Handle {
    fn drop(&mut self) {
        // SAFETY: the handle `open_for_times` opened, closed once.
        unsafe { CloseHandle(self.0) };
    }
}

fn last_error() -> String {
    let e = std::io::Error::last_os_error();
    e.raw_os_error()
        .map(|code| hr_text((code as u32 | 0x8007_0000) as i32))
        .unwrap_or_else(|| e.to_string())
}

fn read_times(h: &Handle) -> Option<Times> {
    let mut t = [FILETIME::default(); 3];
    // SAFETY: an open handle and three out-structs.
    let ok = unsafe { GetFileTime(h.0, &mut t[0], &mut t[1], &mut t[2]) } != 0;
    let v = |f: FILETIME| (u64::from(f.dwHighDateTime) << 32) | u64::from(f.dwLowDateTime);
    ok.then(|| Times {
        created: v(t[0]),
        accessed: v(t[1]),
        modified: v(t[2]),
    })
}

/// Sets the times of `path` as `set` says (created, modified, accessed).
fn set_times(path: &[u16], set: &[Set; 3]) -> StepResult {
    let h = match open_for_times(path) {
        Ok(h) => h,
        Err(why) => return StepResult::Failed(why),
    };
    let Some(before) = read_times(&h) else {
        return StepResult::Failed(last_error());
    };
    let want = times::apply(before, set);
    let ft = |v: Option<u64>| {
        v.map(|v| FILETIME {
            dwLowDateTime: v as u32,
            dwHighDateTime: (v >> 32) as u32,
        })
    };
    let (c, a, m) = (ft(want.0), ft(want.1), ft(want.2));
    let ptr = |f: &Option<FILETIME>| f.as_ref().map_or(core::ptr::null(), |f| f as *const _);
    // SAFETY: an open handle; each time is a FILETIME that lives across
    // the call, or null for "leave it".
    let ok = unsafe { SetFileTime(h.0, ptr(&c), ptr(&a), ptr(&m)) } != 0;
    if !ok {
        return StepResult::Failed(last_error());
    }
    StepResult::Done {
        to: path.to_vec(),
        before: Some(before),
        after: read_times(&h),
    }
}

/// Explorer's name for a shortcut to `name` and its numbered forms:
/// "name - Shortcut.lnk", "name - Shortcut (2).lnk", ...
pub fn shortcut_name(name: &str, n: u32) -> String {
    if n <= 1 {
        format!("{name} - Shortcut.lnk")
    } else {
        format!("{name} - Shortcut ({n}).lnk")
    }
}

/// The name a shortcut to `target` is named after: its name, or a drive's
/// letter.
fn link_base(target: &[u16]) -> String {
    let name = text(name_of(target));
    if name.is_empty() {
        text(target).trim_end_matches(['\\', ':']).to_owned()
    } else {
        name
    }
}

/// Whether `hr` says the folder does not let a file be made there.
fn denied(hr: i32) -> bool {
    // E_ACCESSDENIED, ERROR_WRITE_PROTECT, ERROR_PRIVILEGE_NOT_HELD.
    [0x8007_0005_u32, 0x8007_0013, 0x8007_0522]
        .iter()
        .any(|&d| d as i32 == hr)
}

/// Makes a shortcut to `target` at `link`, or beside it (in `desktop`
/// instead when given) under the first free name.
fn make_shortcut(target: &[u16], link: Option<&[u16]>, desktop: Option<&[u16]>) -> StepResult {
    let path = match link {
        Some(l) => l.to_vec(),
        None => {
            let Some(folder) = desktop.or_else(|| parent(target)) else {
                return StepResult::Failed(hr_text(0x8007_0003_u32 as i32));
            };
            let base = link_base(target);
            let free = (1..=1000)
                .map(|n| {
                    join(
                        folder,
                        &shortcut_name(&base, n).encode_utf16().collect::<Vec<u16>>(),
                    )
                })
                .find(|p| !exists(p));
            match free {
                Some(p) => p,
                None => return StepResult::Failed(TAKEN.to_owned()),
            }
        }
    };
    let target_z = nul(target);
    let path_z = nul(&path);
    let work = parent(target).filter(|_| !is_dir(target)).map(nul);
    // SAFETY: COM is initialised on this thread; every string is
    // NUL-terminated and outlives the call that takes it.
    let saved = unsafe {
        (|| -> windows::core::Result<()> {
            let link: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER)?;
            link.SetPath(PCWSTR(target_z.as_ptr()))?;
            if let Some(w) = &work {
                link.SetWorkingDirectory(PCWSTR(w.as_ptr()))?;
            }
            link.cast::<IPersistFile>()?
                .Save(PCWSTR(path_z.as_ptr()), true)?;
            Ok(())
        })()
    };
    match saved {
        Ok(()) => StepResult::Done {
            to: path,
            before: None,
            after: None,
        },
        Err(e) if denied(e.code().0) => StepResult::Denied,
        Err(e) => StepResult::Failed(hr_text(e.code().0)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::{Path, PathBuf};
    use windows::Win32::System::Com::COINIT_APARTMENTTHREADED as WSTA;
    use windows::Win32::System::Com::CoInitializeEx as WCoInitializeEx;

    fn w(p: &Path) -> Vec<u16> {
        p.as_os_str().to_string_lossy().encode_utf16().collect()
    }

    fn ws(s: &str) -> Vec<u16> {
        s.encode_utf16().collect()
    }

    /// A fresh folder of the test's own under the temporary folder.
    fn scratch(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("goz-gui-filetools-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn task(steps: Vec<Step>, checked: bool) -> Task {
        Task {
            steps,
            checked,
            purpose: Purpose::Do(Action::Rename),
        }
    }

    fn done_to(r: &StepResult) -> PathBuf {
        match r {
            StepResult::Done { to, .. } => PathBuf::from(String::from_utf16_lossy(to)),
            other => panic!("not done: {other:?}"),
        }
    }

    /// Rename, move, copy, a new folder and back: each step's result says
    /// where its item went, and a checked step never writes over anything.
    #[test]
    fn steps_say_where_their_items_went() {
        // SAFETY: COM for this test thread, which ends with the test.
        let _ = unsafe { WCoInitializeEx(None, WSTA) };
        let dir = scratch("steps");
        let (a, b) = (dir.join("a"), dir.join("b"));
        std::fs::create_dir_all(&a).unwrap();
        std::fs::create_dir_all(&b).unwrap();
        std::fs::write(a.join("one.txt"), "1").unwrap();
        std::fs::write(a.join("two.txt"), "2").unwrap();
        let none = core::ptr::null_mut();

        let (outcome, r) = run(
            none,
            &task(
                vec![
                    Step::Rename {
                        path: w(&a.join("one.txt")),
                        name: ws("uno.txt"),
                    },
                    Step::Copy {
                        path: w(&a.join("two.txt")),
                        folder: w(&b),
                        name: None,
                    },
                    // Into its own folder: a name of its own.
                    Step::Copy {
                        path: w(&a.join("two.txt")),
                        folder: w(&a),
                        name: None,
                    },
                    Step::NewFolder {
                        folder: w(&b),
                        name: ws("New folder"),
                        exact: false,
                    },
                ],
                false,
            ),
            true,
        );
        assert_eq!(outcome, Outcome::Done, "{r:?}");
        assert_eq!(done_to(&r[0]), a.join("uno.txt"));
        assert_eq!(done_to(&r[1]), b.join("two.txt"));
        let twin = done_to(&r[2]);
        assert!(twin != a.join("two.txt") && twin.exists(), "{twin:?}");
        assert_eq!(done_to(&r[3]), b.join("New folder"));
        // Again: Windows numbers it.
        let (_, r) = run(
            none,
            &task(
                vec![Step::NewFolder {
                    folder: w(&b),
                    name: ws("New folder"),
                    exact: false,
                }],
                false,
            ),
            true,
        );
        assert_eq!(done_to(&r[0]), b.join("New folder (2)"));

        // A checked move whose target is taken is skipped, not written over.
        std::fs::write(a.join("two.txt"), "keep").unwrap();
        let (outcome, r) = run(
            none,
            &task(
                vec![
                    Step::Move {
                        path: w(&b.join("two.txt")),
                        folder: w(&a),
                        name: None,
                    },
                    Step::Move {
                        path: w(&a.join("uno.txt")),
                        folder: w(&b),
                        name: Some(ws("one.txt")),
                    },
                ],
                true,
            ),
            true,
        );
        assert_eq!(r[0], StepResult::Skipped(TAKEN.into()));
        assert_eq!(std::fs::read_to_string(a.join("two.txt")).unwrap(), "keep");
        assert_eq!(done_to(&r[1]), b.join("one.txt"));
        assert_eq!(outcome, Outcome::Done);
        // A folder that is not empty is not recycled by a checked step
        // (the Recycle Bin itself is left alone by the tests).
        std::fs::write(b.join("New folder (2)").join("x"), "x").unwrap();
        let (_, r) = run(
            none,
            &task(
                vec![Step::Recycle {
                    path: w(&b.join("New folder (2)")),
                    empty_only: true,
                }],
                true,
            ),
            true,
        );
        assert_eq!(r[0], StepResult::Skipped(NOT_EMPTY.into()));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// A copy or move onto an item of its name already there (a folder
    /// merged into it, a file put in its place) is done, but is no copy or
    /// move Undo may take back: that would take what was there along.
    #[test]
    fn what_lands_on_an_item_already_there_is_not_undone() {
        // SAFETY: COM for this test thread, which ends with the test.
        let _ = unsafe { WCoInitializeEx(None, WSTA) };
        let dir = scratch("landed");
        let (src, dest) = (dir.join("src"), dir.join("dest"));
        std::fs::create_dir_all(src.join("X")).unwrap();
        std::fs::create_dir_all(dest.join("X")).unwrap();
        std::fs::write(src.join("X").join("new.txt"), "new").unwrap();
        std::fs::write(dest.join("X").join("pre.txt"), "pre").unwrap();
        std::fs::write(src.join("g.txt"), "src").unwrap();
        std::fs::write(dest.join("g.txt"), "dest").unwrap();
        std::fs::write(src.join("h.txt"), "h").unwrap();
        let steps: Vec<Step> = ["X", "g.txt", "h.txt"]
            .iter()
            .map(|n| Step::Copy {
                path: w(&src.join(n)),
                folder: w(&dest),
                name: None,
            })
            .collect();
        let sources: Vec<Vec<u16>> = steps.iter().map(|s| s.source().to_vec()).collect();
        let (outcome, r) = run(core::ptr::null_mut(), &task(steps, false), true);
        assert_eq!(outcome, Outcome::Done, "{r:?}");
        assert!(matches!(&r[0], StepResult::Landed { .. }), "{r:?}");
        assert!(matches!(&r[1], StepResult::Landed { .. }), "{r:?}");
        assert_eq!(done_to(&r[2]), dest.join("h.txt"));
        // Merged: both files are in the folder.
        assert!(dest.join("X").join("pre.txt").exists());
        assert!(dest.join("X").join("new.txt").exists());
        // Only the copy that made a new item is in the undo record.
        let items = undo::items_of(Action::Copy, &sources, &r);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].to, w(&dest.join("h.txt")));
        // A move the same way: the folder merged is not moved back by Undo.
        let (_, r) = run(
            core::ptr::null_mut(),
            &task(
                vec![Step::Move {
                    path: w(&src.join("X")),
                    folder: w(&dest),
                    name: None,
                }],
                false,
            ),
            true,
        );
        assert!(matches!(&r[0], StepResult::Landed { .. }), "{r:?}");
        assert!(undo::items_of(Action::Move, &[w(&src.join("X"))], &r).is_empty());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// Times are set and read back; a shortcut is made under Explorer's
    /// name, numbered when taken.
    #[test]
    fn times_and_shortcuts() {
        // SAFETY: COM for this test thread, which ends with the test.
        let _ = unsafe { WCoInitializeEx(None, WSTA) };
        let dir = scratch("times");
        let file = dir.join("f.txt");
        std::fs::write(&file, "x").unwrap();
        let none = core::ptr::null_mut();
        // 2001-02-03 04:05:06 UTC.
        let when = 126_257_427_060_000_000u64;
        let (_, r) = run(
            none,
            &task(
                vec![Step::Times {
                    path: w(&file),
                    set: [Set::At(when), Set::At(when), Set::Keep],
                }],
                false,
            ),
            true,
        );
        let StepResult::Done { before, after, .. } = &r[0] else {
            panic!("{r:?}");
        };
        let (before, after) = (before.unwrap(), after.unwrap());
        assert_eq!((after.created, after.modified), (when, when));
        assert_eq!(after.accessed, before.accessed);

        let (_, r) = run(
            none,
            &task(
                vec![
                    Step::Shortcut {
                        target: w(&file),
                        link: None,
                        desktop: None,
                    },
                    Step::Shortcut {
                        target: w(&file),
                        link: None,
                        desktop: None,
                    },
                ],
                false,
            ),
            true,
        );
        assert_eq!(done_to(&r[0]), dir.join("f.txt - Shortcut.lnk"));
        assert_eq!(done_to(&r[1]), dir.join("f.txt - Shortcut (2).lnk"));
        let back = crate::shell::link_target(&w(&dir.join("f.txt - Shortcut.lnk")));
        assert_eq!(
            back.map(|t| String::from_utf16_lossy(&t)),
            Some(file.display().to_string())
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn paths_and_names() {
        assert_eq!(shortcut_name("a.txt", 1), "a.txt - Shortcut.lnk");
        assert_eq!(shortcut_name("a.txt", 3), "a.txt - Shortcut (3).lnk");
        assert_eq!(link_base(&ws(r"C:\x\Docs")), "Docs");
        assert_eq!(link_base(&ws(r"D:\")), "D");
        assert_eq!(join(&ws(r"C:\"), &ws("a")), ws(r"C:\a"));
        assert_eq!(join(&ws(r"C:\x"), &ws("a")), ws(r"C:\x\a"));
        assert_eq!(name_of(&ws(r"C:\x\a.txt")), ws("a.txt"));
        assert!(same_path(&ws(r"C:\X\"), &ws(r"c:\x")));
        assert!(denied(0x8007_0005_u32 as i32) && !denied(0x8007_0002_u32 as i32));
    }
}
