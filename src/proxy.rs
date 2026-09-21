mod client;
mod server;
pub use client::run as run_client;
pub use server::run as run_server;

use crate::{
    config::{Kind, Side},
    crypto, datagram,
};
use anyhow::{Result, ensure};
use quinn::{Connection, RecvStream, SendStream};
use serde::{Deserialize, Serialize};
use std::{sync::Arc, time::Duration};
use tokio::net::{TcpStream, lookup_host};

pub const SETUP: Duration = Duration::from_secs(10);
pub const SESSION_LIFETIME: Duration = Duration::from_secs(86400);

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Registration {
    version: u8,
    services: Vec<Offer>,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Offer {
    name: String,
    kind: Kind,
    token: String,
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

async fn resolve_local(addr: &str, prefer_ipv6: bool) -> Result<std::net::SocketAddr> {
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

fn context(service: u16, send: &SendStream) -> Vec<u8> {
    let mut c = b"tcp".to_vec();
    c.extend_from_slice(&service.to_be_bytes());
    c.extend_from_slice(&u64::from(send.id()).to_be_bytes());
    c
}

async fn tcp_server(
    session: Arc<Session>,
    cfg: Arc<Side>,
    service: u16,
    tcp: TcpStream,
) -> Result<()> {
    let (send, recv, hs) = tokio::time::timeout(SETUP, async {
        let (mut send, mut recv) = session.conn.open_bi().await?;
        crypto::write_frame(&mut send, &service.to_be_bytes()).await?;
        let ctx = context(service, &send);
        let (mut state, payload) = crypto::receive_stream(
            &session.conn,
            &cfg.transport.noise,
            &mut send,
            &mut recv,
            &ctx,
        )
        .await?;
        ensure!(payload.is_empty(), "unexpected TCP handshake payload");
        crypto::write_transport(&mut state, &mut send, b"ok").await?;
        Ok::<_, anyhow::Error>((send, recv, state))
    })
    .await??;
    crypto::bridge(tcp, send, recv, hs).await
}

async fn tcp_client(
    conn: Connection,
    cfg: Arc<Side>,
    mut send: SendStream,
    mut recv: RecvStream,
) -> Result<()> {
    let (tcp, hs) = tokio::time::timeout(SETUP, async {
        let header = crypto::read_frame(&mut recv)
            .await?
            .ok_or_else(|| anyhow::anyhow!("missing TCP header"))?;
        ensure!(header.len() == 2, "invalid TCP header");
        let id = u16::from_be_bytes(header.try_into().unwrap());
        let service = cfg
            .services
            .values()
            .nth(id as usize)
            .ok_or_else(|| anyhow::anyhow!("unknown service"))?;
        ensure!(service.kind == Kind::Tcp, "wrong service type");
        let ctx = context(id, &send);
        let (state, reply) =
            crypto::initiate_stream(&conn, &cfg.transport.noise, &mut send, &mut recv, &ctx, b"")
                .await?;
        ensure!(reply == b"ok", "TCP handshake rejected");
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
        Ok::<_, anyhow::Error>((tcp, state))
    })
    .await??;
    crypto::bridge(tcp, send, recv, hs).await
}

async fn read_packet(
    session: &Session,
    assembly: &mut datagram::Reassembler,
) -> Result<datagram::Packet> {
    loop {
        let bytes = session.conn.read_datagram().await?;
        let decoded = session.data.crypto.lock().unwrap().open(&bytes);
        match decoded.and_then(|p| assembly.push(&p, std::time::Instant::now())) {
            Ok(Some(packet)) => return Ok(packet),
            Ok(None) => (),
            Err(e) => tracing::debug!(error = %e, "discarded invalid datagram"),
        }
    }
}
