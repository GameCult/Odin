# Hermodr Extraction Plan

## Objective

Hermodr is the Eve-to-browser lowering bridge. It should resolve providers
through Odin's directory, subscribe to those providers for their state, render
their surfaces to browsers, carry operator intent back to the provider that owns
the consequence, and — new — emit a surface as static bytes for audiences that
are not Verse participants.

It currently runs its own provider ingress and its own state store instead. This
plan deletes that, moves the daemon into its own repository, and adds static
lowering as a second output mode.

## What Odin is

Odin is a rendezvous organ. It tells you what state is available, what schemas
speak it, and where to reach it. It does not tell you what the state is, and it
is not in the data path.

A provider owns its state and serves it. Odin owns the directory. A consumer
resolves through the directory and then talks to the provider.

Hermodr's command path already works this way. `findProviderCommandRoute` looks
a route up in the catalog and then publishes the command to the provider's own
advertised endpoint. Erycina's surface says the same thing from the other side:
`discoveryOwner: "Odin"`, while `canonicalSurfaceUri` points at
`cultmesh://asgard.starfire.erycina/eve/operator/surface` — Odin tells you where,
you go ask Erycina.

## Current mechanism

Hermodr lives in Odin as Node inside a Rust repository: `src/hermodr-daemon.cjs`
(1,462 lines), `src/hermodr-state-stream.cjs` (32), `test/hermodr-*.test.cjs`
(191), four PowerShell lifecycle scripts, and a persona avatar. Eve holds the
browser-side consumer at `web/hermodr-provider-catalog.mjs`.

### The intent path is already correct

`POST /hermodr/commands/eve` is generic across providers. It resolves a
`cultmesh://` target from the body or from the provider catalog, wraps intent as
`gamecult.eve.command.v1`, stamps `publishedBy: "hermodr-browser-lowering"`,
publishes to the provider's advertised route, and returns a receipt. Eve's client
passes `commandSink: publishCommandIntent` into the renderer and refuses any
response that is not a correlated `gamecult.eve.command_receipt.v1` in state
`reconciled`.

This path is kept unchanged. It is also the reference for what the state path
should look like.

### The state path is the defect

`main()` constructs an `OdinLivePublicationSource` — an in-process `Map` with a
read side (`latest`, `watch`, `watchLifecycle`) and a write side (`accept`,
`withdraw`). It opens its own provider-session ingress, bound to its own address
with its own session token, and feeds accepted documents into that private
store. `HermodrStateStreamRegistry` watches the store and pushes lifecycle
events to browsers.

So providers publish their state directly into Hermodr, and Hermodr decides what
it holds. A surface reaches a browser only if its provider binds to Hermodr,
which makes a required ingress out of something named a bridge.

The class is called `OdinLivePublicationSource` and lives in `src/odin/`. Odin
does not use it. Neither the coordinator nor any Odin test references it, and
the same is true of `provider-session-ingress.cjs`. This is Hermodr's own state
machinery wearing Odin's name and filed under Odin's shared modules, which is
why it reads as though Odin sanctioned it.

### The loop does not close

The command path was added after the fact, on discovering the original bridge
was entirely non-interactive. It was added well, but to one leg only, and the
rest still assumes a viewer.

Intent leaves correctly, to the provider that owns the consequence. The
consequence returns as newly published provider state — and that reaches the
browser only through Hermodr's private ingress. A provider must therefore
publish twice: once wherever the Verse expects it, and once into Hermodr, so the
operator can see their own action land.

The core issue is not a missing write path. Intent goes out the right door and
consequence comes back through the wrong one. Deleting the ingress is what
closes the loop: once state arrives by subscription to the provider, a command's
effect returns by the same route as every other change.

The client already carries the seed of the static case. `publishCommandIntent`
checks `liveHermodr` and, when absent, logs the intent instead of posting it.

## Invariants

- A provider owns its state and serves it. Nothing else holds it as truth.
- Odin owns the directory: what exists, what schema, where. Never the data path.
- Hermodr owns the browser edge: session, transport, lowering, relay of intent.
  It holds projections with sequence numbers, never truth.
- A lowering runtime cannot make something true by rendering it.
- Static output carries no back-channel and must not pretend otherwise.

## Authority map

**Owner.** Each provider owns its own state, its surface document, and its
command routes. Odin owns provider discovery, schema awareness, and route
advertisement.

**Inputs to Hermodr.** Odin's catalog, to resolve providers and routes. Provider
subscriptions, for state. Browser requests.

