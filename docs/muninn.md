# Muninn

Muninn is Odin's portable local telemetry Verse assembler.

It runs on a device body and publishes a typed CultMesh/CultCache surface for
what that body can sense or expose locally: screen capture, loopback audio,
microphones, cameras, and future sensors.

## Authority

- Owner: Muninn owns local telemetry discovery, stream affordance publication,
  and explicit activation of requested local streams.
- Inputs: cheap local probes, operator or Verse activation requests, and local
  capture tools such as FFmpeg or WASAPI helpers.
- Outputs: `muninn.telemetry_surface.v1` and active `muninn.capture_stream.v1`
  records.
- Forbidden writers: daemon startup, Idunn keepalive, Mimir ingest, OBS, Odin,
  Gjallar, and renderer bodies must not start capture by implication.

## Runtime

```powershell
cargo build -p muninn-daemon
muninn serve --store C:\Meta\Odin\state\muninn.telemetry.cc --interval-seconds 15
muninn --health --store C:\Meta\Odin\state\muninn.telemetry.cc
```

`serve` is cheap and idle. It publishes affordances and keeps the local Verse
surface fresh without starting FFmpeg, screen capture, microphone capture, or
loopback capture.

Muninn also writes an OBS stream catalog as typed CultCache state inside:

```text
C:\Meta\Odin\state\muninn.telemetry.cc
```

The record is type `muninn.obs_stream_catalog` with schema
`muninn.obs_stream_catalog.v1` at key `obs`. On Starfire, `health-muninn.ps1`
syncs Raven's `.cc` store to the same local path so the OBS plugin can read
CultCache directly without learning Raven SSH.

Activation is explicit:

```powershell
.\scripts\activate-muninn-raven-av-srt.ps1
```

That Raven activation starts the requested screen and Realtek loopback stream
and publishes `muninn.capture_stream.v1`. Mimir and OBS are stream consumers;
they do not own Muninn.

The deployed loopback helper must accept Muninn's command contract:

```powershell
wasapi-loopback-capture.ps1 -Output stdout -Device Realtek -SampleRate 48000 -Channels 2
```

`stdout` is an alias for binary standard output, and `Device` is the requested
render-device hint. Current helper builds may ignore the hint and use the
default render endpoint, but they must accept the argument so Muninn's generated
mux command remains executable.
