//! Page-fault write trap (spike). One trap per process (statics); each conformance case runs in its own process.
//!
//! Handler rules: only atomics, memcpy and mprotect/VirtualProtect; no allocation, no locks;
//! errno / GetLastError saved and restored; unrelated faults chained to the previously installed handler.
#![allow(static_mut_refs)]
use std::sync::atomic::*;

pub static BASE: AtomicUsize = AtomicUsize::new(0);
pub static NPAGES: AtomicUsize = AtomicUsize::new(0);
pub static PAGE: AtomicUsize = AtomicUsize::new(0);
pub static ARMED: AtomicUsize = AtomicUsize::new(0);
static STATE: AtomicPtr<AtomicU8> = AtomicPtr::new(std::ptr::null_mut());
static CUR: AtomicPtr<AtomicU8> = AtomicPtr::new(std::ptr::null_mut());
static VALID: AtomicPtr<AtomicU8> = AtomicPtr::new(std::ptr::null_mut());
static POOL: AtomicPtr<u8> = AtomicPtr::new(std::ptr::null_mut());
pub static FAULTS: AtomicU64 = AtomicU64::new(0);
pub static HANDLER_CALLS: AtomicU64 = AtomicU64::new(0);

// test hooks (positive/negative controls)
pub static H_CLOBBER_ERRNO: AtomicBool = AtomicBool::new(false);
pub static H_NO_ERRNO_RESTORE: AtomicBool = AtomicBool::new(false);
pub static H_FAIL_PROTECT: AtomicBool = AtomicBool::new(false);
pub static H_ALLOC_IN_HANDLER: AtomicBool = AtomicBool::new(false);
pub static H_MUTATE_SKIP_PROTECT: AtomicBool = AtomicBool::new(false);

const CLEAN: u8 = 0; // protected, baseline valid or not yet copied
const COPYING: u8 = 1;
const DIRTY: u8 = 2; // writable, baseline valid
const FLUSHING: u8 = 3;

pub fn page_size() -> usize {
    #[cfg(unix)]
    unsafe {
        libc::sysconf(libc::_SC_PAGESIZE) as usize
    }
    #[cfg(windows)]
    unsafe {
        use windows_sys::Win32::System::SystemInformation::*;
        let mut si: SYSTEM_INFO = std::mem::zeroed();
        GetSystemInfo(&mut si);
        si.dwPageSize as usize
    }
}

unsafe fn os_protect(addr: usize, len: usize, rw: bool) -> bool {
    if H_FAIL_PROTECT.load(Ordering::Relaxed) {
        return false;
    }
    #[cfg(unix)]
    {
        let prot = libc::PROT_READ | if rw { libc::PROT_WRITE } else { 0 };
        libc::mprotect(addr as *mut libc::c_void, len, prot) == 0
    }
    #[cfg(windows)]
    {
        use windows_sys::Win32::System::Memory::*;
        let mut old = 0u32;
        VirtualProtect(
            addr as _,
            len,
            if rw { PAGE_READWRITE } else { PAGE_READONLY },
            &mut old,
        ) != 0
    }
}

enum Fault {
    Resume,
    NotOurs,
    Fatal,
}

#[inline]
unsafe fn arr(p: &AtomicPtr<AtomicU8>, i: usize) -> &'static AtomicU8 {
    &*p.load(Ordering::Acquire).add(i)
}

unsafe fn buf(pi: usize, which: usize) -> *mut u8 {
    POOL.load(Ordering::Acquire)
        .add((pi * 2 + which) * PAGE.load(Ordering::Acquire))
}

