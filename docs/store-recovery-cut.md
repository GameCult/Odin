# Odin store repair and recovery: cut map

**Operator rulings, 2026-09-30:**
- **Q-R1:** "You got it". A foreign file under a running Odin is set aside and Odin continues.
- **Q-R2:** "Yep". Keep the previous generation on disk.
- **Q-R3:** "Start empty". Nothing decodable at activation means Odin starts empty.
- **Q-R4:** "Yep, and Idunn needs better reporting capabilities so it can serve as watchdog. I ought to get a message
  about this on Discord if it ever happens."
  - Stop writing after the third set-aside in 10 minutes, and keep serving.
  - **New requirement:** Idunn watchdog reporting that reaches the operator on Discord. It is mapped as its own
    campaign and is not an Odin cut.
- **Q-R5:** "That's fine, Odin still runs after all, right?" Yes. Odin keeps serving, and the store condition is
  published as its own typed record, not as a degraded presence.
- **Q-R6:** "Approved". Both drills.

**R1 status (Self, 2026-09-30).** Built on CultLib `hands/cultcache-read-failures` (`c0b56e06`..`b7908668`); it
is in Soul.
- **Consumer follow-up that must travel with the next Muninn CultLib bump:**
  `Muninn scripts/restart-starfire-muninn.ps1:144` matches `*failed to decode MessagePack*` to trigger
  `Reset-CorruptMuninnStore`. R1 removes that text, so the reset would stop firing silently. The script must match
  the typed `CultCacheStoreUnreadable` kind, or the new wording, in the same change.
- C#'s truncated-store exception (`EndOfStreamException`) is an `IOException`, the same I/O-versus-undecodable
  overlap. It goes with R3's C# work.

Status: cut map, Imagination pass 1 (Opus), 2026-09-30. Nothing has landed. This map owns the means for repairing
and recovering `/var/lib/gamecult/odin/topology.cc`. It follows `docs/write-pattern-cut.md` (Cuts 1-3) and starts
from that campaign's landed head.

Anchors:
- Odin `hands/odin-write` at `cda2087` (Cuts 1-2 plus two test commits). Q3 B (peer puts fsynced before ACK) is
  being finished on that branch now. **Re-anchor every Odin `file:line` below on the head that lands; the line
  numbers here are `cda2087`.**
- CultLib `6d0cc963`. `packages/cultcache-rs` is byte-identical to Odin's pin `3bf1c0ce`, so the Rust line numbers
  hold at both.
- Idunn `a96ad9d`, Heimdall `e8e2832`, gamecult-ops `runbooks/gamecult-state-backup.md` as of 2026-09-30.

Operator, 2026-09-30, on the three options Self offered for an undecodable foreign write (degraded but loud, end
Odin, unchanged): **"None of these options involve repair and recovery, which is what ultimately needs to happen."**

Open: Q-R1 to Q-R6 (section 8). D1 (section 6, O1) is a Self default the operator may overrule.

## Short answer

Odin already holds the one thing a repair needs: its in-memory working set, which is authoritative for everything
it owns while it holds the write lease. Today it never uses it for repair. It ends on a decodable foreign write, and
it silently stops persisting on an undecodable one. At activation it has no working set and no second copy, so it
crash-loops.

The repair has one rule: **a store file that is not what Odin last wrote is set aside, never deleted and never
merged, and Odin writes its working set over it under the lease.** At activation Odin has no working set, so it
falls back to a previous generation kept on disk, and then to an empty store that providers refill. The
correlation sequence gets a time floor, so no rollback can recreate the watermark deadlock. Recovery is published as
a typed record in Odin's own catalog. The mechanism (typed read failures, a set-aside exchange, a retained previous
generation, no staging file left behind by a failed write) belongs to CultLib, because every consumer of the
single-file store has the same gap.

## 1. Body facts (probed)

### 1.1 Probe: cultcache-rs single-file store under fault

The probe was a scratch crate at CultLib `3bf1c0ce`, `scratchpad/odin-recovery-probe` (Imagination session), commit
`b19eba9`, run through the stopgap on Yggdrasil in slot 2. It ran with `--tmpfs /small:size=64k --tmpfs /ro:ro`,
using `trap '' XFSZ; ulimit -f 40` for the file-size case. The log is `scratchpad/odin-recovery-probe.log`.

