# Idunn

Idunn is GameCult's independent deployment-admission and continuity control
plane. It decides which exact workload incarnation may run, write canonical
state, receive traffic, and be kept alive. It does not depend on Odin or any
managed workload in order to start or recover its own durable admitted state.

Systemd, container runtimes, proxies, and a future Nomad or Kubernetes driver
provide generic execution and routing. Idunn owns the GameCult-specific
decision about which source, release, runtime identity, process write lease, and
route membership are authoritative.

The detailed contract is
[`idunn-deployment-authority.md`](idunn-deployment-authority.md).

## Authority map

- **Owner:** Idunn selects exact source facts under root policy, compiles the
  repository declaration against an operator binding, seals materialized
  inputs and artifacts, admits one runtime incarnation, separately controls its
  process-bound write lease and route membership, and preserves its continuity.
- **Inputs:** the exact raw repository declaration and operator-binding blobs;
  private source-driver facts; artifact and external-input materialization
  receipts; separate deployment and lifecycle brakes; Odin's observed semantic
  topology when a graph-changing decision needs discovery; signed runtime
  presence and health; Idunn's current observed-activation projection; and
  observations from narrow execution and route drivers.
- **Outputs:** private compiled plans and sealed releases; a release-bound
  sanitized Expected projection for Odin/CultMesh; one launch-scoped runtime
  activation published only after driver observation; process write-lease and
  route decisions; continuity actions; and explicit disagreement or failure
  records.
- **Derived state:** dashboards, operator summaries, proxy displays, process
  listings, and Odin correlation views are projections. They do not establish
  deployment, write, route, presence, or readiness authority.
- **Forbidden writers:** repositories cannot grant host privileges, identities,
  trust anchors, state roots, write leases, routes, placement, or rollout
  policy. Odin cannot deploy, promote, route, or grant writes. Services cannot
  configure themselves Present or Ready. Proxies and process managers cannot
  select the admitted incarnation. Raw command strings are not deployment
  authority.
- **Shared path:** `idunn up`, scheduled deployment, operator deployment,
  restart, crash recovery, and host reboot recovery all use the same admitted
  source-to-release identity. Continuity may restart only that identity; it
  cannot fetch, rebuild, migrate, or silently change configuration.

## Repository declaration

A deployable repository publishes one strict declaration, normally
`deployment/idunn/recipe.toml`. It may contain:

- literal build, test, and package argv for named runner classes;
- generic pinned HTTPS inputs with SHA-256, runner, and relative destination;
- immutable artifact outputs, optionally pinned to an expected SHA-256;
- a constrained launch contract;
- one health-contract identity;
- an optional typed state contract with state schema generation and slots;
- provided capabilities, dependency kinds, startup requirements, capacity, and
  conflicts.

The recipe does not configure runtime health states. GameCult runtime health
uses the shared `warming`, `active`, `degraded`, and `failed` ontology. Present
and Ready are Odin correlations over independently signed runtime evidence, not
recipe fields.

Stateless services declare no state block. Their operator binding therefore has
no state root, state-transition policy, or process write-lease record.

## Operator binding

The root-admitted operator binding supplies the physical policy a repository
cannot grant itself:

- source origin, admitted ref, selection policy, checkout, and Gitlink origins;
- pinned runner images, literal-program allowlists, resources, mounts, secrets,
  and preconfigured network profiles;
- workload driver, identity, isolation, release root, and runtime root;
- runtime ID, expected signer identity, and trust-anchor store;
- optional state root and state-transition policy;
- an optional process write-lease record exactly when a state slot is
  process-bound writable; its sibling `<filename>.lock` path is derived;
- optional route driver and stable/private endpoint binding;
- independent deployment and lifecycle brakes;
- one replica on one admitted node in the v1 foundation.

Route membership is not a write lease. A write lease is not route membership.
Neither may be inferred from a PID, unit, socket, symlink, or health message.

## Deterministic release foundation

The compiled plan retains the exact raw declaration and binding blobs and
parses those exact bytes again during validation. Its content address therefore
covers the real inputs rather than caller-supplied digests beside mutable parsed
copies.

Source-selection facts currently preserve the private driver output: the
exact commit, source tree, recipe blob, admitted ref, and Gitlink tree entries.
The contract does not claim that shape validation proves ancestry, signatures,
or object custody. Those proofs belong to the narrow source driver that will
produce the facts.

A sealed release binds the plan ID to sorted full artifact receipts and sorted
full external-input materialization receipts. Only a sealed release validated
against its exact plan can derive Expected. Expected includes the plan and
release IDs, runtime and signer identities, health contract, node, route,
executable artifact digest, capabilities, selected dependencies, and the
optional canonical state-contract digest computed from the typed
`StateDeclaration` MessagePack encoding.

Expected is stable release and topology intent. Each actuator launch receives a
separate tiny `idunn.runtime_activation.v1` containing the Expected digest and
one Idunn-issued opaque runtime-instance ID. Idunn publishes that activation as
current only after the workload driver observes the matching native process and
artifact. Present requires both that Idunn observation and the service's
independent signed presence; either document alone is insufficient.

Managed dependency candidates are derived from validated Expected documents.
External operator bindings remain expected configuration only; they cannot
impersonate signed presence or readiness.

## Promotion and continuity

For a routed stateful singleton, Idunn starts the candidate privately, observes
the exact actuator launch, publishes its current activation, and observes its
signed warming presence. It then fences the incumbent writer, grants the
candidate its exact process-bound write lease, and waits for signed active
health plus Ready correlation. It changes and observes route membership only
afterward. Only after the proxy has adopted the candidate may Idunn clear the
promotion fence and drain the incumbent.

The deployment brake gates changes to source, release, configuration, schema,
unit, or authority binding. The lifecycle brake separately gates restart of an
already-admitted incarnation. Neither target brake may gate Idunn itself or an
unrelated service.

If Odin is unavailable, Idunn preserves existing routes and continuity for
already-admitted incarnations. It starts no graph-changing deployment or
promotion. A merely sealed candidate waits. An already-fenced transaction may
promote only when its frozen evidence already contains the exact Odin Ready
receipt for this runtime instance and presence digest; otherwise it waits,
rolls back through the same fence when reversible, or fails closed.

## Operator surface

The intended entry points are small:

```text
idunn up ghostlight
idunn up profile:aetheria
idunn up profile:full-gamecult
```

Idunn ensures one compatible Odin exists, publishes the desired fleet, resolves
typed capability dependencies, admits candidates, waits for independently
observed presence and readiness, and promotes routes only when the required
graph conditions hold.

## Current implementation boundary

`crates/idunn-daemon/src/deployment.rs` and `deployment_plan.rs` implement the
deterministic declaration, binding, plan, release, and Expected foundation.
They do not yet drive Git, runners, systemd, proxies, CultCache, CultMesh,
brakes, runtime activations, write leases, or runtime health verification.
Runtime integration must consume these contracts; it must not recreate target
catalogs, raw deployment commands, root Git inspection, or a second admission
opinion.
