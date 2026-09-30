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

**Soul on write batch 2 (2026-09-30).** Q3 B changes the expected effect. Cut 2's row in section 6 ("at most
3.5 MB/s, at most 1.05 writes/s") assumed Q3 A and is **superseded**. Under Q3 B, each peer put (presence
heartbeats included) writes the store once.
- With the live-shaped 3 MB store that is about 2.5-3 writes/s, about 7.5-9 MB/s, and 70-170 ms of the loop
  blocked each second.
- Measured: route-challenge p99 461 ms and 11 kernel drops.
- After Cut 3's Odin half the store is about 80 KB and a put write is p50 about 4 ms.
- **Cut 2 therefore deploys only together with Cut 3's Odin half**, which is now being built on `hands/odin-write`.

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

**Soul on write batch 3 + Cut 3 Odin half (`f4a1f89..726e0f3`, 2026-09-30): not fit to deploy.**
- **S-1 (high; operator question Q5, below).** Every write, the fsync pair included, runs on the serving loop
  (`main.rs:358` → `flush` → `compare_exchange_snapshot`; the interval flush at `:883` is on the same thread).
  - A disk stall passes into route-challenge latency one for one. A probe that held the store lock measured: a 0 ms
    stall gave 86 ms, 300 ms gave 288 ms, 1,500 ms gave 1,487 ms.
  - Interleaved runs (ABBA) under IO pressure of 5-38% gave a challenge p99 no better with Cut 3 than without it.
  - Cut 3 halves the bytes written, not the stalls. **The premise that the store's size caused the stalls was
    wrong; they come from the disk.**
- **S-2 (high).** Q1's put refusal has not landed. Odin's pin `3bf1c0ce` has no put bound, so a 1.2 MB put is
  ACKed, fsynced, and dropped at the next activation. It waits on the CultLib put-serve merge, then a pin bump with
  a `served_record` override.
- **S-3 (medium).** `attempted` grows about 98 KB per failed write, roughly 0.9 GB/h on a full disk, which ends in
  OOM.
  - Fix in the owner: cultcache-rs returns a typed `NotReplaced`/`ReplacedNotDurable` write outcome (added to CultLib
    R1 batch 3), and Odin keeps at most one candidate.
- **S-4 (medium).** Two admitted presences for one target, which a provider in a deploy window produces, refuse the
  whole catalog (`main.rs:~628`).
- **S-5 (medium).** A RUDP transport ACK is recorded at receipt, so with pipelined puts a later put is ACKed while
  an earlier one is still held. With the first datagram lost, the second is never delivered. That is data loss
  under the QUIC-pivot rule; it is tracked in the CultLib ack map. `dcd36fc`'s doc line overstates.
- **S-6, S-8.** Test gaps (the limit taken through `main`; `remove_not_dirty`).
- **S-7.** The drop log echoes the key. Self ruled keys and schema ids are identities and may be logged, as in
  CultLib R1.
- **Held:** F1's logic, F4's projection fallback, F5, F3's refusal, and Cut 3's activation bound, drop and removal.
  Sizing uses `peer_document`, so it matches what is served.
- **Batch 4 in Hands:** S-4, S-6, S-8, the S-5 doc line, the attempted-list fixture gap, and committing Soul's
  probes. S-1 waits on Q5; S-2 and S-3 wait on CultLib.

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
- **Q5 (2026-09-30, from Soul S-1). Where does the durable write run?** Today the serving loop runs the fsync, so
  any disk stall stalls route challenges, catalog reads and every other put.
  - A: a writer thread with group commit.
    - The loop hands each accepted put to one writer thread. The writer writes and fsyncs everything queued since
      its last write in one go, then the loop sends those puts' replies.
    - Challenges and reads are never behind the disk. A stall delays only put replies, and Q3 B still holds (a
      reply means durable).
    - It needs cultmesh-rs to let a put handler reply later (a CultLib API change) plus one Odin thread.
  - B: answer route challenges on their own path. Everything else still stalls, and the challenge would report
    Odin healthy while its catalog is stuck.
  - C: reopen Q3 and go back to the 1 s interval. That brings back lost acknowledged puts on a crash.

  **Recommended: A.** **RULED A by the operator, 2026-09-30: "agree on Odin writer thread".** Mapping is next
  (Imagination): the cultmesh-rs deferred put reply, then the Odin writer thread with group commit. Mapped below in
  "Q5 cut: the writer thread" (cuts C5, W0, W1; forks Q5-A and Q5-B).

## Q5 cut: the writer thread

**Imagination pass (Opus), 2026-09-30.** Maps the operator's ruling on Q5, "agree on Odin writer thread" (option A,
section 7). Anchors:
- Odin `hands/odin-write` at `434756e` (`crates/odin-daemon/src/main.rs`, `lib.rs`), CultLib pin `3bf1c0ce`.
- CultLib `hands/cultmesh-put-serve-bound` tip `e4e2a0b9` (`packages/cultmesh-rs/src/rudp_document_server.rs`).
  Its merge base is `42ba0f08`, which already holds ack Cuts 1, 1b and 1c.
