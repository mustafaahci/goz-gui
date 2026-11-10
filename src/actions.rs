//! What the user can do with results: open them, show them in Explorer, run
//! them as administrator, look at their properties; and the shell calls of
//! the Tools and Help menus.
//!
//! Opening goes through the shell, which can take seconds (a program that
//! starts slowly, a network path that does not answer, a UAC prompt), so it
//! runs on a thread of its own and the window never waits for it. So does
//! Properties, whose items are parsed from their paths on the disk.

use std::collections::{HashMap, VecDeque};
use std::os::windows::io::AsRawHandle;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use windows_sys::Win32::Foundation::{CloseHandle, HWND};
use windows_sys::Win32::System::Com::{
    COINIT_APARTMENTTHREADED, COINIT_DISABLE_OLE1DDE, CoInitializeEx, CoUninitialize,
};
use windows_sys::Win32::System::SystemInformation::GetSystemDirectoryW;
use windows_sys::Win32::System::Threading::{GetThreadId, WaitForSingleObject};
use windows_sys::Win32::UI::Shell::Common::ITEMIDLIST;
use windows_sys::Win32::UI::Shell::{
    ILClone, ILCreateFromPathW, ILFindChild, ILFindLastID, ILFree, ILIsEqual, ILIsParent,
    ILRemoveLastID, SEE_MASK_NOASYNC, SEE_MASK_NOCLOSEPROCESS, SHARD_PATHW, SHAddToRecentDocs,
    SHCreateDataObject, SHELLEXECUTEINFOW, SHMultiFileProperties, SHOP_FILEPATH,
    SHObjectProperties, SHOpenFolderAndSelectItems, ShellExecuteExW,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    DispatchMessageW, GetMessageW, MSG, PM_NOREMOVE, PeekMessageW, PostMessageW,
    PostThreadMessageW, SW_HIDE, SW_SHOWNORMAL, TranslateMessage, WM_APP,
};
use windows_sys::core::GUID;

use crate::store::location::dry_run_instead;
use crate::wide::{nul, wz};

/// In a test's dry run (`GOZ_GUI_DRY_RUN`), notes the action in the data
/// folder's `dry-run.log` instead of running it, and returns true.
fn dry_run<'a>(verb: &str, items: impl IntoIterator<Item = &'a [u16]>) -> bool {
    if !crate::store::location::dry_run() {
        return false;
    }
    let items: Vec<String> = items.into_iter().map(String::from_utf16_lossy).collect();
    dry_run_instead(verb, &items)
}

/// Where goz's documentation starts, for Help > goz Help.
pub const HELP_URL: &str = "https://github.com/mustafaahci/goz#usage";
pub const WEBSITE_URL: &str = "https://github.com/mustafaahci/goz";

const BACKSLASH: u16 = b'\\' as u16;

/// The folder holding `path`, as a path that names it (`C:\` for a file at
/// the root of C:); `None` for a volume root, which no folder holds.
fn parent_folder(path: &[u16]) -> Option<&[u16]> {
    let i = path.iter().rposition(|&c| c == BACKSLASH)?;
    if i + 1 >= path.len() {
        return None;
    }
    // "C:" alone means C:'s current folder, not its root.
    let end = if i > 0 && path[i - 1] == u16::from(b':') {
        i + 1
    } else {
        i
    };
    Some(&path[..end])
}

/// A result to open: its path, and whether it is a folder.
pub struct Target {
    pub path: Vec<u16>,
    pub is_dir: bool,
}

/// Opens each of `targets` with its default verb, as a double-click in
/// Explorer would, off the UI thread. Files opened go into Windows' recent
/// items (Explorer's Recent, the program's jump list), as Explorer does for
/// the files it opens; Windows leaves them out when the user turned that off.
pub fn open(hwnd: HWND, targets: Vec<Target>) {
    if targets.is_empty() || dry_run("open", targets.iter().map(|t| &t.path[..])) {
        return;
    }
    let owner = hwnd as usize;
    in_background(move || {
        for target in targets {
            let dir = parent_folder(&target.path);
            let opened = execute(owner, None, &target.path, None, dir, SW_SHOWNORMAL, None);
            if opened && !target.is_dir {
                let path = nul(&target.path);
                // SAFETY: a NUL-terminated path, as SHARD_PATHW says.
                unsafe { SHAddToRecentDocs(SHARD_PATHW as u32, path.as_ptr().cast()) };
            }
        }
    });
}

