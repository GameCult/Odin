# Odin write pattern: cut map

**2026-09-30, Self.** Operator: "Let's fix Odin up, that's some furious bookkeeping right there. Look at Odin's
write pattern and see if append only or per-document is the way to go." Answer: neither is the first fix. Cuts 1-2
go to Hands now. Q3 (a 1 s interval) and Q4 (the Rust directory store as a CultLib follow-up) are Self defaults,
and the operator may overrule them. **Operator, 2026-09-30: "Q1: yes", so CultMesh refuses puts it could never
serve and Cut 3 goes ahead; "Q2: yeah, of course", so a read-only decode of a copy of live `topology.cc` is
approved. The copy is taken with `sudo cp` and decoded off the live path, and the copy is deleted afterwards.**

**Soul on Cuts 1-2, and two operator rulings (2026-09-30).**
- **Q3 restated.** The premise "puts carry no application acknowledgement" was false. The cultmesh-rs server
  ACKs a put only after `accept_raw_document` returns Ok, so after Cut 1 an ACK meant "in memory", and a SIGKILL
  lost acknowledged puts in 6 of 6 probe runs.
  - **Operator ruling: "Q3: B".** Peer puts are fsynced before they are acknowledged. Only Odin's own
    bookkeeping (heartbeats, correlations) waits for the 1 s interval. An ACK means durable.
- **An undecodable foreign write.** Today Odin stays up, serves from memory and never persists again. Self offered
  three options: degraded but loud, end Odin, or unchanged.
  - **Operator: "None of these options involve repair and recovery, which is what ultimately needs to happen."**
  - Recovery is mapped as its own pass. It covers an undecodable foreign write, a decodable one, and a failed
    load at activation.

**Q2 decode result (Eyes, 2026-09-30; full table in the CultLib session scratchpad `odin-topology-decode.md`).**
- 77 records; 3,222,550 B of payload in a 3,240,078 B file.
- The two "~1.55 MB records" are per-type totals. Each type is dominated by one Ghostlight operator surface:
  `eve:operator:ghostlight.campaign.27d444b3-...` (1,456,210 B) and its `operator-state` twin (1,456,341 B).
  Together they are 89.9% of the file.
- Publisher: provider `gamecult.ghostlight.dungeon`. 99.5% of each record is one Eve `code` node holding a
  pretty-printed JSON dump of the whole campaign state.
- These records are dead residue. They were stored 2026-09-23 and have not been rewritten since. Ghostlight `HEAD`
  (`eeb3a68`) no longer contains the emitter (last touched at `6bb6869`, 2026-09-02).
- **So Q1's refusal breaks no live publisher.** Cut 3's Odin half drops exactly these two at activation. Every other
  surface is 70 KB or less.
- Correction to section 3: the 1 MiB snapshot refusals in the journal are per-type responses, not per-record.
  Cut 3's per-document bound is still right: each of these records is over 1 MiB alone. A type whose many small
  records add up past 1 MiB would still be unservable as a whole-type snapshot. That is a follow-up for CultMesh
  (paged snapshot responses), not part of this campaign.
- Presence history: 41 records, 42 KB, 37 of them withdrawn. F4 is a slow leak, not a size problem.
- The current `streampixels-web` incarnation has a correlation but no presence record; its last presence was
  09-29 19:53 UTC. This matches Idunn's "not Odin-correlated" state.

