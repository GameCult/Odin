# Why Odin cycles: the queued deployment kills the incumbent every time it seals

Written 2026-09-10 ~19:45Z from a live probe on yggdrasil. Supersedes the
mechanism section of `odin-deployment-handoff-2026-09-10.md`; that note's
timeline and "what not to do" still hold.

## The loop, as actually observed

One full cycle, wall clock, unit `8eeef9cb`:

| time | event | evidence |
|---|---|---|
| 19:34:59 | unit starts (continuity restart) | journal |
| 19:35:18 | write lease granted, projection holds Expected `fbbf6ace` + activation `8eeef9cb` + lease | projection snapshot `193601` |
| ~19:35:40 | continuity transaction completes; target free | probe: no running odin transaction |
| 19:37:33 | **projection Expected slot for `odin` replaced** by `incarnation-tx-4ae951cf`, sha `42dd1d45`, source rev `d6b15151`, candidate port 17973. Activation and lease untouched. | snapshot `193733`, byte-identical size to the healthy file |
| 19:37:36 | Odin's own catalog rejects Odin's own heartbeat: `runtime presence has no exact current activation and provider anchor` | journal |
| 19:37:38 | publisher ack timeout; daemon exits 1 | journal |
| 19:37:41 | Idunn demotes projection to Expected-only (activation and lease withdrawn) | snapshot `193741` |
| 19:37:47 | continuity Sealing; 19:37:52 next unit starts | probe |

The decode probe (`projection_authority_probe`, below) run on the two
snapshots gives:

- `193601` (healthy): authority present, no disagreements, lease binds.
- `193733` (kill): **`activation-expected-projection`** — expected
  `42dd1d45` (the candidate's Expected), observed `fbbf6ace` (the Expected the
  incumbent's activation binds). Authority collapses to none, and with no
  authority `select_presence` rejects any incoming presence.

The writer is the queued deployment `up-fc005d83`. It is **not starved**.
Every time a continuity restart completes and the target frees, it freezes,
seals (freeze source, materialize, install), and `advance_sealing` calls
`publish_expected`, which replaces the single `idunn.expected_incarnation[odin]`
record with the candidate's. The incumbent's next heartbeat validates itself
against that slot and fails. Idunn then sees the incumbent dead, aborts the
deployment ("admitted incumbent failed before candidate fencing; deployment
yielded to continuity", 5 such lines in status), restores Expected-only,
schedules continuity, and the command is queued again for the next cycle.
`tx-4ae951cf` exists only in `history.cc` (5 refs), which is why `idunn status`
shows nothing running at the moment of the kill.

Unit lifetime 2.5–16 min is the seal time (materialize is a Docker cargo
build), not a timeout.

This is R17 with the timeline attached. The handoff note's "freeze starvation"
defect is wrong as stated: freezing runs; it is the freeze that kills.

## Authority map

- **Which incarnation am I.** Owner: the immutable runtime bundle Idunn writes
  at launch (`/etc/gamecult/odin/runtime/<instance>/expected.cc` and
  `activation.cc`), loaded into `authority_material` in `main.rs`. Startup
  already treats it that way and deliberately does not compare the projected
  activation ("every launch is issued a fresh one").
- **Am I still admitted to write.** Owner: the write lease file
  (`process-write-lease.cc`), lifetime-locked by Odin, revoked by Idunn at
  Fencing. Already enforced by `require_current_write_lease`.
- **What the projection slot `expected_incarnation[odin]` means to Idunn.** The
  subject of the transaction currently touching the target: the candidate
  during a deployment, the admitted incarnation otherwise. Idunn's warming
  design *requires* the incumbent to stay up while the slot holds the candidate
  ("the healthy incumbent observes the candidate, which is the point").
- **Surviving wrong owner.** `OdinTopologyAuthority::reconcile` →
  `classify_runtime_authority` derives Odin's authority for its *own* presence
  from that slot. For the self target the slot is not identity; it is a
  forbidden writer of identity. This is the cut.
- **Derived / unchanged.** Idunn validates Odin's correlation against the
  *admitted* generation's Expected (`runtime_authority_parts` from
  `AdmittedGeneration`), not the slot, so a self-correlation built from the
  bundle Expected is what Idunn already expects to see.
- **Not owned by this cut.** Two Odin incarnations under one target key in
  Odin's store during an Odin deployment (incumbent + candidate). Today that
  path does not work at all: Odin deployments have only ever succeeded by
  bootstrap after the incumbent died. Keeping the incumbent alive turns that
  from "kills the incumbent" into "candidate warming fails cleanly". That is
  the R17 rebuild proper and is a separate decision.

## The fork to decide

**A. Odin-side (contained to this repo).** For `target == self`, admit and
correlate presence against `authority_material` (bundle Expected + activation,
provider anchor from the projection, which is stable) and use the projection
only for the lease binding. Deleting the slot's authority over self-identity
makes "incumbent dies when a candidate seals" structurally impossible. The
deployment then fails at Warming instead of killing the incumbent, and the
incumbent keeps serving. Candidate observation for Odin-on-Odin deployments
remains unbuilt, as it is today.

**B. Idunn-side.** Publish the candidate Expected under an incarnation-scoped
record and keep the admitted Expected in the slot until Commit. Fixes the same
kill, keeps Odin's classifier as-is, but changes the projection contract that
every target and Odin's `current_projection` read, and Odin still cannot
observe a candidate Odin without the two-incarnation store.

Recommendation: A first. It is the smaller cut, it lives where the wrong owner
lives, and B is still needed for the deployment story either way.

## Probe

`crates/odin-daemon/src/lib.rs` carries `projection_authority_probe`, an
ignored test that decodes a captured projection file and prints what the
classifier sees. Usage is in its doc comment. Capture on the host with a
copy-on-change loop over `/var/lib/gamecult/idunn-projection/topology.cc`;
the version that kills is the one written seconds before the rejection line.
