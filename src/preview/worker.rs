//! The preview's thread: everything that reads a file or calls into the
//! shell for the preview pane runs here, never on the window's thread. It
//! decodes images (WIC), reads the start of text files, asks the shell for
//! icons and thumbnails, and drives preview handlers: COM objects Windows
//! runs in a surrogate process (prevhost.exe) that draw into a window of
//! ours, and that can take long or stop answering altogether.
//!
//! The thread is a COM single-threaded apartment with a message loop (calls
//! from a handler's process into our frame object arrive as messages). The
//! window hands it jobs through a queue, newest first (a preview asked for
//! replaces one not started), and the thread answers through a queue of
//! its own and a message to the pane. While it works it says what on
//! (`Phase`), so the pane can see a handler that does not return: such a
//! thread is abandoned (`Worker::abandon`), left to finish whenever its call
//! returns, and a new thread takes over.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use windows::Win32::Foundation::{
    COLORREF, E_NOINTERFACE, E_POINTER, HWND as WHWND, RECT as WRECT, REGDB_E_CLASSNOTREG, S_FALSE,
    SIZE,
};
use windows::Win32::Graphics::Gdi::LOGFONTW;
use windows::Win32::System::Com::{
    CLSCTX_ACTIVATE_32_BIT_SERVER, CLSCTX_LOCAL_SERVER, CoCreateInstance, STGM_READ,
};
use windows::Win32::System::Ole::IObjectWithSite;
use windows::Win32::UI::Shell::PropertiesSystem::{IInitializeWithFile, IInitializeWithStream};
use windows::Win32::UI::Shell::{
    IInitializeWithItem, IPreviewHandler, IPreviewHandlerFrame, IPreviewHandlerFrame_Impl,
    IPreviewHandlerVisuals, IShellItem, IShellItemImageFactory, PREVIEWHANDLERFRAMEINFO,
    SHCreateItemFromParsingName, SIIGBF_ICONONLY, SIIGBF_RESIZETOFIT, SIIGBF_THUMBNAILONLY,
};
use windows::Win32::UI::WindowsAndMessaging::{HACCEL as WHACCEL, MSG as WMSG};
use windows::core::{GUID, Interface, PCWSTR, implement};
use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, HWND, WAIT_OBJECT_0};
use windows_sys::Win32::Globalization::{CP_ACP, MultiByteToWideChar};
use windows_sys::Win32::Graphics::Gdi::{DeleteObject, HBITMAP};
use windows_sys::Win32::Storage::FileSystem::{
    FILE_ATTRIBUTE_DIRECTORY, FILE_ATTRIBUTE_OFFLINE, FILE_ATTRIBUTE_RECALL_ON_DATA_ACCESS,
    GetFileAttributesW, INVALID_FILE_ATTRIBUTES,
};
use windows_sys::Win32::System::Com::{
    COINIT_APARTMENTTHREADED, COINIT_DISABLE_OLE1DDE, CoFreeUnusedLibrariesEx, CoInitializeEx,
    CoUninitialize,
};
use windows_sys::Win32::System::Registry::{HKEY_CURRENT_USER, RRF_RT_REG_DWORD, RegGetValueW};
use windows_sys::Win32::System::Threading::{CreateEventW, INFINITE, SetEvent};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, VK_CONTROL, VK_MENU, VK_SHIFT, VK_TAB,
};
use windows_sys::Win32::UI::Shell::{ASSOCF_NONE, ASSOCSTR_SHELLEXTENSION, AssocQueryStringW};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    ACCEL, CopyAcceleratorTableW, CreateAcceleratorTableW, DestroyAcceleratorTable,
    DispatchMessageW, FALT, FCONTROL, FSHIFT, FVIRTKEY, HACCEL, MSG, MWMO_INPUTAVAILABLE,
    MsgWaitForMultipleObjectsEx, PM_REMOVE, PeekMessageW, PostMessageW, QS_ALLINPUT,
    TranslateMessage, WM_COMMAND, WM_KEYDOWN, WM_SYSKEYDOWN,
};

use super::image::{Decoded, Wic};
use super::settings::{Handlers, PreviewSettings};
use super::stream::FileStream;
use super::text;
use crate::thumbs::premultiplied;
use crate::wide::{nul, wz};

/// What the pane asks to see.
pub struct Request {
    pub generation: u64,
    pub path: Box<[u16]>,
    /// The content's size in pixels: the box an image is fitted into, the
    /// preview handler's window.
    pub size: (i32, i32),
    /// The icon's size, for items with no preview.
    pub icon_side: i32,
    /// The theme's colours and the window's font, for preview handlers that
    /// take them (`IPreviewHandlerVisuals`).
    pub bg: u32,
    pub fg: u32,
    pub font: LOGFONTW,
}

/// What the thread made of a request.
pub enum Shown {
    /// An image, or a file's thumbnail: drawn fitted to the pane.
    Picture(Decoded),
    /// No preview: the item's icon (if the shell had one). `online_only`:
    /// a cloud file whose data is not on this computer, which is not read
    /// (that would download it).
    Icon {
        icon: Option<Decoded>,
        online_only: bool,
    },
    /// The start of a text file, as the text box takes it.
    Text(Vec<u16>),
    /// The preview handler draws it in the host window.
    Handler,
    /// Nothing to show: the settings leave files or folders out.
    Nothing,
    /// The item could not be read, and why.
    Failed(String),
}

pub struct Reply {
    pub generation: u64,
    pub shown: Shown,
}

/// What the thread is doing, for the pane's watchdog.
#[derive(Clone, Copy, Debug)]
pub struct Phase {
    pub generation: u64,
    /// In a call to a preview handler (which may never return).
    pub handler: bool,
    pub since: Instant,
}

