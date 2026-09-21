# Architecture

## Decision

Build an independent Rust implementation using Tokio, Quinn, rustls and snow.
Use explicit named services and TOML configuration. A byte-stream abstraction for
UDP would conceal the distinction between reliable streams and unreliable
messages. This implementation keeps those paths separate.

The server authenticates a service registration before opening its public
listeners. A client can claim only configured services with matching types and
tokens. Registration is all-or-nothing: a failed bind drops the listeners already
created by that attempt. Two clients cannot own the same bound service port.

```
public TCP <-> server TCP socket <-> Noise records / QUIC stream
                                   <-> UDP Internet <-> client <-> local TCP

public UDP <-> server source mapping <-> Noise / QUIC DATAGRAM
                                      <-> UDP Internet <-> client UDP socket
```

## Transport design

Service configuration is separate from tunnel transport. One multiplexed QUIC
connection carries reliable streams and a distinct datagram channel. TCP bytes
already acknowledged by the proxy must be delivered reliably; UDP messages need
explicit boundaries and bounded reassembly.

The implementation uses Quinn and DER PKCS#8/rustls identities. Graceful server
reload waits for endpoint draining before binding again. Third-party Rust
dependencies retain their own licenses in their packages.

## TCP versus raw-packet tunneling

Cathole terminates each TCP leg independently and forwards application bytes
over QUIC. The Internet tunnel uses UDP. Independent QUIC streams can make
progress without stream-order head-of-line blocking from another stream.
All streams still share congestion capacity and connection flow-control limits;
loss can reduce throughput for the entire connection.

## Loss, decay, and congestion

[RFC 9002](https://www.rfc-editor.org/rfc/rfc9002.html) specifies RTT-based loss
detection, probe timeouts and congestion behavior. Quinn owns these mechanisms;
we do not add a competing exponential retransmission timer or a second ACK layer.
Repeated loss legitimately reduces send rate. Do not disable congestion control
to make a benchmark look faster. Reconnection backoff is a separate bounded
process, with jitter and a reset after a stable session.

If a UDP application message uses k fragments, independent per-fragment loss p
gives delivery probability approximately (1-p)^k, before considering batching or
correlated loss. A 65 KB datagram has 66 fragments here: at 1% independent fragment
loss its delivery probability is only about 51.5%. This is a real exponential
effect; reliability is deliberately not added. Reduce application packet size,
or use a reliable service when the application needs complete delivery.

[RFC 9221](https://www.rfc-editor.org/rfc/rfc9221.html) datagrams participate in
QUIC congestion control without retransmission. Queue limits prevent indefinite
growth. No default FEC: redundant traffic can worsen congestion and needs
workload-specific measurement. Quinn provides packet pacing, loss recovery and
platform-dependent UDP batching/offload; this application does not claim every
OS or network supports those optimizations.

## NAT and bidirectional operation

Client-originated traffic opens the return path to the public server. QUIC
connection IDs and path validation support NAT port rebinding. Keepalives maintain
idle mappings where the router permits them, without guaranteeing traversal of
every firewall. If both endpoints are unreachable behind NAT, a separate
rendezvous/ICE/relay design is needed; it is out of this version's scope.
No raw IP packet forwarding or source-IP transparency is implied.

## Security choice

Standard QUIC mandates TLS 1.3; swapping it for Noise is not standard QUIC.
We use a pinned trust root plus hostname validation for QUIC, and the user's
requested Noise NK inside it. This duplicates encryption and introduces our own
application framing, but does not invent a cipher, handshake pattern, or loss
recovery protocol. The Noise prologue binds to a QUIC TLS exporter and the stream
context, preventing cross-connection/stream substitution. Service tokens remain
encrypted, even in the first NK message. This composition needs independent
review; using mature primitives does not itself audit an application protocol.

For a future leaner version, choose either QUIC/TLS alone or a well-established
Noise-based IP tunnel such as WireGuard plus service enforcement. Neither change
is silently made here. Measure the cost of the two layers before optimization.
