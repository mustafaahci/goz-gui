//! The preview pane (Everything's `preview-pane`): View > Preview (Alt+P)
//! opens a pane in the right dock that shows the focused result. Images are
//! goz-gui's own to draw (decoded by WIC, fitted to the pane with their
//! aspect, turned upright as their EXIF orientation says); other files go
//! to the preview handler Windows has registered for their type (the PDF,
//! Office, font previewers...), drawing into a window of the pane; text
//! files no handler takes show their start in a text box; folders and
//! everything else show their icon (or thumbnail), name and details.
//!
//! Nothing here waits on a file or the shell: `worker` does that on a
//! thread of its own and answers through the main window. The focus moving
//! through the results is followed after it rests (150 ms), the file shown
//! is kept if it is still the one focused (a live update refreshing the
//! rows reloads nothing), and a preview handler that does not return within
//! 3 s is given up on: the pane says so and a new thread takes over. While
//! the window is hidden or minimized, and when the pane closes, the thread
//! and its handler are let go.
//!
//! Re-entrancy: no `RefCell` borrow of the pane's state is held across a
//! call that sends messages (showing, moving or filling its windows, the
//! list's state, the menu).

mod image;
mod settings;
mod stream;
mod text;
mod worker;

use std::any::Any;
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::{Duration, Instant};

use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows_sys::Win32::Graphics::Gdi::{
    AC_SRC_ALPHA, AC_SRC_OVER, AlphaBlend, BLENDFUNCTION, BeginPaint, BitBlt, CLEARTYPE_QUALITY,
    CLIP_DEFAULT_PRECIS, COLORONCOLOR, CreateCompatibleBitmap, CreateCompatibleDC, CreateFontW,
    DEFAULT_CHARSET, DT_CALCRECT, DT_CENTER, DT_EDITCONTROL, DT_END_ELLIPSIS, DT_NOPREFIX,
    DT_WORDBREAK, DeleteDC, DeleteObject, DrawFocusRect, DrawTextW, EndPaint, FF_MODERN,
    FIXED_PITCH, FW_NORMAL, HALFTONE, HBITMAP, HDC, HFONT, InvalidateRect, OUT_DEFAULT_PRECIS,
    PAINTSTRUCT, SRCCOPY, SelectObject, SetBkMode, SetBrushOrgEx, SetStretchBltMode, SetTextColor,
    StretchBlt, TRANSPARENT,
};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::Controls::{EM_SETLIMITTEXT, EM_SETRECTNP, EM_SETSEL};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    GetDoubleClickTime, GetFocus, GetKeyState, SetFocus, VK_CONTROL, VK_MENU,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, ES_AUTOHSCROLL, ES_AUTOVSCROLL, ES_MULTILINE,
    ES_READONLY, GW_CHILD, GWLP_USERDATA, GetClientRect, GetMessageTime, GetParent,
    GetSystemMetrics, GetWindow, GetWindowLongPtrW, HMENU, HWND_MESSAGE, HWND_TOP, IDC_ARROW,
    IsChild, IsIconic, IsWindow, IsWindowVisible, KillTimer, LoadCursorW, MSG, MoveWindow,
    PostMessageW, RegisterClassExW, SM_CXDOUBLECLK, SM_CYDOUBLECLK, SW_HIDE, SW_SHOWNA,
    SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SWP_SHOWWINDOW, SendMessageW, SetParent, SetTimer,
    SetWindowLongPtrW, SetWindowPos, SetWindowTextW, ShowWindow, WM_COMMAND, WM_CONTEXTMENU,
    WM_ERASEBKGND, WM_KEYDOWN, WM_KILLFOCUS, WM_LBUTTONDOWN, WM_PAINT, WM_SETFOCUS, WM_SETFONT,
    WM_TIMER, WNDCLASSEXW, WS_CHILD, WS_CLIPCHILDREN, WS_CLIPSIBLINGS, WS_HSCROLL, WS_VSCROLL,
};

use crate::client::Row;
use crate::dock::PaneId;
use crate::paint::fill;
use crate::pane::{PaneContent, PaneEvent, PaneSpec};
use crate::wide::wz;
use crate::window::App;
use crate::window::slots::F1_MSG;
use crate::{filetype, format, menu, theme};
use image::{Decoded, fit};
pub use settings::PreviewSettings;
use worker::{Request, Shown, Worker};
pub use worker::{TAB_BACK_OUT, TAB_OUT};

/// The main window's messages for the preview (F1's range): a preview
/// thread ended and hands its host window back (wParam), and replies are
/// waiting.
pub const WM_RETIRE: u32 = F1_MSG;
pub const WM_REPLIES: u32 = F1_MSG + 1;

/// The pane window's timers: the focus rested; a preview is slow enough
/// to say so; the watchdog's tick; the pane grew past the image decoded.
const T_FOCUS: usize = 1;
const T_SLOW: usize = 2;
const T_WATCH: usize = 3;
const T_REDECODE: usize = 4;
const FOCUS_DELAY: u32 = 150;
const SLOW_DELAY: u32 = 300;
const WATCH_TICK: u32 = 500;
const REDECODE_DELAY: u32 = 250;
/// A preview handler's call that has not returned by then is given up on.
const HANDLER_TIMEOUT: Duration = Duration::from_secs(3);
/// Anything else (a file on a network that does not answer, a huge image)
/// gets this long.
const WORK_TIMEOUT: Duration = Duration::from_secs(15);
/// How long the window, ending, waits for the handler to let its file go.
const EXIT_WAIT: Duration = Duration::from_millis(500);

/// The icon's side at 96 DPI, for items with no preview.
const ICON_SIDE: i32 = 128;
/// Room round the icon and the text, and between them, at 96 DPI.
const MARGIN: i32 = 12;
const GAP: i32 = 8;

const ID_TEXT: usize = 1;
const ID_HOST: usize = 2;
const HOST_CLASS: &str = "goz-gui preview host";

const NOTHING_SELECTED: &str = "Select an item to preview.";
const LOADING: &str = "Loading\u{2026}";
const NOT_RESPONDING: &str = "The preview handler is not responding.";
const TOO_LONG: &str = "The preview is taking too long.";
const ONLINE_ONLY: &str = "This file is available online only.";
const NO_THREAD: &str = "The preview could not be started.";

