//! Help > Search Syntax: what the search box accepts, in a resizable window
//! of its own whose text can be selected and copied.
//!
//! The list of search terms is goz's own help text (goz-core renders it from
//! the same tables its tests run through the parser), so it describes
//! exactly what gozd accepts; goz-gui adds what its menus do to a search.
//! A gozd that reads Everything 1.5's syntax (`client::boolean_syntax`)
//! gets its operators, modifiers and functions listed too (from the
//! query-language branch's own help); an older one, a line saying a newer
//! gozd adds them.

use std::cell::Cell;

use goz_core::escompat::{HelpScreen, help};
use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows_sys::Win32::Graphics::Gdi::{
    BeginPaint, CLEARTYPE_QUALITY, CLIP_DEFAULT_PRECIS, CreateFontW, DC_BRUSH, DEFAULT_CHARSET,
    DeleteObject, EndPaint, FF_MODERN, FIXED_PITCH, FW_NORMAL, GetStockObject, HFONT,
    InvalidateRect, OUT_DEFAULT_PRECIS, PAINTSTRUCT, SetBkColor, SetDCBrushColor, SetTextColor,
};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::Controls::{EM_SETMARGINS, EM_SETSEL};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    GetKeyState, SetFocus, VK_CONTROL, VK_ESCAPE,
};
use windows_sys::Win32::UI::Shell::{DefSubclassProc, SetWindowSubclass};
use windows_sys::Win32::UI::WindowsAndMessaging::*;

use crate::paint::fill;
use crate::places::{self, Which};
use crate::theme::{self, Palette};
use crate::wide::wz;
use crate::window::app_icon;

const CLASS: &str = "goz-gui-syntax";
const ID_TEXT: usize = 1;

thread_local! {
    /// The window while it is open; one at a time.
    static OPEN: Cell<HWND> = const { Cell::new(core::ptr::null_mut()) };
    static FONT: Cell<HFONT> = const { Cell::new(core::ptr::null_mut()) };
    static DARK: Cell<bool> = const { Cell::new(false) };
}

