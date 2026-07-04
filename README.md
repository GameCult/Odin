# Odin

Odin is the GameCult all-seer: the central CultMesh node every Verse can use to discover the other Verses, inspect schema catalogs, and find translation paths between local realities.

It is not the renderer. It is not Eve. It is not a Starfire utility script wearing a bigger coat. Odin owns discovery, schema awareness, translation planning, and accepted operator surfaces. Eve clients and Gjallar lower Odin's published CultUI surface into whatever body they have.

Odin is also the compliance witness for the GameCult service architecture:
durable service state in CultCache `.cc`, local Verse visibility through
CultMesh, meaningful presentation as Eve GUI/TUI DSL, and renderers as lowerers
only.

## Rust Spine

The target Odin body is Rust-first: ingest through narrow ports, normalize into
typed Odin records, persist through CultCache `.cc`, expose through CultMesh /
CultNet document registries over the shared RUDP transport, and lower
interface state through Eve/CultUI.

The first Rust core lives in `crates/odin-core` and already separates typed
documents, ingest ports, normalization, and repository persistence so unit tests
can use mocked inputs and pipeline smokes can prove typed handoff without
booting the whole daemon. Gjallar is not part of that Rust record spine: it is
the Nightwing-resident terminal compositor in `E:\Projects\Gjallar` that
consumes Odin's accepted `gamecult.eve.surface_state` snapshot over CultNet/RUDP
and renders the live display.

## Gjallar

Gjallar is the herald display daemon that runs on Nightwing. Odin sees the
Verses, accepts provider surfaces, and publishes the `odin.providers` catalog.
Gjallar consumes Odin's accepted surface snapshot over CultNet/RUDP, composes
the multi-scale tiled dashboard from that typed state, lowers Odin's canonical
marquee tape into continuous gutter text, owns dense character-level update
behavior, and writes the visible framebuffer.

Local package surfaces:

- Organ contract: `docs/gjallar.md`
- Branding Persona state: `personas/gjallar.persona_state.cc`
- Runtime source: `E:\Projects\Gjallar`
- Avatar asset: `assets/personas/gjallar-avatar.png`
- Pixel avatar: `assets/personas/gjallar-avatar-pixel-256.png`

## Idunn

Idunn is Odin's keepalive organ: the daemon package that should keep the known
swarm alive after Odin has accepted where each daemon lives and what authority
path may touch it. Individual daemons publish health and command boundaries;
Idunn brings them up after reboots or crashes, watches health, and escalates
operator-needed failures through Bifrost's CultMesh bridge. VoidBot owner-DM
delivery is a demoted compatibility actuator, not the owner; the command belongs
in Bifrost's Verse. Odin sees the daemons; Idunn keeps the apples from rotting.
Agents do not deploy daemons directly. They configure Idunn's target catalog,
release targets, migration commands, and command boundaries so Idunn can run
the shared rollout primitive and leave typed witnesses behind.

Local package surfaces:

- Organ contract: `docs/idunn.md`
- User README: `src/Idunn/README.md`
- Rust daemon crate: `crates/idunn-daemon`
- Rust lifecycle logic: `crates/odin-core/src/idunn.rs`
- Runtime store: `scratch/idunn/idunn.keepalive.cc`
- Local VoidBot keepalive: `scripts\health-voidbot.cmd` and
  `scripts\restart-voidbot.cmd`
- Operator escalation: Bifrost-owned CultMesh crossing; current compatibility
  delivery calls `scripts\notify-idunn-operator-alarm.ps1`, which asks Bifrost
  to publish a typed `gamecult.operator_dm_request.v1` CultMesh command document
  only after Idunn raises an alarm

## Authority Map

- Owner: Odin owns the network-wide Verse registry, schema catalog index, translation map, and accepted provider catalog/proxy surfaces.
- Inputs: CultMesh/CultNet peer announcements, schema catalog responses, daemon
  health/provider publications over `cultnet.transport.rudp.v0`, local
  Docker/ADB debug facts, and provider-owned Eve/CultUI surfaces.
- Outputs: CultCache-backed Odin state, CultMesh documents, and CultNet
  schema/catalog messages. Browser, GUI, TUI, and framebuffer renderers lower
  those documents outside Odin instead of asking Odin to host web surfaces.
- Derived state: Gjallar's attached Nightwing display, browser dashboards, and future Eve clients are projections of Odin state and provider-owned Eve/CultUI surfaces.
- Forbidden writers: renderers do not probe the network or decide Verse truth; individual projects do not maintain private incompatible discovery ledgers once Odin can see them.
- Shared paths: human dashboards, worker schedulers, Verse bootstrap code, and compact TUI views consume the same registry and schema catalog.
- Deletion line: old per-host coordinator scripts should be deleted or reduced to deployment wrappers that start Odin.

## Run Locally On Starfire

```powershell
.\scripts\start-odin.ps1 -IdunnRudpHealth $env:IDUNN_RUDP_HEALTH
```

`-IdunnRudpHealth`, `ODIN_IDUNN_RUDP_HEALTH`, or `IDUNN_RUDP_HEALTH` must name
the Idunn RUDP health endpoint. Odin does not assume a localhost health target.

Odin's native document catalog is addressed by CultMesh URI. Concrete RUDP
bootstrap is configured behind CultMesh URI resolution by the operator or by
Odin/Idunn deployment state:

```text
cultmesh://odin/rendezvous/provider-catalog
```

That URI accepts typed document publication and schema/catalog requests through
the shared CultMesh runtime. Consumers that need Odin's accepted surface can
still request the current CultNet snapshot after CultMesh resolves the transport.

Legacy browser/deck lowerers must consume Odin's CultMesh state through their
own lowering process. Odin no longer hosts browser-deck surfaces or publishes
deck URLs as discovery seed material.

State and logs live under ignored `scratch/odin/`.

## Current First Body

The first executable is deliberately narrow:

- publishes provider catalog `odin.providers`;
- persists the latest surface through local CultMesh/CultCache when `CultLib` packages are available at `E:\Projects\CultLib\packages`;
- writes `scratch/odin/latest-surface.json` only when `--write-debug-surface-json` or `ODIN_WRITE_DEBUG_SURFACE_JSON=1` is explicitly supplied;
- observes Starfire Docker and Periwinkle ADB as local debug/edge facts;
- derives remote Verse presence from provider-owned CultMesh/CultNet
  advertisements and interface records, not TCP/SSH/systemd probes.
- publishes explicit `verse` and `service` nodes for compact Eve/CultUI lowerers.
- ingests provider-owned Eve/CultUI dashboards, including `mimir.live.stats` and `voidbot.swarm`, and embeds them as Odin `interface` nodes;
- accepts live `gamecult.eve.provider_advertisement.v1` announcements through Odin's CultMesh/RUDP rendezvous path so daemons can announce schemas, surfaces, commands, nested Verses, and style capabilities without Odin scraping private dashboards;
- accepts explicit local debug imports only when `--interfaceBindingStore` / `ODIN_INTERFACE_BINDING_STORES` entries are written as `cultmesh-store:file://...` URIs; raw filesystem paths are not discovery configuration;
- preserves provider semantic addresses such as `asgard.starfire.bifrost/eve/tui` and `asgard.starfire.bifrost/eve/gui`, with CultNet routes carried as transport metadata rather than identity;
- persists operator tiling intent as `odin.interface_layout.v1` in the Odin CultMesh store; ignored `scratch/odin/interface-layout.json` is migration input only.

Provider advertisements and CultNet/RUDP transport profiles are the discovery
path. External host probes, product health checks, port probes, and renderer
bridges are debug or lowering surfaces outside Odin only.
