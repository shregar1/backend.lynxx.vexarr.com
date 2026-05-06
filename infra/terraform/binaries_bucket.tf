# ---------------------------------------------------------------------------
# Binaries distribution bucket — holds the downloadable agent binaries
# (lynxx, lynx-gateway, …) per os/arch. Private; the billing service
# mints short-lived V4 signed URLs on demand via /v1/install/binary.
#
# Layout inside the bucket:
#   <os>/<arch>/<binary>   e.g. linux/amd64/lynxx, darwin/arm64/lynx-gateway
# Uploaded by the CI release workflow (separate from the deploy workflow).
# ---------------------------------------------------------------------------

resource "google_storage_bucket" "binaries" {
  name                        = "${var.project_id}-binaries"
  location                    = var.region
  uniform_bucket_level_access = true
  force_destroy               = false

  versioning {
    enabled = true
  }

  lifecycle_rule {
    condition {
      num_newer_versions = 5
    }
    action {
      type = "Delete"
    }
  }
}

# The billing service signs URLs as itself (service account self-impersonation
# via the IAM Credentials API's signBlob method). roles/iam.serviceAccountTokenCreator
# on the SA's own resource gives it permission to sign with its own
# Google-managed key — no key file required.
resource "google_service_account_iam_member" "gateway_self_token_creator" {
  service_account_id = google_service_account.gateway.name
  role               = "roles/iam.serviceAccountTokenCreator"
  member             = "serviceAccount:${google_service_account.gateway.email}"
}

# Signed URLs include the SA's identity in the X-Goog-Credential header,
# so the SA must also have read access to the bucket (the URL itself
# carries proof of that authorization at request time).
resource "google_storage_bucket_iam_member" "gateway_binaries_reader" {
  bucket = google_storage_bucket.binaries.name
  role   = "roles/storage.objectViewer"
  member = "serviceAccount:${google_service_account.gateway.email}"
}

# CI/CD writes binaries to this bucket on tagged releases.
resource "google_storage_bucket_iam_member" "cicd_binaries_writer" {
  bucket = google_storage_bucket.binaries.name
  role   = "roles/storage.objectAdmin"
  member = "serviceAccount:${google_service_account.cicd.email}"
}

output "binaries_bucket" {
  description = "GCS bucket holding the agent binaries (private; signed-URL access only)."
  value       = google_storage_bucket.binaries.name
}
