# Changelog

## 0.1.30
- CI only. `mutants.yml` now fails when the Linux run misses more than 13 mutants. Baseline: run 40064979549 at fe51c75, 170 mutants, 13 missed (1 `Inner::drop`, 6 `disarm`, 5 `flush` arithmetic, 1 `Region::drop` `>`), 126 caught, 23 timeouts, 8 unviable. Same set as the justified misses in 0.1.21. Still not a required check.

## 0.1.29
- CI only. New `mutants.yml` workflow (manual and weekly, not part of `ci.yml`, not a required check) runs `cargo mutants -f src/region.rs` on Linux so the cfg(unix) mutants are covered in CI. Report-only: it writes caught/missed counts and the missed list to the job summary.

## 0.1.28
- CI only. The advisory `portfolio` job is repinned from 789b012 to 30e012b. Not a required check.

## 0.1.27
- Licence only. LICENSE-MIT copyright holder is now "The dirty-ranges contributors" (was a named person), per CireSnave 2026-10-24 ("Go with your recommendations"). No code change.

## 0.1.26
- Config only. `.coderipper.toml` declares the crate's public API under `[reachability] public_api`, per symbol with a reason (`Mode`, `fault_signals_blocked`, `Scanner::scan`, `Scanner::apply_remote`, `ScanError`, and the private helpers reachable only through them: `write_through`, `rebaseline_chunk`, `blocked_fault_signals`). None of these has a caller inside the crate by design: it is a published library. Nothing removed or narrowed.

## 0.1.25
- Tests and one `let _ =` (no behaviour change). CodeRipper unused-return-values: three `flush` calls in `src/region.rs` tests assert the ranges they return; the conformance `flush` that races writers, `Region`'s `Drop` `protect` (failure already documented as ignorable) and the should-panic `run` call discard explicitly.

## 0.1.24
- No library behaviour change. Every `.rs` file starts with `// SPDX-License-Identifier: MIT OR Apache-2.0` (matches `license` in Cargo.toml); the two `allow` attributes CodeRipper flagged (`dead_code` in `tests/common/mod.rs`, `unconditional_recursion` in `tests/conformance_children.rs`) now carry a `reason`. The CodeRipper `portfolio` job is repinned from c665210 to 789b012 (the merge that stops a default-branch run failing on "ratchet not run").

## 0.1.23
- Docs only. ROADMAP.md: row 7 records the final region mutants run (command, Windows and Linux counts); rows 8-12 record the CodeRipper baseline, one per rule (reachability, unused-return-values, lint-suppression, licence) and the job's own exit.

## 0.1.22
- CI only. Adds an advisory `portfolio` job (the CodeRipper reusable workflow, pinned to c665210). Not a required check.

## 0.1.21
- Tests only (`src/region.rs` `mod tests`; no library code changed). `cargo mutants` 27.1.0 on `src/region.rs`, whole file, Windows `--lib`, at 78a083c: 170 mutants, 125 caught, 22 timeouts, 8 unviable, 15 missed. New tests: `disarm` on a region that ends at the mapping (kills `armed * page` -> `armed + page` on Windows; on Linux that mutant survives because `mprotect` past the mapping does not reliably fail), the signal-mask test now asserts its closures ran (kills `on_a_thread_that_blocks` -> `()`), and `Aliased` removes its file. The Unix-only mutants were re-run on Linux (rust:bookworm): `fault_signals_blocked` x3 and, after the new tests, `on_a_thread_that_blocks` and `Aliased::drop` are caught. 9 remain, each with no observable effect: `Inner::drop` -> `()` (leaks the pool; only a leak checker sees it); `Region::disarm` -> `Ok(())` and `armed > 0` -> `<` (the `Drop` that follows unprotects anyway; only the error return differs, and forcing `mprotect` to fail needs an unmapped range, racy across test threads); `Region::flush` loop condition `j + 1 < len && dirty[j + 1] == dirty[j] + 1` (3 mutants: protect page by page instead of by run, same end state); `i = j + 1` inside `cfg(dirty_ranges_conformance)` (2 mutants, exercised by the conformance controls job); `Region::drop` `armed > 0` -> `>= 0` (a zero-length protect whose failure is ignored).

## 0.1.20
- Docs only. ROADMAP.md marks rows 1-6 done and row 7 partly done (region mutation re-run and the publish remain), records the UNVERIFIED platforms and the published state (0.1.0-0.1.2 on crates.io), and `Cargo.toml` `description` no longer says "name reserved; first release pending".

## 0.1.19
- Docs only. README NetBSD row states the evidence: 5 consecutive green BSD runs on `main` (39651281175, 39808283210, 39821155713, 39822655046, 39824176019); the pkgin install is flaky, 2 of the 5 used the rustup fallback.

## 0.1.18
- CI only. The BSD workflow gains `workflow_dispatch`, so a run on `main` can be started without a push (used to count consecutive NetBSD runs). Triggers otherwise unchanged.