thread_local! {
    /// Host windows given back while their thread may still be in a
    /// preview handler, whose window is then still in them: they wait,
    /// hidden and off every window, until that thread ends (`retired`).
    static GRAVEYARD: RefCell<Vec<usize>> = const { RefCell::new(Vec::new()) };
}

/// Registers the preview pane, with the settings it starts with.
pub fn register(app: &App, settings: PreviewSettings) {
    register_host_class();
    app.register_pane(PaneSpec {
        min_high: 64,
        ..PaneSpec::new(
            PaneId::Preview,
            Box::new(move || Rc::new(PreviewPane::new(settings)) as Rc<dyn PaneContent>),
        )
    });
}

/// The pane's content while it is open.
fn open_pane(app: &App) -> Option<Rc<dyn PaneContent>> {
    app.pane_content(PaneId::Preview)
        .filter(|c| c.as_any().is::<PreviewPane>())
}

/// The window is ending: the preview handler gets a moment to let its file
/// go before the windows go.
pub fn exit(app: &App) {
    if let Some(content) = open_pane(app)
        && let Some(pane) = content.as_any().downcast_ref::<PreviewPane>()
    {
        pane.shutdown(app);
    }
}

/// The preview's thread has replies (`WM_REPLIES`).
pub fn replies_ready(app: &App) {
    if let Some(content) = open_pane(app)
        && let Some(pane) = content.as_any().downcast_ref::<PreviewPane>()
    {
        pane.take_replies();
    }
}

/// A preview thread ended (`WM_RETIRE`): its host window, if it was kept
/// waiting for that, goes now.
pub fn retired(host: usize) {
    let known = GRAVEYARD.with(|g| {
        let mut g = g.borrow_mut();
        let at = g.iter().position(|&h| h == host);
        at.map(|i| g.swap_remove(i)).is_some()
    });
    if known {
        // SAFETY: a window of this thread that the graveyard held, so it
        // was never destroyed meanwhile.
        unsafe { DestroyWindow(host as HWND) };
        worker::note("parked host destroyed");
    }
}

/// Lets a host window go: at once if no preview handler's window is in
/// it, else hidden and parked outside every window until its thread ends
/// (destroying it would wait on the handler's process).
fn retire_host(host: HWND) {
    if host.is_null() {
        return;
    }
    // SAFETY: windows of this thread; the host's child, if any, is the
    // handler's, which is only looked at.
    unsafe {
        if IsWindow(host) == 0 {
            return;
        }
        // The keyboard, if it is in the handler's window, comes back to
        // the pane rather than stay in a hidden window.
        let focus = GetFocus();
        if !focus.is_null() && (focus == host || IsChild(host, focus) != 0) {
            SetFocus(GetParent(host));
        }
        ShowWindow(host, SW_HIDE);
        if GetWindow(host, GW_CHILD).is_null() {
            DestroyWindow(host);
            worker::note("host destroyed");
            return;
        }
        SetParent(host, HWND_MESSAGE);
    }
    GRAVEYARD.with(|g| g.borrow_mut().push(host as usize));
    worker::note("host parked");
}

fn register_host_class() {
    let class = wz(HOST_CLASS);
    // SAFETY: registers a class of this module (again is refused,
    // harmlessly).
    unsafe {
        let wc = WNDCLASSEXW {
            cbSize: size_of::<WNDCLASSEXW>() as u32,
            lpfnWndProc: Some(host_proc),
            hInstance: GetModuleHandleW(core::ptr::null()),
            hCursor: LoadCursorW(core::ptr::null_mut(), IDC_ARROW),
            lpszClassName: class.as_ptr(),
            ..core::mem::zeroed()
        };
        RegisterClassExW(&wc);
    }
}

/// The window preview handlers draw in: it paints the background (its
/// GWLP_USERDATA) wherever the handler's window does not cover it.
unsafe extern "system" fn host_proc(hwnd: HWND, msg: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    // SAFETY: a standard paint of this window; `w` is the DC with
    // WM_ERASEBKGND.
    unsafe {
        match msg {
            WM_ERASEBKGND => {
                let mut rc = RECT::default();
                GetClientRect(hwnd, &mut rc);
                fill(w as HDC, &rc, GetWindowLongPtrW(hwnd, GWLP_USERDATA) as u32);
                1
            }
            WM_PAINT => {
                let mut ps: PAINTSTRUCT = core::mem::zeroed();
                let hdc = BeginPaint(hwnd, &mut ps);
                fill(
                    hdc,
                    &ps.rcPaint,
                    GetWindowLongPtrW(hwnd, GWLP_USERDATA) as u32,
                );
                EndPaint(hwnd, &ps);
                0
            }
            _ => DefWindowProcW(hwnd, msg, w, l),
        }
    }
}

/// The item asked for or shown.
#[derive(Clone)]
struct Target {
    path: Box<[u16]>,
    is_dir: bool,
    size: Option<u64>,
    mtime: Option<i64>,
    name: Vec<u16>,
    /// Its type, size and date, a line each.
    details: Vec<u16>,
}

impl Target {
    fn of(row: &Row) -> Target {
        let mut details: Vec<u16> = filetype::name(row).to_vec();
        if !row.is_dir
            && let Some(size) = row.size
        {
            push_line(&mut details, &format!("Size: {}", format::size_auto(size)));
        }
        if let Some(mtime) = row.mtime {
            let mut date = Vec::new();
            format::date(mtime, &mut date);
            if !date.is_empty() {
                push_line(&mut details, "Date modified: ");
                details.extend(date);
            }
        }
        Target {
            path: row.path.clone(),
            is_dir: row.is_dir,
            size: row.size,
            mtime: row.mtime,
            name: row.name().to_vec(),
            details,
        }
    }

    /// Whether `other` is the same file, unchanged: nothing to load again.
    fn same_item(&self, other: &Target) -> bool {
        self.path == other.path
            && self.is_dir == other.is_dir
            && self.size == other.size
            && self.mtime == other.mtime
    }
}

