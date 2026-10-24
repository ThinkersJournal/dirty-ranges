// SPDX-License-Identifier: MIT OR Apache-2.0
//! `vm.max_map_count` on Linux: every page a writer dirties becomes its own mapping (the fault handler makes
//! one page writable, which splits the tracked mapping), and the kernel refuses a split past the limit.
//! The handler then aborts rather than loop (a failed `mprotect` there cannot be recovered), and `flush`
//! re-protects the runs, which merges the mappings again. The cases fill the process's own mapping count to
//! within `HEADROOM` of the limit, so they work with whatever `vm.max_map_count` the host has, without root.
#![cfg(target_os = "linux")]

mod common;

use common::*;
use std::time::Duration;

const LIMIT: Duration = Duration::from_secs(120);
/// Mappings left free after the fill: room for the test's own allocations and a few splits.
const HEADROOM: usize = 24;
/// A host whose limit is above this is not filled (millions of mmaps); the case then says so and returns.
const MAX_FILLABLE: usize = 4_000_000;

fn max_map_count() -> usize {
    std::fs::read_to_string("/proc/sys/vm/max_map_count")
        .expect("read /proc/sys/vm/max_map_count")
        .trim()
        .parse()
        .expect("parse vm.max_map_count")
}

fn mappings() -> usize {
    std::fs::read_to_string("/proc/self/maps")
        .expect("read /proc/self/maps")
        .lines()
        .count()
}

/// Create anonymous one-page mappings with alternating protection (so the kernel cannot merge them) until the
/// process is `HEADROOM` mappings short of the limit. Returns false if the limit is too large to fill.
fn fill_to_headroom() -> bool {
    let max = max_map_count();
    if max > MAX_FILLABLE {
        eprintln!("vm.max_map_count is {max}, above {MAX_FILLABLE}: case NOT RUN on this host");
        return false;
    }
    let page = page_size();
    let mut flip = false;
    loop {
        let have = mappings();
        let free = max.saturating_sub(have);
        if free <= HEADROOM {
            eprintln!("filled: {have} of {max} mappings used");
            return true;
        }
        for _ in 0..free - HEADROOM {
            flip = !flip;
            let prot = if flip {
                libc::PROT_READ
            } else {
                libc::PROT_NONE
            };
            // SAFETY: a fresh anonymous mapping that is never touched or unmapped.
            let p = unsafe {
                libc::mmap(
                    std::ptr::null_mut(),
                    page,
                    prot,
                    libc::MAP_PRIVATE | libc::MAP_ANONYMOUS,
                    -1,
                    0,
                )
            };
            assert_ne!(p, libc::MAP_FAILED, "mmap while filling");
        }
    }
}

/// Dirty every other page of the first `pages` pages (`count` writes), so no two dirty pages are adjacent.
fn dirty_isolated(m: &Mapped, first: usize, count: usize) {
    for i in 0..count {
        m.store((first + 2 * i) * m.page, 1);
    }
}

/// With the mapping count `HEADROOM` short of the limit, a few isolated dirty pages are fine. This is the
/// control for the case below: it shows the fill leaves a working process, so that case's abort is the limit.
#[test]
fn a_few_dirty_pages_fit_under_the_map_limit() {
    let name = "a_few_dirty_pages_fit_under_the_map_limit";
    let Some(outcome) = in_child(name, name, LIMIT, || {
        let m = Mapped::new(80);
        let mut region = m.arm(160);
        if !fill_to_headroom() {
            return;
        }
        dirty_isolated(&m, 0, 4);
        assert_eq!(region.flush().len(), 4);
    }) else {
        return;
    };
    assert!(matches!(outcome, Outcome::Passed { .. }), "{outcome:?}");
}

/// More isolated dirty pages than the limit has mappings left: the handler's `mprotect` gets ENOMEM and the
/// process aborts with the handler's message. It must not hang or carry on with a page it could not unprotect.
#[test]
fn exhausting_the_map_count_aborts_loudly() {
    let name = "exhausting_the_map_count_aborts_loudly";
    let Some(outcome) = in_child(name, name, LIMIT, || {
        let m = Mapped::new(80);
        let _region = m.arm(160);
        if !fill_to_headroom() {
            // not filled: end as the case expects, after the note above
            eprintln!("dirty-ranges: mprotect failed in the fault handler, aborting");
            std::process::abort();
        }
        dirty_isolated(&m, 0, 38);
    }) else {
        return;
    };
    eprintln!("map-count exhaustion outcome: {outcome:?}");
    assert!(
        matches!(outcome, Outcome::Died { signal: Some(s), .. } if s == libc::SIGABRT),
        "expected SIGABRT: {outcome:?}"
    );
    assert!(
        outcome
            .output()
            .contains("mprotect failed in the fault handler, aborting"),
        "{outcome:?}"
    );
}

/// The same total number of isolated dirty pages, but flushed in between: `flush` re-protects the runs, the
/// kernel merges the mappings, and the process stays under the limit. This is the advice for an application
/// with a small `vm.max_map_count`: flush often enough that the dirty pages between flushes stay below it.
#[test]
fn flushing_merges_the_mappings_back() {
    let name = "flushing_merges_the_mappings_back";
    let Some(outcome) = in_child(name, name, LIMIT, || {
        let m = Mapped::new(80);
        let mut region = m.arm(160);
        if !fill_to_headroom() {
            return;
        }
        let used = mappings();
        for round in 0..4 {
            dirty_isolated(&m, 0, 8);
            assert_eq!(region.flush().len(), 8, "round {round}");
            assert!(
                mappings() <= used,
                "round {round}: {} mappings after flush, {used} before the writes",
                mappings()
            );
        }
    }) else {
        return;
    };
    assert!(matches!(outcome, Outcome::Passed { .. }), "{outcome:?}");
}
