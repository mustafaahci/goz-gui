//! The preview pane's settings, under Everything 1.5's INI keys and with its
//! defaults. They are read as the window starts; nothing in goz-gui changes
//! them (Everything keeps them among its advanced settings too).

/// Whether other files than images go to the preview handlers Windows has
/// registered (`show_preview_handlers_in_preview_pane`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Handlers {
    /// `0`: as Explorer's "Show preview handlers in preview pane" says.
    System,
    /// `1`.
    Always,
    /// `2`: images, text and icons only.
    Never,
}

impl Handlers {
    fn from_number(n: i64) -> Option<Handlers> {
        match n {
            0 => Some(Handlers::System),
            1 => Some(Handlers::Always),
            2 => Some(Handlers::Never),
            _ => None,
        }
    }

    fn number(self) -> u8 {
        match self {
            Handlers::System => 0,
            Handlers::Always => 1,
            Handlers::Never => 2,
        }
    }
}

/// How an image is drawn bigger or smaller than it was decoded
/// (`preview_mag_filter`, `preview_min_filter`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stretch {
    /// `0`: goz-gui's choice, half tone.
    Default,
    /// `3`: colour on colour, the faster.
    Fast,
    /// `4`: half tone, the smoother.
    Smooth,
}

impl Stretch {
    fn from_number(n: i64) -> Option<Stretch> {
        match n {
            0 => Some(Stretch::Default),
            3 => Some(Stretch::Fast),
            4 => Some(Stretch::Smooth),
            _ => None,
        }
    }

    fn number(self) -> u8 {
        match self {
            Stretch::Default => 0,
            Stretch::Fast => 3,
            Stretch::Smooth => 4,
        }
    }

    /// Whether drawing goes through half tone (else colour on colour).
    pub fn smooth(self) -> bool {
        self != Stretch::Fast
    }
}

/// The side of the box images are decoded into when `preview_load_size`
/// is 0 (Everything's default, 1024).
pub const LOAD_SIZE: u32 = 1024;
/// The biggest `preview_load_size` taken from the file: 16384 px square is
/// a 1 GB bitmap already.
const LOAD_SIZE_MAX: i64 = 8192;
/// The smallest taken: smaller than a pane makes every image blurred.
const LOAD_SIZE_MIN: i64 = 64;

/// Everything's preview keys that goz-gui follows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PreviewSettings {
    /// `show_preview_handlers_in_preview_pane` (0).
    pub handlers: Handlers,
    /// `preview_fill` (0): an image smaller than the pane is enlarged to
    /// fill it; else it is shown at most at its own size.
    pub fill: bool,
    /// `preview_load_size` (0, which is 1024): the side of the box an image
    /// is decoded into, before it is drawn at the pane's size. A pane
    /// bigger than the box decodes it bigger.
    pub load_size: u32,
    /// `preview_min_filter` and `preview_mag_filter` (0).
    pub min_filter: Stretch,
    pub mag_filter: Stretch,
    /// `preview_release_handler_on_clear` (0): with nothing to preview, the
    /// preview handler is let go (else it is kept, unloaded, for the next
    /// file of its type).
    pub release_on_clear: bool,
    /// `preview_include_files`, `preview_include_folders` (1): files and
    /// folders are previewed at all.
    pub files: bool,
    pub folders: bool,
}

impl Default for PreviewSettings {
    fn default() -> PreviewSettings {
        PreviewSettings {
            handlers: Handlers::System,
            fill: false,
            load_size: 0,
            min_filter: Stretch::Default,
            mag_filter: Stretch::Default,
            release_on_clear: false,
            files: true,
            folders: true,
        }
    }
}

fn number(v: Option<&str>) -> Option<i64> {
    v?.trim().parse().ok()
}

fn flag(v: Option<&str>) -> Option<bool> {
    match v?.trim() {
        "1" => Some(true),
        "0" => Some(false),
        v if v.eq_ignore_ascii_case("true") => Some(true),
        v if v.eq_ignore_ascii_case("false") => Some(false),
        _ => None,
    }
}

