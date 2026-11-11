//! What goz-gui puts in the user's registry and folders when asked in
//! Options > General: starting with Windows (a `Run` value), the "Search
//! goz..." item on folders in Explorer, the `goz:` URL protocol, the
//! Start menu and Desktop shortcuts, and (wave 4) the `.efu` file
//! association; all for the current user, and each removed again when
//! turned off.
//!
//! `GOZ_GUI_REG_ROOT` (tests) moves the registry's under `HKCU\<root>`, and
//! `GOZ_GUI_SHORTCUT_DIR` the shortcuts under `<dir>\Programs` and
//! `<dir>\Desktop`, so a test never touches the real ones.

use std::ffi::OsString;
use std::os::windows::ffi::OsStringExt;
use std::path::PathBuf;

use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, CoCreateInstance, IPersistFile};
use windows::Win32::UI::Shell::{IShellLinkW, ShellLink};
use windows::core::{Interface, PCWSTR};
use windows_sys::Win32::Foundation::ERROR_SUCCESS;
use windows_sys::Win32::System::Com::CoTaskMemFree;
use windows_sys::Win32::System::Registry::{
    HKEY, HKEY_CURRENT_USER, KEY_WRITE, REG_OPTION_NON_VOLATILE, REG_SZ, RRF_RT_REG_SZ,
    RegCloseKey, RegCreateKeyExW, RegDeleteKeyValueW, RegDeleteKeyW, RegDeleteTreeW, RegGetValueW,
    RegSetValueExW,
};
use windows_sys::Win32::UI::Shell::{FOLDERID_Desktop, FOLDERID_Programs, SHGetKnownFolderPath};
use windows_sys::core::GUID;

use crate::wide::wz;

/// The test override of where the keys go.
pub const REG_ROOT_VAR: &str = "GOZ_GUI_REG_ROOT";
/// The test override of where the shortcuts go.
pub const SHORTCUT_DIR_VAR: &str = "GOZ_GUI_SHORTCUT_DIR";

/// The URL scheme goz-gui answers: `goz:<search>?<options>` (`crate::url`).
const PROTOCOL: &str = "goz";
/// The shortcuts' file name, and the tip Explorer shows for them.
const SHORTCUT_NAME: &str = "goz-gui.lnk";
const SHORTCUT_TIP: &str = "Search files and folders by name with goz";

const RUN_VALUE: &str = "goz-gui";
/// The folder item's key name under each `...\shell`.
const VERB: &str = "goz-gui";
const VERB_LABEL: &str = "Search goz...";

fn root() -> Option<String> {
    std::env::var(REG_ROOT_VAR).ok().filter(|v| !v.is_empty())
}

fn run_key() -> String {
    match root() {
        Some(r) => format!(r"{r}\Run"),
        None => r"Software\Microsoft\Windows\CurrentVersion\Run".to_owned(),
    }
}

fn classes() -> String {
    match root() {
        Some(r) => format!(r"{r}\Classes"),
        None => r"Software\Classes".to_owned(),
    }
}

