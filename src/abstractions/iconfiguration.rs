/// Interface for configuration providers. The gateway's TOML-based
/// `GatewayConfig` implements this to support dynamic config access.
pub trait IConfiguration: Send + Sync + Clone + std::fmt::Debug {
    fn get_string(&self, key: &str) -> Option<String>;
    fn get_bool(&self, key: &str) -> Option<bool>;
    fn get_u16(&self, key: &str) -> Option<u16>;
    fn get_u64(&self, key: &str) -> Option<u64>;
}
