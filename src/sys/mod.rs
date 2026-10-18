//! Operating-system primitives. Everything `unsafe` about talking to the OS lives under this module.

#[cfg(unix)]
mod unix;
#[cfg(unix)]
pub(crate) use unix::{install_handler, page_size, protect};
