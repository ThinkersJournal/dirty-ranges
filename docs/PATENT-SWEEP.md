# Patent sweep: dirty-ranges 0.1.x @ 8777411
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
| 2 | US20030167420A1 (parent US6574749B1) | Reliable distributed shared memory | individual inventor | Abandoned application (the page says this is an assumption); parent grant not read | independent 1, 4, 5-8, 12-14 (excerpts); dependent 2, 3, 9-11 reported as having no twin/diff language | description has write-protect, fault, twin, diff; claims are weak-consistency DSM messaging: ordering indication, write notice, vector timestamp, lock manager failover | no locks, vector timestamps, write notices or failover; no inter-node protocol | clear for this application; parent US6574749B1 claims NOT read (see Needs review) |
| 3 | US6178519B1 | Cluster-wide database system | MCI Worldcom / now Salesforce (per page) | Expired (filed 1998-12-10, anticipated expiry 2018-12-10) | independent 1, 9, 20; dependent 2, 15, 16, 19 | fault handler journals the original page before unprotecting; page diffs shipped to nodes (description, unclaimed) | claims need an object-oriented database in a shared virtual address space on cluster nodes | clear (expired) |
| 4 | US9164847B1 | Storage checkpointing for a group of independent applications | Open Invention Network / now IBM (per page) | Page says "Expired - Fee Related" but also lists anticipated expiry 2028-12-15; treat as unresolved | independent 1, 11, 16 (partial excerpts) | description: write-protect user-space pages, SIGSEGV interception, dirty pages into an incremental checkpoint | claims recite `fork()`/`exec()` interceptors, application groups, synchronised checkpoints and storage checkpoints; we do none of these and compute byte ranges, not page checkpoints | clear on the excerpts read; claim text only partly seen |
| 5 | US7711937B1 | Trap-based mechanism for tracking accesses of logical components | Oracle America (ex-Sun) | Active per page, expires 2028-06-14 | independent 1, 9 | trap on a protected page, then "subsequent accesses ... not give rise to a trap" (one-shot trap); log of accesses | claims extract information near the trapped address and log it for "an invocable method and an object" (logical components of a managed runtime); we track writes to raw pages, copy a twin and diff | clear (no logical-component tracking) |
| 6 | US5123101 | Multiple address space mapping technique for shared memory | not read | not read (pre-1990 filing, so expired by age; page not fetched) | none | write-protect fault marks a page dirty on first write | hardware translator design; ours is user-space signal handling | clear by age; claims not read |

Seen in search results only, claims NOT read, listed as leads for the next sweep: US7620766B1 and US6789156B1 (content-based page sharing); US6591355B2 and US6263414B1 (DSM minipages); US7739349 (partial memory replication); US2012/0036334 (shared memory segments, msync); US10037334 (host files for VMs, execution-in-place); US7100089B1 (snapshot differences); US7711914B2 (virtual watchpoints); US6049876 (unauthorised memory access detection); US20160350260A1; US6915387B1; US10114763 and the "Paging of external memory" family (US10275171, US11360679, US11379131).

No search surfaced a patent describing: a per-region switch between page-protection and snapshot-compare detection (Q4; closest was US7711937B1, which has two modes inside one trap handler, not a snapshot mode); a second writable alias used to avoid echoing remote writes (Q5); a lock-free pre-sized twin pool with the diff moved off the faulting path (Q6). These are absences in one search tool's results, not proof of absence in the patent literature.

## Needs review (to the PM)
No hit is a `needs-review` overlap on an independent claim I read. Three items the PM should weigh, none an overlap finding:
- US6574749B1 (the granted parent of hit 2) was not claim-read; its description matches hit 2. Priority 1999-10-29, so a 20-year term ends no later than 2019-10-29 unless extended: expected expired, not verified.
- US9164847B1's legal status is contradictory on its page and only partial claim text was returned. Re-reading the full claims from the PDF is the cheapest way to close it.
- US7711937B1 is active to 2028-06-14; its independent claims are tied to logical components, so I see no overlap, but a lawyer should confirm if the crate is ever used to track objects or methods.

## Limitations
- AI-assisted: a web-search tool restricted to patents.google.com plus page fetches whose text was summarised by a smaller model. Claim quotes are excerpts the tool returned, not verified against the patent PDFs. Hits 4 and 5 returned independent-claim excerpts only; hit 6 and the "seen in search results only" list were not claim-read.
- Seven queries, one result page each, one source. No USPTO, EPO/Espacenet or WIPO query, no CPC-class or inventor/assignee search, no non-English or non-US-family search, no continuation/family tracing, no citation chasing.
- Ideas 7, 9, 10 and 11 were not searched for individually.
- Legal status and expiry are Google Patents' own assumption, not a legal conclusion.
- Software DSM from the 1990s (Munin/TreadMarks-style twin and diff) is the nearest prior art by description; most such patents are expired by age, which lowers the risk but was not checked patent by patent.
- Not legal advice and not a freedom-to-operate opinion; a lawyer's review is a separate purchase. Re-sweep when a new mechanism family is added, and yearly (next by 2027-10-25).
