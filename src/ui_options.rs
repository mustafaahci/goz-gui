//! The settings of Tools > Options' UI, Search, Results and View pages that
//! change how the window behaves and looks, under Everything 1.5's INI keys
//! and with its defaults (`goz-gui.ini`, read with the other settings).
//!
//! A few of Everything 1.5's keys are three-way ([`Tri`]): `0` is "use the
//! default", which 1.5 made on, so its own file holds
//! `date_descending_first=0` while Options shows "Sort date descending
//! first" ticked (Everything's Advanced list shows the same family's
//! `randomize_descending_first=0` as "(Use default)"). goz-gui reads them so
//! and writes `1` and `2` for a choice made here, so a file copied either
//! way means the same.

/// A three-way key of Everything 1.5: `0` the default (on), `1` on, `2` off.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Tri {
    #[default]
    Default,
    On,
    Off,
}

impl Tri {
    pub fn on(self) -> bool {
        self != Tri::Off
    }

    /// A check box's state chosen: a default left as it is stays the
    /// default.
    pub fn with(self, on: bool) -> Tri {
        match (self, on) {
            (_, false) => Tri::Off,
            (Tri::Off, true) => Tri::On,
            (t, true) => t,
        }
    }

    fn from_number(n: i64) -> Tri {
        match n {
            1 => Tri::On,
            2 => Tri::Off,
            _ => Tri::Default,
        }
    }

    fn number(self) -> u8 {
        match self {
            Tri::Default => 0,
            Tri::On => 1,
            Tri::Off => 2,
        }
    }
}

/// How the Size column writes a size (`size_format`, Everything's list).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SizeFormat {
    /// Three digits in the unit that keeps them under 1,000: `87.2 KB`.
    Auto,
    /// Bytes with digit grouping: `89,293`.
    Bytes,
    /// Whole kilobytes, rounded up: `88 KB` (Everything's default).
    #[default]
    Kb,
    /// Whole megabytes, rounded up: `1 MB`.
    Mb,
}

impl SizeFormat {
    pub const ALL: [SizeFormat; 4] = [
        SizeFormat::Auto,
        SizeFormat::Bytes,
        SizeFormat::Kb,
        SizeFormat::Mb,
    ];

    pub fn number(self) -> u8 {
        match self {
            SizeFormat::Auto => 0,
            SizeFormat::Bytes => 1,
            SizeFormat::Kb => 2,
            SizeFormat::Mb => 3,
        }
    }

    pub fn from_number(n: i64) -> Option<SizeFormat> {
        usize::try_from(n)
            .ok()
            .and_then(|i| SizeFormat::ALL.get(i).copied())
    }
}

/// Whether one click opens a result (`single_click_open`, Everything's
/// list): as Explorer is set, or always one click, or always two.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ClickOpen {
    #[default]
    System,
    Single,
    Double,
}

impl ClickOpen {
    pub const ALL: [ClickOpen; 3] = [ClickOpen::System, ClickOpen::Single, ClickOpen::Double];

    fn number(self) -> u8 {
        match self {
            ClickOpen::System => 0,
            ClickOpen::Single => 1,
            ClickOpen::Double => 2,
        }
    }

    fn from_number(n: i64) -> Option<ClickOpen> {
        usize::try_from(n)
            .ok()
            .and_then(|i| ClickOpen::ALL.get(i).copied())
    }

    /// Whether one click opens, `explorer_single` being Explorer's own
    /// setting ("Single-click to open an item").
    pub fn single(self, explorer_single: bool) -> bool {
        match self {
            ClickOpen::System => explorer_single,
            ClickOpen::Single => true,
            ClickOpen::Double => false,
        }
    }
}

