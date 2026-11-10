//! The results list's selection as ranges of rows, kept from the list's
//! own notifications.
//!
//! The list is virtual (owner data): it holds its selection as ranges, but
//! answers only "is this row selected" and "the next selected row after
//! this one". Inverting a selection of all rows but a few would ask it for
//! every selected row and then deselect each one, millions of messages. The
//! list reports every change, though: LVN_ITEMCHANGED for a row or for all
//! of them (row -1), and LVN_ODSTATECHANGED for a range (Shift+click,
//! Shift+End). Kept from those, the ranges tell at once which rows are
//! selected and which are not, so the smaller side is the one set row by
//! row. The list stays the authority: what is kept here is checked against
//! it (`len`, and sample rows) before it is used.

use std::collections::BTreeMap;
use std::ops::Range;

/// Selected rows as ranges: sorted, disjoint, never touching, none empty,
/// all inside `0..count`.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Selection {
    /// Start -> end (exclusive).
    ranges: BTreeMap<usize, usize>,
    /// Rows selected: the sum of the ranges' lengths.
    len: usize,
    /// Rows in the list.
    count: usize,
}

impl Selection {
    /// Rows selected.
    pub fn len(&self) -> usize {
        self.len
    }

    /// Rows in the list.
    pub fn count(&self) -> usize {
        self.count
    }

    /// The list has `count` rows now: rows past its end are gone, and rows
    /// added at the end come unselected (as the list itself has them).
    pub fn set_count(&mut self, count: usize) {
        self.count = count;
        self.set_range(count..usize::MAX, false);
    }

    /// Every row selected, or none.
    pub fn set_all(&mut self, on: bool) {
        self.ranges.clear();
        self.len = 0;
        if on && self.count > 0 {
            self.ranges.insert(0, self.count);
            self.len = self.count;
        }
    }

    /// Rows `rows` selected (`on`) or not.
    pub fn set(&mut self, rows: Range<usize>, on: bool) {
        let rows = rows.start..rows.end.min(self.count);
        self.set_range(rows, on);
    }

    fn set_range(&mut self, rows: Range<usize>, on: bool) {
        if rows.start >= rows.end {
            return;
        }
        // The ranges that overlap `rows` or, for a selection, touch it
        // (they merge into one): those starting at or before its end
        // (just after it, to merge), from the last back, while they reach
        // its start.
        let reach = if on {
            rows.end
        } else {
            rows.end.saturating_sub(1)
        };
        let mut hit = Vec::new();
        for (&start, &end) in self.ranges.range(..=reach).rev() {
            let meets = if on {
                end >= rows.start
            } else {
                end > rows.start
            };
            if !meets {
                break;
            }
            hit.push((start, end));
        }
        let mut merged = rows.clone();
        for &(start, end) in &hit {
            self.ranges.remove(&start);
            self.len -= end - start;
            if on {
                merged.start = merged.start.min(start);
                merged.end = merged.end.max(end);
            } else {
                // What lies outside `rows` stays selected.
                if start < rows.start {
                    self.add(start..rows.start);
                }
                if end > rows.end {
                    self.add(rows.end..end);
                }
            }
        }
        if on {
            self.add(merged);
        }
    }

    /// Puts in a range that overlaps and touches none kept.
    fn add(&mut self, rows: Range<usize>) {
        if rows.start < rows.end {
            self.len += rows.end - rows.start;
            self.ranges.insert(rows.start, rows.end);
        }
    }

    /// Whether row `row` is selected.
    #[cfg(test)]
    pub fn contains(&self, row: usize) -> bool {
        self.ranges
            .range(..=row)
            .next_back()
            .is_some_and(|(_, &end)| row < end)
    }

