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

use windows_sys::Win32::Foundation::{
    CloseHandle, ERROR_ALREADY_EXISTS, GetLastError, HANDLE, HWND, LPARAM, LRESULT, WPARAM,
};
use windows_sys::Win32::System::DataExchange::COPYDATASTRUCT;
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::System::Threading::CreateMutexW;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    AllowSetForegroundWindow, CreateWindowExW, DefWindowProcW, FindWindowExW, GWLP_USERDATA,
    GetWindowLongPtrW, GetWindowThreadProcessId, HWND_MESSAGE, PostMessageW, RegisterClassExW,
    SMTO_ABORTIFHUNG, SendMessageTimeoutW, SetWindowLongPtrW, WM_APP, WM_COPYDATA, WNDCLASSEXW,
};

use crate::wide::wz;

/// The message-only windows' class.
const CLASS: &str = "goz-gui-instance";
/// The class of each window's message-only window, which Exit reaches.
const WINDOW_CLASS: &str = "goz-gui-window";
/// Posted to a window's message-only window: its goz-gui is to end, as
/// Exit was chosen in another window of the instance.
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
    static HELD: OnceLock<usize> = OnceLock::new();
    let name = wz(&format!("Local\\{key}"));
    // SAFETY: a named mutex in this session, never released or closed by
    // the first: the process holding it is the instance. Another closes
    // the handle it was given to the existing one.
    unsafe {
        let m: HANDLE = CreateMutexW(core::ptr::null(), 0, name.as_ptr());
        let first = !m.is_null() && GetLastError() != ERROR_ALREADY_EXISTS;
        if first {
            let _ = HELD.set(m as usize);
        } else if !m.is_null() {
            CloseHandle(m);
        }
        first
    }
}

/// The running instance's window, waiting up to about 3 s for it to appear
/// (it may itself be starting).
fn find(key: &str) -> Option<HWND> {
    let class = wz(CLASS);
    let title = wz(key);
    for _ in 0..60 {
        // SAFETY: NUL-terminated strings; HWND_MESSAGE searches the
        // message-only windows.
        let hwnd = unsafe {
            FindWindowExW(
                HWND_MESSAGE,
                core::ptr::null_mut(),
                class.as_ptr(),
                title.as_ptr(),
            )
        };
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
    let data = encode(args);
    let cds = COPYDATASTRUCT {
        dwData: MAGIC,
        cbData: (data.len() * 2) as u32,
        lpData: data.as_ptr() as *mut core::ffi::c_void,
    };
    // SAFETY: the window found above; the data outlives the synchronous
    // send; a hung instance is given up on after 5 s.
    unsafe {
        let mut pid = 0u32;
        GetWindowThreadProcessId(hwnd, &mut pid);
        AllowSetForegroundWindow(pid);
        let mut answer = 0usize;
        SendMessageTimeoutW(
            hwnd,
            WM_COPYDATA,
            0,
            (&raw const cds) as LPARAM,
            SMTO_ABORTIFHUNG,
            5000,
            &mut answer,
        ) != 0
            && answer == 1
    }
}

/// Makes this instance's message-only window; each command line it is sent
/// is posted to `main` as `msg` with a `Box<Vec<String>>` in lParam.
pub fn listen(key: &str, main: HWND, msg: u32) -> HWND {
    let class = wz(CLASS);
    let title = wz(key);
    // SAFETY: registers and creates a window of this thread; the target
    // and message travel in its GWLP_USERDATA as a leaked pair that lives
    // as long as the process.
    unsafe {
        let hinst = GetModuleHandleW(core::ptr::null());
        let wc = WNDCLASSEXW {
            cbSize: size_of::<WNDCLASSEXW>() as u32,
            lpfnWndProc: Some(proc),
            hInstance: hinst,
            lpszClassName: class.as_ptr(),
            ..core::mem::zeroed()
        };
        RegisterClassExW(&wc);
        let hwnd = CreateWindowExW(
            0,
            class.as_ptr(),
            title.as_ptr(),
            0,
            0,
            0,
            0,
            0,
            HWND_MESSAGE,
            core::ptr::null_mut(),
            hinst,
            core::ptr::null(),
        );
        if !hwnd.is_null() {
            let target: &'static (usize, u32) = Box::leak(Box::new((main as usize, msg)));
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, target as *const (usize, u32) as isize);
        }
        hwnd
    }
}

