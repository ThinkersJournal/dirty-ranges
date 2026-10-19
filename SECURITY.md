# Security policy

## What counts as a vulnerability here

`dirty-ranges` contains `unsafe` code (page protection, a signal / exception handler, raw memory access).
**A soundness bug is a security issue**: safe code that can cause undefined behaviour, memory corruption,
a lost or phantom change report that violates the documented contract, or a handler that can deadlock or
crash the host process. Report these privately, not as public issues.

## Reporting

Use GitHub private vulnerability reporting on this repository:
<https://github.com/ThinkersJournal/dirty-ranges/security/advisories/new>

Please include the platform, the Rust version, and the smallest program that shows the problem. We aim to
acknowledge a report within 7 days.

## What happens next

- Confirmed issues get a fix, a new version and a GitHub security advisory.
- Soundness and memory-safety issues are also filed with [RustSec](https://rustsec.org/) (the
  `rustsec/advisory-db`), so `cargo audit` and `cargo deny` users are warned.
- Reporters are credited in the advisory and CHANGELOG unless they ask not to be.

## Supported versions

Only the latest published release receives fixes (the crate is pre-1.0).

## Known limits are not vulnerabilities

Behaviour documented in the README and `docs/DESIGN.md` is by design, e.g. a write from a thread that blocks
`SIGSEGV`/`SIGBUS` to a trapped region is fatal, and platforms listed UNVERIFIED are untested.
