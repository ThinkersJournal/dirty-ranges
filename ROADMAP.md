# Roadmap

Status as of crate 0.1.20. Keep this file current: update it in the same PR as the work.

1. **Design PR**: DONE. `docs/DESIGN.md`.
2. **Gate: CireSnave approves Commy plan D1/D2**: DONE (implementation proceeded; approval itself not recorded here).
3. CI workflow, crate skeleton, edition 2024 / rust-version 1.94: DONE.
4. Pure logic (range coalescing, word/SIMD diff, `Scanner`): DONE, mutation-tested (cargo-mutants 27.1.0, 55 mutants, all caught or non-terminating).
5. Platform layer for Windows, Linux, macOS, FreeBSD, NetBSD, OpenBSD: DONE. README TESTED rows: Linux x86_64 glibc+musl, Linux aarch64 glibc, Windows x86_64+arm64, macOS arm64+x86_64, FreeBSD/OpenBSD/NetBSD (x86-64 VMs). UNVERIFIED: Linux aarch64 musl, 64 KiB pages, 32-bit, vm.max_map_count, macOS blocked-signal write (hangs).
6. Scanned mode on real mappings, Trapped mode + handler registry: DONE (`tests/conformance*.rs`).
7. `apply_remote`, mode switching, pool exhaustion behaviour: DONE. cargo-mutants on `src/region.rs`: DONE 2026-10-23 (27.1.0, `cargo mutants -f src/region.rs -j 1 --timeout 60 -- --lib`, Windows, whole file, 170 mutants: 125 caught, 22 timeouts, 8 unviable, 15 missed). Of the 15: 6 killed by new tests or Linux (`fault_signals_blocked` x3, `on_a_thread_that_blocks`, `Aliased::drop`, `Region::disarm` length `*`->`+` on Windows); 9 justified in the 0.1.21 CHANGELOG entry (no behaviour change observable: `Inner::drop` leak, `disarm`/`Drop` redundant unprotect and error-only paths, `flush` per-page vs per-run protect, `cfg(dirty_ranges_conformance)` code). **Publish: 0.1.0, 0.1.1 and 0.1.2 are on crates.io (2026-10-19); 0.1.3 onward is unpublished, with `src/` unchanged since 0.1.2 apart from added tests, so no republish is needed for code.** 0.1.20 is published after merge to correct the crates.io description.
