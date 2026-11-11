//! Help > Troubleshooting Information: Everything's read-only text of what
//! goz-gui is and where it runs, for a bug report: goz-gui's version, the
//! OS, gozd's name and version and the protocol it speaks (from its
//! greeting), whether it reads OR, NOT and groups, the command line, the
//! exe, the data folder and why it is that one (portable or not), the
//! instance, the DPI and theme, the process's memory, the open file list,
//! and the settings that differ from the defaults (Everything's "Config").
//! Copy puts it all on the clipboard.

use std::cell::Cell;

use windows_sys::Win32::Foundation::{HWND, LPARAM, RECT, WPARAM};
use windows_sys::Win32::System::ProcessStatus::{
    K32GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS, PROCESS_MEMORY_COUNTERS_EX,
};
use windows_sys::Win32::System::Registry::{
    HKEY_LOCAL_MACHINE, RRF_RT_REG_DWORD, RRF_RT_REG_SZ, RegGetValueW,
};
use windows_sys::Win32::System::Threading::GetCurrentProcess;
use windows_sys::Win32::UI::Controls::{EM_SETSEL, EM_SETTABSTOPS};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::SetFocus;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    BS_DEFPUSHBUTTON, ES_AUTOHSCROLL, ES_AUTOVSCROLL, ES_MULTILINE, ES_NOHIDESEL, ES_READONLY,
    EndDialog, GWLP_USERDATA, GetWindowRect, IDCANCEL, MINMAXINFO, SendMessageW, SetWindowLongPtrW,
    SetWindowTextW, WM_COMMAND, WM_GETMINMAXINFO, WM_INITDIALOG, WM_SIZE, WS_EX_CLIENTEDGE,
    WS_HSCROLL, WS_MAXIMIZEBOX, WS_TABSTOP, WS_THICKFRAME, WS_VSCROLL,
};

use crate::dialog::{
    self, Class, GROW_X, GROW_Y, Look, MOVE_X, MOVE_Y, Template, Units, center_on_owner, common,
    state,
};
use crate::filetools::ui::item;
use crate::store::location::Source;
use crate::wide::wz;

/// What the text tells, gathered by the window.
pub struct Facts {
    pub gozd: Option<(String, u16)>,
    pub boolean_syntax: bool,
    pub command_line: String,
    pub exe: String,
    pub data_dir: Option<String>,
    pub data_source: Option<Source>,
    pub instance_key: String,
    pub instance: Option<String>,
    pub dpi: u32,
    pub zoom: u32,
    pub dark: bool,
    pub theme_setting: &'static str,
    pub file_list: Option<String>,
    /// The settings that differ from the defaults, as `key=value`.
    pub config: Vec<(String, String)>,
}

/// The OS as Everything writes it: `Windows NT 10.0 26300 (x64)`, with the
/// update and the release (`24H2`) when the registry says.
pub fn os_version() -> String {
    const KEY: &str = r"SOFTWARE\Microsoft\Windows NT\CurrentVersion";
    let dword = |name: &str| -> Option<u32> {
        let (key, name) = (wz(KEY), wz(name));
        let mut v = 0u32;
        let mut size = 4u32;
        // SAFETY: NUL-terminated names and a 4-byte buffer whose size is
        // passed; reads HKLM only.
        let rc = unsafe {
            RegGetValueW(
                HKEY_LOCAL_MACHINE,
                key.as_ptr(),
                name.as_ptr(),
                RRF_RT_REG_DWORD,
                core::ptr::null_mut(),
                (&raw mut v).cast(),
                &mut size,
            )
        };
        (rc == 0).then_some(v)
    };
    let string = |name: &str| -> Option<String> {
        let (key, name) = (wz(KEY), wz(name));
        let mut buf = vec![0u16; 256];
        let mut size = (buf.len() * 2) as u32;
        // SAFETY: as above, with a buffer whose byte size is passed.
        let rc = unsafe {
            RegGetValueW(
                HKEY_LOCAL_MACHINE,
                key.as_ptr(),
                name.as_ptr(),
                RRF_RT_REG_SZ,
                core::ptr::null_mut(),
                buf.as_mut_ptr().cast(),
                &mut size,
            )
        };
        if rc != 0 {
            return None;
        }
        buf.truncate((size as usize / 2).saturating_sub(1));
        Some(String::from_utf16_lossy(&buf))
    };
    let major = dword("CurrentMajorVersionNumber").unwrap_or(10);
    let minor = dword("CurrentMinorVersionNumber").unwrap_or(0);
    let build = string("CurrentBuild").unwrap_or_default();
    let mut text = format!("Windows NT {major}.{minor} {build}");
    if let Some(ubr) = dword("UBR") {
        text.push_str(&format!(".{ubr}"));
    }
    if let Some(release) = string("DisplayVersion") {
        text.push_str(&format!(" {release}"));
    }
    text.push_str(" (x64)");
    text
}

