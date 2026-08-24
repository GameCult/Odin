# Gjallar

Gjallar is the Yggdrasil-resident aggregate compositor for Odin's domain.

Odin accepts Verse discovery and provider-owned Eve surfaces. Gjallar consumes
that accepted snapshot and publishes one typed `gjallar.overview` composition
containing every currently visible provider surface. EveCanvas, browsers, TUI
clients, and agent readers lower the same aggregate without becoming its owner.

## Authority Map

- Owner: Gjallar owns aggregate membership, tiling, marquee placement, layout
  intent, visibility, and composition versioning.
- Inputs: Odin's accepted CultMesh/CultNet snapshot, provider-owned surface
  graphs, Odin's canonical marquee tape, and display-independent layout hints.
- Outputs: `gjallar.overview`, its CultCache/CultMesh/CultNet publication, and
  compact composition/freshness telemetry.
- Derived state: panel weights, ordering, tile layout, unavailable presentation,
  and aggregate version are derived from Odin/provider state.
- Forbidden writers: Gjallar does not probe hosts, accept discovery truth,
  mutate provider state, open `/dev/fb0` in daemon mode, capture client input,
  or decide client pixels. Eve clients do not decide aggregate membership.
- Shared paths: iOS/UIKit, browser, optional TUI/framebuffer, and agent capture
  lower the same published overview.
- Deletion line: Nightwing's `gjallar.service`, framebuffer lifecycle target,
  and `nightwing-gjallar` Idunn actuator are obsolete authorities and must not
  survive Yggdrasil admission.

## Body

- Runtime source: `F:\Projects\Gjallar` / `GameCult/Gjallar`.
- Daemon host: Yggdrasil beside `odin.service` and
  `idunn-yggdrasil.service`.
- Unit: `gjallar-yggdrasil.service`.
- Runtime mode: `--headless`; no framebuffer, font atlas, or mouse device is
  opened.
- Input: Yggdrasil-local Odin accepted snapshot over CultNet/RUDP.
- State: `/var/lib/gamecult/gjallar/gjallar.service.cc`.
- Deployment: immutable source/dependency pins, a digest-pinned .NET SDK image,
  and Idunn's root-owned `gjallar` actuator manifest.

## Runtime Contract

```text
provider-owned Eve surfaces
  -> Odin accepted discovery/provider snapshot
  -> Gjallar aggregation and tiling
  -> typed gjallar.overview Eve surface
  -> EveCanvas / browser / TUI / agent lowerers
```

## Invariants

- Odin owns discovery and provider acceptance.
- Providers own their surface truth and command consequences.
- Gjallar owns composition, not discovery or pixel rendering.
- Eve clients own rendering and local input capture.
- Missing or invalid provider surfaces become explicit unavailable state or are
  omitted; Gjallar never invents replacement truth.
- Daemon health follows accepted-snapshot and aggregate-publication freshness,
  not rendered frames.
