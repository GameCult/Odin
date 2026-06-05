# Gjallar

Gjallar is the Nightwing-resident terminal compositor for Odin's domain.

Odin is the all-seer: it accepts Verse discovery, schema catalogs, translation
routes, provider surfaces, and observation projections. Gjallar talks to Odin,
enumerates the active provider surfaces Odin can show, and composes those
surfaces into the live multi-scale dashboard running on Nightwing.

Gjallar exists so Odin does not need to care how a pile of provider-owned TUIs
fits on one fast display, and provider daemons do not need to know the terminal
body they are being lowered into.

## Authority Map

- Owner: Gjallar owns Nightwing dashboard composition, tiling, marquee, visual
  density, framebuffer presentation, and character-level refresh behavior.
- Inputs: Odin's Eve/CultUI deck endpoint, provider ids, provider-owned surface
  graphs, display constraints, font choices, and operator runtime flags.
- Outputs: the visible Nightwing framebuffer and compact Gjallar frame/status
  telemetry.
- Derived state: panel packing, visual weight, tile position, gutter cells,
  marquee tape, glyph size, and frame timing are derived from Odin/provider
  surfaces plus display constraints.
- Forbidden writers: Gjallar does not probe hosts, accept Verse truth, mutate
  provider-owned dashboards, invent schema translation routes, or replace Odin's
  provider registry.
- Shared paths: Nightwing's physical display, local frame dumps, future compact
  overlays, and agent-facing TUI captures should all lower the same Gjallar
  composition behavior when they want the "everything Odin can show" view.
- Deletion line: the old Rust `gjallar.overview` feed is not a runtime
  authority. Any path that wants Nightwing composition belongs in
  `E:\Projects\Gjallar`; any path that decides discovery truth belongs in Odin.

## Body

- `E:\Projects\Gjallar` is Gjallar's executable C# runtime repo.
- `Gjallar.csproj` builds the Nightwing framebuffer compositor.
- Gjallar consumes Odin's provider catalog over HTTP/WebSocket, defaulting to
  `ws://192.168.1.66:8797/eve/deck` with no fixed provider id.
- The old Rust `crates/gjallar-daemon` and `gjallar.overview` records were cut
  because they created an intermediate composition owner that did nothing Odin
  and Gjallar's live renderer could not explain directly.
- `assets/personas/gjallar-avatar.png` and
  `assets/personas/gjallar-avatar-pixel-256.png` remain branding assets for the
  view/persona surface.

Build from the repo root:

```powershell
dotnet build E:\Projects\Gjallar\src\Gjallar\Gjallar.csproj
```

Publish for Nightwing:

```powershell
dotnet publish E:\Projects\Gjallar\src\Gjallar\Gjallar.csproj -c Release -r linux-x64 --self-contained true -o E:\Projects\Gjallar\scratch\publish\gjallar
```

## Runtime Contract

```text
provider-owned Eve/CultUI surfaces
  -> Odin discovery and provider catalog/proxy surfaces
  -> Gjallar provider enumeration, packing, marquee, and framebuffer lowering
  -> Nightwing visible display
```

Nightwing is the host/body. Gjallar is the product that runs there. Odin owns
the accepted discovery/provider view. Each daemon owns its own surface truth.

## Invariants

- Odin remains the accepted owner of all-seer discovery and provider indexing.
- Gjallar owns display composition, not discovery truth.
- Provider surfaces are lowered, not rewritten into status summaries.
- Missing or invalid provider surfaces disappear or render as unavailable; they
  do not become invented truth.
- Frame/status telemetry observes Gjallar's rendering behavior only.
