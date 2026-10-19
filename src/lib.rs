//! Detect which byte ranges of a memory region changed (first real release pending).
//!
//! The design is in `docs/DESIGN.md`. Safe core: range coalescing, the byte diff, the per-page state machine
//! and scan mode over byte slices. On Unix, `Region` adds page-fault trapping; Windows is not implemented yet.
#![deny(unsafe_code)]
#![deny(unsafe_op_in_unsafe_fn)]

mod diff;
mod page_state;
mod range;
#[cfg(any(unix, windows))]
#[expect(
    unsafe_code,
    reason = "page-fault trapping needs signal handlers and raw pointers; see docs/DESIGN.md"
)]
mod region;
mod scan;
#[cfg(any(unix, windows))]
#[expect(
    unsafe_code,
    reason = "OS primitives: mprotect, signal handlers, raw pointers; see docs/DESIGN.md"
)]
mod sys;

/// Test hooks for the conformance suite: switches that make the crate misbehave on purpose, so each
/// conformance case can show it fails when the property it checks is broken. Compiled only with
/// `RUSTFLAGS="--cfg dirty_ranges_conformance"`; never part of a normal build.
#[cfg(all(any(unix, windows), dirty_ranges_conformance))]
#[doc(hidden)]
pub mod conformance_hooks {
    use std::sync::atomic::AtomicBool;
    /// The handler allocates (breaks the no-allocation rule).
    pub static ALLOC_IN_HANDLER: AtomicBool = AtomicBool::new(false);
    /// The handler sets `errno` to `EIO` before returning.
    pub static CLOBBER_ERRNO: AtomicBool = AtomicBool::new(false);
    /// The handler does not restore `errno`.
    pub static NO_ERRNO_RESTORE: AtomicBool = AtomicBool::new(false);
    /// The handler's `mprotect` is reported as failed.
    pub static FAIL_PROTECT: AtomicBool = AtomicBool::new(false);
    /// `flush` skips re-protecting the pages it claimed.
    pub static SKIP_FLUSH_PROTECT: AtomicBool = AtomicBool::new(false);
}

pub use diff::diff;
pub use page_state::{PageState, State};
pub use range::{Range, coalesce};
pub use scan::{ScanError, Scanner};

#[cfg(unix)]
pub use region::fault_signals_blocked;
#[cfg(any(unix, windows))]
pub use region::{Error, Mode, Region};
