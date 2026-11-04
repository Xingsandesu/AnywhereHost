//! 仅用于隔离集成检查的本机 relay。
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let mut config = iroh_relay::server::ServerConfig::default();
    config.relay = Some(iroh_relay::server::RelayConfig::new(([127, 0, 0, 1], 0)));
    let server = iroh_relay::server::Server::spawn(config).await?;
    println!("http://{}", server.http_addr().unwrap());
    tokio::signal::ctrl_c().await?;
    server.shutdown().await?;
    Ok(())
}
