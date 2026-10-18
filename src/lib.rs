//! Detect which byte ranges of a memory region changed (first real release pending).
//!
//! The design is in `docs/DESIGN.md`. This slice holds only the safe core: range coalescing, the byte
//! diff and the per-page state machine. There is no OS-specific code yet.
#![deny(unsafe_code)]
#![deny(unsafe_op_in_unsafe_fn)]

mod diff;
mod page_state;
mod range;
mod scan;

pub use diff::diff;
pub use page_state::{PageState, State};
pub use range::{Range, coalesce};
pub use scan::{ScanError, Scanner};