/// The verbs' keys and the argument each passes: folders and drives name
/// themselves (`%1`), a folder's background its folder (`%V`).
fn verb_keys() -> [(String, &'static str); 3] {
    let c = classes();
    [
        (format!(r"{c}\Directory\shell\{VERB}"), "%1"),
        (format!(r"{c}\Directory\Background\shell\{VERB}"), "%V"),
        (format!(r"{c}\Drive\shell\{VERB}"), "%1"),
    ]
}

/// This exe's path, quoted for a command line.
fn exe() -> Option<String> {
    let path = std::env::current_exe().ok()?;
    Some(format!("\"{}\"", path.display()))
}

/// The command Windows runs at sign-in.
pub fn startup_command() -> Option<String> {
    Some(format!("{} -startup", exe()?))
}

/// The command a folder's "Search goz..." runs, with `arg` for the folder.
fn verb_command(arg: &str) -> Option<String> {
    Some(format!("{} -path \"{arg}\"", exe()?))
}

fn get(key: &str, value: Option<&str>) -> Option<String> {
    let key = wz(key);
    let value = value.map(wz);
    let mut buf = vec![0u16; 2048];
    let mut size = (buf.len() * 2) as u32;
    // SAFETY: NUL-terminated names and a buffer whose byte size is passed.
    let rc = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            key.as_ptr(),
            value.as_ref().map_or(core::ptr::null(), |v| v.as_ptr()),
            RRF_RT_REG_SZ,
            core::ptr::null_mut(),
            buf.as_mut_ptr().cast(),
            &mut size,
        )
    };
    if rc != ERROR_SUCCESS {
        return None;
    }
    let len = (size as usize / 2).saturating_sub(1).min(buf.len());
    buf.truncate(len);
    Some(String::from_utf16_lossy(&buf))
}

fn set(key: &str, value: Option<&str>, data: &str) -> bool {
    let k = wz(key);
    let value = value.map(wz);
    let data = wz(data);
    // SAFETY: creates or opens a key under HKCU, writes a REG_SZ of the
    // NUL-terminated data's byte length, and closes the key.
    unsafe {
        let mut h: HKEY = core::ptr::null_mut();
        if RegCreateKeyExW(
            HKEY_CURRENT_USER,
            k.as_ptr(),
            0,
            core::ptr::null(),
            REG_OPTION_NON_VOLATILE,
            KEY_WRITE,
            core::ptr::null(),
            &mut h,
            core::ptr::null_mut(),
        ) != ERROR_SUCCESS
        {
            return false;
        }
        let rc = RegSetValueExW(
            h,
            value.as_ref().map_or(core::ptr::null(), |v| v.as_ptr()),
            0,
            REG_SZ,
            data.as_ptr().cast(),
            (data.len() * 2) as u32,
        );
        RegCloseKey(h);
        rc == ERROR_SUCCESS
    }
}

/// Whether Windows starts this goz-gui at sign-in.
pub fn starts_with_windows() -> bool {
    get(&run_key(), Some(RUN_VALUE)).is_some()
}

/// Starts this goz-gui (in the background) at sign-in, or stops doing so.
pub fn set_starts_with_windows(on: bool) -> bool {
    if on {
        startup_command().is_some_and(|cmd| set(&run_key(), Some(RUN_VALUE), &cmd))
    } else {
        let key = wz(&run_key());
        let value = wz(RUN_VALUE);
        // SAFETY: NUL-terminated names under HKCU.
        let rc = unsafe { RegDeleteKeyValueW(HKEY_CURRENT_USER, key.as_ptr(), value.as_ptr()) };
        rc == ERROR_SUCCESS || !starts_with_windows()
    }
}

/// Whether folders in Explorer have "Search goz...".
pub fn has_folder_item() -> bool {
    get(&format!(r"{}\command", verb_keys()[0].0), None).is_some()
}

/// Adds or removes "Search goz..." on folders, folder backgrounds and
/// drives.
pub fn set_folder_item(on: bool) -> bool {
    let mut ok = true;
    for (key, arg) in verb_keys() {
        if on {
            ok &= set(&key, None, VERB_LABEL)
                && verb_command(arg).is_some_and(|cmd| set(&format!(r"{key}\command"), None, &cmd));
        } else {
            let k = wz(&key);
            // SAFETY: NUL-terminated paths under HKCU: the verb's subkeys
            // and values, then the verb's key itself (an error for a key
            // already gone is fine).
            unsafe {
                RegDeleteTreeW(HKEY_CURRENT_USER, k.as_ptr());
                RegDeleteKeyW(HKEY_CURRENT_USER, k.as_ptr());
            }
        }
    }
    ok && has_folder_item() == on
}

