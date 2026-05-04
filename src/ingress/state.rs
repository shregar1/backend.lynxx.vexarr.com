use dashmap::DashMap;
use std::sync::Arc;
use uuid::Uuid;

pub type Key = (String, String); // (namespace, name)

#[derive(Default)]
pub struct ControllerState {
    pub svc_map: DashMap<Key, Uuid>,
    pub route_map: DashMap<Key, Uuid>,
    pub plugin_map: DashMap<Key, Uuid>,
}

impl ControllerState {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }
}