/// `paths` grouped by the folder holding them, in the order the folders
/// first come: each group opens as one Explorer window with all of its
/// items selected. A volume root is a group of its own, with no folder.
fn by_folder(paths: &[Vec<u16>]) -> Vec<(Option<&[u16]>, Vec<usize>)> {
    let mut groups: Vec<(Option<&[u16]>, Vec<usize>)> = Vec::new();
    let mut index: HashMap<&[u16], usize> = HashMap::new();
    for (i, path) in paths.iter().enumerate() {
        match parent_folder(path) {
            Some(folder) => {
                let at = *index.entry(folder).or_insert_with(|| {
                    groups.push((Some(folder), Vec::new()));
                    groups.len() - 1
                });
                groups[at].1.push(i);
            }
            None => groups.push((None, vec![i])),
        }
    }
    groups
}

/// How many Explorer windows `reveal` opens for `paths`.
pub fn reveal_windows(paths: &[Vec<u16>]) -> usize {
    by_folder(paths).len()
}

/// Opens the folders holding `paths` in Explorer with those items selected,
/// one window per folder, off the UI thread.
pub fn reveal(paths: Vec<Vec<u16>>) {
    if paths.is_empty() || dry_run("open-path", paths.iter().map(|p| &p[..])) {
        return;
    }
    in_background(move || {
        for (folder, members) in by_folder(&paths) {
            let items: Vec<&[u16]> = members.iter().map(|&i| &paths[i][..]).collect();
            reveal_in(folder, &items);
        }
    });
}

/// Opens `folder` with `items` (paths in it) selected; with no folder, or a
/// single item, each item's own folder.
fn reveal_in(folder: Option<&[u16]>, items: &[&[u16]]) {
    // SAFETY: each path is NUL-terminated for the call that parses it; every
    // PIDL made here is freed here, after the last call that reads it (the
    // children point into their item's PIDL).
    unsafe {
        let pidls: Vec<*mut ITEMIDLIST> = items
            .iter()
            .map(|p| ILCreateFromPathW(nul(p).as_ptr()))
            .filter(|p| !p.is_null())
            .collect();
        let parent = match folder {
            Some(folder) if pidls.len() > 1 => ILCreateFromPathW(nul(folder).as_ptr()),
            _ => core::ptr::null_mut(),
        };
        if parent.is_null() {
            for &pidl in &pidls {
                SHOpenFolderAndSelectItems(pidl, 0, core::ptr::null(), 0);
            }
        } else {
            let children: Vec<*const ITEMIDLIST> = pidls
                .iter()
                .map(|&p| ILFindLastID(p).cast_const())
                .collect();
            SHOpenFolderAndSelectItems(parent, children.len() as u32, children.as_ptr(), 0);
            ILFree(parent);
        }
        for pidl in pidls {
            ILFree(pidl);
        }
    }
}

/// IDataObject's interface id.
const IID_IDATA_OBJECT: GUID = GUID::from_u128(0x0000010e_0000_0000_c000_000000000046);

/// The start of every COM object's method table: IUnknown's three methods.
/// Only Release is called.
#[repr(C)]
struct IUnknownVtbl {
    _query_interface: usize,
    _add_ref: usize,
    release: unsafe extern "system" fn(this: *mut core::ffi::c_void) -> u32,
}

/// Drops this code's reference to a COM object.
///
/// # Safety
/// `object` must be a live COM interface pointer this code holds a
/// reference to; it must not be used afterwards.
unsafe fn release(object: *mut core::ffi::c_void) {
    // SAFETY: a COM object starts with a pointer to its method table, which
    // starts with IUnknown's methods.
    unsafe {
        let vtbl = *(object as *const *const IUnknownVtbl);
        ((*vtbl).release)(object);
    }
}

