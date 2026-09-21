use crate::config::{Noise, key};
use anyhow::{Context, Result, ensure};
use quinn::{Connection, RecvStream, SendStream};
use snow::{HandshakeState, StatelessTransportState};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt},
    net::TcpStream,
};

pub const MAX_FRAME: usize = 65535;
pub const RELAY_BUF: usize = 256 * 1024;
pub const PROTO: &[u8] = b"cathole/2";

pub async fn write_frame<W: AsyncWrite + Unpin>(send: &mut W, data: &[u8]) -> Result<()> {
    ensure!(data.len() <= MAX_FRAME, "frame too large");
    let mut framed = crate::buf::take_vec(4 + data.len());
    framed.extend_from_slice(&(data.len() as u32).to_be_bytes());
    framed.extend_from_slice(data);
    let result = send.write_all(&framed).await;
    crate::buf::give_vec(framed);
    result.map_err(Into::into)
}

pub async fn read_frame_into<R: AsyncRead + Unpin>(
    recv: &mut R,
    data: &mut Vec<u8>,
) -> Result<Option<()>> {
    let mut prefix = [0; 4];
    match recv.read_exact(&mut prefix).await {
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(e) => return Err(e.into()),
    }
    let len = u32::from_be_bytes(prefix) as usize;
    ensure!(len <= MAX_FRAME, "oversized frame");
    data.clear();
    data.resize(len, 0);
    recv.read_exact(data).await?;
    Ok(Some(()))
}

pub async fn read_frame<R: AsyncRead + Unpin>(recv: &mut R) -> Result<Option<Vec<u8>>> {
    let mut data = crate::buf::take_vec(256);
    match read_frame_into(recv, &mut data).await? {
        None => {
            crate::buf::give_vec(data);
            Ok(None)
        }
        Some(()) => Ok(Some(data)),
    }
}

fn handshake_with_prologue(
    cfg: &Noise,
    initiator: bool,
    prologue: &[u8],
) -> Result<HandshakeState> {
    let mut b = snow::Builder::new(cfg.pattern.parse()?).prologue(prologue)?;
    let pattern = cfg.pattern.split('_').nth(1).unwrap_or("");
    let generated;
    let local = if cfg.local_private_key.is_some() {
        Some(key(&cfg.local_private_key)?)
    } else if pattern == "XX" {
        generated = snow::Builder::new(cfg.pattern.parse()?)
            .generate_keypair()?
            .private;
        Some(generated)
    } else {
        None
    };
    let remote = if cfg.remote_public_key.is_some() {
        Some(key(&cfg.remote_public_key)?)
    } else {
        None
    };
    if let Some(k) = local.as_deref() {
        b = b.local_private_key(k)?;
    }
    if let Some(k) = remote.as_deref() {
        b = b.remote_public_key(k)?;
    }
    Ok(if initiator {
        b.build_initiator()?
    } else {
        b.build_responder()?
    })
}

fn quic_prologue(conn: &Connection, context: &[u8]) -> Result<Vec<u8>> {
    let mut binding = [0; 32];
    conn.export_keying_material(&mut binding, b"cathole-noise-v1", context)
        .map_err(|_| anyhow::anyhow!("TLS exporter failed"))?;
    let mut prologue = PROTO.to_vec();
    prologue.extend_from_slice(&binding);
    prologue.extend_from_slice(context);
    Ok(prologue)
}

pub fn handshake(
    conn: &Connection,
    cfg: &Noise,
    initiator: bool,
    context: &[u8],
) -> Result<HandshakeState> {
    handshake_with_prologue(cfg, initiator, &quic_prologue(conn, context)?)
}

pub fn handshake_tcp(cfg: &Noise, initiator: bool, context: &[u8]) -> Result<HandshakeState> {
    let mut prologue = PROTO.to_vec();
    prologue.extend_from_slice(b"tcp");
    prologue.extend_from_slice(context);
    handshake_with_prologue(cfg, initiator, &prologue)
}

