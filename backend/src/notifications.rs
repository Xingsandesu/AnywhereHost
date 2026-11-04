//! Pooled HTTPS/HTTP2 transport to the central notification service.
use anyhow::{Context, Result, ensure};
use base64::{Engine, engine::general_purpose::STANDARD as B64};
use bytes::Bytes;
use http_body_util::{BodyExt, Full, Limited};
use hyper::{Request, client::conn::http2::SendRequest};
use hyper_util::rt::{TokioExecutor, TokioIo};
use ring::{
    rand::SystemRandom,
    signature::{self, KeyPair},
};
use rustls::pki_types::{CertificateDer, pem::PemObject};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tokio::{net::TcpStream, sync::Mutex};

pub fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Config {
    pub enabled: bool,
}
pub const ORIGIN: &str = "https://connect.jintongshu.com";
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Environment {
    Sandbox,
    Production,
}
impl Environment {
    pub fn name(self) -> &'static str {
        match self {
            Self::Sandbox => "sandbox",
            Self::Production => "production",
        }
    }
}
#[derive(Debug, PartialEq, Eq)]
pub enum Delivery {
    Accepted,
    InvalidToken,
    Retry { reason: String, delay: i64 },
    Rejected(String),
}
#[derive(Default)]
struct RetryGate {
    failures: u32,
    until: Option<std::time::Instant>,
}
impl RetryGate {
    fn remaining(&self, now: std::time::Instant) -> Option<i64> {
        self.until
            .filter(|until| *until > now)
            .map(|until| until.duration_since(now).as_secs().saturating_add(1) as i64)
    }
    fn failed(&mut self, now: std::time::Instant, delay: i64) {
        let backoff = (5_i64 * 2_i64.pow(self.failures.min(6))).min(300);
        self.failures = self.failures.saturating_add(1);
        let until = now + Duration::from_secs(delay.max(backoff) as u64);
        self.until = Some(self.until.map_or(until, |previous| previous.max(until)));
    }
}
pub struct Client {
    host: String,
    key: signature::EcdsaKeyPair,
    tls: Arc<rustls::ClientConfig>,
    connection: Mutex<Option<SendRequest<Full<Bytes>>>>,
    status: Mutex<Option<(std::time::Instant, Value)>>,
    retry_gate: std::sync::Mutex<RetryGate>,
}
impl Client {
    pub fn load(dir: &std::path::Path) -> Result<Self> {
        let host = endpoint_host(ORIGIN)?;
        use std::os::unix::fs::PermissionsExt;
        let path = dir.join("notification-key-v2.pk8");
        let rng = SystemRandom::new();
        if !path.exists() {
            let key = signature::EcdsaKeyPair::generate_pkcs8(
                &signature::ECDSA_P256_SHA256_FIXED_SIGNING,
                &rng,
            )
            .map_err(|_| anyhow::anyhow!("Notification key generation failed"))?;
            crate::config::private_write(&path, key.as_ref())?;
        }
        let metadata = std::fs::symlink_metadata(&path)?;
        ensure!(
            metadata.is_file()
                && metadata.len() <= 1024
                && metadata.permissions().mode() & 0o077 == 0,
            "Notification key must be a private regular file"
        );
        let key = signature::EcdsaKeyPair::from_pkcs8(
            &signature::ECDSA_P256_SHA256_FIXED_SIGNING,
            &std::fs::read(path)?,
            &rng,
        )
        .map_err(|_| anyhow::anyhow!("Invalid notification key"))?;
        let mut roots = rustls::RootCertStore::empty();
        let trust_store = "/etc/ssl/cert.pem";
        for cert in CertificateDer::pem_file_iter(trust_store)? {
            roots.add(cert?)?;
        }
        ensure!(!roots.is_empty(), "Notification TLS trust store is empty");
        let mut tls = rustls::ClientConfig::builder_with_provider(Arc::new(
            rustls::crypto::aws_lc_rs::default_provider(),
        ))
        .with_safe_default_protocol_versions()?
        .with_root_certificates(roots)
        .with_no_client_auth();
        tls.alpn_protocols = vec![b"h2".to_vec()];
        Ok(Self {
            host,
            key,
            tls: Arc::new(tls),
            connection: Mutex::new(None),
            status: Mutex::new(None),
            retry_gate: std::sync::Mutex::new(RetryGate::default()),
        })
    }
    pub async fn request(&self, path: &str, body: Value) -> Result<(u16, Value)> {
        if let Some(delay) = self
            .retry_gate
            .lock()
            .unwrap()
            .remaining(std::time::Instant::now())
        {
            return Ok((
                503,
                json!({"error":"NotificationServiceBackoff","retryAfter":delay}),
            ));
        }
        match tokio::time::timeout(Duration::from_secs(20), self.send(path, body)).await {
            Ok(Ok((status, value))) => {
                let mut gate = self.retry_gate.lock().unwrap();
                let now = std::time::Instant::now();
                if value["retryScope"] != "subscription"
                    && (status >= 500 || (status == 429 && value["error"] != "DeliveryInProgress"))
                {
                    gate.failed(
                        now,
                        value["retryAfter"].as_i64().unwrap_or(5).clamp(5, 3600),
                    );
                } else if status == 200 && path != "/v2/status" && gate.remaining(now).is_none() {
                    *gate = RetryGate::default();
                }
                Ok((status, value))
            }
            _ => {
                self.retry_gate
                    .lock()
                    .unwrap()
                    .failed(std::time::Instant::now(), 5);
                *self.connection.lock().await = None;
                anyhow::bail!("Notification service unavailable")
            }
        }
    }
    pub async fn status(&self) -> Value {
        let mut cached = self.status.lock().await;
        if let Some((at, value)) = &*cached
            && at.elapsed() < Duration::from_secs(30)
        {
            return value.clone();
        }
        let value = match self.request("/v2/status", json!({})).await {
            Ok((200, value))
                if value["transport"] == "relay-v2" && value["enabled"].is_boolean() =>
            {
                value
            }
            _ => {
                json!({"enabled":false,"environments":[],"transport":"relay-v2","error":"Notification service unavailable or credentials rejected"})
            }
        };
        *cached = Some((std::time::Instant::now(), value.clone()));
        value
    }
    pub fn grant(
        &self,
        binding: String,
        environment: Environment,
        key_id: &str,
        fingerprint: &str,
        registration: &crate::live_activity::Registration,
        token_digest: &str,
    ) -> Result<Value> {
        ensure!(
            B64.decode(key_id)
                .is_ok_and(|b| b.len() == 32 && B64.encode(&b) == key_id),
            "Invalid attested client identity"
        );
        ensure!(
            environment == Environment::Production,
            "Unified notifications require the production client"
        );
        ensure!(
            token_digest.len() == 64
                && token_digest
                    .bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)),
            "Invalid token digest"
        );
        let public = B64.encode(self.key.public_key().as_ref());
        let message = [
            "Anywhere-Relay-Grant-v2",
            ORIGIN,
            &public,
            key_id,
            &binding,
            environment.name(),
            fingerprint,
            &registration.activity_id,
            &registration.token_version.to_string(),
            token_digest,
        ]
        .join("\n");
        let signature = self
            .key
            .sign(&SystemRandom::new(), message.as_bytes())
            .map_err(|_| anyhow::anyhow!("Notification signature failed"))?;
        Ok(
            json!({"url":ORIGIN,"grant":{"hostPublicKey":public,"keyId":key_id,"binding":binding,"environment":environment.name(),"hostId":fingerprint,"signature":B64.encode(signature.as_ref()),"activityId":registration.activity_id,"tokenVersion":registration.token_version,"tokenDigest":token_digest}}),
        )
    }
    pub async fn validate(
        &self,
        token: &str,
        binding: String,
        environment: Environment,
    ) -> Result<()> {
        let (status, value) = self
            .request(
                "/v2/validate",
                json!({"deliveryToken":token,"binding":binding,"environment":environment}),
            )
            .await?;
        ensure!(
            status == 200 && value["valid"] == true,
            "Notification registration rejected ({status})"
        );
        Ok(())
    }
    pub async fn retire(&self, token: &str) -> Delivery {
        match self
            .request("/v2/retire", json!({"deliveryToken":token}))
            .await
        {
            Ok((200, value)) if value["revoked"] == true => Delivery::Accepted,
            Ok((status, value)) => classify(status, &value),
            Err(_) => Delivery::Retry {
                reason: "NotificationTransportUnavailable".into(),
                delay: 5,
            },
        }
    }
    pub async fn deliver(&self, token: &str, payload: Value) -> Delivery {
        match self
            .request(
                "/v2/deliver",
                json!({"deliveryToken":token,"payload":payload}),
            )
            .await
        {
            Ok((status, value)) => classify(status, &value),
            Err(_) => Delivery::Retry {
                reason: "NotificationTransportUnavailable".into(),
                delay: 5,
            },
        }
    }
    async fn send(&self, path: &str, payload: Value) -> Result<(u16, Value)> {
        let body = serde_json::to_vec(&payload)?;
        ensure!(body.len() <= 8192, "Notification service payload too large");

        let mut slot = self.connection.lock().await;
        if slot
            .as_mut()
            .is_some_and(|connection| connection.is_closed())
        {
            *slot = None;
        }
        if slot.is_none() {
            let host = self.host.as_str();
            let tcp = TcpStream::connect((host, 443)).await?;
            let tls = tokio_rustls::TlsConnector::from(self.tls.clone())
                .connect(
                    rustls::pki_types::ServerName::try_from(host.to_owned())?,
                    tcp,
                )
                .await?;
            ensure!(
                tls.get_ref().1.alpn_protocol() == Some(b"h2".as_slice()),
                "Notification service requires HTTP/2"
            );
            let (sender, connection) =
                hyper::client::conn::http2::handshake(TokioExecutor::new(), TokioIo::new(tls))
                    .await?;
            tokio::spawn(async move {
                let _ = connection.await;
            });
            *slot = Some(sender);
        }
        let mut sender = slot
            .as_ref()
            .context("Notification service connection missing")?
            .clone();
        drop(slot);
        let url = format!("https://{}{path}", self.host);
        let digest = format!("sha-256=:{}:", B64.encode(Sha256::digest(&body)));
        let created = now();
        let nonce = uuid::Uuid::new_v4().simple().to_string();
        let key_id = hex::encode(Sha256::digest(self.key.public_key().as_ref()));
        let params = format!(
            "(\"@method\" \"@target-uri\" \"content-digest\");created={created};expires={};nonce=\"{nonce}\";keyid=\"{key_id}\";alg=\"ecdsa-p256-sha256\";tag=\"anywhere-relay-v2\"",
            created + 90
        );
        let base = format!(
            "\"@method\": POST\n\"@target-uri\": {url}\n\"content-digest\": {digest}\n\"@signature-params\": {params}"
        );
        let signature = self
            .key
            .sign(&SystemRandom::new(), base.as_bytes())
            .map_err(|_| anyhow::anyhow!("Notification signature failed"))?;
        let request = Request::post(url)
            .version(hyper::Version::HTTP_2)
            .header("content-digest", digest)
            .header("signature-input", format!("host={params}"))
            .header(
                "signature",
                format!("host=:{}:", B64.encode(signature.as_ref())),
            )
            .header(
                "user-agent",
                concat!("AnywhereHost/", env!("CARGO_PKG_VERSION")),
            )
            .header("content-type", "application/json")
            .body(Full::new(Bytes::from(body)))?;
        sender.ready().await?;
        let response = sender.send_request(request).await?;
        let status = response.status().as_u16();
        let bytes = Limited::new(response.into_body(), 8192)
            .collect()
            .await
            .map_err(|_| anyhow::anyhow!("Notification service response invalid"))?
            .to_bytes();
        Ok((status, serde_json::from_slice(&bytes)?))
    }
}