/// `HKCU\Software\Classes\goz`, as Everything's `es:` is under `ES`.
fn protocol_key() -> String {
    format!(r"{}\{PROTOCOL}", classes())
}

/// The command Windows runs for a `goz:` link.
fn url_command() -> Option<String> {
    Some(format!("{} -url \"%1\"", exe()?))
}

/// Whether a protocol's command is goz-gui's (this copy's or another's, as
/// [`url_command`] writes it): another program's `goz:` is neither shown
/// as on nor removed.
fn is_url_command(cmd: &str) -> bool {
    cmd.starts_with('"') && cmd.ends_with("\" -url \"%1\"")
}

/// Whether `goz:` links open goz-gui.
pub fn has_url_protocol() -> bool {
    get(&format!(r"{}\shell\open\command", protocol_key()), None)
        .is_some_and(|c| is_url_command(&c))
}

/// Registers the `goz:` URL protocol for this goz-gui, or removes it (only
/// when it is goz-gui's).
pub fn set_url_protocol(on: bool) -> bool {
    let key = protocol_key();
    if on {
        let Some(cmd) = url_command() else {
            return false;
        };
        let icon = std::env::current_exe()
            .map(|p| format!("{},0", p.display()))
            .unwrap_or_default();
        set(&key, None, "URL:goz Protocol")
            && set(&key, Some("URL Protocol"), "")
            && set(&format!(r"{key}\DefaultIcon"), None, &icon)
            && set(&format!(r"{key}\shell\open\command"), None, &cmd)
            && has_url_protocol()
    } else {
        if has_url_protocol() {
            let k = wz(&key);
            // SAFETY: NUL-terminated paths under HKCU: the protocol's
            // subkeys and values, then its key.
            unsafe {
                RegDeleteTreeW(HKEY_CURRENT_USER, k.as_ptr());
                RegDeleteKeyW(HKEY_CURRENT_USER, k.as_ptr());
            }
        }
        !has_url_protocol()
    }
}

/// The `.efu` file association's own key: Everything File Lists open in
/// goz-gui as file lists (`-filelist "%1"`).
const EFU_PROG_ID: &str = "goz-gui.FileList";
/// Where `.efu`'s program was before goz-gui took it, to give it back.
const EFU_PREVIOUS: &str = "goz-gui.previous";

fn efu_key() -> String {
    format!(r"{}\.efu", classes())
}

fn efu_prog_key() -> String {
    format!(r"{}\{EFU_PROG_ID}", classes())
}

/// The command Windows runs to open a file list.
fn efu_command() -> Option<String> {
    Some(format!("{} -filelist \"%1\"", exe()?))
}

/// Whether a command is goz-gui's file list command (this copy's or
/// another's).
fn is_efu_command(cmd: &str) -> bool {
    cmd.starts_with('"') && cmd.ends_with("\" -filelist \"%1\"")
}

/// Whether `.efu` files open in goz-gui.
pub fn has_efu_association() -> bool {
    get(&efu_key(), None).as_deref() == Some(EFU_PROG_ID)
        && get(&format!(r"{}\shell\open\command", efu_prog_key()), None)
            .is_some_and(|c| is_efu_command(&c))
}

/// Tells Explorer that a file association changed (not for a test's keys,
/// which Windows does not read).
fn associations_changed() {
    if root().is_some() {
        return;
    }
    use windows_sys::Win32::UI::Shell::{SHCNE_ASSOCCHANGED, SHCNF_IDLIST, SHChangeNotify};
    // SAFETY: a notification with no items.
    unsafe {
        SHChangeNotify(
            SHCNE_ASSOCCHANGED as i32,
            SHCNF_IDLIST,
            core::ptr::null(),
            core::ptr::null(),
        )
    };
}

