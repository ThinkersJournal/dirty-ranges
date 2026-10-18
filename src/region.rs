//! A trapped region: pages are kept read-only, the first write to each faults into a handler that copies a
//! baseline and unprotects the page, and `flush` re-protects, snapshots and diffs the pages that were written.
//!
//! The handler obeys the rules the trap-conformance harness proved (see `docs/DESIGN.md`): only atomics,
//! `memcpy` and `mprotect`; no allocation and no locks; `errno` preserved; unrelated faults chained.

use crate::page_state::{PageState, State};
use crate::{Range, coalesce, diff, sys};
use std::ptr;
use std::sync::atomic::{AtomicBool, AtomicPtr, AtomicU8, AtomicUsize, Ordering};

/// Why a region could not be armed.
#[derive(Debug)]
pub enum Error {
    /// The pointer or length is not a whole number of OS pages.
    Misaligned { page_size: usize },
    /// The process already tracks the maximum number of regions.
    RegistryFull,
    /// The calling thread blocks `SIGSEGV`/`SIGBUS`. A write to a trapped page from such a thread is fatal
    /// (the kernel kills the process), so refusing here is the safe answer.
    SignalsBlocked { segv: bool, bus: bool },
    /// An operating-system call failed.
    Os(std::io::Error),
}

impl core::fmt::Display for Error {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Misaligned { page_size } => {
                write!(
                    f,
                    "region must start and end on a {page_size}-byte page boundary"
                )
            }
            Self::RegistryFull => write!(
                f,
                "the process already tracks the maximum number of regions"
            ),
            Self::SignalsBlocked { segv, bus } => write!(
                f,
                "the calling thread blocks SIGSEGV={segv} SIGBUS={bus}; a write to a trapped page from it would kill the process"
            ),
            Self::Os(e) => write!(f, "operating system error: {e}"),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Os(e) => Some(e),
            _ => None,
        }
    }
}

/// What the fault handler decided about a fault.
pub(crate) enum Fault {
    /// The page is now writable: re-execute the faulting instruction.
    Resume,
    /// Not in any region of ours: pass it on.
    NotOurs,
    /// The page could not be unprotected: die rather than loop.
    Fatal,
}

/// Regions a process may track at once. A fixed table, so the handler can scan it without a lock.
const MAX_REGIONS: usize = 64;

static SLOTS: [AtomicPtr<Inner>; MAX_REGIONS] =
    [const { AtomicPtr::new(ptr::null_mut()) }; MAX_REGIONS];
/// Handlers currently looking at each slot's region; `Drop` waits for this to reach zero before freeing.
static ACTIVE: [AtomicUsize; MAX_REGIONS] = [const { AtomicUsize::new(0) }; MAX_REGIONS];

/// Everything the handler touches. Never moved or freed while registered in `SLOTS`.
struct Inner {
    base: usize,
    page: usize,
    armed: usize,
    state: Box<[PageState]>,
    /// Which of a page's two baseline buffers currently holds the baseline.
    current: Box<[AtomicU8]>,
    /// Whether the page's baseline has been captured yet (it is taken lazily, at the first fault).
    valid: Box<[AtomicBool]>,
    /// `armed * 2` page-sized buffers.
    pool: *mut u8,
    pool_len: usize,
}

impl Inner {
    /// # Safety
    /// `page_index < armed` and `which` is 0 or 1.
    unsafe fn buffer(&self, page_index: usize, which: usize) -> *mut u8 {
        // SAFETY: in bounds of the `armed * 2 * page` pool by the caller's contract.
        unsafe { self.pool.add((page_index * 2 + which) * self.page) }
    }

    fn live(&self, page_index: usize) -> *mut u8 {
        (self.base + page_index * self.page) as *mut u8
    }
}

impl Drop for Inner {
    fn drop(&mut self) {
        // SAFETY: `pool`/`pool_len` came from `Box::into_raw` of a boxed slice of exactly that length.
        drop(unsafe { Box::from_raw(ptr::slice_from_raw_parts_mut(self.pool, self.pool_len)) });
    }
}

