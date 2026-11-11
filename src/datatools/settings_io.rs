//! Tools > Import and Export Settings...: Everything's wizard, its first
//! page (Export settings, Import settings, Reset settings), then a folder.
//! Export copies goz-gui's files from its data folder to the folder chosen;
//! Import copies them back from one (each file it replaces kept as
//! `name.before-import.ext`: goz-gui's own `name.backup.ext` is the file
//! before its last write, soon the imported one) and goz-gui restarts to
//! use them; Reset puts the settings and the session aside (as
//! `name.before-reset.ext`) and restarts with Everything's defaults,
//! keeping the histories, filters and bookmarks.
//!
//! Unlike Everything's, the search history, run history, filters,
//! bookmarks and session go with the settings: they are files of the same
//! folder, and a copy of goz-gui moved to another computer wants them all.

use std::cell::{Cell, RefCell};
use std::io;
use std::path::{Path, PathBuf};

use windows_sys::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    BS_AUTORADIOBUTTON, BS_DEFPUSHBUTTON, EndDialog, GWLP_USERDATA, IDCANCEL, IDOK,
    SetWindowLongPtrW, WM_COMMAND, WM_INITDIALOG, WS_GROUP, WS_TABSTOP,
};

use crate::dialog::{Class, Look, Template, apply_theme, center_on_owner, common, state};
use crate::filetools::ui::{checked, set_check};
use crate::store::location::SETTINGS_FILE;

/// The files that go, in the data folder.
pub const FILES: [&str; 6] = [
    SETTINGS_FILE,
    crate::filters::FILE_NAME,
    crate::bookmarks::file::FILE_NAME,
    crate::history_file::FILE_NAME,
    crate::run_history_file::FILE_NAME,
    crate::session::FILE_NAME,
];

/// What Reset puts aside: the settings and the session.
const RESET_FILES: [&str; 2] = [SETTINGS_FILE, crate::session::FILE_NAME];

/// The wizard's choice.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    Export,
    Import,
    Reset,
}

impl Action {
    /// The action `GOZ_GUI_SETTINGS_ACTION` names (tests): the wizard is
    /// skipped and its questions answered yes.
    pub fn from_test() -> Option<Action> {
        let v = std::env::var("GOZ_GUI_SETTINGS_ACTION").ok()?;
        match v.trim().to_ascii_lowercase().as_str() {
            "export" => Some(Action::Export),
            "import" => Some(Action::Import),
            "reset" => Some(Action::Reset),
            _ => None,
        }
    }
}

/// Which of [`FILES`] are in `folder`.
pub fn present_in(folder: &Path) -> Vec<&'static str> {
    FILES
        .iter()
        .copied()
        .filter(|f| folder.join(f).is_file())
        .collect()
}

/// `path` with `.<tag>` before its extension: `goz-gui.ini` and
/// `before-import` make `goz-gui.before-import.ini`.
pub fn aside(path: &Path, tag: &str) -> PathBuf {
    let mut name = path.file_stem().unwrap_or_default().to_owned();
    name.push(".");
    name.push(tag);
    if let Some(ext) = path.extension() {
        name.push(".");
        name.push(ext);
    }
    path.with_file_name(name)
}

/// Whether two folders are the same one.
fn same_folder(a: &Path, b: &Path) -> bool {
    let canon = |p: &Path| {
        std::fs::canonicalize(p)
            .or_else(|_| std::path::absolute(p))
            .map(|p| p.to_string_lossy().to_lowercase())
            .ok()
    };
    canon(a).is_some_and(|x| Some(x) == canon(b))
}

/// Copies `from` over `to` by way of a temporary file beside `to`, so `to`
/// is never left half written.
fn copy_whole(from: &Path, to: &Path) -> io::Result<()> {
    let mut tmp = to.as_os_str().to_owned();
    tmp.push(".tmp");
    let tmp = PathBuf::from(tmp);
    std::fs::copy(from, &tmp)?;
    std::fs::rename(&tmp, to).inspect_err(|_| {
        let _ = std::fs::remove_file(&tmp);
    })
}

/// Copies goz-gui's files from the data folder `data` to `to`; the names
/// copied.
pub fn export(data: &Path, to: &Path) -> io::Result<Vec<&'static str>> {
    if same_folder(data, to) {
        return Err(io::Error::other("that is goz-gui's own data folder"));
    }
    let names = present_in(data);
    for name in &names {
        copy_whole(&data.join(name), &to.join(name))?;
    }
    Ok(names)
}

/// Copies goz-gui's files found in `from` into the data folder `data`,
/// each file replaced kept as its backup first; the names copied.
pub fn import(from: &Path, data: &Path) -> io::Result<Vec<&'static str>> {
    if same_folder(data, from) {
        return Err(io::Error::other("that is goz-gui's own data folder"));
    }
    let names = present_in(from);
    if names.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            "it holds none of goz-gui's files",
        ));
    }
    std::fs::create_dir_all(data)?;
    for name in &names {
        let target = data.join(name);
        if target.is_file() {
            std::fs::copy(&target, aside(&target, "before-import"))?;
        }
        copy_whole(&from.join(name), &target)?;
    }
    Ok(names)
}

/// Puts the settings and the session aside as their backups; the names
/// put aside.
pub fn reset(data: &Path) -> io::Result<Vec<&'static str>> {
    let mut done = Vec::new();
    for name in RESET_FILES {
        let path = data.join(name);
        if path.is_file() {
            let backup = aside(&path, "before-reset");
            let _ = std::fs::remove_file(&backup);
            std::fs::rename(&path, &backup)?;
            done.push(name);
        }
    }
    Ok(done)
}

