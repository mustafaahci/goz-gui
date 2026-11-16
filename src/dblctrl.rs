//! Double-Ctrl (Everything 1.5's `double_ctrl_action`): pressing either
//! Ctrl key twice, alone, within the double-click time shows the window
//! (or toggles it, or opens a new one).
//!
//! It takes a low-level keyboard hook, which sees every key typed anywhere
//! (but in programs running as administrator, which Windows keeps from
//! it), so the hook is in only while the option is on. The hook's
//! procedure runs on the UI thread between messages and only watches the
//! Ctrl keys: it posts a message when it sees the double press and lets
//! every key go on.

use std::cell::{Cell, RefCell};

use windows_sys::Win32::Foundation::{HWND, LRESULT};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    WM_KEYDOWN, WM_KEYUP, WM_SYSKEYDOWN, WM_SYSKEYUP,
};

use crate::win::hook::{self, Event, Hook, Kind};
use crate::win::{self, sys};

/// What the keys seen so far add up to.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Detector {
    /// A Ctrl key is down, and since when (ms).
    down: Option<u32>,
    /// Another key went down while Ctrl was: this press is no tap.
    spoiled: bool,
    /// When the last clean tap ended (ms).
    tapped: Option<u32>,
}

/// One key event the hook sees.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Key {
    CtrlDown,
    CtrlUp,
    Other,
}

impl Detector {
    /// Takes one event at `time` (ms, as the hook gives it); true on the
    /// second of two taps within `within` ms of each other. A tap is a Ctrl
    /// pressed and let go with no other key in between, in no longer than
    /// `within` itself.
    pub fn key(&mut self, key: Key, time: u32, within: u32) -> bool {
        match key {
            Key::CtrlDown => {
                // The key repeating while held is no new press.
                if self.down.is_none() {
                    self.down = Some(time);
                    self.spoiled = false;
                }
                false
            }
            Key::Other => {
                self.spoiled = true;
                self.tapped = None;
                false
            }
            Key::CtrlUp => {
                let Some(down) = self.down.take() else {
                    return false;
                };
                if self.spoiled || time.wrapping_sub(down) > within {
                    self.tapped = None;
                    return false;
                }
                match self.tapped {
                    Some(last) if time.wrapping_sub(last) <= within => {
                        self.tapped = None;
                        true
                    }
                    _ => {
                        self.tapped = Some(time);
                        false
                    }
                }
            }
        }
    }
}

thread_local! {
    static HOOK: RefCell<Option<Hook>> = const { RefCell::new(None) };
    static TARGET: Cell<(HWND, u32)> = const { Cell::new((core::ptr::null_mut(), 0)) };
    static STATE: Cell<Detector> = const {
        Cell::new(Detector { down: None, spoiled: false, tapped: None })
    };
}

const VK_CONTROL: u32 = 0x11;
const VK_LCONTROL: u32 = 0xa2;
const VK_RCONTROL: u32 = 0xa3;

/// The hook's view of each key: it watches the Ctrl keys and lets every
/// key go on, as every hook must (`None`).
fn on_key(event: &Event) -> Option<LRESULT> {
    let &Event::Key {
        message, vk, time, ..
    } = event
    else {
        return None;
    };
    let ctrl = matches!(vk, VK_CONTROL | VK_LCONTROL | VK_RCONTROL);
    let key = match (ctrl, message) {
        (true, WM_KEYDOWN | WM_SYSKEYDOWN) => Some(Key::CtrlDown),
        (true, WM_KEYUP | WM_SYSKEYUP) => Some(Key::CtrlUp),
        (false, WM_KEYDOWN | WM_SYSKEYDOWN) => Some(Key::Other),
        _ => None,
    };
    if let Some(key) = key {
        let within = sys::double_click_ms();
        let fire = STATE.with(|s| {
            let mut d = s.get();
            let fire = d.key(key, time, within);
            s.set(d);
            fire
        });
        if fire {
            let (hwnd, message) = TARGET.with(Cell::get);
            // Posts to this thread's window; nothing waits.
            win::post_app(hwnd, message, 0, 0);
        }
    }
    None
}

/// Puts the hook in, posting `message` to `hwnd` on a double Ctrl; false
/// when Windows refused it.
pub fn install(hwnd: HWND, message: u32) -> bool {
    TARGET.with(|t| t.set((hwnd, message)));
    STATE.with(|s| s.set(Detector::default()));
    if HOOK.with_borrow(Option::is_some) {
        return true;
    }
    // A low-level hook, which runs on this thread (which pumps messages).
    let hook = hook::set(Kind::KeyboardLl, on_key);
    let set = hook.is_some();
    HOOK.set(hook);
    set
}

pub fn uninstall() {
    // The hook this thread put in, taken out as it drops.
    drop(HOOK.take());
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(events: &[(Key, u32)]) -> Vec<bool> {
        let mut d = Detector::default();
        events.iter().map(|&(k, t)| d.key(k, t, 500)).collect()
    }

    #[test]
    fn two_quick_taps_fire_once() {
        use Key::*;
        let fired = run(&[
            (CtrlDown, 0),
            (CtrlUp, 80),
            (CtrlDown, 200),
            // Held: the key repeats.
            (CtrlDown, 230),
            (CtrlUp, 290),
            (CtrlDown, 400),
            (CtrlUp, 450),
        ]);
        assert_eq!(fired, [false, false, false, false, true, false, false]);
    }

    #[test]
    fn ctrl_with_another_key_or_slow_taps_do_not() {
        use Key::*;
        // Ctrl+C, then a tap: no double.
        assert!(
            !run(&[
                (CtrlDown, 0),
                (Other, 20),
                (CtrlUp, 60),
                (CtrlDown, 100),
                (CtrlUp, 150)
            ])
            .contains(&true)
        );
        // A key between the taps.
        assert!(
            !run(&[
                (CtrlDown, 0),
                (CtrlUp, 50),
                (Other, 100),
                (CtrlDown, 150),
                (CtrlUp, 200)
            ])
            .contains(&true)
        );
        // Too far apart.
        assert!(
            !run(&[(CtrlDown, 0), (CtrlUp, 50), (CtrlDown, 700), (CtrlUp, 750)]).contains(&true)
        );
        // Held too long.
        assert!(
            !run(&[(CtrlDown, 0), (CtrlUp, 900), (CtrlDown, 950), (CtrlUp, 990)]).contains(&true)
        );
        // A stray key up.
        assert!(!run(&[(CtrlUp, 10)]).contains(&true));
        // The clock wrapping round.
        assert!(
            run(&[
                (CtrlDown, u32::MAX - 100),
                (CtrlUp, u32::MAX - 50),
                (CtrlDown, 10),
                (CtrlUp, 60)
            ])
            .contains(&true)
        );
    }
}
