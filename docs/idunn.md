# Idunn

Idunn is Odin's keepalive organ.

Odin is the all-seer: it accepts Verse discovery, schema catalogs, translation
routes, provider surfaces, and observation projections. Idunn keeps the daemon
swarm alive from that accepted map. It is not a second Odin, not a dashboard,
and not a heroic supervisor with secret service truth in its pockets.

Idunn keeps the apples: desired daemon presence, deployment freshness, boot
recovery, crash recovery, health freshness, deploy/restart intent, operator
escalation, and continuity witness state.

## Authority Map

- Owner: Idunn owns daemon lifecycle management after Odin has accepted where a
  daemon lives: bring-up after host reboot, deployment freshness, crash
  recovery, health watching, deploy/restart policy, and operator escalation.
- Inputs: Odin's accepted service records, provider advertisements, `.cc`
  witnesses, advertised command boundaries, CultNet/RUDP health contracts,
  freshness windows, operator policy, local service manager state, and demoted
  compatibility probes only while daemon CultLib dependencies still lack the
  shared RUDP health publication path.
- Outputs: typed keepalive observations, deployment requests/results, restart
  requests/results, denied-action records, operator alarms, Bifrost
  operator-notification requests, and an Eve/CultUI keepalive surface.
- Derived state: dashboard cells, Bifrost receipts, Discord or owner-DM
  lowerings, agent summaries, and Odin service projections are
  notification-only views of Idunn-owned keepalive records.
- Forbidden writers: Idunn does not decide which Verses exist, invent provider
  schemas, mutate provider dashboards, own identity/session grants, or hide
  restarts behind Odin refresh logic. Individual daemons should not carry
  independent crash-recovery loops once Idunn owns their lifecycle path; they
  publish health, surfaces, state witnesses, and command boundaries instead.
- Shared paths: manual operator deploy/restart, scheduled deploy/restart,
  degraded-health repair, boot rehydration, and future remote worker recovery
  must pass through the same Idunn command primitive.
- Deletion line: any keepalive loop inside Odin, Gjallar, Eve lowerers, or
  renderer code must be cut or demoted to a probe that names Idunn as the
  restart owner.

## Body

Idunn now shares Odin's Rust body:

- `crates/odin-core/src/idunn.rs` owns the keepalive decision engine.
- `crates/odin-core/src/documents.rs` publishes typed Idunn CultMesh records
  beside Odin and Gjallar records.
- `crates/idunn-daemon` is the local keepalive actuator crate and now owns the
  resident Starfire-local swarm scheduler.
- `src/Idunn/README.md` is the user-facing introduction for developers,
  operators, and daemon authors.
- `scripts/start-idunn-local.ps1` is now a narrow bootstrap wrapper: it ensures
  one `idunn.exe` process is alive, checks the shared keepalive store for
  staleness, and lets Rust own the target catalog and per-target scheduling.
- `scripts/deploy-yggdrasil-source-app.ps1` and
  `scripts/health-yggdrasil-source-app.ps1` are the generic Yggdrasil source
  artifact lane. They package committed local `HEAD` with `git archive`, run
  the existing ops-owned deploy/check scripts on Yggdrasil, and stamp a remote
  `gamecult.idunn.deployment_manifest.v1` only after the remote check passes.
- `scripts/idunn-deployment-targets.ps1` is the current swarm deployment target
  catalog. Every known deployable target is either `enforced`, `blocked`,
  `external-owned`, or `not-runtime` with an explicit reason.
- `scripts/health-idunn-swarm-deployment-coverage.ps1` is the coverage probe
  that fails when the target catalog becomes incoherent. The local Idunn
  launcher runs it as `idunn-swarm-deployment-coverage` so missing deploy
  ownership becomes a watched operational fault.
- `scripts/notify-idunn-operator-alarm.ps1` is the local operator crossing:
  Idunn invokes it only after raising an operator alarm, and it asks Bifrost to
  publish a typed `gamecult.operator_dm_request.v1` CultMesh command document
  instead of learning Discord delivery itself.
- `npm run idunn:build` builds the Rust daemon.
- `npm run idunn:start -- ...` still supports the narrow one-daemon probe path
  for manual use.
- `npm run idunn:start -- --swarm-profile starfire-local --repo-root E:\Projects\Odin --execute`
  runs the singular local swarm supervisor.

