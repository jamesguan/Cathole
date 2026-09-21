# Security review and remaining work

This is a local implementation review, not an independent audit.

Implemented and covered by code/tests:

- TLS certificate validation against a dedicated trust root and hostname when a
  root is configured. NK/KK compatibility mode can instead defer peer identity
  to the pinned Noise keys while retaining TLS encryption and signature checks.
- Pinned Noise NK server identity, mutual pinned keys with KK, or XX with a
  mandatory pinned QUIC certificate. Per-service tokens are mandatory.
- Noise prologue binds the session and TCP stream context to the TLS exporter.
- AEAD integrity and a post-authentication UDP replay window; directional nonce
  separation, fresh session keys, fail-closed nonce budget.
- Service registration validation before opening listeners. No client-supplied
  arbitrary destination address. Connected local UDP sockets filter sources.
- Setup deadlines, stream/session/flow limits, bounded frames, queues, replay
  and reassembly state, expiring UDP associations, QUIC Retry.
- Invalid configuration errors avoid echoing secret-bearing TOML source lines.
- Test identities stay in ignored `target/`; production identities stay outside
  version control. `--init` refuses overwrites and creates Unix files as 0600.

Required before treating it as a production replacement:

- Independent cryptographic protocol/composition review, parser fuzzing and
  adversarial load testing, particularly pre-authentication flood resistance.
- Seamless Noise rekey and graceful live-service updates. Present renewal/reload
  closes flows; it is not suitable for uninterrupted multi-day TCP sessions.
- WAN testing across real NAT/firewalls, IPv6 paths, MTU black holes, burst loss,
  outages, and sustained saturation with public untrusted UDP clients.
- Resource/CPU/latency measurements against the existing deployment and
  review of platform-specific UDP offloads and socket buffer behavior.
- Dependency audit and reproducible Linux/container deployment validation.
- Operational counters/exported metrics and rate-limited security event logging.

An authenticated client can consume its configured allowances. Public UDP
services can reflect/amplify traffic according to the exposed application's
behavior. The transport does not make an unsafe public DNS/game/admin service
safe. Set deployment limits and bind only the services you intend to expose.

With NK, only the server identity is a static Noise key and client authorization
uses encrypted bearer tokens. Use KK when both endpoints require pinned static
Noise identities. In certificate-free NK/KK mode, the ephemeral QUIC certificate
is not an identity credential; the inner Noise handshake must succeed before any
token or application payload is accepted. XX therefore requires a pinned QUIC
certificate and is rejected without one.