| Fault | Result |
|---|---|
| P1: file truncated to half | `compare_exchange_snapshot` and `pull_all` return `Err`, **not** `Ok(false)`. There is no `io::Error` in the chain; the decode error is flattened to text (`lib.rs:720-725`). The message is wrong: "invalid type: string \"cultcache.store.v1\", expected struct CultCacheEnvelope". `store_header` (`lib.rs:317-341`) deserializes the *whole* array to find the header, so a truncated v1 file has no header and falls through to the legacy decoder. The file is unchanged by the failed exchange |
| P2: 12 bytes of garbage appended | `pull_all` returns `Ok`, all 3 records. `compare_exchange_snapshot(expected)` returns **`Ok(true)` and silently drops the tail**. `rmp_serde::from_slice` ignores trailing bytes. C#'s `DeserializeSnapshot` (`CultDocumentMessagePackSerialization.cs:179-215`) never checks for the end either |
| P2b: one byte flipped inside a payload | `pull_all` returns `Ok` (payloads are opaque bytes). `compare_exchange_snapshot` returns **`Ok(false)`**. To Odin, bit rot is a *decodable foreign write* |
| P3: header `cultcache.store.v4...` | `Err` with a distinct message, "not readable; this runtime reads v1 only". It is distinguishable only by its text |
| P4: another handle wrote a valid snapshot | `Ok(false)` |
| P5: `hard_link(topology.cc, topology.cc.displaced.<ts>.x)`, then an exchange through the store | The link keeps the old bytes exactly, and the store writes the new file. The staging sweep (`remove_abandoned_staging_files`, `lib.rs:2730-2766`) removes a `<name>.<uuid>.tmp` beside it and leaves the displaced name alone |
| read-only filesystem, no store | `pull_all` returns `Ok(0)`. The exchange fails opening the lock: `io::Error` `ReadOnlyFilesystem` (os 30) |
| `RLIMIT_FSIZE` 40 KiB, 200 KB write | `Err` `io::Error` `FileTooLarge` (os 27). The old file is intact and still decodes. **A 40,960 B partial staging file is left behind.** The next attempt sweeps it and leaves a new one |
| 64 KiB tmpfs, 200 KB write (ENOSPC) | `Err` `StorageFull` (os 28). The old file is intact. **A 61,440 B partial staging file is left behind: all of the filesystem's remaining space.** The same holds on every retry |

What follows from the probe:
- **Decodable is not the same as legitimate.** A flipped payload byte is "decodable and foreign", and appended
  garbage is "decodable and ours". The decodable/undecodable split cannot be what decides recovery. One rule has to
  cover both: *not what Odin last wrote*.
- **A write that fails for lack of space holds the space it failed on.** `/var/lib/gamecult` is shared by every
  service on Yggdrasil. Odin retries once per second (and, under Q3 B, on every put), so it re-takes whatever
  another service frees. C# deletes its temp file in a `finally` (`CultCache.cs:3496-3531`); Rust does not
  (`write_all_unlocked`, `lib.rs:741-768`). This is a Rust parity defect.
- **The atomic write keeps no previous generation.** `write_all_unlocked` stages, fsyncs, renames over the file and
  fsyncs the directory. After the rename, the old inode is gone. The brief's "last good checkpoint kept by the
  atomic write" does not exist on disk. The only last-good copy is Odin's in-memory `flushed` snapshot
  (`lib.rs:506-510`), and it dies with the process.

### 1.2 Odin today (`cda2087`)

| Case | Path | Outcome |
|---|---|---|
| Decodable foreign write while serving | `MemoryOdinTopologyStore::flush` `lib.rs:613-626`: `compare_exchange_snapshot(flushed, current)` returns `false`, so `ForeignStoreWrite`. `survive` `main.rs:879-895` returns it, and Odin ends | Idunn restarts Odin (`control_plane.rs:68-71`: 6 restarts per hour, the wait doubling from 5 s). The new Odin **loads the foreign file as truth**. Odin's own unflushed changes, and every record of Odin's that the foreign file lacks, are lost without a trace. A flipped payload byte (P2b) takes this path |
| Undecodable foreign write while serving | the same exchange returns `Err` (decode). `survive` logs it and retries every second, forever. Nothing is persisted again | **Idunn's crossing breaks too.** `receive` (`Idunn drivers.rs:5005-5030`) reads the same file and propagates the error to `admit_latest_topology` (`control_plane.rs:7903-7907`) and `refresh_admitted_topology` (`:6040`). Every Odin-correlated deployment and refresh fails until the file is readable, which is never |
| Corrupt file at activation | `try_activate` `main.rs:246-297` calls `MemoryOdinTopologyStore::load` `lib.rs:519-538`, which calls `pull_all()?`. `main.rs:762` `while !try_activate()?` exits the process | Crash loop until Idunn's budget is spent. Then Idunn reports once, to its log only (`control_plane.rs:5709-5723`, `report_once` `:4599-4610` is `eprintln`), and stops. The Verse rendezvous is down |
| Disk full or read-only | the exchange returns `Err` (io). `survive` logs and retries every second | Odin serves from memory. Each attempt leaves a partial staging file (1.1). A stop or deploy in this state loses everything unflushed. Nothing typed says so |

Detection latency for a foreign write: `flush` writes only when dirty (`main.rs:307-314`). Odin's own heartbeat
(`HEARTBEAT_INTERVAL` 5 s, `main.rs:69`) dirties the set, so a foreign write is met at the latest one heartbeat plus
one flush interval (about 6 s) after it happens. No polling read is needed, and Cut 1's rule (no reads while
serving) stands.

### 1.3 Other sources of truth, and what they can recover