unsafe fn on_fault(addr: usize) -> Fault {
    let base = BASE.load(Ordering::Acquire);
    let page = PAGE.load(Ordering::Acquire);
    let armed = ARMED.load(Ordering::Acquire);
    if base == 0 || addr < base || addr >= base + armed * page {
        return Fault::NotOurs;
    }
    HANDLER_CALLS.fetch_add(1, Ordering::Relaxed);
    if H_ALLOC_IN_HANDLER.load(Ordering::Relaxed) {
        // NEGATIVE CONTROL: violates the handler rule on purpose.
        let b = Box::new(7u8);
        std::hint::black_box(&b);
    }
    let pi = (addr - base) / page;
    let st = arr(&STATE, pi);
    loop {
        match st.load(Ordering::Acquire) {
            CLEAN => {
                if st
                    .compare_exchange(CLEAN, COPYING, Ordering::AcqRel, Ordering::Acquire)
                    .is_ok()
                {
                    if arr(&VALID, pi).load(Ordering::Acquire) == 0 {
                        let cur = arr(&CUR, pi).load(Ordering::Acquire) as usize;
                        std::ptr::copy_nonoverlapping(
                            (base + pi * page) as *const u8,
                            buf(pi, cur),
                            page,
                        );
                        arr(&VALID, pi).store(1, Ordering::Release);
                    }
                    if !os_protect(base + pi * page, page, true) {
                        st.store(CLEAN, Ordering::Release);
                        return Fault::Fatal;
                    }
                    FAULTS.fetch_add(1, Ordering::Relaxed);
                    st.store(DIRTY, Ordering::Release);
                    return Fault::Resume;
                }
            }
            COPYING | FLUSHING => std::hint::spin_loop(),
            _ => return Fault::Resume, // already writable: another thread won the race
        }
    }
}

// ---------------------------------------------------------------- unix
#[cfg(unix)]
mod os {
    use super::*;
    pub static mut OLD: [std::mem::MaybeUninit<libc::sigaction>; 2] = [
        std::mem::MaybeUninit::uninit(),
        std::mem::MaybeUninit::uninit(),
    ];
    pub static INSTALLED: AtomicBool = AtomicBool::new(false);
    pub const SIGS: [libc::c_int; 2] = [libc::SIGSEGV, libc::SIGBUS];

    #[cfg(any(target_os = "linux", target_os = "android"))]
    pub unsafe fn si_addr(i: *mut libc::siginfo_t) -> usize {
        (*i).si_addr() as usize
    }
    #[cfg(not(any(target_os = "linux", target_os = "android")))]
    pub unsafe fn si_addr(i: *mut libc::siginfo_t) -> usize {
        (*i).si_addr as usize
    }

    pub unsafe fn errno_ptr() -> *mut libc::c_int {
        #[cfg(any(target_os = "linux"))]
        return libc::__errno_location();
        #[cfg(target_os = "android")]
        return libc::__errno();
        #[cfg(not(any(target_os = "linux", target_os = "android")))]
        return libc::__error();
    }

    pub unsafe fn write_msg(m: &[u8]) {
        libc::write(2, m.as_ptr() as *const libc::c_void, m.len());
    }

    pub unsafe fn chain(
        old: &libc::sigaction,
        sig: libc::c_int,
        info: *mut libc::siginfo_t,
        ctx: *mut libc::c_void,
    ) {
        let h = old.sa_sigaction;
        if h == libc::SIG_DFL || h == libc::SIG_IGN {
            libc::signal(sig, libc::SIG_DFL); // re-executed faulting insn then dies with the default action
            return;
        }
        if old.sa_flags & libc::SA_SIGINFO != 0 {
            let f: unsafe extern "C" fn(libc::c_int, *mut libc::siginfo_t, *mut libc::c_void) =
                std::mem::transmute(h);
            f(sig, info, ctx);
        } else {
            let f: unsafe extern "C" fn(libc::c_int) = std::mem::transmute(h);
            f(sig);
        }
    }

    pub unsafe extern "C" fn handler(
        sig: libc::c_int,
        info: *mut libc::siginfo_t,
        ctx: *mut libc::c_void,
    ) {
        let ep = errno_ptr();
        let saved = *ep;
        let r = on_fault(si_addr(info));
        if H_CLOBBER_ERRNO.load(Ordering::Relaxed) {
            *ep = libc::EIO;
        }
        if !H_NO_ERRNO_RESTORE.load(Ordering::Relaxed) {
            *ep = saved;
        }
        match r {
            Fault::Resume => {}
            Fault::Fatal => {
                write_msg(b"trap: protect failed in handler, dying loudly\n");
                libc::signal(sig, libc::SIG_DFL);
            }
            Fault::NotOurs => {
                let i = if sig == libc::SIGSEGV { 0 } else { 1 };
                chain(OLD[i].assume_init_ref(), sig, info, ctx);
            }
        }
    }