/// Makes `.efu` files (Everything File Lists) open in this goz-gui as file
/// lists, or gives them back to the program they had before (only when
/// they are goz-gui's).
pub fn set_efu_association(on: bool) -> bool {
    let (ext, prog) = (efu_key(), efu_prog_key());
    if on {
        let Some(cmd) = efu_command() else {
            return false;
        };
        let icon = std::env::current_exe()
            .map(|p| format!("{},0", p.display()))
            .unwrap_or_default();
        // The program `.efu` had (Everything's, say) is kept to give back.
        let previous = get(&ext, None).filter(|p| p != EFU_PROG_ID && !p.is_empty());
        // W6: and Everything's Edit verb, the File List Editor's.
        let edit = cmd.replace(" -filelist \"%1\"", " -edit \"%1\"");
        let ok = set(&prog, None, "Everything File List")
            && set(&format!(r"{prog}\DefaultIcon"), None, &icon)
            && set(&format!(r"{prog}\shell\open\command"), None, &cmd)
            && set(&format!(r"{prog}\shell\edit\command"), None, &edit)
            && previous.is_none_or(|p| set(&ext, Some(EFU_PREVIOUS), &p))
            && set(&ext, None, EFU_PROG_ID);
        associations_changed();
        ok && has_efu_association()
    } else {
        if get(&ext, None).as_deref() == Some(EFU_PROG_ID) {
            let previous = get(&ext, Some(EFU_PREVIOUS)).unwrap_or_default();
            set(&ext, None, &previous);
            let k = wz(&ext);
            let v = wz(EFU_PREVIOUS);
            // SAFETY: NUL-terminated names under HKCU; a value already gone
            // is fine.
            unsafe { RegDeleteKeyValueW(HKEY_CURRENT_USER, k.as_ptr(), v.as_ptr()) };
        }
        let is_ours =
            get(&format!(r"{prog}\shell\open\command"), None).is_some_and(|c| is_efu_command(&c));
        if is_ours {
            let k = wz(&prog);
            // SAFETY: NUL-terminated paths under HKCU: the key's subkeys
            // and values, then the key.
            unsafe {
                RegDeleteTreeW(HKEY_CURRENT_USER, k.as_ptr());
                RegDeleteKeyW(HKEY_CURRENT_USER, k.as_ptr());
            }
        }
        associations_changed();
        !has_efu_association()
    }
}

/// The shortcuts Options > General makes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Shortcut {
    /// In the user's Start menu (its Programs folder).
    StartMenu,
    /// On the user's desktop.
    Desktop,
}

/// A known folder's path (`FOLDERID_*`).
fn known_folder(id: &GUID) -> Option<PathBuf> {
    let mut raw: *mut u16 = core::ptr::null_mut();
    // SAFETY: the shell allocates the string, which is read up to its NUL
    // and freed with CoTaskMemFree as documented (also on failure).
    unsafe {
        let hr = SHGetKnownFolderPath(id, 0, core::ptr::null_mut(), &mut raw);
        let path = if hr >= 0 && !raw.is_null() {
            let len = (0..).take_while(|&i| *raw.add(i) != 0).count();
            Some(PathBuf::from(OsString::from_wide(
                core::slice::from_raw_parts(raw, len),
            )))
        } else {
            None
        };
        CoTaskMemFree(raw.cast());
        path
    }
}

/// Where shortcut `which` goes.
fn shortcut_path(which: Shortcut) -> Option<PathBuf> {
    let test = std::env::var(SHORTCUT_DIR_VAR)
        .ok()
        .filter(|v| !v.is_empty());
    let folder = match (test, which) {
        (Some(dir), Shortcut::StartMenu) => PathBuf::from(dir).join("Programs"),
        (Some(dir), Shortcut::Desktop) => PathBuf::from(dir).join("Desktop"),
        (None, Shortcut::StartMenu) => known_folder(&FOLDERID_Programs)?,
        (None, Shortcut::Desktop) => known_folder(&FOLDERID_Desktop)?,
    };
    Some(folder.join(SHORTCUT_NAME))
}

