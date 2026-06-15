# Idunn

Idunn is the keepalive daemon for the Odin swarm.

In plain language: Idunn is the little service whose job is to know which
GameCult daemons are supposed to be alive, bring them back after a reboot or
crash, watch whether they are still healthy, and raise a clear alarm when a
human needs to intervene.

It is named for Idunn, the keeper of the apples that keep the gods young. The
job is not glamorous. That is the point. Good infrastructure should make
important things boring.

## What Idunn Is For

GameCult has many daemons: Odin, Bifrost, VoidBot, Mimir, Heimdall, Eve
providers, service workers, renderers, bridges, and local machine helpers. Each
one owns its own work, but each one should not have to carry its own private
lifecycle machinery.

Idunn centralizes that lifecycle work:

- start known daemons after machine boot;
- ensure deployed daemon artifacts match the desired target revision;
- restart daemons after crashes;
- watch health and freshness signals;
- avoid restarting services when authority is unclear;
- record every deployment/restart request and result as typed state;
- escalate to an operator through Bifrost when human action is needed.

The desired shape is simple:

```text
Odin knows what exists.
Idunn keeps it alive.
Bifrost carries operator/public crossings.
Each daemon owns its own work and health publication.
```

## Who It Helps

Idunn is for anyone running a local GameCult swarm or operating a hosted
GameCult machine.

For a developer, Idunn should mean fewer mystery services to restart by hand.
For an operator, Idunn should mean health problems become visible, witnessed,
and routed to the right place. For daemon authors, Idunn should mean they can
publish health, transport profiles, and command boundaries over CultNet RUDP
instead of building one more private watchdog.

## Current State

Idunn is a Rust daemon inside Odin's Cargo workspace. The live local runtime is
one long-lived `idunn.exe` process that owns the whole Starfire-local swarm:
Odin, local adjunct daemons, the Yggdrasil deploy lanes, and the Nightwing
display services. Each target declares a daemon-owned health contract and keeps
its own interval and deploy/restart authority. The current shell health
commands are compatibility probes until each daemon updates its CultLib
dependency and publishes health through CultNet over the shared RUDP transport.
The scheduler and continuity witness now belong to one Rust process instead of
a PowerShell-herded pile of one-daemon workers.

```text
scratch/idunn/idunn.keepalive.cc
```

## Run It

From `E:\Projects\Odin`:

```powershell
npm run idunn:build
npm run idunn:start -- --daemon demo --health-command "exit 0"
```

Install the local Starfire boot watchdog:

```powershell
.\scripts\install-idunn-startup.ps1
```

That task starts one `idunn.exe` swarm supervisor at user logon. The Rust
runtime owns the target catalog for Odin, Stonks, Muninn, the enforced
Yggdrasil source artifact lanes, and the Nightwing display services. The
current Mimir dashboard is observed but not restarted until its restart
authority is named.

It also starts `idunn-swarm-deployment-coverage`, which watches the deployment
target catalog in `scripts/idunn-deployment-targets.ps1`. A repo/service is not
allowed to vanish into shrug-space: it is either enforced by Idunn, explicitly
blocked with the missing authority named, external-owned, or not a runtime.

To record a failed health check and request a restart without actuating:

```powershell
npm run idunn:start -- --daemon demo --health-command "exit 1" --deploy-command "echo deploy demo" --restart-command "echo restart demo"
```

To actually run the restart command:

```powershell
npm run idunn:start -- --daemon demo --health-command "exit 1" --deploy-command "echo deploy demo" --restart-command "echo restart demo" --execute
```

To keep watching on a resident interval:

```powershell
npm run idunn:start -- --daemon demo --health-command "exit 0" --interval-seconds 30
```

To run the built-in Starfire-local swarm profile directly:

```powershell
npm run idunn:start -- --swarm-profile starfire-local --repo-root E:\Projects\Odin --execute
```

Optional store override:

```powershell
npm run idunn:start -- --daemon demo --store E:\path\to\idunn.keepalive.cc --health-command "exit 0"
```

