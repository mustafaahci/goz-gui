//! One goz-gui per data folder (and `-instance` name): a second start hands
//! its command line to the one running and exits, as Everything does.
//!
//! The running goz-gui holds a named mutex and a message-only window whose
//! title is the instance key; a second start that cannot take the mutex
//! finds that window and sends it its arguments in a `WM_COPYDATA`. The key
//! includes the data folder, so a goz-gui started on another folder (a
//! test's `GOZ_GUI_DATA_DIR`, a portable copy) is an instance of its own.
//!
//! File > New Window starts goz-gui again with `-new-window`, which takes
//! neither the mutex nor the hand-over: a window of its own. Every window of
//! an instance (the first included) also has a message-only window of
//! another class, titled with the key, through which Exit in any of them
//! closes them all, as Everything's Exit ends every window.

use std::sync::OnceLock;

use windows_sys::Win32::Foundation::HWND;
use windows_sys::Win32::UI::WindowsAndMessaging::{HWND_MESSAGE, WM_APP, WM_COPYDATA};

use crate::win::class::{self, Class, Create};
use crate::win::process::NamedMutex;
use crate::win::{self, Msg, parcel};

/// The message-only windows' class.
const CLASS: &str = "goz-gui-instance";
/// The class of each window's message-only window, which Exit reaches.
const WINDOW_CLASS: &str = "goz-gui-window";
/// Posted to a window's message-only window: its goz-gui is to end, as
/// Exit was chosen in another window of the instance. wParam 1: it ends
/// writing nothing more (see [`exit_others`]); passed on to the window.
const WM_EXIT_REQUEST: u32 = WM_APP + 1;
/// WM_COPYDATA's dwData: goz-gui's arguments, version 1.
const MAGIC: usize = 0x676f_7a31;

/// The key of this goz-gui's instance: its `-instance` name and its data
/// folder, folded so `C:\X` and `c:\x` are one.
pub fn key(instance: Option<&str>) -> String {
    let dir = crate::store::location::data_dir()
        .map(|p| p.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    let name = instance.unwrap_or("").to_lowercase();
    // FNV-1a: short, stable, and enough to tell folders apart.
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in name.bytes().chain([0]).chain(dir.bytes()) {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x100_0000_01b3);
    }
    format!("goz-gui-{h:016x}")
}

/// Takes the instance: true when this goz-gui is the first with `key`. The
/// mutex is held until the process ends; a goz-gui that is not the first
/// lets go of it at once, so the instance ends with the first goz-gui
/// even while another window of the folder (`-new-window`) runs on.
pub fn claim(key: &str) -> bool {
    static HELD: OnceLock<NamedMutex> = OnceLock::new();
    // A named mutex in this session, never released or closed by the
    // first: the process holding it is the instance. Another closes the
    // handle it was given to the existing one.
    let Some((m, existed)) = NamedMutex::create(&format!("Local\\{key}")) else {
        return false;
    };
    let first = !existed;
    if first && let Err(m) = HELD.set(m) {
        // Held until the process ends, as the first one is.
        std::mem::forget(m);
    }
    first
}

