//! Per-model cost calculator.
//!
//! Costs are configured as `input_per_1k` / `output_per_1k` (USD per 1000
//! tokens, the convention of OpenAI's pricing page). For each request we
//! compute the dollar amount and emit a counter increment with labels
//! `consumer`, `model`, `provider`. PromQL examples in the docs show how to
//! roll these up.

use super::config::ModelCostCfg;
use std::collections::HashMap;
use std::sync::Arc;

#[derive(Clone)]
pub struct CostTable {
    enabled: bool,
    table: Arc<HashMap<String, ModelCostCfg>>,
}

impl CostTable {
    pub fn new(enabled: bool, table: HashMap<String, ModelCostCfg>) -> Self {
        Self {
            enabled,
            table: Arc::new(table),
        }
    }

    pub fn enabled(&self) -> bool {
        self.enabled
    }

    /// Return USD cost for a (model, input_tokens, output_tokens) triple.
    /// Unknown models cost zero — the metric still fires with `model="<id>"`
    /// so operators see they need to add a price entry.
    pub fn dollars(&self, model: &str, input_tokens: u64, output_tokens: u64) -> f64 {
        if !self.enabled {
            return 0.0;
        }
        let Some(p) = self.table.get(model) else {
            return 0.0;
        };
        let input = (input_tokens as f64 / 1000.0) * p.input_per_1k;
        let output = (output_tokens as f64 / 1000.0) * p.output_per_1k;
        input + output
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn computes_dollars_correctly() {
        let mut t = HashMap::new();
        t.insert(
            "gpt-4o".to_string(),
            ModelCostCfg {
                input_per_1k: 0.0025,
                output_per_1k: 0.01,
            },
        );
        let c = CostTable::new(true, t);
        // 1000 input + 500 output = 0.0025 + 0.005 = 0.0075
        let d = c.dollars("gpt-4o", 1000, 500);
        assert!((d - 0.0075).abs() < 1e-9, "got {d}");
    }

    #[test]
    fn unknown_model_costs_zero() {
        let c = CostTable::new(true, HashMap::new());
        assert_eq!(c.dollars("nope", 1000, 1000), 0.0);
    }

    #[test]
    fn disabled_costs_zero() {
        let mut t = HashMap::new();
        t.insert(
            "x".to_string(),
            ModelCostCfg {
                input_per_1k: 1.0,
                output_per_1k: 1.0,
            },
        );
        let c = CostTable::new(false, t);
        assert_eq!(c.dollars("x", 1000, 1000), 0.0);
    }
}