/// goz-gui's own rows: what to type or choose, and what it does, a line of
/// text each.
const NOT_SUPPORTED: &[(&str, &[&str])] = &[(
    "|  !",
    &[
        "OR and NOT are refused; quote them to search",
        "for them as text, as in \"a|b\"",
        "A newer gozd adds OR, NOT and < > grouping",
    ],
)];
/// What a gozd that reads Everything 1.5's syntax adds, from the
/// query-language branch's help (`escompat/help.rs`).
const NEW_SYNTAX: &[(&str, &[&str])] = &[
    ("a b", &["AND: names that contain both, in any order"]),
    (
        "a|b",
        &[
            "OR: either; it binds tighter than AND, so",
            "a b|c is a and (b or c); or: is the word",
        ],
    ),
    (
        "!a",
        &[
            "NOT: names without a, where a term starts",
            "(wow!.txt is a name); not: is the word",
        ],
    ),
    ("<a b>|c", &["A group: a and b, or c; !<a|b> is neither"]),
    ("\"a|b\"", &["In quotes | ! < > are text"]),
    (
        r"src\*.rs",
        &[
            "With a \\, * and ? stay inside one folder: the",
            ".rs files right in a folder named src",
        ],
    ),
    (
        r"src\**\*.rs",
        &["** crosses folders: any folder below src"],
    ),
    (
        "case:, nocase:",
        &["Match case, or not, for one term or <group>"],
    ),
    (
        "path:, nopath:",
        &["Match the full path, or the name alone"],
    ),
    (
        "ww:, prefix:, suffix:",
        &["Whole words; at a word's start or end"],
    ),
    (
        "startwith:, endwith:",
        &["The name starts or ends with the text"],
    ),
    ("whole:, exact:", &["The text is the whole name"]),
    (
        "file:, folder:",
        &["Only files, or only folders; files:, folders:"],
    ),
    (r"parent:C:\src", &[r"Entries right in C:\src (infolder:)"]),
    ("len:>40", &["Names longer than 40 characters"]),
    (
        "ext:mp?;doc*",
        &["Files whose extension matches one in the list"],
    ),
    (
        "size:<1kb|2kb>",
        &["A function's value may be a ; list or a <group>"],
    ),
    (
        "audio:, doc:, exe:",
        &[
            "Everything's filters by extension, as are",
            "image:, video: and zip:",
        ],
    ),
    (
        "dm:lastweek",
        &["The last 7 days; also dm:prevweek, dm:<30days"],
    ),
    (
        "?case:",
        &["A leading ? applies a modifier to every later term"],
    ),
];
const IN_GOZ_GUI: &[(&str, &[&str])] = &[
    ("Search > Match Case", &["Matches case, as case: does"]),
    (
        "Search > Match Path",
        &["Matches full paths, as matchpath: does"],
    ),
    (
        "Search > Audio, ...",
        &[
            "Only files with the filter's extensions, as",
            "ext: does; Folder keeps only folders. The",
            "filter and the search text must both match",
        ],
    ),
    (
        "Ctrl+1, 2, 3, 6",
        &[
            "Sort by name, path, size or date modified;",
            "the same key again reverses the order",
        ],
    ),
    (
        "filelist:a.txt;b.txt",
        &[
            "Items of those whole names, or full paths",
            "with a \\ (* and ? too). In an open file",
            "list; from gozd's index with a gozd that",
            "reads OR",
        ],
    ),
    (
        "filelist0:",
        &[
            "The names in file list slot 0 (to 9): File |",
            "Set File List Slot N (a key in Options >",
            "Keyboard) puts the selected results there",
        ],
    ),
    (
        "Ctrl+click filelist0:",
        &[
            "Edits the slot; also F2 with the caret in",
            "it, or the right-click menu's Edit File",
            "List Slot...",
        ],
    ),
    (
        "parent-filelist0:",
        &[
            "Items in a folder of slot 0 (by full path",
            "or name); ancestor-filelist0: anywhere",
            "under one",
        ],
    ),
    (
        "child-filelist0:",
        &[
            "Folders holding an item of slot 0;",
            "descendant-filelist0: folders above one;",
            "sibling-filelist0: items beside one",
        ],
    ),
    (
        "size-filelist0:",
        &[
            "Files of a slot item's size; name- and",
            "stem-filelist0: of a slot item's name or",
            "stem",
        ],
    ),
    (
        "exists-in-filelist0:$stem:.srt",
        &[
            "Items whose name, made so, is in slot 0",
            "($name:, $stem:, $extension:, $path:,",
            "$full-path:, $parent-name:); in an open",
            "file list",
        ],
    ),
    ("save-filelist0:", &["Puts the search's results in slot 0"]),
    (
        "a save-filelist1: new-search: b",
        &[
            "Runs a, puts its results in slot 1, then",
            "runs b as a new search (b may read",
            "filelist1:); not updated live (F5 is)",
        ],
    ),
    (
        "include-file-list:\"C:\\l.efu\"",
        &[
            "Items a list on the disk names (an EFU or",
            "CSV's Filename, or a name a line);",
            "Ctrl+click it to pick the file",
        ],
    ),
    (
        "open-filelist:\"C:\\l.efu\"",
        &[
            "Searches that file list instead, for this",
            "search; Ctrl+click it to pick the file",
        ],
    ),
    (
        "filelist-filename:l.efu",
        &[
            "Rows of an open file list of that name",
            "(nothing of gozd's index is in one)",
        ],
    ),
];

/// The window's text, lines ending in `\n`, as gozd's syntax is now.
pub fn text() -> String {
    text_for(crate::client::boolean_syntax())
}

/// The window's text; `boolean`: gozd reads OR, NOT and groups.
pub fn text_for(boolean: bool) -> String {
    let full = help(HelpScreen::Full);
    let terms = section(&full, "Search text").unwrap_or(&full);
    let column = text_column(terms);
    let mut out = String::from(
        "Search syntax\n\
         \n\
         goz-gui searches as you type. Separate words with spaces to find\n\
         names that contain all of them, in any order.\n\
         \n",
    );
    out.push_str(terms);
    let first = if boolean {
        ("Operators, modifiers and functions (this gozd)", NEW_SYNTAX)
    } else {
        ("Not supported", NOT_SUPPORTED)
    };
    for (title, rows) in [first, ("In goz-gui", IN_GOZ_GUI)] {
        out.push('\n');
        out.push_str(title);
        out.push('\n');
        for (usage, lines) in rows {
            // A usage wider than its column has a line of its own.
            let own_line = usage.chars().count() + 1 > column.saturating_sub(2);
            if own_line {
                out.push_str(&format!("  {usage}\n"));
            }
            for (i, line) in lines.iter().enumerate() {
                let usage = if i == 0 && !own_line { *usage } else { "" };
                out.push_str(&format!("  {usage:<width$}{line}\n", width = column - 2));
            }
        }
    }
    out
}