async fn complete_handshake<W, R>(
    mut hs: HandshakeState,
    send: &mut W,
    recv: &mut R,
    initiator: bool,
) -> Result<HandshakeState>
where
    W: AsyncWrite + Unpin,
    R: AsyncRead + Unpin,
{
    let mut buf = crate::buf::take_vec(MAX_FRAME);
    if buf.len() < MAX_FRAME {
        buf.resize(MAX_FRAME, 0);
    }
    let mut our_turn = initiator;
    while !hs.is_handshake_finished() {
        if our_turn {
            let n = hs.write_message(&[], &mut buf)?;
            write_frame(send, &buf[..n]).await?;
        } else {
            let message = read_frame(recv)
                .await?
                .context("missing Noise handshake message")?;
            hs.read_message(&message, &mut buf)?;
        }
        our_turn = !our_turn;
    }
    crate::buf::give_vec(buf);
    Ok(hs)
}

async fn complete_handshake_rw<S>(
    mut hs: HandshakeState,
    stream: &mut S,
    initiator: bool,
) -> Result<HandshakeState>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let mut buf = crate::buf::take_vec(MAX_FRAME);
    if buf.len() < MAX_FRAME {
        buf.resize(MAX_FRAME, 0);
    }
    let mut our_turn = initiator;
    while !hs.is_handshake_finished() {
        if our_turn {
            let n = hs.write_message(&[], &mut buf)?;
            write_frame(stream, &buf[..n]).await?;
        } else {
            let message = read_frame(stream)
                .await?
                .context("missing Noise handshake message")?;
            hs.read_message(&message, &mut buf)?;
        }
        our_turn = !our_turn;
    }
    crate::buf::give_vec(buf);
    Ok(hs)
}

/// Directional Noise AEAD for a single authenticated session. Send and receive
/// nonces are independent, so the halves can be used concurrently without a lock.
pub struct SessionAead {
    state: std::sync::Arc<StatelessTransportState>,
    next_send: u64,
    next_recv: u64,
}

pub struct AeadSend {
    state: std::sync::Arc<StatelessTransportState>,
    next: u64,
}

pub struct AeadRecv {
    state: std::sync::Arc<StatelessTransportState>,
    next: u64,
}

impl SessionAead {
    fn from_handshake(hs: HandshakeState) -> Result<Self> {
        Ok(Self {
            state: std::sync::Arc::new(hs.into_stateless_transport_mode()?),
            next_send: 0,
            next_recv: 0,
        })
    }

    pub fn split(self) -> (AeadSend, AeadRecv) {
        (
            AeadSend {
                state: self.state.clone(),
                next: self.next_send,
            },
            AeadRecv {
                state: self.state,
                next: self.next_recv,
            },
        )
    }

    pub async fn write<W: AsyncWrite + Unpin>(
        &mut self,
        send: &mut W,
        payload: &[u8],
    ) -> Result<()> {
        let mut cipher = crate::buf::take_vec(payload.len() + 16);
        let mut tx = AeadSend {
            state: self.state.clone(),
            next: self.next_send,
        };
        tx.seal(payload, &mut cipher)?;
        self.next_send = tx.next;
        let result = write_frame(send, &cipher).await;
        crate::buf::give_vec(cipher);
        result
    }

    pub async fn read<R: AsyncRead + Unpin>(&mut self, recv: &mut R) -> Result<Option<Vec<u8>>> {
        let Some(cipher) = read_frame(recv).await? else {
            return Ok(None);
        };
        let mut plain = crate::buf::take_vec(cipher.len());
        let mut rx = AeadRecv {
            state: self.state.clone(),
            next: self.next_recv,
        };
        let opened = rx.open(&cipher, &mut plain);
        crate::buf::give_vec(cipher);
        opened?;
        self.next_recv = rx.next;
        Ok(Some(plain))
    }
}

impl AeadSend {
    pub fn seal(&mut self, payload: &[u8], out: &mut Vec<u8>) -> Result<()> {
        ensure!(
            self.next < (1u64 << 32),
            "Noise key packet budget exhausted; reconnect required"
        );
        let nonce = self.next;
        out.clear();
        out.resize(payload.len() + 16, 0);
        let n = self.state.write_message(nonce, payload, out)?;
        out.truncate(n);
        self.next += 1;
        Ok(())
    }
}

