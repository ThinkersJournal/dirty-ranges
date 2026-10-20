# dirty-ranges — design (draft, pre-implementation)

Status: **draft for review. No implementation until CireSnave approves the Commy plan (D1/D2).**
Sources (2026-10-18): Commy lane (`[ANSWER]`), commy-sdk-rust lane (`[FINDING]`, spikes `wspike`/`lspike`),
PM bootstrap answer. Spike numbers below are theirs, measured on WSL2/musl (Linux) and Windows; not re-measured here.

## Purpose
Given a read-write mapped memory region, report which byte ranges **the owning process** changed since the
last flush, as coalesced `(offset, len)` ranges. No payload, no network, no Commy dependency.

Consumers: `commy-sdk-rust` (every SDK instance maps a file and finds its own writes) and the Commy server.

## Goals
- Byte-exact, sorted, coalesced `Range { offset, len }` output; a same-value store yields no range.
- Two detection modes per page-rounded sub-range, switchable at runtime:
  - **Trapped**: pages read-only; first write faults (Windows VEH / Linux SIGSEGV); handler copies a twin, unprotects, marks dirty.
  - **Scanned**: no protection; snapshot, compare on `flush()`/`poll()`.
  - trap→scan: unprotect, then snapshot. scan→trap: protect, diff once, drop snapshot.
- `apply_remote(range, bytes)` writes through a separate RW alias and updates twin/snapshot, so remote data never echoes.
- Sync core, `std` only, no async runtime. `Region: Send + Sync`; faults from any thread; one flusher at a time.
- Multiple regions per process behind one global handler (chains to the previous handler for foreign faults; removed on drop).