| Source | Holds | Freshness | Recovers |
|---|---|---|---|
| Odin's working set (W) | everything, including unflushed changes | live | everything, but only while this process lives and holds the lease |
| `flushed` (in memory) | the last snapshot Odin wrote | at most one interval old | nothing more than W; it is the comparison base, not a store |
| previous generation on disk (new, O3 and R3) | the snapshot before the last one this store's owner verified and replaced | one write old | everything except the last write's delta, including publish-once documents, presence history and the watermark |
| nightly backup (`gamecult-state-backup.md`) | `/var/lib/gamecult/odin` whole, shared-lock copy | up to 24 h old, 3 days on Yggdrasil, 14 days plus 8 weekly on Raven | publish-once documents. **Never automatically**: "It never becomes live authority, and nothing restores it automatically"; "restore a store only after stopping its owner" |
| providers republishing | presences (2.7-12 s), Muninn documents (2 s), ads (about 30 s); Heimdall HEAD republishes `heimdall.command_boundary` and its three siblings every 60 s (`Heimdall src/index.ts:19,35-39`, gated on its write lease) | seconds to a minute | every document whose publisher republishes |
| Idunn's projection plus presences | correlations | one refresh pass (250 ms) after the presences return | correlations. They are a projection and never need restoring |
| Idunn's `odin_publisher_sequence_cursor` (`control_plane.rs:1456,1537,6081`) | the highest correlation sequence Idunn admitted, per target | live | nothing directly (Idunn owns it), but it is why the watermark must never roll back |

Premise correction: `heimdall.command_boundary` is not publish-once at Heimdall HEAD. A `stored_at` of 09-23
proves only that the publisher's own `stored_at` has not changed, because Odin stores the publisher's
`stored_at` (`persist_generic_document`, `main.rs:1330-1347`). Whether the deployed Heimdall re-puts is
unverified; O3's outstanding-republish list measures it.

**The watermark is the one record whose loss is not self-healing.** Idunn refuses any correlation sequence at or
below its cursor (`control_plane.rs:1763,1785`). An Odin that starts from an empty store, from the previous
generation, or from a backup restarts a target's count below that cursor. That is the deadlock documented at
`lib.rs:33-49`: Idunn refuses every correlation until the count climbs past the old mark, and the refusal
withdraws the correlation before it can climb. Every recovery source except W has this defect. O1 removes it.

Other facts the design leans on:
- **Idunn fences before it grants.** `CultCacheWriteLeaseDriver` (`Idunn drivers.rs:4050-4205`): "revocation
  follows an exact incumbent stop", and grant is a nonblocking exchange from empty. A second *Odin* cannot hold
  the lease while the first is still writing, so a foreign writer seen by a lease-holding Odin is not an Odin.
- **Idunn has a lifecycle brake** (`idunn-provision lifecycle-brake-engage|release|status`,
  `provisioning.rs:95-97`; `odin-lifecycle-brake.cc` per `runbooks/odin-yggdrasil.md:29-30`). An engaged brake
  denies continuity restarts (`control_plane.rs:5771-5774`, `:6219`). This is the operator's hold for a manual
  restore.
- **A non-`active` Odin presence fans out.** `reconcile` sets `ready` only when the presence state is `active`
  (`lib.rs:946`), and `dependency_evidence` (`lib.rs:1184ff`) makes every managed dependent of Odin not Ready.
  The presence schema allows `degraded` (`cultnet-rs runtime_authority_contracts.rs:193`), but using it would
  be a readiness statement about every dependent, not a store report.
- **The catalog serves only peer types and presences** (`public_document`, `main.rs:590-640`). No `odin.*` record
  is served today, and neither Odin nor Idunn publishes an Eve surface. F3 in the write map (the unowned
  `surface:gamecult.network.status`) is where an Eve lowering belongs.

## 2. Ends

1. No single fault, whether a foreign write of any kind, a corrupt file, or a failed write, ends Odin or leaves it
   silently unpersisted. Only losing the write lease ends it.
2. A store file that is not what Odin last wrote is preserved byte for byte beside the store and never deleted,
   merged or adopted by any automatic path. The owner (Odin under the lease) writes its own state over it.
3. At activation a corrupt store is recovered from the best source on the host (previous generation, then empty),
   and never from the backup automatically.
4. No recovery can make a correlation sequence repeat or fall below one Idunn has admitted.
5. Every repair, and every stretch in which Odin is serving without persisting, is typed state that can be
   queried through Odin's catalog while it is happening, including when the disk refuses writes.

Invariants carried over: one writer (the lease, checked immediately before every write); Idunn's lockless
crossing reads a whole `.cc` snapshot at `topology.cc`; the path never becomes absent (Idunn reads an absent file
as an empty snapshot, which withdraws every correlation); no reads of the store while serving; one bad record
never stops Odin.

## 3. Identity, lifecycle, authority

| Persistent kind | Named by | Over time | Decided by |
|---|---|---|---|
| store file `topology.cc` | the state root plus `TOPOLOGY_SLOT` | replaced whole by the owner's exchange; never absent once created | Odin under the lease |
| displaced file (new) | `topology.cc.displaced.<UTC yyyymmddThhmmssZ>.<uuid>`, same directory (it must not end in `.tmp`) | created by a set-aside exchange as a hard link to the replaced inode; never modified; never deleted by any program. Travels to Raven with the nightly backup | created by Odin through CultLib; **disposed of only by the operator** |
| previous generation (new, Q-R2) | `topology.cc.previous` | replaced on every *verified* exchange (current == expected) by a hard link to the outgoing inode; **not** rotated by a set-aside exchange, so it always holds a generation that some writer verified before replacing | CultLib mechanism; Odin opts in |
| store condition record (new) | type `odin.store_condition`, schema `odin.store_condition.v1`, key `topology` | one record held in W; rewritten when the condition changes; flushed with the store; served by the catalog | Odin |
| correlation sequence floor (new rule, not a record) | per target, in the existing watermark | next = max(mark + 1, prior + 1, now in unix ms) | Odin |
| backup snapshot | `/var/backups/gamecult-state/<date>/` and Raven | nightly; never live authority | the operator restores it, with the lifecycle brake engaged (runbook) |

