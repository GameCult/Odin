# Odin

Odin is the GameCult all-seer: the central CultMesh node every Verse can use to discover the other Verses, inspect schema catalogs, and find translation paths between local realities.

It is not the renderer. It is not Eve. It is not a Starfire utility script wearing a bigger coat. Odin owns discovery, schema awareness, translation planning, and accepted operator surfaces. Eve and the Nightwing compact TUI lower Odin's published CultUI surface into whatever body they have.

Odin is also the compliance witness for the GameCult service architecture:
durable service state in CultCache `.cc`, local Verse visibility through
CultMesh, meaningful presentation as Eve GUI/TUI DSL, and renderers as lowerers
only.

## Rust Spine

The target Odin body is Rust-first: ingest through narrow ports, normalize into
typed Odin records, persist through CultCache `.cc`, expose through CultMesh /
CultNet document registries, and lower interface state through Eve/CultUI.

The first Rust core lives in `crates/odin-core` and already separates typed
documents, ingest ports, normalization, and repository persistence so unit tests
can use mocked inputs and pipeline smokes can prove typed handoff without
booting the whole daemon.

## Gjallar

Gjallar is Odin's herald organ: the daemon/persona package that should carry
Odin's gathered sight into agent context. Odin sees the Verses; Gjallar
transmits the usable tapestry of affordances without becoming the owner of
discovery, probing, rendering, or schema truth.

Local package surfaces:

- Organ contract: `docs/gjallar.md`
- Canonical Persona state: `personas/gjallar.persona_state.cc`
- C# CultMesh organ: `src/Gjallar/Gjallar.csproj`
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

Local package surfaces:

- Organ contract: `docs/idunn.md`
- C# CultMesh organ: `src/Idunn/Idunn.csproj`
- Runtime store: `scratch/idunn/idunn.keepalive.cc`
- Operator escalation: Bifrost-owned CultMesh crossing; current compatibility
  delivery may call `voidbot.operator-dm` command `owner.dm.send`

## Authority Map

- Owner: Odin owns the network-wide Verse registry, schema catalog index, translation map, and the accepted `odin.allseer` Eve surface.
- Inputs: CultMesh/CultNet peer announcements, schema catalog responses, local host probes, Docker/ADB host facts, SSH-reachable ops hosts, Mimir's normalized Eve observation ledger, and later direct Verse subscriptions.
- Outputs: CultCache-backed Odin state, CultMesh documents, CultNet schema/catalog messages, and an Eve-compatible `/eve/deck` surface for dashboards.
- Derived state: Nightwing's attached TUI, browser dashboards, and future Eve clients are projections of Odin state.
- Forbidden writers: renderers do not probe the network or decide Verse truth; individual projects do not maintain private incompatible discovery ledgers once Odin can see them.
- Shared paths: human dashboards, worker schedulers, Verse bootstrap code, and compact TUI views consume the same registry and schema catalog.
- Deletion line: old per-host coordinator scripts should be deleted or reduced to deployment wrappers that start Odin.

## Run Locally On Starfire

```powershell
.\scripts\start-odin.ps1
curl.exe -fsS http://127.0.0.1:8797/health
```

The Eve/CultUI deck endpoint is:

```text
ws://127.0.0.1:8797/eve/deck
```

Nightwing can consume the LAN endpoint:

```text
ws://192.168.1.66:8797/eve/deck
```

State and logs live under ignored `scratch/odin/`.

## Current First Body

The first executable is deliberately narrow:

- publishes provider `odin.allseer`;
- writes the latest surface to `scratch/odin/latest-surface.json`;
- persists the same document through local CultMesh/CultCache when `CultLib` packages are available at `E:\Projects\CultLib\packages`;
- observes Starfire Docker, Periwinkle ADB, Nightwing services/GPU, Raven, EVE, and Yggdrasil reachability;
- exposes Yggdrasil service status when the local SSH alias can reach it.
- publishes explicit `verse` and `service` nodes for compact Eve/CultUI lowerers.
- publishes explicit `observation-stream` nodes for Periwinkle/EVE sensor, microphone, touch, and camera stream summaries when Mimir's CultMesh observation ledger is present.
- discovers Eve deck provider manifests from known/LAN deck endpoints;
- includes local Spotiverse on `ws://127.0.0.1:8796/eve/deck` in the known provider seed list;
- ingests provider-owned Eve/CultUI dashboards, including `mimir.live.stats` and `voidbot.swarm`, and embeds them as Odin `interface` nodes;
- accepts `gamecult.eve.provider_advertisement.v1` documents in CultMesh interface binding stores so daemons can announce schemas, `.cc` witnesses, surfaces, commands, nested Verses, and style capabilities without Odin scraping private dashboards;
- persists operator tiling intent as `odin.interface_layout.v1` under ignored `scratch/odin/interface-layout.json`.

The next real cut is to make provider advertisements the primary discovery
path. Static host probes, LAN Eve deck scans, and hardcoded deck URLs are
compatibility input adapters. They must not become the architecture.
