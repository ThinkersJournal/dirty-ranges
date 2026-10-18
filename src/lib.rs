//! Detect which byte ranges of a memory region changed (first real release pending).
//!
//! The design is in `docs/DESIGN.md`. Safe core: range coalescing, the byte diff, the per-page state machine
//! and scan mode over byte slices. On Unix, `Region` adds page-fault trapping; Windows is not implemented yet.
#![deny(unsafe_code)]
#![deny(unsafe_op_in_unsafe_fn)]

mod diff;
mod page_state;
mod range;
#[cfg(unix)]
#[expect(
    unsafe_code,
    reason = "page-fault trapping needs signal handlers and raw pointers; see docs/DESIGN.md"
)]
mod region;
mod scan;
#[cfg(unix)]
#[expect(
    unsafe_code,
    reason = "OS primitives: mprotect, signal handlers, raw pointers; see docs/DESIGN.md"
)]
mod sys;

pub use diff::diff;
pub use page_state::{PageState, State};
pub use range::{Range, coalesce};
pub use scan::{ScanError, Scanner};

#[cfg(unix)]
pub use region::{Error, Region};
