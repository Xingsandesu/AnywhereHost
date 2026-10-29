//! Token-based APNs over pooled, certificate-verified HTTP/2 connections.
use anyhow::{Context, Result, ensure};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use bytes::Bytes;
use http_body_util::{BodyExt, Full, Limited};
use hyper::{Request, client::conn::http2::SendRequest};
use hyper_util::rt::{TokioExecutor, TokioIo};
use ring::{rand::SystemRandom, signature::{EcdsaKeyPair, ECDSA_P256_SHA256_FIXED_SIGNING}};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{io::BufReader, path::{Path, PathBuf}, sync::Arc, time::{Duration, SystemTime, UNIX_EPOCH}};
use tokio::{net::TcpStream, sync::Mutex};

pub fn now() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs() as i64
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Config {
    pub team_id: String,
    pub bundle_id: String,
    pub sandbox: Option<KeyConfig>,
    pub production: Option<KeyConfig>,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct KeyConfig {
    pub key_id: String,
    pub private_key_path: PathBuf,
}
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Environment { Sandbox, Production }
impl Environment {
    pub fn name(self) -> &'static str { match self { Self::Sandbox => "sandbox", Self::Production => "production" } }
    fn host(self) -> &'static str { match self { Self::Sandbox => "api.sandbox.push.apple.com", Self::Production => "api.push.apple.com" } }
}

struct Provider {
    environment: Environment,
    key_id: String,
    team_id: String,
    topic: String,
    key: EcdsaKeyPair,
    tls: Arc<rustls::ClientConfig>,
    token: Mutex<Option<(i64, String)>>,
    connection: Mutex<Option<SendRequest<Full<Bytes>>>>,
}
pub struct Client { sandbox: Option<Provider>, production: Option<Provider> }

#[derive(Debug, PartialEq, Eq)]
pub enum Delivery {
    Accepted,
    InvalidToken,
    Retry { reason: String, delay: i64 },
    Rejected(String),
}

impl Client {
    /// A separate file lets credentials be prepared before upgrading a running
    /// Host whose strict config.json decoder does not yet recognize APNs.
    pub fn load_optional(inline: Option<&Config>, state_directory: &Path) -> Result<Option<Self>> {
        if let Some(config) = inline { return Self::load(config).map(Some) }
        let path = state_directory.join("apns.json");
        let bytes = match std::fs::read(path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error.into()),
        };
        ensure!(bytes.len() <= 16_384, "APNs configuration is too large");
        let config: Config = serde_json::from_slice(&bytes)?;
        Self::load(&config).map(Some)
    }
    pub fn load(config: &Config) -> Result<Self> {
        ensure!(identifier(&config.team_id), "APNs teamID must have 10 uppercase letters/digits");
        ensure!(config.bundle_id == "com.jintongshu.AnyWhereCoding", "APNs bundleID must match the iOS application");
        ensure!(config.sandbox.is_some() || config.production.is_some(), "APNs requires at least one environment key");
        let mut roots = rustls::RootCertStore::empty();
        // macOS-maintained public CA bundle. Host's self-signed pairing policy
        // is deliberately never used for Apple's APNs endpoint.
        let mut pem = BufReader::new(std::fs::File::open("/etc/ssl/cert.pem")?);
        for cert in rustls_pemfile::certs(&mut pem) { roots.add(cert?)?; }
        ensure!(!roots.is_empty(), "APNs TLS trust store is empty");
        let mut tls = rustls::ClientConfig::builder().with_root_certificates(roots).with_no_client_auth();
        tls.alpn_protocols = vec![b"h2".to_vec()];
        let tls = Arc::new(tls);
        let make = |key: &KeyConfig, environment| -> Result<Provider> {
            ensure!(identifier(&key.key_id), "APNs keyID must have 10 uppercase letters/digits");
            ensure!(key.private_key_path.is_absolute(), "APNs key path must be absolute");
            use std::os::unix::fs::PermissionsExt;
            let metadata = std::fs::metadata(&key.private_key_path)?;
            ensure!(metadata.len() <= 16_384 && metadata.permissions().mode() & 0o077 == 0,
                    "APNs key must be a small private file (chmod 600)");
            let bytes = std::fs::read(&key.private_key_path)?;
            let private = rustls_pemfile::private_key(&mut bytes.as_slice())?.context("APNs private key missing")?;
            let signing = EcdsaKeyPair::from_pkcs8(&ECDSA_P256_SHA256_FIXED_SIGNING, private.secret_der(), &SystemRandom::new())
                .map_err(|_| anyhow::anyhow!("APNs key must be a P-256 PKCS#8 .p8 signing key"))?;
            Ok(Provider { environment, key_id: key.key_id.clone(), team_id: config.team_id.clone(),
                topic: format!("{}.push-type.liveactivity", config.bundle_id), key: signing, tls: tls.clone(),
                token: Mutex::new(None), connection: Mutex::new(None) })
        };
        Ok(Self { sandbox: config.sandbox.as_ref().map(|k| make(k, Environment::Sandbox)).transpose()?,
                  production: config.production.as_ref().map(|k| make(k, Environment::Production)).transpose()? })
    }
    pub fn environments(&self) -> Vec<&'static str> {
        [self.sandbox.as_ref(), self.production.as_ref()].into_iter().flatten().map(|p| p.environment.name()).collect()
    }
    pub fn supports(&self, environment: Environment) -> bool {
        match environment { Environment::Sandbox => self.sandbox.is_some(), Environment::Production => self.production.is_some() }
    }
    pub async fn send(&self, environment: Environment, token: &str, payload: Value, urgent: bool, expiration: i64) -> Delivery {
        let provider = match environment { Environment::Sandbox => &self.sandbox, Environment::Production => &self.production };
        let Some(provider) = provider else { return Delivery::Rejected("EnvironmentNotConfigured".into()) };
        match tokio::time::timeout(Duration::from_secs(20), provider.send(token, payload, urgent, expiration)).await {
            Ok(Ok(result)) => result,
            _ => { *provider.connection.lock().await = None; Delivery::Retry { reason: "TransportUnavailable".into(), delay: 5 } }
        }
    }
}

