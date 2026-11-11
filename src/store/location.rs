//! Where goz-gui keeps its files, and the dry run tests use.
//!
//! The data folder is, in order:
//! 1. the `GOZ_GUI_DATA_DIR` environment variable, when set (tests give
//!    each run a folder of its own this way);
//! 2. the exe's own folder, when a `goz-gui.ini` beside the exe says
//!    `app_data=0` (portable use, as Everything's `app_data`);
//! 3. `%APPDATA%\goz-gui`.
//!
//! With `GOZ_GUI_DRY_RUN=1`, the actions that would start something on the
//! desktop (open, open path, run as, properties, web pages, the service
//! commands) only append a line to `dry-run.log` in the data folder.
//!
//! The other test switches, each read once and only here:
//! - `GOZ_GUI_MENU_DUMP=1`: a context menu is built as for the screen but
//!   written to `menu-dump.log` in the data folder instead of shown
//!   (`crate::popup`); `GOZ_GUI_MENU_PICK=<label or verb>` then chooses
//!   that item, as a click would.
//! - `GOZ_GUI_FILEOP_SILENT=1`: file operations ask nothing and show no
//!   progress (`crate::shell::fileop_flags`, and the shell verbs invoked
//!   from the row menu).
//! - `GOZ_GUI_EXPORT_TO=<path>`: File > Export writes there without asking,
//!   in the format its extension names (.efu, .csv, .txt).
//! - `GOZ_GUI_PICK_FOLDER=<folder>`: Edit > Copy to Folder... and Move to
//!   Folder... use that folder without asking (`crate::filetools`).
//! - `GOZ_GUI_DESKTOP_DIR=<folder>`: Create Shortcut puts a shortcut it
//!   cannot make beside its item there instead of on the Desktop.

use std::ffi::OsString;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use super::ini::Ini;

pub const DATA_DIR_VAR: &str = "GOZ_GUI_DATA_DIR";
pub const DRY_RUN_VAR: &str = "GOZ_GUI_DRY_RUN";
/// The settings file's name, in the data folder (and beside the exe).
pub const SETTINGS_FILE: &str = "goz-gui.ini";
pub const DRY_RUN_LOG: &str = "dry-run.log";
pub const MENU_DUMP_VAR: &str = "GOZ_GUI_MENU_DUMP";
pub const MENU_PICK_VAR: &str = "GOZ_GUI_MENU_PICK";
pub const FILEOP_SILENT_VAR: &str = "GOZ_GUI_FILEOP_SILENT";
pub const MENU_DUMP_LOG: &str = "menu-dump.log";
pub const EXPORT_TO_VAR: &str = "GOZ_GUI_EXPORT_TO";
pub const PICK_FOLDER_VAR: &str = "GOZ_GUI_PICK_FOLDER";
pub const DESKTOP_DIR_VAR: &str = "GOZ_GUI_DESKTOP_DIR";
/// Wave 5: the File List Editor's Open... and Save As... use this file
/// without asking, and its "Save changes?" is answered No.
pub const EDITOR_FILE_VAR: &str = "GOZ_GUI_EDITOR_FILE";
/// W7: the File List Editor's and the Edit File List Slot dialog's Paste
/// take this text (names separated by `|`) instead of the clipboard's, so
/// a test never touches the user's clipboard.
pub const PASTE_TEXT_VAR: &str = "GOZ_GUI_PASTE_TEXT";

/// W7: the names a test pastes (`GOZ_GUI_PASTE_TEXT`, `|` between them), if
/// set: one a line, as the clipboard's text would hold them.
pub fn paste_text() -> Option<String> {
    std::env::var(PASTE_TEXT_VAR)
        .ok()
        .filter(|v| !v.is_empty())
        .map(|v| v.replace('|', "\r\n"))
}

/// Which rule chose the data folder.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    Variable,
    Portable,
    AppData,
}

/// Whether the text of a `goz-gui.ini` beside the exe asks for portable use.
fn portable(ini: &[u8]) -> bool {
    Ini::parse(ini).is_ok_and(|ini| ini.get("app_data").map(str::trim) == Some("0"))
}

/// The data folder by the rules above, from what they look at.
fn choose(
    variable: Option<OsString>,
    exe_dir: Option<&Path>,
    exe_ini: Option<&[u8]>,
    appdata: Option<PathBuf>,
) -> Option<(PathBuf, Source)> {
    if let Some(dir) = variable.filter(|v| !v.is_empty()) {
        let dir = PathBuf::from(dir);
        let dir = std::path::absolute(&dir).unwrap_or(dir);
        return Some((dir, Source::Variable));
    }
    if let (Some(dir), Some(ini)) = (exe_dir, exe_ini)
        && portable(ini)
    {
        return Some((dir.to_owned(), Source::Portable));
    }
    appdata.map(|a| (a.join("goz-gui"), Source::AppData))
}

static DATA_DIR: OnceLock<Option<(PathBuf, Source)>> = OnceLock::new();

