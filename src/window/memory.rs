//! Giving memory back once it is no longer looked at: the thumbnails kept
//! for the thumbnail views, when the window hides and once the view has
//! rested, and the heap's free pages once a pane has closed or the views
//! have rested.
//!
//! The thumbnail cache may hold up to `thumbs::BUDGET` (24 MB) of bitmaps
//! while the user scrolls; a screenful is a fraction of that. Hidden (the
//! tray, minimized), the window keeps none: what shows when it comes back
//! is made again, the pictures from the shell's own thumbnail cache. Shown
//! but left alone for `REST_MS`, it keeps those in view. What was freed
//! then (and what a closed pane freed) is handed back to Windows
//! (`runs::give_back_memory`), which the heap does not do by itself.

use std::cell::Cell;

use windows_sys::Win32::UI::WindowsAndMessaging::{KillTimer, SetTimer};

use super::{App, TIMER_REST, runs};

/// How long the window rests (no thumbnail made, no pane closed) before
/// what is not in view goes.
const REST_MS: u32 = 15_000;

thread_local! {
    /// Whether the next rest is the second since the window was last busy.
    static SECOND_REST: Cell<bool> = const { Cell::new(false) };
}

impl App {
    /// Memory was used and may be let go once the window rests: a
    /// thumbnail arrived, or a pane closed. Each puts the rest off again
    /// (one timer, never repeating).
    pub(super) fn memory_settles(&self) {
        SECOND_REST.set(false);
        // SAFETY: a timer of this thread's window.
        unsafe { SetTimer(self.hwnd, TIMER_REST, REST_MS, None) };
    }

    /// The window rested: thumbnails of items out of view go, and the
    /// heap's free pages back to Windows. Twice, `REST_MS` apart: what the
    /// shell's threads and a closed preview's let go of comes free a while
    /// after (measured: 3 MB more at the second pass after heavy use).
    pub(super) fn memory_rested(&self) {
        self.thumbs.trim();
        runs::give_back_memory();
        if !SECOND_REST.replace(true) {
            // SAFETY: a timer of this thread's window.
            unsafe { SetTimer(self.hwnd, TIMER_REST, REST_MS, None) };
        }
    }

    /// The window was hidden or minimized (`shown` false) or came back.
    /// Hidden, it keeps no thumbnails; shown, the view asks for those it
    /// draws.
    pub(super) fn memory_window_shown(&self, shown: bool) {
        if shown {
            return;
        }
        // SAFETY: a timer of this thread's window (set or not).
        unsafe { KillTimer(self.hwnd, TIMER_REST) };
        self.thumbs.release();
        runs::give_back_memory();
    }
}