fn push_line(out: &mut Vec<u16>, line: &str) {
    if !out.is_empty() {
        out.push(u16::from(b'\n'));
    }
    out.extend(line.encode_utf16());
}

/// An image scaled for the pane once, kept until the pane's size or the
/// theme changes.
struct Scaled {
    width: i32,
    height: i32,
    bg: u32,
    smooth: bool,
    handle: HBITMAP,
}

impl Drop for Scaled {
    fn drop(&mut self) {
        // SAFETY: the bitmap is owned here and selected into no DC (each
        // draw selects it out again).
        unsafe { DeleteObject(self.handle) };
    }
}

/// What the pane shows.
enum View {
    /// Nothing selected.
    Empty,
    /// Waiting long enough to say so.
    Loading,
    Picture {
        image: Decoded,
        scaled: RefCell<Option<Scaled>>,
    },
    Icon {
        icon: Option<Decoded>,
        online_only: bool,
        scaled: RefCell<Option<Scaled>>,
    },
    /// The text box shows it.
    Text,
    /// The preview handler shows it in the host window.
    Handler,
    /// The item and what went wrong.
    Message(String),
    /// The settings leave it out.
    Nothing,
}

/// What is focused in the results.
enum Focus {
    None,
    /// A row whose data is not held yet (a refresh on its way).
    Unknown,
    Item(Target),
}

/// The focused result: the focused row if it is selected, else the first
/// selected one.
fn focused(app: &App) -> Focus {
    use windows_sys::Win32::UI::Controls::{
        LVIS_SELECTED, LVM_GETITEMCOUNT, LVM_GETITEMSTATE, LVM_GETNEXTITEM, LVNI_FOCUSED,
        LVNI_SELECTED,
    };
    let list = app.list;
    // SAFETY: questions to this thread's list; nothing of goz-gui's state
    // is borrowed across them.
    let index = unsafe {
        if SendMessageW(list, LVM_GETITEMCOUNT, 0, 0) <= 0 {
            return Focus::None;
        }
        let focus = SendMessageW(list, LVM_GETNEXTITEM, usize::MAX, LVNI_FOCUSED as isize);
        let selected = focus >= 0
            && SendMessageW(
                list,
                LVM_GETITEMSTATE,
                focus as usize,
                LVIS_SELECTED as isize,
            ) & LVIS_SELECTED as isize
                != 0;
        if selected {
            focus
        } else {
            SendMessageW(list, LVM_GETNEXTITEM, usize::MAX, LVNI_SELECTED as isize)
        }
    };
    if index < 0 {
        return Focus::None;
    }
    let index = index as usize;
    // A copy of the row, so no borrow of the results is held while the
    // shell names its type.
    let row = {
        let model = app.model.borrow();
        model
            .peek(index)
            .or_else(|| model.previous_row(index))
            .map(|r| Row {
                path: r.path.clone(),
                name_start: r.name_start,
                size: r.size,
                mtime: r.mtime,
                is_dir: r.is_dir,
                icon: Cell::new(r.icon.get()),
            })
    };
    match row {
        Some(row) => Focus::Item(Target::of(&row)),
        None => Focus::Unknown,
    }
}

/// Whether the main window is on screen (not hidden, not minimized).
fn on_screen(app: &App) -> bool {
    // SAFETY: reads this thread's window's state.
    unsafe { IsWindowVisible(app.hwnd) != 0 && IsIconic(app.hwnd) == 0 }
}

/// The monospaced font of the text view, at the window's DPI.
fn mono_font(app: &App) -> HFONT {
    // SAFETY: a font the pane owns and deletes.
    unsafe {
        CreateFontW(
            -app.scale(13),
            0,
            0,
            0,
            FW_NORMAL as i32,
            0,
            0,
            0,
            u32::from(DEFAULT_CHARSET),
            u32::from(OUT_DEFAULT_PRECIS),
            u32::from(CLIP_DEFAULT_PRECIS),
            u32::from(CLEARTYPE_QUALITY),
            u32::from(FIXED_PITCH | FF_MODERN),
            wz("Consolas").as_ptr(),
        )
    }
}

/// The window's font, as preview handlers that take one want it.
fn logfont(app: &App) -> windows::Win32::Graphics::Gdi::LOGFONTW {
    use windows::Win32::Graphics::Gdi::{GetObjectW, HGDIOBJ, LOGFONTW};
    let mut lf = LOGFONTW::default();
    // SAFETY: reads the font's description into a struct of its size.
    unsafe {
        GetObjectW(
            HGDIOBJ(app.ui_font() as *mut _),
            size_of::<LOGFONTW>() as i32,
            Some((&raw mut lf).cast()),
        )
    };
    lf
}

fn width(rc: &RECT) -> i32 {
    rc.right - rc.left
}

fn height(rc: &RECT) -> i32 {
    rc.bottom - rc.top
}

/// The preview pane's content.
pub struct PreviewPane {
    settings: PreviewSettings,
    pane: Cell<HWND>,
    /// The content's place in the pane window.
    rc: Cell<RECT>,
    text_box: Cell<HWND>,
    mono: Cell<HFONT>,
    worker: RefCell<Option<Worker>>,
    /// Each request's number; replies to older ones are dropped.
    generation: Cell<u64>,
    target: RefCell<Option<Target>>,
    /// A request waits for its reply, since.
    requested: Cell<Option<Instant>>,
    view: RefCell<View>,
    /// The main window is on screen: previews are made.
    window_shown: Cell<bool>,
    /// The last button press (time and place), to tell a double-click.
    last_down: Cell<Option<(u32, POINT)>>,
}

impl PreviewPane {
    fn new(settings: PreviewSettings) -> PreviewPane {
        PreviewPane {
            settings,
            pane: Cell::new(core::ptr::null_mut()),
            rc: Cell::new(RECT::default()),
            text_box: Cell::new(core::ptr::null_mut()),
            mono: Cell::new(core::ptr::null_mut()),
            worker: RefCell::new(None),
            generation: Cell::new(0),
            target: RefCell::new(None),
            requested: Cell::new(None),
            view: RefCell::new(View::Empty),
            window_shown: Cell::new(false),
            last_down: Cell::new(None),
        }
    }

