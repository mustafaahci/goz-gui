//! The registry: values read, written and deleted under the system's root
//! keys. A key opened here is closed before the call returns.

use windows_sys::Win32::Foundation::{ERROR_MORE_DATA, ERROR_SUCCESS};
use windows_sys::Win32::System::Registry::{
    HKEY, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, KEY_WRITE, REG_OPTION_NON_VOLATILE, REG_SZ,
    RRF_RT_REG_DWORD, RRF_RT_REG_SZ, RegCloseKey, RegCreateKeyExW, RegDeleteKeyValueW,
    RegDeleteKeyW, RegDeleteTreeW, RegGetValueW, RegSetValueExW,
};

use crate::win::wz;

/// A root key of the registry.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Hive {
    /// HKEY_CURRENT_USER.
    CurrentUser,
    /// HKEY_LOCAL_MACHINE.
    LocalMachine,
}

impl Hive {
    fn key(self) -> HKEY {
        match self {
            Hive::CurrentUser => HKEY_CURRENT_USER,
            Hive::LocalMachine => HKEY_LOCAL_MACHINE,
        }
    }
}

fn ptr(s: &Option<Vec<u16>>) -> *const u16 {
    s.as_ref().map_or(core::ptr::null(), |v| v.as_ptr())
}

/// The REG_DWORD value `value` of `key` under `hive`; `None` if it is not
/// there or is not a DWORD.
pub fn get_dword(hive: Hive, key: &str, value: &str) -> Option<u32> {
    let key = wz(key);
    let value = wz(value);
    let mut data: u32 = 0;
    let mut size = size_of::<u32>() as u32;
    // SAFETY: a predefined key; NUL-terminated names read during the call;
    // a DWORD out (RRF_RT_REG_DWORD: nothing else is written) whose size is
    // passed.
    let rc = unsafe {
        RegGetValueW(
            hive.key(),
            key.as_ptr(),
            value.as_ptr(),
            RRF_RT_REG_DWORD,
            core::ptr::null_mut(),
            (&raw mut data).cast(),
            &mut size,
        )
    };
    (rc == ERROR_SUCCESS).then_some(data)
}

/// The REG_SZ value `value` of `key` under `hive` (`None`: the key's
/// default value); `None` if there is none.
pub fn get_string(hive: Hive, key: &str, value: Option<&str>) -> Option<String> {
    let key = wz(key);
    let value = value.map(wz);
    let mut buf = vec![0u16; 2048];
    // Twice more at most, for a value that grows while it is read.
    for _ in 0..3 {
        let mut size = u32::try_from(buf.len() * 2).ok()?;
        // SAFETY: NUL-terminated names (or null for the default value)
        // and a buffer whose size in bytes is passed.
        let rc = unsafe {
            RegGetValueW(
                hive.key(),
                key.as_ptr(),
                ptr(&value),
                RRF_RT_REG_SZ,
                core::ptr::null_mut(),
                buf.as_mut_ptr().cast(),
                &mut size,
            )
        };
        if rc == ERROR_MORE_DATA {
            buf = vec![0u16; (size as usize).div_ceil(2) + 1];
            continue;
        }
        if rc != ERROR_SUCCESS {
            return None;
        }
        // The size counts the NUL.
        let len = (size as usize / 2).saturating_sub(1).min(buf.len());
        buf.truncate(len);
        return Some(String::from_utf16_lossy(&buf));
    }
    None
}