Status: cut map, Imagination pass 1 (Opus), 2026-09-30. Nothing has landed. Anchors are Odin `main` at
`44951a1` (the deployed build, live on Yggdrasil since 2026-09-30 04:20:48 UTC) and CultLib `3bf1c0ce`
(Odin's `Cargo.lock` pin). This map owns the means; there is no separate target document, because the ends are
three sentences (section 2).

Operator's words, 2026-09-30: "Let's fix Odin up, that's some furious bookkeeping right there. Look at Odin's
write pattern and see if append only or per-document is the way to go."

**Short answer.** Neither is the first move. Odin's IO comes from treating its store file as working memory and
from carrying two unservable 1.5 MB Eve documents inside it, not from the file format. Three cuts remove about
99% of the bytes read and written, and bring the rewrites per second from 5 down to at most 1. None of them
changes the on-disk format. For Odin's data, per-document storage is the better fit than append-only, but after
those cuts it would save about 90 KB/s and no fsyncs. It does not earn a place on Odin's critical path
(section 5).

Open: Q1 to Q4 (section 7). Cut 1 needs no ruling. Cut 2 needs Q3. Cut 3 needs Q1 and Q2.

## 1. Body facts (measured)

### 1.1 Live Odin, `/proc/375952/io` and `ss`, read-only, 2026-09-30 ~10:14 UTC

| Quantity | Process lifetime (21,252 s) | 60 s sample (10:15-10:16 UTC) |
|---|---|---|
| logical read (`rchar`) | 1.80 TB, 84.8 MB/s | 6.37 GB, **106 MB/s** |
| written (`wchar`) | 276.5 GB, 13.0 MB/s | 972 MB, **16.2 MB/s** |
| written to disk (`write_bytes`) | 276.8 GB (all of it) | 16.2 MB/s |
| read from disk (`read_bytes`) | **0** | 0 |
| write syscalls (`syscw`) | 85,420, 4.0/s | 300, **5.0/s** |
| bytes per write syscall | 3.24 MB | 3.24 MB |
| CPU | 1,529 s, 7.2% of one core | |
| socket drops (`ss -m`, `d`) | **136,291** | 0 in one 30 s sample (drops come in bursts) |

- The store is `/var/lib/gamecult/odin/topology.cc`: **3,240,078 bytes** (`ls -l`; the contents were not read).
  Every write syscall is one whole-store rewrite: 5.0 per second, each followed by a file `fsync` and a
  directory `fsync`.
- `read_bytes = 0`. The "376 GB read" in the audit is page-cache reads. It costs CPU and memory bandwidth
  (repeated MessagePack decodes of 3.24 MB), not disk. **The disk cost is the writes: 16 MB/s and about 10
  fsyncs per second.**
- The audit's figures (376 GB read, 63 GB written in 4 h, `MORNING-2026-09-30.md` line 214) came from the
  previous process (`c34d167`). This process runs at about 3.7 times those rates, with the same 6.5:1 read/write
  shape. The cause is the same.
- The idle Odin socket drops datagrams in bursts. Its 213 KB "backlog" in the audit is the receive buffer size
  (`rb212992`); the drop counter (136,291) is the real signal.
- Idunn currently reads 211 KB in 30 s. Its `--odin-correlation-store` read of the same file is not a
  significant load.

### 1.2 Cost of one operation on Yggdrasil's disk (scratch probe)

Probe: `scratchpad/odin-write-probe` (Imagination session), commit `71b376a6`. It was run through the stopgap
in slot 5, on `/dev/vda4` (the host disk) with `TMPDIR` on that disk, at CultLib `3bf1c0ce`. Bytes are
`/proc/self/io` deltas and times are wall clock under the live IO pressure (`/proc/pressure/io` full avg60
about 10-12%). The store shapes use the payload sizes the live catalog reports.
TODAY adds two 1.55 MB records; SUBTRACTED omits them.

| Operation | TODAY (3.24 MB store) | SUBTRACTED (131 KB) |
|---|---|---|
| full read and decode | 3.24 MB, 0.78 ms | 131 KB, 0.14 ms |
| one presence heartbeat as coded today (read, reserve watermark CAS, correlation CAS) | 16.2 MB read, **6.49 MB written, 54.8 ms** | 663 KB, 265 KB, 14.4 ms |
| the same heartbeat folded into one CAS | 6.49 MB read, 3.24 MB written, 36.5 ms | 268 KB, **134 KB, 7.1 ms** |
| redb (cultcache-rs `RedbMessagePackBackingStore`), one small-row CAS | 33 KB, 42.6 KB, 5.4 ms | 33 KB, 42.6 KB, 8.7 ms |

Odin's serving loop is single-threaded (`main.rs:739-747`), so every millisecond in the table is a
millisecond in which no datagram is read. Five writes per second at 36-55 ms each block the loop for 180-275
ms of every second on a quiet disk, and for longer under pressure. That is the socket drops and the route
challenge timeouts.

### 1.3 What gets written, and why (source, `44951a1`)

Every mutation reads the whole file, then CAS-rewrites the whole file, through CultLib's
`SingleFileMessagePackBackingStore` (`cultcache-rs/src/lib.rs` at `3bf1c0ce`: `compare_exchange` reads all
entries under an exclusive lock, rewrites all of them, then `fsync`s the file and its directory).

| Trigger | Rate (measured from catalog `stored_at` churn over 88 s, and from the publisher's source) | Store ops per event |
|---|---|---|
| Muninn republishes `muninn.obs_stream_catalog`, `muninn.move_hue_program` and `gamecult.media_stream_advertisement` every 2 s (Muninn `main.rs:101`, `:2241`). The payload changes each time: `updated_at` (`:3263`) | about 1.5/s | `persist_generic_document` `main.rs:1266-1309`: 1 read, then 1 CAS (read and write) |
| Provider presence heartbeats: raven-muninn about every 2.7 s, Odin 5 s, streampixels-service about 10 s, ghostlight about 12 s | about 0.8/s | `admit_presence`, then `reconcile` `lib.rs:771-977`: `store.read` (1 read), `reserve_publisher_sequence` `lib.rs:613-647` (read and **write**), `compare_exchange` `lib.rs:525-584` (read and **write**) |
| Odin's own heartbeat (`HEARTBEAT_INTERVAL` 5 s, `main.rs:71`) | 0.2/s | the same path, through `publish_self_presence` `main.rs:422-427` |
| Provider ads and other documents restamped (voidbot.swarm about every 30 s, and others) | about 0.1/s | `persist_generic_document` |
| Projection refresh every 250 ms (`main.rs:72`, `:770-774`) | 4/s | `incarnation_keys(store)` `main.rs:486-489` (1 read), then per incarnation `refresh` (projection read, then `store.read`). A write only when facts change |
| Catalog queries (Hermodr, Idunn, probes) | on request | `stored_snapshot` `main.rs:509-543` (1 full read, then a projection read per presence) |

- A correlation embeds the observed presence sequence and hash (`lib.rs:887-895`), so
  `same_correlation_facts` (`lib.rs:1273-1282`) never dedupes a heartbeat. **Every heartbeat costs two
  whole-store writes**: first the watermark, then the correlation.
- About 2.1 of the 5.0 writes per second are accounted for by Muninn and the other catalog documents, and about
  2.0 by heartbeats. The remaining 0.9 per second are refresh-driven correlation changes (dependency freshness
  windows, O2 in the Idunn audit) plus writes that were not attributed. The two 1.5 MB records are not among the
  catalog churn seen through the probe; their publisher and rate are unknown (Q2).
- Reads: about 30 whole-store decodes per second. Most are the 250 ms refresh (1 + N incarnations per pass),
  and the rest are admission reads.

### 1.4 What is in the store: truth, cache or projection?

| Kind | Who owns the truth | Odin's role | Who reads Odin's copy | Must it be durable? |
|---|---|---|---|---|
| Provider catalog documents (ads, boundaries, profiles, media ads, schema catalog, surface_state) | the provider | relay cache | the Verse, via the catalog | yes, but not per write. Some publishers put once and never again (for example `heimdall.command_boundary`, whose `stored_at` is from 09-23), so losing the catalog on a restart would lose them. The CultMesh put has no application acknowledgement (`cultmesh-rs rudp_document_server.rs:464-482`), so a put promises the publisher no durability |
| `gamecult.eve.surface` and `surface_state`, 1.55 and 1.56 MB | unknown provider (Q2) | a cache **that can never be served**: a single-record snapshot is over the 1 MiB limit (Odin journal: "snapshot response is 1547739 bytes; limit is 1048576", and 1563575) | nobody can | no. They are 93% of the store's bytes and ride along in every rewrite |
| Runtime presences | the provider (signed) | admitted evidence with anti-replay (O5, O6) | the Verse; Odin's own sequence continuation (`main.rs:1184-1211`) | the latest per incarnation. The 30 s observation window (O6) bounds any replay after a lost write |
| Correlations and the publisher watermark | Odin | a **projection** of presence plus Idunn's projection | Idunn, lockless, from this same file (`--odin-correlation-store /var/lib/gamecult/odin/topology.cc`; Idunn `drivers.rs:5005-5030`) | the watermark must never let a *published* sequence repeat. "Published" means written to this file, because since batch 2 nothing else serves correlations. Both die at Idunn A3 and Odin O-R (`Idunn/docs/route-continuity-cut.md` section 4) |
| Presence history of withdrawn incarnations | nobody | retained on purpose (`withdrawn_expected_removes_current_correlation_but_preserves_presence_history`, `lib.rs:2814`) | only Odin's own sequence continuation reads it, and only Odin's own key | grows without bound with every deploy (follow-up F4) |

## 2. Ends

1. Odin reads its store once, at activation, and never again while it holds the write lease. Its working
   state is in memory.
2. Odin writes its store at most once per flush interval, only when the content changed, as one atomic write
   under the lock. A foreign write to the file is detected and ends Odin.
3. The store holds nothing Odin cannot serve.

Invariants that must survive:
- One writer: the process-write lease (`main.rs:457-474`) is the only authority to write. Nothing writes after
  the lease is lost.
- Idunn's crossing: until A3, Idunn reads correlations from `topology.cc` with
  `pull_all_read_only_snapshot`. The path, the single-file `.cc` format and the atomic replace do not change.
- Correlation sequences never repeat, as published.
- One bad record never stops Odin. This now holds at load: records are skipped and logged, never fatal
  (`dc46be2`).
- Presence anti-replay within an activation (O5).

## 3. Identity, lifecycle, authority

| Persistent kind | Named by | Over time | Decided by |
|---|---|---|---|
| catalog document | `(type, key)`, type from the schema id (`main.rs:1311-1326`) | replaced by each provider put; never deleted | the provider decides the content; Odin decides admission (allow-list `main.rs:1237-1264`, plus the size rule in Cut 3) |
| presence | `presence_store_key(incarnation, signer)` `lib.rs:1528` | replaced each heartbeat; kept after withdrawal (F4) | provider bytes, Odin's admission |
| correlation | incarnation key | replaced when facts change; deleted on withdrawal (`lib.rs:586-611`) | Odin; dies at O-R |
| watermark | target | only ever increases | Odin; dies at O-R |
| in-memory working set (new) | the same keys | loaded at activation, mutated per event, never reloaded while serving | Odin under the lease |
| flush (new) | none: one atomic snapshot | at most one per interval when dirty; one final flush on stop while the lease is held | Odin's serving loop |

## 4. Cuts

Subtraction cuts come before behaviour cuts. Each cut is its own branch and Soul pass. All verification runs on
Yggdrasil through the stopgap (`cargo test --locked -p odin-daemon` from the Odin root).

### Cut 1. The file stops being working memory (Odin, subtraction)

- **Repo/branch:** Odin `hands/odin-memory-store` from `44951a1`. Depends on nothing.
- **First:** a 60 s `/proc/<pid>/io` sample of live Odin, plus the `ss -m` drop counter (section 6 script).
  The numbers in 1.1 serve as the baseline if they are taken within a day.
- **Deletes first:**
  - `lib.rs:432-648` `CultCacheOdinTopologyStore` (about 215 lines): every `pull_all_read_only_snapshot` and
    CAS against the store file, and its four `CAS_ATTEMPTS` retry loops (`:453`, `:588`, `:614`, and the one
    in `reconcile` at `:799`). With one writer under the lease, an in-memory compare-exchange cannot lose, so
    the retry loops protect nothing.
  - `main.rs:1266-1309` `persist_generic_document`'s file CAS loop.
  - `main.rs:486-489`: the store half of `incarnation_keys` (the projection half stays).
  - `main.rs:513-518`: `stored_snapshot`'s file read.
  - `main.rs:1184-1211` `prior_self_publisher_sequence`'s file read (it becomes a query of the loaded set).
- **Keeps:** the `OdinTopologyStore` trait (`lib.rs:410-430`) as the mock point. `OdinTopologyAuthority` does
  not change.
- **Adds:** one in-memory store owned by `RuntimeState`, keyed `(type, key)`, implementing `OdinTopologyStore`.
  It is loaded once in `try_activate` (`main.rs:229-267`) after the lease is verified, with the locked
  `pull_all`, and replaces the load's current callers. Legacy retirement (`main.rs:736`,
  `retire_legacy_correlations` `lib.rs:452-478`) and the self sequence (`main.rs:201-208`) become operations on
  the loaded set. Undecodable records are skipped and logged at load, as today.
  - The projection is read **once per refresh pass**, not once per incarnation: `refresh_all_correlations`
    `main.rs:476-501` builds one projection snapshot and hands it to each `refresh`. Name the type;
    `IdunnProjectionSource` is already the port.
  - Hands chooses whether the trait's `compare_exchange` stays or collapses into `read` plus `commit`. The rule
    is that no path retries a CAS that cannot lose.
- **Not in this cut:** the write policy. At the end of Cut 1 every mutation still writes the whole file
  immediately, now from memory: one read per write inside CultLib's compare, and no read otherwise.
- **Authority map:**
  - Owner: `RuntimeState`'s in-memory set, under the process-write lease.
  - Inputs: one locked read at activation; provider puts; Idunn's projection (read per pass).
  - Outputs: the catalog, correlations, and the file (Cut 2 owns when).
  - Derived state: the file is a durability checkpoint and Idunn's crossing. It is **no longer an owner** of any
    decision while Odin serves.
  - Forbidden writers: no second reader-as-authority. No code path in the serving loop may call
    `pull_all_read_only_snapshot` or `pull_all` on `options.store`.
  - Shared paths: provider presence, self presence, generic puts, refresh and catalog all mutate or read the one
    set.
  - Deletion line: the `lib.rs` file store and the `main.rs` file reads above are deleted before the memory store
    is wired in.
- **Verification:**
  - tests: existing behaviour tests keep passing. Tests that corrupt the store file **while Odin serves**
    (`one_undecodable_incarnation_leaves_the_others_and_the_catalog_readable` `main.rs:2094`,
    `records_that_do_not_decode_never_stop_the_catalog_or_startup` `:2341`) describe a second writer that the
    lease forbids. Re-target them to "the bad record is in the file at activation". Do not delete them.
  - new test: after activation, rewriting `topology.cc` behind Odin's back changes nothing Odin serves until
    restart (pins the rule that the file is not working memory; a mutant that re-reads fails it).
  - new test: N refresh passes perform 0 reads of the store file. Observe it where it is decided, with a
    counting seam on the load path, or with an `strace`-free `/proc/self/io` delta in the scenario.
  - negative: `rg -n "pull_all_read_only_snapshot|pull_all\(" crates/odin-daemon/src/main.rs` matches only the
    runtime-bundle, anchor, lease and projection readers (`:1018`, `:1061`, `:1162`, projection), never
    `options.store`.
  - `cargo mutants --in-diff` on the cut's diff.
- **Estimate:** about -260 / +120 lines.

### Cut 2. One coalesced write per interval, watermark folded in (Odin, behaviour)

- **Repo/branch:** Odin `hands/odin-flush` from Cut 1. Needs Q3.
- **Deletes first:** the file half of `reserve_publisher_sequence` (`lib.rs:613-647`). Reserving becomes an
  in-memory increment, and the mark lands in the same atomic write as the correlation that uses it. The ordering
  comment at `lib.rs:922-926` and the trait doc at `:422-429` are rewritten: "published" means flushed.
- **Adds:**
  - `FLUSH_INTERVAL` (1 s, Q3) in `main.rs:71-76`.
  - A dirty flag.
  - A `flush_if_due` step in `serving_pass` (`main.rs:764-781`). It checks `require_current_write_lease`
    immediately before writing, then writes the whole set with
    `compare_exchange_snapshot(last_flushed, current)`. A `false` result means the file changed under Odin, which
    is a second writer: it is returned as `WriteLeaseLost` (or a sibling type that `survive` treats the same
    way).
  - One final flush on `SIGTERM`/`SIGINT` (`main.rs:740-743`), only if the lease is still current.
- **Authority map:**
  - Owner: the serving loop's flush step decides when the file changes.
  - Inputs: the dirty flag, the flush interval, and the current lease.
  - Outputs: one atomic `.cc` snapshot.
  - Derived: the watermark in the file is at least every sequence in the file. An in-memory sequence that was
    never flushed was never published.
  - Forbidden writers: `admit_presence`, `persist_generic_document`, `withdraw_correlation` and `reconcile`
    never write the file.
  - Shared paths: every mutation goes through the one flush.
  - Deletion line: the per-mutation writes left by Cut 1 are deleted before the flush is added.
- **Verification:**
  - test: 50 heartbeats and puts inside one interval produce exactly 1 file write (pins coalescing).
  - test: no mutation produces no write (pins "only when dirty").
  - test: a lease withdrawn between mutation and flush produces no write and ends Odin (pins the one-writer
    rule). Observe the file bytes, not a log line.
  - test: a crash (drop the state without the stop flush) followed by a reload never republishes a sequence
    that is in the file. In other words, the next correlation's sequence is greater than the flushed mark (pins
    never-repeat under the new meaning of "published").
  - test: a foreign write between flushes ends Odin without overwriting it.
  - Soul must falsify one question: does Idunn fence (stop) an incumbent Odin **before** it writes the
    candidate's lease? If not, an incumbent can pass the lease check and flush after the candidate has loaded.
    The race exists today as well; this cut must not widen it.
  - `cargo mutants --in-diff`; at least one mutant must be a function of the interval, such as a scaled or
    clamped interval.
