# trap-conformance (spike)

Conformance harness for the page-fault write trap that `dirty-ranges` will use to learn which bytes of a
file-backed mapping a program changed. **Spike, not for merge**: it feeds the real crate's test suite.

`cargo run --release` runs every case in its own child process (handlers are process-global; some cases must
die) and prints one `RESULT|<platform>|<case>|PASS/FAIL|<detail>` line per case, plus a table in
`$GITHUB_STEP_SUMMARY`. Exit code is non-zero if any case fails.

## Mechanism under test (`src/trap.rs`)

* Unix: `sigaction(SIGSEGV, SIGBUS)` with `SA_SIGINFO | SA_NODEFER | SA_ONSTACK`; Windows: vectored exception handler.
* Handler uses only atomics, `memcpy` and `mprotect`/`VirtualProtect`; no allocation, no locks; saves/restores
  `errno` / `GetLastError`; unrelated faults are chained to the previously installed handler.
* Pages are page-size aware (runtime query). Two baseline buffers per armed page (double buffer): flush
  re-protects, snapshots into the spare buffer, releases writers, then diffs off the hot path; the snapshot
  becomes the next baseline. The stall window allocates nothing and takes no locks.
* Pool exhaustion is decided at `arm()` time (never in the handler): pages beyond `pool_pages / 2` stay
  writable and untracked; the caller gets the armed count and must scan the rest.

## Cases

| case | what it proves | control that shows the test can fail |
|---|---|---|
| `alloc_hammer` | faults inside the global allocator's critical section (plus Mutex/malloc/free traffic and a concurrent flusher) neither deadlock nor lose updates (mirror == live) | `alloc_hammer_negative_control`: a handler that allocates DOES deadlock |
| `libc_lock_fault` (unix) | faults inside libc `FILE` lock (stdio buffer lives in the trapped region) | - |
| `double_buffer_race` | stores racing the flush window are never lost | `double_buffer_mutation_control`: flush without the re-protect step loses updates |
| `pool_exhaustion` | exhausted pool leaves pages unarmed, reported, scan fallback converges | - |
| `page_size` | alignment and boundary-straddling ranges with the runtime page size | prints whether a hard-coded 4096 would be wrong |
| `errno_restore` | errno / GetLastError preserved across a fault | handler that clobbers and does not restore changes it |
| `sigmask_guard` / `sigmask_blocked_is_fatal` (unix) | SIGSEGV blocked in a thread is fatal (documented); the guard detects it | - |
| `protect_failure_is_loud` | a failing mprotect in the handler kills the process instead of looping | - |
| `chain_ours_first` / `chain_other_first` | another SIGSEGV/SIGBUS (VEH) handler installed before or after ours: both handle their own faults, an unrelated fault reaches the other handler and then the default action | - |
| `stack_overflow_main` / `stack_overflow_thread_trapped` | with the trap armed (and, for the thread case, other threads writing), a stack overflow behaves exactly like the same program without the trap (Rust's overflow message, same exit) | the control run in the same case |

## Not covered (by design or yet)

* `vm.max_map_count` exhaustion (per-page `mprotect` splits mappings; a failing handler `mprotect` is covered only via the
  failure hook).
* Other runtimes' handlers (JVM, Go, sanitizers) beyond the stand-in handler in the chain cases.
* Pages larger than 16 KiB (Linux 64 KiB aarch64 kernels) - no CI runner offers one.
* `apply_remote` (writes through a second alias view) is out of scope here.

## Status per platform (local runs, rustc 1.98-1.99, 2026-10-18)

| platform | status | evidence |
|---|---|---|
| Windows 11 x86_64 MSVC (4 KiB) | TESTED, all cases PASS | local run |
| Linux x86_64 glibc 2.36 (rust:bookworm in Docker/WSL2 kernel 6.18) | TESTED, all PASS | local run |
| Linux x86_64 musl (rust:alpine) | TESTED, all PASS (main-thread overflow prints no message, identical to control) | local run |
| Linux aarch64 glibc | TESTED UNDER QEMU USER-MODE EMULATION ONLY: all PASS. Functional evidence, not hardware (QEMU emulates faults and mprotect) | local run, `docker --platform linux/arm64` |
| Linux aarch64 glibc, real hardware (ubuntu-24.04-arm) | TESTED, all PASS | CI run 39144443214 (read by the PM) |
| Linux x86_64 and aarch64 musl (alpine, real hardware for arm64) | TESTED, all PASS | same run |
| Windows x86_64 and Windows arm64 (real) | TESTED, all PASS | same run |
| FreeBSD (VM) | TESTED, all PASS | same run |
| macOS Apple Silicon (16 KiB pages) and macOS Intel | TESTED: all PASS except `sigmask_blocked_is_fatal`, whose assertion was Linux-shaped (macOS kills with SIGKILL, still fatal); assertion fixed in the next commit, rerun pending | same run |
| NetBSD, OpenBSD | UNVERIFIED | only FreeBSD ran |
| Linux with 64 KiB pages | UNVERIFIED (no runner) | - |
| `vm.max_map_count` exhaustion, real JVM/Go/sanitizer handlers, `apply_remote` alias view | UNVERIFIED / not covered | - |