enum Content {
    Show(Request),
    Clear,
}

/// The jobs waiting; each kind only its newest.
#[derive(Default)]
struct Jobs {
    content: Option<Content>,
    size: Option<(i32, i32)>,
    focus: bool,
    quit: bool,
}

/// Preview handlers that stopped answering, whose threads have not ended:
/// past `MAX_HUNG` of them no more handlers are started.
static HUNG: AtomicUsize = AtomicUsize::new(0);
const MAX_HUNG: usize = 3;

/// Preview handlers' COM interface, `IPreviewHandler`: the value
/// `ASSOCSTR_SHELLEXTENSION` looks up under a type's `shellex`.
const PREVIEW_HANDLER: &str = "{8895b1c6-b41f-4c1c-a562-0d564250836f}";
/// Windows' own plain-text preview handler, which goz-gui's text view
/// replaces: it shows the same text, at once and in the window's theme.
const TXT_PREVIEWER: GUID = GUID::from_u128(0x1531d583_8375_4d3f_b5fb_d23bbd169f22);

struct Shared {
    jobs: Mutex<Jobs>,
    /// Set when a job is queued (auto-reset).
    work: usize,
    /// Set as the thread ends (manual reset).
    done: usize,
    replies: Mutex<Vec<Reply>>,
    phase: Mutex<Option<Phase>>,
    /// The main window, which outlives the pane: told of replies with
    /// `reply_msg`, given the keys the handler passes on as commands, and
    /// handed the host window back as the thread ends (`retire_msg`).
    main: usize,
    reply_msg: u32,
    retire_msg: u32,
    /// The window preview handlers draw in (the pane's thread's).
    host: usize,
    abandoned: AtomicBool,
    settings: PreviewSettings,
    /// `GOZ_GUI_PREVIEW_STALL_MS`: a test's stand-in for a handler that
    /// does not return.
    stall_ms: u32,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

impl Shared {
    fn wake(&self) {
        // SAFETY: a live event handle, owned by `Shared`.
        unsafe { SetEvent(self.work as HANDLE) };
    }

    fn set_phase(&self, generation: u64, handler: bool) {
        *lock(&self.phase) = Some(Phase {
            generation,
            handler,
            since: Instant::now(),
        });
    }

    fn reply(&self, reply: Reply) {
        lock(&self.replies).push(reply);
        // SAFETY: posting to the main window; if it is gone the reply
        // waits in the queue and drops with it.
        unsafe { PostMessageW(self.main as HWND, self.reply_msg, 0, 0) };
    }
}

impl Drop for Shared {
    fn drop(&mut self) {
        // SAFETY: the events are owned here and closed once, after both
        // the thread and the pane let go.
        unsafe {
            CloseHandle(self.work as HANDLE);
            CloseHandle(self.done as HANDLE);
        }
    }
}

/// The pane's handle on its thread.
pub struct Worker {
    shared: Arc<Shared>,
}

impl Worker {
    /// Starts a thread that tells `main` of its replies with `reply_msg`,
    /// draws preview handlers in `host`, passes keys to `main` (as
    /// `WM_COMMAND`) and hands `host` back to it as it ends (`retire_msg`,
    /// the host in wParam).
    pub fn spawn(
        main: HWND,
        reply_msg: u32,
        retire_msg: u32,
        host: HWND,
        settings: PreviewSettings,
    ) -> Option<Worker> {
        // SAFETY: two unnamed events, closed by `Shared`'s drop.
        let (work, done) = unsafe {
            (
                CreateEventW(core::ptr::null(), 0, 0, core::ptr::null()),
                CreateEventW(core::ptr::null(), 1, 0, core::ptr::null()),
            )
        };
        if work.is_null() || done.is_null() {
            // SAFETY: whichever was made is closed again.
            unsafe {
                if !work.is_null() {
                    CloseHandle(work);
                }
                if !done.is_null() {
                    CloseHandle(done);
                }
            }
            return None;
        }
        let stall_ms = std::env::var("GOZ_GUI_PREVIEW_STALL_MS")
            .ok()
            .and_then(|v| v.trim().parse().ok())
            .unwrap_or(0);
        let shared = Arc::new(Shared {
            jobs: Mutex::new(Jobs::default()),
            work: work as usize,
            done: done as usize,
            replies: Mutex::new(Vec::new()),
            phase: Mutex::new(None),
            main: main as usize,
            reply_msg,
            retire_msg,
            host: host as usize,
            abandoned: AtomicBool::new(false),
            settings,
            stall_ms,
        });
        let thread = Arc::clone(&shared);
        std::thread::Builder::new()
            .name("goz-preview".into())
            .spawn(move || run(&thread))
            .ok()?;
        Some(Worker { shared })
    }

    fn jobs(&self) -> MutexGuard<'_, Jobs> {
        lock(&self.shared.jobs)
    }

    /// Asks for `request` to be shown, in place of any not started yet.
    pub fn show(&self, request: Request) {
        self.jobs().content = Some(Content::Show(request));
        self.shared.wake();
    }

    /// Nothing to show: the preview handler unloads its file.
    pub fn clear(&self) {
        self.jobs().content = Some(Content::Clear);
        self.shared.wake();
    }

    /// The content's new size: a preview handler showing follows it.
    pub fn resize(&self, size: (i32, i32)) {
        self.jobs().size = Some(size);
        self.shared.wake();
    }

    /// The pane took the keyboard: the preview handler's window takes it.
    pub fn focus(&self) {
        self.jobs().focus = true;
        self.shared.wake();
    }