fn identifier(value: &str) -> bool { value.len() == 10 && value.bytes().all(|c| c.is_ascii_uppercase() || c.is_ascii_digit()) }

impl Provider {
    async fn jwt(&self) -> Result<String> {
        let mut cached = self.token.lock().await;
        let issued = now();
        if let Some((at, token)) = &*cached && (0..3000).contains(&(issued - at)) { return Ok(token.clone()) }
        let header = URL_SAFE_NO_PAD.encode(serde_json::to_vec(&json!({"alg":"ES256", "kid":self.key_id}))?);
        let claims = URL_SAFE_NO_PAD.encode(serde_json::to_vec(&json!({"iss":self.team_id, "iat":issued}))?);
        let unsigned = format!("{header}.{claims}");
        // APNs/JWS requires fixed-width r||s, not an ASN.1 DER signature.
        let signature = self.key.sign(&SystemRandom::new(), unsigned.as_bytes()).map_err(|_| anyhow::anyhow!("APNs signing failed"))?;
        let token = format!("{unsigned}.{}", URL_SAFE_NO_PAD.encode(signature.as_ref()));
        *cached = Some((issued, token.clone()));
        Ok(token)
    }
    async fn send(&self, token: &str, payload: Value, urgent: bool, expiration: i64) -> Result<Delivery> {
        let body = serde_json::to_vec(&payload)?;
        ensure!(body.len() <= 4096, "APNs payload too large");
        let jwt = self.jwt().await?;
        let mut slot = self.connection.lock().await;
        if slot.as_mut().is_some_and(|connection| connection.is_closed()) { *slot = None; }
        if slot.is_none() {
            let host = self.environment.host();
            let tcp = TcpStream::connect((host, 443)).await?;
            let tls = tokio_rustls::TlsConnector::from(self.tls.clone())
                .connect(rustls::pki_types::ServerName::try_from(host)?, tcp).await?;
            ensure!(tls.get_ref().1.alpn_protocol() == Some(b"h2".as_slice()), "APNs requires HTTP/2");
            let (sender, connection) = hyper::client::conn::http2::handshake(TokioExecutor::new(), TokioIo::new(tls)).await?;
            tokio::spawn(async move { let _ = connection.await; });
            *slot = Some(sender);
        }
        let mut sender = slot.as_ref().context("APNs connection missing")?.clone();
        drop(slot);
        let mut authorization = hyper::header::HeaderValue::from_str(&format!("bearer {jwt}"))?;
        authorization.set_sensitive(true);
        let request = Request::post(format!("https://{}/3/device/{token}", self.environment.host()))
            .version(hyper::Version::HTTP_2)
            .header("authorization", authorization)
            .header("apns-topic", &self.topic).header("apns-push-type", "liveactivity")
            .header("apns-priority", if urgent { "10" } else { "5" })
            .header("apns-expiration", expiration.to_string())
            .header("content-type", "application/json").body(Full::new(Bytes::from(body)))?;
        sender.ready().await?;
        let response = sender.send_request(request).await?;
        let status = response.status().as_u16();
        let retry_after = response.headers().get("retry-after").and_then(|v| v.to_str().ok()).and_then(|v| v.parse::<i64>().ok());
        let bytes = Limited::new(response.into_body(), 4096).collect().await.map_err(|_| anyhow::anyhow!("APNs response invalid"))?.to_bytes();
        Ok(classify(status, &bytes, retry_after))
    }
}

