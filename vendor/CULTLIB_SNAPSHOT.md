# CultLib experimental snapshot

Odin's vendored Rust CultCache/CultNet body is synchronized from:

- repository: `https://github.com/GameCult/CultLib.git`
- local runtime source: `E:\Projects\CultLib-dev-runtime`
- commit: `8965f3c0e0e0082b76e2001772bf1fe600f386f4`
- commit title: `Authenticate provider session ingress`
- containing experimental branches at synchronization time:
  `origin/codex/cultmesh-reliability-control-plane` and
  `origin/codex/cultmesh-mapped-content`

This is the experimental CultMesh/CultNet body used by the live VoidBot runtime
and is ancestral to the CultLib Unity 1.0.15 package consumed by Aetheria. Odin
vendors `packages/cultcache-rs` and `packages/cultnet-rs` because the CultLib
repository is not a Cargo workspace at its root and Cargo cannot consume those
subdirectory crates as one portable git dependency.

Do not patch the vendored transport as an Odin-private fork. Make transport
changes in CultLib's experimental branch, verify them there, then resynchronize
the two package trees and update this receipt.

One local adapter remains in `cultcache-rs`: `put_raw_envelope` lets Odin's
runtime document registry admit a dynamically typed envelope after registration.
It does not alter persistence or transport semantics. Promote this narrow API to
CultLib before the next snapshot refresh so the vendored package can become an
exact copy.
