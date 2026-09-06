# Hermodr Extraction Plan

## Objective

Hermodr is the Eve-to-browser lowering bridge. It should subscribe to accepted
Verse state, render surfaces to browsers, carry operator intent back to the
providers that own the consequence, and — new — emit a surface as static bytes
for audiences that are not Verse participants.

It currently also runs its own provider ingress and its own acceptance store,
which makes it a second Odin. This plan cuts that authority, moves the daemon
into its own repository, and adds static lowering as a second output mode.

## Current mechanism

Hermodr lives in Odin as Node inside a Rust repository: `src/hermodr-daemon.cjs`
(1,462 lines), `src/hermodr-state-stream.cjs` (32), `test/hermodr-*.test.cjs`
(191), four PowerShell lifecycle scripts, and a persona avatar. Eve holds the
browser-side consumer at `web/hermodr-provider-catalog.mjs`.

It requires five modules from `src/odin/` totalling 1,042 lines.
`src/odin-coordinator.cjs` requires eight modules from that same directory, so
`src/odin/` is a shared Node layer, not Hermodr's private library.

### The intent path is already correct

`POST /hermodr/commands/eve` accepts a body, resolves a `cultmesh://` target
either from the body or from the provider catalog, wraps it as
`gamecult.eve.command.v1`, stamps `publishedBy: "hermodr-browser-lowering"`,
publishes it as a command document to the provider's advertised route, and
returns a receipt. It is generic across providers and it does not decide
outcomes. This path is kept as-is.

### The state path is the defect

`main()` constructs its own `OdinLivePublicationSource` — an in-process `Map`
carrying both the read side (`latest`, `watch`, `watchLifecycle`) and the write
side (`accept`, `withdraw`). It then opens its own provider-session ingress,
bound to its own address with its own session token, and feeds accepted
documents into that private store. `HermodrStateStreamRegistry` watches that
store and pushes lifecycle events to browsers.

So providers publish directly to Hermodr, Hermodr decides what is accepted, and
browsers see Hermodr's private view. Odin's coordinator holds a separate
instance of the same class over the same providers with no reconciliation. Which
view is correct depends on where a provider happened to bind.

A surface reaches a browser only if its provider binds to Hermodr. Hermodr is
therefore a required ingress wearing the name of a bridge.

### The loop does not close

The command path was added after the fact, on discovering the original bridge
was entirely non-interactive. It was added well: Eve's browser client passes
`commandSink: publishCommandIntent` into the renderer, controls dispatch through
it, and it refuses any response that is not a correlated
`gamecult.eve.command_receipt.v1` in state `reconciled`.

But it was added to one leg only, and the shape of the rest still assumes a
viewer. Intent leaves correctly, over CultMesh, to the provider that owns the
consequence. The consequence then returns as newly published provider state —
and that state reaches the browser only through Hermodr's private ingress. So a
provider must publish to Odin for the Verse to be correct and to Hermodr for the
operator to see their own action land.

This is the core design issue, and it is not that Hermodr is missing a write
path. Intent goes out the right door and consequence comes back through the
wrong one. Cutting the acceptance authority is what closes the loop: once the
state leg is a CultMesh subscription to Odin-accepted state, a command's effect
returns by the same route as every other change, and the round trip is
genuinely bidirectional rather than two half-connected paths.

The client already carries the seed of the static case. `publishCommandIntent`
checks `liveHermodr` and, when absent, logs the intent instead of posting it.
Static lowering should make that degradation explicit and visible rather than
silent, per the negative check below.

## Invariants

- Odin owns provider ingress, acceptance, and discovery. There is one accepted
  view of Verse state.
- Providers own consequence. A command is a request; the provider decides.
- Hermodr owns the browser edge: session, transport, lowering, and the relay of
  intent. It owns no accepted state.
- A lowering runtime cannot make something true by rendering it.
- Static output carries no back-channel, and must not pretend otherwise.

## Authority map

**Owner.** Odin owns provider-session ingress, live-publication acceptance and
withdrawal, and the provider catalog.

**Inputs to Hermodr.** Odin-accepted state over CultMesh; provider-advertised
surface documents; provider-advertised command routes; browser requests.

**Outputs.** Lowered surfaces to browsers; live state streams; typed
`gamecult.eve.command.v1` documents published to provider command routes;
receipts; and static artifacts.

**Derived, not owner.** Everything Hermodr holds about provider state becomes a
projection with a sequence number. Its staleness marks are display facts, not
withdrawal decisions.

**Forbidden writers.** Hermodr may not call `accept()` or `withdraw()`. It may
not bind a provider session. It may not author a command as itself; every
command carries the originating operator identity and is stamped as relayed.

**Shared paths.** Live lowering and static lowering must render from the same
surface graph through the same composition primitive. If the two diverge
visually, the static path is wrong, not the graph.