    fn host(&self) -> HWND {
        self.worker
            .borrow()
            .as_ref()
            .map_or(core::ptr::null_mut(), Worker::host)
    }

    fn timer(&self, id: usize, ms: u32) {
        // SAFETY: a timer of this pane's window.
        unsafe { SetTimer(self.pane.get(), id, ms, None) };
    }

    fn kill(&self, id: usize) {
        // SAFETY: a timer of this pane's window.
        unsafe { KillTimer(self.pane.get(), id) };
    }

    fn invalidate(&self) {
        let rc = self.rc.get();
        // SAFETY: repaints this thread's pane.
        unsafe { InvalidateRect(self.pane.get(), &rc, 0) };
    }

    /// Follows the focus after it rests.
    fn soon(&self) {
        self.timer(T_FOCUS, FOCUS_DELAY);
    }

    /// Starts the thread (and its host window) if there is none.
    fn ensure_worker(&self, app: &App) -> bool {
        if self.worker.borrow().is_some() {
            return true;
        }
        let rc = self.rc.get();
        let class = wz(HOST_CLASS);
        // SAFETY: a hidden child of this thread's pane window.
        let host = unsafe {
            CreateWindowExW(
                0,
                class.as_ptr(),
                core::ptr::null(),
                WS_CHILD | WS_CLIPCHILDREN | WS_CLIPSIBLINGS,
                rc.left,
                rc.top,
                width(&rc).max(0),
                height(&rc).max(0),
                self.pane.get(),
                ID_HOST as HMENU,
                GetModuleHandleW(core::ptr::null()),
                core::ptr::null(),
            )
        };
        if host.is_null() {
            return false;
        }
        // SAFETY: the background the host paints, in its own data.
        unsafe { SetWindowLongPtrW(host, GWLP_USERDATA, app.pal.get().bg as isize) };
        let worker = Worker::spawn(app.hwnd, WM_REPLIES, WM_RETIRE, host, self.settings);
        let Some(worker) = worker else {
            // SAFETY: the host just made, with nothing in it.
            unsafe { DestroyWindow(host) };
            return false;
        };
        *self.worker.borrow_mut() = Some(worker);
        true
    }

    /// Asks for `target`; `quiet`: the view stays as it is until the answer
    /// (an image decoded again, bigger).
    fn request(&self, app: &App, target: Target, quiet: bool) {
        if !self.ensure_worker(app) {
            *self.target.borrow_mut() = Some(target);
            self.set_view(View::Message(NO_THREAD.into()));
            return;
        }
        let rc = self.rc.get();
        let generation = self.generation.get() + 1;
        self.generation.set(generation);
        let pal = app.pal.get();
        let request = Request {
            generation,
            path: target.path.clone(),
            size: (width(&rc).max(1), height(&rc).max(1)),
            icon_side: app.scale(ICON_SIDE),
            bg: pal.bg,
            fg: pal.fg,
            font: logfont(app),
        };
        *self.target.borrow_mut() = Some(target);
        if let Some(worker) = self.worker.borrow().as_ref() {
            worker.show(request);
        }
        self.requested.set(Some(Instant::now()));
        if !quiet {
            self.timer(T_SLOW, SLOW_DELAY);
        }
        self.timer(T_WATCH, WATCH_TICK);
    }

    /// Nothing to preview: the handler lets its file go.
    fn clear(&self) {
        let idle = self.target.borrow().is_none() && matches!(*self.view.borrow(), View::Empty);
        if idle {
            return;
        }
        self.generation.set(self.generation.get() + 1);
        self.target.borrow_mut().take();
        self.stop_waiting();
        if let Some(worker) = self.worker.borrow().as_ref() {
            worker.clear();
        }
        self.set_view(View::Empty);
    }

    fn stop_waiting(&self) {
        self.requested.set(None);
        self.kill(T_SLOW);
        self.kill(T_WATCH);
    }

    /// Lets the thread, its handler and the host go (the window hides, or
    /// the pane closes); the next preview starts them again.
    fn release(&self) {
        self.generation.set(self.generation.get() + 1);
        self.target.borrow_mut().take();
        self.stop_waiting();
        self.kill(T_REDECODE);
        let worker = self.worker.borrow_mut().take();
        if let Some(worker) = worker {
            worker.quit();
            retire_host(worker.host());
        }
        self.set_view(View::Empty);
    }

    /// The thread is stuck: it is left to end when it can, with its host.
    fn abandon(&self) {
        self.stop_waiting();
        let worker = self.worker.borrow_mut().take();
        if let Some(worker) = worker {
            worker.abandon();
            retire_host(worker.host());
        }
    }

    /// The window is ending: the handler gets `EXIT_WAIT` to let its file
    /// go, then the host goes whatever happened.
    fn shutdown(&self, _app: &App) {
        self.window_shown.set(false);
        self.stop_waiting();
        let worker = self.worker.borrow_mut().take();
        let Some(worker) = worker else { return };
        worker.quit();
        let host = worker.host();
        if worker.wait(EXIT_WAIT) {
            // The thread ended: no handler's window is left in the host.
            // SAFETY: this thread's host window.
            unsafe { DestroyWindow(host) };
        } else {
            retire_host(host);
        }
    }

    /// Shows `view`: its window (the text box, the host) comes up and the
    /// other goes; the keyboard, if it was in the one going, comes to the
    /// pane.
    fn set_view(&self, view: View) {
        let text = matches!(view, View::Text);
        let handler = matches!(view, View::Handler);
        let was_text = matches!(*self.view.borrow(), View::Text);
        // The old view's bitmaps go here.
        drop(self.view.replace(view));
        let (pane, edit, host) = (self.pane.get(), self.text_box.get(), self.host());
        // SAFETY: shows and hides this thread's windows; the host's child,
        // the handler's, is only asked about.
        unsafe {
            let focus = GetFocus();
            if text {
                ShowWindow(edit, SW_SHOWNA);
            }
            if handler && !host.is_null() {
                SetWindowPos(
                    host,
                    HWND_TOP,
                    0,
                    0,
                    0,
                    0,
                    SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_SHOWWINDOW,
                );
            }
            let leaving = |w: HWND| !w.is_null() && (focus == w || IsChild(w, focus) != 0);
            if !text {
                if leaving(edit) {
                    SetFocus(pane);
                }
                ShowWindow(edit, SW_HIDE);
                if was_text {
                    // The text's memory goes.
                    SetWindowTextW(edit, [0u16].as_ptr());
                }
            }
            if !handler && !host.is_null() {
                if leaving(host) {
                    SetFocus(pane);
                }
                ShowWindow(host, SW_HIDE);
            }
        }
        self.invalidate();
    }

