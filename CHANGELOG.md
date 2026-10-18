# Changelog

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
