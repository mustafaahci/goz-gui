//! Wave 9: gozd's index settings, as Options > Indexes shows them, under
//! Everything's key names and with Everything's defaults.
//!
//! The settings are gozd's, not goz-gui's: one index serves every user of
//! the computer, so gozd keeps them, and only an elevated administrator
//! may change them (wave 9 spec, protocol item 4: `GetConfig`,
//! `SetConfig`). The page reads and writes them through an
//! [`IndexSettings`], on a thread of its own (a call may ask gozd).
//!
//! [`source`] gives the gozd-backed [`GozdConfig`] when gozd lists the
//! "config" feature (W9 stream B2): `GetConfig` reads the settings, and a
//! change goes to gozd as `SetConfig` when goz-gui runs as an elevated
//! administrator, else through one elevated `goz.exe --config set KEY=0|1
//! ...` (one UAC prompt), and is read back. A gozd without the feature
//! (v0.6.0) gets [`NoConfig`], which says why the settings cannot be shown,
//! and the page shows them as unavailable rather than pretend to read or
//! save them.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use goz_core::proto::{ConfigChange, feature};

/// The settings, as Everything's keys name them, in the page's order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Key {
    IndexRecentChanges,
    FastSizeSort,
    IndexFolderSize,
    IndexDateCreated,
    FastDateCreatedSort,
    FastDateModifiedSort,
    IndexDateAccessed,
    FastDateAccessedSort,
    IndexAttributes,
    FastAttributesSort,
    FastExtensionSort,
}

impl Key {
    pub const ALL: [Key; 11] = [
        Key::IndexRecentChanges,
        Key::FastSizeSort,
        Key::IndexFolderSize,
        Key::IndexDateCreated,
        Key::FastDateCreatedSort,
        Key::FastDateModifiedSort,
        Key::IndexDateAccessed,
        Key::FastDateAccessedSort,
        Key::IndexAttributes,
        Key::FastAttributesSort,
        Key::FastExtensionSort,
    ];

    /// Everything's name for it (its INI key, gozd's `Config` field).
    pub fn name(self) -> &'static str {
        match self {
            Key::IndexRecentChanges => "index_recent_changes",
            Key::FastSizeSort => "fast_size_sort",
            Key::IndexFolderSize => "index_folder_size",
            Key::IndexDateCreated => "index_date_created",
            Key::FastDateCreatedSort => "fast_date_created_sort",
            Key::FastDateModifiedSort => "fast_date_modified_sort",
            Key::IndexDateAccessed => "index_date_accessed",
            Key::FastDateAccessedSort => "fast_date_accessed_sort",
            Key::IndexAttributes => "index_attributes",
            Key::FastAttributesSort => "fast_attributes_sort",
            Key::FastExtensionSort => "fast_extension_sort",
        }
    }

    /// Everything's default (`generated_full_ini_1424b.ini`).
    pub fn default_value(self) -> bool {
        matches!(
            self,
            Key::FastSizeSort | Key::IndexFolderSize | Key::FastDateModifiedSort
        )
    }

    /// The setting a fast sort needs on (Everything greys "Fast date created
    /// sort" while "Index date created" is off).
    pub fn needs(self) -> Option<Key> {
        match self {
            Key::FastDateCreatedSort => Some(Key::IndexDateCreated),
            Key::FastDateAccessedSort => Some(Key::IndexDateAccessed),
            Key::FastAttributesSort => Some(Key::IndexAttributes),
            _ => None,
        }
    }

    fn at(self) -> usize {
        Key::ALL.iter().position(|&k| k == self).unwrap_or(0)
    }

    pub fn named(name: &str) -> Option<Key> {
        Key::ALL.into_iter().find(|k| k.name() == name)
    }
}

/// gozd's index settings.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct IndexConfig([bool; 11]);

impl Default for IndexConfig {
    /// Everything's defaults.
    fn default() -> IndexConfig {
        IndexConfig(Key::ALL.map(Key::default_value))
    }
}

impl IndexConfig {
    pub fn get(&self, key: Key) -> bool {
        self.0[key.at()]
    }

    pub fn set(&mut self, key: Key, on: bool) {
        self.0[key.at()] = on;
    }

