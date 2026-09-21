# Docker reverse-proxy comparison

This harness compares five paths to one multi-threaded Axum backend:

1. Direct Docker bridge networking.
2. Rathole Noise NK over its TCP transport.
3. WireGuard over UDP, with kernel forwarding and NAT adapting the public TCP
   listener to the WireGuard peer.
4. FRP v0.71.0 using TCP multiplexing, TLS, wire protocol v2, and per-proxy
   encryption without compression.
5. The Cathole workspace build using Noise NK over QUIC/UDP.

The topology uses three isolated Docker networks. The benchmark container can
reach only the public-side containers and the direct backend. Tunnel clients can
reach the private backend. Server/client tunnel traffic has its own network. No
proxy path can silently fall back to the direct public backend address.

The Rathole image is the official pinned `dev-b55f7e5` digest because the stable
v0.5.0 image is several years old. FRP uses the official v0.71.0 `frps` and
`frpc` images. Result JSON records these versions, the Cathole revision, and host
resource details; the effective configs are committed beside the Compose file.
The static keys and tokens in this directory are test-only.

## Requirements

- Docker Engine or Docker Desktop with Compose v2.
- Linux containers with `/dev/net/tun` and the `NET_ADMIN` capability available.
- Enough memory and file descriptors for 1,000 concurrent sockets.
- Internet access for the first image pull and Rust image builds.

WireGuard fails clearly during startup when TUN support is unavailable. The
scripts always tear down containers and volumes unless `KEEP_STACK=1` is set.

## Run

Windows PowerShell:

```powershell
.\benchmarks\docker\run.ps1
```

Linux/macOS/WSL:

```sh
./benchmarks/docker/run.sh
```

Optional tuning variables:

```sh
THREADS=16 DURATION=20 STRESS_DURATION=30 SAMPLES=3 \
SATURATION_STEPS="64 256 1000 2000" \
TESTS="correctness,latency,saturation,stress,stream,seek" \
VIDEO_BYTES=10737418240 ./benchmarks/docker/run.sh
```

The default one-sample run takes about six minutes after images are built, plus
about one to four minutes per target for the 10 GiB stream. Each target is
started **alone**: other reverse-proxy and VPN containers are stopped so they
cannot steal CPU, sockets, or bridge bandwidth. It writes `results/latest.json`
and `results/latest.md`, copies ranked results to `BENCHMARK-RESULTS.md` and
`BENCHMARK-RESULTS.json` at the repository root, and replaces the
generated-results block in the project README.

## The four tests

1. **Correctness:** ten requests each to `/`, `/64k`, and `/1m` through every
   path. It verifies the exact hello body and exact binary response lengths.
2. **Latency:** `wrk` with 64 connections, 16 threads, and a 10-second duration
   against the small hello response.
3. **Saturation:** the same `wrk` process sweeps 64, 256, and 1,000 connections
   against a 64 KiB response. Transfer MiB/s is the primary network-saturation
   metric. Increase `SATURATION_STEPS` until throughput stops increasing.
4. **Stress:** `wrk -c 1000 -t 16 -d 10s` against the hello endpoint.
5. **Emby-style stream:** one HTTP/1.1 GET of a generated 10 GiB `video/mp4`
   body on a single connection (`curl` to `/dev/null`). Optional `seek` runs 16
   scattered 8 MiB `Range` requests. Tune with `VIDEO_BYTES` and `TESTS`.

Every timed path uses `wrk --latency` and records the full latency distribution
plus connect/read/write/status/timeout errors. Each target is warmed for two
seconds, then measured alone. Host scripts start only that stack, run wrk, stop
it, and then move on. The Markdown tables report medians; JSON retains every raw
sample and the ranking scores.

## Interpreting results

This is a same-host container comparison. It measures implementation overhead
under repeatable conditions, not Internet performance. Docker bridge traversal,
the load generator, the backend, encryption, and both tunnel endpoints share the
same CPUs. Run multiple times on an otherwise idle host, report CPU model and
Docker resource limits, and compare medians before drawing performance claims.

Direct is the ceiling for this topology. WireGuard is a routed VPN rather than a
reverse proxy, so kernel NAT provides equivalent public/private TCP endpoints.
FRP's exact security and transport settings are stated above because
changing them materially changes its result.

Configuration formats follow the upstream [Rathole documentation](https://github.com/rathole-org/rathole/blob/main/README.md),
[FRP examples](https://github.com/fatedier/frp/blob/dev/conf/frpc_full_example.toml),
and [WireGuard tools](https://github.com/WireGuard/wireguard-tools). The backend
follows the official [Axum hello-world structure](https://github.com/tokio-rs/axum/tree/main/examples/hello-world).
