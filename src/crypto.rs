use crate::config::{Noise, key};
use anyhow::{Context, Result, ensure};
use quinn::{Connection, RecvStream, SendStream};
use snow::{HandshakeState, StatelessTransportState, TransportState};
use std::sync::{Arc, Mutex};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpStream,
};

pub const MAX_FRAME: usize = 65535;

pub async fn write_frame(send: &mut SendStream, data: &[u8]) -> Result<()> {
    ensure!(data.len() <= MAX_FRAME, "frame too large");
    send.write_all(&(data.len() as u32).to_be_bytes()).await?;
    send.write_all(data).await?;
    Ok(())
}
pub async fn read_frame(recv: &mut RecvStream) -> Result<Option<Vec<u8>>> {
    let mut prefix = [0; 4];
    // Distinguish orderly FIN from a truncated frame.
    if recv.read(&mut prefix[..1]).await?.is_none() {
        return Ok(None);
    }
    recv.read_exact(&mut prefix[1..]).await?;
    let len = u32::from_be_bytes(prefix) as usize;
    ensure!(len <= MAX_FRAME, "oversized frame");
    let mut data = vec![0; len];
    recv.read_exact(&mut data).await?;
    Ok(Some(data))
}

fn handshake(
    conn: &Connection,
    cfg: &Noise,
    initiator: bool,
    context: &[u8],
) -> Result<HandshakeState> {
    let mut binding = [0; 32];
    conn.export_keying_material(&mut binding, b"cathole-noise-v1", context)
        .map_err(|_| anyhow::anyhow!("TLS exporter failed"))?;
    let mut prologue = b"cathole/1".to_vec();
    prologue.extend_from_slice(&binding);
    prologue.extend_from_slice(context);
    let mut b = snow::Builder::new(cfg.pattern.parse()?).prologue(&prologue)?;
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

async fn complete_handshake(
    conn: &Connection,
    cfg: &Noise,
    send: &mut SendStream,
    recv: &mut RecvStream,
    context: &[u8],
    initiator: bool,
) -> Result<HandshakeState> {
    let mut hs = handshake(conn, cfg, initiator, context)?;
    let mut buf = vec![0; MAX_FRAME];
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
    Ok(hs)
}

pub async fn initiate_stream(
    conn: &Connection,
    cfg: &Noise,
    send: &mut SendStream,
    recv: &mut RecvStream,
    context: &[u8],
    payload: &[u8],
) -> Result<(TransportState, Vec<u8>)> {
    let mut state = complete_handshake(conn, cfg, send, recv, context, true)
        .await?
        .into_transport_mode()?;
    write_transport(&mut state, send, payload).await?;
    let reply = read_transport(&mut state, recv)
        .await?
        .context("missing Noise response")?;
    Ok((state, reply))
}
pub async fn receive_stream(
    conn: &Connection,
    cfg: &Noise,
    send: &mut SendStream,
    recv: &mut RecvStream,
    context: &[u8],
) -> Result<(TransportState, Vec<u8>)> {
    let mut state = complete_handshake(conn, cfg, send, recv, context, false)
        .await?
        .into_transport_mode()?;
    let payload = read_transport(&mut state, recv)
        .await?
        .context("missing Noise request")?;
    Ok((state, payload))
}
pub async fn write_transport(
    state: &mut TransportState,
    send: &mut SendStream,
    payload: &[u8],
) -> Result<()> {
    let mut buf = vec![0; payload.len() + 16];
    let n = state.write_message(payload, &mut buf)?;
    write_frame(send, &buf[..n]).await
}
async fn read_transport(
    state: &mut TransportState,
    recv: &mut RecvStream,
) -> Result<Option<Vec<u8>>> {
    let Some(cipher) = read_frame(recv).await? else {
        return Ok(None);
    };
    let mut plain = vec![0; cipher.len()];
    let n = state.read_message(&cipher, &mut plain)?;
    plain.truncate(n);
    Ok(Some(plain))
}

pub async fn initiate_datagram(
    conn: &Connection,
    cfg: &Noise,
    send: &mut SendStream,
    recv: &mut RecvStream,
    context: &[u8],
    payload: &[u8],
) -> Result<(DatagramCrypto, Vec<u8>)> {
    let state = complete_handshake(conn, cfg, send, recv, context, true)
        .await?
        .into_stateless_transport_mode()?;
    let mut crypto = DatagramCrypto::new(state);
    write_frame(send, &crypto.seal(payload)?).await?;
    let reply = crypto.open(&read_frame(recv).await?.context("missing Noise response")?)?;
    Ok((crypto, reply))
}
pub async fn receive_datagram(
    conn: &Connection,
    cfg: &Noise,
    send: &mut SendStream,
    recv: &mut RecvStream,
    context: &[u8],
) -> Result<(DatagramCrypto, Vec<u8>)> {
    let state = complete_handshake(conn, cfg, send, recv, context, false)
        .await?
        .into_stateless_transport_mode()?;
    let mut crypto = DatagramCrypto::new(state);
    let payload = crypto.open(&read_frame(recv).await?.context("missing Noise request")?)?;
    Ok((crypto, payload))
}

pub async fn bridge(
    tcp: TcpStream,
    mut send: SendStream,
    mut recv: RecvStream,
    noise: TransportState,
) -> Result<()> {
    let state = Arc::new(Mutex::new(noise));
    let (mut reader, mut writer) = tcp.into_split();
    let txstate = state.clone();
    let tx = async move {
        let mut plain = vec![0; 16384];
        let mut cipher = vec![0; 16400];
        loop {
            let n = reader.read(&mut plain).await?;
            if n == 0 {
                send.finish()?;
                return Ok::<_, anyhow::Error>(());
            }
            let n = txstate
                .lock()
                .unwrap()
                .write_message(&plain[..n], &mut cipher)?;
            write_frame(&mut send, &cipher[..n]).await?;
        }
    };
    let rx = async move {
        while let Some(cipher) = read_frame(&mut recv).await? {
            let mut plain = vec![0; cipher.len()];
            let n = state.lock().unwrap().read_message(&cipher, &mut plain)?;
            writer.write_all(&plain[..n]).await?;
        }
        writer.shutdown().await?;
        Ok::<_, anyhow::Error>(())
    };
    tokio::try_join!(tx, rx)?;
    Ok(())
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
