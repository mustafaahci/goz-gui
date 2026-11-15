//! What the sidebars' tree views (Folders and Bookmarks, SysTreeView32) have
//! in common: making one in a pane, items in and out, the theme, and an
//! image list of their own.
//!
//! The image list is the tree's own rather than the system's, so it can
//! hold goz-gui's icon (the Folders tree's top) and take its icons at the
//! window's DPI: each one is copied from whichever of the system's lists
//! has them at the size wanted or the next larger (16, 32, 48 px), and
//! scaled down to it, never up.

use windows_sys::Win32::Foundation::{HWND, POINT, RECT};
use windows_sys::Win32::Graphics::Gdi::HFONT;
use windows_sys::Win32::Storage::FileSystem::{FILE_ATTRIBUTE_DIRECTORY, FILE_ATTRIBUTE_NORMAL};
use windows_sys::Win32::UI::Controls::{
    CDDS_ITEMPREPAINT, CDDS_PREPAINT, CDIS_HOT, CDIS_SELECTED, CDRF_DODEFAULT, CDRF_NEWFONT,
    CDRF_NOTIFYITEMDRAW, HTREEITEM, ILC_COLOR32, ILC_MASK, ILD_NORMAL, NMTVCUSTOMDRAW,
    TVE_COLLAPSE, TVE_EXPAND, TVGN_CARET, TVGN_CHILD, TVGN_DROPHILITE, TVGN_NEXT, TVGN_ROOT,
    TVI_FIRST, TVI_LAST, TVI_ROOT, TVIF_CHILDREN, TVIF_IMAGE, TVIF_PARAM, TVIF_SELECTEDIMAGE,
    TVIF_STATE, TVIS_EXPANDED, TVITEMW, TVSIL_NORMAL,
};
use windows_sys::Win32::UI::Shell::{
    SHGFI_SMALLICON, SHGFI_SYSICONINDEX, SHGFI_USEFILEATTRIBUTES, SIID_FIND,
};

use crate::colors::{Item, State};
use crate::theme::{self, Palette};
use crate::win::icon::Icon;
use crate::win::imagelist::{ImageList, SystemList};
use crate::win::{self, gdi, imagelist, tv};

/// No item: the tree's root (as a parent), or nothing (as an answer).
pub const NONE: HTREEITEM = 0;

/// A new item: under `parent` (`NONE` for the top level), after `after`
/// (`NONE` for first, `last` for the end), with `text`, `image` and a
/// `param` of the caller's; `children` shows an expand button before any
/// child is in.
pub struct New<'a> {
    pub parent: HTREEITEM,
    pub after: Option<HTREEITEM>,
    pub text: &'a [u16],
    pub image: i32,
    pub children: bool,
    pub param: isize,
}

/// Puts `item` in `tree`; the new item, or `NONE` if it failed.
pub fn insert(tree: HWND, item: &New<'_>) -> HTREEITEM {
    let after = match item.after {
        None => TVI_LAST,
        Some(NONE) => TVI_FIRST,
        Some(h) => h,
    };
    // The text goes in as it is, UTF-16 (a file's name need not be valid
    // Unicode); TVIF_TEXT is added with it.
    let it = TVITEMW {
        mask: TVIF_PARAM | TVIF_IMAGE | TVIF_SELECTEDIMAGE | TVIF_CHILDREN,
        iImage: item.image,
        iSelectedImage: item.image,
        cChildren: i32::from(item.children),
        lParam: item.param,
        ..Default::default()
    };
    win::tv::insert_units(tree, item.parent, after, it, item.text)
}

/// `item`'s param; `None` for no item.
pub fn param(tree: HWND, item: HTREEITEM) -> Option<isize> {
    if item == NONE {
        return None;
    }
    tv::item_param(tree, item)
}

/// Shows an expand button on `item`, or not.
pub fn set_children(tree: HWND, item: HTREEITEM, children: bool) {
    tv::set_item(
        tree,
        TVITEMW {
            mask: TVIF_CHILDREN,
            hItem: item,
            cChildren: i32::from(children),
            ..Default::default()
        },
        None,
    );
}

