# Benchmark run history

Archived isolated Docker comparison results. Each file pair is a full
`latest.md` / `latest.json` snapshot. The live tables in the repo root and
README are always the newest run.

| Timestamp (UTC) | Notes |
| --- | --- |
| 2026-09-21T05:43:52Z | Cathole **single mux** TCP+Noise. Emby 853 MiB/s; sat peak 577 MiB/s; latency ~18k RPS. Trailed Rathole on multi-conn wrk. |
| 2026-09-21T06:15:37Z | Cathole **data-channel pool** (per-visitor Noise TCP, warm 32, try_send CREATE). Sat **3576 MiB/s**; latency **105k RPS**; Emby **737 MiB/s**; reliability #5 (1238 timeouts / 0.02%). |
| 2026-09-21T16:06:12Z | Warm pool **256** + **awaited CREATE_DATA**. Timeouts **639** (0.01%); sat **3699 MiB/s**; latency **111k RPS**; overall **#2** ahead of Rathole. |
| 2026-09-21T17:31:12Z | **Full clean suite** (all targets, HTTP+stream+seek+TCP/UDP/HTTPS). No warm-pool on HTTPS. Cathole #2: sat **3633**, latency **106k**, Emby **671**, TCP echo **314**, UDP 0% loss, HTTPS **97 RPS**. |