const STATIC: u16 = 0xffff;
// Everything's ids.
const ID_EXPORT: u16 = 1051;
const ID_IMPORT: u16 = 1052;
const ID_RESET: u16 = 1053;
const OK: u16 = IDOK as u16;
const CANCEL: u16 = IDCANCEL as u16;

struct Wizard {
    look: Cell<Look>,
    chosen: RefCell<Option<Action>>,
}

/// The wizard's first page: what to do; `None` for Cancel.
pub fn ask(owner: HWND, look: Look) -> Option<Action> {
    let state = Wizard {
        look: Cell::new(look),
        chosen: RefCell::new(None),
    };
    let radio = WS_TABSTOP | BS_AUTORADIOBUTTON as u32;
    let mut t = Template::new("Import and Export Settings", 0, 260, 132);
    t.control(
        Class::Static,
        "Select Settings Action",
        STATIC,
        0,
        0,
        (7, 7, 246, 9),
    )
    .control(
        Class::Button,
        "&Export settings",
        ID_EXPORT,
        radio | WS_GROUP,
        0,
        (14, 24, 232, 10),
    )
    .control(
        Class::Button,
        "&Import settings",
        ID_IMPORT,
        radio,
        0,
        (14, 38, 232, 10),
    )
    .control(
        Class::Button,
        "&Reset settings",
        ID_RESET,
        radio,
        0,
        (14, 52, 232, 10),
    )
    .control(
        Class::Static,
        "The search history, run history, filters, bookmarks and session \
         go with the settings. Reset keeps them and resets the settings \
         and the session.",
        STATIC,
        0,
        0,
        (14, 70, 232, 26),
    )
    .control(
        Class::Static,
        concat!("goz-gui ", env!("CARGO_PKG_VERSION"), " Settings"),
        STATIC,
        WS_GROUP,
        0,
        (7, 113, 120, 9),
    )
    .control(
        Class::Button,
        "&Next",
        OK,
        WS_TABSTOP | BS_DEFPUSHBUTTON as u32,
        0,
        (149, 111, 50, 14),
    )
    .control(
        Class::Button,
        "Cancel",
        CANCEL,
        WS_TABSTOP,
        0,
        (203, 111, 50, 14),
    );
    t.run(owner, Some(proc), (&raw const state) as LPARAM);
    state.chosen.into_inner()
}

unsafe extern "system" fn proc(dlg: HWND, msg: u32, w: WPARAM, l: LPARAM) -> isize {
    if msg == WM_INITDIALOG {
        // SAFETY: `l` is the `Wizard` that `ask` keeps alive until the
        // dialog has ended.
        unsafe { SetWindowLongPtrW(dlg, GWLP_USERDATA, l) };
    }
    // SAFETY: GWLP_USERDATA is the `Wizard` from `ask`, or null.
    let Some(s) = (unsafe { state::<Wizard>(dlg) }) else {
        return 0;
    };
    if let Some(answer) = common(dlg, msg, w, l, &s.look) {
        return answer;
    }
    match msg {
        WM_INITDIALOG => {
            set_check(dlg, ID_EXPORT, true);
            apply_theme(dlg, s.look.get().dark);
            center_on_owner(dlg);
            1
        }
        WM_COMMAND => {
            match (w & 0xffff) as u16 {
                OK => {
                    let action = if checked(dlg, ID_IMPORT) {
                        Action::Import
                    } else if checked(dlg, ID_RESET) {
                        Action::Reset
                    } else {
                        Action::Export
                    };
                    *s.chosen.borrow_mut() = Some(action);
                    // SAFETY: ends this modal dialog.
                    unsafe { EndDialog(dlg, 1) };
                }
                // SAFETY: ends this modal dialog.
                CANCEL => unsafe {
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

    fn temp(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("goz-gui-settings-io-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn export_import_and_reset_round_trip() {
        let data = temp("data");
        let out = temp("out");
        std::fs::write(data.join(SETTINGS_FILE), "[goz-gui]\r\ntheme=2\r\n").unwrap();
        std::fs::write(data.join(crate::filters::FILE_NAME), "Name,Search\r\n").unwrap();
        std::fs::write(data.join(crate::session::FILE_NAME), "{}").unwrap();
        // Only goz-gui's own files go.
        std::fs::write(data.join("dry-run.log"), "x").unwrap();
        let mut names = export(&data, &out).unwrap();
        names.sort_unstable();
        assert_eq!(names, ["Filters.csv", "Session.json", "goz-gui.ini"]);
        assert!(!out.join("dry-run.log").exists());
        assert!(export(&data, &data).is_err(), "not over its own files");
        // Imported back over changed files: those are kept as backups.
        std::fs::write(data.join(SETTINGS_FILE), "[goz-gui]\r\ntheme=1\r\n").unwrap();
        let names = import(&out, &data).unwrap();
        assert_eq!(names.len(), 3);
        assert!(
            std::fs::read_to_string(data.join(SETTINGS_FILE))
                .unwrap()
                .contains("theme=2")
        );
        assert!(
            std::fs::read_to_string(data.join("goz-gui.before-import.ini"))
                .unwrap()
                .contains("theme=1")
        );
        assert!(import(&temp("empty"), &data).is_err());
        // Reset: the settings and session aside, the filters kept.
        let gone = reset(&data).unwrap();
        assert_eq!(gone, [SETTINGS_FILE, crate::session::FILE_NAME]);
        assert!(!data.join(SETTINGS_FILE).exists());
        assert!(data.join(crate::filters::FILE_NAME).exists());
        assert!(data.join("Session.before-reset.json").exists());
        assert_eq!(
            aside(Path::new(r"C:\d\Search History.csv"), "before-import"),
            Path::new(r"C:\d\Search History.before-import.csv")
        );
        for d in [data, out] {
            let _ = std::fs::remove_dir_all(d);
        }
    }
}
