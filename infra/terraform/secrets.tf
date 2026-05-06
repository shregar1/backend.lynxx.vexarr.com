# ---------------------------------------------------------------------------
# Secret Manager — gateway database URL and Stripe keys.
# The gateway SA can read these; the CI/CD pipeline writes them at deploy time.
# ---------------------------------------------------------------------------

resource "google_secret_manager_secret" "db_url" {
  secret_id = "lynxx-database-url"

  replication {
    auto {}
  }

  depends_on = [google_project_service.apis]
}

resource "google_secret_manager_secret_version" "db_url" {
  secret = google_secret_manager_secret.db_url.id
  secret_data = "postgres://lynxx:${var.db_password}@127.0.0.1:5432/lynxx"

  # This version is replaced every time db_password changes.
  lifecycle {
    create_before_destroy = true
  }
}

# Stripe keys — set these manually after first apply via:
#   gcloud secrets versions add lynxx-stripe-secret-key --data-file=<(echo -n "sk_live_...")
#   gcloud secrets versions add lynxx-stripe-webhook-secret --data-file=<(echo -n "whsec_...")

resource "google_secret_manager_secret" "stripe_secret_key" {
  secret_id = "lynxx-stripe-secret-key"
  replication { auto {} }
  depends_on = [google_project_service.apis]
}

resource "google_secret_manager_secret" "stripe_webhook_secret" {
  secret_id = "lynxx-stripe-webhook-secret"
  replication { auto {} }
  depends_on = [google_project_service.apis]
}
