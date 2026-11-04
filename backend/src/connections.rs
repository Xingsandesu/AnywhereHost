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
        self.0.lock().unwrap().insert(id, connection.clone());
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