/// Called from the signal handler / vectored exception handler. Allocates nothing and takes no lock.
pub(crate) fn on_fault(addr: usize) -> Fault {
    for slot in 0..MAX_REGIONS {
        let seen = SLOTS[slot].load(Ordering::Acquire);
        if seen.is_null() {
            continue;
        }
        // Announce ourselves, then re-read: if the slot still holds the same region, `Drop` will wait for us.
        ACTIVE[slot].fetch_add(1, Ordering::SeqCst);
        let again = SLOTS[slot].load(Ordering::SeqCst);
        // SAFETY: `again == seen` and non-null, and the slot is cleared before the region is freed and
        // `Drop` then waits for `ACTIVE[slot]` to reach zero, so the pointee outlives this use.
        let outcome = if again == seen {
            unsafe { handle(&*again, addr) }
        } else {
            None
        };
        ACTIVE[slot].fetch_sub(1, Ordering::Release);
        if let Some(fault) = outcome {
            return fault;
        }
    }
    Fault::NotOurs
}

/// # Safety
/// Handler context; `inner`'s pages and buffers are valid for the call.
unsafe fn handle(inner: &Inner, addr: usize) -> Option<Fault> {
    if addr < inner.base || addr >= inner.base + inner.armed * inner.page {
        return None;
    }
    #[cfg(dirty_ranges_conformance)]
    if crate::conformance_hooks::ALLOC_IN_HANDLER.load(Ordering::Relaxed) {
        // CONTROL: break the "handler never allocates" rule on purpose.
        std::hint::black_box(Box::new(7u8));
    }
    let pi = (addr - inner.base) / inner.page;
    let state = &inner.state[pi];
    loop {
        match state.load() {
            State::Clean => {
                if !state.begin_capture() {
                    continue;
                }
                if !inner.valid[pi].load(Ordering::Acquire) {
                    let cur = usize::from(inner.current[pi].load(Ordering::Acquire));
                    // SAFETY: `pi < armed`, `cur` is 0 or 1; the page is read-only so nothing writes it now.
                    unsafe {
                        ptr::copy_nonoverlapping(inner.live(pi), inner.buffer(pi, cur), inner.page)
                    };
                    inner.valid[pi].store(true, Ordering::Release);
                }
                #[cfg(dirty_ranges_conformance)]
                let forced_failure = crate::conformance_hooks::FAIL_PROTECT.load(Ordering::Relaxed);
                #[cfg(not(dirty_ranges_conformance))]
                let forced_failure = false;
                // SAFETY: a whole armed page of the caller's mapping.
                if forced_failure
                    || !unsafe { sys::protect(inner.live(pi) as usize, inner.page, true) }
                {
                    state.abort_capture();
                    return Some(Fault::Fatal);
                }
                state.finish_capture();
                return Some(Fault::Resume);
            }
            // Another thread is capturing or the flusher is snapshotting: wait without a lock.
            State::Copying | State::Flushing => std::hint::spin_loop(),
            // Already writable: another thread won the race.
            State::Dirty => return Some(Fault::Resume),
        }
    }
}

/// A memory region whose writes are trapped page by page.
pub struct Region {
    inner: *mut Inner,
    slot: usize,
    /// Pages claimed by the current flush. Reserved at arm time so the flush's stall window never allocates.
    dirty: Vec<usize>,
}

// SAFETY: all shared state is atomics; `flush` needs `&mut`, so one flusher at a time.
unsafe impl Send for Region {}
unsafe impl Sync for Region {}