/// `item`'s text.
pub fn set_text(tree: HWND, item: HTREEITEM, text: &[u16]) {
    win::tv::set_text_units(tree, item, text);
}

/// Whether `item` shows an expand button.
pub fn has_children(tree: HWND, item: HTREEITEM) -> bool {
    let it = TVITEMW {
        mask: TVIF_CHILDREN,
        hItem: item,
        ..Default::default()
    };
    tv::get_item(tree, it, 0).is_some_and(|(it, _)| it.cChildren != 0)
}

/// Whether `item` shows open.
pub fn is_expanded(tree: HWND, item: HTREEITEM) -> bool {
    let it = TVITEMW {
        mask: TVIF_STATE,
        hItem: item,
        stateMask: TVIS_EXPANDED,
        ..Default::default()
    };
    tv::get_item(tree, it, 0).is_some_and(|(it, _)| it.state & TVIS_EXPANDED != 0)
}

/// Opens (`open`) or closes `item`.
pub fn expand(tree: HWND, item: HTREEITEM, open: bool) {
    let code = if open { TVE_EXPAND } else { TVE_COLLAPSE };
    tv::expand(tree, item, code);
}

/// Selects `item` (`NONE`: nothing) and scrolls it into view.
pub fn select(tree: HWND, item: HTREEITEM) {
    tv::select_item(tree, TVGN_CARET, item);
    if item != NONE {
        tv::ensure_visible(tree, item);
    }
}

/// The selected item.
pub fn selected(tree: HWND) -> HTREEITEM {
    tv::next_item(tree, NONE, TVGN_CARET)
}

/// Marks `item` as the one a context menu is for (`NONE`: none), as
/// Explorer's tree does while its menu is up.
pub fn drop_highlight(tree: HWND, item: HTREEITEM) {
    tv::select_item(tree, TVGN_DROPHILITE, item);
}

/// Takes `item` (and what is under it) out; `NONE` empties the tree.
pub fn delete(tree: HWND, item: HTREEITEM) {
    let item = if item == NONE { TVI_ROOT } else { item };
    tv::delete_item(tree, item);
}

/// The items directly under `parent` (`NONE`: the top level), in order.
pub fn children(tree: HWND, parent: HTREEITEM) -> Vec<HTREEITEM> {
    let mut out = Vec::new();
    let mut at = if parent == NONE {
        tv::next_item(tree, NONE, TVGN_ROOT)
    } else {
        tv::next_item(tree, parent, TVGN_CHILD)
    };
    while at != NONE {
        out.push(at);
        at = tv::next_item(tree, at, TVGN_NEXT);
    }
    out
}

/// The item at `pt` (the tree's client coordinates) and where on it
/// (`TVHT_*`).
pub fn hit(tree: HWND, pt: POINT) -> (HTREEITEM, u32) {
    let info = tv::hit_test(tree, pt);
    (info.hItem, info.flags)
}

/// `item`'s row, or its text only (`text`), in the tree's coordinates.
pub fn item_rect(tree: HWND, item: HTREEITEM, text: bool) -> RECT {
    tv::item_rect(tree, item, text).unwrap_or_default()
}

/// The tree in `item`'s colours (Options > Fonts and Colors' sidebar: its
/// Normal background and text), the Explorer look (its hover and
/// selection, dark ones in the dark theme, where [`custom_draw`] leaves
/// them), and its tooltips to match.
pub fn apply_theme(tree: HWND, pal: &Palette, item: Item) {
    let class = if pal.dark {
        "DarkMode_Explorer"
    } else {
        "Explorer"
    };
    let ink = pal.ink(item, State::Normal);
    theme::allow(tree, pal.dark);
    theme::set_class(tree, Some(class));
    tv::set_bk_color(tree, ink.bg);
    tv::set_text_color(tree, ink.fg);
    let tip = tv::tooltips(tree);
    if !tip.is_null() {
        theme::allow(tip, pal.dark);
        theme::set_class(tip, Some(class));
    }
}

