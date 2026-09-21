# Docker comparison benchmark results

Run at: 2026-09-21T04:06:24Z

Stacks were started and measured **one target at a time**. Other reverse-proxy and VPN containers were stopped during each run so CPU, sockets, and the Docker bridge were not shared.

## Overall ranking

| Rank | Target | Score | Throughput | Request rate | Latency | Reliability | Correctness |
| ---: | --- | ---: | ---: | ---: | ---: | ---: | --- |
| 1 | direct | 5 | #1 | #1 | #1 | #1 | PASS |
| 2 | rathole | 3.75 | #2 | #2 | #2 | #3 | PASS |
| 3 | wireguard | 2.55 | #5 | #3 | #3 | #2 | PASS |
| 4 | cathole | 2.34 | #3 | #4 | #4 | #4 | PASS |
| 5 | frp | 1.35 | #4 | #5 | #5 | #5 | PASS |

Score weights: saturation throughput 35%, small-response request rate 20%, p50 latency 20%, error rate 25%. A correctness failure ranks last on reliability.

## Speed: saturated 64 KiB transfer

| Rank | Target | Peak MiB/s | At connections | p50 ms | p99 ms |
| ---: | --- | ---: | ---: | ---: | ---: |
| 1 | direct | 21767.41 | 1000 | 2.11 | 8.39 |
| 2 | rathole | 2877.49 | 1000 | 15.79 | 238.31 |
| 3 | cathole | 268.42 | 256 | 57.79 | 109.69 |
| 4 | frp | 251.72 | 64 | 16.69 | 19.55 |
| 5 | wireguard | 55.85 | 256 | 274.68 | 884.05 |

## Speed: small-response request rate

| Rank | Target | Latency RPS | p50 ms | p99 ms | Stress RPS |
| ---: | --- | ---: | ---: | ---: | ---: |
| 1 | direct | 333729 | 0.17 | 0.46 | 907892 |
| 2 | rathole | 105698 | 0.61 | 1.09 | 200713 |
| 3 | wireguard | 39729 | 1.58 | 2.17 | 44256 |
| 4 | cathole | 17239 | 3.43 | 10.31 | 40363 |
| 5 | frp | 9479 | 6.73 | 8.48 | 9571 |

## Reliability

| Rank | Target | Correctness | Error rate | Timeouts | Connect | Read | Write | Status |
| ---: | --- | --- | ---: | ---: | ---: | ---: | ---: | ---: |
| 1 | direct | PASS | 0% | 982 | 0 | 0 | 0 | 0 |
| 2 | wireguard | PASS | 0.02% | 220 | 0 | 0 | 0 | 0 |
| 3 | rathole | PASS | 0.02% | 1044 | 0 | 0 | 0 | 0 |
| 4 | cathole | PASS | 0.1% | 698 | 0 | 0 | 0 | 0 |
| 5 | frp | PASS | 0.14% | 425 | 0 | 0 | 0 | 0 |

## wrk results

| Test | Target | Connections | Requests/s | Transfer MiB/s | p50 ms | p95 ms | p99 ms | Errors |
| --- | --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| latency | cathole | 64 | 17239 | 2.35 | 3.43 | 7.49 | 10.31 | 0 |
| latency | direct | 64 | 333729 | 45.51 | 0.17 | 0.31 | 0.46 | 0 |
| latency | frp | 64 | 9479 | 1.29 | 6.73 | 7.86 | 8.48 | 0 |
| latency | rathole | 64 | 105698 | 14.41 | 0.61 | 0.9 | 1.09 | 61 |
| latency | wireguard | 64 | 39729 | 5.41 | 1.58 | 1.86 | 2.17 | 0 |
| saturation | cathole | 64 | 3831 | 240.05 | 17.38 | 27.19 | 32.02 | 64 |
| saturation | cathole | 256 | 4277 | 268.42 | 57.79 | 90.32 | 109.69 | 0 |
| saturation | cathole | 1000 | 3919 | 249.29 | 249.98 | 332.84 | 381.75 | 0 |
| saturation | direct | 64 | 245974 | 15407.15 | 0.23 | 0.55 | 0.83 | 62 |
| saturation | direct | 256 | 313260 | 19621.78 | 0.64 | 1.53 | 2.22 | 0 |
| saturation | direct | 1000 | 347515 | 21767.41 | 2.11 | 5.94 | 8.39 | 920 |
| saturation | frp | 64 | 4014 | 251.72 | 16.69 | 18.4 | 19.55 | 64 |
| saturation | frp | 256 | 3947 | 248.07 | 62.55 | 69.71 | 205.36 | 0 |
| saturation | frp | 1000 | 3446 | 219.07 | 251.28 | 812.41 | 1462.03 | 0 |
| saturation | rathole | 64 | 2077 | 130.51 | 43.86 | 48.06 | 48.33 | 0 |
| saturation | rathole | 256 | 9779 | 614.05 | 42.42 | 48.04 | 48.72 | 0 |
| saturation | rathole | 1000 | 45871 | 2877.49 | 15.79 | 59.71 | 238.31 | 983 |
| saturation | wireguard | 64 | 832 | 52.24 | 62.4 | 137.97 | 446.46 | 0 |
| saturation | wireguard | 256 | 879 | 55.85 | 274.68 | 594.59 | 884.05 | 220 |
| saturation | wireguard | 1000 | 791 | 52.53 | 725.75 | 2899.55 | 5088.85 | 0 |
| stress | cathole | 1000 | 40363 | 5.5 | 23.43 | 42.44 | 53.06 | 634 |
| stress | direct | 1000 | 907892 | 123.81 | 0.81 | 2.78 | 4.42 | 0 |
| stress | frp | 1000 | 9571 | 1.3 | 39.89 | 1116.55 | 5647.65 | 361 |
| stress | rathole | 1000 | 200713 | 27.37 | 4.64 | 7.97 | 52.25 | 0 |
| stress | wireguard | 1000 | 44256 | 6.03 | 22.18 | 26.05 | 27.51 | 0 |

Correctness performs 30 requests per target and verifies the hello response plus exact 64 KiB and 1 MiB bodies.

| Target | Correctness |
| --- | --- |
| direct | PASS |
| rathole | PASS |
| wireguard | PASS |
| frp | PASS |
| cathole | PASS |