/// Sets the REG_SZ value `value` of `key` under `hive` (`None`: the
/// key's default) to `data`, making the key if need be; false if it
/// could not.
pub fn set_string(hive: Hive, key: &str, value: Option<&str>, data: &str) -> bool {
    let key = wz(key);
    let value = value.map(wz);
    let data = wz(data);
    let Ok(bytes) = u32::try_from(data.len() * 2) else {
        return false;
    };
    let mut h: HKEY = core::ptr::null_mut();
    // SAFETY: a NUL-terminated name, no class or security attributes,
    // an out-handle.
    let rc = unsafe {
        RegCreateKeyExW(
            hive.key(),
            key.as_ptr(),
            0,
            core::ptr::null(),
            REG_OPTION_NON_VOLATILE,
            KEY_WRITE,
            core::ptr::null(),
            &mut h,
            core::ptr::null_mut(),
        )
    };
    if rc != ERROR_SUCCESS {
        return false;
    }
    // SAFETY: the key just opened; a NUL-terminated name (or null) and
    // the data's `bytes`, its NUL included; then the key is closed.
    unsafe {
        let rc = RegSetValueExW(h, ptr(&value), 0, REG_SZ, data.as_ptr().cast(), bytes);
        RegCloseKey(h);
        rc == ERROR_SUCCESS
    }
}

/// Deletes value `value` of `key` under `hive`; false if it could not
/// (it is not there, say).
pub fn delete_value(hive: Hive, key: &str, value: &str) -> bool {
    let key = wz(key);
    let value = wz(value);
    // SAFETY: NUL-terminated names.
    unsafe { RegDeleteKeyValueW(hive.key(), key.as_ptr(), value.as_ptr()) == ERROR_SUCCESS }
}

/// Deletes `key` under `hive` with its subkeys and values; false if it
/// could not (it is not there, say). An empty `key` (the whole hive) is
/// refused.
pub fn delete_tree(hive: Hive, key: &str) -> bool {
    if key.is_empty() {
        return false;
    }
    let key = wz(key);
    // SAFETY: a NUL-terminated name.
    unsafe { RegDeleteTreeW(hive.key(), key.as_ptr()) == ERROR_SUCCESS }
}

/// Deletes `key` under `hive`, which has no subkeys left; false if it
/// could not.
pub fn delete_key(hive: Hive, key: &str) -> bool {
    let key = wz(key);
    // SAFETY: a NUL-terminated name.
    unsafe { RegDeleteKeyW(hive.key(), key.as_ptr()) == ERROR_SUCCESS }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_system_s_version_is_read() {
        let key = r"SOFTWARE\Microsoft\Windows NT\CurrentVersion";
        assert!(get_dword(Hive::LocalMachine, key, "CurrentMajorVersionNumber").is_some());
        let build = get_string(Hive::LocalMachine, key, Some("CurrentBuild"));
        assert!(build.is_some_and(|b| !b.is_empty() && !b.contains('\0')));
        // A DWORD is not a string.
        assert_eq!(
            get_string(Hive::LocalMachine, key, Some("CurrentMajorVersionNumber")),
            None
        );
        assert_eq!(
            get_dword(Hive::CurrentUser, r"Software\goz-gui-no-such-key", "x"),
            None
        );
    }

    #[test]
    fn a_value_round_trips_under_a_throwaway_key() {
        let root = format!(r"Software\goz-gui-reg-test-{}", std::process::id());
        let key = format!(r"{root}\a");
        assert_eq!(get_string(Hive::CurrentUser, &key, None), None);
        assert!(set_string(Hive::CurrentUser, &key, None, "default"));
        let long = "x".repeat(5000);
        assert!(set_string(Hive::CurrentUser, &key, Some("long"), &long));
        assert_eq!(
            get_string(Hive::CurrentUser, &key, None).as_deref(),
            Some("default")
        );
        assert_eq!(
            get_string(Hive::CurrentUser, &key, Some("long")),
            Some(long)
        );
        assert!(delete_value(Hive::CurrentUser, &key, "long"));
        assert!(!delete_value(Hive::CurrentUser, &key, "long"));
        assert!(!delete_tree(Hive::CurrentUser, ""));
        // A key with no subkeys goes by itself.
        assert!(delete_key(Hive::CurrentUser, &key));
        assert_eq!(get_string(Hive::CurrentUser, &key, None), None);
        // A tree goes whole.
        assert!(set_string(Hive::CurrentUser, &key, None, "again"));
        assert!(delete_tree(Hive::CurrentUser, &root));
        assert!(!delete_key(Hive::CurrentUser, &root), "gone already");
        assert!(!delete_tree(Hive::CurrentUser, &root));
    }
}
