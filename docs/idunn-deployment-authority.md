# Idunn Deployment Authority

Idunn is GameCult's deployment, runtime-admission, continuity, and future swarm
control plane. It decides which exact workload incarnation may act. It does not
replace systemd, container runtimes, nginx, or a future general-purpose
orchestrator.

The architectural test is simple:

- Where and how a workload runs is generic infrastructure work. Use the
  configured systemd, container, proxy, or later Nomad/Kubernetes driver.
- Which GameCult incarnation has authority to serve, write state, or represent
  an admitted workload is Idunn work.

Idunn and Odin are closely integrated but do not share authority. Idunn owns
the admitted physical swarm. Odin owns the discoverable semantic topology of
the Verse. A service owns its signed runtime presence, capabilities, capacity,
and health. CultMesh carries typed projections between them.

## Authority map

- **Owner:** Idunn alone selects an exact source, admits its repository recipe
  under an operator binding, records the resulting artifact and process
  provenance, grants a process-bound state-write lease, separately admits
  route membership, and keeps the admitted incarnation alive.
- **Inputs:** root-admitted operator bindings; source-owned strict deployment
  declarations; exact Git objects read as the `idunn` identity; signed
  deployment or lifecycle-brake state; signed incarnation-bound service
  presence and health; Odin's observed capability topology when graph changes
  require semantic discovery; and observations from the configured workload
  and route drivers.
- **Outputs:** typed CultCache deployment plans and receipts; an Idunn-owned
  expected-fleet CultMesh projection; process-bound write grants; admitted
  route membership; deployment and continuity decisions; and explicit
  expected/present/ready disagreement records.
- **Derived state:** release dashboards, operator summaries, route displays,
  capacity summaries, and Odin's correlation views are projections. A target
  count, proxy backend, service-manager unit, container label, or configured
  endpoint cannot establish admission by itself.
- **Forbidden writers:** target repositories cannot grant themselves users,
  secrets, mounts, network, capabilities, resources, host paths, routes,
  placement, source refs, or rollout policy. Odin cannot promote a process or
  grant state writes. nginx cannot choose membership. systemd and container
  runtimes cannot choose the admitted incarnation. Service configuration cannot
  impersonate observed presence or readiness. Persisted shell strings and
  repository-supplied root programs are not executable authority.
- **Shared paths:** `idunn up`, scheduled deployment, manual deployment,
  scaling, restart, host reboot recovery, and crash recovery use the same
  source-to-recipe-to-artifact provenance and the same distinct write-lease and
  route-membership primitives.
  Deployment may change that chain only while the deployment brake admits the
  exact transaction. Continuity may restart only the already-admitted chain and
  is governed by a separate lifecycle brake.
- **Deletion line:** hard-coded target copies, raw `deploy_command` and
  `restart_command` state, `sh -c` actuation, root Git inspection, staged
  `/srv/odin/deploy-manifests`, target-specific gamecult-ops deploy programs,
  duplicate target units, and stop-incumbent-before-probe rollouts must cease to
  decide Ghostlight or CodexConnector deployment before either target is
  promoted through this design.

### Current implementation boundary

`deployment.rs` and `deployment_plan.rs` are the deterministic contract
foundation only. They parse one strict source declaration and one root-admitted
operator binding from the exact raw blobs retained by the compiled plan, check
private source-selection facts, derive managed providers from validated Expected
documents, and content-address a sealed release. They do not yet
drive Git, runners, systemd, nginx, brakes, CultCache, CultMesh, or the legacy
Idunn supervisor.

The full operator binding, compiled plan, source facts, and sealed release
are private Idunn control-plane state. They contain host paths and operational
bindings and must never be published as Odin topology. Only the sanitized
`ExpectedIncarnation` projection is shaped for later CultMesh publication. It
is derived only from a sealed release validated against its compiled plan and
names both content addresses and the executable artifact digest. Present and
Ready remain Odin-owned correlations over independently verified runtime
evidence; the deployment-plan module deliberately does not model them.

