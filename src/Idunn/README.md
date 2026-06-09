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
publish health and command boundaries instead of building one more private
watchdog.

## Current State

Idunn is a Rust daemon inside Odin's Cargo workspace. It can probe one daemon,
write desired state, health, keepalive decision, deployment request,
deployment result, restart request, restart result, and operator alarm as typed
CultMesh records, and execute deploy/restart commands when the operator
explicitly gives it authority.

```text
scratch/idunn/idunn.keepalive.cc
```

## Run It

From `E:\Projects\Odin`:

```powershell
npm run idunn:build
npm run idunn:start -- --daemon demo --health-command "exit 0"
```

Install the local Starfire boot watchdogs:

```powershell
.\scripts\install-idunn-startup.ps1
```

That task starts Idunn loops for Odin, Stonks, the enforced Yggdrasil source
artifact targets, and the Nightwing display services at user logon. The current
Mimir dashboard is observed but not restarted until its restart authority is
named.

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

The next cuts are direct Odin service/provider ingestion, named adapters for
systemd, Windows services, Docker, and provider-advertised CultMesh commands,
and a Bifrost-owned operator notification request record for alarms.

## Boundaries

Idunn is not Odin. It does not decide which Verses exist.

Idunn is not Bifrost. It does not own public crossings, Discord delivery, owner
DMs, or governance transport.

Idunn is not Eve. It does not own presentation.

Idunn is the keeper of continuity. If a daemon should be alive, Idunn should
know that, check it, help it recover, and leave a witness trail sharp enough
that nobody has to guess what happened.
