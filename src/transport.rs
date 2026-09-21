use crate::config::Quic;
use anyhow::Result;
use quinn::{ClientConfig, Endpoint, ServerConfig, TransportConfig};
use rustls::pki_types::{CertificateDer, PrivatePkcs8KeyDer, ServerName, UnixTime};
use std::{net::SocketAddr, sync::Arc, time::Duration};

fn transport(q: &Quic) -> Result<Arc<TransportConfig>> {
    let mut t = TransportConfig::default();
    t.keep_alive_interval(
        (q.keep_alive_interval > 0).then(|| Duration::from_secs(q.keep_alive_interval)),
    );
    t.max_idle_timeout(if q.max_idle_timeout > 0 {
        Some(Duration::from_secs(q.max_idle_timeout).try_into()?)
    } else {
        None
    });
    t.max_concurrent_bidi_streams((q.max_streams + 1).into());
    t.max_concurrent_uni_streams(0u32.into());
    t.stream_receive_window(q.stream_receive_window.into());
    t.receive_window(q.receive_window.into());
    t.send_window(q.send_window.into());
    t.datagram_receive_buffer_size(Some(1024 * 1024));
    t.datagram_send_buffer_size(1024 * 1024);
    // Quinn's paced congestion control and DPLPMTUD remain enabled.
    Ok(Arc::new(t))
}

fn socket(addr: SocketAddr, q: &Quic) -> Result<std::net::UdpSocket> {
    let s = socket2::Socket::new(
        socket2::Domain::for_address(addr),
        socket2::Type::DGRAM,
        Some(socket2::Protocol::UDP),
    )?;
    s.set_recv_buffer_size(q.socket_buffer)?;
    s.set_send_buffer_size(q.socket_buffer)?;
    let receive = s.recv_buffer_size()?;
    let send = s.send_buffer_size()?;
    if receive < q.socket_buffer || send < q.socket_buffer {
        tracing::warn!(
            requested = q.socket_buffer,
            receive,
            send,
            "OS capped UDP socket buffers; high-rate throughput may be limited"
        );
    }
    s.set_nonblocking(true)?;
    s.bind(&addr.into())?;
    Ok(s.into())
}

pub fn server(addr: SocketAddr, q: &Quic) -> Result<Endpoint> {
    let (cert, key) = if let (Some(cert), Some(key)) = (&q.certificate, &q.private_key) {
        (
            CertificateDer::from(std::fs::read(cert)?),
            PrivatePkcs8KeyDer::from(std::fs::read(key)?),
        )
    } else {
        // QUIC requires TLS. In Noise compatibility mode this short-lived identity
        // encrypts the outer handshake; the configured Noise key authenticates it.
        let generated = rcgen::generate_simple_self_signed(vec![q.hostname.clone()])?;
        (
            generated.cert.der().clone(),
            PrivatePkcs8KeyDer::from(generated.signing_key.serialize_der()),
        )
    };
    let mut tls = rustls::ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(vec![cert], key.into())?;
    tls.alpn_protocols = vec![b"cathole/1".to_vec()];
    let mut config = ServerConfig::with_crypto(Arc::new(
        quinn::crypto::rustls::QuicServerConfig::try_from(tls)?,
    ));
    config.transport_config(transport(q)?);
    // The reverse proxy is not intended to migrate to arbitrary peer addresses.
    // Quinn still handles NAT port rebinding; do not disable path validation.
    Ok(Endpoint::new(
        Default::default(),
        Some(config),
        socket(addr, q)?,
        Arc::new(quinn::TokioRuntime),
    )?)
}

pub fn client(addr: SocketAddr, q: &Quic) -> Result<Endpoint> {
    let builder = rustls::ClientConfig::builder();
    let mut tls = if let Some(root) = &q.trusted_root {
        let mut roots = rustls::RootCertStore::empty();
        roots.add(CertificateDer::from(std::fs::read(root)?))?;
        builder.with_root_certificates(roots).with_no_client_auth()
    } else {
        builder
            .dangerous()
            .with_custom_certificate_verifier(NoiseAuthenticatedOuterTls::new())
            .with_no_client_auth()
    };
    tls.alpn_protocols = vec![b"cathole/1".to_vec()];
    let mut cfg = ClientConfig::new(Arc::new(quinn::crypto::rustls::QuicClientConfig::try_from(
        tls,
    )?));
    cfg.transport_config(transport(q)?);
    let mut endpoint = Endpoint::new(
        Default::default(),
        None,
        socket(addr, q)?,
        Arc::new(quinn::TokioRuntime),
    )?;
    endpoint.set_default_client_config(cfg);
    Ok(endpoint)
}

/// Certificate chain authentication is intentionally deferred to the inner
/// Noise NK/KK handshake when a legacy Noise config has no QUIC trust root.
/// TLS CertificateVerify signatures are still checked here.
#[derive(Debug)]
struct NoiseAuthenticatedOuterTls(Arc<rustls::crypto::CryptoProvider>);
impl NoiseAuthenticatedOuterTls {
    fn new() -> Arc<Self> {
        Arc::new(Self(Arc::new(rustls::crypto::ring::default_provider())))
    }
}
impl rustls::client::danger::ServerCertVerifier for NoiseAuthenticatedOuterTls {
    fn verify_server_cert(
        &self,
        _: &CertificateDer<'_>,
        _: &[CertificateDer<'_>],
        _: &ServerName<'_>,
        _: &[u8],
        _: UnixTime,
    ) -> std::result::Result<rustls::client::danger::ServerCertVerified, rustls::Error> {
        Ok(rustls::client::danger::ServerCertVerified::assertion())
    }
    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &rustls::DigitallySignedStruct,
    ) -> std::result::Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(
            message,
            cert,
            dss,
            &self.0.signature_verification_algorithms,
        )
    }
    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &rustls::DigitallySignedStruct,
    ) -> std::result::Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(
            message,
            cert,
            dss,
            &self.0.signature_verification_algorithms,
        )
    }
    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        self.0.signature_verification_algorithms.supported_schemes()
    }
}
