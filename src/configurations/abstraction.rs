use crate::abstractions::IConfiguration;
use crate::config::GatewayConfig;

impl IConfiguration for GatewayConfig {
    fn get_string(&self, key: &str) -> Option<String> {
        match key {
            "gateway.proxy_port" => Some(self.gateway.proxy_port.to_string()),
            "gateway.admin_port" => Some(self.gateway.admin_port.to_string()),
            "gateway.database_url" => Some(self.gateway.database_url.clone()),
            "license.api_key" => Some(self.license.api_key.clone()),
            "telemetry.endpoint" => Some(self.telemetry.endpoint.clone()),
            "telemetry.service_name" => Some(self.telemetry.service_name.clone()),
            _ => None,
        }
    }

    fn get_bool(&self, key: &str) -> Option<bool> {
        match key {
            "gateway.auth_required" => Some(self.gateway.auth_required),
            "gateway.seed_from_config" => Some(self.gateway.seed_from_config),
            _ => None,
        }
    }

    fn get_u16(&self, key: &str) -> Option<u16> {
        match key {
            "gateway.proxy_port" => Some(self.gateway.proxy_port),
            "gateway.admin_port" => Some(self.gateway.admin_port),
            _ => None,
        }
    }

    fn get_u64(&self, key: &str) -> Option<u64> {
        match key {
            "gateway.proxy_port" => Some(self.gateway.proxy_port as u64),
            "gateway.admin_port" => Some(self.gateway.admin_port as u64),
            _ => None,
        }
    }
}
