//! Short-lived official-app certificates. TLS/QUIC proves possession of `sub`.
use anyhow::{Result, ensure};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD as B64};
use iroh::{Endpoint, RelayMap, RelayMode, SecretKey, endpoint::presets};
use serde::{Deserialize, Serialize};
use std::time::{SystemTime, UNIX_EPOCH};
pub const ORIGIN: &str = "https://connect.jintongshu.com";
pub const PUBLIC_KEY: &str = "9f138e40da67ee941df2098c48bb5d44b4b2e2a77b91f3277b48fff48677abe2";
pub fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_secs()
}
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Claims {
    pub iss: String,
    pub aud: String,
    pub sub: String,
    pub role: String,
    pub iat: u64,
    pub exp: u64,
    pub jti: String,
    pub relays: Vec<String>,
}
pub fn verify(token: &str, endpoint: &str, role: Option<&str>) -> Result<Claims> {
    verify_key(token, endpoint, role, PUBLIC_KEY.parse()?)
}
fn verify_key(
    token: &str,
    endpoint: &str,
    role: Option<&str>,
    key: iroh::PublicKey,
) -> Result<Claims> {
    ensure!(token.len() <= 4096, "Admission certificate too large");
    let parts: Vec<_> = token.split('.').collect();
    ensure!(parts.len() == 3, "Invalid admission certificate");
    let header: serde_json::Value = serde_json::from_slice(&B64.decode(parts[0])?)?;
    ensure!(
        header == serde_json::json!({"alg":"EdDSA","typ":"JWT","kid":"admission-1"}),
        "Unknown admission signing key"
    );
    let bytes: [u8; 64] = B64
        .decode(parts[2])?
        .try_into()
        .map_err(|_| anyhow::anyhow!("Invalid signature"))?;
    let signature = iroh::Signature::from_bytes(&bytes);
    key.verify(format!("{}.{}", parts[0], parts[1]).as_bytes(), &signature)?;
    let c: Claims = serde_json::from_slice(&B64.decode(parts[1])?)?;
    let at = now();
    ensure!(
        c.iss == ORIGIN
            && c.aud == "anywhere-network-v3"
            && c.sub == endpoint
            && matches!(c.role.as_str(), "host" | "client")
            && role.is_none_or(|r| r == c.role),
        "Admission scope mismatch"
    );
    ensure!(
        c.iat <= at + 30 && c.exp > at && c.exp > c.iat && c.exp - c.iat <= 900,
        "Admission expired"
    );
    ensure!(
        !c.relays.is_empty() && c.relays.len() <= 4,
        "Missing relay discovery"
    );
    for url in &c.relays {
        ensure!(url.starts_with("https://"), "Admission relay requires TLS");
        crate::validate_relay(url)?;
    }
    Ok(c)
}
pub fn relay_map(c: &Claims, token: &str) -> Result<RelayMap> {
    Ok(RelayMap::from_iter(
        c.relays
            .iter()
            .map(|u| crate::validate_relay(u))
            .collect::<Result<Vec<_>>>()?,
    )
    .with_auth_token(token))
}
pub async fn endpoint(secret: SecretKey, token: &str, role: &str) -> Result<Endpoint> {
    let c = verify(token, &secret.public().to_string(), Some(role))?;
    Ok(Endpoint::builder(presets::Minimal)
        .secret_key(secret)
        .alpns(vec![crate::ALPN.to_vec()])
        .relay_mode(RelayMode::Custom(relay_map(&c, token)?))
        .bind()
        .await?)
}
pub async fn install(endpoint: &Endpoint, token: &str, role: &str) -> Result<Claims> {
    let c = verify(token, &endpoint.id().to_string(), Some(role))?;
    let map = relay_map(&c, token)?;
    for url in &c.relays {
        let url = crate::validate_relay(url)?;
        endpoint
            .insert_relay(url.clone(), map.get(&url).expect("relay map"))
            .await;
    }
    Ok(c)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn relay_reconnect_uses_the_renewed_token() {
        use iroh::Watcher;
        use iroh_relay::server::{
            Access, AccessControl, ClientRequest, RelayConfig, Server, ServerConfig,
        };
        use std::sync::{
            Arc,
            atomic::{AtomicBool, Ordering},
        };
        #[derive(Debug, Default)]
        struct Gate {
            renewed: AtomicBool,
            seen_renewal: AtomicBool,
        }
        impl AccessControl for Gate {
            async fn on_connect(&self, request: &ClientRequest) -> Access {
                let renewed = self.renewed.load(Ordering::SeqCst);
                let expected = if renewed { "renewed" } else { "initial" };
                if request.auth_token().as_deref() == Some(expected) {
                    self.seen_renewal.store(renewed, Ordering::SeqCst);
                    Access::Allow
                } else {
                    Access::Deny {
                        reason: Some("stale token".into()),
                    }
                }
            }
        }
        let gate = Arc::new(Gate::default());
        let mut relay = RelayConfig::new("127.0.0.1:0".parse::<std::net::SocketAddr>().unwrap());
        relay.access = gate.clone();
        let mut cfg = ServerConfig::default();
        cfg.relay = Some(relay);
        let server = Server::spawn(cfg).await.unwrap();
        let url: iroh::RelayUrl = format!("http://{}", server.http_addr().unwrap())
            .parse()
            .unwrap();
        let map = RelayMap::from_iter([url.clone()]).with_auth_token("initial");
        let endpoint = Endpoint::builder(presets::Minimal)
            .relay_mode(RelayMode::Custom(map))
            .bind()
            .await
            .unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(5), endpoint.online())
            .await
            .unwrap();
        let next = RelayMap::from_iter([url.clone()]).with_auth_token("renewed");
        endpoint
            .insert_relay(url.clone(), next.get(&url).unwrap())
            .await;
        gate.renewed.store(true, Ordering::SeqCst);
        server
            .relay_service()
            .unwrap()
            .clients()
            .disconnect(endpoint.id(), None);
        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            while !gate.seen_renewal.load(Ordering::SeqCst)
                || !endpoint
                    .home_relay_status()
                    .get()
                    .iter()
                    .any(|s| s.is_connected())
            {
                tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            }
        })
        .await
        .unwrap();
        endpoint.close().await;
        server.shutdown().await.unwrap();
    }
    fn token(key: &SecretKey, c: &Claims) -> String {
        let h = B64.encode(br#"{"alg":"EdDSA","typ":"JWT","kid":"admission-1"}"#);
        let p = B64.encode(serde_json::to_vec(c).unwrap());
        let data = format!("{h}.{p}");
        format!(
            "{data}.{}",
            B64.encode(key.sign(data.as_bytes()).to_bytes())
        )
    }
    #[test]
    fn deployed_signing_key_verifies_its_public_fixture() {
        let key: iroh::PublicKey = PUBLIC_KEY.parse().unwrap();
        let bytes: [u8; 64] = B64.decode("KszBueBRtdnHgWUW_H9S6OgRuoVt2-9sESE83yDz7lDWez5BxB5ehjim5dftduqrRjElZZxolLTrF2aw96pbCQ").unwrap().try_into().unwrap();
        key.verify(
            b"Anywhere admission key self-test v3",
            &iroh::Signature::from_bytes(&bytes),
        )
        .unwrap();
    }
    #[test]
    fn certificate_scope_expiry_tampering_and_key_are_enforced() {
        let key = SecretKey::from_bytes(&crate::new_secret());
        let id = SecretKey::from_bytes(&crate::new_secret())
            .public()
            .to_string();
        let mut c = Claims {
            iss: ORIGIN.into(),
            aud: "anywhere-network-v3".into(),
            sub: id.clone(),
            role: "host".into(),
            iat: now(),
            exp: now() + 900,
            jti: "test".into(),
            relays: vec!["https://relay.example.com/".into()],
        };
        let t = token(&key, &c);
        assert!(verify_key(&t, &id, Some("host"), key.public()).is_ok());
        assert!(verify_key(&t, &id, Some("client"), key.public()).is_err());
        assert!(verify_key(&t, &key.public().to_string(), None, key.public()).is_err());
        assert!(verify(&t, &id, None).is_err());
        let mut parts: Vec<_> = t.split('.').map(str::to_owned).collect();
        parts[1] = B64.encode(b"{}");
        assert!(verify_key(&parts.join("."), &id, None, key.public()).is_err());
        c.exp = now() - 1;
        assert!(verify_key(&token(&key, &c), &id, None, key.public()).is_err());
        c.exp = now() + 901;
        assert!(verify_key(&token(&key, &c), &id, None, key.public()).is_err());
        c.exp = now() + 900;
        c.relays = vec!["http://127.0.0.1:3000".into()];
        assert!(verify_key(&token(&key, &c), &id, None, key.public()).is_err());
    }
    #[tokio::test]
    async fn dormant_host_can_install_a_relay_after_app_attestation() {
        let mut cfg = iroh_relay::server::ServerConfig::default();
        cfg.relay = Some(iroh_relay::server::RelayConfig::new(
            "127.0.0.1:0".parse::<std::net::SocketAddr>().unwrap(),
        ));
        let relay = iroh_relay::server::Server::spawn(cfg).await.unwrap();
        let endpoint = crate::endpoint(SecretKey::from_bytes(&crate::new_secret()), &[])
            .await
            .unwrap();
        let url: iroh::RelayUrl = format!("http://{}", relay.http_addr().unwrap())
            .parse()
            .unwrap();
        let map = RelayMap::from_iter([url.clone()]);
        endpoint
            .insert_relay(url.clone(), map.get(&url).unwrap())
            .await;
        tokio::time::timeout(std::time::Duration::from_secs(5), endpoint.online())
            .await
            .unwrap();
        endpoint.close().await;
        relay.shutdown().await.unwrap();
    }
}
