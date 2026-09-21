# Cathole

A reverse proxy with a QUIC/UDP tunnel and Noise-protected payloads, inspired by
Rathole. This is a working **experimental implementation**. Both endpoints must
run Cathole.

* TCP terminates at each proxy. Each connection uses an independent, reliable
  QUIC bidirectional stream. There is no TCP transport between the proxies.
* UDP uses QUIC DATAGRAM, without application retransmission or ordering.
* Noise NK, KK, or XX protects registration and payloads. NK authenticates the
  server; KK authenticates both static keys; XX requires pinned outer TLS.
  Service tokens authorize the client separately for each declared service.
* Standard QUIC TLS 1.3 remains enabled, with a dedicated trusted server
  certificate. **Noise is layered inside QUIC**, so encryption work is duplicated.
* Native QUIC pacing, congestion control, flow control, PMTU discovery, keepalive,
  idle detection, and validated NAT rebinding come from Quinn.

## Build and run

Requires a current Rust toolchain (tested with Rust 1.97.1).

```sh
cargo build --release --locked
cargo run --release -- --init identity
cargo run --release -- identity/server.toml --check
cargo run --release -- identity/client.toml --check
```

`--init` writes new random keys, a TLS identity, and matching example configs. It
refuses to overwrite an existing directory. Private files use mode 0600 on Unix;
on Windows they inherit the destination directory's ACL. Protect that directory.

Run these in separate terminals, with your TCP/UDP service listening on 5201:

```sh
cargo run --release -- identity/server.toml
cargo run --release -- identity/client.toml
```

The example exposes TCP and UDP on **loopback port 5202**. For a public server,
change the server's tunnel `bind_addr` to its desired interface on UDP 2333,
change the service `bind_addr` values explicitly, and set the client's
`remote_addr` to the server address. Permit the tunnel's **UDP** port through the
firewall, along with the configured public service ports. No TUN device or
administrator privileges are needed for high ports.

Copy only `client.toml` and `server.der` to the client. Do not copy `server.toml`
or `server-key.der`. Transfer the certificate and Noise public key through a
trusted channel. Relative certificate paths resolve against the config file.

`--genkey` prints a fresh Noise keypair. `--server` and `--client` optionally
assert the role; otherwise the TOML section selects it. `RUST_LOG=info` controls
logging. Config files are watched every second; a valid edit **restarts sessions
and interrupts active flows**. Invalid edits leave the running config intact.
Identity file changes require touching/changing the TOML as well.

## Reliability and NAT

The home client initiates one outbound UDP connection to a reachable public
server. Replies and server-initiated QUIC streams share that connection. There is
no separate inbound connection to the home router and no P2P hole punching.
Networks that block UDP will not work; there is deliberately no TCP fallback.

QUIC streams recover missing TCP bytes. UDP packets may be lost, reordered, or
dropped when buffers fill. Incomplete fragmented packets expire after 2 seconds;
no partial packet reaches the application. Fragmentation preserves payloads up
to 65,507 bytes, but small UDP packets are strongly preferable on lossy paths.

The compatibility defaults send a server heartbeat every 30 seconds and declare
the client tunnel dead after 40 seconds; either value can be set to zero to
disable it. An orderly close is detected promptly; a silent black hole takes up
to the negotiated idle timeout. Reconnect retries grow from the configured
`retry_interval` to 30 seconds plus 0–1 second jitter,
resetting after a connection lasts 60 seconds. Existing TCP connections cannot
survive a destroyed tunnel and must reconnect at the application layer.

## Verification

```sh
cargo fmt --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked
cargo build --locked
python tests/e2e.py
python tests/e2e.py --ipv6
python tests/e2e.py --loss 0.005
python tests/e2e.py --loss 0.01
python tests/e2e.py --loss 0.02
python tests/e2e.py --loss 0.05
```

The Python harness creates fresh identities under `target/`, runs only loopback
listeners, and cleans up its processes. Loss/reordering is applied to the outer
UDP packets in both directions. It checks TCP integrity and half-close,
concurrent streams, UDP boundaries, source isolation, wrong credentials,
NAT port rebinding, process restart, and configuration reload. These are
correctness tests, not WAN throughput measurements.

See [architecture](docs/architecture.md), [protocol](docs/protocol.md),
[migration](docs/migration.md), [benchmarking](docs/benchmarking.md), and
[security review](docs/security.md). Deployment examples are in `deploy/`.
Measured local results and checks are in [validation](docs/validation.md).

## Current limits

Only TCP and UDP services are implemented. Raw IP, ICMP, Ethernet, P2P discovery,
TURN/STUN, reliable-UDP service mode, seamless reload, and connection resumption
across process restart are not implemented. IPv4 and IPv6 are supported; DNS
respects `prefer_ipv6` and otherwise chooses the first matching address, without
Happy Eyeballs.

Noise sessions have a 24-hour maximum lifetime; renewal closes existing flows.
UDP Noise keys also have a hard 2^32-packet sending budget. Seamless key rotation
and long-lived flow preservation need more work before production deployment.
There is no external security audit and no WAN performance result yet.