## The three truths Odin exposes

For each workload incarnation, Odin keeps three distinct observations:

1. **Expected** — Idunn has sealed this exact incarnation and has published its
   plan and release identities, admitted runtime and signer identities, health
   and optional canonical typed-MessagePack state-contract identities,
   endpoints, declared capabilities, and dependency role.
2. **Present** — Idunn has published the launch-scoped activation only after its
   actuator observed the exact runtime instance, and that instance has
   established its own matching signed CultNet/CultMesh runtime session.
3. **Ready** — the observed incarnation satisfies its signed capability and
   health contract and its required dependency graph is ready.

Correlation never repairs disagreement. Incarnation, runtime identity, endpoint,
schema, capability, and dependency mismatches remain visible and block
promotion. Configuration contributes only expected state; it cannot supply
present or ready state.

Every Idunn-managed deployment has an Odin-visible expected projection before
dependent services are promoted, and an Odin-observed runtime projection before
they are considered ready.

## Source-owned deployment declaration

Each deployable repository publishes one small, reviewable declaration at a
fixed path such as `deployment/idunn/recipe.toml`. Unknown fields are rejected.
The declaration contains no host commands or privileges. It describes:

- schema and target identity;
- requested runner profiles;
- build and test argv executed inside those runners;
- pinned generic HTTPS inputs and immutable artifact outputs;
- a constrained launch contract naming the packaged executable, literal
  arguments, operator-bound argument slots, and required environment names;
- one health contract and, only for stateful services, a state/schema contract;
- capabilities provided, including schema and compatibility information;
- bootstrap, required, optional, shared-infrastructure, private, and external
  operator-bound capability dependencies;
- startup ordering, conflicts, migration conditions, and minimum capacity when
  they matter.

Dependencies resolve by typed capability/provider compatibility. A declaration
names a daemon only when identity itself is the contract; ordinary dependencies
must not hard-code a provider instance.

Runtime health uses the shared `warming`, `active`, `degraded`, and `failed`
ontology. A recipe names only its health contract. It cannot redefine those
states, readiness, or operator-facing detail. Bootstrap dependencies must be
available before process start; the remaining dependency kinds retain their
typed intent without making this foundation a scheduler.

The declaration may run repository code only inside a runner sandbox admitted
by the operator binding. It cannot publish a host path, Docker argument,
systemd property, nginx fragment, UID, secret path, or capability grant. Idunn
deterministically lowers the constrained launch contract through the configured
workload driver; repository-owned raw unit or container-runtime text is not an
input.

## Operator binding

An operator binding is static root-admitted configuration, not observed state.
For one target on one fleet it owns:

- canonical repository origin, admitted ref, and minimum revision;
- exact runner kind, image digest, and affordances for each requested profile;
- source, cache, output, release, and runtime roots, plus state roots and
  transition policy only when the recipe declares state;
- preconfigured runner network profiles, mounts, identities, secrets,
  capabilities, resource bounds, and device access;
- workload driver, transient-unit namespace, private endpoint range, and stable
  route;
- runtime identity, expected signer identity, and trust-anchor store;
- a process-write-lease record only when the recipe declares process-bound
  writable state; its sibling lock path is derived rather than configured;
- route driver and its root-owned configuration destination;
- rollout, drain, irreversible-cut, and retention policy;
- one admitted replica on one named node in v1; later placement and scaling are
  delegated to an established orchestrator driver;
- externally supplied capability bindings.

A recipe request that has no compatible admitted binding fails closed. A
binding can grant less than requested; it cannot silently substitute a
different semantic capability.

`gamecult-ops` publishes these fleet bindings and provisions host substrate. It
does not own application build or deployment programs.

## Typed deployment state

Mutable control-plane state will be CultCache. Runtime integration must use
typed records for:

- requested service or profile;
- resolved capability graph and exact providers;
- exact source and recipe digest;
- runner binding and artifact digests;
- full materialization receipts for every pinned external input and artifact;
- candidate runtime identity, private endpoint, and signed runtime health;
- launch-scoped runtime activation, published as current only after workload-
  driver observation;
