//! goz-gui on the taskbar (Everything 1.5's `taskbar-identity`): an
//! AppUserModelID of its own for each instance (so a pinned goz-gui and
//! its windows group as one button, and a test instance never touches the
//! real one's), the window's relaunch command (what a pin made from the
//! running window starts), and a jump list with the recent searches and
//! the bookmarks: clicking one starts `goz-gui -s ...`, which a running
//! goz-gui takes over (`crate::instance`).
//!
//! The jump list is made on a thread of its own (the shell's destination
//! list can take a while), from what the window hands it.

use std::collections::HashSet;
use std::sync::{Mutex, OnceLock};

use windows::Win32::Foundation::{E_FAIL, PROPERTYKEY};
use windows::core::GUID;

use crate::win::com::Apartment;
use crate::win::ext::b9::shell::{self, JumpList, ObjectCollection, PropertyStore, ShellLink};

/// The most searches, and bookmarks, the jump list shows.
pub const MAX_ITEMS: usize = 10;

/// The longest title an entry gets; a longer search is cut with "…".
const TITLE_MAX: usize = 80;

/// The shell's AppUserModel keys ({9F4C2855-9F79-4B39-A8D0-E1D42DE1D5F3}),
/// and the title of a jump list entry (System.Title).
const APP_USER_MODEL: GUID = GUID::from_u128(0x9f4c2855_9f79_4b39_a8d0_e1d42de1d5f3);
const PKEY_RELAUNCH_COMMAND: PROPERTYKEY = PROPERTYKEY {
    fmtid: APP_USER_MODEL,
    pid: 2,
};
const PKEY_RELAUNCH_ICON: PROPERTYKEY = PROPERTYKEY {
    fmtid: APP_USER_MODEL,
    pid: 3,
};
const PKEY_RELAUNCH_NAME: PROPERTYKEY = PROPERTYKEY {
    fmtid: APP_USER_MODEL,
    pid: 4,
};
const PKEY_ID: PROPERTYKEY = PROPERTYKEY {
    fmtid: APP_USER_MODEL,
    pid: 5,
};
const PKEY_TITLE: PROPERTYKEY = PROPERTYKEY {
    fmtid: GUID::from_u128(0xf29f85e0_4ff9_1068_ab91_08002b27b3d9),
    pid: 2,
};

static APP_ID: OnceLock<String> = OnceLock::new();

/// Entries the user took off the jump list (by their arguments): never put
/// back while goz-gui runs, as the shell asks.
static REMOVED: Mutex<Option<HashSet<String>>> = Mutex::new(None);

/// The AppUserModelID of the instance with `key` (`crate::instance::key`:
/// its data folder and `-instance` name).
pub fn app_id_for(key: &str) -> String {
    let id: String = format!("goz.{key}")
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-'))
        .take(128)
        .collect();
    id
}

/// Takes the instance's AppUserModelID for the process: before any window
/// is made.
pub fn set_app_id(key: &str) {
    let id = APP_ID.get_or_init(|| app_id_for(key));
    let _ = shell::set_app_user_model_id(id);
}

/// A command-line argument quoted as `CommandLineToArgvW` reads it back.
pub fn quote_arg(arg: &str) -> String {
    if !arg.is_empty() && !arg.contains([' ', '\t', '"']) {
        return arg.to_owned();
    }
    let mut out = String::from("\"");
    let mut slashes = 0;
    for c in arg.chars() {
        match c {
            '\\' => slashes += 1,
            '"' => {
                out.extend(std::iter::repeat_n('\\', slashes * 2 + 1));
                out.push('"');
                slashes = 0;
                continue;
            }
            _ => {}
        }
        if c != '\\' {
            out.extend(std::iter::repeat_n('\\', slashes));
            slashes = 0;
            out.push(c);
        }
    }
    out.extend(std::iter::repeat_n('\\', slashes * 2));
    out.push('"');
    out
}

fn args_line(args: &[String]) -> String {
    args.iter()
        .map(|a| quote_arg(a))
        .collect::<Vec<_>>()
        .join(" ")
}

