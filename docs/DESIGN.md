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

## Concurrency (accepted for spike)
Per-page atomic state: 0 clean, 1 copying, 2 dirty, 3 flushing.
Handler: CAS 0→1, copy twin, unprotect, →2. Flush: CAS 2→3, re-protect, memcpy live→scratch, CAS 3→0, then diff
twin vs scratch off the hot path. Double-buffered twin slots; slot freed after diff.
**The handler never allocates or locks.** Pool is pre-sized; if pool < trapped pages, refuse to arm the excess
pages, put them in Scanned mode and report which.

## Hard rules (documented, and enforced where possible)
- Never `read()`/`recv()` into a trapped view (Linux: EFAULT/partial write; Windows: error 998/10053, socket dies).
- Unsupported platform: compile error or an explicit, documented scan-only fallback (macOS best-effort).

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
- Any soundness bug is treated as a security advisory (CireSnave 2026-10-18 RustSec rule).
- Unsafe surface (to be kept current): `sys/windows` (VEH, `VirtualProtect`, `MapViewOfFile` aliasing),
  `sys/unix` (`sigaction`, `mprotect`, `mmap` aliasing), `region` (raw ptr+len slices, twin/scratch pool, atomics-guarded handler state).

## Testing
Red-first. Real `mmap`/`MapViewOfFile` tests on Windows+Linux (Miri not applicable to the handler); pure
diff/coalesce logic gets property tests + `cargo-mutants` before 0.1.0. CI matrix Windows/Linux/macOS.
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
