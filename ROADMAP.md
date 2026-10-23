# Roadmap

Status as of crate 0.1.20. Keep this file current: update it in the same PR as the work.

1. **Design PR**: DONE. `docs/DESIGN.md`.
2. **Gate: CireSnave approves Commy plan D1/D2**: DONE (implementation proceeded; approval itself not recorded here).
3. CI workflow, crate skeleton, edition 2024 / rust-version 1.94: DONE.
4. Pure logic (range coalescing, word/SIMD diff, `Scanner`): DONE, mutation-tested (cargo-mutants 27.1.0, 55 mutants, all caught or non-terminating).
5. Platform layer for Windows, Linux, macOS, FreeBSD, NetBSD, OpenBSD: DONE. README TESTED rows: Linux x86_64 glibc+musl, Linux aarch64 glibc, Windows x86_64+arm64, macOS arm64+x86_64, FreeBSD/OpenBSD/NetBSD (x86-64 VMs). UNVERIFIED: Linux aarch64 musl, 64 KiB pages, 32-bit, vm.max_map_count, macOS blocked-signal write (hangs).
6. Scanned mode on real mappings, Trapped mode + handler registry: DONE (`tests/conformance*.rs`).
7. `apply_remote`, mode switching, pool exhaustion behaviour: DONE. cargo-mutants on `src/region.rs`: PARTIAL (168 mutants at 51af928; 18 test gaps now killed, 12 not killed, whole file not re-run since; see CHANGELOG). **0.1.0 publish: NOT DONE** (PM's call; crate is at 0.1.x unpublished-as-far-as-this-lane-knows).
