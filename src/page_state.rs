//! The per-page state machine shared by the fault handler and the flusher.
//!
//! ```text
//!   Clean --begin_capture--> Copying --finish_capture--> Dirty --begin_flush--> Flushing --finish_flush--> Clean
//!   Copying --abort_capture--> Clean   (the handler could not unprotect the page)
//!   Flushing --abort_flush--> Dirty    (a page being switched to trapped mode could not be re-protected)
//! ```
//!
//! The handler owns the `Clean -> Copying -> Dirty` half and the flusher owns `Dirty -> Flushing -> Clean`.
//! Every method is a single atomic operation: nothing here allocates, locks or spins, so it is usable
//! from a fault handler.

#[cfg(loom)]
use loom::sync::atomic::{AtomicU8, Ordering};
#[cfg(not(loom))]
use std::sync::atomic::{AtomicU8, Ordering};

const CLEAN: u8 = 0;
const COPYING: u8 = 1;
const DIRTY: u8 = 2;
const FLUSHING: u8 = 3;

/// The observable state of a page.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    /// Unchanged since the last flush; the page is armed (protected).
    Clean,
    /// A handler is copying the twin; the page is not yet safe to diff.
    Copying,
    /// Written since the last flush; the twin holds the baseline.
    Dirty,
    /// A flusher is re-protecting and capturing the page.
    Flushing,
}

/// One page's atomic state.
#[derive(Debug)]
pub struct PageState(AtomicU8);

impl PageState {
    pub fn new() -> Self {
        Self(AtomicU8::new(CLEAN))
    }

    pub fn load(&self) -> State {
        match self.0.load(Ordering::Acquire) {
            CLEAN => State::Clean,
            COPYING => State::Copying,
            DIRTY => State::Dirty,
            FLUSHING => State::Flushing,
            other => unreachable!("invalid page state {other}"),
        }
    }

    /// Handler: claim the right to copy the twin. `false` means another handler or the flusher holds the page.
    pub fn begin_capture(&self) -> bool {
        // Acquire pairs with `finish_flush`'s Release: the flusher's reads of the old twin happen before our write.
        self.0
            .compare_exchange(CLEAN, COPYING, Ordering::Acquire, Ordering::Relaxed)
            .is_ok()
    }

    /// Handler: the twin is written and the page unprotected. Panics unless the page was `Copying`.
    pub fn finish_capture(&self) {
        // Release publishes the twin write to the flusher's Acquire in `begin_flush`.
        let r = self
            .0
            .compare_exchange(COPYING, DIRTY, Ordering::Release, Ordering::Relaxed);
        assert!(r.is_ok(), "finish_capture on a page that was not Copying");
    }

    /// Handler: give the page back unchanged after a failed capture (for example `mprotect` failed).
    /// Panics unless the page was `Copying`.
    pub fn abort_capture(&self) {
        let r = self
            .0
            .compare_exchange(COPYING, CLEAN, Ordering::Release, Ordering::Relaxed);
        assert!(r.is_ok(), "abort_capture on a page that was not Copying");
    }

    /// Flusher: claim a dirty page. `false` if it is clean, still being copied, or already being flushed.
    pub fn begin_flush(&self) -> bool {
        self.0
            .compare_exchange(DIRTY, FLUSHING, Ordering::Acquire, Ordering::Relaxed)
            .is_ok()
    }

    /// Flusher: give a claimed page back still dirty, because it could not be re-protected. Panics unless the
    /// page was `Flushing`.
    pub fn abort_flush(&self) {
        let r = self
            .0
            .compare_exchange(FLUSHING, DIRTY, Ordering::Release, Ordering::Relaxed);
        assert!(r.is_ok(), "abort_flush on a page that was not Flushing");
    }

    /// Flusher: the live page is captured. Panics unless the page was `Flushing`.
    pub fn finish_flush(&self) {
        let r = self
            .0
            .compare_exchange(FLUSHING, CLEAN, Ordering::Release, Ordering::Relaxed);
        assert!(r.is_ok(), "finish_flush on a page that was not Flushing");
    }
}

impl Default for PageState {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(all(test, not(loom)))]
mod tests {
    use super::*;

    #[test]
    fn a_new_page_is_clean() {
        assert_eq!(PageState::new().load(), State::Clean);
    }

    #[test]
    fn the_full_cycle_returns_to_clean() {
        let p = PageState::new();
        assert!(p.begin_capture());
        assert_eq!(p.load(), State::Copying);
        p.finish_capture();
        assert_eq!(p.load(), State::Dirty);
        assert!(p.begin_flush());
        assert_eq!(p.load(), State::Flushing);
        p.finish_flush();
        assert_eq!(p.load(), State::Clean);
    }

    #[test]
    fn an_aborted_flush_leaves_the_page_dirty() {
        let p = PageState::new();
        assert!(p.begin_capture());
        p.finish_capture();
        assert!(p.begin_flush());
        p.abort_flush();
        assert_eq!(p.load(), State::Dirty);
        assert!(p.begin_flush());
    }

