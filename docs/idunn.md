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
  witnesses, advertised command boundaries, freshness windows, operator policy,
  local service manager state, and direct local service probes only when no
  provider advertisement exists.
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
- `crates/idunn-daemon` is the local keepalive actuator crate.
- `src/Idunn/README.md` is the user-facing introduction for developers,
  operators, and daemon authors.
- `scripts/start-idunn-local.ps1` starts resident local watchdogs, including
  VoidBot through `health-voidbot.cmd` and `restart-voidbot.cmd`, Muninn
  through `health-muninn.cmd` and `restart-muninn.cmd`, and Nightwing Gjallar
  through `health-nightwing-gjallar.cmd`,
  `deploy-nightwing-gjallar.cmd`, and `restart-nightwing-gjallar.cmd`.
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
- `npm run idunn:start -- ...` probes a daemon, records the decision, and can
  execute the restart command when `--execute` is present.
- `--interval-seconds <seconds>` keeps the same probe and decision path running
  as a resident keepalive loop.

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
- Nightwing Gjallar deployment freshness is now part of Idunn's ops role.
  `health-nightwing-gjallar.ps1` verifies both `gjallar.service` liveness and
  the remote deployment manifest at
  `/opt/gamecult/gjallar/gamecult-gjallar-deploy-manifest.txt`. A missing or
  stale manifest makes health fail. Idunn then runs
  `deploy-nightwing-gjallar.ps1`, which publishes the committed local Gjallar
  revision, writes `gamecult.gjallar.deployment_manifest.v1`, restarts
  `gjallar.service`, and leaves deployment request/result records in its
  keepalive `.cc`.
- Swarm-wide deployment ownership means Idunn owns the target catalog and the
  freshness contract for the repo swarm. It does not mean Idunn invents deploy
  authority for every repo immediately. A target without a safe noninteractive
  deploy command must remain `blocked` or `external-owned` with the missing
  authority named until a wrapper can produce a deployment manifest and route
  through Idunn's typed deployment request/result path.

## First Runtime Direction

Idunn's Rust runtime grows in this order:

1. Probe one named daemon with an explicit health command, once or on a
   resident interval.
2. Normalize that into `idunn.desired_daemon.v1` and
   `idunn.daemon_health.v1`.
3. Emit `idunn.keepalive_decision.v1`.
4. When deploy authority exists, emit `idunn.deployment_request.v1`.
5. When `--execute` is present, execute the deploy command and emit
   `idunn.deployment_result.v1`.
6. When restart authority exists, emit `idunn.restart_request.v1`.
7. When `--execute` is present, execute the restart command and emit
   `idunn.restart_result.v1`.
8. When authority is missing, emit `idunn.operator_alarm.v1` targeting
   Bifrost operator notification.
9. Next: ingest Odin-owned service/provider advertisements directly, add named
   adapters for Windows services, systemd, Docker, and provider-advertised
   CultMesh commands, then publish an Eve/CultUI keepalive surface.

No ad hoc JSON manifest may become the live state owner. Debug projections are
fine when they name the `.cc` record or CultMesh document behind them.
