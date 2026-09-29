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
the Yggdrasil-resident C# composition daemon in `F:\Projects\Gjallar`. It
consumes Odin's accepted provider snapshot and publishes one tiled
`gjallar.overview` Eve surface for GUI, TUI, and agent lowerers.

## Typed access discovery

Odin persists only document schemas it has registered. That is a deliberate
typed-store boundary, not a license to accept anonymous blobs and hope later
code understands them. The live Yggdrasil body at
`ba76a7239a7bb40aa1774df7d93b9388dc27b222`, using CultLib
`75c180782aeba7cfd22d6412877397708a4ed28f`, registers Heimdall's provider,
command-boundary, Eve-plugin, and transport-profile documents plus
Ghostlight's public catalog envelope. Provider daemons own those documents;
Odin owns typed catalog acceptance, ordered durable persistence, and exact
retrieval. Sensitive auth commands and claims travel directly between the
consuming app and Heimdall, never through Odin.

Ghostlight publishes a `ghostlight.schema_catalog.v1` envelope containing only
the state contracts named by its public provider surface. Odin registers that
envelope so it can persist and return the provider-owned catalog; it does not
register every private Ghostlight receipt, transition, or simulation schema.

## Typed access discovery

Odin persists only document schemas it has registered. That is a deliberate
typed-store boundary, not a license to accept anonymous blobs and hope later
code understands them. The live Yggdrasil body at
`b4f9a2e95f0b41cebdeddc49223781d1d3c7b42a` registers Heimdall's provider,
command-boundary, Eve-plugin, and transport-profile documents. Heimdall owns
their contents; Odin owns catalog acceptance and exact retrieval. Sensitive
auth commands and claims travel directly between the consuming app and
Heimdall, never through Odin.

## Gjallar

Gjallar is the herald composition daemon that runs beside Odin and Idunn on
Yggdrasil. Odin sees the Verses and accepts provider surfaces. Gjallar consumes
that accepted snapshot and publishes one multi-scale tiled Eve surface. Eve
clients—including EveCanvas—own graphical and TUI lowering.

Local package surfaces:

- Organ contract: `docs/gjallar.md`
- Branding Persona state: `personas/gjallar.persona_state.cc`
- Runtime source: `F:\Projects\Gjallar`
- Avatar asset: `assets/personas/gjallar-avatar.png`
- Pixel avatar: `assets/personas/gjallar-avatar-pixel-256.png`

## Idunn

Idunn (`GameCult/Idunn`) owns deployment and daemon survival for every GameCult
target, Odin included: it builds from the target's recipe, activates and
restarts the admitted incarnation, and holds the deployment and lifecycle
brakes. Odin owns none of that. Odin does discovery, schema awareness,
rendezvous, and interface aggregation, and it reads Idunn's topology projection
to learn which incarnations exist. Agents do not deploy daemons directly: they
configure the target's recipe and binding and let Idunn run the transaction.

Local package surfaces:

- Deployment recipe: `deployment/idunn/recipe.toml`
- Operator binding: `/etc/gamecult/idunn/bindings/odin.toml` on the host, not in
  this repository
- Live deployment and admission map: `state/map.yaml`
- Idunn's contract, brakes, and runbooks: the `GameCult/Idunn` repository

## Authority Map

- Owner: Odin owns the network-wide Verse registry, schema catalog index, translation map, and accepted provider catalog/proxy surfaces.
- Inputs: CultMesh/CultNet peer announcements, schema catalog responses, daemon
  health/provider publications over `cultnet.transport.rudp.v0`, Idunn's
  read-only topology projection, and provider-owned Eve/CultUI surfaces.
- Outputs: CultCache-backed Odin state, CultMesh documents, and CultNet
  schema/catalog messages. Browser, GUI, TUI, and framebuffer renderers lower
  those documents outside Odin instead of asking Odin to host web surfaces.
- Derived state: Gjallar's aggregate surface is derived from Odin/provider state; Eve clients derive pixels from that aggregate.
- Forbidden writers: renderers do not probe the network or decide Verse truth; individual projects do not maintain private incompatible discovery ledgers once Odin can see them.
- Shared paths: human dashboards, worker schedulers, Verse bootstrap code, and compact TUI views consume the same registry and schema catalog.
- Not Odin's: process lifecycle, restart, deployment, and daemon survival belong to Idunn.

## Runtime Body

Odin runs as `odin-daemon` under Idunn on Yggdrasil, built from
`deployment/idunn/recipe.toml`. There is no local start script. The daemon:

- reads Idunn's read-only projection to learn which incarnations of which
  targets exist;
- admits provider presences over RUDP for those incarnations and persists them
  through CultMesh/CultCache;
- publishes signed runtime-topology correlation records;
- answers CultNet/RUDP snapshot and document requests on its route.

Odin's native document catalog is addressed by CultMesh URI. Concrete RUDP
bootstrap is configured behind CultMesh URI resolution by the operator or by
Odin/Idunn deployment state:

```text
cultmesh://odin/rendezvous/provider-catalog
```

That URI accepts typed document publication and schema/catalog requests through
the shared CultMesh runtime. Consumers that need Odin's accepted surface can
still request the current CultNet snapshot after CultMesh resolves the transport.

Browser and deck lowerers consume Odin's CultMesh state through their own
lowering process. Odin does not host browser-deck surfaces or publish deck URLs
as discovery seed material.

Provider advertisements and CultNet/RUDP transport profiles are the discovery
path. External host probes, product health checks, port probes, and renderer
bridges are debug or lowering surfaces outside Odin only.
