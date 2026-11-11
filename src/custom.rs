//! The customization settings (wave 4, G2), under Everything 1.5's INI
//! keys: the fonts and colours (Options > Fonts and Colors,
//! `crate::colors`), the keyboard shortcuts (Options > Keyboard,
//! `crate::keymap`), the UI scale (`dpi`), the double-Ctrl action
//! (`double_ctrl_action`) and the color filters (`color_filters`).

use std::sync::atomic::{AtomicU32, Ordering};

use crate::colorfilter::{self, ColorFilter};
use crate::colors::Colors;
use crate::keymap::Bindings;

/// Everything's `dpi` steps: the UI scale in 5 % steps of 96, from 75 %
/// to 200 % (Options > UI's slider; the file may hold any value from 50 %
/// to 400 %).
pub const SCALE_MIN: u32 = 75;
pub const SCALE_MAX: u32 = 200;
const DPI_MIN: u32 = 48;
const DPI_MAX: u32 = 384;

/// What pressing Ctrl twice does (`double_ctrl_action`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum DoubleCtrl {
    #[default]
    None,
    Show,
    NewWindow,
    Toggle,
}

impl DoubleCtrl {
    pub fn from_number(n: i64) -> Option<DoubleCtrl> {
        Some(match n {
            0 => DoubleCtrl::None,
            1 => DoubleCtrl::Show,
            2 => DoubleCtrl::NewWindow,
            3 => DoubleCtrl::Toggle,
            _ => return None,
        })
    }

