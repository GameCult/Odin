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
  requests/results, release targets, deployment artifacts, state migration
  plans/results, rollout plans/results, denied-action records, operator alarms,
  Bifrost operator-notification requests, and an Eve/CultUI keepalive surface.
- Derived state: dashboard cells, Bifrost receipts, Discord or owner-DM
  lowerings, agent summaries, and Odin service projections are
  notification-only views of Idunn-owned keepalive records.
- Forbidden writers: Idunn does not decide which Verses exist, invent provider
  schemas, mutate provider dashboards, own identity/session grants, or hide
  restarts behind Odin refresh logic. Individual daemons should not carry
  independent crash-recovery loops once Idunn owns their lifecycle path; they
  publish health, surfaces, state witnesses, and command boundaries instead.
  Agents are also forbidden deploy writers: they configure Idunn release
  targets, command boundaries, migration commands, and rollout policy, then let
  Idunn actuate and witness deployment. They do not run deploy scripts by hand.
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
  artifact lane. They fetch the declared upstream branch, package
  `origin/main` with `git archive`, run any declared daemon-owned migration
  script before the deploy script, run the existing ops-owned deploy/check
  scripts on Yggdrasil, and stamp a remote
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
idunn.release_target.v1
idunn.deployment_artifact.v1
idunn.state_migration_plan.v1
idunn.state_migration_result.v1
idunn.rollout_plan.v1
idunn.rollout_result.v1
idunn.restart_request.v1
idunn.restart_result.v1
idunn.operator_alarm.v1
idunn.swarm_surgery_plan.v1
idunn.daemon_surgery_plan.v1
idunn.daemon_transport_profile.v1
idunn.command_boundary.v1
idunn.runtime_transport_check.v1
idunn.rudp_health_ingress.v1
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
  `idunn.daemon_health.v1` also records `publication_source` and `transport` so
  daemon-published RUDP health can be distinguished from compatibility command
  evidence.
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
- Rust now shares the canonical cross-runtime `cultnet.transport.rudp.v0`
  substrate in `vendor/cultnet-rs`: CNR0 packets, sessions, channels, reliable
  schema frames, and timeout/retry semantics matching the TypeScript/Python
  CultLib implementations. This removes "Rust cannot speak RUDP" as a substrate
  excuse. It does not make any daemon fully migrated until that daemon publishes
  its health and command boundary through the RUDP path and Idunn consumes that
  daemon-owned publication instead of the compatibility command.
- Idunn publishes `idunn.runtime_transport_check.v1` at startup. The current
  check sends a CultNet hello over loopback `cultnet.transport.rudp.v0` and
  records whether the acknowledgement path works in Idunn's own Rust runtime.
  This proves Idunn's local substrate, not fleet migration.
- Idunn also opens a RUDP health ingress and publishes
  `idunn.rudp_health_ingress.v1`. The Starfire local supervisor binds
  `0.0.0.0:17870` so host-local publishers can use `127.0.0.1:17870` and
  WireGuard peers such as Nightwing can publish to `10.77.0.2:17870`. That ingress
  accepts only raw `idunn.daemon_health` CultNet document puts on the `schema`
  channel, decodes the typed MessagePack payload, and writes it into the
  keepalive store. Each one-shot publisher gets its own RUDP session from its
  UDP source address and the session is discarded after a delivered health
  frame. Windows UDP `ConnectionReset`/`ConnectionAborted` reports from closed
  one-shot clients are nonfatal ingress noise, not a reason to kill the worker.
  The ingress does not grant deploy/restart authority and it does not make
  compatibility probes owners; it is the first daemon-owned health publication
  path Idunn can consume.
- During each target cycle, fresh daemon-published RUDP health wins over the
  local compatibility command. Idunn accepts it only when the daemon id, health
  contract, `publication_source=daemon-published`, transport
  `cultnet.transport.rudp.v0`, and `max_silence_seconds` freshness window all
  match. If any of those fail, the command probe remains fallback evidence.
- Idunn publishes the active swarm transport migration plan as
  `idunn.swarm_surgery_plan.v1`. That record names the active profile, owner,
  objective, current mechanism, invariants, ordered phases, current phase, next
  target, cut line, and verification layer. It is the state-surface answer to
  "what surgery is Idunn doing next?" and must be lowered by Nightwing/Gjallar
  before any chat summary or dashboard prose claims ownership of the plan.
- Idunn publishes per-target transport migration plans as
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
- Muninn continuity on Raven is a remote keepalive path. The long-running Raven
  `muninn.exe serve` process now carries its own Idunn RUDP identity and
  publishes `muninn.cultnet-rudp-remote-telemetry-health` directly; the
  `muninn.exe --health` command remains fallback/manual proof only. Idunn still
  restarts Muninn's idle `serve` posture when needed. Raven is the host/body;
  Muninn is the local telemetry Verse assembler. Idunn must not activate
  screen/audio streams as part of keepalive. Raven A/V over SRT is an explicit
  activation path through `activate-muninn-raven-av-srt.ps1`, not daemon
  startup behavior.
