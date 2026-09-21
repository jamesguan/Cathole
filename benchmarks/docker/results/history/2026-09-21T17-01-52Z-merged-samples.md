# Docker comparison benchmark results

Run at: 2026-09-21T17:01:52Z

Stacks were started and measured **one target at a time**. Other reverse-proxy and VPN containers were stopped during each run so CPU, sockets, and the Docker bridge were not shared.

## Overall ranking

| Rank | Target | Score | Throughput | Request rate | Latency | Reliability | Correctness |
| ---: | --- | ---: | ---: | ---: | ---: | ---: | --- |
| 1 | direct | 5 | #1 | #1 | #1 | #1 | PASS |
| 2 | cathole | 4 | #2 | #2 | #2 | #2 | PASS |
| 3 | rathole | 3 | #3 | #3 | #3 | #3 | PASS |
| 4 | wireguard | 1.65 | #5 | #4 | #4 | #4 | PASS |
| 5 | frp | 1.35 | #4 | #5 | #5 | #5 | PASS |

Score weights: saturation throughput 35%, small-response request rate 20%, p50 latency 20%, error rate 25%. A correctness failure ranks last on reliability.

## Speed: saturated 64 KiB transfer

| Rank | Target | Peak MiB/s | At connections | p50 ms | p99 ms |
| ---: | --- | ---: | ---: | ---: | ---: |
| 1 | direct | 20601.14 | 1000 | 2.17 | 8.82 |
| 2 | cathole | 3698.86 | 256 | 4.47 | 8.6 |
| 3 | rathole | 2582.68 | 1000 | 17.07 | 219.8 |
| 4 | frp | 245.63 | 256 | 63.19 | 204.97 |
| 5 | wireguard | 53.01 | 1000 | 739.21 | 5446.71 |

## Speed: small-response request rate

| Rank | Target | Latency RPS | p50 ms | p99 ms | Stress RPS |
| ---: | --- | ---: | ---: | ---: | ---: |
| 1 | direct | 353092 | 0.17 | 0.43 | 838106 |
| 2 | cathole | 102054 | 0.61 | 1.03 | 194096 |
| 3 | rathole | 99301 | 0.62 | 1.25 | 196391 |
| 4 | wireguard | 40978 | 1.59 | 2.14 | 45942 |
| 5 | frp | 10170 | 6.83 | 8.69 | 8795 |

## Reliability

| Rank | Target | Correctness | Error rate | Timeouts | Connect | Read | Write | Status |
| ---: | --- | --- | ---: | ---: | ---: | ---: | ---: | ---: |
| 1 | direct | PASS | 0% | 2907 | 0 | 0 | 0 | 0 |
| 2 | cathole | PASS | 0.02% | 3257 | 0 | 0 | 0 | 0 |
| 3 | rathole | PASS | 0.02% | 3173 | 0 | 0 | 0 | 0 |
| 4 | wireguard | PASS | 0.09% | 2617 | 0 | 0 | 0 | 0 |
| 5 | frp | PASS | 0.33% | 2944 | 0 | 0 | 0 | 0 |

## wrk results

| Test | Target | Connections | Requests/s | Transfer MiB/s | p50 ms | p95 ms | p99 ms | Errors |
| --- | --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| latency | cathole | 64 | 102054 | 13.91 | 0.61 | 0.88 | 1.03 | 127 |
| latency | direct | 64 | 353092 | 48.15 | 0.17 | 0.3 | 0.43 | 184 |
| latency | frp | 64 | 10170 | 1.38 | 6.83 | 7.98 | 8.69 | 320 |
| latency | rathole | 64 | 99301 | 13.54 | 0.62 | 0.91 | 1.25 | 0 |
| latency | wireguard | 64 | 40978 | 5.58 | 1.59 | 1.84 | 2.14 | 192 |
| saturation | cathole | 64 | 57886 | 3625.84 | 1.05 | 1.68 | 2.08 | 125 |
| saturation | cathole | 256 | 59052 | 3698.86 | 4.47 | 7.21 | 8.6 | 1021 |
| saturation | cathole | 1000 | 51451 | 3222.83 | 17.48 | 30.88 | 37.91 | 0 |
| saturation | direct | 64 | 235090 | 14725.45 | 0.23 | 0.53 | 0.8 | 118 |
| saturation | direct | 256 | 302314 | 18936.16 | 0.66 | 1.64 | 2.46 | 0 |
| saturation | direct | 1000 | 328895 | 20601.14 | 2.17 | 6.2 | 8.82 | 2605 |
| saturation | frp | 64 | 3772 | 236.47 | 16.88 | 18.78 | 20.18 | 0 |
| saturation | frp | 256 | 3906 | 245.63 | 63.19 | 70.78 | 204.97 | 0 |
| saturation | frp | 1000 | 3749 | 238.17 | 252.71 | 883.02 | 1463.11 | 2624 |
| saturation | rathole | 64 | 2095 | 131.65 | 43.83 | 48.06 | 48.29 | 0 |
| saturation | rathole | 256 | 10628 | 667.35 | 42.39 | 48.03 | 48.76 | 1280 |
| saturation | rathole | 1000 | 41176 | 2582.68 | 17.07 | 60.56 | 219.8 | 0 |
| saturation | wireguard | 64 | 834 | 52.33 | 63.23 | 136.31 | 427.98 | 117 |
| saturation | wireguard | 256 | 807 | 51.31 | 271.57 | 587.46 | 863.61 | 3 |
| saturation | wireguard | 1000 | 796 | 53.01 | 739.21 | 3135.51 | 5446.71 | 322 |
| stress | cathole | 1000 | 194096 | 26.47 | 5.01 | 8.7 | 10.59 | 1984 |
| stress | direct | 1000 | 838106 | 114.29 | 0.88 | 3.07 | 4.82 | 0 |
| stress | frp | 1000 | 8795 | 1.19 | 42.13 | 2060.71 | 5916.1 | 0 |
| stress | rathole | 1000 | 196391 | 26.78 | 4.76 | 8.3 | 66.28 | 1893 |
| stress | wireguard | 1000 | 45942 | 6.26 | 21.73 | 24.56 | 26.68 | 1983 |

