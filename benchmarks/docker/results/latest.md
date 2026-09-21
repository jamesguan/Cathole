# Docker comparison benchmark results

Run at: 2026-09-21T03:36:17Z

| Test | Target | Connections | Requests/s | Transfer MiB/s | p50 ms | p99 ms | Errors |
| --- | --- | ---: | ---: | ---: | ---: | ---: | ---: |
| latency | cathole | 64 | 16560 | 2.25 | 3.57 | 10.6 | 128 |
| latency | direct | 64 | 331734 | 45.24 | 0.17 | 0.43 | 0 |
| latency | frp | 64 | 9219 | 1.25 | 6.92 | 8.89 | 128 |
| latency | rathole | 64 | 112694 | 15.36 | 0.63 | 1.1 | 252 |
| latency | wireguard | 64 | 38748 | 5.28 | 1.63 | 2.11 | 128 |
| saturation | cathole | 64 | 3448 | 216.14 | 18.9 | 33.85 | 128 |
| saturation | cathole | 256 | 4203 | 263.8 | 60.69 | 105.18 | 512 |
| saturation | cathole | 1000 | 3735 | 234.12 | 269.93 | 424.74 | 1878 |
| saturation | direct | 64 | 226389 | 14180.4 | 0.23 | 0.82 | 0 |
| saturation | direct | 256 | 305825 | 19156.09 | 0.66 | 2.32 | 0 |
| saturation | direct | 1000 | 314957 | 19728.09 | 2.27 | 8.9 | 0 |
| saturation | frp | 64 | 4173 | 261.63 | 17.44 | 20.47 | 128 |
| saturation | frp | 256 | 3785 | 237.78 | 65.88 | 228.96 | 512 |
| saturation | frp | 1000 | 3327 | 211.46 | 259.69 | 1595.1 | 1295 |
| saturation | rathole | 64 | 2433 | 152.89 | 43.81 | 48.35 | 256 |
| saturation | rathole | 256 | 11348 | 712.59 | 25.5 | 48.3 | 1024 |
| saturation | rathole | 1000 | 47027 | 2948.5 | 16.88 | 244.45 | 3961 |
| saturation | wireguard | 64 | 818 | 51.36 | 62.94 | 431.57 | 119 |
| saturation | wireguard | 256 | 805 | 51.16 | 294.94 | 947.14 | 317 |
| saturation | wireguard | 1000 | 770 | 51.19 | 647.17 | 1865.6 | 2113 |
| stress | cathole | 1000 | 36822 | 5.02 | 25.06 | 53.21 | 757 |
| stress | direct | 1000 | 859711 | 117.24 | 0.88 | 4.98 | 1849 |
| stress | frp | 1000 | 8620 | 1.17 | 39.45 | 346.86 | 403 |
| stress | rathole | 1000 | 198436 | 27.06 | 4.75 | 67.75 | 1931 |
| stress | wireguard | 1000 | 48454 | 6.6 | 23.32 | 38.35 | 3964 |

Correctness performs 30 requests per target and verifies the hello response plus exact 64 KiB and 1 MiB bodies.

| Target | Correctness |
| --- | --- |
| direct | PASS |
| rathole | PASS |
| wireguard | PASS |
| frp | PASS |
| cathole | PASS |
