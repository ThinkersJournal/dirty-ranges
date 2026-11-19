// SPDX-License-Identifier: MIT OR Apache-2.0
//! `vm.max_map_count` on Linux: every page a writer dirties becomes its own mapping (the fault handler makes
//! one page writable, which splits the tracked mapping), and the kernel refuses a split past the limit.
//! The handler then kills the process rather than loop (a failed `mprotect` there cannot be recovered), and `flush`
//! re-protects the runs, which merges the mappings again. The cases fill the process's own mapping count to
//! within `HEADROOM` of the limit, so they work with whatever `vm.max_map_count` the host has, without root.
// Not under a sanitizer: its shadow memory and internal allocator use mappings of their own and die first.
#![cfg(all(target_os = "linux", not(dirty_ranges_sanitizer)))]

mod common;

use common::*;
use std::time::Duration;

const LIMIT: Duration = Duration::from_secs(120);
/// Mappings left free after the fill: room for the test's own allocations and a few splits.
const HEADROOM: usize = 24;
/// A host whose limit is above this is not filled (millions of mmaps); the case then says so and returns.
const MAX_FILLABLE: usize = 4_000_000;

/// On a 32-bit process, the most address space the fill may take: half of the 4 GiB, a chosen margin for the
/// binary, libraries, stacks and heap. 1 GiB was filled on the 24.04 image (limit 262144); 4 GiB cannot be.
const ADDRESS_SPACE_FILLABLE: u64 = 2 << 30;

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
    // One page per mapping: filling a limit of `max` takes `max * page` bytes of address space. A 32-bit process
    // has 4 GiB at most, so a large limit cannot be reached (ROADMAP row 26: the ubuntu-26.04 image has
    // 1,048,576, i.e. 4 GiB, and `mmap` fails with ENOMEM at VmSize 4193208 kB). Say so and do not run.
    if usize::BITS == 32 && (max as u64) * (page as u64) > ADDRESS_SPACE_FILLABLE {
        eprintln!(
            "vm.max_map_count is {max}: {max} one-page mappings need more than {} MiB of a {}-bit address space: case NOT RUN on this host",
            ADDRESS_SPACE_FILLABLE >> 20,
            usize::BITS
        );
        return false;
    }
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
            if p == libc::MAP_FAILED {
                // ROADMAP row 26: on 32-bit CI this fails with no detail; say what the process had. Do not call
                // `mappings()` here: reading /proc/self/maps allocates, and the first run showed that failing with
                // OutOfMemory once the address space is gone.
                let errno = std::io::Error::last_os_error();
                let status = std::fs::read_to_string("/proc/self/status").unwrap_or_default();
                let vm: Vec<&str> = status.lines().filter(|l| l.starts_with("Vm")).collect();
                panic!(
                    "mmap while filling: {errno}; max_map_count {max}, pointer width {} bits, {vm:?}",
                    usize::BITS
                );
            }
        }
    }
}

/// Dirty every other page from page `first` (`count` writes of `value`, which must differ from what is
/// there), so no two dirty pages are adjacent.
fn dirty_isolated(m: &Mapped, first: usize, count: usize, value: u8) {
    for i in 0..count {
        m.store((first + 2 * i) * m.page, value);
    }
}

/// With the mapping count `HEADROOM` short of the limit, a few isolated dirty pages are fine. This is the
/// control for the case below: it shows the fill leaves a working process, so that case's death is the limit.
#[test]
fn a_few_dirty_pages_fit_under_the_map_limit() {
    let name = "a_few_dirty_pages_fit_under_the_map_limit";
    let Some(outcome) = in_child(name, name, LIMIT, || {
        let m = Mapped::new(80);
        let mut region = m.arm(160);
        if !fill_to_headroom() {
            return;
        }
        dirty_isolated(&m, 0, 4, 1);
        assert_eq!(region.flush().len(), 4);
    }) else {
        return;
    };
    assert!(matches!(outcome, Outcome::Passed { .. }), "{outcome:?}");
}

/// More isolated dirty pages than the limit has mappings left: the handler's `mprotect` gets ENOMEM and the
/// process prints the handler's message and dies by the fault signal (the handler restores the default
/// action and the store faults again). It must not hang or carry on with a page it could not unprotect.
#[test]
fn exhausting_the_map_count_aborts_loudly() {
    let name = "exhausting_the_map_count_aborts_loudly";
    let Some(outcome) = in_child(name, name, LIMIT, || {
        let m = Mapped::new(80);
        let _region = m.arm(160);
        if !fill_to_headroom() {
            return;
        }
        dirty_isolated(&m, 0, 38, 1);
    }) else {
        return;
    };
    eprintln!("map-count exhaustion outcome: {outcome:?}");
    if matches!(outcome, Outcome::Passed { .. }) && outcome.output().contains("case NOT RUN") {
        return; // the host's limit is too large to fill; the child said so
    }
    assert!(
        outcome.died_by_fault(),
        "expected a fatal fault signal: {outcome:?}"
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
            dirty_isolated(&m, 0, 8, round as u8 + 1);
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