- **Expected live effect:** at most 1 write/s (from 5.0), 2 fsyncs/s (from about 10), about 3.2 MB/s written
  while the surfaces remain (from 16.2), and about 3.2 MB/s read (the compare inside the flush).
- **Estimate:** about -40 / +70 lines.

### Cut 3. Nothing unservable is admitted (CultLib, then Odin)

Needs Q1 and Q2. **Recommended form (Q1 A):**

- **Owner:** CultLib `cultmesh-rs`. The document server admits a put of up to 4 MiB per session
  (`rudp_document_server.rs:154-156` at `3bf1c0ce`) but can serve at most 1 MiB, so it accepts documents it can
  never return. The fix belongs to the server, not Odin. Only `cultmesh-rs` has this server, so there is no
  sibling-runtime parity work.
- **CultLib cut:** `hands/cultmesh-put-serve-bound` from CultLib `main`. `deliver_application_message`
  `DocumentPutRaw` (`:464-482`) refuses, as an application rejection naming both sizes, any document whose
  single-document `SnapshotResponseRaw` would exceed `max_snapshot_response_bytes`. It uses the same encoder
  and limit as the snapshot path (`:543`), so the two can never disagree. Test: a put one byte over the bound is
  refused and one at the bound is served.
- **Odin cut:** a pin bump, plus one step at activation: stored records that the served-size rule would refuse
  are dropped from the loaded set and logged by type, key and size. The next flush removes them from the file.
  Test: a store holding an oversize record activates, serves everything else, and the oversize record is gone
  after the first flush.