    pub fn install() {
        if INSTALLED.swap(true, Ordering::SeqCst) {
            return;
        }
        unsafe {
            for (i, s) in SIGS.iter().enumerate() {
                let mut sa: libc::sigaction = std::mem::zeroed();
                sa.sa_sigaction = handler as *const () as usize;
                sa.sa_flags = libc::SA_SIGINFO | libc::SA_NODEFER | libc::SA_ONSTACK;
                libc::sigemptyset(&mut sa.sa_mask);
                assert_eq!(libc::sigaction(*s, &sa, OLD[i].as_mut_ptr()), 0);
            }
        }
    }
}

// ---------------------------------------------------------------- windows
#[cfg(windows)]
mod os {
    use super::*;
    use windows_sys::Win32::Foundation::{GetLastError, SetLastError};
    use windows_sys::Win32::System::Diagnostics::Debug::*;
    pub static INSTALLED: AtomicBool = AtomicBool::new(false);

    unsafe extern "system" fn veh(info: *mut EXCEPTION_POINTERS) -> i32 {
        let rec = (*info).ExceptionRecord;
        if (*rec).ExceptionCode != 0xC0000005u32 as i32 || (*rec).ExceptionInformation[0] != 1 {
            return 0;
        }
        let saved = GetLastError();
        let r = on_fault((*rec).ExceptionInformation[1]);
        if H_CLOBBER_ERRNO.load(Ordering::Relaxed) {
            SetLastError(5);
        }
        if !H_NO_ERRNO_RESTORE.load(Ordering::Relaxed) {
            SetLastError(saved);
        }
        match r {
            Fault::Resume => -1,
            Fault::NotOurs | Fault::Fatal => 0,
        }
    }

    pub fn install() {
        if INSTALLED.swap(true, Ordering::SeqCst) {
            return;
        }
        unsafe {
            assert!(!AddVectoredExceptionHandler(1, Some(veh)).is_null());
        }
    }
    pub fn last_error() -> u32 {
        unsafe { GetLastError() }
    }
    pub fn set_last_error(v: u32) {
        unsafe { SetLastError(v) }
    }
}

#[cfg(unix)]
pub use os::chain as chain_pub;
pub use os::install;
#[cfg(unix)]
pub use os::{errno_ptr as errno_ptr_pub, si_addr as si_addr_pub, SIGS as SIGS_PUB};
#[cfg(windows)]
pub use os::{last_error, set_last_error};

/// Arms the first `min(npages, pool_pages/2)` pages of the region (2 baseline buffers per armed page).
/// Remaining pages stay writable and untracked (caller must scan them). Returns the armed page count.
pub fn arm(base: *mut u8, len: usize, pool_pages: usize) -> usize {
    let page = page_size();
    assert!(
        len % page == 0 && (base as usize) % page == 0,
        "region must be page aligned"
    );
    let np = len / page;
    let armed = np.min(pool_pages / 2);
    let mk = |v: u8| -> *mut AtomicU8 {
        Box::leak(
            (0..np.max(1))
                .map(|_| AtomicU8::new(v))
                .collect::<Vec<_>>()
                .into_boxed_slice(),
        )
        .as_mut_ptr()
    };
    STATE.store(mk(CLEAN), Ordering::Release);
    CUR.store(mk(0), Ordering::Release);
    VALID.store(mk(0), Ordering::Release);
    let pool: &'static mut [u8] =
        Box::leak(vec![0u8; (pool_pages.max(2)) * page].into_boxed_slice());
    POOL.store(pool.as_mut_ptr(), Ordering::Release);
    PAGE.store(page, Ordering::Release);
    NPAGES.store(np, Ordering::Release);
    ARMED.store(armed, Ordering::Release);
    BASE.store(base as usize, Ordering::Release);
    install();
    if armed > 0 {
        assert!(
            unsafe { os_protect(base as usize, armed * page, false) },
            "initial protect failed"
        );
    }
    armed
}

