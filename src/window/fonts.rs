//! Wave 6: a font for each part of the window, as Everything 1.5's Fonts
//! and Colors page gives each item one (`crate::colors`): the result
//! list, the header, the status bar, the search box, the filter bar, the
//! find bar, the tab strip, the panes' title rows, the preview and the
//! three sidebars, each with its own face and size, else (Default)'s, else
//! Windows' message font.
//!
//! The window keeps two sets of fonts for the parts that share them: the
//! result list's (`App::fonts`, at View > Zoom's size, which the header and
//! the status bar share while their font is the list's) and the window's
//! own ((Default)'s, `App::ui_fonts`, which every other part shares while
//! its font is (Default)'s). Only a part whose font is not the one it would
//! share gets fonts of its own, so with the default fonts the window makes
//! no more than before.
//!
//! Each part measures itself from its font where it is laid out (the
//! heights of the header, the status bar, the search row, the tab strip,
//! the find bar, the title rows, the sidebars' rows and boxes, the panes'
//! least heights), so a new font, DPI, UI scale or zoom is a remake of the
//! fonts ([`App::remake_fonts`]) and a layout.

use std::cell::Ref;

use windows_sys::Win32::Graphics::Gdi::HFONT;

use super::{App, Fonts};
use crate::colors::{FontSpec, Item};
use crate::zoom;

/// The parts that can have fonts of their own besides the result list's
/// and (Default)'s.
const PARTS: [Item; 11] = [
    Item::Header,
    Item::StatusBar,
    Item::SearchEdit,
    Item::FilterBar,
    Item::FindBar,
    Item::Tabs,
    Item::Pane,
    Item::Preview,
    Item::FoldersSidebar,
    Item::FiltersSidebar,
    Item::BookmarksSidebar,
];

/// The fonts a remake replaced, deleted when dropped: kept until every
/// window holding one (`WM_SETFONT`) has the new one.
pub(super) struct OldFonts {
    _list: Fonts,
    _ui: Fonts,
    _parts: Vec<(Item, Fonts)>,
}

/// Which parts need fonts of their own, and in what: those whose font is
/// not the one they would share (the list's for a part that zooms,
/// (Default)'s for the others).
pub(super) fn own_specs(colors: &crate::colors::Colors) -> Vec<(Item, FontSpec)> {
    let list = colors.resolved_font(Item::ResultList);
    let base = colors.resolved_font(Item::Default);
    PARTS
        .into_iter()
        .filter_map(|item| {
            let spec = colors.resolved_font(item);
            let shared = if item.zooms() { &list } else { &base };
            (spec != *shared).then_some((item, spec))
        })
        .collect()
}

impl App {
    /// An item's own font choice (Options > Fonts and Colors).
    pub fn font_spec(&self, item: Item) -> FontSpec {
        self.prefs.borrow().base.custom.colors.font(item)
    }

    /// The fonts of the parts in `specs`, at the DPI and (for those that
    /// zoom) the zoom now.
    fn make_parts(&self, specs: Vec<(Item, FontSpec)>) -> Vec<(Item, Fonts)> {
        let (dpi, z) = (self.dpi.get(), self.zoom.get());
        specs
            .into_iter()
            .map(|(item, spec)| {
                let at = if item.zooms() { z } else { zoom::DEFAULT };
                (item, Fonts::with(dpi, at, &spec))
            })
            .collect()
    }

    /// Makes every font again (a new DPI, UI scale or font choice). The
    /// caller hands the new ones to the windows holding fonts, then drops
    /// what this returns.
    pub(super) fn remake_fonts(&self) -> OldFonts {
        let colors = self.prefs.borrow().base.custom.colors.clone();
        let dpi = self.dpi.get();
        let list = self.fonts.replace(Fonts::with(
            dpi,
            self.zoom.get(),
            &colors.resolved_font(Item::ResultList),
        ));
        let ui = self.ui_fonts.replace(Fonts::with(
            dpi,
            zoom::DEFAULT,
            &colors.resolved_font(Item::Default),
        ));
        let parts = self.make_parts(own_specs(&colors));
        let parts = self.custom.part_fonts.replace(parts);
        OldFonts {
            _list: list,
            _ui: ui,
            _parts: parts,
        }
    }