- CultLib `hands/ack-cutD` tip `e9d5ae2c` (lands on main first), and R1 batch 3 `hands/cultcache-read-failures` tip
  `93ac5945` (`CultCacheStoreWriteFailed`).

The mapping raises two forks, **Q5-A** and **Q5-B** (at the end of this section). Cuts C5 and W1 wait on Q5-A.

**RULED, operator 2026-09-30: "Q5-A: A, but make a note of this for when we move on QUIC, Q5-B: A".**
- **Q5-A = A.** The server withholds the session's transport ACK until the put is answered. The wire is unchanged.
  The note for QUIC (a put-result message) is in the QUIC campaign handoff.
- **Q5-B = A.** Puts are held during a writer stall, bounded by the existing session and byte limits.

### Q5.1 Body facts (probed and read)

**Probe O1, Odin `434756e` + scratch test, Yggdrasil slot 3 (`imag-q5-odin@31953c93`).** A test process holds the
store's sibling lock for 1,500 ms, the way Soul modelled a disk stall. Full serving passes run meanwhile. A route
challenge is sent at 200 ms, then a catalog read.

| Case | Route challenge | Catalog read |
|---|---|---|
| nothing waiting to be written | served in **39 ms** | served in 29 ms |
| only Odin's own heartbeat waiting, and the interval write due | served in **1,342 ms** | 33 ms (sent after the release) |

- No read path takes the store lock. At `434756e` the lock is taken only by `MemoryOdinTopologyStore::load`
  (`lib.rs:509`, at activation) and by `flush` (`lib.rs:612` → `compare_exchange_snapshot`).
- The stall reaches the loop through **both** writes on it: a peer put (`main.rs:363`, Soul S-1) and the interval
  write of Odin's own bookkeeping (`main.rs:933-941`). The interval write stalls the loop with no peer put at all.
  So the bookkeeping write must move to the writer too, not only the put write.

**The loop still reads two other files.** They are not the store and take no lock, but they are disk reads on the
loop:
- The lease file, on every route challenge and catalog read: `raw_snapshot` `main.rs:389-409` →
  `require_current_write_lease` `:546-567` → `read_process_write_lease` `:1323` (`pull_all_read_only_snapshot`).
- Idunn's projection, on every catalog read (`:608`), refresh (`:572`) and heartbeat (`:524-544`).
- Both files are small and rewritten rarely by Idunn, so they are normally served from the page cache. Probe O1
  cannot show whether a real disk stall evicts them. Only the Yggdrasil rerun (Q5.6) can. If it does, the answer
  is to hold the lease record in memory and re-check it where it is decided. That is its own follow-up, not this
  cut.

**What a put's reply is on the wire today.** CultNet has no put-acknowledgement message: the list in
`cultnet-rs/src/contracts.rs` has `document_put_raw` and `error.v0`, and no put result.
- A Rust publisher (`publish_cultnet_message_to_rudp_catalog`, `cultmesh-rs/src/lib.rs:871-924` at `e4e2a0b9`)
  succeeds when its reliable packet is acknowledged by the RUDP transport, and fails on a `cultnet.error.v0`.
  - Its default `flush_timeout` is 300 ms, with a resend every 50 ms (`:144-157`).
- The document server sends that acknowledgement at the end of `poll_once`, after the sink returns
  (`rudp_document_server.rs:468-476`). A refusal ends the session with an Error frame that acknowledges nothing
  (`end_rejected_session` `:760-793`, put-serve branch).
- **So today the put's reply is the transport acknowledgement.** That is how Q3 B's "an ACK means durable" holds:
  the sink writes before it returns.

**Every packet a session sends carries acknowledgement fields** (`ack`, `ack_mask`, from `ack_state`, cultnet-rs
`rudp.rs` at main).
- The fields report every *received* reliable sequence, not only delivered ones.
- `create_packet` stamps them on each new packet: data, Pong, the end-of-poll Ack.
- A reliable packet is stored with the fields it was created with, and resent unchanged (`track_reliable`,
  `due_resends`).

Probes P1-P4 (cultnet-rs sessions at `e4e2a0b9`, scratch test `imag_q5_probe.rs`, Yggdrasil, `imag-q5-cultlib@8dcfe36a`):