/// The data folder, worked out once. `None` only when Windows will not say
/// where the user's AppData folder is. Nothing is created by asking.
pub fn data_dir() -> Option<&'static Path> {
    DATA_DIR
        .get_or_init(|| {
            let exe_dir = std::env::current_exe()
                .ok()
                .and_then(|exe| exe.parent().map(Path::to_owned));
            let exe_ini = exe_dir
                .as_ref()
                .and_then(|dir| std::fs::read(dir.join(SETTINGS_FILE)).ok());
            choose(
                std::env::var_os(DATA_DIR_VAR),
                exe_dir.as_deref(),
                exe_ini.as_deref(),
                roaming_app_data(),
            )
        })
        .as_ref()
        .map(|(dir, _)| dir.as_path())
}

/// Why the data folder is the one it is (Help > Troubleshooting
/// Information).
pub fn data_source() -> Option<Source> {
    data_dir()?;
    DATA_DIR.get()?.as_ref().map(|(_, source)| *source)
}

/// A file in the data folder.
pub fn data_file(name: &str) -> Option<PathBuf> {
    data_dir().map(|dir| dir.join(name))
}

/// The names of the files goz-gui keeps in its data folder.
fn data_file_names() -> [&'static str; 5] {
    [
        SETTINGS_FILE,
        crate::bookmarks::file::FILE_NAME,
        crate::history_file::FILE_NAME,
        crate::run_history_file::FILE_NAME,
        crate::session::FILE_NAME,
    ]
}

/// `path` made comparable: the folder's real path (it exists when a Save
/// dialog chose it) and the name, in lower case, as Windows compares them.
fn comparable(path: &Path) -> Option<(PathBuf, String)> {
    let name = path.file_name()?.to_string_lossy().to_lowercase();
    let dir = path.parent()?;
    let dir = std::fs::canonicalize(dir)
        .or_else(|_| std::path::absolute(dir))
        .ok()?;
    Some((PathBuf::from(dir.to_string_lossy().to_lowercase()), name))
}

/// Whether `path` is one of the files goz-gui keeps in `dir`, or the backup
/// of one.
fn is_kept_in(dir: &Path, path: &Path) -> bool {
    let Some(target) = comparable(path) else {
        return false;
    };
    data_file_names().iter().any(|name| {
        let file = dir.join(name);
        [super::file::backup_path(&file), file]
            .iter()
            .any(|kept| comparable(kept).as_ref() == Some(&target))
    })
}

/// Whether `path` is one of goz-gui's own files in its data folder (or a
/// backup of one): an export is not written there, which would replace the
/// list with what was exported, behind the merge's back.
pub fn is_data_file(path: &Path) -> bool {
    data_dir().is_some_and(|dir| is_kept_in(dir, path))
}

/// The user's roaming AppData folder: the APPDATA variable, else where the
/// shell says it is.
fn roaming_app_data() -> Option<PathBuf> {
    use std::os::windows::ffi::OsStringExt;
    use windows_sys::Win32::System::Com::CoTaskMemFree;
    use windows_sys::Win32::UI::Shell::{FOLDERID_RoamingAppData, SHGetKnownFolderPath};

    if let Some(dir) = std::env::var_os("APPDATA").filter(|v| !v.is_empty()) {
        return Some(PathBuf::from(dir));
    }
    let mut p: *mut u16 = core::ptr::null_mut();
    // SAFETY: the shell allocates a NUL-terminated path into `p`, read up to
    // its NUL and then freed with the allocator the shell used.
    unsafe {
        let hr = SHGetKnownFolderPath(&FOLDERID_RoamingAppData, 0, core::ptr::null_mut(), &mut p);
        let path = (hr >= 0 && !p.is_null()).then(|| {
            let len = (0..).take_while(|&i| *p.add(i) != 0).count();
            PathBuf::from(OsString::from_wide(core::slice::from_raw_parts(p, len)))
        });
        CoTaskMemFree(p.cast());
        path
    }
}

/// Whether the variable asks for a dry run: set, and not empty or `0`.
fn dry_run_value(value: Option<OsString>) -> bool {
    value.is_some_and(|v| {
        let v = v.to_string_lossy();
        let v = v.trim();
        !v.is_empty() && v != "0"
    })
}

/// Whether actions are only logged, not run (`GOZ_GUI_DRY_RUN`).
pub fn dry_run() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| dry_run_value(std::env::var_os(DRY_RUN_VAR)))
}

/// Whether context menus are written to `menu-dump.log` instead of shown
/// (`GOZ_GUI_MENU_DUMP`).
pub fn menu_dump() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| dry_run_value(std::env::var_os(MENU_DUMP_VAR)))
}

/// The item a dumped menu chooses (`GOZ_GUI_MENU_PICK`): a label or a shell
/// verb.
pub fn menu_pick() -> Option<&'static str> {
    static PICK: OnceLock<Option<String>> = OnceLock::new();
    PICK.get_or_init(|| {
        std::env::var(MENU_PICK_VAR)
            .ok()
            .filter(|p| !p.trim().is_empty())
    })
    .as_deref()
}

