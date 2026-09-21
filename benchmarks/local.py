"""Portable release-build benchmark smoke test. Loopback is not a WAN result."""
import json
import os
from pathlib import Path
import socket
import subprocess
import sys
import tempfile
import time

binary = str(Path(sys.argv[1] if len(sys.argv) > 1 else ("target/release/cathole.exe" if os.name == "nt" else "target/release/cathole")).resolve())
work = Path(tempfile.mkdtemp(prefix="bench-", dir="target"))
identity = work / "identity"
subprocess.run([binary, "--init", str(identity)], check=True, stdout=subprocess.DEVNULL)
def port():
    with socket.socket() as s:
        s.bind(("127.0.0.1", 0))
        return s.getsockname()[1]
backend, public, tunnel = port(), port(), port()
for name in ["client", "server"]:
    path = identity / f"{name}.toml"
    text = path.read_text().replace(":2333", f":{tunnel}").replace(":5202", f":{public}").replace(":5201", f":{backend}")
    path.write_text(text)
processes, logs = [], []
def start(command):
    log = open(work / f"process-{len(logs)}.log", "w")
    logs.append(log)
    processes.append(subprocess.Popen(command, stdout=log, stderr=subprocess.STDOUT))
try:
    start([sys.executable, "benchmarks/compare.py", "serve", "--port", str(backend)])
    start([binary, str(identity / "server.toml")])
    start([binary, str(identity / "client.toml")])
    deadline = time.monotonic() + 30
    while True:
        try:
            with socket.create_connection(("127.0.0.1", public), 1) as s:
                s.sendall(b"ready")
                assert s.recv(5) == b"ready"
            break
        except (OSError, AssertionError):
            if time.monotonic() > deadline:
                raise RuntimeError("benchmark tunnel not ready")
            time.sleep(.1)
    results = []
    for parallel in [1, 4, 16]:
        command = [sys.executable, "benchmarks/compare.py", "run", "--target", f"direct=127.0.0.1:{backend}", "--target", f"cathole=127.0.0.1:{public}", "--mib", "32", "--samples", "100", "--parallel", str(parallel)]
        output = subprocess.check_output(command, text=True)
        for line in output.splitlines():
            result = json.loads(line)
            results.append(result)
            print(json.dumps(result), flush=True)
    (work / "results.json").write_text(json.dumps({"environment": "local loopback; direct and Cathole echo endpoints", "results": results}, indent=2))
    print(f"Saved {work / 'results.json'}")
finally:
    for p in processes:
        if p.poll() is None:
            p.terminate()
            p.wait(timeout=10)
    for log in logs:
        log.close()