- process-bound write lease;
- independently admitted route membership;
- expected, present, and ready correlation;
- promotion, drain, failure, and continuity results.

Old records containing raw commands are readable history only. No executor may
interpret their command text.

State transition is also typed authority. The repository may package a
migration artifact and declare its accepted source schemas, destination schema,
and literal/operator-bound argv. The operator binding admits the state roots,
backup root, and whether this rollout permits migration or a fresh-root cut.
Idunn alone issues a fenced one-shot migration grant bound to the exact source,
artifact, state schema generation, and deployment incarnation. A target
migration never receives general deployment or lifecycle authority.

## Initial drivers and replaceable boundary

The first Yggdrasil implementation uses:

- transient systemd units for process lifetime and isolation, regenerated from
  Idunn's admitted state after Idunn starts;
- Docker only for admitted build/test/package runners;
- nginx HTTP proxying for Ghostlight;
- nginx stream proxying for CodexConnector's stable TCP endpoint;
- atomic root-owned files for process-bound write leases and separately
  generated proxy membership;
- graceful nginx validation and reload for route changes.

Idunn calls narrow workload, route, and write-lease/fencing driver ports. Those
ports express stage, start, observe, stop, validate route, fence an incumbent,
publish a process-bound write lease, and separately promote route membership.
They do not express GameCult admission decisions. A later Nomad or Kubernetes
driver may implement the same consequences. Idunn must not grow general
bin-packing, distributed scheduling, service-mesh, container-format,
cryptography, or consensus logic; when those become requirements, place
Idunn's admission semantics over an established orchestrator.

## Candidate promotion

For a singleton incarnation:

1. Resolve the required capability graph. Reuse a compatible admitted shared
   provider, especially Odin, rather than spawning a duplicate.
2. Publish desired fleet intent. Intent is not an Expected incarnation.
3. Freeze the exact source and recipe as `idunn`, then copy and verify it into a
   root-owned actuation stage. The privileged driver never opens Git.
4. Build, test, and package inside the bound runners. Seal the artifact,
   declaration, binding, launch contract, and input digests.
5. Derive and publish the release-bound Expected incarnation from the validated
   sealed release and its exact compiled plan.
6. Issue one opaque runtime-instance activation bound to the Expected digest,
   pass the exact Expected and activation documents through the actuator's
   isolated credential mechanism, and start the candidate privately. It has no
   stable route or canonical write lease.
7. After the workload driver observes the exact native runtime and executable
   artifact, publish that activation as current. Require the candidate's own
   signed matching `warming` presence. Render and validate the proposed route
   before changing either authority.
8. Open a named promotion transaction and separately fence the incumbent
   writer. The incumbent may remain routed but its process-bound write authority
   is revoked and it must return a retryable unavailable result.
9. Grant the private candidate its own process-bound write lease. Keep it off
   the stable route and wait for signed `active` health and the independently
   correlated Ready observation.
10. Publish the candidate backend, gracefully reload the route driver, and
   observe that the proxy has adopted it. Proxy lag is explicit transaction
   state, not readiness.
11. Clear the promotion fence only after route adoption is observed, then drain
    and stop the incumbent.

The state-write lease and route membership are distinct authorities. Fencing
the incumbent does not grant the candidate or change the route. Granting the
candidate permits that exact private process to write but does not route stable
traffic. Changing the route grants no write authority. The promotion
transaction clears its fence only after the independently observed write and
route authorities name the candidate. A PID, endpoint, symlink, or unit alone
grants neither authority.

Promotion persists crash-resumable phases: planned, materialized, sealed,
activation-issued, candidate-started, activation-observed, warming,
incumbent-fenced, write-lease-granted, ready-observed, route-observed,
fence-cleared, draining, and complete. A dead Idunn cannot leave a second
writer. Its successor resumes the exact sealed transaction or restores the
prior admitted incarnation through the same fence; it never infers completion
from a symlink or eventual health convergence.

