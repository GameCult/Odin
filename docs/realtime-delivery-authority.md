# Realtime delivery authority

This map governs Muninn media and Sleipnir input hardening. Transport is a
carrier. Signal semantics own what survives loss.

## Sleipnir input

- Owner: Muninn owns source-local sampling and classification. Sleipnir owns
  mapping and the single local virtual-HID commit path.
- Inputs: physical reports, selected device/stream, mapping intent, connection
  epoch, monotonic state sequence, ordered input edges, and application receipt.
- Outputs: replaceable latest state, replayable ordered edges, virtual-HID
  commits, acknowledgement cursor, and bounded freshness telemetry.
- Derived state: wall-clock capture time is telemetry only. The latest full
  snapshot reconciles held state; it does not prove that intervening edges were
  delivered.
- Forbidden writers: generic retry order, CultCache telemetry, receiver batch
  coalescing, and wall-clock comparison may not decide input freshness or edge
  delivery.
- Shared paths: live input, remapping, reconnect, replay tests, and
  neutralization use the same Sleipnir commit primitive.
- Cut line: a whole controller snapshot is no longer the sole owner of button
  delivery. Replaceable state and non-replaceable edges have separate sequence
  and acknowledgement semantics.
- Verification layer: observe virtual-HID transition timelines through loss,
  duplication, reorder, stalls, clock rollback, and reconnect.

## Muninn media

- Owner: Muninn owns capture/encoder lifecycle, access-unit packetization,
  sender queues, parity/repair material, and response to receiver feedback.
- Inputs: encoded video access units, encoded audio packets, media-clock PTS,
  consumer-derived deadlines, and receiver damage/decode feedback.
- Outputs: deadline-bound video chunks/parity, deadline-bound audio continuity
  packets, selective repairs, encoder recovery requests, and stage telemetry.
- Derived state: RUDP acknowledgements, repair counters, CultCache receipts, and
  OBS lowering are evidence or carrier state. They do not extend usefulness.
- Forbidden writers: generic RUDP retry duration must not decide media lifetime;
  a repair cache must not mint a new deadline; a missing but recoverable chunk
  must not independently force an IDR; persistence and OBS must not own sync.
- Shared paths: live capture, test injection, repair, keyframe recovery, and
  reconnect use the same frame/deadline policy.
- Cut line: remove unbounded payload queues and the two-second realtime default.
  Expired frames, audio packets, parity, and repairs leave the machine together.
- Verification layer: observe decoded/presented timelines through controlled
  loss, jitter, reorder, oversubscription, and reconnect—not merely transport
  counters.

## Transport substrate

- Owner: CultNet RUDP owns connection/session mechanics, packetization,
  retransmission, transport acknowledgement, fragment reassembly, and delivery
  flags. It does not own application usefulness or application receipt.
- Inputs: semantic channel, connection epoch, payload, send deadline, and peer
  packets.
- Outputs: delivered frames, bounded delivery failure, transport counters, and
  liveness state.
- Derived state: pending packet maps, ACK windows, fragment buffers, and reorder
  buffers are bounded transport internals.
- Forbidden writers: one global packet sequence must not let lost deadline
  traffic block ordered edges; transport ACK must not impersonate Sleipnir
  application receipt; reconnect must not silently continue an old epoch.
- Shared paths: input state, input edges, media, control, health, and schema
  traffic use explicit semantic channels whose interference is tested.
- Cut line: do not wrap ambiguous delivery in another retry loop. Split or fix
  the underlying sequence/ACK authority when cross-channel behavior violates a
  signal contract.
- Verification layer: deterministic session tests plus socket impairment tests
  for loss, ACK-window escape, sustained inbound load, buffer bounds, wrap, and
  reconnect.

## Delivery classes

| Class | Supersession | Ordering | Recovery | Expiry |
|---|---|---|---|---|
| input latest state | newer `(epoch, state_sequence)` replaces older | per device | republish current state | tens of milliseconds |
| input edge | never until application-acknowledged or epoch fenced | per device/epoch | bounded replay from edge cursor | session/lease bounded |
| video access unit | whole frame and dependency chain | frame/dependency IDs | parity, selective repair, then keyframe | decode deadline |
| audio continuity | playout order | packet/media clock | small bounded FEC/reorder/concealment | playout deadline |
| control | none unless command contract says so | command sequence | reliable delivery plus application receipt | explicit command deadline |

The machine is reliable when each class loses only what its contract permits.

## Implemented reliability cut: 2026-07-16

- Odin's vendored Rust substrate is pinned to the experimental CultLib runtime
  snapshot recorded in `vendor/CULTLIB_SNAPSHOT.md`.
- Muninn/Sleipnir HID traffic now carries a per-source epoch, monotonic latest
  state sequence, pre-collapse ordered button edges, cumulative application
  acknowledgement, replay until acknowledgement, duplicate/reorder handling,
  and stale-epoch rejection. Wall time is telemetry only.
- Muninn audio/video payloads use CultNet's unreliable `realtime` lane. The
  experimental `media` lane is reliable without expiry and therefore cannot own
  deadline-bound A/V.
- The default LAN media deadline is 100 ms. Encoder-to-sender transport uses a
  bounded synchronous channel; pending audio/video queues are bounded; repair
  cache entries expire on the sender deadline; receiver-declared late frames
  cannot be repaired.
- Recoverable missing video chunks remain repair/FEC damage. Only explicit or
  dependency-derived decode-chain invalidation requests a keyframe.

Still open: real encoder keyframe actuation, bounded long-disconnect admission
for unacknowledged HID edges, Opus audio FEC/PLC, adaptive bitrate/parity from
receiver pressure, and a socket impairment timeline harness.

Receiver audit correction on 2026-07-16: the native Mimir/OBS receiver already
contains production XOR parity reconstruction. Its early repair feedback was
incorrectly placing the still-live frame in `late_frame_ids`, causing Odin's
deadline guard to refuse every repair. The native feedback contract now keeps
early damage live and names the frame late only at assembly expiry; a C++-emitted
fixture is decoded by the Rust packetizer test. The same audit found the sender
emits 10 ms float PCM while the receiver launched FFmpeg as AAC. Mimir now uses
the `f32le`, stereo, 48 kHz input contract, a 40 ms reorder budget, and bounded
short-hole silence concealment. Opus FEC/PLC remains the target for
Moonlight-grade lossy audio.