    /// From gozd's answer, by Everything's names; a name missing keeps
    /// Everything's default, an unknown one is left out.
    pub fn from_pairs<'a>(pairs: impl IntoIterator<Item = (&'a str, bool)>) -> IndexConfig {
        let mut c = IndexConfig::default();
        for (name, on) in pairs {
            if let Some(k) = Key::named(name) {
                c.set(k, on);
            }
        }
        c
    }

    /// What changes `self` into `new`.
    pub fn change_to(&self, new: &IndexConfig) -> IndexChange {
        IndexChange(Key::ALL.map(|k| (self.get(k) != new.get(k)).then_some(new.get(k))))
    }
}

/// A change to gozd's settings: each setting to set, or `None` to keep.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct IndexChange([Option<bool>; 11]);

impl IndexChange {
    pub fn is_empty(&self) -> bool {
        self.0.iter().all(Option::is_none)
    }

    /// The settings it sets, by Everything's names (gozd's `SetConfig`).
    pub fn pairs(&self) -> Vec<(&'static str, bool)> {
        Key::ALL
            .into_iter()
            .filter_map(|k| Some((k.name(), self.0[k.at()]?)))
            .collect()
    }

    /// gozd's `SetConfig` for it.
    pub fn to_gozd(self) -> ConfigChange {
        let mut c = ConfigChange::default();
        for (name, on) in self.pairs() {
            if let Some(slot) = c.slot(name) {
                *slot = Some(on);
            }
        }
        c
    }

    /// The arguments of `goz.exe` that make it: `--config set KEY=0|1 ...`.
    pub fn goz_args(&self) -> String {
        let mut out = String::from("--config set");
        for (name, on) in self.pairs() {
            out.push_str(&format!(" {name}={}", u8::from(on)));
        }
        out
    }
}

/// Where Options > Indexes reads and writes gozd's settings. Both calls run
/// on a thread of their own, never the window's.
pub trait IndexSettings: Send + Sync {
    /// The settings now, or why they cannot be had.
    fn get(&self) -> Result<IndexConfig, String>;

    /// Changes what `change` sets: the status bar's words when done, or
    /// why it was not (gozd's "needs administrator").
    fn set(&self, change: &IndexChange) -> Result<String, String>;
}

/// A gozd that cannot report or change its settings: gozd 0.6 and older,
/// which do not list the "config" feature.
pub struct NoConfig;

impl NoConfig {
    fn reason() -> String {
        match crate::client::server() {
            Some((server, _)) => format!(
                "{server} cannot report or change its index settings: that takes a gozd \
                 that lists the \"config\" feature (goz 0.7)."
            ),
            None => {
                "gozd has not answered goz-gui yet, so its index settings cannot be shown.".into()
            }
        }
    }
}

impl IndexSettings for NoConfig {
    fn get(&self) -> Result<IndexConfig, String> {
        Err(NoConfig::reason())
    }

    fn set(&self, _change: &IndexChange) -> Result<String, String> {
        Err(NoConfig::reason())
    }
}

/// gozd's settings over `GetConfig` and `SetConfig` (a gozd that lists
/// "config").
pub struct GozdConfig;

impl IndexSettings for GozdConfig {
    fn get(&self) -> Result<IndexConfig, String> {
        let c = crate::client::get_config()?;
        Ok(IndexConfig::from_pairs(c.entries()))
    }

    fn set(&self, change: &IndexChange) -> Result<String, String> {
        if change.is_empty() {
            return Ok("gozd's index settings are as they were".into());
        }
        if goz_winfs::current_process_is_elevated_admin().unwrap_or(false) {
            crate::client::set_config(change.to_gozd())?;
        } else {
            // gozd takes them from an elevated administrator only: one
            // elevated goz.exe asks the user once, and gozd is read back.
            let goz = goz_exe().ok_or(
                "goz.exe was not found beside gozd.exe or on the PATH, and goz-gui is not \
                 running as administrator",
            )?;
            run_elevated(&goz, &change.goz_args())?;
        }
        let now = self.get()?;
        let missed: Vec<&str> = change
            .pairs()
            .into_iter()
            .filter(|&(name, on)| Key::named(name).is_some_and(|k| now.get(k) != on))
            .map(|(name, _)| name)
            .collect();
        if !missed.is_empty() {
            return Err(format!("gozd kept {} as it was", missed.join(", ")));
        }
        Ok("gozd's index settings were changed".into())
    }
}

/// The settings' source for the gozd running: gozd's own when it lists
/// "config", else [`NoConfig`].
pub fn source() -> Arc<dyn IndexSettings> {
    if crate::client::has_feature(feature::CONFIG) {
        Arc::new(GozdConfig)
    } else {
        Arc::new(NoConfig)
    }
}