- **Expected live effect:** store about 130 KB, at most 134 KB written per second, at most 7 ms of loop time
  per second.
- **Estimate:** CultLib about +25 plus tests; Odin about +20.

### Cut 4 (not recommended now). Per-document storage

See section 5. Record it as a CultLib follow-up (F1), not an Odin cut.

### Subtraction ledger (estimate)

| Cut | Removed | Added | Formats and targets |
|---|---|---|---|
| 1 | about 260 | about 120 | none |
| 2 | about 40 | about 70 | none |
| 3 | 0 | about 45 (CultLib 25, Odin 20) | none |

## 5. Append-only against per-document, answered

Odin's data is *latest value per key*. About 60 small records (about 1 KB each) are overwritten at 0.1-0.5 Hz
each. Two large records change rarely or never. After Cuts 1-3 the remaining cost is at most one 130 KB
atomic write and two fsyncs per second.

- **Per-document** fits this shape: an overwrite touches only its own record. CultLib has two:
  - C#'s `DirectoryMessagePackBackingStore` (`cultcache.store.v4.directory-content-addressed-pages`: a small
    hot manifest plus cold content-addressed pages). It is the reference format.
  - cultcache-rs `RedbMessagePackBackingStore`. The probe measured 42.6 KB written and 5-9 ms per small-row
    commit.

  After the cuts, this saves about 90 KB/s. The fsync count, which is the stall that matters, stays the same:
  1 per commit against 1 per flush. The costs:
  - redb is not the `.cc` format, and it has no lockless read-only crossing. Idunn's read of the correlation
    file would break until A3.
  - The v4 directory store does not exist in Rust. That is a parity gap in CultLib (F1), not a reason for
    Odin to grow one.
