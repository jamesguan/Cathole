#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")"

command -v docker >/dev/null || { echo 'docker is required' >&2; exit 1; }
docker compose version >/dev/null
export CATHOLE_REVISION=${CATHOLE_REVISION:-$(git -C ../.. describe --always --dirty 2>/dev/null || echo working-tree)}
export DOCKER_VERSION=${DOCKER_VERSION:-$(docker version --format '{{.Server.Version}}' 2>/dev/null || echo unknown)}

cleanup() {
  if [[ ${KEEP_STACK:-0} != 1 ]]; then
    docker compose --profile benchmark down --volumes --remove-orphans
  fi
}
trap cleanup EXIT

docker compose --profile benchmark build
docker compose up -d backend rathole-server rathole-client frp-server frp-client \
  cathole-server cathole-client wireguard-server wireguard-client
docker compose --profile benchmark run --rm benchmark
