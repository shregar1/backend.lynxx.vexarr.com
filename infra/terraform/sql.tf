# ---------------------------------------------------------------------------
# Cloud SQL — references the shared `vexarr-com` Postgres 16 instance.
#
# All vexarr products (lynxx, fluxx, hostx, …) share a single Cloud SQL
# instance to keep cost down and operations simple. Each product owns:
#   - one database         (`<product>`)
#   - one DB user          (`<product>`)
#   - one Secret Manager   (`<PRODUCT>_DB_PASSWORD`)
#
# This file does NOT create the instance — it's pre-existing under shared
# ownership. Per-product DB + user + password are managed out-of-band:
#
#   gcloud sql users     create lynxx --instance=vexarr-com --password=<...>
#   gcloud sql databases create lynxx --instance=vexarr-com
#   echo -n "<...>" | gcloud secrets create LYNXX_DB_PASSWORD --data-file=-
#
# Cloud Run reaches the instance via the Auth Proxy sidecar (configured
# in cloudrun.tf) using the connection name below — no VPC plumbing
# needed.
# ---------------------------------------------------------------------------

data "google_sql_database_instance" "shared" {
  name = "vexarr-com"
}

# Surface the existing LYNXX_DB_PASSWORD secret to other terraform
# resources (Cloud Run env vars) without managing its content here.
data "google_secret_manager_secret" "lynxx_db_password" {
  secret_id = "LYNXX_DB_PASSWORD"
}
