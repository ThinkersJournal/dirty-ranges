//! Operating-system primitives. Everything `unsafe` about talking to the OS lives under this module.

#[cfg(unix)]
mod unix;
#[cfg(unix)]
pub(crate) use unix::{fault_signals_blocked, install_handler, page_size, protect};