**Outputs.** Lowered surfaces; live state streams; typed
`gamecult.eve.command.v1` documents published to provider routes; receipts;
static artifacts.

**Derived, not owner.** Everything Hermodr holds about provider state is a
projection carrying a sequence number. Its staleness marks are display facts,
not withdrawal decisions.

**Forbidden writers.** Hermodr may not accept or withdraw a publication. It may
not bind a provider session. It may not author a command as itself. Odin may not
become a state relay; if a consumer can read state *from* Odin, the directory
has become the data path.

**Shared paths.** State resolution and command routing must use the same
catalog lookup. Live and static lowering must render from the same surface graph
through the same composition primitive; if they diverge visually, the static
path is wrong, not the graph.

## Intended change

1. Delete the ingress and the private store. Hermodr resolves a provider through
   the catalog, exactly as the command path already does, and subscribes to that
   provider for state.
2. `HermodrStateStreamRegistry` survives untouched. It consumes only
   `source.forProvider(id).watchLifecycle(schemaId, recordKey, cb)`, which is
   already a narrow port. Replace what sits behind that port with a provider
   subscription client and every stream, sequence number, and browser keeps
   working.
3. Extract the daemon to `GameCult/Hermodr`.
4. Add static lowering as a library with a CLI entry point.

## Cut line

Deleted outright:

- the `OdinLivePublicationSource` construction and every write into it
- `createProviderSessionIngress`, and the `--provider-session-bind` and
  `--provider-session-token` options

Leaves Odin with Hermodr, because Odin never used it:

- `src/odin/live-publication-source.cjs` and
  `src/odin/provider-session-ingress.cjs`, both referenced only by
  `hermodr-daemon.cjs` and `test/hermodr-live-publication.test.cjs`. They are
  misfiled Hermodr code, not shared Odin modules. If the subscription client
  fully replaces them, they are deleted rather than moved.
- `src/odin/utils.cjs` (68 lines), also Hermodr-only. Copy the handful of
  helpers actually used; do not extract a utility library.

Moved out of Odin, for its own reasons:

- `src/odin/idunn-rudp.cjs` (324 lines) follows Idunn to `GameCult/Idunn` as a
  published health-client contract. Genuinely shared — the coordinator and a
  health test use it too — so both consume it from its owner rather than each
  carrying a copy.

Resolved, not copied:

- `src/odin/documents.cjs` (536 lines) is the only true shared dependency, used
  by the coordinator, Hermodr, and two discovery tests. It survives as a decode
  contract and must become a published package. Forking it creates two
  definitions of one wire format, which is the failure this plan exists to
  remove.

## Subtraction budget

Hermodr loses its ingress and its state store. Odin loses the Node daemon, four
lifecycle scripts, three misfiled Hermodr modules, and `idunn-rudp.cjs`. One
shared document package appears. Net: a duplicate acceptance authority deleted,
`src/odin/` reduced from fourteen modules to ten, one package added, one
repository added.

The static lowering library is the only genuinely new surface. It earns its keep
by making public storefronts hostable without a mesh, a runtime, or a daemon —
the difference between a founding creator being able to self-host and not.

## Build budget

Node only; no Rust targets change. Odin's crates are untouched. The new
repository is one package with a test target, and static lowering adds a CLI
entry to that same package rather than a second package.

## Phases

**Phase 1 — Idunn RUDP client. Done, as a split.** The transport moved to
CultLib as `cultnet-ts`'s `signed-daemon-health`; Idunn now documents its
connection id and schema names; Odin keeps a 65-line adapter that declares that
contract. The whole-file move originally specified was not carried out, for the
reasons below.

**Phase 2 — Delete the ingress, in place. Done.** `ProviderSubscriptionSource`
replaces the private store behind the existing port, the ingress and its options
are gone, and `src/odin/` is down to twelve modules. Verified by unit tests;
behaviour against a live provider over RUDP remains an operator step.

**Phase 3 — Publish the document contract. Done.** The 45 definitions are
`cultcache-ts`'s `defineSwarmDocuments`, verified byte-identical to what
`documents.cjs` produced. Odin keeps a 26-line adapter under the name its
consumers already call. `utils.cjs` left with Hermodr, which used one of its six
functions.

**Phase 4 — Extract. Done.** `GameCult/Hermodr`, MIT, sixteen files. The three
dependencies on Odin were resolved rather than carried: the catalog comes from
CultLib, Idunn's health contract is declared locally over the CultLib transport
with `sourceRuntimeId` `hermodr`, and `parseArgs` is seventeen lines of
`args.cjs`. `src/odin/` is down from fourteen modules to eleven, and Odin's
CommonJS surface from sixteen files to twelve.

