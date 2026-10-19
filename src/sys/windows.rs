//! Windows: `VirtualProtect` plus a vectored exception handler for access violations.

use crate::region::{Fault, on_fault};
use std::ffi::c_void;
use std::sync::Once;
use std::sync::atomic::{AtomicU32, Ordering};
use windows_sys::Win32::Foundation::{GetLastError, SetLastError};
use windows_sys::Win32::System::Diagnostics::Debug::{
    AddVectoredExceptionHandler, EXCEPTION_POINTERS,
};
use windows_sys::Win32::System::Memory::{PAGE_READONLY, PAGE_READWRITE, VirtualProtect};
use windows_sys::Win32::System::SystemInformation::{GetSystemInfo, SYSTEM_INFO};

/// `STATUS_ACCESS_VIOLATION`.
const ACCESS_VIOLATION: i32 = 0xC000_0005_u32 as i32;
/// Handler results: re-execute the faulting instruction / keep looking for another handler.
const CONTINUE_EXECUTION: i32 = -1;
const CONTINUE_SEARCH: i32 = 0;

static INSTALL: Once = Once::new();
/// `0` if installation succeeded, otherwise the `GetLastError` it failed with.
static INSTALL_ERROR: AtomicU32 = AtomicU32::new(0);

pub(crate) fn page_size() -> usize {
    // SAFETY: `GetSystemInfo` fills the struct we give it and cannot fail.
    unsafe {
        let mut info: SYSTEM_INFO = std::mem::zeroed();
        GetSystemInfo(&mut info);
        info.dwPageSize as usize
    }
}

/// Make `len` bytes at `addr` read-only, or read-write if `writable`.
///
/// # Safety
/// `addr` must be page-aligned and `addr..addr+len` must be committed memory the caller may re-protect.
pub(crate) unsafe fn protect(addr: usize, len: usize, writable: bool) -> bool {
    let mut previous = 0u32;
    let protection = if writable {
        PAGE_READWRITE
    } else {
        PAGE_READONLY
    };
    // SAFETY: the caller guarantees the range; `previous` is a valid out-pointer.
    unsafe { VirtualProtect(addr as *const c_void, len, protection, &mut previous) != 0 }
}

/// Install the process-wide exception handler once. Later calls report the first call's outcome.
pub(crate) fn install_handler() -> std::io::Result<()> {
    INSTALL.call_once(|| {
        // SAFETY: `veh` has the signature Windows expects and lives for the whole process.
        let handle = unsafe { AddVectoredExceptionHandler(1, Some(veh)) };
        if handle.is_null() {
            // SAFETY: `GetLastError` has no preconditions.
            INSTALL_ERROR.store(unsafe { GetLastError() }, Ordering::Release);
        }
    });
    match INSTALL_ERROR.load(Ordering::Acquire) {
        0 => Ok(()),
        code => Err(std::io::Error::from_raw_os_error(code as i32)),
    }
}

/// Runs for every exception in the process, first in line. Allocates nothing and takes no lock.
unsafe extern "system" fn veh(info: *mut EXCEPTION_POINTERS) -> i32 {
    // SAFETY: Windows passes valid exception pointers; `ExceptionInformation[0] == 1` is a write access and
    // `[1]` is the address, for an access violation.
    unsafe {
        let record = (*info).ExceptionRecord;
        if (*record).ExceptionCode != ACCESS_VIOLATION || (*record).ExceptionInformation[0] != 1 {
            return CONTINUE_SEARCH;
        }
        let saved = GetLastError();
        let outcome = on_fault((*record).ExceptionInformation[1]);
        #[cfg(dirty_ranges_conformance)]
        {
            use crate::conformance_hooks::{CLOBBER_ERRNO, NO_ERRNO_RESTORE};
            if CLOBBER_ERRNO.load(Ordering::Relaxed) {
                // CONTROL: a handler that disturbs the last error, to show the restore below protects it.
                SetLastError(5);
            }
            if NO_ERRNO_RESTORE.load(Ordering::Relaxed) {
                return finish(outcome);
            }
        }
        SetLastError(saved);
        finish(outcome)
    }
}

fn finish(outcome: Fault) -> i32 {
    match outcome {
        Fault::Resume => CONTINUE_EXECUTION,
        // Not ours, or we could not unprotect the page: let the next handler or the system deal with it,
        // which for an unhandled access violation ends the process rather than looping.
        Fault::NotOurs | Fault::Fatal => CONTINUE_SEARCH,
    }
}
