//! Another goz-gui window: a process of its own on the same data folder,
//! started with `-new-window`, so it does not hand its command line to the
//! goz-gui already running (`crate::instance`) but opens a window of its
//! own, 22 px down and right of where the first one was (Everything's
//! `offset_new_window_position_x`/`_y`). The tray icon and the global
//! hotkeys stay with the first goz-gui; a new window ends when it is
//! closed. The files goz-gui keeps are shared as they are between any two
//! goz-gui windows (each writes its own changes over what the file holds).
//!
//! Options > UI's "Create a new window when running goz-gui"
//! (`allow_multiple_windows`) and "Create a new window from the system
//! tray" (`allow_multiple_windows_from_tray`) open them.

use windows_sys::Win32::Foundation::CloseHandle;
use windows_sys::Win32::System::Threading::{
    CREATE_UNICODE_ENVIRONMENT, CreateProcessW, PROCESS_INFORMATION, STARTUPINFOW,
};
use windows_sys::Win32::UI::WindowsAndMessaging::AllowSetForegroundWindow;

use crate::wide::wz;

/// The switch that makes a window of its own.
pub const SWITCH: &str = "-new-window";

/// How far a new window opens from the first one's place, at 96 DPI.
pub const OFFSET: i32 = 22;

/// `arg` quoted for a command line, so `CommandLineToArgvW` (and Rust's
/// `std::env::args`) read it back as it is: in double quotes when it is
/// empty or holds a space, tab or quote, a quote escaped with a backslash,
/// and the backslashes before a quote (or the closing one) doubled.
pub fn quote_arg(arg: &str) -> String {
    if !arg.is_empty() && !arg.contains([' ', '\t', '"']) {
        return arg.to_owned();
    }
    let mut out = String::with_capacity(arg.len() + 2);
    out.push('"');
    let mut slashes = 0usize;
    for c in arg.chars() {
        match c {
            '\\' => slashes += 1,
            '"' => {
                out.extend(std::iter::repeat_n('\\', slashes * 2 + 1));
                out.push('"');
                slashes = 0;
            }
            _ => {
                out.extend(std::iter::repeat_n('\\', slashes));
                slashes = 0;
                out.push(c);
            }
        }
    }
    out.extend(std::iter::repeat_n('\\', slashes * 2));
    out.push('"');
    out
}

/// The command line that starts a new window of `exe` with `args`.
pub fn command_line(exe: &str, args: &[String]) -> String {
    let mut line = quote_arg(exe);
    line.push(' ');
    line.push_str(SWITCH);
    for a in args {
        line.push(' ');
        line.push_str(&quote_arg(a));
    }
    line
}

/// Starts a new goz-gui window with `args` (a search, its options); false
/// if it could not be started. It may come to the front: this process
/// passes on its right to (a hotkey, a tray click or a start just gave it).
pub fn spawn(args: &[String]) -> bool {
    // A test's dry run notes it instead (`GOZ_GUI_DRY_RUN`).
    if crate::store::location::dry_run_instead("new-window", args) {
        return true;
    }
    let Ok(exe) = std::env::current_exe() else {
        return false;
    };
    let exe = exe.to_string_lossy().into_owned();
    let app = wz(&exe);
    let mut line = wz(&command_line(&exe, args));
    // SAFETY: CreateProcessW gets the NUL-terminated exe path and a
    // writable command line (it may change it in place) that outlive the
    // call, and plain structs; the handles it returns are closed here.
    unsafe {
        let si = STARTUPINFOW {
            cb: size_of::<STARTUPINFOW>() as u32,
            ..core::mem::zeroed()
        };
        let mut pi: PROCESS_INFORMATION = core::mem::zeroed();
        let ok = CreateProcessW(
            app.as_ptr(),
            line.as_mut_ptr(),
            core::ptr::null(),
            core::ptr::null(),
            0,
            CREATE_UNICODE_ENVIRONMENT,
            core::ptr::null(),
            core::ptr::null(),
            &si,
            &mut pi,
        ) != 0;
        if ok {
            AllowSetForegroundWindow(pi.dwProcessId);
            CloseHandle(pi.hThread);
            CloseHandle(pi.hProcess);
        }
        ok
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// How `CommandLineToArgvW` splits `line`, as the new process will.
    fn argv(line: &str) -> Vec<String> {
        use windows_sys::Win32::Foundation::LocalFree;
        use windows_sys::Win32::UI::Shell::CommandLineToArgvW;
        let w = wz(line);
        let mut n = 0;
        // SAFETY: a NUL-terminated line; the array is read for its `n`
        // strings, then freed with LocalFree as documented.
        unsafe {
            let list = CommandLineToArgvW(w.as_ptr(), &mut n);
            let out = (0..n as usize)
                .map(|i| {
                    let p = *list.add(i);
                    let len = (0..).take_while(|&j| *p.add(j) != 0).count();
                    String::from_utf16_lossy(core::slice::from_raw_parts(p, len))
                })
                .collect();
            LocalFree(list.cast());
            out
        }
    }

    #[test]
    fn arguments_come_back_as_they_were() {
        let args: Vec<String> = [
            "plain",
            "two words",
            "",
            r#"say "hi""#,
            r"C:\Program Files\",
            r"C:\x\\",
            r#"a\"b"#,
            r"\\server\share",
            "*.mp3 dm:today",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        let exe = r"C:\Program Files\goz gui\goz-gui.exe";
        let back = argv(&command_line(exe, &args));
        assert_eq!(back[0], exe);
        assert_eq!(back[1], SWITCH);
        assert_eq!(&back[2..], &args[..]);
    }

    #[test]
    fn plain_arguments_stay_unquoted() {
        assert_eq!(quote_arg("-s"), "-s");
        assert_eq!(quote_arg(r"C:\x"), r"C:\x");
        assert_eq!(quote_arg(""), "\"\"");
        assert_eq!(quote_arg("a b"), "\"a b\"");
        assert_eq!(command_line("x.exe", &[]), "x.exe -new-window");
    }
}