impl PreviewSettings {
    /// The settings `get` holds under Everything's keys; what is missing or
    /// unreadable is the default.
    pub fn from_keys<'a>(get: impl Fn(&str) -> Option<&'a str>) -> PreviewSettings {
        let d = PreviewSettings::default();
        PreviewSettings {
            handlers: number(get("show_preview_handlers_in_preview_pane"))
                .and_then(Handlers::from_number)
                .unwrap_or(d.handlers),
            fill: flag(get("preview_fill")).unwrap_or(d.fill),
            load_size: number(get("preview_load_size"))
                .filter(|&n| n == 0 || (LOAD_SIZE_MIN..=LOAD_SIZE_MAX).contains(&n))
                .map_or(d.load_size, |n| n as u32),
            min_filter: number(get("preview_min_filter"))
                .and_then(Stretch::from_number)
                .unwrap_or(d.min_filter),
            mag_filter: number(get("preview_mag_filter"))
                .and_then(Stretch::from_number)
                .unwrap_or(d.mag_filter),
            release_on_clear: flag(get("preview_release_handler_on_clear"))
                .unwrap_or(d.release_on_clear),
            files: flag(get("preview_include_files")).unwrap_or(d.files),
            folders: flag(get("preview_include_folders")).unwrap_or(d.folders),
        }
    }

    /// Every key, with its value, in Everything's order.
    pub fn to_keys(self) -> Vec<(String, String)> {
        let bit = |b: bool| u8::from(b).to_string();
        vec![
            (
                "preview_min_filter".into(),
                self.min_filter.number().to_string(),
            ),
            (
                "preview_mag_filter".into(),
                self.mag_filter.number().to_string(),
            ),
            ("preview_fill".into(), bit(self.fill)),
            (
                "show_preview_handlers_in_preview_pane".into(),
                self.handlers.number().to_string(),
            ),
            ("preview_load_size".into(), self.load_size.to_string()),
            (
                "preview_release_handler_on_clear".into(),
                bit(self.release_on_clear),
            ),
            ("preview_include_folders".into(), bit(self.folders)),
            ("preview_include_files".into(), bit(self.files)),
        ]
    }

    /// The side of the box images are decoded into, in pixels.
    pub fn load_side(self) -> i32 {
        if self.load_size == 0 {
            LOAD_SIZE as i32
        } else {
            self.load_size as i32
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn read(pairs: &[(&str, &str)]) -> PreviewSettings {
        let map: HashMap<&str, &str> = pairs.iter().copied().collect();
        PreviewSettings::from_keys(|k| map.get(k).copied())
    }

    #[test]
    fn everythings_keys_and_defaults() {
        // As Everything 1.5.0.1424b writes them.
        let d = read(&[
            ("preview_min_filter", "0"),
            ("preview_mag_filter", "0"),
            ("preview_fill", "0"),
            ("show_preview_handlers_in_preview_pane", "0"),
            ("preview_load_size", "0"),
            ("preview_release_handler_on_clear", "0"),
            ("preview_include_folders", "1"),
            ("preview_include_files", "1"),
        ]);
        assert_eq!(d, PreviewSettings::default());
        assert_eq!(read(&[]), PreviewSettings::default());
        assert_eq!(d.load_side(), 1024);
        assert!(d.min_filter.smooth() && d.mag_filter.smooth());
        // What is written reads back the same.
        let s = PreviewSettings {
            handlers: Handlers::Never,
            fill: true,
            load_size: 2048,
            min_filter: Stretch::Fast,
            mag_filter: Stretch::Smooth,
            release_on_clear: true,
            files: false,
            folders: false,
        };
        let pairs = s.to_keys();
        let map: HashMap<&str, &str> = pairs
            .iter()
            .map(|(k, v)| (k.as_str(), v.as_str()))
            .collect();
        assert_eq!(PreviewSettings::from_keys(|k| map.get(k).copied()), s);
        assert_eq!(map.get("show_preview_handlers_in_preview_pane"), Some(&"2"));
        assert_eq!(map.get("preview_min_filter"), Some(&"3"));
        assert_eq!(s.load_side(), 2048);
        assert!(!s.min_filter.smooth());
        assert_eq!(
            read(&[("show_preview_handlers_in_preview_pane", "1")]).handlers,
            Handlers::Always
        );
    }

    #[test]
    fn unreadable_values_are_the_defaults() {
        let d = read(&[
            ("show_preview_handlers_in_preview_pane", "3"),
            ("preview_fill", "yes"),
            ("preview_load_size", "-1"),
            ("preview_min_filter", "1"),
            ("preview_mag_filter", "x"),
            ("preview_include_files", ""),
        ]);
        assert_eq!(d, PreviewSettings::default());
        // A load size out of bounds is the default, not clamped: a typo
        // never decodes gigabyte bitmaps.
        assert_eq!(read(&[("preview_load_size", "100000")]).load_size, 0);
        assert_eq!(read(&[("preview_load_size", "10")]).load_size, 0);
        assert_eq!(read(&[("preview_load_size", "640")]).load_side(), 640);
    }
}
