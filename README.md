# Cathole

A reverse proxy with QUIC or TCP+Noise tunnels, inspired by Rathole. This is a
working **experimental implementation**. Both endpoints must run Cathole.

* TCP terminates at each proxy. Over QUIC, each connection is an independent
  bidirectional stream of raw bytes (QUIC TLS encrypts them). Over `tcp`/`noise`,
  each visitor gets a dedicated Noise-encrypted TCP **data channel** (Rathole-style),
  while UDP and signaling stay on one control connection.
* UDP uses QUIC DATAGRAM or control-channel mux frames, without application
  retransmission.
* Noise NK, KK, or XX authenticates the tunnel. QUIC payloads are not
  Noise-encrypted a second time. TCP tunnels keep Noise AEAD on control and data
  channels so the path is still encrypted.
* A pinned QUIC/TLS certificate is optional for NK and KK and required for XX.
  Service tokens authorize the client separately for each declared service.
* Native QUIC pacing, congestion control, flow control, PMTU discovery, keepalive,
  idle detection, and validated NAT rebinding come from Quinn. The process uses a
  multi-thread Tokio runtime (`TOKIO_WORKER_THREADS` overrides the worker count).

### TCP-over-TCP

Cathole is an **application byte-stream** reverse proxy, not a packet tunnel. It
does **not** encapsulate TCP segments or IP packets inside another TCP (the
OpenVPN-TCP style failure mode where both stacks retransmit the same loss).

Public and backend TCP connections **terminate** at each proxy. Only application
bytes cross the middle hop:

`client ↔ proxy` → `encrypted tunnel` → `proxy ↔ backend`

| `transport.type` | Middle hop | Classic TCP-over-TCP meltdown? |
| --- | --- | --- |
| `quic` | QUIC over UDP | Avoided; the tunnel is not TCP |
| `noise` / `tcp` / `tls` | TCP + Noise (per-visitor data channels) | Avoided; byte relay like Rathole/SSH, not nested TCP |

On `noise`/`tcp` you still carry TCP *application* streams over a TCP tunnel
(same model as Rathole). Use `type = "quic"` when you want the middle hop off
TCP entirely.

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

When using the certificate-pinned files from `--init`, copy only `client.toml`
and `server.der` to the client. Do not copy `server.toml` or `server-key.der`.
Transfer the certificate and Noise public key through a trusted channel. Relative
certificate paths resolve against the config file.

## Configuration

Cathole uses the familiar `[server]`, `[client]`, and named service layout.

* `transport.type = "quic"` — QUIC/UDP, session Noise auth, QUIC TLS on the data path.
* `transport.type = "noise"` or `"tcp"` — control channel plus per-visitor TCP+Noise
  data channels (Rathole-style).
* `transport.type = "tls"` — the same model wrapped in TLS.
* `websocket` remains unsupported.

Both sides must run Cathole and must declare matching service names, types, and
tokens.

### Minimal NK configuration without certificates

NK is the simplest migration path. Generate one X25519 keypair:

```sh
cathole --genkey
```

Keep the private key on the server and put its public key on the client. No
certificate files are needed. QUIC still requires TLS internally, so the server
creates an ephemeral outer certificate; the pinned Noise key provides the server
identity before Cathole sends a token or application payload.

`server.toml`:

```toml
[server]
# Public UDP tunnel listener. Permit UDP/2000 through the firewall.
bind_addr = "0.0.0.0:2000"
default_token = "replace-with-a-long-random-token"

[server.transport]
type = "noise"

[server.transport.noise]
pattern = "Noise_NK_25519_ChaChaPoly_BLAKE2s"
local_private_key = "SERVER_PRIVATE_KEY_FROM_GENKEY"

# A public TCP listener forwarded to the matching client service.
[server.services.cheatbuy]
type = "tcp"                         # Optional: TCP is the default.
bind_addr = "0.0.0.0:6969"

[server.services.immich]
type = "tcp"
bind_addr = "0.0.0.0:2283"

# TCP and UDP can use the same numeric port.
[server.services.game_udp]
type = "udp"
bind_addr = "0.0.0.0:2456"
```

