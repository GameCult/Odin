# Gjallar

Gjallar is the overview composition daemon for Odin.

Odin is the all-seer: it accepts Verse discovery, schema catalogs, translation
routes, provider surfaces, and observation projections. Gjallar reads that
Odin-owned state and composes the dense overview feed that Nightwing displays.
Nightwing should not run a bespoke dashboard brain; it should ask Gjallar what
to draw and then lower that feed into fast terminal cells.

Gjallar exists so Odin does not need to care how a pile of dashboards fits on
one screen, and Nightwing does not need to know how Odin's world model is built.

## Authority Map

- Owner: Gjallar owns overview composition for Odin sight.
- Inputs: Odin's accepted snapshot, Verse records, service records, interface
  records, observation stream records, translation route records, and explicit
  operator/Nightwing display constraints.
- Outputs: typed `gjallar.overview.v1` and `gjallar.overview_tile.v1` records:
  overview metadata, tile ordering, density hints, source pointers, status,
  labels, and compact detail text for Nightwing.
- Derived state: tile priority, grouping, target rows/columns, row/column spans,
  visual emphasis hints, and compact summaries are derived from Odin state.
- Forbidden writers: Gjallar does not probe hosts, accept Verse truth, mutate
  provider-owned dashboards, own terminal glyph rendering, or invent schema
  translation routes.
- Shared paths: Nightwing TUI, browser previews, future compact overlays, and
  agent summaries consume the same Gjallar overview feed when they want the
  "everything Odin sees" view.
- Deletion line: any Nightwing path that independently composes Odin dashboards
  should move into Gjallar; any Gjallar path that decides discovery truth should
  move back into Odin.

## Body

- `crates/gjallar-daemon` is Gjallar's Rust CultMesh daemon.
- `crates/odin-core/src/gjallar.rs` owns the pure composition function so unit
  tests can exercise packing decisions without booting a daemon.
- `gjallar.overview.v1` describes the feed Nightwing asks for.
- `gjallar.overview_tile.v1` describes each drawable tile and names its Odin
  source record.
- `assets/personas/gjallar-avatar.png` and
  `assets/personas/gjallar-avatar-pixel-256.png` remain branding assets for the
  view/persona surface.

Run one composition pass from the repo root:

```powershell
cargo run -p gjallar-daemon --
```

By default, Gjallar reads Odin's local CultMesh store at
`scratch/odin/odin.ccmp` and writes its feed to
`scratch/gjallar/gjallar.overview.ccmp`.

## Runtime Contract

```text
Odin typed state
  -> Gjallar overview composition
  -> gjallar.overview / gjallar.overview_tile CultMesh records
  -> Nightwing terminal lowerer
```

Nightwing owns terminal speed, key handling, glyph selection, color lowering,
and screen refresh. Gjallar owns what should be on the overview and how densely
it should be packed. Odin owns the facts being summarized.

## Current Cut

The current Rust daemon has two input paths:

- Preferred: read Rust typed Odin records such as `odin.snapshot.v1`,
  `odin.service.v1`, and `odin.interface.v1` from an Odin CultMesh store.
- Compatibility: read Odin's current CommonJS
  `gamecult.eve.surface_state.v1` record as a raw MessagePack map, then wrap it
  as a typed Gjallar overview tile.

The compatibility path exists so Gjallar can talk to today's Odin without
pretending the migration is complete. The next improvement is for Odin to
publish a typed index/catalog document that Gjallar can read instead of carrying
configured key lists.

## Invariants

- Odin remains the accepted owner of all-seer state.
- Gjallar owns overview composition, not discovery truth.
- Nightwing owns terminal lowering, not dashboard composition.
- Every tile names the Odin source record behind it.
- Missing Odin records disappear from the feed rather than becoming invented
  truth.