    /// Asks the thread to let its handler go and end; it hands the host
    /// window back to the main window as it does.
    pub fn quit(&self) {
        self.jobs().quit = true;
        self.shared.wake();
    }

    /// Gives up on the thread, which is stuck in a call: it ends whenever
    /// that returns, and nothing it makes is wanted any more.
    pub fn abandon(&self) {
        if !self.shared.abandoned.swap(true, Ordering::SeqCst) {
            HUNG.fetch_add(1, Ordering::SeqCst);
        }
        self.quit();
    }

    /// The replies made since the last call.
    pub fn take_replies(&self) -> Vec<Reply> {
        std::mem::take(&mut *lock(&self.shared.replies))
    }

    pub fn phase(&self) -> Option<Phase> {
        *lock(&self.shared.phase)
    }

    pub fn host(&self) -> HWND {
        self.shared.host as HWND
    }

    /// Waits up to `timeout` for the thread to end, answering what other
    /// threads and processes send this one meanwhile (a preview handler
    /// unloading tells its window's parent, which is ours). True when it
    /// ended.
    pub fn wait(&self, timeout: Duration) -> bool {
        use windows_sys::Win32::UI::WindowsAndMessaging::{PM_NOREMOVE, PM_QS_SENDMESSAGE};
        let end = Instant::now() + timeout;
        let done = self.shared.done as HANDLE;
        loop {
            let left = end.saturating_duration_since(Instant::now());
            // SAFETY: waits on a live event; PeekMessage with
            // PM_QS_SENDMESSAGE only answers sent messages, leaving every
            // posted one in the queue.
            unsafe {
                let r = MsgWaitForMultipleObjectsEx(
                    1,
                    &done,
                    left.as_millis() as u32,
                    windows_sys::Win32::UI::WindowsAndMessaging::QS_SENDMESSAGE,
                    MWMO_INPUTAVAILABLE,
                );
                if r == WAIT_OBJECT_0 {
                    return true;
                }
                if left.is_zero() {
                    return false;
                }
                let mut msg: MSG = core::mem::zeroed();
                PeekMessageW(
                    &mut msg,
                    core::ptr::null_mut(),
                    0,
                    0,
                    PM_NOREMOVE | PM_QS_SENDMESSAGE,
                );
            }
        }
    }
}

impl Drop for Worker {
    /// A thread nobody holds any more ends (after its current job).
    fn drop(&mut self) {
        self.quit();
    }
}

/// Whether preview handlers may be used now: as the settings say (with
/// `0`, Explorer's "Show preview handlers in preview pane", on unless
/// turned off), and while not too many hang.
fn handlers_allowed(settings: &PreviewSettings) -> bool {
    if HUNG.load(Ordering::SeqCst) >= MAX_HUNG {
        return false;
    }
    match settings.handlers {
        Handlers::Always => true,
        Handlers::Never => false,
        Handlers::System => {
            let key = wz(r"Software\Microsoft\Windows\CurrentVersion\Explorer\Advanced");
            let value = wz("ShowPreviewHandlers");
            let mut data: u32 = 1;
            let mut size = size_of::<u32>() as u32;
            // SAFETY: reads one DWORD into `data`, whose size is passed.
            let rc = unsafe {
                RegGetValueW(
                    HKEY_CURRENT_USER,
                    key.as_ptr(),
                    value.as_ptr(),
                    RRF_RT_REG_DWORD,
                    core::ptr::null_mut(),
                    (&raw mut data).cast(),
                    &mut size,
                )
            };
            // Missing is Explorer's default, on.
            rc != 0 || data != 0
        }
    }
}

/// A GUID written `{xxxxxxxx-xxxx-xxxx-xxxx-xxxxxxxxxxxx}` (braces optional).
pub fn parse_guid(text: &str) -> Option<GUID> {
    let t = text.trim();
    let t = t
        .strip_prefix('{')
        .and_then(|t| t.strip_suffix('}'))
        .unwrap_or(t);
    let groups: Vec<&str> = t.split('-').collect();
    let lens = [8, 4, 4, 4, 12];
    if groups.len() != 5
        || groups
            .iter()
            .zip(lens)
            .any(|(g, n)| g.len() != n || !g.bytes().all(|b| b.is_ascii_hexdigit()))
    {
        return None;
    }
    u128::from_str_radix(&groups.concat(), 16)
        .ok()
        .map(GUID::from_u128)
}

/// The preview handler registered for extension `ext` (with its dot).
fn handler_for(ext: &[u16]) -> Option<GUID> {
    let ext = nul(ext);
    let iid = wz(PREVIEW_HANDLER);
    let mut buf = [0u16; 64];
    let mut len = buf.len() as u32;
    // SAFETY: NUL-terminated strings; the buffer with its length.
    let hr = unsafe {
        AssocQueryStringW(
            ASSOCF_NONE,
            ASSOCSTR_SHELLEXTENSION,
            ext.as_ptr(),
            iid.as_ptr(),
            buf.as_mut_ptr(),
            &mut len,
        )
    };
    if hr != 0 {
        return None;
    }
    let text = buf.split(|&c| c == 0).next().unwrap_or_default();
    parse_guid(&String::from_utf16_lossy(text))
}

/// `path`'s extension, lower case with its dot; empty with none.
fn extension(path: &[u16]) -> Vec<u16> {
    let name_at = path
        .iter()
        .rposition(|&c| c == u16::from(b'\\'))
        .map_or(0, |i| i + 1);
    let name = &path[name_at..];
    match name.iter().rposition(|&c| c == u16::from(b'.')) {
        Some(dot) if dot + 1 < name.len() => String::from_utf16_lossy(&name[dot..])
            .to_lowercase()
            .encode_utf16()
            .collect(),
        _ => Vec::new(),
    }
}

