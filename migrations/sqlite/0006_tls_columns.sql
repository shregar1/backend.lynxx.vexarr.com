-- Per-service TLS configuration for outbound (gateway -> upstream) mTLS.
--
-- `tls_verify` defaults to 1 (verify) so existing https upstreams keep their
-- current behaviour. The three path columns are NULL by default; when set,
-- the proxy http_client builds a per-service hyper-util client wired with
-- the corresponding cert/key/CA. See `proxy::http_client::ProxyClientPool`.
ALTER TABLE services ADD COLUMN tls_verify INTEGER NOT NULL DEFAULT 1;
ALTER TABLE services ADD COLUMN tls_client_cert_path TEXT;
ALTER TABLE services ADD COLUMN tls_client_key_path TEXT;
ALTER TABLE services ADD COLUMN tls_ca_path TEXT;