`client.toml`:

```toml
[client]
remote_addr = "tunnel.guanjames.com:2000"
default_token = "replace-with-the-same-long-random-token"

[client.transport]
type = "noise"

[client.transport.noise]
pattern = "Noise_NK_25519_ChaChaPoly_BLAKE2s"
remote_public_key = "SERVER_PUBLIC_KEY_FROM_GENKEY"

[client.services.cheatbuy]
local_addr = "0.0.0.0:6969"

[client.services.immich]
local_addr = "0.0.0.0:2283"

[client.services.game_udp]
type = "udp"
local_addr = "127.0.0.1:2456"
```

For compatibility, an NK client may contain `local_private_key`, but NK does not
use a client static key. `0.0.0.0` or `[::]` in a client `local_addr` is treated
as local loopback; using `127.0.0.1` or `[::1]` states the intent more clearly.

Validate and start the pair:

```sh
cathole server.toml --check
cathole client.toml --check
cathole server.toml
cathole client.toml
```

### Complete server example

This example shows every server field. Values shown are the defaults unless the
comment says otherwise. A few compatibility fields are accepted but meaningful
only on the client, as marked below.

```toml
[server]
bind_addr = "0.0.0.0:2000"           # Required QUIC/UDP listener.
default_token = "shared-default"      # Fallback for services without token.
prefer_ipv6 = false                   # Prefer IPv6 when a hostname has both.
heartbeat_interval = 30               # QUIC keepalive seconds; 0 disables it.
heartbeat_timeout = 40                # Accepted here; meaningful on the client.
retry_interval = 1                    # Accepted here; meaningful on the client.

[server.transport]
type = "noise"                        # "noise" and "quic" are equivalent.

[server.transport.tcp]
nodelay = true                         # Default for forwarded TCP sockets.
# proxy = "..."                       # Rejected: a TCP proxy cannot carry UDP.
keepalive_secs = 20                    # Accepted migration placeholder; not applied.
keepalive_interval = 8                 # Accepted migration placeholder; not applied.

[server.transport.noise]
pattern = "Noise_NK_25519_ChaChaPoly_BLAKE2s"
local_private_key = "SERVER_NOISE_PRIVATE_KEY"
# remote_public_key is required here only for KK.

[server.transport.quic]
# These two files are optional for NK/KK and mandatory as a pair for XX.
certificate = "server.der"            # DER X.509 certificate.
private_key = "server-key.der"         # DER PKCS#8 private key.
hostname = "cathole"                   # Certificate DNS name.
keep_alive_interval = 15               # Overridden by heartbeat_interval here.
max_idle_timeout = 60                  # Seconds; 0 disables QUIC idle timeout.
udp_idle_timeout = 60                  # UDP source mapping lifetime; 1..86400.
max_connections = 128                  # Simultaneous tunnel clients; 1..4096.
max_streams = 256                      # Concurrent forwarded TCP streams; 1..4096.
max_udp_flows = 4096                   # UDP source mappings; 1..65536.
stream_receive_window = 16777216       # Per-stream QUIC receive window.
receive_window = 33554432              # Connection QUIC receive window.
send_window = 33554432                 # Connection QUIC send window.
socket_buffer = 4194304                # Requested OS UDP send/receive buffers.

[server.services.web]
type = "tcp"
bind_addr = "0.0.0.0:443"             # Required public service listener.
token = "web-specific-token"           # Overrides default_token.
prefer_ipv6 = false                    # Used when bind_addr is a hostname.
nodelay = true                         # Overrides transport.tcp.nodelay.
retry_interval = 1                     # Accepted; meaningful on client services.

[server.services.dns]
type = "udp"
bind_addr = "0.0.0.0:53"
token = "dns-specific-token"
```

### Complete client example

