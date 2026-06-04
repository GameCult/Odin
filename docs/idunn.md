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
  renderer code should be cut or demoted to a probe that names Idunn as the
  restart owner.

## Body

The initial local package is deliberately small:

- `src/Idunn/Idunn.csproj` is the first C# CultMesh organ.
- `src/Idunn/Program.cs` opens or creates Idunn's local CultCache/CultMesh
  store at `scratch/idunn/idunn.keepalive.cc`.
- `npm run idunn:build` builds the organ beside `gjallar:build`.
- `npm run idunn:start -- --serve` starts the resident keepalive process.

This is the organ contract and runtime foothold. It does not yet restart
anything. The first real keepalive cut should add typed records before adding
actuators:

```text
idunn.desired_daemon.v1
idunn.daemon_health.v1
idunn.keepalive_decision.v1
idunn.restart_request.v1
idunn.restart_result.v1
idunn.operator_alarm.v1
idunn.operator_escalation.v1
```

## Invariants

- Odin remains the accepted owner of Verse and service discovery.
- Idunn owns continuity decisions after a daemon is known.
- Individual daemons own their work and their health publication, not their
  surrounding lifecycle. They should be simple to kill and simple for Idunn to
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
  `gamecult.operator_dm_request.v1` is a compatibility delivery actuator until
  the owner-DM command lives natively in Bifrost's Verse. Idunn must not learn
  Discord token handling, DM delivery internals, or VoidBot-specific transport.

## First Runtime Direction

Idunn's C# runtime should grow in this order:

1. Read Odin-owned service and provider-advertisement records through CultMesh.
2. Normalize them into `idunn.desired_daemon.v1` records.
3. Observe freshness and process state through narrow probe ports.
4. Emit `idunn.keepalive_decision.v1` records without acting.
5. Add restart adapters only for named authority boundaries: local systemd,
   Windows service control, Docker, or provider-advertised commands.
6. Add Bifrost operator escalation for cases that need human action, with
   VoidBot owner-DM delivery only as a demoted compatibility target.
7. Publish an Eve/CultUI keepalive surface from Idunn-owned records.

No ad hoc JSON manifest should become the live state owner. Debug projections
are fine when they name the `.cc` record or CultMesh document behind them.