impl Region {
    /// Arm `len` bytes at `ptr`, tracking up to `pool_pages / 2` pages (two baseline buffers per page).
    /// Pages beyond that stay writable and untracked; the caller must scan them (see [`Region::armed_pages`]).
    ///
    /// # Safety
    /// `ptr..ptr+len` must be a live mapping that stays valid until the `Region` is dropped, and nothing may
    /// read from the OS into it (`read`, `recv`) while armed: the kernel reports `EFAULT` instead of faulting.
    pub unsafe fn arm(ptr: *mut u8, len: usize, pool_pages: usize) -> Result<Region, Error> {
        let (segv, bus) = sys::blocked_fault_signals();
        if segv || bus {
            return Err(Error::SignalsBlocked { segv, bus });
        }
        let page = sys::page_size();
        if !(ptr as usize).is_multiple_of(page) || !len.is_multiple_of(page) {
            return Err(Error::Misaligned { page_size: page });
        }
        let armed = (len / page).min(pool_pages / 2);
        let pool_len = armed * 2 * page;
        let pool = Box::into_raw(vec![0u8; pool_len].into_boxed_slice()).cast::<u8>();
        let inner = Box::into_raw(Box::new(Inner {
            base: ptr as usize,
            page,
            armed,
            state: (0..armed).map(|_| PageState::new()).collect(),
            current: (0..armed).map(|_| AtomicU8::new(0)).collect(),
            valid: (0..armed).map(|_| AtomicBool::new(false)).collect(),
            pool,
            pool_len,
        }));
        // From here `Drop` frees everything, whichever way this returns.
        let mut region = Region {
            inner,
            slot: usize::MAX,
            dirty: Vec::with_capacity(armed),
        };
        sys::install_handler().map_err(Error::Os)?;
        let slot = SLOTS
            .iter()
            .position(|s| {
                s.compare_exchange(ptr::null_mut(), inner, Ordering::SeqCst, Ordering::SeqCst)
                    .is_ok()
            })
            .ok_or(Error::RegistryFull)?;
        region.slot = slot;
        // SAFETY: the caller's mapping, page-aligned and whole pages, now registered so faults find it.
        if armed > 0 && !unsafe { sys::protect(ptr as usize, armed * page, false) } {
            return Err(Error::Os(std::io::Error::last_os_error()));
        }
        Ok(region)
    }

    fn inner(&self) -> &Inner {
        // SAFETY: allocated in `arm`, freed only in `Drop`.
        unsafe { &*self.inner }
    }

    /// How many pages are tracked. Pages from here to the end of the region are not.
    pub fn armed_pages(&self) -> usize {
        self.inner().armed
    }

    /// The OS page size the region was armed with.
    pub fn page_size(&self) -> usize {
        self.inner().page
    }

    /// Report the byte ranges written since the last flush (sorted, coalesced, byte-exact) and re-arm.
    pub fn flush(&mut self) -> Vec<Range> {
        // SAFETY: allocated in `arm`, freed only in `Drop`; a separate borrow so `self.dirty` stays usable.
        let inner = unsafe { &*self.inner };
        let page = inner.page;
        self.dirty.clear();
        // ---- stall window: writers that fault now spin until their page is released. No allocation, no lock.
        for p in 0..inner.armed {
            if inner.state[p].begin_flush() {
                self.dirty.push(p); // capacity was reserved in `arm`
            }
        }
        let mut i = 0;
        while i < self.dirty.len() {
            let mut j = i;
            while j + 1 < self.dirty.len() && self.dirty[j + 1] == self.dirty[j] + 1 {
                j += 1;
            }
            #[cfg(dirty_ranges_conformance)]
            if crate::conformance_hooks::SKIP_FLUSH_PROTECT.load(Ordering::Relaxed) {
                // CONTROL: claim the pages but never re-protect them, so writers race the snapshot.
                i = j + 1;
                continue;
            }
            // SAFETY: whole armed pages of the caller's mapping.
            if !unsafe {
                sys::protect(
                    inner.live(self.dirty[i]) as usize,
                    (j - i + 1) * page,
                    false,
                )
            } {
                // Pages are claimed and writers may be spinning on them: carrying on would hang them.
                eprintln!("dirty-ranges: mprotect failed while flushing, aborting");
                std::process::abort();
            }
            i = j + 1;
        }
        for &p in &self.dirty {
            let cur = usize::from(inner.current[p].load(Ordering::Acquire));
            // SAFETY: `p < armed`; the page is read-only now, so the copy is a consistent snapshot.
            unsafe { ptr::copy_nonoverlapping(inner.live(p), inner.buffer(p, 1 - cur), page) };
            inner.state[p].finish_flush(); // writers may capture again; the page is protected with a valid baseline
        }
        // ---- stall window over: allocating is fine from here.
        let mut out = Vec::new();
        for &p in &self.dirty {
            let cur = usize::from(inner.current[p].load(Ordering::Acquire));
            // SAFETY: two distinct page-sized buffers inside the pool; only this flusher reads them.
            let (old, new) = unsafe {
                (
                    std::slice::from_raw_parts(inner.buffer(p, cur), page),
                    std::slice::from_raw_parts(inner.buffer(p, 1 - cur), page),
                )
            };
            diff(old, new, p * page, &mut out);
            inner.current[p].store((1 - cur) as u8, Ordering::Release); // the snapshot is the next baseline
        }
        coalesce(&mut out);
        out
    }
}