- Raven is an operator-consented shared machine. Idunn restart and health
  actuators for Raven must be background-only and must not open visible
  terminals or interactive windows on that host. The Raven Muninn restart path
  uses a hidden WScript/PowerShell trampoline; replacing it with an interactive
  console launch violates the ops contract.
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
  freshness contract for the repo swarm. For enforced deploy targets, the
  catalog must name upstream remote/branch, rollout strategy, state migration
  authority, and whether zero downtime is actually available. Idunn deploys the
  desired upstream revision, not arbitrary local developer `HEAD`. It does not
  mean Idunn invents deploy authority for every repo immediately. A target
  without a safe noninteractive deploy command must remain `blocked` or
  `external-owned` with the missing authority named until a wrapper can produce
  a deployment manifest and route through Idunn's typed deployment
  request/result path.
- State migration is daemon-owned. Idunn may run and witness a declared
  migration command before deployment, but the daemon/repo owns schema meaning,
  backups, and the migrator. If migration fails, Idunn records
  `idunn.state_migration_result.v1`, stops the deployment, records a failed
  rollout/deployment result, and alarms instead of trying to repair state
  behind the daemon's back.
- Deploy scripts are Idunn actuators, not agent tools. They refuse to run
  unless Idunn invokes them with `IDUNN_ACTUATOR=1`. Agent work is to update the
  target catalog, release target, command boundary, migration plan, and daemon
  publication surfaces so Idunn can run the shared command primitive.
- Zero downtime is a declared rollout capability, not a slogan. If a daemon
  lacks hot reload, blue/green routing, rolling instances, or another named
  in-place swap mechanism, Idunn records `restart-required` and verifies the
  restart path honestly.
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
3. a typed `idunn.swarm_surgery_plan.v1` record, so Idunn's active migration
   order, next target, cut line, and verification layer are explicit state;
4. a typed `idunn.daemon_surgery_plan.v1` record per target, so transport debt
   is visible in the same state surface as daemon health;
5. a typed `idunn.daemon_transport_profile.v1` and
   `idunn.command_boundary.v1` record per target, so compatibility probes and
   local commands cannot pretend to be daemon-owned CultNet/RUDP truth;
6. a startup `idunn.runtime_transport_check.v1` witness proving Idunn's own
   Rust RUDP loopback path;
7. a local `idunn.rudp_health_ingress.v1` listener for daemon-owned RUDP
   health publication;
8. one in-process schedule per target instead of one watchdog process per target;
9. shared typed keepalive records in one CultMesh store;
10. deploy/restart/alarm execution through the same Rust decision engine;
11. recovery of fast local targets like Odin without waiting behind slow remote
   Yggdrasil checks.

Current plan surface: `idunn.swarm_surgery_plan.v1` for profile
`starfire-local` treats the Muninn Rust lanes, Odin's local provider-health
lane, Stonks daemon health, Weksa daemon health, VoidBot stack health, and
Nightwing Gjallar framebuffer composition health as completed substrate cuts.
Muninn's long-running `serve` bodies now publish
`idunn.daemon_health` over RUDP themselves; Starfire publishes to local Idunn,
while Nightwing publishes over WireGuard to `10.77.0.2:17870` and Raven's live
launcher publishes over LAN to `192.168.1.66:17870` using their
target daemon ids and health contracts. `muninn --health` keeps the same path
as fallback/manual proof, but it is no longer the live owner. Odin
publishes `odin.cultnet-rudp-provider-health` after each provider refresh.
Stonks publishes `stonks.cultnet-rudp-market-health` after each serialized
market refresh. Weksa publishes `weksa.cultnet-rudp-provider-health` after each
serialized provider witness refresh. VoidBot publishes
`voidbot.cultnet-rudp-stack-health` after each GameCult Local Orchestrator
pulse. Gjallar publishes
`gjallar.cultnet-rudp-framebuffer-composition-health` from the C# Nightwing
framebuffer service. Mimir Eve dashboard publishes
`mimir.cultnet-rudp-provider-health` from the Nightwing systemd broker, and the
same process publishes `nightwing.cultnet-rudp-eve-dashboard-health` for the
Nightwing dashboard service itself. The Nightwing Eve browser reference now runs
as `Mimir.EveBrowserReference` instead of raw `python3 -m http.server`, serving
the same static lowering and publishing
`nightwing.cultnet-rudp-browser-reference-health` from its own service process.
Live Idunn cycles accept these records before command-probe fallback.
Raven's Muninn scheduled-task repair remains a separate ops invariant:
`GameCult-Muninn`, `GameCult-Muninn-Activate`, and
`GameCult-Muninn-VideoProof` must execute `wscript.exe` hidden launcher actions,
not raw `.cmd` task actions. Their hidden VBS launcher bodies must call
PowerShell entrypoints directly rather than routing through `cmdPath`
trampolines, and the live Raven `serve` process must carry its own
`--idunn-rudp-health`, `--idunn-daemon`, and `--idunn-health-contract`
arguments.

