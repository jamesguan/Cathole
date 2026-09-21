use axum::{
    Router,
    body::Body,
    http::{HeaderMap, Method, StatusCode, Uri, header},
    routing::get,
};
use axum_server::tls_rustls::RustlsConfig;
use bytes::Bytes;
use futures_util::stream::unfold;
use std::convert::Infallible;
use std::net::SocketAddr;
use std::sync::OnceLock;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, UdpSocket};

const CHUNK: usize = 1024 * 1024;
const DEFAULT_VIDEO_BYTES: u64 = 10 * 1024 * 1024 * 1024;

static KIB_64: OnceLock<Bytes> = OnceLock::new();
static MIB_1: OnceLock<Bytes> = OnceLock::new();
static VIDEO_CHUNK: OnceLock<Bytes> = OnceLock::new();
static VIDEO_LEN: OnceLock<u64> = OnceLock::new();

fn video_len() -> u64 {
    *VIDEO_LEN.get_or_init(|| {
        std::env::var("VIDEO_BYTES")
            .ok()
            .and_then(|v| v.parse().ok())
            .filter(|n: &u64| *n > 0)
            .unwrap_or(DEFAULT_VIDEO_BYTES)
    })
}

fn video_chunk() -> Bytes {
    VIDEO_CHUNK
        .get_or_init(|| Bytes::from(vec![0xab; CHUNK]))
        .clone()
}

fn payload(bytes: &'static OnceLock<Bytes>, size: usize) -> axum::http::Response<Body> {
    let body = bytes.get_or_init(|| Bytes::from(vec![b'x'; size])).clone();
    axum::http::Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "application/octet-stream")
        .header(header::CACHE_CONTROL, "no-store")
        .body(Body::from(body))
        .unwrap()
}

fn query_u64(uri: &Uri, name: &str) -> Option<u64> {
    uri.query()?
        .split('&')
        .find_map(|part| part.strip_prefix(&format!("{name}="))?.parse().ok())
}

fn parse_range(headers: &HeaderMap, total: u64) -> Result<Option<(u64, u64)>, ()> {
    let Some(value) = headers.get(header::RANGE) else {
        return Ok(None);
    };
    let value = value.to_str().map_err(|_| ())?;
    let spec = value.strip_prefix("bytes=").ok_or(())?;
    if spec.contains(',') {
        return Err(());
    }
    let (start_raw, end_raw) = spec.split_once('-').ok_or(())?;
    let (start, end) = if start_raw.is_empty() {
        let suffix: u64 = end_raw.parse().map_err(|_| ())?;
        (total.saturating_sub(suffix), total)
    } else {
        let start: u64 = start_raw.parse().map_err(|_| ())?;
        let end = if end_raw.is_empty() {
            total
        } else {
            end_raw
                .parse::<u64>()
                .map_err(|_| ())?
                .saturating_add(1)
                .min(total)
        };
        (start, end)
    };
    if start >= total || end <= start {
        return Err(());
    }
    Ok(Some((start, end.min(total))))
}

fn stream_bytes(start: u64, end: u64) -> Body {
    let remaining = end.saturating_sub(start);
    let skip = (start % CHUNK as u64) as usize;
    Body::from_stream(unfold(
        (remaining, skip),
        |(remaining, skip)| async move {
            if remaining == 0 {
                return None;
            }
            let mut buf = video_chunk();
            if skip != 0 && skip < buf.len() {
                buf = buf.slice(skip..);
            }
            let take = remaining.min(buf.len() as u64) as usize;
            buf = buf.slice(..take);
            Some((
                Ok::<Bytes, Infallible>(buf),
                (remaining - take as u64, 0usize),
            ))
        },
    ))
}

fn video_response(
    method: Method,
    start: u64,
    end: u64,
    total: u64,
    partial: bool,
) -> axum::http::Response<Body> {
    let length = end.saturating_sub(start);
    let mut builder = axum::http::Response::builder()
        .header(header::CONTENT_TYPE, "video/mp4")
        .header(header::ACCEPT_RANGES, "bytes")
        .header(header::CACHE_CONTROL, "no-store")
        .header(header::CONTENT_LENGTH, length);
    builder = if partial {
        builder
            .status(StatusCode::PARTIAL_CONTENT)
            .header(
                header::CONTENT_RANGE,
                format!("bytes {}-{}/{}", start, end.saturating_sub(1), total),
            )
    } else {
        builder.status(StatusCode::OK)
    };
    let body = if method == Method::HEAD {
        Body::empty()
    } else {
        stream_bytes(start, end)
    };
    builder.body(body).unwrap()
}

