# Muninn controllable video encoder

This process owns desktop capture frames and the long-lived NVENC context for
Muninn's realtime video leg. Annex-B H.264 is written to stdout. Stdin accepts
one command per line:

- `IDR` marks the next submitted frame `AV_PICTURE_TYPE_I`. With NVENC
  `forced-idr=1`, FFmpeg lowers that request to an IDR rather than an ordinary
  intra frame.
- `BITRATE <kbps>` reconfigures average/max bitrate and the two-frame VBV on
  the next submitted frame. FFmpeg's NVENC backend performs an in-session
  `nvEncReconfigureEncoder` and forces an IDR for the rate transition.
- `QUIT` ends capture cleanly.

The process does not own transport, audio, packetization, deadlines, or repair.
Muninn owns those and writes `IDR` only when receiver feedback proves the decode
chain is invalid.

Build against the same FFmpeg development snapshot shipped with OBS:

```powershell
cmake -S . -B build -DFFMPEG_ROOT=E:/path/to/obs-deps
cmake --build build --config Release
```

`--input` defaults to `ddagrab=framerate=60:output_idx=0:draw_mouse=1`.
`--force-idr-frame N` exists only for deterministic encoder verification.
`--frames N` bounds deterministic verification runs.