```toml
[client]
remote_addr = "tunnel.example.com:2000" # Required server UDP endpoint.
default_token = "shared-default"
prefer_ipv6 = false
heartbeat_timeout = 40                 # Dead-tunnel timeout; 0 disables it.
heartbeat_interval = 30                # Accepted here; meaningful on the server.
retry_interval = 1                     # Initial reconnect/local-connect delay.

[client.transport]
type = "noise"

[client.transport.tcp]
nodelay = true

[client.transport.noise]
pattern = "Noise_NK_25519_ChaChaPoly_BLAKE2s"
remote_public_key = "SERVER_NOISE_PUBLIC_KEY"

[client.transport.quic]
# Omit trusted_root for certificate-free NK/KK. It is required for XX.
trusted_root = "server.der"
hostname = "cathole"                   # Must match the certificate identity.
keep_alive_interval = 15               # Sends traffic that keeps NAT state alive.
max_idle_timeout = 60                  # Overridden by heartbeat_timeout here.
udp_idle_timeout = 60
max_connections = 128                  # Server-side limit; accepted here too.
max_streams = 256
max_udp_flows = 4096
stream_receive_window = 16777216
receive_window = 33554432
send_window = 33554432
socket_buffer = 4194304

[client.services.web]
type = "tcp"
local_addr = "127.0.0.1:8443"          # Required local destination.
token = "web-specific-token"
prefer_ipv6 = false
nodelay = true
retry_interval = 1

[client.services.dns]
type = "udp"
local_addr = "127.0.0.1:53"
token = "dns-specific-token"
```

`default_token` is optional only when every service has its own nonempty `token`.
Tokens must match for the same service on both sides. Service names are 1–128
bytes, tokens are 1–512 bytes, and each side accepts 1–256 services. Unknown
fields are errors so misspellings do not silently change security or behavior.

The side-level compatibility fields take precedence over two QUIC fields:
`server.heartbeat_interval` sets the server QUIC keepalive, and
`client.heartbeat_timeout` sets the client QUIC idle timeout. Use those familiar
fields for these two settings.

### Noise authentication choices

| Pattern | Server Noise fields | Client Noise fields | QUIC certificate |
| --- | --- | --- | --- |
| NK | `local_private_key` | server `remote_public_key` | Optional |
| KK | server `local_private_key`, client `remote_public_key` | client `local_private_key`, server `remote_public_key` | Optional |
| XX | No static key fields required | No static key fields required | Required and pinned by client |

For KK, generate two keypairs. Each endpoint keeps its own private key and pins
the other endpoint's public key:

```toml
# server.toml
[server.transport.noise]
pattern = "Noise_KK_25519_ChaChaPoly_BLAKE2s"
local_private_key = "SERVER_PRIVATE_KEY"
remote_public_key = "CLIENT_PUBLIC_KEY"

# client.toml
[client.transport.noise]
pattern = "Noise_KK_25519_ChaChaPoly_BLAKE2s"
local_private_key = "CLIENT_PRIVATE_KEY"
remote_public_key = "SERVER_PUBLIC_KEY"
```

XX has no preconfigured Noise static keys, so the outer certificate supplies the
server identity:

```toml
# server.toml
[server.transport.noise]
pattern = "Noise_XX_25519_ChaChaPoly_BLAKE2s"
[server.transport.quic]
certificate = "server.der"
private_key = "server-key.der"
hostname = "cathole"

# client.toml
[client.transport.noise]
pattern = "Noise_XX_25519_ChaChaPoly_BLAKE2s"
[client.transport.quic]
trusted_root = "server.der"
hostname = "cathole"
```

`--init identity` is the easiest way to create a certificate-pinned NK setup. It
writes the DER certificate, PKCS#8 key, random Noise keypair, random token, and
matching example configs. Cathole currently generates only X25519 Noise keys.

### One file containing both roles

A TOML file may contain both `[server]` and `[client]` trees. Select which role
the process runs with `--server`/`-s` or `--client`/`-c`:

