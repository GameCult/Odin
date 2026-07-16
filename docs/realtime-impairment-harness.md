# Realtime impairment harness

`cultnet-impair` is a deterministic bidirectional UDP proxy for pressure-testing
actual CultNet RUDP sessions. It owns transport impairment only. Muninn,
Sleipnir, and their consumers remain the owners of frame, input, recovery, and
presentation semantics.

The proxy listens at the endpoint advertised to a client. A second UDP socket
connects to the real server. Client datagrams and reverse ACK/feedback traffic
therefore cross the same impairment scheduler. The proxy supports one active
client, which matches one isolated test lane. It is not a relay daemon or a
deployment surface.

Build and run:

```powershell
cargo build -p cultnet-impair --release
.\target\release\cultnet-impair.exe `
  --listen 0.0.0.0:17890 `
  --upstream 127.0.0.1:17990 `
  --profile .\tests\realtime-impairment\loss-1pct.toml `
  --seed 424242 `
  --metrics .\artifacts\realtime-impairment\loss-1pct.csv
```

The real receiver must bind the upstream endpoint (`17990` in the example),
while discovery or the test client uses the proxy endpoint (`17890`). Do not
reuse a port for both bodies. For a two-host test, run the proxy on the receiver
host so reverse RUDP traffic also crosses it.

Profiles are a deliberately small TOML subset with unsigned integer values:

- `loss_basis_points`: seeded independent loss, 100 = 1%.
- `burst_every` and `burst_length`: begin a consecutive drop burst on every Nth
  received datagram.
- `duplicate_every`: forward every Nth datagram twice.
- `reorder_every` and `reorder_delay_ms`: hold every Nth datagram so later
  traffic can pass it.
- `delay_ms` and `jitter_ms`: fixed delay plus seeded uniform jitter.
- `stall_at_ms` and `stall_for_ms`: drop all traffic in one elapsed-time window.
- `max_scheduled`: hard queue bound. Admission beyond it is dropped and counted.

The same deterministic schedule currently applies in both directions. Metrics
are transport witness data, not runtime truth: received, forwarded, dropped,
duplicated, reordered, stalled, queue overflow, and maximum scheduled depth.
Semantic acceptance must be read from the owning Muninn, OBS, and Sleipnir
receipts.

The proxy intentionally does not guess daemon ports, launch services, mutate
Odin discovery, or claim glass-to-glass latency. An orchestration layer belongs
after fixed test endpoints and deterministic media/input fixtures exist.
