# Muninn Move Tracker

Muninn owns PS Move sensor stream exposure. This crate is Muninn's first native
optical marker candidate extractor: Rust owns orchestration, FFI, validation,
and the CPU mirror; the compute shader owns the parallel image reduction stage.

This crate deliberately stops at per-frame marker candidates. Final pose,
stereo triangulation, controller association, IMU fusion, prediction, and
calibration remain separate later owners. Mimir consumes Muninn-emitted typed
candidate/observation streams; it does not own raw optical extraction.

## Current Contract

- Input: one luma frame (`Y8`) with width, height, stride, source hash, and frame
  sequence.
- GPU stage: one 16px tile per thread group, emitting at most one candidate per
  tile.
- Output: `MoveMarkerCandidate` records containing weighted centroid, radius,
  area, peak/mean luma, and score.
- Rust mirror: deterministic CPU implementation and FFI wrappers for tests and
  non-GPU smoke paths.
- FFI prefix: `muninn_move_tracker_*`.
- Shader IDs are carried as `uint2` low/high pairs to avoid making 64-bit HLSL
  integer support a dependency of the first extraction pass.

This is the first stage required for PlayStation-class wand tracking. The
remaining stack needs calibrated camera rays, controlled sphere colors/exposure,
multi-camera association, triangulation, IMU fusion, and latency prediction.
