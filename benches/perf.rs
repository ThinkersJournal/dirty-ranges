// SPDX-License-Identifier: MIT OR Apache-2.0
//! Measurements for the DESIGN performance figures (fault, re-protect, diff, scan). Plain `std::time`, no
//! benchmark framework: `cargo bench --bench perf`. Every figure is the median of several runs and is printed with
//! its predicate; nothing here asserts a target.
use dirty_ranges::{Range, Region, Scanner, diff};
use memmap2::MmapMut;
use std::hint::black_box;
use std::time::{Duration, Instant};

const RUNS: usize = 9;

fn median(mut v: Vec<Duration>) -> Duration {
    v.sort();
    v[v.len() / 2]
}

fn time<F: FnMut()>(mut f: F) -> Duration {
    median(
        (0..RUNS)
            .map(|_| {
                let t = Instant::now();
                f();
                t.elapsed()
            })
            .collect(),
    )
}

fn page_size() -> usize {
    // Region reports it; arm a one-page region on a scratch mapping.
    let mut m = MmapMut::map_anon(1 << 16).unwrap();
    let r = unsafe { Region::arm(m.as_mut_ptr(), 1 << 16, 2, None) }.unwrap();
    r.page_size()
}

fn bench_diff() {
    let len = 64 << 20;
    let old = vec![0xA5u8; len];
    let mut new = old.clone();
    let mut out: Vec<Range> = Vec::new();
    let t = time(|| {
        out.clear();
        diff(black_box(&old), black_box(&new), 0, &mut out);
    });
    println!(
        "diff, {} MiB identical:               {:>8.2} GB/s  ({:?})",
        len >> 20,
        len as f64 / t.as_secs_f64() / 1e9,
        t
    );
    let page = 4096;
    for i in (0..len).step_by(page) {
        new[i + 100] ^= 0xFF;
    }
    let t = time(|| {
        out.clear();
        diff(black_box(&old), black_box(&new), 0, &mut out);
    });
    println!(
        "diff, {} MiB, one byte per 4 KiB page: {:>7.2} GB/s  ({:?}, {} ranges)",
        len >> 20,
        len as f64 / t.as_secs_f64() / 1e9,
        t,
        out.len()
    );
}

fn bench_scan() {
    let len = 128 << 20;
    let live = vec![0x5Au8; len];
    let mut s = Scanner::new(&live);
    let t = time(|| {
        black_box(s.scan(black_box(&live)).unwrap());
    });
    println!(
        "Scanner::scan, {} MiB unchanged:      {:>8.2} ms  ({:.2} GB/s)",
        len >> 20,
        t.as_secs_f64() * 1e3,
        len as f64 / t.as_secs_f64() / 1e9
    );
}

fn bench_region() {
    let page = page_size();
    let pages = 1024;
    let len = pages * page;
    let mut m = MmapMut::map_anon(len).unwrap();
    let base = m.as_mut_ptr();
    let mut r = unsafe { Region::arm(base, len, 2 * pages, None) }.unwrap();
    assert_eq!(r.armed_pages(), pages, "pool too small for the bench");
    let mut fault = Vec::new();
    let mut flush = Vec::new();
    let mut idle_flush = Vec::new();
    for round in 0..RUNS + 1 {
        let t = Instant::now();
        for p in 0..pages {
            unsafe { base.add(p * page).write_volatile(round as u8 + 1) };
        }
        let faulted = t.elapsed();
        let t = Instant::now();
        let ranges = r.flush();
        let flushed = t.elapsed();
        assert_eq!(ranges.len(), pages);
        let t = Instant::now();
        let none = r.flush();
        let idle = t.elapsed();
        assert!(none.is_empty());
        if round > 0 {
            fault.push(faulted);
            flush.push(flushed);
            idle_flush.push(idle);
        }
    }
    let (f, fl, id) = (median(fault), median(flush), median(idle_flush));
    println!("page size {page} B, {pages} pages armed");
    println!(
        "first store to a clean page (fault):  {:>8.2} us/page  (median of {RUNS} rounds of {pages} stores)",
        f.as_secs_f64() * 1e6 / pages as f64
    );
    println!(
        "flush, {pages} dirty pages (re-protect + report): {:>6.3} ms  ({:.2} us/page)",
        fl.as_secs_f64() * 1e3,
        fl.as_secs_f64() * 1e6 / pages as f64
    );
    println!(
        "flush, nothing dirty:                 {:>8.3} ms",
        id.as_secs_f64() * 1e3
    );
}

fn main() {
    println!(
        "dirty-ranges {} on {} {}, {} runs, median",
        env!("CARGO_PKG_VERSION"),
        std::env::consts::OS,
        std::env::consts::ARCH,
        RUNS
    );
    bench_diff();
    bench_scan();
    bench_region();
}
