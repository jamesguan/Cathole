# Docker comparison benchmark results

Run at: 2026-09-21T05:43:52Z

Stacks were started and measured **one target at a time**. Other reverse-proxy and VPN containers were stopped during each run so CPU, sockets, and the Docker bridge were not shared.

## Overall ranking

| Rank | Target | Score | Throughput | Request rate | Latency | Reliability | Correctness |
| ---: | --- | ---: | ---: | ---: | ---: | ---: | --- |
| 1 | direct | 5 | #1 | #1 | #1 | #1 | PASS |
| 2 | rathole | 4 | #2 | #2 | #2 | #2 | PASS |
| 3 | cathole | 2.34 | #3 | #4 | #4 | #4 | PASS |
| 4 | wireguard | 2.3 | #5 | #3 | #3 | #3 | PASS |
| 5 | frp | 1.35 | #4 | #5 | #5 | #5 | PASS |

Score weights: saturation throughput 35%, small-response request rate 20%, p50 latency 20%, error rate 25%. A correctness failure ranks last on reliability.

## Speed: saturated 64 KiB transfer

| Rank | Target | Peak MiB/s | At connections | p50 ms | p99 ms |
| ---: | --- | ---: | ---: | ---: | ---: |
| 1 | direct | 20427.29 | 1000 | 2.15 | 8.82 |
| 2 | rathole | 2582.68 | 1000 | 17.07 | 211.75 |
| 3 | cathole | 577.07 | 256 | 27.27 | 35.5 |
| 4 | frp | 245.63 | 256 | 63.19 | 199.91 |
| 5 | wireguard | 53.01 | 1000 | 739.21 | 5446.71 |

## Speed: small-response request rate

| Rank | Target | Latency RPS | p50 ms | p99 ms | Stress RPS |
| ---: | --- | ---: | ---: | ---: | ---: |
| 1 | direct | 353092 | 0.16 | 0.41 | 827651 |
| 2 | rathole | 100766 | 0.61 | 1.25 | 196391 |
| 3 | wireguard | 40978 | 1.56 | 2.18 | 45942 |
| 4 | cathole | 18124 | 3.47 | 5.92 | 18286 |
| 5 | frp | 9782 | 6.62 | 8.33 | 8984 |

## Reliability

| Rank | Target | Correctness | Error rate | Timeouts | Connect | Read | Write | Status |
| ---: | --- | --- | ---: | ---: | ---: | ---: | ---: | ---: |
| 1 | direct | PASS | 0% | 674 | 0 | 0 | 0 | 0 |
| 2 | rathole | PASS | 0% | 256 | 0 | 0 | 0 | 0 |
| 3 | wireguard | PASS | 0% | 67 | 0 | 0 | 0 | 0 |
| 4 | cathole | PASS | 0.01% | 64 | 0 | 0 | 0 | 0 |
| 5 | frp | PASS | 0.02% | 64 | 0 | 0 | 0 | 0 |

## wrk results

| Test | Target | Connections | Requests/s | Transfer MiB/s | p50 ms | p95 ms | p99 ms | Errors |
| --- | --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| latency | cathole | 64 | 18124 | 2.47 | 3.47 | 5.21 | 5.92 | 0 |
| latency | direct | 64 | 353092 | 48.15 | 0.16 | 0.29 | 0.41 | 62 |
| latency | frp | 64 | 9782 | 1.33 | 6.62 | 7.72 | 8.33 | 64 |
| latency | rathole | 64 | 100766 | 13.74 | 0.61 | 0.91 | 1.25 | 0 |
| latency | wireguard | 64 | 40978 | 5.58 | 1.56 | 1.82 | 2.18 | 64 |
| saturation | cathole | 64 | 8281 | 518.72 | 7.71 | 10.01 | 11.63 | 64 |
| saturation | cathole | 256 | 9212 | 577.07 | 27.27 | 32.2 | 35.5 | 0 |
| saturation | cathole | 1000 | 9094 | 570.67 | 108.43 | 116.05 | 939.62 | 0 |
| saturation | direct | 64 | 235090 | 14725.45 | 0.23 | 0.52 | 0.8 | 0 |
| saturation | direct | 256 | 302314 | 18936.16 | 0.66 | 1.64 | 2.46 | 0 |
| saturation | direct | 1000 | 326120 | 20427.29 | 2.15 | 6.2 | 8.82 | 612 |
| saturation | frp | 64 | 3883 | 243.44 | 16.31 | 18.12 | 19.32 | 0 |
| saturation | frp | 256 | 3906 | 245.63 | 63.19 | 70.29 | 199.91 | 0 |
| saturation | frp | 1000 | 3581 | 227.64 | 245.88 | 817.64 | 1463.11 | 0 |
| saturation | rathole | 64 | 2095 | 131.61 | 43.82 | 48.05 | 48.3 | 0 |
| saturation | rathole | 256 | 9990 | 627.28 | 42.39 | 48.03 | 48.88 | 256 |
| saturation | rathole | 1000 | 41176 | 2582.68 | 17.07 | 60.56 | 211.75 | 0 |
| saturation | wireguard | 64 | 834 | 52.33 | 64.43 | 131.74 | 437.27 | 0 |
| saturation | wireguard | 256 | 829 | 52.71 | 271.57 | 586.93 | 863.61 | 3 |
| saturation | wireguard | 1000 | 796 | 53.01 | 739.21 | 3127.86 | 5446.71 | 0 |
| stress | cathole | 1000 | 18286 | 2.49 | 53.78 | 58.8 | 589.83 | 0 |
| stress | direct | 1000 | 827651 | 112.87 | 0.88 | 3.25 | 5.46 | 0 |
| stress | frp | 1000 | 8984 | 1.22 | 42.13 | 2095.86 | 5916.1 | 0 |
| stress | rathole | 1000 | 196391 | 26.78 | 4.74 | 8.25 | 66.28 | 0 |
| stress | wireguard | 1000 | 45942 | 6.26 | 21.4 | 24.52 | 26.5 | 0 |

## Emby-style 10 GiB stream

One HTTP/1.1 GET of a generated 10 GiB `video/mp4` body on a single connection, discarded to `/dev/null`. This matches progressive playback more closely than wrk. 4K HEVC is about 25 Mbps (3.1 MiB/s); anything above that has headroom.

| Rank | Target | MiB/s | Seconds | TTFB s | Complete |
| ---: | --- | ---: | ---: | ---: | --- |
| 1 | direct | 4452.17 | 2.29 | 0 | PASS |
| 2 | cathole | 853.33 | 12 | 0 | PASS |
| 3 | rathole | 261.49 | 39.15 | 0 | PASS |
| 4 | frp | 185.77 | 55.12 | 0 | PASS |
| 5 | wireguard | 54.01 | 189.57 | 0 | PASS |

| Target | Seek (16 x 8 MiB Range) | Seek MiB/s |
| --- | --- | ---: |
| direct | PASS | 673.68 |
| rathole | PASS | 150.58 |
| cathole | PASS | 474.07 |
| wireguard | PASS | 52.24 |
| frp | PASS | 156.09 |

Correctness performs 30 requests per target and verifies the hello response plus exact 64 KiB and 1 MiB bodies. Stream tests also check `/video` Content-Length and one Range response.

| Target | Correctness |
| --- | --- |
| direct | PASS |
| rathole | PASS |
| wireguard | PASS |
| frp | PASS |
| cathole | PASS |