/// goz.exe: `GOZ_GUI_GOZ_EXE` (tests), else beside the goz service's
/// gozd.exe (the build gozd runs), else on the PATH.
fn goz_exe() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("GOZ_GUI_GOZ_EXE") {
        return Some(PathBuf::from(p));
    }
    let beside = service_binary()
        .and_then(|bin| Some(bin.parent()?.join("goz.exe")))
        .filter(|p| p.is_file());
    beside.or_else(|| {
        std::env::var_os("PATH").and_then(|path| {
            std::env::split_paths(&path)
                .map(|dir| dir.join("goz.exe"))
                .find(|p| p.is_file())
        })
    })
}

/// The goz service's program, from its configuration.
fn service_binary() -> Option<PathBuf> {
    use windows_sys::Win32::System::Services::{
        CloseServiceHandle, OpenSCManagerW, OpenServiceW, QUERY_SERVICE_CONFIGW,
        QueryServiceConfigW, SC_MANAGER_CONNECT, SERVICE_QUERY_CONFIG,
    };
    let name = crate::wide::wz(goz_core::proto::SERVICE_NAME);
    // SAFETY: handles opened here are closed here; the buffer is sized by
    // the first call and u64-aligned for QUERY_SERVICE_CONFIGW, whose
    // strings point into it.
    unsafe {
        let scm = OpenSCManagerW(core::ptr::null(), core::ptr::null(), SC_MANAGER_CONNECT);
        if scm.is_null() {
            return None;
        }
        let svc = OpenServiceW(scm, name.as_ptr(), SERVICE_QUERY_CONFIG);
        let mut out = None;
        if !svc.is_null() {
            let mut needed = 0u32;
            QueryServiceConfigW(svc, core::ptr::null_mut(), 0, &mut needed);
            let mut buf = vec![0u64; (needed as usize).div_ceil(8).max(1)];
            let cfg = buf.as_mut_ptr().cast::<QUERY_SERVICE_CONFIGW>();
            if QueryServiceConfigW(svc, cfg, (buf.len() * 8) as u32, &mut needed) != 0 {
                let p = (*cfg).lpBinaryPathName;
                if !p.is_null() {
                    let len = (0..).take_while(|&i| *p.add(i) != 0).count();
                    let text = String::from_utf16_lossy(core::slice::from_raw_parts(p, len));
                    out = Some(PathBuf::from(program_of(&text)));
                }
            }
            CloseServiceHandle(svc);
        }
        CloseServiceHandle(scm);
        out
    }
}

/// The program of a service's command line: quoted, or up to `.exe`.
fn program_of(command: &str) -> &str {
    let c = command.trim();
    if let Some(rest) = c.strip_prefix('"') {
        return rest.split('"').next().unwrap_or(rest);
    }
    match c.to_ascii_lowercase().find(".exe") {
        Some(i) => &c[..i + 4],
        None => c.split(' ').next().unwrap_or(c),
    }
}

