use anyhow::{Result, bail, ensure};
use base64::{Engine, engine::general_purpose::STANDARD};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, path::Path};

pub const PATTERN: &str = "Noise_NK_25519_ChaChaPoly_BLAKE2s";

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub server: Option<Side>,
    pub client: Option<Side>,
}

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Side {
    pub bind_addr: Option<String>,
    pub remote_addr: Option<String>,
    pub default_token: Option<String>,
    #[serde(default)]
    pub prefer_ipv6: bool,
    #[serde(default = "heartbeat_timeout")]
    pub heartbeat_timeout: u64,
    #[serde(default = "heartbeat_interval")]
    pub heartbeat_interval: u64,
    #[serde(default = "retry_interval")]
    pub retry_interval: u64,
    #[serde(default)]
    pub transport: Transport,
    pub services: BTreeMap<String, Service>,
}
fn heartbeat_timeout() -> u64 {
    40
}
fn heartbeat_interval() -> u64 {
    30
}
fn retry_interval() -> u64 {
    1
}

#[derive(Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Transport {
    #[serde(rename = "type")]
    pub kind: String,
    pub tcp: TcpCompat,
    pub tls: Option<TlsCompat>,
    pub noise: Noise,
    pub websocket: Option<WebsocketCompat>,
    pub quic: Quic,
}
impl Default for Transport {
    fn default() -> Self {
        Self {
            kind: "quic".into(),
            tcp: TcpCompat::default(),
            tls: None,
            noise: Noise::default(),
            websocket: None,
            quic: Quic::default(),
        }
    }
}

/// TCP socket options for forwarded connections and for `transport.type = "tcp"|"tls"` tunnels.
#[derive(Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct TcpCompat {
    pub proxy: Option<String>,
    pub nodelay: bool,
    pub keepalive_secs: u64,
    pub keepalive_interval: u64,
}
impl Default for TcpCompat {
    fn default() -> Self {
        Self {
            proxy: None,
            nodelay: true,
            keepalive_secs: 20,
            keepalive_interval: 8,
        }
    }
}

#[derive(Clone, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct TlsCompat {
    pub hostname: Option<String>,
    pub trusted_root: Option<String>,
    pub pkcs12: Option<String>,
    pub pkcs12_password: Option<String>,
}

#[derive(Clone, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct WebsocketCompat {
    pub tls: bool,
}

#[derive(Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Noise {
    pub pattern: String,
    pub local_private_key: Option<String>,
    pub remote_public_key: Option<String>,
}
impl Default for Noise {
    fn default() -> Self {
        Self {
            pattern: PATTERN.into(),
            local_private_key: None,
            remote_public_key: None,
        }
    }
}

