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

## Runtime Body

Odin's executable body is split by ownership:

- `src/odin-coordinator.cjs`: process lifecycle, refresh loop, persistence, health, and transport wiring.
- `src/odin/config.cjs`: runtime paths, seed deck URLs, intervals, and CultLib module path setup.
- `src/odin/documents.cjs`: CultCache/CultMesh document definitions accepted by Odin.
- `src/odin/probes.cjs`: local Docker/ADB/TCP probes plus named SSH service and GPU probes.
- `src/odin/observations.cjs`: Mimir observation ledger tailing and dashboard-ready stream projection.
- `src/odin/interfaces.cjs`: Eve provider manifest discovery, provider WebSocket fetches, and CultMesh interface bindings.
- `src/odin/layout.cjs`: `odin.interface_layout.v1` read/write and merge policy.
- `src/odin/surface.cjs`: `gamecult.eve.surface.v1` tree projection.
- `src/odin/state.cjs`: one refresh's input records into `odin.allseer` dashboard state.
- `src/odin/websocket.cjs`: Eve deck HTTP/WebSocket serving and client WebSocket helpers.

The entrypoint is not allowed to grow new probe, surface, provider, layout, or renderer policy. If a new owner is needed, name the owner and its invariant before adding code.

Gjallar is the named herald organ for future agent-context transmission. Its
current package lives in `docs/gjallar.md`, `personas/gjallar.persona_state.cc`,
`src/Gjallar/Gjallar.csproj`, `assets/personas/gjallar-avatar.png`, and
`assets/personas/gjallar-avatar-pixel-256.png`. When it becomes executable, it
should be added as its own C# CultMesh entrypoint rather than being folded into
Odin's coordinator. Gjallar may read Odin-owned state and emit affordance
packets; it must not own the underlying registry, probe, provider, layout, or
translation decisions.

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

## Service Architecture Contract

Odin is the witness for the GameCult service contract:

```text
durable service state -> CultCache .cc
shared local visibility -> CultMesh
interactive operator surface -> Eve GUI/TUI DSL
discovery and aggregation -> Odin
renderer bodies -> Eve clients, browser, compact TUI, native surfaces, overlays
```

When Odin sees a service, it should be able to answer:

- What Verse owns this service?
- Which typed schemas does it publish?
- Where is its durable `.cc` state or CultCache-compatible store?
- Which CultMesh documents or providers make it visible locally?
- Which Eve GUI/TUI surface represents its meaningful operator interface?
- Which command boundary accepts, denies, forwards, or reconciles user intent?
- Which fields are stale, predicted, denied, or authoritative?

This is not a reporting nicety. It is how Odin prevents services from becoming
private little islands with separate dashboards, separate state formats, and
separate command languages.

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

Odin discovers Eve deck provider manifests from known deck endpoints and LAN hosts exposing the standard Eve deck port. It then ingests each provider's Eve/CultUI state through the provider switch command path. Providers own their compositions; Odin embeds each provider's `surface.root` as an `interface` child with provenance, version, status, source endpoint, and layout metadata.

This is the model for future services: if a service publishes an operator interface, ingest the Eve/CultUI composition graph and lower it. Do not collapse it into a service-status tile unless the graph is unavailable and the tile is explicitly a temporary probe.

The expected provider output is Eve DSL or an equivalent
`gamecult.eve.surface.v1` retained tree. GUI and TUI are lowerings of the same
interactive language; they are not separate dashboard products. Huginn's `.cc`
inspection surface is the current clean example: Huginn inspects CultCache bytes
and emits Eve DSL, while Eve or any other runtime owns presentation.

Odin persists operator layout intent as `odin.interface_layout.v1` under `scratch/odin/interface-layout.json` for the current Starfire body. The durable CultMesh document should replace this local file once the layout schema is promoted. Layout intents name the provider id and request focus, move, resize, or visibility changes; renderers are input devices for those intents, not local layout owners.

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
