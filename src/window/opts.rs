//! Tools > Options from the window's side: the pages the dialog shows
//! (each a module here, `crate::options` the dialog), the page it opens on,
//! and what follows an OK or Apply.

use super::App;
use crate::dialog;
use crate::options::{self, Page};
use crate::settings::Background;

mod general;
mod history;
mod home;
mod keyboard;
mod results;
mod search;
mod ui;
mod view;
// Wave 4 (G2): Fonts and Colors, Advanced.
mod advanced;
mod fonts_colors;

/// The pages, in any order: the dialog puts them in Everything's
/// (`Page::order`, `Page::parent`).
fn pages() -> Vec<Box<dyn Page>> {
    let mut pages: Vec<Box<dyn Page>> = vec![
        Box::new(general::General),
        Box::new(ui::Ui),
        Box::new(search::Search),
        Box::new(view::View),
        Box::new(keyboard::Keyboard::new()),
        Box::new(history::History),
    ];
    // Wave 1: each feature adds its pages on its own lines below, so their
    // changes never meet (FRAMEWORK.md).

    // F1 (preview pane).

    // F2 (sidebars).

    // F3 (tabs and windows).

    // F4 (search tools).

    // F5 (file tools).

    // F6 (integration and the Options pages).
    pages.push(Box::new(home::Home::new()));
    pages.push(Box::new(results::Results));

    // Wave 4 (G2: customization).
    pages.push(Box::new(fonts_colors::FontsColors::new()));
    pages.push(Box::new(advanced::Advanced::new()));

    pages
}

impl App {
    pub(super) fn options_dialog(&self) {
        let first = self.prefs.borrow().base.options_page.clone();
        // Dialogs opened from a background command do not take the
        // foreground (see `dialog::Quiet`).
        let _quiet = dialog::Quiet::begin();
        let last = options::show(self, pages(), &first);
        if last != first {
            self.prefs.borrow_mut().base.options_page = last;
            self.settings_changed();
        }
    }

    /// OK or Apply applied pages: the settings file follows, and the
    /// status bar says what could not be done.
    pub fn options_applied(&self, problems: Vec<String>) {
        self.settings_changed();
        if !problems.is_empty() {
            self.show_note(problems.join("; "));
        }
    }

    /// Options > Advanced applied (`changed`): the settings as they are now
    /// with those values, read as from the file; then what has a setter put
    /// in place at once; the rest (read where it is used, or as goz-gui
    /// starts) is in the settings from now on. The window's own state is
    /// left as it is.
    fn apply_advanced(&self, changed: &[(String, String)], problems: &mut Vec<String>) {
        let mut ini = crate::store::ini::Ini::default();
        // As the window has them now: what the pages applied before this one
        // (OK applies every changed page) stays.
        for (k, v) in self.current_settings().to_pairs() {
            ini.set(&k, &v);
        }
        for (k, v) in changed {
            ini.set(k, v);
        }
        let new = crate::settings::Settings::from_ini(&ini);
        let old = self.prefs.borrow().base.clone();
        {
            // What the window holds, and what the setters below compare
            // with, stays as it was until they run.
            let mut b = new.clone();
            b.window = old.window;
            b.maximized = old.maximized;
            b.match_case = old.match_case;
            b.match_path = old.match_path;
            b.search = old.search.clone();
            b.filter = old.filter;
            b.sort = old.sort;
            b.export_type = old.export_type;
            b.columns = old.columns;
            b.view = old.view;
            b.docks = old.docks;
            b.places = old.places.clone();
            b.options_page = old.options_page.clone();
            b.ui = old.ui;
            b.background = old.background;
            b.custom = old.custom.clone();
            self.prefs.borrow_mut().base = b;
        }
        if new.theme != self.theme_mode() {
            self.set_theme_mode(new.theme);
        }
        if new.on_top != self.on_top.get() {
            self.set_on_top(new.on_top);
        }
        if new.status_bar != self.status_bar_visible() {
            self.set_status_bar_visible(new.status_bar);
        }
        if new.filter_bar != self.filter_bar.get() {
            self.set_filter_bar_visible(new.filter_bar);
        }
        if new.zoom != self.zoom.get() {
            self.set_zoom(new.zoom);
        }
        self.set_ui(new.ui);
        self.change_background(|b| *b = new.background, problems);
        self.set_suggest(new.history.search_always_suggest);
        let c = new.custom;
        self.set_colors(c.colors);
        if c.keys != old.custom.keys {
            self.set_bindings(c.keys);
        }
        if c.dpi != old.custom.dpi {
            self.set_ui_scale(c.dpi);
        }
        if c.double_ctrl != old.custom.double_ctrl {
            self.set_double_ctrl(c.double_ctrl);
        }
        if c.color_filters != old.custom.color_filters {
            self.set_color_filters(c.color_filters);
        }
        // Keys read only as goz-gui starts (or as their feature next opens).
        let live: std::collections::BTreeSet<String> = [
            "theme",
            "ontop",
            "status_bar_visible",
            "filters_visible",
            "zoom",
        ]
        .into_iter()
        .map(String::from)
        .chain(old.ui.to_keys().into_iter().map(|(k, _)| k))
        .chain(old.custom.to_keys().into_iter().map(|(k, _)| k))
        .chain(
            crate::settings::Settings::default()
                .to_pairs()
                .into_iter()
                .map(|(k, _)| k)
                .filter(|k| {
                    k.starts_with("search_history_")
                        || k.starts_with("run_history_")
                        || matches!(
                            k.as_str(),
                            "run_in_background"
                                | "show_tray_icon"
                                | "minimize_to_tray"
                                | "single_click_tray"
                                | "toggle_window_key"
                                | "show_window_key"
                        )
                }),
        )
        .collect();
        if changed.iter().any(|(k, _)| !live.contains(k)) {
            problems
                .push("Some of the settings changed take effect when goz-gui starts again".into());
        }
        self.settings_changed();
    }

    /// Changes the tray and hotkey settings as `change` says (the UI and
    /// Keyboard pages each have some of them) and puts them in place.
    fn change_background(&self, change: impl FnOnce(&mut Background), problems: &mut Vec<String>) {
        let mut b = self.background();
        change(&mut b);
        if b == self.background() {
            return;
        }
        let taken = self.apply_background(b);
        self.prefs.borrow_mut().base.background = b;
        if !taken.is_empty() {
            problems.push(format!(
                "Another program already uses the {} hotkey",
                taken.join(" and ")
            ));
        }
    }
}