#[derive(Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Quic {
    pub certificate: Option<String>,
    pub private_key: Option<String>,
    pub trusted_root: Option<String>,
    pub hostname: String,
    pub keep_alive_interval: u64,
    pub max_idle_timeout: u64,
    pub udp_idle_timeout: u64,
    pub max_connections: usize,
    pub max_streams: u32,
    pub max_udp_flows: usize,
    pub stream_receive_window: u32,
    pub receive_window: u32,
    pub send_window: u32,
    pub socket_buffer: usize,
    #[serde(default = "congestion_cubic")]
    pub congestion: String,
}
fn congestion_cubic() -> String {
    "cubic".into()
}
impl Default for Quic {
    fn default() -> Self {
        Self {
            certificate: None,
            private_key: None,
            trusted_root: None,
            hostname: "cathole".into(),
            keep_alive_interval: 15,
            max_idle_timeout: 60,
            udp_idle_timeout: 60,
            max_connections: 128,
            max_streams: 1024,
            max_udp_flows: 4096,
            stream_receive_window: 16 * 1024 * 1024,
            receive_window: 128 * 1024 * 1024,
            send_window: 128 * 1024 * 1024,
            socket_buffer: 4 * 1024 * 1024,
            congestion: congestion_cubic(),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    #[default]
    Tcp,
    Udp,
}

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Service {
    #[serde(rename = "type", default)]
    pub kind: Kind,
    pub token: Option<String>,
    pub bind_addr: Option<String>,
    pub local_addr: Option<String>,
    #[serde(default)]
    pub prefer_ipv6: bool,
    pub nodelay: Option<bool>,
    pub retry_interval: Option<u64>,
}

impl Side {
    pub fn token<'a>(&'a self, service: &'a Service) -> &'a str {
        service
            .token
            .as_deref()
            .or(self.default_token.as_deref())
            .unwrap_or("")
    }
    pub fn nodelay(&self, service: &Service) -> bool {
        service.nodelay.unwrap_or(self.transport.tcp.nodelay)
    }
    pub fn service_retry(&self, service: &Service) -> u64 {
        service.retry_interval.unwrap_or(self.retry_interval)
    }
    pub fn tcp_tunnel(&self) -> bool {
        matches!(self.transport.kind.as_str(), "tcp" | "tls" | "noise")
    }

    pub fn tls_hostname(&self) -> &str {
        self.transport
            .tls
            .as_ref()
            .and_then(|t| t.hostname.as_deref())
            .filter(|s| !s.is_empty())
            .unwrap_or(self.transport.quic.hostname.as_str())
    }

    pub fn tls_trusted_root(&self) -> Option<&str> {
        self.transport.quic.trusted_root.as_deref().or_else(|| {
            self.transport
                .tls
                .as_ref()
                .and_then(|t| t.trusted_root.as_deref())
        })
    }
}

pub fn key(value: &Option<String>) -> Result<Vec<u8>> {
    let bytes = STANDARD.decode(
        value
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("missing Noise key"))?,
    )?;
    ensure!(
        bytes.len() == 32,
        "Noise X25519 key must decode to 32 bytes"
    );
    Ok(bytes)
}

fn validate_noise(noise: &Noise, server: bool, pinned_tls: bool) -> Result<()> {
    ensure!(
        matches!(
            noise.pattern.as_str(),
            "Noise_NK_25519_ChaChaPoly_BLAKE2s"
                | "Noise_KK_25519_ChaChaPoly_BLAKE2s"
                | "Noise_XX_25519_ChaChaPoly_BLAKE2s"
        ),
        "supported Noise patterns are NK, KK, and XX with 25519/ChaChaPoly/BLAKE2s"
    );
    match noise.pattern.split('_').nth(1) {
        Some("NK") => {
            if server {
                key(&noise.local_private_key)?;
            } else {
                key(&noise.remote_public_key)?;
            }
        }
        Some("KK") => {
            key(&noise.local_private_key)?;
            key(&noise.remote_public_key)?;
        }
        Some("XX") => ensure!(
            pinned_tls,
            "Noise XX requires pinned TLS certificate authentication"
        ),
        _ => unreachable!(),
    }
    Ok(())
}

fn validate_side(s: &mut Side, server: bool) -> Result<()> {
    ensure!(
        matches!(s.transport.kind.as_str(), "noise" | "quic" | "tcp" | "tls"),
        "transport.type must be quic, noise, tcp, or tls (websocket is unsupported)"
    );
    ensure!(
        s.transport.tcp.proxy.is_none(),
        "HTTP/SOCKS TCP proxies cannot carry the tunnel"
    );
    if let Some(tls) = &s.transport.tls {
        ensure!(
            tls.pkcs12.is_none() && tls.pkcs12_password.is_none(),
            "PKCS#12 is unsupported; use DER certificate and private_key in [transport.quic]"
        );
    }
    ensure!(
        matches!(s.transport.quic.congestion.as_str(), "cubic" | "bbr"),
        "transport.quic.congestion must be cubic or bbr"
    );
    let client_tls_root = s.tls_trusted_root().map(str::to_owned);
    let q = &mut s.transport.quic;
    if !server {
        q.max_idle_timeout = s.heartbeat_timeout;
    }
    if server {
        q.keep_alive_interval = s.heartbeat_interval;
    }
    ensure!(
        (65536..=268435456).contains(&q.stream_receive_window)
            && (q.stream_receive_window..=536870912).contains(&q.receive_window)
            && (65536..=536870912).contains(&q.send_window)
            && (65536..=16777216).contains(&q.socket_buffer),
        "flow-control or socket buffer limits out of range"
    );
    ensure!(
        q.keep_alive_interval <= 86400
            && q.max_idle_timeout <= 86400
            && (1..=86400).contains(&q.udp_idle_timeout),
        "timeouts out of range"
    );
    ensure!(
        (1..=4096).contains(&q.max_connections)
            && (1..=8192).contains(&q.max_streams)
            && (1..=65536).contains(&q.max_udp_flows),
        "resource limits out of range"
    );
    ensure!(
        !s.services.is_empty() && s.services.len() <= 256,
        "configure 1..256 services"
    );
    if server {
        ensure!(
            s.bind_addr.is_some() && s.remote_addr.is_none(),
            "server requires bind_addr only"
        );
        ensure!(
            q.certificate.is_some() == q.private_key.is_some(),
            "provide both certificate and private_key, or neither"
        );
        if s.transport.kind == "tls" {
            ensure!(
                q.certificate.is_some() && q.private_key.is_some(),
                "type=tls requires [transport.quic] certificate and private_key"
            );
        }
    } else {
        ensure!(
            s.remote_addr.is_some() && s.bind_addr.is_none(),
            "client requires remote_addr only"
        );
        if s.transport.kind == "tls" {
            ensure!(
                client_tls_root.is_some(),
                "type=tls requires trusted_root on the client"
            );
        }
    }
    let pinned_tls = if server {
        q.certificate.is_some()
    } else {
        client_tls_root.is_some()
    };
    validate_noise(&s.transport.noise, server, pinned_tls)?;
    ensure!(s.retry_interval <= 86400, "retry_interval out of range");
    for (name, service) in &s.services {
        ensure!(
            !name.is_empty() && name.len() <= 128,
            "service name length must be 1..128"
        );
        ensure!(
            !s.token(service).is_empty() && s.token(service).len() <= 512,
            "service {name} requires a token of 1..512 bytes"
        );
        ensure!(
            service.retry_interval.unwrap_or(s.retry_interval) <= 86400,
            "service {name} retry_interval out of range"
        );
        if server && (service.bind_addr.is_none() || service.local_addr.is_some()) {
            bail!("server service {name} requires bind_addr only");
        }
        if !server && (service.local_addr.is_none() || service.bind_addr.is_some()) {
            bail!("client service {name} requires local_addr only");
        }
    }
    Ok(())
}

impl Config {
    pub fn parse(text: &str) -> Result<Self> {
        let mut c: Self = toml::from_str(text).map_err(|e: toml::de::Error| {
            anyhow::anyhow!(
                "invalid TOML or unsupported field at byte range {:?}",
                e.span()
            )
        })?;
        ensure!(
            c.server.is_some() || c.client.is_some(),
            "configure [server], [client], or both"
        );
        if let Some(s) = c.server.as_mut() {
            validate_side(s, true)?;
        }
        if let Some(s) = c.client.as_mut() {
            validate_side(s, false)?;
        }
        Ok(c)
    }
    pub fn read(path: &Path) -> Result<Self> {
        let mut c = Self::parse(&std::fs::read_to_string(path)?)?;
        let dir = path.parent().unwrap_or_else(|| Path::new("."));
        for side in c.server.iter_mut().chain(c.client.iter_mut()) {
            let q = &mut side.transport.quic;
            for p in [&mut q.certificate, &mut q.private_key, &mut q.trusted_root]
                .into_iter()
                .flatten()
            {
                if Path::new(p).is_relative() {
                    *p = dir.join(&*p).to_string_lossy().into_owned();
                }
            }
            if let Some(tls) = side.transport.tls.as_mut()
                && let Some(p) = tls.trusted_root.as_mut()
                && Path::new(p).is_relative()
            {
                *p = dir.join(&*p).to_string_lossy().into_owned();
            }
        }
        Ok(c)
    }
    pub fn select(mut self, server: bool) -> Result<Self> {
        if server {
            ensure!(self.server.is_some(), "--server requires [server]");
            self.client = None;
        } else {
            ensure!(self.client.is_some(), "--client requires [client]");
            self.server = None;
        }
        Ok(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn sample() -> String {
        r#"
[client]
remote_addr = "localhost:2333"
default_token = "test-secret"
heartbeat_timeout = 40
retry_interval = 2
[client.transport]
type = "noise"
[client.transport.noise]
remote_public_key = "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA="
[client.services.minecraft]
local_addr = "localhost:25565"
prefer_ipv6 = true
retry_interval = 3
"#
        .into()
    }
    #[test]
    fn legacy_noise_config_and_defaults() {
        let c = Config::parse(&sample()).unwrap().client.unwrap();
        let s = &c.services["minecraft"];
        assert_eq!(s.kind, Kind::Tcp);
        assert!(c.nodelay(s));
        assert_eq!(c.token(s), "test-secret");
        assert_eq!(c.service_retry(s), 3);
        assert_eq!(c.transport.quic.max_idle_timeout, 40);
        assert!(c.tcp_tunnel());

        let text = sample().replace(
            "[client.transport.noise]",
            "[client.transport.tcp]\nnodelay = false\n[client.transport.noise]",
        );
        let c = Config::parse(&text).unwrap().client.unwrap();
        assert!(!c.nodelay(&c.services["minecraft"]));
    }
    #[test]
    fn accepts_tcp_tunnel_and_rejects_proxy_and_websocket() {
        let tcp = Config::parse(&sample().replace("type = \"noise\"", "type = \"tcp\"")).unwrap();
        assert!(tcp.client.unwrap().tcp_tunnel());
        assert!(
            Config::parse(&sample().replace("type = \"noise\"", "type = \"websocket\"")).is_err()
        );
        assert!(
            Config::parse(&sample().replace(
                "[client.transport.noise]",
                "[client.transport.tcp]\nproxy = \"http://localhost:1\"\n[client.transport.noise]"
            ))
            .is_err()
        );
    }
    #[test]
    fn both_roles_can_share_one_file() {
        let server = r#"
[server]
bind_addr = "localhost:2333"
default_token = "test-secret"
[server.transport]
type = "noise"
[server.transport.noise]
local_private_key = "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA="
[server.services.minecraft]
bind_addr = "localhost:25565"
"#;
        let both = format!("{}\n{}", sample(), server);
        let c = Config::parse(&both).unwrap();
        assert!(c.client.is_some() && c.server.is_some());
        assert!(c.clone().select(true).unwrap().client.is_none());
        assert!(c.select(false).unwrap().server.is_none());
    }
    #[test]
    fn parse_errors_do_not_print_secrets() {
        let text = sample().replace(
            "default_token = \"test-secret\"",
            "default_token = [\"test-secret\"]",
        );
        let error = Config::parse(&text).err().unwrap().to_string();
        assert!(!error.contains("test-secret"));
    }
}
