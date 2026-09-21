# Configuring services and migrating an existing deployment

Configuration uses `[server]` / `[client]`, `bind_addr` / `remote_addr`, named
`services`, per-service `type`, `token`, `local_addr` / `bind_addr`,
`default_token`, and `nodelay` (true by default).

1. Generate fresh keys with `cathole --genkey`, or complete example identities
   with `cathole --init identity`.
2. Existing `type = "noise"` files can retain their section names and service
   definitions. The tunnel underneath them is always QUIC/UDP.
3. NK, KK, and XX are accepted with X25519, ChaChaPoly, and BLAKE2s. NK needs the
   server private key and the matching remote public key on the client. KK also
   requires a pinned client key at the server. XX requires a pinned QUIC TLS
   certificate because XX does not authenticate a known peer by itself.
4. A `transport.quic` table is optional for NK and KK. When it is omitted, the
   server creates an ephemeral outer TLS certificate and Noise authenticates the
   connection before any token or payload is sent. Pin the generated TLS identity
   when using XX or when you want two independent server identity checks.
5. Change the client endpoint/public listener as appropriate. Allow **UDP** for
   the tunnel port; a TCP-only firewall rule is insufficient.
6. Validate with `--check`, start both endpoints, and test each named service.

Example additions to a client config:

```toml
[client.transport]
type = "noise"
[client.transport.noise]
remote_public_key = "YOUR_BASE64_NOISE_PUBLIC_KEY"
[client.transport.quic]
trusted_root = "server.der"
hostname = "cathole"
keep_alive_interval = 15
max_idle_timeout = 60
udp_idle_timeout = 60
max_streams = 256
max_udp_flows = 4096
```

Server QUIC configuration uses `certificate = "server.der"` and
`private_key = "server-key.der"` in place of `trusted_root`. Its Noise table
uses `local_private_key`. Freshly generated identities are preferable to any
example keys. Example configuration files intentionally contain no working
shared credentials in the repository.

Unknown fields are errors, not silently ignored settings. `heartbeat_timeout`,
`heartbeat_interval`, `retry_interval`, `prefer_ipv6`, transport TCP `nodelay`,
and per-service `nodelay`/`retry_interval` are accepted. TCP/WebSocket/TLS tunnel
types and HTTP/SOCKS proxy settings are rejected because they cannot carry this
UDP tunnel; change the transport type to `noise` or `quic`.
Both endpoints must run Cathole and use its versioned wire protocol.

Live edits restart all tunnel sessions, including unaffected services. Plan
around connection interruption. Invalid syntax/identity files are rejected
before shutdown; a server tunnel-bind failure after reload attempts to restore
the previous configuration. Service-port conflicts are reported at client
registration and require correction; they are not an atomic rollback guarantee.

Only one client can own a particular public service port at a time. Different
clients can register disjoint subsets. A stolen service token authorizes that
service until it is rotated; tokens do not constitute device-bound identity.
