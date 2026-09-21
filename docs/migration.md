# Configuring services and migrating an existing deployment

Configuration uses `[server]` / `[client]`, `bind_addr` / `remote_addr`, named
`services`, per-service `type`, `token`, `local_addr` / `bind_addr`,
`default_token`, and `nodelay` (true by default).

1. Generate fresh files with `cathole --init identity`.
2. Copy your explicit service definitions into the matching generated configs.
3. Set `[client.transport]` and `[server.transport]` to `type = "quic"`.
4. Keep the generated Noise keys, or transfer your compatible NK keys. Only
   `Noise_NK_25519_ChaChaPoly_BLAKE2s` is accepted. XX and other patterns are rejected.
5. Keep the generated `transport.quic` identity paths. Clients need the server
   certificate and Noise public key. The server needs both private identities.
6. Change the client endpoint/public listener as appropriate. Allow **UDP** for
   the tunnel port; a TCP-only firewall rule is insufficient.
7. Validate with `--check`, start both endpoints, and test each named service.

Example additions to a client config:

```toml
[client.transport]
type = "quic"
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

Unknown fields are errors, not silently ignored settings. TCP/WebSocket tunnel
options, legacy heartbeat/retry fields, and proxy protocol are unsupported. Put
keepalive and timeout values in the QUIC table and remove obsolete options.
Both endpoints must run Cathole and use its versioned wire protocol.

Live edits restart all tunnel sessions, including unaffected services. Plan
around connection interruption. Invalid syntax/identity files are rejected
before shutdown; a server tunnel-bind failure after reload attempts to restore
the previous configuration. Service-port conflicts are reported at client
registration and require correction; they are not an atomic rollback guarantee.

Only one client can own a particular public service port at a time. Different
clients can register disjoint subsets. A stolen service token authorizes that
service until it is rotated; tokens do not constitute device-bound identity.
