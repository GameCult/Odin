# Odin Architecture

## Objective

Odin is the central all-seer node for GameCult's CultMesh world: every Verse can discover every other Verse, learn which schemas they speak, and ask for a translation route when their local document shape differs.

## Current Mechanism

```text
host / device / service probes
  + Mimir device observation ledger
  + Eve/CultUI provider surfaces
  + provider advertisements
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

## Rust Target Spine

The target Odin machine is Rust-first and typed-state-first:

```text
Verse / host / device / provider inputs
  -> ingest ports
  -> normalization
  -> typed Odin records
  -> CultMesh node
  -> CultCache .cc persistence
  -> CultNet/CultMesh document registry
  -> Odin Eve/CultUI deck projection
  -> Gjallar Nightwing composition and framebuffer lowering
  -> compact display feeds
```

The first Rust core lives in `crates/odin-core`:

- `documents.rs`: typed Odin records and the CultMesh document set.
- `ports.rs`: narrow ingest traits plus clock injection for deterministic tests.
- `pipeline.rs`: collection and normalization from input observations to typed
  Odin records.
- `repository.rs`: `OdinRepository` abstraction, in-memory mock repository, and
  CultMesh-backed repository.

The Rust spine owns the future architecture. The CommonJS daemon remains the
legacy operational body until each organ crosses this typed boundary.

## Runtime Body

Odin's executable body is split by ownership:

- `crates/odin-core`: Rust target core. Owns typed Odin documents, ingest
  ports, normalization, and CultMesh/CultCache repository boundaries. This is
  the replacement spine; JavaScript remains legacy runtime/probe scaffolding
  until each organ has crossed the typed boundary.
- `src/odin-coordinator.cjs`: process lifecycle, serialized refresh loop,
  persistence, health, and transport wiring. Refreshes must not overlap because
  a refresh publishes Odin's daemon health.
- `src/odin/config.cjs`: runtime paths, seed deck URLs, intervals, and CultLib module path setup.
- `src/odin/documents.cjs`: CultCache/CultMesh document definitions accepted by Odin.
- `src/odin/idunn-rudp.cjs`: daemon-owned Odin provider health publication to
  Idunn over the canonical CultNet RUDP `schema` channel.
- `src/odin/probes.cjs`: local Docker/ADB probes plus demoted TCP/SSH/GPU
  compatibility probes while daemon CultLib dependencies migrate to CultNet
  RUDP health publication.
- `src/odin/observations.cjs`: Mimir observation ledger tailing and dashboard-ready stream projection.
- `src/odin/interfaces.cjs`: Eve provider manifest discovery, provider
  WebSocket fetches, provider advertisements, and CultMesh interface bindings.
  WebSocket fetches are compatibility input adapters, not the desired daemon
  transport.
- `src/odin/layout.cjs`: `odin.interface_layout.v1` read/write and merge policy.
- `src/odin/marquee.cjs`: canonical marquee tape assembly from Stonks securities and ordered VoidBot poem lines.
- `src/odin/surface.cjs`: `gamecult.eve.surface.v1` tree projection.
- `src/odin/state.cjs`: one refresh's input records into Odin's provider catalog/proxy state.
- `src/odin/websocket.cjs`: Eve deck HTTP/WebSocket serving and client
  WebSocket helpers. This is a compatibility bridge for current
  dashboard/lowering clients until CultNet RUDP provider subscriptions replace
  it.

The entrypoint is not allowed to grow new probe, surface, provider, layout, or renderer policy. If a new owner is needed, name the owner and its invariant before adding code.

Gjallar is the Nightwing-resident terminal compositor for what Odin can show.
Its runtime lives in `E:\Projects\Gjallar` and consumes Odin's Eve deck
directly.
Odin owns canonical marquee content; Gjallar owns provider enumeration for
display, panel packing, marquee lowering, glyph/color/framebuffer lowering,
frame stats, and the multi-scale terminal product. It must not own the
underlying registry, probe, provider truth, canonical marquee content, or
translation decisions.

Idunn is the named keepalive organ for daemon continuity. Its current Rust
body lives in `crates/idunn-daemon` and `crates/odin-core/src/idunn.rs`. Idunn may read
Odin-owned service records and provider advertisements, then bring daemons up
after reboots or crashes, watch health, emit keepalive observations, restart
requests, denied-action records, and operator alarms. When human action is
needed, Idunn uses CultMesh to request a Bifrost-owned operator
notification crossing. VoidBot's `voidbot.operator-dm` command `owner.dm.send`
is a demoted compatibility delivery actuator, not the owner. The target command
lives in Bifrost's Verse; any still-VoidBot delivery path must be invoked by
Bifrost or documented as migration debt. Idunn must not own Verse discovery,
schema truth, provider dashboards, identity grants, Discord delivery, owner-DM
delivery, or renderer layout. Keepalive loops belong in Idunn, not Odin's
coordinator or individual daemons.

Muninn is the portable local telemetry Verse assembler. Its Rust body lives in
`crates/muninn-daemon` and publishes `muninn.telemetry_surface.v1` through
CultMesh/CultCache. Muninn may run on Raven, Nightwing, Starfire, or any future
device body. It names locally accessible telemetry affordances: screen capture,
loopback audio, microphones, cameras, and future sensors. Muninn does not start
expensive capture streams merely because the daemon is alive. The default
`serve` posture publishes an idle typed surface; explicit activation, such as
`muninn activate` for Raven A/V over SRT, is the only path that starts FFmpeg,
WASAPI loopback, video capture, or similar resource-consuming workers.

Muninn owns local telemetry discovery and stream activation boundaries. It does
not own Mimir's normalized ingest ledger, OBS rendering, Gjallar composition,
Odin discovery truth, or Idunn keepalive policy. Active stream records such as
`muninn.capture_stream.v1` are evidence of requested streams, not permission for
startup to burn capture resources.

Move optical marker extraction belongs to Muninn because it is sensor stream
exposure, not Mimir fusion or Odin registry truth. The native helper lives at
`crates/muninn-move-tracker`; Muninn may publish per-frame candidates as
`muninn.move_marker_candidate.v1`. USB Move controller facts are
`muninn.move_controller_state.v1` receipts. Those records are not the hot
tracking transport: Muninn bundles marker candidates and controller states into
a CultMesh bytes stream frame with metadata schema
`mimir.muninn_move_evidence_stream_frame.v1`. Mimir consumes that stream into
tracking buffers and later fusion. Odin indexes the schema and projection
surface only.

Bifrost is the bridge for Persona speech and other public/owner-facing
crossings. When a Persona interpreter decides a Persona speaks, the accepted
side effect is a Bifrost CultMesh command or document that names actor,
authority, target surface, context, policy result, and receipt path. VoidBot
observes Discord, preserves room cognition, moderates, and may provide
compatibility delivery, but it is not the owner of swarm speech transport.
VoidBot's repo search, Discord history search, archive lookup, and source
retrieval are required native CultCache/CultMesh service surfaces. Any remaining
VoidBot-local or MCP-only implementation is migration debt. MCP is the bridge
for external agentic access, not the native path for GameCult agents that
already have CultMesh affordances.

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
- Muninn advertises local telemetry affordances cheaply and starts capture only
  after an explicit activation request.
- Muninn's live Move evidence is a CultMesh stream frame body; CultCache
  Move records are receipts/debug state and must not become Mimir's hot
  tracking path.
- Translation paths must name source schema, target schema, lossiness, authority, and version.
- Service presentation flows are CultMesh/Eve/CultUI interface projections. Odin aggregates those projection graphs; it does not replace them with nameplate summaries.
- Renderers lower surfaces only. If a renderer fixes network truth, the machine is split-brained.
- CultCache is the durable state substrate; CultNet is the wire vocabulary; CultMesh is the Verse and peer-consensus layer.
- The Eve surface carries explicit `verse`, `service`, and `observation-stream` nodes. Compact renderers may derive visual facets from those nodes, but may not invent observation truth.
- Rust organs must accept mocked inputs through narrow traits. Unit tests prove
  local invariants; pipeline smokes prove adjacent typed handoff; full daemon
  boots are not the only test path.
- JSON is not state authority. It is allowed only for schema publication,
  debugging, compatibility export, or external xenos boundaries.

## Test Surfaces

Current Rust verification:

- `pipeline_collects_from_injected_ports`: proves ingest ports and clock injection.
- `memory_repository_supports_fast_unit_tests`: proves repository consumers can test without CultMesh.
- `cultmesh_repository_round_trips_typed_records`: proves typed Odin records
  persist through CultMesh/CultCache and reload from `.cc`.

## Service Architecture Contract

Odin is the witness for the GameCult service contract:

```text
durable service state -> CultCache .cc
shared local visibility -> CultMesh
interactive presentation -> Eve GUI/TUI DSL
discovery and aggregation -> Odin
renderer bodies -> Eve clients, browser, compact TUI, native surfaces, overlays
```

When Odin sees a service, it should be able to answer:

- What Verse owns this service?
- Which typed schemas does it publish?
- Where is its durable `.cc` state or CultCache-compatible store?
- Which CultMesh documents or providers make it visible locally?
- Which Eve GUI/TUI surface represents its meaningful presentation and controls?
- Which command boundary accepts, denies, forwards, or reconciles user intent?
- Which fields are stale, predicted, denied, or authoritative?

This is not a reporting nicety. It is how Odin prevents services from becoming
private little islands with separate websites, dashboards, state formats, and
separate command languages.

## Current Service Surface

Odin currently publishes service squares for:

- Starfire: Odin, Docker, ADB, Odin CultCache, and running Docker containers.
- Nightwing: SSH, Eve broker, Eve browser reference, visible TUI, Docker unit state, and NVIDIA GPU state.
- EVE: SSH/native Eve reachability.
- Periwinkle: ADB reachability.
- Periwinkle: latest typed motion, microphone, touch, and camera stream summaries from Mimir's CultMesh observation ledger when present.
- Raven: SSH reachability.
- Yggdrasil: compatibility SSH/HTTP/HTTPS reachability plus nginx,
  StreamPixels, Heimdall, Repixelizer, and Bifrost systemd state until those
  daemon surfaces publish health and command boundaries over CultNet RUDP.

Gjallar consumes Odin's deck and composes these surfaces into the Nightwing
display. Nightwing is the host/body; Gjallar is the terminal product running
there. If Odin starts deciding framebuffer composition, the renderer owner has
leaked upward. If individual providers start tuning themselves for Nightwing
instead of emitting clean Eve/CultUI surfaces, provider truth has leaked
downward.

## Current Interface Surface

Odin discovers Eve deck provider manifests from known deck endpoints and LAN hosts exposing the standard Eve deck port. It then ingests each provider's Eve/CultUI state through the provider switch command path. Providers own their compositions; Odin embeds each provider's `surface.root` as an `interface` child with provenance, version, status, source endpoint, and layout metadata.

This is the model for future services: if a service publishes an operator interface, ingest the Eve/CultUI composition graph and lower it. Do not collapse it into a service-status tile unless the graph is unavailable and the tile is explicitly a temporary probe.

The expected provider output is Eve DSL or an equivalent
`gamecult.eve.surface.v1` retained tree. GUI and TUI are lowerings of the same
interactive language; they are not separate dashboard products. Huginn's `.cc`
inspection surface is the current clean example: Huginn inspects CultCache bytes
and emits Eve DSL, while Eve or any other runtime owns presentation.

Provider advertisements are the promotion path out of probing. Odin's CJS
document set already accepts `gamecult.eve.provider_advertisement.v1` alongside
`gamecult.eve.interface_binding.v1` and `gamecult.eve.surface_state.v1`.
Daemons should publish advertisements that name service id, Verse id, schema
catalog, `.cc` witnesses, Eve surface keys, command boundaries, nested Verses,
style capabilities, freshness, and redaction policy. Once an advertisement is
available, Odin should prefer it over LAN scans, hardcoded deck URLs, private
layout files, or web-dashboard scraping. Daemon health and provider state
should publish through `cultnet.transport.rudp.v0`; TCP, HTTP, and WebSocket
routes are compatibility exports only and must not become daemon-owned truth.

Provider advertisements should also publish semantic CultMesh addresses in this
shape:

```text
asgard.<machine>.<service>/<resource>
```

Examples:

```text
asgard.starfire.odin/eve/providers
asgard.starfire.bifrost/eve/tui
asgard.starfire.bifrost/eve/gui
asgard.yggdrasil.streampixels/eve/tui
asgard.yggdrasil.streampixels/eve/gui
```

The canonical service may omit the current machine when identity should survive
relocation, such as `asgard.bifrost`. Located service addresses name the current
host, such as `asgard.starfire.bifrost` now and
`asgard.yggdrasil.bifrost` after migration. CultNet routes are transport
metadata for resolving those names. WebSocket URLs remain compatibility deck
bridges, not native service identity; native daemon transport is CultNet over
the shared RUDP profile.

The canonical contract lives in
`E:\Projects\Eve\docs\provider-advertisement-contract.md`.

Odin persists operator layout intent as `odin.interface_layout.v1` under `scratch/odin/interface-layout.json` for the current Starfire body. The durable CultMesh document should replace this local file once the layout schema is promoted. Layout intents name the provider id and request focus, move, resize, or visibility changes; renderers are input devices for those intents, not local layout owners.

Odin now derives dense top-level layout intent from each provider's retained
`surface.root` tree: element count, leaf count, branch count, depth, text-cell
pressure, and list-like branches. Provider explicit preferred sizes are capped
at the Odin wrapper boundary unless the current intent is fullscreen, so stale
layout files cannot keep empty panels huge. The renderer should use
`props.tree`, `props.layout.signalWeight`, and `props.packing` to allocate space
to nested signal, then recursively lower provider children. Flattening a
provider surface into one log/list is a compatibility failure when the retained
tree has children.

## Current Observation Surface

Odin tails Mimir's normalized observation JSONL at `E:\Projects\Mimir\artifacts\runtime\periwinkle-cultmesh-sensors.out.log` by default. It accepts the typed CultMesh records `mimir.eve_sensor_observation.v1` and `mimir.eve_media_observation.v1`, keeps only the latest record for each `(deviceId, streamId, kind)`, and publishes those summaries as provider-catalog observation records. Streams are considered active for 120 seconds by default so a compact dashboard refresh does not turn a briefly quiet sensor into false red noise.

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
