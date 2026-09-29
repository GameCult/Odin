# Odin Architecture

## Objective

Odin is the central all-seer node for GameCult's CultMesh world: every Verse can discover every other Verse, learn which schemas they speak, and ask for a translation route when their local document shape differs.

## Current Mechanism

```text
Eve/CultUI provider surfaces
  + provider advertisements
  -> Odin observation cycle
  -> Verse-owned service records
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
  -> Gjallar Yggdrasil aggregate Eve composition
  -> compact display feeds
```

The first Rust core lives in `crates/odin-core`:

- `documents.rs`: typed Odin records and the CultMesh document set.
- `ports.rs`: narrow ingest traits plus clock injection for deterministic tests.
- `pipeline.rs`: collection and normalization from input observations to typed
  Odin records.
- `repository.rs`: `OdinRepository` abstraction, in-memory mock repository, and
  CultMesh-backed repository.

The deployed body is `crates/odin-daemon`, built and run by Idunn from
`deployment/idunn/recipe.toml`. `crates/odin-core` holds the typed record spine
that the rest of the machine grows into.

## Runtime Body

Odin's executable body is split by ownership:

- `crates/odin-daemon`: the deployed process. Reads Idunn's read-only topology
  projection, admits provider presences over RUDP for the incarnations Idunn
  projects, persists them through CultMesh/CultCache, publishes signed
  runtime-topology correlation records, and answers CultNet/RUDP snapshot and
  document requests. Deployment, restart, and survival of this process are
  Idunn's.
- `crates/odin-core`: typed Odin documents, ingest ports, normalization, and
  CultMesh/CultCache repository boundaries.

If a new owner is needed, name the owner and its invariant before adding code.

The accepted document registry is also the persistence admission boundary.
Unknown schemas may be observed for diagnostics but are not silently persisted
as untyped truth. Heimdall access discovery therefore registers
`gamecult.eve.provider_advertisement.v1`,
`heimdall.command_boundary.v1`,
`gamecult.eve.plugin_advertisement.v1`, and
`heimdall.transport_profile.v1`. Their globally unique record keys prevent one
schema from overwriting another in the shared catalog. Odin returns the
redacted route metadata; it does not proxy private auth operations or own any
claim, completion, token, or app secret.

Gjallar is the Yggdrasil-resident aggregate compositor for what Odin can show.
Its runtime lives in `/srv/gjallar/current` and consumes Odin's accepted
provider-state snapshot. Gjallar owns enumeration, tiling, and publication of
the typed `gjallar.overview` Eve surface. Eve clients own graphical, terminal,
and framebuffer lowering. Gjallar must not own the underlying registry, probe,
provider truth, client pixels, or translation decisions.

Idunn (`GameCult/Idunn`) owns deployment and daemon survival, including Odin's.
It builds from each target's recipe, activates and restarts the admitted
incarnation, and holds the deployment and lifecycle brakes. Odin publishes what
it can see and reads Idunn's topology projection; it does not start, restart,
deploy, or health-probe any daemon, and no Odin code path keeps a process alive.
When human action is needed, Idunn uses CultMesh to request a Bifrost-owned
operator notification crossing. Idunn must not own Verse discovery, schema
truth, provider dashboards, identity grants, Discord delivery, owner-DM
delivery, or renderer layout.

Muninn is the portable local telemetry Verse assembler. Its Rust body lives in
the `GameCult/Muninn` repository and publishes `muninn.telemetry_surface.v1`
through CultMesh/CultCache. Muninn may run on Raven, Nightwing, Starfire, or any future
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
- The Eve surface carries explicit `verse` and `service` nodes plus provider-owned retained interface trees. Compact renderers may derive visual facets from those nodes, but may not invent observation truth.
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

## Service Surface

Odin's service records come from provider-owned presences and advertisements
received through CultMesh/RUDP, not from host probes.

Gjallar consumes Odin's accepted CultMesh/Eve state and composes every visible
provider surface into one typed aggregate on Yggdrasil. EveCanvas, browser, and
TUI clients lower that aggregate. If Odin starts deciding tile composition, the
composition owner has leaked upward. If providers tune themselves for one
client instead of emitting clean Eve/CultUI surfaces, provider truth has leaked
downward.

## Current Interface Surface

Odin discovers provider advertisements and interface bindings from daemon-owned
CultMesh/CultCache stores plus live CultNet/RUDP announcements. Providers own
their compositions; Odin embeds each provider's `surface.root` as an
`interface` child with provenance, version, status, source witness, and layout
metadata.

This is the model for future services: if a service publishes an operator interface, ingest the Eve/CultUI composition graph and lower it. Do not collapse it into a service-status tile unless the graph is unavailable and the tile is explicitly a temporary probe.

The expected provider output is Eve DSL or an equivalent
`gamecult.eve.surface.v1` retained tree. GUI and TUI are lowerings of the same
interactive language; they are not separate dashboard products. Huginn's `.cc`
inspection surface is the current clean example: Huginn inspects CultCache bytes
and emits Eve DSL, while Eve or any other runtime owns presentation.

Provider advertisements are the promotion path out of probing. Odin's document
registry already accepts `gamecult.eve.provider_advertisement.v1` alongside
`gamecult.eve.interface_binding.v1` and `gamecult.eve.surface_state.v1`.
Daemons should publish advertisements that name service id, Verse id, schema
catalog, `.cc` witnesses, Eve surface keys, command boundaries, nested Verses,
style capabilities, freshness, and redaction policy. Once an advertisement is
available, Odin should prefer it over LAN scans, hardcoded deck URLs, private
layout files, host probes, or web-dashboard scraping. Daemon health and
provider state should publish through `cultnet.transport.rudp.v0`;
product/debug routes are outside-Odin lowerings only and must not become
daemon-owned truth.

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
metadata for resolving those names. Native daemon transport is CultNet over the
shared RUDP profile; renderer URLs belong outside Odin and are not service
identity.

The canonical contract lives in
`E:\Projects\Eve\docs\provider-advertisement-contract.md`.

## Observation Surface

Odin no longer tails Mimir JSONL artifacts for observation truth. Device and
media observations must arrive as provider-owned CultMesh/Odin records or
retained Eve/CultUI surfaces; Odin may project those accepted records, but it
does not scrape artifact logs to synthesize observation streams.

## First Translation Model

Odin's translation registry should start as data, not magic:

- `sourceSchema`
- `targetSchema`
- `translationKind`: `identity`, `projection`, `lossyProjection`, `adapter`, or `unsupported`
- `owner`
- `version`
- `notes`

No regex tribunals for meaning. Natural-language schema interpretation can be assisted by models later, but accepted translation routes must be inspectable typed state.