- **Append-only (log plus compaction)** fits journals: ordered history that consumers replay, such as
  CultNetDatabase shard logs. For Odin, the log grows at the overwrite rate (about 5 records/s, several hundred
  MB/day), so it needs compaction. Idunn's crossing would have to replay it, and it needs a new on-disk format
  with byte parity across four runtimes. It buys Odin nothing that a keyed store does not.
  - The capability is already owed elsewhere. The publication campaign recorded (MORNING log, 06:23 and
    earlier) that CultLib's file log store "truncates then rewrites with no atomic rename, so an append-only
    atomic store must exist before any live daemon turns durable shard logs on."
  - That is CultLib's append-only consumer. Odin should not be its first one.

**Recommendation:** Cuts 1-3 (subtraction), then stop. Take up per-document storage for Odin only if the flush
interval must shrink far below 1 s, or if large documents must live in Odin's store (Q1 B). In either case the
store comes from CultLib, as the Rust port of the v4 directory store, and never as an Odin-local helper.

## 6. Verification after each deploy (disk IO)

Run on Yggdrasil (read-only; it reads no store):

```sh
pid=$(pgrep -x odin-daemon | head -1)
snap(){ sudo cat /proc/$pid/io | awk '/^(rchar|wchar|syscw):/{printf "%s ", $2}'; \
        sudo ss -lunpm | grep -A1 "pid=$pid," | grep -o 'd[0-9]*)' | tr -d 'd)'; }
a=$(snap); sleep 60; b=$(snap)
echo "$a" "$b" | awk '{printf "read %.2f MB/s  write %.2f MB/s  writes %.2f/s  drops %d/min\n", ($5-$1)/60e6, ($6-$2)/60e6, ($7-$3)/60, $8-$4}'
cat /proc/pressure/io
sudo journalctl -u idunn-yggdrasil.service --since -1h --no-pager -o cat | grep -c "admitted odin route continuity: timed out"
```