/// `GOZ_GUI_PREVIEW_LOG=1`: each preview made is noted in `preview.log` in
/// the data folder (what it showed, in how many ms, of which file), and
/// each thread that ends after it was given up on.
fn log_on() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| {
        std::env::var("GOZ_GUI_PREVIEW_LOG").is_ok_and(|v| !v.trim().is_empty() && v.trim() != "0")
    })
}

/// Notes `what` in the test log, if it is kept.
pub fn note(what: &str) {
    if log_on() {
        log(what, Duration::ZERO, &[]);
    }
}

fn log(what: &str, took: Duration, path: &[u16]) {
    let line = format!(
        "{what}\t{}\t{}\r\n",
        took.as_millis(),
        String::from_utf16_lossy(path)
    );
    crate::store::location::append_data_file("preview.log", &line);
}

/// The system's text for error `code`.
fn error_text(code: u32) -> String {
    use windows_sys::Win32::System::Diagnostics::Debug::{
        FORMAT_MESSAGE_FROM_SYSTEM, FORMAT_MESSAGE_IGNORE_INSERTS, FormatMessageW,
    };
    let mut buf = [0u16; 512];
    // SAFETY: the buffer with its length; no inserts are read.
    let n = unsafe {
        FormatMessageW(
            FORMAT_MESSAGE_FROM_SYSTEM | FORMAT_MESSAGE_IGNORE_INSERTS,
            core::ptr::null(),
            code,
            0,
            buf.as_mut_ptr(),
            buf.len() as u32,
            core::ptr::null(),
        )
    };
    let text = String::from_utf16_lossy(&buf[..n as usize]);
    let text = text.trim();
    if text.is_empty() {
        format!("Error {code}.")
    } else {
        text.to_owned()
    }
}

/// The start of `path` as the text view shows it, if it is text.
fn read_text(path: &[u16]) -> Option<Vec<u16>> {
    use std::io::Read;
    use std::os::windows::ffi::OsStringExt;
    let path = std::path::PathBuf::from(std::ffi::OsString::from_wide(path));
    // Opened sharing reading, writing and deleting: the file can be
    // renamed or deleted while it is read.
    let file = std::fs::File::open(path).ok()?;
    let mut bytes = Vec::with_capacity(text::LIMIT + 1);
    file.take(text::LIMIT as u64 + 1)
        .read_to_end(&mut bytes)
        .ok()?;
    let truncated = bytes.len() > text::LIMIT;
    bytes.truncate(text::LIMIT);
    text::text_view(&bytes, truncated, ansi)
}

/// `bytes` in the system's ANSI code page.
fn ansi(bytes: &[u8]) -> Vec<u16> {
    if bytes.is_empty() {
        return Vec::new();
    }
    let len = bytes.len().min(i32::MAX as usize) as i32;
    let mut out = vec![0u16; bytes.len()];
    // SAFETY: both buffers with their lengths; a code page character is
    // never more UTF-16 units than bytes.
    let n = unsafe {
        MultiByteToWideChar(
            CP_ACP,
            0,
            bytes.as_ptr(),
            len,
            out.as_mut_ptr(),
            out.len() as i32,
        )
    };
    out.truncate(n.max(0) as usize);
    out
}

/// A shell image of `path` `side` pixels square, premultiplied.
fn shell_image(
    path: &[u16],
    side: i32,
    flags: windows::Win32::UI::Shell::SIIGBF,
) -> Option<Decoded> {
    let path = nul(path);
    // SAFETY: COM calls on this thread's objects; the bitmap the shell
    // returns is ours, copied and deleted.
    unsafe {
        let factory: IShellItemImageFactory =
            SHCreateItemFromParsingName(PCWSTR(path.as_ptr()), None).ok()?;
        let made = factory.GetImage(SIZE { cx: side, cy: side }, flags).ok()?;
        let made = made.0 as HBITMAP;
        let bitmap = premultiplied(made);
        DeleteObject(made);
        let bitmap = bitmap?;
        let natural = (bitmap.width, bitmap.height);
        Some(Decoded {
            bitmap,
            natural,
            opaque: false,
        })
    }
}

/// The icon of the file `path` (extension `ext`, lower case with its dot),
/// `side` pixels square. A program's, shortcut's or icon file's own icon is
/// extracted off the system image lists (`bigicon`: the shell's factory
/// would keep each such file's icon in the jumbo list for good, about
/// 50 KB a program); any other file shows its type's icon, from the shell.
fn icon_image(path: &[u16], ext: &[u16], side: i32) -> Option<Decoded> {
    let own = ext
        .split_first()
        .is_some_and(|(_, e)| crate::icons::wants_own(e));
    let Some(bitmap) = own.then(|| crate::bigicon::own_icon(path, side)).flatten() else {
        // The type's icon: one entry of the shell's lists for every file
        // of the type.
        return shell_image(path, side, SIIGBF_ICONONLY);
    };
    let natural = (bitmap.width, bitmap.height);
    Some(Decoded {
        bitmap,
        natural,
        opaque: false,
    })
}

/// A preview handler held: its class, and whether a file is loaded in it.
struct Loaded {
    clsid: GUID,
    handler: IPreviewHandler,
    loaded: bool,
}

/// The thread's state.
struct State<'a> {
    shared: &'a Shared,
    wic: Option<Wic>,
    handler: Option<Loaded>,
    frame: Option<IPreviewHandlerFrame>,
    accel: Option<Accel>,
    /// The content's size now.
    size: (i32, i32),
    /// The request shown last, for phases outside a request.
    generation: u64,
    /// How the handler last loaded was given its file (the test log).
    init: Option<Init>,
}