/// Whether file operations run without confirmation or progress UI
/// (`GOZ_GUI_FILEOP_SILENT`).
pub fn fileop_silent() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| dry_run_value(std::env::var_os(FILEOP_SILENT_VAR)))
}

/// Where File > Export writes without asking (`GOZ_GUI_EXPORT_TO`), if set.
pub fn export_to() -> Option<PathBuf> {
    std::env::var_os(EXPORT_TO_VAR)
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
}

/// The folder Copy and Move to Folder use without asking
/// (`GOZ_GUI_PICK_FOLDER`), if set.
pub fn pick_folder() -> Option<PathBuf> {
    std::env::var_os(PICK_FOLDER_VAR)
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
}

/// The file the File List Editor opens and saves as without asking
/// (`GOZ_GUI_EDITOR_FILE`), if set.
pub fn editor_file() -> Option<PathBuf> {
    std::env::var_os(EDITOR_FILE_VAR)
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
}

/// The folder that stands for the Desktop (`GOZ_GUI_DESKTOP_DIR`), if set.
pub fn desktop_dir() -> Option<PathBuf> {
    std::env::var_os(DESKTOP_DIR_VAR)
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
}

/// Appends `text` to the data folder's file `name` (a test's log); false
/// without a data folder or when it cannot be written.
pub fn append_data_file(name: &str, text: &str) -> bool {
    let Some(path) = data_file(name) else {
        return false;
    };
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .and_then(|mut log| log.write_all(text.as_bytes()))
        .is_ok()
}

/// One line of the dry-run log: the verb, then each item, tab-separated.
fn dry_run_line(verb: &str, items: &[String]) -> String {
    let mut line = verb.to_owned();
    for item in items {
        line.push('\t');
        line.push_str(&item.replace(['\t', '\r', '\n'], " "));
    }
    line.push_str("\r\n");
    line
}

/// In a dry run, notes that `verb` would have run on `items` and returns
/// true: the caller then does nothing. False when the action is to run.
pub fn dry_run_instead(verb: &str, items: &[String]) -> bool {
    if !dry_run() {
        return false;
    }
    append_data_file(DRY_RUN_LOG, &dry_run_line(verb, items));
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_variable_then_portable_then_appdata() {
        let exe = Path::new(r"C:\Tools\goz");
        let appdata = Some(PathBuf::from(r"C:\Users\u\AppData\Roaming"));
        let portable_ini = b"[goz-gui]\r\napp_data=0\r\n".as_slice();
        assert_eq!(
            choose(
                Some(r"D:\test\data".into()),
                Some(exe),
                Some(portable_ini),
                appdata.clone()
            ),
            Some((PathBuf::from(r"D:\test\data"), Source::Variable))
        );
        assert_eq!(
            choose(
                Some("".into()),
                Some(exe),
                Some(portable_ini),
                appdata.clone()
            ),
            Some((exe.to_owned(), Source::Portable)),
            "an empty variable counts as unset"
        );
        // A headerless file says it too; app_data=1 or no file does not.
        assert_eq!(
            choose(None, Some(exe), Some(b"app_data=0"), appdata.clone()).map(|c| c.1),
            Some(Source::Portable)
        );
        for ini in [Some(b"[goz-gui]\r\napp_data=1\r\n".as_slice()), None] {
            assert_eq!(
                choose(None, Some(exe), ini, appdata.clone()),
                Some((
                    PathBuf::from(r"C:\Users\u\AppData\Roaming\goz-gui"),
                    Source::AppData
                ))
            );
        }
        assert_eq!(choose(None, None, None, None), None);
    }

    #[test]
    fn goz_guis_own_files_are_told_from_others() {
        let dir = super::super::file::tests::scratch("data-files");
        for name in ["Bookmarks.csv", "Search History.backup.csv", "goz-gui.ini"] {
            assert!(is_kept_in(&dir, &dir.join(name)), "{name}");
        }
        // Any case, and through another spelling of the folder.
        assert!(is_kept_in(&dir, &dir.join("RUN HISTORY.CSV")));
        assert!(is_kept_in(&dir, &dir.join(".").join("Session.json")));
        // Other names, or another folder.
        assert!(!is_kept_in(&dir, &dir.join("Bookmarks export.csv")));
        assert!(!is_kept_in(&dir, &dir.join("Search History.corrupt-1.csv")));
        let other = dir.join("other");
        std::fs::create_dir(&other).expect("folder");
        assert!(!is_kept_in(&dir, &other.join("Bookmarks.csv")));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn dry_run_switch_and_log_lines() {
        assert!(dry_run_value(Some("1".into())));
        assert!(dry_run_value(Some("yes".into())));
        assert!(!dry_run_value(Some("0".into())));
        assert!(!dry_run_value(Some(" ".into())));
        assert!(!dry_run_value(None));
        assert_eq!(
            dry_run_line("open", &[r"C:\a b.txt".into(), "x\ty".into()]),
            "open\tC:\\a b.txt\tx y\r\n"
        );
        assert_eq!(dry_run_line("service-stop", &[]), "service-stop\r\n");
    }
}
