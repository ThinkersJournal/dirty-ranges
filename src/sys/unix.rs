//! Unix: `mprotect` plus a `SIGSEGV`/`SIGBUS` handler (macOS and the BSDs raise `SIGBUS` for a write to a
//! read-only page).

use crate::region::{Fault, on_fault};
use std::cell::UnsafeCell;
use std::mem::MaybeUninit;
use std::sync::Once;
use std::sync::atomic::{AtomicI32, Ordering};

const SIGNALS: [libc::c_int; 2] = [libc::SIGSEGV, libc::SIGBUS];

/// The handlers that were installed before ours, one per entry of `SIGNALS`.
struct Previous(UnsafeCell<[MaybeUninit<libc::sigaction>; 2]>);

// SAFETY: written only inside `INSTALL`'s `call_once`, before any fault can reach our handler, and read-only
// afterwards.
unsafe impl Sync for Previous {}

static PREVIOUS: Previous = Previous(UnsafeCell::new([
    MaybeUninit::uninit(),
    MaybeUninit::uninit(),
]));
static INSTALL: Once = Once::new();
/// `0` if installation succeeded, otherwise the `errno` it failed with.
static INSTALL_ERRNO: AtomicI32 = AtomicI32::new(0);

pub(crate) fn page_size() -> usize {
    // SAFETY: `sysconf` has no preconditions.
    unsafe { libc::sysconf(libc::_SC_PAGESIZE) as usize }
}

/// Make `len` bytes at `addr` read-only, or read-write if `writable`. Async-signal-safe.
///
/// # Safety
/// `addr` must be page-aligned and `addr..addr+len` must be mapped memory the caller may re-protect.
pub(crate) unsafe fn protect(addr: usize, len: usize, writable: bool) -> bool {
    let prot = libc::PROT_READ | if writable { libc::PROT_WRITE } else { 0 };
    // SAFETY: the caller guarantees the range; `mprotect` is async-signal-safe.
    unsafe { libc::mprotect(addr as *mut libc::c_void, len, prot) == 0 }
}

/// Install the process-wide fault handler once. Later calls report the first call's outcome.
pub(crate) fn install_handler() -> std::io::Result<()> {
    INSTALL.call_once(|| {
        // SAFETY: `sigaction` is given a zeroed, then filled, `sigaction` and a valid out-pointer into
        // `PREVIOUS`; this runs once, before our handler can observe `PREVIOUS`.
        unsafe {
            let previous = PREVIOUS.0.get().cast::<MaybeUninit<libc::sigaction>>();
            for (i, &sig) in SIGNALS.iter().enumerate() {
                let mut action: libc::sigaction = std::mem::zeroed();
                action.sa_sigaction = handler as *const () as usize;
                action.sa_flags = libc::SA_SIGINFO | libc::SA_NODEFER | libc::SA_ONSTACK;
                libc::sigemptyset(&mut action.sa_mask);
                if libc::sigaction(sig, &action, (*previous.add(i)).as_mut_ptr()) != 0 {
                    INSTALL_ERRNO.store(
                        std::io::Error::last_os_error()
                            .raw_os_error()
                            .unwrap_or(libc::EINVAL),
                        Ordering::Release,
                    );
                    // Put back what we already replaced, so a failed install leaves nothing behind.
                    for (j, &done) in SIGNALS.iter().enumerate().take(i) {
                        libc::sigaction(done, (*previous.add(j)).as_ptr(), std::ptr::null_mut());
                    }
                    return;
                }
            }
        }
    });
    match INSTALL_ERRNO.load(Ordering::Acquire) {
        0 => Ok(()),
        errno => Err(std::io::Error::from_raw_os_error(errno)),
    }
}

#[cfg(any(target_os = "linux", target_os = "android"))]
unsafe fn fault_address(info: *mut libc::siginfo_t) -> usize {
    // SAFETY: the kernel passes a valid `siginfo_t` to an `SA_SIGINFO` handler.
    unsafe { (*info).si_addr() as usize }
}

#[cfg(not(any(target_os = "linux", target_os = "android")))]
unsafe fn fault_address(info: *mut libc::siginfo_t) -> usize {
    // SAFETY: the kernel passes a valid `siginfo_t` to an `SA_SIGINFO` handler.
    unsafe { (*info).si_addr as usize }
}

unsafe fn errno_location() -> *mut libc::c_int {
    // SAFETY: each of these returns the calling thread's `errno`.
    unsafe {
        #[cfg(target_os = "linux")]
        return libc::__errno_location();
        #[cfg(target_os = "android")]
        return libc::__errno();
        #[cfg(any(
            target_os = "macos",
            target_os = "ios",
            target_os = "freebsd",
            target_os = "dragonfly"
        ))]
        return libc::__error();
        #[cfg(any(target_os = "netbsd", target_os = "openbsd"))]
        return libc::__errno();
    }
}

/// Hand a fault that is not ours to whoever was installed before us.
unsafe fn chain(
    sig: libc::c_int,
    index: usize,
    info: *mut libc::siginfo_t,
    ctx: *mut libc::c_void,
) {
    // SAFETY: `PREVIOUS` was filled before our handler could run.
    let old = unsafe { (*PREVIOUS.0.get())[index].assume_init_ref() };
    let h = old.sa_sigaction;
    // SAFETY: calling the previous handler with the arguments we were given, in the form it registered for.
    unsafe {
        if h == libc::SIG_DFL || h == libc::SIG_IGN {
            // Restore the default action and return: the faulting instruction re-executes and the process
            // dies with the signal it would have died with without us.
            libc::signal(sig, libc::SIG_DFL);
        } else if old.sa_flags & libc::SA_SIGINFO != 0 {
            let f: unsafe extern "C" fn(libc::c_int, *mut libc::siginfo_t, *mut libc::c_void) =
                std::mem::transmute(h);
            f(sig, info, ctx);
        } else {
            let f: unsafe extern "C" fn(libc::c_int) = std::mem::transmute(h);
            f(sig);
        }
    }
}

unsafe extern "C" fn handler(sig: libc::c_int, info: *mut libc::siginfo_t, ctx: *mut libc::c_void) {
    // SAFETY: async-signal context. `on_fault` obeys the handler rules (atomics, memcpy, mprotect only), and
    // `errno` is saved and restored around it so the interrupted code never sees it change.
    unsafe {
        let errno = errno_location();
        let saved = *errno;
        let outcome = on_fault(fault_address(info));
        *errno = saved;
        match outcome {
            Fault::Resume => {}
            Fault::Fatal => {
                const MSG: &[u8] =
                    b"dirty-ranges: mprotect failed in the fault handler, aborting\n";
                libc::write(2, MSG.as_ptr().cast(), MSG.len());
                libc::signal(sig, libc::SIG_DFL);
            }
            Fault::NotOurs => chain(sig, usize::from(sig != libc::SIGSEGV), info, ctx),
        }
    }
}
