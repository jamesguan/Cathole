use axum::{
    Router,
    body::Body,
    http::{HeaderMap, Method, StatusCode, Uri, header},
    routing::get,
};
use bytes::Bytes;
use futures_util::stream::unfold;
use std::convert::Infallible;
use std::sync::OnceLock;

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

#[tokio::main(flavor = "multi_thread")]
async fn main() {
    let app = Router::new()
        .route("/", get(hello))
        .route("/health", get(|| async { StatusCode::NO_CONTENT }))
        .route("/64k", get(kib_64))
        .route("/1m", get(mib_1))
        .route("/video", get(video));
    let listener = tokio::net::TcpListener::bind("0.0.0.0:3000").await.unwrap();
    println!(
        "Axum benchmark backend listening on {}; 10 GiB Emby-style stream on /video ({} bytes)",
        listener.local_addr().unwrap(),
        video_len()
    );
    axum::serve(listener, app).await.unwrap();
}
