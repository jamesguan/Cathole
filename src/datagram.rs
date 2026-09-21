use anyhow::{Result, ensure};
use quinn::Connection;
use std::{
    collections::HashMap,
    time::{Duration, Instant},
};

// Fixed-size fragments fit the QUIC DATAGRAM budget. QUIC packet protection
// encrypts them; there is no second application AEAD on this path.
pub const CHUNK: usize = 1000;
pub const MAX_PAYLOAD: usize = 65507;
const HEADER: usize = 22;
const MAX_ASSEMBLIES: usize = 128;

pub struct Sender {
    next_message: std::sync::atomic::AtomicU64,
}
impl Default for Sender {
    fn default() -> Self {
        Self::new()
    }
}
impl Sender {
    pub fn new() -> Self {
        Self {
            next_message: 0.into(),
        }
    }
    pub fn send(&self, conn: &Connection, service: u16, flow: u64, payload: &[u8]) -> Result<()> {
        ensure!(payload.len() <= MAX_PAYLOAD, "UDP payload too large");
        let message = self
            .next_message
            .fetch_update(
                std::sync::atomic::Ordering::Relaxed,
                std::sync::atomic::Ordering::Relaxed,
                |n| n.checked_add(1),
            )
            .map_err(|_| anyhow::anyhow!("message ID exhausted"))?;
        for i in 0..payload.len().div_ceil(CHUNK).max(1) {
            let start = i * CHUNK;
            let end = (start + CHUNK).min(payload.len());
            let mut p = crate::buf::take_buf(HEADER + end - start);
            p.extend_from_slice(&service.to_be_bytes());
            p.extend_from_slice(&flow.to_be_bytes());
            p.extend_from_slice(&message.to_be_bytes());
            p.extend_from_slice(&(payload.len() as u16).to_be_bytes());
            p.extend_from_slice(&(i as u16).to_be_bytes());
            p.extend_from_slice(&payload[start..end]);
            conn.send_datagram(p.freeze())?;
        }
        Ok(())
    }
}

pub struct Packet {
    pub service: u16,
    pub flow: u64,
    pub payload: Vec<u8>,
}
struct Assembly {
    service: u16,
    flow: u64,
    length: usize,
    parts: Vec<Option<Vec<u8>>>,
    created: Instant,
}
#[derive(Default)]
pub struct Reassembler {
    pending: HashMap<u64, Assembly>,
}
impl Reassembler {
    pub fn expire(&mut self, now: Instant) {
        self.pending
            .retain(|_, a| now.duration_since(a.created) < Duration::from_secs(2));
    }
    pub fn push(&mut self, p: &[u8], now: Instant) -> Result<Option<Packet>> {
        ensure!(p.len() >= HEADER, "short fragment");
        let service = u16::from_be_bytes(p[..2].try_into()?);
        let flow = u64::from_be_bytes(p[2..10].try_into()?);
        let message = u64::from_be_bytes(p[10..18].try_into()?);
        let length = u16::from_be_bytes(p[18..20].try_into()?) as usize;
        let index = u16::from_be_bytes(p[20..22].try_into()?) as usize;
        ensure!(length <= MAX_PAYLOAD, "invalid UDP length");
        let count = length.div_ceil(CHUNK).max(1);
        ensure!(index < count, "invalid fragment index");
        ensure!(
            p.len() - HEADER == (length - index * CHUNK).min(CHUNK),
            "invalid fragment length"
        );
        if count == 1 {
            return Ok(Some(Packet {
                service,
                flow,
                payload: {
                    let mut out = crate::buf::take_vec(p.len() - HEADER);
                    out.extend_from_slice(&p[HEADER..]);
                    out
                },
            }));
        }
        self.expire(now);
        if !self.pending.contains_key(&message) && self.pending.len() >= MAX_ASSEMBLIES {
            return Ok(None);
        }
        let a = self.pending.entry(message).or_insert_with(|| Assembly {
            service,
            flow,
            length,
            parts: vec![None; count],
            created: now,
        });
        ensure!(
            a.service == service && a.flow == flow && a.length == length,
            "inconsistent fragments"
        );
        a.parts[index] = Some({
            let mut part = crate::buf::take_vec(p.len() - HEADER);
            part.extend_from_slice(&p[HEADER..]);
            part
        });
        if a.parts.iter().any(Option::is_none) {
            return Ok(None);
        }
        let a = self.pending.remove(&message).unwrap();
        Ok(Some(Packet {
            service,
            flow,
            payload: a.parts.into_iter().flat_map(|v| v.unwrap()).collect(),
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fragment(index: u16) -> Vec<u8> {
        let mut p = vec![];
        p.extend_from_slice(&3u16.to_be_bytes());
        p.extend_from_slice(&4u64.to_be_bytes());
        p.extend_from_slice(&5u64.to_be_bytes());
        p.extend_from_slice(&1500u16.to_be_bytes());
        p.extend_from_slice(&index.to_be_bytes());
        p.extend(vec![index as u8; if index == 0 { 1000 } else { 500 }]);
        p
    }
    #[test]
    fn reordered_fragments_preserve_packet_boundaries() {
        let mut r = Reassembler::default();
        let now = Instant::now();
        assert!(r.push(&fragment(1), now).unwrap().is_none());
        let p = r.push(&fragment(0), now).unwrap().unwrap();
        assert_eq!((p.service, p.flow, p.payload.len()), (3, 4, 1500));
        assert_eq!(&p.payload[1000..], &[1; 500]);
    }
    #[test]
    fn lost_fragment_expires_without_partial_delivery() {
        let mut r = Reassembler::default();
        let now = Instant::now();
        r.push(&fragment(0), now).unwrap();
        r.expire(now + Duration::from_secs(3));
        assert!(r.pending.is_empty());
        assert!(
            r.push(&fragment(1), now + Duration::from_secs(3))
                .unwrap()
                .is_none()
        );
    }
    #[test]
    fn malformed_fragment_is_rejected() {
        let mut r = Reassembler::default();
        assert!(r.push(&[0; 2], Instant::now()).is_err());
        assert!(r.push(&fragment(2), Instant::now()).is_err());
    }
}
