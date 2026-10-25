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
| Linux aarch64, glibc | PASS | **TESTED** (CI, `ubuntu-24.04-arm`; run 39381650147 on PR #19 at `5ba2758`) |
| Linux x86_64, musl | PASS | **TESTED** (CI, `x86_64-unknown-linux-musl` on `ubuntu-latest`; same run) |
| Linux aarch64, musl | PASS | **TESTED** (CI, `aarch64-unknown-linux-musl` on `ubuntu-24.04-arm`, binary linked against musl, run on the glibc host; run 40127427374 on PR #39 at `2cd3e1c`, 97 lib tests passed) |
| Windows x86_64 | PASS | **TESTED** (CI) |
| Windows arm64 | PASS | **TESTED** (CI, `windows-11-arm`; same run) |
| macOS arm64 (16 KiB pages) | PASS, except one Linux-shaped assertion (macOS kills with SIGKILL; still fatal) | **TESTED** (CI), except the blocked-signal write below (NOT TESTED) |
| macOS x86_64 | PASS | **TESTED** (CI, `macos-15-intel`; same run) |
| FreeBSD 15.1 | PASS (VM) | **TESTED** (CI, `bsd.yml`, QEMU VM, rustc 1.98.1; run 39381482015 on `main` at `f326856`) |
| OpenBSD 7.9 | UNVERIFIED | **TESTED** (CI, `bsd.yml`, QEMU VM, rustc 1.94.1; same run) |
| NetBSD 11.0 | UNVERIFIED | **TESTED** (CI, `bsd.yml`, QEMU VM; 5 consecutive green runs on `main`, 39651281175 to 39824176019; rustc 1.96.1 from pkgin or 1.99.1 from the rustup fallback, because the pkgin install is flaky: 2 of the 5 fell back). NetBSD refuses `mprotect` on a mapping created `PROT_NONE`; a test fixture had relied on it |
| Linux with 64 KiB pages (aarch64) | UNVERIFIED | **TESTED under emulation (functional only, not hardware)** (CI, `qemu-64k.yml`: Ubuntu `7.0.0-38-generic-64k` kernel booted under `qemu-system-aarch64` 8.2.2 TCG, `-cpu neoverse-n1`, on `ubuntu-24.04-arm`; the log shows `getconf PAGESIZE = 65536` and `KernelPageSize: 64 kB`; the suite (`aarch64-unknown-linux-musl` binaries: conformance 6, alloc 1, children 6, map_count 3, lib 97) passed, run 40165909272 on PR #45 at `9608c7a`). Timing, TLB and hardware behaviour are not covered. `-cpu max` hung the guest kernel before init and `cortex-a72` stalled the first conformance test; neither was investigated |
| 32-bit x86 (`i686-unknown-linux-gnu`, `i686-pc-windows-msvc`) | UNVERIFIED | **TESTED** (CI: 32-bit processes on a 64-bit host kernel, `ubuntu-latest` and `windows-latest`; run 40127550486 on PR #40 at `c5414f3`, 97 and 95 lib tests passed) |
| Other 32-bit targets (32-bit ARM, 32-bit on a 32-bit kernel) | UNVERIFIED | UNVERIFIED |

`vm.max_map_count` exhaustion: **TESTED on Linux** (CI, `tests/conformance_map_count.rs`, run 40132071381 on PR #41 at `58ea54d`, green on `ubuntu-latest`, `ubuntu-24.04-arm` and x86_64 musl; the process is filled to 24 mappings short of the limit, which was 262144 on `ubuntu-latest` in the log of the earlier run 40127977219; the later runs hide a passing test's output, so the fill on the arm and musl legs was not seen). Isolated dirty pages beyond what is left kill the process with the handler's message, and `flush` merges the mappings back. Not run on other systems (macOS and the BSDs have no such limit file).

Mapped file on tmpfs (Linux): **TESTED** (CI `test-tmpfs`: the whole suite with `TMPDIR=/dev/shm`, and `tests/conformance_tmpfs.rs` asserts by `statfs` that the temp directory is tmpfs; run 40169611110 on PR #47 at `b930b4c`). Not tested: WSL2 `/mnt/c`, network or FUSE file systems, antivirus on Windows.

Also UNVERIFIED: coexistence with real JVM, Go or sanitizer signal handlers.
Miri runs in CI on the safe modules (`diff`, `range`, `scan`, `page_state`); the unsafe paths cannot run under it
and are listed as UNVERIFIED-by-Miri in `docs/DESIGN.md`. The BSD jobs (`bsd.yml`) were green for FreeBSD, OpenBSD and NetBSD in the run above (VMs, x86-64 only; run on `main`).

macOS CI note: in that same run the `conformance controls (macos-latest)` job failed once, in the control
`flush_without_reprotect_loses_updates_control` (a test that deliberately breaks the crate; the child process
hung). The same job passed on the runs before it (`51af928`, `11665fc`). Not yet investigated.

## Known limits

- **A thread that blocks `SIGSEGV`/`SIGBUS` must not write to a trapped region.** The kernel cannot deliver the fault to our handler, so it kills the process (Linux with `SIGSEGV`, macOS with `SIGKILL`). That is fatal and loud, never silent corruption, and it is tested on Linux. **macOS: on the GitHub `macos-26-arm64` runner image (run 39182659564) the write HANGS instead; the spike saw a kill on other hardware. Documented limit, not fixed; macOS is NOT TESTED for this case. Measured again 2026-10-24 (run 40137219741, `macos-blocked-signal.yml`): the write HANGS in 5 of 5 rounds on `macos-latest` (arm64) and in 5 of 5 on `macos-15-intel` (x86_64), until the test's 90 s limit kills the child. So it is a hang on both macOS architectures of the hosted runners, not a one-off.** `fault_signals_blocked()` lets a caller check a thread before it writes. `Region::arm` does not check the arming thread: the writers are what matter.
  *Likely mechanism (not proven):* in the macOS hang-hunt run 39358930277 the `sample` stacks of the hung child show the writing thread on a Rust frame in the test binary, not waiting in a system call, while the other threads wait: it looks like the faulting store being re-executed forever (a livelock) rather than the process being killed. The frame is unsymbolised.
- Never `read()`/`recv()` from the OS into a trapped region: the kernel reports `EFAULT` or a partial write instead of faulting.
- At most 64 regions per process.

Licensed under MIT OR Apache-2.0.
