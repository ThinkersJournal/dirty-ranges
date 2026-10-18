//! A global allocator that faults inside its own critical section. A fault handler that allocates (or takes a
//! lock the faulting thread may hold) deadlocks here, so the handler is shown to need neither. Ported from the
//! SDK lane's spike (`spike/trap-conformance`, PR #3). It has its own test binary because the allocator is
//! process-wide.
#![cfg(unix)]

mod common;

use common::*;
use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

static HAMMER_ON: AtomicBool = AtomicBool::new(false);
static HAMMER_EVERY: AtomicU64 = AtomicU64::new(32);
static HAMMER_LOCK: AtomicBool = AtomicBool::new(false);
static HAMMER_COUNT: AtomicU64 = AtomicU64::new(0);
static HAMMER_WRITES: AtomicU64 = AtomicU64::new(0);
static REGION_BASE: AtomicUsize = AtomicUsize::new(0);
static REGION_PAGES: AtomicUsize = AtomicUsize::new(0);
static REGION_PAGE: AtomicUsize = AtomicUsize::new(0);

struct Hammer;

unsafe impl GlobalAlloc for Hammer {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        if HAMMER_ON.load(Ordering::Relaxed) {
            let n = HAMMER_COUNT.fetch_add(1, Ordering::Relaxed);
            if n.is_multiple_of(HAMMER_EVERY.load(Ordering::Relaxed)) {
                while HAMMER_LOCK.swap(true, Ordering::Acquire) {
                    std::hint::spin_loop();
                }
                // Fault INSIDE the allocator's critical section: the handler runs while this thread holds it.
                let base = REGION_BASE.load(Ordering::Acquire);
                let pages = REGION_PAGES.load(Ordering::Acquire);
                let page = REGION_PAGE.load(Ordering::Acquire);
                if base != 0 && pages != 0 {
                    let p = (n as usize / 32) % pages;
                    let offset = p * page + (n as usize * 7) % page;
                    unsafe { std::ptr::write_volatile((base + offset) as *mut u8, (n as u8) | 1) };
                    HAMMER_WRITES.fetch_add(1, Ordering::Relaxed);
                }
                HAMMER_LOCK.store(false, Ordering::Release);
            }
        }
        unsafe { System.alloc(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) }
    }
}

#[global_allocator]
static ALLOCATOR: Hammer = Hammer;

/// Eight threads: four store at random bytes of the region, four allocate and free and contend on a mutex,
/// and the allocator itself faults every `every`th allocation. Returns whether, after the writers stopped
/// and one more flush, no write went unreported.
fn run_hammer(every: u64, for_at_most: Duration) -> bool {
    let m = Arc::new(Mapped::new(64));
    let mut region = m.arm(256);
    REGION_BASE.store(m.base as usize, Ordering::Release);
    REGION_PAGES.store(region.armed_pages(), Ordering::Release);
    REGION_PAGE.store(m.page, Ordering::Release);
    HAMMER_EVERY.store(every, Ordering::Relaxed);
    HAMMER_ON.store(true, Ordering::SeqCst);
    let stop = Arc::new(AtomicBool::new(false));
    let mut threads = vec![];
    for t in 0..4u64 {
        let (m, stop) = (m.clone(), stop.clone());
        threads.push(std::thread::spawn(move || {
            let mut s = 0x9E37_79B9_7F4A_7C15 ^ (t + 1);
            while !stop.load(Ordering::Relaxed) {
                m.store((rnd(&mut s) as usize) % m.len, (rnd(&mut s) as u8) | 1);
            }
        }));
    }
    let shared = Arc::new(Mutex::new(Vec::<u8>::new()));
    for t in 0..4u64 {
        let (stop, shared) = (stop.clone(), shared.clone());
        threads.push(std::thread::spawn(move || {
            let mut s = 0xD1B5_4A32_D192_ED03 ^ (t + 1);
            while !stop.load(Ordering::Relaxed) {
                let v: Vec<u8> = Vec::with_capacity((rnd(&mut s) % 512) as usize + 1);
                let b = Box::new(rnd(&mut s));
                let mut g = shared.lock().unwrap();
                g.push(*b as u8);
                if g.len() > 64 {
                    g.clear();
                }
                drop(g);
                drop(v);
            }
        }));
    }
    // Every byte starts at zero and every store writes a non-zero value, so a byte that is non-zero at the
    // end changed, and must lie in a range some flush reported.
    let mut covered = vec![false; m.len];
    let start = Instant::now();
    while start.elapsed() < for_at_most {
        for r in region.flush() {
            covered[r.offset..r.offset + r.len].fill(true);
        }
        std::thread::sleep(Duration::from_micros(300));
    }
    stop.store(true, Ordering::SeqCst);
    for t in threads {
        t.join().unwrap();
    }
    HAMMER_ON.store(false, Ordering::SeqCst);
    for r in region.flush() {
        covered[r.offset..r.offset + r.len].fill(true);
    }
    assert!(
        HAMMER_WRITES.load(Ordering::Relaxed) > 0,
        "no fault was injected inside the allocator"
    );
    m.live()
        .iter()
        .zip(&covered)
        .all(|(&byte, &seen)| byte == 0 || seen)
}

#[test]
fn faults_inside_the_allocators_critical_section_neither_deadlock_nor_lose_updates() {
    assert!(
        run_hammer(32, Duration::from_secs(2)),
        "a store was never reported"
    );
}

#[cfg(dirty_ranges_conformance)]
mod controls {
    use super::*;
    use dirty_ranges::conformance_hooks::ALLOC_IN_HANDLER;

    /// A handler that allocates must deadlock under the same hammer; otherwise the case above proves nothing.
    #[test]
    fn a_handler_that_allocates_deadlocks_control() {
        let name = "controls::a_handler_that_allocates_deadlocks_control";
        let Some(outcome) = in_child(name, name, Duration::from_secs(20), || {
            ALLOC_IN_HANDLER.store(true, Ordering::SeqCst);
            // Faulting on every allocation: the handler's own allocation then needs the allocator lock that the
            // faulting thread holds, which is a certain deadlock. (A lower rate sometimes nests handlers deeply
            // enough to overflow the alternate signal stack instead, which is also a failure but not a hang.)
            run_hammer(1, Duration::from_secs(2));
        }) else {
            return;
        };
        assert!(
            matches!(outcome, Outcome::Hung { .. }),
            "control: an allocating handler should hang, got {outcome:?}"
        );
    }
}