/// The deepest folder holding every one of `pidls` (absolute, at least
/// one), as a new PIDL the caller frees. It is never one of them: a folder
/// selected with a file inside it is held by the folder's parent.
///
/// # Safety
/// Each of `pidls` must be a valid absolute PIDL.
unsafe fn common_folder(pidls: &[*mut ITEMIDLIST]) -> *mut ITEMIDLIST {
    // SAFETY: the clone is this function's until returned; removing IDs
    // from it stops at the desktop's empty PIDL, the parent of everything.
    unsafe {
        let folder = ILClone(pidls[0]);
        if folder.is_null() {
            return folder;
        }
        ILRemoveLastID(folder);
        for &pidl in &pidls[1..] {
            while (ILIsParent(folder, pidl, 0) == 0 || ILIsEqual(folder, pidl) != 0)
                && ILRemoveLastID(folder) != 0
            {}
        }
        folder
    }
}

/// Properties sheets waiting for the sheet thread, with their owners.
struct SheetQueue {
    jobs: Mutex<VecDeque<(usize, Vec<Vec<u16>>)>>,
    /// The sheet thread's id, for waking it.
    thread: u32,
}

/// The message that tells the sheet thread there is work.
const WAKE: u32 = WM_APP;

/// The sheet thread, made the first time Properties is asked for.
static SHEETS: OnceLock<Option<Arc<SheetQueue>>> = OnceLock::new();

/// Shows the shell's Properties for `paths`: one item's own sheet, or one
/// sheet for several (counts, "Multiple Types", "Various Folders", the
/// summed size), as Explorer shows for a selection. Returns at once.
///
/// The work runs on a thread of its own, not the window's: each path is
/// parsed into an item ID list, which reads the disk (thousands of selected
/// rows take seconds). The thread lives on and runs a message loop rather
/// than ending with the job, as the threads of `in_background` do: the
/// shell shows the sheet on yet another thread, which calls back into the
/// data object made here for as long as the sheet is open, and that object
/// belongs to this thread's COM apartment.
pub fn properties(hwnd: HWND, paths: Vec<Vec<u16>>) {
    if paths.is_empty() || dry_run("properties", paths.iter().map(|p| &p[..])) {
        return;
    }
    let Some(sheets) = SHEETS.get_or_init(spawn_sheet_thread) else {
        // Without a thread there is nothing to run it on; the sheet is
        // lost, as it would be had the shell failed.
        return;
    };
    sheets
        .jobs
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .push_back((hwnd as usize, paths));
    // SAFETY: posts to a thread of this process. If its queue does not
    // exist yet, it looks at the jobs once it does (see `sheet_thread`).
    unsafe { PostThreadMessageW(sheets.thread, WAKE, 0, 0) };
}

fn spawn_sheet_thread() -> Option<Arc<SheetQueue>> {
    let (tx, rx) = std::sync::mpsc::channel::<Arc<SheetQueue>>();
    let handle = std::thread::Builder::new()
        .name("goz-properties".into())
        .spawn(move || {
            if let Ok(queue) = rx.recv() {
                sheet_thread(&queue);
            }
        })
        .ok()?;
    // SAFETY: the handle of the thread just started, open until `handle`
    // is dropped below.
    let thread = unsafe { GetThreadId(handle.as_raw_handle()) };
    let queue = Arc::new(SheetQueue {
        jobs: Mutex::new(VecDeque::new()),
        thread,
    });
    tx.send(Arc::clone(&queue)).ok()?;
    Some(queue)
}

