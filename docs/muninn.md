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
- Outputs: `muninn.telemetry_surface.v1`, active `muninn.capture_stream.v1`,
  and Move optical `muninn.move_marker_candidate.v1` records.
- Forbidden writers: daemon startup, Idunn keepalive, Mimir ingest, OBS, Odin,
  Gjallar, and renderer bodies must not start capture by implication.

## Runtime

```powershell
cargo build -p muninn-daemon
muninn serve --store C:\Meta\Odin\state\muninn.telemetry.cc --interval-seconds 15
muninn --health --store C:\Meta\Odin\state\muninn.telemetry.cc
```

`serve` is cheap and idle. It publishes affordances, consumes addressed pending
`muninn.move_light_command.v1` records, and keeps the local Verse surface fresh
without starting FFmpeg, screen capture, microphone capture, or loopback
capture.

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

## Move Marker Candidates

`crates/muninn-move-tracker` is Muninn's native/Rust PS Move optical candidate
extractor. It owns dispatch planning, FFI, configuration validation, a CPU
mirror, and the HLSL 16px-tile luma reduction shader. It emits marker
candidates only: weighted centroid, radius, area, peak/mean luma, and score for
one camera frame.

Muninn owns publishing those candidates as `muninn.move_marker_candidate.v1`.
Mimir is a consumer of the resulting sensor stream. Odin may discover and
project the schema, but Odin does not own raw capture, candidate extraction,
calibration, triangulation, IMU fusion, prediction, or final 6DoF pose.

## Move Light Commands

Muninn is also the local output owner for USB-attached PS Moves. When Mimir
wants structured light pulses for calibration or tracking, it publishes a typed
`muninn.move_light_command.v1` command over CultNet/CultMesh to the Muninn
daemon on the host that owns the Move. `serve` consumes `pending` commands
whose `host_id` matches the local Muninn host, writes PS Move HID report `0x06`
to the command's `hidraw_path`, and updates the same command record to
`running`, `completed`, or `failed`.

Idunn keeps the Muninn daemon alive. Idunn does not learn a Move-specific
watcher, and Mimir does not write HID directly except through temporary smoke
scripts used to prove hardware behavior before a Muninn daemon is available.

## Host Deployments

Raven runs Muninn from `C:\Meta\Odin\Muninn`. `scripts/restart-muninn.ps1`
recreates the `GameCult-Muninn` scheduled task and writes
`start-muninn-serve.cmd` as a short trampoline into
`start-muninn-serve.ps1`; the PowerShell launcher starts `muninn.exe` with
`-WindowStyle Hidden` and redirects logs under `C:\Meta\Odin\logs\muninn`.
The `.cmd` file must not be the long-lived foreground process.

Nightwing runs Muninn as the user service `gamecult-muninn.service`. The binary
is installed at `/home/metacrat/.local/bin/muninn`, the store lives at
`/home/metacrat/.local/state/gamecult/muninn/muninn.telemetry.cc`, and the
service command uses `--host nightwing --interval-seconds 15`.
