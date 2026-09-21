#!/usr/bin/env bash
set -euo pipefail

THREADS=${THREADS:-16}
DURATION=${DURATION:-10}
STRESS_DURATION=${STRESS_DURATION:-10}
SAMPLES=${SAMPLES:-1}
SATURATION_STEPS=${SATURATION_STEPS:-"64 256 1000"}
WRK_TIMEOUT=${WRK_TIMEOUT:-30s}
TESTS=${TESTS:-"correctness,latency,saturation,stress,stream"}
VIDEO_BYTES=${VIDEO_BYTES:-10737418240}
STREAM_TIMEOUT=${STREAM_TIMEOUT:-900}
RESULTS_DIR=${RESULTS_DIR:-/results}
RAW="$RESULTS_DIR/raw.jsonl"
TARGET=${TARGET:-}
GENERATE_REPORT=${GENERATE_REPORT:-0}
mkdir -p "$RESULTS_DIR"

declare -A URLS=(
  [direct]=http://backend:3000
  [rathole]=http://rathole-server:8080
  [wireguard]=http://wireguard-server:8080
  [frp]=http://frp-server:8080
  [cathole]=http://cathole-server:8080
)

wait_for_target() {
  local name=$1 url=$2
  echo "Waiting for $name at $url/health"
  for _ in $(seq 1 180); do
    if [[ $(curl -sS --max-time 2 -o /dev/null -w '%{http_code}' "$url/health" || true) == 204 ]]; then
      echo "$name is ready"
      return 0
    fi
    sleep 1
  done
  echo "$name did not become ready: $url" >&2
  return 1
}

has_test() {
  local needle=",$1,"
  local haystack=",${TESTS// /,},"
  [[ $haystack == *"$needle"* ]]
}

now_s() {
  awk '{print $1}' /proc/uptime
}

CURL_FMT='{"http_code":%{http_code},"size_download":%{size_download},"time_total":%{time_total},"speed_download":%{speed_download},"time_connect":%{time_connect},"time_starttransfer":%{time_starttransfer}}'

run_wrk() {
  local test=$1 name=$2 url=$3 path=$4 connections=$5 duration=$6 sample=$7
  local output json
  echo
  echo "[$test sample $sample/$SAMPLES] $name: wrk --latency -t$THREADS -c$connections -d${duration}s -T$WRK_TIMEOUT $url$path"
  output=$(wrk --latency -t"$THREADS" -c"$connections" -d"${duration}s" -T"$WRK_TIMEOUT" \
    -s /bench/report.lua "$url$path")
  printf '%s\n' "$output"
  json=$(printf '%s\n' "$output" | sed -n 's/^WRK_JSON://p' | tail -n 1)
  [[ -n $json ]]
  jq -cn \
    --arg test "$test" --arg target "$name" --arg path "$path" \
    --argjson connections "$connections" --argjson threads "$THREADS" --argjson sample "$sample" \
    --argjson metrics "$json" \
    '$metrics + {test:$test,target:$target,path:$path,connections:$connections,threads:$threads,sample:$sample}' \
    >> "$RAW"
}

run_stream() {
  local sample=$1
  local json rc elapsed t0 t1
  echo
  echo "[stream sample $sample/$SAMPLES] $TARGET: curl --http1.1 10GiB $url/video"
  t0=$(now_s)
  set +e
  json=$(curl -sS --http1.1 -o /dev/null --max-time "$STREAM_TIMEOUT" -w "$CURL_FMT" "$url/video")
  rc=$?
  set -e
  t1=$(now_s)
  elapsed=$(awk -v a="$t0" -v b="$t1" 'BEGIN{printf "%.6f", b-a}')
  if [[ -z $json ]]; then
    json='{"http_code":0,"size_download":0,"time_total":0,"speed_download":0,"time_connect":0,"time_starttransfer":0}'
  fi
  printf '%s\n' "$json"
  jq -cn \
    --arg test stream --arg target "$TARGET" --arg path /video \
    --argjson sample "$sample" --argjson expected "$VIDEO_BYTES" --argjson exit_code "$rc" \
    --argjson wall "$elapsed" --argjson metrics "$json" \
    '$metrics + {
      test:$test,target:$target,path:$path,sample:$sample,connections:1,threads:1,
      expected_bytes:$expected,exit_code:$exit_code,duration_seconds:$wall,
      requests:1,bytes:($metrics.size_download),
      bytes_per_second:(if $wall > 0 then ($metrics.size_download / $wall) else 0 end),
      requests_per_second:(if $wall > 0 then (1 / $wall) else 0 end),
      errors:(if $exit_code == 0 and $metrics.http_code == 200 and $metrics.size_download == $expected then 0 else 1 end),
      ok:($exit_code == 0 and $metrics.http_code == 200 and $metrics.size_download == $expected)
    }' >> "$RAW"
}