/// The lines of `help` from the line `title` to the next blank line.
fn section<'a>(help: &'a str, title: &str) -> Option<&'a str> {
    let start = help.find(&format!("\n{title}\n"))? + 1;
    let rest = &help[start..];
    let end = rest.find("\n\n").map_or(rest.len(), |i| i + 1);
    Some(&rest[..end])
}

/// Where the text of goz's table starts, so goz-gui's rows line up with it.
fn text_column(table: &str) -> usize {
    let column = table.lines().nth(1).and_then(|row| {
        let usage = row.len() - row.trim_start().len();
        let gap = usage + row[usage..].find("  ")?;
        Some(gap + row[gap..].len() - row[gap..].trim_start().len())
    });
    column.unwrap_or(26).max(24)
}

/// Shows the window, or brings it forward when it is open.
pub fn show(owner: HWND, dark: bool) {
    let open = OPEN.get();
    if !open.is_null() {
        // SAFETY: a window of this thread, still open.
        unsafe {
            ShowWindow(open, SW_RESTORE);
            SetForegroundWindow(open);
        }
        return;
    }
    DARK.set(dark);
    let class = wz(CLASS);
    // SAFETY: registers the class the first time only, and creates the
    // window and its text box on this thread. `existing` is a plain
    // out-struct.
    unsafe {
        let hinst = GetModuleHandleW(core::ptr::null());
        let mut existing: WNDCLASSEXW = core::mem::zeroed();
        existing.cbSize = size_of::<WNDCLASSEXW>() as u32;
        // The class stays registered once the window closes, and its icons
        // with it; loading new ones for a registration that would fail
        // leaks them, two icons per opening.
        if GetClassInfoExW(hinst, class.as_ptr(), &mut existing) == 0 {
            let wc = WNDCLASSEXW {
                cbSize: size_of::<WNDCLASSEXW>() as u32,
                lpfnWndProc: Some(wndproc),
                hInstance: hinst,
                hCursor: LoadCursorW(core::ptr::null_mut(), IDC_ARROW),
                hIcon: app_icon(GetSystemMetrics(SM_CXICON)),
                hIconSm: app_icon(GetSystemMetrics(SM_CXSMICON)),
                lpszClassName: class.as_ptr(),
                ..core::mem::zeroed()
            };
            if RegisterClassExW(&wc) == 0 {
                for icon in [wc.hIcon, wc.hIconSm] {
                    if !icon.is_null() {
                        DestroyIcon(icon);
                    }
                }
                return;
            }
        }
        // Centred on the main window, at its scale; then where it was last
        // (`places`).
        let dpi = crate::custom::window_dpi(owner) as i32;
        let (w, h) = (720 * dpi / 96, 600 * dpi / 96);
        let mut rc = RECT::default();
        GetWindowRect(owner, &mut rc);
        let x = rc.left + (rc.right - rc.left - w) / 2;
        let y = rc.top + (rc.bottom - rc.top - h) / 2;
        let hwnd = CreateWindowExW(
            0,
            class.as_ptr(),
            wz("Search Syntax - goz").as_ptr(),
            WS_OVERLAPPEDWINDOW | WS_CLIPCHILDREN,
            x,
            y,
            w,
            h,
            owner,
            core::ptr::null_mut(),
            hinst,
            core::ptr::null(),
        );
        if hwnd.is_null() {
            return;
        }
        let style = WS_CHILD
            | WS_VISIBLE
            | WS_VSCROLL
            | WS_HSCROLL
            | (ES_MULTILINE | ES_READONLY | ES_AUTOVSCROLL | ES_AUTOHSCROLL | ES_NOHIDESEL) as u32;
        let edit = CreateWindowExW(
            0,
            wz("EDIT").as_ptr(),
            core::ptr::null(),
            style,
            0,
            0,
            0,
            0,
            hwnd,
            ID_TEXT as HMENU,
            hinst,
            core::ptr::null(),
        );
        SetWindowSubclass(edit, Some(text_proc), 1, 0);
        OPEN.set(hwnd);
        apply_fonts(hwnd);
        let body = wz(&text().replace('\n', "\r\n"));
        SetWindowTextW(edit, body.as_ptr());
        SendMessageW(edit, EM_SETSEL, 0, 0);
        apply_theme(dark);
        places::restore(
            hwnd,
            Which::SearchSyntax,
            core::ptr::null_mut(),
            (w / 3, h / 4),
        );
        layout(hwnd);
        // Shown already when it opens maximized.
        if IsWindowVisible(hwnd) == 0 {
            ShowWindow(hwnd, SW_SHOWNORMAL);
        }
        SetFocus(edit);
    }
}