    /// The focus rested: the item focused now is previewed, unless it is
    /// the one shown.
    fn refresh(&self, app: &App) {
        if !self.window_shown.get() {
            return;
        }
        match focused(app) {
            Focus::None => self.clear(),
            Focus::Unknown => {}
            Focus::Item(target) => {
                let same = self
                    .target
                    .borrow()
                    .as_ref()
                    .is_some_and(|t| t.same_item(&target));
                if !same {
                    self.request(app, target, false);
                }
            }
        }
    }

    /// Previews the item shown again from scratch (the theme or the DPI
    /// changed what it looks like).
    fn reload(&self, app: &App) {
        let target = self.target.borrow_mut().take();
        if let Some(target) = target {
            self.request(app, target, true);
        }
    }

    fn take_replies(&self) {
        let replies = match self.worker.borrow().as_ref() {
            Some(worker) => worker.take_replies(),
            None => return,
        };
        for reply in replies {
            if reply.generation != self.generation.get() {
                continue;
            }
            self.stop_waiting();
            self.show(reply.shown);
        }
    }

    fn show(&self, shown: Shown) {
        let view = match shown {
            Shown::Picture(image) => View::Picture {
                image,
                scaled: RefCell::new(None),
            },
            Shown::Icon { icon, online_only } => View::Icon {
                icon,
                online_only,
                scaled: RefCell::new(None),
            },
            Shown::Text(mut text) => {
                text.push(0);
                let edit = self.text_box.get();
                // SAFETY: a NUL-terminated text for this thread's text box.
                unsafe {
                    SetWindowTextW(edit, text.as_ptr());
                    SendMessageW(edit, EM_SETSEL, 0, 0);
                }
                View::Text
            }
            Shown::Handler => View::Handler,
            Shown::Nothing => View::Nothing,
            Shown::Failed(why) => View::Message(why),
        };
        self.set_view(view);
        if self.needs_bigger_image() {
            self.timer(T_REDECODE, REDECODE_DELAY);
        }
    }

    /// The watchdog: a handler's call that has not returned for
    /// `HANDLER_TIMEOUT`, or a request unanswered for `WORK_TIMEOUT`, gives
    /// the thread up.
    fn watch(&self, app: &App) {
        let Some(since) = self.requested.get() else {
            self.kill(T_WATCH);
            return;
        };
        let phase = self.worker.borrow().as_ref().and_then(Worker::phase);
        let current = self.generation.get();
        if let Some(p) = phase.filter(|p| p.handler && p.since.elapsed() >= HANDLER_TIMEOUT) {
            self.abandon();
            if p.generation == current {
                self.set_view(View::Message(NOT_RESPONDING.into()));
            } else {
                // An earlier item's handler hung: this one is asked for
                // again on a new thread.
                let target = self.target.borrow_mut().take();
                if let Some(target) = target {
                    self.request(app, target, false);
                }
            }
        } else if since.elapsed() >= WORK_TIMEOUT {
            self.abandon();
            self.set_view(View::Message(TOO_LONG.into()));
        }
    }

    /// Whether the pane shows the image bigger than it was decoded while
    /// the file has more to give.
    fn needs_bigger_image(&self) -> bool {
        let rc = self.rc.get();
        match &*self.view.borrow() {
            View::Picture { image, .. } => {
                let (bw, bh) = (image.bitmap.width, image.bitmap.height);
                let shown = fit(image.natural, (width(&rc), height(&rc)), self.settings.fill);
                (bw < image.natural.0 || bh < image.natural.1) && (shown.0 > bw || shown.1 > bh)
            }
            _ => false,
        }
    }

    fn theme_text_box(&self, dark: bool) {
        let edit = self.text_box.get();
        theme::allow(edit, dark);
        theme::set_class(
            edit,
            Some(if dark {
                "DarkMode_Explorer"
            } else {
                "Explorer"
            }),
        );
    }

    /// The text box's text keeps off its edges.
    fn text_margins(&self, app: &App) {
        let edit = self.text_box.get();
        let mut rc = RECT::default();
        // SAFETY: reads and sets this thread's text box's rectangles.
        unsafe {
            GetClientRect(edit, &mut rc);
            let inner = RECT {
                left: rc.left + app.scale(6),
                top: rc.top + app.scale(4),
                right: (rc.right - app.scale(4)).max(rc.left + app.scale(6)),
                bottom: rc.bottom,
            };
            SendMessageW(edit, EM_SETRECTNP, 0, (&raw const inner) as isize);
        }
    }

    /// Paints the image, icon or message in `rc`.
    fn paint_view(&self, app: &App, hdc: HDC, rc: &RECT) {
        let pal = app.pal.get();
        let target = self.target.borrow();
        let name: &[u16] = target.as_ref().map_or(&[], |t| &t.name);
        let view = self.view.borrow();
        match &*view {
            View::Empty => {
                let note: Vec<u16> = NOTHING_SELECTED.encode_utf16().collect();
                caption(app, hdc, rc, None, &[(&note, pal.dim)]);
            }
            View::Loading => {
                let note: Vec<u16> = LOADING.encode_utf16().collect();
                caption(app, hdc, rc, None, &[(name, pal.fg), (&note, pal.dim)]);
            }
            View::Message(why) => {
                let note: Vec<u16> = why.encode_utf16().collect();
                caption(app, hdc, rc, None, &[(name, pal.fg), (&note, pal.dim)]);
            }
            View::Icon {
                icon,
                online_only,
                scaled,
            } => {
                let details: &[u16] = target.as_ref().map_or(&[], |t| &t.details);
                let mut lines = vec![(name, pal.fg), (details, pal.dim)];
                let online: Vec<u16> = ONLINE_ONLY.encode_utf16().collect();
                if *online_only {
                    lines.push((&online, pal.dim));
                }
                let image = icon.as_ref().map(|i| (i, scaled, &self.settings));
                caption(app, hdc, rc, image, &lines);
            }
            View::Picture { image, scaled } => {
                let shown = fit(image.natural, (width(rc), height(rc)), self.settings.fill);
                let at = RECT {
                    left: rc.left + (width(rc) - shown.0) / 2,
                    top: rc.top + (height(rc) - shown.1) / 2,
                    right: rc.left + (width(rc) - shown.0) / 2 + shown.0,
                    bottom: rc.top + (height(rc) - shown.1) / 2 + shown.1,
                };
                draw_image(hdc, image, scaled, &at, pal.bg, &self.settings);
            }
            View::Text | View::Handler | View::Nothing => {}
        }
    }
}