## 4. Authority map (the whole campaign)

- **Owner:** Odin, while it holds the current process-write lease, decides what `topology.cc` holds, when it is
  set aside, and what activation recovers from. CultLib owns the mechanism (read classification, set-aside
  exchange, previous generation, staging cleanup) and no policy. Idunn owns whether Odin runs (continuity budget,
  lifecycle brake, deploys). The operator owns restores from backup and the disposal of displaced files.
- **Inputs:** W; `flushed`; the lease (read immediately before every write); at activation, `topology.cc` and then
  `topology.cc.previous`; the clock (the sequence floor).
- **Outputs:** one atomic `topology.cc`; displaced files; `topology.cc.previous`; the `odin.store_condition` record
  in the catalog.
- **Derived state:** `topology.cc` is a checkpoint and Idunn's crossing, never an input while serving. The
  displaced files are evidence only: nothing reads them automatically. The previous generation is read only by
  activation recovery. The condition record is a report and decides nothing.
  - `ForeignStoreWrite` **is no longer an owner of Odin's life**. A foreign write becomes a set-aside event.
- **Forbidden writers and deciders:**
  - Any path that ends Odin on a store condition. `survive`'s `ForeignStoreWrite` arm (`main.rs:884`) is
    deleted; `WriteLeaseLost` stays, and so does the temporary `PresenceAuthorityRefused` rule.
  - Any automatic delete of a displaced or previous file.
  - Any merge of records from a foreign or displaced file into W.
  - Any automatic restore from the backup.
  - Idunn writing or repairing `topology.cc`.
  - Any second writer of the file inside Odin: activation never writes. It prepares W and `flushed`, and the one
    `flush` makes the first write through the same set-aside exchange.
- **Shared paths:**
  - The interval flush, the Q3 B per-put flush, the stop flush and the activation-recovery first flush all go
    through one primitive, `replace_snapshot_setting_aside`.
  - Every path that reserves a correlation sequence goes through the one floored reservation.
- **Deletion line:** `ForeignStoreWrite` (`lib.rs:630-641`) and its `survive` arm are deleted before the set-aside
  exchange is wired in. The `Err` escape in `load` (`lib.rs:521`, `pull_all()?`) is replaced by the recovery
  ladder, not wrapped.

## 5. Recovery by case

| Case | Detected by | Action | Recovered from | Loss bound | Visible as |
|---|---|---|---|---|---|
| **A. Decodable foreign write while serving** (a hand edit, a tool, a restore made against the runbook, or bit rot in a payload per P2b) | the next flush: current != `flushed` | the lease is checked; the file is set aside (hard link); W is written; Odin keeps serving | W | none of Odin's own records. The foreign file's content is preserved, not adopted | condition event `displaced`, reason `mismatch`, with path, sha256, bytes and time |
| **B. Undecodable foreign write while serving** (truncation, garbage, a foreign format, and after R2 trailing bytes too) | the next flush: current does not decode | the same | W | none. Idunn's crossing is unreadable for at most one heartbeat plus one flush (about 6 s), instead of forever | the same, reason `undecodable` |
| **A/B repeated** (a live foreign writer) | a third set-aside inside 10 minutes in one activation | Odin stops writing and keeps serving; puts are not ACKed (Q3 B: ACK means durable); no new displaced files (Q-R4) | W stays in memory | nothing while the process lives | condition `contested`, since when, with the displaced files |
| **C1. Undecodable store at activation** | `load` gets `Undecodable` | W is built from `topology.cc.previous` if it decodes; `flushed` is set to "unreadable", so the first flush sets the corrupt file aside and writes W | the previous generation | the last write's delta: one interval of Odin bookkeeping, or one fsynced put. **A put ACKed as durable can be lost here; say so in Q3 B's record** | event `recovered-previous`, plus an outstanding-republish list: peer records in the previous generation not yet re-put by their publisher, cleared as they arrive |
| **C2. Nothing decodable at activation** | `.previous` absent or undecodable too | start empty (Q-R3); the first flush sets the corrupt file aside | providers republish; correlations re-derive in one pass; the sequence floor holds | every publish-once document; presence history (F4 makes this harmless) | event `started-empty`; outstanding list is `unknown`. The operator's path is the backup, with the lifecycle brake engaged |
| **C3. Unsupported format at activation** (a newer runtime wrote v4, or a downgrade) | `load` gets `UnsupportedFormat` | refuse to activate. The file is untouched and nothing is set aside | the deploy or rollback that caused it; Idunn owns that | none | Idunn's continuity budget and the Odin journal. This is a deployment fault, not a store fault |
| **C4. I/O error reading the store at activation** (EIO, EACCES after a restore with the wrong owner, EISDIR) | `load` gets an `io::Error` | refuse to activate. Nothing is set aside: **only bytes that were read are ever judged** | the operator fixes the host or the mode | none | as C3 |
| **D. Write fails while serving** (ENOSPC, EDQUOT, EFBIG, EROFS, EIO) | the exchange returns an `io::Error` while staging or renaming | nothing is set aside; the staging file is removed (R1); W is kept; retry at the flush interval; puts are not ACKed; Odin keeps serving | W, when the fault clears | nothing, unless Odin is stopped or redeployed before the fault clears (follow-up I-F1) | condition `unpersisted` since T, with the last error kind (`no-space`, `read-only`, `io`) and the dirty-record count, served from memory |
| **D'. Stop while unpersisted** | the stop flush fails | log it and stop, as today (`main.rs:794-799`) | the next activation loads the last good file | the unflushed delta | the final journal line only. I-F1 is the owner fix |

