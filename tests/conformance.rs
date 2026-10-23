// SPDX-License-Identifier: MIT OR Apache-2.0
//! Trap-conformance cases, ported from the SDK lane's spike (`spike/trap-conformance`, PR #3) to run against
//! the real crate. Cases that need a misbehaving crate to prove they can fail (the "controls") need
//! `RUSTFLAGS="--cfg dirty_ranges_conformance"`; the rest run in a normal `cargo test`.
#![cfg(any(unix, windows))]

mod common;

use common::*;
use dirty_ranges::Range;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

fn r(offset: usize, len: usize) -> Range {
    Range { offset, len }
}

#[test]
fn page_size_is_the_runtime_one_and_ranges_straddle_page_boundaries() {
    let page = page_size();
    assert!(
        page.is_power_of_two() && page >= 4096,
        "odd page size {page}"
    );
    let m = Mapped::new(6);
    assert_eq!(m.base as usize % page, 0, "mapping not page aligned");
    let mut region = m.arm(16);
    assert_eq!(region.page_size(), page);
    m.store(2 * page - 1, 0xAA);
    m.store(2 * page, 0xBB);
    m.store(5 * page - 1, 0xCC);
    assert_eq!(region.flush(), vec![r(2 * page - 1, 2), r(5 * page - 1, 1)]);
}

#[test]
fn pool_exhaustion_leaves_the_rest_to_the_caller_and_a_scan_converges() {
    let m = Mapped::new(16);
    let page = m.page;
    let mut region = m.arm(8); // 8 buffers => 4 armed pages
    assert_eq!(region.armed_pages(), 4);
    let mut mirror = vec![0u8; m.len];
    m.store(10, 1); // armed
    m.store(page * 3 + 5, 2); // armed
    m.store(page * 6, 3); // not armed: writable, untracked
    m.store(page * 15 + 1, 4); // not armed
    let ranges = region.flush();
    assert_eq!(ranges, vec![r(10, 1), r(page * 3 + 5, 1)]);
    apply(&mut mirror, m.live(), &ranges);
    assert_ne!(
        mirror,
        m.live(),
        "writes to unarmed pages must be untracked"
    );
    // The caller's fallback: scan what the region does not cover.
    let mut fixed = 0;
    for p in region.armed_pages()..m.len / page {
        let (a, b) = (p * page, (p + 1) * page);
        if mirror[a..b] != m.live()[a..b] {
            mirror[a..b].copy_from_slice(&m.live()[a..b]);
            fixed += 1;
        }
    }
    assert_eq!(fixed, 2);
    assert_eq!(mirror, m.live());
    // A second epoch, after the baseline buffers swapped.
    m.store(11, 9);
    assert_eq!(region.flush(), vec![r(11, 1)]);
}

/// Rounds of: four writers store the round's value (1, 2, 3, ...) at random bytes of three pages while the
/// main thread flushes as fast as it can, so every flush races live stores. After the writers stop and one
/// more flush, every byte that holds this round's value must lie inside a range some flush of the round
/// reported. That is sound without reading values out of the flush: before the round a byte held an earlier
/// value, so a byte that now holds this round's value did change, and a change that went unreported is a
/// lost update. Returns whether no update was lost, and how many flushes ran while the writers were still storing.
fn hammer_the_flush_window(rounds: u8, round_length: Duration) -> (bool, u32) {
    let m = Arc::new(Mapped::new(8));
    let mut region = m.arm(16);
    let hammered = 3 * m.page;
    let mut flushes = 0;
    for round in 1..=rounds {
        let stop = Arc::new(AtomicBool::new(false));
        let writers: Vec<_> = (0..4u64)
            .map(|t| {
                let m = m.clone();
                std::thread::spawn(move || {
                    let mut s = 0xA076_1D64_78BD_642F ^ (t + 1) ^ (u64::from(round) << 32);
                    let until = Instant::now() + round_length;
                    while Instant::now() < until {
                        m.store((rnd(&mut s) as usize) % hammered, round);
                    }
                })
            })
            .collect();
        let mut covered = vec![false; m.len];
        let mark = |covered: &mut Vec<bool>, ranges: Vec<Range>| {
            for r in ranges {
                covered[r.offset..r.offset + r.len].fill(true);
            }
        };
        while !writers.iter().all(|w| w.is_finished()) {
            mark(&mut covered, region.flush());
            flushes += 1; // counted only while writers are still storing: these are the racing flushes
        }
        stop.store(true, Ordering::SeqCst);
        for w in writers {
            w.join().unwrap();
        }
        mark(&mut covered, region.flush());
        for (offset, &byte) in m.live().iter().enumerate() {
            if byte == round && !covered[offset] {
                return (false, flushes);
            }
        }
    }
    (true, flushes)
}

#[test]
fn stores_racing_the_flush_window_are_never_lost() {
    let (consistent, flushes) = hammer_the_flush_window(60, Duration::from_millis(5));
    assert!(
        consistent,
        "a store racing the flush was never reported (lost update)"
    );
    assert!(flushes > 60, "only {flushes} flushes raced the writers");
}

