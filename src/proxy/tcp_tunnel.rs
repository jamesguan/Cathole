use super::{
    Offer, REGISTRATION_VERSION, Registration, SESSION_LIFETIME, SETUP, resolve, resolve_local,
};
use crate::{
    config::{Kind, Service, Side},
    crypto, datagram, transport,
};
use anyhow::{Context, Result, ensure};
use bytes::Bytes;
use rustls::pki_types::ServerName;
use std::{
    collections::{HashMap, HashSet},
    net::SocketAddr,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU32, Ordering},
    },
    time::{Duration, Instant},
};
use subtle::ConstantTimeEq;
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, ReadHalf, WriteHalf},
    net::{TcpListener, TcpStream, UdpSocket},
    sync::{OwnedSemaphorePermit, Semaphore, mpsc},
    task::JoinSet,
};
use tokio_rustls::{TlsAcceptor, TlsConnector};

const OPEN: u8 = 1;
const DATA: u8 = 2;
const FIN: u8 = 3;
const UDP: u8 = 4;
const HEARTBEAT: u8 = 5;
const CHUNK: usize = 32 * 1024;

enum Bound {
    Tcp(TcpListener),
    Udp(Arc<UdpSocket>),
}
enum Incoming {
    Tcp {
        service: u16,
        stream: TcpStream,
    },
    Udp {
        service: u16,
        peer: SocketAddr,
        payload: Bytes,
        socket: Arc<UdpSocket>,
    },
}
enum StreamMsg {
    Data(Bytes),
    Fin,
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

struct MuxOut<W> {
    writer: W,
    aead: crypto::AeadSend,
    cipher: Vec<u8>,
    framed: Vec<u8>,
}
impl<W: AsyncWrite + Unpin> MuxOut<W> {
    fn new(writer: W, aead: crypto::AeadSend) -> Self {
        Self {
            writer,
            aead,
            cipher: crate::buf::take_vec(64),
            framed: crate::buf::take_vec(64),
        }
    }
    async fn send(&mut self, plain: &[u8]) -> Result<()> {
        self.aead.seal(plain, &mut self.cipher)?;
        ensure!(self.cipher.len() <= crypto::MAX_FRAME, "frame too large");
        self.framed.clear();
        self.framed
            .extend_from_slice(&(self.cipher.len() as u32).to_be_bytes());
        self.framed.extend_from_slice(&self.cipher);
        self.writer.write_all(&self.framed).await?;
        Ok(())
    }
}

async fn mux_write<W: AsyncWrite + Unpin>(
    writer: W,
    aead: crypto::AeadSend,
    mut rx: mpsc::Receiver<Bytes>,
) -> Result<()> {
    let mut out = MuxOut::new(writer, aead);
    while let Some(plain) = rx.recv().await {
        out.send(&plain).await?;
    }
    Ok(())
}

async fn mux_read<R: AsyncRead + Unpin>(
    mut reader: R,
    mut aead: crypto::AeadRecv,
    tx: mpsc::Sender<Bytes>,
) -> Result<()> {
    let mut cipher = crate::buf::take_vec(crypto::MAX_FRAME);
    let mut plain = crate::buf::take_vec(crypto::MAX_FRAME);
    loop {
        let Some(()) = crypto::read_frame_into(&mut reader, &mut cipher).await? else {
            return Ok(());
        };
        aead.open(&cipher, &mut plain)?;
        if tx.send(crate::buf::copy_bytes(&plain)).await.is_err() {
            return Ok(());
        }
    }
}

async fn send_frame(out: &mpsc::Sender<Bytes>, plain: &[u8]) -> Result<()> {
    out.send(crate::buf::copy_bytes(plain))
        .await
        .map_err(|_| anyhow::anyhow!("mux writer closed"))
}

fn configure_tunnel_socket(stream: &TcpStream, tcp: &crate::config::TcpCompat) -> Result<()> {
    stream.set_nodelay(tcp.nodelay)?;
    if tcp.keepalive_secs > 0 {
        let mut ka =
            socket2::TcpKeepalive::new().with_time(Duration::from_secs(tcp.keepalive_secs));
        if tcp.keepalive_interval > 0 {
            ka = ka.with_interval(Duration::from_secs(tcp.keepalive_interval));
        }
        socket2::SockRef::from(stream).set_tcp_keepalive(&ka)?;
    }
    Ok(())
}

pub async fn run_server(
    cfg: Arc<Side>,
    mut shutdown: tokio::sync::watch::Receiver<bool>,
) -> Result<()> {
    let listener =
        TcpListener::bind(resolve(cfg.bind_addr.as_ref().unwrap(), cfg.prefer_ipv6).await?).await?;
    tracing::info!(
        address = %listener.local_addr()?,
        transport = %cfg.transport.kind,
        "TCP server listening"
    );
    let slots = Arc::new(Semaphore::new(cfg.transport.quic.max_connections));
    let mut sessions = JoinSet::new();
    loop {
        tokio::select! {
            _ = shutdown.changed() => break,
            Some(result) = sessions.join_next() => {
                if let Err(e) = result { tracing::warn!(error = %e, "session task failed"); }
            },
            accepted = listener.accept() => {
                let (tcp, _) = accepted?;
                let Ok(permit) = slots.clone().try_acquire_owned() else { continue; };
                let cfg = cfg.clone();
                sessions.spawn(async move {
                    let _permit = permit;
                    if let Err(e) = async {
                        tokio::time::timeout(SESSION_LIFETIME, accept_session(cfg, tcp)).await??;
                        Ok::<_, anyhow::Error>(())
                    }
                    .await
                    {
                        tracing::warn!(error = %e, "client session ended");
                    }
                });
            }
        }
    }
    sessions.abort_all();
    while sessions.join_next().await.is_some() {}
    Ok(())
}

async fn accept_session(cfg: Arc<Side>, tcp: TcpStream) -> Result<()> {
    configure_tunnel_socket(&tcp, &cfg.transport.tcp)?;
    if cfg.transport.kind == "tls" {
        let tls = TlsAcceptor::from(Arc::new(transport::rustls_server(&cfg.transport.quic)?));
        server_session(cfg, tls.accept(tcp).await?).await
    } else {
        server_session(cfg, tcp).await
    }
}

pub async fn run_client(cfg: Arc<Side>) -> Result<()> {
    let mut delay = 1u64;
    loop {
        let began = Instant::now();
        if let Err(e) = connect(cfg.clone()).await {
            tracing::warn!(error = %e, "tunnel disconnected; reconnecting");
        }
        if began.elapsed() > Duration::from_secs(60) {
            delay = 1;
        }
        let base = cfg.retry_interval;
        let wait = if base == 0 {
            Duration::from_millis(100)
        } else {
            Duration::from_millis(delay.max(base) * 1000 + rand::random_range(0..=1000))
        };
        tokio::time::sleep(wait).await;
        delay = (delay.max(base) * 2).min(30.max(base));
    }
}

async fn connect(cfg: Arc<Side>) -> Result<()> {
    let remote = resolve(cfg.remote_addr.as_ref().unwrap(), cfg.prefer_ipv6).await?;
    let tcp = tokio::time::timeout(SETUP, TcpStream::connect(remote)).await??;
    configure_tunnel_socket(&tcp, &cfg.transport.tcp)?;
    if cfg.transport.kind == "tls" {
        let root = cfg
            .tls_trusted_root()
            .context("type=tls requires trusted_root")?;
        let tls = TlsConnector::from(Arc::new(transport::rustls_client(root)?));
        let name = ServerName::try_from(cfg.tls_hostname().to_owned())
            .map_err(|_| anyhow::anyhow!("invalid TLS hostname"))?;
        client_session(cfg, tls.connect(name, tcp).await?).await
    } else {
        client_session(cfg, tcp).await
    }
}

async fn server_session<S>(cfg: Arc<Side>, mut stream: S) -> Result<()>
where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    let (bound, aead) = tokio::time::timeout(SETUP, async {
        let (mut aead, payload) =
            crypto::receive_tcp_session(&mut stream, &cfg.transport.noise).await?;
        let reg: Registration = serde_json::from_slice(&payload)?;
        ensure!(
            reg.version == REGISTRATION_VERSION
                && !reg.services.is_empty()
                && reg.services.len() <= 256,
            "invalid registration"
        );
        let mut names = HashSet::new();
        let mut definitions = Vec::new();
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
        crypto::confirm_session(&mut aead, &mut stream).await?;
        Ok::<_, anyhow::Error>((bound, aead))
    })
    .await??;
    tracing::info!(
        services = bound.len(),
        "client authenticated and services registered"
    );
    let (reader, writer) = tokio::io::split(stream);
    let (tx, rx) = aead.split();
    let (events, rx_events) = mpsc::channel(8192);
    let slots = Arc::new(Semaphore::new(cfg.transport.quic.max_streams as usize));
    let mut tasks = JoinSet::new();
    for (id, (name, service, listener)) in bound.into_iter().enumerate() {
        let events = events.clone();
        let cfg = cfg.clone();
        tasks.spawn(async move {
            tracing::info!(service = %name, "service ready");
            match listener {
                Bound::Tcp(listener) => tcp_listen(listener, service, id as u16, cfg, events).await,
                Bound::Udp(socket) => udp_listen(socket, id as u16, events).await,
            }
        });
    }
    drop(events);
    drive(cfg, reader, writer, tx, rx, rx_events, slots, tasks, true).await
}

