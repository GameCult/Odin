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
  Move receipt records, and a CultMesh bytes stream carrying live Move evidence
  frames for Mimir.
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
Muninn also publishes USB controller facts as
`muninn.move_controller_state.v1`: accelerometer, gyro, magnetometer, button,
trigger, battery, sequence, and source timestamp. Mimir is the consumer of both
sensor streams. Odin may discover and project the schemas, but Odin does not
own raw capture, candidate extraction, calibration, triangulation, IMU fusion,
prediction, or final 6DoF pose. Muninn does not synthesize wand pose; it
reports what the local body saw and read from USB.

The hot tracking path is a CultMesh stream, not CultCache polling. When
`serve` has one or more `--move-state` sources, it declares
`muninn:<host>:move-evidence` in Verse `mimir-live` and publishes
MessagePack `mimir.muninn_move_evidence_stream_frame.v1` bytes through a
shared-memory frame ring. The frame contains any marker candidates available
from local optical extraction plus the controller states read from USB. The
`muninn.move_marker_candidate.v1` and `muninn.move_controller_state.v1` records
remain receipts/debug state in the `.cc` store; Mimir drinks the stream and
owns association, calibration, fusion, prediction, and final pose.

On Linux hosts, enable the controller-state feed by passing one or more Move
joystick sources to `serve`:

```bash
muninn serve \
  --store ~/.local/state/gamecult/muninn/muninn.telemetry.cc \
  --host nightwing \
  --move-state move-usb=/dev/input/by-id/usb-Sony_Computer_Entertainment_Motion_Controller-joystick
```

The published values are raw Linux joystick/HID counts. Mimir owns calibration,
axis interpretation, unit conversion, fusion, prediction, and resolved pose
publication. Hidraw remains the local output path for LED reports.
Use `--move-evidence-stream <stream-id>` or `--move-evidence-verse <verse-id>`
only when the default `muninn:<host>:move-evidence` / `mimir-live` address is
not the desired Mimir-facing stream identity.

## Move Light Commands

Muninn is also the local output owner for USB-attached PS Moves. When Mimir
wants structured light pulses for calibration or tracking, it publishes a typed
`muninn.move_light_command.v1` command over CultNet/CultMesh to the Muninn
daemon on the host that owns the Move. `serve` consumes `pending` commands
whose `host_id` matches the local Muninn host, writes PS Move HID report `0x06`
to the command's `hidraw_path`, and updates the same command record to
`running`, `completed`, or `failed`.

For operator smoke and bring-up, `request-move-light` writes that typed command
into the local Muninn store without touching HID directly:

```bash
muninn request-move-light \
  --store ~/.local/state/gamecult/muninn/muninn.telemetry.cc \
  --host nightwing \
  --move move-usb \
  --hidraw /dev/hidraw1 \
  --color 35ff6c \
  --duration-ms 0 \
  --repeat-count 1

muninn move-light-status \
  --store ~/.local/state/gamecult/muninn/muninn.telemetry.cc \
  --host nightwing
```

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

Nightwing Muninn is kept alive by the single Idunn supervisor through the
`nightwing-muninn` daemon target. Idunn learns that target through Odin's typed
daemon surface and invokes `scripts/health-nightwing-muninn.ps1` and
`scripts/restart-nightwing-muninn.ps1`; those scripts are health/restart
actuators only, not lifecycle owners. The binary is installed at
`/home/metacrat/.local/bin/muninn`, the store lives at
`/home/metacrat/.local/state/gamecult/muninn/muninn.telemetry.cc`, and the
restart actuator launches `serve --host nightwing --interval-seconds 15` with
`--move-state move-usb=/dev/input/by-id/usb-Sony_Computer_Entertainment_Motion_Controller-joystick`,
PID, and logs under
`/home/metacrat/.local/state/gamecult/muninn`.
With that Move state source attached, `serve` also publishes the
`muninn:nightwing:move-evidence` CultMesh stream frame body that Mimir's native
Move evidence reservoir consumes.