    #[test]
    fn only_one_handler_wins_the_capture() {
        let p = PageState::new();
        assert!(p.begin_capture());
        assert!(!p.begin_capture());
    }

    #[test]
    fn capture_is_refused_on_a_dirty_page() {
        let p = PageState::new();
        assert!(p.begin_capture());
        p.finish_capture();
        assert!(!p.begin_capture());
    }

    #[test]
    fn capture_is_refused_while_flushing() {
        let p = PageState::new();
        assert!(p.begin_capture());
        p.finish_capture();
        assert!(p.begin_flush());
        assert!(!p.begin_capture());
    }

    #[test]
    fn flush_is_refused_on_a_clean_page() {
        assert!(!PageState::new().begin_flush());
    }

    #[test]
    fn flush_is_refused_while_the_twin_is_being_copied() {
        let p = PageState::new();
        assert!(p.begin_capture());
        assert!(!p.begin_flush());
    }

    #[test]
    fn flush_is_refused_twice() {
        let p = PageState::new();
        assert!(p.begin_capture());
        p.finish_capture();
        assert!(p.begin_flush());
        assert!(!p.begin_flush());
    }

    #[test]
    fn an_aborted_capture_returns_to_clean_and_can_be_retried() {
        let p = PageState::new();
        assert!(p.begin_capture());
        p.abort_capture();
        assert_eq!(p.load(), State::Clean);
        assert!(p.begin_capture());
    }

    #[test]
    #[should_panic(expected = "abort_capture")]
    fn abort_capture_without_begin_panics() {
        PageState::new().abort_capture();
    }

    #[test]
    #[should_panic(expected = "finish_capture")]
    fn finish_capture_without_begin_panics() {
        PageState::new().finish_capture();
    }

    #[test]
    #[should_panic(expected = "finish_flush")]
    fn finish_flush_without_begin_panics() {
        PageState::new().finish_flush();
    }
}

/// Run with `RUSTFLAGS="--cfg loom" cargo test --release loom_`. Loom explores every interleaving and
/// every permitted reordering of the atomics, and flags a data race on `loom::cell::UnsafeCell`.
#[cfg(all(test, loom))]
#[expect(
    unsafe_code,
    reason = "loom::cell::UnsafeCell hands out raw pointers; this test-only unsafe is how loom detects a data race on the twin"
)]
mod loom_tests {
    use super::*;
    use loom::cell::UnsafeCell;
    use loom::sync::Arc;
    use loom::thread;

    /// A page with a "twin" the handler writes and the flusher reads, as in the real design.
    struct Page {
        state: PageState,
        twin: UnsafeCell<u32>,
    }

    // SAFETY: test-only. The state machine is what is under test: it must make the twin accesses race-free,
    // and loom reports a race if it does not.
    unsafe impl Sync for Page {}
    unsafe impl Send for Page {}

    fn page() -> Arc<Page> {
        Arc::new(Page {
            state: PageState::new(),
            twin: UnsafeCell::new(0),
        })
    }

    fn handler(p: &Page) -> bool {
        if p.state.begin_capture() {
            p.twin.with_mut(|t| unsafe { *t = 42 });
            p.state.finish_capture();
            true
        } else {
            false
        }
    }

    fn flusher(p: &Page) -> bool {
        if p.state.begin_flush() {
            let seen = p.twin.with(|t| unsafe { *t });
            assert_eq!(seen, 42, "flusher saw a half-written twin");
            p.state.finish_flush();
            true
        } else {
            false
        }
    }

    #[test]
    fn loom_two_handlers_exactly_one_captures() {
        loom::model(|| {
            let p = page();
            let (a, b) = (p.clone(), p.clone());
            let ha = thread::spawn(move || handler(&a));
            let hb = thread::spawn(move || handler(&b));
            let wins = ha.join().unwrap() as u8 + hb.join().unwrap() as u8;
            assert_eq!(wins, 1);
            assert_eq!(p.state.load(), State::Dirty);
        });
    }

    #[test]
    fn loom_flusher_never_sees_a_half_written_twin() {
        loom::model(|| {
            let p = page();
            let (a, b) = (p.clone(), p.clone());
            let h = thread::spawn(move || handler(&a));
            let f = thread::spawn(move || flusher(&b));
            let captured = h.join().unwrap();
            let flushed = f.join().unwrap();
            assert!(captured);
            let end = p.state.load();
            assert_eq!(end, if flushed { State::Clean } else { State::Dirty });
        });
    }

    #[test]
    fn loom_a_second_capture_after_a_flush_does_not_race_the_flusher_read() {
        loom::model(|| {
            let p = page();
            assert!(handler(&p));
            let (a, b) = (p.clone(), p.clone());
            let f = thread::spawn(move || flusher(&a));
            let h = thread::spawn(move || handler(&b));
            let flushed = f.join().unwrap();
            let captured = h.join().unwrap();
            // Only the flusher can move a Dirty page, so it always wins; a re-capture can only follow it.
            assert!(flushed);
            assert_eq!(
                p.state.load(),
                if captured { State::Dirty } else { State::Clean }
            );
        });
    }
}
