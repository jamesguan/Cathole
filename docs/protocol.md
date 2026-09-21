# Cathole wire protocol v1

Outer transport is QUIC v1 over UDP, ALPN `cathole/1`, validated TLS 1.3. No 0-RTT
application data is used. Only bidirectional streams are permitted.

## Registration

The client opens the first stream and performs the configured NK, KK, or XX
X25519/ChaChaPoly/BLAKE2s Noise handshake. After the handshake it sends a Noise
transport-encrypted JSON registration. JSON contains `version: 1` and an ordered `services`
list of `{name, kind, token}`. IDs are zero-based positions in this list. The
client orders services lexicographically. Up to 256 services are accepted and
each encrypted frame is capped at 65,535 bytes. Excessive combined names/tokens
fail closed.

Noise's prologue is `cathole/1` followed by 32 bytes exported from QUIC TLS using
label `cathole-noise-v1` and context `registration`, followed by that context.
The server authenticates every service, binds all its listeners, and answers
with encrypted `ok`. Both directions finish the control stream. A 10-second
deadline covers the entire setup. The resulting Noise split keys are used only
for UDP payloads, through snow's stateless transport API.

Every framed handshake or TCP record starts with a big-endian u32 length.
Frames are capped at 65,535 bytes. FIN at a frame boundary is orderly; FIN within
a frame is an error. No credentials are logged.

## TCP

Each public TCP connection opens a server-initiated QUIC bidirectional stream.
The first TLS-protected frame contains the big-endian u16 service ID. A fresh
configured Noise handshake follows, still with the home client as initiator. Its
context is `tcp || service_id(u16) || quic_stream_id(u64)`, big-endian, and uses
the same exporter/prologue construction. The handshake request is empty; the
server response is `ok`.

Subsequent frames are Noise transport ciphertext. Plaintext chunks are at most
16,384 bytes; authentication tags add 16 bytes. Separate Noise send and receive
nonces are maintained by snow. A short synchronous mutex protects the state, not
any network operation. TCP FIN shuts down only the matching stream direction,
allowing a response after a request half-close. Errors reset/drop the affected
stream and socket. Each TCP stream gets fresh keys.

## UDP

The server assigns a monotonically increasing flow ID for each
`(service ID, public source IP, public source port)` mapping. The client creates
a connected UDP socket to that service's configured destination. Only that
destination can supply replies to the socket. Replies use the same service/flow
IDs; the server routes only matching live mappings. Idle mappings expire.

Each QUIC DATAGRAM contains:

| Field | Size |
| --- | ---: |
| Explicit Noise nonce, big-endian | 8 bytes |
| Noise ciphertext and authentication tag | variable |

The encrypted plaintext contains:

| Field | Size |
| --- | ---: |
| Service ID | u16 |
| Flow ID | u64 |
| Message ID | u64 |
| Original UDP payload length | u16 |
| Fragment index | u16 |
| Fragment payload | 0–1000 bytes |

Integers are big-endian. Fragment count is `max(1, ceil(length / 1000))`.
Non-final fragments must contain exactly 1000 bytes; the final length must match
the advertised original length. Empty application packets have one empty
fragment. Maximum encrypted QUIC DATAGRAM payload is 1046 bytes. Quinn handles
outer UDP packet sizes/PMTU; this layer does not rely on IP fragmentation.

Each direction uses independent split keys and a strictly increasing send nonce.
The receiver authenticates before updating a 1024-packet replay window.
Reassembly accepts reordering, caps incomplete messages at 128 (about 8.4 MB
payload maximum), and expires them after 2 seconds. Loss discards the application
message, never retransmits it. Traffic delayed beyond the replay window is
dropped as permitted by UDP semantics. Message IDs are never reused within a
session. All maps and keys are discarded on reconnect.

## Bounds and lifecycle

Defaults per connection: 256 TCP streams, 4096 UDP flows, 1 MiB QUIC send/receive
datagram queues, 32 MiB QUIC connection receive/send windows, 16 MiB stream receive
windows, and an 8 MiB aggregate client UDP forwarding queue. Each flow queue has
8 message slots; a full queue drops the new application packet. Server connection
limit is 128; QUIC Retry validates addresses before session allocation. Deployment
limits should be reduced for small machines and untrusted public workloads.

The UDP send-key budget is 2^32 encrypted fragments. Exhaustion closes the tunnel
and triggers fresh authentication rather than wrapping nonces. All sessions also
expire after 24 hours. This is safe but disruptive renewal, not seamless rekey.
QUIC TLS key updates remain Quinn's responsibility. All protocol constants and
limits are versioned with this implementation.
