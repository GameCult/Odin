# Idunn Deployment Authorization Handshake

## Authority map

- **Owner:** Idunn's reconciler owns one current deployment head per daemon target. The
  head owns deployment identity until it is terminal or explicitly superseded.
- **Inputs:** desired source revision, repository and upstream ref, deploy command,
  exact Bifrost authority id and envelope digest when required, the current head
  and result, and the typed deployment-brake observation.
- **Outputs:** an immutable planner intent, a stable typed request/head, an authorization-wait state, a claimed
  execution state, and one terminal result.
- **Derived state:** keepalive decisions and operator status are projections of the
  head. Loop timestamps are observations only; they do not own request identity.
- **Forbidden writers:** planning loops emit immutable intent and may not create or replace heads. They may not replace a byte-equivalent live
  head. Brake denial may not write a terminal deployment result. Migration and
  actuator paths may not run before the exact grant is claimed. Recovery may not
  replay an executing or consequence-unknown request.
- **Shared paths:** automatic drift, stale-health deployment, and manual redeploy
  requests use the same pending-head commit and final consequence gate. Any
  explicitly separate manual path must remain visibly non-deploying until it has
  the same stable grantable identity.
- **Cut line:** remove per-cycle timestamp request ownership and immediate
  persist-then-execute coupling. The current head is reused until success, true
  migration/actuator failure, explicit cancellation, source/authority
  supersession, or interrupted-execution review terminalizes it.
- **Head binding:** the head binds intent digest, exact release authorization id and
  envelope digest, owning Idunn incarnation, phase/result, and terminal reason.
- **Verification layer:** native read-only status output plus hostile tests prove
  stable identity across braked cycles and restart, exact-grant execution, inert
  wrong/expired grants, supersession, claim races, and no duplicate consequence
  after crash recovery.

## State transitions

`awaiting-authorization -> claimed -> executing -> succeeded | failed | consequence-unknown`

There is no generic pending phase. Supersession and cancellation are legal only
from `awaiting-authorization` and replace the head atomically. Claim CAS occurs
while the root brake snapshot lock is held; a CAS loser cannot spawn. Startup
preserves awaiting heads, but claimed/executing heads owned by an earlier Idunn
incarnation become `consequence-unknown` and are never replayed. Idunn guarantees
at-most-once spawn for each named consequence; exactly-once external effects
additionally require actuator idempotency or a durable actuator receipt.

`executing` covers two ordered consequence subphases: release migration, then the
deployment actuator. Each subphase revalidates the same exact grant immediately
before spawn. Idunn does not claim a single process spawn: migration and deployment
may be separate children. Once the head enters `executing`, a crash before, during,
or between those children makes the whole request `consequence-unknown`; startup
replays neither child. The migration result is durable when it completes, but it is
diagnostic evidence rather than permission to resume the second child after an
incarnation boundary.

Engaged is an ordinary authorization wait. Missing, corrupt, foreign, mismatched,
or expired brake state is an invalid-authorization wait with a typed reason. None
of those states is actuator failure. A changed desired source or release authority
terminalizes the old head as superseded before creating its successor.