/// This process's private bytes and working set.
pub fn memory() -> Option<(u64, u64)> {
    // SAFETY: a plain out-struct whose size is passed, for this process.
    unsafe {
        let mut c: PROCESS_MEMORY_COUNTERS_EX = core::mem::zeroed();
        c.cb = size_of::<PROCESS_MEMORY_COUNTERS_EX>() as u32;
        let ok = K32GetProcessMemoryInfo(
            GetCurrentProcess(),
            (&raw mut c).cast::<PROCESS_MEMORY_COUNTERS>(),
            c.cb,
        );
        (ok != 0).then_some((c.PrivateUsage as u64, c.WorkingSetSize as u64))
    }
}

fn megabytes(bytes: u64) -> String {
    format!("{:.1} MB", bytes as f64 / (1024.0 * 1024.0))
}

/// The text, lines ending in `\n`.
pub fn text(f: &Facts, os: &str, memory: Option<(u64, u64)>) -> String {
    let mut lines = vec![
        concat!("goz-gui ", env!("CARGO_PKG_VERSION"), " (x64)").to_owned(),
        format!("OS:\t{os}"),
    ];
    let speaks = goz_core::proto::PROTO_VERSION;
    lines.push(match &f.gozd {
        Some((server, proto)) => {
            format!("gozd:\t{server}, protocol {proto} (goz-gui speaks {speaks})")
        }
        None => format!("gozd:\tnot reached yet (goz-gui speaks protocol {speaks})"),
    });
    lines.push(format!(
        "Search syntax:\t{}",
        if f.boolean_syntax {
            "OR, NOT and groups (Everything 1.5's)"
        } else {
            "goz's first syntax (no OR, NOT or groups)"
        }
    ));
    lines.push(format!("Command line:\t{}", f.command_line));
    lines.push(format!("Binary:\t{}", f.exe));
    let source = match f.data_source {
        Some(Source::Variable) => " (GOZ_GUI_DATA_DIR)",
        Some(Source::Portable) => " (portable: app_data=0 in the goz-gui.ini beside the exe)",
        Some(Source::AppData) => " (%APPDATA%)",
        None => "",
    };
    lines.push(format!(
        "Data folder:\t{}{source}",
        f.data_dir
            .as_deref()
            .unwrap_or("(none: Windows did not say where AppData is)")
    ));
    lines.push(match &f.instance {
        Some(name) => format!("Instance:\t{name} ({})", f.instance_key),
        None => format!("Instance:\t{}", f.instance_key),
    });
    lines.push(format!(
        "DPI:\t{} ({} %), zoom {} %",
        f.dpi,
        f.dpi * 100 / 96,
        f.zoom
    ));
    lines.push(format!(
        "Theme:\t{} (View > Theme: {})",
        if f.dark { "Dark" } else { "Light" },
        f.theme_setting
    ));
    if let Some((private, working)) = memory {
        lines.push(format!(
            "Memory:\t{} private, {} working set",
            megabytes(private),
            megabytes(working)
        ));
    }
    if let Some(list) = &f.file_list {
        lines.push(format!("File list:\t{list}"));
    }
    lines.push(String::new());
    lines.push("Config:".into());
    for (k, v) in &f.config {
        lines.push(format!("{k}={v}"));
    }
    let mut out = lines.join("\n");
    out.push('\n');
    out
}

const ID_TEXT: u16 = 10000;
const ID_COPY: u16 = 1001;
const CLOSE: u16 = IDCANCEL as u16;
const SIZE: (i16, i16) = (340, 250);
const PLACES: [(u16, Units, u8); 3] = [
    (ID_TEXT, (7, 7, SIZE.0 - 14, SIZE.1 - 35), GROW_X | GROW_Y),
    (
        ID_COPY,
        (SIZE.0 - 111, SIZE.1 - 21, 50, 14),
        MOVE_X | MOVE_Y,
    ),
    (CLOSE, (SIZE.0 - 57, SIZE.1 - 21, 50, 14), MOVE_X | MOVE_Y),
];

struct Window {
    look: Cell<Look>,
    text: String,
    min: Cell<(i32, i32)>,
}

/// Shows `text` over `owner` until it is closed.
pub fn show(owner: HWND, text: &str, look: Look) {
    let state = Window {
        look: Cell::new(look),
        text: text.replace('\n', "\r\n"),
        min: Cell::new((0, 0)),
    };
    let mut t = Template::new(
        "Troubleshooting Information",
        WS_THICKFRAME | WS_MAXIMIZEBOX,
        SIZE.0,
        SIZE.1,
    );
    t.control(
        Class::Edit,
        "",
        ID_TEXT,
        WS_TABSTOP
            | WS_VSCROLL
            | WS_HSCROLL
            | (ES_MULTILINE | ES_READONLY | ES_AUTOVSCROLL | ES_AUTOHSCROLL | ES_NOHIDESEL) as u32,
        WS_EX_CLIENTEDGE,
        PLACES[0].1,
    )
    .control(Class::Button, "&Copy", ID_COPY, WS_TABSTOP, 0, PLACES[1].1)
    .control(
        Class::Button,
        "Close",
        CLOSE,
        WS_TABSTOP | BS_DEFPUSHBUTTON as u32,
        0,
        PLACES[2].1,
    );
    t.run(owner, Some(proc), (&raw const state) as LPARAM);
}