The current typed records are:

```text
idunn.desired_daemon.v1
idunn.daemon_health.v1
idunn.keepalive_decision.v1
idunn.deployment_request.v1
idunn.deployment_result.v1
idunn.restart_request.v1
idunn.restart_result.v1
idunn.operator_alarm.v1
idunn.daemon_surgery_plan.v1
idunn.daemon_transport_profile.v1
idunn.command_boundary.v1
```

## Invariants

- Odin remains the accepted owner of Verse and service discovery.
- Idunn owns continuity decisions after a daemon is known.
- Individual daemons own their work and their health publication, not their
  surrounding lifecycle. They must be simple to kill and simple for Idunn to
  bring back.
- Providers own their own command boundaries. Idunn requests deployment or
  restart through advertised authority or a named local service manager adapter.
- Host reboot recovery, crash recovery, stale deployment recovery,
  stale-health recovery, and manual operator deploy/restart must share the same
  Idunn command primitive.
- A repair loop is not an owner. If a daemon becomes healthy only after a later
  Odin refresh or manual click, Idunn's ownership path is still incomplete.
- Restart attempts must be witnessed: requested by whom, against which service,
  through which command boundary, with what result and timestamp.
- Health command exit status is not daemon awareness by itself. Every Idunn
  target must declare a daemon-owned CultNet/RUDP health contract naming what
  health publication should prove and what unmarked failure means.
  `idunn.desired_daemon.v1` and
  `idunn.daemon_health.v1` both record that contract so later readers can
  distinguish process liveness, source deployment freshness, framebuffer
  composition, telemetry capture, and catalog coherence without mistaking a
  temporary HTTP/WebSocket/SSH/systemd probe for the real protocol surface.
- `idunn.desired_daemon.v1` links to
  `idunn.daemon_transport_profile.v1` and `idunn.command_boundary.v1` records.
  The transport profile names the target transport
  `cultnet.transport.rudp.v0`, the still-active compatibility mechanism, and
  the cut line that demotes old probes. The command boundary names restart,
  deploy, health, and alarm authority separately so Idunn can actuate only the
  commands it actually owns.
- The Starfire-local shell probes are compatibility evidence, not the target
  architecture. A daemon is fully Idunn-aware only when it publishes its health,
  command boundary, and transport profile as typed CultNet/CultMesh documents
  over `cultnet.transport.rudp.v0`. TCP, HTTP, WebSocket, and ad hoc port probes
  are migration debt, tolerated only at xenos/legacy boundaries and while the
  daemon's CultLib dependency has not yet been updated.
- Rust now has a narrow `cultnet.transport.rudp.v0` substrate in
  `vendor/cultnet-rs`: one CultNet message per acknowledged UDP datagram with
  timeout/retry semantics. This removes "Rust cannot speak RUDP" as a substrate
  excuse. It does not make any daemon fully migrated until that daemon publishes
  its health and command boundary through the RUDP path and Idunn consumes that
  daemon-owned publication instead of the compatibility command.
- Idunn publishes the transport migration plan as
  `idunn.daemon_surgery_plan.v1` records in the keepalive store. Each daemon
  plan names severity, status, owner, objective, current mechanism, intended
  CultNet/RUDP authority, cut line, ordered steps, blockers, and update time.
  These records are operational state, not documentation garnish: Nightwing,
  Gjallar, Odin, and future Eve lowerings should inspect them when projecting
  the daemon surgery queue.
- A stale deployment is not restartable liveness. If a target reports
  `stale-deployment` without deploy authority, Idunn must alarm instead of
  restarting the stale artifact. If a target reports `dependency-unavailable`
  or `degraded`, Idunn must alarm instead of treating local deploy/restart as
  the owner.
- Idunn should fail closed when authority is unclear. Unknown ownership,
  repeated restart failure, missing command authority, or degraded health that
  needs a human becomes an operator alarm.
- Operator alarms use CultMesh to request Bifrost-owned operator notification.
  Bifrost is the bridge for the crossing and records the receipt. VoidBot's
  `voidbot.operator-dm` command `owner.dm.send` with payload schema
  `gamecult.operator_dm_request.v1` is a demoted compatibility delivery
  actuator, not the owner. The owner-DM command lives in Bifrost's Verse; any
  still-VoidBot delivery path must be invoked by Bifrost or documented as
  migration debt. Idunn must not learn Discord token handling, DM delivery
  internals, or VoidBot-specific transport.
