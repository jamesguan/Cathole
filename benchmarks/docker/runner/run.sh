#!/usr/bin/env bash
set -euo pipefail

THREADS=${THREADS:-16}
DURATION=${DURATION:-10}
STRESS_DURATION=${STRESS_DURATION:-10}
SAMPLES=${SAMPLES:-3}
SATURATION_STEPS=${SATURATION_STEPS:-"64 256 1000"}
RESULTS_DIR=${RESULTS_DIR:-/results}
RAW="$RESULTS_DIR/raw.jsonl"
mkdir -p "$RESULTS_DIR"
: > "$RAW"

names=(direct rathole wireguard frp cathole)
urls=(
  http://backend:3000
  http://rathole-server:8080
  http://wireguard-server:8080
  http://frp-server:8080
  http://cathole-server:8080
)

wait_for_target() {
  local name=$1 url=$2
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

run_wrk() {
  local test=$1 name=$2 url=$3 path=$4 connections=$5 duration=$6 sample=$7
  local output json
  echo "[$test sample $sample/$SAMPLES] $name: wrk -t$THREADS -c$connections -d${duration}s $url$path"
  output=$(wrk --latency -t"$THREADS" -c"$connections" -d"${duration}s" \
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

for i in "${!names[@]}"; do
  wait_for_target "${names[$i]}" "${urls[$i]}"
done

# Test 1: response correctness and exact body sizes through every route.
for i in "${!names[@]}"; do
  name=${names[$i]}; url=${urls[$i]}; passed=true
  for _ in $(seq 1 10); do
    [[ $(curl -fsS "$url/") == "Hello, Cathole benchmark!" ]] || passed=false
    [[ $(curl -fsS "$url/64k" | wc -c | tr -d ' ') == 65536 ]] || passed=false
    [[ $(curl -fsS "$url/1m" | wc -c | tr -d ' ') == 1048576 ]] || passed=false
  done
  jq -cn --arg target "$name" --argjson passed "$passed" \
    '{test:"correctness",target:$target,passed:$passed,requests:30}' >> "$RAW"
  [[ $passed == true ]]
done

# Warm every path before collecting timed results.
for i in "${!names[@]}"; do
  wrk -t2 -c32 -d2s "${urls[$i]}/" >/dev/null
done

# Test 2: latency/request-rate at moderate concurrency. Rotate target order per sample.
for sample in $(seq 1 "$SAMPLES"); do
  for offset in "${!names[@]}"; do
    i=$(( (offset + sample - 1) % ${#names[@]} ))
    run_wrk latency "${names[$i]}" "${urls[$i]}" / 64 "$DURATION" "$sample"
  done
done

# Test 3: sweep concurrency using 64 KiB bodies to find network saturation.
for connections in $SATURATION_STEPS; do
  for sample in $(seq 1 "$SAMPLES"); do
    for offset in "${!names[@]}"; do
      i=$(( (offset + sample - 1) % ${#names[@]} ))
      run_wrk saturation "${names[$i]}" "${urls[$i]}" /64k "$connections" "$DURATION" "$sample"
    done
  done
done

# Test 4: the requested 1,000-connection, 16-thread stress shape.
for sample in $(seq 1 "$SAMPLES"); do
  for offset in "${!names[@]}"; do
    i=$(( (offset + sample - 1) % ${#names[@]} ))
    run_wrk stress "${names[$i]}" "${urls[$i]}" / 1000 "$STRESS_DURATION" "$sample"
  done
done

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
  '{metadata:{timestamp_utc:$timestamp,threads:$threads,duration_seconds:$duration,
    samples:$samples,saturation_connections:$steps,topology:"single Docker host, three isolated bridge networks",
    backend:"Axum/Tokio multi-thread runtime",cpu_model:$cpu_model,cpu_count:$cpu_count,
    memory_kib:$memory_kib,kernel:$kernel,docker_version:$docker,
    versions:{rathole:"dev-b55f7e5",frp:"v0.71.0",wireguard:"Alpine 3.22 wireguard-tools",cathole:$cathole_revision}},results:.}' \
  "$RAW" > "$RESULTS_DIR/latest.json"

{
  echo '# Docker comparison benchmark results'
  echo
  echo "Run at: $timestamp"
  echo
  echo '| Test | Target | Connections | Requests/s | Transfer MiB/s | p50 ms | p99 ms | Errors |'
  echo '| --- | --- | ---: | ---: | ---: | ---: | ---: | ---: |'
  jq -r '
    def median: sort | .[length / 2 | floor];
    [.results[] | select(.test != "correctness")]
    | group_by([.test,.target,.connections])[]
    | {test:.[0].test,target:.[0].target,connections:.[0].connections,
       rps:(map(.requests_per_second)|median),bps:(map(.bytes_per_second)|median),
       p50:(map(.latency_us_p50)|median),p99:(map(.latency_us_p99)|median),
       errors:(map(.errors)|add)}
    | "| \(.test) | \(.target) | \(.connections) | \(.rps|floor) | \((.bps/1048576)*100|floor/100) | \((.p50/1000)*100|floor/100) | \((.p99/1000)*100|floor/100) | \(.errors) |"' \
    "$RESULTS_DIR/latest.json"
  echo
  echo 'Correctness performs 30 requests per target and verifies the hello response plus exact 64 KiB and 1 MiB bodies.'
  echo
  echo '| Target | Correctness |'
  echo '| --- | --- |'
  jq -r '.results[] | select(.test == "correctness") | "| \(.target) | \(if .passed then "PASS" else "FAIL" end) |"' \
    "$RESULTS_DIR/latest.json"
} > "$RESULTS_DIR/latest.md"

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

cat "$RESULTS_DIR/latest.md"