/// An entry's title: the search on one line, cut to a sensible length.
pub fn title(text: &str) -> String {
    let one: String = text
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    if one.chars().count() <= TITLE_MAX {
        one
    } else {
        let mut cut: String = one.chars().take(TITLE_MAX - 1).collect();
        cut.push('…');
        cut
    }
}

/// The jump list's entries: `(category, title, arguments)`, recent
/// searches first.
pub fn entries(
    searches: &[String],
    marks: &[(String, Vec<String>)],
    instance: Option<&str>,
) -> Vec<(&'static str, String, String)> {
    let mut base: Vec<String> = Vec::new();
    if let Some(name) = instance {
        base.push("-instance".into());
        base.push(name.into());
    }
    let mut out = Vec::new();
    for s in searches.iter().filter(|s| !s.is_empty()).take(MAX_ITEMS) {
        let mut args = base.clone();
        args.push("-s".into());
        args.push(s.clone());
        out.push(("Recent Searches", title(s), args_line(&args)));
    }
    for (name, a) in marks.iter().take(MAX_ITEMS) {
        let mut args = base.clone();
        args.extend(a.iter().cloned());
        out.push(("Bookmarks", title(name), args_line(&args)));
    }
    out
}

fn exe_path() -> String {
    std::env::current_exe()
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// Gives the window the instance's AppUserModelID and its relaunch
/// command, name and icon (a pin made from the running window starts the
/// same goz-gui).
pub fn set_relaunch(hwnd: windows_sys::Win32::Foundation::HWND, instance: Option<&str>) {
    let Some(id) = APP_ID.get() else { return };
    let exe = exe_path();
    let mut command = quote_arg(&exe);
    if let Some(name) = instance {
        command.push_str(" -instance ");
        command.push_str(&quote_arg(name));
    }
    // The window is this thread's; the store is released as it drops.
    let Ok(store) = PropertyStore::for_window(hwnd) else {
        return;
    };
    let _ = store.set_string(&PKEY_ID, id);
    let _ = store.set_string(&PKEY_RELAUNCH_COMMAND, &command);
    let _ = store.set_string(&PKEY_RELAUNCH_NAME, "goz-gui");
    let _ = store.set_string(&PKEY_RELAUNCH_ICON, &format!("{exe},0"));
    let _ = store.commit();
}

/// The data folder's `jump-list.log` when the test hook
/// (`GOZ_GUI_JUMP_LIST_LOG`) asks for one.
fn log_path() -> Option<std::path::PathBuf> {
    std::env::var_os("GOZ_GUI_JUMP_LIST_LOG")?;
    crate::store::location::data_file("jump-list.log")
}

/// Whether this goz-gui may make a jump list: a test run (a data folder of
/// its own, `GOZ_GUI_DATA_DIR`) leaves the shell's lists alone unless the
/// test hook asks for one (and then deletes it as it ends).
fn allowed() -> bool {
    std::env::var_os("GOZ_GUI_DATA_DIR").is_none() || log_path().is_some()
}

/// Makes the jump list again, on a thread of its own.
pub fn update(searches: Vec<String>, marks: Vec<(String, Vec<String>)>, instance: Option<String>) {
    if !allowed() {
        return;
    }
    let Some(id) = APP_ID.get().cloned() else {
        return;
    };
    let items = entries(&searches, &marks, instance.as_deref());
    let spawned = std::thread::Builder::new()
        .name("goz-gui jump list".into())
        .spawn(move || {
            let result = commit(&id, &items);
            if let Some(path) = log_path() {
                let mut text = String::new();
                for (category, title, args) in &items {
                    text.push_str(&format!("{category}\t{title}\t{args}\r\n"));
                }
                text.push_str(&match result {
                    Ok(skipped) => format!("committed\t{skipped} removed by the user\r\n"),
                    Err(e) => format!("failed\t{e}\r\n"),
                });
                use std::io::Write;
                if let Ok(mut f) = std::fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(path)
                {
                    let _ = f.write_all(text.as_bytes());
                }
            }
        });
    drop(spawned);
}

/// Builds and commits the list; the number of entries left out because
/// the user removed them.
fn commit(id: &str, items: &[(&'static str, String, String)]) -> windows::core::Result<usize> {
    // This thread's own apartment, ended before it returns; every COM
    // object is released as it drops, before the apartment.
    let com = Apartment::sta().ok_or_else(|| windows::core::Error::from(E_FAIL))?;
    let r = (|| {
        let list = JumpList::new()?;
        list.set_app_id(id)?;
        let removed = list.begin_list()?;
        let mut gone = REMOVED.lock().unwrap_or_else(|p| p.into_inner());
        let gone = gone.get_or_insert_with(HashSet::new);
        for i in 0..removed.count().unwrap_or(0) {
            if let Ok(link) = removed.link_at(i)
                && let Ok(args) = link.arguments()
            {
                gone.insert(args);
            }
        }
        let exe = exe_path();
        let mut skipped = 0;
        for category in ["Recent Searches", "Bookmarks"] {
            let collection = ObjectCollection::new()?;
            let mut any = false;
            for (_, title, args) in items.iter().filter(|(c, _, _)| *c == category) {
                if gone.contains(args) {
                    skipped += 1;
                    continue;
                }
                let link = ShellLink::new()?;
                link.set_path(&exe)?;
                link.set_arguments(args)?;
                link.set_icon_location(&exe, 0)?;
                link.set_description(title)?;
                let store = link.property_store()?;
                store.set_string(&PKEY_TITLE, title)?;
                store.commit()?;
                collection.add(&link)?;
                any = true;
            }
            if any {
                list.append_category(category, &collection)?;
            }
        }
        list.commit_list()?;
        Ok(skipped)
    })();
    drop(com);
    r
}

/// The test hook's clean-up: a test instance's jump list goes as it ends.
pub fn delete_for_test() {
    if log_path().is_none() {
        return;
    }
    let Some(id) = APP_ID.get() else { return };
    // As in `commit`.
    let Some(com) = Apartment::sta() else {
        return;
    };
    if let Ok(list) = JumpList::new() {
        let _ = list.delete_list(id);
    }
    drop(com);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arguments_are_quoted_as_windows_reads_them() {
        assert_eq!(quote_arg("abc"), "abc");
        assert_eq!(quote_arg(""), "\"\"");
        assert_eq!(quote_arg("a b"), "\"a b\"");
        assert_eq!(quote_arg(r#"say "hi""#), r#""say \"hi\"""#);
        assert_eq!(quote_arg(r"C:\x y\"), r#""C:\x y\\""#);
        assert_eq!(quote_arg(r#"a\"b"#), r#""a\\\"b""#);
        assert_eq!(quote_arg(r"C:\dir"), r"C:\dir");
    }

    #[test]
    fn entries_are_searches_then_bookmarks() {
        let searches = vec!["report pdf".to_owned(), String::new(), "x".to_owned()];
        let marks = vec![(
            "Music".to_owned(),
            vec![
                "-s".into(),
                "*.mp3".into(),
                "-filter".into(),
                "Audio".into(),
            ],
        )];
        let e = entries(&searches, &marks, None);
        assert_eq!(e.len(), 3);
        assert_eq!(
            e[0],
            (
                "Recent Searches",
                "report pdf".into(),
                "-s \"report pdf\"".into()
            )
        );
        assert_eq!(e[1].2, "-s x");
        assert_eq!(
            e[2],
            ("Bookmarks", "Music".into(), "-s *.mp3 -filter Audio".into())
        );
        let named = entries(&searches[..1], &[], Some("work"));
        assert_eq!(named[0].2, "-instance work -s \"report pdf\"");
        let many: Vec<String> = (0..30).map(|i| i.to_string()).collect();
        assert_eq!(entries(&many, &[], None).len(), MAX_ITEMS);
    }

    #[test]
    fn titles_are_one_line_and_not_too_long() {
        assert_eq!(title("a\r\nb"), "a  b");
        let long = "x".repeat(200);
        assert_eq!(title(&long).chars().count(), TITLE_MAX);
        assert!(title(&long).ends_with('…'));
        assert_eq!(app_id_for("goz-gui-00ff"), "goz.goz-gui-00ff");
        assert!(app_id_for(&"y".repeat(300)).len() <= 128);
    }
}