```sh
cathole combined.toml --server
cathole combined.toml --client
```

Role selection is mandatory for a combined file. It is unnecessary when the file
contains only one role.

### Command-line options

```text
cathole [OPTIONS] [CONFIG]
```

| Option | Purpose |
| --- | --- |
| `CONFIG` | TOML file to validate or run. Relative identity paths resolve from this file's directory. |
| `--check` | Parse the config, validate Noise keys and build the TLS/QUIC identity, then exit without opening configured service listeners. |
| `--genkey [x25519]` | Print a new Base64 Noise private/public keypair. The curve argument is optional and currently only `x25519` is supported. |
| `--init DIR` | Create `server.toml`, `client.toml`, `server.der`, and `server-key.der` with fresh random credentials. `DIR` must not already exist. |
| `-s`, `--server` | Run the `[server]` tree. Required when both roles are in one file. |
| `-c`, `--client` | Run the `[client]` tree. Required when both roles are in one file. |
| `-h`, `--help` | Print command help. |
| `-V`, `--version` | Print the Cathole version. |

`--server` and `--client` are mutually exclusive. When using `cargo run`, the
first `--` separates Cargo's arguments from Cathole's arguments, for example
`cargo run --release -- client.toml --check`. Set `RUST_LOG=debug` or another
standard tracing filter for more detailed logs.

### Unsupported legacy transport settings

`transport.type = "websocket"` is rejected. `[transport.tcp].proxy` is rejected
because an HTTP or SOCKS TCP proxy cannot carry the tunnel. PKCS#12 identities
are rejected; use DER `certificate` / `private_key` in `[transport.quic]`.

For completeness, the migration-only tables parse in this form:

```toml
[client.transport.tls]
hostname = "legacy.example.com"
trusted_root = "legacy-root.pem"
pkcs12 = "legacy-identity.p12"
pkcs12_password = "legacy-password"

[client.transport.websocket]
tls = false
```

These values are ignored. They should normally be removed rather than copied to
a new Cathole configuration.

`--genkey` prints a fresh Noise keypair. `--server` and `--client` optionally
assert the role; otherwise the TOML section selects it. `RUST_LOG=info` controls
logging. Config files are watched every second; a valid edit **restarts sessions
and interrupts active flows**. Invalid edits leave the running config intact.
Identity file changes require touching/changing the TOML as well.

## Reliability and NAT

The home client initiates one outbound connection to a reachable public
server. `transport.type = "quic"` uses UDP; `tcp`, `tls`, and Rathole-style
`noise` use TCP. Replies and server-initiated streams share that connection.
There is no P2P hole punching. Networks that block UDP still work with the TCP
transports.

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

## CPU profiling with samply

Build a release binary that still has line-table symbols:

```sh
cargo build --profile profiling
```