/// The running instance's window, waiting up to about 3 s for it to appear
/// (it may itself be starting).
fn find(key: &str) -> Option<HWND> {
    for _ in 0..60 {
        // HWND_MESSAGE searches the message-only windows.
        let hwnd = win::find_child(HWND_MESSAGE, core::ptr::null_mut(), Some(CLASS), Some(key));
        if !hwnd.is_null() {
            return Some(hwnd);
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    None
}

/// The arguments as WM_COPYDATA carries them: UTF-16, each ended by a NUL.
fn encode(args: &[String]) -> Vec<u16> {
    args.iter()
        .flat_map(|a| a.encode_utf16().chain(Some(0)))
        .collect()
}

fn decode(units: &[u16]) -> Vec<String> {
    if units.is_empty() {
        return Vec::new();
    }
    let mut out: Vec<String> = units
        .split(|&u| u == 0)
        .map(String::from_utf16_lossy)
        .collect();
    // The final NUL leaves one empty piece after it.
    if units.last() == Some(&0) {
        out.pop();
    }
    out
}

/// Hands `args` to the running instance; false if there is none to take
/// them. The running goz-gui may then come to the front: this process lets
/// it, as the user just started goz-gui.
pub fn forward(key: &str, args: &[String]) -> bool {
    let Some(hwnd) = find(key) else {
        return false;
    };
    let data: Vec<u8> = encode(args).iter().flat_map(|u| u.to_ne_bytes()).collect();
    win::process::allow_set_foreground(win::process_of(hwnd));
    // A hung instance is given up on after 5 s.
    win::send_copy_data(hwnd, core::ptr::null_mut(), MAGIC, &data, 5000) == Some(1)
}

/// The message-only window of class `class`, titled `key`, whose messages
/// go to `handler`; the class is registered first (once: a second
/// registration fails harmlessly).
fn message_window(class: &str, key: &str, handler: impl Fn(&Msg) -> isize + 'static) -> HWND {
    Class::new(class).register();
    class::create(
        &Create {
            ex_style: 0,
            class,
            title: key,
            style: 0,
            rect: (0, 0, 0, 0),
            parent: HWND_MESSAGE,
            menu: core::ptr::null_mut(),
        },
        handler,
    )
}

/// Makes this instance's message-only window; each command line it is sent
/// is posted to `main` as `msg` with a `Vec<String>` as a parcel
/// (`win::parcel::take`).
pub fn listen(key: &str, main: HWND, msg: u32) -> HWND {
    let main = main as usize;
    message_window(CLASS, key, move |m| proc(m, main as HWND, msg))
}

/// Makes this window's message-only window for Exit (`exit_others`): a
/// request to end is posted to `main` as `msg`. Every window of the
/// instance has one, titled with its `key`.
pub fn listen_window(key: &str, main: HWND, msg: u32) -> HWND {
    let main = main as usize;
    message_window(WINDOW_CLASS, key, move |m| {
        window_proc(m, main as HWND, msg)
    })
}

/// Asks every other window of the instance `key` to end (`own` is this
/// window's message-only window, which is passed over). `discard`: they
/// end writing nothing more, as the data folder's files are about to be
/// replaced (Tools > Import and Export Settings...). It only posts, so a
/// window that hangs holds nothing up; the processes asked are returned,
/// for a caller that must wait for them to end.
pub fn exit_others(key: &str, own: HWND, discard: bool) -> Vec<u32> {
    let mut asked = Vec::new();
    let mut after: HWND = core::ptr::null_mut();
    // At most as many as there could be windows: the loop always ends.
    for _ in 0..4096 {
        // HWND_MESSAGE searches the message-only windows, after the one
        // found last.
        let found = win::find_child(HWND_MESSAGE, after, Some(WINDOW_CLASS), Some(key));
        if found.is_null() {
            break;
        }
        if found != own {
            // Reads the process it belongs to, and posts it a message with
            // no pointers (to a window of this process, one of goz-gui's
            // own, or of another); one gone meanwhile is reported and
            // ignored.
            let pid = win::process_of(found);
            let posted = if pid == std::process::id() {
                win::post_app(found, WM_EXIT_REQUEST, usize::from(discard), 0)
            } else {
                win::post_app_to_other_process(found, WM_EXIT_REQUEST, usize::from(discard), 0)
            };
            if posted && pid != 0 && pid != std::process::id() && !asked.contains(&pid) {
                asked.push(pid);
            }
        }
        after = found;
    }
    asked
}

fn window_proc(m: &Msg, main: HWND, post: u32) -> isize {
    if m.id == WM_EXIT_REQUEST {
        win::post_app(main, post, m.w, 0);
        return 0;
    }
    m.def()
}

fn proc(m: &Msg, main: HWND, post: u32) -> isize {
    if m.id == WM_COPYDATA && m.l != 0 {
        // A copy of what came, made while the call runs.
        let Some((magic, bytes)) = m.copy_data() else {
            return 0;
        };
        if magic != MAGIC || !bytes.len().is_multiple_of(2) {
            return 0;
        }
        let units: Vec<u16> = bytes
            .as_chunks::<2>()
            .0
            .iter()
            .map(|&c| u16::from_ne_bytes(c))
            .collect();
        return isize::from(parcel::post(main, post, 0, decode(&units)));
    }
    m.def()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arguments_survive_the_trip() {
        let args: Vec<String> = ["-s", "annual report", "", "ü\\x"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        assert_eq!(decode(&encode(&args)), args);
        assert!(decode(&encode(&[])).is_empty());
    }

    #[test]
    fn instance_names_make_their_own_keys() {
        assert_eq!(key(None), key(None));
        assert_ne!(key(None), key(Some("work")));
        assert_eq!(key(Some("Work")), key(Some("work")));
    }
}
