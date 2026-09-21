"""Standard-library endpoint benchmark. Does not change network settings.

On backend: python benchmarks/compare.py serve --port 5201
On tester:  python benchmarks/compare.py run --target direct=HOST:5201 \
              --target cathole=HOST:5202 --target baseline=HOST:5203
All proxy targets must forward TCP and UDP to the same echo backend.
"""
import argparse
import concurrent.futures
import json
import math
import socket
import statistics
import struct
import threading
import time

def recv_exact(s, count):
    result = bytearray()
    while len(result) < count:
        data = s.recv(min(65536, count - len(result)))
        if not data:
            raise RuntimeError("truncated echo")
        result.extend(data)
    return result

def echo(s):
    with s:
        while True:
            data = s.recv(65536)
            if not data:
                return
            s.sendall(data)

def serve(args):
    family = socket.AF_INET6 if ":" in args.bind else socket.AF_INET
    tcp = socket.socket(family, socket.SOCK_STREAM)
    tcp.bind((args.bind, args.port))
    tcp.listen(256)
    udp = socket.socket(family, socket.SOCK_DGRAM)
    udp.bind((args.bind, args.port))
    def udp_loop():
        while True:
            data, source = udp.recvfrom(65536)
            udp.sendto(data, source)
    threading.Thread(target=udp_loop, daemon=True).start()
    print(f"TCP/UDP echo on {args.bind}:{args.port}", flush=True)
    while True:
        s, _ = tcp.accept()
        s.setsockopt(socket.IPPROTO_TCP, socket.TCP_NODELAY, 1)
        threading.Thread(target=echo, args=(s,), daemon=True).start()

def percentile(values, p):
    ordered = sorted(values)
    return ordered[min(len(ordered)-1, math.ceil(len(ordered)*p)-1)] if values else None

def parse_target(value):
    name, addr = value.split("=", 1)
    host, port = addr.rsplit(":", 1)
    return name, (host.strip("[]"), int(port))

def bulk(addr, count, timeout):
    payload = b"C" * 65536
    with socket.create_connection(addr, timeout) as s:
        s.settimeout(timeout)
        def read_bulk():
            remaining = count
            while remaining:
                chunk = s.recv(min(65536, remaining))
                if not chunk or chunk != b"C" * len(chunk):
                    raise RuntimeError("bulk stream corruption/truncation")
                remaining -= len(chunk)
        with concurrent.futures.ThreadPoolExecutor(max_workers=1) as pool:
            reader = pool.submit(read_bulk)
            for _ in range(count // len(payload)):
                s.sendall(payload)
            reader.result(timeout=timeout)

def measure(target, args):
    label, addr = parse_target(target)
    latency = []
    with socket.create_connection(addr, args.timeout) as s:
        s.settimeout(args.timeout)
        s.setsockopt(socket.IPPROTO_TCP, socket.TCP_NODELAY, 1)
        for i in range(args.samples + 10):
            message = bytes([i % 256]) * 64
            start = time.perf_counter()
            s.sendall(message)
            assert recv_exact(s, len(message)) == message
            elapsed = (time.perf_counter() - start)*1000
            if i >= 10:
                latency.append(elapsed)
    count = args.mib * 1024 * 1024
    with concurrent.futures.ThreadPoolExecutor(max_workers=args.parallel) as pool:
        start = time.perf_counter()
        list(pool.map(lambda _: bulk(addr, count, args.timeout), range(args.parallel)))
        seconds = time.perf_counter() - start
    # Fixed-rate probes, not stop-and-wait: loss does not throttle probe generation.
    family, _, _, _, dest = socket.getaddrinfo(*addr, type=socket.SOCK_DGRAM)[0]
    udp_rtts = []
    with socket.socket(family, socket.SOCK_DGRAM) as s:
        s.connect(dest)
        s.settimeout(0.05)
        sent = {}
        done = threading.Event()
        def collect():
            while not done.is_set():
                try:
                    p = s.recv(65536)
                    if len(p) == 200:
                        seq = struct.unpack("!Q", p[:8])[0]
                        start = sent.pop(seq, None)
                        if start is not None:
                            udp_rtts.append((time.perf_counter()-start)*1000)
                except socket.timeout:
                    pass
        thread = threading.Thread(target=collect)
        thread.start()
        try:
            for seq in range(args.samples):
                sent[seq] = time.perf_counter()
                s.send(struct.pack("!Q", seq) + b"U" * 192)
                time.sleep(0.01)
            time.sleep(min(args.timeout, 2))
        finally:
            done.set()
            thread.join()
    return {"target": label, "address": addr, "parallel": args.parallel, "mib_per_stream": args.mib, "tcp_mbit_s_echo": count*args.parallel*8/seconds/1e6,
        "tcp_rtt_ms_p50": statistics.median(latency), "tcp_rtt_ms_p95": percentile(latency, .95), "tcp_rtt_ms_p99": percentile(latency, .99),
        "udp_sent": args.samples, "udp_received": len(udp_rtts), "udp_loss_fraction": 1-len(udp_rtts)/args.samples,
        "udp_rtt_ms_p50": percentile(udp_rtts, .5), "udp_rtt_ms_p99": percentile(udp_rtts, .99)}

parser = argparse.ArgumentParser()
sub = parser.add_subparsers(dest="mode", required=True)
p = sub.add_parser("serve")
p.add_argument("--bind", default="127.0.0.1")
p.add_argument("--port", type=int, default=5201)
p = sub.add_parser("run")
p.add_argument("--target", action="append", required=True)
p.add_argument("--samples", type=int, default=300)
p.add_argument("--mib", type=int, default=64)
p.add_argument("--timeout", type=float, default=60)
p.add_argument("--parallel", type=int, default=1)
args = parser.parse_args()
if args.mode == "serve":
    serve(args)
else:
    if args.samples <= 0 or args.mib <= 0 or args.timeout <= 0 or args.parallel <= 0:
        parser.error("samples, mib, timeout, and parallel must be positive")
    for target in args.target:
        print(json.dumps(measure(target, args)), flush=True)
