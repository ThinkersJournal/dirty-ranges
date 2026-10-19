//! Operating-system primitives. Everything `unsafe` about talking to the OS lives under this module.

#[cfg(unix)]
mod unix;
#[cfg(unix)]
pub(crate) use unix::{blocked_fault_signals, install_handler, page_size, protect};
