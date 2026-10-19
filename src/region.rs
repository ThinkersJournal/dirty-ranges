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
    /// `apply_remote` needs the writable alias, and the region was armed without one.
    NoAlias,
    /// `offset..offset+len` is not inside the region.
    OutOfRegion {
        offset: usize,
        len: usize,
        region_len: usize,
    },
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
            Self::NoAlias => write!(f, "the region was armed without a writable alias"),
            Self::OutOfRegion {
                offset,
                len,
                region_len,
            } => write!(
                f,
                "{len} bytes at offset {offset} are outside the {region_len}-byte region"
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

/// How a page-rounded part of a [`Region`] finds writes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    /// The pages are read-only; the first write to each faults and marks it dirty. Cheap to flush, costs a
    /// fault per page per flush interval.
    Trapped,
    /// The pages are writable and never fault; each flush compares them with the baseline. No fault cost,
    /// but every flush reads every scanned page.
    Scanned,
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
    /// Length of the whole region, tracked or not.
    len: usize,
    /// Start of the caller's writable alias of the region, or 0 if there is none.
    alias: usize,
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

    /// Write `chunk` (which starts at region offset `at`, inside page `page_index`) through the alias, and
    /// into the page's baseline if one has been captured.
    ///
    /// # Safety
    /// `page_index < armed`; `chunk` lies within that page and the region; the alias is set and writable; the
    /// caller holds the page against the handler's capture (state `Copying` held by it, or `Dirty`).
    unsafe fn write_through(&self, page_index: usize, at: usize, chunk: &[u8]) {
        // SAFETY: by the caller's contract.
        unsafe {
            ptr::copy_nonoverlapping(chunk.as_ptr(), (self.alias + at) as *mut u8, chunk.len());
            if self.valid[page_index].load(Ordering::Acquire) {
                let cur = usize::from(self.current[page_index].load(Ordering::Acquire));
                ptr::copy_nonoverlapping(
                    chunk.as_ptr(),
                    self.buffer(page_index, cur).add(at % self.page),
                    chunk.len(),
                );
            }
        }
    }

    /// Copy `len` live bytes starting at region offset `at` (inside page `page_index`) into the page's baseline,
    /// if one has been captured. Reads the region; writes only the baseline.
    ///
    /// # Safety
    /// `page_index < armed`; `at..at+len` lies within that page; the caller holds the page against the handler's
    /// capture (state `Copying` held by it, or `Dirty`).
    unsafe fn rebaseline_chunk(&self, page_index: usize, at: usize, len: usize) {
        if self.valid[page_index].load(Ordering::Acquire) {
            let cur = usize::from(self.current[page_index].load(Ordering::Acquire));
            // SAFETY: by the caller's contract; the region is readable whether or not the page is protected.
            unsafe {
                ptr::copy_nonoverlapping(
                    (self.base + at) as *const u8,
                    self.buffer(page_index, cur).add(at % self.page),
                    len,
                );
            }
        }
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

/// Whether the calling thread blocks `SIGSEGV` or `SIGBUS`.
///
/// **A thread that blocks them must not write to a trapped region.** The kernel cannot deliver the fault to
/// the handler, so it kills the process (Linux with `SIGSEGV`, macOS with `SIGKILL`). That is fatal and loud,
/// never silent corruption, and it is tested (`a_write_from_a_thread_that_blocks_the_fault_signals_is_fatal`).
/// `Region::arm` does not look at the arming thread's mask, because the threads that matter are the writers,
/// which it cannot see: call this on a thread you are about to let write if its mask is not under your control.
#[cfg(unix)]
pub fn fault_signals_blocked() -> bool {
    let (segv, bus) = sys::blocked_fault_signals();
    segv || bus
}

/// A memory region whose writes are trapped page by page.
///
/// Every thread that writes to the region must leave `SIGSEGV` and `SIGBUS` unblocked; see
/// `fault_signals_blocked` (Unix).
pub struct Region {
    inner: *mut Inner,
    slot: usize,
    /// Pages claimed by the current flush. Reserved at arm time so the flush's stall window never allocates.
    dirty: Vec<usize>,
    /// Pages in [`Mode::Scanned`]. Only `&mut self` methods read or write it; the handler never does. A scanned
    /// page is unprotected, has a captured baseline and sits in state `Dirty` for good, so a stale fault on it
    /// just resumes.
    scanned: Vec<bool>,
    /// Changes found while switching pages back to trapped mode, reported by the next flush.
    pending: Vec<Range>,
}

// SAFETY: all shared state is atomics; `flush` needs `&mut`, so one flusher at a time.
unsafe impl Send for Region {}
unsafe impl Sync for Region {}

impl Region {
    /// Arm `len` bytes at `ptr`, tracking up to `pool_pages / 2` pages (two baseline buffers per page).
    /// Pages beyond that stay writable and untracked; the caller must scan them (see [`Region::armed_pages`]).
    ///
    /// `alias` is a second, writable mapping of the same memory (a different virtual address range over the
    /// same pages). [`Region::apply_remote`] writes through it, because the region itself is read-only
    /// between faults. Without one, `apply_remote` fails with [`Error::NoAlias`].
    ///
    /// # Safety
    /// `ptr..ptr+len` must be a live mapping that stays valid until the `Region` is dropped, and nothing may
    /// read from the OS into it (`read`, `recv`) while armed: the kernel reports `EFAULT` instead of faulting.
    /// If given, `alias..alias+len` must be a live, writable mapping of the same bytes as `ptr..ptr+len`,
    /// valid for as long as the `Region` lives.
    pub unsafe fn arm(
        ptr: *mut u8,
        len: usize,
        pool_pages: usize,
        alias: Option<*mut u8>,
    ) -> Result<Region, Error> {
        let page = sys::page_size();
        if !(ptr as usize).is_multiple_of(page) || !len.is_multiple_of(page) {
            return Err(Error::Misaligned { page_size: page });
        }
        let armed = (len / page).min(pool_pages / 2);
        let pool_len = armed * 2 * page;
        let pool = Box::into_raw(vec![0u8; pool_len].into_boxed_slice()).cast::<u8>();
        let inner = Box::into_raw(Box::new(Inner {
            base: ptr as usize,
            len,
            alias: alias.map_or(0, |a| a as usize),
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
            scanned: vec![false; armed],
            pending: Vec::new(),
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

    /// Record that `bytes` were written at `offset` by someone else (for example a remote peer): write them
    /// through the alias and into the baseline, so the next [`Region::flush`] does not report them.
    ///
    /// Nothing is written if the range is not inside the region. If the local side wrote the very same bytes
    /// at the same moment, the remote value wins and the local write goes unreported; that is a conflict the
    /// caller resolves, not one this crate can see.
    pub fn apply_remote(&mut self, offset: usize, bytes: &[u8]) -> Result<(), Error> {
        let inner = self.inner();
        if inner.alias == 0 {
            return Err(Error::NoAlias);
        }
        let out_of_region = || Error::OutOfRegion {
            offset,
            len: bytes.len(),
            region_len: inner.len,
        };
        let end = offset.checked_add(bytes.len()).ok_or_else(out_of_region)?;
        if end > inner.len {
            return Err(out_of_region());
        }
        let page = inner.page;
        let mut at = offset;
        while at < end {
            let p = at / page;
            let stop = end.min((p + 1) * page);
            let chunk = &bytes[at - offset..stop - offset];
            if p >= inner.armed {
                // SAFETY: inside the region, and the alias is writable by `arm`'s contract.
                unsafe {
                    ptr::copy_nonoverlapping(
                        chunk.as_ptr(),
                        (inner.alias + at) as *mut u8,
                        chunk.len(),
                    )
                };
            } else {
                let state = &inner.state[p];
                loop {
                    match state.load() {
                        State::Clean => {
                            // Hold the page against a capture that would copy half of our write.
                            if !state.begin_capture() {
                                continue;
                            }
                            // SAFETY: `p < armed`; we hold the page in `Copying`.
                            unsafe { inner.write_through(p, at, chunk) };
                            state.abort_capture();
                            break;
                        }
                        // A handler is capturing; `&mut self` rules out a flush.
                        State::Copying | State::Flushing => std::hint::spin_loop(),
                        State::Dirty => {
                            // SAFETY: `p < armed`; the baseline was captured when the page went dirty.
                            unsafe { inner.write_through(p, at, chunk) };
                            break;
                        }
                    }
                }
            }
            at = stop;
        }
        Ok(())
    }

    /// Switch the page-rounded bytes `range` between [`Mode::Trapped`] and [`Mode::Scanned`]. Every page
    /// starts trapped.
    ///
    /// Nothing is lost across a switch: writes made before it are reported by the next flush. Switching to
    /// trapped finds them by comparing the pages with their baseline once, so the next flush may report a
    /// range that a later write has since changed back. Switching a page to the mode it is in does nothing.
    ///
    /// `range` must start and end on page boundaries ([`Error::Misaligned`]) and lie inside the tracked
    /// pages ([`Error::OutOfRegion`], whose `region_len` is then the tracked length, [`Region::armed_pages`]
    /// pages). If an operating-system call fails, the pages before the failing one have already switched.
    pub fn set_mode(&mut self, range: core::ops::Range<usize>, mode: Mode) -> Result<(), Error> {
        // SAFETY: allocated in `arm`, freed only in `Drop`; a separate borrow so `self.scanned` stays usable.
        let inner = unsafe { &*self.inner };
        let page = inner.page;
        let tracked = inner.armed * page;
        if !range.start.is_multiple_of(page) || !range.end.is_multiple_of(page) {
            return Err(Error::Misaligned { page_size: page });
        }
        if range.start > range.end || range.end > tracked {
            return Err(Error::OutOfRegion {
                offset: range.start,
                len: range.end.saturating_sub(range.start),
                region_len: tracked,
            });
        }
        for p in range.start / page..range.end / page {
            match (self.scanned[p], mode) {
                (false, Mode::Scanned) => {
                    let state = &inner.state[p];
                    loop {
                        match state.load() {
                            State::Clean => {
                                if !state.begin_capture() {
                                    continue;
                                }
                                if !inner.valid[p].load(Ordering::Acquire) {
                                    let cur = usize::from(inner.current[p].load(Ordering::Acquire));
                                    // SAFETY: `p < armed`; we hold the page in `Copying` and it is read-only.
                                    unsafe {
                                        ptr::copy_nonoverlapping(
                                            inner.live(p),
                                            inner.buffer(p, cur),
                                            page,
                                        )
                                    };
                                    inner.valid[p].store(true, Ordering::Release);
                                }
                                // SAFETY: a whole armed page of the caller's mapping.
                                if !unsafe { sys::protect(inner.live(p) as usize, page, true) } {
                                    let e = std::io::Error::last_os_error();
                                    state.abort_capture();
                                    return Err(Error::Os(e));
                                }
                                state.finish_capture();
                                break;
                            }
                            // A handler is capturing; `&mut self` rules out a flush.
                            State::Copying | State::Flushing => std::hint::spin_loop(),
                            // Already writable, with a baseline.
                            State::Dirty => break,
                        }
                    }
                    self.scanned[p] = true;
                }
                (true, Mode::Trapped) => {
                    let state = &inner.state[p];
                    // A scanned page is `Dirty`; claiming it makes writers that fault wait.
                    assert!(state.begin_flush(), "a scanned page is not Dirty");
                    // SAFETY: a whole armed page of the caller's mapping.
                    if !unsafe { sys::protect(inner.live(p) as usize, page, false) } {
                        let e = std::io::Error::last_os_error();
                        state.abort_flush();
                        return Err(Error::Os(e));
                    }
                    self.scanned[p] = false;
                    let cur = usize::from(inner.current[p].load(Ordering::Acquire));
                    // SAFETY: `p < armed`; the page is read-only now, so the copy is a consistent snapshot.
                    unsafe {
                        ptr::copy_nonoverlapping(inner.live(p), inner.buffer(p, 1 - cur), page)
                    };
                    state.finish_flush();
                    // SAFETY: two distinct page-sized buffers inside the pool; `&mut self` rules out a flush.
                    let (old, new) = unsafe {
                        (
                            std::slice::from_raw_parts(inner.buffer(p, cur), page),
                            std::slice::from_raw_parts(inner.buffer(p, 1 - cur), page),
                        )
                    };
                    diff(old, new, p * page, &mut self.pending);
                    inner.current[p].store((1 - cur) as u8, Ordering::Release);
                }
                _ => {}
            }
        }
        Ok(())
    }

    /// Make the baseline of `offset..offset+len` match the bytes that are in the region now, so the next
    /// [`Region::flush`] does not report them. Nothing is written to the region, with or without an alias.
    ///
    /// Use it when someone else has already put new bytes there (for example another process writing the
    /// shared file) and you only want to stop them being reported as local writes. Unlike
    /// [`Region::apply_remote`] it cannot overwrite a newer foreign write, because it writes no region bytes.
    /// A local write in the range that has not been flushed yet is absorbed too, and not reported.
    ///
    /// Fails with [`Error::OutOfRegion`] if the range is not inside the region. Bytes in untracked pages have
    /// no baseline, so there is nothing to do for them.
    pub fn rebaseline(&mut self, offset: usize, len: usize) -> Result<(), Error> {
        let inner = self.inner();
        let out_of_region = || Error::OutOfRegion {
            offset,
            len,
            region_len: inner.len,
        };
        let end = offset.checked_add(len).ok_or_else(out_of_region)?;
        if end > inner.len {
            return Err(out_of_region());
        }
        let page = inner.page;
        let mut at = offset;
        while at < end {
            let p = at / page;
            let stop = end.min((p + 1) * page);
            if p >= inner.armed {
                break; // the rest is untracked too
            }
            let state = &inner.state[p];
            loop {
                match state.load() {
                    State::Clean => {
                        // Hold the page against a capture that would race our copy.
                        if !state.begin_capture() {
                            continue;
                        }
                        // SAFETY: `p < armed`; we hold the page in `Copying`.
                        unsafe { inner.rebaseline_chunk(p, at, stop - at) };
                        state.abort_capture();
                        break;
                    }
                    // A handler is capturing; `&mut self` rules out a flush.
                    State::Copying | State::Flushing => std::hint::spin_loop(),
                    State::Dirty => {
                        // SAFETY: `p < armed`; the baseline was captured when the page went dirty.
                        unsafe { inner.rebaseline_chunk(p, at, stop - at) };
                        break;
                    }
                }
            }
            at = stop;
        }
        Ok(())
    }

    /// Make every tracked page writable again and release the region, reporting whether the pages were
    /// unprotected. Dropping a `Region` does the same but cannot report a failure.
    pub fn disarm(self) -> Result<(), Error> {
        let inner = self.inner();
        if inner.armed > 0 {
            // SAFETY: the caller's mapping outlives the region by `arm`'s contract.
            if !unsafe { sys::protect(inner.base, inner.armed * inner.page, true) } {
                return Err(Error::Os(std::io::Error::last_os_error()));
            }
        }
        Ok(())
    }

    /// Report the byte ranges written since the last flush (sorted, coalesced, byte-exact) and re-arm.
    pub fn flush(&mut self) -> Vec<Range> {
        // SAFETY: allocated in `arm`, freed only in `Drop`; a separate borrow so `self.dirty` stays usable.
        let inner = unsafe { &*self.inner };
        let page = inner.page;
        self.dirty.clear();
        // ---- stall window: writers that fault now spin until their page is released. No allocation, no lock.
        for p in 0..inner.armed {
            // A scanned page is always claimed: it is compared like a dirty one, but never re-protected.
            if self.scanned[p] || inner.state[p].begin_flush() {
                self.dirty.push(p); // capacity was reserved in `arm`
            }
        }
        let mut i = 0;
        while i < self.dirty.len() {
            if self.scanned[self.dirty[i]] {
                i += 1;
                continue;
            }
            let mut j = i;
            while j + 1 < self.dirty.len()
                && self.dirty[j + 1] == self.dirty[j] + 1
                && !self.scanned[self.dirty[j + 1]]
            {
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
            if !self.scanned[p] {
                inner.state[p].finish_flush(); // writers may capture again; the page is protected with a valid baseline
            }
        }
        // ---- stall window over: allocating is fine from here.
        let mut out = std::mem::take(&mut self.pending);
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

#[cfg(all(test, any(unix, windows)))]
mod tests {
    use super::*;

    /// An anonymous mapping standing in for a mapped file.
    struct Map {
        _map: memmap2::MmapMut,
        ptr: *mut u8,
        len: usize,
    }
    unsafe impl Send for Map {}
    unsafe impl Sync for Map {}

    fn os_page() -> usize {
        sys::page_size()
    }

    impl Map {
        fn new(pages: usize) -> Map {
            let len = pages * os_page();
            let mut map = memmap2::MmapMut::map_anon(len).unwrap();
            let ptr = map.as_mut_ptr();
            Map {
                _map: map,
                ptr,
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
            unsafe { Region::arm(self.ptr, self.len, pool_pages, None) }.unwrap()
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

    #[cfg(unix)]
    fn on_a_thread_that_blocks(sig: libc::c_int, f: impl FnOnce() + Send + 'static) {
        std::thread::spawn(move || {
            let mut set: libc::sigset_t = unsafe { std::mem::zeroed() };
            unsafe {
                libc::sigemptyset(&mut set);
                libc::sigaddset(&mut set, sig);
                libc::pthread_sigmask(libc::SIG_BLOCK, &set, std::ptr::null_mut());
            }
            f();
        })
        .join()
        .unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn the_query_reports_whether_this_thread_blocks_the_fault_signals() {
        assert!(!fault_signals_blocked());
        on_a_thread_that_blocks(libc::SIGSEGV, || assert!(fault_signals_blocked()));
        on_a_thread_that_blocks(libc::SIGBUS, || assert!(fault_signals_blocked()));
        assert!(!fault_signals_blocked(), "the block was one thread's");
    }

    /// Arming does not look at the arming thread's mask: what is fatal is a *writer* thread that blocks the
    /// signals, and `arm` cannot see the writers. Callers query `fault_signals_blocked` where they care.
    #[cfg(unix)]
    #[test]
    fn arming_does_not_depend_on_the_arming_threads_signal_mask() {
        let m = std::sync::Arc::new(Map::new(2));
        let m2 = m.clone();
        on_a_thread_that_blocks(libc::SIGSEGV, move || {
            assert!(unsafe { Region::arm(m2.ptr, m2.len, 4, None) }.is_ok());
        });
    }

    #[test]
    fn a_misaligned_pointer_is_refused() {
        let m = Map::new(2);
        let e = unsafe { Region::arm(m.ptr.add(1), m.page(), 4, None) }
            .err()
            .unwrap();
        assert!(matches!(e, Error::Misaligned { .. }));
    }

    #[test]
    fn a_length_that_is_not_whole_pages_is_refused() {
        let m = Map::new(2);
        let e = unsafe { Region::arm(m.ptr, m.page() + 1, 4, None) }
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

    /// One file mapped twice, so the second mapping is a true writable alias of the first.
    struct Aliased {
        view: memmap2::MmapMut,
        alias: memmap2::MmapMut,
        path: std::path::PathBuf,
    }
    impl Aliased {
        fn new(pages: usize) -> Aliased {
            static N: AtomicUsize = AtomicUsize::new(0);
            let path = std::env::temp_dir().join(format!(
                "dirty-ranges-alias-{}-{}",
                std::process::id(),
                N.fetch_add(1, Ordering::Relaxed)
            ));
            let file = std::fs::OpenOptions::new()
                .read(true)
                .write(true)
                .create_new(true)
                .open(&path)
                .unwrap();
            file.set_len((pages * os_page()) as u64).unwrap();
            // SAFETY: a private temp file nothing else touches.
            let view = unsafe { memmap2::MmapMut::map_mut(&file) }.unwrap();
            let alias = unsafe { memmap2::MmapMut::map_mut(&file) }.unwrap();
            Aliased { view, alias, path }
        }
        fn arm(&mut self, pool_pages: usize) -> Region {
            let len = self.view.len();
            let alias = self.alias.as_mut_ptr();
            unsafe { Region::arm(self.view.as_mut_ptr(), len, pool_pages, Some(alias)) }.unwrap()
        }
        fn write(&self, offset: usize, v: u8) {
            unsafe { self.view.as_ptr().add(offset).cast_mut().write_volatile(v) };
        }
        fn read(&self, offset: usize) -> u8 {
            unsafe { self.view.as_ptr().add(offset).read_volatile() }
        }
        /// A write by "someone else": through the alias, so it never faults.
        fn foreign_write(&self, offset: usize, v: u8) {
            unsafe { self.alias.as_ptr().add(offset).cast_mut().write_volatile(v) };
        }
    }
    impl Drop for Aliased {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.path);
        }
    }

    #[test]
    fn a_remote_write_lands_in_the_region_and_is_not_reported() {
        let mut m = Aliased::new(2);
        let mut region = m.arm(4);
        region.apply_remote(5, &[1, 2, 3]).unwrap();
        assert_eq!((m.read(5), m.read(6), m.read(7)), (1, 2, 3));
        assert_eq!(region.flush(), vec![]);
    }

    #[test]
    fn a_remote_write_to_a_page_with_a_baseline_is_not_reported() {
        let mut m = Aliased::new(2);
        let mut region = m.arm(4);
        m.write(0, 9);
        assert_eq!(region.flush(), vec![r(0, 1)]); // page 0 now has a captured baseline
        region.apply_remote(8, &[4, 4]).unwrap();
        assert_eq!(region.flush(), vec![]);
        assert_eq!(m.read(9), 4);
    }

    #[test]
    fn a_remote_write_to_a_dirty_page_leaves_the_local_write_reported() {
        let mut m = Aliased::new(1);
        let mut region = m.arm(2);
        m.write(0, 7);
        region.apply_remote(16, &[5; 4]).unwrap();
        assert_eq!(region.flush(), vec![r(0, 1)]);
        assert_eq!(m.read(18), 5);
    }

    #[test]
    fn a_remote_write_across_pages_and_into_untracked_pages_lands() {
        let mut m = Aliased::new(3);
        let page = os_page();
        let mut region = m.arm(2); // one tracked page, two untracked
        region.apply_remote(page - 2, &[1, 2, 3, 4]).unwrap();
        assert_eq!(m.read(page - 1), 2);
        assert_eq!(m.read(page + 1), 4);
        assert_eq!(region.flush(), vec![]);
    }

    #[test]
    fn a_remote_write_outside_the_region_is_refused_and_writes_nothing() {
        let mut m = Aliased::new(1);
        let len = m.view.len();
        let mut region = m.arm(2);
        assert!(matches!(
            region.apply_remote(len - 1, &[1, 2]),
            Err(Error::OutOfRegion { .. })
        ));
        assert!(region.apply_remote(usize::MAX, &[1, 2]).is_err());
        assert_eq!(m.read(len - 1), 0);
    }

    #[test]
    fn a_remote_write_needs_an_alias() {
        let m = Map::new(1);
        let mut region = m.arm(2);
        assert!(matches!(region.apply_remote(0, &[1]), Err(Error::NoAlias)));
    }

    #[test]
    fn disarm_makes_the_pages_writable() {
        let m = Map::new(2);
        let region = m.arm(4);
        region.disarm().unwrap();
        m.write(0, 1); // would fault into a dead region if the pages were still read-only
        assert_eq!(m.read(0), 1);
    }

    #[test]
    fn a_scanned_page_reports_its_writes_once() {
        let m = Map::new(2);
        let page = m.page();
        let mut region = m.arm(4);
        region.set_mode(0..page, Mode::Scanned).unwrap();
        m.write(10, 1);
        m.write(11, 1);
        assert_eq!(region.flush(), vec![r(10, 2)]);
        assert_eq!(region.flush(), vec![]);
        m.write(10, 2);
        assert_eq!(region.flush(), vec![r(10, 1)]);
    }

    #[test]
    fn a_scanned_page_ignores_a_store_of_the_value_already_there() {
        let m = Map::new(1);
        m.write(3, 5);
        let mut region = m.arm(2);
        region.set_mode(0..m.page(), Mode::Scanned).unwrap();
        m.write(3, 5);
        assert_eq!(region.flush(), vec![]);
    }

    #[test]
    fn a_write_made_before_switching_to_scanned_is_still_reported() {
        let m = Map::new(2);
        let page = m.page();
        let mut region = m.arm(4);
        m.write(7, 1); // page 0 is Dirty
        region.set_mode(0..page, Mode::Scanned).unwrap();
        m.write(page + 7, 1); // page 1 stays trapped
        assert_eq!(region.flush(), vec![r(7, 1), r(page + 7, 1)]);
    }

    #[test]
    fn a_write_made_while_scanned_survives_the_switch_back_to_trapped() {
        let m = Map::new(2);
        let page = m.page();
        let mut region = m.arm(4);
        region.set_mode(0..page * 2, Mode::Scanned).unwrap();
        m.write(5, 1);
        region.set_mode(0..page * 2, Mode::Trapped).unwrap();
        assert_eq!(region.flush(), vec![r(5, 1)]);
        assert_eq!(region.flush(), vec![]);
        m.write(6, 1); // trapped again: must fault, not slip by
        assert_eq!(region.flush(), vec![r(6, 1)]);
    }

    #[test]
    fn scanned_and_trapped_pages_side_by_side_coalesce_across_the_boundaries() {
        let m = Map::new(4);
        let page = m.page();
        let mut region = m.arm(8);
        region.set_mode(page..page * 2, Mode::Scanned).unwrap();
        for at in [page - 1, page, page * 2 - 1, page * 2, page * 3] {
            m.write(at, 1);
        }
        assert_eq!(
            region.flush(),
            vec![r(page - 1, 2), r(page * 2 - 1, 2), r(page * 3, 1)]
        );
        m.write(page * 2 + 1, 1);
        m.write(page + 1, 1);
        assert_eq!(region.flush(), vec![r(page + 1, 1), r(page * 2 + 1, 1)]);
    }

    #[test]
    fn switching_to_the_mode_a_page_is_in_changes_nothing() {
        let m = Map::new(2);
        let page = m.page();
        let mut region = m.arm(4);
        region.set_mode(0..page * 2, Mode::Trapped).unwrap();
        m.write(1, 1);
        region.set_mode(page..page * 2, Mode::Scanned).unwrap();
        region.set_mode(page..page * 2, Mode::Scanned).unwrap();
        region.set_mode(0..0, Mode::Scanned).unwrap();
        assert_eq!(region.flush(), vec![r(1, 1)]);
    }

    #[test]
    fn set_mode_refuses_a_range_that_is_not_whole_pages() {
        let m = Map::new(2);
        let page = m.page();
        let mut region = m.arm(4);
        assert!(matches!(
            region.set_mode(1..page, Mode::Scanned),
            Err(Error::Misaligned { .. })
        ));
        assert!(matches!(
            region.set_mode(0..page + 1, Mode::Scanned),
            Err(Error::Misaligned { .. })
        ));
    }

    #[test]
    fn set_mode_refuses_pages_that_are_not_tracked_and_changes_nothing() {
        let m = Map::new(4);
        let page = m.page();
        let mut region = m.arm(4); // two tracked pages
        assert_eq!(region.armed_pages(), 2);
        assert!(matches!(
            region.set_mode(page..page * 3, Mode::Scanned),
            Err(Error::OutOfRegion { region_len, .. }) if region_len == page * 2
        ));
        assert!(matches!(
            region.set_mode(page * 2..page, Mode::Scanned),
            Err(Error::OutOfRegion { .. })
        ));
        m.write(page + 1, 1); // page 1 is still trapped
        assert_eq!(region.flush(), vec![r(page + 1, 1)]);
    }

    #[test]
    fn a_remote_write_to_a_scanned_page_is_not_reported() {
        let mut m = Aliased::new(2);
        let page = os_page();
        let mut region = m.arm(4);
        region.set_mode(0..page, Mode::Scanned).unwrap();
        m.write(0, 9);
        region.apply_remote(8, &[4, 4]).unwrap();
        assert_eq!(region.flush(), vec![r(0, 1)]);
        assert_eq!(m.read(9), 4);
    }

    #[test]
    fn a_scanned_page_can_be_written_from_many_threads_while_flushing() {
        let m = std::sync::Arc::new(Map::new(2));
        let page = m.page();
        let mut region = m.arm(4);
        region.set_mode(0..page, Mode::Scanned).unwrap();
        let writers: Vec<_> = (0..4)
            .map(|t| {
                let m = m.clone();
                std::thread::spawn(move || {
                    for i in 0..200 {
                        m.write(t * 16 + i % 16, 1 + (i % 200) as u8);
                        m.write(page + t * 16, 1 + (i % 200) as u8);
                    }
                })
            })
            .collect();
        let mut seen = vec![];
        while writers.iter().any(|w| !w.is_finished()) {
            seen.extend(region.flush());
        }
        for w in writers {
            w.join().unwrap();
        }
        seen.extend(region.flush());
        let mut touched = vec![false; page * 2];
        for rg in &seen {
            touched[rg.offset..rg.offset + rg.len].fill(true);
        }
        for t in 0..4 {
            assert!(touched[t * 16], "thread {t}'s scanned-page write was lost");
            assert!(
                touched[page + t * 16],
                "thread {t}'s trapped-page write was lost"
            );
        }
    }

    #[test]
    fn a_foreign_write_is_rebaselined_without_being_reported_or_touched() {
        let mut m = Aliased::new(2);
        let mut region = m.arm(4);
        m.write(0, 1);
        assert_eq!(region.flush(), vec![r(0, 1)]); // page 0 has a baseline
        m.foreign_write(5, 42);
        region.rebaseline(5, 1).unwrap();
        assert_eq!(m.read(5), 42, "rebaseline must not write the region");
        assert_eq!(region.flush(), vec![]);
        m.write(5, 43); // a real local write after it is still seen
        assert_eq!(region.flush(), vec![r(5, 1)]);
    }

    #[test]
    fn rebaselining_a_page_with_no_baseline_yet_is_harmless() {
        let mut m = Aliased::new(2);
        let page = os_page();
        let mut region = m.arm(4);
        m.foreign_write(page + 3, 9);
        region.rebaseline(page, page).unwrap();
        assert_eq!(m.read(page + 3), 9);
        assert_eq!(region.flush(), vec![]);
        m.write(page + 4, 1);
        assert_eq!(region.flush(), vec![r(page + 4, 1)]);
    }

    #[test]
    fn rebaselining_a_dirty_page_absorbs_only_the_range_asked_for() {
        let mut m = Aliased::new(1);
        let mut region = m.arm(2);
        m.write(0, 1);
        m.write(100, 1); // page is Dirty
        m.foreign_write(50, 7);
        region.rebaseline(50, 1).unwrap();
        assert_eq!(region.flush(), vec![r(0, 1), r(100, 1)]);
        assert_eq!(m.read(50), 7);
    }

    #[test]
    fn rebaseline_works_on_a_scanned_page_and_across_pages() {
        let mut m = Aliased::new(3);
        let page = os_page();
        let mut region = m.arm(6);
        region.set_mode(0..page, Mode::Scanned).unwrap();
        m.foreign_write(page - 1, 3);
        m.foreign_write(page, 4);
        m.foreign_write(page * 2 - 1, 5);
        region.rebaseline(page - 1, page + 1).unwrap();
        assert_eq!(region.flush(), vec![]);
        assert_eq!(
            (m.read(page - 1), m.read(page), m.read(page * 2 - 1)),
            (3, 4, 5)
        );
    }

    #[test]
    fn rebaseline_refuses_a_range_outside_the_region_and_ignores_untracked_pages() {
        let mut m = Aliased::new(3);
        let page = os_page();
        let mut region = m.arm(2); // one tracked page
        let len = m.view.len();
        assert!(matches!(
            region.rebaseline(len - 1, 2),
            Err(Error::OutOfRegion { .. })
        ));
        assert!(region.rebaseline(usize::MAX, 2).is_err());
        region.rebaseline(page - 2, page * 2).unwrap(); // reaches untracked pages: fine
        region.rebaseline(0, 0).unwrap();
    }

    #[test]
    fn rebaseline_never_overwrites_a_concurrent_foreign_write() {
        let mut m = Aliased::new(2);
        let page = os_page();
        let mut region = m.arm(4);
        m.write(0, 1);
        region.flush(); // both pages: page 0 has a baseline, page 1 is clean
        let m = std::sync::Arc::new(m);
        let writer = {
            let m = m.clone();
            std::thread::spawn(move || {
                for v in 1..=200u8 {
                    m.foreign_write(8, v);
                    m.foreign_write(page + 8, v);
                }
            })
        };
        while !writer.is_finished() {
            region.rebaseline(0, page * 2).unwrap();
        }
        writer.join().unwrap();
        // The region unprotects its pages when dropped, so it must go before the mapping does (`m` is
        // declared later, so it would drop first): another test's mapping may reuse the address.
        drop(region);
        assert_eq!(m.read(8), 200, "a foreign write was lost");
        assert_eq!(m.read(page + 8), 200, "a foreign write was lost");
    }
}
