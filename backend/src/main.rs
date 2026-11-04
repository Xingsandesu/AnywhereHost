mod api;
#[cfg(test)]
mod benchmark;
mod codex;
mod config;
mod connections;
mod desktop;
mod events;
mod history;
mod live_activity;
mod network;
mod notifications;
mod operations;
mod permissions;
mod policy;
mod repository;
mod runtime;
mod security;
mod streaming;
mod workspace;
use anyhow::{Context, Result};
use std::{path::PathBuf, sync::Arc};
use tokio::sync::{Mutex, RwLock};

fn main() -> Result<()> {
    let args: Vec<_> = std::env::args_os().collect();
    if args.get(1).is_some_and(|arg| arg == "--read-git-state") {
        anyhow::ensure!(args.len() == 3, "expected one repository path");
        repository::limit_reader()?;
        println!("{}", repository::read(std::path::Path::new(&args[2]))?);
        return Ok(());
    }
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?
        .block_on(run())
}
async fn run() -> Result<()> {
    let args: Vec<_> = std::env::args_os().collect();
    if args.get(1).is_some_and(|arg| arg == "--version") {
        println!("AnywhereHost {}", env!("CARGO_PKG_VERSION"));
        return Ok(());
    }
    if args
        .get(1)
        .is_some_and(|arg| arg == "--check-directory-access")
    {
        anyhow::ensure!(args.len() == 3, "expected one directory path");
        println!(
            "{}",
            serde_json::to_string(&permissions::check(std::path::Path::new(&args[2])))?
        );
        return Ok(());
    }
    let dir = args
        .windows(2)
        .find(|a| a[0] == "--state-dir")
        .map(|a| PathBuf::from(&a[1]))
        .unwrap_or_else(config::default_state_dir);
    if args.get(1).is_some_and(|a| a == "--control") {
        use tokio::io::AsyncReadExt;
        let mut bytes = Vec::new();
        tokio::io::stdin()
            .take(1048576)
            .read_to_end(&mut bytes)
            .await?;
        let mut stream = tokio::net::UnixStream::connect(dir.join("control.sock")).await?;
        anywhere_transport::write_frame(&mut stream, &bytes).await?;
        let response = anywhere_transport::read_frame(&mut stream).await?;
        println!("{}", String::from_utf8(response)?);
        return Ok(());
    }
    std::fs::create_dir_all(&dir)?;
    use std::os::unix::fs::OpenOptionsExt;
    let lock = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .mode(0o600)
        .open(dir.join("instance.lock"))?;
    lock.try_lock().context("AnywhereHost already running")?;
    let (mut config, secret) = config::prepare(&dir)?;
    if let Some(url) = args.windows(2).find(|a| a[0] == "--test-relay") {
        let url = url[1].to_str().context("Invalid test relay")?;
        anyhow::ensure!(
            cfg!(debug_assertions) && url.starts_with("http://127.0.0.1:"),
            "Test relay requires a debug build and loopback URL"
        );
        anywhere_transport::validate_relay(url)?;
        config.relay_urls = vec![url.to_owned()];
    }
    let endpoint = anywhere_transport::endpoint(secret, &config.relay_urls).await?;
    let notifications = if config.notifications.as_ref().is_some_and(|c| !c.enabled) {
        None
    } else {
        notifications::Client::load(&dir).ok().map(Arc::new)
    };
    let state = Arc::new(api::AppState {
        admission: tokio::sync::RwLock::new(None),
        connections: connections::Connections::default(),
        security: security::Security::open(dir.join("devices.json"))?,
        desktop: desktop::Desktop::new(dir.join("desktop.json")),
        events: Arc::new(events::Events::open(&dir.join("events.sqlite"))?),
        operations: Arc::new(operations::Operations::open(
            &dir.join("operations.sqlite"),
        )?),
        config,
        endpoint,
        codex: RwLock::new(None),
        runtime_error: RwLock::new(None),
        connecting: Mutex::new(()),
        notifications_error: if notifications.is_none() {
            Some("Notification service is not configured".into())
        } else {
            None
        },
        notifications,
    });
    live_activity::start(state.clone());
    let local_state = state.clone();
    let socket = dir.join("control.sock");
    tokio::spawn(async move {
        if let Err(e) = network::local(local_state, &socket).await {
            eprintln!("Local control: {e}");
        }
    });
    let remote = state.clone();
    tokio::spawn(async move {
        network::serve(remote).await;
    });
    let runtime = state.clone();
    tokio::spawn(async move {
        let _ = api::connect_runtime(&runtime).await;
    });
    let recovery = state.clone();
    tokio::spawn(async move {
        loop {
            let _ = recovery.events.recover();
            tokio::time::sleep(std::time::Duration::from_secs(1)).await;
        }
    });
    eprintln!("AnywhereHost endpoint {}", state.endpoint.id());
    let mut term = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
    tokio::select! {_=tokio::signal::ctrl_c()=>{},_=term.recv()=>{}}
    state.endpoint.close().await;
    if let Some(c) = state.codex.read().await.as_ref() {
        c.stop().await;
    }
    Ok(())
}
