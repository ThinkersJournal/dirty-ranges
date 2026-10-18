/// A changed byte range: `len` bytes starting at `offset`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Range {
    pub offset: usize,
    pub len: usize,
}

impl Range {
    /// One past the last byte; saturates rather than wrapping for a range that would run off the address space.
    fn end(&self) -> usize {
        self.offset.saturating_add(self.len)
    }
}

/// Sort `ranges` by offset and merge any that overlap or touch. Empty ranges are dropped.
pub fn coalesce(ranges: &mut Vec<Range>) {
    ranges.retain(|r| r.len != 0);
    ranges.sort_unstable();
    let mut out = 0;
    for i in 1..ranges.len() {
        let next = ranges[i];
        let cur = &mut ranges[out];
        if next.offset <= cur.end() {
            cur.len = next.end().max(cur.end()) - cur.offset;
        } else {
            out += 1;
            ranges[out] = next;
        }
    }
    ranges.truncate((out + 1).min(ranges.len()));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(offset: usize, len: usize) -> Range {
        Range { offset, len }
    }

    fn run(mut v: Vec<Range>) -> Vec<Range> {
        coalesce(&mut v);
        v
    }

    #[test]
    fn empty_input_stays_empty() {
        assert_eq!(run(vec![]), vec![]);
    }

    #[test]
    fn disjoint_ranges_are_sorted_and_kept() {
        assert_eq!(run(vec![r(20, 4), r(0, 4)]), vec![r(0, 4), r(20, 4)]);
    }

    #[test]
    fn touching_ranges_merge() {
        assert_eq!(run(vec![r(0, 4), r(4, 4)]), vec![r(0, 8)]);
    }

    #[test]
    fn overlapping_ranges_merge() {
        assert_eq!(run(vec![r(0, 6), r(4, 6)]), vec![r(0, 10)]);
    }

    #[test]
    fn contained_range_is_absorbed() {
        assert_eq!(run(vec![r(0, 10), r(2, 3)]), vec![r(0, 10)]);
    }

    #[test]
    fn a_one_byte_gap_keeps_ranges_apart() {
        assert_eq!(run(vec![r(0, 4), r(5, 4)]), vec![r(0, 4), r(5, 4)]);
    }

    #[test]
    fn empty_ranges_are_dropped() {
        assert_eq!(run(vec![r(3, 0), r(0, 4), r(9, 0)]), vec![r(0, 4)]);
    }

    #[test]
    fn a_range_ending_at_usize_max_does_not_overflow() {
        assert_eq!(
            run(vec![r(usize::MAX - 2, 2), r(usize::MAX - 4, 2)]),
            vec![r(usize::MAX - 4, 4)]
        );
    }
}
