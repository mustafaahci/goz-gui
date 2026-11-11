//! Live updates: the results follow the files as they change. gozd says
//! nothing when files change, so while the window is on screen the search
//! on it runs again every `live_update_interval_ms` (2 s; 0 turns it off),
//! as F5's refresh does: the scroll position and the selected files stay,
//! the rows not come yet are drawn as they were (`Model::previous_row`),
//! and nothing goes into the search or navigation history.
//!
//! It waits while the user is in the middle of something (a menu, a
//! dialog, a rename, a drag, a mouse button held, an action waiting for its
//! rows, a search on its way), and stops while the window is hidden or
//! minimized: the timer is not set again until it shows. A search gozd
//! takes long over is run less often, so gozd spends at most about a
//! twentieth of its time on them.

use std::time::Duration;

use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    GetCapture, GetKeyState, IsWindowEnabled, VK_LBUTTON, VK_RBUTTON,
};
use windows_sys::Win32::UI::WindowsAndMessaging::SetTimer;

use super::{App, TIMER_LIVE};

/// The longest wait a slow search stretches the interval to (ms).
const SLOW_MAX_MS: u32 = 60_000;

/// The wait before the next live update: the interval asked for, or 20
/// times what the last search took when that is longer (up to a minute).
fn live_delay(interval_ms: u32, last: Option<Duration>) -> u32 {
    let took = last.map_or(0, |d| u32::try_from(d.as_millis()).unwrap_or(u32::MAX));
    let slow = took.saturating_mul(20).min(SLOW_MAX_MS);
    interval_ms.max(slow)
}

impl App {
    /// The interval of the settings file, 0 for off.
    fn live_interval(&self) -> u32 {
        self.prefs.borrow().base.live_update_ms
    }

    /// Sets the live update's timer (again). Not only while the window is
    /// on screen: WM_SHOWWINDOW comes before it is, and the tick looks.
    pub(super) fn arm_live(&self) {
        let interval = self.live_interval();
        if interval == 0 {
            return;
        }
        let delay = live_delay(interval, self.model.borrow().elapsed);
        // SAFETY: a timer of this window; setting it again restarts it.
        unsafe { SetTimer(self.hwnd, TIMER_LIVE, delay, None) };
    }

    /// The live update's timer: the search runs again if the user is not
    /// busy; the timer is set again while the window is on screen.
    pub(super) fn live_tick(&self) {
        if !self.shown() {
            // Set again when the window shows (WM_SHOWWINDOW, WM_SIZE).
            return;
        }
        if self.live_idle() {
            self.start_search(true);
        }
        self.arm_live();
    }

    /// Whether a live update may run now.
    fn live_idle(&self) -> bool {
        // An open file list does not change: nothing to update (wave 4).
        if self.filelist().is_some() {
            return false;
        }
        {
            let m = self.model.borrow();
            // A search on its way brings fresh rows anyway; a failed one is
            // tried again by its own timer.
            if m.pending.is_some() || m.failure.is_some() {
                return false;
            }
        }
        // Results this fresh need no update yet (the user is typing).
        let interval = Duration::from_millis(u64::from(self.live_interval()));
        if self
            .last_search
            .get()
            .is_some_and(|t| t.elapsed() < interval / 2)
        {
            return false;
        }
        // SAFETY: reads this thread's window and input state.
        let (modal, captured, pressed) = unsafe {
            (
                IsWindowEnabled(self.hwnd) == 0,
                !GetCapture().is_null(),
                GetKeyState(i32::from(VK_LBUTTON)) < 0 || GetKeyState(i32::from(VK_RBUTTON)) < 0,
            )
        };
        !(modal
            || captured
            || pressed
            || self.in_menu.get()
            || self.bookmark_dialog.get()
            || self.own_drag.get()
            || self.renaming()
            || self.fetch_waiting()
            || self.shell_menu.borrow().is_some())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slow_searches_run_less_often() {
        let ms = Duration::from_millis;
        assert_eq!(live_delay(2000, None), 2000);
        assert_eq!(live_delay(2000, Some(ms(30))), 2000);
        assert_eq!(live_delay(2000, Some(ms(250))), 5000);
        assert_eq!(live_delay(2000, Some(ms(10_000))), 60_000);
        // A longer interval asked for stands.
        assert_eq!(live_delay(120_000, Some(ms(10_000))), 120_000);
    }
}