## Non-goals / anti-goals
- No payload capture, no transport, no persistence. No `no_std`.
- Detecting writes by *other* processes (only the owner's writes are tracked).
- Never silently do nothing on an unsupported platform.

## Platform support (CireSnave, 2026-10-18: all major OSes, at the least Windows, Linux, BSD, macOS)
First-class targets, all with Trapped and Scanned modes: **Windows, Linux, macOS, FreeBSD, NetBSD, OpenBSD.**
No target is "best effort" by design; where a platform cannot be verified yet it is listed UNVERIFIED with
the reason, never silently downgraded.
- `sys/windows`: VEH + `VirtualProtect`; views via `MapViewOfFile`.
- `sys/unix`: shared `sigaction` (`SA_SIGINFO`) + `mprotect` + double `mmap(MAP_SHARED)` alias for Linux, macOS and
  the BSDs. Per-OS shims: which signals are caught (`SIGSEGV` and `SIGBUS` on macOS/BSD), the fault-address field of
  `siginfo_t`/`ucontext`, runtime page size (16 KB on Apple Silicon).
- macOS alternative if signals prove inadequate: Mach `EXC_BAD_ACCESS` exception port served by a handler thread.
- Scan-only fallback exists for any other target and for pages that cannot be armed; selecting it is explicit and reported.
- Not-yet-verified (from lane knowledge, untested): macOS, FreeBSD, NetBSD, OpenBSD, aarch64. CI covers Linux, Windows,
  macOS on hosted runners; the BSDs need a VM or self-hosted runner (PM request, EXPECTATIONS 5.1g/h) and
  `cross`/`cargo check --target` builds as a floor.

## API sketch (proposal; names are open)
```rust
pub struct Range { pub offset: usize, pub len: usize }          // optional serde feature
pub enum Mode { Trapped, Scanned }
pub struct Region { .. }                                         // Send + Sync
impl Region {
    pub unsafe fn new(ptr: *mut u8, len: usize, mode: Mode) -> Result<Region, Error>;
    pub fn set_mode(&mut self, sub: core::ops::Range<usize>, mode: Mode) -> Result<(), Error>;
    pub fn flush(&mut self) -> Vec<Range>;                       // or poll()
    pub fn apply_remote(&self, range: Range, bytes: &[u8]) -> Result<(), Error>;
}
```
Two views of one file: a protected view for user code and an RW alias for the SDK (the alias is how the
owner's own writes avoid raising events).

### Trap-conformance results (SDK lane harness, 2026-10-18, run 39144443214, GitHub hosted hardware)
TESTED PASS: Linux x86_64 and aarch64 (glibc and musl), Windows x86_64 and arm64, FreeBSD (VM), macOS arm64
(16 KiB pages) and x86_64. The one failing case was a Linux-shaped assertion (macOS kills with SIGKILL when
SIGSEGV is blocked; still fatal) and is being fixed in the harness.
UNVERIFIED (the 0.1.0 gate reads this list): macOS, a write from a thread that blocks SEGV/BUS (HANGS on the GitHub macos-26-arm64 runner image, run 39182659564; the spike saw a kill elsewhere; limit documented, not fixed), NetBSD, OpenBSD, Linux with 64 KiB pages, `vm.max_map_count` exhaustion, real JVM/Go/sanitizer
handlers, 32-bit. The README table is the record; a platform is TESTED for *this crate* only once the crate
passes the same cases in CI (the per-OS release gate).

Design facts the harness proved (each becomes a requirement and a test):
- The handler uses only atomics, `memcpy` and `mprotect`. A handler that allocates deadlocks (negative control).
- The flush stall window allocates nothing and takes no locks.
- `errno` / `GetLastError` must be saved and restored by the handler (the control without restore fails).
- Use the runtime page size (macOS arm64 is 16 KiB).
- Refuse to arm or flush from a thread that has SIGSEGV/SIGBUS blocked (such a fault is fatal).

## Concurrency (accepted for spike)
Per-page atomic state: 0 clean, 1 copying, 2 dirty, 3 flushing.
Handler: CAS 0→1, copy twin, unprotect, →2. Flush: CAS 2→3, re-protect, memcpy live→scratch, CAS 3→0, then diff
twin vs scratch off the hot path. Double-buffered twin slots; slot freed after diff.
**The handler never allocates or locks.** Pool is pre-sized; if pool < trapped pages, refuse to arm the excess
pages, put them in Scanned mode and report which.

## Hard rules (documented, and enforced where possible)
- Never `read()`/`recv()` into a trapped view (Linux: EFAULT/partial write; Windows: error 998/10053, socket dies).
- Unsupported platform (outside the list above): explicit, documented scan-only fallback or compile error, never silent.

## Performance targets
Fault ≤15 µs (spike: 11–12 µs Linux); re-protect 1024 pages ≈0.1 ms (spike 116–145 µs); diff word/SIMD ≥2 GB/s
(spike naive diff: 0.3 GB/s); scan 128 MB ≈18 ms (memcmp).

## Unsafe policy (approved by PM 2026-10-18, with conditions)
- `#![deny(unsafe_code)]` and `#![deny(unsafe_op_in_unsafe_fn)]` at the crate root.
- `#[expect(unsafe_code, reason = "...")]` (reason mandatory, no bare `allow`) only on the `sys/` and `region` modules.
- Every `unsafe` block carries a `// SAFETY:` comment stating the invariant.
- `diff` and range coalescing stay safe code and carry the mutation testing.
- Miri runs on what Miri can run. The VEH / SIGSEGV / `mprotect` paths cannot run under Miri and are named
  **UNVERIFIED-by-Miri**, each with its real-mapping test.
- **Miri status (CI job `miri (safe modules)`, nightly pinned):** `diff`, `range`, `scan` and `page_state` run
  under Miri with strict provenance (45 tests at 0.1.5; `cargo miri test --lib -- diff:: range:: scan:: page_state::`).
- **UNVERIFIED-by-Miri** (cannot run under Miri: they need real page protection and fault handlers). Each is
  covered by a real-mapping test instead:
  - `sys` (`protect`, `install_handler`, `page_size`, the `SIGSEGV`/`SIGBUS` handler and the Windows vectored
    handler): `tests/conformance*.rs` (every case, plus the controls job).
  - `region::on_fault` / `handle` (the fault handler) and the registry `SLOTS`/`ACTIVE`: `region::tests`
    (`writers_on_many_threads_are_all_seen`, `two_regions_in_one_process_are_tracked_independently`) and the
    conformance cases.
  - `Inner::buffer`, `write_through`, `rebaseline_chunk` (raw pointer arithmetic into the baseline pool) and
    `Region::{arm, flush, set_mode, apply_remote, rebaseline, disarm}` and `Drop`: `region::tests`
    (for example `one_written_byte_is_reported_once`, `a_remote_write_to_a_page_with_a_baseline_is_not_reported`,
    `a_write_made_while_scanned_survives_the_switch_back_to_trapped`,
    `rebaseline_never_overwrites_a_concurrent_foreign_write`).
- Any soundness bug is treated as a security advisory (CireSnave 2026-10-18 RustSec rule).
- Unsafe surface (to be kept current): `sys/windows` (VEH, `VirtualProtect`, `MapViewOfFile` aliasing),
  `sys/unix` (`sigaction`, `mprotect`, `mmap` aliasing), `region` (raw ptr+len slices, twin/scratch pool, atomics-guarded handler state).

## Testing
Red-first. Real `mmap`/`MapViewOfFile` tests on Windows+Linux (Miri not applicable to the handler); pure
diff/coalesce logic gets property tests + `cargo-mutants` before 0.1.0. CI matrix Windows/Linux/macOS; BSD runs per the platform section.
Untested so far (from the lanes): glibc, macOS, aarch64, /mnt/c, tmpfs, userfaultfd-wp, second real user,
antivirus, pool exhaustion.

## Conventions (PM)
edition 2024, rust-version 1.94, newest crates.io deps (`libc`, `windows-sys`, optional `memmap2`), no path/git
deps, MIT OR Apache-2.0, CHANGELOG, first release 0.1.0 (PM allocates versions and publishes).

## Open questions
1. (resolved) Unsafe policy.
2. `flush()` returning `Vec` vs caller-provided buffer (allocation-free flush?).
3. Page-size handling (4 KB vs 16 KB aarch64/macOS, Windows allocation granularity 64 KB for views).
4. File-backed vs anonymous regions: does `Region` own the mapping or only borrow ptr+len? (SDK lane: ptr+len.)