/// Draws `image` into `at`, scaled (once, kept in `cache`) unless it is
/// that size already.
fn draw_image(
    hdc: HDC,
    image: &Decoded,
    cache: &RefCell<Option<Scaled>>,
    at: &RECT,
    bg: u32,
    settings: &PreviewSettings,
) {
    let (w, h) = (width(at), height(at));
    let src = &image.bitmap;
    if w <= 0 || h <= 0 {
        return;
    }
    // SAFETY: memory DCs made and deleted here hold bitmaps owned by the
    // view, selected out again before the DCs go.
    unsafe {
        let mem = CreateCompatibleDC(hdc);
        if (w, h) == (src.width, src.height) {
            // Its own size: blended straight onto the background.
            let old = SelectObject(mem, src.handle());
            AlphaBlend(hdc, at.left, at.top, w, h, mem, 0, 0, w, h, blend());
            SelectObject(mem, old);
            DeleteDC(mem);
            return;
        }
        let smooth = if w < src.width {
            settings.min_filter.smooth()
        } else {
            settings.mag_filter.smooth()
        };
        let fresh = cache
            .borrow()
            .as_ref()
            .is_some_and(|s| s.width == w && s.height == h && s.bg == bg && s.smooth == smooth);
        if !fresh {
            *cache.borrow_mut() = scale(hdc, image, w, h, bg, smooth);
        }
        if let Some(s) = cache.borrow().as_ref() {
            let old = SelectObject(mem, s.handle);
            BitBlt(hdc, at.left, at.top, w, h, mem, 0, 0, SRCCOPY);
            SelectObject(mem, old);
        }
        DeleteDC(mem);
    }
}

fn blend() -> BLENDFUNCTION {
    BLENDFUNCTION {
        BlendOp: AC_SRC_OVER as u8,
        BlendFlags: 0,
        SourceConstantAlpha: 255,
        AlphaFormat: AC_SRC_ALPHA as u8,
    }
}

/// `image` on `bg`, `w` by `h`: one with see-through pixels is first laid
/// on the background at its own size, then stretched (half tone when
/// `smooth`).
fn scale(hdc: HDC, image: &Decoded, w: i32, h: i32, bg: u32, smooth: bool) -> Option<Scaled> {
    let src = &image.bitmap;
    let (sw, sh) = (src.width, src.height);
    // SAFETY: bitmaps and DCs made here; each is selected out before it is
    // deleted, and the one returned is owned by `Scaled`.
    unsafe {
        let out = CreateCompatibleBitmap(hdc, w, h);
        if out.is_null() {
            return None;
        }
        let dst = CreateCompatibleDC(hdc);
        let src_dc = CreateCompatibleDC(hdc);
        let old_dst = SelectObject(dst, out);
        let mut flat: HBITMAP = core::ptr::null_mut();
        let old_src = if image.opaque {
            SelectObject(src_dc, src.handle())
        } else {
            flat = CreateCompatibleBitmap(hdc, sw, sh);
            let old = SelectObject(src_dc, flat);
            let whole = RECT {
                left: 0,
                top: 0,
                right: sw,
                bottom: sh,
            };
            fill(src_dc, &whole, bg);
            let pixels = CreateCompatibleDC(hdc);
            let old_px = SelectObject(pixels, src.handle());
            AlphaBlend(src_dc, 0, 0, sw, sh, pixels, 0, 0, sw, sh, blend());
            SelectObject(pixels, old_px);
            DeleteDC(pixels);
            old
        };
        SetStretchBltMode(dst, if smooth { HALFTONE } else { COLORONCOLOR });
        SetBrushOrgEx(dst, 0, 0, core::ptr::null_mut());
        StretchBlt(dst, 0, 0, w, h, src_dc, 0, 0, sw, sh, SRCCOPY);
        SelectObject(src_dc, old_src);
        SelectObject(dst, old_dst);
        DeleteDC(src_dc);
        DeleteDC(dst);
        if !flat.is_null() {
            DeleteObject(flat);
        }
        Some(Scaled {
            width: w,
            height: h,
            bg,
            smooth,
            handle: out,
        })
    }
}

/// The text's height in `w` pixels, at most `lines` lines.
fn text_height(hdc: HDC, text: &[u16], w: i32, lines: i32, line: i32) -> i32 {
    if text.is_empty() {
        return 0;
    }
    let mut rc = RECT {
        left: 0,
        top: 0,
        right: w.max(1),
        bottom: 0,
    };
    // SAFETY: measures into the rectangle; the text is not changed
    // (DT_MODIFYSTRING is not given).
    unsafe {
        DrawTextW(
            hdc,
            text.as_ptr(),
            text.len() as i32,
            &mut rc,
            DT_CALCRECT | DT_WORDBREAK | DT_EDITCONTROL | DT_NOPREFIX,
        );
    }
    rc.bottom.min(lines * line)
}

