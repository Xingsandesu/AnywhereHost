use iroh::endpoint::Connection;
use serde_json::{Value, json};

/// Snapshot of the selected application-data path, never inferred from configured relays.
pub fn connection_status(connection: &Connection) -> Value {
    if let Some(reason) = connection.close_reason() {
        return json!({"connectionId":connection.stable_id().to_string(),"state":"disconnected","mode":"unknown","rttMs":null,"remoteAddress":null,"error":reason.to_string()});
    }
    let paths = connection.paths();
    let selected = paths.iter().find(|path| path.is_selected());
    match selected {
        Some(path) => json!({
            "connectionId":connection.stable_id().to_string(),
            "state":"connected",
            "mode":if path.is_ip() { "direct" } else if path.is_relay() { "relay" } else { "unknown" },
            "rttMs":path.rtt().as_secs_f64() * 1000.0,
            "remoteAddress":path.remote_addr().to_string(),
            "error":null
        }),
        None => {
            json!({"connectionId":connection.stable_id().to_string(),"state":"connected","mode":"unknown","rttMs":null,"remoteAddress":null,"error":null})
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn direct_path_and_closed_snapshot() -> anyhow::Result<()> {
        tokio::time::timeout(std::time::Duration::from_secs(10), async {
            let server =
                crate::endpoint(iroh::SecretKey::from_bytes(&crate::new_secret()), &[]).await?;
            let client =
                crate::endpoint(iroh::SecretKey::from_bytes(&crate::new_secret()), &[]).await?;
            let accept = async { server.accept().await.unwrap().await };
            let (incoming, outgoing) =
                tokio::join!(accept, client.connect(server.addr(), crate::ALPN));
            let incoming = incoming?;
            let outgoing = outgoing?;
            let registration = connection_status(&outgoing);
            assert_eq!(registration["state"], "connected");
            assert_eq!(registration["mode"], "direct");
            assert!(registration["rttMs"].as_f64().is_some());
            outgoing.close(0u32.into(), b"test complete");
            incoming.closed().await;
            let closed = connection_status(&incoming);
            assert_eq!(closed["state"], "disconnected");
            assert_eq!(closed["mode"], "unknown");
            assert!(closed["rttMs"].is_null());
            assert!(closed["remoteAddress"].is_null());
            client.close().await;
            server.close().await;
            Ok::<_, anyhow::Error>(())
        })
        .await??;
        Ok(())
    }
}
