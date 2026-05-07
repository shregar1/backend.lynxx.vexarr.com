# ---------------------------------------------------------------------------
# Binaries distribution — uses the existing top-level `vexarr-com` bucket
# under a `lynxx/binaries/` prefix (siblings: `lynxx/prod/` for terraform
# state). Layout:
#
#   gs://vexarr-com/lynxx/binaries/<os>/<arch>/<binary>
#
# The bucket itself is not managed here (it pre-exists for shared use
# across Vexarr projects). We declare it via a `data` block so the
# IAM bindings below can target it.
# ---------------------------------------------------------------------------

data "google_storage_bucket" "vexarr_com" {
  name = "vexarr-com"
}

# The billing service signs URLs as itself. roles/iam.serviceAccountTokenCreator
# on the SA's own resource lets it sign with its own Google-managed key.
resource "google_service_account_iam_member" "gateway_self_token_creator" {
  service_account_id = google_service_account.gateway.name
  role               = "roles/iam.serviceAccountTokenCreator"
  member             = "serviceAccount:${google_service_account.gateway.email}"
}

# The signed URL carries the SA's identity; access is checked when the
# URL is dereferenced. Object-level read on the shared bucket is fine —
# the install/binary endpoint allowlists which (binary, os, arch) triples
# can be signed, so the SA can't be coaxed into signing arbitrary objects.
resource "google_storage_bucket_iam_member" "gateway_binaries_reader" {
  bucket = data.google_storage_bucket.vexarr_com.name
  role   = "roles/storage.objectViewer"
  member = "serviceAccount:${google_service_account.gateway.email}"
}

# CI/CD writes binaries to the bucket on tagged releases.
resource "google_storage_bucket_iam_member" "cicd_binaries_writer" {
  bucket = data.google_storage_bucket.vexarr_com.name
  role   = "roles/storage.objectAdmin"
  member = "serviceAccount:${google_service_account.cicd.email}"
}

output "binaries_bucket" {
  description = "Shared GCS bucket holding the agent binaries."
  value       = data.google_storage_bucket.vexarr_com.name
}

output "binaries_prefix" {
  description = "Prefix inside the binaries bucket for this project."
  value       = "lynxx/binaries"
}
