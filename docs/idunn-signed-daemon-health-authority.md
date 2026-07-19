# Idunn signed daemon-health authority

## Objective

Idunn must know which daemon authored a health statement before that statement
can affect lifecycle judgment or operator status. A typed packet is a shape,
not an identity. The current generic `idunn.daemon_health.v1` RUDP path checks
shape, daemon id, contract, self-declared publication source, and freshness but
does not authenticate the publisher. It is diagnostic input until replaced.

## Owner

- Each daemon owns its health statement and signs it with its pinned service or
  host identity.
- Idunn owns admission, freshness, monotonic lineage, and the outward projection
  of current authenticated provider health. Lifecycle judgment remains private
  Idunn policy and is not smuggled into that provider-health statement.
- Root deployment configuration owns the trust binding between daemon id,
  health contract, source runtime, and signer identity.

No renderer, transport peer, systemd unit, PID probe, caller boolean, or typed
but unsigned packet owns health.

## Inputs

The daemon-authored statement binds:

- exact daemon id and health contract;
- source runtime id and signer identity;
- observation time, publisher incarnation, and monotonic sequence;
- bounded health state and detail;
- release id, release witness, source revision, and deployment request when the
  managed target is release-bound;
- `private_state_exposed=false`;
- an Ed25519 signature over the complete unsigned statement under a
  domain-separated signing purpose.

The Idunn trust binding names the only public key allowed to speak for that
daemon/contract/runtime tuple and whether release identity is required.

## Outputs

Idunn persists two separate facts:

1. the admitted daemon-authored statement and its digest;
2. an Idunn-authored `idunn.authenticated_provider_health_projection.v1`
   record binding the trust record, admission, exact provider lineage, and
   freshness interval.

The outward record exists only while a generic provider-authored admission is
current and still matches its root trust and optional release binding. Missing
publication, release drift, dependency failure, target absence, and recovery
policy are absence of this projection. They are not synthetic provider states.
The projected state is exactly the authenticated provider state. Its only
explanation is a closed Idunn-generated reason code paired to that state:
`authenticated_provider_active`, `_warming`, `_degraded`, or `_failed`.
There is no free-text detail field.

Consumers such as Epiphany Discord Status receive only the outward projection
through CultNet/CultMesh and verify Idunn's pinned identity. Reading Idunn's
private store is not a consumer protocol.

The query transport is an explicit, read-only CultNet/RUDP listener enabled by
`--public-health-query-bind <addr>`. It has no default and is valid only with
`--swarm-profile`, `--service-identity-store`, and `--public-health-store`.
The listener opens only the dedicated public store and derives its allowlist
from the configured target catalog's daemon/health-contract pairs. It returns
the exact stored projection payload bytes with fixed source runtime
`idunn-daemon` and role `authenticated-provider-health-projector`. It has no
handle to Idunn's private stores and accepts no mutation message.

## Derived state

- Process and systemd observations are diagnostic evidence only.
- RUDP source endpoints and connection ids are transport routing only.
- `publication_source`, `transport`, `ready`, and `healthy` strings are display
  data unless contained inside a valid signed statement.
- An unsigned legacy health packet may be retained temporarily for diagnostics
  but cannot satisfy managed health, prevent recovery, or produce a healthy
  outward projection.

## Forbidden writers

- The inbound packet cannot choose its own trust anchor.
- A daemon cannot change its configured daemon id, contract, or source runtime
  by signing different values.
- Idunn cannot manufacture daemon-authored health from PID, executable,
  container, HTTP, systemd, or command exit observations.
- Epiphany, Bifrost, VoidBot, Eve, and Discord cannot infer Idunn acceptance
  from a sent packet.
- A generic typed `idunn.daemon_health.v1` packet cannot fall back into
  lifecycle authority when signature verification fails or trust is absent.

## Shared paths

Every managed daemon uses the same verification primitive:

`signed statement -> root trust binding -> signature and tuple verification ->
monotonic admission -> target/deployment join -> Idunn outward projection`

Epiphany's deployed `epiphany.idunn_signed_runtime_health.v0` path is migration
evidence and the first working implementation of the signature half. It is not
the permanent excuse for a service-specific verifier. The generic contract
must absorb its invariants before the special path is deleted.