| After | read MB/s | write MB/s | writes/s | drops, route timeouts |
|---|---|---|---|---|
| baseline (1.1) | 106 | 16.2 | 5.0 | 136,291 lifetime; 42 timeouts in 12 h, 0 in the last hour at 10:2x UTC |
| Cut 1 | at most 17 | about 16 (unchanged) | about 5 | should fall (loop no longer decodes about 30 times a second) |
| Cut 2 | at most 3.5 | at most 3.5 | at most 1.05 | at most 1 burst per hour; 0 timeouts over an hour |
| Cut 3 | at most 0.3 | at most 0.2 | at most 1.05 | 0 drops per minute on average |

Also run the post-deploy catalog check (`scratchpad/odin-postdeploy-check.sh`, Soul 2026-09-30). It proves no
provider type stopped being served. After Cut 3 the two surface types are expected to answer with zero
records; update the check's type list in the same pass.

## 7. Operator questions

- **Q1. The two 1.5 MB Eve documents.**
  - A: CultMesh refuses any put it could never serve, and Odin drops the stored ones at activation. Anyone who
    needs big surfaces through Odin gets the content plane (BP-1 in Rust) as its own campaign.
  - B: keep storing them, and move Odin to a per-document store (port the v4 directory store to Rust first) so
    they stay cold.
  - C: leave them, and accept about 3.2 MB per flush.

  **Recommended: A.** Nobody can read them today. They are 93% of the store and of every write. The server's
  put and serve limits disagree, and A fixes that at its owner.