unsafe extern "system" fn proc(dlg: HWND, msg: u32, w: WPARAM, l: LPARAM) -> isize {
    if msg == WM_INITDIALOG {
        // SAFETY: `l` is the `Window` that `show` keeps alive until the
        // dialog has ended.
        unsafe { SetWindowLongPtrW(dlg, GWLP_USERDATA, l) };
    }
    // SAFETY: GWLP_USERDATA is the `Window` from `show`, or null.
    let Some(s) = (unsafe { state::<Window>(dlg) }) else {
        return 0;
    };
    if let Some(answer) = common(dlg, msg, w, l, &s.look) {
        return answer;
    }
    match msg {
        WM_INITDIALOG => {
            let edit = item(dlg, ID_TEXT);
            let z = wz(&s.text);
            // One tab stop past the longest label ("Search syntax:"), in
            // dialog units, so the values line up.
            let stop: u32 = 64;
            // SAFETY: NUL-terminated text, and a tab stop the box copies,
            // for this thread's text box.
            unsafe {
                SendMessageW(edit, EM_SETTABSTOPS, 1, (&raw const stop) as isize);
                SetWindowTextW(edit, z.as_ptr());
                SendMessageW(edit, EM_SETSEL, 0, 0);
            }
            dialog::theme_new(dlg, s.look.get().dark);
            center_on_owner(dlg);
            let mut wr = RECT::default();
            // SAFETY: reads this thread's dialog.
            unsafe { GetWindowRect(dlg, &mut wr) };
            s.min
                .set(((wr.right - wr.left) / 2, (wr.bottom - wr.top) / 2));
            // SAFETY: focuses this thread's text box.
            unsafe { SetFocus(edit) };
            0
        }
        WM_SIZE => {
            dialog::anchor(dlg, SIZE, &PLACES);
            1
        }
        WM_GETMINMAXINFO => {
            let (w, h) = s.min.get();
            // SAFETY: WM_GETMINMAXINFO's lParam is a MINMAXINFO to fill.
            let mmi = unsafe { &mut *(l as *mut MINMAXINFO) };
            mmi.ptMinTrackSize.x = w;
            mmi.ptMinTrackSize.y = h;
            1
        }
        WM_COMMAND => {
            match (w & 0xffff) as u16 {
                ID_COPY => {
                    let units: Vec<u16> = s.text.encode_utf16().collect();
                    crate::clipboard::put_text(dlg, &units);
                }
                // SAFETY: ends this modal dialog.
                CLOSE => unsafe {
                    EndDialog(dlg, 0);
                },
                _ => return 0,
            }
            1
        }
        _ => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn says_what_a_bug_report_needs() {
        let f = Facts {
            gozd: Some(("gozd 0.6.0".into(), 1)),
            boolean_syntax: true,
            command_line: "-s x".into(),
            exe: r"C:\goz-gui.exe".into(),
            data_dir: Some(r"C:\data".into()),
            data_source: Some(Source::Portable),
            instance_key: "goz-gui-abc".into(),
            instance: None,
            dpi: 144,
            zoom: 100,
            dark: true,
            theme_setting: "User Default",
            file_list: Some(r"C:\a.efu, 3 rows".into()),
            config: vec![("theme".into(), "2".into())],
        };
        let t = text(&f, "Windows NT 10.0 26300 (x64)", Some((5 << 20, 12 << 20)));
        assert!(t.starts_with(concat!("goz-gui ", env!("CARGO_PKG_VERSION"))));
        for want in [
            "OS:\tWindows NT 10.0 26300 (x64)\n",
            "gozd:\tgozd 0.6.0, protocol 1",
            "Search syntax:\tOR, NOT and groups",
            "Data folder:\tC:\\data (portable",
            "DPI:\t144 (150 %)",
            "Theme:\tDark (View > Theme: User Default)",
            "Memory:\t5.0 MB private, 12.0 MB working set",
            "File list:\tC:\\a.efu, 3 rows",
            "Config:\ntheme=2\n",
        ] {
            assert!(t.contains(want), "{want:?} in {t}");
        }
        let none = Facts { gozd: None, ..f };
        assert!(text(&none, "x", None).contains("gozd:\tnot reached yet"));
        assert!(os_version().starts_with("Windows NT 10."));
        assert!(memory().is_some_and(|(p, w)| p > 0 && w > 0));
    }
}
