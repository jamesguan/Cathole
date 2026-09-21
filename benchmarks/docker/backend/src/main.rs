use axum::{
    Router,
    body::Body,
    http::{Response, StatusCode, header},
    routing::get,
};
use bytes::Bytes;
use std::sync::OnceLock;

static KIB_64: OnceLock<Bytes> = OnceLock::new();
static MIB_1: OnceLock<Bytes> = OnceLock::new();

fn payload(bytes: &'static OnceLock<Bytes>, size: usize) -> Response<Body> {
    let body = bytes.get_or_init(|| Bytes::from(vec![b'x'; size])).clone();
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "application/octet-stream")
        .header(header::CACHE_CONTROL, "no-store")
        .body(Body::from(body))
        .unwrap()
}

async fn hello() -> &'static str {
    "Hello, Cathole benchmark!\n"
}

async fn kib_64() -> Response<Body> {
    payload(&KIB_64, 64 * 1024)
}

async fn mib_1() -> Response<Body> {
    payload(&MIB_1, 1024 * 1024)
}

#[tokio::main(flavor = "multi_thread")]
async fn main() {
    let app = Router::new()
        .route("/", get(hello))
        .route("/health", get(|| async { StatusCode::NO_CONTENT }))
        .route("/64k", get(kib_64))
        .route("/1m", get(mib_1));
    let listener = tokio::net::TcpListener::bind("0.0.0.0:3000").await.unwrap();
    println!(
        "Axum benchmark backend listening on {}; Tokio worker threads use available cores",
        listener.local_addr().unwrap()
    );
    axum::serve(listener, app).await.unwrap();
}