## What Daemons Should Publish

Idunn should not guess private service truth. A daemon should publish:

- its service id and Verse id;
- where its durable `.cc` state or witness lives;
- a health or freshness signal;
- the command boundary for deployment or artifact refresh, if one exists;
- the command boundary for restart or recovery, if one exists;
- what operator action is needed when automatic recovery is unsafe.

If that information is missing, Idunn should fail closed and create an operator
alarm instead of improvising.

For the repo swarm, deployment ownership begins with catalog coverage. Use:

```powershell
.\scripts\show-idunn-deployment-targets.ps1
.\scripts\health-idunn-swarm-deployment-coverage.cmd
```

The enforced targets are Nightwing Gjallar plus the Yggdrasil source artifact
lanes whose ops runbooks can produce and verify deployment manifests. Bifrost
is blocked until its production database migration path is coherent; mobile
device installs remain blocked at their physical approval boundaries; GitHub
Pages remains external-owned.

## Typed Records

- `idunn.desired_daemon.v1`
- `idunn.daemon_health.v1`
- `idunn.keepalive_decision.v1`
- `idunn.deployment_request.v1`
- `idunn.deployment_result.v1`
- `idunn.restart_request.v1`
- `idunn.restart_result.v1`
- `idunn.operator_alarm.v1`
- `idunn.daemon_surgery_plan.v1`
- `idunn.daemon_transport_profile.v1`
- `idunn.command_boundary.v1`
- `idunn.runtime_transport_check.v1`
- `idunn.rudp_health_ingress.v1`

Idunn publishes one `idunn.daemon_surgery_plan.v1` record per swarm target when
the swarm starts. Those records make the CultNet/RUDP migration queue explicit:
owner, objective, current compatibility mechanism, intended authority, cut line,
steps, blockers, severity, and status.

It also publishes one `idunn.daemon_transport_profile.v1` and one
`idunn.command_boundary.v1` per target. The desired daemon record links to
both. The transport profile names `cultnet.transport.rudp.v0` as the target and
marks the current command probe as compatibility evidence. The command boundary
names restart, deploy, health, and alarm authority separately.

At startup Idunn also publishes `idunn.runtime_transport_check.v1`, currently a
loopback CultNet hello over `cultnet.transport.rudp.v0`. That proves Idunn's
Rust body can use the RUDP substrate before it asks the rest of the swarm to
walk through the same door.

The local swarm also opens a RUDP health ingress at `127.0.0.1:17870` unless
started with `--rudp-health-bind none`. It accepts raw
`idunn.daemon_health` document puts over `cultnet.transport.rudp.v0` and writes
them into the keepalive store. That is daemon-owned health publication, not
restart/deploy authority.

When a fresh daemon-published RUDP health record exists for a target, Idunn uses
that record for keepalive planning before falling back to the local command
probe. The record must match the daemon id, health contract, RUDP transport,
and freshness window; otherwise the compatibility probe remains fallback
evidence.

The next cuts are updating daemon CultLib dependencies so health and command
boundaries publish through `cultnet.transport.rudp.v0`, switching Idunn to those
daemon-owned records, promoting the built-in target catalog into Odin-ingested
service/provider records, keeping named adapters only for legacy service-manager
crossings, and replacing the local operator alarm bridge with a fully
Bifrost-owned notification request record.

Rust no longer gets to claim the transport is imaginary:
`vendor/cultnet-rs` includes a narrow stop-and-wait
`cultnet.transport.rudp.v0` path for acknowledged CultNet messages over UDP.
That is substrate only. Idunn still marks daemon profiles as
`migration-required` until each daemon actually publishes health and command
boundary records through that path.

## Boundaries

Idunn is not Odin. It does not decide which Verses exist.

Idunn is not Bifrost. It does not own public crossings, Discord delivery, owner
DMs, or governance transport.

Idunn is not Eve. It does not own presentation.

Idunn is the keeper of continuity. If a daemon should be alive, Idunn should
know that, check it, help it recover, and leave a witness trail sharp enough
that nobody has to guess what happened.