fn sheet_thread(queue: &SheetQueue) {
    // SAFETY: COM for the shell calls, for the life of the thread; then a
    // standard message loop over a zeroed MSG.
    unsafe {
        CoInitializeEx(
            core::ptr::null(),
            (COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE) as u32,
        );
        let mut msg: MSG = core::mem::zeroed();
        // Makes the thread's message queue, so that a wake posted from now
        // on waits in it; jobs pushed before it are found below anyway.
        PeekMessageW(&mut msg, core::ptr::null_mut(), 0, 0, PM_NOREMOVE);
        loop {
            // After every message, not only after a wake: a modal loop
            // inside a COM call may have taken the wake.
            loop {
                let job = queue
                    .jobs
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .pop_front();
                let Some((owner, paths)) = job else { break };
                properties_now(owner as HWND, &paths);
            }
            if GetMessageW(&mut msg, core::ptr::null_mut(), 0, 0) <= 0 {
                return;
            }
            if msg.hwnd.is_null() && msg.message == WAKE {
                continue;
            }
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
}

/// Starts the Properties sheet for `paths`, on the sheet thread.
fn properties_now(hwnd: HWND, paths: &[Vec<u16>]) {
    match paths {
        [] => {}
        [one] => {
            let file = nul(one);
            // SAFETY: `file` is NUL-terminated.
            unsafe {
                SHObjectProperties(hwnd, SHOP_FILEPATH as u32, file.as_ptr(), core::ptr::null())
            };
        }
        many => {
            // SAFETY: each path is NUL-terminated for the call that parses
            // it; the children point into their item's PIDL, and
            // SHCreateDataObject copies what it keeps, so every PIDL is
            // freed after it; the data object is released once the sheet
            // holds its own reference.
            unsafe {
                let pidls: Vec<*mut ITEMIDLIST> = many
                    .iter()
                    .map(|p| ILCreateFromPathW(nul(p).as_ptr()))
                    .filter(|p| !p.is_null())
                    .collect();
                // The sheet wants the items as children of a folder: with
                // the desktop as their folder and the full PIDLs as children
                // it shows only "The properties for this item are not
                // available". Their common folder works, items in several
                // folders included ("Various Folders").
                let folder = if pidls.is_empty() {
                    core::ptr::null_mut()
                } else {
                    common_folder(&pidls)
                };
                let mut data = core::ptr::null_mut();
                let made = !folder.is_null() && {
                    let children: Vec<*const ITEMIDLIST> = pidls
                        .iter()
                        .map(|&p| ILFindChild(folder, p).cast_const())
                        .collect();
                    SHCreateDataObject(
                        folder,
                        children.len() as u32,
                        children.as_ptr(),
                        core::ptr::null_mut(),
                        &IID_IDATA_OBJECT,
                        &mut data,
                    ) >= 0
                };
                if !folder.is_null() {
                    ILFree(folder);
                }
                for pidl in pidls {
                    ILFree(pidl);
                }
                if made && !data.is_null() {
                    SHMultiFileProperties(data, 0);
                    release(data);
                }
            }
        }
    }
}

/// Runs `f` on a thread of its own with COM ready. Shell calls that show UI
/// (a UAC prompt) or start something slow (a program, a browser, Explorer)
/// go here, so the window never waits on them.
///
/// A thread per action rather than one long-lived shell thread: a launch
/// that hangs (an unreachable network path, a stuck handler, a UAC prompt
/// left open) then holds up only itself, never the opens after it; nothing
/// stays alive between actions, which keeps goz-gui's idle memory where it
/// is; and the thread's own STA ends with the action, so nothing in it
/// waits on a message loop no one runs. Calls that would finish their work
/// asynchronously on the thread are told not to (SEE_MASK_NOASYNC).
/// Properties cannot end with its call, so it has its own thread
/// ([`properties`]).
pub fn in_background(f: impl FnOnce() + Send + 'static) {
    RUNNING.fetch_add(1, Ordering::SeqCst);
    let spawned = std::thread::Builder::new()
        .name("goz-shell".into())
        .spawn(move || {
            // SAFETY: initialises COM for this thread, which ends below.
            let hr = unsafe {
                CoInitializeEx(
                    core::ptr::null(),
                    (COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE) as u32,
                )
            };
            f();
            if hr >= 0 {
                // SAFETY: pairs the successful CoInitializeEx above.
                unsafe { CoUninitialize() };
            }
            RUNNING.fetch_sub(1, Ordering::SeqCst);
        });
    // Without a thread there is nothing to run it on; the action is lost,
    // as it would be had the shell failed.
    if spawned.is_err() {
        RUNNING.fetch_sub(1, Ordering::SeqCst);
    }
}

/// The actions of [`in_background`] still running.
static RUNNING: AtomicUsize = AtomicUsize::new(0);

/// Waits, up to `limit`, for the actions of [`in_background`] still
/// running: the process is ending (an open was just handed to the shell
/// when Options > Results' "Close window on execute" closed the window),
/// and ending would cut them off.
pub fn wait_for_launches(limit: Duration) {
    let end = Instant::now() + limit;
    while RUNNING.load(Ordering::SeqCst) > 0 && Instant::now() < end {
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// One ShellExecuteEx. `verb` `None` is the default verb; `wait_ms` waits
/// that long for the program started to end. True once it started (and,
/// when waited for, ended within the time).
fn execute(
    owner: usize,
    verb: Option<&str>,
    file: &[u16],
    params: Option<&str>,
    dir: Option<&[u16]>,
    show: i32,
    wait_ms: Option<u32>,
) -> bool {
    let verb = verb.map(wz);
    let file = nul(file);
    let params = params.map(wz);
    let dir = dir.map(nul);
    let ptr = |s: &Option<Vec<u16>>| s.as_ref().map_or(core::ptr::null(), |s| s.as_ptr());
    let mut info = SHELLEXECUTEINFOW {
        cbSize: size_of::<SHELLEXECUTEINFOW>() as u32,
        // The thread may end right after: finish any asynchronous part first.
        fMask: SEE_MASK_NOASYNC
            | if wait_ms.is_some() {
                SEE_MASK_NOCLOSEPROCESS
            } else {
                0
            },
        hwnd: owner as HWND,
        lpVerb: ptr(&verb),
        lpFile: file.as_ptr(),
        lpParameters: ptr(&params),
        lpDirectory: ptr(&dir),
        nShow: show,
        ..Default::default()
    };
    // SAFETY: every string is NUL-terminated and outlives the call; the
    // process handle, if any, is closed once waited on.
    unsafe {
        if ShellExecuteExW(&mut info) == 0 {
            return false;
        }
        match wait_ms {
            Some(ms) if !info.hProcess.is_null() => {
                let done = WaitForSingleObject(info.hProcess, ms) == 0;
                CloseHandle(info.hProcess);
                done
            }
            _ => true,
        }
    }
}

/// Runs each of `paths` as administrator; Windows asks the user first.
pub fn run_as(hwnd: HWND, paths: Vec<Vec<u16>>) {
    if paths.is_empty() || dry_run("run-as", paths.iter().map(|p| &p[..])) {
        return;
    }
    let owner = hwnd as usize;
    in_background(move || {
        for path in paths {
            let dir = parent_folder(&path);
            execute(owner, Some("runas"), &path, None, dir, SW_SHOWNORMAL, None);
        }
    });
}

/// Opens a web page in the user's browser.
pub fn open_url(hwnd: HWND, url: &'static str) {
    let link: Vec<u16> = url.encode_utf16().collect();
    if dry_run("url", [&link[..]]) {
        return;
    }
    let owner = hwnd as usize;
    in_background(move || {
        let url: Vec<u16> = url.encode_utf16().collect();
        execute(owner, None, &url, None, None, SW_SHOWNORMAL, None);
    });
}

/// What Tools does to the goz service.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Service {
    Start,
    Stop,
    Restart,
}

impl Service {
    /// The program and arguments that do it, given the system folder. `net
    /// stop` waits for the service to stop, so the start that follows it
    /// finds it stopped; `/y` answers its question about dependent services
    /// should there ever be any, since nobody sees its window.
    fn command(self, system: &str) -> (String, String) {
        let name = goz_core::proto::SERVICE_NAME;
        match self {
            Service::Start => (format!(r"{system}\sc.exe"), format!("start {name}")),
            Service::Stop => (format!(r"{system}\sc.exe"), format!("stop {name}")),
            Service::Restart => (
                format!(r"{system}\cmd.exe"),
                format!(
                    r#"/d /s /c ""{system}\net.exe" stop {name} /y & "{system}\net.exe" start {name}""#
                ),
            ),
        }
    }
}

/// The system folder, such as `C:\Windows\system32`.
fn system_dir() -> String {
    let mut buf = [0u16; 260];
    // SAFETY: the buffer and its length.
    let n = unsafe { GetSystemDirectoryW(buf.as_mut_ptr(), buf.len() as u32) } as usize;
    if n == 0 || n > buf.len() {
        return r"C:\Windows\System32".into();
    }
    String::from_utf16_lossy(&buf[..n])
}

/// Starts, stops or restarts the goz service, elevated (Windows asks the
/// user first), off the UI thread. When the command has finished, `hwnd`
/// gets `msg` with wParam 1, or 0 if it never ran (the user said no).
pub fn service(hwnd: HWND, what: Service, msg: u32) {
    let verb = match what {
        Service::Start => "service-start",
        Service::Stop => "service-stop",
        Service::Restart => "service-restart",
    };
    if dry_run(verb, []) {
        // Nothing ran, so there is nothing to search again for.
        // SAFETY: posts to this thread's window.
        unsafe { PostMessageW(hwnd, msg, 0, 0) };
        return;
    }
    let owner = hwnd as usize;
    in_background(move || {
        let (exe, params) = what.command(&system_dir());
        let exe: Vec<u16> = exe.encode_utf16().collect();
        // Long enough for gozd to stop and start; a start that hangs past it
        // is left to the list's own retries.
        let ran = execute(
            owner,
            Some("runas"),
            &exe,
            Some(&params),
            None,
            SW_HIDE,
            Some(60_000),
        );
        // SAFETY: posts to the window, which ignores it if it is gone.
        unsafe { PostMessageW(owner as HWND, msg, usize::from(ran), 0) };
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn w(s: &str) -> Vec<u16> {
        s.encode_utf16().collect()
    }

    #[test]
    fn service_commands() {
        let sys = r"C:\Windows\system32";
        assert_eq!(
            Service::Start.command(sys),
            (r"C:\Windows\system32\sc.exe".into(), "start goz".into())
        );
        assert_eq!(
            Service::Stop.command(sys),
            (r"C:\Windows\system32\sc.exe".into(), "stop goz".into())
        );
        assert_eq!(
            Service::Restart.command(sys),
            (
                r"C:\Windows\system32\cmd.exe".into(),
                r#"/d /s /c ""C:\Windows\system32\net.exe" stop goz /y & "C:\Windows\system32\net.exe" start goz""#
                    .into()
            )
        );
        assert!(system_dir().to_ascii_lowercase().ends_with("system32"));
    }

    #[test]
    fn programs_run_and_are_waited_for_off_the_calling_thread() {
        // The same path the Tools commands take, with a harmless program
        // and no elevation: a hidden cmd that ends at once.
        let (tx, rx) = std::sync::mpsc::channel();
        in_background(move || {
            let exe: Vec<u16> = format!(r"{}\cmd.exe", system_dir())
                .encode_utf16()
                .collect();
            let ran = execute(
                0,
                None,
                &exe,
                Some("/d /c exit 3"),
                None,
                SW_HIDE,
                Some(10_000),
            );
            tx.send(ran).expect("send");
        });
        assert_eq!(
            rx.recv_timeout(std::time::Duration::from_secs(15)),
            Ok(true)
        );
    }

    #[test]
    fn parent_folders() {
        let parent = |s: &str| parent_folder(&w(s)).map(String::from_utf16_lossy);
        assert_eq!(parent(r"C:\a\b.exe").as_deref(), Some(r"C:\a"));
        assert_eq!(parent(r"C:\b.exe").as_deref(), Some(r"C:\"));
        assert_eq!(
            parent(r"\\srv\share\f.txt").as_deref(),
            Some(r"\\srv\share")
        );
        assert_eq!(parent(r"C:\"), None, "a volume root");
        assert_eq!(parent("b.exe"), None);
    }

    #[test]
    fn open_path_opens_one_window_per_folder() {
        let paths: Vec<Vec<u16>> = [
            r"C:\a\1.txt",
            r"C:\b\2.txt",
            r"C:\a\3.txt",
            r"C:\",
            r"C:\a",
            r"C:\a\4.txt",
            r"D:\",
        ]
        .iter()
        .map(|s| w(s))
        .collect();
        let groups: Vec<(Option<String>, Vec<usize>)> = by_folder(&paths)
            .into_iter()
            .map(|(f, m)| (f.map(String::from_utf16_lossy), m))
            .collect();
        assert_eq!(
            groups,
            [
                (Some(r"C:\a".into()), vec![0, 2, 5]),
                (Some(r"C:\b".into()), vec![1]),
                (None, vec![3]),
                (Some(r"C:\".into()), vec![4]),
                (None, vec![6]),
            ]
        );
        assert_eq!(reveal_windows(&paths), 5);
        assert_eq!(reveal_windows(&[]), 0);
    }
}