    pub fn number(self) -> u8 {
        self as u8
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Custom {
    pub colors: Colors,
    pub keys: Bindings,
    /// `dpi`: the UI's DPI at 100 % system scaling (96 = 100 %); 0 is
    /// Everything's "off", which is 100 %.
    pub dpi: u32,
    pub double_ctrl: DoubleCtrl,
    /// `color_filters`, and its text as read when it was not one goz-gui
    /// can read (kept, so the file keeps it).
    pub color_filters: Vec<ColorFilter>,
    pub color_filters_raw: Option<String>,
}

impl Default for Custom {
    fn default() -> Custom {
        Custom {
            colors: Colors::default(),
            keys: Bindings::default(),
            dpi: 96,
            double_ctrl: DoubleCtrl::None,
            color_filters: Vec::new(),
            color_filters_raw: None,
        }
    }
}

/// The UI scale in percent of a `dpi` value.
pub fn scale_percent(dpi: u32) -> u32 {
    if dpi == 0 { 100 } else { (dpi * 100 + 48) / 96 }
}

/// The `dpi` value of a scale in percent.
pub fn dpi_of_percent(percent: u32) -> u32 {
    (percent * 96 + 50) / 100
}

impl Custom {
    pub fn from_keys<'a>(get: impl Fn(&str) -> Option<&'a str> + Copy) -> Custom {
        let d = Custom::default();
        let number = |k: &str| get(k).and_then(|v| v.trim().parse::<i64>().ok());
        let (color_filters, color_filters_raw) = match get("color_filters") {
            None => (Vec::new(), None),
            Some(v) => match colorfilter::parse(v) {
                Ok(rules) => (rules, None),
                Err(_) => (Vec::new(), Some(v.to_owned())),
            },
        };
        Custom {
            colors: Colors::from_keys(get),
            keys: Bindings::from_keys(get),
            dpi: number("dpi")
                .and_then(|n| u32::try_from(n).ok())
                .map_or(
                    d.dpi,
                    |n| {
                        if n == 0 { 0 } else { n.clamp(DPI_MIN, DPI_MAX) }
                    },
                ),
            double_ctrl: number("double_ctrl_action")
                .and_then(DoubleCtrl::from_number)
                .unwrap_or(d.double_ctrl),
            color_filters,
            color_filters_raw,
        }
    }

    pub fn to_keys(&self) -> Vec<(String, String)> {
        let mut out = vec![
            ("dpi".to_owned(), self.dpi.to_string()),
            (
                "double_ctrl_action".to_owned(),
                self.double_ctrl.number().to_string(),
            ),
            (
                "color_filters".to_owned(),
                match &self.color_filters_raw {
                    Some(raw) => raw.clone(),
                    None => colorfilter::format(&self.color_filters),
                },
            ),
        ];
        out.extend(self.colors.to_keys());
        out.extend(self.keys.to_keys());
        out
    }
}

/// `system` DPI scaled by the `dpi` setting (0 or 96: unchanged).
pub fn scaled_dpi(system: u32, setting: u32) -> u32 {
    if setting == 0 || setting == 96 {
        system
    } else {
        (system * setting + 48) / 96
    }
}

/// The `dpi` setting in force for the whole process: the window's
/// measures and the dialogs' fonts follow it.
static UI_DPI: AtomicU32 = AtomicU32::new(96);

pub fn set_ui_dpi(setting: u32) {
    UI_DPI.store(setting, Ordering::Relaxed);
}

/// A monitor's DPI scaled by the UI scale in force.
pub fn scaled(system: u32) -> u32 {
    scaled_dpi(system, UI_DPI.load(Ordering::Relaxed))
}

/// A window's DPI scaled by the UI scale (what dialogs measure with).
pub fn window_dpi(hwnd: windows_sys::Win32::Foundation::HWND) -> u32 {
    // SAFETY: reads a window's DPI; 0 (no window) reads as 96.
    let dpi = unsafe { windows_sys::Win32::UI::HiDpi::GetDpiForWindow(hwnd) };
    scaled(dpi.max(96))
}

/// A dialog font's size in points, scaled by the UI scale.
pub fn scaled_points(points: u16) -> u16 {
    let s = UI_DPI.load(Ordering::Relaxed);
    if s == 0 || s == 96 {
        points
    } else {
        ((u32::from(points) * s + 48) / 96).clamp(4, 200) as u16
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn read(pairs: &[(&str, &str)]) -> Custom {
        Custom::from_keys(|k| pairs.iter().find(|(p, _)| *p == k).map(|(_, v)| *v))
    }

    #[test]
    fn everythings_defaults() {
        // Everything 1.5.0.1424b: `dpi=96`, `double_ctrl_action=0`,
        // `color_filters=`.
        let c = read(&[
            ("dpi", "96"),
            ("double_ctrl_action", "0"),
            ("color_filters", ""),
        ]);
        assert_eq!(c, Custom::default());
        let out = c.to_keys();
        let get = |k: &str| out.iter().find(|(p, _)| p == k).map(|(_, v)| v.as_str());
        assert_eq!(get("dpi"), Some("96"));
        assert_eq!(get("double_ctrl_action"), Some("0"));
        assert_eq!(get("color_filters"), Some(""));
        assert_eq!(get("file_new_tab_keys"), Some("340"));
        assert_eq!(get("normal_foreground_color"), Some(""));
    }

    #[test]
    fn values_are_read_within_bounds() {
        assert_eq!(read(&[("dpi", "144")]).dpi, 144);
        assert_eq!(read(&[("dpi", "0")]).dpi, 0);
        assert_eq!(read(&[("dpi", "5")]).dpi, 48);
        assert_eq!(read(&[("dpi", "9999")]).dpi, 384);
        assert_eq!(read(&[("dpi", "x")]).dpi, 96);
        assert_eq!(
            read(&[("double_ctrl_action", "3")]).double_ctrl,
            DoubleCtrl::Toggle
        );
        assert_eq!(
            read(&[("double_ctrl_action", "7")]).double_ctrl,
            DoubleCtrl::None
        );
    }

    #[test]
    fn a_color_filters_value_goz_gui_cannot_read_is_kept() {
        let c = read(&[("color_filters", "[{oops")]);
        assert!(c.color_filters.is_empty());
        let out = c.to_keys();
        assert!(out.contains(&("color_filters".into(), "[{oops".into())));
    }

    #[test]
    fn the_scale_is_everythings_dpi() {
        // 96 = 100 %, 120 = 125 %, 144 = 150 %, 168 = 175 %, 192 = 200 %.
        for (dpi, pct) in [
            (96, 100),
            (120, 125),
            (144, 150),
            (168, 175),
            (192, 200),
            (72, 75),
        ] {
            assert_eq!(scale_percent(dpi), pct);
            assert_eq!(dpi_of_percent(pct), dpi);
        }
        assert_eq!(scale_percent(0), 100);
        assert_eq!(scaled_dpi(144, 96), 144);
        assert_eq!(scaled_dpi(144, 0), 144);
        assert_eq!(scaled_dpi(96, 120), 120);
        assert_eq!(scaled_dpi(144, 120), 180);
    }
}