/// Whether shortcut `which` is there.
pub fn has_shortcut(which: Shortcut) -> bool {
    shortcut_path(which).is_some_and(|p| p.is_file())
}

/// Makes shortcut `which` to this goz-gui, or deletes it. The thread has
/// COM (the window's does).
pub fn set_shortcut(which: Shortcut, on: bool) -> bool {
    let Some(path) = shortcut_path(which) else {
        return false;
    };
    if !on {
        return match std::fs::remove_file(&path) {
            Ok(()) => true,
            Err(e) => e.kind() == std::io::ErrorKind::NotFound,
        };
    }
    let Ok(exe) = std::env::current_exe() else {
        return false;
    };
    if let Some(folder) = path.parent()
        && std::fs::create_dir_all(folder).is_err()
    {
        return false;
    }
    write_link(&exe, &path).is_ok() && path.is_file()
}

/// Writes a shortcut at `link` that starts `exe` in its own folder.
fn write_link(exe: &std::path::Path, link: &std::path::Path) -> windows::core::Result<()> {
    let target = wz(&exe.to_string_lossy());
    let dir = wz(&exe
        .parent()
        .map_or(String::new(), |d| d.to_string_lossy().into_owned()));
    let tip = wz(SHORTCUT_TIP);
    let file = wz(&link.to_string_lossy());
    // SAFETY: COM is ready on this thread; every string is NUL-terminated
    // and outlives the calls that read it.
    unsafe {
        let shell_link: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER)?;
        shell_link.SetPath(PCWSTR(target.as_ptr()))?;
        shell_link.SetWorkingDirectory(PCWSTR(dir.as_ptr()))?;
        shell_link.SetDescription(PCWSTR(tip.as_ptr()))?;
        shell_link.SetIconLocation(PCWSTR(target.as_ptr()), 0)?;
        shell_link
            .cast::<IPersistFile>()?
            .Save(PCWSTR(file.as_ptr()), true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Turns both on and off again under a throwaway key, never the
    /// user's real ones.
    #[test]
    fn registry_round_trip_under_a_test_root() {
        let root = format!(r"Software\goz-gui-test-{}", std::process::id());
        // SAFETY: only this module reads the variable, and only this test
        // sets it.
        unsafe { std::env::set_var(REG_ROOT_VAR, &root) };
        assert!(!starts_with_windows() && !has_folder_item() && !has_url_protocol());
        assert!(set_starts_with_windows(true) && starts_with_windows());
        assert!(set_folder_item(true) && has_folder_item());
        let cmd = get(&format!(r"{}\command", verb_keys()[1].0), None).unwrap();
        assert!(cmd.ends_with(" -path \"%V\""), "{cmd}");
        assert!(set_url_protocol(true) && has_url_protocol());
        let key = protocol_key();
        assert!(key.ends_with(r"\Classes\goz"), "{key}");
        assert_eq!(get(&key, None).as_deref(), Some("URL:goz Protocol"));
        assert_eq!(get(&key, Some("URL Protocol")).as_deref(), Some(""));
        let open = get(&format!(r"{key}\shell\open\command"), None).unwrap();
        assert!(
            open.starts_with('"') && open.ends_with("\" -url \"%1\""),
            "{open}"
        );
        assert!(set_starts_with_windows(false) && !starts_with_windows());
        assert!(set_folder_item(false) && !has_folder_item());
        assert!(set_url_protocol(false) && !has_url_protocol());
        assert_eq!(get(&key, None), None, "the whole key goes");
        // Another program's goz: is neither taken for goz-gui's nor removed.
        assert!(set(
            &format!(r"{key}\shell\open\command"),
            None,
            "other.exe \"%1\""
        ));
        assert!(!has_url_protocol());
        assert!(set_url_protocol(false));
        assert!(get(&format!(r"{key}\shell\open\command"), None).is_some());
        // The .efu association (wave 4): Everything's taken over, then
        // given back.
        assert!(!has_efu_association());
        assert!(set(&efu_key(), None, "Everything.FileList"));
        assert!(set_efu_association(true) && has_efu_association());
        let open = get(&format!(r"{}\shell\open\command", efu_prog_key()), None).unwrap();
        assert!(open.ends_with("\" -filelist \"%1\""), "{open}");
        let edit = get(&format!(r"{}\shell\edit\command", efu_prog_key()), None).unwrap();
        assert!(edit.ends_with("\" -edit \"%1\""), "{edit}");
        assert!(set_efu_association(false) && !has_efu_association());
        assert_eq!(
            get(&efu_key(), None).as_deref(),
            Some("Everything.FileList"),
            "the program before is back"
        );
        assert_eq!(get(&efu_prog_key(), None), None);
        let k = wz(&root);
        // SAFETY: deletes the throwaway key made above.
        unsafe { RegDeleteTreeW(HKEY_CURRENT_USER, k.as_ptr()) };
        unsafe { RegDeleteKeyW(HKEY_CURRENT_USER, k.as_ptr()) };
        // SAFETY: as above.
        unsafe { std::env::remove_var(REG_ROOT_VAR) };
    }

    /// Makes and deletes both shortcuts under a throwaway folder, never the
    /// user's own Start menu or desktop.
    #[test]
    fn shortcuts_round_trip_under_a_test_folder() {
        use windows_sys::Win32::System::Com::{COINIT_APARTMENTTHREADED, CoInitializeEx};
        // SAFETY: COM for this test thread (the window's thread has it).
        unsafe { CoInitializeEx(core::ptr::null(), COINIT_APARTMENTTHREADED as u32) };
        let dir = std::env::temp_dir().join(format!("goz-gui-shortcuts-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        // SAFETY: only this test sets the variable, and only this module
        // reads it.
        unsafe { std::env::set_var(SHORTCUT_DIR_VAR, &dir) };
        for which in [Shortcut::StartMenu, Shortcut::Desktop] {
            assert!(!has_shortcut(which));
            assert!(
                set_shortcut(which, true) && has_shortcut(which),
                "{which:?}"
            );
            let path = shortcut_path(which).unwrap();
            assert!(path.starts_with(&dir) && path.ends_with(SHORTCUT_NAME));
            // It is a real shortcut to this exe.
            let w: Vec<u16> = path.to_string_lossy().encode_utf16().collect();
            let target = crate::shell::link_target(&w).map(|t| String::from_utf16_lossy(&t));
            let exe = std::env::current_exe().unwrap();
            assert_eq!(target.as_deref(), Some(&*exe.to_string_lossy()));
            assert!(set_shortcut(which, false) && !has_shortcut(which));
            // Deleting one that is gone is fine.
            assert!(set_shortcut(which, false));
        }
        assert!(dir.join("Programs").is_dir() && dir.join("Desktop").is_dir());
        // SAFETY: as above.
        unsafe { std::env::remove_var(SHORTCUT_DIR_VAR) };
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn only_goz_guis_command_is_its_url_protocol() {
        assert!(is_url_command(r#""C:\x\goz-gui.exe" -url "%1""#));
        assert!(is_url_command(
            r#""D:\Tools\GOZ GUI\goz-gui.exe" -url "%1""#
        ));
        assert!(!is_url_command(r#"C:\x\other.exe "%1""#));
        assert!(!is_url_command(r#""C:\x\goz-gui.exe" "%1""#));
        assert!(url_command().unwrap().ends_with("\" -url \"%1\""));
    }

    #[test]
    fn commands_quote_the_exe() {
        let cmd = startup_command().unwrap();
        assert!(
            cmd.starts_with('"') && cmd.ends_with("\" -startup"),
            "{cmd}"
        );
        let verb = verb_command("%1").unwrap();
        assert!(verb.ends_with(" -path \"%1\""), "{verb}");
    }
}
