//! The file tools' settings, under Everything's keys: `max_undo`; Advanced
//! Rename's `rename_match_case`, `rename_match_diacritics`,
//! `rename_regex`, `rename_ignore_extension` and its format histories
//! (`rename_old_format_history`, `rename_new_format_history`); Set File
//! Time's choices (`set_file_time_date_created_type`, ..._modified_type,
//! ..._accessed_type: 0 No change, 1 and 2 the other two dates, 3 Now,
//! 4 Custom) and Custom dates (`set_file_time_date_created`, ...); the
//! Undo History's sort (`undo_history_sort`, `_ascending`); the folder
//! Copy and Move to Folder last went to (`rename_last_browse_path`, the
//! key Everything's copy and move dialogs keep their Browse folder in); and
//! the renamer's presets (`rename_presets`, as Everything 1.5 writes it: a
//! JSON array on one line).

use crate::csv::{self, Field};
use crate::json::{self, Json};

/// Everything's `max_undo`.
pub const MAX_UNDO: usize = 256;
/// The most kept of a format history.
pub const HISTORY: usize = 20;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ToolSettings {
    pub max_undo: usize,
    pub rename: super::rename::Options,
    /// The formats used, newest first.
    pub old_history: Vec<String>,
    pub new_history: Vec<String>,
    /// Set File Time's choice for date created, modified and accessed.
    pub time_kinds: [u8; 3],
    /// And its Custom dates, as typed.
    pub time_custom: [String; 3],
    /// The Undo History's sort column (0 Date ... 5 New Path), and
    /// direction.
    pub undo_sort: u8,
    pub undo_ascending: bool,
    /// Where Copy and Move to Folder last went.
    pub last_folder: String,
    /// Edit > Advanced > Advanced Copy to Folder...'s and Advanced Move to
    /// Folder...'s switches and formats (wave 4: Everything's
    /// `advanced_copy_to_*` and `advanced_move_to_*`).
    pub copy_to: Formats,
    pub move_to: Formats,
    /// The named Old/New formats the renamer's Presets button keeps, shared
    /// by Advanced Rename and Advanced Copy/Move (`rename_presets`).
    pub presets: Vec<Preset>,
}

/// A renamer's switches and its formats used, newest first.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Formats {
    pub options: super::rename::Options,
    pub old_history: Vec<String>,
    pub new_history: Vec<String>,
}

impl Formats {
    fn from_keys<'a>(prefix: &str, get: &impl Fn(&str) -> Option<&'a str>) -> Formats {
        let flag = |k: &str| {
            get(&format!("{prefix}_{k}"))
                .and_then(|v| v.trim().parse::<i64>().ok())
                .is_some_and(|n| n != 0)
        };
        Formats {
            options: super::rename::Options {
                match_case: flag("match_case"),
                match_diacritics: flag("match_diacritics"),
                regex: flag("regex"),
                ignore_extension: flag("ignore_extension"),
            },
            old_history: history_of(get(&format!("{prefix}_old_format_history"))),
            new_history: history_of(get(&format!("{prefix}_new_format_history"))),
        }
    }

    fn to_keys(&self, prefix: &str, out: &mut Vec<(String, String)>) {
        let bit = |b: bool| u8::from(b).to_string();
        let o = self.options;
        for (k, v) in [
            ("match_case", bit(o.match_case)),
            ("match_diacritics", bit(o.match_diacritics)),
            ("regex", bit(o.regex)),
            ("ignore_extension", bit(o.ignore_extension)),
            ("old_format_history", history_value(&self.old_history)),
            ("new_format_history", history_value(&self.new_history)),
        ] {
            out.push((format!("{prefix}_{k}"), v));
        }
    }
}

/// A named Old format and New format, and the renamer's switches with
/// them (Everything's New Preset dialog keeps those too).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Preset {
    pub name: String,
    pub old_format: String,
    pub new_format: String,
    pub options: super::rename::Options,
}

/// The most presets kept.
pub const PRESETS: usize = 100;

/// The presets as `rename_presets` holds them. Everything 1.5 writes a JSON
/// array on one line (seen live with 1.5.0.1424b):
///
/// `[{"name":"Dates","old_format":"%1-%2","new_format":"%2 %1","case":0,`
/// `"diacritics":0,"regex":0,"ignore_extension":0},...]`
///
/// Before, goz-gui wrote one CSV record of name, old format and new format,
/// preset after preset: still read (with the switches off), and written
/// again as Everything's.
fn presets_of(value: Option<&str>) -> Vec<Preset> {
    let Some(value) = value.map(str::trim).filter(|v| !v.is_empty()) else {
        return Vec::new();
    };
    if value.starts_with('[') {
        return everythings_presets(value);
    }
    let fields: Vec<String> = csv::parse(value)
        .into_iter()
        .next()
        .unwrap_or_default()
        .into_iter()
        .map(|f| f.text)
        .collect();
    fields
        .as_chunks::<3>()
        .0
        .iter()
        .filter(|c| !c[0].is_empty())
        .map(|c| Preset {
            name: c[0].clone(),
            old_format: c[1].clone(),
            new_format: c[2].clone(),
            options: super::rename::Options::default(),
        })
        .take(PRESETS)
        .collect()
}