/// An optional image (an icon), then lines of text (each wrapped, at most
/// three lines), centred in `rc` together.
fn caption(
    app: &App,
    hdc: HDC,
    rc: &RECT,
    image: Option<(&Decoded, &RefCell<Option<Scaled>>, &PreviewSettings)>,
    lines: &[(&[u16], u32)],
) {
    let margin = app.scale(MARGIN);
    let gap = app.scale(GAP);
    let w = (width(rc) - 2 * margin).max(1);
    let line = app.ui_fonts.borrow().height;
    // SAFETY: draws into the paint buffer; the old font is put back.
    unsafe {
        let old = SelectObject(hdc, app.ui_font());
        SetBkMode(hdc, TRANSPARENT as i32);
        let shown = image.map(|(i, _, _)| fit(i.natural, (w, (height(rc) / 2).max(1)), false));
        let heights: Vec<i32> = lines
            .iter()
            .map(|(t, _)| text_height(hdc, t, w, 6, line))
            .collect();
        let texts: i32 = heights.iter().sum::<i32>()
            + gap / 2 * heights.iter().filter(|&&h| h > 0).count().saturating_sub(1) as i32;
        let total = shown.map_or(0, |s| s.1 + gap) + texts;
        let mut y = rc.top + ((height(rc) - total) / 2).max(margin.min(height(rc) / 4));
        if let (Some((img, cache, settings)), Some((iw, ih))) = (image, shown) {
            let left = rc.left + (width(rc) - iw) / 2;
            let at = RECT {
                left,
                top: y,
                right: left + iw,
                bottom: y + ih,
            };
            draw_image(hdc, img, cache, &at, app.pal.get().bg, settings);
            y += ih + gap;
        }
        for ((text, colour), h) in lines.iter().zip(heights) {
            if h == 0 {
                continue;
            }
            let mut at = RECT {
                left: rc.left + margin,
                top: y,
                right: rc.left + margin + w,
                bottom: (y + h).min(rc.bottom),
            };
            SetTextColor(hdc, *colour);
            DrawTextW(
                hdc,
                text.as_ptr(),
                text.len() as i32,
                &mut at,
                DT_CENTER | DT_WORDBREAK | DT_EDITCONTROL | DT_END_ELLIPSIS | DT_NOPREFIX,
            );
            y += h + gap / 2;
        }
        SelectObject(hdc, old);
    }
}

impl PaneContent for PreviewPane {
    fn create(&self, app: &App, pane: HWND) {
        self.pane.set(pane);
        let font = mono_font(app);
        self.mono.set(font);
        let style = WS_CHILD
            | WS_VSCROLL
            | WS_HSCROLL
            | (ES_MULTILINE | ES_READONLY | ES_AUTOVSCROLL | ES_AUTOHSCROLL) as u32;
        // SAFETY: a hidden child of this thread's pane window, given the
        // pane's font; its text has no limit but the 64 KB read.
        unsafe {
            let edit = CreateWindowExW(
                0,
                wz("EDIT").as_ptr(),
                core::ptr::null(),
                style,
                0,
                0,
                0,
                0,
                pane,
                ID_TEXT as HMENU,
                GetModuleHandleW(core::ptr::null()),
                core::ptr::null(),
            );
            SendMessageW(edit, WM_SETFONT, font as usize, 0);
            SendMessageW(edit, EM_SETLIMITTEXT, 0, 0);
            self.text_box.set(edit);
        }
        self.theme_text_box(app.pal.get().dark);
        self.window_shown.set(on_screen(app));
        // The first preview once the pane is in its place.
        self.timer(T_FOCUS, 1);
    }

    fn place(&self, app: &App, rc: RECT) {
        self.rc.set(rc);
        let (w, h) = (width(&rc).max(0), height(&rc).max(0));
        let (edit, host) = (self.text_box.get(), self.host());
        // SAFETY: moves this thread's windows.
        unsafe {
            MoveWindow(edit, rc.left, rc.top, w, h, 1);
            if !host.is_null() {
                MoveWindow(host, rc.left, rc.top, w, h, 1);
            }
        }
        self.text_margins(app);
        if let Some(worker) = self.worker.borrow().as_ref() {
            worker.resize((w.max(1), h.max(1)));
        }
        if self.needs_bigger_image() {
            self.timer(T_REDECODE, REDECODE_DELAY);
        }
        self.invalidate();
    }

    fn destroy(&self, _app: &App) {
        for id in [T_FOCUS, T_SLOW, T_WATCH, T_REDECODE] {
            self.kill(id);
        }
        self.release();
        let edit = self.text_box.replace(core::ptr::null_mut());
        let font = self.mono.replace(core::ptr::null_mut());
        // SAFETY: the text box goes before the font it uses.
        unsafe {
            if !edit.is_null() {
                DestroyWindow(edit);
            }
            if !font.is_null() {
                DeleteObject(font);
            }
        }
    }

    fn focus(&self) -> Option<HWND> {
        matches!(*self.view.borrow(), View::Text).then(|| self.text_box.get())
    }

    fn key(&self, _app: &App, msg: &MSG) -> bool {
        // Ctrl+A selects all the text, which a multi-line text box does
        // not do by itself.
        // SAFETY: reads this thread's keyboard state; selects in its text
        // box.
        unsafe {
            if msg.hwnd == self.text_box.get()
                && msg.message == WM_KEYDOWN
                && msg.wParam == usize::from(b'A')
                && GetKeyState(i32::from(VK_CONTROL)) < 0
                && GetKeyState(i32::from(VK_MENU)) >= 0
            {
                SendMessageW(msg.hwnd, EM_SETSEL, 0, -1);
                return true;
            }
        }
        false
    }

