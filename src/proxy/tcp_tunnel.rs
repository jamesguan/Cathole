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
    collections::{HashMap, HashSet, VecDeque},
    net::SocketAddr,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use subtle::ConstantTimeEq;
use tokio::{
    io::{AsyncRead, AsyncWrite},
    net::{TcpListener, TcpStream, UdpSocket},
    sync::{OwnedSemaphorePermit, Semaphore, mpsc, oneshot},
    task::JoinSet,
};
use tokio_rustls::{TlsAcceptor, TlsConnector};

const HEARTBEAT: u8 = 5;
const UDP: u8 = 4;
const CREATE_DATA: u8 = 6;
const DATA_OPEN: u8 = 0xD0;
/// Idle data channels kept ready per TCP service so wrk connection bursts
/// do not wait on a Noise handshake for the first request.
const WARM_POOL: usize = 256;

/// Pairs public visitor sockets with client-opened Noise data channels.
struct PairingHub {
    waiting_data: Mutex<HashMap<u16, VecDeque<oneshot::Sender<TcpStream>>>>,
    waiting_visitors: Mutex<HashMap<u16, VecDeque<TcpStream>>>,
    /// Reliable create requests; never drop under accept bursts.
    create: mpsc::UnboundedSender<u16>,
}

impl PairingHub {
    fn new(create: mpsc::UnboundedSender<u16>) -> Self {
        Self {
            waiting_data: Mutex::new(HashMap::new()),
            waiting_visitors: Mutex::new(HashMap::new()),
            create,
        }
    }

    fn request_create(&self, service: u16) {
        let _ = self.create.send(service);
    }

    fn seed_warm(&self, service: u16) {
        for _ in 0..WARM_POOL {
            self.request_create(service);
        }
    }

    fn visitor_arrived(&self, service: u16, tcp: TcpStream) {
        if let Some(tx) = self
            .waiting_data
            .lock()
            .unwrap()
            .get_mut(&service)
            .and_then(VecDeque::pop_front)
        {
            let _ = tx.send(tcp);
            // Refill the warm slot that was just consumed.
            self.request_create(service);
            return;
        }
        self.waiting_visitors
            .lock()
            .unwrap()
            .entry(service)
            .or_default()
            .push_back(tcp);
        // One new data channel for this waiting visitor.
        self.request_create(service);
    }

    async fn take_visitor(&self, service: u16) -> Result<TcpStream> {
        if let Some(tcp) = self
            .waiting_visitors
            .lock()
            .unwrap()
            .get_mut(&service)
            .and_then(VecDeque::pop_front)
        {
            return Ok(tcp);
        }
        let (tx, rx) = oneshot::channel();
        self.waiting_data
            .lock()
            .unwrap()
            .entry(service)
            .or_default()
            .push_back(tx);
        rx.await
            .map_err(|_| anyhow::anyhow!("data channel cancelled before visitor arrived"))
    }
}

enum Bound {
    Tcp(TcpListener),
    Udp(Arc<UdpSocket>),
}
enum Incoming {
    Udp {
        service: u16,
        peer: SocketAddr,
        payload: Bytes,
        socket: Arc<UdpSocket>,
    },
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
        tokio::io::AsyncWriteExt::write_all(&mut self.writer, &self.framed).await?;
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

fn data_open_payload(service: u16) -> [u8; 3] {
    let mut p = [DATA_OPEN, 0, 0];
    p[1..].copy_from_slice(&service.to_be_bytes());
    p
}

fn parse_data_open(payload: &[u8]) -> Option<u16> {
    if payload.len() == 3 && payload[0] == DATA_OPEN {
        Some(u16::from_be_bytes(payload[1..3].try_into().ok()?))
    } else {
        None
    }
}

struct HubGuard;
impl Drop for HubGuard {
    fn drop(&mut self) {
        *ACTIVE_HUB.lock().unwrap() = None;
    }
}

static ACTIVE_HUB: Mutex<Option<Arc<PairingHub>>> = Mutex::new(None);

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
    let slots = Arc::new(Semaphore::new(
        (cfg.transport.quic.max_streams as usize)
            .saturating_add(cfg.transport.quic.max_connections)
            .max(64),
    ));
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
                        tokio::time::timeout(SESSION_LIFETIME, accept_connection(cfg, tcp)).await??;
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

async fn accept_connection(cfg: Arc<Side>, tcp: TcpStream) -> Result<()> {
    configure_tunnel_socket(&tcp, &cfg.transport.tcp)?;
    if cfg.transport.kind == "tls" {
        let tls = TlsAcceptor::from(Arc::new(transport::rustls_server(&cfg.transport.quic)?));
        dispatch_incoming(cfg, tls.accept(tcp).await?).await
    } else {
        dispatch_incoming(cfg, tcp).await
    }
}

async fn dispatch_incoming<S>(cfg: Arc<Side>, mut stream: S) -> Result<()>
where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    let (mut aead, payload) =
        crypto::receive_tcp_session(&mut stream, &cfg.transport.noise).await?;
    if let Some(service) = parse_data_open(&payload) {
        crypto::confirm_session(&mut aead, &mut stream).await?;
        let hub = ACTIVE_HUB
            .lock()
            .unwrap()
            .clone()
            .context("data channel without active control session")?;
        let visitor = hub.take_visitor(service).await?;
        crypto::relay_noise(visitor, stream, aead).await
    } else {
        server_control(cfg, stream, aead, payload).await
    }
}