/// Runs `exe args` as administrator (Windows asks the user), hidden, and
/// waits for it: an error when the prompt was refused or it failed.
fn run_elevated(exe: &Path, args: &str) -> Result<(), String> {
    use windows_sys::Win32::Foundation::{CloseHandle, ERROR_CANCELLED, GetLastError};
    use windows_sys::Win32::System::Com::{
        COINIT_APARTMENTTHREADED, COINIT_DISABLE_OLE1DDE, CoInitializeEx, CoUninitialize,
    };
    use windows_sys::Win32::System::Threading::{GetExitCodeProcess, WaitForSingleObject};
    use windows_sys::Win32::UI::Shell::{
        SEE_MASK_NOASYNC, SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW, ShellExecuteExW,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::SW_HIDE;
    let shown = format!("{} {args}", exe.display());
    if crate::store::location::dry_run_instead("config-set", &[shown]) {
        return Err("a dry run: goz.exe was not run".into());
    }
    let verb = crate::wide::wz("runas");
    let file = crate::wide::wz(&exe.to_string_lossy());
    let params = crate::wide::wz(args);
    let mut info = SHELLEXECUTEINFOW {
        cbSize: size_of::<SHELLEXECUTEINFOW>() as u32,
        fMask: SEE_MASK_NOASYNC | SEE_MASK_NOCLOSEPROCESS,
        lpVerb: verb.as_ptr(),
        lpFile: file.as_ptr(),
        lpParameters: params.as_ptr(),
        nShow: SW_HIDE,
        ..Default::default()
    };
    // SAFETY: COM for this thread (the page's own), undone below; every
    // string is NUL-terminated and outlives the call; the process handle
    // is closed once waited on.
    unsafe {
        let com = CoInitializeEx(
            core::ptr::null(),
            (COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE) as u32,
        );
        let ran = ShellExecuteExW(&mut info);
        let err = GetLastError();
        if com >= 0 {
            CoUninitialize();
        }
        if ran == 0 {
            return Err(if err == ERROR_CANCELLED {
                "the administrator prompt was cancelled".into()
            } else {
                format!(
                    "{} could not be run as administrator: {}",
                    exe.display(),
                    std::io::Error::from_raw_os_error(err as i32)
                )
            });
        }
        if info.hProcess.is_null() {
            return Ok(());
        }
        let done = WaitForSingleObject(info.hProcess, 60_000) == 0;
        let mut code = 1u32;
        GetExitCodeProcess(info.hProcess, &mut code);
        CloseHandle(info.hProcess);
        match (done, code) {
            (false, _) => Err("goz.exe --config set did not finish in a minute".into()),
            (true, 0) => Ok(()),
            (true, c) => Err(format!("goz.exe --config set ended with code {c}")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn everythings_names_and_defaults() {
        let d = IndexConfig::default();
        let on: Vec<&str> = Key::ALL
            .into_iter()
            .filter(|&k| d.get(k))
            .map(Key::name)
            .collect();
        assert_eq!(
            on,
            [
                "fast_size_sort",
                "index_folder_size",
                "fast_date_modified_sort"
            ]
        );
        for k in Key::ALL {
            assert_eq!(Key::named(k.name()), Some(k));
        }
        assert_eq!(Key::FastAttributesSort.needs(), Some(Key::IndexAttributes));
        assert_eq!(Key::FastSizeSort.needs(), None);
    }

    #[test]
    fn a_change_sets_only_what_differs() {
        let old = IndexConfig::from_pairs([("index_attributes", true), ("bogus", true)]);
        assert!(old.get(Key::IndexAttributes) && old.get(Key::IndexFolderSize));
        let mut new = old;
        assert!(old.change_to(&new).is_empty());
        new.set(Key::IndexAttributes, false);
        new.set(Key::FastExtensionSort, true);
        let change = old.change_to(&new);
        assert_eq!(
            change.pairs(),
            [("index_attributes", false), ("fast_extension_sort", true)]
        );
    }

    #[test]
    fn an_old_gozd_says_why() {
        // A gozd that lists no "config" (v0.6.0): the page says why.
        let s: Arc<dyn IndexSettings> = Arc::new(NoConfig);
        assert!(s.get().is_err());
        assert!(s.set(&IndexChange::default()).is_err());
    }

    #[test]
    fn the_source_follows_the_config_feature() {
        // No gozd greeted in the test: no "config", the old gozd's source.
        if !crate::client::has_feature("config") {
            assert!(source().get().is_err());
        }
    }

    #[test]
    fn a_change_goes_to_gozd_and_to_goz_exe_in_everythings_names() {
        let old = IndexConfig::default();
        let mut new = old;
        new.set(Key::IndexAttributes, true);
        new.set(Key::FastSizeSort, false);
        let change = old.change_to(&new);
        let c = change.to_gozd();
        assert_eq!(c.index_attributes, Some(true));
        assert_eq!(c.fast_size_sort, Some(false));
        assert_eq!(c.index_date_created, None);
        assert_eq!(
            change.goz_args(),
            "--config set fast_size_sort=0 index_attributes=1"
        );
        // gozd's answer reads back by the same names.
        let back =
            IndexConfig::from_pairs(goz_core::proto::IndexConfig::default().with(&c).entries());
        assert_eq!(back, new);
    }

    #[test]
    fn a_services_program_is_read_from_its_command_line() {
        assert_eq!(
            program_of(r#""C:\Program Files\goz\gozd.exe" run"#),
            r"C:\Program Files\goz\gozd.exe"
        );
        assert_eq!(program_of(r"C:\goz\gozd.exe run"), r"C:\goz\gozd.exe");
        assert_eq!(program_of(r"C:\goz\gozd"), r"C:\goz\gozd");
    }
}