impl State<'_> {
    fn show(&mut self, r: Request) {
        let start = Instant::now();
        // The file shown before is let go first, whatever comes next: a
        // call to the handler, timed as one for the item shown before (if
        // it hangs, the new item is asked for again on a new thread).
        self.unload();
        self.generation = r.generation;
        self.size = r.size;
        self.shared.set_phase(r.generation, false);
        let shown = self.make(&r);
        *lock(&self.shared.phase) = None;
        if log_on() {
            log(&self.describe(&shown), start.elapsed(), &r.path);
        }
        if !self.shared.abandoned.load(Ordering::SeqCst) {
            self.shared.reply(Reply {
                generation: r.generation,
                shown,
            });
        }
    }

    /// What `shown` is, for the test log.
    fn describe(&self, shown: &Shown) -> String {
        match shown {
            Shown::Picture(d) => format!(
                "picture {}x{} of {}x{}",
                d.bitmap.width, d.bitmap.height, d.natural.0, d.natural.1
            ),
            Shown::Icon { icon, online_only } => format!(
                "icon {}{}",
                icon.as_ref().map_or("none".into(), |i| format!(
                    "{}x{}",
                    i.bitmap.width, i.bitmap.height
                )),
                if *online_only { " online-only" } else { "" }
            ),
            Shown::Text(t) => format!("text {}", t.len()),
            Shown::Handler => format!(
                "handler {:?} by {:?}",
                self.handler.as_ref().map(|h| h.clsid).unwrap_or_default(),
                self.init
            ),
            Shown::Nothing => "nothing".into(),
            Shown::Failed(why) => format!("failed {why}"),
        }
    }

    fn make(&mut self, r: &Request) -> Shown {
        let settings = self.shared.settings;
        let path = nul(&r.path);
        // SAFETY: a NUL-terminated path.
        let attrs = unsafe { GetFileAttributesW(path.as_ptr()) };
        if attrs == INVALID_FILE_ATTRIBUTES {
            // SAFETY: reads this thread's last error.
            let code = unsafe { windows_sys::Win32::Foundation::GetLastError() };
            return Shown::Failed(error_text(code));
        }
        if attrs & FILE_ATTRIBUTE_DIRECTORY != 0 {
            if !settings.folders {
                return Shown::Nothing;
            }
            return Shown::Icon {
                icon: shell_image(&r.path, r.icon_side, SIIGBF_RESIZETOFIT),
                online_only: false,
            };
        }
        if !settings.files {
            return Shown::Nothing;
        }
        // A cloud file not on this computer: reading it would download it.
        if attrs & (FILE_ATTRIBUTE_RECALL_ON_DATA_ACCESS | FILE_ATTRIBUTE_OFFLINE) != 0 {
            return Shown::Icon {
                icon: shell_image(&r.path, r.icon_side, SIIGBF_ICONONLY),
                online_only: true,
            };
        }
        let ext = extension(&r.path);
        if !ext.is_empty() {
            if self.wic.is_none() {
                self.wic = Wic::new();
            }
            let side = settings.load_side().max(r.size.0).max(r.size.1);
            if let Some(wic) = self.wic.as_mut()
                && wic.decodes(&ext)
                && let Ok(decoded) = wic.decode(&r.path, side)
            {
                return Shown::Picture(decoded);
            }
            let handler = handlers_allowed(&settings)
                .then(|| handler_for(&ext))
                .flatten()
                .filter(|&clsid| clsid != TXT_PREVIEWER);
            if let Some(clsid) = handler {
                if self.show_handler(r, clsid).is_ok() {
                    return Shown::Handler;
                }
                // What follows is no handler's call.
                self.shared.set_phase(r.generation, false);
            }
        }
        if let Some(text) = read_text(&r.path) {
            return Shown::Text(text);
        }
        // A thumbnail (a video's frame, a document's page), else the icon.
        let thumb_side = r.size.0.max(r.size.1).clamp(32, 1024);
        if let Some(thumb) = shell_image(&r.path, thumb_side, SIIGBF_THUMBNAILONLY) {
            return Shown::Picture(thumb);
        }
        Shown::Icon {
            icon: icon_image(&r.path, &ext, r.icon_side),
            online_only: false,
        }
    }

    /// Shows `r` through the preview handler `clsid`: the one held if it is
    /// of that class (unloaded, it takes the new file), else a new one.
    fn show_handler(&mut self, r: &Request, clsid: GUID) -> windows::core::Result<()> {
        self.shared.set_phase(r.generation, true);
        let reused = match self.handler.take() {
            Some(held) if held.clsid == clsid => Some(held.handler),
            Some(other) => {
                self.release(other);
                None
            }
            None => None,
        };
        let was_reused = reused.is_some();
        let handler = match reused {
            Some(h) => h,
            None => create(clsid)?,
        };
        match self.load(&handler, r) {
            Ok(()) => {
                self.handler = Some(Loaded {
                    clsid,
                    handler,
                    loaded: true,
                });
                Ok(())
            }
            // A handler that cannot take a second file: a new one.
            Err(_) if was_reused => {
                self.release(Loaded {
                    clsid,
                    handler,
                    loaded: true,
                });
                let fresh = create(clsid)?;
                let result = self.load(&fresh, r);
                self.handler = Some(Loaded {
                    clsid,
                    handler: fresh,
                    loaded: result.is_ok(),
                });
                result
            }
            Err(e) => {
                // Kept, unloaded, for the next file of its type.
                // SAFETY: a COM call on this thread's handler.
                let _ = unsafe { handler.Unload() };
                self.handler = Some(Loaded {
                    clsid,
                    handler,
                    loaded: false,
                });
                Err(e)
            }
        }
    }

    /// Hands `r`'s file to `handler` and has it draw in the host window.
    fn load(&mut self, handler: &IPreviewHandler, r: &Request) -> windows::core::Result<()> {
        self.init = Some(initialize(handler, &r.path)?);
        let frame = self.frame();
        // SAFETY: COM calls on this thread's handler; the rectangle and the
        // font outlive the calls.
        unsafe {
            if let Ok(visuals) = handler.cast::<IPreviewHandlerVisuals>() {
                let _ = visuals.SetBackgroundColor(COLORREF(r.bg));
                let _ = visuals.SetTextColor(COLORREF(r.fg));
                let _ = visuals.SetFont(&r.font);
            }
            if let (Ok(site), Some(frame)) = (handler.cast::<IObjectWithSite>(), frame) {
                let _ = site.SetSite(&frame);
            }
            let rc = WRECT {
                left: 0,
                top: 0,
                right: r.size.0.max(1),
                bottom: r.size.1.max(1),
            };
            handler.SetWindow(WHWND(self.shared.host as *mut _), &rc)?;
            if let Err(e) = handler.DoPreview() {
                let _ = handler.Unload();
                return Err(e);
            }
        }
        if self.shared.stall_ms > 0 {
            // The test hook's stand-in for a DoPreview that does not
            // return: the handler's window is up, the call is not over,
            // and this thread answers calls into it meanwhile, as COM's
            // wait for a call does.
            pump_for(Duration::from_millis(u64::from(self.shared.stall_ms)));
        }
        Ok(())
    }

    /// The frame preview handlers pass keys to.
    fn frame(&mut self) -> Option<IPreviewHandlerFrame> {
        if self.frame.is_none() {
            let accel = self.accel.get_or_insert_with(Accel::new);
            let frame = Frame {
                main: self.shared.main,
                accel: accel.handle as usize,
                entries: accel.entries.clone(),
            };
            self.frame = Some(frame.into());
        }
        self.frame.clone()
    }

    /// The file in the handler held is let go (the handler is kept).
    fn unload(&mut self) {
        let generation = self.generation;
        if let Some(held) = self.handler.as_mut()
            && held.loaded
        {
            self.shared.set_phase(generation, true);
            held.loaded = false;
            // SAFETY: a COM call on this thread's handler.
            let _ = unsafe { held.handler.Unload() };
        }
    }

    /// Lets `held` go: its file, its site and the object.
    fn release(&self, held: Loaded) {
        self.shared.set_phase(self.generation, true);
        // SAFETY: COM calls on this thread's handler, before it is
        // released by the drop.
        unsafe {
            if held.loaded {
                let _ = held.handler.Unload();
            }
            if let Ok(site) = held.handler.cast::<IObjectWithSite>() {
                let _ = site.SetSite(None);
            }
        }
    }

    fn clear(&mut self) {
        self.unload();
        if self.shared.settings.release_on_clear
            && let Some(held) = self.handler.take()
        {
            self.release(held);
        }
        *lock(&self.shared.phase) = None;
    }

    fn resize(&mut self, size: (i32, i32)) {
        self.size = size;
        if let Some(held) = self.handler.as_ref()
            && held.loaded
        {
            self.shared.set_phase(self.generation, true);
            let rc = WRECT {
                left: 0,
                top: 0,
                right: size.0.max(1),
                bottom: size.1.max(1),
            };
            // SAFETY: a COM call on this thread's handler.
            let _ = unsafe { held.handler.SetRect(&rc) };
            *lock(&self.shared.phase) = None;
        }
    }

    fn focus(&mut self) {
        if let Some(held) = self.handler.as_ref()
            && held.loaded
        {
            self.shared.set_phase(self.generation, true);
            // SAFETY: a COM call on this thread's handler.
            let _ = unsafe { held.handler.SetFocus() };
            *lock(&self.shared.phase) = None;
        }
    }

    /// The thread is ending: the handler, the frame and the table go.
    fn end(&mut self) {
        if let Some(held) = self.handler.take() {
            self.release(held);
        }
        self.frame = None;
        self.accel = None;
        self.wic = None;
    }
}