/// The states of a sidebar's rows besides Normal (whose colours are the
/// tree's own, [`apply_theme`]).
const ROW_STATES: [State; 3] = [State::Selected, State::InactiveSelected, State::Mouseover];

/// A sidebar tree's `NM_CUSTOMDRAW` (`cd`, which the tree reads back after
/// this returns), for Options > Fonts and Colors' `item`: a row in a state
/// whose colours are not the theme's (chosen, or tinted by an accent) is
/// filled with them and keeps no selection or hover of the visual style; a
/// row whose style is Bold gets `bold`. Every other row is the visual
/// style's.
pub fn custom_draw(
    tree: HWND,
    cd: &mut NMTVCUSTOMDRAW,
    pal: &Palette,
    item: Item,
    bold: HFONT,
) -> isize {
    let wanted = ROW_STATES.iter().any(|&s| pal.is_own(item, s))
        || [
            State::Normal,
            State::Selected,
            State::InactiveSelected,
            State::Mouseover,
        ]
        .iter()
        .any(|&s| pal.is_bold(item, s));
    match cd.nmcd.dwDrawStage {
        CDDS_PREPAINT if wanted => CDRF_NOTIFYITEMDRAW as isize,
        CDDS_ITEMPREPAINT => {
            let st = cd.nmcd.uItemState;
            let focused = win::focus() == tree;
            let state = State::of_row(st & CDIS_SELECTED != 0, focused, st & CDIS_HOT != 0);
            let mut answer = CDRF_DODEFAULT;
            if state != State::Normal && pal.is_own(item, state) {
                let ink = pal.ink(item, state);
                crate::paint::fill(cd.nmcd.hdc, &cd.nmcd.rc, ink.bg);
                cd.nmcd.uItemState &= !(CDIS_SELECTED | CDIS_HOT);
                cd.clrText = ink.fg;
                cd.clrTextBk = ink.bg;
                answer = CDRF_NEWFONT;
            }
            if pal.is_bold(item, state) {
                // The tree's DC for this item; the tree puts its own font
                // back after the item (CDRF_NEWFONT).
                gdi::select(cd.nmcd.hdc, bold);
                answer = CDRF_NEWFONT;
            }
            answer as isize
        }
        _ => CDRF_DODEFAULT as isize,
    }
}

/// The tree's font and the height of its rows: the text with 2 px above
/// and below at 100 % (19 px with the usual 15 px font, as Everything's),
/// never less than the icons and a pixel above and below them.
pub fn set_font(tree: HWND, font: HFONT, font_height: i32, icon: i32, scale: impl Fn(i32) -> i32) {
    win::set_font(tree, font, true);
    let high = (font_height + scale(4)).max(icon + scale(2));
    tv::set_item_height(tree, high.max(1));
}

/// Where an icon of a tree's image list comes from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    /// goz-gui's own icon (the shell's search icon in a build without one).
    App,
    /// The system image list's icon of this index.
    System(i32),
}

/// The system image list's index of a plain folder's icon and of a plain
/// file's (a bookmark's, as in the bookmark dialogs), from the shell's
/// registry alone.
pub fn plain_icons() -> (i32, i32) {
    let lookup = |name: &str, attributes: u32| {
        // An attribute-only lookup.
        let name: Vec<u16> = name.encode_utf16().collect();
        let (_, info) = win::shell::file_info(
            &name,
            attributes,
            SHGFI_SYSICONINDEX | SHGFI_SMALLICON | SHGFI_USEFILEATTRIBUTES,
        );
        info.iIcon
    };
    (
        lookup("folder", FILE_ATTRIBUTE_DIRECTORY),
        lookup("bookmark", FILE_ATTRIBUTE_NORMAL),
    )
}

/// The system image list's index of the shell's search icon (a
/// magnifier); -1 if the shell has none.
fn stock_search_icon() -> i32 {
    win::icon::stock_index(SIID_FIND).unwrap_or(-1)
}

