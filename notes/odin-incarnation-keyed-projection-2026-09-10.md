# The projection is keyed by incarnation now

Written 2026-09-10 ~20:30Z, before the host carried the change. Read
`odin-cycle-mechanism-2026-09-10.md` first for why.

## What changed

Odin `faa53f3` and Idunn `903e1c8`, both on `main`.

- Every Expected, activation, and write-lease record in Idunn's projection,
  and every presence and correlation record in Odin's store, is keyed by
  `{target}@{expected projection sha256}`. The runtime presence trust anchor
  stays keyed by target. The contract is stated in Idunn
  `docs/deployment-authority.md` under "Projection keying" and implemented by
  `IncarnationRef` in `odin-daemon` and `incarnation_key` in Idunn's
  `drivers.rs`. The two strings must agree; lifting the helper into CultLib
  `cultnet-rs` is the open follow-up.
- Odin looks up its own incarnation by its bundle's Expected digest, admits a
  presence under the incarnation the presence names, keeps one correlation per
  incarnation, and resolves dependencies by (provider, expected sha). Records
  keyed by the bare target project nothing.
- Idunn publishes the candidate Expected as Starting's first step, after the
  deployment brake admitted the transaction. A pre-fencing abort withdraws only
  the candidate; a post-fencing abort also demotes the stopped incumbent.
  `restore_admitted_expected_only` and the read-as-absence compensator in
  `admit_latest_topology` are gone.
- A consumed command is archived with its last transaction. This is what ended
  "the queued deployment fires again every cycle": it was never starved, it was
  re-frozen because the command outlived its retired transaction.
- An engaged lifecycle brake stops continuity being minted instead of parking a
  transaction that owns the target. `idunn cancel <command-id>` exists.

## Verification

- Odin: `cargo test -p odin-core -p odin-daemon` in `rust:1.95-bookworm` on
  yggdrasil, 22 + 12 (+1 ignored) + 4 passed. The negative check is
  `a_candidate_expected_beside_the_incumbent_cannot_reject_its_presence`.
- Idunn: `cargo test --lib` in the same image, 114 passed, 2 ignored. The
  driver tests prove withdrawal and demotion are scoped to one key and that a
  legacy target-keyed slot is retired on publish.
- Host: see the dated entries below.

## Deployment order and why

1. Odin `main` first. Under the old Idunn a candidate built from it cannot
   start (it finds no incarnation-keyed Expected) and the old loop continues
   unchanged; nothing is worse.
2. Idunn binary next: `cargo build --release` in the same image, install over
   `/usr/local/bin/idunn`, restart `idunn-yggdrasil.service`. The previous
   binary is kept as `/usr/local/bin/idunn.prev` (sha256 `4f4941bb…`).
3. On its first supervise tick the new Idunn finds the admitted Expected not
   exact under the new key, republishes it and retires the target-keyed slot.
   The running old Odin then finds no Expected it reads and exits; continuity
   restarts the old release, which cannot start for the same reason; after
   three attempts the target is free. The queued `up-fc005d83` freezes once,
   builds `main`, and the new Odin bootstraps as first Odin because no
   admitted Odin can observe it. That command is then consumed.

## Host log

(dated entries appended as the deployment proceeds)

- **20:23:43Z** Idunn `903e1c8` installed over `4f4941bb…`, restarted, active
  with no restarts. It resumed the in-flight `continuity-ac542622` in
  Committing. That phase asks Odin for a correlation under the new key; the
  old Odin writes under the old one; the phase returned "no evidence yet" every
  tick and, owning the target, blocked everything else. Nothing died and
  nothing moved.
- **20:24:46Z** Stopped the old Odin unit by hand (systemd needed a second
  SIGKILL; the process was not its child). Idunn still did not move:
  Committing, Routing and AwaitingReady wait on `admit_latest_topology` before
  they observe the workload, so a candidate that dies while its evidence is
  pending is never observed and never aborted. Fixed in Idunn by observing the
  candidate first in those three phases (`observe_candidate_before_waiting`).
  Second Idunn build in progress.
- **20:28:58Z** Idunn `1a0a0e7` installed and restarted. The parked
  transaction aborted at 20:29:09, the projection was rewritten under
  incarnation keys at 20:29:10 (decoded: one incarnation
  `odin@sha256-fbbf6ace…`, Expected-only, legacy slot gone), and continuity
  began restarting the old release, which fails at start with "Idunn
  projection has runtime authority without Expected" as predicted.
- **20:30:16Z** Continuity did not give up after three attempts. Failed
  restarts retire to `history.cc` as soon as they finish and the give-up
  counter read only the live set, so it counted zero every time. That also
  means the handoff note's "continuity gives up after 3" was never true since
  the history retirement landed. Fixed in Idunn by counting history too. Third
  Idunn build in progress; the old release keeps restarting harmlessly until
  it lands.
- **20:32:45Z** Idunn `6b68d65` installed. Continuity gave up ("failed to
  start 12 times, the target is free"). At 20:33:39 freeze created
  `tx-0f15abe4` for `up-78945539`, an old odin command from earlier today:
  every resident command whose transactions had been retired to history read
  as queued again, and freeze takes the oldest first. That one command has
  hundreds of retired attempts behind it. It sealed Odin `main` `faa53f3` as
  release `sha256-809979ad…` and is waiting on the deployment brake, which is
  the operator gate working: Expected is now published only after the brake,
  so nothing was killed. `up-fc005d83` was redundant and was cancelled with
  the new verb. Fixed in Idunn: a command is consumed by any transaction, live
  or retired; the resident backlog is retired on sight; a busy target no longer
  blocks the commands behind it; `status` reports a live transaction as
  running instead of an older attempt's failure.
- **Pending** an operator release of the odin deployment brake naming release
  `sha256-809979ad…` and deployment `tx-0f15abe4-3d00-4c04-bf11-ebc27aa97a6c`,
  signed with `/etc/gamecult/idunn/deployment-brake-operator-identity.cc`.
  Odin is down until then; nothing else is broken.
