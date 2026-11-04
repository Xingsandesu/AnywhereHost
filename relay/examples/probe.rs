//! Deployment probe: fresh endpoint only, no Host or user sessions.
use anyhow::{Context, Result, ensure};
use iroh::Watcher;
use std::time::Duration;
#[tokio::main]
async fn main() -> Result<()> {
    let input: serde_json::Value = serde_json::from_slice(&std::fs::read(
        std::env::args().nth(1).context("probe input file")?,
    )?)?;
    let secret: [u8; 32] = serde_json::from_value(input["secret"].clone())?;
    let token = input["token"].as_str().unwrap_or("");
    let url = input["url"].as_str().context("relay URL")?;
    let endpoint = if token.is_empty() {
        anywhere_transport::endpoint(iroh::SecretKey::from_bytes(&secret), &[url.into()]).await?
    } else {
        anywhere_transport::admission::endpoint(
            iroh::SecretKey::from_bytes(&secret),
            token,
            "client",
        )
        .await?
    };
    let expected = input["expected"].as_str().unwrap_or("denied");
    tokio::time::timeout(Duration::from_secs(20), async {
        loop {
            let statuses = endpoint.home_relay_status().get();
            if expected == "allowed" && statuses.iter().any(|s| s.is_connected()) {
                break;
            }
            if expected == "denied" && statuses.iter().any(|s| s.auth_denied_reason().is_some()) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    })
    .await
    .context("relay admission probe timed out")?;
    if let Some(renewed) = input["renewToken"].as_str() {
        let old = anywhere_transport::admission::verify(
            token,
            &endpoint.id().to_string(),
            Some("client"),
        )?;
        anywhere_transport::admission::install(&endpoint, renewed, "client").await?;
        tokio::time::sleep(Duration::from_secs(
            old.exp.saturating_sub(anywhere_transport::admission::now()) + 3,
        ))
        .await;
        tokio::time::timeout(Duration::from_secs(10), endpoint.online())
            .await
            .context("renewed certificate did not reconnect after old expiry")?;
        println!("PASS renewed relay certificate survives original expiry");
    }
    if input["waitForExpiry"].as_bool() == Some(true) {
        let c = anywhere_transport::admission::verify(
            token,
            &endpoint.id().to_string(),
            Some("client"),
        )?;
        tokio::time::sleep(Duration::from_secs(
            c.exp - anywhere_transport::admission::now() + 3,
        ))
        .await;
        ensure!(
            !endpoint
                .home_relay_status()
                .get()
                .iter()
                .any(|s| s.is_connected()),
            "expired relay connection survived"
        );
        println!("PASS relay disconnected the expired certificate");
    }
    endpoint.close().await;
    println!("PASS native relay admission: {expected}");
    Ok(())
}
