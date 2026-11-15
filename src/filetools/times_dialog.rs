//! Edit > Advanced > Set File Time...: Everything's dialog. For each of the
//! date created, modified and accessed, a group of choices: No change, the
//! file's own other date, Now, or Custom (an ISO 8601 date typed in its
//! box, `YYYY-MM-DDThh:mm:ss`, the earliest value for what is left out).
//! OK checks the Custom dates first and says which one it cannot read.
//! The choices and dates are kept for next time (Everything's keys).

use std::cell::{Cell, RefCell};

use windows_sys::Win32::Foundation::HWND;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    BN_CLICKED, BS_AUTORADIOBUTTON, BS_DEFPUSHBUTTON, BS_GROUPBOX, ES_AUTOHSCROLL, IDCANCEL, IDOK,
    MB_ICONWARNING, MB_OK, WM_COMMAND, WM_INITDIALOG, WS_EX_CLIENTEDGE, WS_GROUP, WS_TABSTOP,
};

use super::times::{Local, Set, parse_iso};
use super::ui::{checked, enable, item, set_check};
use crate::dialog::{Class, Look, Template, apply_theme, center_on_owner, common_msg};
use crate::history::filetime_now;
use crate::msgbox;
use crate::win::{self, Msg, dlg, edit};

const TITLE: &str = "Set File Time";

/// Each group: its caption and its five choices, Everything's labels and
/// control ids (the radio buttons from `first`, the box `first + 5`, the
/// group box `group`).
struct Group {
    caption: &'static str,
    labels: [&'static str; 5],
    first: u16,
    group: u16,
}

const GROUPS: [Group; 3] = [
    Group {
        caption: "Set date created",
        labels: [
            "No &change",
            "&Date modified",
            "Date &accessed",
            "&Now",
            "C&ustom:",
        ],
        first: 1000,
        group: 1018,
    },
    Group {
        caption: "Set date modified",
        labels: [
            "No c&hange",
            "Date c&reated",
            "Date acce&ssed",
            "N&ow",
            "Cus&tom:",
        ],
        first: 1006,
        group: 1019,
    },
    Group {
        caption: "Set date accessed",
        labels: [
            "No chan&ge",
            "Date cr&eated",
            "Date mod&ified",
            "No&w",
            "Custo&m:",
        ],
        first: 1012,
        group: 1020,
    },
];

/// The choice numbers (Everything's `set_file_time_*_type`).
const NO_CHANGE: u8 = 0;
const NOW: u8 = 3;
const CUSTOM: u8 = 4;

/// What OK chose.
#[derive(Debug)]
pub struct Chosen {
    /// The choice for date created, modified and accessed.
    pub kinds: [u8; 3],
    /// The Custom boxes' text.
    pub custom: [String; 3],
    /// What each time becomes.
    pub set: [Set; 3],
}

/// What choice `kind` of group `g` (0 created, 1 modified, 2 accessed)
/// sets; `now` for Now, `custom` the Custom date.
pub fn set_of(g: usize, kind: u8, now: u64, custom: Option<u64>) -> Option<Set> {
    // The two other dates, in the dialog's order.
    let others = match g {
        0 => [Set::Modified, Set::Accessed],
        1 => [Set::Created, Set::Accessed],
        _ => [Set::Created, Set::Modified],
    };
    Some(match kind {
        NO_CHANGE => Set::Keep,
        1 | 2 => others[usize::from(kind) - 1],
        NOW => Set::At(now),
        _ => Set::At(custom?),
    })
}

/// What OK says when a Custom date cannot be read: Everything's help text.
fn bad_date(caption: &str, text: &str) -> String {
    let example = Local::of_filetime(filetime_now()).map_or_else(String::new, Local::iso);
    format!(
        "{caption}: \"{text}\" is not a date.\n\nType in an ISO 8601 date:\nYYYY-MM-DDThh:mm:ss\nThe earliest time is used for omitted information.\nFor example: {example}"
    )
}

struct Dlg {
    look: Cell<Look>,
    kinds: [u8; 3],
    custom: [String; 3],
}

/// Shows the dialog over `owner` with the choices `kinds` and Custom dates
/// `custom` of last time; what OK chose (`None`: Cancel, or every time
/// left as it is).
pub fn run(owner: HWND, kinds: [u8; 3], custom: [String; 3], look: Look) -> Option<Chosen> {
    let state = Dlg {
        look: Cell::new(look),
        kinds,
        custom,
    };
    let mut t = Template::new(TITLE, 0, 177, 262);
    for (g, group) in GROUPS.iter().enumerate() {
        let top = 7 + 78 * g as i16;
        t.control(
            Class::Button,
            group.caption,
            group.group,
            BS_GROUPBOX as u32,
            0,
            (7, top, 163, 74),
        );
        for (i, label) in group.labels.iter().enumerate() {
            let first = i == 0;
            let style = BS_AUTORADIOBUTTON as u32 | if first { WS_GROUP | WS_TABSTOP } else { 0 };
            let width = if i == 4 { 38 } else { 148 };
            t.control(
                Class::Button,
                label,
                group.first + i as u16,
                style,
                0,
                (14, top + 12 + 11 * i as i16, width, 10),
            );
        }
        t.control(
            Class::Edit,
            "",
            group.first + 5,
            WS_TABSTOP | WS_GROUP | ES_AUTOHSCROLL as u32,
            WS_EX_CLIENTEDGE,
            (54, top + 54, 108, 12),
        );
    }
    t.control(
        Class::Button,
        "OK",
        IDOK as u16,
        WS_TABSTOP | WS_GROUP | BS_DEFPUSHBUTTON as u32,
        0,
        (66, 242, 50, 14),
    )
    .control(
        Class::Button,
        "Cancel",
        IDCANCEL as u16,
        WS_TABSTOP,
        0,
        (120, 242, 50, 14),
    );
    // Where OK puts its answer.
    let out: RefCell<Option<Chosen>> = RefCell::new(None);
    t.show(owner, &|m| dialog_proc(&state, &out, m));
    out.into_inner()
}

impl Dlg {
    fn init(&self, dlg: HWND) {
        let cue = "YYYY-MM-DDThh:mm:ss";
        for (g, group) in GROUPS.iter().enumerate() {
            let kind = self.kinds[g].min(CUSTOM);
            set_check(dlg, group.first + u16::from(kind), true);
            let field = item(dlg, group.first + 5);
            win::set_text(field, &self.custom[g]);
            edit::set_cue_banner(field, cue, true);
        }
        self.update(dlg);
        apply_theme(dlg, self.look.get().dark);
        center_on_owner(dlg);
    }