async fn hello() -> &'static str {
    "Hello, Cathole benchmark!\n"
}

async fn kib_64() -> axum::http::Response<Body> {
    payload(&KIB_64, 64 * 1024)
}

async fn mib_1() -> axum::http::Response<Body> {
    payload(&MIB_1, 1024 * 1024)
}

async fn video(method: Method, uri: Uri, headers: HeaderMap) -> axum::http::Response<Body> {
    let total = query_u64(&uri, "bytes")
        .unwrap_or_else(video_len)
        .min(video_len())
        .max(1);
    match parse_range(&headers, total) {
        Ok(None) => video_response(method, 0, total, total, false),
        Ok(Some((start, end))) => video_response(method, start, end, total, true),
        Err(()) => axum::http::Response::builder()
            .status(StatusCode::RANGE_NOT_SATISFIABLE)
            .header(header::CONTENT_RANGE, format!("bytes */{total}"))
            .body(Body::empty())
            .unwrap(),
    }
}

fn app() -> Router {
    Router::new()
        .route("/", get(hello))
        .route("/health", get(|| async { StatusCode::NO_CONTENT }))
        .route("/64k", get(kib_64))
        .route("/1m", get(mib_1))
        .route("/video", get(video))
}

async fn tcp_echo(listener: TcpListener) {
    loop {
        let Ok((mut stream, _)) = listener.accept().await else {
            continue;
        };
        let _ = stream.set_nodelay(true);
        tokio::spawn(async move {
            let mut buf = vec![0u8; 65536];
            loop {
                match stream.read(&mut buf).await {
                    Ok(0) => break,
                    Ok(n) => {
                        if stream.write_all(&buf[..n]).await.is_err() {
                            break;
                        }
                    }
                    Err(_) => break,
                }
            }
        });
    }
}

async fn udp_echo(socket: UdpSocket) {
    let mut buf = vec![0u8; 65536];
    loop {
        let Ok((n, peer)) = socket.recv_from(&mut buf).await else {
            continue;
        };
        let _ = socket.send_to(&buf[..n], peer).await;
    }
}

async fn self_signed_tls() -> RustlsConfig {
    let certified = rcgen::generate_simple_self_signed(vec![
        "backend".into(),
        "localhost".into(),
        "127.0.0.1".into(),
    ])
    .expect("self-signed cert");
    let cert = certified.cert.pem();
    let key = certified.key_pair.serialize_pem();
    RustlsConfig::from_pem(cert.into_bytes(), key.into_bytes())
        .await
        .expect("tls config")
}

#[tokio::main(flavor = "multi_thread")]
async fn main() {
    rustls::crypto::ring::default_provider()
        .install_default()
        .expect("rustls crypto provider");

    let http = TcpListener::bind("0.0.0.0:3000").await.unwrap();
    let tcp = TcpListener::bind("0.0.0.0:3001").await.unwrap();
    let udp = UdpSocket::bind("0.0.0.0:3002").await.unwrap();
    let https_addr: SocketAddr = "0.0.0.0:3443".parse().unwrap();
    let tls = self_signed_tls().await;

    println!(
        "benchmark backend: HTTP {}:3000, TCP echo {}:3001, UDP echo {}:3002, HTTPS {}:3443; video {} bytes",
        http.local_addr().unwrap().ip(),
        tcp.local_addr().unwrap().ip(),
        udp.local_addr().unwrap().ip(),
        https_addr.ip(),
        video_len()
    );

    let router = app();
    tokio::spawn(tcp_echo(tcp));
    tokio::spawn(udp_echo(udp));
    tokio::spawn(axum_server::bind_rustls(https_addr, tls).serve(router.clone().into_make_service()));
    axum::serve(http, router).await.unwrap();
}
