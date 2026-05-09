-- Fix triggers to produce RFC 3339 timestamps instead of SQLite's default
-- "YYYY-MM-DD HH:MM:SS" format from datetime('now').

DROP TRIGGER IF EXISTS trg_user_type_lk_bump_updated_at;
DROP TRIGGER IF EXISTS trg_billing_tier_lk_bump_updated_at;
DROP TRIGGER IF EXISTS trg_subscription_status_lk_bump_updated_at;
DROP TRIGGER IF EXISTS trg_subscription_type_lk_bump_updated_at;
DROP TRIGGER IF EXISTS trg_api_key_status_lk_bump_updated_at;
DROP TRIGGER IF EXISTS trg_lb_strategy_lk_bump_updated_at;
DROP TRIGGER IF EXISTS trg_services_bump_updated_at;
DROP TRIGGER IF EXISTS trg_routes_bump_updated_at;
DROP TRIGGER IF EXISTS trg_plugins_bump_updated_at;
DROP TRIGGER IF EXISTS trg_consumers_bump_updated_at;
DROP TRIGGER IF EXISTS trg_targets_bump_updated_at;
DROP TRIGGER IF EXISTS trg_admin_tokens_bump_updated_at;
DROP TRIGGER IF EXISTS trg_recordings_bump_updated_at;
DROP TRIGGER IF EXISTS trg_customers_bump_updated_at;
DROP TRIGGER IF EXISTS trg_api_keys_bump_updated_at;

CREATE TRIGGER trg_user_type_lk_bump_updated_at
  AFTER UPDATE ON user_type_lk FOR EACH ROW
  BEGIN UPDATE user_type_lk SET updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now') WHERE id = NEW.id; END;
CREATE TRIGGER trg_billing_tier_lk_bump_updated_at
  AFTER UPDATE ON billing_tier_lk FOR EACH ROW
  BEGIN UPDATE billing_tier_lk SET updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now') WHERE id = NEW.id; END;
CREATE TRIGGER trg_subscription_status_lk_bump_updated_at
  AFTER UPDATE ON subscription_status_lk FOR EACH ROW
  BEGIN UPDATE subscription_status_lk SET updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now') WHERE id = NEW.id; END;
CREATE TRIGGER trg_subscription_type_lk_bump_updated_at
  AFTER UPDATE ON subscription_type_lk FOR EACH ROW
  BEGIN UPDATE subscription_type_lk SET updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now') WHERE id = NEW.id; END;
CREATE TRIGGER trg_api_key_status_lk_bump_updated_at
  AFTER UPDATE ON api_key_status_lk FOR EACH ROW
  BEGIN UPDATE api_key_status_lk SET updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now') WHERE id = NEW.id; END;
CREATE TRIGGER trg_lb_strategy_lk_bump_updated_at
  AFTER UPDATE ON lb_strategy_lk FOR EACH ROW
  BEGIN UPDATE lb_strategy_lk SET updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now') WHERE id = NEW.id; END;
CREATE TRIGGER trg_services_bump_updated_at
  AFTER UPDATE ON services FOR EACH ROW
  BEGIN UPDATE services SET updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now') WHERE id = NEW.id; END;
CREATE TRIGGER trg_routes_bump_updated_at
  AFTER UPDATE ON routes FOR EACH ROW
  BEGIN UPDATE routes SET updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now') WHERE id = NEW.id; END;
CREATE TRIGGER trg_plugins_bump_updated_at
  AFTER UPDATE ON plugins FOR EACH ROW
  BEGIN UPDATE plugins SET updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now') WHERE id = NEW.id; END;
CREATE TRIGGER trg_consumers_bump_updated_at
  AFTER UPDATE ON consumers FOR EACH ROW
  BEGIN UPDATE consumers SET updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now') WHERE id = NEW.id; END;
CREATE TRIGGER trg_targets_bump_updated_at
  AFTER UPDATE ON targets FOR EACH ROW
  BEGIN UPDATE targets SET updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now') WHERE id = NEW.id; END;
CREATE TRIGGER trg_admin_tokens_bump_updated_at
  AFTER UPDATE ON admin_tokens FOR EACH ROW
  BEGIN UPDATE admin_tokens SET updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now') WHERE id = NEW.id; END;
CREATE TRIGGER trg_recordings_bump_updated_at
  AFTER UPDATE ON recordings FOR EACH ROW
  BEGIN UPDATE recordings SET updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now') WHERE id = NEW.id; END;
CREATE TRIGGER trg_customers_bump_updated_at
  AFTER UPDATE ON customers FOR EACH ROW
  BEGIN UPDATE customers SET updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now') WHERE id = NEW.id; END;
CREATE TRIGGER trg_api_keys_bump_updated_at
  AFTER UPDATE ON api_keys FOR EACH ROW
  BEGIN UPDATE api_keys SET updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now') WHERE id = NEW.id; END;