    /// The selected ranges, in order.
    pub fn ranges(&self) -> impl Iterator<Item = Range<usize>> + '_ {
        self.ranges.iter().map(|(&start, &end)| start..end)
    }

    /// The ranges of rows not selected, in order.
    pub fn gaps(&self) -> Vec<Range<usize>> {
        let mut out = Vec::new();
        let mut at = 0;
        for r in self.ranges() {
            if r.start > at {
                out.push(at..r.start);
            }
            at = r.end;
        }
        if at < self.count {
            out.push(at..self.count);
        }
        out
    }

    /// Rows to check against the list before what is kept here is trusted:
    /// the first and last row of each of the first `ranges` ranges, and
    /// the row on either side of each, with whether each is selected.
    pub fn samples(&self, ranges: usize) -> Vec<(usize, bool)> {
        let mut out = Vec::new();
        for r in self.ranges().take(ranges) {
            if r.start > 0 {
                out.push((r.start - 1, false));
            }
            out.push((r.start, true));
            out.push((r.end - 1, true));
            if r.end < self.count {
                out.push((r.end, false));
            }
        }
        if self.ranges.is_empty() && self.count > 0 {
            out.push((0, false));
            out.push((self.count - 1, false));
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A list of `count` rows with `ranges` (start, end) selected.
    fn of(count: usize, ranges: &[(usize, usize)]) -> Selection {
        let mut s = Selection::default();
        s.set_count(count);
        for &(start, end) in ranges {
            s.set(start..end, true);
        }
        s
    }

    /// Ranges as (start, end).
    fn pairs(ranges: impl IntoIterator<Item = Range<usize>>) -> Vec<(usize, usize)> {
        ranges.into_iter().map(|r| (r.start, r.end)).collect()
    }

    fn list(s: &Selection) -> Vec<(usize, usize)> {
        pairs(s.ranges())
    }

    fn gaps(s: &Selection) -> Vec<(usize, usize)> {
        pairs(s.gaps())
    }

    /// Every invariant: sorted, disjoint, not touching, not empty, inside
    /// the list, and `len` their sum.
    fn check(s: &Selection) {
        let mut at = None;
        let mut sum = 0;
        for r in s.ranges() {
            assert!(r.start < r.end, "empty range {r:?}");
            assert!(r.end <= s.count(), "{r:?} past {}", s.count());
            if let Some(end) = at {
                assert!(r.start > end, "{r:?} touches or overlaps the one before");
            }
            at = Some(r.end);
            sum += r.end - r.start;
        }
        assert_eq!(s.len(), sum);
    }

    #[test]
    fn rows_selected_one_by_one_merge_into_ranges() {
        let mut s = of(100, &[]);
        for i in [5, 7, 6, 9, 4] {
            s.set(i..i + 1, true);
            check(&s);
        }
        assert_eq!(list(&s), [(4, 8), (9, 10)]);
        assert_eq!(s.len(), 5);
        // Selecting what is selected changes nothing.
        s.set(5..6, true);
        assert_eq!(list(&s), [(4, 8), (9, 10)]);
        // A range over both, and past them.
        s.set(2..12, true);
        check(&s);
        assert_eq!(list(&s), [(2, 12)]);
    }

    #[test]
    fn deselecting_splits_and_trims() {
        let mut s = of(100, &[(10, 20), (30, 40)]);
        s.set(15..16, false);
        check(&s);
        assert_eq!(list(&s), [(10, 15), (16, 20), (30, 40)]);
        // Over the end of one and the start of the next.
        s.set(18..33, false);
        check(&s);
        assert_eq!(list(&s), [(10, 15), (16, 18), (33, 40)]);
        // Deselecting what is not selected changes nothing.
        s.set(50..60, false);
        s.set(15..16, false);
        assert_eq!(list(&s), [(10, 15), (16, 18), (33, 40)]);
        // Exactly a whole range, then all of what is left.
        s.set(16..18, false);
        assert_eq!(list(&s), [(10, 15), (33, 40)]);
        s.set(0..100, false);
        check(&s);
        assert_eq!(s.len(), 0);
        assert!(list(&s).is_empty());
    }

    #[test]
    fn all_and_none_follow_the_count() {
        let mut s = of(5_000_000, &[]);
        s.set_all(true);
        assert_eq!((s.len(), list(&s)), (5_000_000, vec![(0, 5_000_000)]));
        s.set(10..11, false);
        check(&s);
        assert_eq!(s.len(), 4_999_999);
        assert_eq!(gaps(&s), [(10, 11)]);
        s.set_all(false);
        assert_eq!((s.len(), gaps(&s)), (0, vec![(0, 5_000_000)]));
        // An empty list has nothing to select.
        let mut e = of(0, &[]);
        e.set_all(true);
        assert_eq!(e.len(), 0);
        assert!(e.gaps().is_empty());
    }

    #[test]
    fn rows_past_the_end_go_and_new_rows_come_unselected() {
        let mut s = of(100, &[(10, 20), (90, 100)]);
        s.set_count(95);
        check(&s);
        assert_eq!(list(&s), [(10, 20), (90, 95)]);
        s.set_count(15);
        assert_eq!(list(&s), [(10, 15)]);
        s.set_count(200);
        check(&s);
        assert_eq!(list(&s), [(10, 15)]);
        assert_eq!(gaps(&s), [(0, 10), (15, 200)]);
        // Rows asked for past the end are not selected.
        s.set(190..300, true);
        check(&s);
        assert_eq!(list(&s), [(10, 15), (190, 200)]);
    }

    #[test]
    fn membership_and_gaps() {
        let s = of(50, &[(0, 3), (10, 20)]);
        assert!(s.contains(0) && s.contains(2) && s.contains(10) && s.contains(19));
        assert!(!s.contains(3) && !s.contains(9) && !s.contains(20) && !s.contains(49));
        assert_eq!(gaps(&s), [(3, 10), (20, 50)]);
        let full = of(50, &[(0, 50)]);
        assert!(full.gaps().is_empty());
    }

    #[test]
    fn samples_check_both_edges_of_each_range() {
        let s = of(50, &[(0, 3), (10, 20), (49, 50)]);
        assert_eq!(
            s.samples(8),
            [
                (0, true),
                (2, true),
                (3, false),
                (9, false),
                (10, true),
                (19, true),
                (20, false),
                (48, false),
                (49, true),
                (49, true),
            ]
        );
        // Only the first ranges.
        assert_eq!(s.samples(1).len(), 3);
        // Nothing selected: the ends are checked unselected.
        assert_eq!(of(50, &[]).samples(8), [(0, false), (49, false)]);
        assert_eq!(of(0, &[]).samples(8), []);
    }

    #[test]
    fn a_random_walk_matches_a_plain_bitmap() {
        // A small linear generator: reproducible without a dependency.
        let mut seed = 0x2545_f491_4f6c_dd1du64;
        let mut next = |n: usize| {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            (seed % n as u64) as usize
        };
        let mut count = 300;
        let mut s = of(count, &[]);
        let mut bits = vec![false; count];
        for _ in 0..5_000 {
            match next(10) {
                0 => {
                    let on = next(2) == 0;
                    s.set_all(on);
                    bits.iter_mut().for_each(|b| *b = on);
                }
                1 => {
                    count = 1 + next(400);
                    s.set_count(count);
                    bits.resize(count, false);
                }
                _ => {
                    let a = next(count + 5);
                    let b = a + next(40);
                    let on = next(2) == 0;
                    s.set(a..b, on);
                    for bit in bits.iter_mut().take(b.min(count)).skip(a) {
                        *bit = on;
                    }
                }
            }
            check(&s);
            assert_eq!(s.len(), bits.iter().filter(|b| **b).count());
            for (i, &b) in bits.iter().enumerate() {
                assert_eq!(s.contains(i), b, "row {i}");
            }
        }
    }
}
