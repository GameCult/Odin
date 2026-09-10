# Odin cannot currently be redeployed, and it cycles

> **Correction 2026-09-10 19:23Z.** Per-unit lifetime is 2.5-16 minutes, not
> ~96 s; 96 s is the runtime-presence publisher timeout that precedes the exit.
> Cycle rate holds at ~10 starts/hour. Count starts with `sudo journalctl -q`;
> an unprivileged `journalctl` sees only your own messages and returns zero.
> `up-fc005d83` still queued; lifecycle brake still released (19:03:12Z).
>
> **Correction 2026-09-10 19:45Z.** The "starved by design" section below is
> wrong: the queued deployment freezes every time the target frees. Its Sealing
> phase replaces the projected Expected with the candidate's, the incumbent
> fails `activation-expected-projection` on its next heartbeat and exits, and
> the deployment is aborted for having killed its incumbent. Full timeline and
> authority map in `odin-cycle-mechanism-2026-09-10.md`.
>
> **Correction 2026-09-10 20:35Z.** Superseded by
> `odin-incarnation-keyed-projection-2026-09-10.md`. "What to try, in order" is
> done or wrong: (1) the starvation diagnosis was wrong (see 19:45Z above);
> (2)-(3) are fixed in Idunn `903e1c8` (Expected published after the brake,
> lifecycle brake stops continuity minting instead of parking a transaction,
> commands consumed once). `idunn cancel <command-id>` now exists; "no cancel
> verb (R16)" is false. "Continuity gives up after 3" was false from Idunn
> `222bfcc` (failed restarts retire to history.cc before the counter reads
> them) until `6b68d65`, which counts history too. R16 and R17 are cited here
> as recorded but no active ledger in Odin, Idunn, or gamecult-ops defines
> them; treat the numbers as unresolved references.

Written 2026-09-10 after an incident caused by an optional dependency bump.
Read this before running `idunn up odin`.

## State on yggdrasil as of writing

| | |
|---|---|
| Odin | **up**, cycling: ~9 unit starts/hour, self-terminates every ~96s under load |
| running release | `sha256-61c32655...` (from `up-07cb1979`, admitted 2026-09-09 22:17) |
| Idunn | `active`, 0 restarts, healthy throughout |
| queued deployment | `up-fc005d83-79d4-418c-bc76-80d07dd6eff3` — **stuck `queued`, cannot dequeue** |
| lifecycle brake | released |
| `control.cc` | 6.5 MB · `history.cc` 2.5 MB (archival working) |

Odin is *serving* between restarts. This is degradation, not an outage.

## The loop, precisely

```
Odin rejected CultMesh RUDP application message DocumentPutRaw
  odin-presence:odin-yggdrasil:sha256-<unit>:...
  : runtime presence has no exact current activation and provider anchor
Error: timed out waiting for reliable acknowledgement from CultMesh RUDP catalog 127.0.0.1:17972
```

1. Odin's runtime-presence publisher publishes to **Odin's own catalog**.
2. The catalog rejects it: the projection at
   `/var/lib/gamecult/idunn-projection/topology.cc` holds no activation matching
   the unit this process was launched with.
3. No acknowledgement ever arrives. The publisher times out after ~96 s and the
   daemon **exits 1 on its own** — systemd is not stopping it.
4. Continuity restarts it under a fresh unit, which **mints a new activation**,
   and the projection is behind again.

**Continuity restarting Odin is what keeps the activation ahead of the
projection.** The restart is not the recovery; it is the thing that guarantees
the next failure.

## Why the two obvious moves do not work

### Braking continuity blocks deployment

Engaging the lifecycle brake *does* stop the loop — Odin converged clean, four
minutes up, zero rejections, which is what confirmed the diagnosis. But a
continuity transaction that opened **before** the brake then parks forever:

```
waiting on lifecycle brake denies continuity restart
```

It owns the target, so no deployment can freeze while the brake is engaged.
`F:\Projects\CLAUDE.md` says a lifecycle brake must not gate deployment
authority. **It does.** That separation is not holding, and it is defect 1.

### The queued deployment is starved by design

`run_scheduler_tick` (`Idunn/src/control_plane.rs`) runs
`freeze_one_queued_command` **only** when neither a resumed transaction nor
continuity made progress that tick. The ordering is deliberate — "unfinished
ownership work first, admitted-body continuity second, new commands last" — but
on an unstable target continuity always has work, so a queued command never
runs. Observed across four separate windows where nothing owned the target.

That is defect 2, and it is the shape this estate keeps producing: **the one
command that could repair the instability cannot run because the target is
unstable.**

## Underlying cause, older than this incident

Redeploying Odin destroys the observer Idunn needs in order to admit the
replacement. Idunn publishes the candidate's Expected projection into the store
the incumbent reads; the incumbent can no longer validate its own presence and
dies. This is recorded as R17 and it is why an Odin deployment is never routine.

## What to try, in order

1. **Settle Odin first, deploy second.** The activation/projection race is the
   root; everything else is consequence. A candidate whose projection matches at
   launch is what ended this loop at 22:17 on 09-09.
2. **Fix the freeze starvation** so a queued deployment can take a target that
   continuity is fighting over — otherwise the repair can never be applied by
   the machine that needs it.
3. **Fix the brake separation** so an engaged lifecycle brake suspends
   continuity actuation without parking a transaction on the target.
4. Only then re-run the deployment. The pin bump it carries (`0e540ca`,
   `f7e3fc3`) is committed on `main` and undeployed; nothing is lost by waiting.

There is **no cancel verb** (recorded as R16), so `up-fc005d83` will fire
whenever the target frees. Treat it as armed.

## What not to do

- Do not `systemctl start` a failed Idunn unit by hand. It mints a new
  `InvocationID`, and Idunn then refuses to stop a unit whose invocation it
  cannot match — this blocked a recovery for 20 minutes on 09-09 and needed
  `systemctl reset-failed` to undo.
- Do not read a gap between restarts as convergence. Odin holds for ~90 s
  between cycles; that window was misread as "stable" twice in one night.
  Measure `Started idunn-odin` counts over an hour instead.
- Do not trust `head` on a unit listing. See `AGENTS.md`.

## What is verified working

- History archival: terminal transactions leave `control.cc` for `history.cc`
  on completion. `history.cc` exists and is 2.5 MB; `status` merges both.
- Continuity gives up after `CONTINUITY_RESTART_ATTEMPTS = 3` against one
  generation, which is what frees a target after repeated failure.
- Idunn survives all of this: a faulting scheduler tick is logged, not fatal,
  and startup no longer depends on the deployment brake anchor.