async fn client_session<S>(cfg: Arc<Side>, mut stream: S) -> Result<()>
where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    let aead = tokio::time::timeout(SETUP, async {
        let reg = Registration {
            version: REGISTRATION_VERSION,
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
        crypto::initiate_tcp_session(
            &mut stream,
            &cfg.transport.noise,
            &serde_json::to_vec(&reg)?,
        )
        .await
    })
    .await??;
    tracing::info!(services = cfg.services.len(), "tunnel ready");
    let (reader, writer) = tokio::io::split(stream);
    let (tx, rx) = aead.split();
    let (_unused, rx_events) = mpsc::channel(1);
    let slots = Arc::new(Semaphore::new(cfg.transport.quic.max_streams as usize));
    drive(
        cfg,
        reader,
        writer,
        tx,
        rx,
        rx_events,
        slots,
        JoinSet::new(),
        false,
    )
    .await
}

async fn tcp_listen(
    listener: TcpListener,
    service: Service,
    id: u16,
    cfg: Arc<Side>,
    events: mpsc::Sender<Incoming>,
) -> Result<()> {
    loop {
        let (tcp, _) = listener.accept().await?;
        tcp.set_nodelay(cfg.nodelay(&service))?;
        if events
            .send(Incoming::Tcp {
                service: id,
                stream: tcp,
            })
            .await
            .is_err()
        {
            return Ok(());
        }
    }
}