fn endpoint_host(url: &str) -> Result<String> {
    let host = url
        .strip_prefix("https://")
        .context("Notification URL must use HTTPS")?;
    ensure!(
        host.len() <= 253
            && host.contains('.')
            && host.split('.').all(|label| !label.is_empty()
                && label.len() <= 63
                && !label.starts_with('-')
                && !label.ends_with('-')
                && label
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-'))
            && host.parse::<std::net::IpAddr>().is_err(),
        "Notification URL must be an HTTPS DNS origin without port, credentials, path or query"
    );
    Ok(host.to_owned())
}
fn classify(status: u16, body: &Value) -> Delivery {
    match status {
        200 if body["accepted"] == true => Delivery::Accepted,
        410 => Delivery::InvalidToken,
        401 | 403 | 429 | 500..=599 => Delivery::Retry {
            reason: if status == 401 || status == 403 {
                "NotificationCredentialRejected"
            } else {
                "NotificationServiceUnavailable"
            }
            .into(),
            delay: body["retryAfter"]
                .as_i64()
                .unwrap_or(if status == 401 || status == 403 {
                    900
                } else {
                    5
                })
                .clamp(5, 3600),
        },
        _ => Delivery::Rejected("NotificationRejected".into()),
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn subscription_backoff_does_not_block_other_delivery_or_registration() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        let dir = tempfile::tempdir().unwrap();
        let client = Client::load(dir.path()).unwrap();
        let calls = Arc::new(AtomicUsize::new(0));
        let received = calls.clone();
        let (outgoing, incoming) = tokio::io::duplex(65536);
        let server = tokio::spawn(async move {
            let service = hyper::service::service_fn(
                move |request: Request<hyper::body::Incoming>| {
                    let received = received.clone();
                    async move {
                        received.fetch_add(1, Ordering::SeqCst);
                        let path = request.uri().path().to_owned();
                        let bytes = request.into_body().collect().await.unwrap().to_bytes();
                        let body: Value = serde_json::from_slice(&bytes).unwrap();
                        let (status, value) = match body["deliveryToken"].as_str() {
                            Some("limited-429") => (
                                429,
                                json!({"error":"APNsRateLimited","retryScope":"subscription","retryAfter":60}),
                            ),
                            Some("limited-503") => (
                                503,
                                json!({"error":"APNsUnavailable","retryScope":"subscription","retryAfter":60}),
                            ),
                            Some("busy") => {
                                (429, json!({"error":"DeliveryInProgress","retryAfter":30}))
                            }
                            Some("outage") => {
                                (503, json!({"error":"APNsUnavailable","retryAfter":60}))
                            }
                            _ => (
                                200,
                                match path.as_str() {
                                    "/v2/validate" => json!({"valid":true}),
                                    "/v2/retire" => json!({"revoked":true}),
                                    _ => json!({"accepted":true}),
                                },
                            ),
                        };
                        Ok::<_, std::convert::Infallible>(
                            hyper::Response::builder()
                                .status(status)
                                .body(Full::new(Bytes::from(serde_json::to_vec(&value).unwrap())))
                                .unwrap(),
                        )
                    }
                },
            );
            hyper::server::conn::http2::Builder::new(TokioExecutor::new())
                .serve_connection(TokioIo::new(incoming), service)
                .await
                .unwrap();
        });
        let (sender, connection) =
            hyper::client::conn::http2::handshake(TokioExecutor::new(), TokioIo::new(outgoing))
                .await
                .unwrap();
        let driver = tokio::spawn(connection);
        *client.connection.lock().await = Some(sender);
        for (token, delay) in [("limited-429", 60), ("limited-503", 60), ("busy", 30)] {
            assert!(
                matches!(client.deliver(token, json!({})).await, Delivery::Retry { delay: actual, .. } if actual == delay)
            );
            // Healthy subscriptions and their lifecycle operations must still reach the relay.
            assert_eq!(
                client.deliver("healthy", json!({})).await,
                Delivery::Accepted
            );
            client
                .validate("healthy", "binding".into(), Environment::Production)
                .await
                .unwrap();
            assert_eq!(client.retire("healthy").await, Delivery::Accepted);
        }
        assert_eq!(calls.load(Ordering::SeqCst), 12);
        assert!(matches!(
            client.deliver("outage", json!({})).await,
            Delivery::Retry { delay: 60, .. }
        ));
        assert!(matches!(
            client.deliver("healthy", json!({})).await,
            Delivery::Retry { .. }
        ));
        assert!(
            client
                .validate("healthy", "binding".into(), Environment::Production)
                .await
                .is_err()
        );
        assert!(matches!(
            client.retire("healthy").await,
            Delivery::Retry { .. }
        ));
        assert_eq!(calls.load(Ordering::SeqCst), 13);
        driver.abort();
        server.abort();
    }
    #[test]
    fn shared_service_backoff_bounds_multi_subscription_outages() {
        let mut gate = RetryGate::default();
        let now = std::time::Instant::now();
        assert_eq!(gate.remaining(now), None);
        gate.failed(now, 60);
        assert_eq!(gate.remaining(now), Some(61));
        // A sibling failure cannot shorten a server-requested pause.
        gate.failed(now, 5);
        assert_eq!(gate.remaining(now), Some(61));
        assert_eq!(gate.remaining(now + Duration::from_secs(60)), None);
        for _ in 0..20 {
            gate.failed(now, 5);
        }
        assert_eq!(gate.remaining(now), Some(301));
    }
    #[test]
    fn installation_key_is_private_persistent_and_grant_is_scope_bound() {
        let dir = tempfile::tempdir().unwrap();
        let client = Client::load(dir.path()).unwrap();
        let reloaded = Client::load(dir.path()).unwrap();
        assert_eq!(
            client.key.public_key().as_ref(),
            reloaded.key.public_key().as_ref()
        );
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(dir.path().join("notification-key-v2.pk8"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
        let registration = crate::live_activity::Registration {
            activity_id: "activity".into(),
            delivery_token: String::new(),
            environment: Environment::Production,
            thread_id: "thread".into(),
            turn_id: "turn".into(),
            started_at: 1.0,
            token_version: 1,
        };
        let key_id = B64.encode([1u8; 32]);
        let fingerprint = "a".repeat(64);
        let digest = "b".repeat(64);
        let binding = registration.binding("owner");
        let grant = client
            .grant(
                binding.clone(),
                Environment::Production,
                &key_id,
                &fingerprint,
                &registration,
                &digest,
            )
            .unwrap();
        let g = &grant["grant"];
        let message = [
            "Anywhere-Relay-Grant-v2",
            ORIGIN,
            g["hostPublicKey"].as_str().unwrap(),
            &key_id,
            &binding,
            "production",
            &fingerprint,
            "activity",
            "1",
            &digest,
        ]
        .join("\n");
        let verifier = signature::UnparsedPublicKey::new(
            &signature::ECDSA_P256_SHA256_FIXED,
            client.key.public_key(),
        );
        let signature = B64.decode(g["signature"].as_str().unwrap()).unwrap();
        verifier.verify(message.as_bytes(), &signature).unwrap();
        assert!(
            verifier
                .verify(message.replace("activity", "other").as_bytes(), &signature)
                .is_err()
        );
        assert!(
            client
                .grant(
                    binding,
                    Environment::Sandbox,
                    &key_id,
                    &fingerprint,
                    &registration,
                    &digest
                )
                .is_err()
        );
    }
    #[test]
    fn endpoint_rejects_credential_leaks_and_non_tls() {
        assert!(endpoint_host("https://push.example.com").is_ok());
        for url in [
            "http://push.example.com",
            "https://user@push.example.com",
            "https://push.example.com/",
            "https://push.example.com?x",
            "https://127.0.0.1",
            "https://push.example.com:443",
            "https://push.example.com#x",
        ] {
            assert!(endpoint_host(url).is_err(), "{url}");
        }
    }
    #[test]
    fn success_requires_explicit_provider_acceptance() {
        assert_eq!(
            classify(200, &json!({})),
            Delivery::Rejected("NotificationRejected".into())
        );
        assert_eq!(classify(200, &json!({"accepted":true})), Delivery::Accepted);
        assert_eq!(classify(410, &json!({})), Delivery::InvalidToken);
        assert!(matches!(
            classify(401, &json!({})),
            Delivery::Retry { delay: 900, .. }
        ));
        assert!(matches!(
            classify(503, &json!({"retryAfter":900})),
            Delivery::Retry { delay: 900, .. }
        ));
    }
}