Record a running Cathole process with [samply](https://github.com/mstange/samply).
On Linux, samply uses perf events. On Windows it uses ETW and needs the
[Windows Performance Toolkit](https://learn.microsoft.com/en-us/windows-hardware/test/wpt/)
(`xperf`) plus an elevated shell:

```sh
# Linux / macOS
samply record -- ./target/profiling/cathole identity/server.toml

# Windows (elevated), attach to an already-running process
samply record -p <pid> -d 20 --save-only -o profile.json.gz --no-open
samply load profile.json.gz
```

Override worker-thread count with `TOKIO_WORKER_THREADS` when comparing profiles.

## Docker comparison benchmark

The fully automated benchmark compares direct networking, Rathole Noise over
TCP, WireGuard over UDP, FRP, and Cathole TCP+Noise against the same
multi-threaded Axum HTTP backend. It runs correctness, latency, a 64 KiB
saturation sweep, `wrk -c 1000` stress, an Emby-style 10 GiB HTTP/1.1 stream
plus Range seeks, raw **TCP echo** and **UDP echo** packet probes, and
**HTTPS** requests (TLS on the backend, TCP-forwarded by proxies).

Run it with:

```powershell
.\benchmarks\docker\run.ps1
```

or:

```sh
./benchmarks/docker/run.sh
```

The harness, topology, pinned versions, tunable settings, and interpretation
guidance are documented in [benchmarks/docker](benchmarks/docker/README.md).
Targets are started one at a time so idle proxy stacks cannot skew CPU or
network results. Each run writes ranked `BENCHMARK-RESULTS.md` /
`BENCHMARK-RESULTS.json` at the repository root and replaces the block below.

The tables below are from 2026-09-21T17:31:12Z. Cathole used multiplexed control
plus Rathole-style **per-visitor TCP+Noise data channels** (`transport.type = "noise"`),
a **256-slot warm pool** for plain TCP services (HTTP / TCP echo; not HTTPS), and
reliable (awaited) `CREATE_DATA`. All five targets were measured in one clean
isolated run including TCP echo, UDP echo, and HTTPS. Prior runs are archived
under `benchmarks/docker/results/history/`.

Percent change is `(new − baseline) / baseline`. Positive means Cathole is
faster. The previous Cathole baseline is the same-day single-mux run
(2026-09-21T05:43:52Z).

| Metric | Cathole now | vs Rathole | vs prior Cathole (mux) |
| --- | ---: | ---: | ---: |
| Saturation peak (64 KiB wrk) | 3633 MiB/s | **+41%** (2572) | **+530%** (577) |
| Latency RPS (hello, 64 conn) | 106k | **+7%** (99k) | **+490%** (18k) |
| Stress RPS (hello, 1000 conn) | 177k | −9% (193k) | **+881%** (18k) |
| Emby 10 GiB stream | 671 MiB/s | **+163%** (255) | −21% (853) |
| Seek (16×8 MiB Range) | 533 MiB/s | **+258%** (149) | **+12%** (474) |
| TCP echo bulk | 314 MiB/s | **+19%** (264) | — |
| UDP echo loss | 0% (200/200) | tied | — |
| HTTPS request RPS | 97 | tied (~97) | — |

<!-- docker-benchmark-results:start -->
# Docker comparison benchmark results

Run at: 2026-09-21T17:31:12Z

Stacks were started and measured **one target at a time**. Other reverse-proxy and VPN containers were stopped during each run so CPU, sockets, and the Docker bridge were not shared.

## Overall ranking

| Rank | Target | Score | Throughput | Request rate | Latency | Reliability | Correctness |
| ---: | --- | ---: | ---: | ---: | ---: | ---: | --- |
| 1 | direct | 5 | #1 | #1 | #1 | #1 | PASS |
| 2 | cathole | 3.75 | #2 | #2 | #2 | #3 | PASS |
| 3 | rathole | 3.25 | #3 | #3 | #3 | #2 | PASS |
| 4 | frp | 1.6 | #4 | #5 | #5 | #4 | PASS |
| 5 | wireguard | 1.4 | #5 | #4 | #4 | #5 | PASS |

Score weights: saturation throughput 35%, small-response request rate 20%, p50 latency 20%, error rate 25%. A correctness failure ranks last on reliability.

## Speed: saturated 64 KiB transfer

| Rank | Target | Peak MiB/s | At connections | p50 ms | p99 ms |
| ---: | --- | ---: | ---: | ---: | ---: |
| 1 | direct | 19570.14 | 1000 | 2.21 | 9.78 |
| 2 | cathole | 3633.24 | 64 | 1.04 | 2.04 |
| 3 | rathole | 2572.43 | 1000 | 16.73 | 231.67 |
| 4 | frp | 251.89 | 256 | 63.81 | 240.5 |
| 5 | wireguard | 54.8 | 64 | 62.76 | 391.92 |

## Speed: small-response request rate

| Rank | Target | Latency RPS | p50 ms | p99 ms | Stress RPS |
| ---: | --- | ---: | ---: | ---: | ---: |
| 1 | direct | 333766 | 0.16 | 0.52 | 866651 |
| 2 | cathole | 106259 | 0.6 | 1.03 | 176531 |
| 3 | rathole | 99208 | 0.61 | 1.36 | 193043 |
| 4 | wireguard | 40026 | 1.58 | 2.01 | 48102 |
| 5 | frp | 10015 | 6.59 | 8.27 | 8932 |

## Reliability

| Rank | Target | Correctness | Error rate | Timeouts | Connect | Read | Write | Status |
| ---: | --- | --- | ---: | ---: | ---: | ---: | ---: | ---: |
| 1 | direct | PASS | 0% | 1225 | 0 | 0 | 0 | 0 |
| 2 | rathole | PASS | 0% | 256 | 0 | 0 | 0 | 0 |
| 3 | cathole | PASS | 0.02% | 1036 | 0 | 0 | 0 | 0 |
| 4 | frp | PASS | 0.1% | 320 | 0 | 0 | 0 | 0 |
| 5 | wireguard | PASS | 0.11% | 1052 | 0 | 0 | 0 | 0 |

## wrk results

| Test | Target | Connections | Requests/s | Transfer MiB/s | p50 ms | p95 ms | p99 ms | Errors |
| --- | --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| latency | cathole | 64 | 106259 | 14.49 | 0.6 | 0.87 | 1.03 | 64 |
| latency | direct | 64 | 333766 | 45.51 | 0.16 | 0.32 | 0.52 | 0 |
| latency | frp | 64 | 10015 | 1.36 | 6.59 | 7.67 | 8.27 | 64 |
| latency | rathole | 64 | 99208 | 13.52 | 0.61 | 0.94 | 1.36 | 0 |
| latency | wireguard | 64 | 40026 | 5.45 | 1.58 | 1.81 | 2.01 | 0 |
| saturation | cathole | 64 | 58004 | 3633.24 | 1.04 | 1.65 | 2.04 | 0 |
| saturation | cathole | 256 | 50659 | 3173.14 | 4.82 | 7.83 | 9.39 | 0 |
| saturation | cathole | 1000 | 50920 | 3189.56 | 18.87 | 32.79 | 40.36 | 972 |
| saturation | direct | 64 | 224000 | 14030.78 | 0.23 | 0.58 | 0.89 | 0 |
| saturation | direct | 256 | 308870 | 19346.8 | 0.66 | 1.8 | 3.14 | 251 |
| saturation | direct | 1000 | 312435 | 19570.14 | 2.21 | 6.5 | 9.78 | 0 |
| saturation | frp | 64 | 3749 | 235 | 16.93 | 18.66 | 19.91 | 0 |
| saturation | frp | 256 | 4007 | 251.89 | 63.81 | 69.2 | 240.5 | 256 |
| saturation | frp | 1000 | 3524 | 223.89 | 247.3 | 781.06 | 1417.98 | 0 |
| saturation | rathole | 64 | 2098 | 131.82 | 43.8 | 48.06 | 48.35 | 0 |
| saturation | rathole | 256 | 10078 | 632.88 | 42.41 | 48.04 | 49.34 | 256 |
| saturation | rathole | 1000 | 41013 | 2572.43 | 16.73 | 60.12 | 231.67 | 0 |
| saturation | wireguard | 64 | 874 | 54.8 | 62.76 | 126.87 | 391.92 | 60 |
| saturation | wireguard | 256 | 828 | 52.61 | 286.8 | 701.71 | 1087.39 | 0 |
| saturation | wireguard | 1000 | 782 | 52.08 | 745.85 | 3223.27 | 6044.1 | 0 |
| stress | cathole | 1000 | 176531 | 24.07 | 5.23 | 9.19 | 11.37 | 0 |
| stress | direct | 1000 | 866651 | 118.18 | 0.9 | 3.2 | 4.97 | 974 |
| stress | frp | 1000 | 8932 | 1.21 | 40.74 | 2480.09 | 6399.32 | 0 |
| stress | rathole | 1000 | 193043 | 26.32 | 4.82 | 8.47 | 85.25 | 0 |
| stress | wireguard | 1000 | 48102 | 6.56 | 21.19 | 23.89 | 25.33 | 992 |

## Emby-style 10 GiB stream

One HTTP/1.1 GET of a generated 10 GiB `video/mp4` body on a single connection, discarded to `/dev/null`. This matches progressive playback more closely than wrk. 4K HEVC is about 25 Mbps (3.1 MiB/s); anything above that has headroom.

| Rank | Target | MiB/s | Seconds | TTFB s | Complete |
| ---: | --- | ---: | ---: | ---: | --- |
| 1 | direct | 4923.07 | 2.08 | 0 | PASS |
| 2 | cathole | 671.47 | 15.25 | 0 | PASS |
| 3 | rathole | 255.1 | 40.14 | 0 | PASS |
| 4 | frp | 185.07 | 55.33 | 0 | PASS |
| 5 | wireguard | 53.86 | 190.1 | 0 | PASS |

| Target | Seek (16 x 8 MiB Range) | Seek MiB/s |
| --- | --- | ---: |
| direct | PASS | 711.11 |
| cathole | PASS | 533.33 |
| rathole | PASS | 148.83 |
| frp | PASS | 150.58 |
| wireguard | PASS | 51.2 |

## TCP echo packets

Raw TCP echo (not HTTP): 64-byte RTT samples plus parallel bulk upload/download of mirrored payload.

| Rank | Target | MiB/s | p50 ms | p99 ms | Complete |
| ---: | --- | ---: | ---: | ---: | --- |
| 1 | cathole | 313.59 | 0.28 | 0.43 | PASS |
| 2 | direct | 313.07 | 0.09 | 0.23 | PASS |
| 3 | rathole | 264.44 | 0.3 | 0.39 | PASS |
| 4 | frp | 138.37 | 0.5 | 0.72 | PASS |
| 5 | wireguard | 45.22 | 0.38 | 0.62 | PASS |

## UDP echo packets

Fixed-rate 200-byte UDP echo probes. Loss does not throttle probe generation.

| Rank | Target | Received | Loss | p50 ms | p99 ms | Complete |
| ---: | --- | ---: | ---: | ---: | ---: | --- |
| 1 | cathole | 200 | 0% | 0.34 | 5.96 | PASS |
| 2 | direct | 200 | 0% | 0.12 | 0.25 | PASS |
| 3 | frp | 200 | 0% | 0.63 | 1.15 | PASS |
| 4 | rathole | 200 | 0% | 0.37 | 5.96 | PASS |
| 5 | wireguard | 200 | 0% | 0.41 | 0.69 | PASS |

## HTTPS requests

TLS terminated on the backend; proxies forward raw TCP. Self-signed cert (verification disabled in the probe).

| Rank | Target | RPS | p50 ms | p99 ms | Errors | Complete |
| ---: | --- | ---: | ---: | ---: | ---: | --- |
| 1 | wireguard | 97 | 10.28 | 10.89 | 0 | PASS |
| 2 | rathole | 97 | 10.36 | 10.85 | 0 | PASS |
| 3 | frp | 97 | 10.24 | 11.55 | 0 | PASS |
| 4 | cathole | 97 | 10.33 | 10.86 | 0 | PASS |
| 5 | direct | 96 | 10.38 | 10.97 | 0 | PASS |

Correctness performs 30 requests per target and verifies the hello response plus exact 64 KiB and 1 MiB bodies. Stream tests also check `/video` Content-Length and one Range response. Optional TCP/UDP/HTTPS probes exercise echo and TLS paths.

| Target | Correctness |
| --- | --- |
| direct | PASS |
| rathole | PASS |
| wireguard | PASS |
| frp | PASS |
| cathole | PASS |
<!-- docker-benchmark-results:end -->

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
