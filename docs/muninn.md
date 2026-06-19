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
  Quest access records, Move receipt records, and a CultMesh bytes stream
  carrying live Move evidence frames for Mimir.
- Forbidden writers: daemon startup, Idunn keepalive, Mimir ingest, OBS, Odin,
  Gjallar, and renderer bodies must not start capture by implication.

## Runtime

```powershell
cargo build -p muninn-daemon
muninn serve --store C:\Meta\Odin\state\muninn.telemetry.cc --interval-seconds 15 --idunn-rudp-health 127.0.0.1:17870 --idunn-daemon starfire-muninn --idunn-health-contract muninn.cultnet-rudp-local-telemetry-and-quest-access
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

The current Raven A/V path is a compatibility proof, not the final hot media
shape. Its rebuild map lives in
[`docs/muninn-media-streaming.md`](muninn-media-streaming.md): Muninn should
publish encoded video access units and audio packets as typed CultMesh media
frames with frame ids, deadlines, receiver feedback, and explicit audio/video
policy instead of treating MPEG-TS bytes as the transport's unit of truth.

Activation is explicit:

```powershell
.\scripts\activate-muninn-raven-av-srt.ps1
```

That Raven activation starts the requested screen and Realtek loopback stream
and publishes `muninn.capture_stream.v1`. It is still a background-only Raven
actuator: the remote scheduled task must launch through the hidden WScript /
noninteractive PowerShell trampoline and must not create visible terminal
windows on Raven. Mimir and OBS are stream consumers; they do not own Muninn.

## Quest Access And Unity Return Video

Quest hardware attached to Starfire is a Muninn local telemetry surface, not a
Mimir-owned preflight. Enable it by running Muninn on Starfire with ADB probing:

```powershell
muninn serve `
  --store C:\Meta\Odin\state\muninn.telemetry.cc `
  --host starfire `
  --quest-adb `
  --quest-serial 1WMHHB68PG1515
