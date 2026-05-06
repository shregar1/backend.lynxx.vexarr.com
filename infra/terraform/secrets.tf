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
  replication {
    auto {}
  }
  depends_on = [google_project_service.apis]
}

# Empty placeholder so Cloud Run's `versions/latest` reference resolves
# even before you've added the real Stripe key. The binary tolerates an
# empty STRIPE_SECRET_KEY (paid checkout disabled, signups go to free).
# Replace via:
#   gcloud secrets versions add lynxx-stripe-secret-key --data-file=<(echo -n "sk_live_...")
resource "google_secret_manager_secret_version" "stripe_secret_key_placeholder" {
  secret      = google_secret_manager_secret.stripe_secret_key.id
  secret_data = "PLACEHOLDER_NOT_CONFIGURED"
  lifecycle {
    ignore_changes = [secret_data, enabled]
  }
}

resource "google_secret_manager_secret" "stripe_webhook_secret" {
  secret_id = "lynxx-stripe-webhook-secret"
  replication {
    auto {}
  }
  depends_on = [google_project_service.apis]
}

resource "google_secret_manager_secret_version" "stripe_webhook_secret_placeholder" {
  secret      = google_secret_manager_secret.stripe_webhook_secret.id
  secret_data = "PLACEHOLDER_NOT_CONFIGURED"
  lifecycle {
    ignore_changes = [secret_data, enabled]
  }
}

# Session-signing key for the billing service. Auto-populated with 64
# random bytes on first apply; never rotate while sessions are valid.
resource "random_password" "session_secret" {
  length  = 64
  special = false
}

resource "google_secret_manager_secret" "session_secret" {
  secret_id = "lynxx-session-secret"
  replication {
    auto {}
  }
  depends_on = [google_project_service.apis]
}

resource "google_secret_manager_secret_version" "session_secret" {
  secret      = google_secret_manager_secret.session_secret.id
  secret_data = random_password.session_secret.result
}

# Master admin API key for the billing service `/v1/admin/*` endpoints.
# Auto-generated on first apply; rotate via the GCP console (the
# `ignore_changes` keeps Terraform from clobbering manual rotations).
# Required to provision internal-dev customers — there is no public path.
resource "random_password" "admin_api_key" {
  length  = 64
  special = false
}

resource "google_secret_manager_secret" "admin_api_key" {
  secret_id = "lynxx-admin-api-key"
  replication {
    auto {}
  }
  depends_on = [google_project_service.apis]
}

resource "google_secret_manager_secret_version" "admin_api_key" {
  secret      = google_secret_manager_secret.admin_api_key.id
  secret_data = random_password.admin_api_key.result
  lifecycle {
    ignore_changes = [secret_data]
  }
}
