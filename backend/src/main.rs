mod api;
mod security;
mod ingress;
mod apns;
mod codex;
mod config;
mod desktop;
mod events;
mod history;
mod live_activity;
mod policy;
mod permissions;
mod streaming;
mod tunnel;
mod workspace;
use anyhow::{Context, Result};
use std::{path::PathBuf, sync::Arc};
use tokio::sync::{Mutex, RwLock};

#[tokio::main]
async fn main() -> Result<()> {
    let args: Vec<_> = std::env::args_os().collect();
    if args.get(1).is_some_and(|arg| arg == "--check-directory-access") {
        anyhow::ensure!(args.len() == 3, "expected one directory path");
        println!("{}", serde_json::to_string(&permissions::check(std::path::Path::new(&args[2])))?);
        return Ok(());
    }
    let dir = args
        .windows(2)
        .find(|a| a[0] == "--state-dir")
        .map(|a| PathBuf::from(&a[1]))
        .unwrap_or_else(|| {
            PathBuf::from(std::env::var_os("HOME").expect("HOME"))
                .join("Library/Application Support/AnywhereHost")
        });
    std::fs::create_dir_all(&dir)?;
    use std::os::unix::fs::OpenOptionsExt;
    let instance_lock = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .mode(0o600)
        .open(dir.join("instance.lock"))?;
    instance_lock
        .try_lock()
        .context("AnywhereHost is already running with this state directory")?;
    let (config, token, fingerprint) = config::prepare(&dir)?;
    let events = Arc::new(events::Events::open(&dir.join("events.sqlite"))?);
    let (apns, apns_error) = match apns::Client::load_optional(config.apns.as_ref(), &dir) {
        Ok(client) => (client.map(Arc::new), None),
        Err(_) => { eprintln!("APNs configuration could not be loaded; check signing key, IDs and permissions");
            (None, Some("APNs configuration invalid; check signing key, IDs and permissions".to_owned())) }
    };
    let exe = std::env::current_exe()?;
    let frpc = config
        .frpc_binary
        .clone()
        .unwrap_or_else(|| exe.parent().unwrap().join("frpc"));
    let state = Arc::new(api::AppState {
        security: security::Security::open(dir.join("devices-v2.json"))?,
        desktop: desktop::Desktop::new(dir.join("desktop.json")),
        token,
        fingerprint,
        config: config.clone(),
        codex: RwLock::new(None),
        runtime_error: RwLock::new(None),
        connecting: Mutex::new(()),
        events,
        apns,
        apns_error,
        tunnel: tunnel::Tunnel::new(frpc, dir.clone(), config.listen.port()),
    });
    live_activity::start(state.clone());
    let certificates = rustls_pemfile::certs(&mut std::io::BufReader::new(std::fs::File::open(dir.join("host-cert.pem"))?)).collect::<Result<Vec<_>,_>>()?;
    let key = rustls_pemfile::private_key(&mut std::io::BufReader::new(std::fs::File::open(dir.join("host-key.pem"))?))?.context("missing TLS private key")?;
    let mut tls_config = rustls::ServerConfig::builder_with_provider(Arc::new(rustls::crypto::aws_lc_rs::default_provider()))
        .with_protocol_versions(&[&rustls::version::TLS13])?
        .with_no_client_auth().with_single_cert(certificates,key)?;
    tls_config.max_early_data_size = 0;
    tls_config.alpn_protocols = vec![b"h2".to_vec(), b"http/1.1".to_vec()];
    let tls = axum_server::tls_rustls::RustlsConfig::from_config(Arc::new(tls_config));
    let handle = axum_server::Handle::new();
    let stop_handle = handle.clone();
    let stop_state = state.clone();
    tokio::spawn(async move {
        let mut term = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("SIGTERM handler");
        tokio::select! {_=tokio::signal::ctrl_c()=>{},_=term.recv()=>{}}
        let _ = stop_state.tunnel.stop().await;
        if let Some(c) = stop_state.codex.read().await.as_ref() {
            c.stop().await;
        }
        stop_handle.graceful_shutdown(Some(std::time::Duration::from_secs(2)));
    });
    let connect_state = state.clone();
    tokio::spawn(async move {
        let _ = api::connect_runtime(&connect_state).await;
    });
    eprintln!("AnywhereHost listening on https://{}", config.listen);
    let mut server = axum_server::bind_rustls(config.listen, tls)
        .map(|acceptor| acceptor.handshake_timeout(std::time::Duration::from_secs(5)).acceptor(ingress::ConnectionLimit::new()))
        .handle(handle);
    server.http_builder().http1().timer(hyper_util::rt::TokioTimer::new())
        .header_read_timeout(std::time::Duration::from_secs(10)).max_headers(48).max_buf_size(32768);
    server.http_builder().http2().max_concurrent_streams(32).max_header_list_size(16384);
    server
        .serve(api::router(state).into_make_service())
        .await?;
    Ok(())
}