- In the current local bridge, `--operator-alarm-command` invokes
  `notify-idunn-operator-alarm.cmd`, which forwards `IDUNN_ALARM_*`
  environment variables to `E:\Projects\Bifrost\tools\operator-notification.mjs
  publish-idunn-alarm`. Bifrost publishes the typed CultMesh command document;
  Idunn only decides that an alarm exists.
- Muninn continuity on Raven is a remote keepalive path. Idunn probes the
  deployed Odin `muninn.exe --health` through the `raven` SSH alias and
  restarts Muninn's idle `serve` posture when needed. Raven is the host/body;
  Muninn is the local telemetry Verse assembler. Idunn must not activate
  screen/audio streams as part of keepalive. Raven A/V over SRT is an explicit
  activation path through `activate-muninn-raven-av-srt.ps1`, not daemon
  startup behavior.
- Nightwing Gjallar deployment freshness and visible composition health are now
  part of Idunn's ops role. `health-nightwing-gjallar.ps1` verifies
  `gjallar.service` liveness, the remote deployment manifest at
  `/opt/gamecult/gjallar/gamecult-gjallar-deploy-manifest.txt`. A missing or
  stale manifest emits `idunn.health.state=stale-deployment`; Idunn then runs
  `deploy-nightwing-gjallar.ps1`, which publishes the committed local Gjallar
  revision, writes `gamecult.gjallar.deployment_manifest.v1`, restarts
  `gjallar.service`, and leaves deployment request/result records in its
  keepalive `.cc`. The same health probe also reads `/var/log/gjallar.status`;
  an empty catalog, failed receive loop, stale status witness, or zero composed
  provider panels is unhealthy even when the process is alive. Upstream deck
  failure emits `idunn.health.state=dependency-unavailable` so Idunn does not
  redeploy Gjallar for an Odin/provider input outage.
- Swarm-wide deployment ownership means Idunn owns the target catalog and the
  freshness contract for the repo swarm. It does not mean Idunn invents deploy
  authority for every repo immediately. A target without a safe noninteractive
  deploy command must remain `blocked` or `external-owned` with the missing
  authority named until a wrapper can produce a deployment manifest and route
  through Idunn's typed deployment request/result path.
- Yggdrasil Heimdall, repixelizer, and StreamPixels are enforced through the
  generic source artifact lane and their existing `gamecult-ops` runbooks.
  Bifrost is explicitly blocked as of 2026-06-09: committed Bifrost `HEAD`
  expects `UserAccounts.HeimdallAccountId`, while Yggdrasil's production
  database lacks that column and EF reports no pending migration. Idunn must
  not claim Bifrost deployment freshness until Bifrost owns that schema
  migration path.

## Runtime Direction

Idunn's current Rust runtime now has two postures:

1. One-daemon manual mode for focused probes and explicit local testing.
2. One-process swarm mode for the Starfire-local continuity graph.

The local swarm mode owns:

1. the built-in Starfire-local target catalog;
2. a mandatory health contract per target;
3. a typed `idunn.daemon_surgery_plan.v1` record per target, so transport debt
   is visible in the same state surface as daemon health;
4. a typed `idunn.daemon_transport_profile.v1` and
   `idunn.command_boundary.v1` record per target, so compatibility probes and
   local commands cannot pretend to be daemon-owned CultNet/RUDP truth;
5. one in-process schedule per target instead of one watchdog process per target;
6. shared typed keepalive records in one CultMesh store;
7. deploy/restart/alarm execution through the same Rust decision engine;
8. recovery of fast local targets like Odin without waiting behind slow remote
   Yggdrasil checks.

Next: update daemon CultLib dependencies to the cross-runtime
`cultnet.transport.rudp.v0` surface, ingest Odin-owned service/provider
advertisements directly, promote the target catalog out of hardcoded bootstrap
data, add named adapters only for legacy service-manager crossings, then
publish an Eve/CultUI keepalive surface.

No ad hoc JSON manifest, HTTP endpoint, TCP socket, or WebSocket bridge may
become the live state owner. Debug projections are fine when they name the
`.cc` record, CultNet document, or CultMesh publication behind them.
