# Patent sweep: dirty-ranges 0.1.45 (0.1.x line) @ 8777411
Date: 2026-10-25; Run by: dirty-ranges lane, Claude Sonnet 5.5; Method: AI-assisted search and read; NOT legal advice; NOT a freedom-to-operate opinion.
Covers the 0.1 version line (src/ unchanged since 0.1.29; 0.1.41 is on crates.io). Process: `C:\Projects\PATENT-SWEEP-PROCESS.md`.

## Ideas swept
1. Detect which byte ranges of a memory-mapped region the owning process changed since the last flush, reported as sorted, coalesced `(offset, len)` ranges.
2. Write-protect pages; the first store faults in a signal handler / vectored exception handler that copies the page (a twin), unprotects it and marks it dirty.
3. The fault handler allocates nothing and takes no locks: per-page atomic state (clean, copying, dirty, flushing) and a pre-sized twin pool.
4. Flush: re-protect the dirty pages, copy live to scratch, then diff twin against scratch off the hot path; a same-value store yields no range.
5. Double-buffered twin slots so a store racing the flush window is never lost.
6. Two detection modes per page-rounded sub-range, switchable at run time: Trapped (protection) and Scanned (snapshot and compare), with a defined hand-over in each direction.
7. If the pool holds fewer pages than the region, track only part of it and leave the rest writable and untracked, reported to the caller for scanning.
8. Remote updates written through a second writable alias of the same bytes, with the twin/snapshot updated, so remote data never appears as a local change (no echo).
9. One process-global fault handler serving many regions, chaining to the previous handler for foreign faults, removed on drop.
10. Saving and restoring `errno` / `GetLastError` in the handler; runtime page size (4 KiB, 16 KiB on Apple Silicon).
11. Block-skipping (64-byte `memcmp`) diff and range coalescing.

## Queries (source, query, date)
All 2026-10-25, via a web search tool restricted to patents.google.com (no direct USPTO/EPO/WIPO query interface was used).
- Q1: `patent write-protect memory pages page fault handler copy twin page dirty tracking diff modified bytes distributed shared memory` (ideas 2, 4)
- Q2: `patent memory-mapped file detect modified byte ranges write fault handler unprotect page record dirty range transmit` (1, 2)
- Q3: `patent incremental checkpoint write-protect pages copy-on-write twin page compare changed bytes dirty page tracking user-space` (2, 4)
- Q4: `patent switch between page protection trap mode and snapshot compare scan mode per memory region change detection` (6)
- Q5: `patent remote update applied through second writable alias mapping so local change detection does not echo remote writes shared memory synchronization` (8)
- Q6: `patent "twin" page write fault handler lock-free pre-allocated pool dirty page flush re-protect concurrent writers diff off critical path` (3, 5)
- Q7: `patent shared memory file mapping synchronize changes between processes machines detect changed regions page protection delta replication` (1, 8)

Ideas 7, 9, 10 and 11 got no query of their own (signal-handler chaining, page-size lookup and `memcmp` block skipping are long-standing practice; see Limitations).

## Hits
Claims were read from the Google Patents page where the tool returned them. "Partial" means the tool returned only excerpts.

| # | patent | title | assignee | status | claim(s) read | overlap | our difference | verdict |
|---|---|---|---|---|---|---|---|---|
| 1 | US6298419B1 | Protocol for software distributed shared memory with memory scaling | Compaq / now HPE | Expired (filed 1998-03-26, anticipated expiry 2018-03-26) | independent 1, 9, 17, 27 (preambles); 6/14/22/26 (partial) | twin-vs-working-copy "diffing" in the description; claims are about extra memory storage across DSM nodes, home-node write-back, a difference bit | no DSM nodes, home nodes or additional-storage scheme; owner-only change detection | clear (expired) |
| 2 | US6574749B1 (granted parent of US20030167420A1, abandoned) | Reliable distributed shared memory | Nortel / now RPX Clearinghouse | Priority 1999-10-29; a 20-year term ends no later than 2019-10-29 (not verified on the register) | all 10 claims, in full (tool `patent-claims.py`): independent 1, 3, 4, 8, 9, 10 | description has write-protect, fault, twin, diff; claims are lazy-release/weak-consistency DSM replication: synchronization variable or lock, global ordering, vector timestamp, write notice, lock-manager failover, group-membership recovery | none of locks, timestamps, write notices, failover or nodes exist here | clear |
| 3 | US6178519B1 | Cluster-wide database system | MCI Worldcom / now Salesforce (per page) | Expired (filed 1998-12-10, anticipated expiry 2018-12-10) | independent 1, 9, 20; dependent 2, 15, 16, 19 | fault handler journals the original page before unprotecting; page diffs shipped to nodes (description, unclaimed) | claims need an object-oriented database in a shared virtual address space on cluster nodes | clear (expired) |
| 4 | US9164847B1 | Storage checkpointing for a group of independent applications | Open Invention Network / now IBM | Page legal-status field unclear; anticipated expiry 2028-12-15 listed earlier, treat as possibly in force | independent 1, 11, 16, in full | every independent claim requires interceptors for `fork()`/`exec()` and for file operations, full and incremental checkpoints, group-wide synchronised checkpointing, and a storage checkpoint, plus (last element) write-protecting user-space pages and detecting a write | we have no interceptors, checkpoints, application groups or storage; we report byte ranges, not page checkpoints | clear (missing most claim elements) |
| 5 | US7711937B1 | Trap-based mechanism for tracking accesses of logical components | Oracle America (ex-Sun) | Active per page, expires 2028-06-14 | independent 1, 9, in full | trap on a protected page, then "subsequent accesses ... not give rise to a trap" (claim 3: page unprotected) | both independent claims require extracting a method name or an object's class name from memory near the trapped address and logging it; we log no names, copy a twin and diff | clear |
| 6 | US5123101 | Multiple address space mapping technique for shared memory (fault routine on translator miss) | Xerox | Expired - Lifetime per Google Patents (priority 1986-11-12, filed 1989-08-23) | independent 1, 22, 45, 48, 53, 56, 59 (read via `patent-claims.py`; claims 1, 22, 45, 48, 53, 56, 59 shown, later dependents truncated by the tool) | none: claims are virtual-to-real address translation with caches, translators, address-space identifiers and map tables | we use no translation hardware; we change page protection through the OS | clear (expired, and claims read) |
| 7 | US8127174B1 | Method and apparatus for performing transparent in-memory checkpointing | Symantec Operating / now Cohesity | Active, about 2030 (PM, from the commy-sdk-rust record; expiry not independently checked) | independent 1, 9, 15, 21, in full; dependent 5, 6, 13, 19 | claim 21: "accessing checkpoint data; mapping the checkpoint data to a memory region; ... setting all portions of the memory region to read only; tracking faults that occur each time a write request is executed ...; identifying the portions having faults as ... modified portions; copying the modified portions; and replicating the copied modified portions ... to at least one backup server". Claims 1, 9, 15 use "initiating a backup" after an initial backup, then identify, copy, replicate to a backup server (read-only plus fault tracking is dependent claim 6, 13, 19) | the crate overlaps the middle of claim 21 (read-only, fault tracking, identify, copy) but every independent claim also needs accessing "checkpoint data" and replicating to "at least one backup server"; dirty-ranges does neither: it returns ranges to its caller and has no checkpoint, backup or transport | clear for this crate alone; consumer-level question goes to the consumer's record (see Needs review) |

