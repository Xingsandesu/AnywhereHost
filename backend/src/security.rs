//! Native protocol v2: single-use enrollment and request-bound device proof.
//! No bearer fallback. TLS pinning authenticates the Host; P-256 authenticates devices.
use crate::api::AppState;
use axum::{
    Json,
    body::{Body, to_bytes},
    extract::State,
    http::{Request, StatusCode},
    middleware::Next,
    response::{IntoResponse, Response},
};
use base64::{Engine, engine::general_purpose::STANDARD as B64};
use ring::{hmac, signature};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{Arc, Mutex},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use tokio::sync::Semaphore;

const WINDOW: u64 = 90;
const MAX_NONCES: usize = 32768;
pub const MAX_BODY: usize = 8 * 1024 * 1024;
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Device {
    public_key: String,
}
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Ticket {
    secret_hash: String,
    expires_at: u64,
    #[serde(default)]
    device_id: Option<String>,
}
#[derive(Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Registry {
    devices: HashMap<String, Device>,
    ticket: Option<Ticket>,
}
pub struct Security {
    path: PathBuf,
    registry: Mutex<Registry>,
    nonces: Mutex<HashMap<String, Instant>>,
    pub challenge: String,
    clock: (u64, Instant),
    rate: Mutex<(Instant, f64)>,
    pub requests: Arc<Semaphore>,
    pub streams: Arc<Semaphore>,
}
#[derive(Clone)]
pub struct Principal(pub Option<String>); // None is local management authority.
impl Security {
    pub fn open(path: PathBuf) -> anyhow::Result<Self> {
        let registry = if path.exists() {
            serde_json::from_slice(&std::fs::read(&path)?)?
        } else {
            Registry::default()
        };
        Ok(Self {
            path,
            registry: Mutex::new(registry),
            nonces: Mutex::new(HashMap::new()),
            challenge: random_hex()?,
            clock: (now(), Instant::now()),
            rate: Mutex::new((Instant::now(), 400.0)),
            requests: Arc::new(Semaphore::new(64)),
            streams: Arc::new(Semaphore::new(32)),
        })
    }
    fn timestamp(&self) -> u64 {
        self.clock.0 + self.clock.1.elapsed().as_secs()
    }
    fn admit(&self) -> bool {
        let mut rate = self.rate.lock().unwrap();
        rate.1 = (rate.1 + rate.0.elapsed().as_secs_f64() * 200.0).min(400.0);
        rate.0 = Instant::now();
        if rate.1 < 1.0 {
            return false;
        }
        rate.1 -= 1.0;
        true
    }
    fn persist(&self, registry: &Registry) -> anyhow::Result<()> {
        crate::config::private_write(&self.path, &serde_json::to_vec(registry)?)
    }
    pub fn active(&self, principal: &Principal) -> bool {
        principal
            .0
            .as_ref()
            .is_none_or(|id| self.registry.lock().unwrap().devices.contains_key(id))
    }
    fn consume_nonce(&self, identity: &str, nonce: &str) -> Result<(), StatusCode> {
        let mut used = self.nonces.lock().unwrap();
        used.retain(|_, expiry| *expiry > Instant::now());
        let key = format!("{identity}:{nonce}");
        if used.contains_key(&key) {
            return Err(StatusCode::UNAUTHORIZED);
        }
        if used.len() >= MAX_NONCES {
            return Err(StatusCode::TOO_MANY_REQUESTS);
        }
        // Keep for both sides of the accepted clock window, never evict live entries.
        used.insert(key, Instant::now() + Duration::from_secs(WINDOW * 2 + 5));
        Ok(())
    }
    fn pairing_allowed(&self, ticket: &str, public_key: &str) -> bool {
        if ticket.len() != 64 || !ticket.bytes().all(|b| b.is_ascii_hexdigit()) {
            return false;
        }
        let Ok(key) = B64.decode(public_key) else {
            return false;
        };
        if key.len() != 65 || key[0] != 4 {
            return false;
        }
        let id = hex::encode(Sha256::digest(key));
        self.registry
            .lock()
            .unwrap()
            .ticket
            .as_ref()
            .is_some_and(|entry| {
                entry.expires_at >= now()
                    && entry.secret_hash == hex::encode(Sha256::digest(ticket.as_bytes()))
                    && entry
                        .device_id
                        .as_ref()
                        .is_none_or(|existing| existing == &id)
            })
    }
    fn enroll(&self, ticket: &str, public_key: &str) -> anyhow::Result<String> {
        let bytes = B64.decode(public_key)?;
        anyhow::ensure!(bytes.len() == 65 && bytes[0] == 4, "invalid key");
        let id = hex::encode(Sha256::digest(&bytes));
        let mut registry = self.registry.lock().unwrap();
        let entry = registry
            .ticket
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("pairing unavailable"))?;
        anyhow::ensure!(
            entry.expires_at >= now()
                && entry.secret_hash == hex::encode(Sha256::digest(ticket.as_bytes())),
            "invalid pairing ticket"
        );
        anyhow::ensure!(
            entry
                .device_id
                .as_ref()
                .is_none_or(|existing| existing == &id),
            "ticket consumed"
        );
        anyhow::ensure!(
            registry.devices.contains_key(&id) || registry.devices.len() < 32,
            "device limit"
        );
        // Lost responses may retry with the SAME key only, until ticket expiration.
        let previous = registry.devices.insert(
            id.clone(),
            Device {
                public_key: public_key.to_owned(),
            },
        );
        let old_id = registry
            .ticket
            .as_mut()
            .unwrap()
            .device_id
            .replace(id.clone());
        if let Err(error) = self.persist(&registry) {
            registry.ticket.as_mut().unwrap().device_id = old_id;
            if let Some(old) = previous {
                registry.devices.insert(id.clone(), old);
            } else {
                registry.devices.remove(&id);
            }
            return Err(error);
        }
        Ok(id)
    }
}
pub fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}
fn random_hex() -> anyhow::Result<String> {
    use ring::rand::SecureRandom;
    let mut bytes = [0; 32];
    ring::rand::SystemRandom::new()
        .fill(&mut bytes)
        .map_err(|_| anyhow::anyhow!("random generation failed"))?;
    Ok(hex::encode(bytes))
}
fn error(status: StatusCode, code: &str) -> Response {
    (status, Json(json!({"error":code}))).into_response()
}
fn header<'a>(req: &'a Request<Body>, name: &str) -> Option<&'a str> {
    let mut values = req.headers().get_all(name).iter();
    let value = values.next()?.to_str().ok()?;
    if values.next().is_some() {
        return None;
    }
    Some(value)
}
pub async fn guard(State(s): State<Arc<AppState>>, req: Request<Body>, next: Next) -> Response {
    let mut response = authenticate(s, req, next).await;
    for (name, value) in [
        ("cache-control", "no-store"),
        ("x-content-type-options", "nosniff"),
        ("referrer-policy", "no-referrer"),
    ] {
        response.headers_mut().insert(name, value.parse().unwrap());
    }
    response
}
async fn authenticate(s: Arc<AppState>, req: Request<Body>, next: Next) -> Response {
    if req.headers().contains_key("origin") {
        return error(StatusCode::FORBIDDEN, "browser_origin_not_allowed");
    }
    if req.uri().to_string().len() > 8192 {
        return error(StatusCode::URI_TOO_LONG, "uri_too_long");
    }
    if !s.security.admit() {
        return error(StatusCode::TOO_MANY_REQUESTS, "request_rate");
    }
    let Ok(_permit) = s.security.requests.clone().try_acquire_owned() else {
        return error(StatusCode::TOO_MANY_REQUESTS, "request_limit");
    };
    if req.method() == "GET"
        && req.uri().path() == "/v1/security/challenge"
        && req.uri().query().is_none()
    {
        return Json(
            json!({"challenge":s.security.challenge,"serverTime":s.security.timestamp(),"protocolVersion":2}),
        )
        .into_response();
    }
    let Some(identity) = header(&req, "authorization").map(str::to_owned) else {
        return error(StatusCode::UNAUTHORIZED, "signature_required");
    };
    let is_admin = identity == "Anywhere-Admin";
    let is_pair = identity.starts_with("Anywhere-Pair ");
    let device_id = identity.strip_prefix("Anywhere-Device ");
    let pairing_route = req.uri().path() == "/v1/security/pair" && req.method() == "POST";
    if is_pair != pairing_route {
        return error(StatusCode::FORBIDDEN, "invalid_authority");
    }
    if !is_admin && !is_pair && device_id.is_none() {
        return error(StatusCode::UNAUTHORIZED, "signature_required");
    }
    if !is_admin
        && (req.uri().path().starts_with("/v1/tunnel")
            || req.uri().path().starts_with("/v1/security/devices")
            || req.uri().path() == "/v1/security/ticket")
    {
        return error(StatusCode::FORBIDDEN, "management_authority_required");
    }
    let Some(challenge) = header(&req, "x-anywhere-challenge") else {
        return error(StatusCode::UNAUTHORIZED, "signature_required");
    };
    if challenge != s.security.challenge {
        return error(StatusCode::UNAUTHORIZED, "challenge_expired");
    }
    let Some(timestamp) = header(&req, "x-anywhere-time").map(str::to_owned) else {
        return error(StatusCode::UNAUTHORIZED, "invalid_proof");
    };
    if !timestamp
        .parse::<u64>()
        .is_ok_and(|t| t.abs_diff(s.security.timestamp()) <= WINDOW)
    {
        return error(StatusCode::UNAUTHORIZED, "proof_expired");
    }
    let Some(nonce) = header(&req, "x-anywhere-nonce")
        .filter(|n| n.len() == 32 && n.bytes().all(|b| b.is_ascii_hexdigit()))
        .map(str::to_owned)
    else {
        return error(StatusCode::UNAUTHORIZED, "invalid_proof");
    };
    let Some(proof) = header(&req, "x-anywhere-signature")
        .and_then(|v| B64.decode(v).ok())
        .filter(|v| v.len() <= 80)
    else {
        return error(StatusCode::UNAUTHORIZED, "invalid_proof");
    };
    let public_key = if is_pair {
        header(&req, "x-anywhere-public-key").map(str::to_owned)
    } else {
        device_id.and_then(|id| {
            s.security
                .registry
                .lock()
                .unwrap()
                .devices
                .get(id)
                .map(|d| d.public_key.clone())
        })
    };
    if !is_admin && public_key.is_none() {
        return error(StatusCode::UNAUTHORIZED, "unknown_device");
    }
    // A self-generated key is not enrollment authority. Validate the ticket before
    // allocating replay state; otherwise unauthenticated clients could fill it.
    if is_pair
        && !s.security.pairing_allowed(
            identity.strip_prefix("Anywhere-Pair ").unwrap(),
            public_key.as_ref().unwrap(),
        )
    {
        return error(StatusCode::FORBIDDEN, "pairing_expired_or_consumed");
    }
    let Some(body_digest) = header(&req, "x-anywhere-content-sha256")
        .filter(|v| v.len() == 64 && v.bytes().all(|b| b.is_ascii_hexdigit()))
    else {
        return error(StatusCode::UNAUTHORIZED, "invalid_proof");
    };
    let canonical = format!(
        "Anywhere-v2\n{}\n{}\n{}\n{}\n{}\n{}\n{}\n{}",
        s.fingerprint,
        identity,
        req.method(),
        req.uri()
            .path_and_query()
            .map(|v| v.as_str())
            .unwrap_or("/"),
        body_digest,
        timestamp,
        nonce,
        s.security.challenge
    );
    let valid = if is_admin {
        hmac::verify(
            &hmac::Key::new(hmac::HMAC_SHA256, s.token.as_bytes()),
            canonical.as_bytes(),
            &proof,
        )
        .is_ok()
    } else {
        public_key
            .as_ref()
            .and_then(|v| B64.decode(v).ok())
            .is_some_and(|key| {
                signature::UnparsedPublicKey::new(&signature::ECDSA_P256_SHA256_FIXED, key)
                    .verify(canonical.as_bytes(), &proof)
                    .is_ok()
            })
    };
    if !valid {
        return error(StatusCode::UNAUTHORIZED, "invalid_proof");
    }
    if let Err(status) = s.security.consume_nonce(&identity, &nonce) {
        return error(status, "replayed_or_busy");
    }
    let expected_digest = body_digest.to_owned();
    let (parts, body) = req.into_parts();
    let body = match tokio::time::timeout(Duration::from_secs(15), to_bytes(body, MAX_BODY)).await {
        Ok(Ok(bytes)) => bytes,
        Ok(Err(_)) => return error(StatusCode::PAYLOAD_TOO_LARGE, "body_limit"),
        Err(_) => return error(StatusCode::REQUEST_TIMEOUT, "body_timeout"),
    };
    if hex::encode(Sha256::digest(&body)) != expected_digest {
        return error(StatusCode::UNAUTHORIZED, "body_mismatch");
    }
    if is_pair {
        return match s.security.enroll(
            identity.strip_prefix("Anywhere-Pair ").unwrap(),
            public_key.as_ref().unwrap(),
        ) {
            Ok(id) => Json(json!({"deviceID":id})).into_response(),
            Err(_) => error(StatusCode::FORBIDDEN, "pairing_expired_or_consumed"),
        };
    }
    let principal = Principal(device_id.map(str::to_owned));
    if !s.security.active(&principal) {
        return error(StatusCode::UNAUTHORIZED, "unknown_device");
    }
    let mut req = Request::from_parts(parts, Body::from(body));
    req.extensions_mut().insert(principal);
    // Handler operations retain their existing application timeouts. Do not time out
    // mutations here: cancelling after dispatch can leave their outcome ambiguous.
    next.run(req).await
}
pub async fn ticket(State(s): State<Arc<AppState>>) -> Response {
    let Ok(secret) = random_hex() else {
        return error(StatusCode::INTERNAL_SERVER_ERROR, "random_failed");
    };
    let expires = now() + 300;
    let mut registry = s.security.registry.lock().unwrap();
    let previous = registry.ticket.replace(Ticket {
        secret_hash: hex::encode(Sha256::digest(secret.as_bytes())),
        expires_at: expires,
        device_id: None,
    });
    if s.security.persist(&registry).is_err() {
        registry.ticket = previous;
        return error(StatusCode::INTERNAL_SERVER_ERROR, "storage_failed");
    }
    Json(json!({"ticket":secret,"expiresAt":expires})).into_response()
}
pub async fn devices(State(s): State<Arc<AppState>>) -> Json<serde_json::Value> {
    let registry = s.security.registry.lock().unwrap();
    Json(
        json!({"devices":registry.devices.keys().collect::<Vec<_>>(), "pairingConsumed":registry.ticket.as_ref().is_some_and(|t|t.device_id.is_some())}),
    )
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Revocation {
    #[serde(rename = "deviceID")]
    device_id: String,
}
pub async fn revoke(State(s): State<Arc<AppState>>, Json(v): Json<Revocation>) -> Response {
    let mut registry = s.security.registry.lock().unwrap();
    let old = registry.devices.remove(&v.device_id);
    // A consumed ticket must not be able to resurrect a revoked device.
    let ticket =
        if registry.ticket.as_ref().and_then(|t| t.device_id.as_ref()) == Some(&v.device_id) {
            registry.ticket.take()
        } else {
            None
        };
    if s.security.persist(&registry).is_err() {
        if let Some(old) = old {
            registry.devices.insert(v.device_id, old);
        }
        if ticket.is_some() {
            registry.ticket = ticket;
        }
        return error(StatusCode::INTERNAL_SERVER_ERROR, "storage_failed");
    }
    Json(json!({"revoked":true})).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn replay_cache_fails_closed_without_evicting_live_proofs() {
        let dir = tempfile::tempdir().unwrap();
        let security = Security::open(dir.path().join("devices.json")).unwrap();
        assert!(security.consume_nonce("device", "nonce").is_ok());
        assert_eq!(
            security.consume_nonce("device", "nonce"),
            Err(StatusCode::UNAUTHORIZED)
        );
        {
            let mut cache = security.nonces.lock().unwrap();
            for index in 1..MAX_NONCES {
                cache.insert(index.to_string(), Instant::now() + Duration::from_secs(60));
            }
        }
        assert_eq!(
            security.consume_nonce("device", "new"),
            Err(StatusCode::TOO_MANY_REQUESTS)
        );
        assert_eq!(
            security.consume_nonce("device", "nonce"),
            Err(StatusCode::UNAUTHORIZED)
        );
    }
    #[test]
    fn request_admission_has_a_fixed_burst_budget() {
        let dir = tempfile::tempdir().unwrap();
        let security = Security::open(dir.path().join("devices.json")).unwrap();
        *security.rate.lock().unwrap() = (Instant::now(), 1.0);
        assert!(security.admit());
        assert!(!security.admit());
    }
    #[test]
    fn startup_fails_closed_on_corrupt_registry() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("devices.json");
        std::fs::write(&path, "not json").unwrap();
        assert!(Security::open(path).is_err());
    }
}
