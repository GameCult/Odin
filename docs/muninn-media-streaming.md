# Muninn Media Streaming

Muninn live A/V is the stress organ for CultMesh transport. The goal is not
"make OBS show something once." The goal is encoded game video and audio moving
between nearby PCs at LAN speed with bounded latency, observable ownership, and
transport behavior that teaches CultMesh how to carry hot media.

## Objective

Raven Muninn captures screen video and Realtek loopback audio, encodes them with
hardware-first low-latency settings, publishes them through CultNet/CultMesh
media streams, and lets the Muninn OBS plugin render the stream without owning
capture, daemon lifecycle, or transport truth.

## Current Mechanism

The current Raven A/V path is a compatibility path:

1. OBS reads Raven's `muninn.obs_stream_catalog` from the synced CultCache
   store.
2. The OBS plugin sends a typed `muninn.capture_stream_command` to Raven's
   Muninn daemon over CultNet RUDP.
3. Raven Muninn `serve` owns command acceptance and spawns a daemon-owned
   activation child for the requested stream.
4. The activation child starts WASAPI loopback capture and FFmpeg.
5. FFmpeg muxes encoded video and audio into MPEG-TS on stdout.
6. Muninn slices that byte stream into fixed-size chunks and sends those chunks
   over the RUDP `media` channel.
7. The OBS plugin forwards received bytes into a local FFmpeg source.

That path proved activation, capture, hardware encode, and cross-machine
delivery. It is not the final media architecture. MPEG-TS byte chunks over an
unordered lossy hot path give CultMesh no frame identity, no deadline, no media
dependency graph, and no clean way to choose between retransmit, conceal,
discard, keyframe request, or audio/video resync.

## Invariants

- Muninn owns local capture and stream activation for the machine body where
  the sensors live.
- OBS and Mimir are consumers. They may request a stream and report receiver
  health, but they do not start capture by implication and do not own Raven
  daemon state.
- Idunn owns daemon supervision and health pressure. It does not infer that
  liveness means screen/audio capture should be burning.
- CultMesh owns live media stream semantics: stream identity, frame identity,
  timestamps, deadlines, dependencies, channel policy, and receiver feedback.
- CultCache records are receipts, catalog entries, and operator/debug state.
  They are not the hot media lane.
- Audio and video have separate clocks and recovery policy. They may share a
  session, but they must not be hidden inside an opaque transport byte soup.
- Transport reliability is deadline-bound. Late media is damage, not treasure.

## Intended Change

Replace "MPEG-TS stdout sliced into RUDP packets" with "codec access units and
audio packets published as typed CultMesh media frames."

The stream owner should emit media documents shaped around decisions the
transport and decoder actually need:

```text
muninn.media_video_access_unit.v1
  stream_id
  session_id
  frame_id
  codec
  pts_ticks
  duration_ticks
  timebase_num
  timebase_den
  keyframe
  dependency_frame_id
  deadline_ticks
  chunk_index
  chunk_count
  payload

muninn.media_audio_packet.v1
  stream_id
  session_id
  packet_id
  codec
  pts_ticks
  duration_ticks
  timebase_num
  timebase_den
  deadline_ticks
  payload

muninn.media_receiver_feedback.v1
  stream_id
  session_id
  receiver_id
  highest_decodable_frame_id
  missing_frame_ids
  late_frame_ids
  requested_keyframe
  jitter_us
  decode_queue_us
```

The exact schema names can move when promoted into the shared document catalog,
but the ownership shape should not: frame/access-unit identity is load-bearing.

## Authority Map

- Owner: Muninn capture runtime owns source capture, encode configuration, and
  emission of video access units plus audio packets for a requested stream.
- Inputs: explicit `muninn.capture_stream_command`, local capture devices,
  encoder capabilities, receiver feedback, stream policy, and Idunn-supervised
  daemon runtime state.
- Outputs: typed active stream receipts in CultCache, CultMesh media frames over
  CultNet RUDP, and typed receiver/transport health receipts.