## Intended change

1. Hermodr subscribes to Odin-accepted state over CultMesh instead of running
   ingress. `HermodrStateStreamRegistry` survives untouched: it only consumes
   `source.forProvider(id).watchLifecycle(schemaId, recordKey, cb)`, which is
   already a narrow port. Replace what sits behind that port with a CultMesh
   subscription client and every stream, sequence number, and browser keeps
   working.
2. Extract the daemon to `GameCult/Hermodr`.
3. Add static lowering as a library with a CLI entry point.

## Cut line

Deleted from Hermodr:

- the `OdinLivePublicationSource` construction and every write into it
- `createProviderSessionIngress` and the `--provider-session-bind` and
  `--provider-session-token` options
- `src/odin/live-publication-source.cjs` and
  `src/odin/provider-session-ingress.cjs` as Hermodr dependencies; both stay in
  Odin, which is their real owner

Moved out of Odin:

- `src/odin/idunn-rudp.cjs` (324 lines) follows Idunn to `GameCult/Idunn` as a
  published health-client contract. Odin and Hermodr then both consume it from
  its owner rather than each carrying a copy.

Resolved, not copied:

- `src/odin/documents.cjs` (536 lines) survives as a decode contract. It must
  become a published shared package. Forking it creates two definitions of the
  same wire format, which is the failure this plan exists to remove.
- `src/odin/utils.cjs` (68 lines) is generic helpers. Copy the handful Hermodr
  uses, or fold them into the shared package. Do not extract a utility library.

## Subtraction budget

Hermodr loses its ingress, its acceptance store, and two of its five Odin
imports. Odin loses the Node daemon, its four lifecycle scripts, and
`idunn-rudp.cjs`. One shared document package appears. Net: one process's worth
of duplicated acceptance authority removed, one package added, one repository
added.

The static lowering library is the only genuinely new surface. It earns its keep
by making public storefronts hostable without a mesh, a runtime, or a daemon —
which is the difference between a founding creator being able to self-host and
not.

## Build budget

Node only; no Rust targets change. Odin's Rust crates are untouched. The new
repository is one package with a test target. Static lowering adds a CLI binary
to that same package, not a second package.

## Phases

**Phase 1 — Idunn RUDP client.** Move `src/odin/idunn-rudp.cjs` to the Idunn
repository and consume it from there. Independent of everything else, correct
regardless of whether Hermodr ever moves, and shrinks the surface first.

**Phase 2 — Cut the authority, in place.** Before moving any files. Replace the
private source with a CultMesh subscription behind the existing port, delete the
ingress, delete the options. Hermodr still lives in Odin at this point and must
still serve browsers correctly. If this phase cannot pass its checks, the
extraction is not ready.

**Phase 3 — Publish the document contract.** `documents.cjs` becomes a shared
package consumed by both Odin and Hermodr.

**Phase 4 — Extract.** Move the daemon, state stream, tests, and lifecycle
scripts to `GameCult/Hermodr`. MIT, matching the rest of the chain, so a
self-hosting creator may actually run it.

**Phase 5 — Static lowering.** Library plus CLI in the new repository. It must
be runnable without starting the daemon; requiring the daemon reintroduces the
dependency static lowering exists to remove.

## Verification

Positive:

- A surface published only to Odin reaches a browser through Hermodr. This
  fails today and is the point of the work.
- Live state streams keep their sequence numbers, and stale and reconnected
  transitions still fire across a provider restart.
- A browser command still reaches its provider and returns a receipt.
- Static output of a surface renders equivalently to the live lowering of the
  same graph.

Negative, and these are the ones that prove the authority moved:

- Hermodr cannot be made to accept a provider publication. The code path is
  gone, not merely unused.
- Starting Hermodr with the removed ingress options fails loudly rather than
  silently ignoring them.
- With Hermodr running and Odin stopped, browsers see stale state and no new
  acceptances. Hermodr must not paper over Odin's absence by accepting
  publications itself.
- Two Hermodr instances against one Odin show the same accepted state. Today
  they would not.
- Static output contains no command controls that appear operable. A dead button
  in a static page is a lie about a back-channel that does not exist.

## Open questions for the operator

1. **`documents.cjs` ownership.** Published from Odin, or promoted into CultLib
   beside the other typed contracts? It is a wire format two services share,
   which argues for CultLib, but Odin authors it.
2. **Ephemeral view state.** Expanded panels, unsent field contents, optimistic
   pending states. Client-side today by omission rather than decision. Leaving
   it client-side is defensible; it should be a decision.
3. **Static lowering and identity.** A live surface knows who is viewing. A
   static artifact does not. Either static surfaces are public-only, or the
   lowering takes an audience parameter and emits per-audience artifacts.
   Public-only is smaller and covers the storefront.