fn classify(status: u16, body: &[u8], retry_after: Option<i64>) -> Delivery {
    if status == 200 { return Delivery::Accepted }
    let value: Value = serde_json::from_slice(body).unwrap_or(Value::Null);
    let reason = value["reason"].as_str().filter(|v| v.len() <= 64 && v.bytes().all(|c| c.is_ascii_alphanumeric()))
        .unwrap_or("UnknownAPNsError").to_owned();
    if status == 410 || matches!(reason.as_str(), "BadDeviceToken" | "DeviceTokenNotForTopic" | "Unregistered") {
        Delivery::InvalidToken
    } else if status == 429 || status >= 500 || status == 403 {
        Delivery::Retry { reason, delay: retry_after.unwrap_or(if status == 403 { 900 } else { 5 }).clamp(5, 3600) }
    } else { Delivery::Rejected(reason) }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test] async fn provider_signs_es256_jws_and_reuses_it_for_fifty_minutes() {
        use ring::signature::{KeyPair, UnparsedPublicKey, ECDSA_P256_SHA256_FIXED};
        let random=SystemRandom::new();
        let pkcs8=EcdsaKeyPair::generate_pkcs8(&ECDSA_P256_SHA256_FIXED_SIGNING,&random).unwrap();
        let key=EcdsaKeyPair::from_pkcs8(&ECDSA_P256_SHA256_FIXED_SIGNING,pkcs8.as_ref(),&random).unwrap();
        let public=key.public_key().as_ref().to_vec();
        let provider=Provider { environment:Environment::Sandbox,key_id:"ABCDEFGHIJ".into(),team_id:"775Y7KV7KU".into(),
            topic:"com.jintongshu.AnyWhereCoding.push-type.liveactivity".into(),key,
            tls:Arc::new(rustls::ClientConfig::builder().with_root_certificates(rustls::RootCertStore::empty()).with_no_client_auth()),
            token:Mutex::new(None),connection:Mutex::new(None) };
        let token=provider.jwt().await.unwrap();
        assert_eq!(token,provider.jwt().await.unwrap());
        let parts:Vec<_>=token.split('.').collect();
        let header:Value=serde_json::from_slice(&URL_SAFE_NO_PAD.decode(parts[0]).unwrap()).unwrap();
        let claims:Value=serde_json::from_slice(&URL_SAFE_NO_PAD.decode(parts[1]).unwrap()).unwrap();
        assert_eq!(header["alg"],"ES256"); assert_eq!(header["kid"],"ABCDEFGHIJ");
        assert_eq!(claims["iss"],"775Y7KV7KU"); assert!(now()-claims["iat"].as_i64().unwrap()<5);
        let signature=URL_SAFE_NO_PAD.decode(parts[2]).unwrap(); assert_eq!(signature.len(),64);
        UnparsedPublicKey::new(&ECDSA_P256_SHA256_FIXED,&public).verify(format!("{}.{}",parts[0],parts[1]).as_bytes(),&signature).unwrap();
    }
    #[test] fn response_policy_distinguishes_tokens_auth_and_transient_failures() {
        assert_eq!(classify(200, b"", None), Delivery::Accepted);
        assert_eq!(classify(410, br#"{"reason":"Unregistered"}"#, None), Delivery::InvalidToken);
        assert_eq!(classify(400, br#"{"reason":"BadDeviceToken"}"#, None), Delivery::InvalidToken);
        assert_eq!(classify(429, br#"{"reason":"TooManyRequests"}"#, Some(120)), Delivery::Retry { reason:"TooManyRequests".into(), delay:120 });
        assert!(matches!(classify(403, br#"{"reason":"InvalidProviderToken"}"#, None), Delivery::Retry { delay:900, .. }));
        assert!(matches!(classify(400, br#"{"reason":"BadPayload"}"#, None), Delivery::Rejected(_)));
    }
}
