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
- restart daemons after crashes;
- watch health and freshness signals;
- avoid restarting services when authority is unclear;
- record every restart request and result as typed state;
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

Idunn is currently a small C# CultMesh entrypoint inside the Odin repo. It can
open or create its local keepalive CultCache/CultMesh store:

```text
scratch/idunn/idunn.keepalive.cc
```

It does not yet restart services. The current body is the runtime foothold and
contract boundary, not the full supervisor.

## Run It

From `E:\Projects\Odin`:

```powershell
npm run idunn:build
npm run idunn:start
```

To keep the CultMesh node resident:

```powershell
npm run idunn:start -- --serve
```

Optional store override:

```powershell
npm run idunn:start -- --cache E:\path\to\idunn.keepalive.cc
```

## What Daemons Should Publish

Idunn should not guess private service truth. A daemon should publish:

- its service id and Verse id;
- where its durable `.cc` state or witness lives;
- a health or freshness signal;
- the command boundary for restart or recovery, if one exists;
- what operator action is needed when automatic recovery is unsafe.

If that information is missing, Idunn should fail closed and create an operator
alarm instead of improvising.

## What Comes Next

The next implementation cuts are typed records and real adapters:

- `idunn.desired_daemon.v1`
- `idunn.daemon_health.v1`
- `idunn.keepalive_decision.v1`
- `idunn.restart_request.v1`
- `idunn.restart_result.v1`
- `idunn.operator_alarm.v1`
- `idunn.operator_escalation.v1`

After those records exist, Idunn can add restart adapters for named authority
boundaries such as systemd, Windows services, Docker, or provider-advertised
CultMesh commands.

## Boundaries

Idunn is not Odin. It does not decide which Verses exist.

Idunn is not Bifrost. It does not own public crossings, Discord delivery, owner
DMs, or governance transport.

Idunn is not Eve. It does not own presentation.

Idunn is the keeper of continuity. If a daemon should be alive, Idunn should
know that, check it, help it recover, and leave a witness trail sharp enough
that nobody has to guess what happened.
