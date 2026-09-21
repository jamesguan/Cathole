use anyhow::{Context, Result};
use base64::{Engine, engine::general_purpose::STANDARD};
use cathole::{
    config::{Config, PATTERN},
    proxy,
};
use clap::{Parser, ValueEnum};
use std::{
    io::Write,
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

#[derive(Parser)]
#[command(version, about)]
struct Args {
    /// TOML configuration. Paths inside it are relative to this file.
    config: Option<PathBuf>,
    /// Validate configuration and identity files, then exit.
    #[arg(long)]
    check: bool,
    /// Print a fresh Noise keypair (default: x25519).
    #[arg(long, num_args = 0..=1, default_missing_value = "x25519", value_name = "CURVE")]
    genkey: Option<KeypairType>,
    /// Create local example configs and fresh TLS/Noise identities in a new directory.
    #[arg(long)]
    init: Option<PathBuf>,
    #[arg(long, short = 's', conflicts_with = "client")]
    server: bool,
    #[arg(long, short = 'c', conflicts_with = "server")]
    client: bool,
}

#[derive(Clone, Copy, ValueEnum)]
enum KeypairType {
    X25519,
}

fn create(path: &Path, bytes: &[u8]) -> Result<()> {
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options.open(path)?.write_all(bytes)?;
    Ok(())
}

fn initialize(dir: &Path) -> Result<()> {
    std::fs::create_dir(dir).context("identity directory must not already exist")?;
    let keys = snow::Builder::new(PATTERN.parse()?).generate_keypair()?;
    let cert = rcgen::generate_simple_self_signed(vec!["cathole".into()])?;
    create(&dir.join("server.der"), cert.cert.der())?;
    create(
        &dir.join("server-key.der"),
        &cert.signing_key.serialize_der(),
    )?;
    let token = STANDARD.encode(rand::random::<[u8; 32]>());
    let server = format!(
        r#"[server]
bind_addr = "127.0.0.1:2333"
default_token = "{token}"

[server.transport]
type = "quic"
[server.transport.noise]
local_private_key = "{}"
[server.transport.quic]
certificate = "server.der"
private_key = "server-key.der"
keep_alive_interval = 15
max_idle_timeout = 60

[server.services.echo]
type = "tcp"
bind_addr = "127.0.0.1:5202"
[server.services.echo_udp]
type = "udp"
bind_addr = "127.0.0.1:5202"
"#,
        STANDARD.encode(keys.private)
    );
    let client = format!(
        r#"[client]
remote_addr = "127.0.0.1:2333"
default_token = "{token}"

[client.transport]
type = "quic"
[client.transport.noise]
remote_public_key = "{}"
[client.transport.quic]
trusted_root = "server.der"
hostname = "cathole"
keep_alive_interval = 15
max_idle_timeout = 60

[client.services.echo]
type = "tcp"
local_addr = "127.0.0.1:5201"
[client.services.echo_udp]
type = "udp"
local_addr = "127.0.0.1:5201"
"#,
        STANDARD.encode(keys.public)
    );
    create(&dir.join("server.toml"), server.as_bytes())?;
    create(&dir.join("client.toml"), client.as_bytes())?;
    println!(
        "Created fresh identities and example configs in {}",
        dir.display()
    );
    Ok(())
}

fn validate_files(c: &Config) -> Result<()> {
    let s = c.server.as_ref().or(c.client.as_ref()).unwrap();
    if s.tcp_tunnel() {
        if s.transport.kind == "tls" {
            if c.server.is_some() {
                let _ = cathole::transport::rustls_server(&s.transport.quic)?;
            } else {
                let root = s
                    .tls_trusted_root()
                    .context("type=tls requires trusted_root")?;
                let _ = cathole::transport::rustls_client(root)?;
            }
        }
        return Ok(());
    }
    let q = &s.transport.quic;
    // Build TLS configs on a temporary loopback endpoint to verify keys/certs.
    // This never opens a configured public listener.
    let ep = if c.server.is_some() {
        cathole::transport::server("127.0.0.1:0".parse()?, q)?
    } else {
        cathole::transport::client("127.0.0.1:0".parse()?, q)?
    };
    ep.close(0u32.into(), b"validation");
    Ok(())
}

fn launch(
    c: Config,
) -> (
    tokio::sync::watch::Sender<bool>,
    tokio::task::JoinHandle<Result<()>>,
) {
    let (stop, mut rx) = tokio::sync::watch::channel(false);
    let task = tokio::spawn(async move {
        if let Some(s) = c.server {
            let s = Arc::new(s);
            if s.tcp_tunnel() {
                proxy::run_tcp_server(s, rx).await
            } else {
                proxy::run_server(s, rx).await
            }
        } else {
            let s = Arc::new(c.client.unwrap());
            let tcp = s.tcp_tunnel();
            tokio::select! {
                r = async move {
                    if tcp {
                        proxy::run_tcp_client(s).await
                    } else {
                        proxy::run_client(s).await
                    }
                } => r,
                _ = rx.changed() => Ok(()),
            }
        }
    });
    (stop, task)
}

fn worker_threads() -> usize {
    if let Ok(n) = std::env::var("TOKIO_WORKER_THREADS")
        && let Ok(n) = n.parse::<usize>()
    {
        return n.max(1);
    }
    std::thread::available_parallelism()
        .map(std::num::NonZeroUsize::get)
        .unwrap_or(4)
        .max(2)
}

fn main() -> Result<()> {
    rustls::crypto::ring::default_provider()
        .install_default()
        .ok();
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();
    let workers = worker_threads();
    tracing::info!(worker_threads = workers, "starting multi-thread runtime");
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .worker_threads(workers)
        .max_blocking_threads(workers)
        .thread_name("cathole")
        .build()?
        .block_on(async_main())
}

async fn async_main() -> Result<()> {
    let args = Args::parse();
    if let Some(curve) = args.genkey {
        let pattern = match curve {
            KeypairType::X25519 => PATTERN,
        };
        let k = snow::Builder::new(pattern.parse()?).generate_keypair()?;
        println!(
            "Private Key:\n{}\nPublic Key:\n{}",
            STANDARD.encode(k.private),
            STANDARD.encode(k.public)
        );
        return Ok(());
    }
    if let Some(dir) = args.init {
        return initialize(&dir);
    }
    let path = args
        .config
        .context("provide a config file, --init DIR, or --genkey")?;
    let parsed = Config::read(&path)?;
    let role_server = if args.server {
        true
    } else if args.client {
        false
    } else if parsed.server.is_some() != parsed.client.is_some() {
        parsed.server.is_some()
    } else {
        anyhow::bail!("config contains both roles; select --server/-s or --client/-c")
    };
    let mut current = parsed.select(role_server)?;
    validate_files(&current)?;
    if args.check {
        println!("Configuration and identity files are valid");
        return Ok(());
    }
    let mut seen = std::fs::read(&path)?;
    let (mut stop, mut running) = launch(current.clone());
    let mut rollback: Option<Config> = None;
    let mut tick = tokio::time::interval(Duration::from_secs(1));
    loop {
        tokio::select! {
            _ = tokio::signal::ctrl_c() => { let _ = stop.send(true); let _ = running.await; return Ok(()); },
            result = &mut running => {
                if let Some(old) = rollback.take() {
                    tracing::error!(result = ?result, "reload failed; restoring previous configuration");
                    current = old; (stop, running) = launch(current.clone());
                } else { return result?; }
            },
            _ = tick.tick() => {
                let Ok(bytes) = std::fs::read(&path) else { continue; };
                if bytes == seen { continue; }
                seen = bytes;
                match Config::read(&path).and_then(|c| c.select(role_server)).and_then(|c| { validate_files(&c)?; Ok(c) }) {
                    Ok(next) => {
                        tracing::warn!("config changed; restarting session and active forwarded connections");
                        let _ = stop.send(true); let _ = (&mut running).await;
                        rollback = Some(current); current = next; (stop, running) = launch(current.clone());
                    },
                    Err(e) => tracing::error!(error = %e, "invalid reload ignored; previous configuration remains active"),
                }
            }
        }
    }
}