Seen in search results only, claims NOT read, listed as leads for the next sweep: US7620766B1 and US6789156B1 (content-based page sharing); US6591355B2 and US6263414B1 (DSM minipages); US7739349 (partial memory replication); US2012/0036334 (shared memory segments, msync); US10037334 (host files for VMs, execution-in-place); US7100089B1 (snapshot differences); US7711914B2 (virtual watchpoints); US6049876 (unauthorised memory access detection); US20160350260A1; US6915387B1; US10114763 and the "Paging of external memory" family (US10275171, US11360679, US11379131).

No search surfaced a patent describing: a per-region switch between page-protection and snapshot-compare detection (Q4; closest was US7711937B1, which has two modes inside one trap handler, not a snapshot mode); a second writable alias used to avoid echoing remote writes (Q5); a lock-free pre-sized twin pool with the diff moved off the faulting path (Q6). These are absences in one search tool's results, not proof of absence in the patent literature.

## Needs review (to the PM)
No hit is a `needs-review` overlap for the crate on its own. Two items the PM should weigh:
- **US8127174B1 (hit 7) is the nearest live patent and the crate does the first half of its claim 21.** Read-only pages, fault tracking, identify and copy modified portions are what `Region` does. The claim then needs the memory region to map "checkpoint data" and the copy to be replicated "to at least one backup server". dirty-ranges alone has neither, so each independent claim lacks elements. A consumer that takes the flushed ranges and sends them to a server is the place the full claim could be argued; that question belongs on the consumer's own record (`commy-sdk-rust`, whose sweep flagged this patent; what commy adds is the replication leg). I did not read commy's design, so I do not say whether its use is "checkpoint data" or a "backup server". Not a claim that either is clear.
- US7711937B1 is active to 2028-06-14; both independent claims need a method name or object class name extracted and logged. No overlap, but a lawyer should confirm if the crate is ever used to track objects or methods.
Resolved by reading in full: US6574749B1 (hit 2) and US9164847B1 (hit 4), both clear on their independent claims.

## Limitations
- AI-assisted: a web-search tool restricted to patents.google.com plus page fetches whose text was summarised by a smaller model. Claim text was not checked against the patent PDFs. Hits 1 and 3 are partly excerpt-read; hits 2, 4, 5, 6 and 7 were read in full with `tools/patent-claims.py` (Google Patents claim text); the "seen in search results only" list was not claim-read. Hit 7 was added at the PM's pointer, not found by my own queries.
- Seven queries, one result page each, one source. No USPTO, EPO/Espacenet or WIPO query, no CPC-class or inventor/assignee search, no non-English or non-US-family search, no continuation/family tracing, no citation chasing.
- Ideas 7, 9, 10 and 11 were not searched for individually.
- Legal status and expiry are Google Patents' own assumption, not a legal conclusion.
- Software DSM from the 1990s (Munin/TreadMarks-style twin and diff) is the nearest prior art by description; most such patents are expired by age, which lowers the risk but was not checked patent by patent.
- Not legal advice and not a freedom-to-operate opinion; a lawyer's review is a separate purchase. Re-sweep when a new mechanism family is added, and yearly (next by 2027-10-25).
