//! Windows (Everything 1.5's `multi-window`): File > New Window starts
//! goz-gui again on the same data folder with `-new-window`, which skips
//! the hand-over to the running goz-gui, placed `offset_new_window_position_x`
//! / `_y` (22 px) right of and below this window. Each window is a process
//! of its own; the tray icon and the global hotkeys stay with the first,
//! and the settings, histories, bookmarks and session merge across
//! processes as they always have. Close Window closes this one (into the
//! background for the first with `run_in_background`); Exit closes every
//! window of this goz-gui.

use std::os::windows::ffi::OsStrExt;
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};

use windows_sys::Win32::Foundation::{CloseHandle, HWND};
use windows_sys::Win32::System::Threading::{
    CREATE_DEFAULT_ERROR_MODE, CreateProcessW, PROCESS_INFORMATION, STARTF_USESHOWWINDOW,
    STARTUPINFOW,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    AllowSetForegroundWindow, DestroyWindow, GetForegroundWindow, PostMessageW, SW_HIDE,
    SW_SHOWNOACTIVATE, SW_SHOWNORMAL,
};

use super::{App, slots};
use crate::cmdline::Launch;
use crate::settings::WindowPos;
use crate::tabs;
use crate::wide::wz;

/// A window of the instance asked this one to end (Exit was chosen there).
pub(super) const WM_EXIT_WINDOW: u32 = slots::F3_MSG;
/// File > New Window could not start goz-gui: a `Box<String>` in lParam
/// says why.
pub(super) const WM_NEW_WINDOW_FAILED: u32 = slots::F3_MSG + 1;

/// The environment variable that starts File > New Window's goz-gui
/// hidden (tests: nothing shows on the desktop).
const HIDDEN_VAR: &str = "GOZ_GUI_NEW_WINDOW_HIDDEN";

impl App {
    /// This goz-gui was started by File > New Window.
    pub(super) fn is_new_window(&self) -> bool {
        self.new_window.get()
    }

    /// File > New Window (Ctrl+N): another goz-gui window at this one's
    /// place, offset. It starts after the settings changed here are
    /// written (on the store thread, in turn), so it starts with them.
    pub(super) fn new_window(&self) {
        let exe = match std::env::current_exe() {
            Ok(exe) => exe,
            Err(e) => {
                self.show_note(format!("A new window could not be started: {e}"));
                return;
            }
        };
        let s = self.tab_settings();
        let (dx, dy) = if s.offset_new_window {
            (self.scale(s.offset_x), self.scale(s.offset_y))
        } else {
            (0, 0)
        };
        let (pos, _) = self.placement_now();
        let rect = pos.map(|p| (p.x + dx, p.y + dy, p.width, p.height));
        let instance = self.instance.borrow().clone();
        let args = tabs::new_window_args(rect, instance.as_deref());
        // Brought to the front when this window is (the user's Ctrl+N);
        // shown without taking the foreground otherwise.
        // SAFETY: reads the foreground window.
        let front = unsafe { GetForegroundWindow() } == self.hwnd;
        let show = if std::env::var_os(HIDDEN_VAR).is_some_and(|v| v == "1") {
            SW_HIDE
        } else if front {
            SW_SHOWNORMAL
        } else {
            SW_SHOWNOACTIVATE
        };
        self.save_settings();
        // A key of its own, so two quick New Windows are two jobs.
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let key = format!("<new window {}>", NEXT.fetch_add(1, Ordering::Relaxed));
        let hwnd = self.hwnd as usize;
        self.writer.submit(
            Path::new(&key),
            Box::new(move || {
                if let Err(why) = spawn(&exe, &args, show as u16, front) {
                    let text = Box::into_raw(Box::new(format!(
                        "A new window could not be started: {why}"
                    )));
                    // SAFETY: posts to the window; if that fails the box is
                    // taken back.
                    unsafe {
                        if PostMessageW(hwnd as HWND, WM_NEW_WINDOW_FAILED, 0, text as isize) == 0 {
                            drop(Box::from_raw(text));
                        }
                    }
                }
            }),
        );
    }

    /// Exit: every other window of this goz-gui is asked to close too.
    pub(super) fn exit_other_windows(&self) {
        let key = self.instance_key.borrow().clone();
        crate::instance::exit_others(&key, self.exit_window.get());
    }

    /// The window's message-only window for Exit (`crate::instance`).
    pub(super) fn listen_for_exit(&self) {
        let key = self.instance_key.borrow().clone();
        let own = crate::instance::listen_window(&key, self.hwnd, WM_EXIT_WINDOW);
        self.exit_window.set(own);
    }

    /// F3's WM_APP messages.
    pub(super) fn window_message(&self, msg: u32, l: isize) -> Option<isize> {
        match msg {
            WM_EXIT_WINDOW => {
                // SAFETY: closes this thread's window for good, as -exit
                // does.
                unsafe { DestroyWindow(self.hwnd) };
                Some(0)
            }
            WM_NEW_WINDOW_FAILED => {
                // SAFETY: the box the store thread posted.
                let text = unsafe { Box::from_raw(l as *mut String) };
                self.show_note(*text);
                Some(0)
            }
            _ => None,
        }
    }

    /// `-x`, `-y`, `-width`, `-height` handed over by a later start: the
    /// window moves there (its normal place while it is maximized or
    /// minimized), kept on a monitor.
    pub(super) fn place_window(&self, l: &Launch) {
        let (now, maximized) = self.placement_now();
        let rect = now.map(|p| (p.x, p.y, p.width, p.height));
        let Some((x, y, width, height)) = l.place_over(rect) else {
            return;
        };
        let pos = WindowPos {
            x,
            y,
            width,
            height,
            dpi: None,
        };
        self.restore_placement(Some(pos), maximized);
    }
}

/// Starts `exe` with `args`, shown as `show` (a `SW_*`); `front`: it may
/// take the foreground. Run off the window's thread: starting a process
/// can take a while.
fn spawn(exe: &Path, args: &[String], show: u16, front: bool) -> Result<(), String> {
    // The program by its exact name; the command line's first argument
    // (which goz-gui does not read) may be a lossy copy.
    let app: Vec<u16> = exe.as_os_str().encode_wide().chain(Some(0)).collect();
    let mut line = wz(&tabs::command_line(&exe.to_string_lossy(), args));
    let si = STARTUPINFOW {
        cb: size_of::<STARTUPINFOW>() as u32,
        dwFlags: STARTF_USESHOWWINDOW,
        wShowWindow: show,
        // SAFETY: the rest of STARTUPINFOW is plain data, zero for none.
        ..unsafe { core::mem::zeroed() }
    };
    // SAFETY: plain out-struct.
    let mut pi: PROCESS_INFORMATION = unsafe { core::mem::zeroed() };
    // SAFETY: NUL-terminated strings (the command line in a buffer the
    // call may write to), no handles inherited, this process's environment
    // and folder; the handles it gives back are closed here.
    unsafe {
        if CreateProcessW(
            app.as_ptr(),
            line.as_mut_ptr(),
            core::ptr::null(),
            core::ptr::null(),
            0,
            CREATE_DEFAULT_ERROR_MODE,
            core::ptr::null(),
            core::ptr::null(),
            &si,
            &mut pi,
        ) == 0
        {
            return Err(std::io::Error::last_os_error().to_string());
        }
        if front {
            AllowSetForegroundWindow(pi.dwProcessId);
        }
        CloseHandle(pi.hThread);
        CloseHandle(pi.hProcess);
    }
    Ok(())
}