/// A new preview handler of class `clsid`, in a surrogate process; a
/// 32-bit handler where only that is registered.
fn create(clsid: GUID) -> windows::core::Result<IPreviewHandler> {
    // SAFETY: COM is initialized on this thread.
    unsafe {
        match CoCreateInstance(&clsid, None, CLSCTX_LOCAL_SERVER) {
            Err(e) if e.code() == REGDB_E_CLASSNOTREG => CoCreateInstance(
                &clsid,
                None,
                CLSCTX_LOCAL_SERVER | CLSCTX_ACTIVATE_32_BIT_SERVER,
            ),
            other => other,
        }
    }
}

/// How a preview handler was given its file.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Init {
    Stream,
    File,
    Item,
}

/// Hands `path` to `handler` the first way it takes: as a stream goz-gui
/// opens sharing it with everyone (so the file can be renamed or deleted
/// while it is previewed), by path (it opens the file itself, as it
/// likes), or as a shell item.
fn initialize(handler: &IPreviewHandler, path: &[u16]) -> windows::core::Result<Init> {
    let file = nul(path);
    let at = PCWSTR(file.as_ptr());
    // SAFETY: COM calls on this thread's handler with a NUL-terminated
    // path that outlives them.
    unsafe {
        if let Ok(init) = handler.cast::<IInitializeWithStream>()
            && let Some(stream) = FileStream::open(path)
            && init.Initialize(&stream, STGM_READ.0).is_ok()
        {
            return Ok(Init::Stream);
        }
        if let Ok(init) = handler.cast::<IInitializeWithFile>()
            && init.Initialize(at, STGM_READ.0).is_ok()
        {
            return Ok(Init::File);
        }
        if let Ok(init) = handler.cast::<IInitializeWithItem>()
            && let Ok(item) = SHCreateItemFromParsingName::<_, _, IShellItem>(at, None)
            && init.Initialize(&item, STGM_READ.0).is_ok()
        {
            return Ok(Init::Item);
        }
    }
    Err(E_NOINTERFACE.into())
}