run_seek() {
  local sample=$1
  local i start end got code size ok=true count=16 chunk=8388608
  local t0 t1 elapsed downloaded=0
  echo
  echo "[seek sample $sample/$SAMPLES] $TARGET: 16 x 8MiB HTTP Range requests across /video"
  t0=$(now_s)
  for i in $(seq 0 $((count - 1))); do
    start=$((i * VIDEO_BYTES / count))
    end=$((start + chunk - 1))
    if ((end >= VIDEO_BYTES)); then
      end=$((VIDEO_BYTES - 1))
    fi
    set +e
    got=$(curl -sS --http1.1 -o /tmp/range.body --max-time 120 \
      -w '%{http_code} %{size_download}' -H "Range: bytes=${start}-${end}" "$url/video")
    set -e
    code=${got%% *}
    size=${got##* }
    if [[ $code != 206 ]] || [[ $size != "$chunk" && $size != $((end - start + 1)) ]]; then
      echo "range $start-$end failed: $got"
      ok=false
    else
      downloaded=$((downloaded + size))
    fi
  done
  t1=$(now_s)
  elapsed=$(awk -v a="$t0" -v b="$t1" 'BEGIN{printf "%.6f", b-a}')
  jq -cn \
    --arg test seek --arg target "$TARGET" --argjson sample "$sample" \
    --argjson ok "$ok" --argjson requests "$count" --argjson bytes "$downloaded" \
    --argjson duration "$elapsed" \
    '{
      test:$test,target:$target,path:"/video",sample:$sample,connections:1,threads:1,
      passed:$ok,requests:$requests,bytes:$bytes,duration_seconds:$duration,
      bytes_per_second:(if $duration > 0 then ($bytes / $duration) else 0 end),
      errors:(if $ok then 0 else 1 end)
    }' >> "$RAW"
  [[ $ok == true ]]
}