## 0.1.17
- CI only. NetBSD `Install Rust` tries `pkgin install rust` twice (240 s each), then falls back to rustup's NetBSD build from static.rust-lang.org; `Install Rust` now has 25 minutes. The pkgin mirror stalled it again in run 39394658935 (and on its re-run), so no NetBSD test ran. The Tests steps are unchanged.

## 0.1.16
- CI only. NetBSD `Install Rust` retries `pkgin install rust` up to 3 times, each attempt bounded by `timeout 300`, because the pkgin mirror stalled the step to its 20-minute limit in runs 39370039359 and 39381650352 (no test ran in either).

## 0.1.15
- Repo metadata only. Adds `.github/CODEOWNERS` (`* @ciresnave`).

## 0.1.14
- README only. TESTED rows from CI run 39381650147 (PR #19 at `5ba2758`, all 17 jobs success): Linux aarch64 glibc, Linux x86_64 musl, Windows arm64, macOS x86_64. The three BSD rows now cite the green run on `main` (39381482015 at `f326856`) instead of the PR run. Linux aarch64 musl stays UNVERIFIED.

## 0.1.13
- CI only. `test` also runs on `ubuntu-24.04-arm` (Linux aarch64 glibc), `windows-11-arm` and `macos-15-intel` (macOS x86_64); new job `test (linux x86_64 musl)`. README rows move to TESTED only from a green run.

## 0.1.12
- README: FreeBSD 15.1, OpenBSD 7.9 and NetBSD 11.0 (x86-64, QEMU VMs) are TESTED, from `bsd.yml` run 39379433479 on PR #18 at `358ecf8`: 95 lib tests, the conformance suite, and the suite again with the controls compiled in, on all three; the blocked-signal write dies with signal 11 on all three.

## 0.1.11
- Tests only. NetBSD cause found (run 39378372000): `mprotect` cannot raise a mapping that was created `PROT_NONE` (`Permission denied`), so the signal-chaining cases' own handler could never open its page and the store re-faulted forever. With the page mapped read/write first, both chaining cases pass on NetBSD. `mprotect_can_raise_a_mapping_created_prot_none` now records the OS's answer and lets only NetBSD refuse. Not a crate defect: `Region` only toggles protection on a mapping the caller made readable and writable.

## 0.1.10
- `bsd.yml`: per-step timeouts (20 minutes for installing Rust, 30 for each test step), because the NetBSD job sat in `Install Rust` for 30+ minutes on two attempts of run 39370039359 while FreeBSD and OpenBSD finished.

## 0.1.9
- Tests only. NetBSD hung in both signal-chaining cases at the store to the other handler's own page (run 39367038289, step markers): that page was mapped `PROT_NONE`, and the suspect is NetBSD refusing to raise protection on such a mapping. The page is now mapped read/write and then made inaccessible; the other handler reports a failed `mprotect`; a new test `mprotect_can_raise_a_mapping_created_prot_none` asks the OS directly. README: FreeBSD 15.1 and OpenBSD 7.9 rows TESTED from the BSD workflow, NetBSD stays UNVERIFIED (hang).

## 0.1.8
- Tests only: the signal-chaining conformance child prints a `STEP` marker at each stage, so a hung child's output says where it stopped. First BSD VM run (PR #18, run 39366345598): FreeBSD 15.1 and OpenBSD 7.9 pass everything (95 lib tests, conformance with and without controls); NetBSD 11.0 passes the lib tests and the other conformance cases but `chaining_with_the_other_handler_installed_before_ours` and `..._after_ours` hang. Cause not yet known.

## 0.1.7
- New workflow `bsd.yml`: FreeBSD 15.1, OpenBSD 7.9 and NetBSD 11.0 (x86-64) in QEMU VMs through `cross-platform-actions/action` v1.6.0 (pinned by SHA), each running the tests and the conformance suite with the controls compiled in. Not yet run: the README rows stay UNVERIFIED until it is green. README Known limits gain the observation from macOS hang-hunt run 39358930277 that the blocked-signal write likely livelocks on the faulting store.

## 0.1.6
- New CI job `miri (safe modules)`: `diff`, `range`, `scan` and `page_state` tests under Miri with strict provenance, on a pinned nightly (`nightly-2026-09-29`). The unsafe paths cannot run under Miri; `docs/DESIGN.md` now names each as UNVERIFIED-by-Miri with the real-mapping tests that cover it.

## 0.1.5
- Tests and CI only. A conformance child that hangs now has its stacks captured before it is killed (macOS `sample`), so the failure message says where it is stuck. New manual workflow `macOS hang hunt` (`workflow_dispatch`) runs the whole conformance suite repeatedly on a macOS runner and stops at the first failure. Context: `conformance controls (macos-latest)` hung once in `flush_without_reprotect_loses_updates_control` (run 39287647738, `main` at `cf67413`) and passed 5 of 5 re-runs of the same job.

## 0.1.4
- README: status, API list and platform table brought up to date. TESTED rows come only from our own CI (run 39287647738, `main` at `cf67413`): Linux x86_64 glibc, Windows x86_64, macOS arm64 (except the blocked-signal write). Everything else is UNVERIFIED. Records one macOS failure of a conformance control in that run.

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