Next: move the remaining Yggdrasil deployments off compatibility health/deck
checks, with repixelizer now the explicit next cut, then continue
runtime-by-runtime until compatibility probes can be deleted or demoted.
Heimdall has now crossed the first live transport line: the Yggdrasil runtime
publishes `heimdall.cultnet-rudp-provider-health`, writes a daemon-owned
boundary witness at `/srv/heimdall/cultcache/heimdall.service.cc`, and the
deploy lane ships the required `CultLib` snapshot beside the app artifact.
Heimdall still owes Odin ingestion of that boundary store plus the later
redacted auth-document witness export, but it is no longer waiting on basic
RUDP keepalive surgery. Weksa
now publishes daemon-owned provider advertisement, operator state, Eve surface,
command boundary, and transport profile records in its provider store, and Odin
local discovery can ingest those records. Weksa still owes CultNet/RUDP command
document ingress for MiMo VoiceDesign before its HTTP command route can become a
debug-only lowering. Stonks now publishes daemon-owned provider advertisement,
market snapshot, Eve surface, command boundary, and transport profile records in
its CultCache store, and Odin local discovery can ingest those records; its
HTTP/WebSocket endpoints are renderer/debug lowerings. StreamPixels now has a
service-owned CultCache boundary store at
`E:\Projects\StreamPixels\.streampixels-data\cultcache\streampixels.service.cc`
containing provider advertisement, command boundary, transport profile, and an
Idunn health summary, and Odin local discovery can ingest it. The live
Yggdrasil deployment now keeps
`/srv/streampixels/env/service.env` wired for
`STREAMPIXELS_IDUNN_RUDP_HEALTH=10.77.0.2:17870` with contract
`streampixels.cultnet-rudp-service-health`, publishes the boundary store at
`/srv/streampixels/app/.streampixels-data/cultcache/streampixels.service.cc`,
and has live Idunn acceptance proof for `yggdrasil-streampixels` from
`10.77.0.1`. StreamPixels now owes only the final demotion of
SSH/systemd/HTTP checks from compatibility proof to deployment/debug witness
once Odin consumes the typed store without fallback. Repixelizer still remains
plain GUI/systemd compatibility debt today: `repixelizer-gui.service`,
`/api/health`, `/api/config`, and nginx routing are still the live witnesses
until the runtime publishes internal RUDP health and typed queue/provider
state, so it is the next Yggdrasil target. VoidBot, Gjallar, Mimir, and
the Nightwing Eve runtime services still owe provider advertisement and
command-boundary RUDP publication before their compatibility surfaces can be
purely display/debug lowerings; Gjallar also owes native CultMesh/RUDP deck
input to replace the current Odin WebSocket lowering bridge. Raven Muninn task
action repair is no longer queued: the live host now executes
`GameCult-Muninn`, `GameCult-Muninn-Activate`, and
`GameCult-Muninn-VideoProof` through hidden `wscript.exe` launchers whose VBS
bodies call `.ps1` launchers directly, and the repair actuator remains
`scripts\repair-raven-muninn-task-actions.ps1`.

Vili's Node daemon now has an in-process Idunn RUDP publisher for
`vili.cultnet-rudp-animation-health`, and local smoke proof shows Idunn accepts
that record over `cultnet.transport.rudp.v0`. Vili also writes
`.vili\vili.service.cc` with daemon-owned provider advertisement, operator
state, Eve surface, command boundary, and transport profile records. Odin local
provider discovery can ingest that typed store, including the Vili command
boundary and transport profile. That is now live Raven proof as well: Odin's
`scripts\restart-vili.ps1` refreshes the Raven runtime from the authoritative
local Vili and CultLib files, reinstalls the hidden `GameCult\Vili` task with
`--idunn-rudp-health 10.77.0.2:17870`, and restarts it. Live Idunn accepts
`vili.cultnet-rudp-animation-health` from `10.77.0.4`.

No ad hoc JSON manifest, HTTP endpoint, TCP socket, or WebSocket bridge may
become the live state owner. Debug projections are fine when they name the
`.cc` record, CultNet document, or CultMesh publication behind them.