#[cfg(unix)]
#[test]
fn faults_inside_a_libc_file_lock_do_not_deadlock() {
    let m = Arc::new(Mapped::new(16));
    let mut region = m.arm(64);
    let page = m.page;
    unsafe {
        let f = libc::tmpfile();
        assert!(!f.is_null(), "tmpfile failed");
        // stdio's buffer lives in the trapped region, so fwrite faults while holding the FILE lock.
        assert_eq!(
            libc::setvbuf(f, m.base.cast(), libc::_IOFBF, 4 * page),
            0,
            "setvbuf into the trapped region failed"
        );
        let fp = f as usize;
        let writers: Vec<_> = (0..4u8)
            .map(|t| {
                std::thread::spawn(move || {
                    let record = [t + 1; 100];
                    for _ in 0..400 {
                        libc::fwrite(
                            record.as_ptr().cast(),
                            1,
                            record.len(),
                            fp as *mut libc::FILE,
                        );
                    }
                })
            })
            .collect();
        for w in writers {
            w.join().unwrap();
        }
        libc::fflush(f);
        assert_eq!(libc::ftell(f), 4 * 400 * 100);
        assert!(
            !region.flush().is_empty(),
            "no write was trapped inside stdio"
        );
        // Detach the stream from the trapped buffer before teardown.
        drop(region);
        libc::fclose(f);
    }
}

/// The signal mask of the calling thread as a bit set, one bit per signal number below 32.
#[cfg(unix)]
fn thread_mask_bits() -> u32 {
    unsafe {
        let mut current: libc::sigset_t = std::mem::zeroed();
        assert_eq!(
            libc::pthread_sigmask(libc::SIG_BLOCK, std::ptr::null(), &mut current),
            0
        );
        (1..32)
            .filter(|&sig| libc::sigismember(&current, sig) == 1)
            .fold(0u32, |bits, sig| bits | (1 << sig))
    }
}

/// `fault_signals_blocked` must be false for an ordinary new thread, whatever the other threads are doing.
/// On macOS CI a test thread was once reported as blocking SIGSEGV/SIGBUS (when `Region::arm` still refused
/// such threads) and the cause was never found; this keeps spawning threads while others fault, and reports
/// the first start-up mask that has either signal blocked, so a recurrence is diagnosable.
#[cfg(unix)]
#[test]
fn threads_spawned_while_others_fault_do_not_start_with_the_fault_signals_blocked() {
    let m = Arc::new(Mapped::new(8));
    let mut region = m.arm(16);
    let stop = Arc::new(AtomicBool::new(false));
    let writers: Vec<_> = (0..3u64)
        .map(|t| {
            let (m, stop) = (m.clone(), stop.clone());
            std::thread::spawn(move || {
                let mut s = 0x1234_5678_9ABC_DEF1 ^ (t + 1);
                while !stop.load(Ordering::Relaxed) {
                    m.store((rnd(&mut s) as usize) % m.len, (rnd(&mut s) as u8) | 1);
                }
            })
        })
        .collect();
    let mut bad = vec![];
    for round in 0..300 {
        region.flush(); // keep the pages re-protecting so the writers keep faulting
        let probe = std::thread::spawn(thread_mask_bits).join().unwrap();
        let segv_bus = (1u32 << libc::SIGSEGV) | (1u32 << libc::SIGBUS);
        if probe & segv_bus != 0 {
            bad.push((round, probe));
        }
    }
    stop.store(true, Ordering::SeqCst);
    for w in writers {
        w.join().unwrap();
    }
    assert!(
        bad.is_empty(),
        "threads started with fault signals blocked (round, mask bits): {bad:x?}"
    );
}

#[test]
fn errno_is_preserved_across_a_fault() {
    let m = Mapped::new(4);
    let mut region = m.arm(16);
    set_os_error(777);
    m.store(0, 1);
    assert_eq!(get_os_error(), 777, "a plain fault changed errno");
    assert_eq!(region.flush(), vec![r(0, 1)]);
}

#[cfg(dirty_ranges_conformance)]
mod controls {
    use super::*;
    use dirty_ranges::conformance_hooks::*;

    /// With a handler that clobbers errno, the crate's restore is what keeps the application's errno intact;
    /// and with the restore switched off the clobber is visible, which proves the check can fail.
    #[test]
    fn errno_restore_control() {
        let outcome = in_child(
            "controls::errno_restore_control",
            "controls::errno_restore_control",
            Duration::from_secs(60),
            || {
                let m = Mapped::new(4);
                let page = m.page;
                let _region = m.arm(16);
                CLOBBER_ERRNO.store(true, Ordering::SeqCst);
                set_os_error(777);
                m.store(0, 1);
                let kept = get_os_error();
                NO_ERRNO_RESTORE.store(true, Ordering::SeqCst);
                set_os_error(777);
                m.store(page, 1);
                let lost = get_os_error();
                assert_eq!(kept, 777, "restore failed with a clobbering handler");
                assert_ne!(
                    lost, 777,
                    "control: without the restore errno should be clobbered"
                );
            },
        );
        if let Some(o) = outcome {
            assert!(matches!(o, Outcome::Passed { .. }), "{o:?}");
        }
    }

    /// A flush that skips re-protecting the pages it claimed loses updates; if it did not, the race test
    /// above would prove nothing.
    #[test]
    fn flush_without_reprotect_loses_updates_control() {
        let outcome = in_child(
            "controls::flush_without_reprotect_loses_updates_control",
            "controls::flush_without_reprotect_loses_updates_control",
            Duration::from_secs(120),
            || {
                SKIP_FLUSH_PROTECT.store(true, Ordering::SeqCst);
                // Lost updates are a race, so give the mutation up to ten seconds to show itself.
                let deadline = Instant::now() + Duration::from_secs(10);
                loop {
                    let (consistent, _) = hammer_the_flush_window(20, Duration::from_millis(5));
                    if !consistent {
                        return;
                    }
                    assert!(
                        Instant::now() < deadline,
                        "control: skipping the re-protect never lost an update, so the race test cannot fail"
                    );
                }
            },
        );
        if let Some(o) = outcome {
            assert!(matches!(o, Outcome::Passed { .. }), "{o:?}");
        }
    }
}