impl AeadRecv {
    pub fn open(&mut self, cipher: &[u8], out: &mut Vec<u8>) -> Result<()> {
        ensure!(
            self.next < (1u64 << 32),
            "Noise key packet budget exhausted; reconnect required"
        );
        let nonce = self.next;
        out.clear();
        out.resize(cipher.len(), 0);
        let n = self.state.read_message(nonce, cipher, out)?;
        out.truncate(n);
        self.next += 1;
        Ok(())
    }
}

async fn begin_session<W, R>(
    hs: HandshakeState,
    send: &mut W,
    recv: &mut R,
    initiator: bool,
) -> Result<SessionAead>
where
    W: AsyncWrite + Unpin,
    R: AsyncRead + Unpin,
{
    SessionAead::from_handshake(complete_handshake(hs, send, recv, initiator).await?)
}

pub async fn initiate_session(
    conn: &Connection,
    cfg: &Noise,
    send: &mut SendStream,
    recv: &mut RecvStream,
    payload: &[u8],
) -> Result<()> {
    let mut aead = begin_session(
        handshake(conn, cfg, true, b"registration")?,
        send,
        recv,
        true,
    )
    .await?;
    aead.write(send, payload).await?;
    let reply = aead.read(recv).await?.context("missing Noise response")?;
    ensure!(reply == b"ok", "service registration rejected");
    Ok(())
}

pub async fn receive_session(
    conn: &Connection,
    cfg: &Noise,
    send: &mut SendStream,
    recv: &mut RecvStream,
) -> Result<(SessionAead, Vec<u8>)> {
    let mut aead = begin_session(
        handshake(conn, cfg, false, b"registration")?,
        send,
        recv,
        false,
    )
    .await?;
    let payload = aead.read(recv).await?.context("missing Noise request")?;
    Ok((aead, payload))
}

pub async fn confirm_session<W: AsyncWrite + Unpin>(
    aead: &mut SessionAead,
    send: &mut W,
) -> Result<()> {
    aead.write(send, b"ok").await
}

pub async fn initiate_tcp_session<S: AsyncRead + AsyncWrite + Unpin>(
    stream: &mut S,
    cfg: &Noise,
    payload: &[u8],
) -> Result<SessionAead> {
    let mut aead = SessionAead::from_handshake(
        complete_handshake_rw(handshake_tcp(cfg, true, b"registration")?, stream, true).await?,
    )?;
    aead.write(stream, payload).await?;
    let reply = aead.read(stream).await?.context("missing Noise response")?;
    ensure!(reply == b"ok", "service registration rejected");
    Ok(aead)
}

pub async fn receive_tcp_session<S: AsyncRead + AsyncWrite + Unpin>(
    stream: &mut S,
    cfg: &Noise,
) -> Result<(SessionAead, Vec<u8>)> {
    let mut aead = SessionAead::from_handshake(
        complete_handshake_rw(handshake_tcp(cfg, false, b"registration")?, stream, false).await?,
    )?;
    let payload = aead.read(stream).await?.context("missing Noise request")?;
    Ok((aead, payload))
}

pub async fn relay_raw(tcp: TcpStream, mut send: SendStream, mut recv: RecvStream) -> Result<()> {
    let (mut reader, mut writer) = tcp.into_split();
    let up = async {
        let mut buf = bytes::BytesMut::with_capacity(RELAY_BUF);
        loop {
            if buf.capacity() < RELAY_BUF {
                buf.reserve(RELAY_BUF);
            }
            let n = reader.read_buf(&mut buf).await?;
            if n == 0 {
                send.finish()?;
                return Ok::<_, anyhow::Error>(());
            }
            send.write_chunk(buf.split().freeze()).await?;
        }
    };
    let down = async {
        loop {
            match recv.read_chunk(RELAY_BUF, true).await? {
                None => {
                    writer.shutdown().await?;
                    return Ok::<_, anyhow::Error>(());
                }
                Some(chunk) => writer.write_all(&chunk.bytes).await?,
            }
        }
    };
    tokio::try_join!(up, down)?;
    Ok(())
}

