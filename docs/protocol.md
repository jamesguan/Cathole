# Cathole wire protocol v2

Outer transport is either QUIC v1 over UDP (ALPN `cathole/2`) or a multiplexed
TCP (optional TLS) byte stream. No 0-RTT application data is used. QUIC permits
only bidirectional streams.

## Registration

The client opens the first stream (QUIC) or the tunnel connection (TCP) and
performs the configured NK, KK, or XX X25519/ChaChaPoly/BLAKE2s Noise handshake
**once per session**. After the handshake it sends a Noise transport-encrypted
JSON registration. JSON contains `version: 2` and an ordered `services` list of
`{name, kind, token}`. IDs are zero-based positions in this list. Up to 256
services are accepted and each encrypted frame is capped at 65,535 bytes.

On QUIC, Noise's prologue is `cathole/2` followed by 32 bytes exported from QUIC
TLS using label `cathole-noise-v1` and context `registration`, followed by that
context. On TCP, the prologue is `cathole/2` || `tcp` || `registration`. The
server authenticates every service, binds all its listeners, and answers with
encrypted `ok`. A 10-second deadline covers the entire setup.

Session Noise authenticates the tunnel. It does not encrypt subsequent QUIC
payloads: those are already TLS 1.3. TCP/Noise tunnels keep Noise AEAD on the
mux because the outer TCP path is otherwise plaintext. `type = tls` wraps the
mux in rustls and still authenticates with the same session handshake.

## TCP over QUIC

Each public TCP connection opens a server-initiated QUIC bidirectional stream.
The first two bytes are the big-endian u16 service ID. The remainder is a raw
byte relay with reusable 256 KiB buffers (`write_chunk` / `read_chunk`). There
is no per-stream Noise handshake and no application framing of ordinary TCP
data.

## TCP/Noise mux

After session authentication, frames are Noise-sealed length-prefixed records:

| Type | Layout |
| --- | --- |
| OPEN (1) | `u32` stream ID, `u16` service ID |
| DATA (2) | `u32` stream ID, payload |
| FIN (3) | `u32` stream ID |
| UDP (4) | `u16` service, `u64` flow, payload |
| HEARTBEAT (5) | empty |

Send and receive Noise nonces are independent; the halves are not behind a
shared mutex. Ciphertext buffers are reused from a per-worker pool.

## UDP

The server assigns a monotonically increasing flow ID for each
`(service ID, public source IP, public source port)` mapping. The client creates
a connected UDP socket to that service's configured destination.

On QUIC, each DATAGRAM contains the fragment header plus payload. QUIC packet
protection encrypts it; there is no second application AEAD.

| Field | Size |
| --- | ---: |
| Service ID | u16 |
| Flow ID | u64 |
| Message ID | u64 |
| Original UDP payload length | u16 |
| Fragment index | u16 |
| Fragment payload | 0–1000 bytes |

On TCP/Noise, UDP uses mux type 4 instead of QUIC DATAGRAM.

Reassembly accepts reordering, caps incomplete messages at 128, and expires them
after 2 seconds. Loss discards the application message. All maps and keys are
discarded on reconnect.

## Bounds and lifecycle

Defaults per connection: 1024 TCP streams, 4096 UDP flows, 128 MiB QUIC
connection send/receive windows, 16 MiB stream receive windows, 256 KiB relay
chunks, and an 8 MiB aggregate client UDP forwarding queue. Server connection
limit is 128. QUIC Retry validates addresses before session allocation.
Congestion control is Cubic by default; `transport.quic.congestion = "bbr"`
selects BBR. Sessions expire after 24 hours.
