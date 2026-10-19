# dirty-ranges

Detect which byte ranges of a memory region changed.

**Status: pre-release (0.0.x).** The safe core (range coalescing, byte diff, page-state machine) exists; the
OS-specific page-fault trapping and the public `Region` API do not yet. The design is in
[`docs/DESIGN.md`](docs/DESIGN.md).

## Platform support

Page-fault trapping (with automatic fallback to scanning) is the goal on every major OS. A platform is
**TESTED** only when this crate itself passes the trap-conformance cases in CI on it; until then the table
records what the conformance harness (a throwaway spike, not this crate) showed.

| Platform | Harness result | This crate |
|---|---|---|
| Linux x86_64 / aarch64, glibc | PASS (real hardware) | not yet implemented |
| Linux x86_64 / aarch64, musl | PASS | not yet implemented |
| Windows x86_64 / arm64 | PASS | not yet implemented |
| macOS arm64 (16 KiB pages) / x86_64 | PASS, except one Linux-shaped assertion the harness is fixing (macOS kills with SIGKILL; still fatal, as required) | not yet implemented |
| FreeBSD (VM) | PASS | not yet implemented |
| NetBSD, OpenBSD | UNVERIFIED | not yet implemented |
| Linux with 64 KiB pages | UNVERIFIED | not yet implemented |
| 32-bit targets | UNVERIFIED | not yet implemented |

Also UNVERIFIED: `vm.max_map_count` exhaustion, and coexistence with real JVM, Go or sanitizer signal handlers.
Harness run: GitHub hosted runners, run 39144443214 (reported by the portfolio PM, 2026-10-18).

## Known limits

- **A thread that blocks `SIGSEGV`/`SIGBUS` must not write to a trapped region.** The kernel cannot deliver the fault to our handler, so it kills the process (Linux with `SIGSEGV`, macOS with `SIGKILL`). That is fatal and loud, never silent corruption, and it is tested on Linux. **macOS: on the GitHub `macos-26-arm64` runner image (run 39182659564) the write HANGS instead; the spike saw a kill on other hardware. Documented limit, not fixed; macOS is NOT TESTED for this case.** `fault_signals_blocked()` lets a caller check a thread before it writes. `Region::arm` does not check the arming thread: the writers are what matter.
- Never `read()`/`recv()` from the OS into a trapped region: the kernel reports `EFAULT` or a partial write instead of faulting.
- At most 64 regions per process.

Licensed under MIT OR Apache-2.0.
