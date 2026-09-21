#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")"

command -v docker >/dev/null || { echo 'docker is required' >&2; exit 1; }
docker compose version >/dev/null
export CATHOLE_REVISION=${CATHOLE_REVISION:-$(git -C ../.. describe --always --dirty 2>/dev/null || echo working-tree)}
export DOCKER_VERSION=${DOCKER_VERSION:-$(docker version --format '{{.Server.Version}}' 2>/dev/null || echo unknown)}
export THREADS=${THREADS:-16}
export DURATION=${DURATION:-10}
export STRESS_DURATION=${STRESS_DURATION:-10}
export SAMPLES=${SAMPLES:-1}
export SATURATION_STEPS=${SATURATION_STEPS:-"64 256 1000"}
export TESTS=${TESTS:-"correctness,latency,saturation,stress,stream"}
export VIDEO_BYTES=${VIDEO_BYTES:-10737418240}
export STREAM_TIMEOUT=${STREAM_TIMEOUT:-900}

proxy_services=(
  rathole-server rathole-client
  frp-server frp-client
  cathole-server cathole-client
  wireguard-server wireguard-client
)

stop_proxies() {
  docker compose stop "${proxy_services[@]}" >/dev/null 2>&1 || true
}

cleanup() {
  if [[ ${KEEP_STACK:-0} != 1 ]]; then
    docker compose --profile benchmark down --volumes --remove-orphans
  fi
}
trap cleanup EXIT

docker compose --profile benchmark build
docker compose --profile benchmark down --volumes --remove-orphans
mkdir -p results
python - <<'PY' || : > results/raw.jsonl
import json, pathlib
src = pathlib.Path("results/latest.json")
out = pathlib.Path("results/raw.jsonl")
keep = {"latency", "saturation", "stress"}
lines = []
if src.exists():
    data = json.loads(src.read_text(encoding="utf-8"))
    for row in data.get("results", []):
        if row.get("test") in keep:
            lines.append(json.dumps(row, separators=(",", ":")))
out.write_text("\n".join(lines) + ("\n" if lines else ""), encoding="utf-8")
print("preserved", len(lines), "prior wrk samples")
PY

docker compose up -d backend

run_target() {
  local name=$1
  shift
  echo "========== Isolated target: $name =========="
  stop_proxies
  if (($# > 0)); then
    docker compose up -d --force-recreate "$@"
  fi
  docker compose --profile benchmark run --rm --no-deps \
    -e "TARGET=$name" -e GENERATE_REPORT=0 -e "TESTS=$TESTS" \
    -e "VIDEO_BYTES=$VIDEO_BYTES" --name "bench-$name" benchmark
  if (($# > 0)); then
    docker compose stop "$@"
  fi
  sleep 2
}

run_target direct
run_target rathole rathole-server rathole-client
run_target wireguard wireguard-server wireguard-client
run_target frp frp-server frp-client
run_target cathole cathole-server cathole-client

docker compose --profile benchmark run --rm --no-deps \
  -e TARGET= -e GENERATE_REPORT=1 -e "TESTS=$TESTS" \
  -e "VIDEO_BYTES=$VIDEO_BYTES" --name bench-report benchmark

cp -f results/latest.json ../../BENCHMARK-RESULTS.json
cp -f results/latest.md ../../BENCHMARK-RESULTS.md
