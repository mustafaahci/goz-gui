//! Tools > Options from the window's side: what the page starts with, and
//! applying what it hands back.

use super::App;
use crate::options::{self, Values};
use crate::{dialog, integration};

impl App {
    fn options_now(&self) -> Values {
        let base = self.prefs.borrow().base.clone();
        let h = base.history;
        Values {
            start_with_windows: integration::starts_with_windows(),
            folder_item: integration::has_folder_item(),
            background: self.background(),
            enter: self.enter_action(),
            suggest: h.search_always_suggest,
            search_history: h.search_enabled,
            search_limit: !h.search_keep_forever,
            search_days: h.search_days_to_keep,
            run_history: h.run_enabled,
            run_limit: !h.run_keep_forever,
            run_days: h.run_days_to_keep,
            theme: self.theme_mode(),
            on_top: self.on_top.get(),
            status_bar: self.status_bar_visible(),
            nav_bar: self.nav_bar_visible(),
        }
    }

    pub(super) fn options_dialog(&self) {
        let initial = self.options_now();
        // Dialogs opened from a background command do not take the
        // foreground (see `dialog::Quiet`).
        let _quiet = dialog::Quiet::begin();
        options::show(self.hwnd, dialog::app_look(), initial, &|v| {
            self.apply_options(v)
        });
    }

    fn apply_options(&self, v: &Values) {
        let mut problems: Vec<&str> = Vec::new();
        if v.start_with_windows != integration::starts_with_windows()
            && !integration::set_starts_with_windows(v.start_with_windows)
        {
            problems.push("starting with Windows could not be changed");
        }
        if v.folder_item != integration::has_folder_item()
            && !integration::set_folder_item(v.folder_item)
        {
            problems.push("the folder item in Explorer could not be changed");
        }
        let taken = self.apply_background(v.background);
        {
            let mut prefs = self.prefs.borrow_mut();
            let b = &mut prefs.base;
            b.background = v.background;
            b.enter_action = v.enter;
            b.nav_bar = v.nav_bar;
            let h = &mut b.history;
            h.search_always_suggest = v.suggest;
            h.search_enabled = v.search_history;
            h.search_keep_forever = !v.search_limit;
            h.search_days_to_keep = v.search_days;
            h.run_enabled = v.run_history;
            h.run_keep_forever = !v.run_limit;
            h.run_days_to_keep = v.run_days;
        }
        self.set_enter_action(v.enter);
        self.set_suggest(v.suggest);
        if v.theme != self.theme_mode() {
            self.set_theme_mode(v.theme);
        }
        if v.on_top != self.on_top.get() {
            self.set_on_top(v.on_top);
        }
        if v.status_bar != self.status_bar_visible() {
            self.set_status_bar_visible(v.status_bar);
        }
        if v.nav_bar != self.nav_bar_visible() {
            self.set_nav_bar_visible(v.nav_bar);
        }
        self.settings_changed();
        if !taken.is_empty() {
            self.show_note(format!(
                "Another program already uses the {} hotkey",
                taken.join(" and ")
            ));
        } else if !problems.is_empty() {
            self.show_note(format!("Options: {}", problems.join("; ")));
        }
    }
}
