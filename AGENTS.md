# Odin Instructions

## Project Purpose

Odin is the all-seer and rendezvous organ for the CultMesh Verse: discovery,
schema awareness, translation routes, and interface aggregation. Every other
GameCult target is observed *through* Odin, which is why its availability is
load-bearing for the estate rather than for itself.

Estate-wide doctrine — the Prime Directive, Code Is A Liability, the Loud
Rebuild Contract, Odin-class architecture, the CultCache/CultNet/CultMesh
substrate rules — lives in `F:\Projects\CLAUDE.md` and `~/.claude/CLAUDE.md` and
is loaded automatically. It is not repeated here.

## Canonical State

- `docs/architecture.md` — the source-grounded architecture note.
- `docs/transport-shortcut-inventory.md` — a **dated ledger**. Entries are left
  as written and corrected by dated notes on top, never rewritten. Follow that
  convention for anything else recording history here.
- `notes/` — compact re-entry packets. Read the newest before touching
  deployment.
- `state/map.yaml` — the current deployment and admission map. This is the one
  file that goes stale fastest; verify it against the host before trusting it.
- `personas/gjallar.persona_state.cc` — Gjallar's Persona state, in the portable
  `gamecult.persona_state.v0` shape.

## Important Paths

Relative to the repository root.

- Daemon: `crates/odin-daemon`
- Shared records and documents: `crates/odin-core`
- Deployment recipe Idunn seals and builds from: `deployment/idunn/recipe.toml`
- Operator binding (lives on the host, not here):
  `/etc/gamecult/idunn/bindings/odin.toml`

## Build And Deploy Target

Odin runs on Linux under Idunn on `yggdrasil`. This workstation is Windows and
is where the shell is, not where the artifact runs: **a local compile is not
evidence about the Linux release.** Build on the target or through the deploy
path's own builder, and say which one you used.

```bash
git archive HEAD | ssh ygg 'sudo rm -rf /srv/build/odin && sudo mkdir -p /srv/build/odin && sudo tar -x -C /srv/build/odin && sudo chown -R gamecultadmin:gamecultadmin /srv/build/odin'
ssh ygg 'sudo docker run --rm -v /srv/build/odin:/w -v /etc/machine-id:/etc/machine-id:ro -v /srv/build/cargo-registry:/usr/local/cargo/registry -v /srv/build/cargo-git:/usr/local/cargo/git -w /w rust:1.95-bookworm cargo test -p odin-core -p odin-daemon'
```

`/etc/machine-id` **must** be mounted or every identity-enrolling test fails
with `Linux machine-id is unavailable` — a wall of failures that looks like
broken code and is a missing mount.

`muninn-psmoveapi-tracker` fails to link in that image for want of
`libpsmoveapi`. It declares no CultLib dependency; scope test runs to the crates
you are changing.

## Session Bootstrap And Re-entry Protocol

On fresh session load, before touching deployment:

1. Read the newest file in `notes/`.
2. Read `state/map.yaml`, then **verify it on the host** — it describes a live
   system and is the first thing to rot.
3. `git log --oneline -5` and `git status --short --branch`.

The host is authoritative for every question of the form "is X running", "is X
admitted", "what release is live". Two commands answer most of them:

```bash
ssh ygg 'systemctl list-units --all "idunn-odin-*" --state=active'
ssh ygg 'sudo /usr/local/bin/idunn status --state-store /var/lib/gamecult/idunn/control.cc'
```

## Operating Discipline

- **Never conclude from a partial listing.** `systemctl list-units | head` sorts
  alphabetically and will hide the one active unit among dozens of failed ones.
  Enumerate completely or state that the answer is unknown.
- **A unit can be `active` and own no process.** `odin.service` in
  `/etc/systemd/system/` is exactly that: active, `MainPID 0`, owning nothing.
  It has misled at least two separate readings. Check `-p MainPID` before
  believing a status line.
- Verify changing facts against the host or source, never against a document —
  including the documents in this repository.
- Redeploying Odin is not routine. Read `notes/` first; the failure mode is
  documented and it is not obvious.
