# dirty-ranges

Detect which byte ranges of a memory region changed.

**Status: pre-release (0.1.x), partial.** The API may still change (a breaking change bumps the second number
while we are below 1.0). Page-fault trapping works on Unix and Windows; `Scanner` (scan mode over byte slices)
is safe code with no OS dependency. The design is in [`docs/DESIGN.md`](docs/DESIGN.md).

## What exists

- `Region::arm(ptr, len, pool_pages, alias)`: keep pages read-only; the first write to each faults into a
  handler that copies a baseline and unprotects the page. `alias` is an optional second, writable mapping of
  the same bytes.
- `Region::flush()`: the byte ranges written since the last flush (sorted, coalesced, byte-exact; a store of
  the value already there reports nothing).
- `Region::set_mode(range, Mode::{Trapped, Scanned})`: per page-aligned range, switch between trapping and
  comparing against the baseline at each flush.
- `Region::apply_remote(offset, bytes)`: write someone else's bytes through the alias and into the baseline so
  they are not reported. `Region::rebaseline(offset, len)`: take bytes already in the region as the baseline,
  writing nothing.
- `Region::disarm`, `armed_pages`, `page_size`; `fault_signals_blocked()` (Unix); `Scanner`, `diff`, `coalesce`.

## Platform support

A platform is **TESTED** only when this crate itself passes the trap-conformance cases in our CI on it.
"Harness" is the earlier throwaway spike (not this crate; run 39144443214, 2026-10-18).
"This crate" is our own CI: run 39287647738 on `main` at `cf67413` (2026-10-19), jobs `test` and
`conformance controls` on GitHub-hosted `ubuntu-latest`, `windows-latest` and `macos-latest`.

| Platform | Harness result | This crate |
|---|---|---|
| Linux x86_64, glibc | PASS | **TESTED** (CI) |
| Linux aarch64, glibc / musl; Linux x86_64 musl | PASS | UNVERIFIED (not in our CI) |
| Windows x86_64 | PASS | **TESTED** (CI) |
| Windows arm64 | PASS | UNVERIFIED (not in our CI) |
| macOS arm64 (16 KiB pages) | PASS, except one Linux-shaped assertion (macOS kills with SIGKILL; still fatal) | **TESTED** (CI), except the blocked-signal write below (NOT TESTED) |
| macOS x86_64 | PASS | UNVERIFIED (not in our CI) |
| FreeBSD | PASS (VM) | UNVERIFIED: compiles (`cargo check --target x86_64-unknown-freebsd`), never run |
| NetBSD, OpenBSD | UNVERIFIED | UNVERIFIED: not run |
| Linux with 64 KiB pages | UNVERIFIED | UNVERIFIED |
| 32-bit targets | UNVERIFIED | UNVERIFIED |

Also UNVERIFIED: `vm.max_map_count` exhaustion, and coexistence with real JVM, Go or sanitizer signal handlers.
Miri runs in CI on the safe modules (`diff`, `range`, `scan`, `page_state`); the unsafe paths cannot run under it
and are listed as UNVERIFIED-by-Miri in `docs/DESIGN.md`. Not yet done for a 1.0 gate: a BSD CI job.

macOS CI note: in that same run the `conformance controls (macos-latest)` job failed once, in the control
`flush_without_reprotect_loses_updates_control` (a test that deliberately breaks the crate; the child process
hung). The same job passed on the runs before it (`51af928`, `11665fc`). Not yet investigated.

## Known limits

- **A thread that blocks `SIGSEGV`/`SIGBUS` must not write to a trapped region.** The kernel cannot deliver the fault to our handler, so it kills the process (Linux with `SIGSEGV`, macOS with `SIGKILL`). That is fatal and loud, never silent corruption, and it is tested on Linux. **macOS: on the GitHub `macos-26-arm64` runner image (run 39182659564) the write HANGS instead; the spike saw a kill on other hardware. Documented limit, not fixed; macOS is NOT TESTED for this case.** `fault_signals_blocked()` lets a caller check a thread before it writes. `Region::arm` does not check the arming thread: the writers are what matter.
- Never `read()`/`recv()` from the OS into a trapped region: the kernel reports `EFAULT` or a partial write instead of faulting.
- At most 64 regions per process.

Licensed under MIT OR Apache-2.0.
