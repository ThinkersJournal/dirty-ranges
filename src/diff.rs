use crate::Range;

/// Append to `out` the maximal runs of bytes where `old` and `new` differ, as ranges whose offsets are
/// `base` plus the position in the slices.
///
/// Output is sorted and byte-exact: a run starts at its first differing byte and ends at its last, and
/// two runs are never adjacent. Equal slices append nothing.
///
/// # Panics
///
/// If the slices differ in length.
pub fn diff(old: &[u8], new: &[u8], base: usize, out: &mut Vec<Range>) {
    // Equal blocks are skipped with a slice comparison, which the standard library lowers to `memcmp`
    // (vectorised); only blocks that differ are walked byte by byte.
    const BLOCK: usize = 64;
    assert_eq!(old.len(), new.len(), "diff: slices differ in length");
    let n = old.len();
    let mut i = 0;
    let mut run_start: Option<usize> = None;
    while i < n {
        if run_start.is_none() && i + BLOCK <= n && old[i..i + BLOCK] == new[i..i + BLOCK] {
            i += BLOCK;
            continue;
        }
        match (old[i] != new[i], run_start) {
            (true, None) => run_start = Some(i),
            (false, Some(s)) => {
                out.push(Range {
                    offset: base + s,
                    len: i - s,
                });
                run_start = None;
            }
            _ => {}
        }
        i += 1;
    }
    if let Some(s) = run_start {
        out.push(Range {
            offset: base + s,
            len: n - s,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn r(offset: usize, len: usize) -> Range {
        Range { offset, len }
    }

    fn run(old: &[u8], new: &[u8], base: usize) -> Vec<Range> {
        let mut out = Vec::new();
        diff(old, new, base, &mut out);
        out
    }

    /// The obvious byte-at-a-time answer the fast path must agree with.
    fn naive(old: &[u8], new: &[u8], base: usize) -> Vec<Range> {
        let mut out: Vec<Range> = Vec::new();
        for i in 0..old.len() {
            if old[i] != new[i] {
                match out.last_mut() {
                    Some(l) if l.offset + l.len == base + i => l.len += 1,
                    _ => out.push(r(base + i, 1)),
                }
            }
        }
        out
    }

    #[test]
    fn equal_slices_report_nothing() {
        assert_eq!(run(&[7; 100], &[7; 100], 0), vec![]);
    }

    #[test]
    fn empty_slices_report_nothing() {
        assert_eq!(run(&[], &[], 5), vec![]);
    }

    #[test]
    fn one_changed_byte_is_a_one_byte_range() {
        let old = [0u8; 64];
        let mut new = old;
        new[37] = 1;
        assert_eq!(run(&old, &new, 0), vec![r(37, 1)]);
    }

    #[test]
    fn range_starts_at_first_and_ends_at_last_differing_byte() {
        let old = [0u8; 64];
        let mut new = old;
        new[10..20].fill(9);
        assert_eq!(run(&old, &new, 0), vec![r(10, 10)]);
    }

    #[test]
    fn an_unchanged_byte_between_two_changes_splits_them() {
        let old = [0u8; 16];
        let mut new = old;
        new[3] = 1;
        new[5] = 1;
        assert_eq!(run(&old, &new, 0), vec![r(3, 1), r(5, 1)]);
    }

    #[test]
    fn a_run_crossing_a_word_boundary_stays_one_range() {
        let old = [0u8; 32];
        let mut new = old;
        new[6..10].fill(1);
        assert_eq!(run(&old, &new, 0), vec![r(6, 4)]);
    }

    #[test]
    fn a_tail_shorter_than_a_word_is_compared() {
        let old = [0u8; 21];
        let mut new = old;
        new[20] = 1;
        assert_eq!(run(&old, &new, 0), vec![r(20, 1)]);
    }

    #[test]
    fn offsets_are_shifted_by_base() {
        let old = [0u8; 8];
        let mut new = old;
        new[2] = 1;
        assert_eq!(run(&old, &new, 4096), vec![r(4098, 1)]);
    }

    #[test]
    fn appends_after_what_is_already_in_out() {
        let mut out = vec![r(0, 1)];
        diff(&[0], &[1], 8, &mut out);
        assert_eq!(out, vec![r(0, 1), r(8, 1)]);
    }

    #[test]
    #[should_panic(expected = "differ in length")]
    fn different_lengths_panic() {
        run(&[0; 3], &[0; 4], 0);
    }

    proptest! {
        #[test]
        fn agrees_with_the_byte_at_a_time_reference(
            old in proptest::collection::vec(any::<u8>(), 0..200),
            flips in proptest::collection::vec((any::<usize>(), any::<u8>()), 0..12),
            base in 0usize..10_000,
        ) {
            let mut new = old.clone();
            if !new.is_empty() {
                for (i, v) in flips {
                    let n = new.len();
                    new[i % n] = v;
                }
            }
            prop_assert_eq!(run(&old, &new, base), naive(&old, &new, base));
        }
    }
}