/// Shows the text as gozd's syntax is now, if the window is open.
pub fn refresh() {
    let hwnd = OPEN.get();
    if hwnd.is_null() {
        return;
    }
    let body = wz(&text().replace('\n', "\r\n"));
    // SAFETY: a NUL-terminated text for this thread's text box.
    unsafe { SetWindowTextW(text_box(hwnd), body.as_ptr()) };
}

/// Keeps where the window is, if it is open (the settings are about to be
/// written).
pub fn keep_place() {
    let hwnd = OPEN.get();
    if !hwnd.is_null() {
        places::keep(hwnd, Which::SearchSyntax, core::ptr::null_mut());
    }
}

/// Follows the main window's theme while the window is open.
pub fn apply_theme(dark: bool) {
    let hwnd = OPEN.get();
    if hwnd.is_null() {
        return;
    }
    DARK.set(dark);
    let edit = text_box(hwnd);
    theme::set_titlebar(hwnd, dark);
    theme::allow(edit, dark);
    theme::set_class(
        edit,
        Some(if dark {
            "DarkMode_Explorer"
        } else {
            "Explorer"
        }),
    );
    // SAFETY: repaints this thread's window and its text box.
    unsafe {
        InvalidateRect(hwnd, core::ptr::null(), 1);
        InvalidateRect(edit, core::ptr::null(), 1);
    }
}

fn text_box(hwnd: HWND) -> HWND {
    // SAFETY: looks up a child of this thread's window.
    unsafe { GetDlgItem(hwnd, ID_TEXT as i32) }
}

fn scale(hwnd: HWND, v: i32) -> i32 {
    v * crate::custom::window_dpi(hwnd) as i32 / 96
}

/// A monospaced font at the window's DPI, so goz's two columns line up.
fn apply_fonts(hwnd: HWND) {
    let edit = text_box(hwnd);
    // SAFETY: a font owned here; the old one is deleted once the text box
    // has the new one.
    unsafe {
        let font = CreateFontW(
            -scale(hwnd, 13),
            0,
            0,
            0,
            FW_NORMAL as i32,
            0,
            0,
            0,
            u32::from(DEFAULT_CHARSET),
            u32::from(OUT_DEFAULT_PRECIS),
            u32::from(CLIP_DEFAULT_PRECIS),
            u32::from(CLEARTYPE_QUALITY),
            u32::from(FIXED_PITCH | FF_MODERN),
            wz("Consolas").as_ptr(),
        );
        SendMessageW(edit, WM_SETFONT, font as usize, 1);
        let margin = scale(hwnd, 6) as u32;
        SendMessageW(
            edit,
            EM_SETMARGINS,
            (EC_LEFTMARGIN | EC_RIGHTMARGIN) as usize,
            (margin | margin << 16) as isize,
        );
        let old = FONT.replace(font);
        if !old.is_null() {
            DeleteObject(old);
        }
    }
}

