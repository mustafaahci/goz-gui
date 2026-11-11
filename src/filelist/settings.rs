//! The file lists' settings, under Everything's keys:
//! `file_list_history_enabled` (1: File > Recent File Lists keeps the lists
//! opened), `file_list_history` (them, newest first, one CSV record),
//! `file_list_history_max` (24) and `last_open_filelist_filename` (the list
//! Open File List... opened last: its dialog starts there).

use crate::csv::{self, Field};

/// Everything's `file_list_history_max`.
pub const HISTORY_MAX: usize = 24;
/// The most Recent File Lists lists, whatever the setting says: its menu
/// ids run out there.
pub const MENU_MAX: usize = 40;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ListSettings {
    pub history_enabled: bool,
    pub history: Vec<String>,
    pub history_max: usize,
    pub last_open: String,
}

impl Default for ListSettings {
    fn default() -> ListSettings {
        ListSettings {
            history_enabled: true,
            history: Vec::new(),
            history_max: HISTORY_MAX,
            last_open: String::new(),
        }
    }
}

impl ListSettings {
    pub fn from_keys<'a>(get: impl Fn(&str) -> Option<&'a str>) -> ListSettings {
        let d = ListSettings::default();
        let num = |k: &str| get(k).and_then(|v| v.trim().parse::<i64>().ok());
        let history_max = num("file_list_history_max")
            .and_then(|n| usize::try_from(n).ok())
            .map_or(d.history_max, |n| n.min(MENU_MAX));
        let mut history: Vec<String> = get("file_list_history")
            .filter(|v| !v.is_empty())
            .map(|v| {
                csv::parse(v)
                    .into_iter()
                    .next()
                    .unwrap_or_default()
                    .into_iter()
                    .map(|f| f.text)
                    .filter(|t| !t.is_empty())
                    .collect()
            })
            .unwrap_or_default();
        history.truncate(history_max);
        ListSettings {
            history_enabled: num("file_list_history_enabled").map_or(d.history_enabled, |n| n != 0),
            history,
            history_max,
            last_open: get("last_open_filelist_filename")
                .unwrap_or_default()
                .to_owned(),
        }
    }

    pub fn to_keys(&self) -> Vec<(String, String)> {
        let history = if self.history.is_empty() {
            String::new()
        } else {
            let fields: Vec<Field> = self
                .history
                .iter()
                .map(|t| Field::quoted(t.clone()))
                .collect();
            let mut out = String::new();
            csv::write_record(&fields, &mut out);
            out.trim_end_matches(['\r', '\n']).to_owned()
        };
        vec![
            (
                "file_list_history_enabled".into(),
                u8::from(self.history_enabled).to_string(),
            ),
            ("file_list_history".into(), history),
            ("file_list_history_max".into(), self.history_max.to_string()),
            ("last_open_filelist_filename".into(), self.last_open.clone()),
        ]
    }

    /// `path` was opened: first in Recent File Lists (once, any case),
    /// and where Open File List... starts next time.
    pub fn opened(&mut self, path: &str) {
        self.last_open = path.to_owned();
        if !self.history_enabled {
            return;
        }
        self.history.retain(|p| !p.eq_ignore_ascii_case(path));
        self.history.insert(0, path.to_owned());
        self.history.truncate(self.history_max);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[test]
    fn everythings_keys_and_defaults() {
        let d = ListSettings::from_keys(|_| None);
        assert_eq!(d, ListSettings::default());
        assert!(d.history_enabled);
        assert_eq!(d.history_max, 24);
        let mut s = d.clone();
        s.opened(r"C:\a.efu");
        s.opened(r"C:\b,c.efu");
        s.opened(r"c:\A.EFU");
        assert_eq!(s.history, [r"c:\A.EFU", r"C:\b,c.efu"]);
        assert_eq!(s.last_open, r"c:\A.EFU");
        let keys = s.to_keys();
        let map: HashMap<&str, &str> = keys.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
        assert_eq!(ListSettings::from_keys(|k| map.get(k).copied()), s);
        let off = ListSettings::from_keys(|k| match k {
            "file_list_history_enabled" => Some("0"),
            "file_list_history_max" => Some("1000"),
            _ => None,
        });
        assert_eq!(off.history_max, MENU_MAX);
        let mut off = off;
        off.opened(r"C:\x.efu");
        assert!(off.history.is_empty(), "nothing kept with the history off");
        assert_eq!(off.last_open, r"C:\x.efu");
    }
}