/// The keys a preview handler passes on to its frame: the window's
/// shortcuts, and Tab and Shift+Tab leaving the handler.
struct Accel {
    handle: HACCEL,
    entries: Vec<ACCEL>,
}

/// The commands Tab and Shift+Tab pass to the main window: the keyboard
/// leaves the preview handler for the next or previous tab stop
/// (`menu::F1_FIRST` and the next; window/preview.rs runs them).
pub const TAB_OUT: u16 = crate::menu::F1_FIRST;
pub const TAB_BACK_OUT: u16 = crate::menu::F1_FIRST + 1;

impl Accel {
    fn new() -> Accel {
        let window = crate::keys::accelerators();
        let mut entries = Vec::new();
        // SAFETY: asks for the table's size, then copies it into a buffer
        // of that size; the temporary table is destroyed.
        unsafe {
            let n = CopyAcceleratorTableW(window, core::ptr::null_mut(), 0).max(0);
            entries.resize(
                n as usize,
                ACCEL {
                    fVirt: 0,
                    key: 0,
                    cmd: 0,
                },
            );
            let got = CopyAcceleratorTableW(window, entries.as_mut_ptr(), n).max(0);
            entries.truncate(got as usize);
            DestroyAcceleratorTable(window);
        }
        entries.push(ACCEL {
            fVirt: FVIRTKEY,
            key: VK_TAB,
            cmd: TAB_OUT,
        });
        entries.push(ACCEL {
            fVirt: FVIRTKEY | FSHIFT,
            key: VK_TAB,
            cmd: TAB_BACK_OUT,
        });
        // SAFETY: the entries and their count; Windows copies them.
        let handle = unsafe { CreateAcceleratorTableW(entries.as_ptr(), entries.len() as i32) };
        Accel { handle, entries }
    }
}

impl Drop for Accel {
    fn drop(&mut self) {
        if !self.handle.is_null() {
            // SAFETY: the table made in `new`, destroyed once.
            unsafe { DestroyAcceleratorTable(self.handle) };
        }
    }
}

/// The command `entries` give key `vk` with the modifiers held (`ctrl`,
/// `shift`, `alt`), if any.
pub fn command_for(entries: &[ACCEL], vk: u16, ctrl: bool, shift: bool, alt: bool) -> Option<u16> {
    let mods = (if ctrl { FCONTROL } else { 0 })
        | (if shift { FSHIFT } else { 0 })
        | (if alt { FALT } else { 0 });
    entries
        .iter()
        .find(|e| e.key == vk && e.fVirt & (FCONTROL | FSHIFT | FALT) == mods)
        .map(|e| e.cmd)
}

/// goz-gui's side of a preview handler: the keys the handler does not use
/// come back here, and the window's shortcuts among them go to the main
/// window, so F6, Shift+Esc, Alt+P... work while the handler has the
/// keyboard.
#[implement(IPreviewHandlerFrame)]
struct Frame {
    main: usize,
    accel: usize,
    entries: Vec<ACCEL>,
}

impl IPreviewHandlerFrame_Impl for Frame_Impl {
    fn GetWindowContext(&self) -> windows::core::Result<PREVIEWHANDLERFRAMEINFO> {
        Ok(PREVIEWHANDLERFRAMEINFO {
            haccel: WHACCEL(self.accel as *mut _),
            cAccelEntries: self.entries.len() as u32,
        })
    }

    fn TranslateAccelerator(&self, pmsg: *const WMSG) -> windows::core::Result<()> {
        // SAFETY: the handler passes a message of its own, alive across
        // the call, or null.
        let msg = unsafe { pmsg.as_ref() }.ok_or(windows::core::Error::from(E_POINTER))?;
        if msg.message != WM_KEYDOWN && msg.message != WM_SYSKEYDOWN {
            return Err(S_FALSE.into());
        }
        // The key went to the handler's process: what is held now is the
        // keyboard's own state, not this thread's.
        // SAFETY: reads the keyboard's state.
        let held = |vk: u16| unsafe { GetAsyncKeyState(i32::from(vk)) } < 0;
        let alt = msg.message == WM_SYSKEYDOWN || held(VK_MENU);
        let cmd = command_for(
            &self.entries,
            msg.wParam.0 as u16,
            held(VK_CONTROL),
            held(VK_SHIFT),
            alt,
        );
        let Some(cmd) = cmd else {
            return Err(S_FALSE.into());
        };
        // SAFETY: posts to the main window, which may be gone (then
        // nothing happens).
        unsafe { PostMessageW(self.main as HWND, WM_COMMAND, usize::from(cmd), 0) };
        Ok(())
    }
}

