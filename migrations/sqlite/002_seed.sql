-- Lynxx seed data — sqlite parity with migrations/postgres/002_seed.sql.
-- Order is significant: it determines the AUTOINCREMENT ids that the
-- NOT NULL FK defaults in 001_ddl.sql refer to (1, 2, …).
--
-- No explicit BEGIN/COMMIT: sqlx::migrate wraps each migration in a
-- transaction. INSERT OR IGNORE makes re-running idempotent.

-- user_type_lk
INSERT OR IGNORE INTO user_type_lk (code, label, description) VALUES
  ('admin',    'Admin',    'Full control over the gateway, including token management.'),
  ('operator', 'Operator', 'Day-to-day operations: services, routes, plugins, recordings.'),
  ('viewer',   'Viewer',   'Read-only access for dashboards and audits.'),
  ('internal', 'Internal', 'Internal company developer; full scope of every subscription tier.');

-- billing_tier_lk
INSERT OR IGNORE INTO billing_tier_lk (code, label, description, monthly_call_limit) VALUES
  ('free',       'Free',       '1M calls/month · self-host · API key included.',                  1000000),
  ('pro',        'Pro',        '10M calls/month · clustering · 99.9% SLA.',                       10000000),
  ('business',   'Business',   'Unlimited calls · SSO · audit log retention · 99.99% SLA.',       NULL),
  ('enterprise', 'Enterprise', 'Custom contract, dedicated support, on-prem options.',            NULL);

-- subscription_status_lk
INSERT OR IGNORE INTO subscription_status_lk (code, label, description) VALUES
  ('active',   'Active',   'Subscription is in good standing.'),
  ('past_due', 'Past Due', 'Most recent invoice failed; in grace period.'),
  ('canceled', 'Canceled', 'Subscription terminated; API keys revoked.');

-- subscription_type_lk
INSERT OR IGNORE INTO subscription_type_lk (code, label, description) VALUES
  ('free',     'Free',     'Free-forever tier; no billing relationship.'),
  ('trial',    'Trial',    '14-day Pro trial; converts to paid on first card charge.'),
  ('paid',     'Paid',     'Stripe-billed subscription with active payment method.'),
  ('comp',     'Comp',     'Complimentary subscription granted by sales / support.'),
  ('internal', 'Internal', 'Issued to internal company developers with unlimited scope.');

-- api_key_status_lk
INSERT OR IGNORE INTO api_key_status_lk (code, label, description) VALUES
  ('active',  'Active',  'Key is live and verifies successfully.'),
  ('revoked', 'Revoked', 'Key was deactivated by the customer or by Stripe webhook.'),
  ('expired', 'Expired', 'Key passed its expires_at timestamp.');

-- lb_strategy_lk
INSERT OR IGNORE INTO lb_strategy_lk (code, label, description) VALUES
  ('round-robin',       'Round Robin',       'Cycle through targets in order.'),
  ('weighted',          'Weighted',          'Distribute proportionally to per-target weights.'),
  ('least-connections', 'Least Connections', 'Pick the target with the fewest in-flight requests.'),
  ('hash',              'Consistent Hash',   'Hash a request attribute (header, IP, etc.) to a target.');