| # | Case | Result |
|---|---|---|
| P1 | two pipelined ordered puts, the first lost | the second is delivered to nobody, yet `create_ack` acknowledges it. **S-5 is still open after ack Cuts 1-1c.** `ack_state` at main is the same code, so the ack map's to-do resolves to "not closed by the pin bump" |
| P2 | a put delivered, then a Ping | the Pong acknowledges the put |
| P3 | the resend of a reply created **before** the put | does not acknowledge it |
| P3 | a reliable reply created **after** the put | acknowledges it, and so does its resend |
| P4 | a retransmit of a delivered put | not delivered again (the server's end-of-poll ack still answers it, `:468`) |

Consequence for a reply sent later: holding back only the end-of-poll ack is not enough.
- A Pong, or a snapshot response created on the same session after the put arrived, acknowledges the put too.
- The snapshot response keeps doing so on every resend, even if its first copy were patched.
- So a session with a put awaiting its answer must be **sent nothing created after that put arrived**. Resends of
  earlier packets are safe: they carry the fields they were created with.

**The other runtimes (parity).**
- **TS** (`cultmesh-ts/src/index.ts:5891-5914` at `e4e2a0b9`):
  - It acknowledges each packet at receipt (`createAckForReceived`), before the handler runs.
  - It runs each session's frames through one promise chain (`record.work`), so handling is serial per session.
  - `onDocumentPutRaw` may return a Promise and an application-level receipt document, which is sent when it
    resolves (`:5951-5972`).
  - So TS already "replies later", but at the application level, and its transport ack does not mean handled.
- **Python** (`cultmesh-py/src/cultmesh_py/server.py:407`, `_handle_raw_put` `:415`): the handler is synchronous
  and returns its responses.
- Neither has a durability meaning for its ack. Parity is follow-up **F-Q5a** (Q5.8).

**Consumers of the sink trait** (`CultMeshRudpRawDocumentSink`):
- Odin's `SinkHandle` (`main.rs:168-172`).
- Ratatoskr's catalog test (`Ratatoskr/crates/ratatoskr-core/tests/catalog.rs:29`).
- CultLib's own tests, which use closures through the blanket impl (`rudp_document_server.rs:60-67`).
- Vendored copies (StreamPixels, Heimdall) do not implement it.

**cultcache-rs write outcome (R1 batch 3, `93ac5945`).**
- Every failed single-file write carries `CultCacheStoreWriteFailed { kind: NotReplaced | ReplacedNotDurable }`.
- Any other error from `compare_exchange_snapshot` (lock, read, decode, validation) comes before the write step,
  so the file is unchanged.
- One hole: `with_exclusive_lock` (`cultcache-rs/src/lib.rs:924-932` on that branch) returns the **unlock** error
  after a write that succeeded, with no marker. Odin would read that as "not replaced". Its next write would then
  find its own snapshot and report `ForeignStoreWrite`, and Odin would end.
  - That is rare (unlock failing), and it belongs to R1: follow-up **F-Q5b**, not this cut.

### Q5.2 Target shape

1. **CultLib (cut C5).** A put's sink may answer later.
   - While any put of a session awaits its answer, the server sends that session nothing new.
     - The one exception is resends of packets created before the put arrived.
     - A snapshot request behind the pending put waits in the session's order.
   - An accepted answer releases one ordinary acknowledgement.
   - A refusal ends the session with the Error frame the put-serve branch sends.
   - The wire does not change, and existing publishers keep "acknowledged means the sink accepted", which for Odin
     is durable (Q5-A).
2. **Odin (cut W1).** One writer thread owns the store file after activation.
   - The loop owns the working set, decides when to write, and answers put replies from the writer's outcomes.
   - Every put accepted while a write is in flight is covered by the next single write (group commit).
   - Route challenges and catalog reads never wait on the writer.
3. **Odin (cut W0), before W1.**
   - A pin bump.
   - S-2: a `served_record` override so Odin's admission is exact.
   - S-3: the `attempted` list dies, because the typed write outcome says what the file holds.

### Q5.3 Identity, lifecycle, authority

| Kind | Named by | Over time | Decided by |
|---|---|---|---|
| pending put (CultLib) | a server-minted `u64` put id, unique for the server's life, never reused across session generations | created when an admitted put is offered to the sink; ends when it is answered, or when its session ends (the answer is then discarded) | the sink answers; the server alone sends |
| put reply handle (CultLib) | the put id, inside a `CultMeshRudpPutReply` | owned by whoever the sink hands it to; consumed by `accept` or `refuse`; dropping it unanswered refuses | the sink's owner |
| working-set version (Odin) | a `u64` counter in `MemoryOdinTopologyStore` | +1 on every mutation that changes a record; never reset within an activation | the loop |
| written-through version (Odin) | a `u64` in `RuntimeState` | raised only by a writer outcome `Written { version }` | the writer's outcome, applied by the loop |
| write request (Odin) | its version | at most one in flight; replaced by nothing (the next request is built when the outcome arrives) | the loop submits; the writer executes |
| what the file holds (Odin, `flushed`) | none: one snapshot | set at activation from the load; replaced on `Ok(true)` and on `ReplacedNotDurable` | the writer alone |

"Dirty" stops being a flag (`lib.rs:502`, `dirty: Cell<bool>`): it is derived as `version > written_through`.

### Q5.4 Cuts

#### Cut C5. A put's sink may answer later (CultLib, cultmesh-rs, behaviour)

- **Repo/branch:** CultLib `hands/cultmesh-put-reply-later` from `main` after the put-serve branch merges (Q5.7).
  Anchors below are at put-serve `e4e2a0b9`; they move by Cut D's diff (Q5.7 names the conflicts).
- **Deletes first:** the synchronous sink call and its error mapping in the put arm, `rudp_document_server.rs:621-633`
  (they are replaced, not wrapped). Nothing else is deleted.
- **Public API** (default D-1 below):

  ```rust
  pub trait CultMeshRudpRawDocumentSink {
      /// Answer through `reply` now or later, from any thread.
      fn accept_raw_document(&mut self, receipt: CultMeshRudpRawDocumentReceipt, reply: CultMeshRudpPutReply);
  }
  // Closures keep working and answer at once:
  impl<F: FnMut(CultMeshRudpRawDocumentReceipt) -> Result<()>> CultMeshRudpRawDocumentSink for F { .. }

  #[must_use] pub struct CultMeshRudpPutReply { /* put id, Sender to the server */ }  // Send, !Clone
  impl CultMeshRudpPutReply {
      pub fn accept(self);
      pub fn refuse(self, reason: impl std::fmt::Display);
  }
  // Drop without an answer refuses: "the put was not answered".
  ```

  - `CultMeshRudpRawDocumentReceipt` (`:37-43`) is unchanged; the reply is a separate argument, so the receipt stays
    `Clone + PartialEq`.
  - `CultMeshRudpPollOutcome` (`:286-290`) is unchanged. A later refusal is returned as `ApplicationRejected` with
    `SinkRefused(reason)` from the `poll_once` that sends it.
- **Server changes, `rudp_document_server.rs`:**
  - `CultMeshRudpDocumentServer` (`:303-311`) gains:
    - The answer channel (`std::sync::mpsc`: the server keeps the receiver, each reply holds a sender).
    - `answered: VecDeque`, the answers taken off the channel and not yet handled.
    - A next put id.
    - `pending: BTreeMap<u64, CultMeshRudpSessionKey>`.
  - `SessionEntry` (`:292-297`) gains `pending_puts: BTreeSet<u64>` and `waiting: VecDeque<(u32, CultNetMessage)>`,
    the delivered frames that may not be answered yet.
  - **One predicate owns "withheld":** `fn acknowledgement_withheld(&self, key) -> bool`, true while
    `pending_puts` is non-empty. The S-5 fix, if the ack map takes it here, adds its clause to this function and
    nowhere else.
  - `poll_once`:
    - Drain answers first (before `maintain`, `:366`). Each answer is looked up in `pending`; an unknown id (its
      session ended) is discarded.
    - Refuse → `end_rejected_session(key, reason)`, and return one `ApplicationRejected`. Further answers stay in the
      channel for the next poll.
    - Accept → remove the id. When the session has none left, it is released: send its ack (the rule for which
      poll sends it is under the put arm below), then handle `waiting` in order.
    - `:436-438`: a `result.reply` (Pong) is not sent while withheld.
    - `:468-476`: the end-of-poll ack is not sent while withheld.
  - `deliver_application_message` (`:565`):
    - Put arm (`:572`): admission is unchanged (put-serve `:587-619`). Then mint the id, record it, and call the sink
      with a reply.
    - `poll_once` looks at answers again after the delivered-frame loop (`:445-466`), before the end-of-poll ack,
      but handles only those for this poll's session. The rest stay queued for the next poll's first step. (Answers
      move from the channel into one server-side queue, so they can be taken selectively.) A sink that answered at
      once is therefore acknowledged, or refused, in the same poll with the same datagrams as today, and another
      session's refusal can never cut this session's ack short.
    - A release sends `create_ack()` from the drain, except for the session whose packet this poll is handling:
      that session gets its ordinary end-of-poll ack, so a synchronous sink's datagrams stay exactly as they were.
    - Snapshot arm (`:634`): if the session is withheld, push the request onto `waiting` and return; otherwise
      answer as today.
    - A put that arrives while `waiting` is non-empty also queues, so a session's frames are handled in order. A
      put behind another put that is only pending is offered at once, so pipelined puts can share one write.
  - Session end, wherever a session is removed (`:441`, `:486-489`, `:523`, `end_refused_session`,
    `end_rejected_session`, Cut D's `end_unsendable_session`): remove its ids from `pending`, so their answers are
    discarded.
    - **Put this in one `remove_session(key)` helper, so no removal path can forget it.** It is the one new helper
      the cut earns: there are six removal sites today.
  - Doc comments: the struct doc (`:300-302`, "no background thread") stays true. The sink trait doc says what
    "acknowledged" means: the sink accepted.
- **Authority map:**
  - Owner of what the peer is told about a put: the server, and only through `poll_once`. The sink decides accept or
    refuse; it never sends.
  - Inputs: sink answers (channel); transport receipts.
  - Outputs: one ack per release; one refusal per refused put's session.
  - Derived: "withheld" is derived from `pending_puts`; it is not a flag.
  - Forbidden writers:
    - No send path to a withheld session except `maintain`'s resends.
    - The sink must not be able to acknowledge by returning. The return type carries no answer, so this holds by
      construction.
  - Shared paths: a synchronous closure sink and a deferred sink answer through the same reply and the same drain.
    There is no second code path for "answered at once".
- **Verification (CultLib, Yggdrasil):**
  - Wire-level test, the observer layer: a raw client socket records every datagram the server sends to its session.
    - With the reply held across at least five resend intervals, including a Ping and a snapshot request on that
      session, no datagram acknowledges the put's sequence.
    - After `accept()`, the next poll sends an ack that does, and then the snapshot response.
    - Mutations that must fail it: sending the end-of-poll ack regardless; sending the Pong; answering the snapshot
      request at once.
  - Another session's snapshot request is answered while the first session's put is held (the loop is not blocked).
  - `refuse()` and dropping the reply: the peer receives `cultnet.error.v0` and a goodbye, and nothing acknowledges
    the put (reuse the put-serve refusal assertions).
  - Two pipelined puts on one session, each with its own held reply:
    - Answering the second first releases nothing.
    - Answering the first then releases one ack that covers both.
    - This pins that a later put's acknowledgement never leaves before an earlier one's.
  - Session ends while held (peer Disconnect; a new Connect from the same key): a later `accept()` sends nothing to
    the new generation, and `pending` is empty.
  - Closure sink: the datagram sequence is byte-identical to the pre-cut server for accept and refuse. This pins that
    the synchronous path did not change.
  - An unservable put is refused before the sink sees it: the sink is never called and no reply is minted.
  - `cargo mutants --in-diff` on the cut's diff; every survivor is triaged.
- **Estimate:** about +170 source, -15 source, +350 tests. No new target, dependency or wire field.

#### Cut W0. Pin bump, exact admission and the one-candidate store (Odin, subtraction first)

- **Repo/branch:** Odin `hands/odin-write-w0` from `hands/odin-write` (`434756e`, or batch 4's tip). It bumps the
  CultLib pin to the main commit holding Cut D, put-serve, R1 batch 3 and C5 (Q5.7).
- **Deletes first:**
  - `attempted` (`lib.rs:494-499`, `:525`), the retry loop over it in `flush` (`:619-625`) and its push (`:628-633`).
  - The tests that describe a file holding some older failed attempt:
    - `a_write_that_failed_before_replacing_keeps_every_earlier_attempt_odins` (`lib.rs:2172`).
    - `the_file_may_hold_a_later_failed_attempt_not_only_the_first` (`:2205`).
    - Each models a `NotReplaced` failure that nonetheless changed the file, which the typed outcome now rules out.
- **Changes:**
  - `flush` (`lib.rs:612-640`) matches the error's `CultCacheStoreWriteFailed` kind (`downcast_ref`):
    - `ReplacedNotDurable` → `flushed = current`, and the store stays dirty so the next write re-syncs.
    - Anything else → no change.
  - `a_write_that_failed_after_replacing_the_file_is_not_another_writer` (`:2141`) is re-targeted to inject
    `ReplacedNotDurable` as the real outcome, not a hand-made replace.
  - `a_failed_write_is_made_by_the_next_without_another_change` (`:2118`) stays.
  - **S-2:** Odin's `SnapshotHandle` (`main.rs:174-181`) overrides `served_record`. It builds the envelope as
    `persist_generic_document` (`:1462`) would store it, then serves it through `peer_document` (`:1400`), the
    function `drop_unservable_documents` (`:1428`) sizes with. Admission and activation then use one sizing.
    - A runtime-presence put keeps the default (as received). Presences are served in `public_document`'s shape and
      are never near the bound, and `drop_unservable_documents` does not check them either.
    - A schema that `persist_generic_document` would refuse returns an error, which the server turns into a
      refusal before the sink.
  - `accept_raw_document`'s doc (`main.rs:332-351`) drops its S-5 paragraph; it moves to W1's writer doc.
- **Authority map:** what the file holds is decided by the one write outcome, not by a list of guesses.
  `ForeignStoreWrite` means exactly "a snapshot Odin did not write".
- **Verification:**
  - Test: a `ReplacedNotDurable` failure, then a change, then a write → made over the failed snapshot.
  - Test: a `NotReplaced` failure, after which another writer puts the failed snapshot into the file → `ForeignStoreWrite`.
    Mutation: keeping a candidate fails it.
  - Test: 1,000 failed writes leave nothing retained. This pins S-3; observe the store's retained records, or the
    absence of the field.
  - S-2 test: a 1.2 MB put is refused as `DocumentUnservable` and is not in the working set.
  - Mutation: sizing with the received record instead of `peer_document` survives nothing.
  - Negative: `rg -n attempted crates/odin-daemon/src` matches nothing.
  - The pin bump carries ack Cuts 1-3 and D into Odin. The full Odin suite runs on Yggdrasil, and Soul reads this cut
    as a transport change, not only a subtraction.
- **Estimate:** about -150 (including the two tests) / +60.

#### Cut W1. The writer thread (Odin, behaviour)

- **Repo/branch:** Odin `hands/odin-writer` from W0.
- **Deletes first:**
  - `MemoryOdinTopologyStore::flush` (`lib.rs:602-640`), its `path` (`:490`) and `flushed` (`:493`) fields, and the
    `dirty` flag (`:502`, `:600-602`).
  - `RuntimeState::flush` (`main.rs:317-330`).
  - The `self.flush()?` in `accept_raw_document` (`main.rs:363`).
  - `stop`'s direct write (`main.rs:891-898`).
  - The flush step of `serving_pass` (`main.rs:933-941`).
  - No path is left on the loop that can write the file. That holds before the writer is added.
- **Keeps:**
  - `MemoryOdinTopologyStore::load` (`lib.rs:509`): the one locked read, at activation, on the loop, before any writer
    exists. `load` returns the loaded snapshot beside the store, to become the writer's `flushed`.
  - `ForeignStoreWrite`, `WriteLeaseLost`, `survive` (`main.rs:979-994`), unchanged in meaning.
- **Adds, `crates/odin-daemon/src/writer.rs`** (a module, not a crate):
  - `StoreWriter`: `spawn(file, flushed, lease) -> StoreWriter`, `submit(WriteRequest)`,
    `try_outcome() -> Option<WriteOutcome>` and `finish(self)`.
    - The channel is `sync_channel(1)`. The loop submits only when nothing is in flight, so `submit` never blocks.
    - `WriteRequest { version, records }`.
    - `WriteOutcome { version, result }`, where `result` is one of `Written`, `Failed(text)`, `Foreign` or
      `LeaseLost(text)`.
  - The thread:
    - Checks the lease (the check extracted from `require_current_write_lease` `main.rs:546-567` into a free
      function that the loop and the writer share), then `compare_exchange_snapshot(flushed, records)`.
    - Applies W0's outcome rule to `flushed`, and sends the outcome.
    - It never touches reply handles, the working set or the socket.
  - Mock point: the thread writes through a one-method trait `StoreFile { compare_exchange_snapshot }`, implemented
    for `SingleFileMessagePackBackingStore`. Tests gate it and count it. The lock-hold (Soul's method) is used where
    the real file must be on the path.
- **Changes, `main.rs`:**
  - `RuntimeState` (`:141-161`) gains `writer: Option<StoreWriter>` (spawned in `try_activate` after the load,
    `:284-305`), `written_through: u64`, `in_flight: Option<u64>` and `replies: VecDeque<(u64, CultMeshRudpPutReply)>`.
  - `SinkHandle::accept_raw_document` (`:168-172`) takes the reply.
  - `RuntimeState::accept_raw_document` (`:352-367`):
    - Validates and mutates as today.
    - Pushes `(store.version(), reply)`, or accepts at once when `version <= written_through` (nothing new to make
      durable).
    - Refuses through the reply on a validation error.
    - Submits if nothing is in flight.
  - A new `serving_pass` step, in place of `:933-941`, run every pass:
    - `try_outcome`. On `Written { v }`: `written_through = v`, and accept every reply with version `<= v`, in queue
      order.
    - On `Failed`: refuse the replies with version `<= v`; they remain in the working set (refusal runs one way, as
      documented at `:344-350`).
    - On `Foreign` / `LeaseLost`: return the error to `survive`, which ends Odin.
    - Then, if nothing is in flight and `version > written_through`, submit when a reply is waiting or when
      `FLUSH_INTERVAL` is due.
    - So puts never wait for the interval; bookkeeping-only changes do.
  - `stop` (`:894`):
    - If the lease is still current and anything is unwritten or in flight, wait for the in-flight outcome, submit the
      rest, and wait for that outcome too. This is a blocking wait; it is the only one, and it happens only on the
      way out.
    - Answer the replies, run one `poll_server` so the acks leave, then `finish` (drop the sender and join the
      thread).
    - After `WriteLeaseLost` or `ForeignStoreWrite`, write nothing. Dropping the replies refuses them.
  - `FLUSH_INTERVAL`'s comment (`:73-77`) and the store's doc (`lib.rs:484-488`) say: the writer thread owns the
    file; puts are covered by the next write; bookkeeping waits for the interval.
- **Authority map:**
  - Owner of the store file after activation: the writer thread, the only holder of a
    `SingleFileMessagePackBackingStore` for the store path. It also owns what the file holds (`flushed`).
  - Owner of the working set, of when to write, and of put replies: the loop (`RuntimeState`).
  - Inputs:
    - Writer: one request at a time, and the lease file.
    - Loop: provider puts, Idunn's projection, the lease file, and writer outcomes.
  - Outputs: one atomic `.cc` snapshot per request; one outcome per request; replies answered in version order.
  - Derived state:
    - `dirty` is derived (`version > written_through`).
    - `flushed` is no longer the loop's; it lives only in the writer.
    - The interval decides only bookkeeping-only writes.
  - Forbidden writers: no loop code path calls `compare_exchange_snapshot`, takes the store's lock, or answers a
    put reply before the outcome covering its version.
  - Shared paths: peer puts, self-presence, refresh, activation-time drops and the stop write all mutate the working
    set, and reach the file only through `submit`.
  - Deletion line: every loop-side write (above) is deleted before `writer.rs` is added.
- **Batching rule, stated once.** The loop keeps at most one write in flight. Everything that changed while it was in
  flight, puts and bookkeeping alike, goes into the next request as one snapshot of the working set. N puts accepted
  during one write cost one more write and one fsync pair, not N.
- **Backpressure** (Q5-B, recommended: hold):
  - A stalled writer holds put replies. The loop keeps serving, and the working set keeps admitting (the catalog
    serves admitted-not-durable puts, as it does today after a failed write).
  - The queue is bounded by the server's existing limits: 64 sessions and 32 MiB of admitted payload. It adds no new
    bound.
  - A publisher that stops waiting (the Rust default is 300 ms) disconnects. Its session ends, its answer is
    discarded, and its put is still written by the next write. That is what a publisher sees today during a stall,
    minus the stalled challenges.
- **Ordering.**
  - Replies are answered in version order from one queue, and the server sends acks in answer order.
  - Within a session, C5 releases one ack only when every pending put of the session is answered.
  - So no later put's reply leaves before an earlier one's.
- **Crash mid-batch.**
  - The write is one atomic replace, so the file holds the old snapshot or the new one.
  - No reply of a batch is answered before its `Written` outcome, so a SIGKILL anywhere loses no acknowledged put.
  - A crash after the rename but before the directory sync is `ReplacedNotDurable` if the process lives; if it dies,
    nothing of that batch was acknowledged.
- **Activation.** The store is read once, by `load`, before the writer is spawned. While the writer lives, the loop
  never reads the file (Cut 1's rule, unchanged).
- **Verification (Odin, Yggdrasil):**
  - **Loop latency under a stall.** This pins Q5's ruling; it is Soul's S-3 probe and O1, inverted into assertions.
    - Hold the store's sibling lock for 1,500 ms, with a provider put in flight and Odin's heartbeat dirty with the
      interval due.
    - A route challenge and a catalog read are each served within 200 ms. The put's publisher sees no ack until the
      lock is released, and then does, with the put on disk.
    - Mutation that must fail it: running the write synchronously on the loop, by calling the `StoreFile` directly
      from `serving_pass`.
  - **Group commit.** Gated `StoreFile`, held on the first write, with 10 puts from 10 publishers accepted meanwhile.
    - After release, exactly 2 writes are made and all 10 publishers are acknowledged.
    - Mutation: one request per put gives 11 writes.
  - **Ordering.** Gated `StoreFile`; put A's write is held and put B arrives.
    - B's publisher is not acknowledged before A's; the answer order is observed at the server's answer channel.
    - Mutation: answering the queue from the back.
  - **Durability.** `daemon_process.rs`, extending `an_acknowledged_put_survives_sigkill` (`:326`).
    - 20 rounds, each a stream of puts from 4 publishers while the test holds and releases the store's lock at random
      offsets. SIGKILL at a random offset.
    - Every put its publisher saw acknowledged is in the file after the kill.
    - Mutation: accepting replies at submit instead of at `Written`.
  - **One writer.** With the writer's `StoreFile` gated shut, N serving passes, a heartbeat and a stop request make no
    write from the loop: the file's identity (`file_identity`, `main.rs:3343`) is unchanged until the gate opens.
  - Negative grep: `rg -n "compare_exchange_snapshot|pull_all\(" crates/odin-daemon/src` matches only `writer.rs`,
    `load` and the tests.
  - **Lease lost with a write queued.** No write is made, Odin ends, and the queued replies are refused. The file's
    identity is unchanged.
  - **Foreign write.** Odin ends without writing over it (existing `:3696`, re-targeted through the writer).
  - **Stop.** SIGTERM with a put held behind the lock: after the release Odin writes, the publisher is acknowledged,
    and it exits 0. With the lease lost, it writes nothing.
  - Re-targeted, not deleted: `changes_inside_one_interval_are_one_write` (`:3356`, now bookkeeping-only),
    `a_put_is_written_before_it_is_accepted` (`:3389`, observed at the publisher's ack),
    `a_put_whose_write_is_not_made_is_refused` (`:3516`), and the `ending_*` tests (`:3728-3800`).
  - `cargo mutants --in-diff` on the cut's diff.
- **Estimate:** about -120 / +230 source, +300 tests. One new module, one thread, no new crate, dependency or target.

### Q5.5 What stays on the loop, and what does not

| Path | Before (`434756e`) | After W1 |
|---|---|---|
| route challenge (`raw_snapshot`, exact self query) | lease-file read, sign; blocked behind any write on the loop | the same reads; never behind a write |
| catalog read (`stored_snapshot`) | working set, projection read | unchanged; never behind a write |
| peer put | mutate, then write and fsync on the loop | mutate, queue the reply, submit; the reply is sent after `Written` |
| heartbeat, refresh | mutate; the interval write on the loop | mutate; the interval write on the writer |
| store lock | taken by `flush` on the loop | taken only by `load` (activation) and the writer |

### Q5.6 What only Yggdrasil can show

- **Rerun Soul's ABBA** (`soul-c3/latency_cmd.txt`: `soul_loop_stall_under_live_put_rate`, base against cut, in
  interleaved rounds, with `/proc/pressure/io` before and after each).
  - The base is `434756e`; the cut is W1's tip. Expect route-challenge p99 to stop tracking IO pressure, while put
    round trip p99 still does.
  - Soul's figures at `434756e` and its base over 8 rounds, under 5-38% pressure: challenge p99 310-2,129 ms, put
    round-trip p99 1,121-2,593 ms (`soul-c3/latency.log`).
- Whether the loop's remaining disk reads (the lease and projection files) ever stall under real pressure. The ABBA
  run shows it as challenge p99 outliers that coincide with pressure; if they appear, open the lease-in-memory
  follow-up.
- After deploy, section 6's script, plus the count of "admitted odin route continuity: timed out" per hour.

### Q5.7 Sequencing (recommended order)

1. **Cut D merges to CultLib main** (already slated first).
2. **The put-serve branch rebases onto it and merges.** The conflicts:
   - Cut D changes `send_packet` to return `Option<io::Error>` and adds `end_unsendable_session`.
   - Its snapshot-send failure builds `reason: format!(..)`, where the put-serve branch made `reason` a typed
     `CultMeshRudpRejectionReason`. It needs a variant (for example `ResponseSendFailed(String)`), and the put-serve
     Hands owns it.
   - Both branches edit `poll_once`'s ack site (`:468-476`) and `end_rejected_session`.
3. **R1 batch 3 merges** (cultcache-rs only; no conflict with 1-2).
4. **C5 is cut from that main** (it rewrites the lines 1-2 touched, so it goes after them), then Soul, then merge.
5. **One Odin pin bump to that commit, as W0**, carrying S-2, S-3 and the transport cuts. Then **W1** on top.

W0 may start on the commit after step 3 if C5 slips, at the cost of a second bump before W1. One bump is recommended:
each bump moves Odin across ack Cuts 1, 1b, 1c, 3 and D, and a single Soul pass over that transport delta is cheaper
than two.

### Q5.8 Follow-ups this map does not own

- **F-Q5a (parity).** TS and Python acknowledge a put at receipt, and TS's reply-later is an application receipt.
  - Either they adopt C5's rule ("acknowledged means the sink accepted"), or the QUIC campaign defines an
    application-level put reply for every runtime.
  - QUIC has no transport ack an application can read, so it needs one anyway. Recommended owner: the QUIC campaign.
    Rust C5 is then the RUDP-era contract, and TS's receipt is the seed of the QUIC shape.
- **F-Q5b (R1).** `with_exclusive_lock` returns an unlock error after a successful write, with no write-outcome
  marker (`cultcache-rs/src/lib.rs:924-932` at `93ac5945`). Odin would call its own next write foreign. Fix it in
  cultcache-rs: tag it, or ignore the unlock error, since dropping the file releases the lock.
- **S-5 interface.** C5 adds `acknowledgement_withheld` as the one place that decides "send no ack to this session".
  The ack map's S-5 decision (probe P1) plugs in there as a second clause if it is fixed in the document server. C5
  does not fix S-5.

### Q5.9 Subtraction ledger (estimate)

| Cut | Removed | Added | Targets, dependencies, wire |
|---|---|---|---|
| C5 (CultLib) | ~15 | ~170 source, ~350 tests | none; sink trait signature changes (Odin, Ratatoskr test) |
| W0 (Odin) | ~150 (the `attempted` list and 2 tests) | ~60 | pin bump |
| W1 (Odin) | ~120 (every loop-side write) | ~230 source, ~300 tests | one module, one thread |

Net source is positive: about +175. It buys the ruled capability (challenges and reads independent of the disk, with
Q3 B kept), and it removes S-3's unbounded list.

### Q5.10 Operator questions and defaults

- **Q5-A. What does a put's reply mean on the RUDP wire once it can come later?**
  - A: the document server withholds the transport acknowledgement until the sink answers. Nothing created after the
    put is sent to that session until then. The wire is unchanged, and every existing publisher (Rust
    `publish_cultnet_message_to_rudp_catalog`, TS and Python publishers) keeps "acknowledged means accepted", which
    for Odin is durable.
  - B: a new CultNet message, a put result, sent after durability; the transport ack means received. This is a wire
    change in the C# reference and four runtimes. Every Odin publisher that waits for the transport ack today must
    be changed to wait for it; until then Q3 B silently weakens to "received".
  - **Recommended: A.** It keeps Q3 B for every publisher with a server-only change, and RUDP is frozen except for
    crash and data-loss fixes. B is the right shape under QUIC (F-Q5a), not a reason to change the RUDP wire now.
    - What depends on it: C5's whole design.
    - A also narrows the ack map's Q-A2 meaning for the document server: acknowledged means delivered *and answered*.
      That was already true of the synchronous sink.
- **Q5-B. A writer stalled for seconds: hold put replies, or refuse with a typed error?**
  - A: hold. Replies wait, bounded by the server's session and payload limits. Publishers time out on their own
    clock, and the put is written when the disk returns.
  - B: refuse once more than N replies wait or the oldest waits longer than T. This needs a new `cultnet.error.v0`
    code (the error-contract map owns the registry) and two tunables.
  - **Recommended: A.** A refusal ends the publisher's session exactly as its own timeout does, but adds a code and
    two numbers, and it tells the publisher less than the truth: the put may still become durable. B earns its place
    only if publishers ever wait without a timeout.
- **Default D-1 (Self, overridable). The API is a completion handle** (`CultMeshRudpPutReply`: `accept`, `refuse`,
  and Drop refuses).
  - The alternatives were a token with `server.answer_put(token, result)`, and a reply method keyed by session and
    message id.
  - Keying by session and message id is ambiguous: message ids are publisher-chosen, and a reconnect reuses the
    session key.
  - A token cannot be answered from another thread without routing it back to the server, and a forgotten token
    leaves a session withheld until it ends.
  - The handle is the Rust expression of TS's Promise-returning `onDocumentPutRaw`, and it keeps closure sinks
    source-compatible.

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