## Emby-style 10 GiB stream

One HTTP/1.1 GET of a generated 10 GiB `video/mp4` body on a single connection, discarded to `/dev/null`. This matches progressive playback more closely than wrk. 4K HEVC is about 25 Mbps (3.1 MiB/s); anything above that has headroom.

| Rank | Target | MiB/s | Seconds | TTFB s | Complete |
| ---: | --- | ---: | ---: | ---: | --- |
| 1 | direct | 4675.79 | 2.19 | 0 | PASS |
| 2 | cathole | 703.29 | 14.56 | 0 | PASS |
| 3 | rathole | 256.12 | 39.97 | 0 | PASS |
| 4 | frp | 178.17 | 57.47 | 0 | PASS |
| 5 | wireguard | 52.88 | 193.61 | 0 | PASS |

| Target | Seek (16 x 8 MiB Range) | Seek MiB/s |
| --- | --- | ---: |
| direct | PASS | 711.11 |
| cathole | PASS | 512 |
| rathole | PASS | 147.12 |
| wireguard | PASS | 50.39 |
| frp | PASS | 154.21 |

## TCP echo packets

Raw TCP echo (not HTTP): 64-byte RTT samples plus parallel bulk upload/download of mirrored payload.

| Rank | Target | MiB/s | p50 ms | p99 ms | Complete |
| ---: | --- | ---: | ---: | ---: | --- |
| 1 | direct | 310.42 | 0.1 | 0.18 | PASS |
| 2 | cathole | 302.82 | 0.3 | 0.5 | PASS |
| 3 | rathole | 265.33 | 0.29 | 0.62 | PASS |
| 4 | frp | 135.1 | 0.51 | 0.62 | PASS |
| 5 | wireguard | 41.56 | 0.39 | 0.65 | PASS |

## UDP echo packets

Fixed-rate 200-byte UDP echo probes. Loss does not throttle probe generation.

| Rank | Target | Received | Loss | p50 ms | p99 ms | Complete |
| ---: | --- | ---: | ---: | ---: | ---: | --- |
| 1 | cathole | 200 | 0% | 0.34 | 0.57 | PASS |
| 2 | direct | 200 | 0% | 0.12 | 0.22 | PASS |
| 3 | frp | 200 | 0% | 0.64 | 1.2 | PASS |
| 4 | rathole | 200 | 0% | 0.37 | 6 | PASS |
| 5 | wireguard | 200 | 0% | 0.44 | 0.74 | PASS |

## HTTPS requests

TLS terminated on the backend; proxies forward raw TCP. Self-signed cert (verification disabled in the probe).

| Rank | Target | RPS | p50 ms | p99 ms | Errors | Complete |
| ---: | --- | ---: | ---: | ---: | ---: | --- |
| 1 | rathole | 96 | 10.47 | 10.96 | 0 | PASS |
| 2 | frp | 96 | 10.3 | 11.79 | 0 | PASS |
| 3 | direct | 95 | 10.49 | 11.45 | 0 | PASS |
| 4 | wireguard | 95 | 10.38 | 11.3 | 0 | PASS |
| 5 | cathole | 0 | n/a | n/a | 1 | FAIL |

Correctness performs 30 requests per target and verifies the hello response plus exact 64 KiB and 1 MiB bodies. Stream tests also check `/video` Content-Length and one Range response. Optional TCP/UDP/HTTPS probes exercise echo and TLS paths.

| Target | Correctness |
| --- | --- |
| direct | PASS |
| rathole | PASS |
| wireguard | PASS |
| frp | PASS |
| cathole | PASS |
