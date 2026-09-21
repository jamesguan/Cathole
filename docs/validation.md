# Local validation — 2026-09-20

Windows, Rust 1.97.1, release build with LTO and snow's ring-accelerated backend.
This is a development smoke test, not an independent security audit or WAN
capacity certification. No production server was changed.

## Checks

- `cargo fmt --check`
- `cargo clippy --locked --offline --all-targets -- -D warnings`
- `cargo test --offline`: 10 unit tests
- Release build and black-box TCP/UDP tests on IPv4 and IPv6
- Outer UDP loss/reordering tests: 0%, 0.5%, 1%, 2%, 5%
- TCP bulk integrity, half-close and 24 concurrent connections
- UDP empty packets, boundary sizes, 65,507-byte fragmentation and source isolation
- Rejection of wrong service tokens, Noise pins and TLS trust roots
- Legacy `type = "noise"` configs without QUIC certificate tables using NK and
  mutual-static-key KK, plus certificate-pinned XX
- NAT source-port rebinding, killed-server reconnect, invalid reload retention,
  live service addition and removal

TCP passed with intact data in impaired runs. Some UDP packets were lost at
nonzero loss settings, as expected. The suite does not require unreliable UDP
to deliver every packet. Test configuration uses 2-second keepalives and an
8-second idle timeout to exercise black-hole recovery quickly.

## Loopback throughput smoke result

Command: `python benchmarks/local.py`. Each stream echoed 32 MiB through a
Python backend on the same host. Throughput counts bytes in one direction;
the echo sends the same bytes back. All processes share this machine. The
reported number includes startup/ramp-up and is a single sample per condition.

| Concurrent streams | Direct echo | Cathole echo |
| ---: | ---: | ---: |
| 1 | 1.419 Gbps | 0.535 Gbps |
| 4 | 10.304 Gbps | 1.671 Gbps |
| 16 | 13.583 Gbps | 1.942 Gbps |

Cathole persistent-connection 64-byte TCP median RTT was approximately
0.08–0.11 ms. All 100 UDP echo probes returned in each condition. Full JSON
is in [benchmark-results-local.json](benchmark-results-local.json).

The aggregate local result exceeds 1 Gbps with multiple streams. The single
stream does not meet 1 Gbps in this harness. These measurements do not establish
WAN capacity or performance relative to other proxies.

Before performance conclusions, run repeated longer tests over the user's
actual 1 Gbps path with equivalent proxy and direct baselines, including
CPU/RSS observations. The Python echo process and local TCP stack may themselves
limit the measurement. Linux systemd, Docker, Internet NAT/firewalls, and long
duration soak tests have not been executed here.
