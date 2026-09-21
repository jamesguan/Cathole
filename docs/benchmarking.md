# Reproduce performance before making a speed claim

The performance target is sustained throughput on a 1 Gbps link, measured with
both single and concurrent connections. Establish a direct baseline on the same
path. QUIC cannot remove physical RTT, bandwidth limits, endpoint CPU limits,
bufferbloat or an application's own serialization.

## Repeatable endpoint harness

For the 1 Gbps target, test one stream, then 4 and 16 concurrent streams. At
80 ms RTT the bandwidth-delay product is about 10 MB. Defaults allow 16 MiB
per stream and 32 MiB per connection; adjust both endpoints for higher RTT or
larger aggregate throughput. In `transport.quic`, configurable byte counts are
`stream_receive_window`, `receive_window`, `send_window`, and `socket_buffer`
(the latter defaults to 4 MiB). The OS may cap socket buffers; Cathole logs
the actual values when they are below the request. Window limits allow buffering
and are not preallocated per stream. Larger limits increase worst-case memory.

On the destination machine:

```sh
python benchmarks/compare.py serve --bind 127.0.0.1 --port 5201
```

Forward TCP and UDP 5202 through Cathole to 5201. For an optional existing-proxy
baseline, forward port 5203 to the same backend. Add a direct reachable endpoint
through your existing authorized network path if available. Do not expose the
echo service publicly unless that is intentional.

On the tester:

```sh
python benchmarks/compare.py run \
  --target cathole=PUBLIC_SERVER:5202 \
  --target baseline=PUBLIC_SERVER:5203 \
  --samples 1000 --mib 256
```

Repeat with `--parallel 4` and `--parallel 16`; `--mib` is per stream and reported
throughput is aggregate. `python benchmarks/local.py` starts a disposable local
backend and Cathole pair and runs the 1/4/16-stream smoke matrix automatically.

Each line is JSON: bidirectional TCP echo throughput (counting bytes in one
direction), persistent-connection 64-byte TCP RTT percentiles, and 200-byte UDP
probe loss/RTT. UDP probes send at about 100 packets/second regardless of loss.
Python echo overhead is part of these results. Use iperf3 or application-native
tests for final throughput claims, with the exact same endpoints and settings.

Repeat at least five times, alternating test order. Record software commits,
CPU model/load, RSS, NIC speed, MTU, RTT, packet sizes, connection count, direction,
OS, UDP/TCP socket buffers, and whether TLS/Noise layers are enabled. Run release
builds. Capture process CPU/RSS using the target platform's monitoring tools.
The harness does not pretend to measure proxy process CPU from a remote host.

## Impairment matrix

Run `tests/e2e.py --loss P` for P = 0, 0.005, 0.01, 0.02, 0.05 as a portable
correctness check. It drops actual encrypted tunnel UDP packets and delays a
subset to induce reordering. This is not a bandwidth- or RTT-controlled WAN
emulator and must not be used to infer WAN capacity.

For performance, use two Linux hosts or isolated network namespaces and apply
netem only to the test link: vary RTT (20/80/160 ms), loss (same matrix), rate,
burst loss, MTU, and complete outages independently. Apply equivalent impairment
to both directions of all tested paths. No host-wide qdisc or firewall commands are
automatically run by this repository.

Include interactive small TCP messages, large bulk transfers, concurrent bulk
plus interactive streams, and fixed-rate UDP game-like messages. Compare tail
latency and packet loss as well as averages. Lost UDP messages are expected;
lost TCP bytes, cross-client routing, unbounded memory, or stalled unrelated
streams are failures. Shared QUIC congestion control means unrelated streams
are not guaranteed unchanged latency under a saturated connection.
