# Changelog

## 0.0.4
- `Scanner`: scan mode over byte slices (baseline, coalesced changed ranges, `apply_remote` without echo). Safe code, no OS dependency.

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