/// Everything's JSON presets: the objects with a name; a missing format is
/// empty and a missing switch off. Text that is not such an array gives
/// none (the key is left for Everything's to write again).
fn everythings_presets(value: &str) -> Vec<Preset> {
    let Ok(parsed) = json::parse(value.as_bytes()) else {
        return Vec::new();
    };
    let text = |p: &Json, key: &str| p.get(key).and_then(Json::as_text).unwrap_or_default();
    let flag = |p: &Json, key: &str| p.get(key).and_then(Json::as_flag).unwrap_or(false);
    parsed
        .as_array()
        .unwrap_or_default()
        .iter()
        .map(|p| Preset {
            name: text(p, "name"),
            old_format: text(p, "old_format"),
            new_format: text(p, "new_format"),
            options: super::rename::Options {
                match_case: flag(p, "case"),
                match_diacritics: flag(p, "diacritics"),
                regex: flag(p, "regex"),
                ignore_extension: flag(p, "ignore_extension"),
            },
        })
        .filter(|p| !p.name.is_empty())
        .take(PRESETS)
        .collect()
}

/// `rename_presets` as Everything writes it; empty for none.
fn presets_value(list: &[Preset]) -> String {
    if list.is_empty() {
        return String::new();
    }
    let items = list
        .iter()
        .map(|p| {
            let o = p.options;
            Json::Object(vec![
                ("name".to_owned(), Json::str(&p.name)),
                ("old_format".to_owned(), Json::str(&p.old_format)),
                ("new_format".to_owned(), Json::str(&p.new_format)),
                ("case".to_owned(), Json::bit(o.match_case)),
                ("diacritics".to_owned(), Json::bit(o.match_diacritics)),
                ("regex".to_owned(), Json::bit(o.regex)),
                ("ignore_extension".to_owned(), Json::bit(o.ignore_extension)),
            ])
        })
        .collect();
    json::to_line(&Json::Array(items))
}

impl Default for ToolSettings {
    fn default() -> ToolSettings {
        ToolSettings {
            max_undo: MAX_UNDO,
            rename: super::rename::Options::default(),
            old_history: Vec::new(),
            new_history: Vec::new(),
            time_kinds: [0; 3],
            time_custom: Default::default(),
            // Everything's: by date, newest first.
            undo_sort: 0,
            undo_ascending: false,
            last_folder: String::new(),
            copy_to: Formats::default(),
            move_to: Formats::default(),
            presets: Vec::new(),
        }
    }
}

const TIME_KEYS: [&str; 3] = [
    "set_file_time_date_created",
    "set_file_time_date_modified",
    "set_file_time_date_accessed",
];

/// A history as its key holds it: one CSV record, newest first.
fn history_of(value: Option<&str>) -> Vec<String> {
    let Some(value) = value.filter(|v| !v.is_empty()) else {
        return Vec::new();
    };
    csv::parse(value)
        .into_iter()
        .next()
        .unwrap_or_default()
        .into_iter()
        .map(|f| f.text)
        .filter(|t| !t.is_empty())
        .take(HISTORY)
        .collect()
}

fn history_value(list: &[String]) -> String {
    if list.is_empty() {
        return String::new();
    }
    let fields: Vec<Field> = list.iter().map(|t| Field::quoted(t.clone())).collect();
    let mut out = String::new();
    csv::write_record(&fields, &mut out);
    out.trim_end_matches(['\r', '\n']).to_owned()
}

/// Puts `text` first in `list` (once), keeping at most [`HISTORY`].
pub fn remember(list: &mut Vec<String>, text: &str) {
    if text.is_empty() {
        return;
    }
    list.retain(|t| t != text);
    list.insert(0, text.to_owned());
    list.truncate(HISTORY);
}

