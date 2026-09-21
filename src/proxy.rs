mod client;
mod server;
mod tcp_tunnel;
pub use client::run as run_client;
pub use server::run as run_server;
pub use tcp_tunnel::{run_client as run_tcp_client, run_server as run_tcp_server};

use crate::{
    config::{Kind, Side},
    crypto, datagram,
};
use anyhow::{Result, ensure};
use quinn::{Connection, RecvStream, SendStream};
use serde::{Deserialize, Serialize};
use std::{sync::Arc, time::Duration};
use tokio::net::{TcpStream, lookup_host};

pub(super) const SETUP: Duration = Duration::from_secs(10);
pub const SESSION_LIFETIME: Duration = Duration::from_secs(86400);
pub const REGISTRATION_VERSION: u8 = 2;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Registration {
    pub version: u8,
    pub services: Vec<Offer>,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Offer {
    pub name: String,
    pub kind: Kind,
    pub token: String,
}

struct Session {
    conn: Connection,
    data: Arc<datagram::Sender>,
}
struct CloseOnDrop(Connection);
impl Drop for CloseOnDrop {
    fn drop(&mut self) {
        self.0.close(0u32.into(), b"session ended");
    }
}

pub async fn resolve(addr: &str, prefer_ipv6: bool) -> Result<std::net::SocketAddr> {
    let addresses: Vec<_> = tokio::time::timeout(SETUP, lookup_host(addr))
        .await??
        .collect();
    addresses
        .iter()
        .copied()
        .find(|a| a.is_ipv6() == prefer_ipv6)
        .or_else(|| addresses.first().copied())
        .ok_or_else(|| anyhow::anyhow!("address resolved to no endpoints"))
}

pub(super) async fn resolve_local(addr: &str, prefer_ipv6: bool) -> Result<std::net::SocketAddr> {
    let mut target = resolve(addr, prefer_ipv6).await?;
    if target.ip().is_unspecified() {
        target.set_ip(if target.is_ipv4() {
            std::net::Ipv4Addr::LOCALHOST.into()
        } else {
            std::net::Ipv6Addr::LOCALHOST.into()
        });
    }
    Ok(target)
}

async fn tcp_server(
    session: Arc<Session>,
    _cfg: Arc<Side>,
    service: u16,
    tcp: TcpStream,
) -> Result<()> {
    let (mut send, recv) = session.conn.open_bi().await?;
    send.write_all(&service.to_be_bytes()).await?;
    crypto::relay_raw(tcp, send, recv).await
}

async fn tcp_client(cfg: Arc<Side>, send: SendStream, mut recv: RecvStream) -> Result<()> {
    let mut id = [0u8; 2];
    recv.read_exact(&mut id).await?;
    let service_id = u16::from_be_bytes(id);
    let service = cfg
        .services
        .values()
        .nth(service_id as usize)
        .ok_or_else(|| anyhow::anyhow!("unknown service"))?;
    ensure!(service.kind == Kind::Tcp, "wrong service type");
    let target = resolve_local(
        service.local_addr.as_ref().unwrap(),
        service.prefer_ipv6 || cfg.prefer_ipv6,
    )
    .await?;
    let retry = cfg.service_retry(service);
    let tcp = loop {
        match TcpStream::connect(target).await {
            Ok(tcp) => break tcp,
            Err(e) if retry > 0 => {
                tracing::debug!(error = %e, "local TCP connect failed; retrying");
                tokio::time::sleep(Duration::from_secs(retry)).await;
            }
            Err(e) => return Err(e.into()),
        }
    };
    tcp.set_nodelay(cfg.nodelay(service))?;
    crypto::relay_raw(tcp, send, recv).await
}

async fn read_packet(
    session: &Session,
    assembly: &mut datagram::Reassembler,
) -> Result<datagram::Packet> {
    loop {
        let bytes = session.conn.read_datagram().await?;
        match assembly.push(&bytes, std::time::Instant::now()) {
            Ok(Some(packet)) => return Ok(packet),
            Ok(None) => (),
            Err(e) => tracing::debug!(error = %e, "discarded invalid datagram"),
        }
    }
}