/// The settings, each under its Everything key (the default after `=`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct UiOptions {
    // UI.
    /// `allow_multiple_windows=0`: starting goz-gui again opens another
    /// window instead of showing this one.
    pub new_window_on_start: bool,
    /// `allow_multiple_windows_from_tray=0`: the tray icon opens another
    /// window instead of showing this one.
    pub new_window_from_tray: bool,
    // Search.
    /// `search_as_you_type=1`: the search runs as it is typed; otherwise on
    /// Enter.
    pub search_as_you_type: bool,
    /// `search_delay=0`: how long typing waits before it searches (ms).
    pub search_delay: u32,
    /// `select_search_on_mouse_click=1`: a click that puts the keyboard in
    /// the search box selects its text.
    pub select_on_click: bool,
    /// `focus_search_on_activate=0`: the keyboard goes to the search box
    /// whenever the window is activated.
    pub focus_on_activate: bool,
    // Results.
    /// `hide_empty_search_results=0`: an empty search lists nothing.
    pub hide_empty: bool,
    /// `clear_selection_on_search=1`: a new search starts with nothing
    /// selected; otherwise the files selected stay selected where they are
    /// in the new results.
    pub clear_selection: bool,
    /// `close_on_execute=0`: opening results closes the window.
    pub close_on_execute: bool,
    /// `double_click_path2=1`: a double-click in the Path column opens the
    /// path.
    pub double_click_path: bool,
    /// `double_quote_copy_as_path=0`: Copy Full Path puts each path in
    /// double quotes.
    pub quote_copy_path: bool,
    /// `rename_select_filepart_only` (three-way, on): Rename selects the
    /// name without its extension.
    pub rename_stem: Tri,
    /// `date_descending_first`, `size_descending_first` (three-way, on): a
    /// new sort by date or size starts with the newest or largest.
    pub date_descending: Tri,
    pub size_descending: Tri,
    /// `full_row_select=1`: a row is selected and shown selected across
    /// its columns; otherwise by its name only.
    pub full_row_select: bool,
    /// `single_click_open=0`.
    pub click_open: ClickOpen,
    // View.
    /// `alternate_row_color=0`: every other row is shaded.
    pub alternate_rows: bool,
    /// `grid_lines=0`: lines between the rows and columns.
    pub grid_lines: bool,
    /// `show_mouseover=1`: the row under the mouse lights up.
    pub mouseover: bool,
    /// `show_highlighted_search_terms=1`: what the search matched is bold.
    pub highlight: bool,
    /// `show_selected_item_in_statusbar=1`: one selected result's size,
    /// date and folder in the status bar.
    pub selected_in_status: bool,
    /// `show_size_in_statusbar=1`: the size of the selection, or of every
    /// result, at the status bar's right.
    pub size_in_status: bool,
    /// `size_format=2`.
    pub size_format: SizeFormat,
}

impl Default for UiOptions {
    fn default() -> UiOptions {
        UiOptions {
            new_window_on_start: false,
            new_window_from_tray: false,
            search_as_you_type: true,
            search_delay: 0,
            select_on_click: true,
            focus_on_activate: false,
            hide_empty: false,
            clear_selection: true,
            close_on_execute: false,
            double_click_path: true,
            quote_copy_path: false,
            rename_stem: Tri::Default,
            date_descending: Tri::Default,
            size_descending: Tri::Default,
            full_row_select: true,
            click_open: ClickOpen::System,
            alternate_rows: false,
            grid_lines: false,
            mouseover: true,
            highlight: true,
            selected_in_status: true,
            size_in_status: true,
            size_format: SizeFormat::Kb,
        }
    }
}

/// The longest search delay taken (ms): a minute.
pub const SEARCH_DELAY_MAX: u32 = 60_000;

fn bit(on: bool) -> String {
    u8::from(on).to_string()
}