impl ToolSettings {
    pub fn from_keys<'a>(get: impl Fn(&str) -> Option<&'a str>) -> ToolSettings {
        let d = ToolSettings::default();
        let num = |k: &str| get(k).and_then(|v| v.trim().parse::<i64>().ok());
        let flag = |k: &str, default: bool| num(k).map_or(default, |n| n != 0);
        ToolSettings {
            max_undo: num("max_undo")
                .and_then(|n| usize::try_from(n).ok())
                .map_or(d.max_undo, |n| n.min(100_000)),
            rename: super::rename::Options {
                match_case: flag("rename_match_case", false),
                match_diacritics: flag("rename_match_diacritics", false),
                regex: flag("rename_regex", false),
                ignore_extension: flag("rename_ignore_extension", false),
            },
            old_history: history_of(get("rename_old_format_history")),
            new_history: history_of(get("rename_new_format_history")),
            time_kinds: TIME_KEYS.map(|k| {
                num(&format!("{k}_type"))
                    .and_then(|n| u8::try_from(n).ok())
                    .filter(|&n| n <= 4)
                    .unwrap_or(0)
            }),
            time_custom: TIME_KEYS.map(|k| get(k).unwrap_or_default().to_owned()),
            undo_sort: num("undo_history_sort")
                .and_then(|n| u8::try_from(n).ok())
                .filter(|&n| n < 6)
                .unwrap_or(d.undo_sort),
            undo_ascending: flag("undo_history_sort_ascending", d.undo_ascending),
            last_folder: get("rename_last_browse_path")
                .unwrap_or_default()
                .to_owned(),
            copy_to: Formats::from_keys("advanced_copy_to", &get),
            move_to: Formats::from_keys("advanced_move_to", &get),
            presets: presets_of(get("rename_presets")),
        }
    }

    pub fn to_keys(&self) -> Vec<(String, String)> {
        let bit = |b: bool| u8::from(b).to_string();
        let mut out = vec![
            ("max_undo".to_owned(), self.max_undo.to_string()),
            ("rename_match_case".to_owned(), bit(self.rename.match_case)),
            (
                "rename_match_diacritics".to_owned(),
                bit(self.rename.match_diacritics),
            ),
            ("rename_regex".to_owned(), bit(self.rename.regex)),
            (
                "rename_ignore_extension".to_owned(),
                bit(self.rename.ignore_extension),
            ),
            (
                "rename_old_format_history".to_owned(),
                history_value(&self.old_history),
            ),
            (
                "rename_new_format_history".to_owned(),
                history_value(&self.new_history),
            ),
            (
                "rename_last_browse_path".to_owned(),
                self.last_folder.clone(),
            ),
        ];
        for (i, k) in TIME_KEYS.iter().enumerate() {
            out.push((format!("{k}_type"), self.time_kinds[i].to_string()));
        }
        for (i, k) in TIME_KEYS.iter().enumerate() {
            out.push(((*k).to_owned(), self.time_custom[i].clone()));
        }
        out.push(("undo_history_sort".to_owned(), self.undo_sort.to_string()));
        out.push((
            "undo_history_sort_ascending".to_owned(),
            bit(self.undo_ascending),
        ));
        self.copy_to.to_keys("advanced_copy_to", &mut out);
        self.move_to.to_keys("advanced_move_to", &mut out);
        out.push(("rename_presets".to_owned(), presets_value(&self.presets)));
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[test]
    fn everythings_keys_and_defaults() {
        let none = ToolSettings::from_keys(|_| None);
        assert_eq!(none, ToolSettings::default());
        assert_eq!(none.max_undo, 256);
        // As Everything 1.5.0.1424b writes them.
        let pairs = [
            ("max_undo", "10"),
            ("rename_match_case", "1"),
            ("rename_regex", "1"),
            ("rename_old_format_history", r#""%1.txt","a,b""#),
            ("set_file_time_date_modified_type", "4"),
            ("set_file_time_date_modified", "2020-01-02"),
            ("set_file_time_date_accessed_type", "9"),
            ("undo_history_sort", "3"),
            ("undo_history_sort_ascending", "1"),
            ("rename_last_browse_path", r"D:\x"),
        ];
        let map: HashMap<&str, &str> = pairs.into_iter().collect();
        let s = ToolSettings::from_keys(|k| map.get(k).copied());
        assert_eq!(s.max_undo, 10);
        assert!(s.rename.match_case && s.rename.regex && !s.rename.ignore_extension);
        assert_eq!(s.old_history, ["%1.txt", "a,b"]);
        assert_eq!(s.time_kinds, [0, 4, 0], "a kind out of range is No change");
        assert_eq!(s.time_custom[1], "2020-01-02");
        assert_eq!((s.undo_sort, s.undo_ascending), (3, true));
        assert_eq!(s.last_folder, r"D:\x");
        // What goz-gui writes reads back the same.
        let keys = s.to_keys();
        let map: HashMap<&str, &str> = keys.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
        assert_eq!(ToolSettings::from_keys(|k| map.get(k).copied()), s);
        assert!(map.contains_key("set_file_time_date_created_type"));
        assert!(map.contains_key("rename_match_diacritics"));
    }

    #[test]
    fn copy_move_formats_and_presets_round_trip() {
        let pairs = [
            ("advanced_copy_to_regex", "1"),
            ("advanced_copy_to_old_format_history", r#""a","b""#),
            ("advanced_move_to_ignore_extension", "1"),
            (
                "rename_presets",
                r#""Dates","%1-%2","%2 %1","Odd,name","x","""#,
            ),
        ];
        let map: HashMap<&str, &str> = pairs.into_iter().collect();
        let s = ToolSettings::from_keys(|k| map.get(k).copied());
        assert!(s.copy_to.options.regex && !s.copy_to.options.match_case);
        assert_eq!(s.copy_to.old_history, ["a", "b"]);
        assert!(s.move_to.options.ignore_extension);
        assert_eq!(s.presets.len(), 2);
        assert_eq!(s.presets[1].name, "Odd,name");
        assert_eq!(s.presets[1].new_format, "");
        let keys = s.to_keys();
        let map: HashMap<&str, &str> = keys.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
        assert_eq!(ToolSettings::from_keys(|k| map.get(k).copied()), s);
        assert!(map.contains_key("advanced_move_to_new_format_history"));
        // A record cut short keeps the whole presets in it.
        assert_eq!(presets_of(Some(r#""a","b","c","d""#)).len(), 1);
        assert_eq!(presets_value(&[]), "");
    }

    #[test]
    fn presets_are_read_and_written_as_everything_writes_them() {
        // Written by Everything 1.5.0.1424b's New Preset dialog (a portable
        // copy driven by posted messages; quotes, a backslash, a comma, a
        // colon and non-ASCII text, and two switches on).
        let everything = concat!(
            r#"[{"name":"W6 one, \"two\"","old_format":"w6\"old,1;a:b|c\\%1","#,
            r#""new_format":"w6new_%1 \"x\",y","case":0,"diacritics":0,"regex":0,"#,
            r#""ignore_extension":0},{"name":"Café 日本 /","old_format":"tü%1","#,
            r#""new_format":"n<>&%1","case":0,"diacritics":0,"regex":0,"ignore_extension":0},"#,
            r#"{"name":"W6 flags","old_format":"(.*)","new_format":"","case":1,"#,
            r#""diacritics":0,"regex":1,"ignore_extension":0}]"#
        );
        let list = presets_of(Some(everything));
        assert_eq!(list.len(), 3);
        assert_eq!(list[0].name, r#"W6 one, "two""#);
        assert_eq!(list[0].old_format, r#"w6"old,1;a:b|c\%1"#);
        assert_eq!(list[0].new_format, r#"w6new_%1 "x",y"#);
        assert_eq!(list[1].name, "Café 日本 /");
        assert_eq!(list[1].old_format, "tü%1");
        let flags = list[2].options;
        assert!(flags.match_case && flags.regex);
        assert!(!flags.match_diacritics && !flags.ignore_extension);
        assert_eq!(list[2].new_format, "");
        // Written back byte for byte as Everything wrote it.
        assert_eq!(presets_value(&list), everything);
        // goz-gui's old value is still read, and written as Everything's.
        let old = presets_of(Some(r#""Dates","%1-%2","%2 %1""#));
        assert_eq!(old.len(), 1);
        assert_eq!(old[0].options, crate::filetools::rename::Options::default());
        assert_eq!(
            presets_value(&old),
            r#"[{"name":"Dates","old_format":"%1-%2","new_format":"%2 %1","case":0,"diacritics":0,"regex":0,"ignore_extension":0}]"#
        );
        // Not an array of presets: none; a nameless one is left out; a
        // missing switch is off.
        assert!(presets_of(Some("[{")).is_empty());
        assert!(presets_of(Some("[1,2]")).is_empty());
        let partial = presets_of(Some(r#"[{"name":"","old_format":"x"},{"name":"n"}]"#));
        assert_eq!(partial.len(), 1);
        assert_eq!(
            (partial[0].old_format.as_str(), partial[0].options.regex),
            ("", false)
        );
    }

    #[test]
    fn histories_keep_the_newest_first_once() {
        let mut h = vec!["a".to_owned(), "b".to_owned()];
        remember(&mut h, "b");
        assert_eq!(h, ["b", "a"]);
        remember(&mut h, "");
        assert_eq!(h, ["b", "a"]);
        for i in 0..30 {
            remember(&mut h, &i.to_string());
        }
        assert_eq!(h.len(), HISTORY);
        assert_eq!(h[0], "29");
        assert_eq!(history_of(Some(&history_value(&h))), h);
        assert_eq!(history_value(&[]), "");
        assert!(history_of(Some("")).is_empty());
    }
}
