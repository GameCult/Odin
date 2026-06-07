# Idunn

Idunn is Odin's keepalive organ.

Odin is the all-seer: it accepts Verse discovery, schema catalogs, translation
routes, provider surfaces, and observation projections. Idunn keeps the daemon
swarm alive from that accepted map. It is not a second Odin, not a dashboard,
and not a heroic supervisor with secret service truth in its pockets.

Idunn keeps the apples: desired daemon presence, boot recovery, crash recovery,
health freshness, restart intent, operator escalation, and continuity witness
state.

## Authority Map

- Owner: Idunn owns daemon lifecycle management after Odin has accepted where a
  daemon lives: bring-up after host reboot, crash recovery, health watching,
  restart policy, and operator escalation.
- Inputs: Odin's accepted service records, provider advertisements, `.cc`
  witnesses, advertised command boundaries, freshness windows, operator policy,
  local service manager state, and direct local service probes only when no
  provider advertisement exists.
- Outputs: typed keepalive observations, restart requests, denied-action
  records, operator alarms, Bifrost operator-notification requests, and an
  Eve/CultUI keepalive surface.
- Derived state: dashboard cells, Bifrost receipts, Discord or owner-DM
  lowerings, agent summaries, and Odin service projections are
  notification-only views of Idunn-owned keepalive records.
- Forbidden writers: Idunn does not decide which Verses exist, invent provider
  schemas, mutate provider dashboards, own identity/session grants, or hide
  restarts behind Odin refresh logic. Individual daemons should not carry
  independent crash-recovery loops once Idunn owns their lifecycle path; they
  publish health, surfaces, state witnesses, and command boundaries instead.
- Shared paths: manual operator restart, scheduled restart, degraded-health
  repair, boot rehydration, and future remote worker recovery must pass through
  the same keepalive command primitive.
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
  VoidBot through `health-voidbot.cmd` and `restart-voidbot.cmd`.
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
- Providers own their own command boundaries. Idunn requests restart through
  advertised authority or a named local service manager adapter.
- Host reboot recovery, crash recovery, stale-health recovery, and manual
  operator restart must share the same Idunn command primitive.
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

## First Runtime Direction

Idunn's Rust runtime grows in this order:

1. Probe one named daemon with an explicit health command, once or on a
   resident interval.
2. Normalize that into `idunn.desired_daemon.v1` and
   `idunn.daemon_health.v1`.
3. Emit `idunn.keepalive_decision.v1`.
4. When restart authority exists, emit `idunn.restart_request.v1`.
5. When `--execute` is present, execute the restart command and emit
   `idunn.restart_result.v1`.
6. When authority is missing, emit `idunn.operator_alarm.v1` targeting
   Bifrost operator notification.
7. Next: ingest Odin-owned service/provider advertisements directly, add named
   adapters for Windows services, systemd, Docker, and provider-advertised
   CultMesh commands, then publish an Eve/CultUI keepalive surface.

No ad hoc JSON manifest may become the live state owner. Debug projections are
fine when they name the `.cc` record or CultMesh document behind them.
