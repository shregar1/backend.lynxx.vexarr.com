# ---------------------------------------------------------------------------
# Service accounts + Workload Identity bindings.
#
# Pattern:
#   GCP SA lynxx-gateway@PROJECT.iam.gserviceaccount.com
#     ← bound to →
#   K8s SA lynxx-gateway in namespace lynxx-system
#
# The gateway pod then authenticates to Cloud SQL via the proxy sidecar
# without any static credentials.
# ---------------------------------------------------------------------------

# ---- Gateway GCP service account ------------------------------------------

resource "google_service_account" "gateway" {
  account_id   = "lynxx-gateway"
  display_name = "Lynxx Gateway (GKE Workload Identity)"
}

# Cloud SQL client — lets the Cloud SQL Auth Proxy connect to the instance.
resource "google_project_iam_member" "gateway_sql_client" {
  project = var.project_id
  role    = "roles/cloudsql.client"
  member  = "serviceAccount:${google_service_account.gateway.email}"
}

# Secret Manager accessor — lets the gateway read its own secrets.
resource "google_project_iam_member" "gateway_secret_accessor" {
  project = var.project_id
  role    = "roles/secretmanager.secretAccessor"
  member  = "serviceAccount:${google_service_account.gateway.email}"
}

# Workload Identity binding — the K8s SA impersonates the GCP SA.
resource "google_service_account_iam_member" "gateway_workload_identity" {
  service_account_id = google_service_account.gateway.name
  role               = "roles/iam.workloadIdentityUser"
  member             = "serviceAccount:${var.project_id}.svc.id.goog[lynxx-system/lynxx-gateway]"
}

# ---- CI/CD GCP service account --------------------------------------------
# Used by GitHub Actions to push images and update the cluster.

resource "google_service_account" "cicd" {
  account_id   = "lynxx-cicd"
  display_name = "Lynxx CI/CD (GitHub Actions)"
}

resource "google_project_iam_member" "cicd_ar_writer" {
  project = var.project_id
  role    = "roles/artifactregistry.writer"
  member  = "serviceAccount:${google_service_account.cicd.email}"
}

resource "google_project_iam_member" "cicd_gke_developer" {
  project = var.project_id
  role    = "roles/container.developer"
  member  = "serviceAccount:${google_service_account.cicd.email}"
}

resource "google_project_iam_member" "cicd_run_developer" {
  project = var.project_id
  role    = "roles/run.developer"
  member  = "serviceAccount:${google_service_account.cicd.email}"
}

resource "google_project_iam_member" "cicd_sa_user" {
  project = var.project_id
  role    = "roles/iam.serviceAccountUser"
  member  = "serviceAccount:${google_service_account.cicd.email}"
}

# Create a key for the CI/CD SA. Store this in your GitHub repo secret
# GCP_SA_KEY as base64(JSON).
resource "google_service_account_key" "cicd" {
  service_account_id = google_service_account.cicd.name
}