    /// Makes the fonts that zoom again at the zoom now: the list's, and the
    /// header's and the status bar's where they have their own (no window
    /// holds those: they are chosen as the parts paint).
    pub(super) fn remake_zoomed_fonts(&self) -> Fonts {
        let colors = self.prefs.borrow().base.custom.colors.clone();
        let list = self.fonts.replace(Fonts::with(
            self.dpi.get(),
            self.zoom.get(),
            &colors.resolved_font(Item::ResultList),
        ));
        let zoomed: Vec<(Item, FontSpec)> = own_specs(&colors)
            .into_iter()
            .filter(|(item, _)| item.zooms())
            .collect();
        let fresh = self.make_parts(zoomed);
        let mut parts = self.custom.part_fonts.borrow_mut();
        parts.retain(|(item, _)| !item.zooms());
        parts.extend(fresh);
        list
    }

    /// Every part in its font again after a font choice: the fonts remade
    /// and handed to the windows holding one (the search box, the list,
    /// its tip, the panes' and strips' controls), the rows measured again,
    /// the window and the panes laid out again; the old fonts go last.
    pub(super) fn refont(&self) {
        crate::dropdown::close(self);
        let old = self.remake_fonts();
        self.apply_fonts();
        self.remeasure_rows();
        self.docks_dpi(self.dpi.get());
        self.layout();
        self.docks_place_all();
        if self.view.get().thumbnails {
            // The thumbnails' names are in the list's font.
            self.put_view();
        }
        drop(old);
    }

    /// The part's own fonts once made at start (the list's and the
    /// window's are made with the window).
    pub(super) fn make_part_fonts(&self) {
        let colors = self.prefs.borrow().base.custom.colors.clone();
        let parts = self.make_parts(own_specs(&colors));
        *self.custom.part_fonts.borrow_mut() = parts;
    }

    /// The fonts `item` draws in: its own, or the list's or the window's
    /// that it shares. Held only while drawing or measuring.
    pub fn part_fonts(&self, item: Item) -> Ref<'_, Fonts> {
        let own = self.custom.part_fonts.borrow();
        if let Some(at) = own.iter().position(|(i, _)| *i == item) {
            return Ref::map(own, |v| &v[at].1);
        }
        drop(own);
        if item.zooms() {
            self.fonts.borrow()
        } else {
            self.ui_fonts.borrow()
        }
    }

    /// `item`'s font, or its bold.
    pub fn part_font(&self, item: Item, bold: bool) -> HFONT {
        let f = self.part_fonts(item);
        if bold { f.bold } else { f.normal }
    }

    /// `item`'s text height in pixels.
    pub fn part_height(&self, item: Item) -> i32 {
        self.part_fonts(item).height
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::colors::Colors;

    fn spec(face: &str, size: u32) -> FontSpec {
        FontSpec {
            face: face.into(),
            size,
        }
    }

    /// With the default fonts no part makes fonts of its own; a part gets
    /// its own only where its font is not the one it shares.
    #[test]
    fn parts_share_fonts_until_they_differ() {
        let mut c = Colors::default();
        assert!(own_specs(&c).is_empty());
        // (Default)'s font: every part that does not zoom shares it; the
        // header and status bar zoom with the list, which follows it too.
        c.set_font(Item::Default, spec("Arial", 11));
        assert!(own_specs(&c).is_empty());
        // The list's own: the header and status bar no longer share it.
        c.set_font(Item::ResultList, spec("Consolas", 0));
        let own = own_specs(&c);
        assert_eq!(
            own,
            [
                (Item::Header, spec("Arial", 11)),
                (Item::StatusBar, spec("Arial", 11)),
            ]
        );
        // The header the list's again: it shares the list's fonts.
        c.set_font(Item::Header, spec("Consolas", 0));
        c.set_font(Item::Tabs, spec("", 14));
        c.set_font(Item::Pane, spec("Arial", 0));
        let own = own_specs(&c);
        assert_eq!(
            own,
            [
                (Item::StatusBar, spec("Arial", 11)),
                (Item::Tabs, spec("Arial", 14)),
            ]
        );
        // Every part but the list's and (Default)'s is listed.
        for item in Item::ALL {
            let listed = PARTS.contains(&item);
            assert_eq!(
                listed,
                item.has_font() && !matches!(item, Item::Default | Item::ResultList),
                "{item:?}"
            );
        }
    }
}