generate_report() {
  local timestamp cpu_model cpu_count memory_kib
  timestamp=$(date -u +%Y-%m-%dT%H:%M:%SZ)
  cpu_model=$(awk -F: '/model name/{sub(/^[[:space:]]+/,"",$2); print $2; exit}' /proc/cpuinfo)
  cpu_count=$(getconf _NPROCESSORS_ONLN)
  memory_kib=$(awk '/MemTotal/{print $2}' /proc/meminfo)

  jq -s \
    --arg timestamp "$timestamp" --argjson threads "$THREADS" \
    --argjson duration "$DURATION" --argjson samples "$SAMPLES" --arg steps "$SATURATION_STEPS" \
    --arg cpu_model "$cpu_model" --argjson cpu_count "$cpu_count" --argjson memory_kib "$memory_kib" \
    --arg kernel "$(uname -sr)" --arg docker "${DOCKER_VERSION:-unknown}" \
    --arg cathole_revision "${CATHOLE_REVISION:-working-tree}" \
    --arg isolation "one target at a time; other proxy stacks stopped" \
    --arg wrk "wrk --latency -t$THREADS -d${DURATION}s -T$WRK_TIMEOUT" \
    --arg tests "$TESTS" --argjson video_bytes "$VIDEO_BYTES" \
    '{
      metadata:{
        timestamp_utc:$timestamp,threads:$threads,duration_seconds:$duration,
        samples:$samples,saturation_connections:$steps,
        isolation:$isolation,load_generator:$wrk,tests:$tests,video_bytes:$video_bytes,
        topology:"single Docker host, three isolated bridge networks, sequential exclusive stacks",
        backend:"Axum/Tokio multi-thread runtime",cpu_model:$cpu_model,cpu_count:$cpu_count,
        memory_kib:$memory_kib,kernel:$kernel,docker_version:$docker,
        versions:{rathole:"dev-b55f7e5",frp:"v0.71.0",wireguard:"Alpine 3.22 wireguard-tools",cathole:$cathole_revision}
      },
      results:.
    }' \
    "$RAW" > "$RESULTS_DIR/latest.json"

  jq '
    def median:
      sort as $s
      | if ($s | length) == 0 then null else $s[(($s | length) - 1) / 2 | floor] end;
    def mib: (. / 1048576);
    def ms: (. / 1000);
    def fmt(n): (n * 100 | floor) / 100;
    def rank_desc(key):
      sort_by(.[key]) | reverse | to_entries | map(.value + {(key + "_rank"): (.key + 1)});
    def rank_asc(key):
      sort_by(.[key]) | to_entries | map(.value + {(key + "_rank"): (.key + 1)});

    . as $root
    | (.results) as $all
    | ["direct","rathole","wireguard","frp","cathole"] as $names
    | [
        $names[] as $t
        | ($all | map(select(.target == $t))) as $rows
        | ($rows | map(select(.test == "correctness"))[0]) as $corr
        | ($rows | map(select(.test == "latency"))) as $lat
        | ($rows | map(select(.test == "saturation"))) as $sat
        | ($rows | map(select(.test == "stress"))) as $str
        | ($rows | map(select(.test == "stream"))) as $str_stream
        | ($rows | map(select(.test == "seek"))) as $str_seek
        | ($sat | group_by(.connections) | map({
            connections: .[0].connections,
            rps: (map(.requests_per_second) | median),
            bps: (map(.bytes_per_second) | median),
            p50: (map(.latency_us_p50) | median),
            p99: (map(.latency_us_p99) | median),
            errors: (map(.errors) | add)
          })) as $sat_steps
        | {
            target: $t,
            passed: ($corr.passed // false),
            latency_rps: ($lat | map(.requests_per_second) | median),
            latency_p50_us: ($lat | map(.latency_us_p50) | median),
            latency_p95_us: ($lat | map(.latency_us_p95) | median),
            latency_p99_us: ($lat | map(.latency_us_p99) | median),
            latency_mean_us: ($lat | map(.latency_us_mean) | median),
            stress_rps: ($str | map(.requests_per_second) | median),
            stress_p50_us: ($str | map(.latency_us_p50) | median),
            stress_p99_us: ($str | map(.latency_us_p99) | median),
            stress_errors: ($str | map(.errors) | add),
            sat_peak_bps: (($sat_steps | map(.bps) | max) // 0),
            sat_peak_connections: (
              ($sat_steps | max_by(.bps) | .connections) // 0
            ),
            sat_steps: $sat_steps,
            stream_bps: (($str_stream | map(.bytes_per_second) | median) // null),
            stream_seconds: (($str_stream | map(.duration_seconds) | median) // null),
            stream_ttfb: (($str_stream | map(.time_starttransfer) | median) // null),
            stream_ok: (([$str_stream[] | .ok] | length == 0) or ([$str_stream[] | .ok] | all)),
            seek_bps: (($str_seek | map(.bytes_per_second) | median) // null),
            seek_ok: (([$str_seek[] | .passed] | length == 0) or ([$str_seek[] | .passed] | all)),
            requests: ([($rows[] | select(.test != "correctness") | .requests)] | add // 0),
            errors: ([($rows[] | select(.test != "correctness") | .errors)] | add // 0),
            errors_connect: ([($rows[] | select(.test != "correctness") | (.errors_connect // 0))] | add // 0),
            errors_read: ([($rows[] | select(.test != "correctness") | (.errors_read // 0))] | add // 0),
            errors_write: ([($rows[] | select(.test != "correctness") | (.errors_write // 0))] | add // 0),
            errors_status: ([($rows[] | select(.test != "correctness") | (.errors_status // 0))] | add // 0),
            errors_timeout: ([($rows[] | select(.test != "correctness") | (.errors_timeout // 0))] | add // 0)
          }
        | .error_rate = (
            if (.requests + .errors) == 0 then 0 else (.errors / (.requests + .errors)) end
          )
      ] as $summary
    | ($summary | rank_desc("sat_peak_bps")) as $by_sat
    | ($summary | rank_desc("latency_rps")) as $by_rps
    | ($summary | rank_asc("latency_p50_us")) as $by_p50
    | ($summary | rank_asc("error_rate")) as $by_err
    | [
        $summary[] as $row
        | ($by_sat[] | select(.target == $row.target) | .sat_peak_bps_rank) as $r_sat
        | ($by_rps[] | select(.target == $row.target) | .latency_rps_rank) as $r_rps
        | ($by_p50[] | select(.target == $row.target) | .latency_p50_us_rank) as $r_p50
        | ($by_err[] | select(.target == $row.target) | .error_rate_rank) as $r_err
        | $row + {
            rank_throughput: $r_sat,
            rank_request_rate: $r_rps,
            rank_latency: $r_p50,
            rank_reliability: (if $row.passed then $r_err else 99 end),
            overall_score: (
              (if $row.passed then 1 else 0 end) * (
                (6 - $r_sat) * 0.35 +
                (6 - $r_rps) * 0.20 +
                (6 - $r_p50) * 0.20 +
                (6 - $r_err) * 0.25
              )
            )
          }
      ]
    | sort_by(-.overall_score) as $ranked
    | $ranked
    | to_entries
    | map(.value + {rank_overall: (.key + 1)}) as $final
    | $root + {summary: $final}
  ' "$RESULTS_DIR/latest.json" > /tmp/latest-ranked.json
  cat /tmp/latest-ranked.json > "$RESULTS_DIR/latest.json"

  {
    echo '# Docker comparison benchmark results'
    echo
    echo "Run at: $timestamp"
    echo
    echo 'Stacks were started and measured **one target at a time**. Other reverse-proxy and VPN containers were stopped during each run so CPU, sockets, and the Docker bridge were not shared.'
    echo
    echo '## Overall ranking'
    echo
    echo '| Rank | Target | Score | Throughput | Request rate | Latency | Reliability | Correctness |'
    echo '| ---: | --- | ---: | ---: | ---: | ---: | ---: | --- |'
    jq -r '
      def fmt(n): (n * 100 | floor) / 100;
      .summary[]
      | "| \(.rank_overall) | \(.target) | \(fmt(.overall_score)) | #\(.rank_throughput) | #\(.rank_request_rate) | #\(.rank_latency) | #\(.rank_reliability) | \(if .passed then "PASS" else "FAIL" end) |"
    ' "$RESULTS_DIR/latest.json"
    echo
    echo 'Score weights: saturation throughput 35%, small-response request rate 20%, p50 latency 20%, error rate 25%. A correctness failure ranks last on reliability.'
    echo
    echo '## Speed: saturated 64 KiB transfer'
    echo
    echo '| Rank | Target | Peak MiB/s | At connections | p50 ms | p99 ms |'
    echo '| ---: | --- | ---: | ---: | ---: | ---: |'
    jq -r '
      def fmt(n): (n * 100 | floor) / 100;
      .summary | sort_by(.rank_throughput)[]
      | "| \(.rank_throughput) | \(.target) | \(fmt(.sat_peak_bps / 1048576)) | \(.sat_peak_connections) | \(fmt(.sat_steps | max_by(.bps) | .p50 / 1000)) | \(fmt(.sat_steps | max_by(.bps) | .p99 / 1000)) |"
    ' "$RESULTS_DIR/latest.json"
    echo
    echo '## Speed: small-response request rate'
    echo
    echo '| Rank | Target | Latency RPS | p50 ms | p99 ms | Stress RPS |'
    echo '| ---: | --- | ---: | ---: | ---: | ---: |'
    jq -r '
      def fmt(n): (n * 100 | floor) / 100;
      .summary | sort_by(.rank_request_rate)[]
      | "| \(.rank_request_rate) | \(.target) | \(.latency_rps | floor) | \(fmt(.latency_p50_us / 1000)) | \(fmt(.latency_p99_us / 1000)) | \(.stress_rps | floor) |"
    ' "$RESULTS_DIR/latest.json"
    echo
    echo '## Reliability'
    echo
    echo '| Rank | Target | Correctness | Error rate | Timeouts | Connect | Read | Write | Status |'
    echo '| ---: | --- | --- | ---: | ---: | ---: | ---: | ---: | ---: |'
    jq -r '
      def pct(n): ((n * 10000 | floor) / 100 | tostring) + "%";
      .summary | sort_by(.rank_reliability)[]
      | "| \(.rank_reliability) | \(.target) | \(if .passed then "PASS" else "FAIL" end) | \(pct(.error_rate)) | \(.errors_timeout) | \(.errors_connect) | \(.errors_read) | \(.errors_write) | \(.errors_status) |"
    ' "$RESULTS_DIR/latest.json"
    echo
    echo '## wrk results'
    echo
    echo '| Test | Target | Connections | Requests/s | Transfer MiB/s | p50 ms | p95 ms | p99 ms | Errors |'
    echo '| --- | --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |'
    jq -r '
      def median: sort | .[(length - 1) / 2 | floor];
      def fmt(n): (n * 100 | floor) / 100;
      [.results[] | select(.test != "correctness" and .test != "stream" and .test != "seek" and (.latency_us_p50 != null))]
      | group_by([.test,.target,.connections])[]
      | {test:.[0].test,target:.[0].target,connections:.[0].connections,
         rps:(map(.requests_per_second)|median),bps:(map(.bytes_per_second)|median),
         p50:(map(.latency_us_p50)|median),p95:(map(.latency_us_p95)|median),
         p99:(map(.latency_us_p99)|median),errors:(map(.errors)|add)}
      | "| \(.test) | \(.target) | \(.connections) | \(.rps|floor) | \(fmt(.bps/1048576)) | \(fmt(.p50/1000)) | \(fmt(.p95/1000)) | \(fmt(.p99/1000)) | \(.errors) |"
    ' "$RESULTS_DIR/latest.json"
    echo
    echo '## Emby-style 10 GiB stream'
    echo
    echo 'One HTTP/1.1 GET of a generated 10 GiB `video/mp4` body on a single connection, discarded to `/dev/null`. This matches progressive playback more closely than wrk. 4K HEVC is about 25 Mbps (3.1 MiB/s); anything above that has headroom.'
    echo
    echo '| Rank | Target | MiB/s | Seconds | TTFB s | Complete |'
    echo '| ---: | --- | ---: | ---: | ---: | --- |'
    jq -r '
      def fmt(n): if n == null then "n/a" else ((n * 100 | floor) / 100 | tostring) end;
      [.summary[] | select(.stream_bps != null)]
      | sort_by(-.stream_bps)
      | to_entries[]
      | "| \(.key + 1) | \(.value.target) | \(fmt(.value.stream_bps / 1048576)) | \(fmt(.value.stream_seconds)) | \(fmt(.value.stream_ttfb)) | \(if .value.stream_ok then "PASS" else "FAIL" end) |"
    ' "$RESULTS_DIR/latest.json"
    echo
    echo '| Target | Seek (16 x 8 MiB Range) | Seek MiB/s |'
    echo '| --- | --- | ---: |'
    jq -r '
      def fmt(n): if n == null then "n/a" else ((n * 100 | floor) / 100 | tostring) end;
      .summary[] | select(.seek_bps != null)
      | "| \(.target) | \(if .seek_ok then "PASS" else "FAIL" end) | \(fmt(.seek_bps / 1048576)) |"
    ' "$RESULTS_DIR/latest.json"
    echo
    echo 'Correctness performs 30 requests per target and verifies the hello response plus exact 64 KiB and 1 MiB bodies. Stream tests also check `/video` Content-Length and one Range response.'
    echo
    echo '| Target | Correctness |'
    echo '| --- | --- |'
    jq -r '.results[] | select(.test == "correctness") | "| \(.target) | \(if .passed then "PASS" else "FAIL" end) |"' \
      "$RESULTS_DIR/latest.json"
  } > "$RESULTS_DIR/latest.md"

  cp "$RESULTS_DIR/latest.json" "$RESULTS_DIR/BENCHMARK-RESULTS.json"
  cp "$RESULTS_DIR/latest.md" "$RESULTS_DIR/BENCHMARK-RESULTS.md"

  if [[ -d /workspace ]]; then
    cp "$RESULTS_DIR/latest.json" /workspace/BENCHMARK-RESULTS.json
    cp "$RESULTS_DIR/latest.md" /workspace/BENCHMARK-RESULTS.md
    if [[ -w /workspace/README.md ]]; then
      awk -v replacement="$RESULTS_DIR/latest.md" '
        /<!-- docker-benchmark-results:start -->/ {
          print
          while ((getline line < replacement) > 0) print line
          close(replacement)
          replacing = 1
          next
        }
        /<!-- docker-benchmark-results:end -->/ {
          replacing = 0
          print
          next
        }
        !replacing { print }
      ' /workspace/README.md > /tmp/README.md
      cat /tmp/README.md > /workspace/README.md
    fi
  fi

  cat "$RESULTS_DIR/latest.md"
}

if [[ $GENERATE_REPORT == 1 ]]; then
  generate_report
  exit 0
fi

if [[ -z $TARGET ]]; then
  echo 'TARGET is required (direct|rathole|wireguard|frp|cathole)' >&2
  exit 1
fi
if [[ -z ${URLS[$TARGET]+x} ]]; then
  echo "Unknown TARGET: $TARGET" >&2
  exit 1
fi

url=${URLS[$TARGET]}
echo "========== Isolated target: $TARGET =========="
echo "Other proxy stacks are stopped. Only this path plus the backend should be on CPU."
wait_for_target "$TARGET" "$url"

passed=true
if has_test correctness; then
  for _ in $(seq 1 10); do
    [[ $(curl -fsS "$url/") == "Hello, Cathole benchmark!" ]] || passed=false
    [[ $(curl -fsS "$url/64k" | wc -c | tr -d ' ') == 65536 ]] || passed=false
    [[ $(curl -fsS "$url/1m" | wc -c | tr -d ' ') == 1048576 ]] || passed=false
  done
  if has_test stream; then
    length=$(curl -sSI "$url/video" | awk 'tolower($1)=="content-length:"{gsub("\r","",$2); print $2; exit}')
    [[ $length == "$VIDEO_BYTES" ]] || passed=false
    range_size=$(curl -sS --http1.1 --max-time 30 -o /dev/null -w '%{size_download}' -H 'Range: bytes=0-1048575' "$url/video")
    [[ $range_size == 1048576 ]] || passed=false
  fi
  jq -cn --arg target "$TARGET" --argjson passed "$passed" \
    '{test:"correctness",target:$target,passed:$passed,requests:30}' >> "$RAW"
  [[ $passed == true ]]
fi

if has_test latency || has_test saturation || has_test stress; then
  echo "Warmup $TARGET (wrk)"
  wrk -t2 -c32 -d2s -T"$WRK_TIMEOUT" "$url/" >/dev/null
  sleep 1
fi

if has_test latency; then
  for sample in $(seq 1 "$SAMPLES"); do
    run_wrk latency "$TARGET" "$url" / 64 "$DURATION" "$sample"
    sleep 1
  done
fi

if has_test saturation; then
  for connections in $SATURATION_STEPS; do
    for sample in $(seq 1 "$SAMPLES"); do
      run_wrk saturation "$TARGET" "$url" /64k "$connections" "$DURATION" "$sample"
      sleep 1
    done
  done
fi

if has_test stress; then
  for sample in $(seq 1 "$SAMPLES"); do
    run_wrk stress "$TARGET" "$url" / 1000 "$STRESS_DURATION" "$sample"
    sleep 1
  done
fi

if has_test stream; then
  echo "Warmup $TARGET (8 MiB video range)"
  curl -sS --http1.1 -o /dev/null -H 'Range: bytes=0-8388607' "$url/video" >/dev/null
  sleep 1
  for sample in $(seq 1 "$SAMPLES"); do
    run_stream "$sample"
    sleep 1
  done
fi

if has_test seek; then
  for sample in $(seq 1 "$SAMPLES"); do
    run_seek "$sample"
    sleep 1
  done
fi

echo "Finished isolated target: $TARGET"