**Phase 5 — Static lowering. Not started, and no longer this repository's
business.** Library plus CLI in `GameCult/Hermodr`, runnable without starting
the daemon. Requiring the daemon reintroduces the dependency static lowering
exists to remove.

This document stays here as the record of what left Odin and why. The work it
describes now happens in Hermodr.

## Verification

Positive:

- A provider that publishes only to its own advertised address reaches a browser
  through Hermodr. This fails today and is the point of the work.
- Live state streams keep their sequence numbers, and stale and reconnected
  transitions still fire across a provider restart.
- A browser command still reaches its provider and returns a correlated receipt.
- Static output of a surface renders equivalently to the live lowering of the
  same graph.

Negative, and these are the ones that prove the authority moved:

- Hermodr cannot be made to accept a provider publication. The code path is
  gone, not merely unused.
- Starting Hermodr with the removed ingress options fails loudly rather than
  silently ignoring them.
- With Odin stopped, Hermodr cannot resolve providers it has not already
  resolved, but existing provider subscriptions keep delivering. If stopping
  Odin stops live state, Odin is in the data path and this plan failed.
- Nothing reads state *from* Odin. A consumer able to do so proves the directory
  became a relay.
- Two Hermodr instances against the same provider show the same state.
- Static output contains no command controls that appear operable. A dead button
  in a static page is a lie about a back-channel that does not exist.

## Why Phase 1 was split rather than moved

Reading `idunn-rudp.cjs` before moving it changed the answer. Of its 324 lines,
13 mention Idunn, and most of those are error message strings. The genuinely
Idunn-owned content is two constants: the connection ID `0x1d0d0001` and the
schema name `idunn.signed_daemon_health.v1`. Everything else — Ed25519 signing,
RUDP session management, packet receipt, endpoint parsing, msgpack framing — is
generic CultNet client plumbing.

Moving the file to Idunn would put 311 lines of transport code into a Rust
repository with no Node packaging, and would give Odin a cross-repo filesystem
dependency on a sibling checkout. The file already resolves CultLib by walking
`__dirname/../../../CultLib`, which works only while Odin sits beside CultLib;
Erycina had the identical bug and it broke the moment that repository moved.
Repeating it deliberately, in the plan whose purpose is removing misplaced
ownership, would be a poor trade.

It also buys nothing yet. The duplication it prevents does not exist until
Hermodr leaves in Phase 4, because both consumers still live in this repository.

Three options, in the order I would rank them:

1. **Generic to CultLib, constants to Idunn.** The transport belongs beside
   `cultnet-ts`, which Odin already resolves into and Hermodr will too. Idunn
   owns the connection ID and the schema name. This is the real fix, and it
   removes the sibling-path bug rather than relocating it.
2. **Defer to Phase 4.** Correct and free. Move it when the duplication actually
   threatens, and decide the shape then.
3. **Move it whole to Idunn.** As originally written. Cheapest to type, and it
   puts transport code where it does not belong while adding a fragile path.

Option 1 was chosen and carried out. `idunn-rudp.cjs` fell from 324 lines to 65:
Idunn's three constants and the call shapes Odin's consumers already use.
`packages/cultnet-ts/src/signed-daemon-health.ts` holds the transport, taking
the contract as a parameter. Idunn's
`docs/signed-daemon-health-authority.md` gained the wire-contract table it was
missing — the connection id had existed only in this consumer's code.

Both the moved module and the adapter dropped the sibling-path assumption on the
way, honouring `CULTLIB_ROOT` instead of walking up three directories from
wherever the file happens to sit.

Phase 3's remaining decision is unchanged: `documents.cjs` is now the only
genuinely shared dependency left between Odin and a future Hermodr.

## Open questions for the operator

1. **`documents.cjs` ownership.** Published from Odin, or promoted into CultLib
   beside the other typed contracts? It is a wire format several services share,
   which argues for CultLib, but Odin authors it.
2. **Ephemeral view state.** Expanded panels, unsent field contents, optimistic
   pending states. Client-side today by omission rather than decision.
3. **Static lowering and identity.** A live surface knows who is viewing; a
   static artifact does not. Either static surfaces are public-only, or the
   lowering takes an audience parameter and emits per-audience artifacts.
   Public-only is smaller and covers the storefront.
