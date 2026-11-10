//! Where the wave-1 features (F1 to F6, see FRAMEWORK.md in the wave's
//! notes) hook into the window: what they set up as it starts (panes,
//! strips, menu items) and let go of as it ends, their menus about to
//! open, their commands, their WM_APP messages and their main-window
//! timers, each by a range of its own. Each feature fills only its own slot
//! below, so their changes merge without touching each other's lines. Ids
//! outside every range never get here.

use windows_sys::Win32::Foundation::{LPARAM, LRESULT, WPARAM};
use windows_sys::Win32::UI::WindowsAndMessaging::{HMENU, WM_APP};

use super::App;

/// The WM_APP messages each feature may post to the main window: 20 each,
/// from `WM_APP + 120` (the framework's own are `WM_APP + 100..120`).
pub const F1_MSG: u32 = WM_APP + 120;
pub const F2_MSG: u32 = WM_APP + 140;
pub const F3_MSG: u32 = WM_APP + 160;
pub const F4_MSG: u32 = WM_APP + 180;
pub const F5_MSG: u32 = WM_APP + 200;
pub const F6_MSG: u32 = WM_APP + 220;
pub const MSG_RANGE: u32 = 20;

/// The main window's timer ids each feature may use: 10 each, from 110
/// (the framework's own are 100..110). A pane's or strip's own window has
/// timers of its own and needs none of these.
pub const F1_TIMER: usize = 110;
pub const F2_TIMER: usize = 120;
pub const F3_TIMER: usize = 130;
pub const F4_TIMER: usize = 140;
pub const F5_TIMER: usize = 150;
pub const F6_TIMER: usize = 160;
pub const TIMER_RANGE: usize = 10;

impl App {
    /// The window starts (its controls are made and themed, it is not
    /// shown yet): the features register their panes (`register_pane`),
    /// put in their strips (`set_strip`) and add their menu items
    /// (`menu::add_items`).
    pub(super) fn start_features(&self) {
        // F1 (preview pane).
        let preview = self.prefs.borrow().base.preview;
        crate::preview::register(self, preview);

        // F2 (sidebars).
        self.start_sidebars();

        // F3 (tabs and windows).
        self.set_strip(
            crate::strip::StripSlot::Tabs,
            Some(std::rc::Rc::new(super::tabstrip::TabStrip::new())),
        );

        // F4 (search tools).

        // F5 (file tools).

        // F6 (integration and the Options pages).
    }

    /// The window is ending (its settings are saved, its panes still
    /// open): the features stop their threads and let go of what they
    /// hold.
    pub(super) fn exit_features(&self) {
        // F1 (preview pane).
        crate::preview::exit(self);

        // F2 (sidebars).
        self.exit_sidebars();

        // F3 (tabs and windows).

        // F4 (search tools).

        // F5 (file tools).

        // F6 (integration and the Options pages).
    }

    /// A menu of the bar (or a submenu) is about to open, after the
    /// window readied its own items: the features check, grey or rebuild
    /// theirs.
    pub(super) fn feature_menu_popup(&self, popup: HMENU) {
        // F1 (preview pane).

        // F2 (sidebars).

        // F3 (tabs and windows).
        self.tab_menu_popup(popup);

        // F4 (search tools).

        // F5 (file tools).

        // F6 (integration and the Options pages).
    }

    /// A command in a feature's range (`menu::F1_FIRST`, ...): true when
    /// the feature ran it.
    pub(super) fn feature_command(&self, id: u16) -> bool {
        match id {
            // F1: 3000..=3099.
            3000..=3099 => self.preview_command(id),

            // F2: 3100..=3199.
            3100..=3199 => self.sidebar_command(id),

            // F3: 3200..=3299, and its tab entries 4000..=4255.
            3200..=3299 | 4000..=4255 => self.tab_command(id),

            // F4: 3300..=3399, and its custom filters 3800..=3999.
            3300..=3399 | 3800..=3999 => false,

            // F5: 3400..=3499.
            3400..=3499 => false,

            // F6: 3500..=3599.
            3500..=3599 => false,

            _ => false,
        }
    }

    /// A feature's WM_APP message (its range above): the answer, or `None`
    /// for DefWindowProc.
    pub(super) fn feature_message(&self, msg: u32, w: WPARAM, l: LPARAM) -> Option<LRESULT> {
        match msg {
            // F1.
            m if (F1_MSG..F1_MSG + MSG_RANGE).contains(&m) => self.preview_message(m, w, l),

            // F2.
            m if (F2_MSG..F2_MSG + MSG_RANGE).contains(&m) => None,

            // F3.
            m if (F3_MSG..F3_MSG + MSG_RANGE).contains(&m) => self.window_message(m, l),

            // F4.
            m if (F4_MSG..F4_MSG + MSG_RANGE).contains(&m) => None,

            // F5.
            m if (F5_MSG..F5_MSG + MSG_RANGE).contains(&m) => None,

            // F6.
            m if (F6_MSG..F6_MSG + MSG_RANGE).contains(&m) => None,

            _ => None,
        }
    }

    /// A feature's main-window timer (its range above) went off; the timer
    /// is already killed, as the window's own are. True when one took it.
    pub(super) fn feature_timer(&self, id: usize) -> bool {
        match id {
            // F1.
            t if (F1_TIMER..F1_TIMER + TIMER_RANGE).contains(&t) => false,

            // F2.
            t if (F2_TIMER..F2_TIMER + TIMER_RANGE).contains(&t) => false,

            // F3.
            super::tabs::TIMER_THAW => {
                self.thaw_list();
                true
            }
            t if (F3_TIMER..F3_TIMER + TIMER_RANGE).contains(&t) => false,

            // F4.
            t if (F4_TIMER..F4_TIMER + TIMER_RANGE).contains(&t) => false,

            // F5.
            t if (F5_TIMER..F5_TIMER + TIMER_RANGE).contains(&t) => false,

            // F6.
            t if (F6_TIMER..F6_TIMER + TIMER_RANGE).contains(&t) => false,

            _ => false,
        }
    }
}

/// Whether `msg` is one of the features' WM_APP messages.
pub fn is_feature_message(msg: u32) -> bool {
    (F1_MSG..F6_MSG + MSG_RANGE).contains(&msg)
}

/// Whether `id` is one of the features' main-window timers.
pub fn is_feature_timer(id: usize) -> bool {
    (F1_TIMER..F6_TIMER + TIMER_RANGE).contains(&id)
}

// The ranges follow each other and stay clear of the window's own.
const _: () = {
    assert!(F2_MSG == F1_MSG + MSG_RANGE && F3_MSG == F2_MSG + MSG_RANGE);
    assert!(F4_MSG == F3_MSG + MSG_RANGE && F5_MSG == F4_MSG + MSG_RANGE);
    assert!(F6_MSG == F5_MSG + MSG_RANGE);
    assert!(super::WM_PANE_CLOSE < F1_MSG);
    assert!(F2_TIMER == F1_TIMER + TIMER_RANGE && F3_TIMER == F2_TIMER + TIMER_RANGE);
    assert!(F4_TIMER == F3_TIMER + TIMER_RANGE && F5_TIMER == F4_TIMER + TIMER_RANGE);
    assert!(F6_TIMER == F5_TIMER + TIMER_RANGE);
};
