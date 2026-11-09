//! What goz-gui puts in the user's registry when asked in Options: starting
//! with Windows (a `Run` value) and the "Search goz..." item on folders in
//! Explorer, both under HKEY_CURRENT_USER and both removed again when
//! turned off.
//!
//! `GOZ_GUI_REG_ROOT` (tests) moves both under `HKCU\<root>` so a test never
//! touches the real keys.

use windows_sys::Win32::Foundation::ERROR_SUCCESS;
use windows_sys::Win32::System::Registry::{
    HKEY, HKEY_CURRENT_USER, KEY_WRITE, REG_OPTION_NON_VOLATILE, REG_SZ, RRF_RT_REG_SZ,
    RegCloseKey, RegCreateKeyExW, RegDeleteKeyValueW, RegDeleteKeyW, RegDeleteTreeW, RegGetValueW,
    RegSetValueExW,
};

use crate::wide::wz;

/// The test override of where the keys go.
pub const REG_ROOT_VAR: &str = "GOZ_GUI_REG_ROOT";

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
        assert!(!starts_with_windows() && !has_folder_item());
        assert!(set_starts_with_windows(true) && starts_with_windows());
        assert!(set_folder_item(true) && has_folder_item());
        let cmd = get(&format!(r"{}\command", verb_keys()[1].0), None).unwrap();
        assert!(cmd.ends_with(" -path \"%V\""), "{cmd}");
        assert!(set_starts_with_windows(false) && !starts_with_windows());
        assert!(set_folder_item(false) && !has_folder_item());
        let k = wz(&root);
        // SAFETY: deletes the throwaway key made above.
        unsafe { RegDeleteTreeW(HKEY_CURRENT_USER, k.as_ptr()) };
        unsafe { RegDeleteKeyW(HKEY_CURRENT_USER, k.as_ptr()) };
        // SAFETY: as above.
        unsafe { std::env::remove_var(REG_ROOT_VAR) };
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