```

When `adb devices -l` reports the Quest as `device`, Muninn publishes
`muninn.quest_access.v1` at `quest-access` and
`muninn:<host>:quest-access:<serial>`. The record advertises:

- `muninn:<host>:quest-input`: Quest buttons/analog/controller input once a
  Quest/OpenXR witness is running.
- `muninn:<host>:quest-poses`: headset and controller poses from that same
  witness.
- `muninn:<host>:quest-warped-video-input`: warp-corrected video frames that
  Brokkr can route from Starfire Unity editor play mode toward the Quest device.

ADB authorization proves local USB access only. It does not by itself expose
OpenXR poses or accept video frames. A Quest/OpenXR witness still owns headset
runtime sampling, while Brokkr owns the Unity editor adapter that sends
warp-corrected play-mode frames to Muninn's advertised Quest video input.

Read the current record with:

```powershell
muninn quest-access-status --store C:\Meta\Odin\state\muninn.telemetry.cc
```

For Starfire's local Quest-attached daemon, Idunn supervises the
`starfire-muninn` target through:

```powershell
E:\Projects\Odin\scripts\restart-starfire-muninn.cmd
E:\Projects\Odin\scripts\health-starfire-muninn.cmd
```

The restart script launches Muninn hidden with `--host starfire --quest-adb`,
`--idunn-rudp-health 127.0.0.1:17870`, `--idunn-daemon starfire-muninn`, and
`--idunn-health-contract muninn.cultnet-rudp-local-telemetry-and-quest-access`.
If the CultCache store at `C:\Meta\Odin\state\starfire.muninn.telemetry.cc`
fails MessagePack decode on boot, the restart path archives the corrupt file,
clears the stale `.lock`, and relaunches the daemon instead of leaving the lane
dead.

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
When `serve` is launched with `--idunn-rudp-health`, `--idunn-daemon`, and
`--idunn-health-contract`, the long-running Muninn body publishes
`idunn.daemon_health` directly to Idunn over `cultnet.transport.rudp.v0` on its
normal cadence. `--health` keeps the same publication path for manual proof and
compatibility probes, but the live owner is the daemon's `serve` process.
Quest ADB probing is a telemetry input, not daemon liveness. If `adb` is
missing or the Quest is unavailable, Muninn publishes `muninn.quest_access` as
`unavailable` and keeps serving the local telemetry surface.

## Host Deployments

Raven runs Muninn from `C:\Meta\Odin\Muninn`. `scripts/restart-muninn.ps1`
recreates the `GameCult-Muninn` scheduled task with `wscript.exe` as the task
action and `start-muninn-serve-hidden.vbs` as its argument. It also repairs
`GameCult-Muninn-Activate` and `GameCult-Muninn-VideoProof` to execute hidden
VBS launchers whose bodies call PowerShell entrypoints directly, not `.cmd`
payloads. The serve VBS launches `start-muninn-serve.ps1` with noninteractive hidden
PowerShell. The PowerShell launcher starts `muninn.exe` with
`-WindowStyle Hidden`, passes `--idunn-rudp-health 192.168.1.66:17870`,
`--idunn-daemon muninn`, and
`--idunn-health-contract muninn.cultnet-rudp-remote-telemetry-health`, and
redirects logs under `C:\Meta\Odin\logs\muninn`.
Raven is an operator-consented host: Muninn operations on Raven must be
background-only and must not create visible terminal windows. `.cmd` files may
exist only as manual compatibility entrypoints that call the same hidden VBS
launchers; neither Task Scheduler nor the hidden VBS layer may route through a
`cmdPath` trampoline on Raven.
The standalone repair actuator for live Raven task drift is
`E:\Projects\Odin\scripts\repair-raven-muninn-task-actions.ps1`. It must
register `GameCult-Muninn`, `GameCult-Muninn-Activate`, and
`GameCult-Muninn-VideoProof` with `wscript.exe` as the task action and the
corresponding `*-hidden.vbs` launcher as arguments while also verifying that the
hidden VBS bodies reference `.ps1` launchers instead of `.cmd` payloads. If
Raven is unreachable, the repo is prepared but the live scheduler is not clean.
Run `scripts\verify-muninn-media-profile.ps1` after changing Raven media
startup scripts; it verifies that the video proof path still uses the same
low-latency NVENC profile as Muninn's RUDP media lane instead of drifting back
to legacy buffered encoder settings.

Nightwing Muninn is kept alive by the single Idunn supervisor through the
`nightwing-muninn` daemon target. Idunn learns that target through Odin's typed
daemon surface and invokes `scripts/health-nightwing-muninn.ps1` and
`scripts/restart-nightwing-muninn.ps1`; those scripts are health/restart
actuators only, not lifecycle owners. The binary is installed at
`/home/metacrat/.local/bin/muninn`, the store lives at
`/home/metacrat/.local/state/gamecult/muninn/muninn.telemetry.cc`, and the
restart actuator launches `serve --host nightwing --interval-seconds 15` with
`--move-state move-usb=/dev/input/by-id/usb-Sony_Computer_Entertainment_Motion_Controller-joystick`,
`--idunn-rudp-health 10.77.0.2:17870`,
`--idunn-daemon nightwing-muninn`,
`--idunn-health-contract muninn.cultnet-rudp-remote-telemetry-and-move-hid`,
PID, and logs under
`/home/metacrat/.local/state/gamecult/muninn`.
With that Move state source attached, `serve` also publishes the
`muninn:nightwing:move-evidence` CultMesh stream frame body that Mimir's native
Move evidence reservoir consumes.
Nightwing source discovery must emit one Move source per controller id. The USB
pairing collection and Bluetooth joystick can expose the same controller as
separate `/dev/input/js*` paths; `nightwing-move-state-sources.sh` prefers the
Bluetooth `HID_ID=0005:0000054C:000003D5` path so Idunn health does not demand
fresh records from two faces of the same controller. The Nightwing health
actuator publishes `nightwing-muninn` health over RUDP to Starfire Idunn at
`10.77.0.2:17870`, but that command is fallback proof only; the live keepalive
contract is now published by the long-running Nightwing `serve` process.
