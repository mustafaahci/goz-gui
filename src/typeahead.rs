//! Type-ahead in the results list, as in Explorer: typing a name's first
//! letters while the list has the keyboard moves to the next row whose
//! name starts with them.
//!
//! The typed text builds up and starts again after a second without
//! typing. A new text looks after the focused row (wrapping round to the
//! top); a longer one looks from the focused row on, so the row that
//! matched so far stays when it still does. The same letter typed again
//! and again goes through the rows starting with it, one at a time.
//!
//! The list is virtual over gozd's results, so only the rows held (those
//! near what has been on screen) are looked at.

use std::time::{Duration, Instant};

use crate::highlight::fold;

/// How long the typed text is kept without typing.
pub const RESET: Duration = Duration::from_millis(1000);

/// The text typed so far, case-folded.
#[derive(Default)]
pub struct TypeAhead {
    typed: Vec<u16>,
    last: Option<Instant>,
}

/// What a key typed looks for.
#[derive(Debug, PartialEq, Eq)]
pub struct Query<'a> {
    /// The text the name starts with, case-folded.
    pub prefix: &'a [u16],
    /// Look after the focused row, not from it.
    pub after: bool,
}

impl TypeAhead {
    /// Typing is under way at `now`: a space then is part of the text, not
    /// the list's own key.
    pub fn typing(&self, now: Instant) -> bool {
        !self.typed.is_empty() && self.last.is_some_and(|t| now.duration_since(t) <= RESET)
    }

    /// `ch` typed at `now`.
    pub fn typed(&mut self, ch: u16, now: Instant) -> Query<'_> {
        if !self.typing(now) {
            self.typed.clear();
        }
        self.last = Some(now);
        self.typed.push(fold(ch));
        let first = self.typed[0];
        if self.typed.len() > 1 && self.typed.iter().all(|&c| c == first) {
            // The same letter again: the next row starting with it.
            Query {
                prefix: &self.typed[..1],
                after: true,
            }
        } else {
            Query {
                prefix: &self.typed,
                after: self.typed.len() == 1,
            }
        }
    }

    /// Forgets the text: the next key starts a new one.
    pub fn reset(&mut self) {
        self.typed.clear();
        self.last = None;
    }
}

/// Where a query looks from, with the focused row `focus`.
pub fn start(focus: Option<usize>, after: bool) -> usize {
    match focus {
        Some(f) if after => f + 1,
        Some(f) => f,
        None => 0,
    }
}

/// Of `rows` (each one's index and name), the first whose name starts with
/// `prefix` (case-folded), looking from row `start` to the end and then
/// from the top.
pub fn find<'a>(
    rows: impl IntoIterator<Item = (usize, &'a [u16])>,
    prefix: &[u16],
    start: usize,
) -> Option<usize> {
    rows.into_iter()
        .filter(|(_, name)| starts_with(name, prefix))
        .map(|(i, _)| i)
        .min_by_key(|&i| (i < start, i))
}

fn starts_with(name: &[u16], prefix: &[u16]) -> bool {
    name.len() >= prefix.len() && name.iter().zip(prefix).all(|(&a, &b)| fold(a) == b)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn w(s: &str) -> Vec<u16> {
        s.encode_utf16().collect()
    }

    /// Types `keys` `gap` apart over `names`, moving the focus as a match
    /// would, and gives the focus after each key.
    fn run(names: &[&str], focus: Option<usize>, keys: &str, gap: u64) -> Vec<Option<usize>> {
        let names: Vec<Vec<u16>> = names.iter().map(|n| w(n)).collect();
        let mut ta = TypeAhead::default();
        let mut focus = focus;
        let mut now = Instant::now();
        let mut out = Vec::new();
        for ch in keys.encode_utf16() {
            let q = ta.typed(ch, now);
            let rows = names.iter().enumerate().map(|(i, n)| (i, n.as_slice()));
            if let Some(found) = find(rows, q.prefix, start(focus, q.after)) {
                focus = Some(found);
            }
            out.push(focus);
            now += Duration::from_millis(gap);
        }
        out
    }

    const NAMES: &[&str] = &[
        "alpha.txt",
        "Beta.doc",
        "bravo",
        "Charlie",
        "BRAVO two",
        "delta",
    ];

    #[test]
    fn a_letter_moves_to_the_next_name_starting_with_it() {
        assert_eq!(run(NAMES, None, "b", 100), [Some(1)]);
        assert_eq!(run(NAMES, Some(1), "b", 100), [Some(2)]);
        // Wrapping round to the top.
        assert_eq!(run(NAMES, Some(4), "b", 100), [Some(1)]);
        assert_eq!(run(NAMES, Some(5), "a", 100), [Some(0)]);
        // Nothing starts with it: the focus stays.
        assert_eq!(run(NAMES, Some(3), "z", 100), [Some(3)]);
    }

    #[test]
    fn typed_letters_build_up_from_the_row_found() {
        // "br" stays on "bravo" rather than going past it.
        assert_eq!(run(NAMES, None, "bra", 100), [Some(1), Some(2), Some(2)]);
        assert_eq!(
            run(NAMES, Some(2), "bravo ", 100),
            [Some(4), Some(4), Some(4), Some(4), Some(4), Some(4)]
        );
        assert_eq!(run(NAMES, None, "CHAR", 100), [Some(3); 4]);
    }

    #[test]
    fn the_same_letter_again_goes_through_its_rows() {
        assert_eq!(
            run(NAMES, None, "bbbb", 100),
            [Some(1), Some(2), Some(4), Some(1)]
        );
    }

    #[test]
    fn a_pause_starts_the_text_again() {
        // "b", then "d" a moment later: "bd" matches nothing, the focus
        // stays; after a second's pause "d" is a new text.
        assert_eq!(run(NAMES, None, "bd", 100), [Some(1), Some(1)]);
        assert_eq!(run(NAMES, None, "bd", 1100), [Some(1), Some(5)]);
        let mut ta = TypeAhead::default();
        let t = Instant::now();
        assert!(!ta.typing(t));
        ta.typed(u16::from(b'x'), t);
        assert!(ta.typing(t + Duration::from_millis(900)));
        assert!(!ta.typing(t + Duration::from_millis(1100)));
        ta.reset();
        assert!(!ta.typing(t));
    }

    #[test]
    fn only_the_rows_given_are_searched() {
        // Rows 10 and 20 held, the rest not: from row 15 "a" finds 20.
        let a = w("a1");
        let b = w("a2");
        let rows = [(10, a.as_slice()), (20, b.as_slice())];
        assert_eq!(find(rows, &w("a"), 15), Some(20));
        assert_eq!(find(rows, &w("a"), 21), Some(10));
        assert_eq!(find(rows, &w("b"), 0), None);
        assert_eq!(start(Some(3), true), 4);
        assert_eq!(start(Some(3), false), 3);
        assert_eq!(start(None, true), 0);
    }
}