    fn message(&self, app: &App, pane: HWND, msg: u32, w: WPARAM, l: LPARAM) -> Option<LRESULT> {
        match msg {
            WM_TIMER => {
                match w {
                    T_FOCUS => {
                        self.kill(T_FOCUS);
                        self.refresh(app);
                    }
                    T_SLOW => {
                        self.kill(T_SLOW);
                        if self.requested.get().is_some() {
                            self.set_view(View::Loading);
                        }
                    }
                    T_WATCH => self.watch(app),
                    T_REDECODE => {
                        self.kill(T_REDECODE);
                        if self.needs_bigger_image() {
                            let target = self.target.borrow().clone();
                            if let Some(target) = target {
                                self.request(app, target, true);
                            }
                        }
                    }
                    _ => return None,
                }
                Some(0)
            }
            WM_SETFOCUS => {
                if matches!(*self.view.borrow(), View::Handler) {
                    if let Some(worker) = self.worker.borrow().as_ref() {
                        worker.focus();
                    }
                } else {
                    self.invalidate();
                }
                None
            }
            WM_KILLFOCUS => {
                self.invalidate();
                None
            }
            WM_LBUTTONDOWN => {
                // A double-click on the picture or icon opens the item (the
                // pane's class takes no double-clicks of its own).
                let pt = POINT {
                    x: (l & 0xffff) as i16 as i32,
                    y: ((l >> 16) & 0xffff) as i16 as i32,
                };
                // SAFETY: reads the message's time and the system's
                // double-click settings.
                let (now, limit, dx, dy) = unsafe {
                    (
                        GetMessageTime() as u32,
                        GetDoubleClickTime(),
                        GetSystemMetrics(SM_CXDOUBLECLK) / 2,
                        GetSystemMetrics(SM_CYDOUBLECLK) / 2,
                    )
                };
                let double = self.last_down.get().is_some_and(|(t, p)| {
                    now.wrapping_sub(t) <= limit
                        && (pt.x - p.x).abs() <= dx
                        && (pt.y - p.y).abs() <= dy
                });
                self.last_down.set((!double).then_some((now, pt)));
                let item = matches!(
                    *self.view.borrow(),
                    View::Picture { .. } | View::Icon { .. } | View::Message(_)
                );
                if double && item {
                    // SAFETY: posted to this thread's main window, as the
                    // menu's File > Open.
                    unsafe { PostMessageW(app.hwnd, WM_COMMAND, usize::from(menu::OPEN), 0) };
                }
                None
            }
            WM_CONTEXTMENU if w as HWND == pane => {
                let (mut x, mut y) = (
                    (l & 0xffff) as i16 as i32,
                    ((l >> 16) & 0xffff) as i16 as i32,
                );
                if l == -1 {
                    // From the keyboard: at the content's top left.
                    let rc = self.rc.get();
                    let mut pt = POINT {
                        x: rc.left + app.scale(8),
                        y: rc.top + app.scale(8),
                    };
                    // SAFETY: converts a point of this thread's window.
                    unsafe {
                        windows_sys::Win32::Graphics::Gdi::ClientToScreen(pane, &mut pt);
                    }
                    (x, y) = (pt.x, pt.y);
                }
                app.layout_menu(x, y);
                Some(0)
            }
            _ => None,
        }
    }

    fn paint(&self, app: &App, hdc: HDC, rc: &RECT) {
        fill(hdc, rc, app.pal.get().bg);
        self.paint_view(app, hdc, rc);
        let pane = self.pane.get();
        let inner = matches!(*self.view.borrow(), View::Text | View::Handler);
        // SAFETY: reads this thread's focus; draws into the buffer.
        unsafe {
            if !inner && GetFocus() == pane {
                let r = RECT {
                    left: rc.left + 2,
                    top: rc.top + 2,
                    right: rc.right - 2,
                    bottom: rc.bottom - 2,
                };
                SetTextColor(hdc, app.pal.get().fg);
                DrawFocusRect(hdc, &r);
            }
        }
    }

    fn theme(&self, app: &App, dark: bool) {
        self.theme_text_box(dark);
        let host = self.host();
        if !host.is_null() {
            // SAFETY: this thread's host's background, repainted.
            unsafe {
                SetWindowLongPtrW(host, GWLP_USERDATA, app.pal.get().bg as isize);
                InvalidateRect(host, core::ptr::null(), 1);
            }
        }
        // A preview handler takes the colours as it loads a file.
        if matches!(*self.view.borrow(), View::Handler) {
            self.reload(app);
        }
        self.invalidate();
    }

    fn dpi(&self, app: &App, _dpi: u32) {
        let font = mono_font(app);
        let old = self.mono.replace(font);
        // SAFETY: the text box takes the new font before the old one goes.
        unsafe {
            SendMessageW(self.text_box.get(), WM_SETFONT, font as usize, 1);
            if !old.is_null() {
                DeleteObject(old);
            }
        }
        // The icon is made at the window's DPI.
        if matches!(*self.view.borrow(), View::Icon { .. }) {
            self.reload(app);
        }
    }

    fn event(&self, _app: &App, event: PaneEvent) {
        match event {
            PaneEvent::SelectionChanged
            | PaneEvent::SearchChanged
            | PaneEvent::ResultsChanged { .. } => {
                if self.window_shown.get() {
                    self.soon();
                }
            }
            PaneEvent::WindowShown(false) => {
                self.window_shown.set(false);
                self.kill(T_FOCUS);
                self.release();
            }
            PaneEvent::WindowShown(true) => {
                self.window_shown.set(true);
                self.timer(T_FOCUS, 1);
            }
        }
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn target(path: &str, size: Option<u64>, mtime: Option<i64>) -> Target {
        Target {
            path: path.encode_utf16().collect(),
            is_dir: false,
            size,
            mtime,
            name: Vec::new(),
            details: Vec::new(),
        }
    }

    #[test]
    fn the_same_unchanged_file_is_not_loaded_again() {
        let a = target(r"C:\a.png", Some(10), Some(5));
        assert!(a.same_item(&target(r"C:\a.png", Some(10), Some(5))));
        // Changed since: loaded again.
        assert!(!a.same_item(&target(r"C:\a.png", Some(11), Some(5))));
        assert!(!a.same_item(&target(r"C:\a.png", Some(10), Some(6))));
        assert!(!a.same_item(&target(r"C:\b.png", Some(10), Some(5))));
        // Its name and details do not count.
        let mut b = target(r"C:\a.png", Some(10), Some(5));
        b.details = "PNG File".encode_utf16().collect();
        assert!(a.same_item(&b));
    }

    #[test]
    fn details_go_a_line_each() {
        let mut out = Vec::new();
        push_line(&mut out, "PNG File");
        push_line(&mut out, "Size: 4 KB");
        assert_eq!(String::from_utf16_lossy(&out), "PNG File\nSize: 4 KB");
        let mut empty = Vec::new();
        push_line(&mut empty, "Date modified: x");
        assert_eq!(String::from_utf16_lossy(&empty), "Date modified: x");
    }
}
