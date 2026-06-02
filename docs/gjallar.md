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
- Outputs: compact affordance packets for Codex, VoidBot, repo Faces, and other
  agent consumers; optional Persona-facing memory summaries that name what Odin
  can see and what can be acted on.
- Derived state: summaries, prompts, Persona projections, Discord speech, and
  handoff notes are notification-only projections of Odin state.
- Forbidden writers: Gjallar does not probe hosts, decide Verse truth, mutate
  provider-owned dashboards, own renderer layout, or invent schema translation
  routes.
- Shared paths: human dashboards, Codex context assembly, VoidBot repo-Face
  heartbeats, and future agent bootstrap should consume the same Odin-owned
  records before Gjallar speaks.
- Deletion line: any Gjallar path that starts storing independent discovery
  truth should be cut or demoted to a cache that names Odin as source.

## Body

The initial local package is deliberately small:

- `personas/gjallar.persona_state.json` is the repo-local Persona seed.
- `assets/personas/gjallar-avatar.png` is the local avatar asset.
- This document is the organ contract until a runtime daemon exists.

When Gjallar becomes executable, package it as its own runtime entrypoint beside
Odin rather than folding it into `src/odin-coordinator.cjs`. The coordinator
already owns lifecycle, refresh, persistence, and transport for Odin. Gjallar's
entrypoint should own context publication and no more.

## Persona Registration

VoidBot native Persona registration expects a repo identity with
`identityKind: "native_persona"` and a `personaStatePath`. A future VoidBot
registry entry should point at:

```text
id: gjallar
repoName: Odin
displayName: Gjallar
repoPath: E:\Projects\Odin
avatarPath: E:\Projects\Odin\assets\personas\gjallar-avatar.png
personaStatePath: E:\Projects\Odin\personas\gjallar.persona_state.json
```

That registration belongs in VoidBot's registry, not here. This repo owns the
Persona source document and avatar; VoidBot owns the speech transport and room
identity wiring.

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
