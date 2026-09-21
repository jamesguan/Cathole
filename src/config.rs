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

#[cfg(test)]
mod tests {
    use super::*;
    fn sample() -> String {
        r#"
[client]
remote_addr = "localhost:2333"
default_token = "test-secret"
[client.transport]
type = "quic"
[client.transport.noise]
remote_public_key = "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA="
[client.transport.quic]
trusted_root = "server.der"
[client.services.minecraft]
local_addr = "localhost:25565"
"#
        .into()
    }
    #[test]
    fn service_defaults_and_tokens() {
        let c = Config::parse(&sample()).unwrap().client.unwrap();
        let s = &c.services["minecraft"];
        assert_eq!(s.kind, Kind::Tcp);
        assert!(s.nodelay);
        assert_eq!(c.token(s), "test-secret");
    }
    #[test]
    fn rejects_unsafe_or_unsupported_config() {
        for text in [
            sample().replace("test-secret", ""),
            sample().replace("quic\"", "tcp\""),
            sample().replace("trusted_root", "trust_any_certificate"),
            sample().replace(
                "[client.services.minecraft]",
                "keep_alive_interval = 18446744073709551615\n[client.services.minecraft]",
            ),
        ] {
            assert!(Config::parse(&text).is_err());
        }
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

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Side {
    pub bind_addr: Option<String>,
    pub remote_addr: Option<String>,
    pub default_token: Option<String>,
    pub transport: Transport,
    pub services: BTreeMap<String, Service>,
}

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Transport {
    #[serde(rename = "type")]
    pub kind: String,
    pub noise: Noise,
    pub quic: Quic,
}

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Noise {
    #[serde(default = "pattern")]
    pub pattern: String,
    pub local_private_key: Option<String>,
    pub remote_public_key: Option<String>,
}
fn pattern() -> String {
    PATTERN.into()
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
            max_streams: 256,
            max_udp_flows: 4096,
            stream_receive_window: 16 * 1024 * 1024,
            receive_window: 32 * 1024 * 1024,
            send_window: 32 * 1024 * 1024,
            socket_buffer: 4 * 1024 * 1024,
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
    #[serde(default = "yes")]
    pub nodelay: bool,
}
fn yes() -> bool {
    true
}

impl Side {
    pub fn token<'a>(&'a self, service: &'a Service) -> &'a str {
        service
            .token
            .as_deref()
            .or(self.default_token.as_deref())
            .unwrap_or("")
    }
}

pub fn key(value: &Option<String>) -> Result<Vec<u8>> {
    let bytes = STANDARD.decode(
        value
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("missing Noise key"))?,
    )?;
    ensure!(bytes.len() == 32, "Noise key must decode to 32 bytes");
    Ok(bytes)
}

impl Config {
    pub fn parse(text: &str) -> Result<Self> {
        // TOML's Display error includes source lines, which can contain secrets.
        let c: Self = toml::from_str(text).map_err(|e: toml::de::Error| {
            anyhow::anyhow!(
                "invalid TOML or unsupported field at byte range {:?}",
                e.span()
            )
        })?;
        ensure!(
            c.server.is_some() != c.client.is_some(),
            "configure exactly one of [server] or [client]"
        );
        let server = c.server.is_some();
        let s = c.server.as_ref().or(c.client.as_ref()).unwrap();
        ensure!(
            s.transport.kind == "quic",
            "transport.type must be quic; no TCP fallback exists"
        );
        ensure!(
            s.transport.noise.pattern == PATTERN,
            "only pinned Noise NK is supported"
        );
        let q = &s.transport.quic;
        ensure!(
            (65536..=268435456).contains(&q.stream_receive_window)
                && (q.stream_receive_window..=536870912).contains(&q.receive_window)
                && (65536..=536870912).contains(&q.send_window)
                && (65536..=16777216).contains(&q.socket_buffer),
            "flow-control or socket buffer limits out of range"
        );
        ensure!(
            (1..=86400).contains(&q.keep_alive_interval)
                && q.max_idle_timeout > q.keep_alive_interval * 3,
            "idle timeout must exceed three keepalive intervals"
        );
        ensure!(
            (1..=86400).contains(&q.max_idle_timeout) && (1..=86400).contains(&q.udp_idle_timeout),
            "timeouts must be 1..86400 seconds"
        );
        ensure!(
            (1..=4096).contains(&q.max_connections)
                && (1..=4096).contains(&q.max_streams)
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
                q.certificate.is_some() && q.private_key.is_some(),
                "server requires QUIC certificate and private_key DER files"
            );
            key(&s.transport.noise.local_private_key)?;
        } else {
            ensure!(
                s.remote_addr.is_some() && s.bind_addr.is_none(),
                "client requires remote_addr only"
            );
            ensure!(
                q.trusted_root.is_some(),
                "client requires QUIC trusted_root DER file"
            );
            key(&s.transport.noise.remote_public_key)?;
        }
        for (name, service) in &s.services {
            ensure!(
                !name.is_empty() && name.len() <= 128,
                "service name length must be 1..128"
            );
            ensure!(
                !s.token(service).is_empty() && s.token(service).len() <= 512,
                "service {name} requires a token of 1..512 bytes"
            );
            if server && (service.bind_addr.is_none() || service.local_addr.is_some()) {
                bail!("server service {name} requires bind_addr only");
            }
            if !server && (service.local_addr.is_none() || service.bind_addr.is_some()) {
                bail!("client service {name} requires local_addr only");
            }
        }
        Ok(c)
    }
    pub fn read(path: &Path) -> Result<Self> {
        let mut c = Self::parse(&std::fs::read_to_string(path)?)?;
        let dir = path.parent().unwrap_or(Path::new("."));
        let q = &mut c
            .server
            .as_mut()
            .or(c.client.as_mut())
            .unwrap()
            .transport
            .quic;
        for p in [&mut q.certificate, &mut q.private_key, &mut q.trusted_root]
            .into_iter()
            .flatten()
        {
            if Path::new(p).is_relative() {
                *p = dir.join(&p).to_string_lossy().into_owned();
            }
        }
        Ok(c)
    }
}