- **Q2. May Imagination or Soul decode a copy of `topology.cc`?** A read-only decode of a *copy*, as approved
  for Idunn's `control.cc`, to name the two large records (key, `stored_at`, publisher) and count the presence
  history (F4). The publisher's name decides who is told that their puts will be refused under Q1 A.
  **Recommended: yes**, before Cut 3 lands.
- **Q3. Flush interval and durability.**
  - A: 1 s. An Odin crash loses at most 1 s of catalog and presence changes. Providers republish, puts carry
    no application acknowledgement, and no published correlation sequence can repeat.
  - B: 250 ms. This means up to 4 flushes and 8 fsyncs per second.
  - C: flush correlations immediately and everything else at 1 s.

  **Recommended: A.** It is one rule, and Idunn's correlation freshness window is 30 s.
- **Q4. CultLib parity: port `DirectoryMessagePackBackingStore` (v4) to cultcache-rs?** It is not needed by
  Odin after Cuts 1-3. A reasonable consumer of the Rust runtime would expect the reference's store.
  **Recommended: record it as CultLib follow-up F1 with no deadline.** It is independent of this map.

## 8. Related findings

**Shared cause (folded in above):**
- **Route-challenge timeouts and the socket backlog.** The single-threaded loop blocks for 36-55 ms per
  whole-store write, five times a second, and longer under IO pressure. Cuts 1-3 remove this. The 42 in 12 h
  now (against about 1,100 in the audit) is consistent with the verify slots being cut from 8 to 5. Cut 2's
  check measures it directly.