    /// The choice ticked in group `g`.
    fn kind(dlg: HWND, g: usize) -> u8 {
        (0..5u8)
            .find(|&k| checked(dlg, GROUPS[g].first + u16::from(k)))
            .unwrap_or(NO_CHANGE)
    }

    /// A Custom box can be typed in only while Custom is chosen.
    fn update(&self, dlg: HWND) {
        for (g, group) in GROUPS.iter().enumerate() {
            enable(dlg, group.first + 5, Dlg::kind(dlg, g) == CUSTOM);
        }
    }

    /// OK: the choices, once every Custom date reads; else says which does
    /// not and puts the keyboard in its box.
    fn ok(&self, dlg: HWND) -> Option<Chosen> {
        let now = filetime_now();
        let kinds: [u8; 3] = core::array::from_fn(|g| Dlg::kind(dlg, g));
        let custom: [String; 3] =
            core::array::from_fn(|g| win::text(item(dlg, GROUPS[g].first + 5)).trim().to_owned());
        let mut set = [Set::Keep; 3];
        for g in 0..3 {
            let date = (kinds[g] == CUSTOM)
                .then(|| parse_iso(&custom[g]).and_then(Local::to_filetime))
                .flatten();
            match set_of(g, kinds[g], now, date) {
                Some(s) => set[g] = s,
                None => {
                    msgbox::show(
                        dlg,
                        &bad_date(GROUPS[g].caption, &custom[g]),
                        TITLE,
                        MB_OK | MB_ICONWARNING,
                    );
                    let field = item(dlg, GROUPS[g].first + 5);
                    win::set_focus(field);
                    edit::select_all(field);
                    return None;
                }
            }
        }
        Some(Chosen { kinds, custom, set })
    }
}

/// The dialog's procedure: `s` its state, `out` where OK puts its answer.
fn dialog_proc(s: &Dlg, out: &RefCell<Option<Chosen>>, m: &Msg) -> isize {
    let dlg = m.hwnd;
    if let Some(answer) = common_msg(m, &s.look) {
        return answer;
    }
    match m.id {
        WM_INITDIALOG => {
            s.init(dlg);
            // The dialog manager puts the keyboard on the first choice.
            1
        }
        WM_COMMAND => {
            let (id, code) = (win::loword(m.w), u32::from(win::hiword(m.w)));
            match id {
                x if x == IDOK as u16 => {
                    if let Some(chosen) = s.ok(dlg) {
                        // Nothing changes: nothing to do.
                        let any = chosen.set.iter().any(|&t| t != Set::Keep);
                        if any {
                            *out.borrow_mut() = Some(chosen);
                        }
                        dlg::end(dlg, 1);
                    }
                }
                x if x == IDCANCEL as u16 => dlg::end(dlg, 0),
                _ if code == BN_CLICKED => s.update(dlg),
                _ => {}
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
    fn each_choice_sets_its_time() {
        // Date created: from date modified or accessed; modified: from
        // created or accessed; accessed: from created or modified.
        assert_eq!(set_of(0, 1, 9, None), Some(Set::Modified));
        assert_eq!(set_of(0, 2, 9, None), Some(Set::Accessed));
        assert_eq!(set_of(1, 1, 9, None), Some(Set::Created));
        assert_eq!(set_of(1, 2, 9, None), Some(Set::Accessed));
        assert_eq!(set_of(2, 1, 9, None), Some(Set::Created));
        assert_eq!(set_of(2, 2, 9, None), Some(Set::Modified));
        assert_eq!(set_of(1, NO_CHANGE, 9, None), Some(Set::Keep));
        assert_eq!(set_of(2, NOW, 9, None), Some(Set::At(9)));
        assert_eq!(set_of(0, CUSTOM, 9, Some(5)), Some(Set::At(5)));
        // A Custom date that does not read.
        assert_eq!(set_of(0, CUSTOM, 9, None), None);
    }

    #[test]
    fn labels_and_ids_are_everythings() {
        let ids: Vec<u16> = GROUPS.iter().map(|g| g.first).collect();
        assert_eq!(ids, [1000, 1006, 1012]);
        assert_eq!(GROUPS[2].labels[2], "Date mod&ified");
        // Each group's access keys differ from every other's.
        let keys: Vec<char> = GROUPS
            .iter()
            .flat_map(|g| g.labels)
            .map(|l| {
                let at = l.find('&').unwrap();
                l[at + 1..].chars().next().unwrap().to_ascii_lowercase()
            })
            .collect();
        let mut unique = keys.clone();
        unique.sort_unstable();
        unique.dedup();
        assert_eq!(unique.len(), keys.len(), "{keys:?}");
        assert!(bad_date("Set date created", "x").contains("YYYY-MM-DDThh:mm:ss"));
    }
}
