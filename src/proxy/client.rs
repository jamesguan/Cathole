use super::*;
use crate::transport;
use std::{collections::HashMap, net::SocketAddr};
use tokio::{
    net::UdpSocket,
    sync::{OwnedSemaphorePermit, Semaphore, mpsc},
    task::JoinSet,
};

type QueuedPacket = (Vec<u8>, OwnedSemaphorePermit);

pub async fn run(cfg: Arc<Side>) -> Result<()> {
    let mut delay = 1u64;
    loop {
        let began = std::time::Instant::now();
        if let Err(e) = connect(cfg.clone()).await {
            tracing::warn!(error = %e, "tunnel disconnected; reconnecting");
        }
        if began.elapsed() > Duration::from_secs(60) {
            delay = 1;
        }
        // Cap exponential backoff and add jitter; no retry storm or unbounded delay.
        let wait = Duration::from_millis(delay * 1000 + rand::random_range(0..=1000));
        tokio::time::sleep(wait).await;
        delay = (delay * 2).min(30);
    }
}

async fn connect(cfg: Arc<Side>) -> Result<()> {
    let remote = resolve(cfg.remote_addr.as_ref().unwrap()).await?;
    let bind = if remote.is_ipv4() {
        "0.0.0.0:0"
    } else {
        "[::]:0"
    };
    let endpoint = transport::client(bind.parse()?, &cfg.transport.quic)?;
    let conn = tokio::time::timeout(
        SETUP,
        endpoint.connect(remote, &cfg.transport.quic.hostname)?,
    )
    .await??;
    let close = CloseOnDrop(conn.clone());
    let result = tokio::time::timeout(SESSION_LIFETIME, session(conn, cfg)).await;
    drop(close);
    endpoint.close(0u32.into(), b"reconnect");
    let _ = tokio::time::timeout(Duration::from_secs(2), endpoint.wait_idle()).await;
    result?
}

async fn session(conn: Connection, cfg: Arc<Side>) -> Result<()> {
    let session = tokio::time::timeout(SETUP, async {
        let reg = Registration {
            version: 1,
            services: cfg
                .services
                .iter()
                .map(|(name, s)| Offer {
                    name: name.clone(),
                    kind: s.kind,
                    token: cfg.token(s).into(),
                })
                .collect(),
        };
        let (mut send, mut recv) = conn.open_bi().await?;
        let (hs, reply) = crypto::initiate(
            &conn,
            &cfg.transport.noise,
            &mut send,
            &mut recv,
            b"registration",
            &serde_json::to_vec(&reg)?,
        )
        .await?;
        ensure!(reply == b"ok", "service registration rejected");
        send.finish()?;
        ensure!(
            crypto::read_frame(&mut recv).await?.is_none(),
            "unexpected control data"
        );
        Ok::<_, anyhow::Error>(Arc::new(Session {
            conn: conn.clone(),
            data: Arc::new(datagram::Sender::new(crypto::DatagramCrypto::new(
                hs.into_stateless_transport_mode()?,
            ))),
        }))
    })
    .await??;
    tracing::info!(services = cfg.services.len(), "tunnel ready");
    let mut destinations = HashMap::new();
    for (id, s) in cfg.services.values().enumerate() {
        if s.kind == Kind::Udp {
            destinations.insert(id as u16, resolve(s.local_addr.as_ref().unwrap()).await?);
        }
    }
    let slots = Arc::new(Semaphore::new(cfg.transport.quic.max_streams as usize));
    let mut streams = JoinSet::new();
    let mut workers = JoinSet::new();
    let mut flows: HashMap<(u16, u64), mpsc::Sender<QueuedPacket>> = HashMap::new();
    let queue_bytes = Arc::new(Semaphore::new(8 * 1024 * 1024));
    let mut assembly = datagram::Reassembler::default();
    let mut tick = tokio::time::interval(Duration::from_secs(1));
    loop {
        tokio::select! {
            reason = conn.closed() => anyhow::bail!("connection closed: {reason}"),
            Some(_) = streams.join_next() => (),
            Some(done) = workers.join_next() => { let key = done?; flows.remove(&key); },
            _ = tick.tick() => assembly.expire(std::time::Instant::now()),
            incoming = conn.accept_bi() => {
                let (send, recv) = incoming?;
                let Ok(permit) = slots.clone().try_acquire_owned() else { continue; };
                let conn = conn.clone(); let cfg = cfg.clone();
                streams.spawn(async move {
                    let _permit = permit;
                    if let Err(e) = tcp_client(conn, cfg, send, recv).await { tracing::debug!(error = %e, "TCP forwarding ended"); }
                });
            },
            packet = read_packet(&session, &mut assembly) => {
                let p = packet?;
                let Some(destination) = destinations.get(&p.service).copied() else { continue; };
                let key = (p.service, p.flow);
                if !flows.contains_key(&key) {
                    if flows.len() >= cfg.transport.quic.max_udp_flows { continue; }
                    let (tx, rx) = mpsc::channel(8);
                    flows.insert(key, tx);
                    let session = session.clone(); let idle = cfg.transport.quic.udp_idle_timeout;
                    workers.spawn(async move {
                        if let Err(e) = udp_flow(session, key, destination, rx, idle).await { tracing::debug!(error = %e, "UDP mapping ended"); }
                        key
                    });
                }
                // Drop on a full queue: UDP must not block control/TCP or grow memory.
                if let Ok(budget) = queue_bytes.clone().try_acquire_many_owned(p.payload.len().max(1) as u32) {
                    let _ = flows[&key].try_send((p.payload, budget));
                }
            }
        }
    }
}

async fn udp_flow(
    session: Arc<Session>,
    key: (u16, u64),
    destination: SocketAddr,
    mut rx: mpsc::Receiver<QueuedPacket>,
    idle: u64,
) -> Result<()> {
    let bind = if destination.is_ipv4() {
        "0.0.0.0:0"
    } else {
        "[::]:0"
    };
    let socket = UdpSocket::bind(bind).await?;
    socket.connect(destination).await?;
    let mut buf = vec![0; 65536];
    loop {
        let idle = tokio::time::sleep(Duration::from_secs(idle));
        tokio::select! {
            _ = idle => return Ok(()),
            p = rx.recv() => match p { Some((p, _budget)) => { socket.send(&p).await?; }, None => return Ok(()) },
            n = socket.recv(&mut buf) => {
                let n = n?;
                if n <= datagram::MAX_PAYLOAD { session.data.send(&session.conn, key.0, key.1, &buf[..n])?; }
            }
        }
    }
}
