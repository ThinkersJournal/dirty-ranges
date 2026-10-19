//! Operating-system primitives. Everything `unsafe` about talking to the OS lives under this module.

#[cfg(unix)]
mod unix;
#[cfg(unix)]
pub(crate) use unix::{blocked_fault_signals, install_handler, page_size, protect};
#[cfg(windows)]
mod windows;
#[cfg(windows)]
pub(crate) use windows::{install_handler, page_size, protect};