fn layout(hwnd: HWND) {
    let mut rc = RECT::default();
    let inset = scale(hwnd, 6);
    // SAFETY: reads and moves this thread's windows.
    unsafe {
        GetClientRect(hwnd, &mut rc);
        MoveWindow(
            text_box(hwnd),
            inset,
            inset,
            (rc.right - 2 * inset).max(0),
            (rc.bottom - 2 * inset).max(0),
            1,
        );
    }
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    // SAFETY: each arm reads `w`/`l` as the message defines them.
    unsafe {
        match msg {
            WM_SIZE => {
                layout(hwnd);
                0
            }
            WM_PAINT => {
                let mut ps: PAINTSTRUCT = core::mem::zeroed();
                let hdc = BeginPaint(hwnd, &mut ps);
                fill(hdc, &ps.rcPaint, Palette::new(DARK.get()).bg);
                EndPaint(hwnd, &ps);
                0
            }
            WM_ERASEBKGND => 1,
            // A read-only text box asks for the static colours.
            WM_CTLCOLORSTATIC | WM_CTLCOLOREDIT => {
                let pal = Palette::new(DARK.get());
                SetTextColor(w as _, pal.fg);
                SetBkColor(w as _, pal.bg);
                SetDCBrushColor(w as _, pal.bg);
                GetStockObject(DC_BRUSH) as LRESULT
            }
            WM_SETFOCUS => {
                SetFocus(text_box(hwnd));
                0
            }
            WM_DPICHANGED => {
                let rc = &*(l as *const RECT);
                SetWindowPos(
                    hwnd,
                    core::ptr::null_mut(),
                    rc.left,
                    rc.top,
                    rc.right - rc.left,
                    rc.bottom - rc.top,
                    SWP_NOZORDER | SWP_NOACTIVATE,
                );
                apply_fonts(hwnd);
                layout(hwnd);
                0
            }
            WM_DESTROY => {
                places::keep(hwnd, Which::SearchSyntax, core::ptr::null_mut());
                OPEN.set(core::ptr::null_mut());
                let font = FONT.replace(core::ptr::null_mut());
                if !font.is_null() {
                    DeleteObject(font);
                }
                0
            }
            _ => DefWindowProcW(hwnd, msg, w, l),
        }
    }
}

/// The text box's subclass: Esc closes the window, and Ctrl+A selects all
/// the text, which a multi-line text box does not do by itself.
unsafe extern "system" fn text_proc(
    hwnd: HWND,
    msg: u32,
    w: WPARAM,
    l: LPARAM,
    _id: usize,
    _data: usize,
) -> LRESULT {
    // SAFETY: passes messages on to the text box; closes its parent.
    unsafe {
        match msg {
            WM_KEYDOWN if w == usize::from(VK_ESCAPE) => {
                PostMessageW(GetParent(hwnd), WM_CLOSE, 0, 0);
                return 0;
            }
            WM_KEYDOWN if w == usize::from(b'A') && GetKeyState(i32::from(VK_CONTROL)) < 0 => {
                SendMessageW(hwnd, EM_SETSEL, 0, -1);
                return 0;
            }
            // Esc and Ctrl+A as characters, handled above.
            WM_CHAR if w == 0x1b || w == 0x01 => return 0,
            _ => {}
        }
        DefSubclassProc(hwnd, msg, w, l)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_terms_are_gozs() {
        let full = help(HelpScreen::Full);
        let terms = section(&full, "Search text").expect("goz's help has the section");
        assert!(terms.starts_with("Search text\n"));
        assert!(terms.ends_with('\n') && !terms.ends_with("\n\n"));
        for term in [
            "ext:cs;txt",
            "size:>1mb",
            "dm:today",
            "folder:",
            "matchpath:",
        ] {
            assert!(terms.contains(term), "{term}");
        }
        for boolean in [false, true] {
            let text = text_for(boolean);
            assert!(text.contains(terms));
            // goz's help wraps at 79 columns; goz-gui's own lines do too.
            assert!(
                text.lines().all(|l| l.chars().count() <= 79),
                "{boolean}: {text}"
            );
        }
    }

    #[test]
    fn a_newer_gozd_gets_its_syntax_listed() {
        let old = text_for(false);
        assert!(old.contains("A newer gozd adds OR, NOT and < > grouping"));
        assert!(!old.contains("ww:"));
        let new = text_for(true);
        assert!(!new.contains("Not supported"));
        for usage in [
            "a|b",
            "!a",
            "<a b>|c",
            r"src\**\*.rs",
            "ww:",
            "parent:",
            "len:",
        ] {
            assert!(new.contains(usage), "{usage}");
        }
    }

    #[test]
    fn the_quoted_example_is_searchable() {
        assert!(goz_core::query::parse_query("\"a|b\"").is_ok());
        assert!(goz_core::query::parse_query("a|b").is_err());
    }
}
