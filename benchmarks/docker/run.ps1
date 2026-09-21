$ErrorActionPreference = 'Stop'
Set-Location $PSScriptRoot

$dockerCommand = Get-Command docker -ErrorAction SilentlyContinue
if ($dockerCommand) {
    $docker = $dockerCommand.Source
}
else {
    $docker = Join-Path $env:LOCALAPPDATA 'Programs\DockerDesktop\resources\bin\docker.exe'
    if (-not (Test-Path -LiteralPath $docker)) {
        throw 'docker is required and was not found on PATH or in the Docker Desktop installation'
    }
}
& $docker compose version | Out-Null
$env:CATHOLE_REVISION = if ($env:CATHOLE_REVISION) { $env:CATHOLE_REVISION } else { (git -C ../.. describe --always --dirty 2>$null) }
if (-not $env:CATHOLE_REVISION) { $env:CATHOLE_REVISION = 'working-tree' }
$env:DOCKER_VERSION = if ($env:DOCKER_VERSION) { $env:DOCKER_VERSION } else { (& $docker version --format '{{.Server.Version}}' 2>$null) }
if (-not $env:DOCKER_VERSION) { $env:DOCKER_VERSION = 'unknown' }

try {
    & $docker compose --profile benchmark build
    if ($LASTEXITCODE -ne 0) { throw 'Docker image build failed' }
    & $docker compose up -d backend rathole-server rathole-client frp-server frp-client cathole-server cathole-client wireguard-server wireguard-client
    if ($LASTEXITCODE -ne 0) { throw 'Docker environment startup failed' }
    & $docker compose --profile benchmark run --rm benchmark
    if ($LASTEXITCODE -ne 0) { throw 'Docker benchmark failed' }
}
finally {
    if ($env:KEEP_STACK -ne '1') {
        & $docker compose --profile benchmark down --volumes --remove-orphans
    }
}