Rollback safety, for C1, C2 and an operator restore: every source except W carries a watermark below Idunn's
cursor. The floor (O1) makes the next sequence exceed every sequence published before, whatever the store holds.

## 6. Cuts

Order: the Rust substrate gaps first (R1), then the watermark floor (O1, independent and small), the set-aside
primitive (R3), Odin's serving repair (O2), activation recovery (O3), parity hardening (R2, R3b), and finally the
runbook and drill (X1). Each cut is its own branch and its own Soul pass. Odin verification:
`cargo test --locked -p odin-daemon` from the Odin root. CultLib Rust: `cd packages/cultcache-rs && cargo test
--locked`. All of it runs on Yggdrasil through the stopgap. **A permission-based test is unproven there (root).**
The injections below avoid permissions: truncation, a directory at the store path (EISDIR), tmpfs size and
read-only options, and `RLIMIT_FSIZE`.

### R1. cultcache-rs: typed read failures, no staging file left behind, a header that survives truncation (CultLib)

- **Repo/branch:** CultLib `hands/cultcache-rs-store-faults` from `main`. Depends on nothing.
- **Deletes first:** the flattening `map_err(|error| anyhow!(...))` in `read_all_unlocked` (`lib.rs:720-725`);
  `store_header`'s whole-array visitor (`lib.rs:317-341`).
- **Adds:**
  - `pub struct CultCacheStoreUnreadable { path, kind: Undecodable | UnsupportedFormat }`, carried in the error
    chain of every read of a single-file store whose bytes were read and not accepted, so consumers can
    `downcast_ref`. An `io::Error` stays an `io::Error`, and the two never overlap.
  - `store_header` reads only the array header and the first string, reusing `read_array_header` and
    `read_string` (`lib.rs:2661-2700`). A truncated v1 file then reports as an undecodable v1 file, not as legacy.
  - `write_all_unlocked` removes its own staging file on every failure after creating it. This is C# parity with
    `CultCache.cs:3525-3530`.
- **Rules that must die under their own mutation:**
  - a truncated store is `Undecodable`, and a v4 header is `UnsupportedFormat`;
  - an `io::Error` read failure is never classified `Unreadable`;
  - after a failed write, the directory holds no `<name>.*.tmp`.