async fn server_control<S>(
    cfg: Arc<Side>,
    mut stream: S,
    mut aead: crypto::SessionAead,
    payload: Vec<u8>,
) -> Result<()>
where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
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
    tracing::info!(
        services = bound.len(),
        "client authenticated and services registered"
    );

    let (create_tx, mut create_rx) = mpsc::unbounded_channel::<u16>();
    let hub = Arc::new(PairingHub::new(create_tx));
    *ACTIVE_HUB.lock().unwrap() = Some(hub.clone());
    let _hub_guard = HubGuard;

    let (reader, writer) = tokio::io::split(stream);
    let (tx, rx) = aead.split();
    let (events, rx_events) = mpsc::channel(8192);
    let slots = Arc::new(Semaphore::new(cfg.transport.quic.max_streams as usize));
    let mut tasks = JoinSet::new();
    for (id, (name, service, listener)) in bound.into_iter().enumerate() {
        let events = events.clone();
        let cfg = cfg.clone();
        let hub = hub.clone();
        let sid = id as u16;
        tasks.spawn(async move {
            tracing::info!(service = %name, "service ready");
            match listener {
                Bound::Tcp(listener) => {
                    hub.seed_warm(sid);
                    tcp_listen(listener, service, sid, cfg, hub, events).await
                }
                Bound::Udp(socket) => udp_listen(socket, sid, events).await,
            }
        });
    }
    drop(events);
    drive_server(
        cfg,
        reader,
        writer,
        tx,
        rx,
        rx_events,
        &mut create_rx,
        slots,
        tasks,
    )
    .await
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