impl UiOptions {
    /// The settings `get` holds under Everything's keys; what is missing
    /// or unreadable is the default.
    pub fn from_keys<'a>(get: impl Fn(&str) -> Option<&'a str>) -> UiOptions {
        let d = UiOptions::default();
        let number = |key: &str| get(key).and_then(|v| v.trim().parse::<i64>().ok());
        let flag = |key: &str, default: bool| match get(key).map(str::trim) {
            Some("1") => true,
            Some("0") => false,
            Some(v) if v.eq_ignore_ascii_case("true") => true,
            Some(v) if v.eq_ignore_ascii_case("false") => false,
            _ => default,
        };
        let tri = |key: &str| number(key).map_or(Tri::Default, Tri::from_number);
        UiOptions {
            new_window_on_start: flag("allow_multiple_windows", d.new_window_on_start),
            new_window_from_tray: flag("allow_multiple_windows_from_tray", d.new_window_from_tray),
            search_as_you_type: flag("search_as_you_type", d.search_as_you_type),
            search_delay: number("search_delay")
                .and_then(|n| u32::try_from(n).ok())
                .map_or(d.search_delay, |n| n.min(SEARCH_DELAY_MAX)),
            select_on_click: flag("select_search_on_mouse_click", d.select_on_click),
            focus_on_activate: flag("focus_search_on_activate", d.focus_on_activate),
            hide_empty: flag("hide_empty_search_results", d.hide_empty),
            clear_selection: flag("clear_selection_on_search", d.clear_selection),
            close_on_execute: flag("close_on_execute", d.close_on_execute),
            double_click_path: flag("double_click_path2", d.double_click_path),
            quote_copy_path: flag("double_quote_copy_as_path", d.quote_copy_path),
            rename_stem: tri("rename_select_filepart_only"),
            date_descending: tri("date_descending_first"),
            size_descending: tri("size_descending_first"),
            full_row_select: flag("full_row_select", d.full_row_select),
            click_open: number("single_click_open")
                .and_then(ClickOpen::from_number)
                .unwrap_or(d.click_open),
            alternate_rows: flag("alternate_row_color", d.alternate_rows),
            grid_lines: flag("grid_lines", d.grid_lines),
            mouseover: flag("show_mouseover", d.mouseover),
            highlight: flag("show_highlighted_search_terms", d.highlight),
            selected_in_status: flag("show_selected_item_in_statusbar", d.selected_in_status),
            size_in_status: flag("show_size_in_statusbar", d.size_in_status),
            size_format: number("size_format")
                .and_then(SizeFormat::from_number)
                .unwrap_or(d.size_format),
        }
    }

    /// Every key, with its value, in Everything's order.
    pub fn to_keys(self) -> Vec<(String, String)> {
        let mut out: Vec<(String, String)> = Vec::new();
        let mut put = |k: &str, v: String| out.push((k.to_owned(), v));
        put("allow_multiple_windows", bit(self.new_window_on_start));
        put("alternate_row_color", bit(self.alternate_rows));
        put("show_mouseover", bit(self.mouseover));
        put("show_highlighted_search_terms", bit(self.highlight));
        put("hide_empty_search_results", bit(self.hide_empty));
        put("clear_selection_on_search", bit(self.clear_selection));
        put(
            "show_selected_item_in_statusbar",
            bit(self.selected_in_status),
        );
        put("show_size_in_statusbar", bit(self.size_in_status));
        put("single_click_open", self.click_open.number().to_string());
        put(
            "allow_multiple_windows_from_tray",
            bit(self.new_window_from_tray),
        );
        put("close_on_execute", bit(self.close_on_execute));
        put("double_click_path2", bit(self.double_click_path));
        put("double_quote_copy_as_path", bit(self.quote_copy_path));
        put(
            "rename_select_filepart_only",
            self.rename_stem.number().to_string(),
        );
        put("select_search_on_mouse_click", bit(self.select_on_click));
        put("focus_search_on_activate", bit(self.focus_on_activate));
        put("full_row_select", bit(self.full_row_select));
        put("search_as_you_type", bit(self.search_as_you_type));
        put("search_delay", self.search_delay.to_string());
        put(
            "date_descending_first",
            self.date_descending.number().to_string(),
        );
        put(
            "size_descending_first",
            self.size_descending.number().to_string(),
        );
        put("size_format", self.size_format.number().to_string());
        put("grid_lines", bit(self.grid_lines));
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn read(pairs: &[(&str, &str)]) -> UiOptions {
        UiOptions::from_keys(|k| pairs.iter().find(|(key, _)| *key == k).map(|(_, v)| *v))
    }

    #[test]
    fn everythings_own_values_are_the_defaults() {
        // As Everything 1.5.0.1424b writes them with its defaults.
        let everything = [
            ("allow_multiple_windows", "0"),
            ("alternate_row_color", "0"),
            ("show_mouseover", "1"),
            ("show_highlighted_search_terms", "1"),
            ("hide_empty_search_results", "0"),
            ("clear_selection_on_search", "1"),
            ("show_selected_item_in_statusbar", "1"),
            ("show_size_in_statusbar", "1"),
            ("single_click_open", "0"),
            ("allow_multiple_windows_from_tray", "0"),
            ("close_on_execute", "0"),
            ("double_click_path2", "1"),
            ("double_quote_copy_as_path", "0"),
            ("rename_select_filepart_only", "0"),
            ("select_search_on_mouse_click", "1"),
            ("focus_search_on_activate", "0"),
            ("full_row_select", "1"),
            ("search_as_you_type", "1"),
            ("search_delay", "0"),
            ("date_descending_first", "0"),
            ("size_descending_first", "0"),
            ("size_format", "2"),
            ("grid_lines", "0"),
        ];
        let d = UiOptions::default();
        assert_eq!(read(&everything), d);
        assert_eq!(read(&[]), d);
        // Written back as Everything wrote them, key for key.
        let mut written = d.to_keys();
        written.sort();
        let mut expected: Vec<(String, String)> = everything
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect();
        expected.sort();
        assert_eq!(written, expected);
        // Everything 1.5's three-way defaults are on.
        assert!(d.rename_stem.on() && d.date_descending.on() && d.size_descending.on());
    }

    #[test]
    fn every_value_reads_back() {
        let o = UiOptions {
            new_window_on_start: true,
            new_window_from_tray: true,
            search_as_you_type: false,
            search_delay: 250,
            select_on_click: false,
            focus_on_activate: true,
            hide_empty: true,
            clear_selection: false,
            close_on_execute: true,
            double_click_path: false,
            quote_copy_path: true,
            rename_stem: Tri::Off,
            date_descending: Tri::On,
            size_descending: Tri::Off,
            full_row_select: false,
            click_open: ClickOpen::Single,
            alternate_rows: true,
            grid_lines: true,
            mouseover: false,
            highlight: false,
            selected_in_status: false,
            size_in_status: false,
            size_format: SizeFormat::Mb,
        };
        let pairs = o.to_keys();
        let back = UiOptions::from_keys(|k| {
            pairs
                .iter()
                .find(|(key, _)| key == k)
                .map(|(_, v)| v.as_str())
        });
        assert_eq!(back, o);
    }

    #[test]
    fn odd_values_read_as_the_defaults() {
        let o = read(&[
            ("size_format", "9"),
            ("single_click_open", "-1"),
            ("search_delay", "-5"),
            ("full_row_select", "maybe"),
            ("date_descending_first", "7"),
        ]);
        assert_eq!(o, UiOptions::default());
        // A delay past a minute is a minute; true and false are flags too.
        let o = read(&[("search_delay", "999999"), ("grid_lines", "TRUE")]);
        assert_eq!(o.search_delay, SEARCH_DELAY_MAX);
        assert!(o.grid_lines);
    }

    #[test]
    fn three_way_keys_keep_their_default_until_changed() {
        assert!(Tri::Default.on() && Tri::On.on() && !Tri::Off.on());
        // Left ticked: a default stays one (written as 0, as Everything).
        assert_eq!(Tri::Default.with(true), Tri::Default);
        assert_eq!(Tri::On.with(true), Tri::On);
        // Unticked, then ticked again: on.
        assert_eq!(Tri::Default.with(false), Tri::Off);
        assert_eq!(Tri::Off.with(true), Tri::On);
        assert_eq!(Tri::On.with(false), Tri::Off);
    }

    #[test]
    fn one_click_follows_explorer_unless_set() {
        assert!(ClickOpen::System.single(true));
        assert!(!ClickOpen::System.single(false));
        assert!(ClickOpen::Single.single(false));
        assert!(!ClickOpen::Double.single(true));
    }
}