- **Verification:**
  - Tests describe behaviour. The staging-cleanup test forces the failure without permissions. One option is a
    child process under `RLIMIT_FSIZE` with SIGXFSZ ignored (the probe's method); another is a path whose parent
    fills a size-capped tmpfs, if the test runner can mount one. Hands picks one and names it.
  - `cargo mutants --in-diff`.
- **Estimate:** about -30 / +70.

### O1. The correlation sequence has a time floor (Odin) — default D1

- **Repo/branch:** Odin `hands/odin-sequence-floor` from the landed `hands/odin-write` head. Depends on nothing
  else.
- **Change:** `reserve_publisher_sequence` (`lib.rs:702-723`) returns max(mark + 1, prior + 1, now in unix ms),
  and records it as the mark. The doc comment at `lib.rs:33-49` gains the rule: "A mark read from any older copy
  of the store is a floor, never the next value; the clock makes every rollback safe."
- **Why a floor and not a read of Idunn's cursor:** it needs no new crossing. It holds for every recovery source,
  including the backup, and the host clock is already trusted for presence freshness.
- **Assumption:** the clock does not step back across a rollback by more than the rollback's age. Soul checks that
  Idunn accepts a jump: its rule is "strictly above the cursor" (`control_plane.rs:1763,1785`). No test may pin a
  gap limit.
- **Verification:**
  - Existing tests pinning counts (`withdrawing_a_correlation_does_not_reset_its_publisher_sequence`
    `lib.rs:1996-2014`) are restated as order properties.
  - New test: a store rolled back to an earlier snapshot, then one refresh, yields a correlation sequence greater
    than every sequence published before the rollback.
  - New test: two reservations inside one millisecond are strictly increasing.
  - The clock must be injectable at the reservation. At least one mutant must be a function of the time (an offset
    or a scale), with probe values where it would show.
- **Estimate:** about -5 / +25.

### R3. The set-aside exchange and the previous generation (CultLib, Rust and C#; TS and Python in R3b)

- **Repo/branch:** CultLib `hands/cultcache-set-aside` from R1. Q-R2 decides the previous-generation half.
- **Adds, on `SingleFileMessagePackBackingStore`:**
  - `replace_snapshot_setting_aside(expected, replacements) -> Result<SnapshotReplacement>`, with
    `SnapshotReplacement::{Exchanged, SetAside { displaced: PathBuf, sha256, bytes, reason: Mismatch |
    Undecodable }}`. It makes one exclusive lock hold, in this order:
    1. stage the replacement (write and fsync);
    2. read current;
    3. if current equals expected, rename the staged file over (rotating `.previous` first when enabled);
    4. if the read was an `io::Error`, remove the staging file, return the error and change nothing;
    5. otherwise hard-link current to the displaced name, fsync the directory, rename the staged file over, and
       fsync the directory again.

    Staging comes first, so a full disk sets nothing aside. The path is never absent: a link, never a rename. A
    set-aside never rotates `.previous`.
  - Opt-in previous generation: a constructor option (`retaining_previous_generation`, or Hands' spelling of it)
    plus `pull_previous_generation() -> Result<Vec<CultCacheEnvelope>>`, read under the shared lock and classified
    like any read. Off by default, because existing consumers must not grow files they did not ask for.
  - Filesystems without hard links: return an explicit error naming the filesystem. Do not fall back to a
    copy-then-rename, which would make the path briefly hold new content while the evidence is still being
    written. Windows NTFS links work; the Windows path is **not yet reached** by the Yggdrasil suite and is
    recorded as such.
  - C# reference: the same operation on `SingleFileBackingStore` (`CultCache.cs:3268ff`), with the same names
    across runtimes.
- **Verification (Rust and C#):**
  - a mismatch preserves the foreign bytes exactly and writes the replacement;
  - an undecodable current is preserved and replaced;
  - an I/O read failure (a directory at the path) changes nothing and leaves no staging file;
  - a staging failure (size cap) changes nothing and sets nothing aside;
  - the path is readable at every instant: a reader thread loops `pull_all_read_only_snapshot` through 1,000
    exchanges and never sees an absent file or an empty snapshot;
  - `.previous` rotates only on a verified exchange;
  - two set-asides in the same second get distinct names;
  - mutation tools: `cargo mutants --in-diff`, and Stryker.NET on the C# diff.
- **R3b (parity, not blocking Odin):** cultcache-ts `single-file-messagepack-backing-store.ts:34` and cultcache-py
  `stores.py:83` gain the same operation and option, with a shared fixture test.
- **Estimate:** Rust about +140, C# about +120, R3b about +200.

### O2. A foreign store file while serving is set aside, not fatal (Odin)

- **Repo/branch:** Odin `hands/odin-store-repair` from O1, with a CultLib pin bump to R3. Needs Q-R1 and Q-R4.
- **Deletes first:** `ForeignStoreWrite` (`lib.rs:630-641`) and its arm in `survive` (`main.rs:884`). The test
  `a_flush_writes_only_changes_and_never_over_a_foreign_file` (`lib.rs:2055-2090`) and the `survive` tests at
  `main.rs:2842-2844` and `:3110` pin the old rule: restate them, do not delete them.
- **Changes:**
  - `MemoryOdinTopologyStore::flush` (`lib.rs:613-626`) calls `replace_snapshot_setting_aside(flushed, current)`.
    On `SetAside` it records a `displaced` event and counts it toward the contested bound. On an I/O failure it
    records `unpersisted` and returns the error, which `survive` logs and retries as today.
  - `RuntimeState::flush` (`main.rs:307-314`) keeps its lease check immediately before the write.
  - The Q3 B per-put path uses this same flush. Hands re-anchors on the Q3 B head.
- **Adds:**
  - The `odin.store_condition` record (schema in `odin-core` `documents.rs`), held in W. It carries:
    - `generation` and `written_by`, updated inside the flush before the snapshot is taken;
    - `unpersisted_since` and `last_write_error`;
    - `contested_since`;
    - a bounded list of the last 16 recovery events: kind, time, displaced path, sha256 and bytes, source, records
      loaded, and the outstanding list (bounded).
  - One arm in `public_document` (`main.rs:590-608`) that serves exactly this type. Providers still cannot write
    it: `is_peer_document_type` is unchanged.
- **Rules that must die under their own mutation:**
  - a flush never leaves the store dirty (the condition record is written inside the flush, not after it);
  - a set-aside is preceded by a lease check;
  - the third set-aside inside the window stops writing, and the second does not;
  - after `contested`, no file is written or created;
  - on an I/O failure, nothing is set aside;
  - the condition is served while writes fail.
- **Authority map:** as in section 4. The deletion line is the `ForeignStoreWrite` type and arm.
- **Verification (fault injection, each at the file layer, not a log line):**
  - T-A: activate, serve, write a different valid snapshot behind Odin. After the next pass the file decodes to
    W, the displaced file's bytes equal the foreign bytes, `serving_pass` returned `Ok`, the catalog is unchanged,
    and the served condition names the displaced file and its sha256.
  - T-A2 (P2b): flip one payload byte behind Odin; the same outcome, reason `mismatch`.
  - T-B: truncate the file behind Odin; the same outcome, reason `undecodable`, and
    `pull_all_read_only_snapshot` (Idunn's crossing) decodes the file after one pass.
  - T-B-latency: with no puts, the set-aside happens within one heartbeat plus one flush interval.
  - T-contested: three foreign writes inside the window make exactly two set-asides and a third `contested`
    report. After that the file and directory are untouched by further passes, and Odin still serves.
  - T-D: replace the store path with a directory (EISDIR, which works as root). Nothing is set aside, no staging
    file remains, the condition is `unpersisted` and is served, and a put is not ACKed. Remove the directory, and
    the next pass writes W, clears `unpersisted` and ACKs.
  - T-lease: the lease withdrawn before a flush that would set aside means no link, no write, and Odin ends
    (`WriteLeaseLost`).
  - Negative: `rg -n "ForeignStoreWrite" crates` matches nothing; `rg -n "remove_file|remove_dir" crates/odin-daemon/src`
    matches nothing new outside tests.
  - `cargo mutants --in-diff`.
- **Estimate:** about -40 / +160.

### O3. Activation recovers a corrupt store (Odin)

- **Repo/branch:** Odin `hands/odin-store-activation` from O2. Needs Q-R2 and Q-R3.
- **Change:** `MemoryOdinTopologyStore::load` (`lib.rs:519-538`) becomes the recovery ladder:

  | `load` result | Outcome |
  |---|---|
  | `Ok` | as today |
  | absent | empty, as today |
  | `Unreadable::Undecodable` | try `pull_previous_generation()`, then empty (Q-R3); `flushed` becomes a sentinel meaning "the file is not a snapshot Odin can compare", so the first flush's exchange sets it aside |
  | `Unreadable::UnsupportedFormat` | return the error: refuse |
  | `io::Error` | return the error: refuse |

  Activation itself never writes. The first `serving_pass` flush (`main.rs:833-842`, due at once) writes W. That
  set-aside is recorded under the activation event and is not counted toward the contested bound.
- **Adds:** the outstanding-republish list. It holds the peer-type records present in the recovered previous
  generation, each cleared when its publisher re-puts it. After 10 minutes the remainder is marked "not
  republished", which is the live answer to "who is publish-once". For `started-empty` the list is `unknown`.
- **Rules that must die under their own mutation:**
  - Unsupported-format and I/O failures never set aside, never fall back, and never start.
  - A recovered activation writes before it serves its first catalog snapshot, or it serves the recovered set and
    marks the condition before the first write. Hands picks one and states it.
- **Verification:**
  - T-C1: a truncated `topology.cc` and a valid `.previous` activate with the previous content. The first pass
    writes it and sets the corrupt file aside byte-exact, and the condition is `recovered-previous` with its
    outstanding list. Re-putting one listed document clears it.
  - T-C2: both undecodable (Q-R3 A) start empty. Both files are preserved, the condition is `started-empty`, and
    the next correlation sequence is above the floor.
  - T-C3: a v4 header refuses to activate. The file is byte-identical afterwards, and there is no displaced file
    and no `.previous` change.
  - T-C4: a directory at the store path refuses, as C3.
  - T-restart: a recovered Odin stopped and restarted loads its own first write normally, with no second
    set-aside.
  - `cargo mutants --in-diff`.
- **Estimate:** about -10 / +110.

### R2. Single-file decoders reject trailing bytes, in every runtime (CultLib)

- **Repo/branch:** CultLib `hands/cultcache-trailing-bytes`. Independent of Odin; land it before O3 ships so that
  an appended file is caught as undecodable, not overwritten silently (P2).
- **Change:** C# `DeserializeSnapshot` (`CultDocumentMessagePackSerialization.cs:179-215`) checks `reader.End`.
  Rust `decode_store_snapshot` (`lib.rs:2583ff`) and the legacy path check that the slice is consumed. TS and
  Python do the same. **One release**: a runtime that accepts what another rejects is a parity split.
- **Verification:** one shared fixture (a valid store plus one byte) is rejected as undecodable by all four
  runtimes, and the valid store is accepted by all four.
- **Estimate:** about +40 plus tests across four runtimes.

### X1. Runbook and recovery drill (gamecult-ops, then live)

- **Repo:** gamecult-ops `runbooks/odin-yggdrasil.md` gains a "Store recovery" section:
  - how to read `odin.store_condition` from the catalog;
  - what displaced and previous files mean, and that disposing of them is the operator's decision;
  - the manual restore: engage Odin's lifecycle brake, stop the `idunn-odin-*` unit, set the live file aside with
    `ln` (never `rm`), extract the snapshot's `topology.cc` with its owner, mode and `.lock`, release the brake,
    and read the condition. The sequence floor makes an old watermark safe.
- **Drill D-1 (off-live, after O3; needs the Q2-style approval, Q-R6):** extract the newest Yggdrasil snapshot's
  `topology.cc` into a container on Yggdrasil. Activate a test-harness Odin on it through the existing fixture
  authority. Then truncate the copy and activate again with the extracted file as `.previous`. Report the records
  per type, the outstanding list and the sequence floor. Delete the copy afterwards.
- **Drill D-2 (live, operator-scheduled, Q-R6), after O3 deploys:**
  1. Record the baseline: catalog types, Idunn correlated targets, route timeouts.
  2. Engage the lifecycle brake and stop Odin.
  3. `ln topology.cc /var/tmp/odin-drill-keep.cc` (the evidence of the evidence), then truncate `topology.cc` in
     place to half.
  4. Release the brake and let Idunn restart Odin.
  5. Pass when all of these hold:
     - Odin is serving within one restart;
     - the condition is `recovered-previous`, with a displaced file whose sha256 equals the truncated file's;
     - every Odin-correlated target is Ready again within 60 s;
     - there are 0 route-challenge timeouts in the next hour;
     - the catalog's type set equals the baseline after 10 minutes, or the outstanding list names every
       difference.
  6. Then run D-2b, a foreign write while serving: `ln` the live file aside, then truncate it. It passes when the
     condition shows `displaced` within about 6 s and Idunn's correlation reads never fail for longer than that
     (the journal).

## 7. Subtraction ledger (estimate)

| Cut | Removed | Added | Formats and targets |
|---|---|---|---|
| R1 | about 30 | about 70 | none |
| O1 | about 5 | about 25 | none |
| R3 | 0 | about 260 (Rust 140, C# 120); R3b about 200 | two sibling-file conventions (`.displaced.*`, `.previous`); no format change |
| O2 | about 40 | about 160 | one Odin schema, `odin.store_condition.v1` |
| O3 | about 10 | about 110 | none |
| R2 | 0 | about 40 plus tests | none (it tightens the decoders) |

The campaign is net additive, at about +570 without R3b. It buys ends 1-5, which the operator asked for, and
removes one fatal path and one silent path. Every addition in CultLib is a capability a general consumer of the
single-file store would expect, and two of them (staging cleanup, and the header that survives truncation) are
defects.

## 8. Operator questions

- **Q-R1. A store file changed underneath a running Odin (decodable or not).**
  - Context: Idunn fences the old Odin before granting the lease, so the other writer is never an Odin. It is a
    tool, a hand edit, a restore made against the runbook, or disk corruption; P2b shows a flipped byte looks
    exactly like a foreign write. Today a decodable change ends Odin, and the restart adopts the foreign file
    silently. An undecodable change stops persistence forever and breaks Idunn's crossing.
  - A: set the file aside (kept byte for byte, never deleted), write Odin's working set over it, keep serving,
    and report it as typed state.
  - B: set it aside, write the working set, then end Odin so a fresh process starts from the rewritten file.
  - **Recommended: A.** B's restart protects nothing that the lease check before the write does not already
    protect, and costs a route outage.
- **Q-R2. Keep one previous generation on disk (`topology.cc.previous`)?**
  - Context: without it, a store that is corrupt at activation has no on-host source except "empty", and the
    backup is up to 24 h old and needs the operator. It costs one hard link per write (a directory update, no data
    written) and one more file of the store's size. It is a CultLib option, off by default for other consumers.
  - A: yes. B: no; activation recovery goes straight to empty.
  - **Recommended: A.** It is the only source that recovers publish-once documents, presence history and
    correlations without the operator. Its loss bound is one write.
- **Q-R3. Activation finds nothing decodable: no usable `topology.cc`, no usable previous generation.**
  - Context: presences and most catalog documents return within seconds to a minute, and correlations re-derive
    in one refresh pass. Publish-once documents (if any exist, see 1.3) stay missing until their publisher
    re-puts them, or until the operator restores from the backup.
  - A: start empty automatically, set the corrupt file aside, and report `started-empty`.
  - B: refuse to start, so Odin crash-loops until the operator restores. Every service's rendezvous is down
    meanwhile.
  - **Recommended: A.** A down Odin loses everything the Verse needs; an empty one loses only what nobody
    republishes, and the restore path stays open.
- **Q-R4. A foreign writer keeps writing (a third set-aside within 10 minutes).**
  - Context: repeated repair against a live writer makes a displaced file every second.
  - A: stop writing, keep serving from memory, withhold put ACKs, and report `contested` until the operator
    finds the writer. The working set survives.
  - B: end Odin. Idunn restarts it, and the restart loads whatever the foreign writer last wrote.
  - **Recommended: A.** B hands the store to the unknown writer. A is loud and loses nothing that is in memory.
- **Q-R5. Should a store condition change Odin's presence to `degraded`?**
  - Context: a non-`active` Odin presence makes every managed dependent of Odin not Ready (`lib.rs:946`,
    `dependency_evidence`). The store condition does not affect serving.
  - A: no. The presence stays `active`, and the condition record is the signal.
  - B: `degraded` while `unpersisted` or `contested`.
  - **Recommended: A.** Otherwise a full disk becomes an outage of every dependent, and a restart that Idunn might
    take in response would destroy the only complete copy.
- **Q-R6. The drills.**
  - D-1: decode a copy of last night's backup `topology.cc` in a container on Yggdrasil, the same terms as Q2 for
    the write map, with the copy deleted afterwards.
  - D-2: after O3 deploys, a scheduled live drill that truncates the real `topology.cc` behind the lifecycle brake,
    keeping a hard link of the original.
  - **Recommended: yes to both**, D-1 before O3 ships and D-2 in a window the operator names.

## 9. Follow-ups outside this map

- **I-F1 (Idunn):** a deploy or a stop of Odin while `odin.store_condition` says `unpersisted` or `contested`
  destroys the only complete copy. Idunn could read the condition from the catalog and hold the deployment
  (deployment brake, never continuity). This is Idunn's decision, raised with the Idunn map.
- **I-F2 (Idunn):** "Idunn stopped restarting admitted odin" is a log line (`report_once`, `control_plane.rs:4599`),
  not typed state. It belongs to Idunn's own operator surface.
- **F3 (from the write map):** the Eve lowering of `odin.store_condition` belongs to Odin's operator surface.
  This map publishes the typed record only.
- **H-F1 (Heimdall):** confirm whether the deployed Heimdall re-puts its Odin documents every 60 s. O3's
  outstanding list measures it after any recovery.
- **Q3 B's record in the write map:** "an ACK means durable" holds against crashes, not against a corruption at
  activation, which loses at most the last write (C1).
