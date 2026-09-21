use super::*;
use crate::{config::Service, transport};
use anyhow::Context;
use std::{
    collections::{HashMap, HashSet},
    net::SocketAddr,
    sync::Mutex,
    time::Instant,
};
use subtle::ConstantTimeEq;
use tokio::{
    net::{TcpListener, UdpSocket},
    sync::Semaphore,
    task::JoinSet,
};

enum Bound {
    Tcp(TcpListener),
    Udp(Arc<UdpSocket>),
}
struct Flow {
    service: u16,
    peer: SocketAddr,
    socket: Arc<UdpSocket>,
    touched: Instant,
}
struct Flows {
    next: u64,
    by_id: HashMap<u64, Flow>,
    by_peer: HashMap<(u16, SocketAddr), u64>,
}
impl Flows {
    fn expire(&mut self, idle: Duration) {
        self.by_id.retain(|_, f| f.touched.elapsed() < idle);
        self.by_peer.retain(|_, id| self.by_id.contains_key(id));
    }
}

pub async fn run(cfg: Arc<Side>, mut shutdown: tokio::sync::watch::Receiver<bool>) -> Result<()> {
    let endpoint = transport::server(
        resolve(cfg.bind_addr.as_ref().unwrap(), cfg.prefer_ipv6).await?,
        &cfg.transport.quic,
    )?;
    tracing::info!(address = %endpoint.local_addr()?, "QUIC server listening");
    let slots = Arc::new(Semaphore::new(cfg.transport.quic.max_connections));
    let mut sessions = JoinSet::new();
    loop {
        tokio::select! {
            _ = shutdown.changed() => break,
            Some(result) = sessions.join_next() => { if let Err(e) = result { tracing::warn!(error = %e, "session task failed"); } },
            incoming = endpoint.accept() => {
                let Some(incoming) = incoming else { break; };
                // Stateless QUIC Retry validates the source before allocating a session.
                if !incoming.remote_address_validated() { let _ = incoming.retry(); continue; }
                let Ok(permit) = slots.clone().try_acquire_owned() else { incoming.refuse(); continue; };
                let cfg = cfg.clone();
                sessions.spawn(async move {
                    let _permit = permit;
                    let result = async {
                        let conn = tokio::time::timeout(SETUP, incoming).await??;
                        let _close = CloseOnDrop(conn.clone());
                        let result = tokio::time::timeout(SESSION_LIFETIME, session(conn, cfg)).await;
                        result?
                    }.await;
                    if let Err(e) = result { tracing::warn!(error = %e, "client session ended"); }
                });
            }
        }
    }
    endpoint.close(0u32.into(), b"server restarting");
    sessions.abort_all();
    while sessions.join_next().await.is_some() {}
    endpoint.wait_idle().await;
    Ok(())
}

