# Idunn Deployment Authority

Idunn is GameCult's deployment, runtime-admission, continuity, and future swarm
control plane. It decides which exact workload incarnation may act. It does not
replace systemd, container runtimes, nginx, or a future general-purpose
orchestrator.

The architectural test is simple:

- Where and how a workload runs is generic infrastructure work. Use the
  configured systemd, container, proxy, or later Nomad/Kubernetes driver.
- Which GameCult incarnation has authority to serve, write state, or represent
  an admitted generation is Idunn work.

Idunn and Odin are closely integrated but do not share authority. Idunn owns
the admitted physical swarm. Odin owns the discoverable semantic topology of
the Verse. A service owns its signed runtime presence, capabilities, capacity,
and health. CultMesh carries typed projections between them.

## Authority map

- **Owner:** Idunn alone selects an exact source, admits its repository recipe
  under an operator binding, records the resulting artifact and process
  provenance, grants process-bound route and state-write admission, and keeps
  the admitted generation alive.
- **Inputs:** root-admitted operator bindings; source-owned strict deployment
  declarations; exact Git objects read as the `idunn` identity; signed
  deployment or lifecycle-brake state; signed generation-bound service
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
  runtimes cannot choose the admitted generation. Service configuration cannot
  impersonate observed presence or readiness. Persisted shell strings and
  repository-supplied root programs are not executable authority.
- **Shared paths:** `idunn up`, scheduled deployment, manual deployment,
  scaling, restart, host reboot recovery, and crash recovery use the same
  source-to-recipe-to-artifact provenance and the same admission primitive.
  Deployment may change that chain only while the deployment brake admits the
  exact transaction. Continuity may restart only the already-admitted chain and
  is governed by a separate lifecycle brake.
- **Deletion line:** hard-coded target copies, raw `deploy_command` and
  `restart_command` state, `sh -c` actuation, root Git inspection, staged
  `/srv/odin/deploy-manifests`, target-specific gamecult-ops deploy programs,
  duplicate target units, and stop-incumbent-before-probe rollouts must cease to
  decide Ghostlight or CodexConnector deployment before either target is
  promoted through this design.

## The three truths Odin exposes

For each generation, Odin keeps three distinct observations:

1. **Expected** — Idunn has admitted or intends this exact generation and has
   published its deployment identity, endpoints, declared capabilities, and
   dependency role.
2. **Present** — that generation has established its own signed CultNet/CultMesh
   runtime session.
3. **Ready** — the observed generation satisfies its signed capability and
   health contract and its required dependency graph is ready.

Correlation never repairs disagreement. Generation, runtime identity, endpoint,
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
- package inputs and immutable artifact outputs;
- a constrained launch contract naming the packaged executable, literal
  arguments, operator-bound argument slots, and required environment names;
- health, staged-readiness, and state/schema contracts;
- capabilities provided, including schema and compatibility information;
- bootstrap, required, optional, shared-infrastructure, private, and external
  operator-bound capability dependencies;
- startup ordering, conflicts, migration conditions, and minimum capacity when
  they matter.

Dependencies resolve by typed capability/provider compatibility. A declaration
names a daemon only when identity itself is the contract; ordinary dependencies
must not hard-code a provider instance.

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
- source, cache, output, release, state, and runtime roots;
- network mode, allowed dependency egress, mounts, identities, secrets,
  capabilities, resource bounds, and device access;
- workload driver, transient-unit namespace, private endpoint range, and stable
  route;
- route driver and its root-owned configuration destination;
- rollout, drain, irreversible-cut, and retention policy;
- desired replicas and allowed placement;
- externally supplied capability bindings.

A recipe request that has no compatible admitted binding fails closed. A
binding can grant less than requested; it cannot silently substitute a
different semantic capability.

`gamecult-ops` publishes these fleet bindings and provisions host substrate. It
does not own application build or deployment programs.

## Typed deployment state

Mutable control-plane state is CultCache. The live path uses typed records for:

- requested service or profile;
- resolved capability graph and exact providers;
- exact source and recipe digest;
- runner binding and artifact digests;
- candidate process, private endpoint, and signed staged health;
- process-bound write lease and route admission;
- expected, present, and ready correlation;
- promotion, drain, failure, and continuity results.

Old records containing raw commands are readable history only. No executor may
interpret their command text.

State transition is also typed authority. The repository may package a
migration artifact and declare its accepted source schemas, destination schema,
and literal/operator-bound argv. The operator binding admits the state roots,
backup root, and whether this rollout permits migration or a fresh-root cut.
Idunn alone issues a fenced one-shot migration grant bound to the exact source,
artifact, state generation, and deployment. A target migration never receives
general deployment or lifecycle authority.

## Initial drivers and replaceable boundary

The first Yggdrasil implementation uses:

- transient systemd units for process lifetime and isolation, regenerated from
  Idunn's admitted state after Idunn starts;
- Docker only for admitted build/test/package runners;
- nginx HTTP proxying for Ghostlight;
- nginx stream proxying for CodexConnector's stable TCP endpoint;
- atomic root-owned files for process/write admission and generated proxy
  membership;
