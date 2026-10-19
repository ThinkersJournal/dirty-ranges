# Changelog

## 0.1.3
- Tests only. `cargo mutants` 27.1.0 on `src/region.rs`, no exclusions, Windows `--lib`, at 51af928: 168 mutants, 106 caught, 22 timeouts (non-terminating), 7 unviable, 33 missed. 18 were test gaps and are now killed by new tests (error text and `source`, `set_mode` page selection and offsets, `rebaseline` byte range, `apply_remote` page boundary, non-adjacent pages re-armed, drop/disarm unprotect every page); 3 are in `fault_signals_blocked`, which is Unix-only and was re-run on Linux (3 of 3 caught). The other 12 are not killed (re-run of the touched functions after the new tests, `apply_remote` re-run alone; the whole file was not re-run):
  - `Inner::drop` -> `()`: leaks the baseline pool; no test can see a leak.
  - `Region::disarm` (`Ok(())`, `armed > 0` with `<`, `armed * page` with `+`): `disarm` consumes `self`, so `Drop` unprotects the pages anyway; only a failing `VirtualProtect`/`mprotect` would show it, and that needs fault injection.
  - `Drop for Region` `armed > 0` -> `>=`: an unprotect of length 0, whose failure is ignored.
  - `flush` run grouping (`<` -> `>`, `+` -> `*` in the adjacency test): no grouping means one `protect` per page; same result, more calls.
  - `flush` `i = j + 1` (two mutants): inside `#[cfg(dirty_ranges_conformance)]`, so not compiled here; the conformance controls job exercises it.
  - Test-module helpers `on_a_thread_that_blocks` and `Aliased::drop`.

## 0.1.2
- New `Region::rebaseline(offset, len)`: set the baseline of a range from the bytes that are in the region now, writing nothing to the region. For a write someone else already made (for example another process on the shared file) that must not be reported as local. `apply_remote` writes through the alias and can overwrite a newer foreign write; `rebaseline` cannot.

## 0.1.1
- New `Mode { Trapped, Scanned }` and `Region::set_mode(range, mode)`: switch page-aligned parts of a region between trapped (read-only, first write faults) and scanned (writable, compared with the baseline at each flush) at runtime. Every page starts trapped. Writes made before a switch are still reported by the next flush. `PageState::abort_flush`.

## 0.1.0
- Breaking: `Region::arm` takes a fourth argument, `alias: Option<*mut u8>` (a second writable mapping of the region). New `Region::apply_remote(offset, &[u8])` writes through the alias and updates the baseline so the write is not reported (`Error::NoAlias` without an alias, `Error::OutOfRegion` outside the region, nothing written on error). New `Region::disarm(self)` unprotects explicitly and reports failure. Tested on Windows; Unix by CI only.

## 0.0.9
- `SECURITY.md`: private reporting via GitHub, RustSec filing for soundness bugs, credit policy.

## 0.0.8
- Windows page-fault trapping: `Region` now works on Windows (`VirtualProtect` plus a vectored exception handler that saves and restores the last error and chains unrelated exceptions). The conformance suite runs on Windows too (including its controls, the VEH chaining cases and the stack-overflow cases); the Unix-only cases (libc `FILE` lock, blocked-signal write, signal-mask probe) stay Unix-only. New CI check `conformance controls (windows-latest)`.

## 0.0.7
- Known limit: on the GitHub `macos-26-arm64` runner image a write from a thread that blocks `SIGSEGV`/`SIGBUS` to a trapped region hangs instead of being killed (run 39182659564). The test accepts a hang on macOS only and prints the outcome; macOS stays NOT TESTED for that case.
- Rest of the conformance suite: `tests/conformance_children.rs` (blocked-signal write is fatal, chaining with another handler installed before and after ours, stack overflow with the trap armed behaves as without it, failed `protect` in the handler kills the process) and `tests/conformance_alloc.rs` (allocator that faults in its own critical section, with a control showing an allocating handler deadlocks). Child-process cases run through `tests/common`.

## 0.0.6
- Conformance suite (`tests/conformance.rs`), ported from the SDK lane's spike: page size and boundary-straddling ranges, pool exhaustion with scan fallback, stores racing the flush window, faults inside a libc `FILE` lock, `errno` preserved. Controls (compiled only with `--cfg dirty_ranges_conformance`) show the checks can fail: clobbered `errno`, and a flush that skips re-protecting. New CI job `conformance`.
- `fault_signals_blocked()` (Unix): whether the calling thread blocks `SIGSEGV`/`SIGBUS`. A thread that blocks them must not write to a trapped region (the process is killed: fatal, never silent corruption); `Region::arm` does not check the arming thread (PM ruling 2026-10-18: the writers are what matter).

## 0.0.5
- Unix page-fault trapping: `Region::arm` / `flush` / `armed_pages` / `page_size`, a process-wide `SIGSEGV`/`SIGBUS` handler with a lock-free multi-region registry (64 slots), `mprotect`, and double-buffered baselines. `PageState::abort_capture`. Linux glibc tested locally (Docker); macOS, FreeBSD, NetBSD only type-checked. Not yet: Windows, mode switching, `apply_remote`, the conformance cases in CI.

## 0.0.4
- `Scanner`: scan mode over byte slices (baseline, coalesced changed ranges, `apply_remote` without echo). Safe code, no OS dependency. Mutation-tested (cargo-mutants 27.1.0, no exclusions): 55 mutants over range/diff/scan, all caught or non-terminating.

## 0.0.3
- Docs: README platform table (TESTED/UNVERIFIED) and the trap-conformance results and handler rules in `docs/DESIGN.md`.

## 0.0.2
- Safe core, no OS code yet: `Range`, `coalesce`, `diff` (byte-exact, block-skipping), and the atomic `PageState` machine with Loom tests (`RUSTFLAGS="--cfg loom" cargo test --release loom_`). New CI job `loom`.

## 0.0.1
- Add draft design (`docs/DESIGN.md`) and `ROADMAP.md`. 0.1.0 is the first real release.
- Plan: first-class support for Windows, Linux, macOS, FreeBSD, NetBSD, OpenBSD.
- Add CI (fmt, clippy, test on Linux/Windows/macOS, doc, MSRV 1.94, cargo-deny), `deny.toml`, edition 2024, `rust-version = "1.94"`, and an empty lib skeleton.

## 0.0.0
- Reserve the crate name.
