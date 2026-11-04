use anyhow::{Context, Result};
use anywhere_transport::admission;
use iroh_relay::server::{
    Access, AccessControl, CertConfig, ClientRateLimit, ClientRequest, ConnectionId,
    DEFAULT_CERT_RELOAD_INTERVAL, QuicConfig, RelayConfig, Server, ServerConfig, TlsConfig,
    reloading_resolver,
};
use std::{
    collections::HashMap,
    net::SocketAddr,
    path::PathBuf,
    sync::{Arc, Mutex},
    time::Duration,
};
#[derive(Debug, Default)]
struct Gate {
    active: Mutex<HashMap<ConnectionId, (iroh::EndpointId, u64)>>,
}
impl AccessControl for Gate {
    async fn on_connect(&self, request: &ClientRequest) -> Access {
        let token = request.auth_token().unwrap_or_default();
        match admission::verify(&token, &request.endpoint_id().to_string(), None) {
            Ok(c) => {
                let mut active = self.active.lock().unwrap();
                if active.len() >= 4096
                    || active
                        .values()
                        .filter(|(id, _)| *id == request.endpoint_id())
                        .count()
                        >= 4
                {
                    return Access::Deny {
                        reason: Some("official application admission required".into()),
                    };
                }
                active.insert(request.connection_id(), (request.endpoint_id(), c.exp));
                Access::Allow
            }
            Err(_) => Access::Deny {
                reason: Some("official application admission required".into()),
            },
        }
    }
    fn on_disconnect(&self, _: iroh::EndpointId, id: ConnectionId) {
        self.active.lock().unwrap().remove(&id);
    }
}
#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct Config {
    http: SocketAddr,
    https: SocketAddr,
    quic: SocketAddr,
    metrics: SocketAddr,
    certificate: PathBuf,
    private_key: PathBuf,
}
#[tokio::main]
async fn main() -> Result<()> {
    let path = std::env::args()
        .nth(1)
        .context("Usage: anywhere-relay CONFIG")?;
    let c: Config = toml::from_str(&std::fs::read_to_string(path)?)?;
    anyhow::ensure!(
        c.http.ip().is_loopback() && c.metrics.ip().is_loopback(),
        "HTTP and metrics must be loopback-only"
    );
    let provider = Arc::new(rustls::crypto::aws_lc_rs::default_provider());
    let resolver = reloading_resolver(
        &provider,
        c.certificate,
        c.private_key,
        DEFAULT_CERT_RELOAD_INTERVAL,
    )
    .await?;
    let tls = rustls::ServerConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()?
        .with_no_client_auth()
        .with_cert_resolver(resolver);
    let gate = Arc::new(Gate::default());
    let mut relay = RelayConfig::new(c.http);
    relay.access = gate.clone();
    relay.tls = Some(TlsConfig::new(
        c.https,
        CertConfig::Manual { server_config: tls },
    ));
    relay.limits.client_rx = Some(ClientRateLimit::new(10_485_760.try_into()?));
    relay.key_cache_capacity = Some(8192);
    let mut config = ServerConfig::default();
    config.relay = Some(relay);
    config.quic = Some(QuicConfig::new(c.quic));
    config.metrics_addr = Some(c.metrics);
    let server = Server::spawn(config).await?;
    let clients = server
        .relay_service()
        .context("relay service")?
        .clients()
        .clone();
    let mut interval = tokio::time::interval(Duration::from_secs(1));
    let mut term = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
    loop {
        tokio::select! {
            _=tokio::signal::ctrl_c()=>break,
            _=term.recv()=>break,
            _=interval.tick()=>{
                let expired:Vec<_>=gate.active.lock().unwrap().iter().filter(|(_,(_,exp))|*exp<=admission::now()).map(|(conn,(id,_))|(*conn,*id)).collect();
                for (conn,id) in expired {clients.disconnect(id,Some(conn));}
            }
        }
    }
    server.shutdown().await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use iroh::Watcher;
    #[tokio::test]
    async fn relay_rejects_an_unsigned_native_endpoint() {
        let gate = Arc::new(Gate::default());
        let mut relay = RelayConfig::new("127.0.0.1:0".parse::<SocketAddr>().unwrap());
        relay.access = gate;
        let mut cfg = ServerConfig::default();
        cfg.relay = Some(relay);
        let server = Server::spawn(cfg).await.unwrap();
        let endpoint = anywhere_transport::endpoint(
            iroh::SecretKey::from_bytes(&anywhere_transport::new_secret()),
            &[format!("http://{}", server.http_addr().unwrap())],
        )
        .await
        .unwrap();
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                if endpoint
                    .home_relay_status()
                    .get()
                    .iter()
                    .any(|s| s.auth_denied_reason().is_some())
                {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        })
        .await
        .unwrap();
        endpoint.close().await;
        server.shutdown().await.unwrap();
    }
}
