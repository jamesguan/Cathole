$ErrorActionPreference = 'Stop'
$PSNativeCommandUseErrorActionPreference = $false
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
if (-not $env:THREADS) { $env:THREADS = '16' }
if (-not $env:DURATION) { $env:DURATION = '10' }
if (-not $env:STRESS_DURATION) { $env:STRESS_DURATION = '10' }
if (-not $env:SAMPLES) { $env:SAMPLES = '1' }
if (-not $env:TESTS) { $env:TESTS = 'correctness,latency,saturation,stress,stream,tcp_packets,udp_packets,https' }
if (-not $env:VIDEO_BYTES) { $env:VIDEO_BYTES = '10737418240' }
if (-not $env:STREAM_TIMEOUT) { $env:STREAM_TIMEOUT = '900' }

$proxyServices = @(
    'rathole-server', 'rathole-client',
    'frp-server', 'frp-client',
    'cathole-server', 'cathole-client',
    'wireguard-server', 'wireguard-client'
)

$targets = @(
    @{ Name = 'direct'; Services = @() },
    @{ Name = 'rathole'; Services = @('rathole-server', 'rathole-client') },
    @{ Name = 'wireguard'; Services = @('wireguard-server', 'wireguard-client') },
    @{ Name = 'frp'; Services = @('frp-server', 'frp-client') },
    @{ Name = 'cathole'; Services = @('cathole-server', 'cathole-client') }
)

function Invoke-Docker {
    param([Parameter(Mandatory = $true)][string[]]$ComposeArgs)
    & $docker @ComposeArgs
    if ($LASTEXITCODE -ne 0) {
        throw "docker $($ComposeArgs -join ' ') failed with exit $LASTEXITCODE"
    }
}

function Stop-ProxyStacks {
    & $docker --log-level error compose --progress quiet stop @proxyServices | Out-Null
}

try {
    Invoke-Docker -ComposeArgs @('compose', '--profile', 'benchmark', 'build')
    Invoke-Docker -ComposeArgs @('compose', '--profile', 'benchmark', 'down', '--volumes', '--remove-orphans')
    New-Item -ItemType Directory -Force -Path .\results | Out-Null
    if ($env:PRESERVE_WRK -eq '1') {
        python -c @"
import json, pathlib
src = pathlib.Path(r'results/latest.json')
out = pathlib.Path(r'results/raw.jsonl')
keep = {'latency', 'saturation', 'stress'}
lines = []
if src.exists():
    data = json.loads(src.read_text(encoding='utf-8'))
    for row in data.get('results', []):
        if row.get('test') in keep:
            lines.append(json.dumps(row, separators=(',', ':')))
out.write_text(('\n'.join(lines) + ('\n' if lines else '')), encoding='utf-8')
print('preserved', len(lines), 'prior wrk samples')
"@
    }
    else {
        Set-Content -Path .\results\raw.jsonl -Value '' -Encoding utf8
        Write-Host 'starting with empty raw.jsonl (set PRESERVE_WRK=1 to keep prior wrk samples)'
    }
    Invoke-Docker -ComposeArgs @('compose', 'up', '-d', 'backend')

    foreach ($target in $targets) {
        Write-Host "========== Isolated target: $($target.Name) =========="
        Stop-ProxyStacks
        # Fresh backend per target so TLS/echo sockets from prior stacks cannot accumulate.
        Invoke-Docker -ComposeArgs @('compose', 'up', '-d', '--force-recreate', 'backend')
        if ($target.Services.Count -gt 0) {
            Invoke-Docker -ComposeArgs (@('compose', 'up', '-d', '--force-recreate') + $target.Services)
        }
        Invoke-Docker -ComposeArgs @(
            'compose', '--profile', 'benchmark', 'run', '--rm', '--no-deps',
            '-e', "TARGET=$($target.Name)", '-e', 'GENERATE_REPORT=0',
            "--env=TESTS=$($env:TESTS)", "--env=VIDEO_BYTES=$($env:VIDEO_BYTES)",
            '--name', "bench-$($target.Name)", 'benchmark'
        )
        if ($target.Services.Count -gt 0) {
            Invoke-Docker -ComposeArgs (@('compose', 'stop') + $target.Services)
        }
        Start-Sleep -Seconds 2
    }

    Invoke-Docker -ComposeArgs @(
        'compose', '--profile', 'benchmark', 'run', '--rm', '--no-deps',
        '-e', 'TARGET=', '-e', 'GENERATE_REPORT=1',
        "--env=TESTS=$($env:TESTS)", "--env=VIDEO_BYTES=$($env:VIDEO_BYTES)",
        '--name', 'bench-report', 'benchmark'
    )

    Copy-Item -Force .\results\latest.json ..\..\BENCHMARK-RESULTS.json
    Copy-Item -Force .\results\latest.md ..\..\BENCHMARK-RESULTS.md
}
finally {
    if ($env:KEEP_STACK -ne '1') {
        & $docker --log-level error compose --progress quiet --profile benchmark down --volumes --remove-orphans | Out-Null
    }
}
