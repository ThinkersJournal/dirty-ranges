// SPDX-License-Identifier: MIT OR Apache-2.0
use crate::{Range, diff};

/// A scan-mode failure.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScanError {
    /// The live region is not the length the scanner was created for.
    LengthMismatch { expected: usize, actual: usize },
    /// A remote write reaches past the end of the region.
    OutOfBounds {
        offset: usize,
        len: usize,
        region_len: usize,
    },
}

impl core::fmt::Display for ScanError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::LengthMismatch { expected, actual } => {
                write!(
                    f,
                    "live region is {actual} bytes, scanner was created for {expected}"
                )
            }
            Self::OutOfBounds {
                offset,
                len,
                region_len,
            } => {
                write!(
                    f,
                    "write of {len} bytes at {offset} is outside a {region_len}-byte region"
                )
            }
        }
    }
}

impl std::error::Error for ScanError {}

/// Scan mode over plain byte slices: keeps a baseline copy and reports what changed in the live bytes.
///
/// This holds no pointer and no OS state; the caller supplies the live bytes on each call.
#[derive(Debug)]
pub struct Scanner {
    baseline: Vec<u8>,
}

impl Scanner {
    /// Start tracking `live`; its current contents are the baseline.
    pub fn new(live: &[u8]) -> Self {
        Self {
            baseline: live.to_vec(),
        }
    }

    /// Length of the tracked region.
    pub fn len(&self) -> usize {
        self.baseline.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Report the byte ranges of `live` that differ from the baseline (sorted, coalesced), and make `live`
    /// the new baseline.
    pub fn scan(&mut self, live: &[u8]) -> Result<Vec<Range>, ScanError> {
        if live.len() != self.baseline.len() {
            return Err(ScanError::LengthMismatch {
                expected: self.baseline.len(),
                actual: live.len(),
            });
        }
        let mut changed = Vec::new();
        diff(&self.baseline, live, 0, &mut changed);
        // Only the changed runs need copying into the baseline.
        for r in &changed {
            self.baseline[r.offset..r.offset + r.len]
                .copy_from_slice(&live[r.offset..r.offset + r.len]);
        }
        Ok(changed)
    }

    /// Record that `bytes` were written at `offset` by someone else (for example a remote peer), so the next
    /// scan does not report them as local changes. The caller writes the same bytes to the live region.
    pub fn apply_remote(&mut self, offset: usize, bytes: &[u8]) -> Result<(), ScanError> {
        let out_of_bounds = ScanError::OutOfBounds {
            offset,
            len: bytes.len(),
            region_len: self.baseline.len(),
        };
        let end = offset.checked_add(bytes.len()).ok_or(out_of_bounds)?;
        self.baseline
            .get_mut(offset..end)
            .ok_or(out_of_bounds)?
            .copy_from_slice(bytes);
        Ok(())
    }
}

#[cfg(all(test, not(loom)))]
mod tests {
    use super::*;

    fn r(offset: usize, len: usize) -> Range {
        Range { offset, len }
    }

    #[test]
    fn an_untouched_region_reports_nothing() {
        let live = vec![1u8; 100];
        let mut s = Scanner::new(&live);
        assert_eq!(s.scan(&live).unwrap(), vec![]);
    }

    #[test]
    fn a_write_is_reported_once() {
        let mut live = vec![0u8; 100];
        let mut s = Scanner::new(&live);
        live[10..14].fill(5);
        assert_eq!(s.scan(&live).unwrap(), vec![r(10, 4)]);
        assert_eq!(s.scan(&live).unwrap(), vec![]);
    }

    #[test]
    fn writing_the_same_value_reports_nothing() {
        let mut live = vec![7u8; 16];
        let mut s = Scanner::new(&live);
        live[3] = 7;
        assert_eq!(s.scan(&live).unwrap(), vec![]);
    }

    #[test]
    fn a_change_that_is_reverted_before_the_scan_reports_nothing() {
        let mut live = vec![0u8; 16];
        let mut s = Scanner::new(&live);
        live[3] = 9;
        live[3] = 0;
        assert_eq!(s.scan(&live).unwrap(), vec![]);
    }

    #[test]
    fn separate_writes_give_separate_sorted_ranges() {
        let mut live = vec![0u8; 200];
        let mut s = Scanner::new(&live);
        live[150] = 1;
        live[2] = 1;
        assert_eq!(s.scan(&live).unwrap(), vec![r(2, 1), r(150, 1)]);
    }

    #[test]
    fn a_wrong_length_is_an_error_and_leaves_the_baseline_alone() {
        let live = vec![0u8; 8];
        let mut s = Scanner::new(&live);
        assert_eq!(
            s.scan(&live[..7]),
            Err(ScanError::LengthMismatch {
                expected: 8,
                actual: 7
            })
        );
        let mut changed = live.clone();
        changed[0] = 1;
        assert_eq!(s.scan(&changed).unwrap(), vec![r(0, 1)]);
    }

    #[test]
    fn a_remote_write_is_not_reported_as_a_local_change() {
        let mut live = vec![0u8; 32];
        let mut s = Scanner::new(&live);
        live[4..8].copy_from_slice(&[1, 2, 3, 4]);
        s.apply_remote(4, &[1, 2, 3, 4]).unwrap();
        assert_eq!(s.scan(&live).unwrap(), vec![]);
    }

    #[test]
    fn a_local_write_beside_a_remote_write_is_still_reported() {
        let mut live = vec![0u8; 32];
        let mut s = Scanner::new(&live);
        live[4..8].copy_from_slice(&[1, 2, 3, 4]);
        s.apply_remote(4, &[1, 2, 3, 4]).unwrap();
        live[20] = 9;
        assert_eq!(s.scan(&live).unwrap(), vec![r(20, 1)]);
    }

    #[test]
    fn a_remote_write_past_the_end_is_refused_and_changes_nothing() {
        let live = vec![0u8; 8];
        let mut s = Scanner::new(&live);
        assert_eq!(
            s.apply_remote(6, &[1, 2, 3]),
            Err(ScanError::OutOfBounds {
                offset: 6,
                len: 3,
                region_len: 8
            })
        );
        assert_eq!(s.scan(&live).unwrap(), vec![]);
    }

    #[test]
    fn a_remote_write_with_an_overflowing_offset_is_refused() {
        let live = vec![0u8; 8];
        let mut s = Scanner::new(&live);
        assert!(s.apply_remote(usize::MAX, &[1, 2]).is_err());
    }

    #[test]
    fn an_empty_region_works() {
        let mut s = Scanner::new(&[]);
        assert!(s.is_empty());
        assert_eq!(s.scan(&[]).unwrap(), vec![]);
    }

    #[test]
    fn len_and_is_empty_describe_the_region() {
        let s = Scanner::new(&[1, 2, 3]);
        assert_eq!(s.len(), 3);
        assert!(!s.is_empty());
    }

    #[test]
    fn errors_say_what_went_wrong() {
        assert_eq!(
            ScanError::LengthMismatch {
                expected: 8,
                actual: 7
            }
            .to_string(),
            "live region is 7 bytes, scanner was created for 8"
        );
        assert_eq!(
            ScanError::OutOfBounds {
                offset: 6,
                len: 3,
                region_len: 8
            }
            .to_string(),
            "write of 3 bytes at 6 is outside a 8-byte region"
        );
    }
}