/// Makes this window's message-only window for Exit (`exit_others`): a
/// request to end is posted to `main` as `msg`. Every window of the
/// instance has one, titled with its `key`.
pub fn listen_window(key: &str, main: HWND, msg: u32) -> HWND {
    let class = wz(WINDOW_CLASS);
    let title = wz(key);
    // SAFETY: registers and creates a window of this thread; the target
    // and message travel in its GWLP_USERDATA as a leaked pair that lives
    // as long as the process.
    unsafe {
        let hinst = GetModuleHandleW(core::ptr::null());
        let wc = WNDCLASSEXW {
            cbSize: size_of::<WNDCLASSEXW>() as u32,
            lpfnWndProc: Some(window_proc),
            hInstance: hinst,
            lpszClassName: class.as_ptr(),
            ..core::mem::zeroed()
        };
        RegisterClassExW(&wc);
        let hwnd = CreateWindowExW(
            0,
            class.as_ptr(),
            title.as_ptr(),
            0,
            0,
            0,
            0,
            0,
            HWND_MESSAGE,
            core::ptr::null_mut(),
            hinst,
            core::ptr::null(),
        );
        if !hwnd.is_null() {
            let target: &'static (usize, u32) = Box::leak(Box::new((main as usize, msg)));
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, target as *const (usize, u32) as isize);
        }
        hwnd
    }
}

/// Asks every other window of the instance `key` to end (`own` is this
/// window's message-only window, which is passed over). It only posts, so
/// a window that hangs holds nothing up.
pub fn exit_others(key: &str, own: HWND) {
    let class = wz(WINDOW_CLASS);
    let title = wz(key);
    let mut after: HWND = core::ptr::null_mut();
    // At most as many as there could be windows: the loop always ends.
    for _ in 0..4096 {
        // SAFETY: NUL-terminated strings; HWND_MESSAGE searches the
        // message-only windows, after the one found last.
        let found = unsafe { FindWindowExW(HWND_MESSAGE, after, class.as_ptr(), title.as_ptr()) };
        if found.is_null() {
            break;
        }
        if found != own {
            // SAFETY: posts a message with no pointers to a window; one
            // gone meanwhile is reported and ignored.
            unsafe { PostMessageW(found, WM_EXIT_REQUEST, 0, 0) };
        }
        after = found;
    }
}

unsafe extern "system" fn window_proc(hwnd: HWND, msg: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    // SAFETY: GWLP_USERDATA is the pair `listen_window` leaked, or null
    // before it is set.
    unsafe {
        if msg == WM_EXIT_REQUEST {
            let target = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *const (usize, u32);
            if let Some(&(main, post)) = target.as_ref() {
                PostMessageW(main as HWND, post, 0, 0);
            }
            return 0;
        }
        DefWindowProcW(hwnd, msg, w, l)
    }
}

unsafe extern "system" fn proc(hwnd: HWND, msg: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    // SAFETY: GWLP_USERDATA is the pair `listen` leaked, or null before it
    // is set; WM_COPYDATA's lParam is a COPYDATASTRUCT valid during the
    // call.
    unsafe {
        if msg == WM_COPYDATA && l != 0 {
            let target = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *const (usize, u32);
            let cds = &*(l as *const COPYDATASTRUCT);
            if target.is_null() || cds.dwData != MAGIC || !cds.cbData.is_multiple_of(2) {
                return 0;
            }
            let units = if cds.cbData == 0 || cds.lpData.is_null() {
                &[][..]
            } else {
                core::slice::from_raw_parts(cds.lpData as *const u16, cds.cbData as usize / 2)
            };
            let (main, post) = *target;
            let args = Box::into_raw(Box::new(decode(units)));
            if PostMessageW(main as HWND, post, 0, args as LPARAM) == 0 {
                drop(Box::from_raw(args));
                return 0;
            }
            return 1;
        }
        DefWindowProcW(hwnd, msg, w, l)
    }
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