impl Drop for Region {
    fn drop(&mut self) {
        let inner = self.inner();
        if inner.armed > 0 {
            // SAFETY: the caller's mapping outlives the region by `arm`'s contract. Failure leaves pages
            // read-only, which is no worse than not having tried.
            unsafe { sys::protect(inner.base, inner.armed * inner.page, true) };
        }
        if self.slot != usize::MAX {
            SLOTS[self.slot].store(ptr::null_mut(), Ordering::SeqCst);
            while ACTIVE[self.slot].load(Ordering::SeqCst) != 0 {
                std::hint::spin_loop();
            }
        }
        // SAFETY: allocated by `Box::into_raw` in `arm`, no longer reachable from `SLOTS` or any handler.
        drop(unsafe { Box::from_raw(self.inner) });
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    /// An anonymous mapping standing in for a mapped file.
    struct Map {
        ptr: *mut u8,
        len: usize,
    }
    unsafe impl Send for Map {}
    unsafe impl Sync for Map {}

    fn os_page() -> usize {
        unsafe { libc::sysconf(libc::_SC_PAGESIZE) as usize }
    }

    impl Map {
        fn new(pages: usize) -> Map {
            let len = pages * os_page();
            let ptr = unsafe {
                libc::mmap(
                    std::ptr::null_mut(),
                    len,
                    libc::PROT_READ | libc::PROT_WRITE,
                    libc::MAP_PRIVATE | libc::MAP_ANON,
                    -1,
                    0,
                )
            };
            assert_ne!(ptr, libc::MAP_FAILED);
            Map {
                ptr: ptr as *mut u8,
                len,
            }
        }
        fn page(&self) -> usize {
            os_page()
        }
        fn write(&self, offset: usize, v: u8) {
            assert!(offset < self.len);
            unsafe { self.ptr.add(offset).write_volatile(v) };
        }
        fn read(&self, offset: usize) -> u8 {
            unsafe { self.ptr.add(offset).read_volatile() }
        }
        fn arm(&self, pool_pages: usize) -> Region {
            unsafe { Region::arm(self.ptr, self.len, pool_pages) }.unwrap()
        }
    }
    impl Drop for Map {
        fn drop(&mut self) {
            unsafe { libc::munmap(self.ptr as *mut _, self.len) };
        }
    }

    fn r(offset: usize, len: usize) -> Range {
        Range { offset, len }
    }

    #[test]
    fn an_untouched_region_reports_nothing() {
        let m = Map::new(4);
        let mut region = m.arm(8);
        assert_eq!(region.flush(), vec![]);
    }

    #[test]
    fn one_written_byte_is_reported_once() {
        let m = Map::new(4);
        let mut region = m.arm(8);
        m.write(10, 1);
        assert_eq!(region.flush(), vec![r(10, 1)]);
        assert_eq!(region.flush(), vec![]);
    }

    #[test]
    fn a_write_after_a_flush_is_trapped_again() {
        let m = Map::new(4);
        let mut region = m.arm(8);
        m.write(10, 1);
        region.flush();
        m.write(10, 2);
        m.write(20, 3);
        assert_eq!(region.flush(), vec![r(10, 1), r(20, 1)]);
    }

    #[test]
    fn storing_the_value_already_there_reports_nothing() {
        let m = Map::new(2);
        m.write(5, 7);
        let mut region = m.arm(4);
        m.write(5, 7);
        assert_eq!(region.flush(), vec![]);
    }

    #[test]
    fn a_write_that_is_undone_before_the_flush_reports_nothing() {
        let m = Map::new(2);
        let mut region = m.arm(4);
        m.write(5, 9);
        m.write(5, 0);
        assert_eq!(region.flush(), vec![]);
    }

    #[test]
    fn writes_in_different_pages_give_separate_ranges() {
        let m = Map::new(4);
        let page = m.page();
        let mut region = m.arm(8);
        m.write(page * 2 + 3, 1);
        m.write(1, 1);
        assert_eq!(region.flush(), vec![r(1, 1), r(page * 2 + 3, 1)]);
    }

    #[test]
    fn a_run_crossing_a_page_boundary_is_one_range() {
        let m = Map::new(2);
        let page = m.page();
        let mut region = m.arm(4);
        m.write(page - 2, 1);
        m.write(page - 1, 1);
        m.write(page, 1);
        m.write(page + 1, 1);
        assert_eq!(region.flush(), vec![r(page - 2, 4)]);
    }

    #[test]
    fn the_page_size_is_the_runtime_one() {
        let m = Map::new(1);
        let region = m.arm(2);
        assert_eq!(region.page_size(), m.page());
    }

    #[test]
    fn arming_from_a_thread_that_blocks_the_fault_signals_is_refused() {
        let m = std::sync::Arc::new(Map::new(2));
        let m2 = m.clone();
        let err = std::thread::spawn(move || {
            let mut set: libc::sigset_t = unsafe { std::mem::zeroed() };
            unsafe {
                libc::sigemptyset(&mut set);
                libc::sigaddset(&mut set, libc::SIGSEGV);
                libc::pthread_sigmask(libc::SIG_BLOCK, &set, std::ptr::null_mut());
                Region::arm(m2.ptr, m2.len, 4).err()
            }
        })
        .join()
        .unwrap();
        assert!(matches!(err, Some(Error::SignalsBlocked { .. })));
        // The block was one thread's: other threads still arm fine.
        assert!(unsafe { Region::arm(m.ptr, m.len, 4) }.is_ok());
    }

    #[test]
    fn arming_from_a_thread_that_blocks_sigbus_is_refused() {
        let m = std::sync::Arc::new(Map::new(2));
        let err = std::thread::spawn(move || {
            let mut set: libc::sigset_t = unsafe { std::mem::zeroed() };
            unsafe {
                libc::sigemptyset(&mut set);
                libc::sigaddset(&mut set, libc::SIGBUS);
                libc::pthread_sigmask(libc::SIG_BLOCK, &set, std::ptr::null_mut());
                Region::arm(m.ptr, m.len, 4).err()
            }
        })
        .join()
        .unwrap();
        assert!(matches!(err, Some(Error::SignalsBlocked { .. })));
    }

    #[test]
    fn a_misaligned_pointer_is_refused() {
        let m = Map::new(2);
        let e = unsafe { Region::arm(m.ptr.add(1), m.page(), 4) }
            .err()
            .unwrap();
        assert!(matches!(e, Error::Misaligned { .. }));
    }

    #[test]
    fn a_length_that_is_not_whole_pages_is_refused() {
        let m = Map::new(2);
        let e = unsafe { Region::arm(m.ptr, m.page() + 1, 4) }
            .err()
            .unwrap();
        assert!(matches!(e, Error::Misaligned { .. }));
    }

    #[test]
    fn a_small_pool_arms_only_what_it_can_hold_and_leaves_the_rest_writable() {
        let m = Map::new(4);
        let page = m.page();
        let mut region = m.arm(4); // two pages' worth of double buffers
        assert_eq!(region.armed_pages(), 2);
        m.write(0, 1); // armed page
        m.write(page * 3, 1); // unarmed page: writable and untracked, must not crash
        assert_eq!(region.flush(), vec![r(0, 1)]);
    }

    #[test]
    fn dropping_a_region_makes_its_pages_writable_again() {
        let m = Map::new(2);
        let region = m.arm(4);
        drop(region);
        m.write(0, 5); // would fault fatally if the pages were still read-only
        assert_eq!(m.read(0), 5);
    }

    #[test]
    fn two_regions_in_one_process_are_tracked_independently() {
        let a = Map::new(2);
        let b = Map::new(2);
        let mut ra = a.arm(4);
        let mut rb = b.arm(4);
        a.write(1, 1);
        b.write(2, 1);
        b.write(3, 1);
        assert_eq!(ra.flush(), vec![r(1, 1)]);
        assert_eq!(rb.flush(), vec![r(2, 2)]);
    }

    #[test]
    fn writers_on_many_threads_are_all_seen() {
        let m = std::sync::Arc::new(Map::new(16));
        let page = m.page();
        let mut region = m.arm(32);
        let handles: Vec<_> = (0..16)
            .map(|t| {
                let m = m.clone();
                std::thread::spawn(move || m.write(t * page + 7, 1))
            })
            .collect();
        for h in handles {
            h.join().unwrap();
        }
        let want: Vec<Range> = (0..16).map(|t| r(t * page + 7, 1)).collect();
        assert_eq!(region.flush(), want);
    }
}