## Cut line

1. Add generic signed-health and trust-binding contracts.
2. Make generic Idunn admission verify the pinned tuple, signature, sequence,
   incarnation, freshness, and release requirements.
3. Migrate Epiphany and Bifrost publishers and install their root-owned trust
   bindings.
4. Publish and verify Idunn's outward signed authenticated-provider-health
   projection using the fixed CultNet signature profile
   `idunn.authenticated-provider-health-projection.v1`.
5. Point Discord Status at the outward projections.
6. Delete generic unsigned health from managed-health authority and delete the
   Epiphany-only verifier after its last publisher migrates.

The rebuild is incomplete while an unsigned packet can keep a managed daemon
healthy or while a status consumer must inspect Idunn's private store.

## Implemented cut (2026-07-19)

The deployed-lineage source now has the first executable generic admission
path. `idunn.signed_daemon_health.v1` is verified against an exact root-store
daemon/contract/runtime binding, the signer identity is derived from the bound
public key, the Ed25519 signature covers the positional empty-signature
statement under the domain-separated document purpose, and Idunn atomically
persists the signed statement, its generic admission, and the compatibility
health row. Managed-health selection rejoins all three with the current root
binding on every read; trust rotation invalidates old health. Sequence,
incarnation, observation, freshness, and optional deployment lineage advance
under CAS.

Unsigned `idunn.daemon_health.v1` packets now persist only as
`idunn.unsigned_daemon_health_diagnostic.v1` under `diagnostic:<daemon>`.
They cannot overwrite the daemon health key, create an admission, reset the
missing-publication clock, suppress recovery, or produce authenticated health.
The evaluation clock is passed through the whole target cycle instead of being
resampled inside the health selector.

Idunn now also has the local outward projection writer. One publisher opens an
already-enrolled Idunn service identity before workers start, captures an
incarnation-stable root trust snapshot, and writes only
`idunn.authenticated_provider_health_projection.v1` envelopes to a distinct
public store. Each write rejoins the exact health, admission, signed statement,
root binding, and optional current deployment lineage. The signed positional
record has a fixed hash-derived key, a store-monotonic sequence, a process UUID,
and an expiry capped by the provider observation's managed-health silence
window. Missing, legacy, unsigned, drifted, or mutated input cannot write,
refresh, or delete a projection; an old row simply expires for consumers.

The public projection has a dedicated multi-peer CultNet/RUDP snapshot
listener. Startup synchronously binds the explicit address and validates the
public store before target workers start. Bind failure or public-store
contamination is fatal startup evidence. Each request rereads only that store
and passes exact records through CultNet's read-only snapshot server with the
target-derived `(schema,key)` allowlist. Unknown schema/key requests return no
record. Malformed datagrams or messages are refused per peer without killing
the daemon. No request can write, delete, import, or inspect private Idunn
state.

This source is deliberately not deployable yet: Bifrost and the remaining
generic publishers still emit the unsigned diagnostic contract, production
root trust and Idunn identity stores have not been installed, and Epiphany
still uses its signed v0 migration path. The query transport and Epiphany
consumer verifier now exist in source, but neither makes an unmigrated
publisher authoritative. Deploying this intermediate source would correctly
classify unmigrated daemons as missing health and could therefore actuate
recovery. Publisher migration and trust enrollment must land before promotion.

## Verification layer

Hostile tests must prove:

- valid shape with no signature is not admitted;
- wrong key, daemon, contract, runtime, release, deployment, or trust record is
  not admitted;
- replay, sequence rollback, time rollback, incarnation substitution, and
  future/stale observations are not admitted;
- a live PID, existing executable, green HTTP response, or generic `ready`
  string cannot repair failed authentication;
- one daemon's trust record cannot authenticate another daemon;
- unsigned legacy health cannot suppress recovery or yield outward healthy;
- outward provider health is signed by Idunn under the fixed, typed purpose and
  binds the exact admitted statement;
- missing health, release drift, and dependency failure cannot write or refresh
  the projection; any prior row expires instead of being rewritten as an
  Idunn-authored provider state;
- caller-selected signing purposes, signer ids, reason strings, and synthetic
  states fail closed;
- private detail and filesystem/process internals never enter the operator
  projection.