/// Max plaintext per Noise frame on a dedicated TCP data channel.
pub const DATA_CHUNK: usize = 60 * 1024;

/// Bidirectional relay of a local TCP socket over a Noise-framed tunnel stream.
pub async fn relay_noise<S>(tcp: TcpStream, tunnel: S, aead: SessionAead) -> Result<()>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let (mut tun_r, mut tun_w) = tokio::io::split(tunnel);
    let (mut tx, mut rx) = aead.split();
    let (mut local_r, mut local_w) = tcp.into_split();
    let up = async {
        let mut plain = crate::buf::take_vec(DATA_CHUNK);
        plain.resize(DATA_CHUNK, 0);
        let mut cipher = crate::buf::take_vec(DATA_CHUNK + 16);
        loop {
            let n = local_r.read(&mut plain).await?;
            if n == 0 {
                crate::buf::give_vec(plain);
                crate::buf::give_vec(cipher);
                tokio::io::AsyncWriteExt::shutdown(&mut tun_w).await?;
                return Ok::<_, anyhow::Error>(());
            }
            tx.seal(&plain[..n], &mut cipher)?;
            write_frame(&mut tun_w, &cipher).await?;
        }
    };
    let down = async {
        let mut cipher = crate::buf::take_vec(MAX_FRAME);
        let mut plain = crate::buf::take_vec(MAX_FRAME);
        loop {
            let Some(()) = read_frame_into(&mut tun_r, &mut cipher).await? else {
                local_w.shutdown().await?;
                crate::buf::give_vec(cipher);
                crate::buf::give_vec(plain);
                return Ok::<_, anyhow::Error>(());
            };
            rx.open(&cipher, &mut plain)?;
            local_w.write_all(&plain).await?;
        }
    };
    match tokio::try_join!(up, down) {
        Ok(((), ())) => Ok(()),
        Err(e) => Err(e),
    }
}

/// Explicit nonces permit unordered UDP delivery. Replay state changes only
/// after authentication succeeds. The state is never reused across sessions.
pub struct DatagramCrypto {
    state: StatelessTransportState,
    next: u64,
    replay: ReplayWindow,
}
impl DatagramCrypto {
    pub fn new(state: StatelessTransportState) -> Self {
        Self {
            state,
            next: 0,
            replay: ReplayWindow::default(),
        }
    }
    pub fn seal(&mut self, plain: &[u8]) -> Result<Vec<u8>> {
        ensure!(
            self.next < (1u64 << 32),
            "Noise UDP key packet budget exhausted; reconnect required"
        );
        let nonce = self.next;
        self.next += 1;
        let mut out = vec![0; plain.len() + 24];
        out[..8].copy_from_slice(&nonce.to_be_bytes());
        let n = self.state.write_message(nonce, plain, &mut out[8..])?;
        out.truncate(n + 8);
        Ok(out)
    }
    pub fn open(&mut self, cipher: &[u8]) -> Result<Vec<u8>> {
        ensure!(cipher.len() >= 24, "short Noise datagram");
        let n = u64::from_be_bytes(cipher[..8].try_into()?);
        ensure!(
            n < (1u64 << 32) && self.replay.acceptable(n),
            "replayed or stale datagram"
        );
        let mut out = vec![0; cipher.len() - 8];
        let len = self.state.read_message(n, &cipher[8..], &mut out)?;
        self.replay.record(n);
        out.truncate(len);
        Ok(out)
    }
}