pub struct Flusher {
    dirty: Vec<usize>,
}
pub struct Range {
    pub off: usize,
    pub bytes: Vec<u8>,
}

impl Flusher {
    pub fn new() -> Self {
        Flusher {
            dirty: Vec::with_capacity(ARMED.load(Ordering::Acquire).max(1)),
        }
    }

    /// Re-protect dirty pages, snapshot them (writers stall only for protect+memcpy), then diff off the hot path.
    /// The stall window performs NO allocation and takes NO locks (a writer parked in a handler may hold any).
    pub fn flush(&mut self) -> Vec<Range> {
        let base = BASE.load(Ordering::Acquire);
        let page = PAGE.load(Ordering::Acquire);
        let armed = ARMED.load(Ordering::Acquire);
        self.dirty.clear();
        unsafe {
            for p in 0..armed {
                if arr(&STATE, p)
                    .compare_exchange(DIRTY, FLUSHING, Ordering::AcqRel, Ordering::Acquire)
                    .is_ok()
                {
                    self.dirty.push(p); // capacity reserved: no allocation
                }
            }
            let mut i = 0;
            while i < self.dirty.len() {
                let mut j = i;
                while j + 1 < self.dirty.len() && self.dirty[j + 1] == self.dirty[j] + 1 {
                    j += 1;
                }
                if !H_MUTATE_SKIP_PROTECT.load(Ordering::Relaxed) {
                    assert!(
                        os_protect_nofail(base + self.dirty[i] * page, (j - i + 1) * page),
                        "flush protect failed"
                    );
                }
                i = j + 1;
            }
            for &p in &self.dirty {
                let cur = arr(&CUR, p).load(Ordering::Acquire) as usize;
                std::ptr::copy_nonoverlapping(
                    (base + p * page) as *const u8,
                    buf(p, 1 - cur),
                    page,
                );
                arr(&STATE, p).store(CLEAN, Ordering::Release); // writers released; page is RO with a valid baseline
            }
            // ---- stall window over; allocation is fine from here
            let mut out: Vec<Range> = vec![];
            for &p in &self.dirty {
                let cur = arr(&CUR, p).load(Ordering::Acquire) as usize;
                let old = std::slice::from_raw_parts(buf(p, cur), page);
                let new = std::slice::from_raw_parts(buf(p, 1 - cur), page);
                let mut k = 0;
                while k < page {
                    if k % 8 == 0 && old[k..k + 8] == new[k..k + 8] {
                        k += 8;
                        continue;
                    }
                    if old[k] != new[k] {
                        let off = p * page + k;
                        match out.last_mut() {
                            Some(r) if r.off + r.bytes.len() == off => r.bytes.push(new[k]),
                            _ => out.push(Range {
                                off,
                                bytes: vec![new[k]],
                            }),
                        }
                    }
                    k += 1;
                }
                arr(&CUR, p).store((1 - cur) as u8, Ordering::Release);
            }
            out
        }
    }
}

unsafe fn os_protect_nofail(addr: usize, len: usize) -> bool {
    // flush path ignores the failure hook (the hook targets the handler path)
    let h = H_FAIL_PROTECT.swap(false, Ordering::SeqCst);
    let r = os_protect(addr, len, false);
    H_FAIL_PROTECT.store(h, Ordering::SeqCst);
    r
}

pub fn states_summary() -> (usize, usize, usize) {
    // (clean, dirty-or-busy, armed)
    let armed = ARMED.load(Ordering::Acquire);
    let mut clean = 0;
    for p in 0..armed {
        if unsafe { arr(&STATE, p) }.load(Ordering::Acquire) == CLEAN {
            clean += 1;
        }
    }
    (clean, armed - clean, armed)
}

pub fn disarm_all() {
    let base = BASE.load(Ordering::Acquire);
    let page = PAGE.load(Ordering::Acquire);
    let np = NPAGES.load(Ordering::Acquire);
    unsafe { os_protect(base, np * page, true) };
}
