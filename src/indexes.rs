//! Wave 9: gozd's index settings, as Options > Indexes shows them, under
//! Everything's key names and with Everything's defaults.
//!
//! The settings are gozd's, not goz-gui's: one index serves every user of
//! the computer, so gozd keeps them, and only an elevated administrator
//! may change them (wave 9 spec, protocol item 4: `GetConfig`,
//! `SetConfig`). The page reads and writes them through an
//! [`IndexSettings`], on a thread of its own (a call may ask gozd).
//!
//! **Seam for stream B2:** [`source`] gives the gozd-backed implementation
//! once gozd lists the "config" feature. Until then it gives
//! [`NoConfig`], which says why the settings cannot be shown, and the page
//! shows them as unavailable rather than pretend to read or save them.

use std::sync::Arc;

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
    #[allow(
        dead_code,
        reason = "gozd's Config field names: stream B2 maps GetConfig and SetConfig with them"
    )]
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

    #[allow(dead_code, reason = "stream B2: gozd's GetConfig answer, by name")]
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
    #[allow(dead_code, reason = "stream B2: gozd's GetConfig answer")]
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
    #[allow(dead_code, reason = "stream B2: gozd's SetConfig request")]
    pub fn pairs(&self) -> Vec<(&'static str, bool)> {
        Key::ALL
            .into_iter()
            .filter_map(|k| Some((k.name(), self.0[k.at()]?)))
            .collect()
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

/// The settings' source for the gozd running. Stream B2: the gozd-backed
/// one when gozd lists "config".
pub fn source() -> Arc<dyn IndexSettings> {
    Arc::new(NoConfig)
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
        let s = source();
        assert!(s.get().is_err());
        assert!(s.set(&IndexChange::default()).is_err());
    }
}
