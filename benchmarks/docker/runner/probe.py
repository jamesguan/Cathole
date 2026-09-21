#!/usr/bin/env python3
"""TCP echo, UDP echo, and HTTPS request probes for the Docker harness."""
from __future__ import annotations

import argparse
import concurrent.futures
import json
import math
import socket
import ssl
import struct
import threading
import time
import urllib.error
import urllib.request


def percentile(values: list[float], p: float) -> float | None:
    if not values:
        return None
    ordered = sorted(values)
    return ordered[min(len(ordered) - 1, math.ceil(len(ordered) * p) - 1)]


def recv_exact(sock: socket.socket, count: int) -> bytes:
    result = bytearray()
    while len(result) < count:
        chunk = sock.recv(min(65536, count - len(result)))
        if not chunk:
            raise RuntimeError("truncated echo")
        result.extend(chunk)
    return bytes(result)


def tcp_probe(host: str, port: int, samples: int, mib: int, parallel: int, timeout: float) -> dict:
    addr = (host, port)
    latencies_ms: list[float] = []
    with socket.create_connection(addr, timeout) as sock:
        sock.settimeout(timeout)
        sock.setsockopt(socket.IPPROTO_TCP, socket.TCP_NODELAY, 1)
        for i in range(samples + 10):
            message = bytes([i % 256]) * 64
            start = time.perf_counter()
            sock.sendall(message)
            assert recv_exact(sock, len(message)) == message
            elapsed = (time.perf_counter() - start) * 1000
            if i >= 10:
                latencies_ms.append(elapsed)

    payload = b"C" * 65536
    total = mib * 1024 * 1024

    def bulk(_: int) -> None:
        with socket.create_connection(addr, timeout) as sock:
            sock.settimeout(timeout)
            sock.setsockopt(socket.IPPROTO_TCP, socket.TCP_NODELAY, 1)

            def reader() -> None:
                remaining = total
                while remaining:
                    chunk = sock.recv(min(65536, remaining))
                    if not chunk or chunk != b"C" * len(chunk):
                        raise RuntimeError("bulk stream corruption/truncation")
                    remaining -= len(chunk)

            with concurrent.futures.ThreadPoolExecutor(max_workers=1) as pool:
                fut = pool.submit(reader)
                for _ in range(total // len(payload)):
                    sock.sendall(payload)
                fut.result(timeout=timeout)

    start = time.perf_counter()
    with concurrent.futures.ThreadPoolExecutor(max_workers=parallel) as pool:
        list(pool.map(bulk, range(parallel)))
    seconds = max(time.perf_counter() - start, 1e-9)
    bytes_total = total * parallel
    return {
        "protocol": "tcp",
        "host": host,
        "port": port,
        "samples": samples,
        "latency_ms_p50": percentile(latencies_ms, 0.50),
        "latency_ms_p95": percentile(latencies_ms, 0.95),
        "latency_ms_p99": percentile(latencies_ms, 0.99),
        "latency_ms_mean": (sum(latencies_ms) / len(latencies_ms)) if latencies_ms else None,
        "bytes": bytes_total,
        "duration_seconds": seconds,
        "bytes_per_second": bytes_total / seconds,
        "requests": samples,
        "errors": 0,
        "ok": True,
    }


def udp_probe(host: str, port: int, samples: int, timeout: float) -> dict:
    family, _, _, _, dest = socket.getaddrinfo(host, port, type=socket.SOCK_DGRAM)[0]
    rtts_ms: list[float] = []
    sent: dict[int, float] = {}
    lock = threading.Lock()
    done = threading.Event()

    with socket.socket(family, socket.SOCK_DGRAM) as sock:
        sock.connect(dest)
        sock.settimeout(timeout)

        def collect() -> None:
            while not done.is_set():
                try:
                    packet = sock.recv(65536)
                except socket.timeout:
                    continue
                except OSError:
                    return
                if len(packet) != 200:
                    continue
                seq = struct.unpack("!Q", packet[:8])[0]
                with lock:
                    start = sent.pop(seq, None)
                if start is not None:
                    rtts_ms.append((time.perf_counter() - start) * 1000)

        thread = threading.Thread(target=collect, daemon=True)
        thread.start()
        try:
            for seq in range(samples):
                payload = struct.pack("!Q", seq) + bytes([seq % 256]) * 192
                with lock:
                    sent[seq] = time.perf_counter()
                sock.send(payload)
                time.sleep(0.002)
            time.sleep(0.25)
        finally:
            done.set()
            thread.join(timeout=1)

    received = len(rtts_ms)
    lost = samples - received
    return {
        "protocol": "udp",
        "host": host,
        "port": port,
        "samples": samples,
        "received": received,
        "lost": lost,
        "loss_rate": (lost / samples) if samples else 0.0,
        "latency_ms_p50": percentile(rtts_ms, 0.50),
        "latency_ms_p95": percentile(rtts_ms, 0.95),
        "latency_ms_p99": percentile(rtts_ms, 0.99),
        "latency_ms_mean": (sum(rtts_ms) / len(rtts_ms)) if rtts_ms else None,
        "bytes": received * 200,
        "requests": samples,
        "errors": lost,
        "ok": received > 0,
    }


def https_probe(url: str, samples: int, duration: float, workers: int, path: str) -> dict:
    target = url.rstrip("/") + path
    ctx = ssl.create_default_context()
    ctx.check_hostname = False
    ctx.verify_mode = ssl.CERT_NONE
    latencies_ms: list[float] = []
    errors = 0

    def one_get() -> float:
        req = urllib.request.Request(target, method="GET")
        start = time.perf_counter()
        with urllib.request.urlopen(req, context=ctx, timeout=15) as resp:
            body = resp.read()
            if resp.status != 200 or not body:
                raise RuntimeError(f"bad https response status={resp.status}")
        return (time.perf_counter() - start) * 1000

    def one_get_retry(attempts: int = 8) -> float:
        last: Exception | None = None
        for i in range(attempts):
            try:
                return one_get()
            except Exception as exc:  # noqa: BLE001
                last = exc
                time.sleep(0.25 * (i + 1))
        assert last is not None
        raise last

    for _ in range(10):
        one_get_retry()
    for _ in range(samples):
        try:
            latencies_ms.append(one_get_retry(attempts=3))
        except Exception:
            errors += 1

    counter = {"ok": 0, "err": 0}
    stop_at = time.perf_counter() + duration

    def worker() -> None:
        while time.perf_counter() < stop_at:
            try:
                one_get()
                counter["ok"] += 1
            except Exception:
                counter["err"] += 1

    start = time.perf_counter()
    with concurrent.futures.ThreadPoolExecutor(max_workers=workers) as pool:
        futs = [pool.submit(worker) for _ in range(workers)]
        for fut in concurrent.futures.as_completed(futs):
            fut.result()
    seconds = max(time.perf_counter() - start, 1e-9)
    total_ok = counter["ok"]
    total_err = counter["err"] + errors
    return {
        "protocol": "https",
        "url": target,
        "samples": samples,
        "latency_ms_p50": percentile(latencies_ms, 0.50),
        "latency_ms_p95": percentile(latencies_ms, 0.95),
        "latency_ms_p99": percentile(latencies_ms, 0.99),
        "latency_ms_mean": (sum(latencies_ms) / len(latencies_ms)) if latencies_ms else None,
        "duration_seconds": seconds,
        "requests": total_ok + samples,
        "requests_per_second": total_ok / seconds,
        "errors": total_err,
        "ok": errors == 0 and total_ok > 0,
    }


def main() -> None:
    parser = argparse.ArgumentParser()
    sub = parser.add_subparsers(dest="cmd", required=True)

    tcp = sub.add_parser("tcp")
    tcp.add_argument("host")
    tcp.add_argument("port", type=int)
    tcp.add_argument("--samples", type=int, default=100)
    tcp.add_argument("--mib", type=int, default=32)
    tcp.add_argument("--parallel", type=int, default=4)
    tcp.add_argument("--timeout", type=float, default=15.0)

    udp = sub.add_parser("udp")
    udp.add_argument("host")
    udp.add_argument("port", type=int)
    udp.add_argument("--samples", type=int, default=200)
    udp.add_argument("--timeout", type=float, default=0.05)

    https = sub.add_parser("https")
    https.add_argument("url")
    https.add_argument("--samples", type=int, default=50)
    https.add_argument("--duration", type=float, default=5.0)
    https.add_argument("--workers", type=int, default=32)
    https.add_argument("--path", default="/")

    args = parser.parse_args()
    try:
        if args.cmd == "tcp":
            result = tcp_probe(args.host, args.port, args.samples, args.mib, args.parallel, args.timeout)
        elif args.cmd == "udp":
            result = udp_probe(args.host, args.port, args.samples, args.timeout)
        else:
            result = https_probe(args.url, args.samples, args.duration, args.workers, args.path)
    except Exception as exc:  # noqa: BLE001 - surface as JSON for the harness
        result = {
            "protocol": args.cmd,
            "ok": False,
            "errors": 1,
            "requests": 0,
            "error": str(exc),
        }
    print(json.dumps(result, separators=(",", ":")))


if __name__ == "__main__":
    main()
