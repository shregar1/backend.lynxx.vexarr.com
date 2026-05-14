pub use crate::abstractions::IUtility;

use uuid::Uuid;

pub struct IdGenerator;

impl IdGenerator {
    pub fn new_uuid() -> Uuid {
        Uuid::new_v4()
    }

    pub fn new_uuid_string() -> String {
        Uuid::new_v4().to_string()
    }
}

impl IUtility for IdGenerator {}

pub struct TimeProvider;

impl TimeProvider {
    pub fn now() -> chrono::DateTime<chrono::Utc> {
        chrono::Utc::now()
    }

    pub fn now_rfc3339() -> String {
        chrono::Utc::now().to_rfc3339()
    }
}

impl IUtility for TimeProvider {}