async fn session(conn: Connection, cfg: Arc<Side>) -> Result<()> {
    let (session, bound) = tokio::time::timeout(SETUP, async {
        let (mut send, mut recv) = conn.accept_bi().await?;
        let (mut data_crypto, payload) = crypto::receive_datagram(
            &conn,
            &cfg.transport.noise,
            &mut send,
            &mut recv,
            b"registration",
        )
        .await?;
        let reg: Registration = serde_json::from_slice(&payload)?;
        ensure!(
            reg.version == 1 && !reg.services.is_empty() && reg.services.len() <= 256,
            "invalid registration"
        );
        let mut names = HashSet::new();
        let mut definitions = Vec::new();
        // Validate the whole registration before opening any public service port.
        for offer in reg.services {
            ensure!(names.insert(offer.name.clone()), "duplicate service");
            let service = cfg
                .services
                .get(&offer.name)
                .context("service authentication failed")?;
            ensure!(
                offer.kind == service.kind
                    && bool::from(cfg.token(service).as_bytes().ct_eq(offer.token.as_bytes())),
                "service authentication failed"
            );
            definitions.push((offer.name, service.clone()));
        }
        let mut bound = Vec::new();
        for (name, service) in definitions {
            let addr = service.bind_addr.as_ref().unwrap();
            let listener = match service.kind {
                Kind::Tcp => Bound::Tcp(TcpListener::bind(addr).await?),
                Kind::Udp => Bound::Udp(Arc::new(UdpSocket::bind(addr).await?)),
            };
            bound.push((name, service, listener));
        }
        crypto::write_frame(&mut send, &data_crypto.seal(b"ok")?).await?;
        send.finish()?;
        // Require orderly completion of control request, not abandoned auth streams.
        ensure!(
            crypto::read_frame(&mut recv).await?.is_none(),
            "unexpected control data"
        );
        let data = Arc::new(datagram::Sender::new(data_crypto));
        Ok::<_, anyhow::Error>((
            Arc::new(Session {
                conn: conn.clone(),
                data,
            }),
            bound,
        ))
    })
    .await??;
    tracing::info!(
        services = bound.len(),
        "client authenticated and services registered"
    );
    let flows = Arc::new(Mutex::new(Flows {
        next: 0,
        by_id: HashMap::new(),
        by_peer: HashMap::new(),
    }));
    let stream_slots = Arc::new(Semaphore::new(cfg.transport.quic.max_streams as usize));
    let mut tasks = JoinSet::new();
    for (id, (name, service, listener)) in bound.into_iter().enumerate() {
        let session = session.clone();
        let cfg = cfg.clone();
        let flows = flows.clone();
        let slots = stream_slots.clone();
        tasks.spawn(async move {
            tracing::info!(service = %name, "service ready");
            match listener {
                Bound::Tcp(listener) => {
                    tcp_listener(listener, service, id as u16, session, cfg, slots).await
                }
                Bound::Udp(socket) => udp_listener(socket, id as u16, session, cfg, flows).await,
            }
        });
    }
    let mut assembly = datagram::Reassembler::default();
    let mut tick = tokio::time::interval(Duration::from_secs(1));
    loop {
        tokio::select! {
            reason = conn.closed() => anyhow::bail!("connection closed: {reason}"),
            result = conn.accept_bi() => { let _ = result?; anyhow::bail!("unexpected client stream"); },
            Some(result) = tasks.join_next() => { result??; anyhow::bail!("service listener stopped"); },
            _ = tick.tick() => {
                flows.lock().unwrap().expire(Duration::from_secs(cfg.transport.quic.udp_idle_timeout));
                assembly.expire(Instant::now());
            },
            packet = read_packet(&session, &mut assembly) => {
                let packet = packet?;
                let destination = {
                    let mut flows = flows.lock().unwrap();
                    flows.by_id.get_mut(&packet.flow).filter(|f| f.service == packet.service).map(|f| {
                        f.touched = Instant::now(); (f.socket.clone(), f.peer)
                    })
                };
                if let Some((socket, peer)) = destination { let _ = socket.send_to(&packet.payload, peer).await; }
            }
        }
    }
}

async fn tcp_listener(
    listener: TcpListener,
    service: Service,
    id: u16,
    session: Arc<Session>,
    cfg: Arc<Side>,
    slots: Arc<Semaphore>,
) -> Result<()> {
    let mut tasks = JoinSet::new();
    loop {
        tokio::select! {
            Some(_) = tasks.join_next() => (),
            accepted = listener.accept() => {
                let (tcp, _) = accepted?;
                let Ok(permit) = slots.clone().try_acquire_owned() else { continue; };
                tcp.set_nodelay(cfg.nodelay(&service))?;
                let session = session.clone(); let cfg = cfg.clone();
                tasks.spawn(async move {
                    let _permit = permit;
                    if let Err(e) = tcp_server(session, cfg, id, tcp).await { tracing::debug!(error = %e, "TCP forwarding ended"); }
                });
            }
        }
    }
}

async fn udp_listener(
    socket: Arc<UdpSocket>,
    id: u16,
    session: Arc<Session>,
    cfg: Arc<Side>,
    flows: Arc<Mutex<Flows>>,
) -> Result<()> {
    let mut buf = vec![0; 65536];
    loop {
        let (n, peer) = match socket.recv_from(&mut buf).await {
            Ok(packet) => packet,
            Err(e)
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::ConnectionReset | std::io::ErrorKind::ConnectionRefused
                ) =>
            {
                continue;
            }
            Err(e) => return Err(e.into()),
        };
        if n > datagram::MAX_PAYLOAD {
            continue;
        }
        let flow = {
            let mut f = flows.lock().unwrap();
            if let Some(flow) = f.by_peer.get(&(id, peer)).copied() {
                f.by_id.get_mut(&flow).unwrap().touched = Instant::now();
                Some(flow)
            } else if f.by_id.len() < cfg.transport.quic.max_udp_flows {
                let flow = f.next;
                f.next = f.next.checked_add(1).context("flow IDs exhausted")?;
                f.by_peer.insert((id, peer), flow);
                f.by_id.insert(
                    flow,
                    Flow {
                        service: id,
                        peer,
                        socket: socket.clone(),
                        touched: Instant::now(),
                    },
                );
                Some(flow)
            } else {
                None
            }
        };
        if let Some(flow) = flow {
            session.data.send(&session.conn, id, flow, &buf[..n])?;
        }
    }
}
