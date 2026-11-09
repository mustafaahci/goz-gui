//! View > Zoom: the result list's size, in percent, in Everything's steps
//! (25 % to 500 %). It multiplies the DPI scale of the list's fonts, rows,
//! header and status bar; the search box keeps its size, as in Everything.

/// The zoom levels Zoom In and Zoom Out step through.
pub const STEPS: [u32; 17] = [
    25, 33, 50, 67, 75, 80, 90, 100, 110, 125, 150, 175, 200, 250, 300, 400, 500,
];
pub const DEFAULT: u32 = 100;
pub const MIN: u32 = STEPS[0];
pub const MAX: u32 = STEPS[STEPS.len() - 1];

/// The next step in (`up`) or out from `zoom`, which need not be a step
/// itself; the same at either end.
pub fn step(zoom: u32, up: bool) -> u32 {
    if up {
        STEPS.into_iter().find(|&s| s > zoom).unwrap_or(MAX)
    } else {
        STEPS.into_iter().rev().find(|&s| s < zoom).unwrap_or(MIN)
    }
}

/// A zoom read from the settings: kept in range, 100 if unreadable.
pub fn clamp(zoom: i64) -> u32 {
    u32::try_from(zoom).map_or(DEFAULT, |z| z.clamp(MIN, MAX))
}

/// `v` pixels at 96 DPI and 100 %, at `dpi` and `zoom` %, rounded.
pub fn scale(v: i32, dpi: u32, zoom: u32) -> i32 {
    let num = i64::from(v) * i64::from(dpi) * i64::from(zoom);
    let den = 96 * 100;
    // Rounded half away from zero, so a negative font height scales as its
    // size does.
    ((num + num.signum() * den / 2) / den) as i32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn steps_in_and_out() {
        assert_eq!(step(100, true), 110);
        assert_eq!(step(100, false), 90);
        assert_eq!(step(500, true), 500);
        assert_eq!(step(25, false), 25);
        // Off a step (set in the file): to the next one either way.
        assert_eq!(step(105, true), 110);
        assert_eq!(step(105, false), 100);
        // All the way in and back out lands on 100 again.
        let mut z = DEFAULT;
        for _ in 0..STEPS.len() {
            z = step(z, true);
        }
        assert_eq!(z, MAX);
        for _ in 0..STEPS.len() {
            z = step(z, false);
        }
        assert_eq!(z, MIN);
        assert!(STEPS.windows(2).all(|w| w[0] < w[1]));
    }

    #[test]
    fn read_zoom_is_in_range() {
        assert_eq!(clamp(100), 100);
        assert_eq!(clamp(130), 130);
        assert_eq!(clamp(5), 25);
        assert_eq!(clamp(9000), 500);
        assert_eq!(clamp(-3), 100);
    }

    #[test]
    fn scales_with_dpi_and_zoom() {
        assert_eq!(scale(19, 96, 100), 19);
        assert_eq!(scale(19, 144, 100), 29);
        assert_eq!(scale(19, 96, 200), 38);
        assert_eq!(scale(19, 144, 150), 43);
        assert_eq!(scale(-12, 96, 125), -15);
        assert_eq!(scale(-12, 96, 50), -6);
    }
}