#[derive(Default)]
struct ReplayWindow {
    highest: Option<u64>,
    seen: [u64; 16],
}
impl ReplayWindow {
    fn acceptable(&self, n: u64) -> bool {
        match self.highest {
            None => true,
            Some(h) if n > h => true,
            Some(h) => h - n < 1024 && self.seen[(n % 1024 / 64) as usize] & (1 << (n % 64)) == 0,
        }
    }
    fn record(&mut self, n: u64) {
        if let Some(h) = self.highest
            && n > h
        {
            if n - h >= 1024 {
                self.seen = [0; 16];
            } else {
                for x in h + 1..=n {
                    self.seen[(x % 1024 / 64) as usize] &= !(1 << (x % 64));
                }
            }
        }
        self.highest = Some(self.highest.map_or(n, |h| h.max(n)));
        self.seen[(n % 1024 / 64) as usize] |= 1 << (n % 64);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::PATTERN;
    fn pair() -> (DatagramCrypto, DatagramCrypto) {
        let keys = snow::Builder::new(PATTERN.parse().unwrap())
            .generate_keypair()
            .unwrap();
        let mut a = snow::Builder::new(PATTERN.parse().unwrap())
            .remote_public_key(&keys.public)
            .unwrap()
            .build_initiator()
            .unwrap();
        let mut b = snow::Builder::new(PATTERN.parse().unwrap())
            .local_private_key(&keys.private)
            .unwrap()
            .build_responder()
            .unwrap();
        let mut wire = [0; 1024];
        let mut plain = [0; 1024];
        let n = a.write_message(b"registration", &mut wire).unwrap();
        b.read_message(&wire[..n], &mut plain).unwrap();
        let n = b.write_message(b"ok", &mut wire).unwrap();
        a.read_message(&wire[..n], &mut plain).unwrap();
        (
            DatagramCrypto::new(a.into_stateless_transport_mode().unwrap()),
            DatagramCrypto::new(b.into_stateless_transport_mode().unwrap()),
        )
    }
    #[test]
    fn noise_datagrams_authenticate_reorder_and_reject_replay() {
        let (mut a, mut b) = pair();
        let first = a.seal(b"first").unwrap();
        let second = a.seal(b"second").unwrap();
        let mut forged = first.clone();
        *forged.last_mut().unwrap() ^= 1;
        assert!(b.open(&forged).is_err());
        assert_eq!(b.open(&second).unwrap(), b"second");
        assert_eq!(b.open(&first).unwrap(), b"first");
        assert!(b.open(&first).is_err());
        assert_eq!(a.open(&b.seal(b"response").unwrap()).unwrap(), b"response");
        let (_, mut other) = pair();
        assert!(other.open(&first).is_err());
    }
    #[test]
    fn session_aead_splits_without_shared_lock() {
        let keys = snow::Builder::new(PATTERN.parse().unwrap())
            .generate_keypair()
            .unwrap();
        let mut a = snow::Builder::new(PATTERN.parse().unwrap())
            .remote_public_key(&keys.public)
            .unwrap()
            .build_initiator()
            .unwrap();
        let mut b = snow::Builder::new(PATTERN.parse().unwrap())
            .local_private_key(&keys.private)
            .unwrap()
            .build_responder()
            .unwrap();
        let mut wire = [0; 1024];
        let mut plain = [0; 1024];
        let n = a.write_message(&[], &mut wire).unwrap();
        b.read_message(&wire[..n], &mut plain).unwrap();
        let n = b.write_message(&[], &mut wire).unwrap();
        a.read_message(&wire[..n], &mut plain).unwrap();
        let (mut atx, mut arx) = SessionAead::from_handshake(a).unwrap().split();
        let (mut btx, mut brx) = SessionAead::from_handshake(b).unwrap().split();
        let mut cipher = Vec::new();
        let mut out = Vec::new();
        atx.seal(b"up", &mut cipher).unwrap();
        brx.open(&cipher, &mut out).unwrap();
        assert_eq!(out, b"up");
        btx.seal(b"down", &mut cipher).unwrap();
        arx.open(&cipher, &mut out).unwrap();
        assert_eq!(out, b"down");
    }
    #[test]
    fn nonce_budget_fails_closed() {
        let (mut a, _) = pair();
        a.next = 1u64 << 32;
        assert!(a.seal(b"no wrap").is_err());
    }
    #[test]
    fn replay_window_handles_reordering_and_large_gaps() {
        let mut r = ReplayWindow::default();
        for n in [10, 8, 9, 1025, 1024, 3000] {
            assert!(r.acceptable(n));
            r.record(n);
            assert!(!r.acceptable(n));
        }
        assert!(!r.acceptable(8));
        assert!(r.acceptable(2999));
    }
}