An irreversible state cut fails closed after the write lease moves. A reversible
cut may restore the incumbent only through the same process-write-lease
primitive and only while its artifact and state contract remain valid.

## Continuity and outage behavior

Continuity restarts the exact sealed release, binding, endpoint, runtime
identity, and route membership. Every restart receives a new runtime-instance
activation and, for a state writer, a new process-bound lease only after the new
warming presence is observed. A lease from the dead process is never reused. A
continuity restart does not fetch a branch, rebuild, rerun a migration, alter
the launch contract, or consume the deployment brake. An explicit lifecycle
brake may suspend this actuation without changing deployment authority.

Idunn starts and recovers from its own durable admitted state. Odin is the first
managed semantic daemon, never an Idunn bootstrap dependency. Odin's initial
admission is the sole graph-bootstrap exception: Idunn uses its root-admitted
local binding, publishes Odin's expected projection into durable CultMesh state,
and waits for Odin's observed activation and signed presence before admitting
dependents.

When Odin is unavailable, Idunn continues health observation and continuity for
already admitted incarnations and leaves current routes in place. It does not
begin a graph-changing transaction, and a merely sealed candidate may not start
promotion. Only a transaction whose incumbent has already been fenced may
promote when its frozen evidence already contains the exact Odin Ready receipt
for this runtime instance and signed-presence digest. Otherwise it waits, rolls
back through the same fence when reversible, or fails closed; Idunn never
manufactures Ready locally. New deployment, dependency
substitution, promotion, or scaling waits for semantic discovery. When Odin
returns, disagreement is correlated and exposed before graph-changing work
resumes.

Source selection is explicit per binding: a pinned object, the captured head of
an admitted ref, or a signed release authorization. The current deterministic
foundation retains the exact selected commit, tree, recipe blob, and Gitlink
facts, but shape validation is not proof of Git ancestry, signature authority,
or object custody. Those proofs belong to the narrow Idunn-owned source driver
that will produce these private facts. “Whatever the branch points to when root
runs” is not a selection policy.

## Garden path

The ordinary operator surface is:

```text
install/start Idunn
idunn up ghostlight
idunn up profile:aetheria
idunn up profile:full-gamecult
```

`idunn up` ensures a compatible Odin is admitted first, publishes desired fleet
state, resolves the capability graph, stages candidates, waits for signed
presence and readiness, then promotes routes. An “Idunn script” means the small
repository declaration plus this standard command, not another imperative host
deployment program.

## Verification

The first live cut is complete only when tests and host observations prove:

- exact source → recipe → artifact → process provenance;
- an issued but driver-unobserved activation cannot establish Present, and a
  prior runtime instance cannot replay presence after restart;
- target requests cannot grant an omitted runner or host affordance;
- old command-bearing records and old target scripts are unexecutable;
- a candidate cannot write before its process-bound lease or receive the stable
  route before separate route admission;
- route membership and write leases remain separate records and name the same
  incarnation and process before the promotion fence clears;
- expected, present, and ready disagreement blocks promotion and remains
  observable;
- replayed or stale signed health cannot satisfy present or ready;
- external operator-bound dependencies still require their declared observed
  health/readiness contract; configuration supplies only expected state;
- a compatible shared Odin is reused and a second provider is not spawned;
- Idunn death during every promotion phase resumes or fails closed without a
  second writer;
- proxy-driver lag remains visible and cannot be mistaken for route admission;
- the old process rejects writes immediately after fence or lease revocation;
- same-release crash recovery works while the deployment brake is engaged;
- the lifecycle brake can stop continuity without admitting a deployment;
- Odin loss preserves current continuity/routes and blocks graph changes;
- the incumbent remains routed until the candidate has signed `active` health
  and is correlated Ready;
- the old unit, release, state root, grant, and route cannot regain authority;
- Ghostlight restart preserves the exact world journal and digest identity.
