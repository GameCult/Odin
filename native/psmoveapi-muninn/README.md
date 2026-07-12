# Muninn PSMoveAPI Observer Patch

Muninn owns PS Move LED output. PSMoveAPI owns camera exposure, HSV filtering,
morphology, contour extraction, and marker position estimation.

`expected-color-observer.patch` adds only the missing authority boundary:

- enable tracking without blinking or writing LEDs;
- update the expected camera hue without writing LEDs.

The expected hue comes from Muninn's timestamp-driven `gold(x)` schedule. This
patch must be applied to the pinned PSMoveAPI source with `git apply --recount`
before building the Nightwing tracker libraries.