- **Eve surfaces over 1 MiB.** Cut 3 handles the storage side. *Serving* large surfaces is the content-plane
  follow-up (F2).

**Separate follow-ups (not caused by the write pattern):**
- **F1** CultLib: cultcache-rs lacks the C# v4 directory store (Q4).
- **F2** Eve surfaces larger than 1 MiB need the CultMesh content plane (BP-1 exists in Rust). This is a
  campaign for whoever publishes them (Q2).
- **F3** `surface:gamecult.network.status` has had no publisher since the Node coordinator retired (09-11).
  Gjallar (`Program.cs:562-592`), Hermodr, Mimir and VoidBot read it. The owner question (Odin's own Eve
  surface or someone else's) is its own map.
- **F4** Presence history of withdrawn incarnations is never pruned (`lib.rs:2814` pins its retention). Only
  Odin's own sequence continuation reads it, and only for Odin's own key. The store grows with every deploy.
  Needs Q2's count, then a retention rule.
- **F5** Muninn republishes three catalog documents every 2 s with a fresh `updated_at` (Muninn `main.rs:101`,
  `:3263`). Liveness belongs in its presence, not in catalog documents. After Cut 2 this costs Odin nothing
  durable, but it is still traffic.
- **F6** Yggdrasil keeps 389 `idunn-odin-*` unit residues, nearly all `failed` (`systemctl list-units
  'idunn-odin*' --all`). They belong to Idunn and systemd, not to this map.
- **F7** `accept_raw_document` (`main.rs:269-280`) checks `activated()` but not the *current* lease before
  mutating. Cut 2 moves the lease check to the only write, which closes this for the file. Soul should confirm
  that nothing else acts on a lost lease.
