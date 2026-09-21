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
