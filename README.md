# Odin

Odin is the GameCult all-seer: the central CultMesh node every Verse can use to discover the other Verses, inspect schema catalogs, and find translation paths between local realities.

It is not the renderer. It is not Eve. It is not a Starfire utility script wearing a bigger coat. Odin owns discovery, schema awareness, translation planning, and accepted operator surfaces. Eve and the Nightwing compact TUI lower Odin's published CultUI surface into whatever body they have.

## Authority Map

- Owner: Odin owns the network-wide Verse registry, schema catalog index, translation map, and the accepted `odin.allseer` Eve surface.
- Inputs: CultMesh/CultNet peer announcements, schema catalog responses, local host probes, Docker/ADB host facts, SSH-reachable ops hosts, and later direct Verse subscriptions.
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
- ingests the `voidbot.swarm` Eve/CultUI dashboard from Mimir's `/eve/deck` provider surface and embeds it as an Odin `interface` node.

The next real cut is to replace static host probes with CultNet schema discovery and CultMesh Verse peer exchange. The probe layer is an input adapter. It must not become the architecture.