- Derived state: OBS catalog entries are discovery hints; local FFmpeg bridge
  URLs are compatibility lowering details; packet counters and logs are
  observability only.
- Forbidden writers: OBS source settings, local UDP bridge behavior, scheduled
  task wrappers, health scripts, and replayed command receipts must not decide
  capture state, stream identity, frame order, or media repair policy.
- Shared paths: direct operator requests, OBS plugin requests, future Mimir
  requests, restart recovery, and reconnect recovery must all publish the same
  typed capture command and consume the same active stream receipts.
- Deletion line: delete the hot-path assumption that a media stream is an
  MPEG-TS byte stream. Keep it only as a named compatibility lowering until the
  OBS receiver can consume typed media frames directly.

## Codec Direction

Use vendor hardware encode before inventing a codec. NVIDIA's practical answer
in this space is NVENC/NVDEC, low-latency encode presets, high LAN bitrate,
short GOP or intra-refresh, no B-frames, no lookahead, and application-level
transport that understands frame deadlines.

Preferred order:

1. AV1 NVENC when both encode and decode bodies support it.
2. HEVC NVENC for strong quality at high LAN bitrates.
3. H.264 NVENC as the compatibility fallback for broad decoder support.

The codec should produce elementary video access units, not a muxed MPEG-TS
program. Audio should use Opus low-delay packets unless the OBS lowering needs
a temporary PCM/AAC compatibility bridge.

## CultNet RUDP Media Policy

The RUDP `media` channel should become a media-aware lane, not merely a reliable
unordered byte lane.

Required behavior:

- Frame-level fragmentation and reassembly keyed by `(stream_id, session_id,
  frame_id)`.
- Deadline-aware retransmission: resend while useful, drop when the playout
  budget is gone.
- Receiver feedback that can NACK missing chunks, request a keyframe, and
  report jitter/decode queue pressure.
- Small playout buffer chosen by policy, for example 8-20 ms on a wired LAN.
- Optional FEC/parity for video chunks where one repair packet beats a resend.
- Separate audio policy that favors continuity and bounded drift over perfect
  packet recovery.

## OBS Lowering

The OBS plugin should lower typed media frames into OBS-friendly video/audio.
The clean target is direct decode from elementary streams using libavcodec or
NVDEC where available, then hand decoded video frames and audio packets to OBS.

Temporary bridge allowed:

- Reconstruct an elementary stream from typed video access units.
- Feed the local FFmpeg source only as a compatibility decoder.
- Keep audio as a separate OBS audio path so audio sync is explicit instead of
  being hidden inside a damaged MPEG-TS stream.

The compatibility bridge must not own stream truth. It is a lowering target,
not the protocol.

## Cut Plan

1. Promote media-frame schemas in Odin/CultLib and add unit tests for
   encode/decode round trips.
2. Teach Muninn activation to emit video access units and audio packets instead
   of arbitrary MPEG-TS byte chunks.
3. Teach CultNet RUDP media receive to reassemble by frame and deadline, with
   explicit NACK/keyframe feedback.
4. Teach the OBS plugin to consume typed media frames and feed OBS through a
   direct decoder or a narrowly named compatibility elementary-stream bridge.
5. Remove the MPEG-TS byte-chunk hot path once the typed receiver is stable.

## Verification

The rebuild is not proven by "OBS shows video once." Minimum proof:

- Raven `serve` remains idle until an explicit capture command arrives.
- OBS request activates Raven without manual Raven terminals.
- Sender logs and receiver telemetry agree on stream id, session id, frame id,
  codec, PTS, deadlines, drops, NACKs, and keyframe requests.
- A forced packet drop causes bounded damage and recovery, not OBS freeze.
- Audio remains present as an OBS audio source and stays within the chosen
  sync budget.
- Restarting OBS does not strand Raven capture or poison the next request with
  stale command receipts.
- Restarting Raven Muninn restores catalog/health without starting capture
  until requested.