/// Answers this thread's messages for `time`, and nothing else.
fn pump_for(time: Duration) {
    let end = Instant::now() + time;
    loop {
        let left = end.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return;
        }
        // SAFETY: waits for this thread's messages and dispatches them.
        unsafe {
            MsgWaitForMultipleObjectsEx(
                0,
                core::ptr::null(),
                left.as_millis() as u32,
                QS_ALLINPUT,
                MWMO_INPUTAVAILABLE,
            );
            let mut msg: MSG = core::mem::zeroed();
            while PeekMessageW(&mut msg, core::ptr::null_mut(), 0, 0, PM_REMOVE) != 0 {
                TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
    }
}

/// Waits for a job, answering messages meanwhile (COM's calls into this
/// apartment come as messages).
fn wait_for_work(shared: &Shared) {
    let work = shared.work as HANDLE;
    loop {
        // SAFETY: waits on a live event; the messages are this thread's.
        unsafe {
            let r =
                MsgWaitForMultipleObjectsEx(1, &work, INFINITE, QS_ALLINPUT, MWMO_INPUTAVAILABLE);
            if r == WAIT_OBJECT_0 {
                return;
            }
            let mut msg: MSG = core::mem::zeroed();
            while PeekMessageW(&mut msg, core::ptr::null_mut(), 0, 0, PM_REMOVE) != 0 {
                TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
    }
}

fn run(shared: &Shared) {
    // SAFETY: COM for this thread, as an apartment of its own: preview
    // handlers, WIC and the shell's image factories want one.
    let hr = unsafe {
        CoInitializeEx(
            core::ptr::null(),
            (COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE) as u32,
        )
    };
    let mut state = State {
        shared,
        wic: None,
        handler: None,
        frame: None,
        accel: None,
        size: (0, 0),
        generation: 0,
        init: None,
    };
    loop {
        let jobs = std::mem::take(&mut *lock(&shared.jobs));
        if jobs.quit || shared.abandoned.load(Ordering::SeqCst) {
            break;
        }
        let busy = jobs.content.is_some() || jobs.size.is_some() || jobs.focus;
        match jobs.content {
            Some(Content::Show(r)) => state.show(r),
            Some(Content::Clear) => state.clear(),
            None => {}
        }
        if let Some(size) = jobs.size {
            state.resize(size);
        }
        if jobs.focus {
            state.focus();
        }
        if !busy {
            wait_for_work(shared);
        }
    }
    state.end();
    drop(state);
    if hr >= 0 {
        // SAFETY: every COM object of the thread is released above; the
        // libraries none uses any more (WIC's codecs, the shell's image
        // makers) go at once rather than in ten minutes, then COM ends on
        // this thread.
        unsafe {
            CoFreeUnusedLibrariesEx(0, 0);
            CoUninitialize();
        }
    }
    if shared.abandoned.load(Ordering::SeqCst) {
        HUNG.fetch_sub(1, Ordering::SeqCst);
        note("ended after being given up on");
    }
    *lock(&shared.phase) = None;
    // SAFETY: a live event; then the host window goes back to the main
    // window, which destroys it (it may be gone itself: then nothing).
    unsafe {
        SetEvent(shared.done as HANDLE);
        PostMessageW(shared.main as HWND, shared.retire_msg, shared.host, 0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn guids_parse_with_or_without_braces() {
        let g = parse_guid("{8895b1c6-b41f-4c1c-a562-0d564250836f}").expect("a GUID");
        assert_eq!(g, GUID::from_u128(0x8895b1c6_b41f_4c1c_a562_0d564250836f));
        assert_eq!(
            parse_guid("1531D583-8375-4D3F-B5FB-D23BBD169F22"),
            Some(TXT_PREVIEWER)
        );
        assert_eq!(parse_guid(""), None);
        assert_eq!(parse_guid("{8895b1c6-b41f-4c1c-a562-0d564250836}"), None);
        assert_eq!(parse_guid("{8895b1c6-b41f-4c1c-a562-0d564250836x}"), None);
        assert_eq!(parse_guid("8895b1c6b41f4c1ca5620d564250836f"), None);
    }

    #[test]
    fn extensions_are_the_names_lower_cased() {
        let ext = |p: &str| String::from_utf16_lossy(&extension(&wz(p)[..p.len()]));
        assert_eq!(ext(r"C:\a\Photo.JPG"), ".jpg");
        assert_eq!(ext(r"C:\a.b\readme"), "");
        assert_eq!(ext(r"C:\a\archive.tar.gz"), ".gz");
        assert_eq!(ext(r"C:\a\dot."), "");
        assert_eq!(ext(r"C:\a\.gitignore"), ".gitignore");
    }

    #[test]
    fn keys_the_handler_passes_on_find_their_commands() {
        let entries = [
            ACCEL {
                fVirt: FVIRTKEY,
                key: 0x75, // F6
                cmd: 3611,
            },
            ACCEL {
                fVirt: FVIRTKEY | FSHIFT,
                key: 0x75,
                cmd: 3612,
            },
            ACCEL {
                fVirt: FVIRTKEY | FALT,
                key: u16::from(b'P'),
                cmd: 3603,
            },
        ];
        assert_eq!(command_for(&entries, 0x75, false, false, false), Some(3611));
        assert_eq!(command_for(&entries, 0x75, false, true, false), Some(3612));
        assert_eq!(command_for(&entries, 0x75, true, false, false), None);
        assert_eq!(
            command_for(&entries, u16::from(b'P'), false, false, true),
            Some(3603)
        );
        assert_eq!(
            command_for(&entries, u16::from(b'P'), false, false, false),
            None
        );
        // The window's own table, with Tab out added.
        let accel = Accel::new();
        assert!(!accel.handle.is_null());
        assert_eq!(
            command_for(&accel.entries, VK_TAB, false, false, false),
            Some(TAB_OUT)
        );
        assert_eq!(
            command_for(&accel.entries, VK_TAB, false, true, false),
            Some(TAB_BACK_OUT)
        );
        assert_eq!(
            command_for(&accel.entries, u16::from(b'P'), false, false, true),
            Some(crate::menu::VIEW_PANE_PREVIEW)
        );
    }
}
