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
- Idunn owns admission, freshness, monotonic lineage, managed-health judgment,
  and the outward status projection.
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
2. an Idunn-authored outward status projection binding the trust record,
   admission, managed target, current deployment lineage, freshness judgment,
   and observation time.

Consumers such as Epiphany Discord Status receive only the outward projection
through CultNet/CultMesh and verify Idunn's pinned identity. Reading Idunn's
private store is not a consumer protocol.

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
4. Publish and verify Idunn's outward signed projection.
5. Point Discord Status at the outward projections.
6. Delete generic unsigned health from managed-health authority and delete the
   Epiphany-only verifier after its last publisher migrates.

The rebuild is incomplete while an unsigned packet can keep a managed daemon
healthy or while a status consumer must inspect Idunn's private store.

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
- outward status is signed by Idunn and binds the exact admitted statement;
- private detail and filesystem/process internals never enter the operator
  projection.