/// The system image list to copy `size` px icons from: the one whose icons
/// are that size or the least larger, else the largest.
fn system_list(size: i32) -> Option<SystemList> {
    // SHIL_LARGE (32), SHIL_SMALL (16), SHIL_EXTRALARGE (48).
    let mut best: Option<(i32, SystemList)> = None;
    let mut largest: Option<(i32, SystemList)> = None;
    for which in [1, 0, 2] {
        // The shell hands out its own image list, which lives as long as
        // the process; it is never released.
        let Some(list) = win::imagelist::system(which) else {
            continue;
        };
        let (cx, _) = imagelist::icon_size(&list).unwrap_or((0, 0));
        if cx >= size && best.is_none_or(|(b, _)| cx < b) {
            best = Some((cx, list));
        }
        if largest.is_none_or(|(l, _)| cx > l) {
            largest = Some((cx, list));
        }
    }
    best.or(largest).map(|(_, list)| list)
}

/// A tree's own image list: the icons it shows, at one size. The list is
/// destroyed as this drops, when no tree shows it any more (see
/// `resized`, and the panes' destroy).
pub struct Images {
    /// `None` if comctl32 could not make one.
    list: Option<ImageList>,
    size: i32,
    /// What each image is, in order.
    sources: Vec<Source>,
}

impl Images {
    /// An empty list of `size` px icons.
    pub fn new(size: i32) -> Images {
        let size = size.max(1);
        // A new image list, destroyed as this drops.
        let list = ImageList::create(size, size, ILC_COLOR32 | ILC_MASK, 8, 8);
        Images {
            list,
            size,
            sources: Vec::new(),
        }
    }

    pub fn size(&self) -> i32 {
        self.size
    }

    /// The image of `source`, put in first if it is not there yet; -1 if it
    /// could not be.
    pub fn get(&mut self, source: Source) -> i32 {
        if let Some(i) = self.sources.iter().position(|&s| s == source) {
            return i as i32;
        }
        if self.add(source) {
            self.sources.push(source);
            self.sources.len() as i32 - 1
        } else {
            -1
        }
    }

    fn add(&self, source: Source) -> bool {
        let system = |index: i32| -> Option<Icon> {
            let list = system_list(self.size)?;
            if index < 0 {
                return None;
            }
            // A copy of one of the system list's icons, destroyed as it
            // drops.
            win::imagelist::icon(&list, index, ILD_NORMAL)
        };
        let icon = match source {
            // A build without an icon of its own (no assets/goz.ico) shows
            // the shell's search icon: the magnifier, as Everything's top
            // item has its own.
            Source::App => {
                crate::window::app_icon(self.size).or_else(|| system(stock_search_icon()))
            }
            Source::System(index) => system(index),
        };
        let Some(icon) = icon else {
            return false;
        };
        // The list copies the icon (drawn at its own size); the copy made
        // above is destroyed as it drops.
        let at = self
            .list
            .as_ref()
            .map_or(-1, |l| l.replace_icon(-1, icon.handle()));
        at >= 0
    }

    /// The same images at `size` px: a new list, in the same order, so the
    /// items keep their image numbers. The caller hands the tree the new
    /// list before this one is dropped.
    pub fn resized(&self, size: i32) -> Images {
        let mut new = Images::new(size);
        for &source in &self.sources {
            // A source that fails now still takes its place, so the numbers
            // after it hold: a blank image.
            if !new.add(source)
                && let Some(list) = &new.list
            {
                // Grows this list by one blank image.
                list.set_image_count(new.sources.len() as u32 + 1);
            }
            new.sources.push(source);
        }
        new
    }
}

/// Hands `tree` the image list `images` (`None`: none), which the tree
/// keeps alive while it shows it (`tv::set_image_list`).
pub fn set_images(tree: HWND, images: Option<&Images>) {
    tv::set_image_list(tree, TVSIL_NORMAL, images.and_then(|i| i.list.as_ref()));
}