async fn udp_listen(socket: Arc<UdpSocket>, id: u16, events: mpsc::Sender<Incoming>) -> Result<()> {
    let mut buf = crate::buf::take_vec(65536);
    buf.resize(65536, 0);
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
        if events
            .send(Incoming::Udp {
                service: id,
                peer,
                payload: crate::buf::copy_bytes(&buf[..n]),
                socket: socket.clone(),
            })
            .await
            .is_err()
        {
            return Ok(());
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn drive<S>(
    cfg: Arc<Side>,
    reader: ReadHalf<S>,
    writer: WriteHalf<S>,
    send: crypto::AeadSend,
    recv: crypto::AeadRecv,
    mut incoming: mpsc::Receiver<Incoming>,
    slots: Arc<Semaphore>,
    mut listeners: JoinSet<Result<()>>,
    server: bool,
) -> Result<()>
where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    let mut destinations = HashMap::new();
    if !server {
        for (id, s) in cfg.services.values().enumerate() {
            if s.kind == Kind::Udp {
                destinations.insert(
                    id as u16,
                    resolve_local(
                        s.local_addr.as_ref().unwrap(),
                        s.prefer_ipv6 || cfg.prefer_ipv6,
                    )
                    .await?,
                );
            }
        }
    }
    let flows = Arc::new(Mutex::new(Flows {
        next: 0,
        by_id: HashMap::new(),
        by_peer: HashMap::new(),
    }));
    let mut streams: HashMap<u32, mpsc::Sender<StreamMsg>> = HashMap::new();
    let mut forwards: JoinSet<u32> = JoinSet::new();
    let mut workers = JoinSet::new();
    let mut udp_tx: HashMap<(u16, u64), mpsc::Sender<(Bytes, OwnedSemaphorePermit)>> =
        HashMap::new();
    let queue_bytes = Arc::new(Semaphore::new(8 * 1024 * 1024));
    let next_stream = AtomicU32::new(1);
    let mut last = Instant::now();
    let mut tick = tokio::time::interval(Duration::from_secs(1));
    let (out, out_rx) = mpsc::channel::<Bytes>(1024);
    let (in_tx, mut inbound) = mpsc::channel::<Bytes>(256);
    let mut io = JoinSet::new();
    io.spawn(async move { mux_write(writer, send, out_rx).await });
    io.spawn(async move { mux_read(reader, recv, in_tx).await });
    loop {
        tokio::select! {
            Some(result) = io.join_next() => {
                result??;
                anyhow::bail!("tunnel closed");
            },
            Some(result) = listeners.join_next() => { result??; anyhow::bail!("service listener stopped"); },
            Some(done) = forwards.join_next() => {
                if let Ok(id) = done {
                    streams.remove(&id);
                }
            },
            Some(done) = workers.join_next() => { let key = done?; udp_tx.remove(&key); },
            _ = tick.tick() => {
                if cfg.heartbeat_timeout > 0
                    && last.elapsed() > Duration::from_secs(cfg.heartbeat_timeout)
                {
                    anyhow::bail!("tunnel heartbeat timeout");
                }
                if cfg.heartbeat_interval > 0 {
                    let _ = out.try_send(Bytes::from_static(&[HEARTBEAT]));
                }
                flows.lock().unwrap().expire(Duration::from_secs(cfg.transport.quic.udp_idle_timeout));
            },
            event = incoming.recv(), if server => match event {
                None => anyhow::bail!("service listener closed"),
                Some(Incoming::Tcp { service, stream }) => {
                    let id = next_stream.fetch_add(1, Ordering::Relaxed);
                    let (tx, rx) = mpsc::channel(256);
                    streams.insert(id, tx);
                    let slots = slots.clone();
                    let out = out.clone();
                    forwards.spawn(async move {
                        let Ok(permit) = slots.acquire_owned().await else { return id; };
                        let _permit = permit;
                        let mut open = crate::buf::take_vec(7);
                        open.push(OPEN);
                        open.extend_from_slice(&id.to_be_bytes());
                        open.extend_from_slice(&service.to_be_bytes());
                        let sent = send_frame(&out, &open).await;
                        crate::buf::give_vec(open);
                        if sent.is_err() {
                            return id;
                        }
                        if let Err(e) = pipe_tcp(stream, id, out, rx).await {
                            tracing::debug!(error = %e, "TCP forwarding ended");
                        }
                        id
                    });
                }
                Some(Incoming::Udp { service, peer, payload, socket }) => {
                    let flow = {
                        let mut f = flows.lock().unwrap();
                        if let Some(flow) = f.by_peer.get(&(service, peer)).copied() {
                            f.by_id.get_mut(&flow).unwrap().touched = Instant::now();
                            Some(flow)
                        } else if f.by_id.len() < cfg.transport.quic.max_udp_flows {
                            let flow = f.next;
                            f.next = f.next.checked_add(1).context("flow IDs exhausted")?;
                            f.by_peer.insert((service, peer), flow);
                            f.by_id.insert(flow, Flow {
                                service,
                                peer,
                                socket,
                                touched: Instant::now(),
                            });
                            Some(flow)
                        } else {
                            None
                        }
                    };
                    if let Some(flow) = flow {
                        send_udp(&out, service, flow, &payload).await?;
                    }
                }
            },
            Some(plain) = inbound.recv() => {
                last = Instant::now();
                dispatch(
                    &plain,
                    &cfg,
                    server,
                    &out,
                    &mut streams,
                    &mut forwards,
                    &slots,
                    &destinations,
                    &flows,
                    &mut udp_tx,
                    &mut workers,
                    &queue_bytes,
                ).await?;
            }
        }
    }
}

async fn send_udp(
    out: &mpsc::Sender<Bytes>,
    service: u16,
    flow: u64,
    payload: &[u8],
) -> Result<()> {
    let mut frame = crate::buf::take_vec(11 + payload.len());
    frame.push(UDP);
    frame.extend_from_slice(&service.to_be_bytes());
    frame.extend_from_slice(&flow.to_be_bytes());
    frame.extend_from_slice(payload);
    let result = send_frame(out, &frame).await;
    crate::buf::give_vec(frame);
    result
}

#[allow(clippy::too_many_arguments)]
async fn dispatch(
    plain: &[u8],
    cfg: &Arc<Side>,
    server: bool,
    out: &mpsc::Sender<Bytes>,
    streams: &mut HashMap<u32, mpsc::Sender<StreamMsg>>,
    forwards: &mut JoinSet<u32>,
    slots: &Arc<Semaphore>,
    destinations: &HashMap<u16, SocketAddr>,
    flows: &Arc<Mutex<Flows>>,
    udp_tx: &mut HashMap<(u16, u64), mpsc::Sender<(Bytes, OwnedSemaphorePermit)>>,
    workers: &mut JoinSet<(u16, u64)>,
    queue_bytes: &Arc<Semaphore>,
) -> Result<()> {
    ensure!(!plain.is_empty(), "empty mux frame");
    match plain[0] {
        HEARTBEAT => Ok(()),
        OPEN => {
            ensure!(!server && plain.len() == 7, "invalid OPEN");
            let id = u32::from_be_bytes(plain[1..5].try_into()?);
            let service_id = u16::from_be_bytes(plain[5..7].try_into()?);
            let (tx, rx) = mpsc::channel(256);
            streams.insert(id, tx);
            let cfg = cfg.clone();
            let out = out.clone();
            let slots = slots.clone();
            forwards.spawn(async move {
                if let Err(e) = accept_open(cfg, service_id, id, slots, out, rx).await {
                    tracing::debug!(error = %e, "TCP forwarding ended");
                }
                id
            });
            Ok(())
        }
        DATA => {
            ensure!(plain.len() >= 5, "short DATA");
            let id = u32::from_be_bytes(plain[1..5].try_into()?);
            if let Some(tx) = streams.get(&id) {
                enqueue(tx, StreamMsg::Data(crate::buf::copy_bytes(&plain[5..])));
            }
            Ok(())
        }
        FIN => {
            ensure!(plain.len() == 5, "invalid FIN");
            let id = u32::from_be_bytes(plain[1..5].try_into()?);
            if let Some(tx) = streams.remove(&id) {
                enqueue(&tx, StreamMsg::Fin);
            }
            Ok(())
        }
        UDP => {
            ensure!(plain.len() >= 11, "short UDP");
            let service = u16::from_be_bytes(plain[1..3].try_into()?);
            let flow = u64::from_be_bytes(plain[3..11].try_into()?);
            let payload = &plain[11..];
            if server {
                let destination = {
                    let mut flows = flows.lock().unwrap();
                    flows
                        .by_id
                        .get_mut(&flow)
                        .filter(|f| f.service == service)
                        .map(|f| {
                            f.touched = Instant::now();
                            (f.socket.clone(), f.peer)
                        })
                };
                if let Some((socket, peer)) = destination {
                    let _ = socket.send_to(payload, peer).await;
                }
                return Ok(());
            }
            let Some(destination) = destinations.get(&service).copied() else {
                return Ok(());
            };
            let key = (service, flow);
            if !udp_tx.contains_key(&key) {
                if udp_tx.len() >= cfg.transport.quic.max_udp_flows {
                    return Ok(());
                }
                let (tx, rx) = mpsc::channel(8);
                udp_tx.insert(key, tx);
                let out = out.clone();
                let idle = cfg.transport.quic.udp_idle_timeout;
                workers.spawn(async move {
                    if let Err(e) = udp_flow(out, key, destination, rx, idle).await {
                        tracing::debug!(error = %e, "UDP mapping ended");
                    }
                    key
                });
            }
            if let Ok(budget) = queue_bytes
                .clone()
                .try_acquire_many_owned(payload.len().max(1) as u32)
            {
                let _ = udp_tx[&key].try_send((crate::buf::copy_bytes(payload), budget));
            }
            Ok(())
        }
        _ => anyhow::bail!("unknown mux frame"),
    }
}

fn enqueue(tx: &mpsc::Sender<StreamMsg>, msg: StreamMsg) {
    if let Err(error) = tx.try_send(msg) {
        match error {
            mpsc::error::TrySendError::Full(msg) => {
                let tx = tx.clone();
                tokio::spawn(async move {
                    let _ = tx.send(msg).await;
                });
            }
            mpsc::error::TrySendError::Closed(_) => {}
        }
    }
}

async fn accept_open(
    cfg: Arc<Side>,
    service_id: u16,
    id: u32,
    slots: Arc<Semaphore>,
    out: mpsc::Sender<Bytes>,
    rx: mpsc::Receiver<StreamMsg>,
) -> Result<()> {
    let service = cfg
        .services
        .values()
        .nth(service_id as usize)
        .ok_or_else(|| anyhow::anyhow!("unknown service"))?;
    ensure!(service.kind == Kind::Tcp, "wrong service type");
    let Ok(permit) = slots.acquire_owned().await else {
        return Ok(());
    };
    let target = resolve_local(
        service.local_addr.as_ref().unwrap(),
        service.prefer_ipv6 || cfg.prefer_ipv6,
    )
    .await?;
    let retry = cfg.service_retry(service);
    let nodelay = cfg.nodelay(service);
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
    tcp.set_nodelay(nodelay)?;
    let _permit = permit;
    pipe_tcp(tcp, id, out, rx).await
}

async fn pipe_tcp(
    tcp: TcpStream,
    id: u32,
    out: mpsc::Sender<Bytes>,
    mut rx: mpsc::Receiver<StreamMsg>,
) -> Result<()> {
    let (mut reader, mut writer) = tcp.into_split();
    let up = async {
        let mut buf = crate::buf::take_vec(CHUNK);
        buf.resize(CHUNK, 0);
        let mut frame = crate::buf::take_vec(5 + CHUNK);
        loop {
            let n = reader.read(&mut buf).await?;
            if n == 0 {
                frame.clear();
                frame.push(FIN);
                frame.extend_from_slice(&id.to_be_bytes());
                send_frame(&out, &frame).await?;
                crate::buf::give_vec(buf);
                crate::buf::give_vec(frame);
                return Ok::<_, anyhow::Error>(());
            }
            frame.clear();
            frame.push(DATA);
            frame.extend_from_slice(&id.to_be_bytes());
            frame.extend_from_slice(&buf[..n]);
            send_frame(&out, &frame).await?;
        }
    };
    let down = async {
        loop {
            match rx.recv().await {
                Some(StreamMsg::Data(data)) => writer.write_all(&data).await?,
                Some(StreamMsg::Fin) | None => {
                    writer.shutdown().await?;
                    return Ok::<_, anyhow::Error>(());
                }
            }
        }
    };
    tokio::try_join!(up, down)?;
    Ok(())
}

async fn udp_flow(
    out: mpsc::Sender<Bytes>,
    key: (u16, u64),
    destination: SocketAddr,
    mut rx: mpsc::Receiver<(Bytes, OwnedSemaphorePermit)>,
    idle: u64,
) -> Result<()> {
    let bind = if destination.is_ipv4() {
        "0.0.0.0:0"
    } else {
        "[::]:0"
    };
    let socket = UdpSocket::bind(bind).await?;
    socket.connect(destination).await?;
    let mut buf = crate::buf::take_vec(65536);
    buf.resize(65536, 0);
    loop {
        let idle = tokio::time::sleep(Duration::from_secs(idle));
        tokio::select! {
            _ = idle => return Ok(()),
            p = rx.recv() => match p {
                Some((p, _budget)) => { socket.send(&p).await?; },
                None => return Ok(()),
            },
            n = socket.recv(&mut buf) => {
                let n = n?;
                if n <= datagram::MAX_PAYLOAD {
                    send_udp(&out, key.0, key.1, &buf[..n]).await?;
                }
            }
        }
    }
}
