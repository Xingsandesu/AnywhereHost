use iroh::endpoint::Connection;
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

#[derive(Clone, Default)]
pub struct Connections(Arc<Mutex<HashMap<usize, Connection>>>);
impl Connections {
    pub fn track(&self, connection: &Connection) -> Registration {
        let id = connection.stable_id();
        let mut connections = self.0.lock().unwrap();
        // All streams from a device multiplex one authenticated connection.
        // A replacement must retire its predecessor even when a suspended peer
        // could not send a close packet. Dropping its registration is ID-scoped.
        connections.retain(|_, previous| {
            if previous.remote_id() != connection.remote_id() {
                return true;
            }
            previous.close(0u32.into(), b"replaced by device connection");
            false
        });
        connections.insert(id, connection.clone());
        Registration {
            connections: self.clone(),
            id,
        }
    }
    pub fn for_device(&self, device: &str) -> Value {
        let connections = self.0.lock().unwrap();
        json!(
            connections
                .values()
                .filter(|c| c.remote_id().to_string() == device && c.close_reason().is_none())
                .map(anywhere_transport::connection_status)
                .collect::<Vec<_>>()
        )
    }
}
pub struct Registration {
    connections: Connections,
    id: usize,
}
impl Drop for Registration {
    fn drop(&mut self) {
        self.connections.0.lock().unwrap().remove(&self.id);
    }
}
