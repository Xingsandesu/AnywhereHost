use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::BTreeSet,
    path::PathBuf,
    sync::Mutex,
    time::{Duration, Instant},
};
use subtle::ConstantTimeEq;

#[derive(Default, Serialize, Deserialize)]
struct Devices {
    authorized: BTreeSet<String>,
}
struct Ticket {
    secret: String,
    expires: Instant,
}
pub struct Security {
    path: PathBuf,
    devices: Mutex<Devices>,
    ticket: Mutex<Option<Ticket>>,
    pub revocations: tokio::sync::broadcast::Sender<String>,
}
impl Security {
    pub fn open(path: PathBuf) -> Result<Self> {
        let devices = if path.exists() {
            serde_json::from_slice(&std::fs::read(&path)?)?
        } else {
            Devices::default()
        };
        Ok(Self {
            path,
            devices: Mutex::new(devices),
            ticket: Mutex::new(None),
            revocations: tokio::sync::broadcast::channel(64).0,
        })
    }
    pub fn active_owner(&self, id: &str) -> bool {
        self.devices.lock().unwrap().authorized.contains(id)
    }
    pub fn ticket(&self) -> String {
        let secret = hex::encode(anywhere_transport::new_secret());
        *self.ticket.lock().unwrap() = Some(Ticket {
            secret: secret.clone(),
            expires: Instant::now() + Duration::from_secs(120),
        });
        secret
    }
    pub fn pair(&self, id: &str, secret: &str) -> Result<Value> {
        // Repeated enrollment from the same authenticated key survives a lost reply.
        if self.active_owner(id) {
            return Ok(json!({"deviceId":id}));
        }
        let mut ticket = self.ticket.lock().unwrap();
        let expected = ticket.as_ref().context("Pairing expired")?;
        ensure!(expected.expires > Instant::now(), "Pairing expired");
        let supplied = hex::decode(secret).unwrap_or_default();
        let known = hex::decode(&expected.secret)?;
        ensure!(
            bool::from(supplied.ct_eq(&known)),
            "Invalid pairing credential"
        );
        let mut devices = self.devices.lock().unwrap();
        ensure!(devices.authorized.len() < 32, "Device limit reached");
        devices.authorized.insert(id.to_owned());
        if let Err(error) =
            crate::config::private_write(&self.path, &serde_json::to_vec(&*devices)?)
        {
            devices.authorized.remove(id);
            return Err(error);
        }
        *ticket = None;
        Ok(json!({"deviceId":id}))
    }
    pub fn devices(&self) -> Value {
        json!({"devices":self.devices.lock().unwrap().authorized})
    }
    pub fn revoke(&self, id: &str) -> Result<Value> {
        let mut devices = self.devices.lock().unwrap();
        let mut next = Devices {
            authorized: devices.authorized.clone(),
        };
        next.authorized.remove(id);
        crate::config::private_write(&self.path, &serde_json::to_vec(&next)?)?;
        *devices = next;
        let _ = self.revocations.send(id.to_owned());
        Ok(json!({"revoked":true}))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn one_time_expiring_pairing_and_durable_revocation() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("devices.json");
        let security = Security::open(path.clone()).unwrap();
        let stale = security.ticket();
        let fresh = security.ticket();
        assert!(security.pair("device", &stale).is_err());
        assert!(security.pair("device", &fresh).is_ok());
        assert!(security.pair("other", &fresh).is_err());
        assert!(Security::open(path.clone()).unwrap().active_owner("device"));
        security.revoke("device").unwrap();
        assert!(!Security::open(path).unwrap().active_owner("device"));
        assert!(security.pair("device", &fresh).is_err());
        let expired = security.ticket();
        security.ticket.lock().unwrap().as_mut().unwrap().expires =
            Instant::now() - Duration::from_secs(1);
        assert!(security.pair("device", &expired).is_err());
    }
}
