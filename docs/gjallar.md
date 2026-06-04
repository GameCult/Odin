# Gjallar

Gjallar is the herald organ for Odin.

Odin is the all-seer: it accepts Verse discovery, schema catalogs, translation
routes, provider surfaces, and observation projections. Gjallar is the
transmission daemon that turns that sight into agent-ready context: a fractal
tapestry of affordances across every Verse this Odin can currently see.

Gjallar should make the next capable agent less blind. It should not become a
second Odin.

## Authority Map

- Owner: Gjallar owns agent-context transmission of Odin-owned sight.
- Inputs: Odin's accepted registry, schema index, translation map, provider
  interfaces, observation-stream summaries, layout hints, and provenance
  metadata.
- Outputs: compact affordance packets for Codex, native CultMesh agents, repo
  Faces, Bifrost, and other agent consumers; optional Persona-facing memory
  summaries that name what Odin can see and what can be acted on.
- Derived state: summaries, prompts, Persona projections, Bifrost speech
  requests, Discord lowerings, and handoff notes are notification-only
  projections of Odin state.
- Forbidden writers: Gjallar does not probe hosts, decide Verse truth, mutate
  provider-owned dashboards, own renderer layout, or invent schema translation
  routes.
- Shared paths: human dashboards, Codex context assembly, native CultMesh agent
  bootstrap, Bifrost-routed Persona speech, and compatibility MCP access should
  consume the same Odin-owned records before Gjallar speaks.
- Deletion line: any Gjallar path that starts storing independent discovery
  truth should be cut or demoted to a cache that names Odin as source.

## Body

The initial local package is deliberately small:

- `personas/gjallar.persona_state.cc` is the canonical repo-local Persona
  state: a CultCache MessagePack store containing a `gamecult.persona_state.v0`
  document keyed as `persona:gjallar`.
- `scripts/write-gjallar-persona-cc.mjs` mints and inspects that store from the
  canonical EpiphanyAgent schema. The script is a local writer, not Gjallar's
  runtime body.
- `src/Gjallar/Gjallar.csproj` is the first C# CultMesh organ. It opens the
  Persona `.cc` store through `CultMesh.CreateNodeAsync`; typed Persona decode
  waits for a generated C# `gamecult.persona_state.v0` model.
- `assets/personas/gjallar-avatar.png` is the local avatar asset.
- `assets/personas/gjallar-avatar-pixel-256.png` is the 256px pixel-art avatar
  variant for compact Persona surfaces.
- This document is the organ contract until a runtime daemon exists.

When Gjallar becomes executable, package it as its own C# CultMesh runtime
entrypoint beside Odin rather than folding it into `src/odin-coordinator.cjs`.
The coordinator already owns lifecycle, refresh, persistence, and transport for
Odin. Gjallar's entrypoint should own context publication and no more.

## Persona Registration

VoidBot's current native Persona reader expects a JSON `personaStatePath`. That
is a compatibility projection boundary, not the state owner and not the future
transport owner. A future registry entry should point at a generated projection
or learn to read the CultCache record; the canonical source remains:

```text
id: gjallar
repoName: Odin
displayName: Gjallar
repoPath: E:\Projects\Odin
avatarPath: E:\Projects\Odin\assets\personas\gjallar-avatar.png
pixelAvatarPath: E:\Projects\Odin\assets\personas\gjallar-avatar-pixel-256.png
personaStateStore: E:\Projects\Odin\personas\gjallar.persona_state.cc
personaStateKey: persona:gjallar
personaStateSchema: gamecult.persona_state.v0
```

That compatibility registration belongs in the current Discord adapter, not
here. This repo owns the Persona source record and avatar. Bifrost should own
the CultMesh speech crossing when a Persona interpreter decides it wants to
speak; Discord is then a Bifrost lowering/receipt path, with VoidBot acting only
as legacy room cognition and delivery machinery until the native bridge is
complete.

## Transport Boundary

Persona speech is a bridge crossing. The interpreter may decide that a Face
should speak, but the public transport request should be a Bifrost CultMesh
command or document, not a VoidBot-owned side effect.

Target shape:

```text
Persona runtime / interpreter
  -> Bifrost speech request through CultMesh
  -> Bifrost policy, Heimdall claims, topic/work/context linkage
  -> Discord, GitHub, owner DM, or future public surface lowering
  -> Bifrost receipt
```

VoidBot still owns Discord observation, room cognition, moderation judgment, and
compatibility delivery while that bridge is being moved. Its repo search,
Discord history search, and archive/source retrieval should become native
CultCache/CultMesh services. MCP remains a bridge for external agentic access,
not the native affordance path for GameCult agents that can speak CultMesh.

## CultMesh Runtime Direction

Gjallar's runtime body should be C# over `GameCult.Mesh`:

```csharp
using GameCult.Mesh;

using var node = await CultMesh.CreateNodeAsync("personas/gjallar.persona_state.cc");
```

The current C# entrypoint opens the CultMesh node without pulling typed payloads
because the C# document model for `gamecult.persona_state.v0` has not been
generated yet. The next durable act should generate that model, read the
`persona:gjallar` record from the local CultCache, and publish an affordance
packet document through CultMesh. Do not make a JSON sidecar the source of truth
while waiting for that runtime.

## Invariants

- Gjallar is a herald, not a second source of truth.
- Odin remains the accepted owner of all-seer state.
- Verse owners remain owners of their local schemas, command boundaries, and
  provider surfaces.
- Agent consumers receive provenance: what Odin saw, where it came from, and
  whether it is authoritative, stale, predicted, denied, or unavailable.
- If Gjallar cannot name the Odin record behind a claim, the claim is not ready
  for durable transmission.

## First Affordance Packet Shape

Keep the first packet plain until the schema earns promotion:

```text
source: Odin state document or Eve/CultUI provider record
verse: Verse or provider id
surface: service, interface, observation-stream, schema, or translation route
affordance: what an agent can inspect, request, invoke, or lower
authority: owner of the real decision
status: authoritative, stale, predicted, denied, unavailable, or unknown
provenance: endpoint, document id, timestamp, and version when available
```

No regex tribunal for meaning. If natural-language interpretation is needed,
use a capable reader and keep the accepted packet inspectable.