- graceful nginx validation and reload for route changes.

Idunn calls narrow workload, route, and admission/fencing driver ports. Those
ports express stage, start, observe, stop, validate route, publish a fenced or
process-bound admission, and promote route. They do not express GameCult
admission decisions. A later Nomad or Kubernetes driver may implement the same
consequences. Idunn must not grow general bin-packing, distributed
scheduling, service-mesh, container-format, cryptography, or consensus logic;
when those become requirements, place Idunn's admission semantics over an
established orchestrator.

## Candidate promotion

For a singleton generation:

1. Resolve the required capability graph. Reuse a compatible admitted shared
   provider, especially Odin, rather than spawning a duplicate.
2. Publish the desired fleet and candidate as expected state.
3. Freeze the exact source and recipe as `idunn`, then copy and verify it into a
   root-owned actuation stage. The privileged driver never opens Git.
4. Build, test, and package inside the bound runners. Seal the artifact, recipe,
   unit, and input digests.
5. Start the candidate through the workload driver on a private endpoint. It
   has no stable route or canonical write lease.
6. Require its own signed generation-bound `staged` health and signed runtime
   presence. Render and validate the proposed route before changing admission.
7. Compare-and-swap the current process/write-admission record into a named
   promotion fence. No generation may write while fenced; the incumbent may
   remain routed but must return a retryable unavailable result.
8. Publish the candidate backend, gracefully reload the route driver, and
   observe that the proxy has adopted it. Proxy lag is explicit transaction
   state, not readiness.
9. Compare-and-swap the fence into the candidate's process-bound write grant.
   Wait for signed `active` health, then drain and stop the incumbent.

The admission record is the one writer lease. A PID, endpoint, symlink, unit,
or route alone never grants writes. Outside the explicit promotion fence, the
write-admitted process and routed generation must agree. Runtimes must bind and
publish staged health without opening canonical writable state; they open or
mutate that state only after the exact process-bound admission becomes current.

Promotion persists crash-resumable phases: planned, materialized, sealed,
candidate-started, staged, fenced, route-observed, write-admitted, active,
draining, and complete. A dead Idunn cannot leave a second writer. Its successor
resumes the exact sealed transaction or restores the prior admitted generation
through the same fence; it never infers completion from a symlink or eventual
health convergence.

An irreversible state cut fails closed after the write lease moves. A reversible
cut may restore the incumbent only through the same admission primitive and
only while its artifact and state contract remain valid.

## Continuity and outage behavior

Continuity restarts the exact sealed release, binding, endpoint, and admission
identity. It does not fetch a branch, rebuild, rerun a migration, alter a unit,
or consume the deployment brake. An explicit lifecycle brake may suspend this
actuation without changing deployment authority.

Idunn starts and recovers from its own durable admitted state. Odin is the first
managed semantic daemon, never an Idunn bootstrap dependency. Odin's initial
admission is the sole graph-bootstrap exception: Idunn uses its root-admitted
local binding, publishes Odin's expected projection into durable CultMesh state,
and waits for Odin's own signed presence before admitting dependents.

When Odin is unavailable, Idunn continues health observation and continuity for
already admitted generations and leaves current routes in place. It does not
begin a graph-changing transaction. An already sealed transaction may continue
only from its pinned graph and expected-generation snapshot; once a promotion
fence exists, safety recovery proceeds without waiting for Odin so the fleet is
not stranded with an ambiguous writer. New deployment, dependency substitution,
or scaling waits for semantic discovery. When Odin returns, disagreement is
correlated and exposed before graph-changing work resumes.

Source selection is explicit per binding: a pinned object, the captured head of
an admitted ref, or a signed release authorization. Every policy freezes one
exact commit in the plan and proves origin, reachability, recipe digest, and any
minimum floor before build. “Whatever the branch points to when root runs” is
not a selection policy.

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
- target requests cannot grant an omitted runner or host affordance;
- old command-bearing records and old target scripts are unexecutable;
- a candidate cannot write or receive the stable route before admission;
- route and write admission name the same generation and process;
- expected, present, and ready disagreement blocks promotion and remains
  observable;
- replayed or stale signed health cannot satisfy present or ready;
- external operator-bound dependencies still require observed signed presence
  and readiness; configuration supplies only expected state;
- a compatible shared Odin is reused and a second provider is not spawned;
- Idunn death during every promotion phase resumes or fails closed without a
  second writer;
- proxy-driver lag remains visible and cannot be mistaken for route admission;
- the old process rejects writes immediately after fence or lease revocation;
- same-release crash recovery works while the deployment brake is engaged;
- the lifecycle brake can stop continuity without admitting a deployment;
- Odin loss preserves current continuity/routes and blocks graph changes;
- the incumbent remains routed until the candidate is signed-staged;
- the old unit, release, state root, grant, and route cannot regain authority;
- Ghostlight restart preserves the exact world journal and digest identity.
