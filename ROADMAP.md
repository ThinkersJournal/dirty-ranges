# Roadmap

1. **Design PR** (this): `docs/DESIGN.md`. Blocked on nothing; reviewed by Commy + SDK lanes and PM.
2. **Gate: CireSnave approves Commy plan D1/D2** before any implementation.
3. CI workflow (modelled on PersistAnt), crate skeleton, edition 2024 / rust-version 1.94.
4. Pure logic first: range coalescing + word/SIMD diff, red-first tests.
5. Platform layer for Windows, Linux, macOS, FreeBSD, NetBSD, OpenBSD (CireSnave 2026-10-18); BSD verification needs a runner (ask PM).
6. Scanned mode on real mappings (Windows+Linux), then Trapped mode + handler registry.
7. `apply_remote`, mode switching, pool exhaustion behaviour, cargo-mutants, 0.1.0.