async fn client_session<S>(cfg: Arc<Side>, mut stream: S) -> Result<()>
where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    let aead = tokio::time::timeout(SETUP, async {
        let reg = Registration {
            version: REGISTRATION_VERSION,
            services: {
                let mut names: Vec<_> = cfg.services.keys().cloned().collect();
                names.sort();
                names
                    .into_iter()
                    .map(|name| {
                        let s = &cfg.services[&name];
                        Offer {
                            name,
                            kind: s.kind,
                            token: cfg.token(s).into(),
                        }
                    })
                    .collect()
            },
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
    drive_client(cfg, reader, writer, tx, rx).await
}

async fn open_data_channel(cfg: Arc<Side>, service_id: u16) -> Result<()> {
    let mut names: Vec<_> = cfg.services.keys().cloned().collect();
    names.sort();
    let name = names
        .get(service_id as usize)
        .context("unknown service")?;
    let service = &cfg.services[name];
    ensure!(
        service.kind == Kind::Tcp,
        "data channel for non-TCP service"
    );
    let remote = resolve(cfg.remote_addr.as_ref().unwrap(), cfg.prefer_ipv6).await?;
    let mut tcp = tokio::time::timeout(SETUP, TcpStream::connect(remote)).await??;
    configure_tunnel_socket(&tcp, &cfg.transport.tcp)?;

    let aead = if cfg.transport.kind == "tls" {
        let root = cfg
            .tls_trusted_root()
            .context("type=tls requires trusted_root")?;
        let tls = TlsConnector::from(Arc::new(transport::rustls_client(root)?));
        let name = ServerName::try_from(cfg.tls_hostname().to_owned())
            .map_err(|_| anyhow::anyhow!("invalid TLS hostname"))?;
        let mut tls = tls.connect(name, tcp).await?;
        let aead = crypto::initiate_tcp_session(
            &mut tls,
            &cfg.transport.noise,
            &data_open_payload(service_id),
        )
        .await?;
        let local = connect_local(&cfg, service).await?;
        return crypto::relay_noise(local, tls, aead).await;
    } else {
        crypto::initiate_tcp_session(
            &mut tcp,
            &cfg.transport.noise,
            &data_open_payload(service_id),
        )
        .await?
    };
    let local = connect_local(&cfg, service).await?;
    crypto::relay_noise(local, tcp, aead).await
}

async fn connect_local(cfg: &Side, service: &Service) -> Result<TcpStream> {
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
    Ok(tcp)
}

async fn tcp_listen(
    listener: TcpListener,
    service: Service,
    id: u16,
    cfg: Arc<Side>,
    hub: Arc<PairingHub>,
    _events: mpsc::Sender<Incoming>,
) -> Result<()> {
    loop {
        let (tcp, _) = listener.accept().await?;
        tcp.set_nodelay(cfg.nodelay(&service))?;
        hub.visitor_arrived(id, tcp);
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
async fn drive_server<S>(
    cfg: Arc<Side>,
    reader: tokio::io::ReadHalf<S>,
    writer: tokio::io::WriteHalf<S>,
    send: crypto::AeadSend,
    recv: crypto::AeadRecv,
    mut incoming: mpsc::Receiver<Incoming>,
    create_rx: &mut mpsc::UnboundedReceiver<u16>,
    _slots: Arc<Semaphore>,
    mut listeners: JoinSet<Result<()>>,
) -> Result<()>
where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    let flows = Arc::new(Mutex::new(Flows {
        next: 0,
        by_id: HashMap::new(),
        by_peer: HashMap::new(),
    }));
    let mut workers = JoinSet::new();
    let mut udp_tx: HashMap<(u16, u64), mpsc::Sender<(Bytes, OwnedSemaphorePermit)>> =
        HashMap::new();
    let queue_bytes = Arc::new(Semaphore::new(8 * 1024 * 1024));
    let mut tick = tokio::time::interval(Duration::from_secs(1));
    // Large enough for CREATE bursts under wrk (1000+) without blocking accepts.
    let (out, out_rx) = mpsc::channel::<Bytes>(8192);
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
            Some(done) = workers.join_next() => { let key = done?; udp_tx.remove(&key); },
            Some(service) = create_rx.recv() => {
                let mut frame = [CREATE_DATA, 0, 0];
                frame[1..].copy_from_slice(&service.to_be_bytes());
                // Must await: dropping CREATE under load was the wrk timeout source.
                out.send(Bytes::copy_from_slice(&frame)).await?;
            },
            _ = tick.tick() => {
                if cfg.heartbeat_interval > 0 {
                    let _ = out.try_send(Bytes::from_static(&[HEARTBEAT]));
                }
                flows.lock().unwrap().expire(Duration::from_secs(cfg.transport.quic.udp_idle_timeout));
            },
            event = incoming.recv() => match event {
                None => anyhow::bail!("service listener closed"),
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
                dispatch_control(
                    &plain,
                    &cfg,
                    true,
                    &out,
                    &HashMap::new(),
                    &flows,
                    &mut udp_tx,
                    &mut workers,
                    &queue_bytes,
                ).await?;
            }
        }
    }
}

async fn drive_client<S>(
    cfg: Arc<Side>,
    reader: tokio::io::ReadHalf<S>,
    writer: tokio::io::WriteHalf<S>,
    send: crypto::AeadSend,
    recv: crypto::AeadRecv,
) -> Result<()>
where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    let mut destinations = HashMap::new();
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
    let flows = Arc::new(Mutex::new(Flows {
        next: 0,
        by_id: HashMap::new(),
        by_peer: HashMap::new(),
    }));
    let mut workers = JoinSet::new();
    let mut udp_tx: HashMap<(u16, u64), mpsc::Sender<(Bytes, OwnedSemaphorePermit)>> =
        HashMap::new();
    let queue_bytes = Arc::new(Semaphore::new(8 * 1024 * 1024));
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
            Some(done) = workers.join_next() => { let key = done?; udp_tx.remove(&key); },
            _ = tick.tick() => {
                if cfg.heartbeat_timeout > 0
                    && last.elapsed() > Duration::from_secs(cfg.heartbeat_timeout)
                {
                    anyhow::bail!("tunnel heartbeat timeout");
                }
                flows.lock().unwrap().expire(Duration::from_secs(cfg.transport.quic.udp_idle_timeout));
            },
            Some(plain) = inbound.recv() => {
                last = Instant::now();
                if !plain.is_empty() && plain[0] == CREATE_DATA {
                    ensure!(plain.len() == 3, "invalid CREATE_DATA");
                    let service = u16::from_be_bytes(plain[1..3].try_into()?);
                    let cfg = cfg.clone();
                    tokio::spawn(async move {
                        if let Err(e) = open_data_channel(cfg, service).await {
                            tracing::debug!(error = %e, "data channel ended");
                        }
                    });
                    continue;
                }
                dispatch_control(
                    &plain,
                    &cfg,
                    false,
                    &out,
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
async fn dispatch_control(
    plain: &[u8],
    cfg: &Arc<Side>,
    server: bool,
    out: &mpsc::Sender<Bytes>,
    destinations: &HashMap<u16, SocketAddr>,
    flows: &Arc<Mutex<Flows>>,
    udp_tx: &mut HashMap<(u16, u64), mpsc::Sender<(Bytes, OwnedSemaphorePermit)>>,
    workers: &mut JoinSet<(u16, u64)>,
    queue_bytes: &Arc<Semaphore>,
) -> Result<()> {
    ensure!(!plain.is_empty(), "empty mux frame");
    match plain[0] {
        HEARTBEAT | CREATE_DATA => Ok(()),
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
