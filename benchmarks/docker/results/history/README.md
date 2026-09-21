# Benchmark run history

Archived isolated Docker comparison results. Each file pair is a full
`latest.md` / `latest.json` snapshot. The live tables in the repo root and
README are always the newest run.

| Timestamp (UTC) | Notes |
| --- | --- |
| 2026-09-21T05:43:52Z | Cathole **single mux** TCP+Noise. Emby 853 MiB/s; sat peak 577 MiB/s; latency ~18k RPS. Trailed Rathole on multi-conn wrk. |
| 2026-09-21T06:15:37Z | Cathole **data-channel pool** (per-visitor Noise TCP, UDP on control). Sat peak **3576 MiB/s** (beats Rathole 2583); latency **105k RPS**; Emby **737 MiB/s**; seeks 492 MiB/s. |
