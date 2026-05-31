# Odin Architecture

## Objective

Odin is the central all-seer node for GameCult's CultMesh world: every Verse can discover every other Verse, learn which schemas they speak, and ask for a translation route when their local document shape differs.

## Current Mechanism

```text
host / device / service probes
  + Mimir device observation ledger
  + Eve/CultUI provider surfaces
  -> Odin observation cycle
  -> Verse-owned service records
  -> Device observation-stream records
  -> Provider-owned interface records
  -> Odin state document
  -> CultMesh/CultCache persistence
  -> Eve dashboard state
  -> Eve, browser, compact TUI, and future renderers
```

This first path proves the operator surface and persistent state. It does not yet pretend to be full peer exchange.

## Target Mechanism

```text
Verse announcement
  -> CultNet hello and schema catalog exchange
  -> Odin registry
  -> compatibility and translation index
  -> subscriptions / worker routing / dashboard projection
```

## Invariants

- Odin owns the accepted registry of known Verses.
- A Verse owns its own schemas and authority model; Odin indexes and translates, it does not silently rewrite local truth.
- Device clients own sensor and media capture; Mimir owns the normalized ingest ledger; Odin owns the aggregate operator projection.
- Translation paths must name source schema, target schema, lossiness, authority, and version.
- Service dashboards are CultMesh/Eve/CultUI interface projections. Odin aggregates those projection graphs; it does not replace them with nameplate summaries.
- Renderers lower surfaces only. If a renderer fixes network truth, the machine is split-brained.
- CultCache is the durable state substrate; CultNet is the wire vocabulary; CultMesh is the Verse and peer-consensus layer.
- The Eve surface carries explicit `verse`, `service`, and `observation-stream` nodes. Compact renderers may derive visual facets from those nodes, but may not invent observation truth.

## Current Service Surface

Odin currently publishes service squares for:

- Starfire: Odin, Docker, ADB, Odin CultCache, and running Docker containers.
- Nightwing: SSH, Eve broker, Eve browser reference, visible TUI, Docker unit state, and NVIDIA GPU state.
- EVE: SSH/native Eve reachability.
- Periwinkle: ADB reachability.
- Periwinkle: latest typed motion, microphone, touch, and camera stream summaries from Mimir's CultMesh observation ledger when present.
- Raven: SSH reachability.
- Yggdrasil: SSH/HTTP/HTTPS reachability plus nginx, StreamPixels, Heimdall, Repixelizer, and Bifrost systemd state.

The compact Nightwing TUI lowers these into dense cells and fills surplus screen space with derived schema, route, owner, and probe facets from the same Odin-owned service records.

## Current Interface Surface

Odin currently ingests `mimir.live.stats` and `voidbot.swarm` from the local Mimir Eve deck at `ws://127.0.0.1:8795/eve/deck`. `mimir.live.stats` owns the compact Mimir telemetry composition: RMS bars, sync confidence, runtime buffer facts, actuator state, and device observation streams. `voidbot.swarm` owns the VoidBot swarm composition: CTB rail, selected Face summary, state graph, and state detail. Odin embeds each provider's `surface.root` as an `interface` child with provenance, version, status, and source endpoint.

This is the model for future services: if a service publishes an operator interface, ingest the Eve/CultUI composition graph and lower it. Do not collapse it into a service-status tile unless the graph is unavailable and the tile is explicitly a temporary probe.

## Current Observation Surface

Odin tails Mimir's normalized observation JSONL at `E:\Projects\Mimir\artifacts\runtime\periwinkle-cultmesh-sensors.out.log` by default. It accepts the typed CultMesh records `mimir.eve_sensor_observation.v1` and `mimir.eve_media_observation.v1`, keeps only the latest record for each `(deviceId, streamId, kind)`, and publishes those summaries as `observation-stream` nodes inside `odin.allseer`. Streams are considered active for 120 seconds by default so a compact dashboard refresh does not turn a briefly quiet sensor into false red noise.

This is an ingest projection, not a new capture authority. Device clients still own capture, Mimir still owns the evidence ledger, and Odin only publishes the dashboard-ready state that the Eve GUI and Nightwing TUI lower.

## First Translation Model

Odin's translation registry should start as data, not magic:

- `sourceSchema`
- `targetSchema`
- `translationKind`: `identity`, `projection`, `lossyProjection`, `adapter`, or `unsupported`
- `owner`
- `version`
- `notes`

No regex tribunals for meaning. Natural-language schema interpretation can be assisted by models later, but accepted translation routes must be inspectable typed state.
